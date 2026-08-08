//! Durable page and block storage for the interactive Knowledge surface.
//!
//! The older `store` module models the complete Knowledge table family for architecture and
//! contract tests. This module is the production PostgreSQL path used by Edge: encrypted page
//! titles and block bodies, transaction-scoped tenant RLS, optimistic versions, and an outbox
//! event committed in the same transaction as every mutation.

use myelin_events::{
    derive_envelope, Actor, AggregateKey, DataRole, EmitContext, EventDraft, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_storage::encryption::EncryptedColumn;
use myelin_storage::kms::PiiKeyRef;
use myelin_tenancy::{Region, TenantId};
use sqlx::postgres::{PgPool, PgRow};
use sqlx::Row;

pub const KNOWLEDGE_PAGE_TABLE: &str = "knowledge_page";
pub const KNOWLEDGE_BLOCK_TABLE: &str = "knowledge_block";
pub const KNOWLEDGE_PAGE_RECENT_INDEX: &str = "knowledge_page_recent";

pub const KNOWLEDGE_PAGE_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS knowledge_page (
    tenant_id       text        NOT NULL,
    region          text        NOT NULL,
    page_id         text        NOT NULL,
    space_key       text        NOT NULL,
    parent_page_id  text,
    title_key_ref   text        NOT NULL,
    title_nonce     bytea       NOT NULL,
    title_ciphertext bytea      NOT NULL,
    owner           text        NOT NULL,
    visibility      text        NOT NULL CHECK (visibility IN ('private', 'team')),
    version         bigint      NOT NULL DEFAULT 1 CHECK (version > 0),
    archived        boolean     NOT NULL DEFAULT false,
    client_nonce    text        NOT NULL,
    created_at      timestamptz NOT NULL DEFAULT now(),
    updated_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, region, page_id),
    UNIQUE (tenant_id, region, client_nonce)
);
CREATE INDEX IF NOT EXISTS knowledge_page_recent
    ON knowledge_page (tenant_id, region, page_id DESC)
    WHERE NOT archived;
"#;

pub const KNOWLEDGE_BLOCK_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS knowledge_block (
    tenant_id        text        NOT NULL,
    region           text        NOT NULL,
    page_id          text        NOT NULL,
    block_id         text        NOT NULL,
    ordinal          integer     NOT NULL CHECK (ordinal >= 0),
    block_type       text        NOT NULL CHECK (block_type IN (
      'paragraph','heading','bullet_list','ordered_list','task_list','blockquote','code_block',
      'callout','divider')),
    inline_key_ref   text        NOT NULL,
    inline_nonce     bytea       NOT NULL,
    inline_ciphertext bytea      NOT NULL,
    created_by       text        NOT NULL,
    edited_by        text        NOT NULL,
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, region, page_id, block_id),
    UNIQUE (tenant_id, region, page_id, ordinal),
    FOREIGN KEY (tenant_id, region, page_id)
      REFERENCES knowledge_page (tenant_id, region, page_id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS knowledge_block_page_order
    ON knowledge_block (tenant_id, region, page_id, ordinal);
"#;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KnowledgeVisibility {
    Private,
    Team,
}

impl KnowledgeVisibility {
    pub fn as_str(self) -> &'static str {
        match self {
            KnowledgeVisibility::Private => "private",
            KnowledgeVisibility::Team => "team",
        }
    }

    pub fn parse(value: &str) -> Option<KnowledgeVisibility> {
        match value {
            "private" => Some(KnowledgeVisibility::Private),
            "team" => Some(KnowledgeVisibility::Team),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgeBlockRecord {
    pub block_id: String,
    pub block_type: String,
    pub inline: EncryptedColumn,
    pub created_by: String,
    pub edited_by: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnowledgePageRecord {
    pub tenant: String,
    pub region: String,
    pub page_id: String,
    pub space_key: String,
    pub parent_page_id: Option<String>,
    pub title: EncryptedColumn,
    pub owner: String,
    pub visibility: KnowledgeVisibility,
    pub version: i64,
    pub created_at_epoch: i64,
    pub updated_at_epoch: i64,
    pub blocks: Vec<KnowledgeBlockRecord>,
}

#[derive(Clone, Debug)]
pub struct NewKnowledgePage {
    pub tenant: String,
    pub region: String,
    pub page_id: String,
    pub space_key: String,
    pub parent_page_id: Option<String>,
    pub title: EncryptedColumn,
    pub owner: String,
    pub visibility: KnowledgeVisibility,
    pub client_nonce: String,
    pub blocks: Vec<KnowledgeBlockRecord>,
}

#[derive(Clone, Debug)]
pub struct SaveKnowledgePage {
    pub tenant: String,
    pub region: String,
    pub page_id: String,
    pub owner: String,
    pub expected_version: i64,
    pub title: EncryptedColumn,
    pub visibility: KnowledgeVisibility,
    pub blocks: Vec<KnowledgeBlockRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KnowledgePageError {
    NotFound,
    Conflict { current_version: i64 },
    Invalid(String),
    Storage(String),
}

impl core::fmt::Display for KnowledgePageError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            KnowledgePageError::NotFound => f.write_str("Knowledge page not found"),
            KnowledgePageError::Conflict { current_version } => write!(
                f,
                "Knowledge page changed while it was being edited (current version {current_version})"
            ),
            KnowledgePageError::Invalid(reason) => write!(f, "invalid Knowledge page: {reason}"),
            KnowledgePageError::Storage(reason) => {
                write!(f, "Knowledge page storage failed: {reason}")
            }
        }
    }
}

impl std::error::Error for KnowledgePageError {}

#[derive(Clone)]
pub struct KnowledgePageStore {
    pool: PgPool,
}

impl KnowledgePageStore {
    pub fn new(pool: PgPool) -> KnowledgePageStore {
        KnowledgePageStore { pool }
    }

    async fn begin_scoped(
        &self,
        tenant: &str,
        region: &str,
    ) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, KnowledgePageError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(storage("begin tenant-scoped Knowledge transaction"))?;
        sqlx::query(
            "SELECT set_config('myelin.tenant_id', $1, true), \
                    set_config('myelin.region', $2, true)",
        )
        .bind(tenant)
        .bind(region)
        .execute(&mut *tx)
        .await
        .map_err(storage("set transaction-scoped Knowledge tenant"))?;
        Ok(tx)
    }

    pub async fn create(
        &self,
        page: &NewKnowledgePage,
        event_id: EventId,
        actor: Actor,
        occurred_at: Timestamp,
    ) -> Result<(String, bool), KnowledgePageError> {
        validate_space_key(&page.space_key)?;
        validate_blocks(&page.blocks)?;
        let mut tx = self.begin_scoped(&page.tenant, &page.region).await?;
        let inserted = sqlx::query(
            "INSERT INTO knowledge_page (
               tenant_id, region, page_id, space_key, parent_page_id,
               title_key_ref, title_nonce, title_ciphertext, owner, visibility, client_nonce
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
             ON CONFLICT (tenant_id, region, client_nonce) DO NOTHING",
        )
        .bind(&page.tenant)
        .bind(&page.region)
        .bind(&page.page_id)
        .bind(&page.space_key)
        .bind(&page.parent_page_id)
        .bind(page.title.key_ref.to_uri())
        .bind(page.title.nonce.to_vec())
        .bind(&page.title.ciphertext)
        .bind(&page.owner)
        .bind(page.visibility.as_str())
        .bind(&page.client_nonce)
        .execute(&mut *tx)
        .await
        .map_err(storage("insert Knowledge page"))?
        .rows_affected();

        if inserted == 0 {
            let page_id = sqlx::query_scalar::<_, String>(
                "SELECT page_id FROM knowledge_page
                  WHERE tenant_id = $1 AND region = $2 AND client_nonce = $3",
            )
            .bind(&page.tenant)
            .bind(&page.region)
            .bind(&page.client_nonce)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage("read idempotent Knowledge create"))?
            .ok_or_else(|| {
                KnowledgePageError::Storage(
                    "create nonce conflicted but its page could not be read".into(),
                )
            })?;
            tx.commit()
                .await
                .map_err(storage("commit idempotent Knowledge create"))?;
            return Ok((page_id, false));
        }

        insert_blocks(&mut tx, &page.tenant, &page.region, &page.page_id, &page.blocks).await?;
        let event = page_event(
            &page.tenant,
            &page.region,
            &page.page_id,
            myelin_content::events::KNOWLEDGE_PAGE_CREATED,
            1,
            event_id,
            actor,
            occurred_at,
        );
        myelin_storage::pgrelay::PgRelay::co_commit_in_tx(&mut tx, &page.page_id, &event)
            .await
            .map_err(|error| KnowledgePageError::Storage(format!("co-commit page create: {error}")))?;
        tx.commit()
            .await
            .map_err(storage("commit Knowledge page create"))?;
        Ok((page.page_id.clone(), true))
    }

    pub async fn list_visible(
        &self,
        tenant: &str,
        region: &str,
        viewer: &str,
        before: Option<&str>,
        limit: u32,
    ) -> Result<Vec<KnowledgePageRecord>, KnowledgePageError> {
        if limit == 0 || limit > 101 {
            return Err(KnowledgePageError::Invalid(
                "page limit must be between 1 and 101".into(),
            ));
        }
        let mut tx = self.begin_scoped(tenant, region).await?;
        let rows = sqlx::query(
            "SELECT tenant_id, region, page_id, space_key, parent_page_id,
                    title_key_ref, title_nonce, title_ciphertext, owner, visibility, version,
                    EXTRACT(EPOCH FROM created_at)::bigint AS created_at_epoch,
                    EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_epoch
               FROM knowledge_page
              WHERE tenant_id = $1 AND region = $2 AND NOT archived
                AND (owner = $3 OR visibility = 'team')
                AND ($4::text IS NULL OR page_id < $4)
              ORDER BY page_id DESC LIMIT $5",
        )
        .bind(tenant)
        .bind(region)
        .bind(viewer)
        .bind(before)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(storage("list visible Knowledge pages"))?;
        let pages = rows.iter().map(row_to_page).collect::<Result<Vec<_>, _>>()?;
        tx.commit()
            .await
            .map_err(storage("commit Knowledge page list"))?;
        Ok(pages)
    }

    pub async fn get_visible(
        &self,
        tenant: &str,
        region: &str,
        viewer: &str,
        page_id: &str,
    ) -> Result<KnowledgePageRecord, KnowledgePageError> {
        let mut tx = self.begin_scoped(tenant, region).await?;
        let row = sqlx::query(
            "SELECT tenant_id, region, page_id, space_key, parent_page_id,
                    title_key_ref, title_nonce, title_ciphertext, owner, visibility, version,
                    EXTRACT(EPOCH FROM created_at)::bigint AS created_at_epoch,
                    EXTRACT(EPOCH FROM updated_at)::bigint AS updated_at_epoch
               FROM knowledge_page
              WHERE tenant_id = $1 AND region = $2 AND page_id = $3 AND NOT archived
                AND (owner = $4 OR visibility = 'team')",
        )
        .bind(tenant)
        .bind(region)
        .bind(page_id)
        .bind(viewer)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage("read visible Knowledge page"))?
        .ok_or(KnowledgePageError::NotFound)?;
        let mut page = row_to_page(&row)?;
        let rows = sqlx::query(
            "SELECT block_id, block_type, inline_key_ref, inline_nonce, inline_ciphertext,
                    created_by, edited_by
               FROM knowledge_block
              WHERE tenant_id = $1 AND region = $2 AND page_id = $3
              ORDER BY ordinal ASC",
        )
        .bind(tenant)
        .bind(region)
        .bind(page_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(storage("read Knowledge page blocks"))?;
        page.blocks = rows
            .iter()
            .map(row_to_block)
            .collect::<Result<Vec<_>, _>>()?;
        tx.commit()
            .await
            .map_err(storage("commit Knowledge page read"))?;
        Ok(page)
    }

    pub async fn save(
        &self,
        page: &SaveKnowledgePage,
        event_id: EventId,
        actor: Actor,
        occurred_at: Timestamp,
    ) -> Result<i64, KnowledgePageError> {
        validate_blocks(&page.blocks)?;
        if page.expected_version < 1 {
            return Err(KnowledgePageError::Invalid(
                "expected version must be positive".into(),
            ));
        }
        let next_version = page.expected_version.checked_add(1).ok_or_else(|| {
            KnowledgePageError::Invalid("page version exhausted its storage range".into())
        })?;
        let mut tx = self.begin_scoped(&page.tenant, &page.region).await?;
        let updated = sqlx::query(
            "UPDATE knowledge_page
                SET title_key_ref = $1, title_nonce = $2, title_ciphertext = $3,
                    visibility = $4, version = $5, updated_at = now()
              WHERE tenant_id = $6 AND region = $7 AND page_id = $8
                AND owner = $9 AND version = $10 AND NOT archived",
        )
        .bind(page.title.key_ref.to_uri())
        .bind(page.title.nonce.to_vec())
        .bind(&page.title.ciphertext)
        .bind(page.visibility.as_str())
        .bind(next_version)
        .bind(&page.tenant)
        .bind(&page.region)
        .bind(&page.page_id)
        .bind(&page.owner)
        .bind(page.expected_version)
        .execute(&mut *tx)
        .await
        .map_err(storage("compare-and-save Knowledge page"))?
        .rows_affected();
        if updated == 0 {
            let current = sqlx::query_as::<_, (i64, String)>(
                "SELECT version, owner FROM knowledge_page
                  WHERE tenant_id = $1 AND region = $2 AND page_id = $3 AND NOT archived",
            )
            .bind(&page.tenant)
            .bind(&page.region)
            .bind(&page.page_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage("read Knowledge save conflict"))?;
            return match current {
                Some((version, owner)) if owner == page.owner => {
                    Err(KnowledgePageError::Conflict { current_version: version })
                }
                _ => Err(KnowledgePageError::NotFound),
            };
        }
        replace_blocks(&mut tx, &page.tenant, &page.region, &page.page_id, &page.blocks).await?;
        let event = page_event(
            &page.tenant,
            &page.region,
            &page.page_id,
            myelin_content::events::KNOWLEDGE_DOC_UPDATED,
            next_version,
            event_id,
            actor,
            occurred_at,
        );
        myelin_storage::pgrelay::PgRelay::co_commit_in_tx(&mut tx, &page.page_id, &event)
            .await
            .map_err(|error| KnowledgePageError::Storage(format!("co-commit page save: {error}")))?;
        tx.commit()
            .await
            .map_err(storage("commit Knowledge page save"))?;
        Ok(next_version)
    }
}

async fn insert_blocks(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant: &str,
    region: &str,
    page_id: &str,
    blocks: &[KnowledgeBlockRecord],
) -> Result<(), KnowledgePageError> {
    for (ordinal, block) in blocks.iter().enumerate() {
        sqlx::query(
            "INSERT INTO knowledge_block (
               tenant_id, region, page_id, block_id, ordinal, block_type,
               inline_key_ref, inline_nonce, inline_ciphertext, created_by, edited_by
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
        )
        .bind(tenant)
        .bind(region)
        .bind(page_id)
        .bind(&block.block_id)
        .bind(i32::try_from(ordinal).map_err(|_| {
            KnowledgePageError::Invalid("too many blocks for one page".into())
        })?)
        .bind(&block.block_type)
        .bind(block.inline.key_ref.to_uri())
        .bind(block.inline.nonce.to_vec())
        .bind(&block.inline.ciphertext)
        .bind(&block.created_by)
        .bind(&block.edited_by)
        .execute(&mut **tx)
        .await
        .map_err(storage("insert Knowledge block"))?;
    }
    Ok(())
}

async fn replace_blocks(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant: &str,
    region: &str,
    page_id: &str,
    blocks: &[KnowledgeBlockRecord],
) -> Result<(), KnowledgePageError> {
    let retained = blocks
        .iter()
        .map(|block| block.block_id.clone())
        .collect::<Vec<_>>();
    sqlx::query(
        "DELETE FROM knowledge_block
          WHERE tenant_id = $1 AND region = $2 AND page_id = $3
            AND NOT (block_id = ANY($4::text[]))",
    )
    .bind(tenant)
    .bind(region)
    .bind(page_id)
    .bind(&retained)
    .execute(&mut **tx)
    .await
    .map_err(storage("remove deleted Knowledge blocks"))?;

    // Move retained rows out of the live ordinal range before upserting the new order. This keeps
    // the page-order UNIQUE constraint useful without making reorder depend on update sequence.
    sqlx::query(
        "UPDATE knowledge_block SET ordinal = ordinal + 1000000
          WHERE tenant_id = $1 AND region = $2 AND page_id = $3",
    )
    .bind(tenant)
    .bind(region)
    .bind(page_id)
    .execute(&mut **tx)
    .await
    .map_err(storage("stage Knowledge block reorder"))?;

    for (ordinal, block) in blocks.iter().enumerate() {
        sqlx::query(
            "INSERT INTO knowledge_block (
               tenant_id, region, page_id, block_id, ordinal, block_type,
               inline_key_ref, inline_nonce, inline_ciphertext, created_by, edited_by
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
             ON CONFLICT (tenant_id, region, page_id, block_id) DO UPDATE SET
               ordinal = EXCLUDED.ordinal,
               block_type = EXCLUDED.block_type,
               inline_key_ref = EXCLUDED.inline_key_ref,
               inline_nonce = EXCLUDED.inline_nonce,
               inline_ciphertext = EXCLUDED.inline_ciphertext,
               edited_by = EXCLUDED.edited_by,
               updated_at = now()",
        )
        .bind(tenant)
        .bind(region)
        .bind(page_id)
        .bind(&block.block_id)
        .bind(i32::try_from(ordinal).map_err(|_| {
            KnowledgePageError::Invalid("too many blocks for one page".into())
        })?)
        .bind(&block.block_type)
        .bind(block.inline.key_ref.to_uri())
        .bind(block.inline.nonce.to_vec())
        .bind(&block.inline.ciphertext)
        .bind(&block.created_by)
        .bind(&block.edited_by)
        .execute(&mut **tx)
        .await
        .map_err(storage("upsert Knowledge block"))?;
    }
    Ok(())
}

fn row_to_page(row: &PgRow) -> Result<KnowledgePageRecord, KnowledgePageError> {
    Ok(KnowledgePageRecord {
        tenant: row.get("tenant_id"),
        region: row.get("region"),
        page_id: row.get("page_id"),
        space_key: row.get("space_key"),
        parent_page_id: row.get("parent_page_id"),
        title: encrypted_from_row(row, "title_key_ref", "title_nonce", "title_ciphertext")?,
        owner: row.get("owner"),
        visibility: KnowledgeVisibility::parse(row.get::<String, _>("visibility").as_str())
            .ok_or_else(|| KnowledgePageError::Storage("stored visibility is invalid".into()))?,
        version: row.get("version"),
        created_at_epoch: row.get("created_at_epoch"),
        updated_at_epoch: row.get("updated_at_epoch"),
        blocks: Vec::new(),
    })
}

fn row_to_block(row: &PgRow) -> Result<KnowledgeBlockRecord, KnowledgePageError> {
    Ok(KnowledgeBlockRecord {
        block_id: row.get("block_id"),
        block_type: row.get("block_type"),
        inline: encrypted_from_row(row, "inline_key_ref", "inline_nonce", "inline_ciphertext")?,
        created_by: row.get("created_by"),
        edited_by: row.get("edited_by"),
    })
}

fn encrypted_from_row(
    row: &PgRow,
    key_column: &str,
    nonce_column: &str,
    ciphertext_column: &str,
) -> Result<EncryptedColumn, KnowledgePageError> {
    let key_ref = PiiKeyRef::parse(row.get::<String, _>(key_column).as_str())
        .ok_or_else(|| KnowledgePageError::Storage("stored encryption key reference is invalid".into()))?;
    let nonce = row.get::<Vec<u8>, _>(nonce_column);
    let nonce = nonce
        .try_into()
        .map_err(|_| KnowledgePageError::Storage("stored encryption nonce is invalid".into()))?;
    Ok(EncryptedColumn {
        key_ref,
        nonce,
        ciphertext: row.get(ciphertext_column),
    })
}

fn validate_space_key(space_key: &str) -> Result<(), KnowledgePageError> {
    if space_key.is_empty()
        || space_key.len() > 64
        || !space_key
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(KnowledgePageError::Invalid(
            "space key must be 1-64 lowercase URL-safe characters".into(),
        ));
    }
    Ok(())
}

fn validate_blocks(blocks: &[KnowledgeBlockRecord]) -> Result<(), KnowledgePageError> {
    if blocks.is_empty() || blocks.len() > 500 {
        return Err(KnowledgePageError::Invalid(
            "a page must contain between 1 and 500 blocks".into(),
        ));
    }
    let mut ids = std::collections::HashSet::with_capacity(blocks.len());
    for block in blocks {
        if block.block_id.len() != 26
            || !block
                .block_id
                .bytes()
                .all(|byte| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&byte))
        {
            return Err(KnowledgePageError::Invalid(
                "block ids must be canonical ULIDs".into(),
            ));
        }
        if !ids.insert(&block.block_id) {
            return Err(KnowledgePageError::Invalid("block ids must be unique".into()));
        }
        if !matches!(
            block.block_type.as_str(),
            "paragraph"
                | "heading"
                | "bullet_list"
                | "ordered_list"
                | "task_list"
                | "blockquote"
                | "code_block"
                | "callout"
                | "divider"
        ) {
            return Err(KnowledgePageError::Invalid(
                "block type is not in the shared content taxonomy".into(),
            ));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn page_event(
    tenant: &str,
    region: &str,
    page_id: &str,
    event_type: &str,
    version: i64,
    event_id: EventId,
    actor: Actor,
    occurred_at: Timestamp,
) -> myelin_events::EventEnvelope {
    let subject = crate::page_ref(&TenantId(tenant.to_string()), page_id);
    derive_envelope(
        EventDraft {
            type_: EventType(event_type.to_string()),
            subject: subject.clone(),
            aggregate: AggregateKey(subject.0.clone()),
            payload: serde_json::json!({ "subject": subject.0, "version": version }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        },
        EmitContext {
            event_id,
            tenant: TenantId(tenant.to_string()),
            region: Region(region.to_string()),
            actor,
            schema_ver: 1,
            occurred_at: occurred_at.clone(),
            recorded_at: occurred_at,
            caused_by: None,
        },
        None,
    )
}

fn storage(context: &'static str) -> impl FnOnce(sqlx::Error) -> KnowledgePageError {
    move |error| KnowledgePageError::Storage(format!("{context}: {error}"))
}

pub fn knowledge_page_migrations() -> myelin_substrate::Migrations {
    use myelin_substrate::{Migration, Migrations};

    let page = Box::leak(
        format!(
            "{KNOWLEDGE_PAGE_DDL}\nSELECT myelin_make_tenant_scoped('{KNOWLEDGE_PAGE_TABLE}');"
        )
        .into_boxed_str(),
    );
    let block = Box::leak(
        format!(
            "{KNOWLEDGE_BLOCK_DDL}\nSELECT myelin_make_tenant_scoped('{KNOWLEDGE_BLOCK_TABLE}');"
        )
        .into_boxed_str(),
    );
    Migrations::of([
        Migration::plain_on("knowledge_web_0001_page", page, KNOWLEDGE_PAGE_TABLE),
        Migration::plain_on("knowledge_web_0002_block", block, KNOWLEDGE_BLOCK_TABLE),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_storage::kms::KeyClass;

    fn encrypted(subject: &str) -> EncryptedColumn {
        EncryptedColumn {
            key_ref: PiiKeyRef::new(
                TenantId("acme".into()),
                1,
                KeyClass::Subject(subject.into()),
            ),
            nonce: [7; myelin_storage::kms::NONCE_LEN],
            ciphertext: vec![1, 2, 3],
        }
    }

    fn block(id: &str) -> KnowledgeBlockRecord {
        KnowledgeBlockRecord {
            block_id: id.into(),
            block_type: "paragraph".into(),
            inline: encrypted("psn:alice"),
            created_by: "psn:alice".into(),
            edited_by: "psn:alice".into(),
        }
    }

    #[test]
    fn production_migrations_are_rls_scoped_and_indexed() {
        let migrations = knowledge_page_migrations();
        assert_eq!(migrations.0.len(), 2);
        assert!(migrations.0[0].ddl.contains(KNOWLEDGE_PAGE_RECENT_INDEX));
        for (migration, table) in migrations
            .0
            .iter()
            .zip([KNOWLEDGE_PAGE_TABLE, KNOWLEDGE_BLOCK_TABLE])
        {
            assert!(migration.ddl.contains(&format!("CREATE TABLE IF NOT EXISTS {table}")));
            assert!(migration
                .ddl
                .contains(&format!("myelin_make_tenant_scoped('{table}')")));
        }
    }

    #[test]
    fn page_shape_rejects_duplicate_blocks_and_unknown_types() {
        let same = block("01J00000000000000000000000");
        assert!(validate_space_key("engineering").is_ok());
        assert!(validate_space_key("Engineering").is_err());
        assert!(validate_blocks(&[same.clone()]).is_ok());
        assert!(validate_blocks(&[same.clone(), same]).is_err());
        let mut unknown = block("01J00000000000000000000001");
        unknown.block_type = "raw_html".into();
        assert!(validate_blocks(&[unknown]).is_err());
    }
}

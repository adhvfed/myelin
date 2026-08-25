use sqlx::postgres::PgPool;
use sqlx::Acquire;
use sqlx::Row;
use std::collections::BTreeSet;

use myelin_events::{
    derive_envelope, Actor, DataRole, EmitContext, EventDraft, EventId, EventType, Timestamp,
    Visibility,
};

use crate::conversation::{Conversation, ConversationError, ConversationKind};
use crate::store::{is_canonical_ulid, ConversationId};

const EXACT_CONVERSATION_BATCH_MAX: usize = 10_000;

pub const CONVERSATION_TABLE: &str = "chat_conversation";
pub const MESSAGE_TABLE: &str = "chat_message";
pub const THREAD_PARTICIPANT_TABLE: &str = "chat_message_thread_participant";
pub const MESSAGE_ERASURE_OPERATION_TABLE: &str = "chat_message_erasure_operation";
pub const CONVERSATION_RECENT_INDEX: &str = "chat_conversation_recent";
pub const CONVERSATION_CLIENT_NONCE_INDEX: &str = "chat_conversation_client_nonce";
pub const CONVERSATION_PROJECT_RECENT_INDEX: &str = "chat_conversation_project_recent";
pub const CONVERSATION_PROJECT_TOPIC_INDEX: &str = "chat_conversation_project_topic_unique";
pub const MESSAGE_IDENTITY_INDEX: &str = "chat_message_identity";
pub const MESSAGE_THREAD_RANGE_INDEX: &str = "chat_message_thread_range";

pub const MESSAGE_ENUM_CONSTRAINTS_EXPAND_DDL: &str = r#"
DO $myelin$
BEGIN
  IF NOT EXISTS (
    SELECT 1 FROM pg_catalog.pg_constraint
     WHERE conrelid = 'chat_message'::regclass
       AND conname = 'chat_message_author_kind_known'
  ) THEN
    ALTER TABLE chat_message
      ADD CONSTRAINT chat_message_author_kind_known
      CHECK (author_kind BETWEEN 0 AND 2) NOT VALID;
  END IF;
  IF NOT EXISTS (
    SELECT 1 FROM pg_catalog.pg_constraint
     WHERE conrelid = 'chat_message'::regclass
       AND conname = 'chat_message_state_known'
  ) THEN
    ALTER TABLE chat_message
      ADD CONSTRAINT chat_message_state_known
      CHECK (state BETWEEN 0 AND 3) NOT VALID;
  END IF;
END
$myelin$;"#;

pub const MESSAGE_ENUM_CONSTRAINTS_VALIDATE_DDL: &str = r#"
ALTER TABLE chat_message VALIDATE CONSTRAINT chat_message_author_kind_known;
ALTER TABLE chat_message VALIDATE CONSTRAINT chat_message_state_known;"#;

pub fn visible_public_conversations_cte() -> String {
    format!(
        "{},
         visible_conversation(conversation_id) AS (
           SELECT conversation.conversation_id
             FROM chat_conversation conversation
             JOIN visible_project project
               ON project.object_id = 'project:' || conversation.parent_project
             JOIN rebac_tuple parent_acl
               ON parent_acl.tenant_id = conversation.tenant_id
              AND parent_acl.region = conversation.region
              AND parent_acl.object_id = 'channel:' || conversation.conversation_id
              AND parent_acl.relation = 'parent_project'
              AND parent_acl.subject = project.object_id || '#view'
              AND (parent_acl.expires_at IS NULL OR parent_acl.expires_at > CURRENT_TIMESTAMP)
             JOIN rebac_tuple member_acl
               ON member_acl.tenant_id = conversation.tenant_id
              AND member_acl.region = conversation.region
              AND member_acl.object_id = 'channel:' || conversation.conversation_id
              AND member_acl.relation = 'member'
              AND member_acl.subject = project.object_id || '#view'
              AND (member_acl.expires_at IS NULL OR member_acl.expires_at > CURRENT_TIMESTAMP)
            WHERE conversation.tenant_id = $1 AND conversation.region = $2
              AND conversation.kind = 'channel_public' AND NOT conversation.archived
              AND conversation.acl_zookie IS NOT NULL
         )",
        myelin_identity_service::VISIBLE_PROJECTS_CTE,
    )
}

const VISIBLE_CONVERSATION_COLUMNS: &str = "
  conversation.tenant_id, conversation.region, conversation.conversation_id,
  conversation.kind, conversation.home_cell, conversation.parent_project,
  conversation.name, conversation.topic, conversation.linked_ref,
  conversation.pinned_canvas, conversation.retention_days, conversation.archived,
  conversation.created_by, conversation.acl_zookie";

fn visible_conversations_query(tail: &str) -> String {
    format!(
        "{}
         SELECT {VISIBLE_CONVERSATION_COLUMNS}
           FROM chat_conversation conversation
          WHERE conversation.tenant_id = $1 AND conversation.region = $2
            AND NOT conversation.archived
            AND conversation.acl_zookie IS NOT NULL
            AND (
              conversation.conversation_id IN (
                SELECT conversation_id FROM visible_conversation
              )
              OR EXISTS (
                SELECT 1
                  FROM rebac_tuple direct_member
                 WHERE direct_member.tenant_id = conversation.tenant_id
                   AND direct_member.region = conversation.region
                   AND direct_member.object_id = 'channel:' || conversation.conversation_id
                   AND direct_member.relation = 'member'
                   AND direct_member.subject = $3
                   AND (direct_member.expires_at IS NULL
                        OR direct_member.expires_at > CURRENT_TIMESTAMP)
              )
            )
            {tail}",
        visible_public_conversations_cte(),
    )
}

fn canonical_conversation_batch(
    conversation_ids: &[String],
) -> Result<Vec<String>, ConversationError> {
    if conversation_ids.len() > EXACT_CONVERSATION_BATCH_MAX {
        return Err(ConversationError::SchemaViolation(format!(
            "at most {EXACT_CONVERSATION_BATCH_MAX} conversations may be read at once"
        )));
    }
    conversation_ids
        .iter()
        .map(|conversation_id| {
            is_canonical_ulid(conversation_id)
                .then_some(conversation_id.clone())
                .ok_or_else(|| {
                    ConversationError::SchemaViolation(
                        "conversation reference id must be a canonical ULID".into(),
                    )
                })
        })
        .collect::<Result<BTreeSet<_>, _>>()
        .map(|ids| ids.into_iter().collect())
}

pub const CONVERSATION_TABLE_DDL: &str = "\
CREATE TABLE IF NOT EXISTS chat_conversation (
    tenant_id       text        NOT NULL,
    region          text        NOT NULL,
    conversation_id text        NOT NULL,
    kind            text        NOT NULL CHECK (kind IN (
      'channel_public','channel_private','dm','group_dm','artifact_linked','announcement')),
    home_cell       text        NOT NULL,
    parent_project  text,
    name            text        NOT NULL CHECK (octet_length(name) BETWEEN 1 AND 255),
    topic           text        NOT NULL CHECK (octet_length(topic) BETWEEN 1 AND 255),
    linked_ref      text,
    pinned_canvas   text,
    retention_days  int,
    archived        boolean     NOT NULL DEFAULT false,
    created_by      text        NOT NULL,
    acl_zookie      text,
    created_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, region, conversation_id),
    UNIQUE (tenant_id, region, name, topic)
);
CREATE INDEX IF NOT EXISTS chat_conversation_recent
    ON chat_conversation (tenant_id, region, conversation_id DESC)
    WHERE NOT archived;";

#[derive(Clone)]
pub struct PgConversationStore {
    pool: PgPool,
}

impl PgConversationStore {
    pub fn new(pool: PgPool) -> PgConversationStore {
        PgConversationStore { pool }
    }

    async fn set_session_scope(
        &self,
        conn: &mut sqlx::pool::PoolConnection<sqlx::Postgres>,
        tenant: &str,
        region: &str,
    ) -> Result<(), ConversationError> {
        sqlx::query("SELECT set_config('myelin.tenant_id', $1, false), set_config('myelin.region', $2, false)")
            .bind(tenant)
            .bind(region)
            .execute(&mut **conn)
            .await
            .map_err(storage("set conversation session scope"))?;
        Ok(())
    }

    pub async fn create(&self, conversation: &Conversation) -> Result<(), ConversationError> {
        validate_durable_channel(conversation)?;
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(storage("acquire conversation connection"))?;
        self.set_session_scope(&mut conn, &conversation.id.tenant, &conversation.id.region)
            .await?;
        let mut tx = conn
            .begin()
            .await
            .map_err(storage("begin conversation create"))?;
        if insert_conversation(&mut tx, conversation, None).await? == 0 {
            return Err(ConversationError::AlreadyExists(
                conversation.id.conversation_id.clone(),
            ));
        }
        tx.commit()
            .await
            .map_err(storage("commit conversation create"))?;
        Ok(())
    }

    pub async fn create_co_commit(
        &self,
        conversation: &Conversation,
        client_nonce: &str,
        event_id: EventId,
        actor: Actor,
        occurred: Timestamp,
        recorded: Timestamp,
    ) -> Result<(), ConversationError> {
        validate_durable_channel(conversation)?;
        let subject =
            crate::subs::mint_channel(&conversation.id.tenant, &conversation.id.conversation_id)
                .map_err(|error| {
                    ConversationError::SchemaViolation(format!("mint channel ref: {error}"))
                })?;
        let envelope = derive_envelope(
            EventDraft {
                type_: EventType(crate::events::CHAT_CHANNEL_CREATED.into()),
                subject,
                aggregate: crate::events::channel_aggregate(&conversation.id.conversation_id),
                payload: serde_json::json!({
                    "conversation_id": conversation.id.conversation_id,
                    "parent_project": conversation.parent_project,
                    "channel": conversation.name,
                    "topic": conversation.topic,
                    "kind": conversation.kind.as_token(),
                }),
                data_role: DataRole::Controller,
                visibility: Visibility::Internal,
                contains_personal_data: false,
                pii_key_ref: None,
            },
            EmitContext {
                event_id,
                tenant: myelin_tenancy::TenantId(conversation.id.tenant.clone()),
                region: myelin_tenancy::Region(conversation.id.region.clone()),
                actor,
                schema_ver: 1,
                occurred_at: occurred,
                recorded_at: recorded,
                caused_by: None,
            },
            None,
        );

        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(storage("acquire conversation connection"))?;
        self.set_session_scope(&mut conn, &conversation.id.tenant, &conversation.id.region)
            .await?;
        let mut tx = conn
            .begin()
            .await
            .map_err(storage("begin conversation co-commit"))?;
        let result = insert_conversation(&mut tx, conversation, Some(client_nonce)).await?;
        if result == 0 {
            return Err(ConversationError::AlreadyExists(
                conversation.id.conversation_id.clone(),
            ));
        }
        myelin_storage::pgrelay::PgRelay::co_commit_in_tx(
            &mut tx,
            &envelope.aggregate.0,
            &envelope,
        )
        .await
        .map_err(|error| {
            ConversationError::Storage(format!("co-commit conversation event: {error}"))
        })?;
        tx.commit().await.map_err(storage("commit conversation"))?;
        Ok(())
    }

    pub async fn get(&self, id: &ConversationId) -> Result<Conversation, ConversationError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(storage("acquire conversation connection"))?;
        self.set_session_scope(&mut conn, &id.tenant, &id.region)
            .await?;
        let row = sqlx::query(
            "SELECT tenant_id, region, conversation_id, kind, home_cell, parent_project, name,
                    topic, linked_ref, pinned_canvas, retention_days, archived, created_by, acl_zookie
               FROM chat_conversation
              WHERE tenant_id = $1 AND region = $2 AND conversation_id = $3",
        )
        .bind(&id.tenant)
        .bind(&id.region)
        .bind(&id.conversation_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(storage("read conversation"))?
        .ok_or_else(|| ConversationError::NotFound(id.conversation_id.clone()))?;
        row_to_conversation(&row)
    }

    pub async fn stamp_acl_zookie(
        &self,
        id: &ConversationId,
        zookie: &str,
    ) -> Result<(), ConversationError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(storage("acquire conversation authorization connection"))?;
        self.set_session_scope(&mut conn, &id.tenant, &id.region)
            .await?;
        let result = sqlx::query(
            "UPDATE chat_conversation SET acl_zookie = $4
              WHERE tenant_id = $1 AND region = $2 AND conversation_id = $3",
        )
        .bind(&id.tenant)
        .bind(&id.region)
        .bind(&id.conversation_id)
        .bind(zookie)
        .execute(&mut *conn)
        .await
        .map_err(storage("stamp conversation authorization watermark"))?;
        if result.rows_affected() == 0 {
            return Err(ConversationError::NotFound(id.conversation_id.clone()));
        }
        Ok(())
    }

    pub async fn find_public_by_name_topic(
        &self,
        tenant: &str,
        region: &str,
        parent_project: &str,
        name: &str,
        topic: &str,
    ) -> Result<Option<Conversation>, ConversationError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(storage("acquire conversation connection"))?;
        self.set_session_scope(&mut conn, tenant, region).await?;
        let row = sqlx::query(
            "SELECT tenant_id, region, conversation_id, kind, home_cell, parent_project, name,
                    topic, linked_ref, pinned_canvas, retention_days, archived, created_by, acl_zookie
               FROM chat_conversation
              WHERE tenant_id = $1 AND region = $2 AND kind = 'channel_public' AND NOT archived
                AND parent_project = $3 AND name = $4 AND topic = $5",
        )
        .bind(tenant)
        .bind(region)
        .bind(parent_project)
        .bind(name)
        .bind(topic)
        .fetch_optional(&mut *conn)
        .await
        .map_err(storage("find public conversation by name and topic"))?;
        row.as_ref().map(row_to_conversation).transpose()
    }

    pub async fn find_by_client_nonce(
        &self,
        tenant: &str,
        region: &str,
        client_nonce: &str,
    ) -> Result<Option<Conversation>, ConversationError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(storage("acquire conversation connection"))?;
        self.set_session_scope(&mut conn, tenant, region).await?;
        let row = sqlx::query(
            "SELECT tenant_id, region, conversation_id, kind, home_cell, parent_project, name,
                    topic, linked_ref, pinned_canvas, retention_days, archived, created_by, acl_zookie
               FROM chat_conversation
              WHERE tenant_id = $1 AND region = $2 AND client_nonce = $3",
        )
        .bind(tenant)
        .bind(region)
        .bind(client_nonce)
        .fetch_optional(&mut *conn)
        .await
        .map_err(storage("find conversation by client nonce"))?;
        row.as_ref().map(row_to_conversation).transpose()
    }

    pub async fn list_visible(
        &self,
        tenant: &str,
        region: &str,
        subject: &str,
        before: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Conversation>, ConversationError> {
        if limit == 0 || limit > 101 {
            return Err(ConversationError::SchemaViolation(
                "conversation storage page limit must be between 1 and 101".into(),
            ));
        }
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(storage("acquire conversation connection"))?;
        self.set_session_scope(&mut conn, tenant, region).await?;
        let query = visible_conversations_query(
            "AND ($4::text IS NULL OR conversation.conversation_id < $4)
             ORDER BY conversation.conversation_id DESC LIMIT $5",
        );
        let rows = sqlx::query(&query)
            .bind(tenant)
            .bind(region)
            .bind(subject)
            .bind(before)
            .bind(i64::from(limit))
            .fetch_all(&mut *conn)
            .await
            .map_err(storage("list conversations"))?;
        rows.iter().map(row_to_conversation).collect()
    }

    /// Resolve an exact bounded set through the same live membership predicate as the list.
    pub async fn get_visible_exact(
        &self,
        tenant: &str,
        region: &str,
        subject: &str,
        conversation_ids: &[String],
    ) -> Result<Vec<Conversation>, ConversationError> {
        let conversation_ids = canonical_conversation_batch(conversation_ids)?;
        if conversation_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(storage("acquire exact conversation connection"))?;
        self.set_session_scope(&mut conn, tenant, region).await?;
        let query = visible_conversations_query(
            "AND conversation.conversation_id = ANY($4::text[])
             ORDER BY conversation.conversation_id ASC",
        );
        let rows = sqlx::query(&query)
            .bind(tenant)
            .bind(region)
            .bind(subject)
            .bind(conversation_ids)
            .fetch_all(&mut *conn)
            .await
            .map_err(storage("read exact visible conversations"))?;
        rows.iter().map(row_to_conversation).collect()
    }
}

async fn insert_conversation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    conversation: &Conversation,
    client_nonce: Option<&str>,
) -> Result<u64, ConversationError> {
    let result = sqlx::query(
        "INSERT INTO chat_conversation (
           tenant_id, region, conversation_id, kind, home_cell, parent_project, name, topic,
           linked_ref, pinned_canvas, retention_days, archived, created_by, acl_zookie, client_nonce
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
         ON CONFLICT DO NOTHING",
    )
    .bind(&conversation.id.tenant)
    .bind(&conversation.id.region)
    .bind(&conversation.id.conversation_id)
    .bind(conversation.kind.as_token())
    .bind(&conversation.home_cell)
    .bind(&conversation.parent_project)
    .bind(&conversation.name)
    .bind(&conversation.topic)
    .bind(&conversation.linked_ref)
    .bind(&conversation.pinned_canvas)
    .bind(conversation.retention_days)
    .bind(conversation.archived)
    .bind(&conversation.created_by)
    .bind(&conversation.acl_zookie)
    .bind(client_nonce)
    .execute(&mut **tx)
    .await
    .map_err(storage("insert conversation"))?;
    Ok(result.rows_affected())
}

fn validate_durable_channel(conversation: &Conversation) -> Result<(), ConversationError> {
    if !matches!(
        conversation.kind,
        ConversationKind::ChannelPublic | ConversationKind::ChannelPrivate
    ) {
        return Err(ConversationError::SchemaViolation(
            "the durable channel store accepts only public or private channel conversations".into(),
        ));
    }
    if conversation.kind == ConversationKind::ChannelPublic && conversation.parent_project.is_none()
    {
        return Err(ConversationError::SchemaViolation(
            "a public topic requires a parent project".into(),
        ));
    }
    if let Some(project_id) = conversation.parent_project.as_deref() {
        let parsed_project = sqlx::types::Uuid::parse_str(project_id).map_err(|_| {
            ConversationError::SchemaViolation(
                "a channel parent project must be a canonical UUID".into(),
            )
        })?;
        if parsed_project.to_string() != project_id {
            return Err(ConversationError::SchemaViolation(
                "a channel parent project must be a canonical UUID".into(),
            ));
        }
    }
    for (field, value) in [
        ("channel name", conversation.name.as_deref()),
        ("topic", conversation.topic.as_deref()),
    ] {
        let Some(value) = value else {
            return Err(ConversationError::SchemaViolation(format!(
                "{field} is required"
            )));
        };
        if value.trim() != value
            || value.is_empty()
            || value.len() > 255
            || value.chars().any(char::is_control)
        {
            return Err(ConversationError::SchemaViolation(format!(
                "{field} must be 1-255 clean UTF-8 bytes without surrounding whitespace"
            )));
        }
    }
    Ok(())
}

fn row_to_conversation(row: &sqlx::postgres::PgRow) -> Result<Conversation, ConversationError> {
    let kind = row.get::<String, _>("kind");
    let kind = ConversationKind::from_token(&kind)
        .ok_or_else(|| ConversationError::Storage("stored conversation kind is invalid".into()))?;
    Ok(Conversation {
        id: ConversationId::new(
            row.get::<String, _>("tenant_id"),
            row.get::<String, _>("region"),
            row.get::<String, _>("conversation_id"),
        ),
        kind,
        home_cell: row.get("home_cell"),
        parent_project: row.get("parent_project"),
        name: row.get("name"),
        topic: row.get("topic"),
        linked_ref: row.get("linked_ref"),
        pinned_canvas: row.get("pinned_canvas"),
        retention_days: row.get("retention_days"),
        archived: row.get("archived"),
        created_by: row.get("created_by"),
        acl_zookie: row.get("acl_zookie"),
    })
}

fn storage(context: &'static str) -> impl FnOnce(sqlx::Error) -> ConversationError {
    move |error| ConversationError::Storage(format!("{context}: {error}"))
}

pub fn chat_migrations() -> myelin_substrate::Migrations {
    use myelin_substrate::{Migration, Migrations};

    let conversation = format!(
        "{CONVERSATION_TABLE_DDL}\nSELECT myelin_make_tenant_scoped('{CONVERSATION_TABLE}');"
    );
    let message_ddl = super::pg::MESSAGE_TABLE_DDL.replace("{table}", MESSAGE_TABLE);
    let message = format!("{message_ddl}\nSELECT myelin_make_tenant_scoped('{MESSAGE_TABLE}');");
    let conversation_client_nonce = format!(
        "ALTER TABLE {CONVERSATION_TABLE} ADD COLUMN IF NOT EXISTS client_nonce text;
             CREATE UNIQUE INDEX IF NOT EXISTS {CONVERSATION_CLIENT_NONCE_INDEX}
               ON {CONVERSATION_TABLE} (tenant_id, region, client_nonce)
               WHERE client_nonce IS NOT NULL;"
    );
    let conversation_project_recent = format!(
        "CREATE INDEX IF NOT EXISTS {CONVERSATION_PROJECT_RECENT_INDEX}
               ON {CONVERSATION_TABLE}
                 (tenant_id, region, parent_project, conversation_id DESC)
               WHERE kind = 'channel_public' AND NOT archived AND acl_zookie IS NOT NULL;"
    );
    let conversation_project_topic = format!(
        "ALTER TABLE {CONVERSATION_TABLE}
               DROP CONSTRAINT IF EXISTS chat_conversation_tenant_id_region_name_topic_key;
             CREATE UNIQUE INDEX IF NOT EXISTS {CONVERSATION_PROJECT_TOPIC_INDEX}
               ON {CONVERSATION_TABLE}
                 (tenant_id, region, parent_project, name, topic)
               WHERE parent_project IS NOT NULL;"
    );
    let public_conversation_project_topic = format!(
        "DROP INDEX IF EXISTS {CONVERSATION_PROJECT_TOPIC_INDEX};
             CREATE UNIQUE INDEX {CONVERSATION_PROJECT_TOPIC_INDEX}
               ON {CONVERSATION_TABLE}
                 (tenant_id, region, parent_project, name, topic)
               WHERE kind = 'channel_public' AND parent_project IS NOT NULL;"
    );
    let message_identity = format!(
        "CREATE UNIQUE INDEX IF NOT EXISTS {MESSAGE_IDENTITY_INDEX}
               ON {MESSAGE_TABLE} (tenant_id, region, message_id);"
    );
    let message_thread_range = format!(
        "CREATE INDEX IF NOT EXISTS {MESSAGE_THREAD_RANGE_INDEX}
               ON {MESSAGE_TABLE}
               (tenant_id, region, conversation_id, thread_root_id, message_id);"
    );
    let thread_participant = format!(
        "{}\nSELECT myelin_make_tenant_scoped('{THREAD_PARTICIPANT_TABLE}');",
        super::pg::THREAD_PARTICIPANT_TABLE_DDL.replace("{table}", MESSAGE_TABLE),
    );
    let thread_root_author_index =
        super::pg::THREAD_ROOT_AUTHOR_INDEX_DDL.replace("{table}", MESSAGE_TABLE);
    let thread_following = super::pg::THREAD_FOLLOWING_DDL.replace("{table}", MESSAGE_TABLE);
    let message_erasure = format!(
        "{}\nSELECT myelin_make_tenant_scoped('{MESSAGE_ERASURE_OPERATION_TABLE}');",
        super::pg::MESSAGE_ERASURE_OPERATION_DDL.replace("{table}", MESSAGE_TABLE),
    );
    Migrations::of([
        Migration::plain_on("chat_0001_conversation", conversation, CONVERSATION_TABLE),
        Migration::plain_on("chat_0002_message", message, MESSAGE_TABLE),
        Migration::plain_on(
            "chat_0003_conversation_client_nonce",
            conversation_client_nonce,
            CONVERSATION_TABLE,
        ),
        Migration::plain_on(
            "chat_0004_conversation_project_recent",
            conversation_project_recent,
            CONVERSATION_TABLE,
        ),
        Migration::plain_on(
            "chat_0005_conversation_project_topic",
            conversation_project_topic,
            CONVERSATION_TABLE,
        ),
        Migration::plain_on(
            "chat_0006_public_conversation_project_topic",
            public_conversation_project_topic,
            CONVERSATION_TABLE,
        ),
        Migration::plain_on(
            "chat_0007_message_enum_constraints_expand",
            MESSAGE_ENUM_CONSTRAINTS_EXPAND_DDL,
            MESSAGE_TABLE,
        ),
        Migration::plain_on(
            "chat_0008_message_enum_constraints_validate",
            MESSAGE_ENUM_CONSTRAINTS_VALIDATE_DDL,
            MESSAGE_TABLE,
        ),
        Migration::plain_on(
            "chat_0009_message_identity",
            message_identity,
            MESSAGE_TABLE,
        ),
        Migration::plain_on(
            "chat_0010_message_thread_range",
            message_thread_range,
            MESSAGE_TABLE,
        ),
        Migration::plain_on(
            "chat_0011_thread_participant",
            thread_participant,
            THREAD_PARTICIPANT_TABLE,
        ),
        Migration::plain_on(
            "chat_0012_thread_root_author",
            thread_root_author_index,
            THREAD_PARTICIPANT_TABLE,
        ),
        Migration::plain_on(
            "chat_0013_thread_following",
            thread_following,
            THREAD_PARTICIPANT_TABLE,
        ),
        Migration::plain_on(
            "chat_0014_message_erasure_operation",
            message_erasure,
            MESSAGE_ERASURE_OPERATION_TABLE,
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_channel_validation_is_strict() {
        let id = ConversationId::new("acme", "fr-par", "01J00000000000000000000000");
        let valid = Conversation {
            id,
            kind: ConversationKind::ChannelPublic,
            home_cell: "fr-par:acme".into(),
            parent_project: Some("11111111-1111-1111-1111-111111111111".into()),
            name: Some("engineering".into()),
            topic: Some("deployments".into()),
            linked_ref: None,
            pinned_canvas: None,
            retention_days: None,
            archived: false,
            created_by: "chat-author:1".into(),
            acl_zookie: None,
        };
        assert!(validate_durable_channel(&valid).is_ok());
        assert!(validate_durable_channel(&Conversation {
            topic: Some(" bad".into()),
            ..valid.clone()
        })
        .is_err());
        assert!(validate_durable_channel(&Conversation {
            kind: ConversationKind::ChannelPrivate,
            parent_project: None,
            ..valid
        })
        .is_ok());
    }

    #[test]
    fn exact_conversation_batches_are_canonical_bounded_and_deduplicated() {
        let first = "01J00000000000000000000000".to_string();
        let second = "01J00000000000000000000001".to_string();
        assert!(canonical_conversation_batch(&[]).unwrap().is_empty());
        assert_eq!(
            canonical_conversation_batch(&[second.clone(), first.clone(), second]).unwrap(),
            [first.clone(), "01J00000000000000000000001".into()]
        );
        assert!(canonical_conversation_batch(&["not-a-ulid".into()]).is_err());
        assert!(
            canonical_conversation_batch(&vec![first; EXACT_CONVERSATION_BATCH_MAX + 1]).is_err()
        );
    }
}

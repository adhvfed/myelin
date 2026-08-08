use sqlx::postgres::PgPool;
use sqlx::Acquire;
use sqlx::Row;

use myelin_events::{
    derive_envelope, Actor, AggregateKey, DataRole, EmitContext, EventDraft, EventId, EventType,
    Timestamp, Visibility,
};

use crate::conversation::{Conversation, ConversationError, ConversationKind};
use crate::store::ConversationId;

pub const CONVERSATION_TABLE: &str = "chat_conversation";
pub const MESSAGE_TABLE: &str = "chat_message";
pub const CONVERSATION_RECENT_INDEX: &str = "chat_conversation_recent";

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
        validate_public_topic(conversation)?;
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
        if insert_conversation(&mut tx, conversation).await? == 0 {
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
        event_id: EventId,
        actor: Actor,
        occurred: Timestamp,
        recorded: Timestamp,
    ) -> Result<(), ConversationError> {
        validate_public_topic(conversation)?;
        let subject =
            crate::subs::mint_channel(&conversation.id.tenant, &conversation.id.conversation_id)
                .map_err(|error| {
                    ConversationError::SchemaViolation(format!("mint channel ref: {error}"))
                })?;
        let envelope = derive_envelope(
            EventDraft {
                type_: EventType(crate::events::CHAT_CHANNEL_CREATED.into()),
                subject,
                aggregate: AggregateKey(conversation.id.conversation_id.clone()),
                payload: serde_json::json!({
                    "conversation_id": conversation.id.conversation_id,
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
        let result = insert_conversation(&mut tx, conversation).await?;
        if result == 0 {
            return Err(ConversationError::AlreadyExists(
                conversation.id.conversation_id.clone(),
            ));
        }
        myelin_storage::pgrelay::PgRelay::co_commit_in_tx(
            &mut tx,
            &conversation.id.conversation_id,
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

    pub async fn list_public(
        &self,
        tenant: &str,
        region: &str,
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
        let rows = sqlx::query(
            "SELECT tenant_id, region, conversation_id, kind, home_cell, parent_project, name,
                    topic, linked_ref, pinned_canvas, retention_days, archived, created_by, acl_zookie
               FROM chat_conversation
              WHERE tenant_id = $1 AND region = $2 AND kind = 'channel_public' AND NOT archived
                AND ($3::text IS NULL OR conversation_id < $3)
              ORDER BY conversation_id DESC LIMIT $4",
        )
        .bind(tenant)
        .bind(region)
        .bind(before)
        .bind(i64::from(limit))
        .fetch_all(&mut *conn)
        .await
        .map_err(storage("list conversations"))?;
        rows.iter().map(row_to_conversation).collect()
    }
}

async fn insert_conversation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    conversation: &Conversation,
) -> Result<u64, ConversationError> {
    let result = sqlx::query(
        "INSERT INTO chat_conversation (
           tenant_id, region, conversation_id, kind, home_cell, parent_project, name, topic,
           linked_ref, pinned_canvas, retention_days, archived, created_by, acl_zookie
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
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
    .execute(&mut **tx)
    .await
    .map_err(storage("insert conversation"))?;
    Ok(result.rows_affected())
}

fn validate_public_topic(conversation: &Conversation) -> Result<(), ConversationError> {
    if conversation.kind != ConversationKind::ChannelPublic {
        return Err(ConversationError::SchemaViolation(
            "the public topic store accepts only public channel conversations".into(),
        ));
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

    let conversation = Box::leak(
        format!(
            "{CONVERSATION_TABLE_DDL}\nSELECT myelin_make_tenant_scoped('{CONVERSATION_TABLE}');"
        )
        .into_boxed_str(),
    );
    let message_ddl = super::pg::MESSAGE_TABLE_DDL.replace("{table}", MESSAGE_TABLE);
    let message = Box::leak(
        format!("{message_ddl}\nSELECT myelin_make_tenant_scoped('{MESSAGE_TABLE}');")
            .into_boxed_str(),
    );
    Migrations::of([
        Migration::plain_on("chat_0001_conversation", conversation, CONVERSATION_TABLE),
        Migration::plain_on("chat_0002_message", message, MESSAGE_TABLE),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_create_and_tenant_scope_both_tables() {
        let migrations = chat_migrations();
        assert_eq!(migrations.0.len(), 2);
        for (migration, table) in migrations.0.iter().zip([CONVERSATION_TABLE, MESSAGE_TABLE]) {
            assert!(migration
                .ddl
                .contains(&format!("CREATE TABLE IF NOT EXISTS {table}")));
            assert!(migration
                .ddl
                .contains(&format!("myelin_make_tenant_scoped('{table}')")));
        }
        assert!(migrations.0[0].ddl.contains(CONVERSATION_RECENT_INDEX));
    }

    #[test]
    fn public_topic_validation_is_strict() {
        let id = ConversationId::new("acme", "fr-par", "01J00000000000000000000000");
        let valid = Conversation {
            id,
            kind: ConversationKind::ChannelPublic,
            home_cell: "fr-par:acme".into(),
            parent_project: None,
            name: Some("engineering".into()),
            topic: Some("deployments".into()),
            linked_ref: None,
            pinned_canvas: None,
            retention_days: None,
            archived: false,
            created_by: "chat-author:1".into(),
            acl_zookie: None,
        };
        assert!(validate_public_topic(&valid).is_ok());
        assert!(validate_public_topic(&Conversation {
            topic: Some(" bad".into()),
            ..valid
        })
        .is_err());
    }
}

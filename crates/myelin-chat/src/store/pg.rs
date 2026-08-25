use std::collections::BTreeSet;

use sqlx::postgres::PgPool;
use sqlx::Row;

use myelin_events::Actor;
use myelin_identity::PrincipalId;

use super::{
    is_canonical_ulid, AuthorKind, ConversationId, Message, MessageId, MessageLocation,
    RangeCursor, StoreError,
};
#[cfg(any(test, feature = "test-support"))]
use super::{NewMessage, TombstoneReason};

mod append;
mod erase;
mod thread;
mod thread_notification;

const EXACT_MESSAGE_BATCH_MAX: usize = 100;
pub const AUTHOR_ERASURE_BATCH_MAX: usize = 100;

pub const MESSAGE_TABLE_DDL: &str = "\
CREATE TABLE IF NOT EXISTS {table} (
    tenant_id       text        NOT NULL,
    region          text        NOT NULL,
    conversation_id text        NOT NULL,
    message_id      text        NOT NULL,
    thread_root_id  text,
    author          text        NOT NULL,
    author_kind     smallint    NOT NULL,
    body_inline     bytea       NOT NULL,
    body_nodes      bytea       NOT NULL DEFAULT '\\x',
    client_nonce    text        NOT NULL,
    edited_seq      int         NOT NULL DEFAULT 0,
    state           smallint    NOT NULL DEFAULT 0,
    created_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, region, conversation_id, message_id),
    CONSTRAINT {table}_nonce_unique UNIQUE (tenant_id, region, conversation_id, client_nonce)
);
CREATE INDEX IF NOT EXISTS {table}_range
    ON {table} (tenant_id, region, conversation_id, message_id DESC);";

pub(crate) const THREAD_PARTICIPANT_TABLE_DDL: &str = "\
CREATE TABLE IF NOT EXISTS {table}_thread_participant (
    tenant_id       text NOT NULL,
    region          text NOT NULL,
    conversation_id text NOT NULL,
    thread_root_id  text NOT NULL,
    principal_id    text NOT NULL,
    role            smallint NOT NULL CHECK (role BETWEEN 0 AND 1),
    PRIMARY KEY (tenant_id, region, conversation_id, thread_root_id, principal_id),
    FOREIGN KEY (tenant_id, region, conversation_id, thread_root_id)
      REFERENCES {table} (tenant_id, region, conversation_id, message_id)
      ON DELETE CASCADE
);";

pub(crate) const THREAD_FOLLOWING_DDL: &str = "\
ALTER TABLE {table}_thread_participant
    ADD COLUMN IF NOT EXISTS notifications_enabled boolean NOT NULL DEFAULT true;";

pub(crate) const THREAD_ROOT_AUTHOR_INDEX_DDL: &str = "\
CREATE UNIQUE INDEX IF NOT EXISTS {table}_thread_root_author
    ON {table}_thread_participant
       (tenant_id, region, conversation_id, thread_root_id)
    WHERE role = 0;";

pub const MESSAGE_ERASURE_OPERATION_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS {table}_erasure_operation (
    tenant_id       text        NOT NULL,
    region          text        NOT NULL,
    operation_id    text        NOT NULL CHECK (length(operation_id) BETWEEN 1 AND 255),
    author          text        NOT NULL CHECK (length(author) BETWEEN 1 AND 255),
    started_at      timestamptz NOT NULL DEFAULT now(),
    completed_at    timestamptz,
    messages_erased bigint,
    events_emitted  bigint,
    PRIMARY KEY (tenant_id, region, operation_id),
    CHECK (
      (completed_at IS NULL AND messages_erased IS NULL AND events_emitted IS NULL)
      OR
      (completed_at IS NOT NULL
       AND messages_erased IS NOT NULL AND messages_erased >= 0
       AND events_emitted IS NOT NULL AND events_emitted = messages_erased)
    )
);
CREATE INDEX IF NOT EXISTS {table}_erasure_in_progress
    ON {table}_erasure_operation (tenant_id, region, author)
    WHERE completed_at IS NULL;
CREATE OR REPLACE FUNCTION {table}_guard_erasure_completion()
RETURNS trigger
LANGUAGE plpgsql
AS $myelin$
BEGIN
  IF NEW.tenant_id IS DISTINCT FROM OLD.tenant_id OR
     NEW.region IS DISTINCT FROM OLD.region OR
     NEW.operation_id IS DISTINCT FROM OLD.operation_id OR
     NEW.author IS DISTINCT FROM OLD.author OR
     NEW.started_at IS DISTINCT FROM OLD.started_at OR
     OLD.completed_at IS NOT NULL OR
     NEW.completed_at IS NULL THEN
    RAISE EXCEPTION 'Chat message erasure permits only its one-way completion transition';
  END IF;
  RETURN NEW;
END
$myelin$;
DROP TRIGGER IF EXISTS {table}_erasure_guard_update ON {table}_erasure_operation;
CREATE TRIGGER {table}_erasure_guard_update
BEFORE UPDATE ON {table}_erasure_operation
FOR EACH ROW EXECUTE FUNCTION {table}_guard_erasure_completion();
"#;

pub(crate) const MESSAGE_ERASURE_PROGRESS_DDL: &str = r#"
ALTER TABLE {table}_erasure_operation
    ADD COLUMN IF NOT EXISTS validation_cursor text,
    ADD COLUMN IF NOT EXISTS validation_completed_at timestamptz,
    ADD COLUMN IF NOT EXISTS messages_erased_so_far bigint NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS events_emitted_so_far bigint NOT NULL DEFAULT 0;

DROP TRIGGER IF EXISTS {table}_erasure_guard_update ON {table}_erasure_operation;

UPDATE {table}_erasure_operation
   SET validation_completed_at = completed_at,
       messages_erased_so_far = messages_erased,
       events_emitted_so_far = events_emitted
 WHERE completed_at IS NOT NULL
   AND validation_completed_at IS NULL;

DO $myelin$
BEGIN
  IF NOT EXISTS (
    SELECT 1 FROM pg_catalog.pg_constraint
     WHERE conrelid = '{table}_erasure_operation'::regclass
       AND conname = '{table}_erasure_progress_valid'
  ) THEN
    ALTER TABLE {table}_erasure_operation
      ADD CONSTRAINT {table}_erasure_progress_valid CHECK (
        messages_erased_so_far >= 0 AND
        events_emitted_so_far = messages_erased_so_far AND
        (validation_completed_at IS NOT NULL OR messages_erased_so_far = 0) AND
        (completed_at IS NULL OR (
          validation_completed_at IS NOT NULL AND
          messages_erased = messages_erased_so_far AND
          events_emitted = events_emitted_so_far
        ))
      );
  END IF;
END
$myelin$;

CREATE INDEX IF NOT EXISTS {table}_author_erasure_batch
    ON {table} (tenant_id, region, author, message_id)
    WHERE state <> 3;

CREATE OR REPLACE FUNCTION {table}_guard_erasure_completion()
RETURNS trigger
LANGUAGE plpgsql
AS $myelin$
BEGIN
  IF NEW.tenant_id IS DISTINCT FROM OLD.tenant_id OR
     NEW.region IS DISTINCT FROM OLD.region OR
     NEW.operation_id IS DISTINCT FROM OLD.operation_id OR
     NEW.author IS DISTINCT FROM OLD.author OR
     NEW.started_at IS DISTINCT FROM OLD.started_at OR
     OLD.completed_at IS NOT NULL OR
     NEW.messages_erased_so_far < OLD.messages_erased_so_far OR
     NEW.events_emitted_so_far < OLD.events_emitted_so_far OR
     NEW.messages_erased_so_far <> NEW.events_emitted_so_far OR
     (OLD.validation_cursor IS NOT NULL AND
       NEW.validation_cursor IS DISTINCT FROM OLD.validation_cursor AND
       (NEW.validation_cursor IS NULL OR NEW.validation_cursor <= OLD.validation_cursor)) OR
     (OLD.validation_completed_at IS NOT NULL AND
       (NEW.validation_completed_at IS DISTINCT FROM OLD.validation_completed_at OR
        NEW.validation_cursor IS DISTINCT FROM OLD.validation_cursor)) OR
     (NEW.validation_completed_at IS NULL AND NEW.messages_erased_so_far <> 0) OR
     (NEW.completed_at IS NULL AND
       (NEW.messages_erased IS NOT NULL OR NEW.events_emitted IS NOT NULL)) OR
     (NEW.completed_at IS NOT NULL AND
       (NEW.validation_completed_at IS NULL OR
        NEW.messages_erased IS DISTINCT FROM NEW.messages_erased_so_far OR
        NEW.events_emitted IS DISTINCT FROM NEW.events_emitted_so_far)) THEN
    RAISE EXCEPTION 'Chat message erasure permits only forward validation, progress, and completion';
  END IF;
  RETURN NEW;
END
$myelin$;

DROP TRIGGER IF EXISTS {table}_erasure_guard_update ON {table}_erasure_operation;
CREATE TRIGGER {table}_erasure_guard_update
BEFORE UPDATE ON {table}_erasure_operation
FOR EACH ROW EXECUTE FUNCTION {table}_guard_erasure_completion();
"#;

#[derive(Clone, Debug)]
pub struct MessageAttribution {
    event_actor: Actor,
    notification_recipient: PrincipalId,
}

impl MessageAttribution {
    pub fn new(event_actor: Actor, notification_recipient: PrincipalId) -> Self {
        Self {
            event_actor,
            notification_recipient,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MessageErasureAttempt {
    pub(crate) operation_id: String,
    pub(crate) actor: Actor,
    pub(crate) occurred: myelin_events::Timestamp,
    pub(crate) recorded: myelin_events::Timestamp,
}

pub(crate) struct VerifiedMessageErasureAttempt {
    pub(crate) author: String,
    pub(crate) attempt: MessageErasureAttempt,
}

impl VerifiedMessageErasureAttempt {
    pub(crate) fn after_key_destruction(
        author: impl Into<String>,
        attempt: MessageErasureAttempt,
    ) -> Self {
        Self {
            author: author.into(),
            attempt,
        }
    }
}

impl MessageErasureAttempt {
    pub fn new(
        operation_id: impl Into<String>,
        actor: Actor,
        occurred: myelin_events::Timestamp,
        recorded: myelin_events::Timestamp,
    ) -> Self {
        Self {
            operation_id: operation_id.into(),
            actor,
            occurred,
            recorded,
        }
    }

    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }
}

#[derive(Clone)]
pub struct PgMessageStore {
    pool: PgPool,
    region: String,
    table: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthoredMessageEraseReceipt {
    pub messages_tombstoned: u64,
    pub erasure_events_co_committed: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthoredMessageErasureState {
    Pending,
    Completed(AuthoredMessageEraseReceipt),
}

impl PgMessageStore {
    pub fn new(
        pool: PgPool,
        region: impl Into<String>,
        table: impl Into<String>,
    ) -> PgMessageStore {
        PgMessageStore {
            pool,
            region: region.into(),
            table: table.into(),
        }
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub async fn migrate(&self) -> Result<(), StoreError> {
        let ddl = MESSAGE_TABLE_DDL.replace("{table}", &self.table);
        sqlx::raw_sql(&ddl)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Cold(format!("message DDL: {e}")))?;
        let participant_table = self.thread_participant_table();
        let participant_ddl = THREAD_PARTICIPANT_TABLE_DDL.replace("{table}", &self.table);
        sqlx::raw_sql(&participant_ddl)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Cold(format!("thread participant DDL: {e}")))?;
        let erasure_table = self.erasure_operation_table();
        let erasure_ddl = MESSAGE_ERASURE_OPERATION_DDL.replace("{table}", &self.table);
        sqlx::raw_sql(&erasure_ddl)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Cold(format!("message erasure DDL: {e}")))?;
        let erasure_progress_ddl = MESSAGE_ERASURE_PROGRESS_DDL.replace("{table}", &self.table);
        sqlx::raw_sql(&erasure_progress_ddl)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Cold(format!("message erasure progress DDL: {e}")))?;
        let following_ddl = THREAD_FOLLOWING_DDL.replace("{table}", &self.table);
        sqlx::raw_sql(&following_ddl)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Cold(format!("thread following DDL: {e}")))?;
        let root_author_index_ddl = THREAD_ROOT_AUTHOR_INDEX_DDL.replace("{table}", &self.table);
        sqlx::raw_sql(&root_author_index_ddl)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Cold(format!("thread root author index DDL: {e}")))?;
        sqlx::raw_sql(&format!(
            "CREATE UNIQUE INDEX IF NOT EXISTS {table}_identity \
             ON {table} (tenant_id, region, message_id)",
            table = self.table,
        ))
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Cold(format!("message identity index: {e}")))?;
        sqlx::raw_sql(&format!(
            "CREATE INDEX IF NOT EXISTS {table}_thread_range \
             ON {table} (tenant_id, region, conversation_id, thread_root_id, message_id)",
            table = self.table,
        ))
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Cold(format!("message thread range index: {e}")))?;
        sqlx::raw_sql(&format!(
            "DO $myelin$ \
             BEGIN \
               IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_constraint \
                 WHERE conrelid = '{table}'::regclass \
                   AND conname = '{table}_author_kind_known') THEN \
                 ALTER TABLE {table} ADD CONSTRAINT {table}_author_kind_known \
                   CHECK (author_kind BETWEEN 0 AND 2); \
               END IF; \
               IF NOT EXISTS (SELECT 1 FROM pg_catalog.pg_constraint \
                 WHERE conrelid = '{table}'::regclass \
                   AND conname = '{table}_state_known') THEN \
                 ALTER TABLE {table} ADD CONSTRAINT {table}_state_known \
                   CHECK (state BETWEEN 0 AND 3); \
               END IF; \
             END \
             $myelin$;",
            table = self.table,
        ))
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Cold(format!("message enum constraints: {e}")))?;
        sqlx::raw_sql(&format!(
            "SELECT myelin_make_tenant_scoped('{}')",
            self.table
        ))
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Cold(format!("make tenant scoped: {e}")))?;
        sqlx::raw_sql(&format!(
            "SELECT myelin_make_tenant_scoped('{participant_table}')"
        ))
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Cold(format!("make thread participants tenant scoped: {e}")))?;
        sqlx::raw_sql(&format!(
            "SELECT myelin_make_tenant_scoped('{erasure_table}')"
        ))
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Cold(format!("make message erasures tenant scoped: {e}")))?;
        sqlx::raw_sql(&format!("GRANT ALL ON {} TO myelin_app", self.table))
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Cold(format!("grant: {e}")))?;
        sqlx::raw_sql(&format!("GRANT ALL ON {participant_table} TO myelin_app"))
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Cold(format!("thread participant grant: {e}")))?;
        sqlx::raw_sql(&format!("GRANT ALL ON {erasure_table} TO myelin_app"))
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Cold(format!("message erasure grant: {e}")))?;
        Ok(())
    }

    fn thread_participant_table(&self) -> String {
        format!("{}_thread_participant", self.table)
    }

    fn erasure_operation_table(&self) -> String {
        format!("{}_erasure_operation", self.table)
    }

    async fn set_session_scope(
        &self,
        conn: &mut sqlx::pool::PoolConnection<sqlx::Postgres>,
        tenant: &str,
        region: &str,
    ) -> Result<(), StoreError> {
        if region != self.region {
            return Err(StoreError::Cold(format!(
                "message store is pinned to region {}, not {region}",
                self.region
            )));
        }
        sqlx::query("SELECT set_config('myelin.tenant_id', $1, false), set_config('myelin.region', $2, false)")
            .bind(tenant)
            .bind(region)
            .execute(&mut **conn)
            .await
            .map_err(|e| StoreError::Cold(format!("set session scope: {e}")))?;
        Ok(())
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Mutates only the message table for storage-parity tests; emits no event.
    pub async fn append_storage_only(
        &self,
        minter: &dyn super::UlidSource,
        msg: NewMessage,
    ) -> Result<MessageId, StoreError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StoreError::Cold(format!("acquire: {e}")))?;
        self.set_session_scope(&mut conn, &msg.conv.tenant, &msg.conv.region)
            .await?;

        if let Some(existing) = sqlx::query_scalar::<_, String>(&format!(
            "SELECT message_id FROM {} WHERE tenant_id = $1 AND region = $2 \
             AND conversation_id = $3 AND client_nonce = $4",
            self.table
        ))
        .bind(&msg.conv.tenant)
        .bind(&msg.conv.region)
        .bind(&msg.conv.conversation_id)
        .bind(&msg.client_nonce)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| StoreError::Cold(format!("nonce check: {e}")))?
        {
            return Ok(MessageId(existing));
        }

        let message_id = minter.mint();
        let stored_message_id = sqlx::query_scalar::<_, String>(&format!(
            "INSERT INTO {} (tenant_id, region, conversation_id, message_id, thread_root_id, \
             author, author_kind, body_inline, body_nodes, client_nonce, edited_seq, state) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 0, 0) \
             ON CONFLICT (tenant_id, region, conversation_id, client_nonce) DO UPDATE \
             SET client_nonce = EXCLUDED.client_nonce \
             RETURNING message_id",
            self.table
        ))
        .bind(&msg.conv.tenant)
        .bind(&msg.conv.region)
        .bind(&msg.conv.conversation_id)
        .bind(message_id.as_str())
        .bind(msg.thread_root_id.as_ref().map(|t| t.0.clone()))
        .bind(&msg.author)
        .bind(author_kind_code(msg.author_kind) as i16)
        .bind(&msg.body_inline)
        .bind(&msg.body_nodes)
        .bind(&msg.client_nonce)
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| StoreError::Cold(format!("insert: {e}")))?;
        Ok(MessageId(stored_message_id))
    }

    pub async fn range(
        &self,
        conv: &ConversationId,
        cursor: RangeCursor,
        limit: u32,
    ) -> Result<Vec<Message>, StoreError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StoreError::Cold(format!("acquire: {e}")))?;
        self.set_session_scope(&mut conn, &conv.tenant, &conv.region)
            .await?;

        let (where_clause, bound, order): (&str, Option<String>, &str) = match &cursor {
            RangeCursor::Recent => ("", None, "DESC"),
            RangeCursor::Before(id) => ("AND message_id < $5", Some(id.0.clone()), "DESC"),
            RangeCursor::After(id) => ("AND message_id > $5", Some(id.0.clone()), "ASC"),
        };

        let range_sql_tenant_id = format!(
            "SELECT tenant_id, region, conversation_id, message_id, thread_root_id, author, \
             author_kind, body_inline, body_nodes, client_nonce, edited_seq, state \
             FROM {} WHERE tenant_id = $1 AND region = $2 AND conversation_id = $3 {} \
             ORDER BY message_id {} LIMIT $4",
            self.table, where_clause, order,
        );
        let mut q = sqlx::query(&range_sql_tenant_id)
            .bind(&conv.tenant)
            .bind(&conv.region)
            .bind(&conv.conversation_id)
            .bind(limit as i64);
        if let Some(b) = &bound {
            q = q.bind(b);
        }
        let rows = q
            .fetch_all(&mut *conn)
            .await
            .map_err(|e| StoreError::Cold(format!("range select: {e}")))?;

        let mut out: Vec<Message> = rows.iter().map(row_to_message).collect::<Result<_, _>>()?;
        out.sort_by(|a, b| a.message_id.cmp(&b.message_id));
        Ok(out)
    }

    pub async fn resync_from(
        &self,
        conv: &ConversationId,
        cursor: &MessageId,
    ) -> Result<Vec<Message>, StoreError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StoreError::Cold(format!("acquire: {e}")))?;
        self.set_session_scope(&mut conn, &conv.tenant, &conv.region)
            .await?;
        let rows = sqlx::query(&format!(
            "SELECT tenant_id, region, conversation_id, message_id, thread_root_id, author, \
             author_kind, body_inline, body_nodes, client_nonce, edited_seq, state \
             FROM {} WHERE tenant_id = $1 AND region = $2 AND conversation_id = $3 \
             AND message_id > $4 ORDER BY message_id ASC",
            self.table
        ))
        .bind(&conv.tenant)
        .bind(&conv.region)
        .bind(&conv.conversation_id)
        .bind(cursor.as_str())
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| StoreError::Cold(format!("resync select: {e}")))?;
        rows.iter().map(row_to_message).collect()
    }

    /// Finds one message by its artifact identity without materializing any other conversation.
    /// Callers must still authorize the returned parent conversation before exposing the row.
    pub async fn get_exact(
        &self,
        tenant: &str,
        message_id: &MessageId,
    ) -> Result<Option<Message>, StoreError> {
        validate_exact_message_ids(std::slice::from_ref(message_id))?;
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StoreError::Cold(format!("acquire: {e}")))?;
        self.set_session_scope(&mut conn, tenant, &self.region)
            .await?;
        let row = sqlx::query(&format!(
            "SELECT tenant_id, region, conversation_id, message_id, thread_root_id, author, \
             author_kind, body_inline, body_nodes, client_nonce, edited_seq, state \
             FROM {} WHERE tenant_id = $1 AND region = $2 AND message_id = $3",
            self.table
        ))
        .bind(tenant)
        .bind(&self.region)
        .bind(message_id.as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| StoreError::Cold(format!("exact message select: {e}")))?;
        row.as_ref().map(row_to_message).transpose()
    }

    /// Resolves a bounded card viewport to content-free message coordinates in one query.
    /// Conversation visibility is deliberately not inferred here; the Edge composes that owner.
    pub async fn locate_exact(
        &self,
        tenant: &str,
        message_ids: &[MessageId],
    ) -> Result<Vec<MessageLocation>, StoreError> {
        let message_ids = validate_exact_message_ids(message_ids)?;
        if message_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StoreError::Cold(format!("acquire: {e}")))?;
        self.set_session_scope(&mut conn, tenant, &self.region)
            .await?;
        let rows = sqlx::query(&format!(
            "SELECT tenant_id, region, conversation_id, message_id, thread_root_id, state \
             FROM {} WHERE tenant_id = $1 AND region = $2 AND message_id = ANY($3::text[]) \
             ORDER BY message_id ASC",
            self.table
        ))
        .bind(tenant)
        .bind(&self.region)
        .bind(message_ids)
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| StoreError::Cold(format!("exact message location select: {e}")))?;
        rows.iter().map(row_to_message_location).collect()
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Mutates only the message table for storage-parity tests; emits no event.
    pub async fn revise_storage_only(
        &self,
        conv: &ConversationId,
        msg_id: &MessageId,
        body_inline: Vec<u8>,
        body_nodes: Vec<u8>,
        expect_seq: i32,
    ) -> Result<(), StoreError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StoreError::Cold(format!("acquire: {e}")))?;
        self.set_session_scope(&mut conn, &conv.tenant, &conv.region)
            .await?;
        let revise_sql_tenant_id = format!(
            "UPDATE {} SET body_inline = $5, body_nodes = $6, edited_seq = edited_seq + 1, \
             state = 1 WHERE tenant_id = $1 AND region = $2 AND conversation_id = $3 \
             AND message_id = $4 AND edited_seq = $7",
            self.table
        );
        let updated = sqlx::query(&revise_sql_tenant_id)
            .bind(&conv.tenant)
            .bind(&conv.region)
            .bind(&conv.conversation_id)
            .bind(msg_id.as_str())
            .bind(&body_inline)
            .bind(&body_nodes)
            .bind(expect_seq)
            .execute(&mut *conn)
            .await
            .map_err(|e| StoreError::Cold(format!("revise: {e}")))?;
        if updated.rows_affected() == 0 {
            let actual: Option<i32> = sqlx::query_scalar(&format!(
                "SELECT edited_seq FROM {} WHERE tenant_id = $1 AND region = $2 \
                 AND conversation_id = $3 AND message_id = $4",
                self.table
            ))
            .bind(&conv.tenant)
            .bind(&conv.region)
            .bind(&conv.conversation_id)
            .bind(msg_id.as_str())
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| StoreError::Cold(format!("revise seq read: {e}")))?;
            return match actual {
                Some(actual) => Err(StoreError::CasConflict {
                    message_id: msg_id.clone(),
                    expected: expect_seq,
                    actual,
                }),
                None => Err(StoreError::NotFound(msg_id.clone())),
            };
        }
        Ok(())
    }

    #[cfg(any(test, feature = "test-support"))]
    /// Mutates only the message table for storage-parity tests; emits no event.
    pub async fn tombstone_storage_only(
        &self,
        conv: &ConversationId,
        msg_id: &MessageId,
        _reason: TombstoneReason,
    ) -> Result<(), StoreError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StoreError::Cold(format!("acquire: {e}")))?;
        self.set_session_scope(&mut conn, &conv.tenant, &conv.region)
            .await?;
        let tombstone_sql_tenant_id = format!(
            "UPDATE {} SET state = 3, body_inline = '\\x', body_nodes = '\\x' \
             WHERE tenant_id = $1 AND region = $2 AND conversation_id = $3 AND message_id = $4",
            self.table
        );
        let updated = sqlx::query(&tombstone_sql_tenant_id)
            .bind(&conv.tenant)
            .bind(&conv.region)
            .bind(&conv.conversation_id)
            .bind(msg_id.as_str())
            .execute(&mut *conn)
            .await
            .map_err(|e| StoreError::Cold(format!("tombstone: {e}")))?;
        if updated.rows_affected() == 0 {
            return Err(StoreError::NotFound(msg_id.clone()));
        }
        Ok(())
    }
}

fn row_to_message(r: &sqlx::postgres::PgRow) -> Result<Message, StoreError> {
    let author_kind = decode_author_kind(r.get("author_kind"))?;
    let state = decode_message_state(r.get("state"))?;
    Ok(Message {
        message_id: MessageId(r.get::<String, _>("message_id")),
        conv: ConversationId {
            tenant: r.get::<String, _>("tenant_id"),
            region: r.get::<String, _>("region"),
            conversation_id: r.get::<String, _>("conversation_id"),
        },
        thread_root_id: r.get::<Option<String>, _>("thread_root_id").map(MessageId),
        author: r.get::<String, _>("author"),
        author_kind,
        body_inline: r.get::<Vec<u8>, _>("body_inline"),
        body_nodes: r.get::<Vec<u8>, _>("body_nodes"),
        client_nonce: r.get::<String, _>("client_nonce"),
        edited_seq: r.get::<i32, _>("edited_seq"),
        state,
    })
}

fn row_to_message_location(row: &sqlx::postgres::PgRow) -> Result<MessageLocation, StoreError> {
    Ok(MessageLocation {
        message_id: MessageId(row.get("message_id")),
        conv: ConversationId {
            tenant: row.get("tenant_id"),
            region: row.get("region"),
            conversation_id: row.get("conversation_id"),
        },
        thread_root_id: row
            .get::<Option<String>, _>("thread_root_id")
            .map(MessageId),
        state: decode_message_state(row.get("state"))?,
    })
}

fn validate_exact_message_ids(message_ids: &[MessageId]) -> Result<Vec<String>, StoreError> {
    if message_ids.len() > EXACT_MESSAGE_BATCH_MAX {
        return Err(StoreError::Cold(format!(
            "exact message lookup exceeds {EXACT_MESSAGE_BATCH_MAX} coordinates"
        )));
    }
    let mut canonical = BTreeSet::new();
    for message_id in message_ids {
        if !is_canonical_ulid(message_id.as_str()) {
            return Err(StoreError::Cold(
                "exact message lookup requires canonical identifiers".into(),
            ));
        }
        canonical.insert(message_id.0.clone());
    }
    Ok(canonical.into_iter().collect())
}

fn decode_author_kind(code: i16) -> Result<AuthorKind, StoreError> {
    u8::try_from(code)
        .ok()
        .and_then(super::author_kind_from_code)
        .ok_or_else(|| StoreError::Cold("stored message author kind is invalid".into()))
}

fn decode_message_state(code: i16) -> Result<super::MessageState, StoreError> {
    u8::try_from(code)
        .ok()
        .and_then(super::state_from_code)
        .ok_or_else(|| StoreError::Cold("stored message state is invalid".into()))
}

fn author_kind_code(k: AuthorKind) -> u8 {
    match k {
        AuthorKind::Human => 0,
        AuthorKind::Agent => 1,
        AuthorKind::Service => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_enum_codes_never_fall_back_to_human_or_active() {
        assert_eq!(decode_author_kind(0).unwrap(), AuthorKind::Human);
        assert_eq!(decode_author_kind(1).unwrap(), AuthorKind::Agent);
        assert_eq!(decode_author_kind(2).unwrap(), AuthorKind::Service);
        for invalid in [-1, 3, i16::MAX] {
            assert!(
                decode_author_kind(invalid).is_err(),
                "unknown author kind {invalid} must not be attributed to a human"
            );
        }

        assert_eq!(
            decode_message_state(0).unwrap(),
            crate::store::MessageState::Active
        );
        assert_eq!(
            decode_message_state(3).unwrap(),
            crate::store::MessageState::Tombstoned
        );
        for invalid in [-1, 4, i16::MAX] {
            assert!(
                decode_message_state(invalid).is_err(),
                "unknown message state {invalid} must not resurrect as active"
            );
        }
    }

    #[test]
    fn exact_message_lookups_are_canonical_deduplicated_and_bounded() {
        let first = MessageId("01J00000000000000000000000".into());
        let second = MessageId("01J00000000000000000000001".into());
        assert_eq!(
            validate_exact_message_ids(&[second.clone(), first.clone(), second]),
            Ok(vec![first.0, "01J00000000000000000000001".into()])
        );
        assert!(validate_exact_message_ids(&[MessageId("not-an-id".into())]).is_err());
        assert!(validate_exact_message_ids(
            &(0..=EXACT_MESSAGE_BATCH_MAX)
                .map(|_| MessageId("01J00000000000000000000000".into()))
                .collect::<Vec<_>>()
        )
        .is_err());
    }
}

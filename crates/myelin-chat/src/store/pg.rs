use sqlx::postgres::PgPool;
use sqlx::{Acquire, Row};

use myelin_events::{
    derive_envelope, Actor, AggregateKey, DataRole, EmitContext, EventDraft, EventEnvelope,
    EventId, EventType, Timestamp, Visibility,
};

use super::{
    AuthorKind, ConversationId, Message, MessageId, MessageState, NewMessage, RangeCursor,
    StoreError, TombstoneReason,
};

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

#[derive(Clone)]
pub struct PgMessageStore {
    pool: PgPool,
    region: String,
    table: String,
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
        sqlx::raw_sql(&format!(
            "SELECT myelin_make_tenant_scoped('{}')",
            self.table
        ))
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Cold(format!("make tenant scoped: {e}")))?;
        sqlx::raw_sql(&format!("GRANT ALL ON {} TO myelin_app", self.table))
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Cold(format!("grant: {e}")))?;
        Ok(())
    }

    async fn set_session_scope(
        &self,
        conn: &mut sqlx::pool::PoolConnection<sqlx::Postgres>,
        tenant: &str,
        region: &str,
    ) -> Result<(), StoreError> {
        sqlx::query("SELECT set_config('myelin.tenant_id', $1, false), set_config('myelin.region', $2, false)")
            .bind(tenant)
            .bind(region)
            .execute(&mut **conn)
            .await
            .map_err(|e| StoreError::Cold(format!("set session scope: {e}")))?;
        Ok(())
    }

    pub async fn append(
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
        sqlx::query(&format!(
            "INSERT INTO {} (tenant_id, region, conversation_id, message_id, thread_root_id, \
             author, author_kind, body_inline, body_nodes, client_nonce, edited_seq, state) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 0, 0) \
             ON CONFLICT DO NOTHING",
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
        .execute(&mut *conn)
        .await
        .map_err(|e| StoreError::Cold(format!("insert: {e}")))?;
        Ok(message_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn append_co_commit(
        &self,
        minter: &dyn super::UlidSource,
        msg: NewMessage,
        event_id: EventId,
        actor: Actor,
        occurred: Timestamp,
        recorded: Timestamp,
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
        let envelope =
            self.message_created_envelope(&msg, &message_id, event_id, actor, occurred, recorded)?;

        let mut dbtx = conn
            .begin()
            .await
            .map_err(|e| StoreError::Cold(format!("begin co-commit tx: {e}")))?;

        sqlx::query(&format!(
            "INSERT INTO {} (tenant_id, region, conversation_id, message_id, thread_root_id, \
             author, author_kind, body_inline, body_nodes, client_nonce, edited_seq, state) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 0, 0) \
             ON CONFLICT DO NOTHING",
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
        .execute(&mut *dbtx)
        .await
        .map_err(|e| StoreError::Cold(format!("co-commit message insert: {e}")))?;

        myelin_storage::pgrelay::PgRelay::co_commit_in_tx(
            &mut dbtx,
            &msg.conv.conversation_id,
            &envelope,
        )
        .await
        .map_err(|e| StoreError::Cold(format!("co-commit outbox insert: {e}")))?;

        dbtx.commit()
            .await
            .map_err(|e| StoreError::Cold(format!("co-commit: {e}")))?;
        Ok(message_id)
    }

    #[allow(clippy::too_many_arguments)]
    fn message_created_envelope(
        &self,
        msg: &NewMessage,
        message_id: &MessageId,
        event_id: EventId,
        actor: Actor,
        occurred: Timestamp,
        recorded: Timestamp,
    ) -> Result<EventEnvelope, StoreError> {
        let subject = crate::subs::mint_message(&msg.conv.tenant, message_id.as_str())
            .map_err(|e| StoreError::Cold(format!("mint message #sub anchor: {e}")))?;
        let draft = EventDraft {
            type_: EventType(crate::events::CHAT_MESSAGE_CREATED.to_string()),
            subject,
            aggregate: AggregateKey(msg.conv.conversation_id.clone()),
            payload: serde_json::json!({
                "conversation_id": msg.conv.conversation_id,
                "message_id": message_id.as_str(),
                "author": msg.author,
                "thread_root_id": msg.thread_root_id.as_ref().map(|t| t.as_str().to_string()),
            }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        };
        let ctx = EmitContext {
            event_id,
            tenant: myelin_tenancy::TenantId(msg.conv.tenant.clone()),
            region: myelin_tenancy::Region(msg.conv.region.clone()),
            actor,
            schema_ver: 1,
            occurred_at: occurred,
            recorded_at: recorded,
            caused_by: None,
        };
        Ok(derive_envelope(draft, ctx, None))
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

        let mut out: Vec<Message> = rows.iter().map(row_to_message).collect();
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
        Ok(rows.iter().map(row_to_message).collect())
    }

    pub async fn revise(
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

    pub async fn tombstone(
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

fn row_to_message(r: &sqlx::postgres::PgRow) -> Message {
    Message {
        message_id: MessageId(r.get::<String, _>("message_id")),
        conv: ConversationId {
            tenant: r.get::<String, _>("tenant_id"),
            region: r.get::<String, _>("region"),
            conversation_id: r.get::<String, _>("conversation_id"),
        },
        thread_root_id: r.get::<Option<String>, _>("thread_root_id").map(MessageId),
        author: r.get::<String, _>("author"),
        author_kind: author_kind_from_code(r.get::<i16, _>("author_kind") as u8),
        body_inline: r.get::<Vec<u8>, _>("body_inline"),
        body_nodes: r.get::<Vec<u8>, _>("body_nodes"),
        client_nonce: r.get::<String, _>("client_nonce"),
        edited_seq: r.get::<i32, _>("edited_seq"),
        state: state_from_code(r.get::<i16, _>("state") as u8),
    }
}

fn author_kind_code(k: AuthorKind) -> u8 {
    match k {
        AuthorKind::Human => 0,
        AuthorKind::Agent => 1,
        AuthorKind::Service => 2,
    }
}

fn author_kind_from_code(c: u8) -> AuthorKind {
    match c {
        1 => AuthorKind::Agent,
        2 => AuthorKind::Service,
        _ => AuthorKind::Human,
    }
}

fn state_from_code(c: u8) -> MessageState {
    match c {
        1 => MessageState::Edited,
        2 => MessageState::Deleted,
        3 => MessageState::Tombstoned,
        _ => MessageState::Active,
    }
}

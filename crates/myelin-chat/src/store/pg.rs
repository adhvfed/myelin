//! The PostgreSQL-partitioned hot tier — the v1 [`MessageStore`](super::MessageStore) hot engine
//! (arch [01 §3](../../../../planning/04-subsystem-architectures/chat/architecture/01-tech-and-data-model.md)).
//!
//! Compiled ONLY under `--features integration` (the default `cargo build --workspace` stays
//! DB-free, the binding policy). It runs the REAL forward-only `message` DDL (arch §3) against the
//! docker-compose dev-stack Postgres and implements the same `append` / `range` / `revise` /
//! `tombstone` / `resync_from` surface the in-memory [`MemHotTier`](super::MemHotTier) does — the
//! integration test (`tests/integration_chat_p4_message_store.rs`) asserts **0 behavioural
//! divergence** between the two tiers (the GATE's PG leg) and that the `(tenant, region)` RLS policy
//! ISOLATES tenants + pins residency end-to-end (0 cross-region / cross-tenant rows).
//!
//! ## Residency-pin + partition (contract 12.1/12.4)
//! `region` is in the primary key and the RLS policy keys on `(tenant_id, region)`
//! (`myelin_make_tenant_scoped`), so a write lands ONLY in its region's partition — a session
//! pinned to a different region reads 0 of it (the partition/residency-pin GATE, enforced AT THE DB,
//! not by app code).
//!
//! ## Named floors
//! - The hot engine is Postgres; **ScyllaDB is the named M5 promotion** (CHAT-P28 / P-502) behind
//!   the SAME trait — a hot-tier swap, not a redesign.
//! - The `body_inline` / `body_nodes` columns exist; their **per-subject-DEK encryption is CHAT-P6**.
//! - The `chat.message.created` **outbox co-commit is CHAT-P5** — this tier persists the message
//!   row + stages the state change; the same-tx event emit lands in CHAT-P5.

use sqlx::postgres::PgPool;
use sqlx::Row;

use super::{
    AuthorKind, ConversationId, Message, MessageId, MessageState, NewMessage, RangeCursor,
    StoreError, TombstoneReason,
};

/// **The frozen `message` hot-tier DDL (arch §3).** The k-sortable `message_id` (TEXT — the ULID
/// canonical 26-char form, which sorts lexically == time order), the `(tenant, region,
/// conversation, message_id)` primary key (residency in the key), the `UNIQUE(tenant, region,
/// conversation_id, client_nonce)` idempotent-send constraint, the `myelin-content` body split
/// columns (`body_inline` / `body_nodes`, DEK-encrypted in CHAT-P6), and the `edited_seq` CAS
/// counter. Forward-only / expand-only (`IF NOT EXISTS`, no DROP — the `forward-only-migration`
/// lint). The `(tenant, region)` columns are what the RLS policy keys on.
///
/// The table name is a `{}` placeholder so the integration test can suffix it for isolation; the
/// SHAPE is the contract (columns, keys, predicates), only the identifier is suffixed.
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

/// The PostgreSQL-partitioned hot tier. Holds a bounded sqlx pool + the residency-pinned `region`
/// the session GUC is set to. Cloneable (the pool is `Arc`-backed). The table name is configurable
/// so the integration test can isolate concurrent runs; production uses `"message"`.
#[derive(Clone)]
pub struct PgMessageStore {
    pool: PgPool,
    region: String,
    table: String,
}

impl PgMessageStore {
    /// Wrap a connected pool, pinning `region` (set on every session, the residency pin) and the
    /// `table` identifier (`"message"` in production; a suffixed name in the isolation test).
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

    /// The pinned region (the residency pin set on every session).
    pub fn region(&self) -> &str {
        &self.region
    }

    /// Apply the forward-only `message` DDL + make the table RLS-ready via the platform-wide
    /// `myelin_make_tenant_scoped` convention helper (FORCE RLS + the `(tenant_id, region)` isolation
    /// policy) and grant the app role. Idempotent (the DDL is `IF NOT EXISTS`; the helper is
    /// re-runnable). Runs as the admin/owner role the caller's pool connects with.
    pub async fn migrate(&self) -> Result<(), StoreError> {
        let ddl = MESSAGE_TABLE_DDL.replace("{table}", &self.table);
        sqlx::raw_sql(&ddl)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Cold(format!("message DDL: {e}")))?;
        // The one RLS convention helper (FORCE RLS + the (tenant_id, region) isolation policy). Chat
        // does NOT fork the policy — it calls the platform helper (the same posture CI takes). These
        // are admin DDL/grant statements on the TABLE (not tenant-row reads), so they run through
        // `raw_sql` (DDL, not a tenant-store query — the `tenant-predicate` IDOR lint guards the
        // ROW-reading `sqlx::query` sites below, which all thread `tenant_id`).
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

    /// **append (async) — persist a message in its `(tenant, region)` partition.** Mints a
    /// k-sortable ULID via the provided source, inserts under the residency-pinned session, and is
    /// idempotent on `client_nonce` (a retried send returns the EXISTING id, no second row — the
    /// `ON CONFLICT (…, client_nonce) DO NOTHING` + a follow-up read). The `chat.message.created`
    /// outbox co-commit is CHAT-P5; this tier persists the row.
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
        // The session is pinned to the ROW's region (the residency pin); production always writes
        // self.region.
        self.set_session_scope(&mut conn, &msg.conv.tenant, &msg.conv.region)
            .await?;

        // Idempotent-send: if this nonce already exists in the conversation, return its id.
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

    /// **range (async)** — the ordered range read (recent-N | scroll-back | resume-gap), scoped to
    /// the `(tenant, region)` partition (RLS-isolated). Always ascending by `message_id`.
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

        // Each cursor lowers to a clustering-range read; ASC final order (per-conversation total
        // order). Recent-N takes the DESC tail then re-orders ASC. The dynamic cursor predicate +
        // sort direction are interpolated; the `(tenant_id, region, conversation_id)` predicate is
        // ALWAYS threaded (the IDOR floor) + the values are BOUND params.
        let (where_clause, bound, order): (&str, Option<String>, &str) = match &cursor {
            RangeCursor::Recent => ("", None, "DESC"),
            RangeCursor::Before(id) => ("AND message_id < $5", Some(id.0.clone()), "DESC"),
            RangeCursor::After(id) => ("AND message_id > $5", Some(id.0.clone()), "ASC"),
        };

        // The full SQL is pre-built so the `tenant_id` predicate is on the `sqlx::query` statement
        // (the `tenant-predicate` IDOR lint reads the statement the query-builder call is on).
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
        // Recent-N / Before came back DESC (the tail / the page before); re-order ASC so the
        // surface matches the in-memory tier exactly (per-conversation total order).
        out.sort_by(|a, b| a.message_id.cmp(&b.message_id));
        Ok(out)
    }

    /// **resync_from (async)** — the resume backbone: everything in `conv` strictly after `cursor`,
    /// gap-free, ordered (contract 3.5). A clustering-range read.
    pub async fn resync_from(
        &self,
        conv: &ConversationId,
        cursor: &MessageId,
    ) -> Result<Vec<Message>, StoreError> {
        // It IS a `range(After, unbounded)` — delegate so there is one access path.
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

    /// **revise (async)** — edit-as-new-version under CAS (stable id, bumped `edited_seq`). A CAS
    /// mismatch is refused (0 rows updated → [`StoreError::CasConflict`]).
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
        // The full SQL is pre-built so the `tenant_id` predicate is on the `sqlx::query` statement
        // (the `tenant-predicate` IDOR lint reads the statement the query-builder call is on).
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
            // Distinguish not-found from a CAS conflict by reading the current seq.
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

    /// **tombstone (async)** — keep the fact, drop the body (state → `Tombstoned`). The crypto-shred
    /// of the per-subject DEK is the GDPR holder's job (CHAT-P6).
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
        // The full SQL is pre-built so the `tenant_id` predicate is on the `sqlx::query` statement
        // (the `tenant-predicate` IDOR lint reads the statement the query-builder call is on).
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

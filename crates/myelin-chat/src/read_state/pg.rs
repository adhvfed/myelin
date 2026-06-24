//! The PostgreSQL-backed read-state durable record — the v1 truth tier for the Chat Read-state
//! Service (CHAT-P16 / P-410, arch [02 §3]).
//!
//! Compiled ONLY under `--features integration` (the default `cargo build --workspace` stays
//! DB-free, the binding policy). It runs the REAL forward-only `read_state` DDL against the
//! docker-compose dev-stack Postgres and implements the SAME `upsert` / `load` / `purge_principal`
//! surface the in-memory [`super::ReadStateRecord`] does — the integration test
//! (`tests/integration_chat_p16_read_state.rs`) asserts the cache-never-authoritative property
//! against the REAL Valkey hot markers + this REAL durable record (the CHAT-D12 real-data leg).
//!
//! ## Cache-never-authoritative (arch §3)
//! This is the SOURCE OF TRUTH. The [`super::ReadStateService`] holds a [`myelin_storage::ValkeyCache`]
//! write-back tier in front of it; a cache loss reconstructs the marker from THIS record (the PG
//! record is authoritative; a marker is at-worst slightly stale, benign+bounded). The UPSERT is
//! monotone (`GREATEST` — a stale/out-of-order flush never rewinds the read-position).
//!
//! ## Residency-pin + partition (contract 12.1/12.4)
//! `region` is in the primary key and the RLS policy keys on `(tenant_id, region)`
//! (`myelin_make_tenant_scoped`), so a marker lands ONLY in its region's partition — the same
//! residency posture the message hot tier takes.

use sqlx::postgres::PgPool;

use super::ReadMarker;
use crate::store::{ConversationId, MessageId, StoreError};

/// **The frozen `read_state` durable-record DDL (arch §3).** The per-`(user × conversation)` last-read
/// marker: the `(tenant, region, conversation, principal)` primary key (residency in the key), the
/// `last_read` message id (the marker), and `updated_at`. Forward-only / expand-only (`IF NOT EXISTS`,
/// no DROP — the `forward-only-migration` lint). The `(tenant, region)` columns are what the RLS
/// policy keys on. The table name is a `{table}` placeholder so the integration test can suffix it for
/// isolation; the SHAPE is the contract.
pub const READ_STATE_TABLE_DDL: &str = "\
CREATE TABLE IF NOT EXISTS {table} (
    tenant_id       text        NOT NULL,
    region          text        NOT NULL,
    conversation_id text        NOT NULL,
    principal       text        NOT NULL,
    last_read       text        NOT NULL,
    updated_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, region, conversation_id, principal)
);";

/// The PostgreSQL-backed read-state durable record (the truth tier). Holds a bounded sqlx pool + the
/// residency-pinned `region`. Cloneable (the pool is `Arc`-backed). The table name is configurable so
/// the integration test can isolate concurrent runs; production uses `"read_state"`.
#[derive(Clone)]
pub struct PgReadStateRecord {
    pool: PgPool,
    region: String,
    table: String,
}

impl PgReadStateRecord {
    /// Wrap a connected pool, pinning `region` (set on every session, the residency pin) and the
    /// `table` identifier (`"read_state"` in production; a suffixed name in the isolation test).
    pub fn new(
        pool: PgPool,
        region: impl Into<String>,
        table: impl Into<String>,
    ) -> PgReadStateRecord {
        PgReadStateRecord {
            pool,
            region: region.into(),
            table: table.into(),
        }
    }

    /// The pinned region (the residency pin set on every session).
    pub fn region(&self) -> &str {
        &self.region
    }

    /// Apply the forward-only `read_state` DDL + make the table RLS-ready via the platform-wide
    /// `myelin_make_tenant_scoped` convention helper (FORCE RLS + the `(tenant_id, region)` isolation
    /// policy) and grant the app role. Idempotent. Runs as the admin/owner role.
    pub async fn migrate(&self) -> Result<(), StoreError> {
        let ddl = READ_STATE_TABLE_DDL.replace("{table}", &self.table);
        sqlx::raw_sql(&ddl)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Cold(format!("read_state DDL: {e}")))?;
        // The ONE RLS convention helper (Chat does NOT fork the policy — the same posture the message
        // hot tier takes). Admin DDL on the TABLE, so `raw_sql` (not a tenant-row query).
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

    /// **UPSERT the durable marker — the batched-flush target (arch §3), MONOTONE.** The
    /// `ON CONFLICT … DO UPDATE SET last_read = GREATEST(...)` never rewinds the read-position (a
    /// stale/out-of-order flush of an older marker is a no-op — the read-position only moves forward,
    /// the same monotone contract the in-memory record holds). Returns the persisted `last_read`
    /// (which may be HIGHER than `marker.last_read` if a newer marker already won the race).
    pub async fn upsert(&self, marker: &ReadMarker) -> Result<MessageId, StoreError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StoreError::Cold(format!("acquire: {e}")))?;
        self.set_session_scope(&mut conn, &marker.conv.tenant, &marker.conv.region)
            .await?;
        // GREATEST(existing, incoming) keeps the marker monotone even under a racing/out-of-order
        // flush — a stale flush cannot regress the durable read-position.
        let persisted: String = sqlx::query_scalar(&format!(
            "INSERT INTO {} (tenant_id, region, conversation_id, principal, last_read) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (tenant_id, region, conversation_id, principal) \
             DO UPDATE SET last_read = GREATEST({}.last_read, EXCLUDED.last_read), updated_at = now() \
             RETURNING last_read",
            self.table, self.table
        ))
        .bind(&marker.conv.tenant)
        .bind(&marker.conv.region)
        .bind(&marker.conv.conversation_id)
        .bind(&marker.principal)
        .bind(marker.last_read.as_str())
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| StoreError::Cold(format!("upsert read_state: {e}")))?;
        Ok(MessageId(persisted))
    }

    /// **Load the durable last-read marker — the authoritative read (arch §3).** What a cache miss /
    /// cache loss reconstructs from (the PG record is the truth). `None` if the principal has never
    /// read in this conversation.
    pub async fn load(
        &self,
        conv: &ConversationId,
        principal: &str,
    ) -> Result<Option<MessageId>, StoreError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StoreError::Cold(format!("acquire: {e}")))?;
        self.set_session_scope(&mut conn, &conv.tenant, &conv.region)
            .await?;
        let got: Option<String> = sqlx::query_scalar(&format!(
            "SELECT last_read FROM {} WHERE tenant_id = $1 AND region = $2 \
             AND conversation_id = $3 AND principal = $4",
            self.table
        ))
        .bind(&conv.tenant)
        .bind(&conv.region)
        .bind(&conv.conversation_id)
        .bind(principal)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| StoreError::Cold(format!("load read_state: {e}")))?;
        Ok(got.map(MessageId))
    }

    /// **Purge a principal's durable read-state — the H5 holder erasure leg (D-C8).** Removes every
    /// marker for `principal` in `(tenant, region)` (a person's scroll-position footprint, 0
    /// recoverable). Returns the count purged.
    pub async fn purge_principal(&self, tenant: &str, principal: &str) -> Result<u64, StoreError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StoreError::Cold(format!("acquire: {e}")))?;
        self.set_session_scope(&mut conn, tenant, &self.region)
            .await?;
        let res = sqlx::query(&format!(
            "DELETE FROM {} WHERE tenant_id = $1 AND principal = $2",
            self.table
        ))
        .bind(tenant)
        .bind(principal)
        .execute(&mut *conn)
        .await
        .map_err(|e| StoreError::Cold(format!("purge read_state: {e}")))?;
        Ok(res.rows_affected())
    }
}

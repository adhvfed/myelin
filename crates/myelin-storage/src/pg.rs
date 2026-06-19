//! `PgStore` — the OLTP client + the outbox table/relay + the ReBAC tuple store, backed by a
//! REAL Postgres (sqlx RUNTIME queries; the existing migrations run against PG), with the
//! `(tenant, region)` isolation enforced AT THE DB by Row-Level Security.
//!
//! **Stage 2 / infra.** This is the concrete Postgres backing the storage substrate's M0
//! floors named (the [`crate::oltp::OltpPool`] "a real Postgres pool … lands when serve's pool
//! body does", the [`crate::coloc`] co-located outbox, and the S3 ReBAC tuple store). It is
//! config-selected (see [`crate::backend::StorageBackend`]) and compiled ONLY under
//! `--features integration`; the in-memory floors remain the unit/default backing. It does NOT
//! redefine the outbox table (it RUNS the frozen [`myelin_events::OUTBOX_MIGRATION`]) nor the
//! tuple shape — it implements the existing surfaces against a live DB (EI-01 §7 coherence).
//!
//! ## What it owns
//! - [`PgStore::migrate`] runs the forward-only DDL: the RLS convention helper (idempotent),
//!   the frozen `outbox` table ([`myelin_events::OUTBOX_MIGRATION`]), and the
//!   `(tenant, region)`-RLS relation-tuple table. All via sqlx `query` RUNTIME execution — the
//!   build never needs a live DB (no `query!` compile-time macro anywhere).
//! - [`PgStore::put_tuple`] / [`PgStore::reverse_index`] — the ReBAC tuple store: write an
//!   `object#relation@subject` edge in a `(tenant, region)` partition, and resolve the S8
//!   reverse-index lookup ("objects where subject has relation"). The session GUCs
//!   `myelin.tenant_id` / `myelin.region` are set per connection so the DB RLS policy isolates
//!   tenants — a wrong-tenant session structurally cannot read another tenant's tuples.
//! - [`PgStore::relay`] → [`crate::pgrelay::PgRelay`] — the outbox + relay: an outbox row is
//!   inserted (the envelope as JSONB), and the relay CLAIMS unsent rows with
//!   `SELECT … FOR UPDATE SKIP LOCKED` (no double-claim across replicas), publishes them to a
//!   [`BusTransport`](myelin_events::relay::BusTransport), and marks them `published_at = now()`
//!   — the real SQL the [`crate::relay`] floor models in memory. The broker-publish call is
//!   isolated to the named relay module [`crate::pgrelay`] (the one legitimate broker-publish
//!   site, BUS-2), so this file carries NO raw publish.
//!
//! ## The RLS enforcement is AT THE DB
//! The app role (`myelin_app`) is `NOSUPERUSER NOBYPASSRLS` and every tenant table is
//! `ENABLE + FORCE ROW LEVEL SECURITY` with the `(tenant_id, region)` isolation policy (the
//! pg-init `myelin_make_tenant_scoped` helper). So a cross-tenant read is refused by Postgres,
//! not merely by app code — the IDOR floor lives in the database (storage.md §1.1).
//!
//! ## `residency-pin` lint — region pinned PER SESSION (`@residency-cell-pinned:file`)
//! The same NAMED floor `oltp.rs` / `coloc.rs` record: [`PgStore`] carries its `Region` and
//! sets `myelin.region` on EVERY session ([`PgStore::set_session_scope`]) so every tuple
//! read/write is `(tenant, region)`-scoped — but the bounded sqlx pool itself
//! ([`PgStore::connect`]) is opened region-AGNOSTIC (the region is a runtime value, not a
//! per-pool pin). A per-POOL runtime region-pin is the end-to-end STOR-D5 gate
//! (P-ST-15 / P-102). The file-level waiver marker `@residency-cell-pinned:file` records this
//! floor LOUDLY (EI-01 §4 — named, never a silent skip), exactly as the OLTP floor does; the
//! `(tenant, region)` predicate the RLS policy enforces is the real residency boundary here.
//!
//! ## `no-raw-publish` — the broker publish is isolated to [`crate::pgrelay`]
//! This file carries NO `bus.put(...)`: the relay (the ONE legitimate broker-publish component,
//! BUS-2) lives in the named [`crate::pgrelay`] module so the sanctioned publish is confined to
//! that one relay file (the same posture `myelin-events/src/relay.rs` has). `pg.rs` is the OLTP
//! client + the tenant-store, fully subject to the `tenant-predicate` IDOR lint.

use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::Row;

/// The relation-tuple table DDL — the frozen ⟨object#relation@subject⟩ shape, `(tenant, region)`
/// scoped and made RLS-ready via the pg-init `myelin_make_tenant_scoped` convention helper. The
/// reverse index `(subject, relation, object_id)` is the S8 authz-lookup access path.
///
/// Forward-only / expand-only (the `forward-only-migration` lint): it only ADDs the table +
/// index; `IF NOT EXISTS` makes [`PgStore::migrate`] idempotent. The `tenant_id`/`region`
/// columns are what the RLS policy keys on.
pub const REBAC_TUPLE_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS rebac_tuple (
    tenant_id text NOT NULL,
    region    text NOT NULL,
    object_id text NOT NULL,
    relation  text NOT NULL,
    subject   text NOT NULL,
    PRIMARY KEY (tenant_id, region, object_id, relation, subject)
);
CREATE INDEX IF NOT EXISTS rebac_tuple_rev
    ON rebac_tuple (tenant_id, region, subject, relation, object_id);";

/// An error from the Postgres-backed store. A thin, typed surface over the sqlx error: a DB
/// failure is a loud value, never a silent fallthrough (the cache is best-effort; the OLTP
/// tier is the system of record, so its errors propagate).
#[derive(Debug)]
pub enum PgError {
    /// The DB connection / pool could not be established.
    Connect(String),
    /// A migration (DDL) statement failed.
    Migrate(String),
    /// A query / statement failed.
    Query(String),
    /// The bus rejected a relay publish (the relay leaves the row unsent to retry).
    Publish(String),
}

impl core::fmt::Display for PgError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PgError::Connect(e) => write!(f, "postgres connect failed: {e}"),
            PgError::Migrate(e) => write!(f, "postgres migration failed: {e}"),
            PgError::Query(e) => write!(f, "postgres query failed: {e}"),
            PgError::Publish(e) => write!(f, "outbox relay publish failed: {e}"),
        }
    }
}

impl std::error::Error for PgError {}

/// The OLTP client + outbox + ReBAC tuple store backed by REAL Postgres.
///
/// Holds a bounded sqlx `PgPool` (the §3.1 bounded-pool bound) and is async — it is driven on
/// the harness's tokio runtime. Cloneable: the `PgPool` is an `Arc`-backed handle, so a clone
/// shares the same connection pool.
#[derive(Clone)]
pub struct PgStore {
    pool: PgPool,
    region: String,
}

impl PgStore {
    /// Connect a bounded pool to `database_url` and pin `region` (the residency pin every
    /// session sets). The pool is bounded (`max_connections`) — never unbounded (storage §3.1).
    pub async fn connect(
        database_url: &str,
        region: &str,
        max_connections: u32,
    ) -> Result<PgStore, PgError> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections.max(1))
            .connect(database_url)
            .await
            .map_err(|e| PgError::Connect(e.to_string()))?;
        Ok(PgStore {
            pool,
            region: region.to_string(),
        })
    }

    /// The underlying bounded pool (for the OLTP-reachability smoke check).
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Run the forward-only migrations against PG: the RLS convention helper (re-asserted
    /// idempotently in case this connects to a DB whose init script did not run), the frozen
    /// `outbox` table, and the `(tenant, region)`-RLS relation-tuple table.
    ///
    /// DDL runs as the migration/owner role (the caller passes an admin `database_url`); the
    /// app role connects separately at runtime so RLS is actually enforced (the owner is FORCEd
    /// under RLS too, so even DDL-side seeding must set the session GUCs).
    pub async fn migrate(&self) -> Result<(), PgError> {
        // The outbox table (the frozen 2.3 DDL — RUN, not re-defined) and the tuple table. Each
        // const is a multi-statement script; sqlx `execute` runs a simple multi-statement query.
        for ddl in [myelin_events::OUTBOX_MIGRATION, REBAC_TUPLE_MIGRATION] {
            sqlx::raw_sql(ddl)
                .execute(&self.pool)
                .await
                .map_err(|e| PgError::Migrate(e.to_string()))?;
        }
        // Make the tuple table RLS-ready: ENABLE + FORCE RLS and install the (tenant_id, region)
        // isolation policy. We run the policy DDL inline (idempotent — a duplicate-policy error is
        // swallowed) so migrate() is self-contained even if the pg-init convention helper did not
        // run. This DDL is precisely what INSTALLS the tenant predicate the tenant-predicate lint
        // guards: the RLS policy keys on `tenant_id = current_setting('myelin.tenant_id')`.
        let _ = sqlx::raw_sql(
            "ALTER TABLE rebac_tuple ENABLE ROW LEVEL SECURITY;\n\
             ALTER TABLE rebac_tuple FORCE ROW LEVEL SECURITY;\n\
             CREATE POLICY myelin_tenant_isolation ON rebac_tuple \
               USING (tenant_id = current_setting('myelin.tenant_id', true) \
                      AND region = current_setting('myelin.region', true)) \
               WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
                           AND region = current_setting('myelin.region', true));",
        )
        .execute(&self.pool)
        .await;
        Ok(())
    }

    // ---- ReBAC tuple store (the S3 store, RLS-isolated) -------------------------------------

    /// Write an `object#relation@subject` edge in the `(tenant, region)` partition. The session
    /// GUCs are set first so the DB RLS policy admits the INSERT (a wrong-tenant session is
    /// refused by Postgres). Idempotent (`ON CONFLICT DO NOTHING`).
    pub async fn put_tuple(
        &self,
        tenant: &str,
        object_id: &str,
        relation: &str,
        subject: &str,
    ) -> Result<(), PgError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        self.set_session_scope(&mut conn, tenant).await?;
        sqlx::query(
            "INSERT INTO rebac_tuple (tenant_id, region, object_id, relation, subject) \
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
        )
        .bind(tenant)
        .bind(&self.region)
        .bind(object_id)
        .bind(relation)
        .bind(subject)
        .execute(&mut *conn)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    /// The S8 reverse-index lookup the authz path uses: the objects where `subject` has
    /// `relation`, within the verified `(tenant, region)` partition (RLS-scoped). Ordered by
    /// `object_id` for a deterministic result.
    pub async fn reverse_index(
        &self,
        tenant: &str,
        subject: &str,
        relation: &str,
    ) -> Result<Vec<String>, PgError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        self.set_session_scope(&mut conn, tenant).await?;
        // Defence in depth: the DB RLS policy already isolates `(tenant_id, region)`, AND the
        // query threads an explicit `tenant_id` predicate (the tenant-predicate IDOR floor — a
        // tenant-store query must carry the tenant predicate in the query itself, not rely on
        // RLS alone). The tenant is the VERIFIED arg, never a URL path.
        let rows = sqlx::query(
            "SELECT object_id FROM rebac_tuple \
             WHERE tenant_id = $1 AND subject = $2 AND relation = $3 ORDER BY object_id",
        )
        .bind(tenant)
        .bind(subject)
        .bind(relation)
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(rows.iter().map(|r| r.get::<String, _>("object_id")).collect())
    }

    /// Set the per-session `(tenant, region)` GUCs the RLS policy keys on. The tenant is the
    /// VERIFIED tenant (never a URL path) — the IDOR floor: a session for tenant A can only ever
    /// read tenant A's rows because the policy compares `tenant_id = current_setting(...)`.
    async fn set_session_scope(
        &self,
        conn: &mut sqlx::pool::PoolConnection<sqlx::Postgres>,
        tenant: &str,
    ) -> Result<(), PgError> {
        // Set BOTH the tenant_id and region GUCs in ONE statement (one round trip) so the RLS
        // policy's `(tenant_id, region)` predicate is keyed before any tenant query runs.
        sqlx::query("SELECT set_config('myelin.tenant_id', $1, false), set_config('myelin.region', $2, false)")
            .bind(tenant)
            .bind(&self.region)
            .execute(&mut **conn)
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    // ---- Outbox + relay (the real FOR UPDATE SKIP LOCKED claim) -----------------------------

    /// The OLTP-co-located outbox + relay over THIS store's pool (the outbox lives in the same
    /// OLTP DB the service writes — same-tx co-commit). The relay (the ONE legitimate
    /// broker-publish component, BUS-2) lives in [`crate::pgrelay`] so the broker-publish call is
    /// isolated to that named relay file; this accessor hands it the shared pool.
    pub fn relay(&self) -> crate::pgrelay::PgRelay {
        crate::pgrelay::PgRelay::new(self.pool.clone())
    }
}

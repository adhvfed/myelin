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
//!   reverse-index lookup ("objects where subject has relation"). The `(tenant, region)` GUCs
//!   `myelin.tenant_id` / `myelin.region` are set TRANSACTION-scoped (`set_config(..., true)`, the
//!   MR-022 [`crate::tenant_tx::with_tenant_tx`] convention) so the DB RLS policy isolates tenants —
//!   a wrong-tenant transaction structurally cannot read another tenant's tuples, and the GUC is
//!   discarded at COMMIT so NO tenant identity bleeds across a pooled checkout (the SI-005 fix).
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
//! ## `residency-pin` lint — region pinned PER TRANSACTION + fail-fast (`@residency-cell-pinned:file`)
//! The same NAMED floor `oltp.rs` / `coloc.rs` record: [`PgStore`] carries its `Region` and sets
//! `myelin.region` TRANSACTION-scoped on every tenant op (the `with_tenant_tx` convention) so every
//! tuple read/write is `(tenant, region)`-scoped. MR-013 adds **region fail-fast**: [`PgStore::connect`]
//! REFUSES a blank region (no region-less pool is ever opened) and every tenant-scoped entry point
//! re-checks (belt-and-suspenders), and the pool is now built through
//! [`crate::tenant_tx::connect_pool_with_reset`] so each connection is tagged
//! `application_name = myelin:<region>` and scrubbed with `RESET ALL` on release. The mTLS half of
//! the region pin (peer-cert ⇄ region binding) genuinely belongs to the runtime transport layer and
//! is deferred there. The file-level waiver marker `@residency-cell-pinned:file` records this floor
//! LOUDLY (EI-01 §4 — named, never a silent skip); the `(tenant, region)` predicate the RLS policy
//! enforces is the real residency boundary here.
//!
//! ## `no-raw-publish` — the broker publish is isolated to [`crate::pgrelay`]
//! This file carries NO `bus.put(...)`: the relay (the ONE legitimate broker-publish component,
//! BUS-2) lives in the named [`crate::pgrelay`] module so the sanctioned publish is confined to
//! that one relay file (the same posture `myelin-events/src/relay.rs` has). `pg.rs` is the OLTP
//! client + the tenant-store, fully subject to the `tenant-predicate` IDOR lint.

use sqlx::postgres::PgPool;
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
    /// A round-trip counter incremented once per authz query the ReBAC store issues
    /// ([`check_tuple`](PgStore::check_tuple) / [`list_objects`](PgStore::list_objects) /
    /// [`reverse_index`](PgStore::reverse_index)). It is the instrument the stage-3
    /// ReBAC-NO-LEAK/NO-N+1 drill reads to PROVE `list_objects` is ONE reverse-index query for the
    /// whole visible set — not one `check` per candidate object (the N+1 anti-pattern). Shared
    /// across clones (an `Arc`) so a cloned handle observes the same count.
    authz_queries: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl PgStore {
    /// Connect a bounded pool to `database_url` and pin `region` (the residency pin every
    /// session sets). The pool is bounded (`max_connections`) — never unbounded (storage §3.1).
    pub async fn connect(
        database_url: &str,
        region: &str,
        max_connections: u32,
    ) -> Result<PgStore, PgError> {
        // **Region fail-fast (residency pin; P-531 / STOR-D5).** A blank region is never a valid
        // residency boundary — refuse it LOUDLY at construction rather than open a region-less pool
        // whose every tenant-scoped op would silently run with an empty `myelin.region` GUC (an op
        // that matches nothing, or — worse, if the policy were ever relaxed — everything). The mTLS
        // half of the region pin (peer-cert ⇄ region binding) genuinely belongs to the runtime
        // transport layer and is deferred there; this is the cheap, real, in-process part.
        if region.trim().is_empty() {
            return Err(PgError::Connect(
                "region pin is empty — refusing to open a region-less OLTP pool (residency \
                 fail-fast, P-531 / STOR-D5)"
                    .to_string(),
            ));
        }
        // Build the bounded pool through the MR-022 reset-on-release helper
        // ([`crate::tenant_tx::connect_pool_with_reset`]): every connection is tagged with its
        // residency region (`application_name = myelin:<region>`) and scrubbed with `RESET ALL` on
        // release (defence-in-depth against any session GUC residue). Combined with the
        // TRANSACTION-scoped `(tenant, region)` GUC every tenant op now sets (the MR-022
        // `with_tenant_tx` convention), no tenant identity can bleed across a pooled checkout — the
        // structural SI-005 fix MR-013 lands.
        let pool =
            crate::tenant_tx::connect_pool_with_reset(database_url, region, max_connections).await?;
        Ok(PgStore {
            pool,
            region: region.to_string(),
            authz_queries: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        })
    }

    /// The number of authz round-trips this store has issued (the N+1 instrument; see
    /// [`authz_queries`](PgStore#structfield.authz_queries)). The stage-3 ReBAC drill snapshots
    /// this before `list_objects` and asserts the delta is exactly 1 (one reverse-index query for
    /// the whole visible set — no per-candidate `check` fan-out).
    pub fn authz_query_count(&self) -> u64 {
        self.authz_queries.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// **OLTP-reachability health check (the SI-005 replacement for the former bare `pool()` hatch).**
    /// Runs a trivial `SELECT 1` over the bounded pool and returns `Ok(())` iff the OLTP tier answers
    /// — the reachability probe the smoke check needs WITHOUT handing out the raw `&PgPool`. The old
    /// `pub fn pool(&self) -> &PgPool` let any caller `.acquire()` a connection that bypassed the
    /// tenant-scoped RLS path (the bare-hatch leg of census SI-005); it is GONE. Every tenant op
    /// routes through the `(tenant, region)` transaction-scoped convention
    /// ([`crate::tenant_tx::with_tenant_tx`] / [`Self::scoped_conn`]); there is deliberately no
    /// raw-pool accessor on this tenant store.
    pub async fn health_check(&self) -> Result<(), PgError> {
        use sqlx::Connection;
        // A server round-trip liveness PING (not a query-builder call) — proves the OLTP tier
        // answers WITHOUT issuing any tenant-store query, so the `tenant-predicate` IDOR floor stays
        // fully live over pg.rs (a `SELECT 1` here would be a tenant-less query the floor rejects).
        // The connection touches no tenant data and returns to the pool scrubbed (reset-on-release).
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        conn.ping()
            .await
            .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    /// Region fail-fast for a tenant-scoped op (residency pin; P-531 / STOR-D5). `connect` already
    /// refuses a blank `self.region`, so this is belt-and-suspenders: an op never runs region-less.
    fn ensure_region(&self) -> Result<(), PgError> {
        if self.region.trim().is_empty() {
            return Err(PgError::Query(
                "region pin is empty — refusing a region-less tenant-scoped op (residency \
                 fail-fast, P-531 / STOR-D5)"
                    .to_string(),
            ));
        }
        Ok(())
    }

    /// Run the forward-only migrations against PG: the RLS convention helper (re-asserted
    /// idempotently in case this connects to a DB whose init script did not run), the frozen
    /// `outbox` table, and the `(tenant, region)`-RLS relation-tuple table.
    ///
    /// DDL runs as the migration/owner role (the caller passes an admin `database_url`); the
    /// app role connects separately at runtime so RLS is actually enforced (the owner is FORCEd
    /// under RLS too, so even DDL-side seeding must set the session GUCs).
    pub async fn migrate(&self) -> Result<(), PgError> {
        // Route the forward-only DDL through the race-safe [`crate::pg_migrator::PgMigrator`] (the
        // P-S12 driver): each statement is one Migration with a STABLE id, applied under a Postgres
        // session advisory lock + recorded in `myelin_applied_migration`. This SERIALIZES concurrent
        // migrate() across processes/tests, fixing the `pg_type_typname_nsp_index` race the bare
        // `raw_sql(ddl).execute(&pool)` loop had, and makes re-runs idempotent (an already-applied id
        // is SKIPPED, never re-run — so the RLS-policy CREATE, which is NOT `IF NOT EXISTS`-able,
        // runs exactly once and never errors on a second migrate()).
        //
        // The RLS-policy migration INSTALLS the tenant predicate the tenant-predicate lint guards:
        // `tenant_id = current_setting('myelin.tenant_id')` (+ region). Its idempotency is now the
        // version table's job (the id is recorded once), not a swallowed duplicate-policy error.
        let migrations = crate::migration::Migrations::of([
            crate::migration::Migration::plain("0001_outbox", myelin_events::OUTBOX_MIGRATION),
            crate::migration::Migration::plain("0002_rebac_tuple", REBAC_TUPLE_MIGRATION),
            crate::migration::Migration::plain(
                "0003_rebac_rls_policy",
                // `DROP POLICY IF EXISTS` makes the CREATE idempotent against a DB that already
                // carries the policy (e.g. one migrated before the version table existed, or seeded
                // by the pg-init `myelin_make_tenant_scoped` helper). It drops only a POLICY, never a
                // table/column, so it is forward-only-legal (`is_destructive` is false). Under the
                // advisory lock this whole script runs serialized + exactly once (recorded).
                "ALTER TABLE rebac_tuple ENABLE ROW LEVEL SECURITY;\n\
                 ALTER TABLE rebac_tuple FORCE ROW LEVEL SECURITY;\n\
                 DROP POLICY IF EXISTS myelin_tenant_isolation ON rebac_tuple;\n\
                 CREATE POLICY myelin_tenant_isolation ON rebac_tuple \
                   USING (tenant_id = current_setting('myelin.tenant_id', true) \
                          AND region = current_setting('myelin.region', true)) \
                   WITH CHECK (tenant_id = current_setting('myelin.tenant_id', true) \
                               AND region = current_setting('myelin.region', true));",
            ),
        ]);
        crate::pg_migrator::PgMigrator::apply(&self.pool, &migrations).await
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
        self.ensure_region()?;
        let region = self.region.clone();
        let (tenant_owned, object_id, relation, subject) = (
            tenant.to_string(),
            object_id.to_string(),
            relation.to_string(),
            subject.to_string(),
        );
        // Route through the MR-022 convention: acquire → BEGIN → SET LOCAL `(tenant, region)` →
        // INSERT → COMMIT. The GUC is transaction-scoped, so it is discarded on commit and no tenant
        // identity bleeds onto the returned pooled connection (the SI-005 fix).
        crate::tenant_tx::with_tenant_tx(&self.pool, tenant, &self.region, move |conn| {
            Box::pin(async move {
                Self::insert_tuple_on_conn(
                    conn,
                    &tenant_owned,
                    &region,
                    &object_id,
                    &relation,
                    &subject,
                )
                .await
            })
        })
        .await
    }

    /// **The residency write-boundary probe (P-CP-12 / CP-D3 / STOR-D5 — layer 3 at the LIVE DB).**
    /// Attempt to write a tuple whose **row `region` column** is `row_region` while the session
    /// (the cell) is pinned to `self.region`. This is the cross-region-egress attempt the
    /// four-layer enforcement (§5.3 layer 3) forbids: a write where `row.region ≠ cell.region`.
    /// When `row_region != self.region` the DB's `WITH CHECK (region = current_setting(...))` RLS
    /// policy **REJECTS** the INSERT — proving the residency write boundary is enforced by Postgres,
    /// not by app code (an out-of-region write never lands; 0 cross-region rows are admitted). When
    /// `row_region == self.region` the write is admitted (the green leg).
    ///
    /// This is a DELIBERATE out-of-region probe (the row region is decoupled from the session
    /// region), so it lives behind an explicit, loudly-named method rather than in the normal
    /// [`Self::put_tuple`] path (which always writes `self.region` — the cell's region the harness
    /// threaded — so the production write path is structurally region-pinned). The drill calls this
    /// to FORCE the boundary to fire (a gate that cannot go red is not a gate, EI-01 §3).
    pub async fn put_tuple_in_region(
        &self,
        tenant: &str,
        row_region: &str,
        object_id: &str,
        relation: &str,
        subject: &str,
    ) -> Result<(), PgError> {
        self.ensure_region()?;
        let (tenant_owned, row_region, object_id, relation, subject) = (
            tenant.to_string(),
            row_region.to_string(),
            object_id.to_string(),
            relation.to_string(),
            subject.to_string(),
        );
        // The cell SESSION is pinned to self.region (the transaction-scoped GUC); the ROW carries
        // row_region. When they differ, the RLS `WITH CHECK (region = current_setting('myelin.region',
        // true))` predicate fails and Postgres refuses the INSERT (a 42501 / row-violates-policy
        // error) — the op returns Err and the transaction rolls back. Now TRANSACTION-scoped: no
        // session GUC bleed.
        crate::tenant_tx::with_tenant_tx(&self.pool, tenant, &self.region, move |conn| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO rebac_tuple (tenant_id, region, object_id, relation, subject) \
                     VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
                )
                .bind(&tenant_owned)
                .bind(&row_region)
                .bind(&object_id)
                .bind(&relation)
                .bind(&subject)
                .execute(&mut *conn)
                .await
                .map_err(|e| PgError::Query(e.to_string()))?;
                Ok(())
            })
        })
        .await
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
        self.ensure_region()?;
        // Defence in depth: the DB RLS policy already isolates `(tenant_id, region)`, AND the
        // query threads an explicit `tenant_id` predicate (the tenant-predicate IDOR floor — a
        // tenant-store query must carry the tenant predicate in the query itself, not rely on
        // RLS alone). The tenant is the VERIFIED arg, never a URL path.
        self.authz_queries
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (tenant_owned, subject, relation) =
            (tenant.to_string(), subject.to_string(), relation.to_string());
        crate::tenant_tx::with_tenant_tx(&self.pool, tenant, &self.region, move |conn| {
            Box::pin(async move {
                let rows = sqlx::query(
                    "SELECT object_id FROM rebac_tuple \
                     WHERE tenant_id = $1 AND subject = $2 AND relation = $3 ORDER BY object_id",
                )
                .bind(&tenant_owned)
                .bind(&subject)
                .bind(&relation)
                .fetch_all(&mut *conn)
                .await
                .map_err(|e| PgError::Query(e.to_string()))?;
                Ok(rows
                    .iter()
                    .map(|r| r.get::<String, _>("object_id"))
                    .collect())
            })
        })
        .await
    }

    /// **check (contract 4.2) — the per-action fail-closed gate, one tuple existence query.**
    /// Returns `true` iff the `⟨object#relation@subject⟩` edge exists in the verified
    /// `(tenant, region)` partition. ONE query — never a candidate fan-out. Fail-closed by
    /// construction: an absent edge (or a DB hiccup, which propagates as `Err`) is a DENY, never a
    /// silent allow. Used by the stage-3 ReBAC drill to assert allow/deny correctness.
    pub async fn check_tuple(
        &self,
        tenant: &str,
        object_id: &str,
        relation: &str,
        subject: &str,
    ) -> Result<bool, PgError> {
        self.ensure_region()?;
        self.authz_queries
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (tenant_owned, object_id, relation, subject) = (
            tenant.to_string(),
            object_id.to_string(),
            relation.to_string(),
            subject.to_string(),
        );
        crate::tenant_tx::with_tenant_tx(&self.pool, tenant, &self.region, move |conn| {
            Box::pin(async move {
                let exists: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM rebac_tuple \
                     WHERE tenant_id = $1 AND object_id = $2 AND relation = $3 AND subject = $4)",
                )
                .bind(&tenant_owned)
                .bind(&object_id)
                .bind(&relation)
                .bind(&subject)
                .fetch_one(&mut *conn)
                .await
                .map_err(|e| PgError::Query(e.to_string()))?;
                Ok(exists)
            })
        })
        .await
    }

    /// **list_objects (contract 4.3) — the leak-free pre-filter, ONE reverse-index query.**
    /// Returns EXACTLY the set of objects the `subject` may access under `relation`, in the
    /// verified `(tenant, region)` partition — the S8 reverse-index access path
    /// ([`reverse_index`](PgStore::reverse_index)) `list_objects` lowers to (the `InRelation` /
    /// `TupleSet` JOIN). The leak-free property is structural: the server computes the visible set
    /// from the tuple store and returns ONLY it — an unauthorized object is never a candidate the
    /// caller post-filters (no leak). The NO-N+1 property is structural: this is ONE query for the
    /// WHOLE set, NOT one [`check_tuple`](PgStore::check_tuple) per candidate object — the stage-3
    /// drill snapshots [`authz_query_count`](PgStore::authz_query_count) around this call and
    /// asserts the delta is exactly 1.
    pub async fn list_objects(
        &self,
        tenant: &str,
        subject: &str,
        relation: &str,
    ) -> Result<Vec<String>, PgError> {
        // list_objects IS the S8 reverse-index lookup (one query). Delegating keeps the single
        // access path single — there is deliberately no second, candidate-iterating code path.
        self.reverse_index(tenant, subject, relation).await
    }

    /// **RLS-scoped tenant TRANSACTION (stage-3 (TENANT,REGION)-RLS-ISOLATION drill).** Open a
    /// transaction on a pooled connection and set its `(tenant, region)` GUC TRANSACTION-scoped
    /// (`set_config(..., true)` — the MR-022 convention), then hand the transaction back so the
    /// caller can run reads under THAT scope. The RLS-isolation drill uses this to run a deliberately
    /// tenant-predicate-LESS `SELECT *` and prove the DB's FORCE-RLS policy — not app code — filters
    /// out every other tenant's rows. Because the GUC is transaction-scoped it is discarded at
    /// commit/rollback (when the returned tx is dropped) — NO residual tenant identity bleeds onto
    /// the pooled connection (the SI-005 fix; the former session-scoped `scoped_conn` WAS the bleed).
    /// The unscoped probe query itself lives in the test (under `/tests/`, the home for deliberate
    /// red/probe samples) so `pg.rs` keeps every tenant-store query tenant-bound (the
    /// `tenant-predicate` IDOR lint stays fully live here). A read transaction needs no commit (drop
    /// = rollback); a caller that mutated must `tx.commit().await`.
    pub async fn scoped_conn(
        &self,
        acting_tenant: &str,
    ) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, PgError> {
        self.scoped_conn_in_region(acting_tenant, &self.region).await
    }

    /// **A tenant TRANSACTION scoped to an EXPLICIT `(tenant, region)` (P-CP-12 / STOR-D5 drill seam).**
    /// Like [`Self::scoped_conn`] but pins the transaction GUC to `region` instead of the cell's
    /// `self.region`. The drill uses this to read as a session in a region DIFFERENT from the
    /// tenant's rows and prove the DB returns 0 of them (cross-region read impossible). The
    /// `(tenant, region)` GUC is set TRANSACTION-scoped (`set_config(..., true)`) so it is discarded
    /// at commit/rollback — no session bleed. An empty `region` is refused LOUDLY (residency
    /// fail-fast; P-531 / STOR-D5).
    pub async fn scoped_conn_in_region(
        &self,
        acting_tenant: &str,
        region: &str,
    ) -> Result<sqlx::Transaction<'static, sqlx::Postgres>, PgError> {
        if region.trim().is_empty() {
            return Err(PgError::Query(
                "region pin is empty — refusing a region-less tenant-scoped transaction \
                 (residency fail-fast, P-531 / STOR-D5)"
                    .to_string(),
            ));
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| PgError::Query(format!("begin tenant-scoped transaction: {e}")))?;
        // Set BOTH GUCs in ONE round trip, TRANSACTION-scoped (is_local = true): they live only for
        // this transaction and are discarded at COMMIT/ROLLBACK — the RLS policy's `(tenant, region)`
        // predicate is keyed before any tenant query runs, with no residual scope on the pooled conn.
        sqlx::query(
            "SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)",
        )
        .bind(acting_tenant)
        .bind(region)
        .execute(&mut *tx)
        .await
        .map_err(|e| PgError::Query(format!("set transaction-scoped tenant GUC: {e}")))?;
        Ok(tx)
    }

    /// **The reverse-index lookup under an EXPLICIT region session (P-CP-12 / STOR-D5 drill seam).**
    /// Like [`Self::reverse_index`] but scopes the session to `region`. When `region` differs from
    /// the region the tenant's rows were written in, the DB's `(tenant, region)` RLS policy returns
    /// ZERO rows — proving a cross-region read is impossible (0 cross-region PII egress).
    pub async fn reverse_index_in_region(
        &self,
        tenant: &str,
        region: &str,
        subject: &str,
        relation: &str,
    ) -> Result<Vec<String>, PgError> {
        if region.trim().is_empty() {
            return Err(PgError::Query(
                "region pin is empty — refusing a region-less tenant-scoped read (residency \
                 fail-fast, P-531 / STOR-D5)"
                    .to_string(),
            ));
        }
        self.authz_queries
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // The query threads the tenant predicate (defence in depth); the REGION is enforced by the
        // RLS policy keyed on current_setting('myelin.region', true) — set TRANSACTION-scoped to
        // `region` by the convention. No session GUC bleed.
        let (tenant_owned, subject, relation) =
            (tenant.to_string(), subject.to_string(), relation.to_string());
        crate::tenant_tx::with_tenant_tx(&self.pool, tenant, region, move |conn| {
            Box::pin(async move {
                let rows = sqlx::query(
                    "SELECT object_id FROM rebac_tuple \
                     WHERE tenant_id = $1 AND subject = $2 AND relation = $3 ORDER BY object_id",
                )
                .bind(&tenant_owned)
                .bind(&subject)
                .bind(&relation)
                .fetch_all(&mut *conn)
                .await
                .map_err(|e| PgError::Query(e.to_string()))?;
                Ok(rows
                    .iter()
                    .map(|r| r.get::<String, _>("object_id"))
                    .collect())
            })
        })
        .await
    }

    // ---- Outbox + relay (the real FOR UPDATE SKIP LOCKED claim) -----------------------------

    /// The OLTP-co-located outbox + relay over THIS store's pool (the outbox lives in the same
    /// OLTP DB the service writes — same-tx co-commit). The relay (the ONE legitimate
    /// broker-publish component, BUS-2) lives in [`crate::pgrelay`] so the broker-publish call is
    /// isolated to that named relay file; this accessor hands it the shared pool.
    pub fn relay(&self) -> crate::pgrelay::PgRelay {
        crate::pgrelay::PgRelay::new(self.pool.clone())
    }

    // ---- Conn-bound rebac_tuple ops (the MR-022 with_tenant_tx-convention twins, MR-007) -------
    //
    // These are the TRANSACTION-scoped twins of [`put_tuple`] / [`reverse_index`]: they take an
    // EXISTING `&mut PgConnection` (the one MR-022's [`crate::tenant_tx::with_tenant_tx`] has
    // already opened + `SET LOCAL`-scoped to `(tenant, region)`), so the caller controls the GUC
    // transaction-scoped and no session GUC bleeds. They write/read the SAME `rebac_tuple` table
    // ([`REBAC_TUPLE_MIGRATION`]) under the SAME `myelin_tenant_isolation` FORCE-RLS policy — this
    // is NOT a second tuple store, it is the convention-correct access path the identity-layer
    // `TupleStore` durable binding (MR-007) drives. The legacy acquire-based [`put_tuple`] (which
    // sets the GUC session-scoped) is the path MR-013 reconciles; these twins are already correct.

    /// Insert an `object#relation@subject` edge on a tenant-scoped connection (idempotent;
    /// `ON CONFLICT DO NOTHING`). The `(tenant, region)` GUC must already be SET LOCAL on `conn`
    /// (the [`with_tenant_tx`](crate::tenant_tx::with_tenant_tx) convention); the explicit
    /// `tenant_id`/`region` binds are the defence-in-depth tenant predicate (never a path).
    pub async fn insert_tuple_on_conn(
        conn: &mut sqlx::PgConnection,
        tenant: &str,
        region: &str,
        object_id: &str,
        relation: &str,
        subject: &str,
    ) -> Result<(), PgError> {
        sqlx::query(
            "INSERT INTO rebac_tuple (tenant_id, region, object_id, relation, subject) \
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
        )
        .bind(tenant)
        .bind(region)
        .bind(object_id)
        .bind(relation)
        .bind(subject)
        .execute(&mut *conn)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    /// Delete an `object#relation@subject` edge on a tenant-scoped connection (the `Remove` delta).
    /// RLS + the explicit `(tenant_id, region)` predicate make a cross-tenant delete a no-op.
    pub async fn delete_tuple_on_conn(
        conn: &mut sqlx::PgConnection,
        tenant: &str,
        region: &str,
        object_id: &str,
        relation: &str,
        subject: &str,
    ) -> Result<(), PgError> {
        sqlx::query(
            "DELETE FROM rebac_tuple \
             WHERE tenant_id = $1 AND region = $2 AND object_id = $3 AND relation = $4 \
               AND subject = $5",
        )
        .bind(tenant)
        .bind(region)
        .bind(object_id)
        .bind(relation)
        .bind(subject)
        .execute(&mut *conn)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(())
    }

    /// Read every `(object_id, relation, subject)` edge in the `(tenant, region)` partition on a
    /// tenant-scoped connection (the reverse-index feed / `tuples_in`). Deterministically ordered.
    pub async fn tuples_on_conn(
        conn: &mut sqlx::PgConnection,
        tenant: &str,
        region: &str,
    ) -> Result<Vec<(String, String, String)>, PgError> {
        let rows = sqlx::query(
            "SELECT object_id, relation, subject FROM rebac_tuple \
             WHERE tenant_id = $1 AND region = $2 ORDER BY object_id, relation, subject",
        )
        .bind(tenant)
        .bind(region)
        .fetch_all(&mut *conn)
        .await
        .map_err(|e| PgError::Query(e.to_string()))?;
        Ok(rows
            .iter()
            .map(|r| {
                (
                    r.get::<String, _>("object_id"),
                    r.get::<String, _>("relation"),
                    r.get::<String, _>("subject"),
                )
            })
            .collect())
    }
}

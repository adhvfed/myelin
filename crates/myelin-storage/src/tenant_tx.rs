//! # The tenant-scoped-TRANSACTION connection convention (RESHAPE-002 / MR-022, the SI-005 floor)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §1.1 (the `(tenant, region)`
//! RLS isolation floor) + the MR-006 shape review `RESHAPE-002`.
//!
//! ## Why this exists (the bleed RESHAPE-002 forecloses BEFORE the durable stores bind)
//! The pre-MR-022 pattern ([`crate::pg::PgStore::set_session_scope_in_region`]) runs
//! `set_config('myelin.tenant_id', $1, false)` — `is_local = false` → a **SESSION-scoped** GUC —
//! on a **bare pooled connection with no transaction**. On a pooled connection that GUC survives the
//! checkout, so the NEXT tenant to borrow the same connection inherits the previous tenant's scope:
//! a silent cross-tenant bleed (census SI-005). And the "obvious fix" of flipping `false → true`
//! WITHOUT introducing a transaction is a SILENT NO-OP: `SET LOCAL` / `set_config(..., true)` only
//! lives for the current transaction, and a bare pooled connection has none — RLS would simply not
//! apply (standard Postgres semantics, confirmed in the shape review).
//!
//! ## The convention every durable tenant-scoped store acquires through
//! [`with_tenant_tx`] is the one correct shape: **acquire → BEGIN → set the `(tenant, region)` GUC
//! TRANSACTION-scoped (`set_config(..., true)`, the `SET LOCAL` form) → run the store op on that
//! transaction-bound connection → COMMIT.** Because the GUC is transaction-scoped it is discarded at
//! `COMMIT`/`ROLLBACK`, so a returned pooled connection carries NO residual tenant identity — the
//! bleed is structurally impossible, by construction, for any store that acquires through this helper.
//! [`connect_pool_with_reset`] adds defence-in-depth: an `after_release(RESET ALL)` hook scrubs any
//! session-level GUC residue (e.g. from a code path that did a session `SET`) before a connection
//! returns to the pool — `RESET ALL` resets configuration parameters WITHOUT deallocating prepared
//! statements (so it is safe with sqlx's per-connection statement cache, unlike `DISCARD ALL`).
//!
//! ## Scope (MR-022 vs MR-013)
//! This is the **connection plumbing** the RLS policy needs to be correct — the CONVENTION +
//! MECHANISM. The full RLS hardening sweep (replacing every remaining `set_config(..., false)` in
//! `pg.rs`, the identifier allowlist, mTLS/region fail-fast, driving the `no-bare-tenant-pool`
//! scanner green) is **MR-013 (P-531)**. MR-022 gets the convention right FIRST so the four durable
//! store MRs (007/008/023/024) bind to it correct-by-construction and MR-013 hardens *policy* on a
//! sound foundation rather than re-plumbing N stores.
//!
//! ## `residency-pin` lint — region pinned OUT-OF-BAND (`@residency-cell-pinned:file`)
//! [`connect_pool_with_reset`] builds a bounded sqlx pool, which has no native `Region` type to pin
//! on the construction statement. The region IS pinned: the [`crate::provider::SubstrateProvider`]
//! that owns this pool carries the cell's `config.region` and threads it here, where it is tagged
//! onto every connection's `application_name` (`myelin:<region>`) — the same per-session,
//! out-of-band region-pin posture as [`crate::pg`] and [`crate::oltp`]. The file-level waiver marker
//! `@residency-cell-pinned:file` records this LOUDLY (EI-01 §4 — named, never a silent skip); it is
//! NOT a weakening (an unmarked region-less store-open in any caller/application file still fires).
//! The end-to-end per-pool runtime region fail-fast lands with the rest of MR-013 (P-531).

use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;

use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use sqlx::{Executor, PgConnection};

use crate::pg::PgError;

/// The future a [`with_tenant_tx`] op returns — a `Send` boxed future bound to the borrow of the
/// transaction-scoped connection (`'c`). A store op is `|conn| Box::pin(async move { … })`.
pub type TxScope<'c, R> = Pin<Box<dyn Future<Output = Result<R, PgError>> + Send + 'c>>;

/// A transaction-scoped operation that can preserve a caller's typed domain error. Durable
/// subsystem stores use this form when a lost lease, stale cursor, or invariant violation must be
/// distinguished from a database failure without giving up the one RLS/GUC transaction convention.
pub type TypedTxScope<'c, R, E> = Pin<Box<dyn Future<Output = Result<R, E>> + Send + 'c>>;

/// Generic form of [`with_tenant_tx`] that preserves typed caller errors and rolls the transaction
/// back whenever the operation returns one. `E: From<PgError>` keeps begin/scope/commit failures on
/// the same error channel while allowing a subsystem to fail closed with its own machine variants.
pub async fn with_tenant_tx_error<R, F, E>(
    pool: &PgPool,
    tenant: &str,
    region: &str,
    op: F,
) -> Result<R, E>
where
    F: for<'c> FnOnce(&'c mut PgConnection) -> TypedTxScope<'c, R, E> + Send,
    R: Send,
    E: From<PgError>,
{
    let mut tx = pool.begin().await.map_err(|e| {
        E::from(PgError::Query(format!(
            "begin tenant-scoped transaction: {e}"
        )))
    })?;

    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)",
    )
    .bind(tenant)
    .bind(region)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        E::from(PgError::Query(format!(
            "set transaction-scoped tenant GUC: {e}"
        )))
    })?;

    let out = op(&mut tx).await?;
    tx.commit().await.map_err(|e| {
        E::from(PgError::Query(format!(
            "commit tenant-scoped transaction: {e}"
        )))
    })?;
    Ok(out)
}

/// **The tenant-scoped-transaction convention (RESHAPE-002).** Acquire a connection, open a
/// transaction, set the `(tenant, region)` GUC TRANSACTION-scoped (`set_config(..., true)`), run
/// `op` on the transaction-bound connection, then COMMIT — so the GUC is discarded on commit and the
/// returned pooled connection carries no residual tenant identity.
///
/// `tenant` MUST be the VERIFIED tenant (from the token, never a URL path — the §1.1 IDOR floor); the
/// DB RLS policy keyed on `current_setting('myelin.tenant_id', true)` then isolates the op to that
/// tenant. On any error (`op`, the GUC set, or the commit) the transaction rolls back (the GUC is
/// discarded either way) and the error propagates loudly — a tenant-scoped failure is never a silent
/// fallthrough.
///
/// ```ignore
/// with_tenant_tx(&pool, verified_tenant, region, |conn| Box::pin(async move {
///     sqlx::query("INSERT INTO rebac_tuple ( … ) VALUES ( … )")
///         .execute(&mut *conn).await.map_err(|e| PgError::Query(e.to_string()))?;
///     Ok(())
/// })).await?;
/// ```
pub async fn with_tenant_tx<R, F>(
    pool: &PgPool,
    tenant: &str,
    region: &str,
    op: F,
) -> Result<R, PgError>
where
    F: for<'c> FnOnce(&'c mut PgConnection) -> TxScope<'c, R> + Send,
    R: Send,
{
    with_tenant_tx_error(pool, tenant, region, op).await
}

/// Run one tenant-scoped read over a PostgreSQL `REPEATABLE READ`, read-only snapshot.
///
/// Multi-statement materializers use this when an authorization-bearing identity row and its child
/// rows must describe one database instant. A concurrent parent mutation may commit, but it cannot
/// change what later statements in this transaction observe. The tenant/region GUCs remain
/// transaction-local exactly as in [`with_tenant_tx`].
pub async fn with_tenant_repeatable_read_tx<R, F>(
    pool: &PgPool,
    tenant: &str,
    region: &str,
    op: F,
) -> Result<R, PgError>
where
    F: for<'c> FnOnce(&'c mut PgConnection) -> TxScope<'c, R> + Send,
    R: Send,
{
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| PgError::Query(format!("begin tenant-scoped read transaction: {e}")))?;
    // @tenant-cross-scope: configures the transaction snapshot before the tenant GUC is installed.
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await
        .map_err(|e| PgError::Query(format!("set tenant read snapshot isolation: {e}")))?;
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)",
    )
    .bind(tenant)
    .bind(region)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        PgError::Query(format!(
            "set repeatable-read transaction-scoped tenant GUC: {e}"
        ))
    })?;
    let out = op(&mut tx).await?;
    tx.commit()
        .await
        .map_err(|e| PgError::Query(format!("commit tenant-scoped read transaction: {e}")))?;
    Ok(out)
}

/// Open a bounded sqlx pool with **reset-on-release** wired (RESHAPE-002 defence-in-depth). The
/// `after_release` hook runs `RESET ALL` on every connection as it returns to the pool — scrubbing
/// any session-level configuration residue so no GUC can bleed to the next checkout, even from a code
/// path that did a session `SET`. `RESET ALL` resets configuration parameters only; it does NOT
/// deallocate prepared statements (so it is safe with sqlx's per-connection statement cache — the
/// reason this is `RESET ALL`, not `DISCARD ALL`).
///
/// This is the pool every durable tenant-scoped store should be constructed over so the
/// [`with_tenant_tx`] convention has belt-and-suspenders isolation. The pool is bounded
/// (`max_connections`, never unbounded — storage §3.1).
pub async fn connect_pool_with_reset(
    database_url: &str,
    region: &str,
    max_connections: u32,
) -> Result<PgPool, PgError> {
    // Pin the pool to its residency `Region` (ADR-11 / residency-pin): there is NO global,
    // region-less pool. The region is tagged onto every connection's `application_name`
    // (`myelin:<region>`) so the pool is observably bound to one residency boundary — data cannot
    // silently leave it, and PG-side observability (pg_stat_activity) shows the residency of each
    // connection. (Region fail-fast / mTLS hardening is MR-013; this is the construction-site pin.)
    let opts = PgConnectOptions::from_str(database_url)
        .map_err(|e| PgError::Connect(e.to_string()))?
        .application_name(&format!("myelin:{region}"));
    PgPoolOptions::new()
        .max_connections(max_connections.max(1))
        .after_release(|conn, _meta| {
            Box::pin(async move {
                // RESET ALL: scrub session GUC residue (defence in depth) without touching the
                // prepared-statement cache. Returns `Ok(true)` to KEEP the scrubbed connection.
                conn.execute("RESET ALL").await?;
                Ok(true)
            })
        })
        .connect_with(opts)
        .await
        .map_err(|e| PgError::Connect(e.to_string()))
}

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

use std::future::Future;
use std::pin::Pin;

use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::{Executor, PgConnection};

use crate::pg::PgError;

/// The future a [`with_tenant_tx`] op returns — a `Send` boxed future bound to the borrow of the
/// transaction-scoped connection (`'c`). A store op is `|conn| Box::pin(async move { … })`.
pub type TxScope<'c, R> = Pin<Box<dyn Future<Output = Result<R, PgError>> + Send + 'c>>;

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
    // acquire → BEGIN.
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| PgError::Query(format!("begin tenant-scoped transaction: {e}")))?;

    // set the (tenant, region) GUC TRANSACTION-scoped (is_local = true == SET LOCAL): it lives only
    // for THIS transaction and is discarded at COMMIT/ROLLBACK — no residual scope on the pooled
    // connection. One round trip sets both GUCs the RLS policy's `(tenant, region)` predicate keys on.
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true), set_config('myelin.region', $2, true)",
    )
    .bind(tenant)
    .bind(region)
    .execute(&mut *tx)
    .await
    .map_err(|e| PgError::Query(format!("set transaction-scoped tenant GUC: {e}")))?;

    // run the store op on the transaction-bound connection.
    let out = op(&mut tx).await?;

    // COMMIT — the GUC is discarded here (and reset-on-release scrubs any residue as defence in depth).
    tx.commit()
        .await
        .map_err(|e| PgError::Query(format!("commit tenant-scoped transaction: {e}")))?;
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
    max_connections: u32,
) -> Result<PgPool, PgError> {
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
        .connect(database_url)
        .await
        .map_err(|e| PgError::Connect(e.to_string()))
}

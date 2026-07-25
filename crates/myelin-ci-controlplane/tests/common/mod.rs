//! **Shared panic-safe test-schema teardown.** Every per-test Postgres schema this crate's
//! integration tests create (`ci_ct004a_<pid>`, `ci_ct004c2_<pid>_<tag>`, ...) was previously ONLY
//! ever cleaned up via `DROP SCHEMA IF EXISTS ...; CREATE SCHEMA ...` at the START of that same
//! test's NEXT run — never at the end of the CURRENT run, and never on a panicking assertion. That
//! left orphaned schemas to accumulate on a long-lived shared dev Postgres forever (234 confirmed on
//! this host before this fix), some of which leaked far enough to be reachable by other roles and to
//! re-contaminate the REAL shared `public.job_queue`/`public.ci_run` tables.
//!
//! [`with_schema_cleanup`] fixes this: it runs the test body, then UNCONDITIONALLY drops the schema
//! afterward — success, a failed assertion, or a panic all still clean up. A synchronous `Drop` impl
//! cannot safely run an async `DROP SCHEMA` query, so this catches an in-flight panic with
//! `FutureExt::catch_unwind`, always runs the cleanup, then resumes the unwind so the test still
//! fails/reports exactly as it did before (`cargo test` output is unchanged either way).
//!
//! This lives in `tests/common/mod.rs` (the standard Rust integration-test convention for a helper
//! module shared across `tests/*.rs` binaries) rather than duplicated per file: `tests/common/mod.rs`
//! is not itself compiled as a test target (unlike `tests/<name>.rs`), so `mod common;` in each file
//! that needs it pulls in ONE shared copy.
#![cfg(feature = "integration")]

use futures::FutureExt;
use sqlx::{Executor, PgPool};

/// Run `body`, then unconditionally `DROP SCHEMA IF EXISTS <schema> CASCADE` on `pool` afterward.
///
/// `pool` should be a handle that stays valid for the whole call even if the test body drops its
/// OWN pool/store bindings mid-test (some of these tests intentionally drop their working pool to
/// simulate a kill-9/reopen) — pass a cheap `.clone()` (or a fresh, separately-connected pool) of the
/// admin pool DEDICATED to this call, not a binding the body also consumes or explicitly `.close()`s.
/// `PgPool::close()` shuts down the WHOLE shared pool for every clone (not just the caller's handle),
/// so if the body ever closes the pool it cloned `pool` from, this cleanup's own DROP SCHEMA would
/// silently no-op against an already-closed pool — exactly the kind of silent leak this exists to
/// prevent, so the error below is surfaced (not swallowed) precisely to catch that class of mistake.
pub async fn with_schema_cleanup<Fut>(pool: &PgPool, schema: &str, body: impl FnOnce() -> Fut)
where
    Fut: std::future::Future<Output = ()>,
{
    let result = std::panic::AssertUnwindSafe(body()).catch_unwind().await;
    if let Err(error) = pool
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
    {
        // Never let a cleanup failure mask the test's real outcome (a panic below would shadow a
        // genuine assertion failure caught above) — but never let it go silent either.
        eprintln!(
            "with_schema_cleanup: DROP SCHEMA IF EXISTS {schema} CASCADE failed (schema may have \
             leaked): {error}"
        );
    }
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

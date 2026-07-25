//! **Shared panic-safe test-state teardown** for this crate's ad-hoc-table/row integration tests.
//!
//! This crate has ANOTHER, genuinely-per-process-`CREATE SCHEMA` shape elsewhere in `tests/`
//! (`integration_pg_bootstrap.rs`, `integration_w7_boot_migrations.rs`,
//! `integration_outbox_quarantine.rs`, `integration_elected_relay.rs`,
//! `integration_mr009b_outbox_durable.rs`, `integration_mr023_events_serve.rs`) — the shape the
//! sibling fix in `myelin-ci-controlplane/tests/common/mod.rs` (`with_schema_cleanup`) targets,
//! where 234 orphaned schemas were confirmed to have accumulated on this host because the only
//! teardown was a `DROP SCHEMA IF EXISTS ...; CREATE SCHEMA ...` reset at the START of that SAME
//! test's next run, never at the end of the current run, and never on a panicking assertion.
//!
//! The five files that pull in THIS module (`integration_migrate_concurrent.rs`,
//! `integration_migration_checksum_guard.rs`, `smoke_backends.rs`, `stage3_drills.rs`,
//! `stage4_floor_smokes.rs`) do NOT call `CREATE SCHEMA` at all (checked directly — grepping all
//! five for `fn schema_name` / `CREATE SCHEMA` is empty). Their ad-hoc per-run Postgres state is
//! instead a set of process/time-tagged TABLES and ROWS living in `public` (e.g.
//! `racetest_<pid>_<ns>_t0`, `drill1_state_<pid>-<n>`, `rebac_tuple`/`outbox` rows scoped by a
//! tagged tenant/aggregate). The SAME structural bug applies at this finer grain: every one of
//! these tests used to clean up its own tables/rows with a bare call written at the END of the
//! happy path, which a mid-test `assert!`/`assert_eq!`/`.expect(..)` panic skips entirely —
//! leaving the tagged tables/rows behind forever, exactly like the schema case, just one level
//! down (tables/rows instead of a whole schema).
//!
//! [`with_cleanup`] closes that gap for this shape: it runs the test's real body, then
//! UNCONDITIONALLY runs the test's own cleanup afterward — success, a failed assertion, or a
//! panic all still clean up. A synchronous `Drop` impl cannot safely run an async cleanup query,
//! so this catches an in-flight panic with `FutureExt::catch_unwind`, always runs cleanup, then
//! resumes the unwind so the test still fails/reports exactly as it did before (`cargo test`
//! output is unchanged either way).
//!
//! This lives in `tests/common/mod.rs` (the standard Rust integration-test convention for a
//! helper module shared across `tests/*.rs` binaries — the same layout
//! `myelin-ci-controlplane/tests/common/mod.rs` uses) rather than duplicated per file:
//! `tests/common/mod.rs` is not itself compiled as a test target (unlike `tests/<name>.rs`), so
//! `mod common;` in each of the five files pulls in ONE shared copy.
//!
//! Retrofit convention: each call site's `cleanup` closure only ever references pre-computed,
//! deterministic identifiers (table/tenant/aggregate names, or values re-derivable from the same
//! tag the body used, e.g. a content hash recomputed from the same deterministic bytes) rather
//! than reading `body`'s locals — `body` and `cleanup` are two independent closures passed to the
//! same call, so they cannot share a `&mut`-then-`&` borrow of body-local state.
#![cfg(feature = "integration")]

use futures::FutureExt;

/// Run `body()`, then unconditionally run `cleanup()` afterward — regardless of whether `body`
/// finished normally or panicked (a failed `assert!`/`assert_eq!`/`.expect(..)`). The panic (if
/// any) is re-raised AFTER `cleanup` has run, so the test still fails/reports correctly; it is
/// never swallowed.
pub async fn with_cleanup<BodyFut, CleanupFut>(
    body: impl FnOnce() -> BodyFut,
    cleanup: impl FnOnce() -> CleanupFut,
) where
    BodyFut: std::future::Future<Output = ()>,
    CleanupFut: std::future::Future<Output = ()>,
{
    let result = std::panic::AssertUnwindSafe(body()).catch_unwind().await;
    cleanup().await;
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

/// Delete every `outbox` row (and any `outbox_quarantine` row referencing it) for `aggregate`.
///
/// `outbox_quarantine.event_id` FKs to `outbox.event_id` with `ON DELETE RESTRICT`. This crate's
/// dev DB is a real SHARED host (this file's tests are only a few of many suites writing to the
/// SAME `outbox`/`outbox_quarantine` tables): some OTHER concurrently-running test/process can
/// quarantine one of this aggregate's own event_ids (reason `invalid_event_taxonomy`) at any
/// moment, including in the narrow gap between clearing `outbox_quarantine` and deleting from
/// `outbox` below — a single delete-quarantine-then-delete-outbox pass can lose that race and
/// leave the row stuck (observed live on this host). Retrying a bounded number of times shrinks
/// that window close to zero without pretending to eliminate it outright — genuinely serializing
/// against an independent test suite's concurrent writes on a live shared table is a bigger,
/// separate concern than this crate's own per-test cleanup.
///
/// `#[allow(dead_code)]`: this shared module is compiled once PER test binary that declares
/// `mod common;` (`integration_migrate_concurrent.rs` / `integration_migration_checksum_guard.rs`
/// never touch the `outbox` table, so they never call this helper — that is not dead code in the
/// module's actual callers, `smoke_backends.rs` / `stage3_drills.rs` / `stage4_floor_smokes.rs`).
#[allow(dead_code)]
pub async fn delete_outbox_for_aggregate(pool: &sqlx::PgPool, aggregate: &str) {
    for _ in 0..5 {
        let _ = sqlx::query("DELETE FROM outbox_quarantine WHERE aggregate = $1")
            .bind(aggregate)
            .execute(pool)
            .await;
        if sqlx::query("DELETE FROM outbox WHERE aggregate = $1")
            .bind(aggregate)
            .execute(pool)
            .await
            .is_ok()
        {
            return;
        }
    }
}

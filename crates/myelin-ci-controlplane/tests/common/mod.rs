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
// `tests/common/mod.rs` is compiled INTO each `tests/*.rs` binary that declares `mod common;`, so a
// helper used by only some of them is genuinely dead code in the others. That is the module's whole
// point — one shared copy, used where it applies — so the allowance is scoped to this file rather
// than every helper being forced on every suite.
#![allow(dead_code)]

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

/// **The fixture lock for tests that create per-test schemas carrying CI scheduler grants.**
///
/// Those schemas are the problem: the production `CiSchedulerDbProvider` excess-privilege probe
/// scans EVERY non-system schema, so while one test's schema exists, any concurrently-booting test
/// sees the scheduler role holding grants on `<schema>.job_queue` and refuses with "privileges
/// outside claim/reap/run discovery". Serializing the whole fixture lifecycle — setup, body and
/// cleanup — is what makes those suites safe to run alongside the boot tests.
///
/// The value is an arbitrary fixed key ("CIFIXTUR" in ASCII); only agreement between callers
/// matters.
pub const CI_PRIVILEGE_FIXTURE_LOCK: i64 = 0x4349_4649_5854_5552;

/// Run `body` while holding [`CI_PRIVILEGE_FIXTURE_LOCK`] on a DEDICATED connection, sweeping the
/// listed schema prefixes first.
///
/// A database advisory lock, deliberately, rather than inferring ownership from "the owning PID is
/// gone": host PIDs are reused, differ across containers, and say nothing about whether another
/// process is mid-test. Holding the lock also means the sweep may drop schemas belonging to the
/// CURRENT pid — an earlier crashed run of this same binary is exactly the case that leaks — which
/// a PID-based rule could never do safely.
///
/// Panic-safe: the lock is released and the panic resumed, so one failing test cannot wedge the
/// suite. Enumeration and drop failures are LOUD; a silently swallowed cleanup failure is how the
/// leak this exists to fix went unnoticed in the first place.
pub async fn with_privilege_fixture_lock<Fut>(
    admin_url: &str,
    sweep_prefixes: &[&str],
    body: impl FnOnce() -> Fut,
) where
    Fut: std::future::Future<Output = ()>,
{
    use sqlx::Row;
    // One dedicated pool of exactly one connection: a session advisory lock lives on a single
    // backend, so it must not be able to land on a different pooled connection mid-fixture.
    let lock_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(admin_url)
        .await
        .expect("connect the fixture-lock session");
    let mut lock_conn = lock_pool
        .acquire()
        .await
        .expect("acquire the dedicated fixture-lock connection");
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(CI_PRIVILEGE_FIXTURE_LOCK)
        .execute(&mut *lock_conn)
        .await
        .expect("take the privilege-fixture advisory lock");

    let sweep = async {
        for prefix in sweep_prefixes {
            let leaked = sqlx::query("SELECT nspname FROM pg_namespace WHERE nspname LIKE $1")
                .bind(format!("{prefix}%"))
                .fetch_all(&mut *lock_conn)
                .await
                .unwrap_or_else(|error| {
                    panic!("enumerate leaked `{prefix}` fixture schemas: {error}")
                });
            for row in leaked {
                let schema: String = row.get("nspname");
                // Identifier-quote: a catalogue-returned name is data, never trusted as SQL text.
                let quoted: String = sqlx::query_scalar("SELECT quote_ident($1)")
                    .bind(&schema)
                    .fetch_one(&mut *lock_conn)
                    .await
                    .unwrap_or_else(|error| panic!("quote leaked schema `{schema}`: {error}"));
                sqlx::query(&format!("DROP SCHEMA IF EXISTS {quoted} CASCADE"))
                    .execute(&mut *lock_conn)
                    .await
                    .unwrap_or_else(|error| {
                        panic!("drop leaked fixture schema `{schema}`: {error}")
                    });
            }
        }
    };

    let result = std::panic::AssertUnwindSafe(async {
        sweep.await;
        body().await;
    })
    .catch_unwind()
    .await;

    let unlocked = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(CI_PRIVILEGE_FIXTURE_LOCK)
        .execute(&mut *lock_conn)
        .await;
    if let Err(error) = unlocked {
        eprintln!("with_privilege_fixture_lock: releasing the advisory lock FAILED: {error}");
    }
    drop(lock_conn);
    lock_pool.close().await;
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

/// Run `body`, then unconditionally drop `role` — panic-safe, so a failing test never leaves a
/// global role behind for another test's privilege probe to trip over. Dependent privileges are
/// revoked first because PostgreSQL refuses to drop a role that still owns grants.
pub async fn with_throwaway_role<Fut>(admin: &PgPool, role: &str, body: impl FnOnce() -> Fut)
where
    Fut: std::future::Future<Output = ()>,
{
    let result = std::panic::AssertUnwindSafe(body()).catch_unwind().await;
    let quoted: Result<String, _> = sqlx::query_scalar("SELECT quote_ident($1)")
        .bind(role)
        .fetch_one(admin)
        .await;
    match quoted {
        Ok(quoted) => {
            for statement in [
                format!("DROP OWNED BY {quoted} CASCADE"),
                format!("DROP ROLE IF EXISTS {quoted}"),
            ] {
                if let Err(error) = admin.execute(statement.as_str()).await {
                    eprintln!("with_throwaway_role: `{statement}` failed (role may leak): {error}");
                }
            }
        }
        Err(error) => eprintln!("with_throwaway_role: could not quote `{role}`: {error}"),
    }
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

/// **Apply migrations to an isolated fixture schema without exposing scheduler grants to concurrent
/// tests.**
///
/// The CI control-plane migration set installs `myelin_ci_region_scheduler` grants in whatever
/// schema it is applied to. The production `CiSchedulerDbProvider` excess-privilege probe scans
/// EVERY non-system schema, so while such a fixture schema exists, a concurrently-booting
/// scheduler-boundary or `production_pg_bootstrap_source` test sees the scheduler role holding
/// privileges on `<fixture>.job_queue` and refuses with `ExcessPrivileges`.
///
/// This closes that by BOTH holding [`CI_PRIVILEGE_FIXTURE_LOCK`] across the apply and immediately
/// revoking those grants from the fixture schema afterwards. Both halves are deliberate:
///
/// - The REVOKE is what makes the schema permanently invisible to the probe for the rest of the
///   test, so the body runs unlocked. That is the low-contention part: full test bodies here run
///   for tens of seconds and must not serialize against each other. A caller that migrates MORE
///   THAN ONCE must pass every apply and everything between them as a single `migrate` closure —
///   otherwise the first pass's grants stand, unrevoked and unlocked, until the last one finishes.
/// - The LOCK closes the millisecond window between the migration committing its grants and the
///   revoke landing. It costs almost nothing extra, because `PgMigrator` already takes its own
///   global advisory lock for the duration of an apply — two fixtures could never migrate
///   concurrently anyway. Lock order is fixture-lock → migration-lock everywhere, so there is no
///   cycle with the suites that hold the fixture lock across a whole body.
pub async fn with_fixture_migration_lock<Fut>(
    admin_url: &str,
    admin: &PgPool,
    schema: &str,
    migrate: impl FnOnce() -> Fut,
) where
    Fut: std::future::Future<Output = ()>,
{
    let lock_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(admin_url)
        .await
        .expect("connect the fixture-migration lock session");
    let mut lock_conn = lock_pool
        .acquire()
        .await
        .expect("acquire the dedicated fixture-migration lock connection");
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(CI_PRIVILEGE_FIXTURE_LOCK)
        .execute(&mut *lock_conn)
        .await
        .expect("take the privilege-fixture advisory lock for migration");

    // The migration and the revocation are caught SEPARATELY, and the revocation runs whether or not
    // the migration panicked. A single combined future skipped revocation on a migration panic and
    // then released the lock — publishing a scheduler-granted schema to every concurrent boot probe
    // for as long as the outer schema cleanup (which runs after this helper returns) took to land.
    let migrated = std::panic::AssertUnwindSafe(migrate())
        .catch_unwind()
        .await;
    let revoked = std::panic::AssertUnwindSafe(revoke_scheduler_grants(admin, schema))
        .catch_unwind()
        .await;

    if let Err(error) = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(CI_PRIVILEGE_FIXTURE_LOCK)
        .execute(&mut *lock_conn)
        .await
    {
        eprintln!("with_fixture_migration_lock: releasing the advisory lock FAILED: {error}");
    }
    drop(lock_conn);
    lock_pool.close().await;
    // The migration's failure is the test's real outcome, so it is resumed FIRST; a revoke failure
    // on top of it would only shadow the diagnosis.
    if let Err(payload) = migrated {
        std::panic::resume_unwind(payload);
    }
    if let Err(payload) = revoked {
        std::panic::resume_unwind(payload);
    }
}

/// Strip every `myelin_ci_region_scheduler` privilege from one fixture schema.
///
/// Table-level and COLUMN-level are revoked separately on purpose: PostgreSQL does not remove
/// separately granted column ACLs when a table-level privilege is revoked, and the CI migration set
/// grants both shapes (`GRANT SELECT ON job_queue` and `GRANT UPDATE (state, lease_owner, ...)`).
async fn revoke_scheduler_grants(admin: &PgPool, schema: &str) {
    let quoted: String = sqlx::query_scalar("SELECT quote_ident($1)")
        .bind(schema)
        .fetch_one(admin)
        .await
        .unwrap_or_else(|error| panic!("quote fixture schema `{schema}`: {error}"));
    let statement = format!(
        "DO $revoke$
         DECLARE
           target record;
         BEGIN
           IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'myelin_ci_region_scheduler') THEN
             RETURN;
           END IF;
           FOR target IN
             SELECT c.oid::regclass AS relation, a.attname
               FROM pg_class c
               JOIN pg_namespace n ON n.oid = c.relnamespace
               JOIN pg_attribute a ON a.attrelid = c.oid
              WHERE n.nspname = '{schema}'
                AND c.relkind IN ('r', 'p', 'v', 'm')
                AND a.attnum > 0 AND NOT a.attisdropped
           LOOP
             EXECUTE format(
               'REVOKE ALL (%I) ON TABLE %s FROM myelin_ci_region_scheduler',
               target.attname, target.relation);
           END LOOP;
           EXECUTE 'REVOKE ALL PRIVILEGES ON ALL TABLES IN SCHEMA {quoted}                     FROM myelin_ci_region_scheduler';
           EXECUTE 'REVOKE ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA {quoted}                     FROM myelin_ci_region_scheduler';
           EXECUTE 'REVOKE ALL ON SCHEMA {quoted} FROM myelin_ci_region_scheduler';
         END
         $revoke$;"
    );
    admin.execute(statement.as_str()).await.unwrap_or_else(|error| {
        panic!("revoke scheduler grants from fixture schema `{schema}`: {error}")
    });
}

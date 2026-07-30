//! **The ROLLING-UPGRADE migration proof (CT-004d.2).** The existing migration tests prove a FRESH
//! bootstrap applies the whole set. This proves the UPGRADE path from the already-shipped completion
//! WIP: apply the sequence with `ci_0004a`/`ci_0015a`/`ci_0016a`, then apply the full set. Those
//! shared ids checksum-match and the new additive migrations add nonce authority, queued-run
//! discovery, and active-workflow recovery. This is the checked invariant behind editing NO
//! applied migration DDL (the base creates stay byte-frozen).
#![cfg(feature = "integration")]

mod common;

use common::with_schema_cleanup;
use myelin_ci_controlplane::{
    ci_controlplane_hot_tables, ci_controlplane_migrations,
    CI_JOB_QUEUE_CLAIM_AUTHORITY_MIGRATION_ID, CI_RUN_CONCURRENCY_GROUP_MIGRATION_ID,
    CI_RUN_PR_HEAD_GENERATION_MIGRATION_ID, CI_RUN_QUEUED_REGION_INDEX_MIGRATION_ID,
    CI_RUN_SURFACE_REPO_CREATED_INDEX_MIGRATION_ID, CI_SCHEDULER_CI_RUN_DISCOVERY_MIGRATION_ID,
    CI_SCHEDULER_CI_WORKFLOW_DISCOVERY_MIGRATION_ID, CI_SCHEDULER_CLAIM_NONCE_GRANT_MIGRATION_ID,
    CI_WORKFLOW_ACTIVE_REGION_INDEX_MIGRATION_ID,
};
use myelin_storage::{migration::HotTables, PgMigrator};
use myelin_substrate::Migrations;
use sqlx::{Executor, PgPool};

fn admin_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn schema_name() -> String {
    format!("ci_upgrade_{}", std::process::id())
}

async fn pinned_pool(schema: &str) -> PgPool {
    let pinned = schema.to_string();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |conn, _| {
            let pinned = pinned.clone();
            Box::pin(async move {
                conn.execute(format!("SET search_path TO {pinned}, public").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(&admin_url())
        .await
        .expect("connect schema-pinned admin pool (is the dev stack up?)")
}

/// The additive follow-ons; every other migration id/DDL represents the deployed WIP schema.
const NEW_SUB_MIGRATION_IDS: &[&str] = &[
    CI_RUN_CONCURRENCY_GROUP_MIGRATION_ID,
    CI_RUN_PR_HEAD_GENERATION_MIGRATION_ID,
    CI_JOB_QUEUE_CLAIM_AUTHORITY_MIGRATION_ID,
    CI_SCHEDULER_CLAIM_NONCE_GRANT_MIGRATION_ID,
    CI_RUN_QUEUED_REGION_INDEX_MIGRATION_ID,
    CI_SCHEDULER_CI_RUN_DISCOVERY_MIGRATION_ID,
    CI_WORKFLOW_ACTIVE_REGION_INDEX_MIGRATION_ID,
    CI_SCHEDULER_CI_WORKFLOW_DISCOVERY_MIGRATION_ID,
    CI_RUN_SURFACE_REPO_CREATED_INDEX_MIGRATION_ID,
];

/// The full set with the new sub-migrations filtered OUT — the sequence a pre-CT-004d.2 deploy applied.
fn old_sequence() -> Migrations {
    Migrations::of(
        ci_controlplane_migrations()
            .0
            .into_iter()
            .filter(|m| !NEW_SUB_MIGRATION_IDS.contains(&m.id))
            .collect::<Vec<_>>(),
    )
}

async fn column_exists(pool: &PgPool, schema: &str, table: &str, column: &str) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM information_schema.columns \
         WHERE table_schema = $1 AND table_name = $2 AND column_name = $3)",
    )
    .bind(schema)
    .bind(table)
    .bind(column)
    .fetch_one(pool)
    .await
    .expect("query information_schema")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claim_authority_followons_preserve_applied_wip_checksums() {
    let schema = schema_name();
    let bare = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect bare admin pool");
    bare.execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .expect("drop any stale schema");
    bare.execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create the upgrade schema");
    let cleanup_bare = bare.clone();
    let schema_for_cleanup = schema.clone();
    with_schema_cleanup(&cleanup_bare, &schema_for_cleanup, move || async move {
    let pool = pinned_pool(&schema).await;

    // CI's recovery-route follow-ons intentionally target Flow's authoritative workflow_run table.
    // Install that prerequisite in the same isolated schema so this proof cannot mutate or inherit
    // policies/indexes from public.workflow_run.
    PgMigrator::apply_validated(
        &pool,
        &myelin_flow::migrations::migrations(),
        &HotTables::declare(["workflow_run"]),
    )
    .await
    .expect("the isolated Flow prerequisite applies");

    // Steps 1–4 are ONE locked region. `old_sequence()` filters only nine migration ids and still
    // contains the `myelin_ci_region_scheduler` grant migrations, so its grants are live from step 1
    // onward; locking only the step-2 apply left them exposed to every concurrently-booting
    // scheduler probe throughout the step-1 assertions. The helper revokes once, after step 4, and
    // only then releases the lock.
    common::with_fixture_migration_lock(&admin_url(), &pool, &schema, || async {
    // ── 1. Apply the already-shipped WIP sequence (including immutable a-suffix migrations). ──
    PgMigrator::apply_validated(&pool, &old_sequence(), &ci_controlplane_hot_tables())
        .await
        .expect("the already-shipped completion WIP sequence applies");
    assert!(
        column_exists(&pool, &schema, "ci_job_spec", "stage").await,
        "the applied ci_0015a migration already owns ci_job_spec.stage"
    );
    assert!(
        column_exists(&pool, &schema, "job_queue", "lease_epoch").await,
        "the applied ci_0004a migration already owns job_queue.lease_epoch"
    );
    assert!(!column_exists(&pool, &schema, "job_queue", "claim_nonce").await);
    assert!(!column_exists(&pool, &schema, "job_queue", "stage").await);
    assert!(
        !column_exists(&pool, &schema, "ci_run", "concurrency_group").await,
        "the deployed WIP schema predates canonical PR concurrency identity"
    );
    assert!(
        !column_exists(&pool, &schema, "ci_run", "pr_head_generation").await,
        "the deployed WIP schema predates producer-authored PR ordering authority"
    );

    // ── 2. Apply the NEW full set over the old schema — the shared applied ids checksum-match (the base
    //       a-suffix DDL is byte-frozen), and only the additive follow-ons apply. ──
    PgMigrator::apply_validated(
        &pool,
        &ci_controlplane_migrations(),
        &ci_controlplane_hot_tables(),
    )
    .await
    .expect("the NEW full set applies forward-only over the old schema (no checksum conflict)");

    // ── 3. The new columns now exist (the rolling upgrade converged on the fresh-bootstrap shape). ──
    assert!(
        column_exists(&pool, &schema, "ci_job_spec", "stage").await,
        "ci_0015a added ci_job_spec.stage on the upgrade"
    );
    assert!(
        column_exists(&pool, &schema, "job_queue", "lease_epoch").await,
        "ci_0004a added job_queue.lease_epoch on the upgrade"
    );
    assert!(
        column_exists(&pool, &schema, "job_queue", "completion_receipt").await,
        "ci_0004a added job_queue.completion_receipt on the upgrade"
    );
    assert!(
        column_exists(&pool, &schema, "job_queue", "claim_nonce").await,
        "ci_0004b added job_queue.claim_nonce on the upgrade"
    );
    assert!(
        column_exists(&pool, &schema, "job_queue", "stage").await,
        "ci_0004b added job_queue.stage on the upgrade"
    );
    assert!(
        column_exists(&pool, &schema, "job_queue", "claim_started_at").await,
        "ci_0004c added job_queue.claim_started_at on the upgrade"
    );
    assert!(
        column_exists(&pool, &schema, "job_queue", "claim_expires_at").await,
        "ci_0004c added job_queue.claim_expires_at on the upgrade"
    );
    assert!(
        column_exists(&pool, &schema, "ci_run", "concurrency_group").await,
        "ci_0001c added canonical PR concurrency identity on the upgrade"
    );
    assert!(
        column_exists(&pool, &schema, "ci_run", "pr_head_generation").await,
        "ci_0001d added producer-authored PR ordering authority on the upgrade"
    );

    // ── 4. Idempotent re-apply is a clean no-op (every id already recorded). ──
    PgMigrator::apply_validated(
        &pool,
        &ci_controlplane_migrations(),
        &ci_controlplane_hot_tables(),
    )
    .await
    .expect("a second apply of the full set is an idempotent no-op");
    })
    .await;

    pool.close().await;
    // NOTE: do NOT close `bare` here — `cleanup_bare` (passed to `with_schema_cleanup` above) is a
    // `.clone()` of `bare`, sharing the SAME underlying pool. `PgPool::close()` shuts the whole shared
    // pool down for every clone, which would silently break the wrapper's own post-body `DROP SCHEMA`
    // (its error is deliberately swallowed by `with_schema_cleanup`, so this failed with no visible
    // signal — exactly the kind of silent leak this retrofit exists to prevent). Let `bare` drop
    // normally; the wrapper closes/drops the schema through `cleanup_bare` after this closure returns.
    })
    .await;
}

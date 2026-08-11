#![cfg(feature = "integration")]

mod common;

use common::with_schema_cleanup;
use myelin_ci_controlplane::{
    ci_controlplane_hot_tables, ci_controlplane_migrations,
    CI_JOB_QUEUE_CLAIM_AUTHORITY_MIGRATION_ID, CI_RUN_CONCURRENCY_GROUP_MIGRATION_ID,
    CI_RUN_PR_HEAD_GENERATION_MIGRATION_ID, CI_RUN_QUEUED_REGION_INDEX_MIGRATION_ID,
    CI_RUN_SOURCE_REF_CONSTRAINT_MIGRATION_ID, CI_RUN_SOURCE_REF_CONSTRAINT_VALIDATE_MIGRATION_ID,
    CI_RUN_SOURCE_REF_MIGRATION_ID, CI_RUN_SURFACE_REPO_CREATED_INDEX_MIGRATION_ID,
    CI_SCHEDULER_CI_RUN_DISCOVERY_MIGRATION_ID, CI_SCHEDULER_CI_WORKFLOW_DISCOVERY_MIGRATION_ID,
    CI_SCHEDULER_CLAIM_NONCE_GRANT_MIGRATION_ID, CI_WORKFLOW_ACTIVE_REGION_INDEX_MIGRATION_ID,
};
use myelin_storage::{migration::HotTables, PgMigrator};
use myelin_substrate::Migrations;
use sqlx::{Executor, PgPool};

fn admin_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn schema_name(story: &str) -> String {
    format!("ci_upgrade_{}_{}", story, std::process::id())
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
    CI_RUN_SOURCE_REF_MIGRATION_ID,
    CI_RUN_SOURCE_REF_CONSTRAINT_MIGRATION_ID,
    CI_RUN_SOURCE_REF_CONSTRAINT_VALIDATE_MIGRATION_ID,
];

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
async fn forward_upgrades_preserve_history_and_repair_intermediate_source_refs() {
    let schema = schema_name("intermediate");
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

        PgMigrator::apply_validated(
            &pool,
            &myelin_flow::migrations::migrations(),
            &HotTables::declare(["workflow_run"]),
        )
        .await
        .expect("the isolated Flow prerequisite applies");

        common::with_fixture_migration_lock(&admin_url(), &pool, &schema, || async {
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

            pool.execute("ALTER TABLE ci_run ADD COLUMN source_ref text")
                .await
                .expect("the intermediate release added source_ref directly to ci_run");
            pool.execute(
                "INSERT INTO ci_run (\
           tenant_id, region, run_id, project_id, pipeline_id, wf_run_id, source_ref, \
           definition_snapshot, trigger_kind, trust_tier, state, correlation_id\
         ) VALUES (\
           'upgrade-tenant', 'fr-par', '10000000-0000-0000-0000-000000000001', \
           '20000000-0000-0000-0000-000000000001', \
           '30000000-0000-0000-0000-000000000001', \
           '40000000-0000-0000-0000-000000000001', \
           'refs/heads/main', 'blake3:upgrade-snapshot', 'push', 'trusted', 'queued', \
           'upgrade-source-ref'\
         )",
            )
            .await
            .expect("a canonical source ref existed before the repair");
            let source_ref_checks_before: i64 = sqlx::query_scalar(
                "SELECT count(*)::bigint FROM pg_catalog.pg_constraint \
         WHERE conrelid = 'ci_run'::regclass \
           AND pg_catalog.pg_get_constraintdef(oid) ILIKE '%source_ref%'",
            )
            .fetch_one(&pool)
            .await
            .expect("inspect the intermediate constraints");
            assert_eq!(
                source_ref_checks_before, 0,
                "the historical intermediate schema really has an unguarded source_ref column"
            );

            PgMigrator::apply_validated(
                &pool,
                &ci_controlplane_migrations(),
                &ci_controlplane_hot_tables(),
            )
            .await
            .expect(
                "the NEW full set applies forward-only over the old schema (no checksum conflict)",
            );

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
            let (validated, definition): (bool, String) = sqlx::query_as(
                "SELECT convalidated, pg_catalog.pg_get_constraintdef(oid) \
         FROM pg_catalog.pg_constraint \
         WHERE conrelid = 'ci_run'::regclass AND conname = 'ci_run_source_ref_shape'",
            )
            .fetch_one(&pool)
            .await
            .expect("the upgrade installs the named source-ref contract");
            assert!(validated, "the repaired contract is validated before boot");
            assert!(
        definition.contains("trigger_kind = 'push'::text")
            && definition.contains("^refs/heads/"),
        "the named contract guards both trigger provenance and branch-ref shape: {definition}"
    );

            let rejected = pool
                .execute(
                    "UPDATE ci_run SET source_ref = 'refs/tags/release' \
             WHERE tenant_id = 'upgrade-tenant' \
               AND run_id = '10000000-0000-0000-0000-000000000001'::uuid",
                )
                .await
                .expect_err("a push cannot acquire a tag-shaped source ref after the repair");
            let database = rejected
                .as_database_error()
                .expect("the repaired check rejects the write in PostgreSQL");
            assert_eq!(database.code().as_deref(), Some("23514"));
            assert_eq!(database.constraint(), Some("ci_run_source_ref_shape"));

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
    })
    .await;

    a_fresh_database_adopts_one_named_source_ref_contract().await;
}

async fn a_fresh_database_adopts_one_named_source_ref_contract() {
    let schema = schema_name("fresh");
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
        .expect("create the fresh schema");
    let cleanup_bare = bare.clone();
    let schema_for_cleanup = schema.clone();
    with_schema_cleanup(&cleanup_bare, &schema_for_cleanup, move || async move {
        let pool = pinned_pool(&schema).await;

        PgMigrator::apply_validated(
            &pool,
            &myelin_flow::migrations::migrations(),
            &HotTables::declare(["workflow_run"]),
        )
        .await
        .expect("the isolated Flow prerequisite applies");

        common::with_fixture_migration_lock(&admin_url(), &pool, &schema, || async {
            PgMigrator::apply_validated(
                &pool,
                &ci_controlplane_migrations(),
                &ci_controlplane_hot_tables(),
            )
            .await
            .expect("the complete CI schema applies from empty");

            let contracts: Vec<(String, bool)> = sqlx::query_as(
                "SELECT conname, convalidated FROM pg_catalog.pg_constraint \
                 WHERE conrelid = 'ci_run'::regclass \
                   AND pg_catalog.pg_get_constraintdef(oid) ILIKE '%source_ref%' \
                 ORDER BY conname",
            )
            .fetch_all(&pool)
            .await
            .expect("inspect source-ref contracts");
            assert_eq!(
                contracts,
                vec![("ci_run_source_ref_shape".to_owned(), true)],
                "the repair adopts the original anonymous check instead of evaluating it twice"
            );
        })
        .await;

        pool.close().await;
    })
    .await;
}

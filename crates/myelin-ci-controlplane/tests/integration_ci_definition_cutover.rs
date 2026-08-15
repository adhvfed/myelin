#![cfg(feature = "integration")]

mod common;

use std::time::Duration;

use common::with_schema_cleanup;
use myelin_ci_controlplane::{
    ci_production_runtime_factory_test_support, ci_region_run_discovery_test_support,
    ActivationReadinessProbe, CiSupersededDefinitionGuardError, CutoverPlan,
    CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION, CI_MANIFEST_PIPELINE_VERSION,
};
use myelin_config::MyelinConfig;
use myelin_storage::{DurableCostLedger, HotTables, PgMigrator, SubstrateProvider};
use myelin_tenancy::Region;
use sqlx::{Acquire, Executor, PgPool, Row};

static MIGRATION_SCENARIO_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const TENANT: &str = "cutover-tenant";
const REGION: &str = "fr-par";
const OTHER_REGION: &str = "de-fra";

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn scoped_url(url: &str, schema: &str) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}options=-c%20search_path%3D{schema}%2Cpublic")
}

async fn pinned_pool(url: &str, schema: &str, connections: u32) -> PgPool {
    let schema = schema.to_owned();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(connections)
        .after_connect(move |connection, _| {
            let schema = schema.clone();
            Box::pin(async move {
                connection
                    .execute(format!("SET search_path TO {schema}, public").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(url)
        .await
        .expect("connect to live PostgreSQL (is the dev stack up?)")
}

fn schema_name(tag: &str) -> String {
    format!(
        "ci_cutover_{}_{}_{}",
        std::process::id(),
        tag,
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    )
}

async fn cutover_schema(tag: &str) -> (String, PgPool, PgPool) {
    let schema = schema_name(tag);
    let bootstrap = pinned_pool(&admin_url(), "public", 2).await;
    bootstrap
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .unwrap();
    bootstrap
        .execute(format!("CREATE SCHEMA {schema} AUTHORIZATION myelin_admin").as_str())
        .await
        .unwrap();
    let admin = pinned_pool(&admin_url(), &schema, 6).await;
    admin
        .execute(
            format!(
                "GRANT USAGE ON SCHEMA {schema} TO myelin_app;
                 ALTER DEFAULT PRIVILEGES FOR ROLE myelin_admin IN SCHEMA {schema}
                   GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO myelin_app;"
            )
            .as_str(),
        )
        .await
        .unwrap();
    PgMigrator::apply_validated(
        &admin,
        &myelin_flow::migrations::migrations(),
        &HotTables::declare(["workflow_run"]),
    )
    .await
    .unwrap();
    PgMigrator::apply_validated(
        &admin,
        &myelin_ci_controlplane::ci_controlplane_migrations(),
        &myelin_ci_controlplane::ci_controlplane_hot_tables(),
    )
    .await
    .unwrap();
    install_schema_local_probe(&admin, &schema).await;
    install_schema_local_readiness_probe(&admin, &schema).await;
    sqlx::query(
        "INSERT INTO wf_definition (wf_type, version, code_hash, status)
         VALUES ('ci.pipeline', $1, 'blake3:production-predecessor-hash', 'active')
         ON CONFLICT (wf_type, version)
         DO UPDATE SET code_hash = EXCLUDED.code_hash, status = EXCLUDED.status",
    )
    .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
    .execute(&admin)
    .await
    .unwrap();
    (schema, bootstrap, admin)
}

async fn install_schema_local_probe(admin: &PgPool, schema: &str) {
    admin
        .execute(
            format!(
                "GRANT USAGE, CREATE ON SCHEMA {schema} TO myelin_ci_definition_fence;
                 GRANT SELECT (wf_type, wf_version, state)
                   ON TABLE {schema}.workflow_run TO myelin_ci_definition_fence;
                 SET LOCAL ROLE myelin_ci_definition_fence;
                 CREATE FUNCTION {schema}.myelin_ci_pipeline_version_has_nonterminal_runs(
                   version integer)
                 RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER
                 SET search_path = pg_catalog SET row_security = off
                 AS $probe$SELECT EXISTS (SELECT 1 FROM {schema}.workflow_run
                   WHERE wf_type = 'ci.pipeline' AND wf_version = $1
                     AND state IN ('running', 'waiting'))$probe$;
                 RESET ROLE;"
            )
            .as_str(),
        )
        .await
        .expect("install the schema-local backlog probe as the fence role");
}

async fn tagged_pool(url: &str, schema: &str, application_name: &str, connections: u32) -> PgPool {
    let schema = schema.to_owned();
    let application_name = application_name.to_owned();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(connections)
        .after_connect(move |connection, _| {
            let schema = schema.clone();
            let application_name = application_name.clone();
            Box::pin(async move {
                connection
                    .execute(
                        format!(
                            "SET search_path TO {schema}, public;
                             SET application_name TO '{application_name}'"
                        )
                        .as_str(),
                    )
                    .await?;
                Ok(())
            })
        })
        .connect(url)
        .await
        .expect("connect the tagged pool")
}

async fn cutover_factory(
    pool: &PgPool,
    schema: &str,
) -> myelin_ci_controlplane::CiProductionRuntimeFactory {
    let mut config = MyelinConfig::dev();
    config.database_url = scoped_url(&admin_url(), schema);
    config.region = REGION.to_owned();
    let provider = SubstrateProvider::connect(config, 2).await.unwrap();
    ci_production_runtime_factory_test_support(
        pool.clone(),
        Region(REGION.into()),
        DurableCostLedger::new(provider),
        tokio::runtime::Handle::current(),
    )
    .unwrap()
    .with_backlog_probe_call_for_tests(format!(
        "SELECT {schema}.myelin_ci_pipeline_version_has_nonterminal_runs($1)"
    ))
    .replace_activation_readiness_probe_call_for_tests(schema_local_readiness_call(schema))
}

fn diagnostics(admin: &PgPool) -> myelin_ci_controlplane::CiRegionRunDiscovery {
    ci_region_run_discovery_test_support(admin.clone())
}

async fn definition_row(admin: &PgPool, version: i32) -> Option<(String, String)> {
    sqlx::query(
        "SELECT code_hash, status FROM wf_definition WHERE wf_type='ci.pipeline' AND version=$1",
    )
    .bind(version)
    .fetch_optional(admin)
    .await
    .unwrap()
    .map(|row| (row.get("code_hash"), row.get("status")))
}

async fn seed_workflow_run(admin: &PgPool, region: &str, run_id: &str, version: i32, state: &str) {
    sqlx::query(
        "INSERT INTO workflow_run (
           tenant_id, region, run_id, wf_type, wf_version, input, state, correlation_id,
           depth, partition
         ) VALUES ($1, $2, $3, 'ci.pipeline', $4, '[]'::jsonb, $5, $3, 0, 0)",
    )
    .bind(TENANT)
    .bind(region)
    .bind(run_id)
    .bind(version)
    .bind(state)
    .execute(admin)
    .await
    .unwrap();
}

async fn is_blocked_on_lock(observer: &PgPool, pid: i32) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
           SELECT 1 FROM pg_stat_activity
           WHERE pid = $1 AND wait_event_type = 'Lock' AND state = 'active'
         )",
    )
    .bind(pid)
    .fetch_one(observer)
    .await
    .unwrap()
}

async fn wait_until_blocked(observer: &PgPool, pid: i32) {
    for _ in 0..200 {
        if is_blocked_on_lock(observer, pid).await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("connection {pid} never entered a lock wait - the fence is not actually exclusive");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_old_predecessor_admission_holding_the_share_lock_makes_the_cutover_observe_its_workflow(
) {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("old_wins").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let mut old_admission = admin.acquire().await.unwrap();
        let mut old_tx = old_admission.begin().await.unwrap();
        let locked: Option<String> = sqlx::query_scalar(
            "SELECT status FROM wf_definition
             WHERE wf_type = 'ci.pipeline' AND version = $1 FOR SHARE",
        )
        .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
        .fetch_optional(&mut *old_tx)
        .await
        .unwrap();
        assert_eq!(
            locked.as_deref(),
            Some("active"),
            "the old binary sees its predecessor definition active and proceeds"
        );

        let cutover_tag = format!("myelin-cutover-old-wins-{}", std::process::id());
        let cutover_pool = tagged_pool(&admin_url(), &schema, &cutover_tag, 2).await;
        let factory = cutover_factory(&cutover_pool, &schema).await;
        let diagnostics_owned = diagnostics(&admin);
        let cutover =
            tokio::spawn(async move { factory.cutover_definition(&diagnostics_owned).await });

        let observer = pinned_pool(&admin_url(), &schema, 2).await;
        let mut waiting_pid = None;
        for _ in 0..400 {
            waiting_pid = sqlx::query_scalar::<_, i32>(
                "SELECT pid FROM pg_stat_activity
                 WHERE application_name = $1
                   AND wait_event_type = 'Lock'
                   AND state = 'active'
                 LIMIT 1",
            )
            .bind(&cutover_tag)
            .fetch_optional(&observer)
            .await
            .unwrap();
            if waiting_pid.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let waiting_pid = waiting_pid.expect(
            "the cutover's OWN backend must BLOCK behind the old admission's FOR SHARE, not \
             proceed past it",
        );
        let blocked_on_definition: bool = sqlx::query_scalar(
            "SELECT query ILIKE '%wf_definition%' FROM pg_stat_activity WHERE pid = $1",
        )
        .bind(waiting_pid)
        .fetch_one(&observer)
        .await
        .unwrap();
        assert!(
            blocked_on_definition,
            "the cutover backend must be waiting on the wf_definition fence"
        );
        assert!(
            !cutover.is_finished(),
            "the cutover cannot have completed while the old admission holds the fence"
        );

        sqlx::query(
            "INSERT INTO workflow_run (
               tenant_id, region, run_id, wf_type, wf_version, input, state, correlation_id,
               depth, partition
             ) VALUES ($1, $2, 'late-predecessor-run', 'ci.pipeline', $3, '[]'::jsonb, 'running', 'c', 0, 0)",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
        .execute(&mut *old_tx)
        .await
        .unwrap();
        old_tx.commit().await.unwrap();

        let refusal = cutover
            .await
            .unwrap()
            .expect_err("the cutover must observe the late predecessor admission and refuse");
        assert!(
            matches!(refusal, CiSupersededDefinitionGuardError::Backlog(_)),
            "expected a backlog refusal, got {refusal:?}"
        );

        assert_eq!(
            definition_row(&admin, CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
                .await
                .map(|(_, status)| status),
            Some("active".into()),
            "a refused cutover leaves the old fleet fully operational"
        );
        assert_eq!(
            definition_row(&admin, CI_MANIFEST_PIPELINE_VERSION).await,
            None,
            "no current-version row may exist after a refused cutover"
        );
        observer.close().await;
        cutover_pool.close().await;
    })
    .await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cutover_holding_the_update_lock_blocks_and_then_refuses_a_fresh_predecessor_admission() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(
        &admin_url(),
        &["ci_cutover_", "ci_lease_topology_"],
        || async {
            let (schema, bootstrap, admin) = cutover_schema("cutover_wins").await;
            with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
                let mut fence_conn = admin.acquire().await.unwrap();
                let mut fence = fence_conn.begin().await.unwrap();
                sqlx::query(
                    "SELECT code_hash, status FROM wf_definition
             WHERE wf_type = 'ci.pipeline' AND version = $1 FOR UPDATE",
                )
                .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
                .fetch_one(&mut *fence)
                .await
                .unwrap();

                let admission_pool = pinned_pool(&admin_url(), &schema, 2).await;
                let mut admission_conn = admission_pool.acquire().await.unwrap();
                let admission_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                    .fetch_one(&mut *admission_conn)
                    .await
                    .unwrap();
                let admission = tokio::spawn(async move {
                    let mut tx = admission_conn.begin().await.unwrap();
                    let status: String = sqlx::query_scalar(
                        "SELECT status FROM wf_definition
                 WHERE wf_type = 'ci.pipeline' AND version = $1 FOR SHARE",
                    )
                    .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
                    let eligible = status == "active";
                    tx.rollback().await.unwrap();
                    (status, eligible)
                });

                let observer = pinned_pool(&admin_url(), &schema, 2).await;
                wait_until_blocked(&observer, admission_pid).await;
                assert!(
                    !admission.is_finished(),
                    "the fresh admission must be blocked behind the cutover fence"
                );

                let backlog: bool = sqlx::query_scalar(
                    "SELECT myelin_ci_pipeline_version_has_nonterminal_runs($1)",
                )
                .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
                .fetch_one(&mut *fence)
                .await
                .unwrap();
                assert!(!backlog, "this schema has no predecessor runs");
                sqlx::query(
                    "UPDATE wf_definition SET status='draining'
             WHERE wf_type='ci.pipeline' AND version=$1 AND status='active'",
                )
                .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
                .execute(&mut *fence)
                .await
                .unwrap();
                sqlx::query(
                    "INSERT INTO wf_definition (wf_type, version, code_hash, status)
             VALUES ('ci.pipeline', $1, $2, 'active') ON CONFLICT DO NOTHING",
                )
                .bind(CI_MANIFEST_PIPELINE_VERSION)
                .bind(
                    myelin_ci_controlplane::ci_manifest_pipeline_definition()
                        .unwrap()
                        .code_hash(),
                )
                .execute(&mut *fence)
                .await
                .unwrap();
                fence.commit().await.unwrap();

                let (status, eligible) = admission.await.unwrap();
                assert_eq!(status, "draining");
                assert!(
            !eligible,
            "a draining definition is not eligible for a fresh start; this is the refusal the \
             already-deployed predecessor binary reports as CorruptRun"
        );

                let predecessor_runs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM workflow_run WHERE wf_type='ci.pipeline' AND wf_version=$1",
        )
        .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
        .fetch_one(&admin)
        .await
        .unwrap();
                assert_eq!(
                    predecessor_runs, 0,
                    "the fenced-out admission wrote no workflow"
                );
                for table in ["ci_drive_manifest", "ci_job", "job_queue"] {
                    let rows: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
                        .fetch_one(&admin)
                        .await
                        .unwrap();
                    assert_eq!(rows, 0, "the fenced-out admission wrote no {table} row");
                }
                observer.close().await;
                admission_pool.close().await;
            })
            .await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_injected_probe_failure_rolls_the_cutover_back_and_leaves_predecessor_active() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("probe_fail").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        admin
            .execute(
                format!(
                    "CREATE OR REPLACE FUNCTION
                       {schema}.myelin_ci_pipeline_version_has_nonterminal_runs(version integer)
                     RETURNS boolean LANGUAGE plpgsql AS $$
                     BEGIN RAISE EXCEPTION 'injected probe failure'; END $$;"
                )
                .as_str(),
            )
            .await
            .unwrap();
        let factory = cutover_factory(&admin, &schema).await;
        let refusal = factory
            .cutover_definition(&diagnostics(&admin))
            .await
            .expect_err("a probe that cannot be answered must fail closed");
        assert!(
            matches!(refusal, CiSupersededDefinitionGuardError::ProbeFailed(_)),
            "expected a probe failure, got {refusal:?}"
        );
        assert_eq!(
            definition_row(&admin, CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
                .await
                .map(|(_, status)| status),
            Some("active".into()),
            "the predecessor stays active so the old fleet keeps running"
        );
        assert_eq!(definition_row(&admin, CI_MANIFEST_PIPELINE_VERSION).await, None);

        admin
            .execute(
                format!(
                    "DROP FUNCTION {schema}.myelin_ci_pipeline_version_has_nonterminal_runs(integer)"
                )
                .as_str(),
            )
            .await
            .expect("remove the injected raising probe");
        install_schema_local_probe(&admin, &schema).await;
        factory
            .cutover_definition(&diagnostics(&admin))
            .await
            .expect("the next attempt commits the cutover");
        assert_eq!(
            definition_row(&admin, CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
                .await
                .map(|(_, status)| status),
            Some("draining".into())
        );
    })
    .await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_divergent_preexisting_current_hash_refuses_and_leaves_predecessor_active() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(
        &admin_url(),
        &["ci_cutover_", "ci_lease_topology_"],
        || async {
            let (schema, bootstrap, admin) = cutover_schema("hash_clash").await;
            with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
                sqlx::query(
                    "INSERT INTO wf_definition (wf_type, version, code_hash, status)
             VALUES ('ci.pipeline', $1, 'blake3:some-other-binarys-hash', 'active')",
                )
                .bind(CI_MANIFEST_PIPELINE_VERSION)
                .execute(&admin)
                .await
                .unwrap();

                let factory = cutover_factory(&admin, &schema).await;
                let refusal = factory
                    .cutover_definition(&diagnostics(&admin))
                    .await
                    .expect_err("a current row from a different source tree must refuse");
                assert!(
                    matches!(
                        refusal,
                        CiSupersededDefinitionGuardError::ActivationRefused(_)
                    ),
                    "expected an activation refusal, got {refusal:?}"
                );
                assert!(refusal.to_string().contains("DIFFERENT code hash"));
                assert_eq!(
                    definition_row(&admin, CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
                        .await
                        .map(|(_, status)| status),
                    Some("active".into()),
                    "the rollback leaves the predecessor active"
                );
            })
            .await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cutover_is_idempotent_across_reboots_and_never_reactivates_predecessor() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(
        &admin_url(),
        &["ci_cutover_", "ci_lease_topology_"],
        || async {
            let (schema, bootstrap, admin) = cutover_schema("idempotent").await;
            with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
                let factory = cutover_factory(&admin, &schema).await;
                factory
                    .cutover_definition(&diagnostics(&admin))
                    .await
                    .expect("first cutover");
                let after_first = definition_row(&admin, CI_MANIFEST_PIPELINE_VERSION).await;
                factory
                    .cutover_definition(&diagnostics(&admin))
                    .await
                    .expect("reboot cutover");
                factory
                    .cutover_definition(&diagnostics(&admin))
                    .await
                    .expect("third cutover");

                assert_eq!(
                    definition_row(&admin, CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
                        .await
                        .map(|(_, status)| status),
                    Some("draining".into()),
                    "a reboot NEVER reactivates the superseded version"
                );
                assert_eq!(
                    definition_row(&admin, CI_MANIFEST_PIPELINE_VERSION).await,
                    after_first,
                    "the activated row is byte-identical across reboots"
                );
                let (hash, status) = after_first.unwrap();
                assert_eq!(status, "active");
                assert_eq!(
                    hash,
                    myelin_ci_controlplane::ci_manifest_pipeline_definition()
                        .unwrap()
                        .code_hash()
                );
            })
            .await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_existing_predecessor_run_keeps_draining_while_a_fresh_predecessor_start_is_refused() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(
        &admin_url(),
        &["ci_cutover_", "ci_lease_topology_"],
        || async {
            let (schema, bootstrap, admin) = cutover_schema("drain").await;
            with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
                seed_workflow_run(
                    &admin,
                    REGION,
                    "draining-run",
                    CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION,
                    "running",
                )
                .await;
                sqlx::query(
            "UPDATE wf_definition SET status='draining' WHERE wf_type='ci.pipeline' AND version=$1",
        )
        .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
        .execute(&admin)
        .await
        .unwrap();

                let status: String = sqlx::query_scalar(
                    "SELECT status FROM wf_definition WHERE wf_type='ci.pipeline' AND version=$1",
                )
                .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
                .fetch_one(&admin)
                .await
                .unwrap();
                assert!(
                    matches!(status.as_str(), "active" | "draining"),
                    "an in-flight predecessor run may still be replayed while draining"
                );
                assert_ne!(
                    status, "active",
                    "a fresh predecessor start is refused once the definition drains"
                );

                let state: String = sqlx::query_scalar(
                    "SELECT state FROM workflow_run WHERE run_id='draining-run'",
                )
                .fetch_one(&admin)
                .await
                .unwrap();
                assert_eq!(state, "running");
            })
            .await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_backlog_in_another_region_still_refuses_the_database_global_cutover() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(
        &admin_url(),
        &["ci_cutover_", "ci_lease_topology_"],
        || async {
            let (schema, bootstrap, admin) = cutover_schema("global").await;
            with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
                seed_workflow_run(
                    &admin,
                    OTHER_REGION,
                    "other-region-predecessor",
                    CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION,
                    "running",
                )
                .await;

                let discovery = ci_region_run_discovery_test_support(admin.clone());
                let local = discovery
                    .superseded_definition_runs(REGION, CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION, 16)
                    .await
                    .unwrap();
                assert!(
                    local.is_empty(),
                    "the regional diagnostic is blind to the other region - by construction"
                );

                let global: bool = sqlx::query_scalar(
                    "SELECT myelin_ci_pipeline_version_has_nonterminal_runs($1)",
                )
                .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
                .fetch_one(&admin)
                .await
                .unwrap();
                assert!(global, "the global probe must see every region's backlog");

                let factory = cutover_factory(&admin, &schema).await;
                let refusal = factory
                    .cutover_definition(&diagnostics(&admin))
                    .await
                    .expect_err(
                        "a cross-region backlog must refuse the database-global transition",
                    );
                assert!(matches!(
                    refusal,
                    CiSupersededDefinitionGuardError::Backlog(_)
                ));
                assert_eq!(
                    definition_row(&admin, CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
                        .await
                        .map(|(_, status)| status),
                    Some("active".into())
                );
            })
            .await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_backlog_probe_is_executable_only_by_the_runtime_role() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("privilege").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let public_execute: bool = sqlx::query_scalar(
            "SELECT has_function_privilege('public',
               'myelin_ci_security.myelin_ci_pipeline_version_has_nonterminal_runs(integer)', 'EXECUTE')",
        )
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(!public_execute, "PUBLIC must never execute the global probe");
        let app_execute: bool = sqlx::query_scalar(
            "SELECT has_function_privilege('myelin_app',
               'myelin_ci_security.myelin_ci_pipeline_version_has_nonterminal_runs(integer)', 'EXECUTE')",
        )
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(
            app_execute,
            "the runtime role that registers wf_definition must be able to run the fence's probe"
        );
        let scheduler_execute: bool = sqlx::query_scalar(
            "SELECT has_function_privilege('myelin_ci_region_scheduler',
               'myelin_ci_security.myelin_ci_pipeline_version_has_nonterminal_runs(integer)', 'EXECUTE')",
        )
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(
            !scheduler_execute,
            "the scheduler capability has no business running the global registry fence"
        );

        let (security_definer, config): (bool, Option<Vec<String>>) = sqlx::query_as(
            "SELECT p.prosecdef, p.proconfig
             FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
             WHERE n.nspname='myelin_ci_security'
               AND p.proname='myelin_ci_pipeline_version_has_nonterminal_runs'",
        )
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(security_definer, "the probe must be SECURITY DEFINER");
        assert_eq!(
            config,
            Some(vec![
                "search_path=pg_catalog".to_string(),
                "row_security=off".to_string()
            ]),
            "a SECURITY DEFINER function without a pinned search_path is a privilege-escalation \
             hazard"
        );

        seed_workflow_run(
            &admin,
            REGION,
            "probe_visible",
            CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION,
            "running",
        )
        .await;
        let app = pinned_pool(&app_url(), &schema, 2).await;
        let direct_read: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM workflow_run WHERE wf_type='ci.pipeline' AND wf_version=$1",
        )
        .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
        .fetch_one(&app)
        .await
        .unwrap();
        assert_eq!(
            direct_read, 0,
            "FORCE RLS hides the row from the runtime role's own read - which is exactly why the \
             probe must be SECURITY DEFINER rather than an inline query"
        );
        let seen: bool = sqlx::query_scalar(
            "SELECT myelin_ci_pipeline_version_has_nonterminal_runs($1)",
        )
        .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
        .fetch_one(&app)
        .await
        .expect("the runtime role executes the probe");
        assert!(
            seen,
            "SECURITY DEFINER is what lets the NOBYPASSRLS runtime role see the global backlog"
        );
        app.close().await;
    })
    .await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_superseded_run_diagnostic_can_use_the_active_region_partial_index() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(
        &admin_url(),
        &["ci_cutover_", "ci_lease_topology_"],
        || async {
            let (schema, bootstrap, admin) = cutover_schema("explain").await;
            with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
                for index in 0..64 {
                    seed_workflow_run(
                        &admin,
                        REGION,
                        &format!("history_{index}"),
                        CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION,
                        "completed",
                    )
                    .await;
                }
                admin.execute("ANALYZE workflow_run").await.unwrap();

                let mut conn = admin.acquire().await.unwrap();
                sqlx::query("SET enable_seqscan = off")
                    .execute(&mut *conn)
                    .await
                    .unwrap();
                let plan: Vec<String> = sqlx::query_scalar(&format!(
                    "EXPLAIN {}",
                    myelin_ci_controlplane::DISCOVER_SUPERSEDED_CI_PIPELINE_RUNS_QUERY
                        .replace("$1", &format!("'{REGION}'"))
                        .replace("$2", &CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION.to_string())
                        .replace("$3", "16")
                ))
                .fetch_all(&mut *conn)
                .await
                .expect("explain the superseded-run diagnostic");
                let plan = plan.join("\n");
                assert!(
            plan.contains("ci_workflow_active_region"),
            "the diagnostic must be able to use the active-region partial index; plan was:\n{plan}"
        );

                let negative_plan: Vec<String> = sqlx::query_scalar(&format!(
                    "EXPLAIN SELECT tenant_id, run_id FROM workflow_run
             WHERE region = '{REGION}' AND wf_type = 'ci.pipeline'
               AND wf_version = {CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION}
               AND state NOT IN ('completed','failed','terminated','nondeterministic')
             ORDER BY tenant_id, run_id LIMIT 16"
                ))
                .fetch_all(&mut *conn)
                .await
                .unwrap();
                let negative_plan = negative_plan.join("\n");
                assert!(
            !negative_plan.contains("ci_workflow_active_region"),
            "if the NOT IN form were index-eligible this whole change would be unnecessary; plan \
             was:\n{negative_plan}"
        );
            })
            .await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_predecessor_row_refuses_instead_of_skipping_the_fence() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(
        &admin_url(),
        &["ci_cutover_", "ci_lease_topology_"],
        || async {
            let (schema, bootstrap, admin) = cutover_schema("no_predecessor").await;
            with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
                seed_workflow_run(
                    &admin,
                    REGION,
                    "orphaned-predecessor",
                    CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION,
                    "running",
                )
                .await;
                sqlx::query("DELETE FROM wf_definition WHERE wf_type='ci.pipeline' AND version=$1")
                    .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
                    .execute(&admin)
                    .await
                    .unwrap();

                let factory = cutover_factory(&admin, &schema).await;
                let refusal = factory
                    .cutover_definition(&diagnostics(&admin))
                    .await
                    .expect_err("a missing predecessor must fail closed");
                assert!(
                    matches!(
                        refusal,
                        CiSupersededDefinitionGuardError::PredecessorMissing
                    ),
                    "expected PredecessorMissing, got {refusal:?}"
                );
                let message = refusal.to_string();
                assert!(message.contains("ABSENT"));
                assert!(
                    message.contains("ci_0026_ci_pipeline_v4_cutover_fence_row"),
                    "the refusal must name the bootstrap remediation; got: {message}"
                );
                assert_eq!(
            definition_row(&admin, CI_MANIFEST_PIPELINE_VERSION).await,
            None,
            "the current version must not activate over a missing fence; the orphaned predecessor would strand"
        );

                admin
                    .execute(myelin_ci_controlplane::SEED_CI_PIPELINE_V4_CUTOVER_FENCE_ROW_DDL)
                    .await
                    .unwrap();
                let (hash, status) =
                    definition_row(&admin, CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
                        .await
                        .expect("the seed establishes the predecessor");
                assert_eq!(
                    status, "retired",
                    "a fresh database's predecessor is retired because it never ran there"
                );
                assert!(
                    hash.starts_with("sentinel:"),
                    "the seeded hash must never be mistakable for a real source-derived pin"
                );
                let refusal = factory
                    .cutover_definition(&diagnostics(&admin))
                    .await
                    .expect_err("the orphaned run is now probed");
                assert!(matches!(
                    refusal,
                    CiSupersededDefinitionGuardError::Backlog(_)
                ));
                assert!(refusal.to_string().contains("orphaned-predecessor"));
            })
            .await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_predecessor_seed_never_disturbs_an_existing_definition_row() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(
        &admin_url(),
        &["ci_cutover_", "ci_lease_topology_"],
        || async {
            let (schema, bootstrap, admin) = cutover_schema("seed_noop").await;
            with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
                let before = definition_row(&admin, CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION).await;
                assert_eq!(before.as_ref().map(|(_, s)| s.as_str()), Some("active"));
                for _ in 0..3 {
                    admin
                        .execute(myelin_ci_controlplane::SEED_CI_PIPELINE_V4_CUTOVER_FENCE_ROW_DDL)
                        .await
                        .expect("the seed is idempotent");
                }
                assert_eq!(
                    definition_row(&admin, CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION).await,
                    before,
                    "an existing predecessor row is never rewritten by the seed"
                );
            })
            .await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_probe_owner_has_bypass_authority_and_a_non_bypass_owner_fails_loudly() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("owner").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let (owner, bypass, superuser, can_login): (String, bool, bool, bool) = sqlx::query_as(
            "SELECT owner.rolname, owner.rolbypassrls, owner.rolsuper, owner.rolcanlogin
             FROM pg_proc p
             JOIN pg_namespace n ON n.oid = p.pronamespace
             JOIN pg_roles owner ON owner.oid = p.proowner
             WHERE n.nspname='myelin_ci_security'
               AND p.proname='myelin_ci_pipeline_version_has_nonterminal_runs'",
        )
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(
            owner, "myelin_ci_definition_fence",
            "the probe must be owned by the dedicated fence role, not whatever role happened to \
             create or replace it"
        );
        assert!(
            bypass || superuser,
            "the owner must be able to see past FORCE RLS, or the probe silently returns false"
        );
        assert!(
            !can_login,
            "the bypass authority must not be a connectable identity"
        );

        seed_workflow_run(
            &admin,
            REGION,
            "backlog_for_owner_probe",
            CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION,
            "running",
        )
        .await;
        let substitute = format!("myelin_test_nobypass_{}", std::process::id());
        let admin_for_role = admin.clone();
        let substitute_for_body = substitute.clone();
        common::with_throwaway_role(&admin_for_role, &substitute, || async move {
        let substitute = substitute_for_body;
        admin
            .execute(
                format!(
                    "DO $$ BEGIN
                       IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{substitute}') THEN
                         CREATE ROLE {substitute} NOLOGIN NOSUPERUSER NOBYPASSRLS;
                       END IF;
                     END $$;
                     GRANT USAGE ON SCHEMA {schema} TO {substitute};
                     GRANT SELECT ON TABLE {schema}.workflow_run TO {substitute};
                     GRANT {substitute} TO myelin_admin WITH INHERIT FALSE, SET FALSE;
                     ALTER FUNCTION {schema}.myelin_ci_pipeline_version_has_nonterminal_runs(integer)
                       OWNER TO {substitute};"
                )
                .as_str(),
            )
            .await
            .expect("re-own the schema-local probe to a NOBYPASSRLS role");
        let non_bypass: bool = sqlx::query_scalar(&format!(
            "SELECT rolbypassrls OR rolsuper FROM pg_roles WHERE rolname='{substitute}'"
        ))
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(!non_bypass, "the substitute owner must lack bypass authority");

        let loud = sqlx::query_scalar::<_, bool>(&format!(
            "SELECT {schema}.myelin_ci_pipeline_version_has_nonterminal_runs($1)"
        ))
        .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
        .fetch_one(&admin)
        .await;
        let error = loud.expect_err(
            "a non-bypass owner must RAISE, never silently return false - a false negative here is \
             a fail-open cutover",
        );
        assert!(
            error.to_string().contains("row-level security"),
            "expected the row_security=off refusal, got: {error}"
        );

        let factory = cutover_factory(&admin, &schema).await;
        let refusal = factory
            .cutover_definition(&diagnostics(&admin))
            .await
            .expect_err("an unanswerable probe must fail closed");
        assert!(
            matches!(refusal, CiSupersededDefinitionGuardError::ProbeFailed(_)),
            "expected ProbeFailed, got {refusal:?}"
        );
        assert_eq!(
            definition_row(&admin, CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
                .await
                .map(|(_, status)| status),
            Some("active".into()),
            "the predecessor stays active when the probe cannot be trusted"
        );
        assert_eq!(definition_row(&admin, CI_MANIFEST_PIPELINE_VERSION).await, None);

        admin
            .execute(
                format!(
                    "ALTER FUNCTION {schema}.myelin_ci_pipeline_version_has_nonterminal_runs(integer)
                       OWNER TO myelin_ci_definition_fence;
                     REVOKE ALL ON TABLE {schema}.workflow_run FROM {substitute};
                     REVOKE ALL ON SCHEMA {schema} FROM {substitute};"
                )
                .as_str(),
            )
            .await
            .expect("restore the fence owner");
        })
        .await;
    })
    .await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_indefinitely_held_fence_times_the_cutover_out_and_leaves_predecessor_active() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(
        &admin_url(),
        &["ci_cutover_", "ci_lease_topology_"],
        || async {
            let (schema, bootstrap, admin) = cutover_schema("lock_timeout").await;
            with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
                let holder_pool = pinned_pool(&admin_url(), &schema, 2).await;
                let mut holder_conn = holder_pool.acquire().await.unwrap();
                let mut holder = holder_conn.begin().await.unwrap();
                sqlx::query(
                    "SELECT status FROM wf_definition
             WHERE wf_type='ci.pipeline' AND version=$1 FOR UPDATE",
                )
                .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
                .fetch_one(&mut *holder)
                .await
                .unwrap();

                let factory = cutover_factory(&admin, &schema).await;
                let started = std::time::Instant::now();
                let refusal = factory
                    .cutover_definition(&diagnostics(&admin))
                    .await
                    .expect_err("a held fence must time out, not hang forever");
                let elapsed = started.elapsed();

                assert!(
                    matches!(
                        refusal,
                        CiSupersededDefinitionGuardError::FenceUnavailable(_)
                    ),
                    "expected FenceUnavailable, got {refusal:?}"
                );
                assert!(
                    refusal.to_string().contains(
                        &myelin_ci_controlplane::CI_DEFINITION_FENCE_LOCK_TIMEOUT_MS.to_string()
                    ),
                    "the refusal must state the bound it waited"
                );
                let bound = Duration::from_millis(
                    myelin_ci_controlplane::CI_DEFINITION_FENCE_LOCK_TIMEOUT_MS,
                );
                assert!(
                    elapsed >= bound.mul_f32(0.5),
                    "the cutover must actually wait for the fence, waited only {elapsed:?}"
                );
                assert!(
                    elapsed < bound * 3,
                    "the cutover must stop at its bound, waited {elapsed:?}"
                );

                assert_eq!(
                    definition_row(&admin, CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
                        .await
                        .map(|(_, status)| status),
                    Some("active".into()),
                    "a timed-out cutover leaves the old fleet fully operational"
                );
                assert_eq!(
                    definition_row(&admin, CI_MANIFEST_PIPELINE_VERSION).await,
                    None
                );

                holder.rollback().await.unwrap();
                drop(holder_conn);
                factory
                    .cutover_definition(&diagnostics(&admin))
                    .await
                    .expect("the cutover succeeds once the fence is free");
                holder_pool.close().await;
            })
            .await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_crash_between_ddl_commit_and_ledger_insert_retries_cleanly() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("crash_window").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let before: (i64, String, String, Vec<String>) = sqlx::query_as(
            "SELECT p.oid::bigint, pg_get_userbyid(p.proowner), btrim(p.prosrc), p.proconfig
               FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
              WHERE n.nspname = 'myelin_ci_security'
                AND p.proname = 'myelin_ci_pipeline_version_has_nonterminal_runs'",
        )
        .fetch_one(&admin)
        .await
        .expect("ci_0020h created the probe");

        let deleted = sqlx::query(
            "DELETE FROM myelin_applied_migration WHERE id = 'ci_0020h_ci_pipeline_version_backlog_probe'",
        )
        .execute(&admin)
        .await
        .expect("simulate the crash window");
        assert_eq!(deleted.rows_affected(), 1, "the ledger row existed to delete");

        PgMigrator::apply_validated(
            &admin,
            &myelin_ci_controlplane::ci_controlplane_migrations(),
            &myelin_ci_controlplane::ci_controlplane_hot_tables(),
        )
        .await
        .expect("the retry after a crash-window must succeed, not refuse");

        let after: (i64, String, String, Vec<String>) = sqlx::query_as(
            "SELECT p.oid::bigint, pg_get_userbyid(p.proowner), btrim(p.prosrc), p.proconfig
               FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
              WHERE n.nspname = 'myelin_ci_security'
                AND p.proname = 'myelin_ci_pipeline_version_has_nonterminal_runs'",
        )
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(
            after.0, before.0,
            "the function must be ADOPTED, not dropped and recreated - a changed OID would mean \
             the retry rewrote an object another operator may have been depending on"
        );
        assert_eq!(after.1, "myelin_ci_definition_fence");
        assert_eq!(after.2, before.2, "the body is untouched");
        assert_eq!(after.3, before.3, "the proconfig is untouched");

        let ledger: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM myelin_applied_migration
              WHERE id = 'ci_0020h_ci_pipeline_version_backlog_probe'",
        )
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(ledger, 1, "exactly one restored ledger row");

    })
    .await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_cutover_readiness_refuses_a_null_claim_window() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(
        &admin_url(),
        &["ci_cutover_", "ci_lease_topology_"],
        || async {
            let (schema, bootstrap, admin) = cutover_schema("production_ready_nullwin").await;
            with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
                seed_job_queue_row(
                    &admin,
                    "41111111-1111-1111-1111-111111111111",
                    None,
                    Some(2),
                    "queued",
                )
                .await;
                let pool = pinned_pool(&admin_url(), &schema, 2).await;
                let factory = cutover_factory(&pool, &schema).await;
                let error = factory
                    .cutover_definition(&diagnostics(&admin))
                    .await
                    .expect_err("the production cutover must reject a null claim window");
                assert!(
                    matches!(error, CiSupersededDefinitionGuardError::ActivationNotReady { unsafe_rows } if unsafe_rows == 1),
                    "expected one production-readiness refusal, got {error:?}"
                );
                assert_eq!(
                    definition_row(&admin, CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
                        .await
                        .map(|(_, status)| status),
                    Some("active".into())
                );
                assert_eq!(definition_row(&admin, CI_MANIFEST_PIPELINE_VERSION).await, None);
                pool.close().await;
            })
            .await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_cutover_readiness_is_database_global_for_a_null_marker() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(
        &admin_url(),
        &["ci_cutover_", "ci_lease_topology_"],
        || async {
            let (schema, bootstrap, admin) = cutover_schema("production_ready_global").await;
            with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
                let job_id = "42222222-2222-2222-2222-222222222222";
                seed_job_queue_row(&admin, job_id, Some(600), None, "running").await;
                sqlx::query("UPDATE job_queue SET region='us-east' WHERE job_id=$1::uuid")
                    .bind(job_id)
                    .execute(&admin)
                    .await
                    .unwrap();
                let pool = pinned_pool(&admin_url(), &schema, 2).await;
                let factory = cutover_factory(&pool, &schema).await;
                let error = factory
                    .cutover_definition(&diagnostics(&admin))
                    .await
                    .expect_err(
                        "an unsafe queue row outside the runner region must reject activation",
                    );
                assert!(
                    matches!(error, CiSupersededDefinitionGuardError::ActivationNotReady { unsafe_rows } if unsafe_rows == 1),
                    "expected one database-global production-readiness refusal, got {error:?}"
                );
                assert_eq!(
                    definition_row(&admin, CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
                        .await
                        .map(|(_, status)| status),
                    Some("active".into())
                );
                assert_eq!(definition_row(&admin, CI_MANIFEST_PIPELINE_VERSION).await, None);
                pool.close().await;
            })
            .await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_cutover_readiness_probe_failure_rolls_the_fence_back() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(
        &admin_url(),
        &["ci_cutover_", "ci_lease_topology_"],
        || async {
            let (schema, bootstrap, admin) = cutover_schema("production_ready_error").await;
            with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
                let pool = pinned_pool(&admin_url(), &schema, 2).await;
                let factory = cutover_factory(&pool, &schema)
                    .await
                    .replace_activation_readiness_probe_call_for_tests("SELECT (1 / 0)::bigint");
                let error = factory
                    .cutover_definition(&diagnostics(&admin))
                    .await
                    .expect_err("a failed production readiness probe must refuse activation");
                assert!(
                    matches!(error, CiSupersededDefinitionGuardError::ProbeFailed(_)),
                    "expected a fail-closed production probe refusal, got {error:?}"
                );
                assert_eq!(
                    definition_row(&admin, CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
                        .await
                        .map(|(_, status)| status),
                    Some("active".into())
                );
                assert_eq!(
                    definition_row(&admin, CI_MANIFEST_PIPELINE_VERSION).await,
                    None
                );
                pool.close().await;
            })
            .await;
        },
    )
    .await;
}

const NEXT_PREDECESSOR_VERSION: i32 = CI_MANIFEST_PIPELINE_VERSION;
const NEXT_CURRENT_VERSION: i32 = NEXT_PREDECESSOR_VERSION + 1;
const NEXT_CURRENT_HASH: &str = "blake3:synthetic-next-current-code-hash";

fn next_cutover_plan() -> CutoverPlan {
    CutoverPlan::for_tests(
        NEXT_PREDECESSOR_VERSION,
        NEXT_CURRENT_VERSION,
        NEXT_CURRENT_HASH,
    )
}

async fn upsert_definition_row(admin: &PgPool, version: i32, code_hash: &str, status: &str) {
    sqlx::query(
        "INSERT INTO wf_definition (wf_type, version, code_hash, status)
         VALUES ('ci.pipeline', $1, $2, $3)
         ON CONFLICT (wf_type, version)
         DO UPDATE SET code_hash = EXCLUDED.code_hash, status = EXCLUDED.status",
    )
    .bind(version)
    .bind(code_hash)
    .bind(status)
    .execute(admin)
    .await
    .unwrap();
}

async fn seed_next_predecessor(admin: &PgPool) {
    upsert_definition_row(
        admin,
        NEXT_PREDECESSOR_VERSION,
        "blake3:synthetic-next-predecessor",
        "active",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn next_an_old_predecessor_admission_holding_the_share_lock_is_observed_by_cutover() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("next_old_wins").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        seed_next_predecessor(&admin).await;

        let mut old_admission = admin.acquire().await.unwrap();
        let mut old_tx = old_admission.begin().await.unwrap();
        let locked: Option<String> = sqlx::query_scalar(
            "SELECT status FROM wf_definition
             WHERE wf_type = 'ci.pipeline' AND version = $1 FOR SHARE",
        )
        .bind(NEXT_PREDECESSOR_VERSION)
        .fetch_optional(&mut *old_tx)
        .await
        .unwrap();
        assert_eq!(
            locked.as_deref(),
            Some("active"),
            "the predecessor admission sees its definition active and proceeds"
        );

        let cutover_tag = format!("myelin-cutover-next-old-wins-{}", std::process::id());
        let cutover_pool = tagged_pool(&admin_url(), &schema, &cutover_tag, 2).await;
        let factory = cutover_factory(&cutover_pool, &schema).await;
        let diagnostics_owned = diagnostics(&admin);
        let cutover = tokio::spawn(async move {
            factory
                .cutover_definition_with_plan(&next_cutover_plan(), &diagnostics_owned)
                .await
        });

        let observer = pinned_pool(&admin_url(), &schema, 2).await;
        let mut waiting_pid = None;
        for _ in 0..400 {
            waiting_pid = sqlx::query_scalar::<_, i32>(
                "SELECT pid FROM pg_stat_activity
                 WHERE application_name = $1 AND wait_event_type = 'Lock' AND state = 'active'
                 LIMIT 1",
            )
            .bind(&cutover_tag)
            .fetch_optional(&observer)
            .await
            .unwrap();
            if waiting_pid.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let waiting_pid = waiting_pid.expect(
            "the next cutover's own backend must block behind the predecessor admission's share lock",
        );
        let blocked_on_definition: bool = sqlx::query_scalar(
            "SELECT query ILIKE '%wf_definition%' FROM pg_stat_activity WHERE pid = $1",
        )
        .bind(waiting_pid)
        .fetch_one(&observer)
        .await
        .unwrap();
        assert!(blocked_on_definition, "the cutover must wait on the wf_definition fence");
        assert!(
            !cutover.is_finished(),
            "the cutover cannot complete while the predecessor admission holds the fence"
        );

        sqlx::query(
            "INSERT INTO workflow_run (
               tenant_id, region, run_id, wf_type, wf_version, input, state, correlation_id,
               depth, partition
             ) VALUES ($1, $2, 'late-predecessor-run', 'ci.pipeline', $3, '[]'::jsonb, 'running', 'c', 0, 0)",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(NEXT_PREDECESSOR_VERSION)
        .execute(&mut *old_tx)
        .await
        .unwrap();
        old_tx.commit().await.unwrap();

        let refusal = cutover
            .await
            .unwrap()
            .expect_err("the cutover must observe the late predecessor admission and refuse");
        assert!(
            matches!(refusal, CiSupersededDefinitionGuardError::Backlog(_)),
            "expected a backlog refusal, got {refusal:?}"
        );

        assert_eq!(
            definition_row(&admin, NEXT_PREDECESSOR_VERSION).await.map(|(_, s)| s),
            Some("active".into()),
            "a refused cutover leaves the predecessor fleet fully operational"
        );
        assert_eq!(
            definition_row(&admin, NEXT_CURRENT_VERSION).await,
            None,
            "no next-version row may exist after a refused cutover"
        );
        observer.close().await;
        cutover_pool.close().await;
    })
    .await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn next_a_cutover_holding_the_update_lock_blocks_a_fresh_predecessor_admission() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(
        &admin_url(),
        &["ci_cutover_", "ci_lease_topology_"],
        || async {
            let (schema, bootstrap, admin) = cutover_schema("next_cutover_wins").await;
            with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
                seed_next_predecessor(&admin).await;

                let mut fence_conn = admin.acquire().await.unwrap();
                let mut fence = fence_conn.begin().await.unwrap();
                sqlx::query(
                    "SELECT code_hash, status FROM wf_definition
             WHERE wf_type = 'ci.pipeline' AND version = $1 FOR UPDATE",
                )
                .bind(NEXT_PREDECESSOR_VERSION)
                .fetch_one(&mut *fence)
                .await
                .unwrap();

                let admission_pool = pinned_pool(&admin_url(), &schema, 2).await;
                let mut admission_conn = admission_pool.acquire().await.unwrap();
                let admission_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                    .fetch_one(&mut *admission_conn)
                    .await
                    .unwrap();
                let admission = tokio::spawn(async move {
                    let mut tx = admission_conn.begin().await.unwrap();
                    let status: String = sqlx::query_scalar(
                        "SELECT status FROM wf_definition
                 WHERE wf_type = 'ci.pipeline' AND version = $1 FOR SHARE",
                    )
                    .bind(NEXT_PREDECESSOR_VERSION)
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
                    let eligible = status == "active";
                    tx.rollback().await.unwrap();
                    (status, eligible)
                });

                let observer = pinned_pool(&admin_url(), &schema, 2).await;
                wait_until_blocked(&observer, admission_pid).await;
                assert!(
                    !admission.is_finished(),
                    "the fresh predecessor admission must block behind the fence"
                );

                let backlog: bool = sqlx::query_scalar(
                    "SELECT myelin_ci_pipeline_version_has_nonterminal_runs($1)",
                )
                .bind(NEXT_PREDECESSOR_VERSION)
                .fetch_one(&mut *fence)
                .await
                .unwrap();
                assert!(!backlog, "this schema has no predecessor runs");
                sqlx::query(
                    "UPDATE wf_definition SET status='draining'
             WHERE wf_type='ci.pipeline' AND version=$1 AND status='active'",
                )
                .bind(NEXT_PREDECESSOR_VERSION)
                .execute(&mut *fence)
                .await
                .unwrap();
                sqlx::query(
                    "INSERT INTO wf_definition (wf_type, version, code_hash, status)
             VALUES ('ci.pipeline', $1, $2, 'active') ON CONFLICT DO NOTHING",
                )
                .bind(NEXT_CURRENT_VERSION)
                .bind(NEXT_CURRENT_HASH)
                .execute(&mut *fence)
                .await
                .unwrap();
                fence.commit().await.unwrap();

                let (status, eligible) = admission.await.unwrap();
                assert_eq!(status, "draining");
                assert!(
                    !eligible,
                    "a draining predecessor definition is not eligible for a fresh start"
                );

                let predecessor_runs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM workflow_run WHERE wf_type='ci.pipeline' AND wf_version=$1",
        )
        .bind(NEXT_PREDECESSOR_VERSION)
        .fetch_one(&admin)
        .await
        .unwrap();
                assert_eq!(
                    predecessor_runs, 0,
                    "the fenced-out admission wrote no workflow"
                );
                observer.close().await;
                admission_pool.close().await;
            })
            .await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn next_a_nonterminal_predecessor_run_blocks_activation() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(
        &admin_url(),
        &["ci_cutover_", "ci_lease_topology_"],
        || async {
            let (schema, bootstrap, admin) = cutover_schema("next_backlog").await;
            with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
                seed_next_predecessor(&admin).await;
                seed_workflow_run(
                    &admin,
                    REGION,
                    "predecessor-inflight",
                    NEXT_PREDECESSOR_VERSION,
                    "running",
                )
                .await;

                let factory = cutover_factory(&admin, &schema).await;
                let refusal = factory
                    .cutover_definition_with_plan(&next_cutover_plan(), &diagnostics(&admin))
                    .await
                    .expect_err("a non-terminal predecessor run must refuse the next cutover");
                assert!(
                    matches!(refusal, CiSupersededDefinitionGuardError::Backlog(_)),
                    "expected a backlog refusal, got {refusal:?}"
                );
                assert!(
                    refusal.to_string().contains("predecessor-inflight"),
                    "the refusal must name the stranding run for the operator; got: {refusal}"
                );
                assert_eq!(
                    definition_row(&admin, NEXT_PREDECESSOR_VERSION)
                        .await
                        .map(|(_, s)| s),
                    Some("active".into()),
                    "the refused cutover leaves the predecessor active"
                );
                assert_eq!(definition_row(&admin, NEXT_CURRENT_VERSION).await, None);
            })
            .await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn next_a_missing_predecessor_row_refuses_instead_of_skipping_the_fence() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(
        &admin_url(),
        &["ci_cutover_", "ci_lease_topology_"],
        || async {
            let (schema, bootstrap, admin) = cutover_schema("next_no_predecessor").await;
            with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
                seed_workflow_run(
                    &admin,
                    REGION,
                    "orphaned-predecessor",
                    NEXT_PREDECESSOR_VERSION,
                    "running",
                )
                .await;
                assert_eq!(
                    definition_row(&admin, NEXT_PREDECESSOR_VERSION).await,
                    None,
                    "the synthetic predecessor is deliberately absent for this case"
                );

                let factory = cutover_factory(&admin, &schema).await;
                let refusal = factory
                    .cutover_definition_with_plan(&next_cutover_plan(), &diagnostics(&admin))
                    .await
                    .expect_err("a missing predecessor must fail closed");
                assert!(
                    matches!(
                        refusal,
                        CiSupersededDefinitionGuardError::PredecessorMissing
                    ),
                    "expected PredecessorMissing, got {refusal:?}"
                );
                assert_eq!(
                    definition_row(&admin, NEXT_CURRENT_VERSION).await,
                    None,
                    "the next version must not be activated over a missing fence"
                );

                seed_next_predecessor(&admin).await;
                let refusal = factory
                    .cutover_definition_with_plan(&next_cutover_plan(), &diagnostics(&admin))
                    .await
                    .expect_err("the orphaned predecessor run is now probed");
                assert!(matches!(
                    refusal,
                    CiSupersededDefinitionGuardError::Backlog(_)
                ));
                assert!(refusal.to_string().contains("orphaned-predecessor"));
            })
            .await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn next_a_divergent_preexisting_current_hash_refuses_and_leaves_predecessor_active() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(
        &admin_url(),
        &["ci_cutover_", "ci_lease_topology_"],
        || async {
            let (schema, bootstrap, admin) = cutover_schema("next_hash_clash").await;
            with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
                seed_next_predecessor(&admin).await;
                upsert_definition_row(
                    &admin,
                    NEXT_CURRENT_VERSION,
                    "blake3:some-other-next-binarys-hash",
                    "active",
                )
                .await;

                let factory = cutover_factory(&admin, &schema).await;
                let refusal = factory
                    .cutover_definition_with_plan(&next_cutover_plan(), &diagnostics(&admin))
                    .await
                    .expect_err("a next-version row from a different source tree must refuse");
                assert!(
                    matches!(
                        refusal,
                        CiSupersededDefinitionGuardError::ActivationRefused(_)
                    ),
                    "expected an activation refusal, got {refusal:?}"
                );
                assert!(refusal.to_string().contains("DIFFERENT code hash"));
                assert!(
            refusal.to_string().contains(&format!("ci.pipeline@{NEXT_CURRENT_VERSION}")),
            "the refusal must name the PLAN's current version, not a hard-coded one; got: {refusal}"
        );
                assert_eq!(
                    definition_row(&admin, NEXT_PREDECESSOR_VERSION)
                        .await
                        .map(|(_, s)| s),
                    Some("active".into()),
                    "the rollback leaves the predecessor active"
                );
            })
            .await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn next_the_cutover_is_idempotent_across_reboots_and_never_reactivates_its_predecessor() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(
        &admin_url(),
        &["ci_cutover_", "ci_lease_topology_"],
        || async {
            let (schema, bootstrap, admin) = cutover_schema("next_idempotent").await;
            with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
                seed_next_predecessor(&admin).await;
                let factory = cutover_factory(&admin, &schema).await;

                factory
                    .cutover_definition_with_plan(&next_cutover_plan(), &diagnostics(&admin))
                    .await
                    .expect("first next-version cutover");
                let after_first = definition_row(&admin, NEXT_CURRENT_VERSION).await;
                factory
                    .cutover_definition_with_plan(&next_cutover_plan(), &diagnostics(&admin))
                    .await
                    .expect("reboot cutover");
                factory
                    .cutover_definition_with_plan(&next_cutover_plan(), &diagnostics(&admin))
                    .await
                    .expect("third cutover");

                assert_eq!(
                    definition_row(&admin, NEXT_PREDECESSOR_VERSION)
                        .await
                        .map(|(_, s)| s),
                    Some("draining".into()),
                    "a reboot never reactivates the predecessor"
                );
                assert_eq!(
                    definition_row(&admin, NEXT_CURRENT_VERSION).await,
                    after_first,
                    "the activated next-version row is byte-identical across reboots"
                );
                let (hash, status) = after_first.unwrap();
                assert_eq!(status, "active");
                assert_eq!(
                    hash, NEXT_CURRENT_HASH,
                    "the plan's current hash is what was activated"
                );
            })
            .await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn next_an_indefinitely_held_fence_times_out_and_leaves_the_predecessor_active() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(
        &admin_url(),
        &["ci_cutover_", "ci_lease_topology_"],
        || async {
            let (schema, bootstrap, admin) = cutover_schema("next_lock_timeout").await;
            with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
                seed_next_predecessor(&admin).await;

                let holder_pool = pinned_pool(&admin_url(), &schema, 2).await;
                let mut holder_conn = holder_pool.acquire().await.unwrap();
                let mut holder = holder_conn.begin().await.unwrap();
                sqlx::query(
                    "SELECT status FROM wf_definition
             WHERE wf_type='ci.pipeline' AND version=$1 FOR UPDATE",
                )
                .bind(NEXT_PREDECESSOR_VERSION)
                .fetch_one(&mut *holder)
                .await
                .unwrap();

                let factory = cutover_factory(&admin, &schema).await;
                let started = std::time::Instant::now();
                let refusal = factory
                    .cutover_definition_with_plan(&next_cutover_plan(), &diagnostics(&admin))
                    .await
                    .expect_err("a held predecessor fence must time out, not hang forever");
                let elapsed = started.elapsed();

                assert!(
                    matches!(
                        refusal,
                        CiSupersededDefinitionGuardError::FenceUnavailable(_)
                    ),
                    "expected FenceUnavailable, got {refusal:?}"
                );
                let bound = Duration::from_millis(
                    myelin_ci_controlplane::CI_DEFINITION_FENCE_LOCK_TIMEOUT_MS,
                );
                assert!(
                    elapsed >= bound.mul_f32(0.5),
                    "the cutover must actually wait, waited {elapsed:?}"
                );
                assert!(
                    elapsed < bound * 3,
                    "the cutover must stop at its bound, waited {elapsed:?}"
                );
                assert_eq!(
                    definition_row(&admin, NEXT_PREDECESSOR_VERSION)
                        .await
                        .map(|(_, s)| s),
                    Some("active".into()),
                    "a timed-out cutover leaves the predecessor active"
                );
                assert_eq!(definition_row(&admin, NEXT_CURRENT_VERSION).await, None);

                holder.rollback().await.unwrap();
                drop(holder_conn);
                factory
                    .cutover_definition_with_plan(&next_cutover_plan(), &diagnostics(&admin))
                    .await
                    .expect("the next-version cutover succeeds once the fence is free");
                holder_pool.close().await;
            })
            .await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn next_an_ambiguous_commit_fails_closed_and_leaves_the_registry_whole() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(
        &admin_url(),
        &["ci_cutover_", "ci_lease_topology_"],
        || async {
            let (schema, bootstrap, admin) = cutover_schema("next_commit_ambiguity").await;
            with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
                seed_next_predecessor(&admin).await;

                admin
                    .execute(
                        format!(
                            "CREATE OR REPLACE FUNCTION {schema}.fail_wf_definition_commit()
                       RETURNS trigger LANGUAGE plpgsql AS
                       $$ BEGIN RAISE EXCEPTION 'injected commit-time failure'; END $$;
                     CREATE CONSTRAINT TRIGGER myelin_next_commit_ambiguity
                       AFTER INSERT OR UPDATE ON {schema}.wf_definition
                       DEFERRABLE INITIALLY DEFERRED
                       FOR EACH ROW EXECUTE FUNCTION {schema}.fail_wf_definition_commit();"
                        )
                        .as_str(),
                    )
                    .await
                    .expect("install the deferred commit-failure trigger");

                let factory = cutover_factory(&admin, &schema).await;
                let refusal = factory
                    .cutover_definition_with_plan(&next_cutover_plan(), &diagnostics(&admin))
                    .await
                    .expect_err("a commit that raises must fail closed, not report success");
                assert!(
                    matches!(refusal, CiSupersededDefinitionGuardError::ProbeFailed(_)),
                    "an ambiguous commit is surfaced as ProbeFailed, got {refusal:?}"
                );
                assert!(
                    refusal.to_string().contains("ambiguous"),
                    "the refusal must flag the ambiguous-commit window; got: {refusal}"
                );

                assert_eq!(
                    definition_row(&admin, NEXT_PREDECESSOR_VERSION)
                        .await
                        .map(|(_, s)| s),
                    Some("active".into()),
                    "a failed commit rolls the transition back; the predecessor stays active"
                );
                assert_eq!(
                    definition_row(&admin, NEXT_CURRENT_VERSION).await,
                    None,
                    "the cutover is atomic: no half-applied next-version row survives"
                );

                admin
                    .execute(
                        format!(
                            "DROP TRIGGER myelin_next_commit_ambiguity ON {schema}.wf_definition;
                     DROP FUNCTION {schema}.fail_wf_definition_commit();"
                        )
                        .as_str(),
                    )
                    .await
                    .expect("remove the commit-failure trigger");
                factory
                    .cutover_definition_with_plan(&next_cutover_plan(), &diagnostics(&admin))
                    .await
                    .expect("with the commit trigger gone the retry commits the next cutover");
                assert_eq!(
                    definition_row(&admin, NEXT_PREDECESSOR_VERSION)
                        .await
                        .map(|(_, s)| s),
                    Some("draining".into()),
                    "the successful retry drains the predecessor"
                );
                let (hash, status) = definition_row(&admin, NEXT_CURRENT_VERSION)
                    .await
                    .expect("the next version is now active");
                assert_eq!(status, "active");
                assert_eq!(hash, NEXT_CURRENT_HASH);
            })
            .await;
        },
    )
    .await;
}

async fn install_schema_local_readiness_probe(admin: &PgPool, schema: &str) {
    admin
        .execute(
            format!(
                "GRANT SELECT (region, state, claim_window_secs, reservation_write_version)
                   ON TABLE {schema}.job_queue TO myelin_ci_definition_fence;
                 SET LOCAL ROLE myelin_ci_definition_fence;
                 CREATE OR REPLACE FUNCTION {schema}.myelin_ci_v2_activation_readiness_unsafe_count()
                 RETURNS bigint LANGUAGE sql STABLE SECURITY DEFINER
                 SET search_path = pg_catalog SET row_security = off
                 AS $probe$SELECT count(*) FROM {schema}.job_queue
                   WHERE state <> 'terminal'
                     AND (claim_window_secs IS NULL
                          OR reservation_write_version IS DISTINCT FROM 2)$probe$;
                 RESET ROLE;"
            )
            .as_str(),
        )
        .await
        .expect("install the schema-local readiness probe as the fence role");
}

fn schema_local_readiness_call(schema: &str) -> String {
    format!("SELECT {schema}.myelin_ci_v2_activation_readiness_unsafe_count()")
}

fn next_cutover_plan_with_readiness(schema: &str) -> CutoverPlan {
    next_cutover_plan().with_activation_readiness(ActivationReadinessProbe::with_call_for_tests(
        schema_local_readiness_call(schema),
    ))
}

async fn seed_job_queue_row(
    admin: &PgPool,
    job_id: &str,
    claim_window_secs: Option<i64>,
    reservation_write_version: Option<i16>,
    state: &str,
) {
    sqlx::query(
        "INSERT INTO job_queue (
           tenant_id, region, job_id, run_id, lane, trust_tier, fair_key, idem_token, state,
           claim_window_secs, reservation_write_version
         ) VALUES ($1, $2, $3::uuid, gen_random_uuid(), 'batch', 'trusted', $1, $4, $5, $6, $7)",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(job_id)
    .bind(job_id)
    .bind(state)
    .bind(claim_window_secs)
    .bind(reservation_write_version)
    .execute(admin)
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn next_readiness_refuses_on_a_nonterminal_null_claim_window_row() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("next_ready_nullwin").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        seed_next_predecessor(&admin).await;
        install_schema_local_readiness_probe(&admin, &schema).await;
        seed_job_queue_row(&admin, "11111111-1111-1111-1111-111111111111", None, Some(2), "queued")
            .await;

        let factory = cutover_factory(&pinned_pool(&admin_url(), &schema, 2).await, &schema).await;
        let error = factory
            .cutover_definition_with_plan(&next_cutover_plan_with_readiness(&schema), &diagnostics(&admin))
            .await
            .expect_err("a NULL-claim-window non-terminal row must refuse the activation");
        assert!(
            matches!(error, CiSupersededDefinitionGuardError::ActivationNotReady { unsafe_rows } if unsafe_rows == 1),
            "the readiness predicate refuses with the unsafe-row count, got {error:?}"
        );
        assert_eq!(
            definition_row(&admin, NEXT_PREDECESSOR_VERSION).await.map(|(_, s)| s),
            Some("active".into()),
            "the fence rolled back: the predecessor remains active"
        );
        assert_eq!(
            definition_row(&admin, NEXT_CURRENT_VERSION).await,
            None,
            "the next version was never activated"
        );
    })
    .await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn next_readiness_refuses_on_a_nonterminal_non_two_reservation_marker() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("next_ready_marker").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        seed_next_predecessor(&admin).await;
        install_schema_local_readiness_probe(&admin, &schema).await;
        seed_job_queue_row(&admin, "22222222-2222-2222-2222-222222222222", Some(600), None, "running")
            .await;

        let factory = cutover_factory(&pinned_pool(&admin_url(), &schema, 2).await, &schema).await;
        let error = factory
            .cutover_definition_with_plan(&next_cutover_plan_with_readiness(&schema), &diagnostics(&admin))
            .await
            .expect_err("a non-2 reservation marker must refuse the activation");
        assert!(
            matches!(error, CiSupersededDefinitionGuardError::ActivationNotReady { unsafe_rows } if unsafe_rows == 1),
            "the readiness predicate refuses with the unsafe-row count, got {error:?}"
        );
        assert_eq!(
            definition_row(&admin, NEXT_PREDECESSOR_VERSION).await.map(|(_, s)| s),
            Some("active".into()),
            "the fence rolled back: the predecessor remains active"
        );
        assert_eq!(
            definition_row(&admin, NEXT_CURRENT_VERSION).await,
            None,
            "the next version was never activated"
        );
    })
    .await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn next_readiness_fails_closed_on_a_null_probe_result() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("next_ready_null").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        seed_next_predecessor(&admin).await;
        let plan = next_cutover_plan().with_activation_readiness(
            ActivationReadinessProbe::with_call_for_tests("SELECT NULL::bigint"),
        );
        let factory = cutover_factory(&pinned_pool(&admin_url(), &schema, 2).await, &schema).await;
        let error = factory
            .cutover_definition_with_plan(&plan, &diagnostics(&admin))
            .await
            .expect_err("a NULL readiness result must fail closed");
        assert!(
            matches!(&error, CiSupersededDefinitionGuardError::ProbeFailed(detail) if detail.contains("NULL")),
            "a NULL count is fail-closed, got {error:?}"
        );
        assert_eq!(
            definition_row(&admin, NEXT_PREDECESSOR_VERSION).await.map(|(_, s)| s),
            Some("active".into()),
            "the fence rolled back: the predecessor remains active"
        );
        assert_eq!(
            definition_row(&admin, NEXT_CURRENT_VERSION).await,
            None,
            "the next version was never activated"
        );
    })
    .await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn next_readiness_fails_closed_on_a_probe_error() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(
        &admin_url(),
        &["ci_cutover_", "ci_lease_topology_"],
        || async {
            let (schema, bootstrap, admin) = cutover_schema("next_ready_err").await;
            with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
                seed_next_predecessor(&admin).await;
                let plan = next_cutover_plan().with_activation_readiness(
                    ActivationReadinessProbe::with_call_for_tests(
                        "SELECT myelin_ci_security.readiness_probe_that_does_not_exist()",
                    ),
                );
                let factory =
                    cutover_factory(&pinned_pool(&admin_url(), &schema, 2).await, &schema).await;
                let error = factory
                    .cutover_definition_with_plan(&plan, &diagnostics(&admin))
                    .await
                    .expect_err("a readiness probe error must fail closed");
                assert!(
                    matches!(error, CiSupersededDefinitionGuardError::ProbeFailed(_)),
                    "a probe error is fail-closed, got {error:?}"
                );
                assert_eq!(
                    definition_row(&admin, NEXT_PREDECESSOR_VERSION)
                        .await
                        .map(|(_, s)| s),
                    Some("active".into()),
                    "the fence rolled back: the predecessor remains active"
                );
            })
            .await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn next_readiness_activates_over_a_safe_queue() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(
        &admin_url(),
        &["ci_cutover_", "ci_lease_topology_"],
        || async {
            let (schema, bootstrap, admin) = cutover_schema("next_ready_clean").await;
            with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
                seed_next_predecessor(&admin).await;
                install_schema_local_readiness_probe(&admin, &schema).await;
                seed_job_queue_row(
                    &admin,
                    "33333333-3333-3333-3333-333333333333",
                    Some(600),
                    Some(2),
                    "running",
                )
                .await;
                seed_job_queue_row(
                    &admin,
                    "44444444-4444-4444-4444-444444444444",
                    None,
                    None,
                    "terminal",
                )
                .await;

                let factory =
                    cutover_factory(&pinned_pool(&admin_url(), &schema, 2).await, &schema).await;
                factory
                    .cutover_definition_with_plan(
                        &next_cutover_plan_with_readiness(&schema),
                        &diagnostics(&admin),
                    )
                    .await
                    .expect("a safe queue must let the next-version activation proceed");
                assert_eq!(
                    definition_row(&admin, NEXT_PREDECESSOR_VERSION)
                        .await
                        .map(|(_, s)| s),
                    Some("draining".into()),
                    "the clean activation drains the predecessor"
                );
                let (hash, status) = definition_row(&admin, NEXT_CURRENT_VERSION)
                    .await
                    .expect("the next version is active");
                assert_eq!(status, "active");
                assert_eq!(hash, NEXT_CURRENT_HASH);
            })
            .await;
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn next_a_none_readiness_plan_ignores_unsafe_rows() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(
        &admin_url(),
        &["ci_cutover_", "ci_lease_topology_"],
        || async {
            let (schema, bootstrap, admin) = cutover_schema("next_ready_none").await;
            with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
                seed_next_predecessor(&admin).await;
                seed_job_queue_row(
                    &admin,
                    "55555555-5555-5555-5555-555555555555",
                    None,
                    None,
                    "queued",
                )
                .await;

                let plan = next_cutover_plan();
                assert!(
                    !plan.has_activation_readiness(),
                    "the control plan carries no predicate"
                );
                let factory =
                    cutover_factory(&pinned_pool(&admin_url(), &schema, 2).await, &schema).await;
                factory
                    .cutover_definition_with_plan(&plan, &diagnostics(&admin))
                    .await
                    .expect("a None-readiness plan proceeds regardless of unsafe queue rows");
                assert_eq!(
                    definition_row(&admin, NEXT_PREDECESSOR_VERSION)
                        .await
                        .map(|(_, s)| s),
                    Some("draining".into()),
                    "the no-readiness cutover drains the predecessor despite the unsafe row"
                );
                assert_eq!(
                    definition_row(&admin, NEXT_CURRENT_VERSION)
                        .await
                        .map(|(_, s)| s),
                    Some("active".into()),
                    "the next version activates because no readiness probe was configured"
                );
            })
            .await;
        },
    )
    .await;
}

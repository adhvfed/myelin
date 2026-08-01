//! Live-PostgreSQL proof for the CT-007 `ci.pipeline` definition CUTOVER FENCE.
//!
//! A version bump strands every non-terminal run pinned to the old version: a Flow worker claims
//! only locally-registered `(wf_type, version)` keys, so a v3-only binary can never drive a v2 row.
//! A preflight `SELECT` cannot prevent that — an old-binary starter transaction already in flight
//! can commit a fresh v3 workflow after the snapshot. The fence closes it by reusing the lock the
//! old binary ALREADY takes: `validate_definition_pin` holds `wf_definition@3 FOR SHARE` until its
//! start transaction resolves, so the cutover's `FOR UPDATE` on that row is genuine mutual
//! exclusion.
//!
//! The two barrier drills below are the load-bearing tests, run at the same rigor as the
//! preparation/launch CAS race: real concurrent connections, a real `std::sync::Barrier`, and a
//! positive proof that the loser is genuinely BLOCKED rather than merely slower.
#![cfg(feature = "integration")]

mod common;

use std::time::Duration;

use common::with_schema_cleanup;
use myelin_ci_controlplane::{
    ci_production_runtime_factory_test_support, ci_region_run_discovery_test_support,
    ActivationReadinessProbe, CiSupersededDefinitionGuardError, CutoverPlan,
    CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION, CI_MANIFEST_PIPELINE_VERSION,
};
use myelin_storage::{DurableCostLedger, HotTables, PgMigrator, SubstrateProvider};
use myelin_config::MyelinConfig;
use myelin_tenancy::Region;
use sqlx::{Acquire, Executor, PgPool, Row};

/// Independent `PgMigrator` sequences against one live PostgreSQL deadlock on the migration
/// advisory lock when run concurrently.
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



/// A schema carrying the real Flow + CI migration sets, with `ci.pipeline@3` seeded `active` — the
/// pre-cutover registry state a rolling deploy actually starts from.
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
    // Install a SCHEMA-LOCAL copy of the database-wide probe from the SAME production DDL text,
    // rewritten to this schema (the `.replace(...)` fixture convention this crate already uses for
    // `job_queue`/`workflow_run`). Without it the probe would read `public.workflow_run` and be
    // blind to every row these isolated tests seed. `cutover_definition` names the function
    // unqualified, so the connection's `search_path` selects this copy.
    install_schema_local_probe(&admin, &schema).await;
    install_schema_local_readiness_probe(&admin, &schema).await;
    // `ci_0022d` seeds the predecessor row as `retired` (the fresh-database sentinel). These tests
    // model an EXISTING fleet mid-rolling-deploy, so promote it to the deployed-v3 shape: an active
    // row carrying a real-looking legacy hash.
    sqlx::query(
        "INSERT INTO wf_definition (wf_type, version, code_hash, status)
         VALUES ('ci.pipeline', $1, 'blake3:legacy-v3-hash', 'active')
         ON CONFLICT (wf_type, version)
         DO UPDATE SET code_hash = EXCLUDED.code_hash, status = EXCLUDED.status",
    )
    .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
    .execute(&admin)
    .await
    .unwrap();
    (schema, bootstrap, admin)
}

/// **The schema-local mirror of the production probe.** The production `ci_0020h` DDL is one
/// transaction that VERIFIES operator provisioning and then creates the function in the real
/// `myelin_ci_security` schema — neither of which an isolated per-test schema can satisfy. So the
/// fixture builds the same function shape (same owner, volatility, security, `proconfig` and body,
/// pointed at this schema's `workflow_run`) and the factory's test seam points the fence at it. The
/// PRODUCTION shape itself is asserted separately, against the real `public`/`myelin_ci_security`
/// objects, by `the_backlog_probe_is_executable_only_by_the_runtime_role`.
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

/// A pool whose connections carry a unique `application_name`, so `pg_stat_activity` can identify
/// exactly which backend is lock-waiting rather than "some backend touching wf_definition".
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

/// The production cutover factory, over the given pool.
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
    // Production resolution stays `public.`-qualified (round-3 blocker 3). A schema-isolated
    // fixture cannot see `public.workflow_run`, so it points the fence at its OWN schema's copy of
    // the probe through the dedicated test seam rather than the production call being weakened.
    .with_backlog_probe_call_for_tests(format!(
        "SELECT {schema}.myelin_ci_pipeline_version_has_nonterminal_runs($1)"
    ))
    .replace_activation_readiness_probe_call_for_tests(schema_local_readiness_call(schema))
}

/// The scheduler-shaped diagnostic discovery these tests pass to the cutover. Real deployments pass
/// `CiSchedulerDbProvider::region_run_discovery()`; the verdict authority is the global probe either
/// way, so an admin-backed discovery is honest here.
fn diagnostics(admin: &PgPool) -> myelin_ci_controlplane::CiRegionRunDiscovery {
    ci_region_run_discovery_test_support(admin.clone())
}

async fn definition_row(admin: &PgPool, version: i32) -> Option<(String, String)> {
    sqlx::query("SELECT code_hash, status FROM wf_definition WHERE wf_type='ci.pipeline' AND version=$1")
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

/// Is `pid` genuinely waiting on a lock right now? This is what turns "the other side hasn't
/// finished yet" into a positive proof of blocking.
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
    panic!("connection {pid} never entered a lock wait — the fence is not actually exclusive");
}

// ═════════════ 1. old admission wins the lock ════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_old_v3_admission_holding_the_share_lock_makes_the_cutover_observe_its_workflow() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("old_wins").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        // Connection A: the EXACT lock the old v3 starter takes, held open across a barrier.
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
            "the old binary sees v3 active and proceeds"
        );

        // Connection B: attempt the cutover concurrently on a UNIQUELY TAGGED pool, so the
        // lock-wait proof identifies THIS backend rather than accepting any concurrent activity on
        // the shared dev database (round-3 finding 6).
        let cutover_tag = format!("myelin-cutover-old-wins-{}", std::process::id());
        let cutover_pool = tagged_pool(&admin_url(), &schema, &cutover_tag, 2).await;
        let factory = cutover_factory(&cutover_pool, &schema).await;
        let diagnostics_owned = diagnostics(&admin);
        let cutover =
            tokio::spawn(async move { factory.cutover_definition(&diagnostics_owned).await });

        // Wait until a backend WITH THAT EXACT application_name is blocked on a lock.
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

        // A now commits a fresh v3 workflow — exactly the race a preflight SELECT would have missed.
        sqlx::query(
            "INSERT INTO workflow_run (
               tenant_id, region, run_id, wf_type, wf_version, input, state, correlation_id,
               depth, partition
             ) VALUES ($1, $2, 'late-v2-run', 'ci.pipeline', $3, '[]'::jsonb, 'running', 'c', 0, 0)",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
        .execute(&mut *old_tx)
        .await
        .unwrap();
        old_tx.commit().await.unwrap();

        // B wakes, its post-lock snapshot SEES the committed v3 run, and it refuses.
        let refusal = cutover
            .await
            .unwrap()
            .expect_err("the cutover must observe the late v2 admission and refuse");
        assert!(
            matches!(refusal, CiSupersededDefinitionGuardError::Backlog(_)),
            "expected a backlog refusal, got {refusal:?}"
        );

        // v3 is untouched and v4 was never registered.
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
            "no v4 row may exist after a refused cutover"
        );
        observer.close().await;
        cutover_pool.close().await;
    })
    .await;
    })
    .await;
}

// ═════════════ 2. cutover wins the lock ══════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cutover_holding_the_update_lock_blocks_and_then_refuses_a_fresh_v3_admission() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("cutover_wins").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        // Connection B: take the cutover's own fence by hand so the barrier is deterministic, then
        // perform exactly the transition the production path performs.
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

        // Connection A: a fresh old-binary admission now attempts its FOR SHARE and must BLOCK.
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
            // This mirrors `validate_definition_pin`'s own eligibility rule for a FRESH start.
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

        // The fence observes zero backlog and commits both halves atomically.
        let backlog: bool = sqlx::query_scalar(
            "SELECT myelin_ci_pipeline_version_has_nonterminal_runs($1)",
        )
        .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
        .fetch_one(&mut *fence)
        .await
        .unwrap();
        assert!(!backlog, "this schema has no v3 runs");
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
        .bind(myelin_ci_controlplane::ci_manifest_pipeline_definition().code_hash())
        .execute(&mut *fence)
        .await
        .unwrap();
        fence.commit().await.unwrap();

        // A wakes and sees `draining` — the exact status the deployed v3 binary refuses a fresh
        // start on, before it writes a manifest, jobs, or a workflow.
        let (status, eligible) = admission.await.unwrap();
        assert_eq!(status, "draining");
        assert!(
            !eligible,
            "a draining definition is not eligible for a FRESH start — this is the refusal the \
             already-deployed v3 binary reports as CorruptRun"
        );

        // Nothing was admitted under v3.
        let v3_runs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM workflow_run WHERE wf_type='ci.pipeline' AND wf_version=$1",
        )
        .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(v3_runs, 0, "the fenced-out admission wrote no workflow");
        for table in ["ci_drive_manifest", "ci_job", "job_queue"] {
            let rows: i64 =
                sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
                    .fetch_one(&admin)
                    .await
                    .unwrap();
            assert_eq!(rows, 0, "the fenced-out admission wrote no {table} row");
        }
        observer.close().await;
        admission_pool.close().await;
    })
    .await;
    })
    .await;
}

// ═════════════ 3. probe failure rolls the fence back ═════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_injected_probe_failure_rolls_the_cutover_back_and_leaves_v3_active() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("probe_fail").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        // Shadow the SECURITY DEFINER probe with a raising one in the schema's own search path.
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
            "v2 stays active so the old fleet keeps running"
        );
        assert_eq!(definition_row(&admin, CI_MANIFEST_PIPELINE_VERSION).await, None);

        // With the real probe restored, the retried cutover succeeds — proving the refusal was the
        // guard failing closed, not the cutover being broken.
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

// ═════════════ 4. a divergent pre-seeded v3 hash refuses ═════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_divergent_preexisting_v3_hash_refuses_and_leaves_v3_active() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
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
            .expect_err("a v3 row from a different source tree must refuse");
        assert!(
            matches!(refusal, CiSupersededDefinitionGuardError::ActivationRefused(_)),
            "expected an activation refusal, got {refusal:?}"
        );
        assert!(refusal.to_string().contains("DIFFERENT code hash"));
        assert_eq!(
            definition_row(&admin, CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
                .await
                .map(|(_, status)| status),
            Some("active".into()),
            "the rollback leaves v2 active"
        );
    })
    .await;
    })
    .await;
}

// ═════════════ 5. idempotent reboot ══════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cutover_is_idempotent_across_reboots_and_never_reactivates_v3() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("idempotent").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let factory = cutover_factory(&admin, &schema).await;
        factory.cutover_definition(&diagnostics(&admin)).await.expect("first cutover");
        let after_first = definition_row(&admin, CI_MANIFEST_PIPELINE_VERSION).await;
        factory.cutover_definition(&diagnostics(&admin)).await.expect("reboot cutover");
        factory.cutover_definition(&diagnostics(&admin)).await.expect("third cutover");

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
            myelin_ci_controlplane::ci_manifest_pipeline_definition().code_hash()
        );
    })
    .await;
    })
    .await;
}

// ═════════════ 6. drain compatibility ════════════════════════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_existing_v3_run_keeps_draining_while_a_fresh_v3_start_is_refused() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("drain").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        // Mark v3 draining directly — the post-cutover state — with an existing v3 run present.
        seed_workflow_run(&admin, REGION, "draining-run", CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION, "running").await;
        sqlx::query(
            "UPDATE wf_definition SET status='draining' WHERE wf_type='ci.pipeline' AND version=$1",
        )
        .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
        .execute(&admin)
        .await
        .unwrap();

        // `validate_definition_pin`'s rule: replay accepts `active | draining`; a fresh start
        // requires `active`. Assert both halves against the durable status.
        let status: String = sqlx::query_scalar(
            "SELECT status FROM wf_definition WHERE wf_type='ci.pipeline' AND version=$1",
        )
        .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(
            matches!(status.as_str(), "active" | "draining"),
            "an in-flight v3 run may still be replayed/driven while draining"
        );
        assert_ne!(
            status, "active",
            "a FRESH v2 start is refused once the definition drains"
        );

        // And the run itself is untouched by the cutover — draining is not cancelling.
        let state: String =
            sqlx::query_scalar("SELECT state FROM workflow_run WHERE run_id='draining-run'")
                .fetch_one(&admin)
                .await
                .unwrap();
        assert_eq!(state, "running");
    })
    .await;
    })
    .await;
}

// ═════════════ 7. the fence is DATABASE-WIDE, not regional ═══════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_backlog_in_another_region_still_refuses_the_database_global_cutover() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("global").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        // A stranded v3 run in a DIFFERENT region. `wf_definition` has no region column, so
        // draining v2 here would fence that region out too.
        seed_workflow_run(
            &admin,
            OTHER_REGION,
            "other-region-v2",
            CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION,
            "running",
        )
        .await;

        // The REGIONAL diagnostic sees nothing — which is exactly why it cannot be the authority.
        let discovery = ci_region_run_discovery_test_support(admin.clone());
        let local = discovery
            .superseded_definition_runs(REGION, CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION, 16)
            .await
            .unwrap();
        assert!(
            local.is_empty(),
            "the regional diagnostic is blind to the other region — by construction"
        );

        // The DATABASE-WIDE probe sees it, and the cutover refuses.
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
            .expect_err("a cross-region backlog must refuse the database-global transition");
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
    })
    .await;
}

// ═════════════ 8. the probe's least-privilege boundary ═══════════════════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_backlog_probe_is_executable_only_by_the_runtime_role() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("privilege").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        // PUBLIC must not hold EXECUTE, and the runtime role must.
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

        // Hardening: SECURITY DEFINER with a pinned search_path and no dynamic SQL.
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

        // And the runtime role really can call it end to end. The row is seeded by admin; the point
        // is that `myelin_app` — NOBYPASSRLS, and with NO tenant/region GUC set, so its own RLS
        // scope would hide this row entirely — still gets a truthful global answer.
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
            "FORCE RLS hides the row from the runtime role's own read — which is exactly why the \
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

// ═════════════ 9. the production admission path still takes the fence ════════════════════════════

/// **Source pin: fresh production `ci.pipeline` admission still locks the superseded definition row
/// `FOR SHARE` before `start_with_id_on_conn`.** The whole fence rests on that lock existing in the
/// old binary's path; if a refactor dropped or reordered it, the cutover would stop being mutually
/// exclusive with admission and every race test above would silently become vacuous.
#[test]
fn production_admission_locks_the_definition_row_before_starting_the_workflow() {
    let starter = include_str!("../src/pg_pipeline_starter.rs");
    // The lock itself still exists, on the exact pinned row, and is `FOR SHARE` (the mode the
    // cutover's `FOR UPDATE` conflicts with).
    assert!(
        starter.contains("WHERE wf_type = $1 AND version = $2 FOR SHARE"),
        "fresh admission must lock the pinned definition row FOR SHARE"
    );
    // And it is taken BEFORE the workflow start, inside the same admission transaction. Compare
    // CALL SITES, not definition order: `validate_definition_pin` is defined far below its callers.
    let fresh_validate = starter
        .find("validate_definition_pin(&mut transaction, &self.definition, false)")
        .expect("a fresh (non-replay) start validates the definition pin");
    let start = starter
        .find("start_with_id_on_conn(")
        .expect("admission starts the workflow on the same connection");
    assert!(
        fresh_validate < start,
        "the definition FOR SHARE lock must be taken BEFORE the workflow is started, or the \
         cutover fence is not mutually exclusive with admission"
    );
    // Both run on the SAME transaction, so the lock is still held at insert time.
    assert!(
        starter.contains("HandlerTx::with_connection(&mut *transaction)"),
        "the workflow start must ride the same transaction that holds the definition lock"
    );
    assert!(
        starter.contains("pinned workflow definition status `{status}` is not eligible for this start"),
        "the draining refusal the fenced-out old binary reports must stay in the admission path"
    );
    // The test-support-only driver must never be a production admission path.
    let driver = include_str!("../src/ci_pipeline_driver.rs");
    let driver_start = driver
        .find("pub struct CiPipelineDriver")
        .expect("the test-support driver exists");
    let gate = driver[..driver_start].rfind("#[cfg(any(test, feature = \"test-support\"))]");
    assert!(
        gate.is_some(),
        "CiPipelineDriver must stay behind the dev-only test-support boundary"
    );
}

// ═════════════ 10. the clean-path diagnostic is index-eligible ═══════════════════════════════════

/// **The superseded-run diagnostic must not seq-scan the durable workflow history.** The clean case
/// — no backlog — is the one every restart takes, and `LIMIT` cannot help when no row matches: a
/// sequential scan reads the whole table before returning nothing.
///
/// This asserts ELIGIBILITY, not the default plan: on a tiny fixture PostgreSQL will rationally
/// prefer a seq scan regardless, so forcing `enable_seqscan = off` and requiring the planner to be
/// ABLE to use `ci_workflow_active_region` is the honest question. The earlier `NOT IN (terminal
/// states)` form fails this even with seq scans disabled, because the planner cannot prove that
/// predicate implies the partial index's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_superseded_run_diagnostic_can_use_the_active_region_partial_index() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("explain").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        // The partial index is created CONCURRENTLY by the migration; make sure the planner has
        // statistics rather than an empty-relation default.
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

        // The negative control: the predicate form this slice REPLACED cannot use the index even
        // with sequential scans disabled — which is exactly why it was replaced.
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
    })
    .await;
}

// ═════════════ 11. a missing predecessor row FAILS CLOSED ════════════════════════════════════════

/// **Absence is never "nothing to fence" (round-3 blocker 1).** With no `ci.pipeline@3` row there is
/// nothing to lock `FOR UPDATE`, so a concurrently-booting older binary could `register_definition`
/// the superseded version against no conflicting lock and reopen late admission — and an orphaned
/// non-terminal run under it would never be probed. The old code took `commit_activation(tx, None)`
/// on this path and activated v3 anyway.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_predecessor_row_refuses_instead_of_skipping_the_fence() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("no_predecessor").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        // An orphaned non-terminal v3 run, and NO v3 definition row — the exact shape that used to
        // activate v3 and strand it.
        seed_workflow_run(
            &admin,
            REGION,
            "orphaned_v2",
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
            message.contains("ci_0022d_ci_pipeline_v3_cutover_fence_row"),
            "the refusal must name the bootstrap remediation; got: {message}"
        );
        assert_eq!(
            definition_row(&admin, CI_MANIFEST_PIPELINE_VERSION).await,
            None,
            "v4 must NOT be activated over a missing fence — the orphaned v3 run would strand"
        );

        // The migration's seed is what makes this unreachable on a real fresh database.
        admin
            .execute(myelin_ci_controlplane::SEED_CI_PIPELINE_V3_CUTOVER_FENCE_ROW_DDL)
            .await
            .unwrap();
        let (hash, status) = definition_row(&admin, CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
            .await
            .expect("the seed establishes the predecessor");
        assert_eq!(
            status, "retired",
            "a fresh database's predecessor is retired: v2 never ran here and must never be admitted"
        );
        assert!(
            hash.starts_with("sentinel:"),
            "the seeded hash must never be mistakable for a real source-derived pin"
        );
        // With the fence row present the backlog is still real, so the cutover still refuses — now
        // for the right reason, with remediation ids.
        let refusal = factory
            .cutover_definition(&diagnostics(&admin))
            .await
            .expect_err("the orphaned run is now probed");
        assert!(matches!(
            refusal,
            CiSupersededDefinitionGuardError::Backlog(_)
        ));
        assert!(refusal.to_string().contains("orphaned_v2"));
    })
    .await;
    })
    .await;
}

/// The seed is a strict no-op against an existing database — the property that makes it safe as an
/// additive migration on a fleet that already has a deployed v2.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_predecessor_seed_never_disturbs_an_existing_definition_row() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("seed_noop").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let before = definition_row(&admin, CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION).await;
        assert_eq!(before.as_ref().map(|(_, s)| s.as_str()), Some("active"));
        for _ in 0..3 {
            admin
                .execute(myelin_ci_controlplane::SEED_CI_PIPELINE_CUTOVER_FENCE_ROW_DDL)
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
    })
    .await;
}

// ═════════════ 12. the probe's OWNER carries the bypass authority ════════════════════════════════

/// **A SECURITY DEFINER function's RLS authority is its OWNER's (round-3 blocker 2).** The intended
/// production migration role is a non-superuser schema owner WITHOUT `BYPASSRLS`; owning this probe
/// as that role would leave the `EXISTS` silently filtered — false despite a real backlog, a
/// fail-OPEN cutover. So ownership, the owner's capability, and the `row_security=off` safety net
/// are all asserted structurally.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_probe_owner_has_bypass_authority_and_a_non_bypass_owner_fails_loudly() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("owner").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        // The PRODUCTION function's owner is the dedicated fence role, and that role really does
        // carry bypass authority — asserted on the catalogue, not assumed from provisioning.
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

        // The safety net: with `row_security = off` pinned, an owner WITHOUT bypass authority
        // produces a LOUD error instead of a silent false negative. Prove it by re-owning the
        // schema-local copy to a non-bypass role and calling it with a real backlog present.
        seed_workflow_run(
            &admin,
            REGION,
            "backlog_for_owner_probe",
            CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION,
            "running",
        )
        .await;
        // A DEDICATED throwaway non-bypass role, never a production capability role: granting a
        // real capability role anything (even in a temporary schema) would trip the scheduler
        // provider's excess-privilege probe in any concurrently-booting test.
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
            "a non-bypass owner must RAISE, never silently return false — a false negative here is \
             a fail-open cutover",
        );
        assert!(
            error.to_string().contains("row-level security"),
            "expected the row_security=off refusal, got: {error}"
        );

        // And the cutover therefore fails CLOSED rather than draining v2 over a real backlog.
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
            "v2 stays active when the probe cannot be trusted"
        );
        assert_eq!(definition_row(&admin, CI_MANIFEST_PIPELINE_VERSION).await, None);

        // Re-own before the schema drops, so the throwaway role can be removed and leaves no
        // lingering grant for any other test's privilege probe to see.
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

// ═════════════ 13. the fence wait is bounded ═════════════════════════════════════════════════════

/// **An abandoned admission transaction must not hang boot (round-3 finding 5).** PostgreSQL's
/// default `lock_timeout` is 0 — wait forever. With the bounded transaction-local timeout the
/// cutover reaches its typed fail-closed error instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_indefinitely_held_fence_times_the_cutover_out_and_leaves_v3_active() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("lock_timeout").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        // A stalled admission holding the fence and never resolving.
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
            refusal
                .to_string()
                .contains(&myelin_ci_controlplane::CI_DEFINITION_FENCE_LOCK_TIMEOUT_MS.to_string()),
            "the refusal must state the bound it waited"
        );
        // It genuinely waited (so the bound is real, not an instant failure) and genuinely stopped
        // (so boot cannot hang).
        let bound = Duration::from_millis(myelin_ci_controlplane::CI_DEFINITION_FENCE_LOCK_TIMEOUT_MS);
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
        assert_eq!(definition_row(&admin, CI_MANIFEST_PIPELINE_VERSION).await, None);

        // Once the holder resolves, the retry succeeds.
        holder.rollback().await.unwrap();
        drop(holder_conn);
        factory
            .cutover_definition(&diagnostics(&admin))
            .await
            .expect("the cutover succeeds once the fence is free");
        holder_pool.close().await;
    })
    .await;
    })
    .await;
}

// ═════════════ 14. the migration-ledger crash window ═════════════════════════════════════════════

/// **A crash between `ci_0020h`'s DDL commit and its ledger insert must be safely retryable.**
/// The migration's DDL is atomic on its own — `PgMigrator` sends it as one Simple Query message, so
/// PostgreSQL runs every statement in one IMPLICIT transaction and a `RAISE` rolls the whole prefix
/// back (an explicit `BEGIN` there would instead leave the pooled connection in `25P02`). But the
/// ledger row is a separate later statement, so the crash window between them is real. On retry the migration re-adopts the fence
/// role, finds the function already present and EXACTLY as expected, adopts it without replacement,
/// re-normalizes grants idempotently, and commits. The function's OID is unchanged, which is what
/// proves it was adopted rather than dropped and recreated.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_crash_between_ddl_commit_and_ledger_insert_retries_cleanly() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("crash_window").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        // The real production DDL, applied to `public` by `cutover_schema`'s migration run. Read
        // the committed identity before simulating the crash.
        let before: (i64, String, String, Vec<String>) = sqlx::query_as(
            "SELECT p.oid::bigint, pg_get_userbyid(p.proowner), btrim(p.prosrc), p.proconfig
               FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
              WHERE n.nspname = 'myelin_ci_security'
                AND p.proname = 'myelin_ci_pipeline_version_has_nonterminal_runs'",
        )
        .fetch_one(&admin)
        .await
        .expect("ci_0020h created the probe");

        // THE CRASH: the DDL committed, the ledger insert did not.
        let deleted = sqlx::query(
            "DELETE FROM myelin_applied_migration WHERE id = 'ci_0020h_ci_pipeline_version_backlog_probe'",
        )
        .execute(&admin)
        .await
        .expect("simulate the crash window");
        assert_eq!(deleted.rows_affected(), 1, "the ledger row existed to delete");

        // REBOOT: re-apply the complete set. This is the real migrator, not a hand-run script.
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
            "the function must be ADOPTED, not dropped and recreated — a changed OID would mean \
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

        // THE DIVERGENT NEGATIVE CONTROL LIVES IN THE DISPOSABLE DRILL, NOT HERE.
        //
        // `myelin_ci_security` is a GLOBAL schema (the migration qualifies it explicitly), so
        // tampering with the probe to prove the adopt-or-create branch refuses would mutate a shared
        // object. No restoration discipline makes that safe on a shared stack: a panic between the
        // DROP and the re-apply strands every later test AND the dev stack with no probe and no
        // `ci_0020h` ledger row, and `CI_PRIVILEGE_FIXTURE_LOCK` is only observed by cooperating
        // tests — a production boot or an unrelated migrator never takes it.
        //
        // `integration_ci_definition_fence_fresh` runs that control against a throwaway container
        // where tampering is free, and does so under the non-superuser migration posture as well.
        // What stays here is only the NON-DESTRUCTIVE half: the crash-window retry above, which
        // adopts the existing function without modifying it.
    })
    .await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn production_v3_to_v4_readiness_refuses_a_null_claim_window() {
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
                    .expect_err("the production v3→v4 plan must reject a NULL claim window");
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
async fn production_v3_to_v4_readiness_is_database_global_for_a_null_marker() {
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
async fn production_v3_to_v4_readiness_probe_failure_rolls_the_fence_back() {
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
                    .replace_activation_readiness_probe_call_for_tests(
                        "SELECT (1 / 0)::bigint",
                    );
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
                assert_eq!(definition_row(&admin, CI_MANIFEST_PIPELINE_VERSION).await, None);
                pool.close().await;
            })
            .await;
        },
    )
    .await;
}

// ═════════════════════════════════════════════════════════════════════════════════════════════════
//  SYNTHETIC v4→v5 CUTOVER SUITE (CT-007 slice 5b.3-6d step 5).
//
//  The cutover fence is now GENERALIZED over a typed `CutoverPlan{predecessor, current, hash}`. The
//  production wrapper runs ONLY the v3→v4 plan — every test above exercises that production path.
//  This suite drives the SAME generalized fence body over a synthetic
//  v4→v5 plan, with the v4/v5 `wf_definition` rows seeded IN-TEST (slice 6e.2 ships the production
//  v3→v4 activation + the retired-v3 fresh-DB sentinel migration; this suite proves the fence
//  generalizes). The predecessor is v4, the current is a synthetic v5 that no production root runs.
//
//  Reuses the frozen fixtures above verbatim: `cutover_schema` (Flow + CI migrations, the version-
//  parameterized schema-local backlog probe, the tagged/pinned pools, `pg_stat_activity` lock-wait
//  hygiene, `with_privilege_fixture_lock` / `with_schema_cleanup`).
// ═════════════════════════════════════════════════════════════════════════════════════════════════

/// The synthetic predecessor of the v4→v5 pair — the current production version, which a v4→v5
/// cutover would supersede. No production code runs this plan; it exists only in this suite.
const V4V5_PREDECESSOR_VERSION: i32 = 4;
/// The synthetic current version. Deliberately one past the production pin so nothing in this repo
/// registers it outside these tests.
const V4V5_CURRENT_VERSION: i32 = 5;
/// The source-derived pin a real v5 binary would embed — synthetic here.
const V4V5_CURRENT_HASH: &str = "blake3:synthetic-v5-current-code-hash";

/// The (v4, v5) plan under test. Production never constructs this — only `CutoverPlan::for_tests`,
/// the test-support seam, can build an arbitrary pair.
fn v4_to_v5_plan() -> CutoverPlan {
    assert_eq!(
        V4V5_PREDECESSOR_VERSION,
        CI_MANIFEST_PIPELINE_VERSION,
        "the synthetic suite must begin at the version this binary actually activates"
    );
    CutoverPlan::for_tests(V4V5_PREDECESSOR_VERSION, V4V5_CURRENT_VERSION, V4V5_CURRENT_HASH)
}

/// Seed (or overwrite) a `wf_definition` row for the synthetic suite.
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

/// Seed the pre-cutover v5-active predecessor a mid-deploy v4→v5 fleet starts from. The fence never
/// consults the predecessor hash — only its existence — so a synthetic legacy hash is honest.
async fn seed_v4_predecessor(admin: &PgPool) {
    upsert_definition_row(admin, V4V5_PREDECESSOR_VERSION, "blake3:synthetic-v4-predecessor", "active")
        .await;
}

// ─── v4→v5 · 1. old admission wins the lock ──────────────────────────────────────────────────────

/// The generalized-fence twin of `an_old_v2_admission_holding_the_share_lock…`: an in-flight v5
/// admission holding `wf_definition@3 FOR SHARE` makes the v4→v5 cutover's OWN backend BLOCK on that
/// row (proven via `pg_stat_activity`), commit a late v5 run, and the woken cutover then observes it
/// under the fence and refuses. Proves the generalized factory takes `FOR UPDATE` on the PLAN's
/// predecessor (v5), not a hard-coded v2.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v4v5_an_old_v4_admission_holding_the_share_lock_makes_the_cutover_observe_its_workflow() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("v4v5_old_wins").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        seed_v4_predecessor(&admin).await;

        // Connection A: the exact lock a v5 starter takes on the PLAN's predecessor row.
        let mut old_admission = admin.acquire().await.unwrap();
        let mut old_tx = old_admission.begin().await.unwrap();
        let locked: Option<String> = sqlx::query_scalar(
            "SELECT status FROM wf_definition
             WHERE wf_type = 'ci.pipeline' AND version = $1 FOR SHARE",
        )
        .bind(V4V5_PREDECESSOR_VERSION)
        .fetch_optional(&mut *old_tx)
        .await
        .unwrap();
        assert_eq!(locked.as_deref(), Some("active"), "the v5 binary sees v5 active and proceeds");

        // Connection B: the generalized v4→v5 cutover on a uniquely tagged pool.
        let cutover_tag = format!("myelin-cutover-v4v5-old-wins-{}", std::process::id());
        let cutover_pool = tagged_pool(&admin_url(), &schema, &cutover_tag, 2).await;
        let factory = cutover_factory(&cutover_pool, &schema).await;
        let diagnostics_owned = diagnostics(&admin);
        let cutover = tokio::spawn(async move {
            factory
                .cutover_definition_with_plan(&v4_to_v5_plan(), &diagnostics_owned)
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
            "the v4→v5 cutover's OWN backend must BLOCK behind the v5 admission's FOR SHARE",
        );
        let blocked_on_definition: bool = sqlx::query_scalar(
            "SELECT query ILIKE '%wf_definition%' FROM pg_stat_activity WHERE pid = $1",
        )
        .bind(waiting_pid)
        .fetch_one(&observer)
        .await
        .unwrap();
        assert!(blocked_on_definition, "the cutover must wait on the wf_definition fence");
        assert!(!cutover.is_finished(), "the cutover cannot complete while v5 holds the fence");

        // A commits a fresh v5 workflow — the race a preflight SELECT would miss.
        sqlx::query(
            "INSERT INTO workflow_run (
               tenant_id, region, run_id, wf_type, wf_version, input, state, correlation_id,
               depth, partition
             ) VALUES ($1, $2, 'late-v4-run', 'ci.pipeline', $3, '[]'::jsonb, 'running', 'c', 0, 0)",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(V4V5_PREDECESSOR_VERSION)
        .execute(&mut *old_tx)
        .await
        .unwrap();
        old_tx.commit().await.unwrap();

        let refusal = cutover
            .await
            .unwrap()
            .expect_err("the cutover must observe the late v5 admission and refuse");
        assert!(
            matches!(refusal, CiSupersededDefinitionGuardError::Backlog(_)),
            "expected a backlog refusal, got {refusal:?}"
        );

        // v5 untouched, v5 never registered.
        assert_eq!(
            definition_row(&admin, V4V5_PREDECESSOR_VERSION).await.map(|(_, s)| s),
            Some("active".into()),
            "a refused cutover leaves the v5 fleet fully operational"
        );
        assert_eq!(
            definition_row(&admin, V4V5_CURRENT_VERSION).await,
            None,
            "no v5 row may exist after a refused cutover"
        );
        observer.close().await;
        cutover_pool.close().await;
    })
    .await;
    })
    .await;
}

// ─── v4→v5 · 2. cutover wins the lock ────────────────────────────────────────────────────────────

/// The generalized-fence twin of `a_cutover_holding_the_update_lock…`: with the fence held on v5 by
/// hand (so the barrier is deterministic), a fresh v5 admission's `FOR SHARE` genuinely BLOCKS, and
/// once the transition (drain v5, activate v5) commits it wakes to `draining` — the status the
/// deployed v5 binary refuses a fresh start on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v4v5_a_cutover_holding_the_update_lock_blocks_and_then_refuses_a_fresh_v4_admission() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("v4v5_cutover_wins").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        seed_v4_predecessor(&admin).await;

        let mut fence_conn = admin.acquire().await.unwrap();
        let mut fence = fence_conn.begin().await.unwrap();
        sqlx::query(
            "SELECT code_hash, status FROM wf_definition
             WHERE wf_type = 'ci.pipeline' AND version = $1 FOR UPDATE",
        )
        .bind(V4V5_PREDECESSOR_VERSION)
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
            .bind(V4V5_PREDECESSOR_VERSION)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
            let eligible = status == "active";
            tx.rollback().await.unwrap();
            (status, eligible)
        });

        let observer = pinned_pool(&admin_url(), &schema, 2).await;
        wait_until_blocked(&observer, admission_pid).await;
        assert!(!admission.is_finished(), "the fresh v5 admission must block behind the fence");

        // The exact transition the generalized fence performs for a (v5, v5) plan.
        let backlog: bool =
            sqlx::query_scalar("SELECT myelin_ci_pipeline_version_has_nonterminal_runs($1)")
                .bind(V4V5_PREDECESSOR_VERSION)
                .fetch_one(&mut *fence)
                .await
                .unwrap();
        assert!(!backlog, "this schema has no v5 runs");
        sqlx::query(
            "UPDATE wf_definition SET status='draining'
             WHERE wf_type='ci.pipeline' AND version=$1 AND status='active'",
        )
        .bind(V4V5_PREDECESSOR_VERSION)
        .execute(&mut *fence)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO wf_definition (wf_type, version, code_hash, status)
             VALUES ('ci.pipeline', $1, $2, 'active') ON CONFLICT DO NOTHING",
        )
        .bind(V4V5_CURRENT_VERSION)
        .bind(V4V5_CURRENT_HASH)
        .execute(&mut *fence)
        .await
        .unwrap();
        fence.commit().await.unwrap();

        let (status, eligible) = admission.await.unwrap();
        assert_eq!(status, "draining");
        assert!(!eligible, "a draining v5 definition is not eligible for a FRESH start");

        let v4_runs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM workflow_run WHERE wf_type='ci.pipeline' AND wf_version=$1",
        )
        .bind(V4V5_PREDECESSOR_VERSION)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(v4_runs, 0, "the fenced-out admission wrote no workflow");
        observer.close().await;
        admission_pool.close().await;
    })
    .await;
    })
    .await;
}

// ─── v4→v5 · 3. backlog refuses ──────────────────────────────────────────────────────────────────

/// A non-terminal v5 run blocks v4→v5 activation: the version-parameterized backlog probe, bound to
/// the PLAN's predecessor (3), sees it and the generalized fence fails closed with v5 left active.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v4v5_a_nonterminal_v4_run_blocks_activation_and_leaves_v4_active() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("v4v5_backlog").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        seed_v4_predecessor(&admin).await;
        seed_workflow_run(&admin, REGION, "v4-inflight", V4V5_PREDECESSOR_VERSION, "running").await;

        let factory = cutover_factory(&admin, &schema).await;
        let refusal = factory
            .cutover_definition_with_plan(&v4_to_v5_plan(), &diagnostics(&admin))
            .await
            .expect_err("a non-terminal v5 run must refuse the v4→v5 cutover");
        assert!(
            matches!(refusal, CiSupersededDefinitionGuardError::Backlog(_)),
            "expected a backlog refusal, got {refusal:?}"
        );
        assert!(
            refusal.to_string().contains("v4-inflight"),
            "the refusal must name the stranding run for the operator; got: {refusal}"
        );
        assert_eq!(
            definition_row(&admin, V4V5_PREDECESSOR_VERSION).await.map(|(_, s)| s),
            Some("active".into()),
            "the refused cutover leaves v5 active"
        );
        assert_eq!(definition_row(&admin, V4V5_CURRENT_VERSION).await, None);
    })
    .await;
    })
    .await;
}

// ─── v4→v5 · 4. missing predecessor fails closed ─────────────────────────────────────────────────

/// No v5 row → nothing to lock `FOR UPDATE` → the generalized fence returns `PredecessorMissing`
/// rather than vacuously activating v5 over an absent fence. Mirrors the v2→v5 blocker-1 proof for
/// the PLAN's predecessor.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v4v5_a_missing_v4_predecessor_row_refuses_instead_of_skipping_the_fence() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("v4v5_no_predecessor").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        // An orphaned non-terminal v5 run and NO v5 definition row — the shape that would strand.
        seed_workflow_run(&admin, REGION, "orphaned_v4", V4V5_PREDECESSOR_VERSION, "running").await;
        // `cutover_schema` seeds only the v2 predecessor; there is no v5 row to delete.
        assert_eq!(
            definition_row(&admin, V4V5_PREDECESSOR_VERSION).await,
            None,
            "the synthetic v4 predecessor is deliberately absent for this case"
        );

        let factory = cutover_factory(&admin, &schema).await;
        let refusal = factory
            .cutover_definition_with_plan(&v4_to_v5_plan(), &diagnostics(&admin))
            .await
            .expect_err("a missing v4 predecessor must fail closed");
        assert!(
            matches!(refusal, CiSupersededDefinitionGuardError::PredecessorMissing),
            "expected PredecessorMissing, got {refusal:?}"
        );
        assert_eq!(
            definition_row(&admin, V4V5_CURRENT_VERSION).await,
            None,
            "v5 must NOT be activated over a missing fence"
        );

        // With the fence row present the orphaned run is now probed and the cutover refuses for the
        // right reason.
        seed_v4_predecessor(&admin).await;
        let refusal = factory
            .cutover_definition_with_plan(&v4_to_v5_plan(), &diagnostics(&admin))
            .await
            .expect_err("the orphaned v5 run is now probed");
        assert!(matches!(refusal, CiSupersededDefinitionGuardError::Backlog(_)));
        assert!(refusal.to_string().contains("orphaned_v4"));
    })
    .await;
    })
    .await;
}

// ─── v4→v5 · 5. divergent current hash refuses ───────────────────────────────────────────────────

/// A pre-existing v5 row from a DIFFERENT source tree (wrong hash) makes the generalized fence refuse
/// with `ActivationRefused`, rolling back with v5 still active — the plan's `current_code_hash` is
/// the authority, not a hard-coded v5 pin.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v4v5_a_divergent_preexisting_v5_hash_refuses_and_leaves_v4_active() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("v4v5_hash_clash").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        seed_v4_predecessor(&admin).await;
        upsert_definition_row(&admin, V4V5_CURRENT_VERSION, "blake3:some-other-v5-binarys-hash", "active")
            .await;

        let factory = cutover_factory(&admin, &schema).await;
        let refusal = factory
            .cutover_definition_with_plan(&v4_to_v5_plan(), &diagnostics(&admin))
            .await
            .expect_err("a v5 row from a different source tree must refuse");
        assert!(
            matches!(refusal, CiSupersededDefinitionGuardError::ActivationRefused(_)),
            "expected an activation refusal, got {refusal:?}"
        );
        assert!(refusal.to_string().contains("DIFFERENT code hash"));
        assert!(
            refusal.to_string().contains(&format!("ci.pipeline@{V4V5_CURRENT_VERSION}")),
            "the refusal must name the PLAN's current version, not a hard-coded one; got: {refusal}"
        );
        assert_eq!(
            definition_row(&admin, V4V5_PREDECESSOR_VERSION).await.map(|(_, s)| s),
            Some("active".into()),
            "the rollback leaves v5 active"
        );
    })
    .await;
    })
    .await;
}

// ─── v4→v5 · 6. idempotent reboot ────────────────────────────────────────────────────────────────

/// Re-running the v4→v5 cutover after it has committed is a no-op: v5 stays `draining` (never
/// resurrected to active), and the v5 row is byte-identical to the plan across reboots.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v4v5_the_cutover_is_idempotent_across_reboots_and_never_reactivates_v4() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("v4v5_idempotent").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        seed_v4_predecessor(&admin).await;
        let factory = cutover_factory(&admin, &schema).await;

        factory
            .cutover_definition_with_plan(&v4_to_v5_plan(), &diagnostics(&admin))
            .await
            .expect("first v4→v5 cutover");
        let after_first = definition_row(&admin, V4V5_CURRENT_VERSION).await;
        factory
            .cutover_definition_with_plan(&v4_to_v5_plan(), &diagnostics(&admin))
            .await
            .expect("reboot cutover");
        factory
            .cutover_definition_with_plan(&v4_to_v5_plan(), &diagnostics(&admin))
            .await
            .expect("third cutover");

        assert_eq!(
            definition_row(&admin, V4V5_PREDECESSOR_VERSION).await.map(|(_, s)| s),
            Some("draining".into()),
            "a reboot NEVER reactivates the superseded v5"
        );
        assert_eq!(
            definition_row(&admin, V4V5_CURRENT_VERSION).await,
            after_first,
            "the activated v5 row is byte-identical across reboots"
        );
        let (hash, status) = after_first.unwrap();
        assert_eq!(status, "active");
        assert_eq!(hash, V4V5_CURRENT_HASH, "the plan's current hash is what was activated");
    })
    .await;
    })
    .await;
}

// ─── v4→v5 · 7. bounded fence wait times out ─────────────────────────────────────────────────────

/// An indefinitely held lock on the PLAN's predecessor (v5) makes the generalized cutover reach its
/// typed `FenceUnavailable` at the shared bound, then succeed once released — boot cannot hang.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v4v5_an_indefinitely_held_fence_times_the_cutover_out_and_leaves_v4_active() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("v4v5_lock_timeout").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        seed_v4_predecessor(&admin).await;

        let holder_pool = pinned_pool(&admin_url(), &schema, 2).await;
        let mut holder_conn = holder_pool.acquire().await.unwrap();
        let mut holder = holder_conn.begin().await.unwrap();
        sqlx::query(
            "SELECT status FROM wf_definition
             WHERE wf_type='ci.pipeline' AND version=$1 FOR UPDATE",
        )
        .bind(V4V5_PREDECESSOR_VERSION)
        .fetch_one(&mut *holder)
        .await
        .unwrap();

        let factory = cutover_factory(&admin, &schema).await;
        let started = std::time::Instant::now();
        let refusal = factory
            .cutover_definition_with_plan(&v4_to_v5_plan(), &diagnostics(&admin))
            .await
            .expect_err("a held v5 fence must time out, not hang forever");
        let elapsed = started.elapsed();

        assert!(
            matches!(refusal, CiSupersededDefinitionGuardError::FenceUnavailable(_)),
            "expected FenceUnavailable, got {refusal:?}"
        );
        let bound =
            Duration::from_millis(myelin_ci_controlplane::CI_DEFINITION_FENCE_LOCK_TIMEOUT_MS);
        assert!(elapsed >= bound.mul_f32(0.5), "the cutover must actually wait, waited {elapsed:?}");
        assert!(elapsed < bound * 3, "the cutover must stop at its bound, waited {elapsed:?}");
        assert_eq!(
            definition_row(&admin, V4V5_PREDECESSOR_VERSION).await.map(|(_, s)| s),
            Some("active".into()),
            "a timed-out cutover leaves v5 active"
        );
        assert_eq!(definition_row(&admin, V4V5_CURRENT_VERSION).await, None);

        holder.rollback().await.unwrap();
        drop(holder_conn);
        factory
            .cutover_definition_with_plan(&v4_to_v5_plan(), &diagnostics(&admin))
            .await
            .expect("the v4→v5 cutover succeeds once the fence is free");
        holder_pool.close().await;
    })
    .await;
    })
    .await;
}

// ─── v4→v5 · 8. commit ambiguity fails closed ────────────────────────────────────────────────────

/// **The fail-closed-on-ambiguous-commit path.** A DEFERRED constraint trigger on `wf_definition`
/// lets every in-transaction statement (drain v5, insert v5, all verification reads) succeed but
/// makes the final `COMMIT` raise — exactly the ambiguous-commit window the fence guards. The
/// generalized cutover must surface `ProbeFailed` ("state is ambiguous; re-run to observe it") and
/// the aborted transaction must leave the registry in its complete OLD state (v5 active, v5 absent),
/// never half-applied. With the trigger gone the retry commits cleanly.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v4v5_an_ambiguous_commit_fails_closed_and_leaves_the_registry_whole() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("v4v5_commit_ambiguity").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        seed_v4_predecessor(&admin).await;

        // A deferred constraint trigger fires only at COMMIT, so the fence's own drain/insert/verify
        // statements all succeed and only the commit raises.
        admin
            .execute(
                format!(
                    "CREATE OR REPLACE FUNCTION {schema}.fail_wf_definition_commit()
                       RETURNS trigger LANGUAGE plpgsql AS
                       $$ BEGIN RAISE EXCEPTION 'injected commit-time failure'; END $$;
                     CREATE CONSTRAINT TRIGGER myelin_v4v5_commit_ambiguity
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
            .cutover_definition_with_plan(&v4_to_v5_plan(), &diagnostics(&admin))
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

        // The aborted transaction left the registry WHOLE: v5 still active, v5 never inserted.
        assert_eq!(
            definition_row(&admin, V4V5_PREDECESSOR_VERSION).await.map(|(_, s)| s),
            Some("active".into()),
            "a failed commit rolls the entire transition back — v5 stays active"
        );
        assert_eq!(
            definition_row(&admin, V4V5_CURRENT_VERSION).await,
            None,
            "the cutover is atomic: no half-applied v5 row survives a failed commit"
        );

        // Remove the trigger; the retry commits cleanly — proving it was the COMMIT that failed, not
        // the transition itself.
        admin
            .execute(
                format!(
                    "DROP TRIGGER myelin_v4v5_commit_ambiguity ON {schema}.wf_definition;
                     DROP FUNCTION {schema}.fail_wf_definition_commit();"
                )
                .as_str(),
            )
            .await
            .expect("remove the commit-failure trigger");
        factory
            .cutover_definition_with_plan(&v4_to_v5_plan(), &diagnostics(&admin))
            .await
            .expect("with the commit trigger gone the retry commits the v4→v5 cutover");
        assert_eq!(
            definition_row(&admin, V4V5_PREDECESSOR_VERSION).await.map(|(_, s)| s),
            Some("draining".into()),
            "the successful retry drains v5"
        );
        let (hash, status) = definition_row(&admin, V4V5_CURRENT_VERSION)
            .await
            .expect("v5 is now active");
        assert_eq!(status, "active");
        assert_eq!(hash, V4V5_CURRENT_HASH);
    })
    .await;
    })
    .await;
}

// ═════════════════════════════════════════════════════════════════════════════════════════════════
//  v4→v5 · THE ACTIVATION-READINESS PREDICATE (CT-007 slice 5b.3-6e.1 — DORMANT).
//
//  The queue-safety half of the v4→v5 fence. A `CutoverPlan` may carry an optional readiness
//  predicate that runs AFTER the FOR-UPDATE fence and the workflow-backlog probe, BEFORE drain. It
//  counts, database-wide, the non-terminal `job_queue` rows that still lack a claim window or carry a
//  reservation marker other than 2. A probe failure, a NULL result, or ANY unsafe row rolls the fence
//  back with v5 still active. The production v2→v5 plan carries NO predicate, so those tests above are
//  the byte/behavior-identical control: nothing here changes them.
//
//  These reuse the same `cutover_schema` fixture. Like the backlog probe, the readiness probe is
//  installed schema-local (pointed at THIS schema's `job_queue`) and selected through the
//  `ActivationReadinessProbe::with_call_for_tests` seam, so production resolution stays
//  `myelin_ci_security.`-qualified.
// ═════════════════════════════════════════════════════════════════════════════════════════════════

/// Install a SCHEMA-LOCAL copy of the `ci_0022c` readiness probe, pointed at this schema's
/// `job_queue`, built from the SAME hardened shape (fence-owned, `SECURITY DEFINER`, `row_security =
/// off`). The PRODUCTION shape over `public.job_queue`/`myelin_ci_security` is asserted separately by
/// the fresh-volume drill and the migration DDL-shape unit test.
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

/// The schema-local readiness call the fixtures point the plan at.
fn schema_local_readiness_call(schema: &str) -> String {
    format!("SELECT {schema}.myelin_ci_v2_activation_readiness_unsafe_count()")
}

/// A v4→v5 plan carrying the schema-local readiness predicate.
fn v4_to_v5_plan_with_readiness(schema: &str) -> CutoverPlan {
    v4_to_v5_plan()
        .with_activation_readiness(ActivationReadinessProbe::with_call_for_tests(
            schema_local_readiness_call(schema),
        ))
}

/// Seed one `job_queue` row with the given (claim_window_secs, reservation_write_version, state). The
/// `= 2` marker CHECK is enforced on inserts, so an "unsafe marker" row is expressed as `NULL`
/// (`IS DISTINCT FROM 2`), never a forbidden literal like 1.
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
    .bind(job_id) // idem_token unique per row
    .bind(state)
    .bind(claim_window_secs)
    .bind(reservation_write_version)
    .execute(admin)
    .await
    .unwrap();
}

// ─── v4→v5 · readiness · a NULL-claim-window non-terminal row refuses ─────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v4v5_readiness_refuses_on_a_nonterminal_null_claim_window_row() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("v4v5_ready_nullwin").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        seed_v4_predecessor(&admin).await;
        install_schema_local_readiness_probe(&admin, &schema).await;
        // Unsafe: a non-terminal row with NO claim window (marker is a valid 2, isolating the cause).
        seed_job_queue_row(&admin, "11111111-1111-1111-1111-111111111111", None, Some(2), "queued")
            .await;

        let factory = cutover_factory(&pinned_pool(&admin_url(), &schema, 2).await, &schema).await;
        let error = factory
            .cutover_definition_with_plan(&v4_to_v5_plan_with_readiness(&schema), &diagnostics(&admin))
            .await
            .expect_err("a NULL-claim-window non-terminal row must refuse the activation");
        assert!(
            matches!(error, CiSupersededDefinitionGuardError::ActivationNotReady { unsafe_rows } if unsafe_rows == 1),
            "the readiness predicate refuses with the unsafe-row count, got {error:?}"
        );
        // v5 stays active; v5 was never registered.
        assert_eq!(
            definition_row(&admin, V4V5_PREDECESSOR_VERSION).await.map(|(_, s)| s),
            Some("active".into()),
            "the fence rolled back: v5 remains active"
        );
        assert_eq!(definition_row(&admin, V4V5_CURRENT_VERSION).await, None, "v5 was never activated");
    })
    .await;
    })
    .await;
}

// ─── v4→v5 · readiness · a non-terminal row with a marker other than 2 refuses ────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v4v5_readiness_refuses_on_a_nonterminal_non_two_reservation_marker() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("v4v5_ready_marker").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        seed_v4_predecessor(&admin).await;
        install_schema_local_readiness_probe(&admin, &schema).await;
        // Unsafe: a valid claim window but NO reservation marker (NULL `IS DISTINCT FROM 2`).
        seed_job_queue_row(&admin, "22222222-2222-2222-2222-222222222222", Some(600), None, "running")
            .await;

        let factory = cutover_factory(&pinned_pool(&admin_url(), &schema, 2).await, &schema).await;
        let error = factory
            .cutover_definition_with_plan(&v4_to_v5_plan_with_readiness(&schema), &diagnostics(&admin))
            .await
            .expect_err("a non-2 reservation marker must refuse the activation");
        assert!(
            matches!(error, CiSupersededDefinitionGuardError::ActivationNotReady { unsafe_rows } if unsafe_rows == 1),
            "the readiness predicate refuses with the unsafe-row count, got {error:?}"
        );
        assert_eq!(
            definition_row(&admin, V4V5_PREDECESSOR_VERSION).await.map(|(_, s)| s),
            Some("active".into()),
            "the fence rolled back: v5 remains active"
        );
        assert_eq!(definition_row(&admin, V4V5_CURRENT_VERSION).await, None, "v5 was never activated");
    })
    .await;
    })
    .await;
}

// ─── v4→v5 · readiness · a NULL probe result fails closed ─────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v4v5_readiness_fails_closed_on_a_null_probe_result() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("v4v5_ready_null").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        seed_v4_predecessor(&admin).await;
        // A degenerate probe that returns NULL — a shadowing `SELECT NULL` must never read as "safe".
        let plan = v4_to_v5_plan().with_activation_readiness(
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
            definition_row(&admin, V4V5_PREDECESSOR_VERSION).await.map(|(_, s)| s),
            Some("active".into()),
            "the fence rolled back: v5 remains active"
        );
        assert_eq!(definition_row(&admin, V4V5_CURRENT_VERSION).await, None, "v5 was never activated");
    })
    .await;
    })
    .await;
}

// ─── v4→v5 · readiness · a probe FAILURE fails closed ─────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v4v5_readiness_fails_closed_on_a_probe_error() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("v4v5_ready_err").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        seed_v4_predecessor(&admin).await;
        // A probe naming a function that does not exist — a broken probe is never a passed probe.
        let plan = v4_to_v5_plan().with_activation_readiness(
            ActivationReadinessProbe::with_call_for_tests(
                "SELECT myelin_ci_security.readiness_probe_that_does_not_exist()",
            ),
        );
        let factory = cutover_factory(&pinned_pool(&admin_url(), &schema, 2).await, &schema).await;
        let error = factory
            .cutover_definition_with_plan(&plan, &diagnostics(&admin))
            .await
            .expect_err("a readiness probe error must fail closed");
        assert!(
            matches!(error, CiSupersededDefinitionGuardError::ProbeFailed(_)),
            "a probe error is fail-closed, got {error:?}"
        );
        assert_eq!(
            definition_row(&admin, V4V5_PREDECESSOR_VERSION).await.map(|(_, s)| s),
            Some("active".into()),
            "the fence rolled back: v5 remains active"
        );
    })
    .await;
    })
    .await;
}

// ─── v4→v5 · readiness · a CLEAN queue activates ──────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v4v5_readiness_activates_over_a_safe_queue() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("v4v5_ready_clean").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        seed_v4_predecessor(&admin).await;
        install_schema_local_readiness_probe(&admin, &schema).await;
        // Only SAFE rows: a non-terminal V2 row (window set, marker 2) and an already-terminal row
        // (excluded regardless of its columns). The predicate must NOT block this activation.
        seed_job_queue_row(&admin, "33333333-3333-3333-3333-333333333333", Some(600), Some(2), "running")
            .await;
        seed_job_queue_row(&admin, "44444444-4444-4444-4444-444444444444", None, None, "terminal").await;

        let factory = cutover_factory(&pinned_pool(&admin_url(), &schema, 2).await, &schema).await;
        factory
            .cutover_definition_with_plan(&v4_to_v5_plan_with_readiness(&schema), &diagnostics(&admin))
            .await
            .expect("a safe queue must let the v4→v5 activation proceed");
        assert_eq!(
            definition_row(&admin, V4V5_PREDECESSOR_VERSION).await.map(|(_, s)| s),
            Some("draining".into()),
            "the clean activation drains v5"
        );
        let (hash, status) = definition_row(&admin, V4V5_CURRENT_VERSION).await.expect("v5 active");
        assert_eq!(status, "active");
        assert_eq!(hash, V4V5_CURRENT_HASH);
    })
    .await;
    })
    .await;
}

// ─── v4→v5 · readiness · a `None` plan IGNORES unsafe rows (production control) ───────────────────

/// The byte/behavior-identical control: a plan with NO readiness predicate never runs the probe, so
/// even an unsafe queue does not block it. This is exactly the production v2→v5 posture — the frozen
/// fence — expressed on the synthetic pair so the two live side by side.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v4v5_a_none_readiness_plan_ignores_unsafe_rows() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("v4v5_ready_none").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        seed_v4_predecessor(&admin).await;
        // An unsafe row that WOULD refuse a readiness-bearing plan.
        seed_job_queue_row(&admin, "55555555-5555-5555-5555-555555555555", None, None, "queued").await;

        let plan = v4_to_v5_plan();
        assert!(!plan.has_activation_readiness(), "the control plan carries no predicate");
        let factory = cutover_factory(&pinned_pool(&admin_url(), &schema, 2).await, &schema).await;
        factory
            .cutover_definition_with_plan(&plan, &diagnostics(&admin))
            .await
            .expect("a None-readiness plan proceeds regardless of unsafe queue rows");
        assert_eq!(
            definition_row(&admin, V4V5_PREDECESSOR_VERSION).await.map(|(_, s)| s),
            Some("draining".into()),
            "the None-readiness cutover drains v5 despite the unsafe row"
        );
        assert_eq!(
            definition_row(&admin, V4V5_CURRENT_VERSION).await.map(|(_, s)| s),
            Some("active".into()),
            "v5 activates: the readiness probe was never consulted"
        );
    })
    .await;
    })
    .await;
}

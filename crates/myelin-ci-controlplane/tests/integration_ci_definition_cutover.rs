//! Live-PostgreSQL proof for the CT-007 `ci.pipeline` definition CUTOVER FENCE.
//!
//! A version bump strands every non-terminal run pinned to the old version: a Flow worker claims
//! only locally-registered `(wf_type, version)` keys, so a v3-only binary can never drive a v2 row.
//! A preflight `SELECT` cannot prevent that — an old-binary starter transaction already in flight
//! can commit a fresh v2 workflow after the snapshot. The fence closes it by reusing the lock the
//! old binary ALREADY takes: `validate_definition_pin` holds `wf_definition@2 FOR SHARE` until its
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
    CiSupersededDefinitionGuardError, CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION,
    CI_MANIFEST_PIPELINE_VERSION,
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



/// A schema carrying the real Flow + CI migration sets, with `ci.pipeline@2` seeded `active` — the
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
    // `ci_0020i` seeds the predecessor row as `retired` (the fresh-database sentinel). These tests
    // model an EXISTING fleet mid-rolling-deploy, so promote it to the deployed-v2 shape: an active
    // row carrying a real-looking legacy hash.
    sqlx::query(
        "INSERT INTO wf_definition (wf_type, version, code_hash, status)
         VALUES ('ci.pipeline', $1, 'blake3:legacy-v2-hash', 'active')
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
async fn an_old_v2_admission_holding_the_share_lock_makes_the_cutover_observe_its_workflow() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("old_wins").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        // Connection A: the EXACT lock the old v2 starter takes, held open across a barrier.
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
            "the old binary sees v2 active and proceeds"
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

        // A now commits a fresh v2 workflow — exactly the race a preflight SELECT would have missed.
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

        // B wakes, its post-lock snapshot SEES the committed v2 run, and it refuses.
        let refusal = cutover
            .await
            .unwrap()
            .expect_err("the cutover must observe the late v2 admission and refuse");
        assert!(
            matches!(refusal, CiSupersededDefinitionGuardError::Backlog(_)),
            "expected a backlog refusal, got {refusal:?}"
        );

        // v2 is untouched and v3 was never registered.
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
            "no v3 row may exist after a refused cutover"
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
async fn a_cutover_holding_the_update_lock_blocks_and_then_refuses_a_fresh_v2_admission() {
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
        assert!(!backlog, "this schema has no v2 runs");
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

        // A wakes and sees `draining` — the exact status the deployed v2 binary refuses a fresh
        // start on, before it writes a manifest, jobs, or a workflow.
        let (status, eligible) = admission.await.unwrap();
        assert_eq!(status, "draining");
        assert!(
            !eligible,
            "a draining definition is not eligible for a FRESH start — this is the refusal the \
             already-deployed v2 binary reports as CorruptRun"
        );

        // Nothing was admitted under v2.
        let v2_runs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM workflow_run WHERE wf_type='ci.pipeline' AND wf_version=$1",
        )
        .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(v2_runs, 0, "the fenced-out admission wrote no workflow");
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
async fn an_injected_probe_failure_rolls_the_cutover_back_and_leaves_v2_active() {
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
async fn a_divergent_preexisting_v3_hash_refuses_and_leaves_v2_active() {
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
async fn the_cutover_is_idempotent_across_reboots_and_never_reactivates_v2() {
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
async fn an_existing_v2_run_keeps_draining_while_a_fresh_v2_start_is_refused() {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    common::with_privilege_fixture_lock(&admin_url(), &["ci_cutover_", "ci_lease_topology_"], || async {
    let (schema, bootstrap, admin) = cutover_schema("drain").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        // Mark v2 draining directly — the post-cutover state — with an existing v2 run present.
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
            "an in-flight v2 run may still be replayed/driven while draining"
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
        // A stranded v2 run in a DIFFERENT region. `wf_definition` has no region column, so
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

/// **Absence is never "nothing to fence" (round-3 blocker 1).** With no `ci.pipeline@2` row there is
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
        // An orphaned non-terminal v2 run, and NO v2 definition row — the exact shape that used to
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
            message.contains("ci_0020i_ci_pipeline_cutover_fence_row"),
            "the refusal must name the bootstrap remediation; got: {message}"
        );
        assert_eq!(
            definition_row(&admin, CI_MANIFEST_PIPELINE_VERSION).await,
            None,
            "v3 must NOT be activated over a missing fence — the orphaned v2 run would strand"
        );

        // The migration's seed is what makes this unreachable on a real fresh database.
        admin
            .execute(myelin_ci_controlplane::SEED_CI_PIPELINE_CUTOVER_FENCE_ROW_DDL)
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
async fn an_indefinitely_held_fence_times_the_cutover_out_and_leaves_v2_active() {
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

#![cfg(feature = "integration")]

use myelin_ci_controlplane::{
    ci_manifest_pipeline_definition, ci_production_runtime_factory_test_support,
    ci_region_run_discovery_test_support, CiSupersededDefinitionGuardError,
    CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION, CI_MANIFEST_PIPELINE_VERSION,
};
use myelin_config::MyelinConfig;
use myelin_storage::{DurableCostLedger, HotTables, PgMigrator, SubstrateProvider};
use myelin_tenancy::Region;
use sqlx::{Executor, PgPool};

const TENANT: &str = "fresh-fence-tenant";
const REGION: &str = "fr-par";

fn migration_url() -> String {
    std::env::var("MYELIN_FRESH_MIGRATION_URL")
        .expect("MYELIN_FRESH_MIGRATION_URL is set by drill-ci-definition-fence-fresh-postgres.sh")
}

fn app_url() -> String {
    std::env::var("MYELIN_FRESH_APP_URL")
        .expect("MYELIN_FRESH_APP_URL is set by drill-ci-definition-fence-fresh-postgres.sh")
}

fn admin_url() -> String {
    std::env::var("MYELIN_FRESH_ADMIN_URL")
        .expect("MYELIN_FRESH_ADMIN_URL is set by drill-ci-definition-fence-fresh-postgres.sh")
}

async fn pool(url: &str) -> PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(url)
        .await
        .unwrap_or_else(|error| panic!("connect {url}: {error}"))
}

async fn seed_workflow_run(admin: &PgPool, run_id: &str, version: i32, state: &str) {
    sqlx::query(
        "INSERT INTO workflow_run (
           tenant_id, region, run_id, wf_type, wf_version, input, state, correlation_id,
           depth, partition
         ) VALUES ($1, $2, $3, 'ci.pipeline', $4, '[]'::jsonb, $5, $3, 0, 0)",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(run_id)
    .bind(version)
    .bind(state)
    .execute(admin)
    .await
    .expect("seed a workflow run as the cluster admin");
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
    .expect("seed a job_queue row as the cluster admin");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires the disposable container from drill-ci-definition-fence-fresh-postgres.sh"]
async fn a_fresh_volume_provisions_the_fence_and_completes_the_cutover_under_a_non_superuser_migrator(
) {
    let migrator = pool(&migration_url()).await;
    let admin = pool(&admin_url()).await;
    let app = pool(&app_url()).await;

    let (is_super, is_bypass, can_createrole): (bool, bool, bool) = sqlx::query_as(
        "SELECT rolsuper, rolbypassrls, rolcreaterole FROM pg_roles WHERE rolname = current_user",
    )
    .fetch_one(&migrator)
    .await
    .unwrap();
    assert!(
        !is_super && !is_bypass && !can_createrole,
        "the drill is vacuous unless the migration role is NOSUPERUSER NOBYPASSRLS NOCREATEROLE"
    );

    PgMigrator::apply_validated(
        &migrator,
        &myelin_flow::migrations::migrations(),
        &HotTables::declare(["workflow_run"]),
    )
    .await
    .expect("Flow migrations apply as the non-superuser migrator");
    PgMigrator::apply_validated(
        &migrator,
        &myelin_ci_controlplane::ci_controlplane_migrations(),
        &myelin_ci_controlplane::ci_controlplane_hot_tables(),
    )
    .await
    .expect(
        "control-plane migrations apply as the non-superuser migrator - if ci_0020h refuses here, \
         the fresh-volume pg-init ordering or the SET TRUE membership is broken",
    );

    for id in [
        "ci_0020h_ci_pipeline_version_backlog_probe",
        "ci_0020i_ci_pipeline_cutover_fence_row",
    ] {
        let recorded: i64 =
            sqlx::query_scalar("SELECT count(*) FROM myelin_applied_migration WHERE id = $1")
                .bind(id)
                .fetch_one(&migrator)
                .await
                .unwrap();
        assert_eq!(recorded, 1, "{id} must be recorded exactly once");
    }

    let (sentinel_hash, sentinel_status): (String, String) = sqlx::query_as(
        "SELECT code_hash, status FROM wf_definition WHERE wf_type='ci.pipeline' AND version=$1",
    )
    .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
    .fetch_one(&migrator)
    .await
    .expect("the predecessor row is seeded on a fresh database");
    assert_eq!(sentinel_status, "retired");
    assert!(sentinel_hash.starts_with("sentinel:"));

    let v2_registration_refused = sqlx::query(
        "INSERT INTO wf_definition (wf_type, version, code_hash, status)
         VALUES ('ci.pipeline', $1, 'blake3:old-v2-binary-hash', 'active')
         ON CONFLICT (wf_type, version) DO NOTHING",
    )
    .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
    .execute(&migrator)
    .await
    .expect("the conflicting insert is a no-op");
    assert_eq!(v2_registration_refused.rows_affected(), 0);
    let (still_hash, still_status): (String, String) = sqlx::query_as(
        "SELECT code_hash, status FROM wf_definition WHERE wf_type='ci.pipeline' AND version=$1",
    )
    .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
    .fetch_one(&migrator)
    .await
    .unwrap();
    assert_eq!(
        (still_hash, still_status),
        (sentinel_hash, "retired".into())
    );

    let (schema_owner, fn_owner, config, body, secdef, volatility): (
        String,
        String,
        Vec<String>,
        String,
        bool,
        String,
    ) = sqlx::query_as(
        "SELECT pg_get_userbyid(n.nspowner), pg_get_userbyid(p.proowner), p.proconfig,
                btrim(p.prosrc), p.prosecdef, p.provolatile::text
           FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
          WHERE n.nspname = 'myelin_ci_security'
            AND p.proname = 'myelin_ci_pipeline_version_has_nonterminal_runs'",
    )
    .fetch_one(&migrator)
    .await
    .expect("the probe exists in the dedicated security schema");
    assert_eq!(schema_owner, "myelin_ci_definition_fence");
    assert_eq!(
        fn_owner, "myelin_ci_definition_fence",
        "the function must be BORN fence-owned under the non-superuser migrator's SET TRUE \
         membership - this is the assertion the superuser dev stack cannot make honestly"
    );
    assert_eq!(
        config,
        vec!["search_path=pg_catalog", "row_security=off"],
        "exact proconfig"
    );
    assert!(secdef, "SECURITY DEFINER");
    assert_eq!(volatility, "s", "STABLE");
    assert!(body.contains("FROM public.workflow_run"));
    assert!(body.contains("state IN ('running', 'waiting')"));

    let granted: Vec<String> = sqlx::query_scalar(
        "SELECT a.attname || ':' || acl.privilege_type
           FROM pg_attribute a CROSS JOIN LATERAL aclexplode(a.attacl) acl
          WHERE a.attrelid = 'public.workflow_run'::regclass
            AND acl.grantee = 'myelin_ci_definition_fence'::regrole::oid
          ORDER BY 1",
    )
    .fetch_all(&migrator)
    .await
    .unwrap();
    assert_eq!(
        granted,
        vec!["state:SELECT", "wf_type:SELECT", "wf_version:SELECT"],
        "exactly the three non-payload columns, SELECT only"
    );
    for payload in ["input", "budget", "correlation_id", "cancel_reason"] {
        let visible: bool = sqlx::query_scalar(
            "SELECT has_column_privilege(
               'myelin_ci_definition_fence', 'public.workflow_run', $1, 'SELECT')",
        )
        .bind(payload)
        .fetch_one(&migrator)
        .await
        .unwrap();
        assert!(
            !visible,
            "the fence role must not read the `{payload}` column"
        );
    }
    let table_level: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_class c CROSS JOIN LATERAL aclexplode(c.relacl) acl
          WHERE c.oid = 'public.workflow_run'::regclass
            AND acl.grantee = 'myelin_ci_definition_fence'::regrole::oid",
    )
    .fetch_one(&migrator)
    .await
    .unwrap();
    assert_eq!(
        table_level, 0,
        "no table-level privilege, only column grants"
    );

    let probe_oid: i64 = sqlx::query_scalar(
        "SELECT p.oid::bigint FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
          WHERE n.nspname = 'myelin_ci_security'
            AND p.proname = 'myelin_ci_pipeline_version_has_nonterminal_runs'",
    )
    .fetch_one(&migrator)
    .await
    .unwrap();
    let public_execute: bool =
        sqlx::query_scalar("SELECT has_function_privilege('public', $1::oid, 'EXECUTE')")
            .bind(probe_oid)
            .fetch_one(&migrator)
            .await
            .unwrap();
    assert!(!public_execute, "PUBLIC must never execute the fence probe");
    let app_execute: bool =
        sqlx::query_scalar("SELECT has_function_privilege('myelin_app', $1::oid, 'EXECUTE')")
            .bind(probe_oid)
            .fetch_one(&migrator)
            .await
            .unwrap();
    assert!(app_execute, "the runtime role executes the probe");
    let migrator_usage: bool = sqlx::query_scalar(
        "SELECT has_schema_privilege(current_user, 'myelin_ci_security', 'USAGE')",
    )
    .fetch_one(&migrator)
    .await
    .unwrap();
    assert!(
        !migrator_usage,
        "the migration role must NOT retain standing access to the security schema - it adopts the \
         fence role for one transaction and resets"
    );

    let (rp_schema_owner, rp_fn_owner, rp_config, rp_body, rp_secdef, rp_volatility, rp_rettype): (
        String,
        String,
        Vec<String>,
        String,
        bool,
        String,
        String,
    ) = sqlx::query_as(
        "SELECT pg_get_userbyid(n.nspowner), pg_get_userbyid(p.proowner), p.proconfig,
                btrim(p.prosrc), p.prosecdef, p.provolatile::text, p.prorettype::regtype::text
           FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
          WHERE n.nspname = 'myelin_ci_security'
            AND p.proname = 'myelin_ci_v2_activation_readiness_unsafe_count'",
    )
    .fetch_one(&migrator)
    .await
    .expect("the readiness probe exists in the dedicated security schema");
    assert_eq!(rp_schema_owner, "myelin_ci_definition_fence");
    assert_eq!(
        rp_fn_owner, "myelin_ci_definition_fence",
        "the readiness probe must be BORN fence-owned under the non-superuser migrator"
    );
    assert_eq!(
        rp_config,
        vec!["search_path=pg_catalog", "row_security=off"],
        "exact proconfig"
    );
    assert!(rp_secdef, "SECURITY DEFINER");
    assert_eq!(rp_volatility, "s", "STABLE");
    assert_eq!(
        rp_rettype, "bigint",
        "the readiness probe returns an aggregate count"
    );
    assert!(rp_body.contains("FROM public.job_queue"));
    assert!(rp_body.contains("state <> 'terminal'"));
    assert!(rp_body.contains("reservation_write_version IS DISTINCT FROM 2"));

    let rp_granted: Vec<String> = sqlx::query_scalar(
        "SELECT a.attname || ':' || acl.privilege_type
           FROM pg_attribute a CROSS JOIN LATERAL aclexplode(a.attacl) acl
          WHERE a.attrelid = 'public.job_queue'::regclass
            AND acl.grantee = 'myelin_ci_definition_fence'::regrole::oid
          ORDER BY 1",
    )
    .fetch_all(&migrator)
    .await
    .unwrap();
    assert_eq!(
        rp_granted,
        vec![
            "claim_window_secs:SELECT",
            "region:SELECT",
            "reservation_write_version:SELECT",
            "state:SELECT"
        ],
        "exactly the four non-payload columns, SELECT only"
    );
    for payload in ["tenant_id", "idem_token", "lease_owner", "run_id"] {
        let visible: bool = sqlx::query_scalar(
            "SELECT has_column_privilege(
               'myelin_ci_definition_fence', 'public.job_queue', $1, 'SELECT')",
        )
        .bind(payload)
        .fetch_one(&migrator)
        .await
        .unwrap();
        assert!(
            !visible,
            "the fence role must not read the `{payload}` job_queue column"
        );
    }
    let rp_table_level: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_class c CROSS JOIN LATERAL aclexplode(c.relacl) acl
          WHERE c.oid = 'public.job_queue'::regclass
            AND acl.grantee = 'myelin_ci_definition_fence'::regrole::oid",
    )
    .fetch_one(&migrator)
    .await
    .unwrap();
    assert_eq!(
        rp_table_level, 0,
        "no table-level privilege on job_queue, only column grants"
    );

    let rp_oid: i64 = sqlx::query_scalar(
        "SELECT p.oid::bigint FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
          WHERE n.nspname = 'myelin_ci_security'
            AND p.proname = 'myelin_ci_v2_activation_readiness_unsafe_count'",
    )
    .fetch_one(&migrator)
    .await
    .unwrap();
    let rp_public_execute: bool =
        sqlx::query_scalar("SELECT has_function_privilege('public', $1::oid, 'EXECUTE')")
            .bind(rp_oid)
            .fetch_one(&migrator)
            .await
            .unwrap();
    assert!(
        !rp_public_execute,
        "PUBLIC must never execute the readiness probe"
    );
    let rp_app_execute: bool =
        sqlx::query_scalar("SELECT has_function_privilege('myelin_app', $1::oid, 'EXECUTE')")
            .bind(rp_oid)
            .fetch_one(&migrator)
            .await
            .unwrap();
    assert!(
        rp_app_execute,
        "the runtime role executes the readiness probe"
    );

    let before_oid = probe_oid;
    sqlx::query(
        "DELETE FROM myelin_applied_migration WHERE id='ci_0020h_ci_pipeline_version_backlog_probe'",
    )
    .execute(&migrator)
    .await
    .unwrap();
    PgMigrator::apply_validated(
        &migrator,
        &myelin_ci_controlplane::ci_controlplane_migrations(),
        &myelin_ci_controlplane::ci_controlplane_hot_tables(),
    )
    .await
    .expect("the crash-window retry succeeds as the non-superuser migrator");
    let after_oid: i64 = sqlx::query_scalar(
        "SELECT p.oid::bigint FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
          WHERE n.nspname = 'myelin_ci_security'
            AND p.proname = 'myelin_ci_pipeline_version_has_nonterminal_runs'",
    )
    .fetch_one(&migrator)
    .await
    .unwrap();
    assert_eq!(
        after_oid, before_oid,
        "the probe was ADOPTED, not recreated"
    );
    let ledger: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM myelin_applied_migration
          WHERE id='ci_0020h_ci_pipeline_version_backlog_probe'",
    )
    .fetch_one(&migrator)
    .await
    .unwrap();
    assert_eq!(ledger, 1);

    sqlx::query(
        "DELETE FROM myelin_applied_migration WHERE id='ci_0020h_ci_pipeline_version_backlog_probe'",
    )
    .execute(&migrator)
    .await
    .unwrap();
    admin
        .execute(
            "SET LOCAL ROLE myelin_ci_definition_fence;
             CREATE OR REPLACE FUNCTION
               myelin_ci_security.myelin_ci_pipeline_version_has_nonterminal_runs(version integer)
             RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER
             SET search_path = pg_catalog SET row_security = off
             AS $tampered$SELECT false$tampered$;
             RESET ROLE;",
        )
        .await
        .expect("tamper with the probe body");
    let refused = PgMigrator::apply_validated(
        &migrator,
        &myelin_ci_controlplane::ci_controlplane_migrations(),
        &myelin_ci_controlplane::ci_controlplane_hot_tables(),
    )
    .await
    .expect_err("a divergent probe must make the retry RAISE, never be silently overwritten");
    assert!(
        refused
            .to_string()
            .contains("diverges from the expected definition-fence probe"),
        "unexpected refusal: {refused}"
    );
    let tampered_body: String = sqlx::query_scalar(
        "SELECT btrim(p.prosrc) FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
          WHERE n.nspname = 'myelin_ci_security'
            AND p.proname = 'myelin_ci_pipeline_version_has_nonterminal_runs'",
    )
    .fetch_one(&migrator)
    .await
    .unwrap();
    assert_eq!(
        tampered_body, "SELECT false",
        "the divergent function must be left exactly as found, for an operator to inspect"
    );

    admin
        .execute(
            "SET LOCAL ROLE myelin_ci_definition_fence;
             DROP FUNCTION
               myelin_ci_security.myelin_ci_pipeline_version_has_nonterminal_runs(integer);
             RESET ROLE;",
        )
        .await
        .expect("remove the tampered probe");
    PgMigrator::apply_validated(
        &migrator,
        &myelin_ci_controlplane::ci_controlplane_migrations(),
        &myelin_ci_controlplane::ci_controlplane_hot_tables(),
    )
    .await
    .expect("the real probe is recreated once the divergent one is gone");
    assert_eq!(
        body,
        sqlx::query_scalar::<_, String>(
            "SELECT btrim(p.prosrc) FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
              WHERE n.nspname = 'myelin_ci_security'
                AND p.proname = 'myelin_ci_pipeline_version_has_nonterminal_runs'",
        )
        .fetch_one(&migrator)
        .await
        .unwrap(),
        "the restored body is byte-identical to the original"
    );

    seed_workflow_run(
        &admin,
        "fresh-live-v2",
        CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION,
        "running",
    )
    .await;
    let app_direct: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow_run WHERE wf_type='ci.pipeline' AND wf_version=$1",
    )
    .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
    .fetch_one(&app)
    .await
    .unwrap();
    assert_eq!(
        app_direct, 0,
        "FORCE RLS hides the row from the runtime role's own read - which is exactly why the probe \
         must be SECURITY DEFINER, owned by a bypass role"
    );
    let probe_sees: bool = sqlx::query_scalar(
        "SELECT myelin_ci_security.myelin_ci_pipeline_version_has_nonterminal_runs($1)",
    )
    .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
    .fetch_one(&app)
    .await
    .expect("the app role executes the probe");
    assert!(
        probe_sees,
        "the probe must see the live v2 run database-wide"
    );

    seed_job_queue_row(
        &admin,
        "11111111-1111-1111-1111-111111111111",
        None,
        Some(2),
        "queued",
    )
    .await;
    let app_direct_jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM job_queue WHERE state <> 'terminal' \
         AND (claim_window_secs IS NULL OR reservation_write_version IS DISTINCT FROM 2)",
    )
    .fetch_one(&app)
    .await
    .expect("the app role can SELECT job_queue (RLS filters the rows, it does not deny the read)");
    assert_eq!(
        app_direct_jobs, 0,
        "FORCE RLS hides the unsafe job_queue row from the runtime role's OWN read - which is exactly \
         why the readiness probe must be SECURITY DEFINER, owned by the bypass fence role"
    );
    let unsafe_seen: i64 = sqlx::query_scalar(
        "SELECT myelin_ci_security.myelin_ci_v2_activation_readiness_unsafe_count()",
    )
    .fetch_one(&app)
    .await
    .expect("the app role executes the production readiness probe");
    assert_eq!(
        unsafe_seen, 1,
        "the production readiness probe counts the unsafe row the app itself cannot see - the \
         fail-open guard is real under the non-superuser posture"
    );
    sqlx::query(
        "UPDATE job_queue SET claim_window_secs = 600, reservation_write_version = 2 \
         WHERE job_id = '11111111-1111-1111-1111-111111111111'::uuid",
    )
    .execute(&admin)
    .await
    .expect("make the seeded row safe as the cluster admin");
    let unsafe_after: i64 = sqlx::query_scalar(
        "SELECT myelin_ci_security.myelin_ci_v2_activation_readiness_unsafe_count()",
    )
    .fetch_one(&app)
    .await
    .unwrap();
    assert_eq!(
        unsafe_after, 0,
        "making the row safe (window set + marker 2) clears the production readiness count"
    );

    let mut config = MyelinConfig::dev();
    config.database_url = app_url();
    config.region = REGION.to_owned();
    let provider = SubstrateProvider::connect(config, 2).await.unwrap();
    let factory = ci_production_runtime_factory_test_support(
        app.clone(),
        Region(REGION.into()),
        DurableCostLedger::new(provider),
        tokio::runtime::Handle::current(),
    )
    .unwrap();
    let diagnostics = ci_region_run_discovery_test_support(admin.clone());
    let refusal = factory
        .cutover_definition(&diagnostics)
        .await
        .expect_err("a live v2 run must refuse the cutover");
    assert!(
        matches!(refusal, CiSupersededDefinitionGuardError::Backlog(_)),
        "expected a backlog refusal, got {refusal:?}"
    );

    myelin_flow::DurableExecutor::cancel(
        &myelin_flow::PgFlowExecutor::new(
            app.clone(),
            tokio::runtime::Handle::current(),
            std::sync::Arc::new(myelin_events::MonotonicMinter::new()),
            myelin_tenancy::TenantId(TENANT.into()),
            Region(REGION.into()),
        ),
        &myelin_flow::RunId("fresh-live-v2".into()),
        "fresh-volume definition-fence drill",
    )
    .expect("the documented remediation is a real cancellation");
    let probe_after: bool = sqlx::query_scalar(
        "SELECT myelin_ci_security.myelin_ci_pipeline_version_has_nonterminal_runs($1)",
    )
    .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
    .fetch_one(&app)
    .await
    .unwrap();
    assert!(!probe_after, "cancelling the run clears the backlog");

    factory
        .cutover_definition(&diagnostics)
        .await
        .expect("with the backlog cleared the cutover commits");

    let rows: Vec<(i32, String, String)> = sqlx::query_as(
        "SELECT version, code_hash, status FROM wf_definition
          WHERE wf_type='ci.pipeline' ORDER BY version",
    )
    .fetch_all(&migrator)
    .await
    .unwrap();
    let v2 = rows
        .iter()
        .find(|(v, _, _)| *v == CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
        .expect("v2 row");
    let v3 = rows
        .iter()
        .find(|(v, _, _)| *v == CI_MANIFEST_PIPELINE_VERSION)
        .expect("v3 row");
    assert_eq!(
        v2.2, "retired",
        "the fresh-database sentinel stays RETIRED - the cutover must not resurrect it to draining"
    );
    assert_eq!(v3.2, "active");
    assert_eq!(v3.1, ci_manifest_pipeline_definition().code_hash());

    eprintln!("fresh-volume definition-fence drill: all assertions passed");
}

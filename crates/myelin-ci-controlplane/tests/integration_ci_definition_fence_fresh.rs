//! **The FRESH-VOLUME definition-fence drill (CT-007 round-4b spec point 11).**
//!
//! Every other gate in this repo runs against the persistent dev stack, where `pg-init` ran long
//! ago and the migration role is a superuser. Two failure classes are therefore structurally
//! invisible to them:
//!
//! 1. **init ordering** — that `01-ci-definition-fence.sql` completes before any application table
//!    exists, so `ci_0020h` finds its provisioning already in place on a brand-new database;
//! 2. **the non-superuser migration posture** — that a `NOSUPERUSER NOBYPASSRLS NOCREATEROLE`
//!    migration role can still adopt the fence role through its explicit `SET TRUE` membership and
//!    create the probe as its final owner. On the dev stack `myelin_admin` is a superuser, so it
//!    would succeed even if the membership were missing entirely.
//!
//! This target is `#[ignore]`d: it requires the disposable container
//! `scripts/drill-ci-definition-fence-fresh-postgres.sh` builds, and is meaningless without it.
//! Run it through that script, which is wired as the `fresh-definition-fence` CI job.
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

/// The NON-SUPERUSER migration role the drill script provisions. Everything below runs through it.
fn migration_url() -> String {
    std::env::var("MYELIN_FRESH_MIGRATION_URL")
        .expect("MYELIN_FRESH_MIGRATION_URL is set by drill-ci-definition-fence-fresh-postgres.sh")
}

/// The ordinary runtime (`myelin_app`) URL — `NOSUPERUSER NOBYPASSRLS`, as in production.
fn app_url() -> String {
    std::env::var("MYELIN_FRESH_APP_URL")
        .expect("MYELIN_FRESH_APP_URL is set by drill-ci-definition-fence-fresh-postgres.sh")
}

/// The cluster-admin URL, used only to seed rows the app role cannot (FORCE RLS) and to assert.
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires the disposable container from drill-ci-definition-fence-fresh-postgres.sh"]
async fn a_fresh_volume_provisions_the_fence_and_completes_the_cutover_under_a_non_superuser_migrator()
{
    let migrator = pool(&migration_url()).await;
    let admin = pool(&admin_url()).await;
    let app = pool(&app_url()).await;

    // ── The migration role really is the constrained posture this drill exists to exercise ──────
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

    // ── (1) The complete migration sets, applied through the NON-SUPERUSER migration URL ────────
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
        "control-plane migrations apply as the non-superuser migrator — if ci_0020h refuses here, \
         the fresh-volume pg-init ordering or the SET TRUE membership is broken",
    );

    // ── (2) Exact ledger rows ───────────────────────────────────────────────────────────────────
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

    // ── (3) The retired v2 sentinel, and an old v2 binary's registration refused ────────────────
    let (sentinel_hash, sentinel_status): (String, String) = sqlx::query_as(
        "SELECT code_hash, status FROM wf_definition WHERE wf_type='ci.pipeline' AND version=$1",
    )
    .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
    .fetch_one(&migrator)
    .await
    .expect("the predecessor row is seeded on a fresh database");
    assert_eq!(sentinel_status, "retired");
    assert!(sentinel_hash.starts_with("sentinel:"));

    // What a still-deployed v2 binary would do: `register_definition` inserts-or-verifies, and a
    // non-active row makes it refuse rather than activate itself against a fresh database.
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
    assert_eq!((still_hash, still_status), (sentinel_hash, "retired".into()));

    // ── (4) Exact security schema + function ownership ──────────────────────────────────────────
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
         membership — this is the assertion the superuser dev stack cannot make honestly"
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

    // ── (5) Exactly three fence columns, zero payload access, zero table-level privilege ────────
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
        assert!(!visible, "the fence role must not read the `{payload}` column");
    }
    let table_level: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_class c CROSS JOIN LATERAL aclexplode(c.relacl) acl
          WHERE c.oid = 'public.workflow_run'::regclass
            AND acl.grantee = 'myelin_ci_definition_fence'::regrole::oid",
    )
    .fetch_one(&migrator)
    .await
    .unwrap();
    assert_eq!(table_level, 0, "no table-level privilege, only column grants");

    // ── (6) PUBLIC cannot execute; the app role can ─────────────────────────────────────────────
    // Resolved by CATALOGUE JOIN, not `::regprocedure`: the migration role deliberately has no
    // USAGE on `myelin_ci_security` (it only ever touches it while acting AS the fence role), and
    // signature parsing would need that privilege. That the migrator cannot resolve the name is
    // itself the least-privilege posture working.
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
    let migrator_usage: bool =
        sqlx::query_scalar("SELECT has_schema_privilege(current_user, 'myelin_ci_security', 'USAGE')")
            .fetch_one(&migrator)
            .await
            .unwrap();
    assert!(
        !migrator_usage,
        "the migration role must NOT retain standing access to the security schema — it adopts the \
         fence role for one transaction and resets"
    );

    // ── (7) The crash window: delete only the ledger row, reapply, adopt without rewriting ──────
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
    assert_eq!(after_oid, before_oid, "the probe was ADOPTED, not recreated");
    let ledger: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM myelin_applied_migration
          WHERE id='ci_0020h_ci_pipeline_version_backlog_probe'",
    )
    .fetch_one(&migrator)
    .await
    .unwrap();
    assert_eq!(ledger, 1);

    // ── (7b) THE DIVERGENT NEGATIVE CONTROL — safe only here, on a throwaway volume ─────────────
    // What separates "idempotent adoption" from "blind replace": a function occupying the probe's
    // exact signature but with a different body must make the retry RAISE and must be left exactly
    // as found for an operator to inspect. This tampers with a schema-global object, so it belongs
    // in a disposable container rather than the shared-stack suite, where no restoration discipline
    // could make it safe (a panic mid-restore would strand every later test with no probe).
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

    // Restore for the remaining assertions. On a disposable volume a failure here is contained.
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

    // ── (8) RLS reality: the app role cannot see a live v2 row, but the probe reports it ────────
    seed_workflow_run(&admin, "fresh-live-v2", CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION, "running")
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
        "FORCE RLS hides the row from the runtime role's own read — which is exactly why the probe \
         must be SECURITY DEFINER, owned by a bypass role"
    );
    let probe_sees: bool = sqlx::query_scalar(
        "SELECT myelin_ci_security.myelin_ci_pipeline_version_has_nonterminal_runs($1)",
    )
        .bind(CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
        .fetch_one(&app)
        .await
        .expect("the app role executes the probe");
    assert!(probe_sees, "the probe must see the live v2 run database-wide");

    // ── (9) The cutover refuses while that row is live ──────────────────────────────────────────
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

    // ── (10) Real cancellation clears it, and the cutover then activates v3 ─────────────────────
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
        "the fresh-database sentinel stays RETIRED — the cutover must not resurrect it to draining"
    );
    assert_eq!(v3.2, "active");
    assert_eq!(v3.1, ci_manifest_pipeline_definition().code_hash());

    eprintln!("fresh-volume definition-fence drill: all assertions passed");
}

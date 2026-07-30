//! Live proof of the dedicated, least-privilege CI region scheduler boundary.
#![cfg(feature = "integration")]

mod common;

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use myelin_ci_controlplane::CI_RUNNER_EXECUTION_LEASE_TTL_SECS;
use myelin_ci_controlplane::{
    ci_controlplane_hot_tables, ci_controlplane_migrations, ci_job_queue_store,
    CiSchedulerDbConfig, CiSchedulerDbError, CiSchedulerDbProvider, DurableEnqueue, EnqueueOutcome,
    JobQueueReaper, Lane,
};
use futures::FutureExt;
use myelin_ci_sandbox::TrustTier;
use myelin_storage::{connect_pool_with_reset, PgMigrator};
use sqlx::postgres::{PgPoolOptions, PgQueryResult};
use sqlx::{Executor, PgPool};

const FR_PAR: &str = "fr-par";
const DE_FRA: &str = "de-fra";
const APP_DEFAULT: &str = "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin";
const ADMIN_DEFAULT: &str = "postgres://myelin_admin:myelin_dev_pw@localhost:5433/myelin";
const SCHEDULER_DEFAULT: &str =
    "postgres://myelin_ci_scheduler_fr_par:myelin_ci_scheduler_dev_pw@localhost:5433/myelin";

fn configured_url(name: &str, fallback: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| fallback.to_owned())
}

fn distinct_raw_url(url: &str, tag: &str) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}application_name={tag}")
}

fn job(tenant: &str, region: &str, label: &str, ordinal: u16) -> DurableEnqueue {
    DurableEnqueue {
        tenant_id: tenant.to_owned(),
        region: region.to_owned(),
        job_id: format!("00000000-0000-0000-0000-{ordinal:012}"),
        run_id: format!("10000000-0000-0000-0000-{ordinal:012}"),
        lane: Lane::Interactive,
        labels: vec![label.to_owned()],
        trust_tier: TrustTier::Trusted,
        concurrency_group: None,
        fair_key: tenant.to_owned(),
        idem_token: format!("scheduler-boundary-{ordinal}"),
        stage: "build".into(),
        claim_window_secs: CI_RUNNER_EXECUTION_LEASE_TTL_SECS,
    }
}

fn assert_permission_denied(result: Result<PgQueryResult, sqlx::Error>, operation: &str) {
    let error = result.expect_err(operation);
    let code = error
        .as_database_error()
        .and_then(|database| database.code())
        .map(|code| code.into_owned());
    assert_eq!(code.as_deref(), Some("42501"), "{operation}: {error}");
}

async fn cleanup(admin: &PgPool, tenants: &[&str]) {
    sqlx::query("DELETE FROM workflow_run WHERE tenant_id = ANY($1)")
        .bind(tenants)
        .execute(admin)
        .await
        .expect("clean scheduler-boundary workflow fixtures");
    sqlx::query("DELETE FROM job_queue WHERE tenant_id = ANY($1)")
        .bind(tenants)
        .execute(admin)
        .await
        .expect("clean scheduler-boundary queue fixtures");
    sqlx::query("DELETE FROM fair_deficit WHERE tenant_id = ANY($1)")
        .bind(tenants)
        .execute(admin)
        .await
        .expect("clean scheduler-boundary fairness fixtures");
    sqlx::query("DELETE FROM ci_run WHERE tenant_id = ANY($1)")
        .bind(tenants)
        .execute(admin)
        .await
        .expect("clean scheduler-boundary run fixtures");
}

async fn cleanup_stale_fixtures(admin: &PgPool) {
    for table in ["workflow_run", "job_queue", "fair_deficit", "ci_run"] {
        sqlx::query(&format!(
            "DELETE FROM {table}
              WHERE tenant_id LIKE 'scheduler-a-%'
                 OR tenant_id LIKE 'scheduler-b-%'
                 OR tenant_id LIKE 'scheduler-de-%'"
        ))
        .execute(admin)
        .await
        .unwrap_or_else(|error| panic!("clean stale scheduler fixtures from {table}: {error}"));
    }
}

async fn insert_queued_run(
    admin: &PgPool,
    tenant: &str,
    region: &str,
    ordinal: u16,
    created_at: &str,
) {
    let run_id = format!("20000000-0000-0000-0000-{ordinal:012}");
    let wf_run_id = format!("30000000-0000-0000-0000-{ordinal:012}");
    sqlx::query(
        "INSERT INTO ci_run (
           tenant_id, region, run_id, project_id, pipeline_id, wf_run_id,
           definition_snapshot, trigger_kind, trust_tier, state, correlation_id, created_at
         ) VALUES (
           $1, $2, $3::uuid, '22222222-2222-2222-2222-222222222222'::uuid,
           '33333333-3333-3333-3333-333333333333'::uuid, $4::uuid,
           'myelin://ci/scheduler-boundary', 'push', 'trusted', 'queued', $3, $5::timestamptz
         )",
    )
    .bind(tenant)
    .bind(region)
    .bind(&run_id)
    .bind(&wf_run_id)
    .bind(created_at)
    .execute(admin)
    .await
    .expect("insert queued run discovery fixture");
}

async fn insert_active_job_owner(admin: &PgPool, tenant: &str, region: &str, ordinal: u16) {
    let wf_run_id = format!("10000000-0000-0000-0000-{ordinal:012}");
    let ci_run_id = format!("40000000-0000-0000-0000-{ordinal:012}");
    sqlx::query(
        "INSERT INTO ci_run (
           tenant_id, region, run_id, project_id, pipeline_id, wf_run_id,
           definition_snapshot, trigger_kind, trust_tier, state, correlation_id
         ) VALUES (
           $1, $2, $3::uuid, '22222222-2222-2222-2222-222222222222'::uuid,
           '33333333-3333-3333-3333-333333333333'::uuid, $4::uuid,
           'myelin://ci/scheduler-active-owner', 'push', 'trusted', 'running', $3
         )",
    )
    .bind(tenant)
    .bind(region)
    .bind(&ci_run_id)
    .bind(&wf_run_id)
    .execute(admin)
    .await
    .expect("insert active CI owner for scheduler fixture");
    sqlx::query(
        "INSERT INTO workflow_run (
           tenant_id, region, run_id, wf_type, wf_version, input, state, correlation_id,
           depth, partition
         ) VALUES (
           $1, $2, $3, 'ci.pipeline', 1, '[]'::jsonb, 'running', $3, 0, 0
         )",
    )
    .bind(tenant)
    .bind(region)
    .bind(&wf_run_id)
    .execute(admin)
    .await
    .expect("insert active Flow owner for scheduler fixture");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dedicated_scheduler_role_is_region_bound_least_privilege_and_reset_safe() {
    // This test boots the REAL `CiSchedulerDbProvider`, whose excess-privilege probe scans every
    // non-system schema — so it must not run while another suite's per-test schema (carrying CI
    // scheduler grants) exists. Same advisory lock, same sweep.
    let lock_admin_url = configured_url("DATABASE_MIGRATION_URL", ADMIN_DEFAULT);
    common::with_privilege_fixture_lock(
        &lock_admin_url,
        &["ci_cutover_", "ci_lease_topology_"],
        || async {
    let app_url = configured_url("DATABASE_URL", APP_DEFAULT);
    let admin_url = configured_url("DATABASE_MIGRATION_URL", ADMIN_DEFAULT);
    let scheduler_url = configured_url("MYELIN_CI_SCHEDULER_DATABASE_URL", SCHEDULER_DEFAULT);
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url)
        .await
        .expect("connect migration role");

    PgMigrator::apply_validated(
        &admin,
        &ci_controlplane_migrations(),
        &ci_controlplane_hot_tables(),
    )
    .await
    .expect("apply the real CI control-plane migrations in public");

    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let tenant_a = format!("scheduler-a-{}-{suffix}", std::process::id());
    let tenant_b = format!("scheduler-b-{}-{suffix}", std::process::id());
    let tenant_other_region = format!("scheduler-de-{}-{suffix}", std::process::id());
    let proof_label = format!("scheduler-boundary-{suffix}");
    let tenants = [
        tenant_a.as_str(),
        tenant_b.as_str(),
        tenant_other_region.as_str(),
    ];
    cleanup_stale_fixtures(&admin).await;
    cleanup(&admin, &tenants).await;

    // BUG FIX (investigation, 2026-07-25): this test's ONLY cleanup used to be the `cleanup(&admin,
    // &tenants).await` call at the very end of the happy path — so a panicking assertion anywhere in
    // between (e.g. `CiSchedulerDbProvider::connect(...)` failing with `ExcessPrivileges`, which this
    // test itself is designed to be ABLE to hit) left every fixture row behind in the REAL shared
    // `public.job_queue`/`ci_run` tables forever. That is exactly the contamination chain found today:
    // this test's own stray `scheduler-*` tenants were the rows a separate, unrelated smoke test
    // (`production_pg_bootstrap_source.rs`'s "zero pre-existing active work" check) tripped over.
    // Wrapping the rest of the body in catch_unwind + unconditional cleanup + resume_unwind (mirrors
    // `tests/common::with_schema_cleanup`'s pattern, applied here to targeted-tenant DELETEs instead
    // of a schema DROP) makes cleanup run whether this test passes, fails an assertion, or panics.
    let result = std::panic::AssertUnwindSafe(async {
    let app = connect_pool_with_reset(&app_url, FR_PAR, 4)
        .await
        .expect("connect constrained app pool");
    let tenant_store = ci_job_queue_store(app.clone());
    for (fixture, ordinal) in [
        (job(&tenant_a, FR_PAR, &proof_label, 101), 101),
        (job(&tenant_b, FR_PAR, &proof_label, 102), 102),
        (job(&tenant_other_region, DE_FRA, &proof_label, 103), 103),
    ] {
        assert_eq!(
            tenant_store
                .enqueue(&fixture)
                .await
                .expect("tenant enqueue"),
            EnqueueOutcome::Inserted
        );
        insert_active_job_owner(&admin, &fixture.tenant_id, &fixture.region, ordinal).await;
    }
    insert_queued_run(&admin, &tenant_a, FR_PAR, 201, "2001-01-01T00:00:00Z").await;
    insert_queued_run(&admin, &tenant_b, FR_PAR, 202, "2000-01-01T00:00:00Z").await;
    insert_queued_run(
        &admin,
        &tenant_other_region,
        DE_FRA,
        203,
        "1999-01-01T00:00:00Z",
    )
    .await;
    insert_queued_run(&admin, &tenant_a, FR_PAR, 204, "1900-01-01T00:00:00Z").await;
    insert_queued_run(&admin, &tenant_b, FR_PAR, 206, "1900-01-01T00:00:01Z").await;
    insert_queued_run(
        &admin,
        &tenant_other_region,
        DE_FRA,
        205,
        "1899-01-01T00:00:00Z",
    )
    .await;
    sqlx::query(
        "UPDATE ci_run SET state = 'running'
          WHERE run_id IN (
            '20000000-0000-0000-0000-000000000204'::uuid,
            '20000000-0000-0000-0000-000000000205'::uuid,
            '20000000-0000-0000-0000-000000000206'::uuid
          )",
    )
    .execute(&admin)
    .await
    .expect("mark active workflow discovery fixtures running");
    for (tenant, region, run_id, partition, created_at) in [
        (
            tenant_a.as_str(),
            FR_PAR,
            "30000000-0000-0000-0000-000000000204",
            7_i16,
            "1900-01-01T00:00:00Z",
        ),
        (
            tenant_b.as_str(),
            FR_PAR,
            "30000000-0000-0000-0000-000000000206",
            8_i16,
            "1900-01-01T00:00:01Z",
        ),
        (
            tenant_other_region.as_str(),
            DE_FRA,
            "30000000-0000-0000-0000-000000000205",
            9_i16,
            "1899-01-01T00:00:00Z",
        ),
    ] {
        sqlx::query(
            "INSERT INTO workflow_run (
               tenant_id, region, run_id, wf_type, wf_version, input, state, correlation_id,
               depth, partition, created_at
             ) VALUES (
               $1, $2, $3, 'ci.pipeline', 1, '[]'::jsonb, 'running', $3, 0, $4,
               $5::timestamptz
             )",
        )
        .bind(tenant)
        .bind(region)
        .bind(run_id)
        .bind(partition)
        .bind(created_at)
        .execute(&admin)
        .await
        .expect("insert active workflow discovery fixture");
    }

    let scheduler_config = CiSchedulerDbConfig::from_parts(
        scheduler_url.clone(),
        &app_url,
        &admin_url,
        FR_PAR.to_owned(),
    )
    .expect("three distinct database credentials");
    let scheduler = CiSchedulerDbProvider::connect(scheduler_config, &app)
        .await
        .expect("validate the dedicated scheduler role");
    let region_store = scheduler.region_queue_store();
    let run_discovery = scheduler.region_run_discovery();
    let labels = vec![proof_label.clone()];

    assert_eq!(
        run_discovery
            .next_queued_tenant(FR_PAR)
            .await
            .expect("discover oldest same-region queued run"),
        Some(myelin_tenancy::TenantId(tenant_b.clone())),
        "discovery returns only the authoritative tenant owning the oldest visible queued run"
    );
    assert!(
        run_discovery
            .next_queued_tenant(DE_FRA)
            .await
            .expect("wrong-region discovery is safely empty")
            .is_none(),
        "changing the client region GUC cannot expose a run outside the server-owned mapping"
    );
    let active_routes = run_discovery
        .active_run_page(FR_PAR, None, 64)
        .await
        .expect("discover active same-region workflow routes");
    assert!(
        active_routes.routes.iter().any(|route| {
            route.tenant.0 == tenant_a
                && route.wf_run_id == "30000000-0000-0000-0000-000000000204"
                && route.partition == 7
        }),
        "active recovery returns the exact tenant and workflow UUID"
    );
    assert!(
        active_routes
            .routes
            .iter()
            .all(|route| route.tenant.0 != tenant_other_region),
        "active recovery cannot cross the server-mapped region"
    );
    assert!(run_discovery
        .active_run_page(DE_FRA, None, 64)
        .await
        .expect("wrong-region active discovery is safely empty")
        .routes
        .is_empty());
    let first_active = run_discovery
        .active_run_page(FR_PAR, None, 1)
        .await
        .expect("first active keyset page");
    assert_eq!(first_active.routes.len(), 1);
    assert_eq!(first_active.routes[0].tenant.0, tenant_a);
    let second_active = run_discovery
        .active_run_page(FR_PAR, first_active.next_cursor.as_ref(), 1)
        .await
        .expect("second active keyset page");
    assert_eq!(second_active.routes.len(), 1);
    assert_eq!(second_active.routes[0].tenant.0, tenant_b);

    let baseline_null_stage = region_store
        .count_non_terminal_null_stage_jobs(FR_PAR)
        .await
        .expect("region-authorized activation guard");
    sqlx::query("UPDATE job_queue SET stage = NULL WHERE tenant_id = $1")
        .bind(&tenant_b)
        .execute(&admin)
        .await
        .expect("simulate one historical pre-stage row");
    assert_eq!(
        region_store
            .count_non_terminal_null_stage_jobs(FR_PAR)
            .await
            .expect("guard reads job_queue through scheduler capability"),
        baseline_null_stage + 1,
        "the guard observes this fixture in addition to any historical dev-stack backlog"
    );
    sqlx::query("UPDATE job_queue SET stage = 'build' WHERE tenant_id = $1")
        .bind(&tenant_b)
        .execute(&admin)
        .await
        .expect("repair the historical fixture before claim");
    assert_eq!(
        region_store
            .count_non_terminal_null_stage_jobs(FR_PAR)
            .await
            .expect("guard after fixture repair"),
        baseline_null_stage
    );

    // CT-007 lease/topology reconciliation: the checkout-composition activation guard, read through
    // the SAME scheduler capability. It counts pre-expand rows without gating the ordinary runner
    // lane — a legacy row's workload still runs correctly under the flat fallback.
    let baseline_null_window = region_store
        .count_non_terminal_null_claim_window_jobs(FR_PAR)
        .await
        .expect("region-authorized claim-window activation guard");
    sqlx::query("UPDATE job_queue SET claim_window_secs = NULL WHERE tenant_id = $1")
        .bind(&tenant_b)
        .execute(&admin)
        .await
        .expect("simulate one historical pre-expand row");
    assert_eq!(
        region_store
            .count_non_terminal_null_claim_window_jobs(FR_PAR)
            .await
            .expect("guard observes the pre-expand fixture"),
        baseline_null_window + 1
    );
    sqlx::query("UPDATE job_queue SET claim_window_secs = $2 WHERE tenant_id = $1")
        .bind(&tenant_b)
        .bind(CI_RUNNER_EXECUTION_LEASE_TTL_SECS)
        .execute(&admin)
        .await
        .expect("repair the pre-expand fixture before claim");
    assert_eq!(
        region_store
            .count_non_terminal_null_claim_window_jobs(FR_PAR)
            .await
            .expect("guard after claim-window repair"),
        baseline_null_window
    );

    let first = region_store
        .claim(FR_PAR, &labels, &[TrustTier::Trusted], "worker-one", 30)
        .await
        .expect("first cross-tenant claim")
        .expect("first same-region row");
    let second = region_store
        .claim(FR_PAR, &labels, &[TrustTier::Trusted], "worker-two", 30)
        .await
        .expect("second cross-tenant claim")
        .expect("second same-region row");
    assert_eq!(
        BTreeSet::from([first.tenant_id.clone(), second.tenant_id.clone()]),
        BTreeSet::from([tenant_a.clone(), tenant_b.clone()]),
        "one region scheduler claims across tenants, but only in its mapped region"
    );
    assert!(
        region_store
            .claim(DE_FRA, &labels, &[TrustTier::Trusted], "wrong-region", 30)
            .await
            .expect("other-region claim is safely empty")
            .is_none(),
        "changing the client region GUC cannot escape the server-owned fr-par mapping"
    );

    sqlx::query(
        "UPDATE job_queue
            SET lease_expires = now() - interval '1 second'
          WHERE tenant_id = ANY($1) AND state = 'leased'",
    )
    .bind(&tenants[..2])
    .execute(&admin)
    .await
    .expect("expire one lease in each tenant");
    let reaper = JobQueueReaper::new(region_store.clone(), FR_PAR, Duration::from_secs(15));
    assert!(
        reaper.reap_once().await.expect("real reaper sweep") >= 2,
        "the production reaper capability recovers expired leases across same-region tenants"
    );
    let recovered: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM job_queue
          WHERE tenant_id = ANY($1) AND state = 'queued' AND lease_owner IS NULL",
    )
    .bind(&tenants[..2])
    .fetch_one(&admin)
    .await
    .expect("verify both proof leases recovered");
    assert_eq!(recovered, 2);
    assert!(tenant_store
        .complete(&tenant_a, FR_PAR, &first.job_id.to_string())
        .await
        .expect("terminalize tenant A fixture"));
    assert!(tenant_store
        .complete(&tenant_b, FR_PAR, &second.job_id.to_string())
        .await
        .expect("terminalize tenant B fixture"));

    for (fixture, ordinal) in [
        (job(&tenant_a, FR_PAR, &proof_label, 104), 104),
        (job(&tenant_b, FR_PAR, &proof_label, 105), 105),
    ] {
        tenant_store
            .enqueue(&fixture)
            .await
            .expect("enqueue concurrent fixture");
        insert_active_job_owner(&admin, &fixture.tenant_id, &fixture.region, ordinal).await;
    }
    let claim_a = region_store.clone();
    let claim_b = region_store.clone();
    let labels_a = labels.clone();
    let labels_b = labels.clone();
    let (claimed_a, claimed_b) = tokio::join!(
        async move {
            claim_a
                .claim(FR_PAR, &labels_a, &[TrustTier::Trusted], "concurrent-a", 30)
                .await
        },
        async move {
            claim_b
                .claim(FR_PAR, &labels_b, &[TrustTier::Trusted], "concurrent-b", 30)
                .await
        }
    );
    let claimed_a = claimed_a.expect("concurrent claim A").expect("row A");
    let claimed_b = claimed_b.expect("concurrent claim B").expect("row B");
    assert_ne!(
        claimed_a.job_id, claimed_b.job_id,
        "FOR UPDATE SKIP LOCKED prevents a double lease"
    );

    // Use the same reset-on-release constructor as the provider with one physical connection so a
    // released session-level poison must be scrubbed before that exact connection is reused.
    let raw_scheduler = connect_pool_with_reset(&scheduler_url, FR_PAR, 1)
        .await
        .expect("connect scheduler proof pool");
    {
        let mut connection = raw_scheduler
            .acquire()
            .await
            .expect("acquire proof connection");
        connection
            .execute("SET myelin.tenant_id = 'poison-tenant'; SET myelin.region = 'poison-region'")
            .await
            .expect("set session residue");
    }
    let reset_scope: (String, String) = sqlx::query_as(
        "SELECT current_setting('myelin.tenant_id', true),
                current_setting('myelin.region', true)",
    )
    .fetch_one(&raw_scheduler)
    .await
    .expect("reuse reset connection");
    assert_eq!(reset_scope, (String::new(), String::new()));

    // The existing PUBLIC permissive tenant policy is not an escape hatch. Even with a real tenant
    // and the mapped region selected, the scheduler's RESTRICTIVE guard requires empty tenant scope.
    let mut tenant_escape = raw_scheduler
        .begin()
        .await
        .expect("begin tenant escape attempt");
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true),
                set_config('myelin.region', 'fr-par', true)",
    )
    .bind(&tenant_a)
    .execute(&mut *tenant_escape)
    .await
    .expect("set forged tenant scope");
    let escaped_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM job_queue")
        .fetch_one(&mut *tenant_escape)
        .await
        .expect("restrictive policy read");
    let escaped_updates = sqlx::query("UPDATE job_queue SET state = 'queued' WHERE tenant_id = $1")
        .bind(&tenant_a)
        .execute(&mut *tenant_escape)
        .await
        .expect("restrictive policy update");
    let escaped_runs: Vec<String> = sqlx::query_scalar("SELECT tenant_id FROM ci_run")
        .fetch_all(&mut *tenant_escape)
        .await
        .expect("restrictive ci_run policy read");
    assert_eq!(escaped_rows, 0);
    assert_eq!(escaped_updates.rows_affected(), 0);
    assert!(escaped_runs.is_empty());
    tenant_escape
        .rollback()
        .await
        .expect("rollback escape attempt");

    let mut wrong_region = raw_scheduler
        .begin()
        .await
        .expect("begin wrong-region attempt");
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', '', true),
                set_config('myelin.region', 'de-fra', true)",
    )
    .execute(&mut *wrong_region)
    .await
    .expect("set wrong client region");
    let other_region_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM job_queue")
        .fetch_one(&mut *wrong_region)
        .await
        .expect("mapped-region policy read");
    let other_region_runs: Vec<String> = sqlx::query_scalar("SELECT tenant_id FROM ci_run")
        .fetch_all(&mut *wrong_region)
        .await
        .expect("mapped-region ci_run policy read");
    assert_eq!(other_region_rows, 0);
    assert!(other_region_runs.is_empty());
    wrong_region
        .rollback()
        .await
        .expect("rollback wrong-region attempt");

    assert_permission_denied(
        sqlx::query("INSERT INTO job_queue DEFAULT VALUES")
            .execute(&raw_scheduler)
            .await,
        "scheduler INSERT must be denied",
    );
    assert_permission_denied(
        sqlx::query("DELETE FROM job_queue WHERE true")
            .execute(&raw_scheduler)
            .await,
        "scheduler DELETE must be denied",
    );
    // CT-007 lease/topology reconciliation: the immutable claim window is dispatch authority the
    // scheduler only READS when sizing `claim_expires_at`. An explicit negative alongside the
    // dynamic excess-column probe in `ci_scheduler_db.rs`.
    assert_permission_denied(
        sqlx::query("UPDATE job_queue SET claim_window_secs = 1 WHERE true")
            .execute(&raw_scheduler)
            .await,
        "scheduler claim_window_secs mutation must be denied",
    );
    let scheduler_reads_claim_window: bool = sqlx::query_scalar(
        "SELECT pg_catalog.has_column_privilege(
           session_user, 'public.job_queue', 'claim_window_secs', 'SELECT')",
    )
    .fetch_one(&raw_scheduler)
    .await
    .expect("inspect scheduler claim-window read privilege");
    assert!(
        scheduler_reads_claim_window,
        "the claim must be able to READ the durable window it sizes claim_expires_at from"
    );

    // CT-007 round-2 blocker 4: a POSITIVE, real-role proof — not merely "the query is permitted".
    // Seed a VISIBLE superseded-version run, observe it through the exact production scheduler
    // provider's discovery, remediate it through the REAL `DurableExecutor::cancel`, and prove the
    // diagnostic then reports nothing. `fetch_optional().is_ok()` would have proven none of this.
    let stranded_run = format!("stranded-{suffix}");
    sqlx::query(
        "INSERT INTO workflow_run (
           tenant_id, region, run_id, wf_type, wf_version, input, state, correlation_id,
           depth, partition
         ) VALUES ($1, $2, $3, 'ci.pipeline', $4, '[]'::jsonb, 'running', $3, 0, 0)",
    )
    .bind(&tenant_a)
    .bind(FR_PAR)
    .bind(&stranded_run)
    .bind(myelin_ci_controlplane::CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION)
    .execute(&admin)
    .await
    .expect("seed a visible superseded-version run");

    let visible = run_discovery
        .superseded_definition_runs(
            FR_PAR,
            myelin_ci_controlplane::CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION,
            16,
        )
        .await
        .expect("the guard's read is granted to the dedicated scheduler role");
    assert!(
        visible
            .iter()
            .any(|run| run.wf_run_id == stranded_run && run.tenant.0 == tenant_a),
        "FORCE RLS must EXPOSE the seeded row to the real scheduler provider — a permitted query \
         that returns nothing proves nothing; got {visible:?}"
    );

    // REAL remediation: the production cancellation path the refusal message names, not an UPDATE.
    let executor = myelin_flow::PgFlowExecutor::new(
        app.clone(),
        tokio::runtime::Handle::current(),
        std::sync::Arc::new(myelin_events::MonotonicMinter::new()),
        myelin_tenancy::TenantId(tenant_a.clone()),
        myelin_tenancy::Region(FR_PAR.to_owned()),
    );
    myelin_flow::DurableExecutor::cancel(
        &executor,
        &myelin_flow::RunId(stranded_run.clone()),
        "ci.pipeline definition cutover remediation",
    )
    .expect("DurableExecutor::cancel is the documented remediation");
    let cancelled_state: String =
        sqlx::query_scalar("SELECT state FROM workflow_run WHERE run_id = $1")
            .bind(&stranded_run)
            .fetch_one(&admin)
            .await
            .expect("read the remediated run");
    assert_eq!(
        cancelled_state, "terminated",
        "the real cancel path writes a terminal state"
    );
    assert!(
        run_discovery
            .superseded_definition_runs(
                FR_PAR,
                myelin_ci_controlplane::CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION,
                16,
            )
            .await
            .expect("post-remediation discovery")
            .iter()
            .all(|run| run.wf_run_id != stranded_run),
        "performing the documented remediation must actually clear the diagnostic"
    );

    assert_permission_denied(
        sqlx::query("SELECT input FROM workflow_run LIMIT 1")
            .execute(&raw_scheduler)
            .await,
        "scheduler workflow payload read must stay denied",
    );
    assert_permission_denied(
        sqlx::query("SELECT * FROM ci_run LIMIT 1")
            .execute(&raw_scheduler)
            .await,
        "scheduler broad ci_run read must be denied",
    );
    assert_permission_denied(
        sqlx::query("SELECT definition_snapshot FROM ci_run LIMIT 1")
            .execute(&raw_scheduler)
            .await,
        "scheduler sensitive ci_run column read must be denied",
    );
    assert_permission_denied(
        sqlx::query("UPDATE ci_run SET state = 'running' WHERE true")
            .execute(&raw_scheduler)
            .await,
        "scheduler ci_run mutation must be denied",
    );
    assert_permission_denied(
        sqlx::query("SELECT * FROM myelin_ci_scheduler_region_map")
            .execute(&raw_scheduler)
            .await,
        "scheduler mapping read must be denied",
    );
    assert_permission_denied(
        sqlx::query(
            "INSERT INTO myelin_ci_scheduler_region_map (session_role, region)
             VALUES ('myelin_ci_scheduler_fr_par', 'de-fra')",
        )
        .execute(&raw_scheduler)
        .await,
        "scheduler mapping mutation must be denied",
    );

    let app_capability: bool = sqlx::query_scalar(
        "SELECT pg_catalog.pg_has_role(current_user, 'myelin_ci_region_scheduler', 'MEMBER')",
    )
    .fetch_one(&app)
    .await
    .expect("inspect app membership");
    assert!(!app_capability, "the tenant app is not a scheduler member");
    let mut app_tenant = app.begin().await.expect("begin app tenant read");
    sqlx::query(
        "SELECT set_config('myelin.tenant_id', $1, true),
                set_config('myelin.region', 'fr-par', true)",
    )
    .bind(&tenant_a)
    .execute(&mut *app_tenant)
    .await
    .expect("scope app tenant read");
    let own_rows: i64 = sqlx::query_scalar("SELECT count(*) FROM job_queue WHERE tenant_id = $1")
        .bind(&tenant_a)
        .fetch_one(&mut *app_tenant)
        .await
        .expect("read own tenant rows");
    let foreign_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM job_queue WHERE tenant_id = $1")
            .bind(&tenant_b)
            .fetch_one(&mut *app_tenant)
            .await
            .expect("read foreign tenant rows");
    assert!(own_rows > 0);
    assert_eq!(foreign_rows, 0, "ordinary app RLS remains tenant-isolated");
    app_tenant.rollback().await.expect("rollback app read");

    let app_as_scheduler = CiSchedulerDbConfig::from_parts(
        distinct_raw_url(&app_url, "scheduler-wrong-app"),
        &app_url,
        &admin_url,
        FR_PAR.to_owned(),
    )
    .expect("raw DSNs differ for identity probe");
    let app_refusal = match CiSchedulerDbProvider::connect(app_as_scheduler, &app).await {
        Err(error) => error,
        Ok(_) => panic!("app credential must be rejected"),
    };
    assert_eq!(app_refusal, CiSchedulerDbError::RolesNotDistinct);
    let admin_as_scheduler = CiSchedulerDbConfig::from_parts(
        distinct_raw_url(&admin_url, "scheduler-wrong-admin"),
        &app_url,
        &admin_url,
        FR_PAR.to_owned(),
    )
    .expect("raw DSNs differ for identity probe");
    let admin_refusal = match CiSchedulerDbProvider::connect(admin_as_scheduler, &app).await {
        Err(error) => error,
        Ok(_) => panic!("admin credential must be rejected"),
    };
    assert_eq!(admin_refusal, CiSchedulerDbError::Superuser);
    let wrong_region_config =
        CiSchedulerDbConfig::from_parts(scheduler_url, &app_url, &admin_url, DE_FRA.to_owned())
            .expect("valid scheduler credential with mismatched configured region");
    let region_refusal = match CiSchedulerDbProvider::connect(wrong_region_config, &app).await {
        Err(error) => error,
        Ok(_) => panic!("server mapping must override configured client region"),
    };
    assert_eq!(region_refusal, CiSchedulerDbError::RegionMismatch);
    })
    .catch_unwind()
    .await;

    cleanup(&admin, &tenants).await;
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
    },
    )
    .await;
}

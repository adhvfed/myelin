//! Live proof of the dedicated, least-privilege CI region scheduler boundary.
#![cfg(feature = "integration")]

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use myelin_ci_controlplane::{
    ci_controlplane_hot_tables, ci_controlplane_migrations, ci_job_queue_store,
    CiSchedulerDbConfig, CiSchedulerDbError, CiSchedulerDbProvider, DurableEnqueue, EnqueueOutcome,
    JobQueueReaper, Lane,
};
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dedicated_scheduler_role_is_region_bound_least_privilege_and_reset_safe() {
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
    cleanup(&admin, &tenants).await;

    let app = connect_pool_with_reset(&app_url, FR_PAR, 4)
        .await
        .expect("connect constrained app pool");
    let tenant_store = ci_job_queue_store(app.clone());
    for fixture in [
        job(&tenant_a, FR_PAR, &proof_label, 101),
        job(&tenant_b, FR_PAR, &proof_label, 102),
        job(&tenant_other_region, DE_FRA, &proof_label, 103),
    ] {
        assert_eq!(
            tenant_store
                .enqueue(&fixture)
                .await
                .expect("tenant enqueue"),
            EnqueueOutcome::Inserted
        );
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
    let labels = vec![proof_label.clone()];

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

    for fixture in [
        job(&tenant_a, FR_PAR, &proof_label, 104),
        job(&tenant_b, FR_PAR, &proof_label, 105),
    ] {
        tenant_store
            .enqueue(&fixture)
            .await
            .expect("enqueue concurrent fixture");
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
    assert_eq!(escaped_rows, 0);
    assert_eq!(escaped_updates.rows_affected(), 0);
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
    assert_eq!(other_region_rows, 0);
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
    assert_permission_denied(
        sqlx::query("SELECT * FROM ci_run LIMIT 1")
            .execute(&raw_scheduler)
            .await,
        "scheduler unrelated-table read must be denied",
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

    cleanup(&admin, &tenants).await;
}

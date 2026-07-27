//! R4.2 Tier-P operational reservation proof.
//!
//! The concrete provider commits one complete manifest batch into Storage's durable reservation
//! ledger, returns the same request-bound handles after acknowledgement loss, serializes concurrent
//! retries, refuses divergent authority and exhausted tenant capacity, and leaves no partial rows
//! when PostgreSQL fails midway through the batch. This is operational capacity control only:
//! there is no wallet, billing, Stripe, or customer price.
#![cfg(feature = "integration")]

use myelin_ci_controlplane::{
    CiJobBudgetReservationProvider, CiJobRuntimeAuthorityRequest, CiManifestLimitsV1,
    PgTierPCiJobBudgetReservation, LINUX_SMALL_V1_POLICY_REVISION,
};
use myelin_ci_sandbox::{derive_checkout_authorization_scope, JobKind, WorkspaceSpec};
use myelin_storage::{reserve_settle_durable_migrations, with_tenant_tx, PgError, PgMigrator};
use sqlx::{Executor, PgPool, Row};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

async fn pinned_pool(url: &str, schema: &str) -> PgPool {
    let schema = schema.to_owned();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
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
        .expect("connect to the live development PostgreSQL")
}

fn request(tenant: &str, run_suffix: u8, job_suffix: u8) -> CiJobRuntimeAuthorityRequest {
    CiJobRuntimeAuthorityRequest {
        tenant_id: tenant.into(),
        region: "fr-par".into(),
        ci_run_id: format!("10000000-0000-0000-0000-{run_suffix:012}"),
        wf_run_id: format!("20000000-0000-0000-0000-{run_suffix:012}"),
        project_id: "30000000-0000-0000-0000-000000000001".into(),
        job_id: format!("40000000-0000-0000-0000-{job_suffix:012}"),
        stage: format!("stage-{job_suffix}"),
        concrete_name: format!("job-{job_suffix}"),
        trigger_kind: "push".into(),
        trust_tier: "trusted".into(),
        source_snapshot_digest: format!("blake3:{}", "a".repeat(64)),
        workflow_definition_version: 1,
        workflow_code_hash: format!("blake3:{}", "b".repeat(64)),
        policy_revision: LINUX_SMALL_V1_POLICY_REVISION.into(),
        limits: CiManifestLimitsV1 {
            cpu_millis: 1_000,
            mem_bytes: 256 * 1024 * 1024,
            disk_bytes: 1024 * 1024 * 1024,
            pids_max: 128,
            timeout_secs: 600,
        },
        checkout: derive_checkout_authorization_scope(
            JobKind::Ci,
            &WorkspaceSpec {
                repo_ref: Some(format!("myelin://{tenant}/git/repo/core")),
                commit: Some("deadbeef00deadbeef00deadbeef00deadbeef00".into()),
            },
        )
        .unwrap(),
    }
}

async fn reservation_rows(pool: &PgPool, tenant: &str) -> Vec<(String, i64, String)> {
    sqlx::query(
        "SELECT run_id, reserved, state FROM cost_reservation \
         WHERE tenant_id = $1 ORDER BY run_id",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .map(|row| (row.get("run_id"), row.get("reserved"), row.get("state")))
    .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tier_p_operational_reservation_is_atomic_retry_stable_and_bounded() {
    let schema = format!("ci_operational_reservation_{}", std::process::id());
    let bootstrap = pinned_pool(&admin_url(), "public").await;
    bootstrap
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .unwrap();
    bootstrap
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .unwrap();
    let admin = pinned_pool(&admin_url(), &schema).await;
    PgMigrator::apply(&admin, &reserve_settle_durable_migrations())
        .await
        .unwrap();
    admin
        .execute(format!("GRANT USAGE ON SCHEMA {schema} TO myelin_app").as_str())
        .await
        .unwrap();
    admin
        .execute("GRANT SELECT, INSERT, UPDATE ON cost_reservation TO myelin_app")
        .await
        .unwrap();
    let runtime = pinned_pool(&app_url(), &schema).await;

    let provider = PgTierPCiJobBudgetReservation::new(runtime.clone(), "fr-par", 4).unwrap();
    let batch = vec![request("personal", 1, 1), request("personal", 1, 2)];

    // Concurrent acknowledgement-loss retries serialize on the tenant lock. One transaction
    // inserts; its waiter observes the exact complete batch and returns the same ordered handles.
    let (first, concurrent_retry) = tokio::join!(
        provider.reserve_batch(batch.clone()),
        provider.reserve_batch(batch.clone())
    );
    let first = first.unwrap();
    assert_eq!(concurrent_retry.unwrap(), first);
    assert_eq!(first.len(), 2);
    assert_ne!(first[0], first[1]);
    assert!(first
        .iter()
        .all(|handle| handle.starts_with("ci-reserve:v1:10000000-0000-0000-0000-000000000001:")));
    let rows = reservation_rows(&admin, "personal").await;
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .all(|(_, amount, state)| *amount == 750 && state == "reserved"));

    // A later replay recovers the same authority even after lifecycle advancement; it does not
    // create a second reservation or try to rewind settled work.
    sqlx::query(
        "UPDATE cost_reservation SET state = CASE run_id \
           WHEN $1 THEN 'inflight' ELSE 'settled' END WHERE tenant_id = 'personal'",
    )
    .bind(&first[0])
    .execute(&admin)
    .await
    .unwrap();
    assert_eq!(provider.reserve_batch(batch.clone()).await.unwrap(), first);
    assert_eq!(reservation_rows(&admin, "personal").await.len(), 2);

    // Batch membership and order are part of the authority. Neither a strict subset nor a reorder
    // may masquerade as an exact retry of the original complete batch.
    let subset_error = provider
        .reserve_batch(vec![batch[0].clone()])
        .await
        .unwrap_err();
    assert!(subset_error.0.contains("run authority diverged"));
    let mut reordered = batch.clone();
    reordered.reverse();
    let reorder_error = provider.reserve_batch(reordered).await.unwrap_err();
    assert!(reorder_error.0.contains("run authority diverged"));
    let disjoint_error = provider
        .reserve_batch(vec![request("personal", 1, 7), request("personal", 1, 8)])
        .await
        .unwrap_err();
    assert!(disjoint_error.0.contains("run authority diverged"));
    assert_eq!(reservation_rows(&admin, "personal").await.len(), 2);

    // The job id is the durable idempotency identity. Changed immutable authority under that id
    // refuses instead of allocating a second handle.
    let mut divergent = batch.clone();
    divergent[0].stage = "forged-stage".into();
    let error = provider.reserve_batch(divergent).await.unwrap_err();
    assert!(error.0.contains("run authority diverged"));
    assert_eq!(reservation_rows(&admin, "personal").await.len(), 2);

    // One still-inflight reservation plus a fresh two-job batch exceeds a ceiling of two. The
    // complete new batch is refused and leaves no rows.
    let tight = PgTierPCiJobBudgetReservation::new(runtime.clone(), "fr-par", 2).unwrap();
    let before = reservation_rows(&admin, "personal").await.len();
    let error = tight
        .reserve_batch(vec![request("personal", 2, 3), request("personal", 2, 4)])
        .await
        .unwrap_err();
    assert!(error.0.contains("ceiling is exhausted"));
    assert_eq!(reservation_rows(&admin, "personal").await.len(), before);

    // Different fresh runs contend under the same tenant ceiling. The advisory lock makes the
    // capacity decision serial: exactly one complete two-job batch commits, never both.
    let race_provider = PgTierPCiJobBudgetReservation::new(runtime.clone(), "fr-par", 2).unwrap();
    let race_a = vec![
        request("capacity-race", 4, 9),
        request("capacity-race", 4, 10),
    ];
    let race_b = vec![
        request("capacity-race", 5, 11),
        request("capacity-race", 5, 12),
    ];
    let (race_a_result, race_b_result) = tokio::join!(
        race_provider.reserve_batch(race_a),
        race_provider.reserve_batch(race_b)
    );
    assert_ne!(race_a_result.is_ok(), race_b_result.is_ok());
    assert_eq!(reservation_rows(&admin, "capacity-race").await.len(), 2);

    // Force PostgreSQL to fail on the second INSERT. The provider's one transaction rolls the first
    // INSERT back too; after the fault is removed, an ordinary retry commits both.
    let crash_tenant = "crash-proof";
    let crash_batch = vec![request(crash_tenant, 3, 5), request(crash_tenant, 3, 6)];
    admin
        .execute(
            "CREATE FUNCTION fail_second_operational_reservation() RETURNS trigger \
             LANGUAGE plpgsql AS $$ BEGIN \
               IF NEW.run_id LIKE 'ci-reserve:v1:%:40000000-0000-0000-0000-000000000006:%' \
               THEN RAISE EXCEPTION 'injected mid-batch failure'; END IF; RETURN NEW; END $$",
        )
        .await
        .unwrap();
    admin
        .execute(
            "CREATE TRIGGER fail_second_operational_reservation \
             BEFORE INSERT ON cost_reservation FOR EACH ROW \
             EXECUTE FUNCTION fail_second_operational_reservation()",
        )
        .await
        .unwrap();
    let crash_provider = PgTierPCiJobBudgetReservation::new(runtime.clone(), "fr-par", 4).unwrap();
    assert!(crash_provider
        .reserve_batch(crash_batch.clone())
        .await
        .is_err());
    assert!(
        reservation_rows(&admin, crash_tenant).await.is_empty(),
        "a mid-batch database failure rolls the entire reservation transaction back"
    );
    admin
        .execute("DROP TRIGGER fail_second_operational_reservation ON cost_reservation")
        .await
        .unwrap();
    admin
        .execute("DROP FUNCTION fail_second_operational_reservation()")
        .await
        .unwrap();
    let recovered = crash_provider
        .reserve_batch(crash_batch.clone())
        .await
        .unwrap();
    assert_eq!(
        crash_provider.reserve_batch(crash_batch).await.unwrap(),
        recovered
    );
    assert_eq!(reservation_rows(&admin, crash_tenant).await.len(), 2);

    // The concrete runtime role sees only the transaction-scoped tenant through FORCE RLS.
    let visible: i64 = with_tenant_tx(&runtime, "personal", "fr-par", |conn| {
        Box::pin(async move {
            sqlx::query_scalar("SELECT count(*) FROM cost_reservation")
                .fetch_one(conn)
                .await
                .map_err(|error| PgError::Query(error.to_string()))
        })
    })
    .await
    .unwrap();
    assert_eq!(visible, 2);

    runtime.close().await;
    admin.close().await;
    bootstrap
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .unwrap();
    bootstrap.close().await;
}

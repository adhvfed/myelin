//! R4.2 Tier-P operational reservation proof.
//!
//! The concrete provider commits one complete manifest batch into Storage's durable reservation
//! ledger, returns the same request-bound handles after acknowledgement loss, serializes concurrent
//! retries, refuses divergent authority and exhausted tenant capacity, and leaves no partial rows
//! when PostgreSQL fails midway through the batch. This is operational capacity control only:
//! there is no wallet, billing, Stripe, or customer price.
#![cfg(feature = "integration")]

use myelin_ci_controlplane::{
    CiAttemptBudgetPolicy, CiAttemptBudgetRevision, CiJobBudgetReservationProvider,
    CiJobRuntimeAuthorityRequest, CiManifestLimitsV1, OperationalReservationWriteVersion,
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

    let provider = PgTierPCiJobBudgetReservation::new(
        runtime.clone(),
        "fr-par",
        4,
        CiAttemptBudgetPolicy::production(),
        OperationalReservationWriteVersion::V2,
    )
    .unwrap();
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
        .all(|handle| handle.starts_with("ci-reserve:v2:10000000-0000-0000-0000-000000000001:")));
    let rows = reservation_rows(&admin, "personal").await;
    assert_eq!(rows.len(), 2);
    // CT-007 slice 5b.3-4a.1c: fresh batches now mint v2 for real, so the reservation amount covers
    // the full parent-attempt*max_attempts budget (checkout job, production policy) instead of one
    // v1 workload execution's ceiling (750).
    assert!(rows.iter().all(|(run_id, amount, state)| *amount == 15_000
        && state == "reserved"
        && run_id.starts_with("ci-reserve:v2:")));

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
    let tight = PgTierPCiJobBudgetReservation::new(
        runtime.clone(),
        "fr-par",
        2,
        CiAttemptBudgetPolicy::production(),
        OperationalReservationWriteVersion::V2,
    )
    .unwrap();
    let before = reservation_rows(&admin, "personal").await.len();
    let error = tight
        .reserve_batch(vec![request("personal", 2, 3), request("personal", 2, 4)])
        .await
        .unwrap_err();
    assert!(error.0.contains("ceiling is exhausted"));
    assert_eq!(reservation_rows(&admin, "personal").await.len(), before);

    // Different fresh runs contend under the same tenant ceiling. The advisory lock makes the
    // capacity decision serial: exactly one complete two-job batch commits, never both.
    let race_provider = PgTierPCiJobBudgetReservation::new(
        runtime.clone(),
        "fr-par",
        2,
        CiAttemptBudgetPolicy::production(),
        OperationalReservationWriteVersion::V2,
    )
    .unwrap();
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
               IF NEW.run_id LIKE 'ci-reserve:v2:%:40000000-0000-0000-0000-000000000006:%' \
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
    let crash_provider = PgTierPCiJobBudgetReservation::new(
        runtime.clone(),
        "fr-par",
        4,
        CiAttemptBudgetPolicy::production(),
        OperationalReservationWriteVersion::V2,
    )
    .unwrap();
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

/// CT-007 slice 5b.3-4a.1b: the active-reservation-ceiling COUNT must treat a durable `ci-reserve:v2:...`
/// row as counting against the SAME tenant ceiling as `ci-reserve:v1:...` rows, even though this slice's
/// writer still only ever mints `v1` handles. Manually insert one `v2`-shaped row to simulate a future
/// writer, then prove a fresh `v1` batch request is refused once the ceiling is exhausted by it alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_v2_reservation_row_counts_toward_the_v1_batch_ceiling() {
    let schema = format!(
        "ci_operational_reservation_v2_ceiling_{}",
        std::process::id()
    );
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

    let tenant = "v2-ceiling-tenant";
    let v2_run_id = "ci-reserve:v2:90000000-0000-0000-0000-000000000001:budget:trusted:\
                     40000000-0000-0000-0000-000000000099:deadbeef";
    sqlx::query(
        "INSERT INTO cost_reservation (tenant_id, region, run_id, reserved, state) \
         VALUES ($1, 'fr-par', $2, 500, 'reserved')",
    )
    .bind(tenant)
    .bind(v2_run_id)
    .execute(&admin)
    .await
    .unwrap();

    let provider = PgTierPCiJobBudgetReservation::new(
        runtime.clone(),
        "fr-par",
        1,
        CiAttemptBudgetPolicy::production(),
        OperationalReservationWriteVersion::V1,
    )
    .unwrap();
    let error = provider
        .reserve_batch(vec![request(tenant, 9, 50)])
        .await
        .unwrap_err();
    assert!(
        error.0.contains("ceiling is exhausted"),
        "the pre-existing v2 row alone already saturates a ceiling of 1: {}",
        error.0
    );
    assert_eq!(
        reservation_rows(&admin, tenant).await,
        vec![(v2_run_id.into(), 500, "reserved".into())],
        "the refused v1 batch leaves no new row and the v2 row untouched"
    );

    runtime.close().await;
    admin.close().await;
    bootstrap
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .unwrap();
    bootstrap.close().await;
}

/// CT-007 slice 5b.3-4a.1c: a run that already has durable `v1` rows must keep replaying `v1`
/// exactly, even when the provider handling the retry is configured to WRITE `v2` for fresh
/// batches -- durable precedence, not the provider's current write setting, decides what replays.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_existing_v1_batch_replays_unchanged_through_a_v2_writing_provider() {
    let schema = format!(
        "ci_operational_reservation_v1_precedence_{}",
        std::process::id()
    );
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

    let batch = vec![
        request("v1-precedence", 6, 20),
        request("v1-precedence", 6, 21),
    ];

    let v1_writer = PgTierPCiJobBudgetReservation::new(
        runtime.clone(),
        "fr-par",
        4,
        CiAttemptBudgetPolicy::production(),
        OperationalReservationWriteVersion::V1,
    )
    .unwrap();
    let seeded = v1_writer.reserve_batch(batch.clone()).await.unwrap();
    assert!(seeded
        .iter()
        .all(|handle| handle.starts_with("ci-reserve:v1:")));
    assert_eq!(reservation_rows(&admin, "v1-precedence").await.len(), 2);

    let v2_writer = PgTierPCiJobBudgetReservation::new(
        runtime.clone(),
        "fr-par",
        4,
        CiAttemptBudgetPolicy::production(),
        OperationalReservationWriteVersion::V2,
    )
    .unwrap();
    let replayed = v2_writer.reserve_batch(batch).await.unwrap();
    assert_eq!(
        replayed, seeded,
        "an existing v1 batch must replay unchanged"
    );
    assert_eq!(
        reservation_rows(&admin, "v1-precedence").await.len(),
        2,
        "a v2-writing provider must not insert a second row for an already-durable v1 run"
    );

    runtime.close().await;
    admin.close().await;
    bootstrap
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .unwrap();
    bootstrap.close().await;
}

/// CT-007 slice 5b.3-4a.1c: a `v2` batch written under one attempt policy must still replay
/// correctly through a provider configured with a DIFFERENT policy -- replay recovers the policy
/// from the durable handle's own descriptor, never from whatever the replaying provider is
/// currently configured with.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn v2_written_under_one_policy_replays_correctly_under_a_differently_configured_provider() {
    let schema = format!(
        "ci_operational_reservation_v2_policy_drift_{}",
        std::process::id()
    );
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

    let batch = vec![
        request("v2-policy-drift", 7, 30),
        request("v2-policy-drift", 7, 31),
    ];

    let written_policy = CiAttemptBudgetPolicy::new(
        CiAttemptBudgetRevision::V1,
        std::num::NonZeroU32::new(3).unwrap(),
    );
    let original_writer = PgTierPCiJobBudgetReservation::new(
        runtime.clone(),
        "fr-par",
        4,
        written_policy,
        OperationalReservationWriteVersion::V2,
    )
    .unwrap();
    let written = original_writer.reserve_batch(batch.clone()).await.unwrap();
    assert!(written
        .iter()
        .all(|handle| handle.contains(":budget-v1:a3:")));

    let reconfigured = PgTierPCiJobBudgetReservation::new(
        runtime.clone(),
        "fr-par",
        4,
        CiAttemptBudgetPolicy::production(),
        OperationalReservationWriteVersion::V2,
    )
    .unwrap();
    let replayed = reconfigured.reserve_batch(batch).await.unwrap();
    assert_eq!(
        replayed, written,
        "replay must recover the ORIGINAL 3-attempt policy from the durable handle, not \
         production's 5-attempt policy this second provider is configured with"
    );
    assert_eq!(reservation_rows(&admin, "v2-policy-drift").await.len(), 2);

    runtime.close().await;
    admin.close().await;
    bootstrap
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .unwrap();
    bootstrap.close().await;
}

/// CT-007 slice 5b.3-4a.1c: a genuinely fresh `v2` write whose ceiling arithmetic overflows must
/// leave zero durable rows -- `build_v2_candidates` computes the complete batch before any INSERT,
/// so a single unrepresentable job refuses the whole batch atomically.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fresh_v2_overflow_leaves_zero_rows() {
    let schema = format!(
        "ci_operational_reservation_v2_overflow_{}",
        std::process::id()
    );
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

    // Checkout job (4 executions/attempt) under production's 5-attempt policy: 20x the raw
    // ceiling. mem_bytes = u64::MAX / 15 makes that 20x overflow u64 -- same fixture the unit
    // tests use to trigger this exact overflow.
    let mut overflow_request = request("v2-overflow", 8, 40);
    overflow_request.limits.mem_bytes = u64::MAX / 15;
    overflow_request.limits.timeout_secs = 1;

    let provider = PgTierPCiJobBudgetReservation::new(
        runtime.clone(),
        "fr-par",
        4,
        CiAttemptBudgetPolicy::production(),
        OperationalReservationWriteVersion::V2,
    )
    .unwrap();
    let error = provider
        .reserve_batch(vec![overflow_request])
        .await
        .unwrap_err();
    assert!(error.0.contains("overflow"), "message was: {}", error.0);
    assert!(
        reservation_rows(&admin, "v2-overflow").await.is_empty(),
        "an overflowing fresh v2 batch must leave zero durable rows"
    );

    runtime.close().await;
    admin.close().await;
    bootstrap
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .unwrap();
    bootstrap.close().await;
}

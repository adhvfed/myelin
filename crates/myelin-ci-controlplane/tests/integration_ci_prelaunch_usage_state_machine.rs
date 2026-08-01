//! Live PostgreSQL proof for CT-007 slice 5b.3-4a.2's Rust journal state machine.
#![cfg(feature = "integration")]

mod common;

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use common::with_schema_cleanup;
use myelin_ci_controlplane::claim_window_secs_for_template;
use myelin_ci_controlplane::{
    resolve_prelaunch_usage_on_conn, CiAttemptBudgetPolicy, CiAttemptBudgetRevision,
    CiDriveManifestStore, CiDriveManifestV1, CiJobBudgetReservationProvider,
    CiJobRuntimeAuthorityRequest, CiJobSpecStore, CiJobTokenRequest, CiManifestLaneV1,
    CiParentAttemptAdmission,
    CiManifestLimitsV1, CiManifestSchedulingV1, CiManifestTrustTierV1, CiManifestWorkspaceV1,
    CiPrelaunchJournalOutcome, CiPrelaunchParentExpectation, CiPrelaunchSettlementIdentity,
    CiPrelaunchUnresolvedPolicy, CiPrelaunchUsageJournal, CiPrelaunchUsageJournalError,
    CiPrelaunchUsagePhase, DurableCiJobLaunchTemplate, DurableEnqueue, GrantedCiJobV1, Lane,
    ManifestBoundCiJobTokenAuthority, OperationalReservationWriteVersion,
    PgTierPCiJobBudgetReservation,
};
use myelin_ci_sandbox::{
    derive_checkout_authorization_scope, EgressPolicy, IdemToken, ImageRef, JobKind,
    JobSpecTemplate, MeterTarget, ResourceLimits, ResourceUsage, TrustTier, WorkspaceSpec,
};
use myelin_storage::{reserve_settle_durable_migrations, HotTables, PgMigrator};
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, PgPool};

const TENANT: &str = "prelaunch-state";
const REGION: &str = "fr-par";
const REPO_REF: &str = "myelin://prelaunch-state/git/repo/core";
const COMMIT_OID: &str = "deadbeef00deadbeef00deadbeef00deadbeef00";

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
        .max_connections(6)
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
        .expect("connect to live PostgreSQL")
}

fn digest(byte: char) -> String {
    format!("blake3:{}", byte.to_string().repeat(64))
}

fn uuid(prefix: u8, seed: u64) -> String {
    format!("{prefix:02x}000000-0000-4000-8000-{seed:012x}")
}

fn checkout_scope() -> myelin_ci_sandbox::CheckoutAuthorizationScope {
    derive_checkout_authorization_scope(
        JobKind::Ci,
        &WorkspaceSpec {
            repo_ref: Some(REPO_REF.into()),
            commit: Some(COMMIT_OID.into()),
        },
    )
    .unwrap()
    .unwrap()
}

#[derive(Clone, Copy)]
enum FixtureMutation {
    None,
    DispatchedMeter,
    ReservationDigest,
    ReservationAmount,
    ReservationSettled,
}

struct Fixture {
    claim: CiJobTokenRequest,
    reserve_handle: String,
}

async fn seed_fixture(
    app: &PgPool,
    admin: &PgPool,
    seed: u64,
    policy: CiAttemptBudgetPolicy,
    mutation: FixtureMutation,
) -> Fixture {
    let ci_run_id = uuid(0x10, seed);
    let wf_run_id = uuid(0x20, seed);
    let project_id = uuid(0x30, seed);
    let job_id = uuid(0x40, seed);
    let pipeline_id = uuid(0x50, seed);
    let claim_nonce = uuid(0x60, seed);
    let limits = CiManifestLimitsV1 {
        cpu_millis: 1_000,
        mem_bytes: 256 * 1024 * 1024,
        disk_bytes: 1024 * 1024 * 1024,
        pids_max: 128,
        timeout_secs: 120,
    };
    let authority = CiJobRuntimeAuthorityRequest {
        tenant_id: TENANT.into(),
        region: REGION.into(),
        ci_run_id: ci_run_id.clone(),
        wf_run_id: wf_run_id.clone(),
        project_id: project_id.clone(),
        job_id: job_id.clone(),
        stage: "build".into(),
        concrete_name: "build".into(),
        trigger_kind: "push".into(),
        trust_tier: "trusted".into(),
        source_snapshot_digest: digest('a'),
        workflow_definition_version: 3,
        workflow_code_hash: digest('c'),
        policy_revision: "linux-small-v1:1".into(),
        limits: limits.clone(),
        checkout: Some(checkout_scope()),
    };
    let provider = PgTierPCiJobBudgetReservation::new(
        app.clone(),
        REGION,
        100,
        policy,
        OperationalReservationWriteVersion::V2,
    )
    .unwrap();
    let original_handle = provider
        .reserve_batch(vec![authority.clone()])
        .await
        .unwrap()
        .remove(0);
    let reserve_handle = match mutation {
        FixtureMutation::ReservationDigest => {
            let mut tampered = original_handle.clone();
            let last = tampered.pop().unwrap();
            tampered.push(if last == '0' { '1' } else { '0' });
            sqlx::query(
                "UPDATE cost_reservation SET run_id = $1
                 WHERE tenant_id = $2 AND region = $3 AND run_id = $4",
            )
            .bind(&tampered)
            .bind(TENANT)
            .bind(REGION)
            .bind(&original_handle)
            .execute(admin)
            .await
            .unwrap();
            tampered
        }
        _ => original_handle,
    };
    match mutation {
        FixtureMutation::ReservationAmount => {
            sqlx::query(
                "UPDATE cost_reservation SET reserved = reserved + 1
                 WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
            )
            .bind(TENANT)
            .bind(REGION)
            .bind(&reserve_handle)
            .execute(admin)
            .await
            .unwrap();
        }
        FixtureMutation::ReservationSettled => {
            sqlx::query(
                "UPDATE cost_reservation SET state = 'settled'
                 WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
            )
            .bind(TENANT)
            .bind(REGION)
            .bind(&reserve_handle)
            .execute(admin)
            .await
            .unwrap();
        }
        _ => {}
    }

    sqlx::query(
        "INSERT INTO ci_run (
           tenant_id, region, run_id, project_id, repo_ref, commit_oid, pipeline_id, wf_run_id,
           definition_snapshot, trigger_kind, trust_tier, state, correlation_id
         ) VALUES ($1, $2, $3::uuid, $4::uuid, $5, $6, $7::uuid, $8::uuid,
                   'snapshot', 'push', 'trusted', 'running', $9)",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(&ci_run_id)
    .bind(&project_id)
    .bind(REPO_REF)
    .bind(COMMIT_OID)
    .bind(&pipeline_id)
    .bind(&wf_run_id)
    .bind(format!("corr-{seed}"))
    .execute(admin)
    .await
    .unwrap();

    let token_authority_handle = ManifestBoundCiJobTokenAuthority::handle_for(&authority);
    let image = ImageRef::pinned(format!("registry.example/ci@sha256:{}", "b".repeat(64))).unwrap();
    let manifest = CiDriveManifestV1 {
        schema_version: 1,
        tenant_id: TENANT.into(),
        region: REGION.into(),
        wf_run_id: wf_run_id.clone(),
        ci_run_id: ci_run_id.clone(),
        source_snapshot_ref: format!("myelin://{TENANT}/ci/artifact/snapshot-{}", digest('a')),
        source_plan_schema_version: 2,
        launch_request_digest: digest('b'),
        workflow_type: "ci.pipeline".into(),
        workflow_definition_version: authority.workflow_definition_version,
        workflow_code_hash: authority.workflow_code_hash.clone(),
        authority_policy_revision: authority.policy_revision.clone(),
        repo_ref: REPO_REF.into(),
        commit_oid: COMMIT_OID.into(),
        run_ref: format!("myelin://{TENANT}/ci/run/{ci_run_id}"),
        started_at: "2026-07-29T12:00:00.000000Z".into(),
        trust_tier: CiManifestTrustTierV1::Trusted,
        check_attempts: BTreeMap::from([("build".into(), 1)]),
        merge_waiter: None,
        jobs: vec![GrantedCiJobV1 {
            job_id: job_id.clone(),
            stage: "build".into(),
            name: "build".into(),
            check_context: "build".into(),
            needs: Vec::new(),
            matrix_key: BTreeMap::new(),
            image: image.reference.clone(),
            command: vec!["true".into()],
            env: BTreeMap::new(),
            secret_handles: BTreeMap::new(),
            egress_allow: Vec::new(),
            limits: limits.clone(),
            workspace: CiManifestWorkspaceV1 {
                repo_ref: REPO_REF.into(),
                commit_oid: COMMIT_OID.into(),
                read_only_root: true,
                tmpfs_scratch: true,
            },
            scheduling: CiManifestSchedulingV1 {
                lane: CiManifestLaneV1::Batch,
                labels: vec!["linux".into()],
                concurrency_group: None,
                fair_key: format!("project-{seed}"),
            },
            reserve_handle: reserve_handle.clone(),
            token_authority_handle: token_authority_handle.clone(),
            continue_on_error: false,
        }],
    };
    CiDriveManifestStore::new(app.clone(), TenantId(TENANT.into()), Region(REGION.into()))
        .unwrap()
        .insert(&manifest)
        .await
        .unwrap();

    let dispatched_reserve = if matches!(mutation, FixtureMutation::DispatchedMeter) {
        format!("{reserve_handle}-different")
    } else {
        reserve_handle.clone()
    };
    let idem_token = format!("prelaunch-{seed}");
    let launch = DurableCiJobLaunchTemplate {
        spec: JobSpecTemplate {
            kind: JobKind::Ci,
            image,
            command: vec!["true".into()],
            env: Vec::new(),
            secret_refs: Vec::new(),
            egress: EgressPolicy::deny_all(),
            limits: ResourceLimits {
                cpu_millis: limits.cpu_millis,
                mem_bytes: limits.mem_bytes,
                disk_bytes: limits.disk_bytes,
                tmpfs_bytes: limits.disk_bytes,
                pids_max: limits.pids_max,
                timeout_secs: limits.timeout_secs,
            },
            workspace: WorkspaceSpec {
                repo_ref: Some(REPO_REF.into()),
                commit: Some(COMMIT_OID.into()),
            },
            trust_tier: TrustTier::Trusted,
            meter_to: MeterTarget {
                reserve_id: dispatched_reserve,
            },
            idem_token: IdemToken(idem_token.clone()),
        },
        ci_run_id: ci_run_id.clone(),
        token_authority_handle: token_authority_handle.clone(),
    };
    CiJobSpecStore::with_pg(app.clone())
        .co_persist_dispatch(
            &DurableEnqueue {
                tenant_id: TENANT.into(),
                region: REGION.into(),
                job_id: job_id.clone(),
                run_id: wf_run_id.clone(),
                lane: Lane::Batch,
                labels: vec!["linux".into()],
                trust_tier: TrustTier::Trusted,
                concurrency_group: None,
                fair_key: format!("project-{seed}"),
                idem_token: idem_token.clone(),
                stage: "build".into(),
                claim_window_secs: claim_window_secs_for_template(&launch.spec).unwrap(),
                reservation_write_version: myelin_ci_controlplane::ReservationWriteVersionMarker::derive_from_reserve_handle(
                    &launch.spec.meter_to.reserve_id,
                ),
            },
            &launch,
            "build",
        )
        .await
        .unwrap();

    let now = chrono::Utc::now().timestamp();
    let claim_started_at_epoch_secs = now - 1;
    let claim_expires_at_epoch_secs = now + 120;
    sqlx::query(
        "UPDATE job_queue
         SET state = 'leased', lease_owner = 'runner-1', lease_epoch = 1,
             claim_nonce = $1::uuid, claim_started_at = to_timestamp($2),
             claim_expires_at = to_timestamp($3), lease_expires = to_timestamp($3)
         WHERE tenant_id = $4 AND region = $5 AND job_id = $6::uuid",
    )
    .bind(&claim_nonce)
    .bind(claim_started_at_epoch_secs)
    .bind(claim_expires_at_epoch_secs)
    .bind(TENANT)
    .bind(REGION)
    .bind(&job_id)
    .execute(admin)
    .await
    .unwrap();
    Fixture {
        claim: CiJobTokenRequest {
            tenant_id: TENANT.into(),
            region: REGION.into(),
            wf_run_id,
            ci_run_id,
            job_id,
            token_authority_handle,
            idem_token,
            lease_owner: "runner-1".into(),
            lease_epoch: 1,
            claim_nonce,
            claim_started_at_epoch_secs,
            claim_expires_at_epoch_secs,
        },
        reserve_handle,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_prelaunch_usage_state_machine_is_exact_replay_safe_and_fail_closed() {
    let schema = format!(
        "ci_prelaunch_state_{}_{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    );
    let bootstrap = pinned_pool(&admin_url(), "public").await;
    bootstrap
        .execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str())
        .await
        .unwrap();
    bootstrap
        .execute(format!("CREATE SCHEMA {schema} AUTHORIZATION myelin_admin").as_str())
        .await
        .unwrap();
    with_schema_cleanup(&bootstrap, &schema, || async {
        let admin = pinned_pool(&admin_url(), &schema).await;
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
            &reserve_settle_durable_migrations(),
            &HotTables::declare(["cost_event"]),
        )
        .await
        .unwrap();
        common::with_fixture_migration_lock(&admin_url(), &admin, &schema, || async {
            PgMigrator::apply_validated(
                &admin,
                &myelin_ci_controlplane::ci_controlplane_migrations(),
                &myelin_ci_controlplane::ci_controlplane_hot_tables(),
            )
            .await
            .unwrap();
        })
        .await;
        let app = pinned_pool(&app_url(), &schema).await;
        let journal = CiPrelaunchUsageJournal::new(app.clone(), REGION).unwrap();

        // Fresh admission transitions a reserved row to inflight and creates exactly one attempt.
        let exact = seed_fixture(
            &app,
            &admin,
            1,
            CiAttemptBudgetPolicy::production(),
            FixtureMutation::None,
        )
        .await;
        let (attempt, outcome) = journal
            .begin_parent_attempt(&exact.claim, &exact.reserve_handle)
            .await
            .unwrap();
        assert_eq!(outcome, CiPrelaunchJournalOutcome::Applied);
        let state: String =
            sqlx::query_scalar("SELECT state FROM cost_reservation WHERE run_id = $1")
                .bind(&exact.reserve_handle)
                .fetch_one(&admin)
                .await
                .unwrap();
        assert_eq!(state, "inflight");
        let (replayed, outcome) = journal
            .begin_parent_attempt(&exact.claim, &exact.reserve_handle)
            .await
            .unwrap();
        assert_eq!(outcome, CiPrelaunchJournalOutcome::Replayed);
        assert_eq!(replayed.job_id(), attempt.job_id());
        let attempts: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM ci_job_parent_attempt WHERE job_id = $1::uuid",
        )
        .bind(&exact.claim.job_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(attempts, 1);

        // Two simultaneous acknowledgers serialize on the reservation/advisory boundary: exactly
        // one inserts and the other recovers the same immutable generation.
        let concurrent = seed_fixture(
            &app,
            &admin,
            8,
            CiAttemptBudgetPolicy::production(),
            FixtureMutation::None,
        )
        .await;
        let left = journal.clone();
        let right = journal.clone();
        let left_claim = concurrent.claim.clone();
        let right_claim = concurrent.claim.clone();
        let left_handle = concurrent.reserve_handle.clone();
        let right_handle = concurrent.reserve_handle.clone();
        let (left_result, right_result) = tokio::join!(
            async move { left.begin_parent_attempt(&left_claim, &left_handle).await },
            async move {
                right
                    .begin_parent_attempt(&right_claim, &right_handle)
                    .await
            }
        );
        let left_outcome = left_result.unwrap().1;
        let right_outcome = right_result.unwrap().1;
        assert!(
            matches!(
                (left_outcome, right_outcome),
                (
                    CiPrelaunchJournalOutcome::Applied,
                    CiPrelaunchJournalOutcome::Replayed
                ) | (
                    CiPrelaunchJournalOutcome::Replayed,
                    CiPrelaunchJournalOutcome::Applied
                )
            ),
            "one concurrent begin applies and the other exactly replays"
        );
        let concurrent_rows: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM ci_job_parent_attempt WHERE job_id = $1::uuid",
        )
        .bind(&concurrent.claim.job_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(concurrent_rows, 1);

        assert_eq!(
            journal
                .begin_phase(&attempt, CiPrelaunchUsagePhase::CheckoutTransport)
                .await
                .unwrap(),
            CiPrelaunchJournalOutcome::Applied
        );
        assert_eq!(
            journal
                .begin_phase(&attempt, CiPrelaunchUsagePhase::CheckoutTransport)
                .await
                .unwrap(),
            CiPrelaunchJournalOutcome::Replayed
        );
        let measured = ResourceUsage {
            cpu_seconds: 7,
            mem_byte_seconds: 9,
        };
        assert_eq!(
            journal
                .complete_phase(&attempt, CiPrelaunchUsagePhase::CheckoutTransport, measured,)
                .await
                .unwrap(),
            CiPrelaunchJournalOutcome::Applied
        );
        assert_eq!(
            journal
                .complete_phase(&attempt, CiPrelaunchUsagePhase::CheckoutTransport, measured,)
                .await
                .unwrap(),
            CiPrelaunchJournalOutcome::Replayed
        );
        assert_eq!(
            journal
                .complete_phase(
                    &attempt,
                    CiPrelaunchUsagePhase::CheckoutTransport,
                    ResourceUsage {
                        cpu_seconds: 8,
                        mem_byte_seconds: 9,
                    },
                )
                .await
                .unwrap_err(),
            CiPrelaunchUsageJournalError::PhaseDivergence
        );
        assert_eq!(
            journal
                .seal_phase(&attempt, CiPrelaunchUsagePhase::CheckoutTransport)
                .await
                .unwrap_err(),
            CiPrelaunchUsageJournalError::IllegalPhaseTransition
        );

        assert_eq!(
            journal
                .begin_phase(&attempt, CiPrelaunchUsagePhase::CheckoutMaterialization,)
                .await
                .unwrap(),
            CiPrelaunchJournalOutcome::Applied
        );
        assert_eq!(
            journal
                .seal_phase(&attempt, CiPrelaunchUsagePhase::CheckoutMaterialization,)
                .await
                .unwrap(),
            CiPrelaunchJournalOutcome::Applied
        );
        assert_eq!(
            journal
                .seal_phase(&attempt, CiPrelaunchUsagePhase::CheckoutMaterialization,)
                .await
                .unwrap(),
            CiPrelaunchJournalOutcome::Replayed
        );
        assert_eq!(
            journal
                .complete_phase(
                    &attempt,
                    CiPrelaunchUsagePhase::CheckoutMaterialization,
                    measured,
                )
                .await
                .unwrap_err(),
            CiPrelaunchUsageJournalError::IllegalPhaseTransition
        );

        let mut resolve_tx = app.begin().await.unwrap();
        sqlx::query(
            "SELECT set_config('myelin.tenant_id', $1, true),
                    set_config('myelin.region', $2, true)",
        )
        .bind(TENANT)
        .bind(REGION)
        .execute(&mut *resolve_tx)
        .await
        .unwrap();
        let accrual = resolve_prelaunch_usage_on_conn(
            &mut resolve_tx,
            CiPrelaunchSettlementIdentity {
                tenant_id: TENANT,
                region: REGION,
                job_id: &exact.claim.job_id,
                wf_run_id: &exact.claim.wf_run_id,
                ci_run_id: &exact.claim.ci_run_id,
                reserve_handle: &exact.reserve_handle,
            },
            CiPrelaunchParentExpectation::Required,
            CiPrelaunchUnresolvedPolicy::Refuse,
        )
        .await
        .unwrap();
        resolve_tx.commit().await.unwrap();
        assert_eq!(accrual.parent_attempts, 1);
        assert_eq!(accrual.measured_phases, 1);
        assert_eq!(accrual.sealed_phases, 1);
        assert_eq!(
            accrual.usage,
            ResourceUsage {
                cpu_seconds: 127,
                mem_byte_seconds: 32_212_254_729,
            },
            "exact transport usage plus the materialization ceiling is consumed once"
        );

        // A terminal owner may either refuse an unresolved phase or atomically seal it to the
        // immutable ceiling while holding the same queue/advisory lock order.
        let unresolved = seed_fixture(
            &app,
            &admin,
            9,
            CiAttemptBudgetPolicy::production(),
            FixtureMutation::None,
        )
        .await;
        let (unresolved_attempt, _) = journal
            .begin_parent_attempt(&unresolved.claim, &unresolved.reserve_handle)
            .await
            .unwrap();
        journal
            .begin_phase(
                &unresolved_attempt,
                CiPrelaunchUsagePhase::CheckoutMaterialization,
            )
            .await
            .unwrap();
        let mut refuse_tx = app.begin().await.unwrap();
        sqlx::query(
            "SELECT set_config('myelin.tenant_id', $1, true),
                    set_config('myelin.region', $2, true)",
        )
        .bind(TENANT)
        .bind(REGION)
        .execute(&mut *refuse_tx)
        .await
        .unwrap();
        let unresolved_identity = CiPrelaunchSettlementIdentity {
            tenant_id: TENANT,
            region: REGION,
            job_id: &unresolved.claim.job_id,
            wf_run_id: &unresolved.claim.wf_run_id,
            ci_run_id: &unresolved.claim.ci_run_id,
            reserve_handle: &unresolved.reserve_handle,
        };
        assert_eq!(
            resolve_prelaunch_usage_on_conn(
                &mut refuse_tx,
                unresolved_identity,
                CiPrelaunchParentExpectation::Required,
                CiPrelaunchUnresolvedPolicy::Refuse,
            )
            .await
            .unwrap_err(),
            CiPrelaunchUsageJournalError::UnresolvedPhase
        );
        refuse_tx.rollback().await.unwrap();

        let mut seal_tx = app.begin().await.unwrap();
        sqlx::query(
            "SELECT set_config('myelin.tenant_id', $1, true),
                    set_config('myelin.region', $2, true)",
        )
        .bind(TENANT)
        .bind(REGION)
        .execute(&mut *seal_tx)
        .await
        .unwrap();
        let sealed_accrual = resolve_prelaunch_usage_on_conn(
            &mut seal_tx,
            unresolved_identity,
            CiPrelaunchParentExpectation::Required,
            CiPrelaunchUnresolvedPolicy::SealToCeiling,
        )
        .await
        .unwrap();
        seal_tx.commit().await.unwrap();
        assert_eq!(sealed_accrual.measured_phases, 0);
        assert_eq!(sealed_accrual.sealed_phases, 1);
        assert_eq!(
            sealed_accrual.usage,
            ResourceUsage {
                cpu_seconds: 120,
                mem_byte_seconds: 32_212_254_720,
            }
        );

        // Worker-side phase mutation re-verifies the exact live queue generation before touching
        // the journal. A requeued/stale generation is refused before an INSERT can occur.
        let stale = seed_fixture(
            &app,
            &admin,
            10,
            CiAttemptBudgetPolicy::production(),
            FixtureMutation::None,
        )
        .await;
        let (stale_attempt, _) = journal
            .begin_parent_attempt(&stale.claim, &stale.reserve_handle)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE job_queue SET state = 'queued', lease_owner = NULL, lease_expires = NULL
             WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(&stale.claim.job_id)
        .execute(&admin)
        .await
        .unwrap();
        assert_eq!(
            journal
                .begin_phase(
                    &stale_attempt,
                    CiPrelaunchUsagePhase::CheckoutMaterialization,
                )
                .await
                .unwrap_err(),
            CiPrelaunchUsageJournalError::ClaimUnavailable
        );

        // Each authority/refusal seam is exercised independently.
        let wrong_manifest_handle = {
            let mut value = exact.reserve_handle.clone();
            let last = value.pop().unwrap();
            value.push(if last == '0' { '1' } else { '0' });
            value
        };
        assert_eq!(
            journal
                .begin_parent_attempt(&exact.claim, &wrong_manifest_handle)
                .await
                .unwrap_err(),
            CiPrelaunchUsageJournalError::ManifestReserveHandleMismatch
        );

        let meter = seed_fixture(
            &app,
            &admin,
            2,
            CiAttemptBudgetPolicy::production(),
            FixtureMutation::DispatchedMeter,
        )
        .await;
        assert_eq!(
            journal
                .begin_parent_attempt(&meter.claim, &meter.reserve_handle)
                .await
                .unwrap_err(),
            CiPrelaunchUsageJournalError::DispatchedReserveHandleMismatch
        );
        assert_eq!(
            journal
                .begin_parent_attempt(
                    &exact.claim,
                    &format!(
                        "ci-reserve:v1:{}:{}:{}:{}",
                        exact.claim.ci_run_id,
                        "a".repeat(64),
                        exact.claim.job_id,
                        "b".repeat(64)
                    ),
                )
                .await
                .unwrap_err(),
            CiPrelaunchUsageJournalError::LegacyReservation
        );

        let settled = seed_fixture(
            &app,
            &admin,
            3,
            CiAttemptBudgetPolicy::production(),
            FixtureMutation::ReservationSettled,
        )
        .await;
        assert_eq!(
            journal
                .begin_parent_attempt(&settled.claim, &settled.reserve_handle)
                .await
                .unwrap_err(),
            CiPrelaunchUsageJournalError::ReservationNotLaunchable
        );

        let digest_mismatch = seed_fixture(
            &app,
            &admin,
            4,
            CiAttemptBudgetPolicy::production(),
            FixtureMutation::ReservationDigest,
        )
        .await;
        assert_eq!(
            journal
                .begin_parent_attempt(&digest_mismatch.claim, &digest_mismatch.reserve_handle)
                .await
                .unwrap_err(),
            CiPrelaunchUsageJournalError::ReservationAuthorityMismatch
        );
        let amount_mismatch = seed_fixture(
            &app,
            &admin,
            5,
            CiAttemptBudgetPolicy::production(),
            FixtureMutation::ReservationAmount,
        )
        .await;
        assert_eq!(
            journal
                .begin_parent_attempt(&amount_mismatch.claim, &amount_mismatch.reserve_handle)
                .await
                .unwrap_err(),
            CiPrelaunchUsageJournalError::ReservationAuthorityMismatch
        );

        // The cap counts parent-attempt rows, including a zero-preparation generation.
        let one_attempt_policy =
            CiAttemptBudgetPolicy::new(CiAttemptBudgetRevision::V1, NonZeroU32::new(1).unwrap());
        let capped = seed_fixture(&app, &admin, 6, one_attempt_policy, FixtureMutation::None).await;
        journal
            .begin_parent_attempt(&capped.claim, &capped.reserve_handle)
            .await
            .unwrap();
        let mut replacement = capped.claim.clone();
        replacement.lease_epoch = 2;
        replacement.claim_nonce = uuid(0x61, 6);
        replacement.claim_started_at_epoch_secs += 1;
        replacement.claim_expires_at_epoch_secs += 1;
        sqlx::query(
            "UPDATE job_queue
             SET lease_epoch = $1, claim_nonce = $2::uuid,
                 claim_started_at = to_timestamp($3), claim_expires_at = to_timestamp($4)
             WHERE tenant_id = $5 AND region = $6 AND job_id = $7::uuid",
        )
        .bind(replacement.lease_epoch)
        .bind(&replacement.claim_nonce)
        .bind(replacement.claim_started_at_epoch_secs)
        .bind(replacement.claim_expires_at_epoch_secs)
        .bind(TENANT)
        .bind(REGION)
        .bind(&replacement.job_id)
        .execute(&admin)
        .await
        .unwrap();
        assert_eq!(
            journal
                .begin_parent_attempt(&replacement, &capped.reserve_handle)
                .await
                .unwrap_err(),
            CiPrelaunchUsageJournalError::ParentAttemptLimitExceeded
        );

        // ---- CT-007 slice 5b.3-6d: the typed exhaustion capability ----
        // The FIRST admitted attempt transitioned the reservation reserved -> inflight, and that
        // committed. This is the load-bearing invariant: a reserve that has ANY committed parent row
        // is already inflight, so an exhausted terminal never strands a `reserved` reservation.
        let reservation_state = |handle: String| {
            let admin = admin.clone();
            async move {
                sqlx::query_scalar::<_, String>(
                    "SELECT state FROM cost_reservation
                     WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
                )
                .bind(TENANT)
                .bind(REGION)
                .bind(handle)
                .fetch_one(&admin)
                .await
                .unwrap()
            }
        };
        assert_eq!(
            reservation_state(capped.reserve_handle.clone()).await,
            "inflight",
            "the first admitted attempt transitioned the reserve to inflight"
        );
        let parent_rows = |job_id: String| {
            let admin = admin.clone();
            async move {
                sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM ci_job_parent_attempt
                     WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
                )
                .bind(TENANT)
                .bind(REGION)
                .bind(job_id)
                .fetch_one(&admin)
                .await
                .unwrap()
            }
        };
        assert_eq!(parent_rows(capped.claim.job_id.clone()).await, 1);

        // `admit_parent_attempt` for the SAME exhausted, never-admitted replacement generation returns
        // the typed AttemptsExhausted capability (NOT an error), carrying the settleable reserve.
        match journal
            .admit_parent_attempt(&replacement, &capped.reserve_handle)
            .await
            .unwrap()
        {
            CiParentAttemptAdmission::AttemptsExhausted { reserve_handle } => {
                assert_eq!(reserve_handle, capped.reserve_handle);
            }
            CiParentAttemptAdmission::Admitted { .. } => {
                panic!("an exhausted budget must not admit a new parent attempt")
            }
        }
        // The commit created NO row for the refused generation, and left the reserve inflight (never
        // stranded reserved): the exhausted terminal path can settle it.
        assert_eq!(parent_rows(capped.claim.job_id.clone()).await, 1);
        assert_eq!(
            reservation_state(capped.reserve_handle.clone()).await,
            "inflight",
            "an exhausted admission commits, leaving the reserve inflight and settleable"
        );

        // The Admitted arm: a fresh, non-exhausted claim admits through `admit_parent_attempt` and
        // transitions its own reserve to inflight in the same tenant transaction.
        let admittable = seed_fixture(
            &app,
            &admin,
            11,
            CiAttemptBudgetPolicy::production(),
            FixtureMutation::None,
        )
        .await;
        match journal
            .admit_parent_attempt(&admittable.claim, &admittable.reserve_handle)
            .await
            .unwrap()
        {
            CiParentAttemptAdmission::Admitted { attempt, outcome } => {
                assert_eq!(outcome, CiPrelaunchJournalOutcome::Applied);
                assert_eq!(attempt.job_id(), admittable.claim.job_id);
            }
            CiParentAttemptAdmission::AttemptsExhausted { .. } => {
                panic!("a fresh claim within budget must admit")
            }
        }
        assert_eq!(parent_rows(admittable.claim.job_id.clone()).await, 1);
        assert_eq!(
            reservation_state(admittable.reserve_handle.clone()).await,
            "inflight"
        );
        // Exact replay of the admitted generation is idempotent (Replayed, still one row).
        assert!(matches!(
            journal
                .admit_parent_attempt(&admittable.claim, &admittable.reserve_handle)
                .await
                .unwrap(),
            CiParentAttemptAdmission::Admitted {
                outcome: CiPrelaunchJournalOutcome::Replayed,
                ..
            }
        ));
        assert_eq!(parent_rows(admittable.claim.job_id.clone()).await, 1);

        // ---- CT-007 slice 5b.3-6d: the exhausted admission COMMITS its OWN reserved -> inflight
        // transition. A regression moving the transition AFTER exhaustion detection would strand the
        // reserve as `reserved`. Construct a full budget whose reservation is DELIBERATELY still
        // `reserved` (max prior rows inserted directly, never through the admit path that flips it). ----
        let stranded = seed_fixture(
            &app,
            &admin,
            12,
            CiAttemptBudgetPolicy::production(),
            FixtureMutation::None,
        )
        .await;
        sqlx::query(
            "UPDATE cost_reservation SET state = 'reserved'
             WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(&stranded.reserve_handle)
        .execute(&admin)
        .await
        .unwrap();
        // Five parent rows for OTHER generations (epochs 2..=6) fill the exact-policy budget without
        // ever transitioning the reservation. The fixture's own claim (epoch 1) has no row.
        for epoch in 2_i64..=6 {
            sqlx::query(
                "INSERT INTO ci_job_parent_attempt (
                   tenant_id, region, job_id, wf_run_id, ci_run_id, reserve_handle, lease_owner,
                   lease_epoch, claim_nonce, claim_started_at_epoch_secs,
                   claim_expires_at_epoch_secs, budget_revision, max_parent_attempts
                 ) VALUES ($1,$2,$3::uuid,$4::uuid,$5::uuid,$6,$7,$8,$9::uuid,$10,$11,1,5)",
            )
            .bind(TENANT)
            .bind(REGION)
            .bind(&stranded.claim.job_id)
            .bind(&stranded.claim.wf_run_id)
            .bind(&stranded.claim.ci_run_id)
            .bind(&stranded.reserve_handle)
            .bind(format!("prior-runner-{epoch}"))
            .bind(epoch)
            .bind(uuid(0x6b, epoch as u64))
            .bind(stranded.claim.claim_started_at_epoch_secs - epoch)
            .bind(stranded.claim.claim_expires_at_epoch_secs - epoch)
            .execute(&admin)
            .await
            .unwrap();
        }
        assert_eq!(
            reservation_state(stranded.reserve_handle.clone()).await,
            "reserved",
            "budget is full but the reservation is deliberately still reserved"
        );
        match journal
            .admit_parent_attempt(&stranded.claim, &stranded.reserve_handle)
            .await
            .unwrap()
        {
            CiParentAttemptAdmission::AttemptsExhausted { reserve_handle } => {
                assert_eq!(reserve_handle, stranded.reserve_handle);
            }
            CiParentAttemptAdmission::Admitted { .. } => panic!("a full budget must not admit"),
        }
        // THE regression catch: the committed exhaustion transitioned the reserve to inflight.
        assert_eq!(
            reservation_state(stranded.reserve_handle.clone()).await,
            "inflight",
            "an exhausted admission COMMITS its own reserved -> inflight transition"
        );
        // No row was created for the refused (current) generation.
        assert_eq!(parent_rows(stranded.claim.job_id.clone()).await, 5);
        let epoch_one_rows: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM ci_job_parent_attempt
             WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid AND lease_epoch = 1",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(&stranded.claim.job_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(epoch_one_rows, 0, "the refused generation has no parent row");

        // settled / cancelled / absent reservations NEVER yield the capability and are left unchanged.
        for forced in ["settled", "cancelled"] {
            sqlx::query(
                "UPDATE cost_reservation SET state = $4
                 WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
            )
            .bind(TENANT)
            .bind(REGION)
            .bind(&stranded.reserve_handle)
            .bind(forced)
            .execute(&admin)
            .await
            .unwrap();
            assert_eq!(
                journal
                    .admit_parent_attempt(&stranded.claim, &stranded.reserve_handle)
                    .await
                    .unwrap_err(),
                CiPrelaunchUsageJournalError::ReservationNotLaunchable
            );
            assert_eq!(
                reservation_state(stranded.reserve_handle.clone()).await,
                forced,
                "a non-launchable reservation is refused before exhaustion and left unchanged"
            );
        }
        sqlx::query(
            "DELETE FROM cost_reservation WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(&stranded.reserve_handle)
        .execute(&admin)
        .await
        .unwrap();
        assert_eq!(
            journal
                .admit_parent_attempt(&stranded.claim, &stranded.reserve_handle)
                .await
                .unwrap_err(),
            CiPrelaunchUsageJournalError::ReservationUnavailable
        );
        assert_eq!(parent_rows(stranded.claim.job_id.clone()).await, 5);

        // The Rust text-cast path preserves the complete u64 domain introduced by the schema slice.
        let max_usage = seed_fixture(
            &app,
            &admin,
            7,
            CiAttemptBudgetPolicy::production(),
            FixtureMutation::None,
        )
        .await;
        let (max_attempt, _) = journal
            .begin_parent_attempt(&max_usage.claim, &max_usage.reserve_handle)
            .await
            .unwrap();
        journal
            .begin_phase(&max_attempt, CiPrelaunchUsagePhase::CheckoutMaterialization)
            .await
            .unwrap();
        let full_u64 = ResourceUsage {
            cpu_seconds: u64::MAX,
            mem_byte_seconds: u64::MAX,
        };
        journal
            .complete_phase(
                &max_attempt,
                CiPrelaunchUsagePhase::CheckoutMaterialization,
                full_u64,
            )
            .await
            .unwrap();
        let persisted: (String, String) = sqlx::query_as(
            "SELECT exact_cpu_seconds::text, exact_mem_byte_seconds::text
             FROM ci_job_prelaunch_usage
             WHERE tenant_id = $1 AND job_id = $2::uuid
               AND phase = 'checkout_materialization'",
        )
        .bind(TENANT)
        .bind(&max_usage.claim.job_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(persisted.0, u64::MAX.to_string());
        assert_eq!(persisted.1, u64::MAX.to_string());
    })
    .await;
    bootstrap.close().await;
}

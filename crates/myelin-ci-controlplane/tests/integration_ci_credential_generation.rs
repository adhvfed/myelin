#![cfg(feature = "integration")]

mod common;

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use common::with_schema_cleanup;
use myelin_ci_controlplane::ci_checkout_composition::V2CheckoutComposition;
use myelin_ci_controlplane::{
    acquire_phase_generation_ownership, ci_job_queue_store, ci_region_queue_store_test_support,
    claim_window_secs_for_template, expected_phase_jti, phase_generation_id,
    verify_phase_generation_live, CiCredentialGenerationError, CiCredentialGenerationOutcome,
    CiCredentialPurpose, CiDriveManifestStore, CiDriveManifestV1, CiJobBudgetReservationProvider,
    CiJobCredentialGenerationStore, CiJobCredentialWriteVersion, CiJobLaunchClaim,
    CiJobRuntimeAuthorityRequest, CiJobSpecStore, CiJobTokenIssueError, CiJobTokenRequest,
    CiManifestLaneV1, CiManifestLimitsV1, CiManifestSchedulingV1, CiManifestTrustTierV1,
    CiManifestWorkspaceV1, CiPhaseCredentialMintRequest, CiPhaseCredentialMinter,
    CiPhaseGenerationGate, CiPhaseGenerationInputs, DurableCiJobLaunchTemplate, DurableEnqueue,
    GrantedCiJobV1, IdentityCiJobCredentialMinter, Lane, ManifestBoundCiJobTokenAuthority,
    MintedPhaseCredential, OperationalReservationWriteVersion, PgTierPCiJobBudgetReservation,
    CI_PHASE_CREDENTIAL_BINDING_V1,
};
use myelin_ci_sandbox::checkout_orchestration::ParentAttemptAdmission;
use myelin_ci_sandbox::{
    derive_checkout_authorization_scope, CheckoutPhase, EgressPolicy, IdemToken, ImageRef, JobKind,
    JobSpec, JobSpecTemplate, MeterTarget, PreparationPhase, ResourceLimits, ResourceUsage,
    RunTokenCredential, TrustTier, WorkspaceSpec,
};
use myelin_identity_service::{
    CellTokenAuthority, PasetoCapabilitySigner, RevocationStore, RunTokenMinter,
};
use myelin_storage::{reserve_settle_durable_migrations, HotTables, PgMigrator};
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, PgPool, Row};

static MIGRATION_SCENARIO_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const TENANT: &str = "phase-credential";
const REGION: &str = "fr-par";
const REPO_REF: &str = "myelin://phase-credential/git/repo/core";
const COMMIT_OID: &str = "deadbeef00deadbeef00deadbeef00deadbeef00";
const WINDOW_SECS: i64 = 4 * (120 + 600);

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
        .expect("connect to live PostgreSQL (is the dev stack up?)")
}

async fn tagged_pool(url: &str, schema: &str, application_name: &str) -> PgPool {
    tagged_pool_capped(url, schema, application_name, 4).await
}

async fn tagged_pool_capped(
    url: &str,
    schema: &str,
    application_name: &str,
    connections: u32,
) -> PgPool {
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

async fn await_lock_waiter(observer: &PgPool, application_name: &str) -> i32 {
    for _ in 0..400 {
        if let Some(pid) = sqlx::query_scalar::<_, i32>(
            "SELECT pid FROM pg_stat_activity
             WHERE application_name = $1
               AND wait_event_type = 'Lock'
               AND state = 'active'
             LIMIT 1",
        )
        .bind(application_name)
        .fetch_optional(observer)
        .await
        .unwrap()
        {
            return pid;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("no backend tagged {application_name:?} ever blocked on a lock");
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

struct CountingPhaseMinter {
    inner: IdentityCiJobCredentialMinter,
    calls: Arc<AtomicUsize>,
}

impl CiPhaseCredentialMinter for CountingPhaseMinter {
    fn mint_phase<'a>(
        &'a self,
        request: CiPhaseCredentialMintRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RunTokenCredential, CiJobTokenIssueError>> + Send + 'a>>
    {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.mint_phase(request)
    }
}

struct BlockingPhaseMinter {
    inner: IdentityCiJobCredentialMinter,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    already_blocked: Arc<std::sync::atomic::AtomicBool>,
}

impl CiPhaseCredentialMinter for BlockingPhaseMinter {
    fn mint_phase<'a>(
        &'a self,
        request: CiPhaseCredentialMintRequest,
    ) -> Pin<Box<dyn Future<Output = Result<RunTokenCredential, CiJobTokenIssueError>> + Send + 'a>>
    {
        let first = !self.already_blocked.swap(true, Ordering::SeqCst);
        let entered = self.entered.clone();
        let release = self.release.clone();
        Box::pin(async move {
            if first {
                entered.notify_one();
                release.notified().await;
            }
            self.inner.mint_phase(request).await
        })
    }
}

fn real_minter() -> (Arc<CountingPhaseMinter>, Arc<AtomicUsize>) {
    let s7 = RevocationStore::new();
    let cell = Arc::new(CellTokenAuthority::from_seed(&[21_u8; 32], &[22_u8; 32]).unwrap());
    let signer = Arc::new(PasetoCapabilitySigner::new(cell));
    let minter = RunTokenMinter::with_signer_and_tuples(s7, None, signer);
    let calls = Arc::new(AtomicUsize::new(0));
    (
        Arc::new(CountingPhaseMinter {
            inner: IdentityCiJobCredentialMinter::new(minter),
            calls: calls.clone(),
        }),
        calls,
    )
}

fn store(
    app: &PgPool,
    minter: Arc<CountingPhaseMinter>,
    write_version: CiJobCredentialWriteVersion,
) -> CiJobCredentialGenerationStore {
    CiJobCredentialGenerationStore::with_pg_and_write_version(
        app.clone(),
        REGION,
        minter,
        write_version,
    )
}

struct Fixture {
    claim: CiJobTokenRequest,
    reserve_handle: String,
}

async fn seed_fixture(app: &PgPool, admin: &PgPool, seed: u64, claim_age_secs: i64) -> Fixture {
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
    let mut authority = CiJobRuntimeAuthorityRequest {
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
        reserve_id: None,
        checkout_commit: Some(checkout_scope().commit_hex().to_owned()),
        checkout: Some(checkout_scope()),
    };
    let reserve_handle = PgTierPCiJobBudgetReservation::new(
        app.clone(),
        REGION,
        100,
        myelin_ci_controlplane::CiAttemptBudgetPolicy::production(),
        OperationalReservationWriteVersion::V2,
    )
    .unwrap()
    .reserve_batch(vec![authority.clone()])
    .await
    .unwrap()
    .remove(0);
    authority.reserve_id = Some(reserve_handle.clone());

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
    sqlx::query(
        "INSERT INTO ci_job (
           tenant_id, region, job_id, run_id, stage, name, needs, spec_ref, state, attempt
         ) VALUES ($1, $2, $3::uuid, $4::uuid, 'build', 'build', '{}'::uuid[], $5, 'queued', 1)",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(&job_id)
    .bind(&ci_run_id)
    .bind(digest('a'))
    .execute(admin)
    .await
    .unwrap();

    let token_authority_handle = ManifestBoundCiJobTokenAuthority::handle_for(&authority);
    let image = ImageRef::pinned(format!("registry.example/ci@sha256:{}", "b".repeat(64))).unwrap();
    let manifest = CiDriveManifestV1 {
        schema_version: 1,
        tenant_id: TENANT.into(),
        region: REGION.into(),
        project_id: project_id.clone(),
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
        source_ref: Some("refs/heads/main".into()),
        commit_oid: COMMIT_OID.into(),
        run_ref: format!("myelin://{TENANT}/ci/run/{ci_run_id}"),
        started_at: "2026-07-30T12:00:00.000000Z".into(),
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

    let idem_token = format!("phase-credential-{seed}");
    let launch = DurableCiJobLaunchTemplate {
        project_id: project_id.clone(),
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
                reserve_id: reserve_handle.clone(),
            },
            idem_token: IdemToken(idem_token.clone()),
        },
        ci_run_id: ci_run_id.clone(),
        token_authority_handle: token_authority_handle.clone(),
    };
    let window = claim_window_secs_for_template(&launch.spec).unwrap();
    assert_eq!(window, WINDOW_SECS);
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
                claim_window_secs: window,
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
    let claim_started_at_epoch_secs = now - claim_age_secs;
    let claim_expires_at_epoch_secs = claim_started_at_epoch_secs + window;
    sqlx::query(
        "UPDATE job_queue
         SET state = 'leased', lease_owner = 'runner-1', lease_epoch = 1,
             claim_nonce = $1::uuid, claim_started_at = to_timestamp($2),
             claim_expires_at = to_timestamp($3), lease_expires = to_timestamp($4)
         WHERE tenant_id = $5 AND region = $6 AND job_id = $7::uuid",
    )
    .bind(&claim_nonce)
    .bind(claim_started_at_epoch_secs)
    .bind(claim_expires_at_epoch_secs)
    .bind(now + 900)
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
            project_id: project_id.clone(),
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

async fn admit_parent(admin: &PgPool, fixture: &Fixture) {
    sqlx::query(
        "INSERT INTO ci_job_parent_attempt (
           tenant_id, region, job_id, wf_run_id, ci_run_id, reserve_handle, lease_owner,
           lease_epoch, claim_nonce, claim_started_at_epoch_secs, claim_expires_at_epoch_secs,
           budget_revision, max_parent_attempts
         ) VALUES ($1, $2, $3::uuid, $4::uuid, $5::uuid, $6, $7, $8, $9::uuid, $10, $11, 1, 3)",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(&fixture.claim.job_id)
    .bind(&fixture.claim.wf_run_id)
    .bind(&fixture.claim.ci_run_id)
    .bind(&fixture.reserve_handle)
    .bind(&fixture.claim.lease_owner)
    .bind(fixture.claim.lease_epoch)
    .bind(&fixture.claim.claim_nonce)
    .bind(fixture.claim.claim_started_at_epoch_secs)
    .bind(fixture.claim.claim_expires_at_epoch_secs)
    .execute(admin)
    .await
    .unwrap();
}

async fn set_phase(admin: &PgPool, fixture: &Fixture, phase: &str, status: &str) {
    let (exact, resolved) = match status {
        "started" => (None, None),
        "measured" => (Some("1"), Some(true)),
        _ => (None, Some(true)),
    };
    sqlx::query(
        "INSERT INTO ci_job_prelaunch_usage (
           tenant_id, region, job_id, lease_epoch, claim_nonce, phase, status,
           ceiling_cpu_seconds, ceiling_mem_byte_seconds, exact_cpu_seconds,
           exact_mem_byte_seconds, started_at, resolved_at, seal_after
         ) VALUES ($1, $2, $3::uuid, $4, $5::uuid, $6, $7, 100, 100,
                   $8::text::numeric, $8::text::numeric, statement_timestamp(),
                   CASE WHEN $9 THEN statement_timestamp() ELSE NULL END,
                   statement_timestamp() + interval '1 hour')
         ON CONFLICT (tenant_id, region, job_id, lease_epoch, claim_nonce, phase) DO UPDATE
           SET status = EXCLUDED.status,
               exact_cpu_seconds = EXCLUDED.exact_cpu_seconds,
               exact_mem_byte_seconds = EXCLUDED.exact_mem_byte_seconds,
               resolved_at = EXCLUDED.resolved_at",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(&fixture.claim.job_id)
    .bind(fixture.claim.lease_epoch)
    .bind(&fixture.claim.claim_nonce)
    .bind(phase)
    .bind(status)
    .bind(exact)
    .bind(resolved.unwrap_or(false))
    .execute(admin)
    .await
    .unwrap();
}

async fn insert_overdue_started_transport(admin: &PgPool, fixture: &Fixture) {
    sqlx::query(
        "INSERT INTO ci_job_prelaunch_usage (
           tenant_id, region, job_id, lease_epoch, claim_nonce, phase, status,
           ceiling_cpu_seconds, ceiling_mem_byte_seconds, started_at, seal_after
         ) VALUES ($1, $2, $3::uuid, $4, $5::uuid, 'checkout_transport', 'started',
                   100, 100, statement_timestamp() - interval '2 minutes',
                   statement_timestamp() - interval '1 minute')",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(&fixture.claim.job_id)
    .bind(fixture.claim.lease_epoch)
    .bind(&fixture.claim.claim_nonce)
    .execute(admin)
    .await
    .unwrap();
}

async fn phase_status(admin: &PgPool, fixture: &Fixture, phase: &str) -> Option<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT status FROM ci_job_prelaunch_usage
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
           AND lease_epoch = $4 AND claim_nonce = $5::uuid AND phase = $6",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(&fixture.claim.job_id)
    .bind(fixture.claim.lease_epoch)
    .bind(&fixture.claim.claim_nonce)
    .bind(phase)
    .fetch_optional(admin)
    .await
    .unwrap()
}

async fn queue_row_lockable(admin: &PgPool, fixture: &Fixture) -> bool {
    match sqlx::query_scalar::<_, i32>(
        "SELECT 1 FROM job_queue
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
         FOR UPDATE NOWAIT",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(&fixture.claim.job_id)
    .fetch_optional(admin)
    .await
    {
        Ok(_) => true,
        Err(error) => {
            assert_eq!(
                error.as_database_error().and_then(|e| e.code()).as_deref(),
                Some("55P03"),
                "the only expected failure is lock_not_available, got: {error}"
            );
            false
        }
    }
}

async fn mint_full_sequence(
    store: &CiJobCredentialGenerationStore,
    admin: &PgPool,
    fixture: &Fixture,
) -> MintedPhaseCredential {
    admit_parent(admin, fixture).await;
    store
        .mint_phase_credential(&fixture.claim, CiCredentialPurpose::CheckoutAdvertise)
        .await
        .expect("advertise");
    set_phase(admin, fixture, "checkout_transport", "started").await;
    store
        .mint_phase_credential(&fixture.claim, CiCredentialPurpose::CheckoutFetch)
        .await
        .expect("fetch");
    set_phase(admin, fixture, "checkout_transport", "measured").await;
    set_phase(admin, fixture, "checkout_materialization", "started").await;
    store
        .mint_phase_credential(&fixture.claim, CiCredentialPurpose::CheckoutMaterialization)
        .await
        .expect("materialization");
    set_phase(admin, fixture, "checkout_materialization", "measured").await;
    store
        .mint_phase_credential(&fixture.claim, CiCredentialPurpose::Workload)
        .await
        .expect("workload")
}

fn launch_claim_of(claim: &CiJobTokenRequest) -> CiJobLaunchClaim {
    CiJobLaunchClaim {
        tenant_id: claim.tenant_id.clone(),
        region: claim.region.clone(),
        wf_run_id: claim.wf_run_id.clone(),
        job_id: claim.job_id.clone(),
        lease_owner: claim.lease_owner.clone(),
        lease_epoch: claim.lease_epoch,
        claim_nonce: claim.claim_nonce.clone(),
        claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
        claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
    }
}

async fn generation_rows(admin: &PgPool, fixture: &Fixture) -> Vec<(String, i16, String, String)> {
    sqlx::query(
        "SELECT purpose, phase_ordinal, generation_id, jti
         FROM ci_job_credential_generation
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
           AND lease_epoch = $4 AND claim_nonce = $5::uuid
         ORDER BY phase_ordinal",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(&fixture.claim.job_id)
    .bind(fixture.claim.lease_epoch)
    .bind(&fixture.claim.claim_nonce)
    .fetch_all(admin)
    .await
    .unwrap()
    .into_iter()
    .map(|row| {
        (
            row.get::<String, _>("purpose"),
            row.get::<i16, _>("phase_ordinal"),
            row.get::<String, _>("generation_id"),
            row.get::<String, _>("jti"),
        )
    })
    .collect()
}

fn expected_generation(
    claim: &CiJobTokenRequest,
    purpose: CiCredentialPurpose,
    issued: i64,
    expires: i64,
) -> (String, String) {
    let generation_id = phase_generation_id(CiPhaseGenerationInputs {
        tenant_id: &claim.tenant_id,
        region: &claim.region,
        wf_run_id: &claim.wf_run_id,
        ci_run_id: &claim.ci_run_id,
        job_id: &claim.job_id,
        token_authority_handle: &claim.token_authority_handle,
        idem_token: &claim.idem_token,
        lease_owner: &claim.lease_owner,
        lease_epoch: claim.lease_epoch,
        claim_nonce: &claim.claim_nonce,
        claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
        claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
        purpose,
        issued_at_epoch_secs: issued,
        expires_at_epoch_secs: expires,
        binding_version: CI_PHASE_CREDENTIAL_BINDING_V1,
    });
    let jti = expected_phase_jti(&generation_id, issued).unwrap();
    (generation_id, jti)
}

fn gate_for(
    claim: &CiJobTokenRequest,
    purpose: CiCredentialPurpose,
    minted: &MintedPhaseCredential,
) -> CiPhaseGenerationGate {
    CiPhaseGenerationGate {
        tenant_id: claim.tenant_id.clone(),
        region: claim.region.clone(),
        wf_run_id: claim.wf_run_id.clone(),
        ci_run_id: claim.ci_run_id.clone(),
        job_id: claim.job_id.clone(),
        token_authority_handle: claim.token_authority_handle.clone(),
        idem_token: claim.idem_token.clone(),
        checkout_commit: minted
            .checkout
            .as_ref()
            .map(|scope| scope.commit_hex().to_owned()),
        lease_owner: claim.lease_owner.clone(),
        lease_epoch: claim.lease_epoch,
        claim_nonce: claim.claim_nonce.clone(),
        claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
        claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
        purpose,
        binding_version: minted.binding.binding_version,
        generation_id: minted.binding.generation_id.clone(),
        jti: minted.binding.jti.clone(),
        issued_at_epoch_secs: minted.binding.issued_at_epoch_secs,
        expires_at_epoch_secs: minted.binding.expires_at_epoch_secs,
    }
}

async fn migrated_schema(tag: &str) -> (String, PgPool, PgPool, PgPool) {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    let schema = format!(
        "ci_phase_credential_{}_{}_{}",
        std::process::id(),
        tag,
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
    (schema, bootstrap, admin, app)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fractional_claim_epoch_is_floored_and_the_real_credential_seam_issues() {
    let (schema, bootstrap, admin, app) = migrated_schema("fractional_claim_epoch").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let (minter, calls) = real_minter();
        let store = store(&app, minter, CiJobCredentialWriteVersion::V2PhaseBound);
        let mut fixture = seed_fixture(&app, &admin, 50, 5).await;

        sqlx::query(
            "INSERT INTO workflow_run (
               tenant_id, region, run_id, wf_type, wf_version, input, state, correlation_id,
               depth, partition
             ) VALUES ($1, $2, $3, 'ci.pipeline', 1, '[]'::jsonb, 'running', $3, 0, 0)",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(&fixture.claim.wf_run_id)
        .execute(&admin)
        .await
        .unwrap();

        sqlx::query(
            "UPDATE job_queue
             SET state = 'queued', lease_owner = NULL, lease_expires = NULL,
                 claim_nonce = NULL, claim_started_at = NULL, claim_expires_at = NULL
             WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(&fixture.claim.job_id)
        .execute(&admin)
        .await
        .unwrap();

        let scheduler = ci_region_queue_store_test_support(admin.clone());
        let (leased, exact_started_epoch) = loop {
            let fraction: f64 = sqlx::query_scalar::<_, String>(
                "SELECT (EXTRACT(EPOCH FROM clock_timestamp()) % 1)::text",
            )
            .fetch_one(&admin)
            .await
            .unwrap()
            .parse()
            .unwrap();
            let wait = if fraction < 0.50 {
                0.50 - fraction
            } else if fraction >= 0.60 {
                1.50 - fraction
            } else {
                0.0
            };
            if wait > 0.0 {
                tokio::time::sleep(std::time::Duration::from_secs_f64(wait)).await;
            }

            let leased = scheduler
                .claim(
                    REGION,
                    &["linux".into()],
                    &[TrustTier::Trusted],
                    "fractional-runner",
                    900,
                )
                .await
                .unwrap()
                .expect("the real scheduler claims the fixture");
            let exact_started_epoch: f64 = sqlx::query_scalar::<_, String>(
                "SELECT EXTRACT(EPOCH FROM claim_started_at)::text
                 FROM job_queue WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
            )
            .bind(TENANT)
            .bind(REGION)
            .bind(&fixture.claim.job_id)
            .fetch_one(&admin)
            .await
            .unwrap()
            .parse()
            .unwrap();
            if exact_started_epoch.fract() >= 0.5 && exact_started_epoch.fract() < 0.7 {
                break (leased, exact_started_epoch);
            }
            sqlx::query(
                "UPDATE job_queue
                 SET state = 'queued', lease_owner = NULL, lease_expires = NULL,
                     claim_nonce = NULL, claim_started_at = NULL, claim_expires_at = NULL
                 WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
            )
            .bind(TENANT)
            .bind(REGION)
            .bind(&fixture.claim.job_id)
            .execute(&admin)
            .await
            .unwrap();
        };

        fixture.claim.lease_owner = leased.lease_owner;
        fixture.claim.lease_epoch = leased.lease_epoch;
        fixture.claim.claim_nonce = leased.claim_nonce;
        fixture.claim.claim_started_at_epoch_secs = leased.claim_started_at_epoch_secs;
        fixture.claim.claim_expires_at_epoch_secs = leased.claim_expires_at_epoch_secs;

        let minted = store
            .mint_phase_credential(
                &fixture.claim,
                CiCredentialPurpose::CheckoutAdvertise,
            )
            .await
            .expect("a fractional-second claim must issue at the credential-generation seam");

        assert_eq!(
            fixture.claim.claim_started_at_epoch_secs,
            exact_started_epoch.floor() as i64,
            "the scheduler persists the integer claim anchor by flooring"
        );
        assert!(
            minted.binding.issued_at_epoch_secs >= fixture.claim.claim_started_at_epoch_secs,
            "issued_at={} must not precede floored claim_started_at={} (exact={exact_started_epoch})",
            minted.binding.issued_at_epoch_secs,
            fixture.claim.claim_started_at_epoch_secs,
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1, "Identity issues exactly once");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_phase_sequence_is_ordered_replay_stable_and_bounded_to_four_generations() {
    let (schema, bootstrap, admin, app) = migrated_schema("sequence").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let (minter, calls) = real_minter();
        let store = store(&app, minter, CiJobCredentialWriteVersion::V2PhaseBound);
        let fixture = seed_fixture(&app, &admin, 1, 5).await;
        let claim = &fixture.claim;

        let advertise = store
            .mint_phase_credential(claim, CiCredentialPurpose::CheckoutAdvertise)
            .await
            .expect("advertise mints in the resolver, before admission");
        assert_eq!(advertise.outcome, CiCredentialGenerationOutcome::Applied);
        assert_eq!(generation_rows(&admin, &fixture).await.len(), 1);
        assert_eq!(advertise.credential.jti, advertise.binding.jti);
        assert_eq!(
            advertise.credential.ttl_secs(),
            u64::try_from(advertise.binding.expires_at_epoch_secs - advertise.binding.issued_at_epoch_secs).unwrap()
        );
        assert!(advertise.binding.generation_id.starts_with("ci-credential:v1:"));

        let replay = store
            .mint_phase_credential(claim, CiCredentialPurpose::CheckoutAdvertise)
            .await
            .expect("an exact retry replays");
        assert_eq!(replay.outcome, CiCredentialGenerationOutcome::Replayed);
        assert_eq!(generation_rows(&admin, &fixture).await.len(), 1);
        assert_eq!(replay.binding, advertise.binding);
        assert_eq!(
            replay.credential, advertise.credential,
            "an acknowledgement-loss retry reproduces the IDENTICAL bearer"
        );

        assert_eq!(
            store.mint_phase_credential(claim, CiCredentialPurpose::CheckoutFetch).await.unwrap_err(),
            CiCredentialGenerationError::MissingParentAttempt
        );
        admit_parent(&admin, &fixture).await;
        assert_eq!(
            store.mint_phase_credential(claim, CiCredentialPurpose::CheckoutFetch).await.unwrap_err(),
            CiCredentialGenerationError::JournalPredicateUnmet,
            "fetch requires checkout_transport = started"
        );
        assert_eq!(
            store.mint_phase_credential(claim, CiCredentialPurpose::CheckoutMaterialization).await.unwrap_err(),
            CiCredentialGenerationError::OutOfOrderGeneration
        );
        assert_eq!(
            store.mint_phase_credential(claim, CiCredentialPurpose::Workload).await.unwrap_err(),
            CiCredentialGenerationError::OutOfOrderGeneration
        );

        let advertise_gate = gate_for(claim, CiCredentialPurpose::CheckoutAdvertise, &advertise);
        assert!(
            !verify_phase_generation_live(&app, &advertise_gate).await.unwrap(),
            "advertise is unusable until its journal phase is started"
        );
        set_phase(&admin, &fixture, "checkout_transport", "started").await;
        assert!(
            verify_phase_generation_live(&app, &advertise_gate).await.unwrap(),
            "advertise is usable once the parent and journal phase exist"
        );

        let fetch = store
            .mint_phase_credential(claim, CiCredentialPurpose::CheckoutFetch)
            .await
            .expect("fetch mints once transport is started");
        assert_eq!(fetch.outcome, CiCredentialGenerationOutcome::Applied);
        assert_ne!(fetch.binding.generation_id, advertise.binding.generation_id);
        assert_ne!(fetch.credential.jti, advertise.credential.jti);

        assert!(
            !verify_phase_generation_live(&app, &advertise_gate).await.unwrap(),
            "the advertise generation is retired the moment fetch is appended"
        );
        assert!(verify_phase_generation_live(&app, &gate_for(claim, CiCredentialPurpose::CheckoutFetch, &fetch))
            .await
            .unwrap());
        assert_eq!(
            store.mint_phase_credential(claim, CiCredentialPurpose::CheckoutAdvertise).await.unwrap_err(),
            CiCredentialGenerationError::OutOfOrderGeneration
        );

        assert_eq!(
            store.mint_phase_credential(claim, CiCredentialPurpose::CheckoutMaterialization).await.unwrap_err(),
            CiCredentialGenerationError::JournalPredicateUnmet
        );
        set_phase(&admin, &fixture, "checkout_transport", "measured").await;
        assert_eq!(
            store.mint_phase_credential(claim, CiCredentialPurpose::CheckoutMaterialization).await.unwrap_err(),
            CiCredentialGenerationError::JournalPredicateUnmet,
            "materialization also requires its OWN journal row to be started"
        );
        set_phase(&admin, &fixture, "checkout_materialization", "started").await;
        let materialization = store
            .mint_phase_credential(claim, CiCredentialPurpose::CheckoutMaterialization)
            .await
            .expect("materialization mints");

        assert_eq!(
            store.mint_phase_credential(claim, CiCredentialPurpose::Workload).await.unwrap_err(),
            CiCredentialGenerationError::JournalPredicateUnmet
        );
        set_phase(&admin, &fixture, "checkout_materialization", "measured").await;
        let workload = store
            .mint_phase_credential(claim, CiCredentialPurpose::Workload)
            .await
            .expect("workload mints once both phases are measured");

        let rows = generation_rows(&admin, &fixture).await;
        assert_eq!(
            rows.iter().map(|row| (row.0.as_str(), row.1)).collect::<Vec<_>>(),
            vec![("checkout_advertise", 1), ("checkout_fetch", 2), ("checkout_materialization", 3), ("workload", 4),],
            "one claim holds AT MOST four credential generations"
        );
        let unique: std::collections::BTreeSet<&String> = rows.iter().map(|row| &row.2).collect();
        assert_eq!(unique.len(), 4, "every generation id is distinct");
        let unique_jtis: std::collections::BTreeSet<&String> = rows.iter().map(|row| &row.3).collect();
        assert_eq!(unique_jtis.len(), 4, "every expected JTI is distinct");

        for (purpose, minted) in [
            (CiCredentialPurpose::CheckoutAdvertise, &advertise),
            (CiCredentialPurpose::CheckoutFetch, &fetch),
            (CiCredentialPurpose::CheckoutMaterialization, &materialization),
        ] {
            assert!(
                !verify_phase_generation_live(&app, &gate_for(claim, purpose, minted)).await.unwrap(),
                "{purpose:?} is retired once the workload generation exists"
            );
        }

        let queue = ci_job_queue_store(app.clone());
        let launch_claim = CiJobLaunchClaim {
            tenant_id: claim.tenant_id.clone(),
            region: claim.region.clone(),
            wf_run_id: claim.wf_run_id.clone(),
            job_id: claim.job_id.clone(),
            lease_owner: claim.lease_owner.clone(),
            lease_epoch: claim.lease_epoch,
            claim_nonce: claim.claim_nonce.clone(),
            claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
            claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
        };
        let mut substituted = gate_for(claim, CiCredentialPurpose::Workload, &materialization);
        substituted.purpose = CiCredentialPurpose::Workload;
        assert!(
            !queue.authorize_launch_v2(&launch_claim, &substituted).await.unwrap(),
            "a materialization generation cannot drive the workload launch CAS"
        );
        let workload_gate = gate_for(claim, CiCredentialPurpose::Workload, &workload);
        assert!(queue.authorize_launch_v2(&launch_claim, &workload_gate).await.unwrap());
        let state: String = sqlx::query_scalar(
            "SELECT state FROM job_queue WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(&claim.job_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(state, "running");
        assert!(!queue.authorize_launch_v2(&launch_claim, &workload_gate).await.unwrap());

        assert!(calls.load(Ordering::SeqCst) >= 4, "each accepted mint invoked the REAL Identity seam");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_same_purpose_mints_produce_exactly_one_durable_row() {
    let (schema, bootstrap, admin, app) = migrated_schema("race").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let fixture = Arc::new(seed_fixture(&app, &admin, 2, 5).await);

        let a_tag = format!("myelin-cred-mint-a-{}", std::process::id());
        let b_tag = format!("myelin-cred-mint-b-{}", std::process::id());
        let a_pool = tagged_pool(&app_url(), &schema, &a_tag).await;
        let b_pool = tagged_pool(&app_url(), &schema, &b_tag).await;

        let s7 = RevocationStore::new();
        let cell = Arc::new(CellTokenAuthority::from_seed(&[31_u8; 32], &[32_u8; 32]).unwrap());
        let identity = RunTokenMinter::with_signer_and_tuples(
            s7,
            None,
            Arc::new(PasetoCapabilitySigner::new(cell)),
        );
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let blocking = Arc::new(BlockingPhaseMinter {
            inner: IdentityCiJobCredentialMinter::new(identity.clone()),
            entered: entered.clone(),
            release: release.clone(),
            already_blocked: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        let store_a = Arc::new(CiJobCredentialGenerationStore::with_pg_and_write_version(
            a_pool.clone(),
            REGION,
            blocking,
            CiJobCredentialWriteVersion::V2PhaseBound,
        ));
        let b_calls = Arc::new(AtomicUsize::new(0));
        let store_b = Arc::new(CiJobCredentialGenerationStore::with_pg_and_write_version(
            b_pool.clone(),
            REGION,
            Arc::new(CountingPhaseMinter {
                inner: IdentityCiJobCredentialMinter::new(identity),
                calls: b_calls.clone(),
            }),
            CiJobCredentialWriteVersion::V2PhaseBound,
        ));

        let a_fixture = fixture.clone();
        let a_store = store_a.clone();
        let racer_a = tokio::spawn(async move {
            a_store
                .mint_phase_credential(&a_fixture.claim, CiCredentialPurpose::CheckoutAdvertise)
                .await
        });
        entered.notified().await;

        let b_fixture = fixture.clone();
        let b_store = store_b.clone();
        let racer_b = tokio::spawn(async move {
            b_store
                .mint_phase_credential(&b_fixture.claim, CiCredentialPurpose::CheckoutAdvertise)
                .await
        });

        let observer = pinned_pool(&admin_url(), &schema).await;
        let waiting_pid = await_lock_waiter(&observer, &b_tag).await;
        let blocked_on_queue: bool = sqlx::query_scalar(
            "SELECT query ILIKE '%job_queue%' FROM pg_stat_activity WHERE pid = $1",
        )
        .bind(waiting_pid)
        .fetch_one(&observer)
        .await
        .unwrap();
        assert!(
            blocked_on_queue,
            "the second mint must block on the FIRST mint's job_queue row lock"
        );
        assert_eq!(
            generation_rows(&admin, &fixture).await.len(),
            0,
            "the first mint has not committed yet, so no row is visible"
        );

        release.notify_one();
        let minted = [
            racer_a.await.unwrap().expect("racer A succeeds"),
            racer_b.await.unwrap().expect("racer B succeeds"),
        ];

        let rows = generation_rows(&admin, &fixture).await;
        assert_eq!(
            rows.len(),
            1,
            "the per-job advisory lock plus the purpose-unique primary key admit exactly one row"
        );
        assert_eq!(
            minted[0].binding, minted[1].binding,
            "both racers observe the SAME durable generation"
        );
        assert_eq!(
            minted[0].credential, minted[1].credential,
            "both racers receive the identical bearer"
        );
        let applied = minted
            .iter()
            .filter(|m| m.outcome == CiCredentialGenerationOutcome::Applied)
            .count();
        assert_eq!(applied, 1, "exactly one racer inserted; the other replayed");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn retained_phase_ownership_blocks_a_concurrent_requeue_until_release() {
    let (schema, bootstrap, admin, app) = migrated_schema("ownership").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let (minter, _calls) = real_minter();
        let store = store(&app, minter, CiJobCredentialWriteVersion::V2PhaseBound);
        let fixture = seed_fixture(&app, &admin, 9, 5).await;
        admit_parent(&admin, &fixture).await;
        set_phase(&admin, &fixture, "checkout_transport", "started").await;
        let advertise = store
            .mint_phase_credential(&fixture.claim, CiCredentialPurpose::CheckoutAdvertise)
            .await
            .unwrap();
        let gate = gate_for(
            &fixture.claim,
            CiCredentialPurpose::CheckoutAdvertise,
            &advertise,
        );

        let mut owned = acquire_phase_generation_ownership(&app, &gate)
            .await
            .unwrap()
            .expect("the current generation grants ownership");

        let reaper_tag = format!("myelin-cred-reaper-{}", std::process::id());
        let reaper_pool = tagged_pool(&admin_url(), &schema, &reaper_tag).await;
        let job_id = fixture.claim.job_id.clone();
        let requeue = tokio::spawn(async move {
            sqlx::query(
                "UPDATE job_queue
                 SET state = 'queued', lease_owner = NULL, lease_expires = NULL,
                     claim_nonce = NULL
                 WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
            )
            .bind(TENANT)
            .bind(REGION)
            .bind(&job_id)
            .execute(&reaper_pool)
            .await
            .map(|done| done.rows_affected())
        });

        let observer = pinned_pool(&admin_url(), &schema).await;
        let waiting_pid = await_lock_waiter(&observer, &reaper_tag).await;
        let blocked_on_queue: bool = sqlx::query_scalar(
            "SELECT query ILIKE '%job_queue%' FROM pg_stat_activity WHERE pid = $1",
        )
        .bind(waiting_pid)
        .fetch_one(&observer)
        .await
        .unwrap();
        assert!(
            blocked_on_queue,
            "the requeue must block on the retained FOR SHARE row lock"
        );

        owned
            .validate()
            .await
            .expect("revalidation under the held lock succeeds");
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT state FROM job_queue
                 WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
            )
            .bind(TENANT)
            .bind(REGION)
            .bind(&fixture.claim.job_id)
            .fetch_one(&observer)
            .await
            .unwrap(),
            "leased",
            "the requeue cannot have landed while ownership is held"
        );

        owned.release().await.expect("ownership releases cleanly");
        assert_eq!(
            requeue.await.unwrap().expect("the requeue eventually runs"),
            1
        );

        assert!(
            acquire_phase_generation_ownership(&app, &gate)
                .await
                .unwrap()
                .is_none(),
            "after the requeue the generation is no longer current: nothing may spawn"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_requeue_that_wins_first_makes_phase_ownership_unacquirable() {
    let (schema, bootstrap, admin, app) = migrated_schema("ownership_lost").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let (minter, _calls) = real_minter();
        let store = store(&app, minter, CiJobCredentialWriteVersion::V2PhaseBound);
        let fixture = seed_fixture(&app, &admin, 10, 5).await;
        admit_parent(&admin, &fixture).await;
        set_phase(&admin, &fixture, "checkout_transport", "started").await;
        let advertise = store
            .mint_phase_credential(&fixture.claim, CiCredentialPurpose::CheckoutAdvertise)
            .await
            .unwrap();
        let gate = gate_for(
            &fixture.claim,
            CiCredentialPurpose::CheckoutAdvertise,
            &advertise,
        );
        sqlx::query(
            "UPDATE job_queue
             SET state = 'queued', lease_owner = NULL, lease_expires = NULL, claim_nonce = NULL
             WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(&fixture.claim.job_id)
        .execute(&admin)
        .await
        .unwrap();
        assert!(acquire_phase_generation_ownership(&app, &gate)
            .await
            .unwrap()
            .is_none());

        let mut workload_gate = gate.clone();
        workload_gate.purpose = CiCredentialPurpose::Workload;
        assert_eq!(
            acquire_phase_generation_ownership(&app, &workload_gate)
                .await
                .err(),
            Some(CiCredentialGenerationError::PurposeUnavailableForJobShape)
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_expired_generation_refuses_rather_than_rotating_and_v1_refuses_everything() {
    let (schema, bootstrap, admin, app) = migrated_schema("expiry").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let (minter, calls) = real_minter();
        let fixture = seed_fixture(&app, &admin, 3, 900).await;
        let claim = &fixture.claim;

        let v1 = store(
            &app,
            minter.clone(),
            CiJobCredentialWriteVersion::V1ClaimBound,
        );
        assert_eq!(
            v1.write_version(),
            CiJobCredentialWriteVersion::V1ClaimBound
        );
        for purpose in [
            CiCredentialPurpose::CheckoutAdvertise,
            CiCredentialPurpose::CheckoutFetch,
            CiCredentialPurpose::CheckoutMaterialization,
            CiCredentialPurpose::Workload,
        ] {
            assert_eq!(
                v1.mint_phase_credential(claim, purpose).await.unwrap_err(),
                CiCredentialGenerationError::WriteVersionPinned
            );
        }
        assert_eq!(generation_rows(&admin, &fixture).await.len(), 0);
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let now = chrono::Utc::now().timestamp();
        let issued = now - 400;
        let expires = now - 100;
        let (generation_id, jti) = expected_generation(
            claim,
            CiCredentialPurpose::CheckoutAdvertise,
            issued,
            expires,
        );
        sqlx::query(
            "INSERT INTO ci_job_credential_generation (
               tenant_id, region, job_id, wf_run_id, ci_run_id, token_authority_handle, idem_token,
               lease_owner, lease_epoch, claim_nonce, claim_started_at_epoch_secs,
               claim_expires_at_epoch_secs, binding_version, purpose, phase_ordinal,
               issued_at_epoch_secs, expires_at_epoch_secs, generation_id, jti
             ) VALUES ($1, $2, $3::uuid, $4::uuid, $5::uuid, $6, $7, $8, $9, $10::uuid, $11, $12,
                       1, 'checkout_advertise', 1, $13, $14, $15, $16)",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(&claim.job_id)
        .bind(&claim.wf_run_id)
        .bind(&claim.ci_run_id)
        .bind(&claim.token_authority_handle)
        .bind(&claim.idem_token)
        .bind(&claim.lease_owner)
        .bind(claim.lease_epoch)
        .bind(&claim.claim_nonce)
        .bind(claim.claim_started_at_epoch_secs)
        .bind(claim.claim_expires_at_epoch_secs)
        .bind(issued)
        .bind(expires)
        .bind(&generation_id)
        .bind(&jti)
        .execute(&admin)
        .await
        .unwrap();

        let v2 = store(&app, minter, CiJobCredentialWriteVersion::V2PhaseBound);
        assert_eq!(
            v2.mint_phase_credential(claim, CiCredentialPurpose::CheckoutAdvertise)
                .await
                .unwrap_err(),
            CiCredentialGenerationError::GenerationExpired,
            "a claim NEVER remints a phase; the parent attempt must requeue instead"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "an expired generation refuses BEFORE Identity is invoked"
        );
        assert_eq!(
            generation_rows(&admin, &fixture).await.len(),
            1,
            "no rotation row was appended"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_divergent_claim_fact_refuses_before_identity_is_invoked() {
    let (schema, bootstrap, admin, app) = migrated_schema("divergent").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let (minter, calls) = real_minter();
        let store = store(&app, minter, CiJobCredentialWriteVersion::V2PhaseBound);
        let fixture = seed_fixture(&app, &admin, 4, 5).await;

        type Mutation = (&'static str, fn(&mut CiJobTokenRequest));
        let mutations: [Mutation; 12] = [
            ("tenant", |c| c.tenant_id = "some-other-tenant".into()),
            ("region", |c| c.region = "eu-west".into()),
            ("job", |c| {
                c.job_id = "99000000-0000-4000-8000-000000000096".into()
            }),
            ("idem token", |c| {
                c.idem_token = "phase-credential-other".into()
            }),
            ("owner", |c| c.lease_owner = "runner-other".into()),
            ("epoch", |c| c.lease_epoch += 1),
            ("nonce", |c| {
                c.claim_nonce = "99000000-0000-4000-8000-000000000099".into()
            }),
            ("claim start", |c| c.claim_started_at_epoch_secs += 1),
            ("claim expiry", |c| c.claim_expires_at_epoch_secs += 1),
            ("workflow run", |c| {
                c.wf_run_id = "99000000-0000-4000-8000-000000000098".into()
            }),
            ("CI run", |c| {
                c.ci_run_id = "99000000-0000-4000-8000-000000000097".into()
            }),
            ("authority handle", |c| {
                c.token_authority_handle = format!("ci-token-authority:v2:{}", "0".repeat(64))
            }),
        ];
        for (label, mutate) in mutations {
            let mut divergent = fixture.claim.clone();
            mutate(&mut divergent);
            assert!(
                store
                    .mint_phase_credential(&divergent, CiCredentialPurpose::CheckoutAdvertise)
                    .await
                    .is_err(),
                "a divergent {label} must refuse"
            );
        }

        sqlx::query(
            "UPDATE job_queue SET lease_expires = statement_timestamp() - interval '1 second'
             WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(&fixture.claim.job_id)
        .execute(&admin)
        .await
        .unwrap();
        assert_eq!(
            store
                .mint_phase_credential(&fixture.claim, CiCredentialPurpose::CheckoutAdvertise)
                .await
                .unwrap_err(),
            CiCredentialGenerationError::ClaimUnavailable,
            "a lapsed execution lease refuses under the landed lease contract"
        );
        sqlx::query(
            "UPDATE job_queue SET lease_expires = statement_timestamp() + interval '900 seconds'
             WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(&fixture.claim.job_id)
        .execute(&admin)
        .await
        .unwrap();

        sqlx::query(
            "UPDATE ci_job SET state = 'cancelled'
             WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(&fixture.claim.job_id)
        .execute(&admin)
        .await
        .unwrap();
        assert_eq!(
            store
                .mint_phase_credential(&fixture.claim, CiCredentialPurpose::CheckoutAdvertise)
                .await
                .unwrap_err(),
            CiCredentialGenerationError::ClaimUnavailable
        );
        sqlx::query(
            "UPDATE ci_job SET state = 'queued'
             WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(&fixture.claim.job_id)
        .execute(&admin)
        .await
        .unwrap();

        sqlx::query(
            "UPDATE job_queue SET state = 'running'
             WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(&fixture.claim.job_id)
        .execute(&admin)
        .await
        .unwrap();
        assert_eq!(
            store
                .mint_phase_credential(&fixture.claim, CiCredentialPurpose::CheckoutAdvertise)
                .await
                .unwrap_err(),
            CiCredentialGenerationError::ClaimUnavailable
        );

        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "NOTHING above reached Identity: every refusal is durable-state-first"
        );
        assert_eq!(generation_rows(&admin, &fixture).await.len(), 0);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_requeue_retires_every_credential_and_refuses_every_further_mint() {
    let (schema, bootstrap, admin, app) = migrated_schema("requeue").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let (minter, _calls) = real_minter();
        let store = store(&app, minter, CiJobCredentialWriteVersion::V2PhaseBound);
        let fixture = seed_fixture(&app, &admin, 5, 5).await;
        admit_parent(&admin, &fixture).await;
        set_phase(&admin, &fixture, "checkout_transport", "started").await;

        let advertise = store
            .mint_phase_credential(&fixture.claim, CiCredentialPurpose::CheckoutAdvertise)
            .await
            .unwrap();
        let gate = gate_for(
            &fixture.claim,
            CiCredentialPurpose::CheckoutAdvertise,
            &advertise,
        );
        assert!(verify_phase_generation_live(&app, &gate).await.unwrap());

        sqlx::query(
            "UPDATE job_queue
             SET state = 'queued', lease_owner = NULL, lease_expires = NULL, claim_nonce = NULL
             WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(&fixture.claim.job_id)
        .execute(&admin)
        .await
        .unwrap();

        assert!(
            !verify_phase_generation_live(&app, &gate).await.unwrap(),
            "every credential of a requeued generation fails the queue-generation predicate"
        );
        assert!(
            store
                .mint_phase_credential(&fixture.claim, CiCredentialPurpose::CheckoutFetch)
                .await
                .is_err(),
            "a post-requeue mint refuses"
        );
        assert_eq!(generation_rows(&admin, &fixture).await.len(), 1);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_identity_success_with_a_rolled_back_transaction_leaves_an_unusable_orphan() {
    let (schema, bootstrap, admin, app) = migrated_schema("orphan").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let (minter, calls) = real_minter();
        let store = store(&app, minter, CiJobCredentialWriteVersion::V2PhaseBound);
        let fixture = seed_fixture(&app, &admin, 6, 5).await;

        admin
            .execute(
                "CREATE OR REPLACE FUNCTION myelin_test_fail_at_commit() RETURNS trigger
                 LANGUAGE plpgsql AS $$
                 BEGIN
                   RAISE EXCEPTION 'simulated post-Identity commit failure';
                 END $$;
                 CREATE CONSTRAINT TRIGGER myelin_test_credential_commit_failure
                 AFTER INSERT ON ci_job_credential_generation
                 DEFERRABLE INITIALLY DEFERRED
                 FOR EACH ROW EXECUTE FUNCTION myelin_test_fail_at_commit();",
            )
            .await
            .unwrap();

        let refused = store
            .mint_phase_credential(&fixture.claim, CiCredentialPurpose::CheckoutAdvertise)
            .await
            .expect_err("the commit fails");
        assert_eq!(refused, CiCredentialGenerationError::Database);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "Identity WAS invoked and succeeded before the rollback"
        );
        assert_eq!(
            generation_rows(&admin, &fixture).await.len(),
            0,
            "no generation row committed"
        );

        admit_parent(&admin, &fixture).await;
        set_phase(&admin, &fixture, "checkout_transport", "started").await;
        let now = chrono::Utc::now().timestamp();
        for anchor_offset in -2..=2_i64 {
            let issued = now + anchor_offset;
            let expires = (issued + 300).min(fixture.claim.claim_expires_at_epoch_secs);
            let (generation_id, jti) = expected_generation(
                &fixture.claim,
                CiCredentialPurpose::CheckoutAdvertise,
                issued,
                expires,
            );
            let gate = CiPhaseGenerationGate {
                tenant_id: TENANT.into(),
                region: REGION.into(),
                wf_run_id: fixture.claim.wf_run_id.clone(),
                ci_run_id: fixture.claim.ci_run_id.clone(),
                job_id: fixture.claim.job_id.clone(),
                token_authority_handle: fixture.claim.token_authority_handle.clone(),
                idem_token: fixture.claim.idem_token.clone(),
                checkout_commit: Some(COMMIT_OID.into()),
                lease_owner: fixture.claim.lease_owner.clone(),
                lease_epoch: fixture.claim.lease_epoch,
                claim_nonce: fixture.claim.claim_nonce.clone(),
                claim_started_at_epoch_secs: fixture.claim.claim_started_at_epoch_secs,
                claim_expires_at_epoch_secs: fixture.claim.claim_expires_at_epoch_secs,
                purpose: CiCredentialPurpose::CheckoutAdvertise,
                binding_version: CI_PHASE_CREDENTIAL_BINDING_V1,
                generation_id,
                jti,
                issued_at_epoch_secs: issued,
                expires_at_epoch_secs: expires,
            };
            assert!(
                !verify_phase_generation_live(&app, &gate).await.unwrap(),
                "an orphaned generation (anchor offset {anchor_offset}) has no durable row"
            );
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_credential_log_is_tenant_isolated_and_structurally_immutable() {
    let (schema, bootstrap, admin, app) = migrated_schema("rls").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let (minter, _calls) = real_minter();
        let store = store(&app, minter, CiJobCredentialWriteVersion::V2PhaseBound);
        let fixture = seed_fixture(&app, &admin, 7, 5).await;
        store.mint_phase_credential(&fixture.claim, CiCredentialPurpose::CheckoutAdvertise).await.unwrap();

        for (tenant, expected) in [(TENANT, 1_i64), ("some-other-tenant", 0)] {
            let mut connection = app.acquire().await.unwrap();
            sqlx::query("SELECT set_config('myelin.tenant_id', $1, false), set_config('myelin.region', $2, false)")
                .bind(tenant)
                .bind(REGION)
                .execute(&mut *connection)
                .await
                .unwrap();
            let visible: i64 = sqlx::query_scalar("SELECT count(*) FROM ci_job_credential_generation")
                .fetch_one(&mut *connection)
                .await
                .unwrap();
            assert_eq!(visible, expected, "tenant {tenant} visibility");
        }

        let update = sqlx::query(
            "UPDATE ci_job_credential_generation SET jti = 'tampered'
             WHERE tenant_id = $1 AND region = $2",
        )
        .bind(TENANT)
        .bind(REGION)
        .execute(&admin)
        .await;
        assert_eq!(
            update
                .expect_err("the immutability trigger refuses an UPDATE")
                .as_database_error()
                .and_then(|error| error.code())
                .as_deref(),
            Some("P0001")
        );
        let delete = sqlx::query("DELETE FROM ci_job_credential_generation WHERE tenant_id = $1 AND region = $2")
            .bind(TENANT)
            .bind(REGION)
            .execute(&admin)
            .await;
        assert_eq!(
            delete
                .expect_err("the immutability trigger refuses a DELETE")
                .as_database_error()
                .and_then(|error| error.code())
                .as_deref(),
            Some("P0001")
        );

        let forged = sqlx::query(
            "INSERT INTO ci_job_credential_generation (
               tenant_id, region, job_id, wf_run_id, ci_run_id, token_authority_handle, idem_token,
               lease_owner, lease_epoch, claim_nonce, claim_started_at_epoch_secs,
               claim_expires_at_epoch_secs, binding_version, purpose, phase_ordinal,
               issued_at_epoch_secs, expires_at_epoch_secs, generation_id, jti
             ) VALUES ($1, $2, $3::uuid, $4::uuid, $5::uuid, 'h', 'i', 'o', 1,
                       $6::uuid, $7, $8, 1, 'checkout_fetch', 1, $7, $8, 'g', 'j')",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(&fixture.claim.job_id)
        .bind(&fixture.claim.wf_run_id)
        .bind(&fixture.claim.ci_run_id)
        .bind("99000000-0000-4000-8000-0000000000ff")
        .bind(fixture.claim.claim_started_at_epoch_secs)
        .bind(fixture.claim.claim_expires_at_epoch_secs)
        .execute(&admin)
        .await;
        assert_eq!(
            forged
                .expect_err("a purpose at the wrong ordinal is refused by the schema")
                .as_database_error()
                .and_then(|error| error.code())
                .as_deref(),
            Some("23514")
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sealed_ceiling_phase_satisfies_no_purpose_and_retires_its_own_credential() {
    let (schema, bootstrap, admin, app) = migrated_schema("sealed").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let (minter, _calls) = real_minter();
        let store = store(&app, minter, CiJobCredentialWriteVersion::V2PhaseBound);
        let fixture = seed_fixture(&app, &admin, 8, 5).await;
        admit_parent(&admin, &fixture).await;
        set_phase(&admin, &fixture, "checkout_transport", "started").await;

        let advertise = store
            .mint_phase_credential(&fixture.claim, CiCredentialPurpose::CheckoutAdvertise)
            .await
            .unwrap();
        let gate = gate_for(
            &fixture.claim,
            CiCredentialPurpose::CheckoutAdvertise,
            &advertise,
        );
        assert!(verify_phase_generation_live(&app, &gate).await.unwrap());

        sqlx::query(
            "UPDATE ci_job_prelaunch_usage
             SET status = 'sealed_ceiling', resolved_at = statement_timestamp()
             WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
               AND lease_epoch = $4 AND claim_nonce = $5::uuid AND phase = 'checkout_transport'",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(&fixture.claim.job_id)
        .bind(fixture.claim.lease_epoch)
        .bind(&fixture.claim.claim_nonce)
        .execute(&admin)
        .await
        .unwrap();

        assert!(
            !verify_phase_generation_live(&app, &gate).await.unwrap(),
            "the advertise credential's own gate requires a STARTED transport phase"
        );
        for purpose in [
            CiCredentialPurpose::CheckoutFetch,
            CiCredentialPurpose::CheckoutMaterialization,
            CiCredentialPurpose::Workload,
        ] {
            assert!(
                matches!(
                    store
                        .mint_phase_credential(&fixture.claim, purpose)
                        .await
                        .unwrap_err(),
                    CiCredentialGenerationError::JournalPredicateUnmet
                        | CiCredentialGenerationError::OutOfOrderGeneration
                ),
                "a sealed transport phase must not satisfy {purpose:?}"
            );
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_v2_launch_cas_refuses_an_expired_execution_lease() {
    let (schema, bootstrap, admin, app) = migrated_schema("expired_lease").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let (minter, _calls) = real_minter();
        let store = store(&app, minter, CiJobCredentialWriteVersion::V2PhaseBound);
        let fixture = seed_fixture(&app, &admin, 11, 5).await;
        let workload = mint_full_sequence(&store, &admin, &fixture).await;
        let gate = gate_for(&fixture.claim, CiCredentialPurpose::Workload, &workload);
        let launch_claim = launch_claim_of(&fixture.claim);
        let queue = ci_job_queue_store(app.clone());

        sqlx::query(
            "UPDATE job_queue SET lease_expires = statement_timestamp() - interval '1 second'
             WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(&fixture.claim.job_id)
        .execute(&admin)
        .await
        .unwrap();
        let claim_still_open: bool = sqlx::query_scalar(
            "SELECT claim_expires_at > statement_timestamp() FROM job_queue
             WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(&fixture.claim.job_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(
            claim_still_open,
            "the claim window must still be open, so ONLY the lease predicate can refuse"
        );

        assert!(
            !queue
                .authorize_launch_v2(&launch_claim, &gate)
                .await
                .unwrap(),
            "the V2 CAS must refuse a lapsed execution lease"
        );

        let after: (String, String, bool) = sqlx::query_as(
            "SELECT q.state,
                    (SELECT s.state FROM ci_job s
                      WHERE s.tenant_id = q.tenant_id AND s.region = q.region
                        AND s.job_id = q.job_id),
                    q.lease_expires > statement_timestamp()
             FROM job_queue q
             WHERE q.tenant_id = $1 AND q.region = $2 AND q.job_id = $3::uuid",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(&fixture.claim.job_id)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(
            after,
            ("leased".to_string(), "queued".to_string(), false),
            "queue state, surface state, and the lapsed lease are ALL unchanged"
        );

        sqlx::query(
            "UPDATE job_queue SET lease_expires = statement_timestamp() + interval '600 seconds'
             WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(&fixture.claim.job_id)
        .execute(&admin)
        .await
        .unwrap();
        assert!(
            queue
                .authorize_launch_v2(&launch_claim, &gate)
                .await
                .unwrap(),
            "with a live lease the identical workload generation launches"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn aborting_or_dropping_acquisition_leaks_no_transaction_scope_or_lock() {
    let (schema, bootstrap, admin, app) = migrated_schema("cancel_safe").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let (minter, _calls) = real_minter();
        let one_pool = tagged_pool_capped(&app_url(), &schema, "myelin-cred-cancel", 1).await;
        let store = store(&app, minter, CiJobCredentialWriteVersion::V2PhaseBound);
        let fixture = seed_fixture(&app, &admin, 12, 5).await;
        admit_parent(&admin, &fixture).await;
        set_phase(&admin, &fixture, "checkout_transport", "started").await;
        let advertise = store
            .mint_phase_credential(&fixture.claim, CiCredentialPurpose::CheckoutAdvertise)
            .await
            .unwrap();
        let gate = gate_for(
            &fixture.claim,
            CiCredentialPurpose::CheckoutAdvertise,
            &advertise,
        );

        let mut blocker = admin.begin().await.unwrap();
        sqlx::query(
            "SELECT 1 FROM job_queue WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
             FOR UPDATE",
        )
        .bind(TENANT)
        .bind(REGION)
        .bind(&fixture.claim.job_id)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();

        let aborted = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            acquire_phase_generation_ownership(&one_pool, &gate),
        )
        .await;
        assert!(
            aborted.is_err(),
            "the acquisition must still be blocked mid-transaction when the timeout fires"
        );
        blocker.rollback().await.unwrap();

        let scope: String = sqlx::query_scalar("SELECT current_setting('myelin.tenant_id', true)")
            .fetch_one(&one_pool)
            .await
            .unwrap();
        assert_eq!(
            scope, "",
            "a cancelled acquisition left no tenant scope on the connection"
        );
        let usable: i32 = sqlx::query_scalar("SELECT 1")
            .fetch_one(&one_pool)
            .await
            .unwrap();
        assert_eq!(usable, 1);
        assert!(
            queue_row_lockable(&admin, &fixture).await,
            "a cancelled acquisition left no lock on the queue row"
        );

        let owned = acquire_phase_generation_ownership(&one_pool, &gate)
            .await
            .unwrap()
            .expect("the current generation grants ownership");
        assert!(
            !queue_row_lockable(&admin, &fixture).await,
            "while ownership is held the queue row is genuinely locked"
        );
        drop(owned);
        let scope: String = sqlx::query_scalar("SELECT current_setting('myelin.tenant_id', true)")
            .fetch_one(&one_pool)
            .await
            .unwrap();
        assert_eq!(scope, "", "dropping the handle left no tenant scope");
        assert!(
            queue_row_lockable(&admin, &fixture).await,
            "dropping the handle without release() still released the row lock (RAII rollback)"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn retained_ownership_freezes_the_journal_status_against_the_production_sealer() {
    let (schema, bootstrap, admin, app) = migrated_schema("sealer_race").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let (minter, _calls) = real_minter();
        let store = store(&app, minter, CiJobCredentialWriteVersion::V2PhaseBound);
        let fixture = seed_fixture(&app, &admin, 13, 5).await;
        admit_parent(&admin, &fixture).await;
        insert_overdue_started_transport(&admin, &fixture).await;
        let advertise = store
            .mint_phase_credential(&fixture.claim, CiCredentialPurpose::CheckoutAdvertise)
            .await
            .expect("advertise mints while transport is started");
        let gate = gate_for(
            &fixture.claim,
            CiCredentialPurpose::CheckoutAdvertise,
            &advertise,
        );

        let sealer = ci_region_queue_store_test_support(admin.clone());

        let mut owned = acquire_phase_generation_ownership(&app, &gate)
            .await
            .unwrap()
            .expect("the current generation grants ownership");

        assert_eq!(
            sealer.seal_expired_prelaunch_usage(REGION).await.unwrap(),
            0,
            "the sealer's FOR UPDATE SKIP LOCKED skips the row held FOR SHARE by retained ownership"
        );
        assert_eq!(
            phase_status(&admin, &fixture, "checkout_transport")
                .await
                .as_deref(),
            Some("started"),
            "the phase cannot seal while ownership is retained"
        );
        owned
            .validate()
            .await
            .expect("revalidation holds under the retained journal lock");

        let waiter_tag = format!("myelin-cred-journal-waiter-{}", std::process::id());
        let waiter_pool = tagged_pool(&app_url(), &schema, &waiter_tag).await;
        let job_id = fixture.claim.job_id.clone();
        let lease_epoch = fixture.claim.lease_epoch;
        let claim_nonce = fixture.claim.claim_nonce.clone();
        let waiter = tokio::spawn(async move {
            let mut tx = waiter_pool.begin().await.unwrap();
            sqlx::query(
                "SELECT set_config('myelin.tenant_id', $1, true),
                        set_config('myelin.region', $2, true)",
            )
            .bind(TENANT)
            .bind(REGION)
            .execute(&mut *tx)
            .await
            .unwrap();
            sqlx::query(
                "SELECT 1 FROM ci_job_prelaunch_usage
                 WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
                   AND lease_epoch = $4 AND claim_nonce = $5::uuid
                   AND phase = 'checkout_transport'
                 FOR UPDATE",
            )
            .bind(TENANT)
            .bind(REGION)
            .bind(&job_id)
            .bind(lease_epoch)
            .bind(&claim_nonce)
            .execute(&mut *tx)
            .await
            .unwrap();
            tx.rollback().await.unwrap();
        });
        let observer = pinned_pool(&admin_url(), &schema).await;
        let waiting_pid = await_lock_waiter(&observer, &waiter_tag).await;
        let on_journal: bool = sqlx::query_scalar(
            "SELECT query ILIKE '%ci_job_prelaunch_usage%' FROM pg_stat_activity WHERE pid = $1",
        )
        .bind(waiting_pid)
        .fetch_one(&observer)
        .await
        .unwrap();
        assert!(
            on_journal,
            "the conflicting FOR UPDATE must Lock-wait on the journal row specifically"
        );

        owned.release().await.expect("ownership releases cleanly");
        waiter.await.unwrap();
        assert_eq!(
            sealer.seal_expired_prelaunch_usage(REGION).await.unwrap(),
            1,
            "after release the overdue phase seals on the next sweep"
        );
        assert_eq!(
            phase_status(&admin, &fixture, "checkout_transport")
                .await
                .as_deref(),
            Some("sealed_ceiling")
        );
    })
    .await;
}

fn adapter_composition(pool: &PgPool, minter: Arc<CountingPhaseMinter>) -> V2CheckoutComposition {
    V2CheckoutComposition::new(
        pool.clone(),
        REGION,
        minter,
        ci_job_queue_store(pool.clone()),
        tokio::runtime::Handle::current(),
    )
    .expect("compose the dormant V2 checkout authorities")
}

fn resolved_checkout_spec(comp: &V2CheckoutComposition, fixture: &Fixture) -> JobSpec {
    let scope = checkout_scope();
    let (minted, context) = comp
        .mint_initial_phase_credential(&fixture.claim, &fixture.reserve_handle, Some(&scope))
        .expect("the resolver mints the initial advertise credential");
    let mut spec = JobSpec::new(
        JobKind::Ci,
        ImageRef::pinned(format!("registry.example/ci@sha256:{}", "b".repeat(64))).unwrap(),
        vec!["true".into()],
        Vec::new(),
        Vec::new(),
        EgressPolicy::deny_all(),
        ResourceLimits {
            cpu_millis: 1_000,
            mem_bytes: 256 * 1024 * 1024,
            disk_bytes: 1024 * 1024 * 1024,
            tmpfs_bytes: 1024 * 1024 * 1024,
            pids_max: 128,
            timeout_secs: 120,
        },
        WorkspaceSpec {
            repo_ref: Some(REPO_REF.into()),
            commit: Some(COMMIT_OID.into()),
        },
        TrustTier::Trusted,
        minted.credential,
        MeterTarget {
            reserve_id: fixture.reserve_handle.clone(),
        },
        IdemToken(fixture.claim.idem_token.clone()),
    )
    .expect("resolve the checkout job spec");
    spec.run_token_authorization = Some(context);
    spec
}

async fn reservation_state(admin: &PgPool, reserve_handle: &str) -> String {
    sqlx::query_scalar::<_, String>(
        "SELECT state FROM cost_reservation WHERE tenant_id = $1 AND region = $2 AND run_id = $3",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(reserve_handle)
    .fetch_one(admin)
    .await
    .unwrap()
}

async fn parent_row_count(admin: &PgPool, fixture: &Fixture) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM ci_job_parent_attempt
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(&fixture.claim.job_id)
    .fetch_one(admin)
    .await
    .unwrap()
}

struct LeaseFacts {
    claim_started: i64,
    claim_expires: i64,
    claim_nonce: String,
    lease_owner: String,
    lease_epoch: i64,
    lease_expires: i64,
}

async fn lease_facts(admin: &PgPool, fixture: &Fixture) -> LeaseFacts {
    let row = sqlx::query(
        "SELECT FLOOR(EXTRACT(EPOCH FROM claim_started_at))::bigint AS cs,
                FLOOR(EXTRACT(EPOCH FROM claim_expires_at))::bigint AS ce,
                claim_nonce::text AS nonce, lease_owner, lease_epoch,
                FLOOR(EXTRACT(EPOCH FROM lease_expires))::bigint AS le
         FROM job_queue WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(&fixture.claim.job_id)
    .fetch_one(admin)
    .await
    .unwrap();
    LeaseFacts {
        claim_started: row.get("cs"),
        claim_expires: row.get("ce"),
        claim_nonce: row.get("nonce"),
        lease_owner: row.get("lease_owner"),
        lease_epoch: row.get("lease_epoch"),
        lease_expires: row.get("le"),
    }
}

fn binding_generation_of(context: &myelin_ci_sandbox::RunTokenAuthorizationContext) -> String {
    match context {
        myelin_ci_sandbox::RunTokenAuthorizationContext::CiJob(c) => c
            .credential_binding
            .as_ref()
            .expect("a V2 context carries a credential binding")
            .generation_id
            .clone(),
    }
}

async fn insert_prior_parent(admin: &PgPool, fixture: &Fixture, lease_epoch: i64, nonce: &str) {
    sqlx::query(
        "INSERT INTO ci_job_parent_attempt (
           tenant_id, region, job_id, wf_run_id, ci_run_id, reserve_handle, lease_owner,
           lease_epoch, claim_nonce, claim_started_at_epoch_secs, claim_expires_at_epoch_secs,
           budget_revision, max_parent_attempts
         ) VALUES ($1, $2, $3::uuid, $4::uuid, $5::uuid, $6, $7, $8, $9::uuid, $10, $11, 1, 5)",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(&fixture.claim.job_id)
    .bind(&fixture.claim.wf_run_id)
    .bind(&fixture.claim.ci_run_id)
    .bind(&fixture.reserve_handle)
    .bind(format!("worker-{lease_epoch}"))
    .bind(lease_epoch)
    .bind(nonce)
    .bind(fixture.claim.claim_started_at_epoch_secs)
    .bind(fixture.claim.claim_expires_at_epoch_secs)
    .execute(admin)
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_checkout_adapter_drives_the_full_phase_sequence() {
    let (schema, bootstrap, admin, app) = migrated_schema("adapter_full").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let (minter, calls) = real_minter();
        let comp = adapter_composition(&app, minter);
        let fixture = seed_fixture(&app, &admin, 1, 5).await;

        let spec = resolved_checkout_spec(&comp, &fixture);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the resolver seam mints advertise exactly once"
        );
        let before = lease_facts(&admin, &fixture).await;

        let comp_t = comp.clone();
        let spec_t = spec.clone();
        let gens = std::thread::spawn(move || {
            let hook = comp_t.parent_attempt_reserve_hook();
            let authority = match hook(&spec_t).expect("admission succeeds") {
                ParentAttemptAdmission::Admitted {
                    attempt_authority, ..
                } => attempt_authority,
                ParentAttemptAdmission::AttemptsExhausted { .. } => {
                    panic!("a fresh generation must be admitted, not exhausted")
                }
            };
            let usage = ResourceUsage {
                cpu_seconds: 3,
                mem_byte_seconds: 7,
            };
            authority
                .begin_phase(PreparationPhase::CheckoutTransport)
                .expect("begin transport");
            let advertise = authority
                .mint_phase_credential(CheckoutPhase::Advertise)
                .expect("advertise replays under the same generation");
            let fetch = authority
                .mint_phase_credential(CheckoutPhase::Fetch)
                .expect("fetch mints");
            authority
                .complete_phase(PreparationPhase::CheckoutTransport, usage)
                .expect("complete transport");
            authority
                .renew_preparation_lease()
                .expect("renew after transport");
            authority
                .begin_phase(PreparationPhase::CheckoutMaterialization)
                .expect("begin materialization");
            let materialization = authority
                .mint_phase_credential(CheckoutPhase::Materialization)
                .expect("materialization mints");
            authority
                .complete_phase(PreparationPhase::CheckoutMaterialization, usage)
                .expect("complete materialization");
            authority
                .renew_preparation_lease()
                .expect("renew before workload");
            let workload = authority
                .mint_workload_credential()
                .expect("workload mints");
            (
                advertise.generation_id().to_string(),
                fetch.generation_id().to_string(),
                materialization.generation_id().to_string(),
                workload.generation_id().to_string(),
                binding_generation_of(advertise.authorization_context()),
            )
        })
        .join()
        .expect("the runner thread drove the sequence");

        assert_eq!(
            reservation_state(&admin, &fixture.reserve_handle).await,
            "inflight",
            "admission drove reserved -> inflight"
        );
        assert_eq!(parent_row_count(&admin, &fixture).await, 1);

        assert_eq!(
            phase_status(&admin, &fixture, "checkout_transport")
                .await
                .as_deref(),
            Some("measured")
        );
        assert_eq!(
            phase_status(&admin, &fixture, "checkout_materialization")
                .await
                .as_deref(),
            Some("measured")
        );

        let rows = generation_rows(&admin, &fixture).await;
        let purposes: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(
            purposes,
            [
                "checkout_advertise",
                "checkout_fetch",
                "checkout_materialization",
                "workload"
            ]
        );
        let (adv, fet, mat, wl, adv_ctx_gen) = &gens;
        assert_eq!(
            adv, &rows[0].2,
            "advertise carrier names the durable advertise generation"
        );
        assert_eq!(
            fet, &rows[1].2,
            "fetch carrier names the durable fetch generation"
        );
        assert_eq!(
            mat, &rows[2].2,
            "materialization carrier names its generation"
        );
        assert_eq!(wl, &rows[3].2, "workload carrier names its generation");
        assert_eq!(
            adv_ctx_gen, &rows[0].2,
            "the carrier's authorization context binds the same generation"
        );

        assert_eq!(calls.load(Ordering::SeqCst), 5);

        let after = lease_facts(&admin, &fixture).await;
        assert_eq!(after.claim_started, before.claim_started);
        assert_eq!(after.claim_expires, before.claim_expires);
        assert_eq!(after.claim_nonce, before.claim_nonce);
        assert_eq!(after.lease_owner, before.lease_owner);
        assert_eq!(after.lease_epoch, before.lease_epoch);
        assert_ne!(
            after.lease_expires, before.lease_expires,
            "renew_preparation_lease moved lease_expires and nothing else"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn adapter_admission_loses_cleanly_to_claim_generation_change() {
    let (schema, bootstrap, admin, app) = migrated_schema("adapter_race").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        {
            let (minter, _calls) = real_minter();
            let comp = adapter_composition(&app, minter);
            let fixture = seed_fixture(&app, &admin, 10, 5).await;
            let spec = resolved_checkout_spec(&comp, &fixture);
            let comp_t = comp.clone();
            let spec_t = spec.clone();
            let admitted = std::thread::spawn(move || {
                matches!(comp_t.parent_attempt_reserve_hook()(&spec_t), Ok(ParentAttemptAdmission::Admitted { .. }))
            })
            .join()
            .unwrap();
            assert!(admitted, "the control admission succeeds");
            assert_eq!(reservation_state(&admin, &fixture.reserve_handle).await, "inflight");
            assert_eq!(parent_row_count(&admin, &fixture).await, 1);
        }

        let tag = format!("myelin-adapter-admit-{}", std::process::id());
        let admit_pool = tagged_pool(&app_url(), &schema, &tag).await;
        let (minter, _calls) = real_minter();
        let comp = adapter_composition(&admit_pool, minter);
        let fixture = seed_fixture(&app, &admin, 11, 5).await;
        let spec = resolved_checkout_spec(&comp, &fixture);

        let holder = pinned_pool(&admin_url(), &schema).await;
        let mut held = holder.begin().await.unwrap();
        sqlx::query("SELECT 1 FROM job_queue WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid FOR UPDATE")
            .bind(TENANT)
            .bind(REGION)
            .bind(&fixture.claim.job_id)
            .execute(&mut *held)
            .await
            .unwrap();

        let comp_t = comp.clone();
        let spec_t = spec.clone();
        let admission = std::thread::spawn(move || comp_t.parent_attempt_reserve_hook()(&spec_t));

        let observer = pinned_pool(&admin_url(), &schema).await;
        await_lock_waiter(&observer, &tag).await;

        sqlx::query("UPDATE job_queue SET lease_epoch = 2 WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid")
            .bind(TENANT)
            .bind(REGION)
            .bind(&fixture.claim.job_id)
            .execute(&mut *held)
            .await
            .unwrap();
        held.commit().await.unwrap();

        let result = admission.join().unwrap();
        assert!(result.is_err(), "admission must refuse a claim whose generation changed under the lock");
        assert_eq!(
            reservation_state(&admin, &fixture.reserve_handle).await,
            "reserved",
            "a lost admission performs NO reservation transition"
        );
        assert_eq!(parent_row_count(&admin, &fixture).await, 0, "a lost admission inserts NO parent row");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adapter_exhaustion_retry_and_stale_authority_matrix() {
    let (schema, bootstrap, admin, app) = migrated_schema("adapter_matrix").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        {
            let (minter, _calls) = real_minter();
            let comp = adapter_composition(&app, minter);
            let fixture = seed_fixture(&app, &admin, 20, 5).await;
            insert_prior_parent(&admin, &fixture, 90, &uuid(0x61, 20)).await;
            insert_prior_parent(&admin, &fixture, 91, &uuid(0x62, 20)).await;
            insert_prior_parent(&admin, &fixture, 92, &uuid(0x63, 20)).await;
            let spec = resolved_checkout_spec(&comp, &fixture);
            let comp_t = comp.clone();
            let spec_t = spec.clone();
            let requeue =
                std::thread::spawn(move || match comp_t.parent_attempt_reserve_hook()(&spec_t).expect("admitted") {
                    ParentAttemptAdmission::Admitted { attempt_authority, .. } => attempt_authority.should_requeue(),
                    ParentAttemptAdmission::AttemptsExhausted { .. } => panic!("not exhausted yet"),
                })
                .join()
                .unwrap();
            assert_eq!(parent_row_count(&admin, &fixture).await, 4, "three priors + admitted current");
            assert!(requeue, "count(4) < max(5) still permits another attempt");
        }

        {
            let (minter, _calls) = real_minter();
            let comp = adapter_composition(&app, minter);
            let fixture = seed_fixture(&app, &admin, 23, 5).await;
            insert_prior_parent(&admin, &fixture, 90, &uuid(0x61, 23)).await;
            insert_prior_parent(&admin, &fixture, 91, &uuid(0x62, 23)).await;
            insert_prior_parent(&admin, &fixture, 92, &uuid(0x63, 23)).await;
            insert_prior_parent(&admin, &fixture, 93, &uuid(0x64, 23)).await;
            let spec = resolved_checkout_spec(&comp, &fixture);
            let comp_t = comp.clone();
            let spec_t = spec.clone();
            let requeue =
                std::thread::spawn(move || match comp_t.parent_attempt_reserve_hook()(&spec_t).expect("admitted") {
                    ParentAttemptAdmission::Admitted { attempt_authority, .. } => attempt_authority.should_requeue(),
                    ParentAttemptAdmission::AttemptsExhausted { .. } => {
                        panic!("the fifth attempt is still admitted; exhaustion is the SIXTH")
                    }
                })
                .join()
                .unwrap();
            assert_eq!(parent_row_count(&admin, &fixture).await, 5, "four priors + admitted current");
            assert!(!requeue, "count(5) == max(5) permits NO further attempt");
        }

        {
            let (minter, _calls) = real_minter();
            let comp = adapter_composition(&app, minter);
            let fixture = seed_fixture(&app, &admin, 21, 5).await;
            insert_prior_parent(&admin, &fixture, 90, &uuid(0x61, 21)).await;
            insert_prior_parent(&admin, &fixture, 91, &uuid(0x62, 21)).await;
            insert_prior_parent(&admin, &fixture, 92, &uuid(0x63, 21)).await;
            insert_prior_parent(&admin, &fixture, 93, &uuid(0x64, 21)).await;
            insert_prior_parent(&admin, &fixture, 94, &uuid(0x65, 21)).await;
            let spec = resolved_checkout_spec(&comp, &fixture);
            let comp_t = comp.clone();
            let spec_t = spec.clone();
            let reserve = std::thread::spawn(move || {
                match comp_t.parent_attempt_reserve_hook()(&spec_t).expect("typed admission") {
                    ParentAttemptAdmission::AttemptsExhausted { reserve, .. } => reserve.0,
                    ParentAttemptAdmission::Admitted { .. } => panic!("must be exhausted at max"),
                }
            })
            .join()
            .unwrap();
            assert_eq!(reserve, fixture.reserve_handle, "the exhausted admission surfaces the settleable reserve");
            assert_eq!(
                reservation_state(&admin, &fixture.reserve_handle).await,
                "inflight",
                "exhaustion still commits reserved -> inflight so the terminal report can settle"
            );
            assert_eq!(
                parent_row_count(&admin, &fixture).await,
                5,
                "no parent row was created for the exhausted generation"
            );
        }

        {
            let (minter, calls) = real_minter();
            let comp = adapter_composition(&app, minter);
            let fixture = seed_fixture(&app, &admin, 22, 5).await;
            let spec = resolved_checkout_spec(&comp, &fixture);
            let after_resolve = calls.load(Ordering::SeqCst);

            let comp_t = comp.clone();
            let spec_t = spec.clone();
            let authority =
                std::thread::spawn(move || match comp_t.parent_attempt_reserve_hook()(&spec_t).expect("admitted") {
                    ParentAttemptAdmission::Admitted { attempt_authority, .. } => attempt_authority,
                    ParentAttemptAdmission::AttemptsExhausted { .. } => panic!("admitted"),
                })
                .join()
                .unwrap();

            sqlx::query(
                "UPDATE job_queue SET lease_epoch = 2 WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
            )
            .bind(TENANT)
            .bind(REGION)
            .bind(&fixture.claim.job_id)
            .execute(&admin)
            .await
            .unwrap();
            let gens_before = generation_rows(&admin, &fixture).await.len();

            let (renew_err, begin_err, mint_err) = std::thread::spawn(move || {
                let renew = authority.renew_preparation_lease().is_err();
                let begin = authority.begin_phase(PreparationPhase::CheckoutTransport).is_err();
                let mint = authority.mint_phase_credential(CheckoutPhase::Fetch).is_err();
                (renew, begin, mint)
            })
            .join()
            .unwrap();
            assert!(renew_err, "a stale lease renewal refuses");
            assert!(begin_err, "a stale begin_phase refuses");
            assert!(mint_err, "a stale credential mint refuses");
            assert_eq!(calls.load(Ordering::SeqCst), after_resolve, "a refused mint never reaches Identity");
            assert_eq!(
                generation_rows(&admin, &fixture).await.len(),
                gens_before,
                "a refused stale mint creates no generation row"
            );
            assert_eq!(
                phase_status(&admin, &fixture, "checkout_transport").await,
                None,
                "a refused stale begin_phase creates no journal row"
            );
        }
    })
    .await;
}

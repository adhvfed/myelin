//! **Live-PostgreSQL proof for CT-007's phase-credential generations.**
//!
//! Everything here runs against the REAL migration set, the REAL production SQL constants, and a
//! REAL Identity (PASETO + S7) minter — never a stub that merely returns a string.
//!
//! The properties proven:
//! 1. exact retry returns ONE durable row and an identical generation id, JTI, anchor, expiry, TTL,
//!    and bearer;
//! 2. concurrent same-purpose mints (a genuine two-connection barrier drill) produce exactly one row;
//! 3. an expired same-purpose generation REFUSES rather than rotating;
//! 4. every divergent claim fact refuses BEFORE Identity is ever invoked;
//! 5. out-of-order purposes, the full journal-status matrix (including `sealed_ceiling`), a missing
//!    parent attempt, a lapsed execution lease, an expired claim, and a non-`leased` state all refuse;
//! 6. appending a successor instantly retires its predecessor at the durable execution gate
//!    (stale-generation replay after supersession), and a requeue retires everything;
//! 7. an Identity-success/DB-rollback orphan cannot pass the durable phase gate;
//! 8. the V1 production pin refuses every phase mint, and V2 is an explicit opt-in;
//! 9. the workload V2 launch CAS requires the current `workload` generation;
//! 10. RLS isolates the credential log across tenants.
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

/// Independent `PgMigrator` sequences against the same live PostgreSQL deadlock on the migration
/// advisory lock when run concurrently — the same guard `integration_ci_lease_topology` uses.
static MIGRATION_SCENARIO_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const TENANT: &str = "phase-credential";
const REGION: &str = "fr-par";
const REPO_REF: &str = "myelin://phase-credential/git/repo/core";
const COMMIT_OID: &str = "deadbeef00deadbeef00deadbeef00deadbeef00";
/// timeout 120s, checkout-bearing → `4 * (120 + 600)`.
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

/// A pool whose connections carry a unique `application_name`, so `pg_stat_activity` can identify
/// exactly WHICH backend is lock-waiting rather than "some backend touching job_queue". Mirrors the
/// definition-cutover drills' convention.
async fn tagged_pool(url: &str, schema: &str, application_name: &str) -> PgPool {
    tagged_pool_capped(url, schema, application_name, 4).await
}

/// Like [`tagged_pool`] but with an explicit connection cap (a ONE-connection pool is how the
/// cancellation drill proves a leaked transaction would reach the next borrower).
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

/// Poll until a backend carrying EXACTLY `application_name` is blocked on a lock. Returns its pid.
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

// =================================================================================================
// The Identity seam under test, wrapped so a test can prove a refusal happened BEFORE any mint.
// =================================================================================================

/// A REAL Identity minter (PASETO signer + S7 revocation store) that also counts invocations. Every
/// "refuses before Identity" assertion reads this counter — a refusal that merely produced an error
/// AFTER signing would still have created a live S7 token.
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

/// A real Identity minter that BLOCKS its first invocation. Because the store calls Identity while
/// still holding the `job_queue` row lock and the per-job advisory lock, blocking here parks a mint
/// transaction in exactly the state a concurrent racer must contend with.
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

// =================================================================================================
// Fixture.
// =================================================================================================

struct Fixture {
    claim: CiJobTokenRequest,
    reserve_handle: String,
}

/// One complete, live, checkout-bearing leased generation: reservation, `ci_run`, immutable
/// manifest, durable `ci_job_spec`, the public `ci_job` surface row, and a `job_queue` row leased
/// with a topology-sized claim window and a live execution lease.
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

/// Admit the exact durable parent attempt this claim's execution gates require.
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

/// Write one prelaunch journal phase row directly, so the journal-status matrix can be driven
/// exhaustively without running the real Hop A/B.
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

/// Insert a `started` `checkout_transport` journal row whose `seal_after` deadline is ALREADY
/// overdue, so the production topology-deadline sealer will pick it up on the next sweep. The
/// deadline column is immutable (a transition-guard trigger), so it must be seeded overdue at insert
/// time rather than updated afterwards.
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

/// The current status of one journal phase (or `None` if absent).
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

/// Try to lock the exact `job_queue` row `FOR UPDATE NOWAIT` from a fresh admin connection.
/// `Ok(true)` = the lock was free (nothing retained it); `Ok(false)` = it is currently held.
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

/// Drive the complete phase sequence to a live `workload` generation: parent admitted, both journal
/// phases measured, all four credentials minted in order.
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

/// Recompute the generation id/JTI a mint WOULD have produced for these exact inputs — used both to
/// pre-seed an expired row and to name the orphan generation an aborted mint would have created.
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

// =================================================================================================
// 1. Determinism, ordering, supersession, and the journal matrix.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_phase_sequence_is_ordered_replay_stable_and_bounded_to_four_generations() {
    let (schema, bootstrap, admin, app) = migrated_schema("sequence").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let (minter, calls) = real_minter();
        let store = store(&app, minter, CiJobCredentialWriteVersion::V2PhaseBound);
        let fixture = seed_fixture(&app, &admin, 1, 5).await;
        let claim = &fixture.claim;

        // ---- advertise: the resolver's first mint, BEFORE the parent attempt exists ----
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

        // ---- EXACT retry: one row, identical everything, identical bearer ----
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

        // ---- out of order: fetch has no parent/journal yet ----
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
        // Skipping a purpose is refused outright.
        assert_eq!(
            store.mint_phase_credential(claim, CiCredentialPurpose::CheckoutMaterialization).await.unwrap_err(),
            CiCredentialGenerationError::OutOfOrderGeneration
        );
        assert_eq!(
            store.mint_phase_credential(claim, CiCredentialPurpose::Workload).await.unwrap_err(),
            CiCredentialGenerationError::OutOfOrderGeneration
        );

        // ---- the advertise credential's execution gate: parent + started transport ----
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

        // ---- fetch ----
        let fetch = store
            .mint_phase_credential(claim, CiCredentialPurpose::CheckoutFetch)
            .await
            .expect("fetch mints once transport is started");
        assert_eq!(fetch.outcome, CiCredentialGenerationOutcome::Applied);
        assert_ne!(fetch.binding.generation_id, advertise.binding.generation_id);
        assert_ne!(fetch.credential.jti, advertise.credential.jti);

        // **Stale-generation replay after supersession.** Appending fetch retired advertise at the
        // durable gate, in the same commit — with no revocation write anywhere.
        assert!(
            !verify_phase_generation_live(&app, &advertise_gate).await.unwrap(),
            "the advertise generation is retired the moment fetch is appended"
        );
        assert!(verify_phase_generation_live(&app, &gate_for(claim, CiCredentialPurpose::CheckoutFetch, &fetch))
            .await
            .unwrap());
        // And a superseded purpose can never be minted again, even as a "retry".
        assert_eq!(
            store.mint_phase_credential(claim, CiCredentialPurpose::CheckoutAdvertise).await.unwrap_err(),
            CiCredentialGenerationError::OutOfOrderGeneration
        );

        // ---- materialization: transport measured, materialization started ----
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

        // ---- workload: both journal rows measured ----
        assert_eq!(
            store.mint_phase_credential(claim, CiCredentialPurpose::Workload).await.unwrap_err(),
            CiCredentialGenerationError::JournalPredicateUnmet
        );
        set_phase(&admin, &fixture, "checkout_materialization", "measured").await;
        let workload = store
            .mint_phase_credential(claim, CiCredentialPurpose::Workload)
            .await
            .expect("workload mints once both phases are measured");

        // ---- exactly four rows, one per purpose, in ordinal order ----
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

        // Every preparation credential is now retired at the durable gate; only workload is current.
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

        // ---- the workload V2 launch CAS requires exactly the current workload generation ----
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
        // A PREPARATION generation presented at the workload CAS matches nothing.
        let mut substituted = gate_for(claim, CiCredentialPurpose::Workload, &materialization);
        substituted.purpose = CiCredentialPurpose::Workload;
        assert!(
            !queue.authorize_launch_v2(&launch_claim, &substituted).await.unwrap(),
            "a materialization generation cannot drive the workload launch CAS"
        );
        // The exact current workload generation wins.
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
        // And it is one-shot: the row is no longer `leased`.
        assert!(!queue.authorize_launch_v2(&launch_claim, &workload_gate).await.unwrap());

        assert!(calls.load(Ordering::SeqCst) >= 4, "each accepted mint invoked the REAL Identity seam");
    })
    .await;
}

// =================================================================================================
// 2. Concurrency: a real two-connection barrier drill.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_same_purpose_mints_produce_exactly_one_durable_row() {
    let (schema, bootstrap, admin, app) = migrated_schema("race").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let fixture = Arc::new(seed_fixture(&app, &admin, 2, 5).await);

        // Round-1 major 4: the earlier drill barriered BEFORE either task acquired a connection, so
        // it could pass entirely sequentially. This one BLOCKS the first mint inside its Identity
        // call — at which point it is holding the `job_queue` row lock `FOR UPDATE` and the per-job
        // advisory lock — then starts the second mint on its own tagged connection and PROVES, via
        // `pg_stat_activity`, that the second backend is genuinely lock-waiting before releasing.
        let a_tag = format!("myelin-cred-mint-a-{}", std::process::id());
        let b_tag = format!("myelin-cred-mint-b-{}", std::process::id());
        let a_pool = tagged_pool(&app_url(), &schema, &a_tag).await;
        let b_pool = tagged_pool(&app_url(), &schema, &b_tag).await;

        // ONE Identity signer + S7 store shared by both racers: a determinism claim about the
        // returned bearer is only meaningful if both sides could actually have produced it.
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

        // A: enters the locked transaction and parks inside Identity.
        let a_fixture = fixture.clone();
        let a_store = store_a.clone();
        let racer_a = tokio::spawn(async move {
            a_store
                .mint_phase_credential(&a_fixture.claim, CiCredentialPurpose::CheckoutAdvertise)
                .await
        });
        entered.notified().await;

        // B: starts only once A is provably holding the locks.
        let b_fixture = fixture.clone();
        let b_store = store_b.clone();
        let racer_b = tokio::spawn(async move {
            b_store
                .mint_phase_credential(&b_fixture.claim, CiCredentialPurpose::CheckoutAdvertise)
                .await
        });

        // B's OWN backend must be blocked on a lock — not merely "slower".
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
        // Nothing is committed while A is parked.
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

/// **Round-1 blocker 1: the retained preparation gate really holds the row through the child-release
/// boundary.** While ownership is held, a concurrent requeue BLOCKS (proved via `pg_stat_activity`,
/// not by timing); revalidation under the held lock still succeeds; and once released, the requeue
/// lands and the generation is refused — so there is no window in which a child could be released
/// under a generation that has already been invalidated.
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

        // Take retained ownership — this is what a committed `LaunchPermit` holds while the child is
        // mechanically blocked at the launch gate.
        let mut owned = acquire_phase_generation_ownership(&app, &gate)
            .await
            .unwrap()
            .expect("the current generation grants ownership");

        // A concurrent reaper-style requeue on its own tagged backend.
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

        // The generation is still verifiably current WHILE the requeue waits — this is exactly the
        // guarantee a released child depends on.
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

        // Release (the post-gate release) — only now may the requeue proceed.
        owned.release().await.expect("ownership releases cleanly");
        assert_eq!(
            requeue.await.unwrap().expect("the requeue eventually runs"),
            1
        );

        // And afterwards the generation is refused: a NEW acquisition attempt spawns nothing.
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

/// The other direction of the same race: when the requeue wins FIRST, acquisition simply refuses —
/// there is no stale spawn, and the refusal costs no lock.
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

        // The workload purpose may never route through the preparation gate at all: that would
        // authorize a spawn without ever running the launch CAS.
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

// =================================================================================================
// 3. Expiry never rotates; the write-version pin; divergent claim facts.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_expired_generation_refuses_rather_than_rotating_and_v1_refuses_everything() {
    let (schema, bootstrap, admin, app) = migrated_schema("expiry").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let (minter, calls) = real_minter();
        let fixture = seed_fixture(&app, &admin, 3, 900).await;
        let claim = &fixture.claim;

        // ---- the production pin refuses before touching the database at all ----
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

        // ---- an already-expired generation refuses; it never rotates ----
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
        // Round-1 minor 6: the matrix now covers EVERY identity field the mint binds, including
        // tenant, region, job id, and idem token.
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

        // A lapsed EXECUTION lease refuses even though the claim window is still open.
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

        // A cancelled public surface refuses.
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

        // A `running` (already-launched) row refuses a PREPARATION mint outright.
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

// =================================================================================================
// 4. Requeue, the Identity-success/DB-rollback orphan, and RLS.
// =================================================================================================

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

        // The reaper's requeue: the exact generation is gone.
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
        // The old audit row survives — the log is append-only and never cleaned by the reaper.
        assert_eq!(generation_rows(&admin, &fixture).await.len(), 1);
    })
    .await;
}

/// **Identity succeeded, then the durable transaction rolled back.** A `DEFERRABLE INITIALLY
/// DEFERRED` constraint trigger raises at COMMIT — i.e. strictly AFTER the insert AND after the
/// Identity mint — reproducing the exact orphan the design names. The S7 token is genuinely live;
/// the point is that it has no committed generation row and therefore cannot pass any phase gate.
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

        // The orphan S7 token names a generation that does not exist durably. Even with the exact
        // parent and journal state in place, no phase gate can accept it.
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

        // FORCE-RLS: the owning tenant sees its row; another tenant sees nothing.
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

        // UPDATE/DELETE are revoked from the app role AND blocked by the immutability trigger.
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

        // The purpose-to-ordinal CHECK is real, not merely a Rust convention.
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

/// **The `sealed_ceiling` half of the journal-status matrix.** A phase the reaper sealed (rather
/// than the worker measuring) never satisfies any downstream purpose, and instantly retires the
/// credential whose own boundary required that phase to be `started`.
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

        // The reaper seals the abandoned phase at its ceiling.
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

/// **Dormancy — round-1 major 5: exact allowed occurrences across BOTH crates.**
///
/// The earlier version scanned only `myelin-ci-controlplane/src`, excluded the four defining modules
/// wholesale, and never looked at `myelin-ci-sandbox` at all — so an activation added inside an
/// excluded module or anywhere in the sandbox would have stayed green.
///
/// This version walks the PRODUCTION source of both crates (each file truncated at its top-level
/// test module) and pins the exact number of occurrences of every activation marker, per file. Any
/// new occurrence ANYWHERE — including inside a defining module — fails, so a future activation goes
/// red and has to be an explicit, reviewed edit to this table.
#[test]
fn the_v2_phase_credential_surface_has_exactly_its_known_occurrences() {
    /// Every token that would constitute reaching (or composing) the dormant V2 surface.
    const MARKERS: [&str; 22] = [
        // control plane
        "mint_phase_credential(",
        "authorize_workload_v2_retained(",
        "authorize_checkout_advertise_retained(",
        "authorize_checkout_fetch_retained(",
        "authorize_checkout_materialization_retained(",
        "authorize_launch_v2(",
        "authorize_launch_v2_retained(",
        "acquire_phase_generation_ownership(",
        "V2PhaseBound",
        "CiJobCredentialGenerationStore",
        // CT-007 5b.3-6d STEP 3: the dormant composition FAÇADE. Wiring ANY of these into a
        // production composition root activates the V2 checkout path WITHOUT touching the credential
        // markers above — so the dormancy guarantee requires the scan to cover them directly.
        "V2CheckoutComposition",
        "v2_phase_credential_store(",
        "mint_initial_phase_credential(",
        "parent_attempt_reserve_hook(",
        // CT-007 5b.3-6e.1: the activation-chassis SELECTORS. These are BARE symbols (no leading `.`
        // and no trailing `(`), so the scan catches BOTH method-call `x.with_checkout_config(y)` AND
        // UFCS `GvisorBackend::with_checkout_config(x, y)` forms (Sol's 6e.1 major 2). Selecting a
        // checkout config, the readiness predicate, or the production readiness probe from ANY
        // composition root is exactly what these trip on.
        "with_checkout_config",
        "with_activation_readiness",
        "ActivationReadinessProbe::production",
        // sandbox
        "authorize_checkout_phase(",
        "with_checkout_phase_authorization(",
        "fetch_checkout_pack_within_parent_attempt_v2(",
        "run_checkout_preparation_v2(",
        "PhaseAuthorization",
    ];

    /// `(crate, file, marker) -> exact allowed occurrence count` in PRODUCTION source. Everything
    /// absent from this table must have ZERO occurrences.
    /// `(crate, file, marker) -> exact allowed CODE occurrence count` in PRODUCTION source.
    /// Everything absent from this table must have ZERO occurrences in EITHER crate.
    const ALLOWED: [(&str, &str, &str, usize); 84] = [
        // --- the definition sites ---
        (
            "myelin-ci-controlplane",
            "ci_credential_generation.rs",
            "mint_phase_credential(",
            1,
        ),
        (
            "myelin-ci-controlplane",
            "ci_credential_generation.rs",
            "acquire_phase_generation_ownership(",
            1,
        ),
        (
            "myelin-ci-controlplane",
            "ci_credential_generation.rs",
            "V2PhaseBound",
            2,
        ),
        (
            "myelin-ci-controlplane",
            "ci_credential_generation.rs",
            "CiJobCredentialGenerationStore",
            2,
        ),
        (
            "myelin-ci-controlplane",
            "ci_identity_adapter.rs",
            "authorize_workload_v2_retained(",
            1,
        ),
        (
            "myelin-ci-controlplane",
            "ci_identity_adapter.rs",
            "authorize_checkout_advertise_retained(",
            1,
        ),
        (
            "myelin-ci-controlplane",
            "ci_identity_adapter.rs",
            "authorize_checkout_fetch_retained(",
            1,
        ),
        (
            "myelin-ci-controlplane",
            "ci_identity_adapter.rs",
            "authorize_checkout_materialization_retained(",
            1,
        ),
        (
            "myelin-ci-controlplane",
            "ci_identity_adapter.rs",
            "authorize_launch_v2_retained(",
            1,
        ),
        (
            "myelin-ci-controlplane",
            "ci_identity_adapter.rs",
            "acquire_phase_generation_ownership(",
            1,
        ),
        (
            "myelin-ci-controlplane",
            "job_queue_store.rs",
            "authorize_launch_v2(",
            1,
        ),
        (
            "myelin-ci-controlplane",
            "job_queue_store.rs",
            "authorize_launch_v2_retained(",
            2,
        ),
        (
            "myelin-ci-controlplane",
            "lib.rs",
            "CiJobCredentialGenerationStore",
            1,
        ),
        // CT-007 slice 5b.3-6e.2 Stage A: the protocol DESCRIPTOR records the production credential
        // writer choice `V2PhaseBound` ONCE, as a dormant `pub const`. No production root reads it
        // until the atomic Stage B activation (which also adds this file to the definition hash), so
        // this is a definition-only occurrence, NOT a composition — the composition-root zeros stay
        // zero.
        (
            "myelin-ci-controlplane",
            "ci_pipeline_protocol.rs",
            "V2PhaseBound",
            1,
        ),
        // CT-007 5b.3-6d STEP 3: the DORMANT control-plane composition module. It is the real
        // durable backing for the sandbox `AttemptAuthority`/resolver seam — a NEW definition site,
        // NOT a composition root (no production root constructs `V2CheckoutComposition`, and
        // production `RunnerHooks` never install its parent-attempt reserve hook), so the
        // composition-root zeros below stay zero. `V2PhaseBound` (the phase-store factory) x1,
        // `CiJobCredentialGenerationStore` (import + factory return + factory body + the two durable-
        // authority struct fields) x5, `mint_phase_credential(` (the initial resolver mint + the
        // per-generation mint + the `AttemptAuthority` trait method) x3.
        (
            "myelin-ci-controlplane",
            "ci_checkout_composition.rs",
            "V2PhaseBound",
            1,
        ),
        (
            "myelin-ci-controlplane",
            "ci_checkout_composition.rs",
            "CiJobCredentialGenerationStore",
            5,
        ),
        (
            "myelin-ci-controlplane",
            "ci_checkout_composition.rs",
            "mint_phase_credential(",
            3,
        ),
        // CT-007 5b.3-6d STEP 3: the composition-façade DEFINITION sites (all in the dormant module).
        // `V2CheckoutComposition` = the struct def + its `impl` block; `v2_phase_credential_store(` =
        // the factory def + its one call inside `V2CheckoutComposition::new`; the resolver seam and the
        // reserve-hook constructor are each defined once. These are the exact symbols whose appearance
        // in ANY composition root below means the V2 checkout path was activated.
        (
            "myelin-ci-controlplane",
            "ci_checkout_composition.rs",
            "V2CheckoutComposition",
            2,
        ),
        (
            "myelin-ci-controlplane",
            "ci_checkout_composition.rs",
            "v2_phase_credential_store(",
            2,
        ),
        (
            "myelin-ci-controlplane",
            "ci_checkout_composition.rs",
            "mint_initial_phase_credential(",
            1,
        ),
        (
            "myelin-ci-controlplane",
            "ci_checkout_composition.rs",
            "parent_attempt_reserve_hook(",
            1,
        ),
        // ...and EXPLICITLY ZERO in every production composition root: constructing the façade, minting
        // the initial credential, or installing the reserve hook from any of these turns the scan RED.
        (
            "myelin-ci-controlplane",
            "ci_runtime_composition.rs",
            "V2CheckoutComposition",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "ci_runtime_composition.rs",
            "v2_phase_credential_store(",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "ci_runtime_composition.rs",
            "mint_initial_phase_credential(",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "ci_runtime_composition.rs",
            "parent_attempt_reserve_hook(",
            0,
        ),
        // CT-007 slice 5b.3-6e.2 Stage A: `ci_runner_v2_wiring` is the DORMANT V2 runner composition
        // root. It CONSTRUCTS the façade (`V2CheckoutComposition` x2 = the resolver's arg + the hooks'
        // arg, one shared value), installs the ONE parent-attempt reserve hook (compute AND checkout),
        // the per-phase checkout authorization, and the workload/advertise/fetch/materialization
        // retained authorizers. It is a NEW definition site, NOT selected by any production root —
        // `main.rs` / `ci_runtime_composition.rs` / `lib.rs` stay ZERO (the atomic Stage B flip points
        // `main` here). The V2 resolver seam lives in `runner_bind.rs`: it names the composition type
        // (its one param) once and calls `mint_initial_phase_credential` once.
        (
            "myelin-ci-controlplane",
            "ci_runner_composition.rs",
            "V2CheckoutComposition",
            2,
        ),
        (
            "myelin-ci-controlplane",
            "ci_runner_composition.rs",
            "v2_phase_credential_store(",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "ci_runner_composition.rs",
            "mint_initial_phase_credential(",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "ci_runner_composition.rs",
            "parent_attempt_reserve_hook(",
            1,
        ),
        (
            "myelin-ci-controlplane",
            "ci_runner_composition.rs",
            "authorize_workload_v2_retained(",
            1,
        ),
        (
            "myelin-ci-controlplane",
            "ci_runner_composition.rs",
            "authorize_checkout_advertise_retained(",
            1,
        ),
        (
            "myelin-ci-controlplane",
            "ci_runner_composition.rs",
            "authorize_checkout_fetch_retained(",
            1,
        ),
        (
            "myelin-ci-controlplane",
            "ci_runner_composition.rs",
            "authorize_checkout_materialization_retained(",
            1,
        ),
        (
            "myelin-ci-controlplane",
            "runner_bind.rs",
            "V2CheckoutComposition",
            1,
        ),
        (
            "myelin-ci-controlplane",
            "runner_bind.rs",
            "v2_phase_credential_store(",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "runner_bind.rs",
            "mint_initial_phase_credential(",
            1,
        ),
        (
            "myelin-ci-controlplane",
            "runner_bind.rs",
            "parent_attempt_reserve_hook(",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "main.rs",
            "V2CheckoutComposition",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "main.rs",
            "v2_phase_credential_store(",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "main.rs",
            "mint_initial_phase_credential(",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "main.rs",
            "parent_attempt_reserve_hook(",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "lib.rs",
            "V2CheckoutComposition",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "lib.rs",
            "v2_phase_credential_store(",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "lib.rs",
            "mint_initial_phase_credential(",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "lib.rs",
            "parent_attempt_reserve_hook(",
            0,
        ),
        (
            "myelin-ci-sandbox",
            "checkout_authorization.rs",
            "authorize_checkout_phase(",
            1,
        ),
        (
            "myelin-ci-sandbox",
            "checkout_authorization.rs",
            "PhaseAuthorization",
            6,
        ),
        (
            "myelin-ci-sandbox",
            "gvisor.rs",
            "fetch_checkout_pack_within_parent_attempt_v2(",
            1,
        ),
        // CT-007 5b.3-6a (Sol's r4): the capsule + reshaped Hop B entry relocated to the dedicated
        // `gvisor/checkout_runtime.rs` submodule so module privacy enforces field inseparability.
        // `run_checkout_preparation_v2(` and one `PhaseAuthorization` (the v2 signature) moved with it;
        // the submodule also imports `PhaseAuthorization` (its own `use`), so its count is 2.
        // CT-007 5b.3-6c: the DORMANT orchestrator + continuation (in gvisor.rs) now CALL the V2 phase
        // surface — mint the advertise/fetch/materialization generations and run the fused Hop B — so
        // gvisor.rs's `PhaseAuthorization` count is 10, it gains `mint_phase_credential` (advertise +
        // fetch + materialization = 3) and one `run_checkout_preparation_v2(` call. The actual
        // `authorize_checkout_phase(` calls live in the `checkout_orchestration::authorize_phase_generation`
        // helper (Sol's finding 1: authorize the per-phase ROTATED spec), so gvisor.rs no longer calls it
        // directly. These are dormant CALL sites in NON-composition-root modules (the outer orchestrator
        // itself has zero production callers — see the sandbox dormancy pin
        // `the_checkout_runtime_capsule_has_no_production_caller`), NOT a production activation. The
        // composition-root zeros below stay zero.
        // 11 (not 10): CT-007 5b.3-6c Sol's r4 finding-1 control inversion added the
        // `prepare_materialization` closure whose return type names `PhaseAuthorization` once more.
        ("myelin-ci-sandbox", "gvisor.rs", "PhaseAuthorization", 11),
        (
            "myelin-ci-sandbox",
            "gvisor.rs",
            "mint_phase_credential(",
            3,
        ),
        (
            "myelin-ci-sandbox",
            "gvisor.rs",
            "run_checkout_preparation_v2(",
            1,
        ),
        // CT-007 5b.3-6c: the sandbox-side capability vocabulary + the per-phase authorization helper.
        // `mint_phase_credential(` is the `AttemptAuthority` trait method DEFINITION (1); the helper
        // `authorize_phase_generation` holds the ONE `authorize_checkout_phase(` call site + one
        // `PhaseAuthorization` in its return type — no control-plane dependency crosses the boundary.
        (
            "myelin-ci-sandbox",
            "checkout_orchestration.rs",
            "mint_phase_credential(",
            1,
        ),
        (
            "myelin-ci-sandbox",
            "checkout_orchestration.rs",
            "authorize_checkout_phase(",
            1,
        ),
        (
            "myelin-ci-sandbox",
            "checkout_orchestration.rs",
            "PhaseAuthorization",
            1,
        ),
        (
            "myelin-ci-sandbox",
            "gvisor/checkout_runtime.rs",
            "run_checkout_preparation_v2(",
            1,
        ),
        // 6e.2 S2: the third occurrence is the explicitly test-support-gated Hop-B seam consuming
        // the real materialization authorization; the production-zero composition-root pins below
        // remain unchanged.
        (
            "myelin-ci-sandbox",
            "gvisor/checkout_runtime.rs",
            "PhaseAuthorization",
            3,
        ),
        (
            "myelin-ci-sandbox",
            "lib.rs",
            "with_checkout_phase_authorization(",
            1,
        ),
        ("myelin-ci-sandbox", "lib.rs", "PhaseAuthorization", 4),
        // --- explicitly ZERO everywhere they could be composed into production ---
        (
            "myelin-ci-controlplane",
            "ci_runtime_composition.rs",
            "CiJobCredentialGenerationStore",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "ci_runtime_composition.rs",
            "V2PhaseBound",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "ci_runner_composition.rs",
            "with_checkout_phase_authorization(",
            1,
        ),
        (
            "myelin-ci-controlplane",
            "ci_runner_composition.rs",
            "CiJobCredentialGenerationStore",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "runner_bind.rs",
            "V2PhaseBound",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "runner_bind.rs",
            "mint_phase_credential(",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "main.rs",
            "CiJobCredentialGenerationStore",
            0,
        ),
        ("myelin-ci-sandbox", "runner.rs", "PhaseAuthorization", 0),
        // --- CT-007 5b.3-6e.1 activation-chassis selectors (Sol's major 2) ---
        // Definition sites: the selector methods are DEFINED once each (dormant). `with_checkout_config`
        // is the sandbox GvisorBackend builder; `with_activation_readiness` is the control-plane
        // CutoverPlan builder. `ActivationReadinessProbe::production` (the qualified CALL form) never
        // appears at its own definition (`fn production(`), so it is ZERO everywhere until a caller wires
        // it — the pure composition-root zero.
        ("myelin-ci-sandbox", "gvisor.rs", "with_checkout_config", 1),
        (
            "myelin-ci-controlplane",
            "ci_runtime_composition.rs",
            "with_activation_readiness",
            1,
        ),
        // ...and EXPLICITLY ZERO in every composition root that could select them (the same set the
        // V2CheckoutComposition zeros cover). A premature selection in ANY of these turns the scan RED.
        (
            "myelin-ci-controlplane",
            "main.rs",
            "with_checkout_config",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "runner_bind.rs",
            "with_checkout_config",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "ci_runner_composition.rs",
            "with_checkout_config",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "ci_runtime_composition.rs",
            "with_checkout_config",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "lib.rs",
            "with_checkout_config",
            0,
        ),
        ("myelin-ci-sandbox", "lib.rs", "with_checkout_config", 0),
        ("myelin-ci-sandbox", "runner.rs", "with_checkout_config", 0),
        (
            "myelin-ci-controlplane",
            "main.rs",
            "with_activation_readiness",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "runner_bind.rs",
            "with_activation_readiness",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "ci_runner_composition.rs",
            "with_activation_readiness",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "lib.rs",
            "with_activation_readiness",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "main.rs",
            "ActivationReadinessProbe::production",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "runner_bind.rs",
            "ActivationReadinessProbe::production",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "ci_runner_composition.rs",
            "ActivationReadinessProbe::production",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "ci_runtime_composition.rs",
            "ActivationReadinessProbe::production",
            0,
        ),
        (
            "myelin-ci-controlplane",
            "lib.rs",
            "ActivationReadinessProbe::production",
            0,
        ),
    ];

    fn production_of(source: &str) -> &str {
        // Split at the TOP-LEVEL test module specifically — several of these files carry inline
        // `#[cfg(test)]` helpers far above it, and splitting on the bare attribute would truncate
        // most of the production body and make the whole scan vacuous.
        match source.find("\n#[cfg(test)]\nmod tests {") {
            Some(end) => &source[..end],
            None => source,
        }
    }

    /// Count CODE occurrences only: a doc/line comment naming a seam is documentation, not a call,
    /// and pinning prose would make this table churn on every wording change while saying nothing
    /// about reachability.
    fn code_occurrences(source: &str, marker: &str) -> usize {
        production_of(source)
            .lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                !trimmed.starts_with("//") && !trimmed.starts_with("*")
            })
            .map(|line| line.matches(marker).count())
            .sum()
    }

    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/");
    let mut scanned_files = 0_usize;
    let mut observed: std::collections::BTreeMap<(String, String, &str), usize> =
        std::collections::BTreeMap::new();
    // Walk `src` RECURSIVELY (CT-007 5b.3-6a, Sol's r4: the checkout capsule now lives in the
    // `gvisor/checkout_runtime.rs` submodule, so a non-recursive `read_dir` would leave it unscanned).
    // The `name` is the path RELATIVE to `src` (e.g. "gvisor/checkout_runtime.rs"), so files directly
    // in `src` keep their bare basenames.
    fn collect_rs(
        dir: &std::path::Path,
        base: &std::path::Path,
        out: &mut Vec<(String, std::path::PathBuf)>,
    ) {
        for entry in std::fs::read_dir(dir).unwrap_or_else(|_| panic!("{dir:?} is a directory")) {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect_rs(&path, base, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .replace('\\', "/");
                out.push((rel, path));
            }
        }
    }
    for krate in ["myelin-ci-controlplane", "myelin-ci-sandbox"] {
        let source_dir = workspace.join(krate).join("src");
        let mut files = Vec::new();
        collect_rs(&source_dir, &source_dir, &mut files);
        for (name, path) in files {
            scanned_files += 1;
            let source = std::fs::read_to_string(&path).unwrap();
            for marker in MARKERS {
                let count = code_occurrences(&source, marker);
                if count > 0 {
                    observed.insert((krate.to_string(), name.clone(), marker), count);
                }
            }
        }
    }
    assert!(
        scanned_files > 60,
        "the dormancy scan really walked both crates (saw {scanned_files} files)"
    );

    let expected: std::collections::BTreeMap<(String, String, &str), usize> = ALLOWED
        .iter()
        .filter(|(_, _, _, count)| *count > 0)
        .map(|(krate, file, marker, count)| {
            ((krate.to_string(), file.to_string(), *marker), *count)
        })
        .collect();
    assert_eq!(
        observed, expected,
        "the V2 phase-credential surface's production occurrences changed. If this is a deliberate \
         ACTIVATION, that is exactly what this pin exists to surface: update the table in the same \
         commit that composes it. If it is not, something reached the dormant surface."
    );

    // The explicit zeroes are asserted separately so a typo in a file name cannot hide them.
    for (krate, file, marker, count) in ALLOWED {
        if count != 0 {
            continue;
        }
        let source = std::fs::read_to_string(workspace.join(krate).join("src").join(file)).unwrap();
        assert_eq!(
            code_occurrences(&source, marker),
            0,
            "{krate}/src/{file} must never contain `{marker}` while production is V1-pinned"
        );
    }

    // Belt and braces: the production composition roots construct the V1 store shape only.
    let composition = std::fs::read_to_string(
        workspace
            .join("myelin-ci-controlplane")
            .join("src")
            .join("ci_runtime_composition.rs"),
    )
    .unwrap();
    assert!(
        code_occurrences(&composition, "CiJobAccountingStore::with_pg(") > 0
            && code_occurrences(&composition, "with_pg_and_write_version") == 0,
        "the production runtime composition root builds default (V1/V3) stores only"
    );
}

/// **Round-1 blocker 3: the V2 workload CAS may never resurrect an expired execution lease.**
///
/// A workload credential minted moments before lease expiry, then presented AFTER expiry but before
/// the reaper wins the row-lock race, must not transition the stale owner to `running` and install a
/// fresh lease. The V2 CAS carries `lease_expires > statement_timestamp()` for exactly this; the
/// byte-frozen legacy CAS deliberately does not, which is why the two queries are separate.
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

        // The EXECUTION lease lapses while the immutable claim window is still wide open — exactly
        // the state the reaper is about to reclaim.
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

        // Neither the queue row nor the public surface moved, and no fresh lease was installed.
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

        // Restore a live lease and the SAME credential launches — proving the lease predicate was
        // the one and only reason for the refusal.
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

/// **Round-2 blocker 1: acquisition is cancellation-safe — an aborted future leaks no open
/// transaction, no stale tenant scope, and no retained lock onto the pooled connection.**
///
/// Two cases on a ONE-connection pool: (a) the acquisition future is aborted while blocked mid-
/// transaction (BEGIN + SET LOCAL done, blocked taking the `job_queue` FOR SHARE); (b) a fully
/// acquired handle is dropped WITHOUT `release()`. Both must roll back through the RAII
/// `sqlx::Transaction` so the next borrower on that single connection sees a clean session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn aborting_or_dropping_acquisition_leaks_no_transaction_scope_or_lock() {
    let (schema, bootstrap, admin, app) = migrated_schema("cancel_safe").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let (minter, _calls) = real_minter();
        // A ONE-connection app pool: if a cancelled/dropped acquisition leaked its transaction onto
        // the connection, the very next borrow would inherit the open tx / stale GUC / held lock.
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

        // ---- (a) abort mid-transaction ----
        // Hold the queue row FOR UPDATE from admin so acquisition's statement A blocks.
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
        // Release the blocker; the aborted acquisition's transaction must already be torn down.
        blocker.rollback().await.unwrap();

        // The next borrow on the SINGLE connection sees no stale tenant scope (a leaked open tx
        // would still carry the SET LOCAL scope).
        let scope: String = sqlx::query_scalar("SELECT current_setting('myelin.tenant_id', true)")
            .fetch_one(&one_pool)
            .await
            .unwrap();
        assert_eq!(
            scope, "",
            "a cancelled acquisition left no tenant scope on the connection"
        );
        // ...and a fresh statement on that connection works (it is not stuck in a leftover tx).
        let usable: i32 = sqlx::query_scalar("SELECT 1")
            .fetch_one(&one_pool)
            .await
            .unwrap();
        assert_eq!(usable, 1);
        // ...and the queue row lock was never leaked.
        assert!(
            queue_row_lockable(&admin, &fixture).await,
            "a cancelled acquisition left no lock on the queue row"
        );

        // ---- (b) drop a fully acquired handle without release() ----
        let owned = acquire_phase_generation_ownership(&one_pool, &gate)
            .await
            .unwrap()
            .expect("the current generation grants ownership");
        assert!(
            !queue_row_lockable(&admin, &fixture).await,
            "while ownership is held the queue row is genuinely locked"
        );
        drop(owned); // NO release() — the RAII Transaction Drop must roll back.
                     // Force the pooled connection to flush its queued rollback, then observe a clean session.
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

/// **Round-2 blocker 2: the retained ownership also freezes the journal status against the
/// production topology-deadline sealer, which locks ONLY the journal row (never `job_queue`).**
///
/// With an overdue `seal_after`, the real `seal_expired_prelaunch_usage` sweep would otherwise
/// transition `started → sealed_ceiling`. While ownership is retained it CANNOT: the sealer's
/// `FOR UPDATE SKIP LOCKED` skips the row this transaction holds `FOR SHARE`, so it seals nothing and
/// the row stays `started`. A plain (waiting) `FOR UPDATE` on that journal row from a tagged backend
/// is proven Lock-waiting via `pg_stat_activity`, evidencing the conflict on the journal row itself.
/// After release, the next sweep seals it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn retained_ownership_freezes_the_journal_status_against_the_production_sealer() {
    let (schema, bootstrap, admin, app) = migrated_schema("sealer_race").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let (minter, _calls) = real_minter();
        let store = store(&app, minter, CiJobCredentialWriteVersion::V2PhaseBound);
        let fixture = seed_fixture(&app, &admin, 13, 5).await;
        admit_parent(&admin, &fixture).await;
        // A started transport phase whose seal deadline is ALREADY overdue.
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

        // The REAL production sealer (region-scoped) — admin bypasses RLS in the isolated schema,
        // exactly as every other admin read/write in this suite does.
        let sealer = ci_region_queue_store_test_support(admin.clone());

        // (The positive control that an unguarded overdue row DOES seal is the post-release sweep
        // below: it seals exactly this row once ownership is dropped. A region-wide sweep here on a
        // sibling would also seal the primary row, since the sealer is region-scoped.)

        // ---- take ownership: this locks q FOR SHARE and the transport journal row FOR SHARE ----
        let mut owned = acquire_phase_generation_ownership(&app, &gate)
            .await
            .unwrap()
            .expect("the current generation grants ownership");

        // The production sealer now SKIPS the locked row: it seals nothing and the row stays started.
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
        // Revalidation under the held locks still holds.
        owned
            .validate()
            .await
            .expect("revalidation holds under the retained journal lock");

        // Evidence the FOR SHARE lock is genuinely on the JOURNAL row: a plain (waiting) FOR UPDATE
        // on it from a tagged backend Lock-waits until release.
        let waiter_tag = format!("myelin-cred-journal-waiter-{}", std::process::id());
        let waiter_pool = tagged_pool(&app_url(), &schema, &waiter_tag).await;
        let job_id = fixture.claim.job_id.clone();
        let lease_epoch = fixture.claim.lease_epoch;
        let claim_nonce = fixture.claim.claim_nonce.clone();
        let waiter = tokio::spawn(async move {
            // One transaction on one connection: the tenant scope and the FOR UPDATE must share it,
            // or RLS hides the row from the app role and the lock is never contended.
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
            // A plain FOR UPDATE (NOT skip-locked): it must WAIT for the retained FOR SHARE.
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
            // Release immediately so the following seal sweep can proceed.
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

        // ---- release: the phase can seal again, and the waiter completes ----
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

// =================================================================================================
// 11. CT-007 5b.3-6d STEP 3: the DORMANT durable checkout-composition adapter, live-PG.
//
// These prove the `DurableAttemptAuthority`/parent-attempt reserve hook COMPOSE the journal, lease,
// credential, and reservation authorities correctly — state predicates, RLS role, lock ordering,
// Identity invocation, and the synchronous off-runtime bridge — which the component tests (each
// exercising one authority in isolation) and 6c's fake authorities do not.
// =================================================================================================

/// Build the dormant composition over one pool, injecting the call-counting Identity minter.
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

/// Drive the V2 resolver seam (mint the initial `CheckoutAdvertise`) and build the resolved checkout
/// [`JobSpec`] the parent-attempt reserve hook reconstructs its claim from — exactly what the runner
/// would hand `RunnerAgent`.
fn resolved_checkout_spec(comp: &V2CheckoutComposition, fixture: &Fixture) -> JobSpec {
    let scope = checkout_scope();
    let (minted, context) = comp
        .mint_initial_phase_credential(&fixture.claim, Some(&scope))
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
        "SELECT EXTRACT(EPOCH FROM claim_started_at)::bigint AS cs,
                EXTRACT(EPOCH FROM claim_expires_at)::bigint AS ce,
                claim_nonce::text AS nonce, lease_owner, lease_epoch,
                EXTRACT(EPOCH FROM lease_expires)::bigint AS le
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

/// The generation id the carrier's ephemeral authorization CONTEXT binds — proves the context (not
/// just the returned string) names the exact durable generation.
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

/// Insert one prior (superseded) parent-attempt row under the SAME reserve/policy but a distinct
/// generation, so a later admission counts it toward the exact-policy budget.
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

        // Resolver seam: the initial CheckoutAdvertise is minted before any parent attempt exists.
        let spec = resolved_checkout_spec(&comp, &fixture);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the resolver seam mints advertise exactly once"
        );
        let before = lease_facts(&admin, &fixture).await;

        // Drive the WHOLE adapter phase sequence from a dedicated OFF-runtime thread (the runner
        // thread `CiRunnerLoop::spawn` uses — where the sync bridge does a direct `block_on`).
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
            // transport: begin -> advertise REPLAY -> fetch mint -> complete
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
            // materialization: begin -> mint -> complete -> renew -> workload mint
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

        // The ADAPTER performed the reservation transition and inserted exactly one parent row.
        assert_eq!(
            reservation_state(&admin, &fixture.reserve_handle).await,
            "inflight",
            "admission drove reserved -> inflight"
        );
        assert_eq!(parent_row_count(&admin, &fixture).await, 1);

        // Both journal phases are measured.
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

        // Exactly four generation rows in order, and the adapter's carriers name each durable
        // generation (the ephemeral context binds the same generation id).
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

        // Identity was invoked once per mint call — advertise(apply) + advertise(replay) + fetch +
        // materialization + workload = 5 — yet only FOUR generation rows exist: the replay reproduced
        // the advertise generation deterministically, it did not create a fifth row.
        assert_eq!(calls.load(Ordering::SeqCst), 5);

        // Renew moved ONLY the execution lease; every immutable claim fact is unchanged.
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
        // Control: a clean admission proves the ADAPTER (not a mock) performs the transition.
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

        // Race: hold the exact queue row lock, prove admission is Lock-waiting, then reclaim the
        // generation and commit — admission must refuse with NO durable mutation.
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

        // Reclaim: a reaper-style generation bump, then commit — releases the lock and supersedes
        // the claim the admission is reconstructing.
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
        // (A) The should_requeue() boundary is EXACT at `count < max` (production max = 5). Both
        // sides are pinned so a `count < max - 1` regression (which would wrongly refuse the legal
        // 4/5 requeue) and a `count <= max` regression (which would wrongly permit 5/5) each turn RED.
        //
        // (A1) count == max - 1 (== 4): admitted, should_requeue() == TRUE. Three prior rows + the
        // admitted current row = 4/5. A `count < max - 1` regression refuses here → caught.
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

        // (A2) count == max (== 5): admitted (the fifth attempt IS the last legal admission), but
        // should_requeue() == FALSE — the budget is now spent. Four prior rows + the admitted current
        // = 5/5. A `count <= max` regression would wrongly requeue here → caught.
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

        // (B) exhaustion: the typed exhausted admission, reservation inflight, no new parent row.
        // Production `max_parent_attempts` is 5, so five prior rows exhaust the exact-policy budget.
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

        // (C) a stale authority (its generation reclaimed) refuses renew/begin/mint, touches no
        // durable row, and never reaches Identity.
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

            // Reclaim: bump the generation so the authority's bound claim is now stale.
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

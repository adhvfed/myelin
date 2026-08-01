//! **CT-007 slice 5b.3-6e.2 Stage A — the composed active-CHECKOUT-path §4 proofs, LIVE PostgreSQL.**
//!
//! These tests drive the REAL outer checkout orchestrator (steps 1–14 single-sourced through
//! `launch_checkout_orchestrated_with_given`) via the DORMANT, `test-support`-gated runsc-driver seam,
//! substituting ONLY the hardware (the Hop-A git-container execution + the workload runsc spawn), with
//! an injectable Hop-B outcome (success / terminal / retryable). Unlike the sandbox-crate
//! `orchestrated_active_path_6e2` proofs (which admit with a NO-OP test authority, PG-free), these
//! obtain the REAL
//! `DurableAttemptAuthority` the PRODUCTION way: the real V2 hooks from
//! [`ci_runner_v2_wiring`](myelin_ci_controlplane::ci_runner_v2_wiring), whose
//! `reserve_parent_attempt` transitions the durable reservation `reserved → inflight`, inserts the
//! parent-attempt row, and hands back the durable authority that mints every phase credential and
//! measures every journal phase against LIVE PG.
//!
//! What each test proves is therefore the composition the component tests cannot: the admission
//! handoff, the transport journal, the advertise/fetch generation ordering + renewals, the capsule
//! acquisition, and — crucially — the DURABLE rows the active path writes (credential generations,
//! prelaunch-usage journal phases, the parent-attempt row, and the reservation transition). Each
//! present-and-shaped assertion is deletion-sensitive (a missing row fails the exact-count/exact-shape
//! check), and each test carries a row-ABSENT control (a bogus-key query that MUST return nothing) so
//! the positive assertion is proven non-vacuous.
//!
//! Nothing here is `runsc`/Btrfs/subuid/KVM-dependent: the deterministic directory substrate + the
//! substituted executions run on any host. It DOES require live PG (run as a NON-root user).
#![cfg(feature = "integration")]

mod common;

use std::collections::BTreeMap;

use common::with_schema_cleanup;
use myelin_ci_controlplane::ci_claim_token_issuer::runtime_authorities_from_durable_claim;
use myelin_ci_controlplane::ci_runner_composition::ci_runner_v2_wiring;
use myelin_ci_controlplane::runner_bind::JobSpecResolver;
use myelin_ci_controlplane::{
    ci_production_runtime_factory, ci_runner_identity_authorities, claim_window_secs_for_template,
    CiDriveManifestStore,
    CiDriveManifestV1, CiJobBudgetReservationProvider, CiJobRuntimeAuthorityRequest,
    CiJobSpecStore, CiJobTokenRequest, CiManifestLaneV1, CiManifestLimitsV1,
    CiManifestSchedulingV1, CiManifestTrustTierV1, CiManifestWorkspaceV1, CiRunRecord,
    DurableCiJobLaunchTemplate, DurableEnqueue, GrantedCiJobV1, Lane, LeasedJob,
    ManifestBoundCiJobTokenAuthority, OperationalReservationWriteVersion,
    PgTierPCiJobBudgetReservation, CiPipelineReporterRouter,
};
use myelin_ci_sandbox::checkout_orchestration::CheckoutContinuationOutcome;
use myelin_ci_sandbox::gvisor::checkout_transport_test_support::{
    deterministic_enabled_backend_for_tests, stage_checkout_repo_root,
};
use myelin_ci_sandbox::gvisor::runsc_driver::InjectedHopBOutcome;
use myelin_ci_sandbox::gvisor::GvisorBackend;
use myelin_ci_sandbox::runner::PreparationTerminalDisposition;
use myelin_ci_sandbox::{
    derive_checkout_authorization_scope, CompletionClaim, EgressPolicy, IdemToken, ImageRef,
    JobKind, JobSpec, JobSpecTemplate, MeterTarget, ResourceLimits, ResourceUsage, RunnerHooks,
    TerminalReport, TerminalReporter, TrustTier, WorkspaceSpec,
};
use myelin_config::MyelinConfig;
use myelin_storage::{
    reserve_settle_durable_migrations, HotTables, PgMigrator, SealKey, SubstrateProvider,
};
use myelin_tenancy::{Region, TenantId};
use sqlx::{Executor, PgPool, Row};

static MIGRATION_SCENARIO_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const TENANT: &str = "acme";
const REGION: &str = "fr-par";
const REPO_REF: &str = "myelin://acme/git/repo/widgets";
/// 0xC7 repeated 20 times — the EXACT commit `checkout_spec_for_backend` / the deterministic driver
/// advertise for, a valid non-zero lowercase SHA-1 object id.
const COMMIT_OID: &str = "c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7c7";

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

/// Pin the per-test schema INTO the DSN — `SubstrateProvider::connect` opens its OWN pool from a URL.
fn scoped_url(url: &str, schema: &str) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}options=-csearch_path%3D{schema}%2Cpublic")
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

async fn migrated_schema(tag: &str) -> (String, PgPool, PgPool, PgPool) {
    let _guard = MIGRATION_SCENARIO_LOCK.lock().await;
    let schema = format!(
        "ci_6e2_active_{}_{}_{}",
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

fn uuid(prefix: u8, seed: u64) -> String {
    format!("{prefix:02x}000000-0000-4000-8000-{seed:012x}")
}

fn digest(byte: char) -> String {
    format!("blake3:{}", byte.to_string().repeat(64))
}

/// The checkout scope derived PURELY from `REPO_REF` + `COMMIT_OID` — identical to what the
/// orchestrator re-derives from `spec.workspace`, so the minted credential authorizes.
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
// Fixture: one complete leased generation (reservation, ci_run, manifest, ci_job_spec, job_queue).
// =================================================================================================

struct Fixture {
    claim: CiJobTokenRequest,
    reserve_handle: String,
    /// The immutable durable claim window this generation was sized from — carried onto the
    /// reconstructed [`LeasedJob`] so the wiring's resolver never refuses it as a legacy null-window row.
    claim_window_secs: i64,
}

/// Reconstruct the EXACT leased `job_queue` generation the fixture seeded (`state = 'leased'`), so the
/// wiring's REAL `durable_v2_spec_resolver` can resolve it exactly as production does — reading the
/// durable launch template, minting the initial advertise credential through the SAME composition the
/// hooks share, and producing the digest-pinned `JobSpec` the backend drives.
fn leased_job(fixture: &Fixture) -> LeasedJob {
    LeasedJob {
        tenant_id: TENANT.into(),
        job_id: fixture
            .claim
            .job_id
            .parse()
            .expect("fixture job id is a uuid"),
        run_id: fixture
            .claim
            .wf_run_id
            .parse()
            .expect("fixture run id is a uuid"),
        lane: Lane::Batch,
        concurrency_group: None,
        fair_key: "project".into(),
        trust_tier: TrustTier::Trusted,
        lease_owner: fixture.claim.lease_owner.clone(),
        lease_epoch: fixture.claim.lease_epoch,
        claim_nonce: fixture.claim.claim_nonce.clone(),
        claim_started_at_epoch_secs: fixture.claim.claim_started_at_epoch_secs,
        claim_expires_at_epoch_secs: fixture.claim.claim_expires_at_epoch_secs,
        claim_window_secs: Some(fixture.claim_window_secs),
    }
}

/// Seed a complete leased generation. The MANIFEST job is always checkout-bearing (the schema mandates
/// it). `spec_workspace` is what the durable `ci_job_spec` (dispatched launch template) carries: pass the
/// matching checkout workspace for the positive tests, or a compute `(None, None)` workspace to seed the
/// negative reject-path fixture (a compute spec beneath a checkout manifest — a provenance divergence the
/// durable authority resolution MUST refuse).
async fn seed_fixture(
    app: &PgPool,
    admin: &PgPool,
    seed: u64,
    spec_workspace: WorkspaceSpec,
    image: ImageRef,
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
    let checkout = Some(checkout_scope());
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
        checkout,
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
        "INSERT INTO workflow_run (
           tenant_id, region, run_id, wf_type, wf_version, input, state, correlation_id,
           depth, partition
         ) VALUES ($1, $2, $3::uuid, 'ci.pipeline', $4, '[]'::jsonb, 'running', $5, 0, 0)",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(&wf_run_id)
    .bind(authority.workflow_definition_version)
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
    // Seed the DETERMINISTIC backend image into BOTH the manifest granted job and the durable launch
    // template, so the wiring's resolver produces a `JobSpec` whose `image` the backend actually
    // resolves — the active path is driven through the real resolver, not a hand-built spec.
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

    let idem_token = format!("6e2-active-{seed}");
    let workspace = spec_workspace;
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
            workspace,
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
    let claim_started_at_epoch_secs = now - 5;
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
        claim_window_secs: window,
    }
}

// =================================================================================================
// The ONE production V2 runner wiring — resolver + hooks from a SINGLE Identity (S3).
// =================================================================================================

/// Build the REAL production V2 runner wiring exactly as Stage B will — `ci_runner_v2_wiring` composes
/// the resolver AND the hooks over ONE shared `V2CheckoutComposition` backed by ONE Identity (via
/// `ci_runner_identity_authorities`). Returning BOTH `(resolver, hooks)` is the honesty line: the same
/// composition mints the initial advertise credential (through the resolver) that the hooks'
/// `reserve_parent_attempt` + per-phase authorizations later verify — a broken resolver, a wrong
/// store/region, or a resolver/hooks mispairing can no longer pass unseen behind a hand-built parallel.
async fn real_v2_wiring(
    schema: &str,
) -> (JobSpecResolver, RunnerHooks, CiPipelineReporterRouter) {
    let mut config = MyelinConfig::dev();
    config.database_url = scoped_url(&app_url(), schema);
    config.region = REGION.into();
    let provider = SubstrateProvider::connect(config, 4)
        .await
        .expect("connect the production app-role provider");
    let seal_key = SealKey::from_bytes([0x6e; 32]);
    let identity = ci_runner_identity_authorities(
        provider.clone(),
        "ci-6e2-active-cell",
        &seal_key,
        tokio::runtime::Handle::current(),
    )
    .await
    .expect("compose the production Identity authorities");
    let wiring = ci_runner_v2_wiring(provider.clone(), &identity, tokio::runtime::Handle::current())
        .expect("compose the dormant V2 runner wiring")
        .into_parts();
    let reporter = ci_production_runtime_factory(provider, tokio::runtime::Handle::current())
        .expect("compose production V4 runtime")
        .reporter_router()
        .expect("compose production V4 reporter router");
    (wiring.0, wiring.1, reporter)
}

/// Resolve the seeded leased generation through the wiring's REAL resolver — reading the durable launch
/// template, minting the initial advertise credential, and producing the digest-pinned `JobSpec` the
/// orchestrator drives. The resolver runs the SYNC off-runtime bridge (a direct `block_on`), so it MUST
/// run on a dedicated non-runtime thread — exactly the thread `CiRunnerLoop::spawn` uses.
fn resolve_active_spec(resolver: &JobSpecResolver, fixture: &Fixture) -> JobSpec {
    let leased = leased_job(fixture);
    let resolver = resolver.clone();
    std::thread::spawn(move || resolver(&leased))
        .join()
        .expect("the off-runtime resolve thread completed")
        .expect("the wiring's resolver resolves the leased checkout generation")
}

// =================================================================================================
// Off-runtime driver bridge. The driver calls `reserve_parent_attempt` + every authority phase op
// through the SYNC bridge (a direct `block_on`), so it MUST run on a dedicated non-runtime thread —
// exactly the thread `CiRunnerLoop::spawn` uses.
// =================================================================================================

type CheckoutDrive = (
    Result<
        CheckoutContinuationOutcome,
        myelin_ci_sandbox::checkout_orchestration::CheckoutOrchestrationError,
    >,
    Vec<(String, bool)>,
);

fn drive_checkout_off_runtime(
    backend: GvisorBackend,
    hooks: RunnerHooks,
    spec: JobSpec,
    repo_root: std::path::PathBuf,
    injection: InjectedHopBOutcome,
) -> CheckoutDrive {
    std::thread::spawn(move || {
        backend.drive_checkout_cycle_with_injected_hop_b(
            &spec,
            &hooks,
            &repo_root,
            "checkout.sentinel",
            b"6e2-active-provenance-sentinel",
            injection,
        )
    })
    .join()
    .expect("the off-runtime driver thread completed")
}

// =================================================================================================
// Durable-row assertion helpers.
// =================================================================================================

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

/// The exact durable prelaunch-usage tuple `(status, exact_cpu_seconds, exact_mem_byte_seconds)` for
/// one phase — the deletion/mutation-sensitive proof that the REAL path measured the EXACT usage the
/// substituted executions produced (a wrong total or an unresolved `started` row fails the tuple).
async fn phase_usage(admin: &PgPool, fixture: &Fixture, phase: &str) -> Option<(String, i64, i64)> {
    sqlx::query(
        "SELECT status, exact_cpu_seconds::bigint AS cpu, exact_mem_byte_seconds::bigint AS mem
         FROM ci_job_prelaunch_usage
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
    .map(|row| {
        (
            row.get::<String, _>("status"),
            row.get::<Option<i64>, _>("cpu").unwrap_or(-1),
            row.get::<Option<i64>, _>("mem").unwrap_or(-1),
        )
    })
}

/// The current `job_queue.state` for the fixture's job — the durable proof the workload launch fence's
/// queue→running CAS committed (a leased row that never reached `running` fails this).
async fn job_queue_state(admin: &PgPool, fixture: &Fixture) -> String {
    sqlx::query_scalar::<_, String>(
        "SELECT state FROM job_queue WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(&fixture.claim.job_id)
    .fetch_one(admin)
    .await
    .unwrap()
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

async fn reservation_marker(admin: &PgPool, fixture: &Fixture) -> Option<i16> {
    sqlx::query_scalar(
        "SELECT reservation_write_version FROM job_queue
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(&fixture.claim.job_id)
    .fetch_one(admin)
    .await
    .unwrap()
}

async fn v4_accounting_final(admin: &PgPool, fixture: &Fixture) -> Option<(String, String)> {
    sqlx::query(
        "SELECT terminal_disposition, completion_receipt_v4 FROM ci_job_accounting
         WHERE tenant_id = $1 AND job_id = $2::uuid",
    )
    .bind(TENANT)
    .bind(&fixture.claim.job_id)
    .fetch_optional(admin)
    .await
    .unwrap()
    .map(|row| (row.get("terminal_disposition"), row.get("completion_receipt_v4")))
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

/// A row-ABSENT control: the SAME shaped query keyed to a job_id that does not exist MUST return
/// nothing — proving the positive present-and-shaped assertion is deletion-sensitive, not vacuous.
async fn generation_rows_for_bogus_job(admin: &PgPool) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM ci_job_credential_generation
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(uuid(0xEE, 999_999))
    .fetch_one(admin)
    .await
    .unwrap()
}

// =================================================================================================
// §4.1 — checkout SUCCESS: the full advertise → fetch → materialization → workload path settles, and
// writes the exact durable rows.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn checkout_success_drives_the_active_path_and_writes_the_durable_rows() {
    let (schema, bootstrap, admin, app) = migrated_schema("success").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let root = unique_root("success");
        let (backend, image) = deterministic_enabled_backend_for_tests(&root);
        let repo_root = stage_checkout_repo_root(&root.join("repos"));
        let fixture = seed_fixture(
            &app,
            &admin,
            1,
            WorkspaceSpec {
                repo_ref: Some(REPO_REF.into()),
                commit: Some(COMMIT_OID.into()),
            },
            image,
        )
        .await;

        // Row-ABSENT control BEFORE anything is minted: nothing is written yet, so the positive
        // assertions below cannot be vacuously satisfied.
        assert!(
            generation_rows(&admin, &fixture).await.is_empty(),
            "no generation rows before mint"
        );
        assert_eq!(
            parent_row_count(&admin, &fixture).await,
            0,
            "no parent attempt before drive"
        );
        assert_eq!(
            reservation_state(&admin, &fixture.reserve_handle).await,
            "reserved",
            "the reservation is merely reserved before the admission"
        );
        assert_eq!(
            job_queue_state(&admin, &fixture).await,
            "leased",
            "the seeded generation is leased"
        );

        // The ONE production wiring: the resolver + hooks from a SINGLE Identity/composition (S3).
        let (resolver, hooks, reporter) = real_v2_wiring(&schema).await;
        // Resolving through the REAL wiring resolver mints the INITIAL advertise generation — after this
        // exactly ONE generation (advertise) exists; the drive adds fetch/materialization/workload.
        let spec = resolve_active_spec(&resolver, &fixture);
        assert_eq!(
            generation_rows(&admin, &fixture).await.len(),
            1,
            "the wiring resolver minted exactly the initial advertise generation before the drive"
        );

        let (result, recorded) = drive_checkout_off_runtime(
            backend,
            hooks,
            spec,
            repo_root.clone(),
            InjectedHopBOutcome::Success,
        );

        // Exactly the two scripted transport legs ran (executor panics on a third), under DISTINCT
        // durable credentials, and BOTH phase permits committed.
        assert_eq!(
            recorded.len(),
            2,
            "exactly two transport hops spawn: {recorded:?}"
        );
        assert_ne!(
            recorded[0].0, recorded[1].0,
            "advertise/fetch under DISTINCT jtis: {recorded:?}"
        );
        assert!(
            recorded[0].1 && recorded[1].1,
            "both transport permits commit: {recorded:?}"
        );

        // The active path settled to a clean workload launch carrying the exact substituted usage.
        let workload_usage = match result {
            Ok(CheckoutContinuationOutcome::WorkloadLaunched(launch)) => {
                assert!(
                    launch.output_complete,
                    "the substituted workload completes cleanly"
                );
                assert_eq!(
                    launch.result.usage,
                    ResourceUsage {
                        cpu_seconds: 3,
                        mem_byte_seconds: 7
                    },
                    "the settled workload carries exactly the substituted workload usage"
                );
                launch.result.usage
            }
            other => panic!("expected a clean WorkloadLaunched, got {other:?}"),
        };

        // Durable rows the active path wrote — present-and-shaped (deletion-sensitive):
        // FOUR credential generations, in phase order.
        let rows = generation_rows(&admin, &fixture).await;
        let purposes: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(
            purposes,
            [
                "checkout_advertise",
                "checkout_fetch",
                "checkout_materialization",
                "workload"
            ],
            "the four durable generations, in order: {rows:?}"
        );
        // TWO prelaunch-usage journal phases, both measured to the EXACT usage the substituted
        // executions produced (S4): transport = advertise (3/7) + fetch (11/13) = 14/20; the
        // materialization phase = the substituted Hop-B evidence usage 2/5. A wrong total or an
        // unresolved `started` row fails these exact tuples.
        assert_eq!(
            phase_usage(&admin, &fixture, "checkout_transport").await,
            Some(("measured".to_string(), 14, 20)),
            "the transport phase is measured to the exact advertise+fetch total 14/20"
        );
        assert_eq!(
            phase_usage(&admin, &fixture, "checkout_materialization").await,
            Some(("measured".to_string(), 2, 5)),
            "the materialization phase is measured to the exact substituted Hop-B usage 2/5"
        );
        // S1: the workload launch fence committed the queue→running CAS — the leased row is now running.
        assert_eq!(
            job_queue_state(&admin, &fixture).await,
            "running",
            "the workload launch fence drove the job_queue row leased → running"
        );
        // ONE parent-attempt row, and the reservation drove reserved → inflight.
        assert_eq!(
            parent_row_count(&admin, &fixture).await,
            1,
            "exactly one parent attempt"
        );
        assert_eq!(
            reservation_state(&admin, &fixture.reserve_handle).await,
            "inflight",
            "admission drove the reservation reserved → inflight"
        );
        assert_eq!(
            reservation_marker(&admin, &fixture).await,
            Some(2),
            "the V2 reserve handle persisted the exact Stage-B queue marker"
        );

        // Completion-side Stage-B proof: route the exact running claim through the production V4
        // reporter. This atomically writes the V4 final receipt and settles the inflight reservation.
        reporter
            .report_done(
                &CompletionClaim {
                    tenant: TenantId(TENANT.into()),
                    run: myelin_flow::RunId(fixture.claim.wf_run_id.clone()),
                    job_id: fixture.claim.job_id.clone(),
                    idem_token: fixture.claim.idem_token.clone(),
                    lease_owner: fixture.claim.lease_owner.clone(),
                    lease_epoch: fixture.claim.lease_epoch,
                    claim_nonce: fixture.claim.claim_nonce.clone(),
                },
                &TerminalReport {
                    passed: true,
                    timed_out: false,
                    usage: workload_usage,
                    result_refs: Vec::new(),
                },
            )
            .expect("the production V4 reporter finalizes the exact checkout claim");
        let (disposition, receipt) = v4_accounting_final(&admin, &fixture)
            .await
            .expect("the V4 accounting-final row exists");
        assert_eq!(disposition, "workload_passed");
        assert!(receipt.starts_with("v4:"), "authoritative V4 receipt: {receipt}");
        assert_eq!(
            reservation_state(&admin, &fixture.reserve_handle).await,
            "settled",
            "completion settled the reservation inflight → settled"
        );
        // The row-absent control query returns nothing — the assertions above are non-vacuous.
        assert_eq!(
            generation_rows_for_bogus_job(&admin).await,
            0,
            "a bogus-key query finds no rows"
        );
    })
    .await;
}

// =================================================================================================
// §4.3 — Hop-B PreparationTerminal: terminal disposition, NO workload/settle path.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hop_b_terminal_reports_a_preparation_terminal_and_never_launches() {
    let (schema, bootstrap, admin, app) = migrated_schema("terminal").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let root = unique_root("terminal");
        let (backend, image) = deterministic_enabled_backend_for_tests(&root);
        let repo_root = stage_checkout_repo_root(&root.join("repos"));
        let fixture = seed_fixture(&app, &admin, 3, WorkspaceSpec {
            repo_ref: Some(REPO_REF.into()),
            commit: Some(COMMIT_OID.into()),
        }, image)
        .await;

        let (resolver, hooks, _reporter) = real_v2_wiring(&schema).await;
        let spec = resolve_active_spec(&resolver, &fixture);

        let (result, recorded) = drive_checkout_off_runtime(
            backend,
            hooks,
            spec,
            repo_root.clone(),
            InjectedHopBOutcome::TerminalFailed,
        );

        // Transport still ran (both hops) — the failure is injected at Hop B, after transport.
        assert_eq!(recorded.len(), 2, "the two transport hops still spawned: {recorded:?}");

        // A terminal preparation disposition, carrying the reporting claim — and NO workload launched.
        match result {
            Ok(CheckoutContinuationOutcome::PreparationTerminal { claim, disposition, diagnostic: _ }) => {
                assert!(
                    matches!(disposition, PreparationTerminalDisposition::Failed { .. }),
                    "the injected Hop-B failure is a terminal Failed disposition, got {disposition:?}"
                );
                assert_eq!(
                    claim.job_id, fixture.claim.job_id,
                    "the terminal carries the admission's reporting claim UNCHANGED"
                );
            }
            other => panic!("expected a PreparationTerminal, got {other:?}"),
        }

        // The transport phase measured to its exact 14/20; the materialization phase was begun then
        // RESOLVED to the injected Hop-B usage 2/5 (never left `started` for the sealer) — S4. But the
        // workload generation was NEVER minted (no settle path).
        assert_eq!(
            phase_usage(&admin, &fixture, "checkout_transport").await,
            Some(("measured".to_string(), 14, 20)),
            "the transport phase measured 14/20 before the Hop-B failure"
        );
        assert_eq!(
            phase_usage(&admin, &fixture, "checkout_materialization").await,
            Some(("measured".to_string(), 2, 5)),
            "the materialization phase was RESOLVED to the injected Hop-B usage 2/5"
        );
        let purposes: Vec<String> = generation_rows(&admin, &fixture)
            .await
            .into_iter()
            .map(|r| r.0)
            .collect();
        assert!(
            !purposes.contains(&"workload".to_string()),
            "NO workload generation on a terminal Hop-B failure: {purposes:?}"
        );
        // The workload launch fence never ran — the job_queue row never reached running.
        assert_ne!(
            job_queue_state(&admin, &fixture).await,
            "running",
            "a terminal Hop-B failure never commits the queue→running CAS"
        );
        // The parent attempt exists (admission happened) — the row-absent control still finds nothing.
        assert_eq!(parent_row_count(&admin, &fixture).await, 1, "the admission inserted one parent row");
        assert_eq!(generation_rows_for_bogus_job(&admin).await, 0, "a bogus-key query finds no rows");
    })
    .await;
}

// =================================================================================================
// §4.4 — Hop-B PreparationRetryable: requeue request, NO job.done / workload.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hop_b_retryable_reports_a_preparation_retry_and_never_launches() {
    let (schema, bootstrap, admin, app) = migrated_schema("retry").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let root = unique_root("retry");
        let (backend, image) = deterministic_enabled_backend_for_tests(&root);
        let repo_root = stage_checkout_repo_root(&root.join("repos"));
        let fixture = seed_fixture(&app, &admin, 4, WorkspaceSpec {
            repo_ref: Some(REPO_REF.into()),
            commit: Some(COMMIT_OID.into()),
        }, image)
        .await;

        let (resolver, hooks, _reporter) = real_v2_wiring(&schema).await;
        let spec = resolve_active_spec(&resolver, &fixture);

        let (result, recorded) = drive_checkout_off_runtime(
            backend,
            hooks,
            spec,
            repo_root.clone(),
            InjectedHopBOutcome::RetryableInfrastructure,
        );

        assert_eq!(recorded.len(), 2, "the two transport hops still spawned: {recorded:?}");

        // A retry REQUEST (budget not yet exhausted), carrying the reporting claim — NO workload.
        match result {
            Ok(CheckoutContinuationOutcome::PreparationRetryable { claim, .. }) => {
                assert_eq!(
                    claim.job_id, fixture.claim.job_id,
                    "the retry carries the admission's reporting claim UNCHANGED"
                );
            }
            other => panic!("expected a PreparationRetryable, got {other:?}"),
        }

        // Both prelaunch phases resolve to their exact measured tuples (S4): transport is the
        // advertised 3/7 + fetch 11/13 total, and materialization is the injected 2/5 usage. Neither
        // may be left `started` for the sealer even though no workload path ran.
        assert_eq!(
            phase_usage(&admin, &fixture, "checkout_transport").await,
            Some(("measured".to_string(), 14, 20)),
            "the transport phase measured 14/20 before the retryable Hop-B failure"
        );
        assert_eq!(
            phase_usage(&admin, &fixture, "checkout_materialization").await,
            Some(("measured".to_string(), 2, 5)),
            "the materialization phase was RESOLVED to the injected Hop-B usage 2/5 on the retry path"
        );
        // No workload generation was minted (no job.done path).
        let purposes: Vec<String> = generation_rows(&admin, &fixture)
            .await
            .into_iter()
            .map(|r| r.0)
            .collect();
        assert!(
            !purposes.contains(&"workload".to_string()),
            "NO workload generation on a retryable Hop-B failure: {purposes:?}"
        );
        assert_ne!(
            job_queue_state(&admin, &fixture).await,
            "running",
            "a retryable Hop-B failure never commits the queue→running CAS"
        );
        assert_eq!(parent_row_count(&admin, &fixture).await, 1, "the admission inserted one parent row");
    })
    .await;
}

// =================================================================================================
// §4.4 — THE INVARIANT (replaces compute-through-V2, which is structurally unrepresentable): EVERY CI
// manifest job reconstructs a checkout-bearing `(Some, Some)` authority — the durable guarantee that the
// `run_cycle` / `ci_runner_v2_wiring` compute arm is DEAD-in-CI today. This is the tripwire: a future
// change that makes a compute CI authority representable MUST break this assertion, and (per its
// message) may not ship without a hardware-independent compute-through-V2 live-PG proof.
// =================================================================================================

/// Build a valid multi-job [`CiDriveManifestV1`] + its matching [`CiRunRecord`] + a claim, all
/// mutually consistent so [`runtime_authorities_from_durable_claim`] accepts them. Every job is
/// checkout-bearing (the manifest schema mandates a `repo_ref`/`commit_oid`).
fn multi_job_manifest_and_run(
    seed: u64,
    jobs: usize,
) -> (CiDriveManifestV1, CiRunRecord, CiJobTokenRequest) {
    let ci_run_id = uuid(0x10, seed);
    let wf_run_id = uuid(0x20, seed);
    let project_id = uuid(0x30, seed);
    let pipeline_id = uuid(0x50, seed);
    let limits = CiManifestLimitsV1 {
        cpu_millis: 1_000,
        mem_bytes: 256 * 1024 * 1024,
        disk_bytes: 1024 * 1024 * 1024,
        pids_max: 128,
        timeout_secs: 120,
    };
    // Each job needs a DISTINCT stage (the manifest requires `check_context == stage`, jobs strictly
    // sorted by name, and `check_attempts` to exactly cover the distinct contexts). `stage{i}` sorts
    // lexicographically, so name-order and context-coverage both hold.
    let stages: Vec<String> = (0..jobs).map(|i| format!("stage{i}")).collect();
    let granted: Vec<GrantedCiJobV1> = (0..jobs)
        .map(|i| {
            let authority = CiJobRuntimeAuthorityRequest {
                tenant_id: TENANT.into(),
                region: REGION.into(),
                ci_run_id: ci_run_id.clone(),
                wf_run_id: wf_run_id.clone(),
                project_id: project_id.clone(),
                job_id: uuid(0x40, seed + i as u64),
                stage: stages[i].clone(),
                concrete_name: stages[i].clone(),
                trigger_kind: "push".into(),
                trust_tier: "trusted".into(),
                source_snapshot_digest: digest('a'),
                workflow_definition_version: 3,
                workflow_code_hash: digest('c'),
                policy_revision: "linux-small-v1:1".into(),
                limits: limits.clone(),
                checkout: Some(checkout_scope()),
            };
            GrantedCiJobV1 {
                job_id: uuid(0x40, seed + i as u64),
                stage: stages[i].clone(),
                name: stages[i].clone(),
                check_context: stages[i].clone(),
                needs: Vec::new(),
                matrix_key: BTreeMap::new(),
                image: format!("registry.example/ci@sha256:{}", "b".repeat(64)),
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
                reserve_handle: format!("ci-reserve:v2:invariant-{seed}-{i}"),
                token_authority_handle: ManifestBoundCiJobTokenAuthority::handle_for(&authority),
                continue_on_error: false,
            }
        })
        .collect();
    let check_attempts: BTreeMap<String, u32> = stages.iter().map(|s| (s.clone(), 1)).collect();
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
        workflow_definition_version: 3,
        workflow_code_hash: digest('c'),
        authority_policy_revision: "linux-small-v1:1".into(),
        repo_ref: REPO_REF.into(),
        commit_oid: COMMIT_OID.into(),
        run_ref: format!("myelin://{TENANT}/ci/run/{ci_run_id}"),
        started_at: "2026-07-30T12:00:00.000000Z".into(),
        trust_tier: CiManifestTrustTierV1::Trusted,
        check_attempts,
        merge_waiter: None,
        jobs: granted,
    };
    let run = CiRunRecord {
        tenant_id: TENANT.into(),
        run_id: ci_run_id.clone(),
        region: REGION.into(),
        project_id,
        pipeline_id,
        wf_run_id: wf_run_id.clone(),
        repo_ref: Some(REPO_REF.into()),
        commit_oid: Some(COMMIT_OID.into()),
        cause_event_id: None,
        cause_depth: 0,
        caused_by: None,
        definition_snapshot: "snapshot".into(),
        trigger_kind: "push".into(),
        concurrency_group: None,
        pr_head_generation: None,
        trust_tier: "trusted".into(),
        state: "running".into(),
        correlation_id: format!("corr-{seed}"),
    };
    let claim = CiJobTokenRequest {
        tenant_id: TENANT.into(),
        region: REGION.into(),
        wf_run_id,
        ci_run_id,
        job_id: uuid(0x40, seed),
        token_authority_handle: manifest.jobs[0].token_authority_handle.clone(),
        idem_token: format!("invariant-{seed}"),
        lease_owner: "runner-1".into(),
        lease_epoch: 1,
        claim_nonce: uuid(0x60, seed),
        claim_started_at_epoch_secs: 0,
        claim_expires_at_epoch_secs: 1,
    };
    (manifest, run, claim)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_ci_manifest_authority_is_checkout_bearing() {
    // Pure invariant (no PG needed) — runs in the integration binary as the 4th §4 slot. Because
    // `CiManifestWorkspaceV1` mandates `repo_ref`/`commit_oid`, EVERY manifest job reconstructs a
    // `(Some, Some)` checkout authority, so a compute CI job is unrepresentable through the durable
    // manifest authority — which is exactly why compute-through-V2 has no live-PG proof today.
    let (manifest, run, claim) = multi_job_manifest_and_run(7, 3);
    let authorities = runtime_authorities_from_durable_claim(&claim, &run, &manifest)
        .expect("the durable authority reconstruction succeeds for a valid multi-job manifest");
    assert_eq!(authorities.len(), 3, "one authority per manifest job");
    for authority in &authorities {
        assert!(
            authority.checkout.is_some(),
            "EVERY CI manifest job reconstructs a checkout-bearing authority — the compute arm is \
             dead-in-CI. CHANGING THIS ASSERTION REQUIRES LANDING A HARDWARE-INDEPENDENT \
             compute-through-V2 PROOF IN THE SAME CHANGE. Offending authority: {authority:?}"
        );
    }
}

// =================================================================================================
// §4.5 — the NEGATIVE reject-path proof: a compute-shaped durable `ci_job_spec` beneath a checkout
// manifest is refused as DurableAuthorityUnavailable, with ZERO mutation. This is the deletion/
// mutation-sensitive complement: a rejection that leaked a partial row would be caught by the
// post-rejection row-count assertions.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_compute_spec_beneath_a_checkout_manifest_is_rejected_with_zero_mutation() {
    let (schema, bootstrap, admin, app) = migrated_schema("negative").await;
    with_schema_cleanup(&bootstrap.clone(), &schema.clone(), || async move {
        let image = ImageRef::pinned(format!(
            "test.local/checkout-workload@sha256:{}",
            "a".repeat(64)
        ))
        .unwrap();
        // A CHECKOUT manifest but a COMPUTE-shaped durable `ci_job_spec` (workspace `(None, None)`) — the
        // provenance divergence the durable authority resolution MUST refuse.
        let fixture = seed_fixture(
            &app,
            &admin,
            5,
            WorkspaceSpec {
                repo_ref: None,
                commit: None,
            },
            image,
        )
        .await;

        // Baseline BEFORE the attempt: reservation merely reserved, and no credential/parent rows.
        assert_eq!(
            reservation_state(&admin, &fixture.reserve_handle).await,
            "reserved"
        );
        assert!(
            generation_rows(&admin, &fixture).await.is_empty(),
            "no credential rows before the attempt"
        );
        assert_eq!(
            parent_row_count(&admin, &fixture).await,
            0,
            "no parent attempt before the attempt"
        );

        // Resolving the leased row through the REAL wiring resolver runs the initial mint, which resolves
        // the durable authority and refuses (compute spec ≠ checkout manifest) with
        // DurableAuthorityUnavailable — never signing a credential. Driving the PRODUCTION resolver (not a
        // hand-built composition) is the honesty line: a broken resolver/store/region pairing would fail here.
        let (resolver, _hooks, _reporter) = real_v2_wiring(&schema).await;
        let leased = leased_job(&fixture);
        let err = std::thread::spawn(move || resolver(&leased))
            .join()
            .expect("the off-runtime resolve thread completed")
            .expect_err("a compute spec beneath a checkout manifest must be refused");
        assert!(
            err.contains("durable run, manifest, or launch authority is unavailable"),
            "the refusal is a durable-authority-unavailable rejection, got: {err}"
        );

        // ZERO mutation: a rejection that leaked a partial row would be caught here.
        assert!(
            generation_rows(&admin, &fixture).await.is_empty(),
            "the refused mint wrote NO credential generation row"
        );
        assert_eq!(
            parent_row_count(&admin, &fixture).await,
            0,
            "the refused mint inserted NO parent attempt"
        );
        assert_eq!(
            reservation_state(&admin, &fixture.reserve_handle).await,
            "reserved",
            "the refused mint left the reservation untouched (never reserved → inflight)"
        );
    })
    .await;
}

fn unique_root(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "myelin-6e2-active-{tag}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

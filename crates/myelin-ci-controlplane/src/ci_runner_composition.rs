//! Production Identity composition for the CI runner lane.
//!
//! This module owns the one construction path from the sealed durable cell token root and durable
//! S7 store to the claim-time issuer, scoped Tier-P reservation lifecycle, and final pre-spawn
//! authorizer. It deliberately does not spawn a runner: activation remains refused until the
//! exact-tenant workflow worker/reporter and complete crash matrix are composed around these
//! authorities.

use std::sync::Arc;

use myelin_ci_sandbox::hardening::HardeningProfile;
use myelin_ci_sandbox::{
    CompletionSettlementOwner, HookError, JobKind, JobSpec, JobSpecTemplate, ReserveHandle,
    ResourceUsage, RunTokenAuthorizationContext, RunnerHooks,
};
use myelin_identity_service::mint::RunTokenAuthorizer;
use myelin_identity_service::{
    CellTokenAuthority, PasetoCapabilitySigner, PasetoCapabilityVerifier, RevocationStore,
    RunTokenMinter,
};
use myelin_storage::reserve_settle::{MeteredUnit, RunId as CostRunId};
use myelin_storage::{
    with_tenant_tx_error, DurableCellRootBacking, DurableCostLedger, DurableRevocationBacking,
    PgError, SealKey, SubstrateProvider,
};
use myelin_tenancy::{Region, TenantId};
use sqlx::Row;

use crate::ci_pipeline_driver::validate_reservation_pricing_policy;
use crate::{
    CiDriveManifestStore, CiJobAccountingPricer, CiJobQueueStore, CiJobSpecStore,
    IdentityCiJobCredentialMinter, IdentityCiJobLaunchAuthorizer, LockedManifestCiJobTokenIssuer,
    Meter, TierPOperationalCiJobPricer,
};

/// Credential-free refusal from the production CI Identity composition root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiRunnerIdentityCompositionError {
    InvalidCellId,
    DurableCellRootUnavailable,
    InvalidCellRoot,
}

impl std::fmt::Display for CiRunnerIdentityCompositionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCellId => f.write_str("CI runner cell identity is invalid"),
            Self::DurableCellRootUnavailable => {
                f.write_str("CI runner durable cell token authority is unavailable")
            }
            Self::InvalidCellRoot => {
                f.write_str("CI runner durable cell token authority is invalid")
            }
        }
    }
}

impl std::error::Error for CiRunnerIdentityCompositionError {}

/// The two production Identity authorities consumed by the durable runner path.
#[derive(Clone)]
pub struct CiRunnerIdentityAuthorities {
    token_issuer: LockedManifestCiJobTokenIssuer,
    launch_authorizer: Arc<IdentityCiJobLaunchAuthorizer>,
    /// CT-007 slice 5b.3-6e.2: the SHARED concrete Identity phase-credential minter — the SAME
    /// `IdentityCiJobCredentialMinter` backing the V1 token issuer (never a second Identity), handed
    /// to [`V2CheckoutComposition`](crate::ci_checkout_composition::V2CheckoutComposition) so the V2
    /// per-phase credential path mints through one Identity.
    phase_credential_minter: Arc<IdentityCiJobCredentialMinter>,
}

impl CiRunnerIdentityAuthorities {
    pub fn token_issuer(&self) -> &LockedManifestCiJobTokenIssuer {
        &self.token_issuer
    }

    pub fn launch_authorizer(&self) -> Arc<IdentityCiJobLaunchAuthorizer> {
        self.launch_authorizer.clone()
    }

    /// The shared Identity phase-credential minter (see the field doc). Returned as the trait object
    /// [`V2CheckoutComposition::new`](crate::ci_checkout_composition::V2CheckoutComposition::new)
    /// consumes.
    pub fn phase_credential_minter(
        &self,
    ) -> Arc<dyn crate::ci_credential_generation::CiPhaseCredentialMinter> {
        self.phase_credential_minter.clone()
    }
}

/// Compose the production runner's scoped launch lifecycle around the durable Identity authorizer.
///
/// Successful usage settlement belongs to the exact-tenant terminal reporter. These hooks advance
/// the immutable Tier-P reservation to in-flight before the final launch CAS. A pre-spawn refusal
/// zero-settles only when the exact claim generation is already proven canceled; retryable,
/// replacement, running, and reporter-completed generations retain their reservation.
pub fn ci_runner_hooks(
    provider: SubstrateProvider,
    launch_authorizer: Arc<IdentityCiJobLaunchAuthorizer>,
    rt: tokio::runtime::Handle,
) -> RunnerHooks {
    let lifecycle = Arc::new(PgTierPCiJobLifecycle::new(provider, rt));
    let begin = lifecycle.clone();
    let verify = lifecycle.clone();
    let release = lifecycle;
    let checkout_authorizer = launch_authorizer.clone();
    RunnerHooks::new_with_launch_fence(
        CompletionSettlementOwner::TerminalReporter,
        Box::new(move |spec| begin.begin(spec)),
        Box::new(move |spec, handle, usage| release.release_unused(spec, handle, usage)),
        Box::new(move |spec| {
            verify.verify_for_launch(spec)?;
            launch_authorizer.authorize_retained(spec)
        }),
        Box::new(|spec| {
            HardeningProfile::derive(spec)
                .assert_enforced()
                .map_err(|_| HookError("mandatory sandbox isolation profile is unavailable".into()))
        }),
    )
    // CT-007 slice 5b.3-2c: the real pre-Hop-A checkout-authorization hook, backed by the SAME
    // `IdentityCiJobLaunchAuthorizer` the launch fence above uses (`authorize_checkout` shares its
    // verification core with `authorize_retained`, so both stay backed by one implementation).
    .with_checkout_authorization(Box::new(move |spec, scope| {
        checkout_authorizer.authorize_checkout(spec, scope)
    }))
}

/// **CT-007 slice 5b.3-6e.2: the V2 runner composition root's bundled output.** The V2 spec resolver
/// and V2 [`RunnerHooks`] selected by Stage B stay together so `main` cannot wire a V1 resolver to V2
/// hooks (or vice versa).
pub struct CiRunnerV2Wiring {
    resolver: crate::runner_bind::JobSpecResolver,
    hooks: RunnerHooks,
}

impl CiRunnerV2Wiring {
    /// The V2 spec resolver (checkout → initial `CheckoutAdvertise`, compute → initial `Workload`).
    pub fn resolver(&self) -> crate::runner_bind::JobSpecResolver {
        self.resolver.clone()
    }

    /// Consume into the exact `(resolver, hooks)` pair `CiRunnerLoop::new` takes.
    pub fn into_parts(self) -> (crate::runner_bind::JobSpecResolver, RunnerHooks) {
        (self.resolver, self.hooks)
    }
}

/// **CT-007 slice 5b.3-6e.2: the ONE named V2 runner composition root.** Composes, for one
/// region, the coupled V2 activation choices in a single reviewable place:
///
/// 1. SHARES the concrete `IdentityCiJobCredentialMinter` from `identity` (never a second Identity);
/// 2. constructs exactly one [`V2CheckoutComposition`](crate::ci_checkout_composition::V2CheckoutComposition);
/// 3. builds the V2 resolver ([`durable_v2_spec_resolver`](crate::runner_bind::durable_v2_spec_resolver));
/// 4. builds the V2 hooks: the V2 workload launch fence (`authorize_workload_v2_retained`), the
///    per-phase checkout authorization hook, and the parent-attempt reservation admission (compute
///    AND checkout).
///
/// The Stage-B composition root selects this as one atomic unit; it never pairs a V1 resolver with
/// V2 hooks (or vice versa).
///
/// **CT-007 5b.3-6e.2 Stage A (Sol ruling) — the parent-attempt reserve hook's COMPUTE arm is a
/// FUTURE / non-manifest path, DEAD-in-CI today.** The hook admits both compute `(None, None)` and
/// checkout `(Some, Some)` jobs, but every CI manifest job is checkout-bearing: `CiDriveManifestV1`'s
/// `workspace` mandates a `repo_ref`/`commit_oid` and `runtime_authorities_from_durable_claim` always
/// reconstructs a `(Some, Some)` checkout authority (see the `None`-for-compute note at
/// `ci_launch_authority.rs:68`). A compute CI job therefore cannot be seeded through this manifest-bound
/// durable authority, so the compute arm is exercised by NO live-PG §4 proof today. It is kept
/// intentionally — a future compute authority (workload-as-first-generation) can activate it without
/// reshaping — and the invariant test `every_ci_manifest_authority_is_checkout_bearing`
/// (`integration_ci_6e2_active_path.rs`) is the tripwire that forces a compute-through-V2 proof to land
/// in the same change that first makes a compute CI authority representable.
pub fn ci_runner_v2_wiring(
    provider: SubstrateProvider,
    identity: &CiRunnerIdentityAuthorities,
    rt: tokio::runtime::Handle,
) -> Result<CiRunnerV2Wiring, HookError> {
    let pool = provider.db_pool().clone();
    let region = provider.config().region.clone();
    let composition = crate::ci_checkout_composition::V2CheckoutComposition::new(
        pool.clone(),
        region.clone(),
        identity.phase_credential_minter(),
        crate::ci_job_queue_store(pool.clone()),
        rt.clone(),
    )?;
    let resolver = crate::runner_bind::durable_v2_spec_resolver(
        crate::ci_job_spec_store(pool),
        region,
        rt.clone(),
        composition.clone(),
    );
    let hooks = ci_runner_v2_hooks(provider, identity.launch_authorizer(), composition, rt);
    Ok(CiRunnerV2Wiring { resolver, hooks })
}

/// The V2 runner hooks (see [`ci_runner_v2_wiring`]): the SAME scoped Tier-P lifecycle + checkout
/// authorization as [`ci_runner_hooks`], PLUS the V2 workload launch fence, the per-phase checkout
/// authorization hook, and the parent-attempt reservation admission.
fn ci_runner_v2_hooks(
    provider: SubstrateProvider,
    launch_authorizer: Arc<IdentityCiJobLaunchAuthorizer>,
    composition: crate::ci_checkout_composition::V2CheckoutComposition,
    rt: tokio::runtime::Handle,
) -> RunnerHooks {
    use myelin_ci_sandbox::CheckoutPhase;
    let lifecycle = Arc::new(PgTierPCiJobLifecycle::new(provider, rt));
    let begin = lifecycle.clone();
    let verify = lifecycle.clone();
    let release = lifecycle;
    let checkout_authorizer = launch_authorizer.clone();
    let phase_authorizer = launch_authorizer.clone();
    let workload_authorizer = launch_authorizer;
    RunnerHooks::new_with_launch_fence(
        CompletionSettlementOwner::TerminalReporter,
        Box::new(move |spec| begin.begin(spec)),
        Box::new(move |spec, handle, usage| release.release_unused(spec, handle, usage)),
        // The V2 workload launch fence: verify the reservation is launchable, then authorize the
        // WORKLOAD GENERATION (a V2 phase credential — any preparation credential is refused here by
        // purpose/generation), NOT the legacy claim-bound credential.
        Box::new(move |spec| {
            verify.verify_for_launch(spec)?;
            workload_authorizer.authorize_workload_v2_retained(spec)
        }),
        Box::new(|spec| {
            HardeningProfile::derive(spec)
                .assert_enforced()
                .map_err(|_| HookError("mandatory sandbox isolation profile is unavailable".into()))
        }),
    )
    .with_checkout_authorization(Box::new(move |spec, scope| {
        checkout_authorizer.authorize_checkout(spec, scope)
    }))
    // The per-phase Hop A/Hop B authorization: each preparation generation is authorized against its
    // OWN retained boundary (advertise / fetch / materialization).
    .with_checkout_phase_authorization(Box::new(move |spec, scope, phase| match phase {
        CheckoutPhase::Advertise => {
            phase_authorizer.authorize_checkout_advertise_retained(spec, scope)
        }
        CheckoutPhase::Fetch => phase_authorizer.authorize_checkout_fetch_retained(spec, scope),
        CheckoutPhase::Materialization => {
            phase_authorizer.authorize_checkout_materialization_retained(spec, scope)
        }
    }))
    // Parent-attempt admission (reserve → inflight + parent row) — for compute AND checkout.
    .with_parent_attempt_reservation(composition.parent_attempt_reserve_hook())
}

/// Test-support job-local cancellation coordinator for Tier-P reservation crash probes.
///
/// Production supersession is run-level and producer-generation ordered in
/// [`crate::PgCiRunSupersession`]. This narrower helper remains available only to tests that exercise
/// the two reservation crash windows in isolation.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone)]
pub struct CiRunnerCancellationCoordinator {
    pool: sqlx::PgPool,
    region: Region,
    ledger: DurableCostLedger,
    rt: tokio::runtime::Handle,
}

/// Compose the test-support job-local cancellation capability.
#[cfg(any(test, feature = "test-support"))]
pub fn ci_runner_cancellation_coordinator(
    provider: SubstrateProvider,
    rt: tokio::runtime::Handle,
) -> CiRunnerCancellationCoordinator {
    CiRunnerCancellationCoordinator {
        pool: provider.db_pool().clone(),
        region: Region(provider.config().region.clone()),
        ledger: DurableCostLedger::with_runtime(provider, rt.clone()),
        rt,
    }
}

#[cfg(any(test, feature = "test-support"))]
impl CiRunnerCancellationCoordinator {
    /// Atomically terminalize all superseded queued/leased jobs and settle their exact reservations
    /// at zero. A transaction failure preserves both the schedulable job and held reservation.
    pub fn cancel_superseded(
        &self,
        tenant: &TenantId,
        concurrency_group: &str,
        keep_job_id: &str,
    ) -> Result<Vec<String>, HookError> {
        if !valid_machine_token(&tenant.0)
            || !valid_idem_token(concurrency_group)
            || !concurrency_group.starts_with("pr:")
            || sqlx::types::Uuid::parse_str(keep_job_id).is_err()
        {
            return Err(HookError("CI cancel-superseded scope is invalid".into()));
        }
        bridge(
            &self.rt,
            cancel_superseded_and_settle(
                self.pool.clone(),
                self.region.clone(),
                self.ledger.clone(),
                tenant.clone(),
                concurrency_group.to_owned(),
                keep_job_id.to_owned(),
            ),
        )
        .map_err(|_| HookError("durable CI cancel-superseded transaction refused".into()))
    }
}

#[derive(Clone)]
struct PgTierPCiJobLifecycle {
    pool: sqlx::PgPool,
    region: Region,
    ledger: DurableCostLedger,
    rt: tokio::runtime::Handle,
}

impl PgTierPCiJobLifecycle {
    fn new(provider: SubstrateProvider, rt: tokio::runtime::Handle) -> Self {
        let region = Region(provider.config().region.clone());
        Self {
            pool: provider.db_pool().clone(),
            region,
            ledger: DurableCostLedger::with_runtime(provider, rt.clone()),
            rt,
        }
    }

    fn begin(&self, spec: &JobSpec) -> Result<ReserveHandle, HookError> {
        let scope = reservation_scope(spec, &self.region)?;
        let handle = ReserveHandle(scope.reserve_handle.clone());
        bridge(
            &self.rt,
            scoped_reservation_transition(
                self.pool.clone(),
                self.region.clone(),
                self.ledger.clone(),
                scope,
                ReservationTransition::Begin,
            ),
        )
        .map_err(|_| HookError("durable CI reservation begin refused".into()))?;
        Ok(handle)
    }

    fn release_unused(
        &self,
        spec: &JobSpec,
        handle: &ReserveHandle,
        usage: ResourceUsage,
    ) -> Result<(), HookError> {
        let scope = reservation_scope(spec, &self.region)?;
        if handle.0 != scope.reserve_handle || usage != zero_usage() {
            return Err(HookError(
                "unused CI reservation release scope is invalid".into(),
            ));
        }
        bridge(
            &self.rt,
            scoped_reservation_transition(
                self.pool.clone(),
                self.region.clone(),
                self.ledger.clone(),
                scope,
                ReservationTransition::ReleaseUnused,
            ),
        )
        .map_err(|_| HookError("durable unused CI reservation release refused".into()))
    }

    fn verify_for_launch(&self, spec: &JobSpec) -> Result<(), HookError> {
        let scope = reservation_scope(spec, &self.region)?;
        bridge(
            &self.rt,
            scoped_launch_verification(self.pool.clone(), self.region.clone(), scope),
        )
        .map_err(|_| HookError("durable CI executable authority refused".into()))
    }
}

#[derive(Clone)]
struct CiReservationScope {
    tenant: TenantId,
    region: String,
    wf_run_id: String,
    job_id: String,
    reserve_handle: String,
    idem_token: String,
    trust_tier: String,
    lease_owner: String,
    lease_epoch: i64,
    claim_nonce: String,
    claim_started_at_epoch_secs: i64,
    claim_expires_at_epoch_secs: i64,
    template: JobSpecTemplate,
}

fn reservation_scope(spec: &JobSpec, region: &Region) -> Result<CiReservationScope, HookError> {
    let Some(RunTokenAuthorizationContext::CiJob(context)) = spec.run_token_authorization.as_ref()
    else {
        return Err(HookError(
            "CI reservation requires claimed-job authorization context".into(),
        ));
    };
    let valid = spec.kind == JobKind::Ci
        && valid_machine_token(&context.tenant_id)
        && context.region == region.0
        && sqlx::types::Uuid::parse_str(&context.wf_run_id).is_ok()
        && sqlx::types::Uuid::parse_str(&context.job_id).is_ok()
        && valid_machine_token(&context.lease_owner)
        && context.lease_epoch > 0
        && sqlx::types::Uuid::parse_str(&context.claim_nonce).is_ok()
        && context.claim_started_at_epoch_secs > 0
        && context.claim_expires_at_epoch_secs > context.claim_started_at_epoch_secs
        && valid_idem_token(&spec.idem_token.0)
        && valid_reserve_handle(&spec.meter_to.reserve_id);
    if !valid {
        return Err(HookError("CI reservation scope is invalid".into()));
    }
    Ok(CiReservationScope {
        tenant: TenantId(context.tenant_id.clone()),
        region: context.region.clone(),
        wf_run_id: context.wf_run_id.clone(),
        job_id: context.job_id.clone(),
        reserve_handle: spec.meter_to.reserve_id.clone(),
        idem_token: spec.idem_token.0.clone(),
        trust_tier: trust_tier_token(spec.trust_tier).into(),
        lease_owner: context.lease_owner.clone(),
        lease_epoch: context.lease_epoch,
        claim_nonce: context.claim_nonce.clone(),
        claim_started_at_epoch_secs: context.claim_started_at_epoch_secs,
        claim_expires_at_epoch_secs: context.claim_expires_at_epoch_secs,
        template: spec_template(spec),
    })
}

fn spec_template(spec: &JobSpec) -> JobSpecTemplate {
    JobSpecTemplate {
        kind: spec.kind,
        image: spec.image.clone(),
        command: spec.command.clone(),
        env: spec.env.clone(),
        secret_refs: spec.secret_refs.clone(),
        egress: spec.egress.clone(),
        limits: spec.limits,
        workspace: spec.workspace.clone(),
        trust_tier: spec.trust_tier,
        meter_to: spec.meter_to.clone(),
        idem_token: spec.idem_token.clone(),
    }
}

const fn trust_tier_token(trust_tier: myelin_ci_sandbox::TrustTier) -> &'static str {
    match trust_tier {
        myelin_ci_sandbox::TrustTier::Trusted => "trusted",
        myelin_ci_sandbox::TrustTier::UntrustedFork => "untrusted_fork",
        myelin_ci_sandbox::TrustTier::SelfHosted => "self_hosted",
    }
}

fn valid_machine_token(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn valid_reserve_handle(value: &str) -> bool {
    (value.starts_with(crate::ci_pipeline_driver::TIER_P_OPERATIONAL_RESERVATION_PREFIX)
        || value.starts_with(crate::ci_pipeline_driver::TIER_P_OPERATIONAL_RESERVATION_V2_PREFIX))
        && value.len() <= 512
        && !value.chars().any(char::is_control)
}

fn valid_idem_token(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.len() <= 512
        && !value.chars().any(char::is_control)
}

#[derive(Clone, Copy)]
enum ReservationTransition {
    Begin,
    ReleaseUnused,
}

#[derive(Debug)]
enum ReservationTransitionError {
    Scope,
    Manifest,
    Pricing,
    Ledger,
    Refused,
}

impl From<PgError> for ReservationTransitionError {
    fn from(_: PgError) -> Self {
        Self::Scope
    }
}

async fn scoped_reservation_transition(
    pool: sqlx::PgPool,
    region: Region,
    ledger: DurableCostLedger,
    scope: CiReservationScope,
    transition: ReservationTransition,
) -> Result<(), ReservationTransitionError> {
    let manifest_store =
        CiDriveManifestStore::new(pool.clone(), scope.tenant.clone(), region.clone())
            .map_err(|_| ReservationTransitionError::Manifest)?;
    let spec_store = CiJobSpecStore::with_pg(pool.clone());
    let transaction_tenant = scope.tenant.clone();
    let transaction_region = region.clone();
    let scope_tenant = scope.tenant.0.clone();
    with_tenant_tx_error(&pool, &scope_tenant, &region.0, move |connection| {
        Box::pin(async move {
            let (manifest, _) = manifest_store
                .load_by_wf_run_on_conn(connection, &scope.wf_run_id)
                .await
                .map_err(|_| ReservationTransitionError::Manifest)?
                .ok_or(ReservationTransitionError::Refused)?;
            let job = manifest
                .jobs
                .iter()
                .find(|job| job.job_id == scope.job_id)
                .ok_or(ReservationTransitionError::Refused)?;
            if manifest.tenant_id != transaction_tenant.0
                || manifest.region != transaction_region.0
                || job.reserve_handle != scope.reserve_handle
            {
                return Err(ReservationTransitionError::Refused);
            }
            verify_executable_authority(connection, &spec_store, &manifest, job, &scope).await?;
            let run = CostRunId(scope.reserve_handle.clone());
            match transition {
                ReservationTransition::Begin => {
                    lock_exact_live_claim(connection, &scope, &job.stage).await?;
                    ledger
                        .begin_in_tx(connection, &transaction_tenant, &run)
                        .await
                        .map_err(|_| ReservationTransitionError::Ledger)
                }
                ReservationTransition::ReleaseUnused => {
                    match lock_release_disposition(connection, &scope).await? {
                        ReleaseDisposition::CanceledBeforeLaunch => {
                            let units = tier_p_settlement_units(zero_usage(), &run.0)?;
                            ledger
                                .settle_in_tx(connection, &transaction_tenant, &run, &units)
                                .await
                                .map(|_| ())
                                .map_err(|_| ReservationTransitionError::Ledger)
                        }
                        ReleaseDisposition::RetryOrTerminalOwner => Ok(()),
                    }
                }
            }
        })
    })
    .await
}

async fn scoped_launch_verification(
    pool: sqlx::PgPool,
    region: Region,
    scope: CiReservationScope,
) -> Result<(), ReservationTransitionError> {
    let manifest_store =
        CiDriveManifestStore::new(pool.clone(), scope.tenant.clone(), region.clone())
            .map_err(|_| ReservationTransitionError::Manifest)?;
    let spec_store = CiJobSpecStore::with_pg(pool.clone());
    let scope_tenant = scope.tenant.0.clone();
    with_tenant_tx_error(&pool, &scope_tenant, &region.0, move |connection| {
        Box::pin(async move {
            let (manifest, _) = manifest_store
                .load_by_wf_run_on_conn(connection, &scope.wf_run_id)
                .await
                .map_err(|_| ReservationTransitionError::Manifest)?
                .ok_or(ReservationTransitionError::Refused)?;
            let job = manifest
                .jobs
                .iter()
                .find(|job| job.job_id == scope.job_id)
                .ok_or(ReservationTransitionError::Refused)?;
            verify_executable_authority(connection, &spec_store, &manifest, job, &scope).await?;
            lock_exact_live_claim(connection, &scope, &job.stage).await
        })
    })
    .await
}

#[cfg(any(test, feature = "test-support"))]
async fn cancel_superseded_and_settle(
    pool: sqlx::PgPool,
    region: Region,
    ledger: DurableCostLedger,
    tenant: TenantId,
    concurrency_group: String,
    keep_job_id: String,
) -> Result<Vec<String>, ReservationTransitionError> {
    let keep_job_id = sqlx::types::Uuid::parse_str(&keep_job_id)
        .map_err(|_| ReservationTransitionError::Scope)?;
    let spec_store = CiJobSpecStore::with_pg(pool.clone());
    let tenant_id = tenant.clone();
    let transaction_region = region.clone();
    with_tenant_tx_error(&pool, &tenant.0, &region.0, move |connection| {
        Box::pin(async move {
            let rows = sqlx::query(crate::CANCEL_SUPERSEDED_QUERY)
                .bind(&tenant_id.0)
                .bind(&transaction_region.0)
                .bind(&concurrency_group)
                .bind(keep_job_id)
                .fetch_all(&mut *connection)
                .await
                .map_err(|_| ReservationTransitionError::Scope)?;
            let mut canceled = Vec::with_capacity(rows.len());
            for row in rows {
                let job_id: sqlx::types::Uuid = row.get("job_id");
                let job_id_text = job_id.to_string();
                let identity = spec_store
                    .get_dispatch_identity_on_conn(connection, &tenant_id.0, job_id, &job_id_text)
                    .await
                    .map_err(|_| ReservationTransitionError::Manifest)?
                    .ok_or(ReservationTransitionError::Refused)?;
                let run = CostRunId(identity.reserve_handle);
                let units = tier_p_settlement_units(zero_usage(), &run.0)?;
                ledger
                    .settle_in_tx(connection, &tenant_id, &run, &units)
                    .await
                    .map_err(|_| ReservationTransitionError::Ledger)?;
                canceled.push(job_id_text);
            }
            Ok(canceled)
        })
    })
    .await
}

async fn verify_executable_authority(
    connection: &mut sqlx::PgConnection,
    spec_store: &CiJobSpecStore,
    manifest: &crate::CiDriveManifestV1,
    job: &crate::GrantedCiJobV1,
    scope: &CiReservationScope,
) -> Result<(), ReservationTransitionError> {
    let durable = spec_store
        .get_launch_template_on_conn(connection, &scope.tenant.0, &scope.job_id)
        .await
        .map_err(|_| ReservationTransitionError::Manifest)?;
    if durable.spec != scope.template
        || durable.ci_run_id != manifest.ci_run_id
        || durable.token_authority_handle != job.token_authority_handle
        || !manifest_matches_template(manifest, job, &scope.template)
    {
        return Err(ReservationTransitionError::Refused);
    }
    Ok(())
}

fn manifest_matches_template(
    manifest: &crate::CiDriveManifestV1,
    job: &crate::GrantedCiJobV1,
    template: &JobSpecTemplate,
) -> bool {
    let env_matches = template.env.len() == job.env.len()
        && template
            .env
            .iter()
            .zip(job.env.iter())
            .all(|(actual, (name, value))| actual.name == *name && actual.value == *value);
    let secrets_match = template.secret_refs.len() == job.secret_handles.len()
        && template
            .secret_refs
            .iter()
            .zip(job.secret_handles.iter())
            .all(|(actual, (name, handle))| actual.name == *name && actual.handle == *handle);
    let trust_matches = matches!(
        (manifest.trust_tier, template.trust_tier),
        (
            crate::CiManifestTrustTierV1::Trusted,
            myelin_ci_sandbox::TrustTier::Trusted
        ) | (
            crate::CiManifestTrustTierV1::UntrustedFork,
            myelin_ci_sandbox::TrustTier::UntrustedFork
        ) | (
            crate::CiManifestTrustTierV1::SelfHosted,
            myelin_ci_sandbox::TrustTier::SelfHosted
        )
    );
    template.kind == JobKind::Ci
        && template.image.reference == job.image
        && template.command == job.command
        && env_matches
        && secrets_match
        && template.egress.allow == job.egress_allow
        && template.limits.cpu_millis == job.limits.cpu_millis
        && template.limits.mem_bytes == job.limits.mem_bytes
        && template.limits.disk_bytes == job.limits.disk_bytes
        && template.limits.pids_max == job.limits.pids_max
        && template.limits.timeout_secs == job.limits.timeout_secs
        && template.workspace.repo_ref.as_deref() == Some(job.workspace.repo_ref.as_str())
        && template.workspace.commit.as_deref() == Some(job.workspace.commit_oid.as_str())
        && job.workspace.read_only_root
        && job.workspace.tmpfs_scratch
        && trust_matches
        && template.meter_to.reserve_id == job.reserve_handle
}

async fn lock_exact_live_claim(
    connection: &mut sqlx::PgConnection,
    scope: &CiReservationScope,
    stage: &str,
) -> Result<(), ReservationTransitionError> {
    let row = sqlx::query(
        "SELECT 1 FROM job_queue
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid AND run_id = $4::uuid
           AND idem_token = $5 AND stage = $6 AND trust_tier = $7
           AND state = 'leased' AND lease_owner = $8 AND lease_epoch = $9
           AND claim_nonce = $10::uuid
           AND EXTRACT(EPOCH FROM claim_started_at)::bigint = $11
           AND EXTRACT(EPOCH FROM claim_expires_at)::bigint = $12
           AND claim_expires_at > statement_timestamp()
           AND completion_receipt IS NULL
         FOR UPDATE",
    )
    .bind(&scope.tenant.0)
    .bind(&scope.region)
    .bind(&scope.job_id)
    .bind(&scope.wf_run_id)
    .bind(&scope.idem_token)
    .bind(stage)
    .bind(&scope.trust_tier)
    .bind(&scope.lease_owner)
    .bind(scope.lease_epoch)
    .bind(&scope.claim_nonce)
    .bind(scope.claim_started_at_epoch_secs)
    .bind(scope.claim_expires_at_epoch_secs)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|_| ReservationTransitionError::Scope)?;
    row.map(|_| ()).ok_or(ReservationTransitionError::Refused)
}

#[derive(Clone, Copy)]
enum ReleaseDisposition {
    CanceledBeforeLaunch,
    RetryOrTerminalOwner,
}

async fn lock_release_disposition(
    connection: &mut sqlx::PgConnection,
    scope: &CiReservationScope,
) -> Result<ReleaseDisposition, ReservationTransitionError> {
    let row = sqlx::query(
        "SELECT state, lease_owner, lease_epoch, claim_nonce::text AS claim_nonce,
                EXTRACT(EPOCH FROM claim_started_at)::bigint AS claim_started_at_epoch_secs,
                EXTRACT(EPOCH FROM claim_expires_at)::bigint AS claim_expires_at_epoch_secs,
                completion_receipt
         FROM job_queue
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid AND run_id = $4::uuid
         FOR UPDATE",
    )
    .bind(&scope.tenant.0)
    .bind(&scope.region)
    .bind(&scope.job_id)
    .bind(&scope.wf_run_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|_| ReservationTransitionError::Scope)?
    .ok_or(ReservationTransitionError::Refused)?;
    let state: String = row.get("state");
    let lease_owner: Option<String> = row.get("lease_owner");
    let lease_epoch: i64 = row.get("lease_epoch");
    let claim_nonce: Option<String> = row.get("claim_nonce");
    let claim_started_at: Option<i64> = row.get("claim_started_at_epoch_secs");
    let claim_expires_at: Option<i64> = row.get("claim_expires_at_epoch_secs");
    let completion_receipt: Option<String> = row.get("completion_receipt");
    let same_generation = lease_epoch == scope.lease_epoch
        && claim_nonce.as_deref() == Some(scope.claim_nonce.as_str())
        && claim_started_at == Some(scope.claim_started_at_epoch_secs)
        && claim_expires_at == Some(scope.claim_expires_at_epoch_secs);
    if same_generation
        && state == "terminal"
        && lease_owner.is_none()
        && completion_receipt.is_none()
    {
        Ok(ReleaseDisposition::CanceledBeforeLaunch)
    } else {
        // A live/replacement generation, an acknowledgement-lost launch CAS, and a completed job
        // all retain the deterministic reservation. The reaper/retry or terminal reporter owns it.
        Ok(ReleaseDisposition::RetryOrTerminalOwner)
    }
}

fn tier_p_settlement_units(
    usage: ResourceUsage,
    reserve_handle: &str,
) -> Result<Vec<MeteredUnit>, ReservationTransitionError> {
    let priced = TierPOperationalCiJobPricer
        .price(usage)
        .map_err(|_| ReservationTransitionError::Pricing)?;
    validate_reservation_pricing_policy(reserve_handle, usage, &priced)
        .map_err(|_| ReservationTransitionError::Pricing)?;
    Ok(vec![
        MeteredUnit {
            unit: Meter::CpuSeconds.token(),
            wholesale: priced.cpu_wholesale,
            markup: priced.cpu_markup,
        },
        MeteredUnit {
            unit: Meter::MemGbSeconds.token(),
            wholesale: priced.memory_wholesale,
            markup: priced.memory_markup,
        },
    ])
}

const fn zero_usage() -> ResourceUsage {
    ResourceUsage {
        cpu_seconds: 0,
        mem_byte_seconds: 0,
    }
}

fn bridge<F: std::future::Future>(rt: &tokio::runtime::Handle, future: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(_) => tokio::task::block_in_place(|| rt.block_on(future)),
        Err(_) => rt.block_on(future),
    }
}

/// Recover the cell's sealed signing root and compose one shared durable S7 lifecycle into the real
/// PASETO claim minter and verifier.
///
/// The returned issuer first locks and reconstructs the exact scheduler/run/manifest authority. The
/// returned authorizer verifies the resulting signed credential and performs the one-shot durable
/// scheduler-generation launch CAS immediately before sandbox spawn.
pub async fn ci_runner_identity_authorities(
    provider: SubstrateProvider,
    cell_id: impl Into<String>,
    seal_key: &SealKey,
    rt: tokio::runtime::Handle,
) -> Result<CiRunnerIdentityAuthorities, CiRunnerIdentityCompositionError> {
    let cell_id = cell_id.into();
    if !valid_cell_id(&cell_id) {
        return Err(CiRunnerIdentityCompositionError::InvalidCellId);
    }

    let material = DurableCellRootBacking::new(provider.db_pool().clone(), cell_id)
        .load_or_generate(seal_key)
        .await
        .map_err(|_| CiRunnerIdentityCompositionError::DurableCellRootUnavailable)?;
    let cell = Arc::new(
        CellTokenAuthority::from_material(&material)
            .map_err(|_| CiRunnerIdentityCompositionError::InvalidCellRoot)?,
    );
    let revocations =
        RevocationStore::with_pg(DurableRevocationBacking::new(provider.clone()), rt.clone());
    let signer = Arc::new(PasetoCapabilitySigner::new(cell.clone()));
    let minter = RunTokenMinter::with_signer_and_tuples(revocations.clone(), None, signer);
    let verifier = Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor()));
    let region = provider.config().region.clone();
    // CT-007 slice 5b.3-6e.2: build the Identity phase-credential minter ONCE and share it — the V1
    // token issuer and (via `ci_runner_v2_wiring`) the V2 checkout composition mint through the SAME
    // `IdentityCiJobCredentialMinter`, never two divergent Identities.
    let phase_credential_minter = Arc::new(IdentityCiJobCredentialMinter::new(minter));
    let token_issuer = LockedManifestCiJobTokenIssuer::new(
        provider.db_pool().clone(),
        region.clone(),
        phase_credential_minter.clone(),
    );
    let launch_authorizer = Arc::new(IdentityCiJobLaunchAuthorizer::new(
        RunTokenAuthorizer::new(verifier, revocations),
        CiJobQueueStore::with_pg(provider.db_pool().clone()),
        myelin_tenancy::Region(region),
        rt,
    ));

    Ok(CiRunnerIdentityAuthorities {
        token_issuer,
        launch_authorizer,
        phase_credential_minter,
    })
}

fn valid_cell_id(cell_id: &str) -> bool {
    !cell_id.is_empty()
        && cell_id.trim() == cell_id
        && cell_id.len() <= 128
        && !cell_id.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::{valid_cell_id, valid_reserve_handle};

    #[test]
    fn cell_id_is_canonical_before_durable_root_lookup() {
        assert!(valid_cell_id("cell-eu-1"));
        assert!(!valid_cell_id(""));
        assert!(!valid_cell_id(" cell-eu-1"));
        assert!(!valid_cell_id("cell-eu-1 "));
        assert!(!valid_cell_id("cell-\neu-1"));
        assert!(!valid_cell_id(&"x".repeat(129)));
    }

    #[test]
    fn reserve_handle_accepts_both_v1_and_v2_prefixes() {
        // CT-007 slice 5b.3-4a.1b: the inbound reservation-scope format validator must recognize a
        // `v2` handle exactly as readily as a `v1` one -- otherwise every `v2`-reserved job would be
        // refused at claim time before it ever reaches the launch hook.
        assert!(valid_reserve_handle("ci-reserve:v1:run:batch:job:item"));
        assert!(valid_reserve_handle(
            "ci-reserve:v2:run:budget-v1:a5:batch:job:item"
        ));
        assert!(!valid_reserve_handle("ci-reserve:v3:run:job"));
        assert!(!valid_reserve_handle("reserve:job"));
        assert!(!valid_reserve_handle(&format!(
            "ci-reserve:v2:{}",
            "x".repeat(600)
        )));
        assert!(!valid_reserve_handle("ci-reserve:v2:run:job\ncontrol"));
    }
}

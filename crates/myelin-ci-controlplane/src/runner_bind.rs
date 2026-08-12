use std::sync::Arc;
use std::time::Duration;

use myelin_ci_sandbox::asset_registry::{
    CargoVendorAssetBinding, GvisorAssetRegistry, RootfsAssetBinding,
};
use myelin_ci_sandbox::gvisor::GvisorBackend;
use myelin_ci_sandbox::{
    derive_checkout_authorization_scope, resolved_gvisor_cargo_vendor,
    resolved_gvisor_cargo_vendor_workspace, resolved_gvisor_rootfs, resolved_gvisor_rust_rootfs,
    verified_gvisor_git_rootfs, ImageRef, JobKind, JobSpec, LeaseStore, QueuedJob, RunnerError,
    RunnerHooks, TrustTier, CARGO_VENDOR_SMOKE_LOCK_SHA256, CARGO_VENDOR_SMOKE_TREE_SHA256,
    CARGO_VENDOR_WORKSPACE_LOCK_SHA256, CARGO_VENDOR_WORKSPACE_TREE_SHA256,
    GVISOR_GIT_ROOTFS_SHA256, LINUX_RUST_V1_ROOTFS_SHA256, LINUX_SMALL_V1_ROOTFS_SHA256,
};
use myelin_storage::s3blob::S3BlobStore;
use myelin_tenancy::{Region, TenantId};

pub fn production_gvisor_registry() -> Result<Arc<GvisorAssetRegistry>, String> {
    let image = |name: &str, digest: &str| {
        ImageRef::pinned(format!("myelin.local/{name}@sha256:{digest}"))
            .map_err(|error| format!("invalid production asset reference `{name}`: {error}"))
    };
    let git_rootfs = verified_gvisor_git_rootfs()
        .map_err(|error| format!("verify production git rootfs: {error}"))?;
    GvisorAssetRegistry::from_bindings_with_cargo_vendor(
        vec![
            RootfsAssetBinding {
                image: image("linux-small-v1-rootfs", LINUX_SMALL_V1_ROOTFS_SHA256)?,
                rootfs: resolved_gvisor_rootfs(),
            },
            RootfsAssetBinding {
                image: image("linux-rust-v1-rootfs", LINUX_RUST_V1_ROOTFS_SHA256)?,
                rootfs: resolved_gvisor_rust_rootfs(),
            },
            RootfsAssetBinding {
                image: image("git-v1-rootfs", GVISOR_GIT_ROOTFS_SHA256)?,
                rootfs: git_rootfs,
            },
        ],
        vec![
            CargoVendorAssetBinding {
                reference: image("cargo-vendor-smoke-v1", CARGO_VENDOR_SMOKE_TREE_SHA256)?,
                root: resolved_gvisor_cargo_vendor(),
                cargo_lock_sha256: CARGO_VENDOR_SMOKE_LOCK_SHA256.to_string(),
            },
            CargoVendorAssetBinding {
                reference: image(
                    "cargo-vendor-workspace-v1",
                    CARGO_VENDOR_WORKSPACE_TREE_SHA256,
                )?,
                root: resolved_gvisor_cargo_vendor_workspace(),
                cargo_lock_sha256: CARGO_VENDOR_WORKSPACE_LOCK_SHA256.to_string(),
            },
        ],
    )
    .map(Arc::new)
    .map_err(|error| format!("verify production runner asset registry: {error}"))
}

use crate::ci_claim_token_issuer::LockedManifestCiJobTokenIssuer;
use crate::ci_identity_adapter::ci_job_authorization_context;
use crate::ci_manifest_job_runner::{
    resolve_claim_launch_secrets, secret_withhold_machine_reason, validate_run_token,
    CiJobSecretResolver, CiJobTokenIssuer, CiJobTokenRequest,
};
use crate::ci_pipeline_reporter_router::CiPipelineReporterRouter;
use crate::job_spec_store::MAX_JOB_TIMEOUT_SECS;
use crate::{
    CiJobQueueStore, CiJobSpecStore, CiRegionQueueStore, DurableLogPersist, LeasedJob,
    LogPipelineSink,
};

pub const CI_RUNNER_EXECUTION_LEASE_TTL_SECS_U64: u64 = MAX_JOB_TIMEOUT_SECS as u64 + 600;
pub const CI_RUNNER_EXECUTION_LEASE_TTL_SECS: i64 = CI_RUNNER_EXECUTION_LEASE_TTL_SECS_U64 as i64;

pub type JobSpecResolver = Arc<dyn Fn(&LeasedJob) -> Result<JobSpec, String> + Send + Sync>;

trait SecretWithholdTerminalizer {
    fn terminalize(&self, claim: &CiJobTokenRequest, diagnostic: &str) -> Result<(), String>;
}

impl SecretWithholdTerminalizer for CiPipelineReporterRouter {
    fn terminalize(&self, claim: &CiJobTokenRequest, diagnostic: &str) -> Result<(), String> {
        use myelin_ci_sandbox::{
            PreparationPhase, PreparationReportClaim, PreparationTerminalDisposition,
            TerminalReporter,
        };

        let report_claim = PreparationReportClaim {
            tenant_id: claim.tenant_id.clone(),
            region: claim.region.clone(),
            project_id: claim.project_id.clone(),
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
        };
        self.report_preparation_terminal(
            &report_claim,
            PreparationTerminalDisposition::Failed {
                phase: PreparationPhase::SecretResolution,
            },
            Some(diagnostic),
        )
        .map(|_| ())
        .map_err(|error| format!("terminal secret-withhold settlement failed: {error}"))
    }
}

fn finish_claim_secret_resolution(
    resolution: Result<JobSpec, crate::SecretLaunchError>,
    claim: &CiJobTokenRequest,
    terminalizer: &impl SecretWithholdTerminalizer,
) -> Result<JobSpec, String> {
    match resolution {
        Ok(spec) => Ok(spec),
        Err(crate::SecretLaunchError::Withheld(withheld)) => {
            let diagnostic = secret_withhold_machine_reason(&withheld);
            terminalizer.terminalize(claim, &diagnostic)?;
            Err(format!(
                "secret-bearing claim settled terminally: {diagnostic}"
            ))
        }
        Err(error) => Err(error.to_string()),
    }
}

fn finish_v1_claim_secret_resolution(
    resolution: Result<JobSpec, crate::SecretLaunchError>,
    claim: &CiJobTokenRequest,
    terminalizer: &impl SecretWithholdTerminalizer,
) -> Result<JobSpec, String> {
    finish_claim_secret_resolution(resolution, claim, terminalizer)
}

pub(crate) fn bridge<F: std::future::Future>(rt: &tokio::runtime::Handle, fut: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(_) => tokio::task::block_in_place(|| rt.block_on(fut)),
        Err(_) => rt.block_on(fut),
    }
}

pub struct DurableLeaseAdapter {
    region_store: CiRegionQueueStore,
    tenant_store: CiJobQueueStore,
    region: String,
    rt: tokio::runtime::Handle,
    resolve: JobSpecResolver,
}

impl DurableLeaseAdapter {
    pub fn new(
        region_store: CiRegionQueueStore,
        tenant_store: CiJobQueueStore,
        region: impl Into<String>,
        rt: tokio::runtime::Handle,
        resolve: JobSpecResolver,
    ) -> DurableLeaseAdapter {
        DurableLeaseAdapter {
            region_store,
            tenant_store,
            region: region.into(),
            rt,
            resolve,
        }
    }
}

impl LeaseStore for DurableLeaseAdapter {
    fn claim_for_labels(
        &self,
        worker: &str,
        runner_labels: &[String],
        allowed_tiers: &[TrustTier],
        region: &Region,
        now: i64,
        lease_ttl_secs: i64,
    ) -> Option<QueuedJob> {
        let ttl = lease_ttl_secs.max(0) as u64;
        let claimed = bridge(
            &self.rt,
            self.region_store
                .claim(&region.0, runner_labels, allowed_tiers, worker, ttl),
        );
        let leased = match claimed {
            Ok(Some(l)) => l,
            Ok(None) => return None,
            Err(e) => {
                eprintln!(
                    "ci-runner[{worker}]: durable claim FAILED in region `{}` (no launch; will \
                     retry): {e}",
                    region.0
                );
                return None;
            }
        };
        let spec = match (self.resolve)(&leased) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "ci-runner[{worker}]: leased job {} has no resolvable JobSpec (CT-004d spec \
                     store); leaving it leased for the reaper: {e}",
                    leased.job_id
                );
                return None;
            }
        };
        Some(QueuedJob {
            tenant: TenantId(leased.tenant_id.clone()),
            region: region.clone(),
            run_id: leased.run_id.to_string(),
            job_id: leased.job_id.to_string(),
            labels: runner_labels.to_vec(),
            spec,
            lease_owner: Some(worker.to_string()),
            lease_expires: Some(now + lease_ttl_secs),
            lease_epoch: leased.lease_epoch,
            claim_nonce: leased.claim_nonce,
        })
    }

    fn heartbeat(
        &self,
        worker: &str,
        tenant: &TenantId,
        job_id: &str,
        _now: i64,
        lease_ttl_secs: i64,
    ) -> bool {
        let ttl = lease_ttl_secs.max(0) as u64;
        match bridge(
            &self.rt,
            self.tenant_store
                .heartbeat(&tenant.0, &self.region, job_id, worker, ttl),
        ) {
            Ok(extended) => extended,
            Err(e) => {
                eprintln!(
                    "ci-runner[{worker}]: heartbeat FAILED for job {job_id} (treating as \
                     lease-lost, fail-closed): {e}"
                );
                false
            }
        }
    }

    fn settle(&self, tenant: &TenantId, job_id: &str) {
        if let Err(e) = bridge(
            &self.rt,
            self.tenant_store.complete(&tenant.0, &self.region, job_id),
        ) {
            eprintln!(
                "ci-runner: settle/complete FAILED for job {job_id} in region `{}` (the job.done \
                 idempotency still holds; the reaper reconciles): {e}",
                self.region
            );
        }
    }
}

pub struct CiRunnerLoop {
    worker_id: String,
    labels: Vec<String>,
    allowed_tiers: Vec<TrustTier>,
    region: String,
    lease_ttl_secs: i64,
    region_store: CiRegionQueueStore,
    tenant_store: CiJobQueueStore,
    rt: tokio::runtime::Handle,
    resolve: JobSpecResolver,
    reporter: CiPipelineReporterRouter,
    hooks: RunnerHooks,
    idle_backoff: Duration,
    error_backoff: Duration,
    pool: sqlx::postgres::PgPool,
    s3: myelin_config::S3Config,
    gvisor_workspace_config: myelin_ci_sandbox::gvisor::GvisorWorkspaceConfig,
    gvisor_checkout_config: myelin_ci_sandbox::gvisor::GvisorCheckoutConfig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiRunnerLoopExit {
    Shutdown,
    SettlementOwnerMismatch,
    TerminalReportFailed,
    SandboxBackendInitializationFailed,
}

impl CiRunnerLoop {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        worker_id: impl Into<String>,
        labels: Vec<String>,
        allowed_tiers: Vec<TrustTier>,
        region: impl Into<String>,
        lease_ttl_secs: i64,
        region_store: CiRegionQueueStore,
        tenant_store: CiJobQueueStore,
        rt: tokio::runtime::Handle,
        resolve: JobSpecResolver,
        reporter: CiPipelineReporterRouter,
        hooks: RunnerHooks,
        pool: sqlx::postgres::PgPool,
        s3: myelin_config::S3Config,
        gvisor_workspace_config: myelin_ci_sandbox::gvisor::GvisorWorkspaceConfig,
        gvisor_checkout_config: myelin_ci_sandbox::gvisor::GvisorCheckoutConfig,
    ) -> CiRunnerLoop {
        CiRunnerLoop {
            worker_id: worker_id.into(),
            labels,
            allowed_tiers,
            region: region.into(),
            lease_ttl_secs,
            region_store,
            tenant_store,
            rt,
            resolve,
            reporter,
            hooks,
            pool,
            s3,
            gvisor_workspace_config,
            gvisor_checkout_config,
            idle_backoff: Duration::from_millis(500),
            error_backoff: Duration::from_secs(2),
        }
    }

    pub fn with_backoff(mut self, idle: Duration, error: Duration) -> CiRunnerLoop {
        self.idle_backoff = idle;
        self.error_backoff = error;
        self
    }

    pub(crate) fn try_spawn_until_shutdown(
        self,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> std::io::Result<std::thread::JoinHandle<CiRunnerLoopExit>> {
        std::thread::Builder::new()
            .name("ci-runner".into())
            .spawn(move || self.run_until_shutdown(shutdown))
    }

    pub fn run(self) -> CiRunnerLoopExit {
        let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        self.run_until_shutdown(shutdown_rx)
    }

    pub fn run_until_shutdown(
        self,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> CiRunnerLoopExit {
        if runner_shutdown_requested(&mut shutdown) {
            return CiRunnerLoopExit::Shutdown;
        }
        let CiRunnerLoop {
            worker_id,
            labels,
            allowed_tiers,
            region,
            lease_ttl_secs,
            region_store,
            tenant_store,
            rt,
            resolve,
            reporter,
            hooks,
            pool,
            s3,
            gvisor_workspace_config,
            gvisor_checkout_config,
            idle_backoff,
            error_backoff,
        } = self;

        let incident_sink: myelin_ci_sandbox::workspace_manager::IncidentSink = {
            let worker_id = worker_id.clone();
            Arc::new(move |message: &str| {
                eprintln!("ci-runner[{worker_id}]: GVISOR SECURITY INCIDENT: {message}");
            })
        };
        let registry = match production_gvisor_registry() {
            Ok(registry) => registry,
            Err(error) => {
                eprintln!(
                    "ci-runner[{worker_id}]: sandbox asset initialization FAILED (fail-closed, \
                     no claim attempted): {error}"
                );
                return CiRunnerLoopExit::SandboxBackendInitializationFailed;
            }
        };
        let backend = match GvisorBackend::try_new(registry, gvisor_workspace_config, incident_sink)
        {
            Ok(backend) => backend.with_checkout_config(gvisor_checkout_config),
            Err(error) => {
                eprintln!(
                    "ci-runner[{worker_id}]: sandbox backend initialization FAILED (fail-closed, \
                     no claim attempted): {error}"
                );
                return CiRunnerLoopExit::SandboxBackendInitializationFailed;
            }
        };
        let firehose = LogPipelineSink::new(
            Region(region.clone()),
            S3BlobStore::connect(&s3, rt.clone()),
            DurableLogPersist::with_pg(pool, rt.clone()),
        );
        let adapter =
            DurableLeaseAdapter::new(region_store, tenant_store, region.clone(), rt, resolve);
        let agent = myelin_ci_sandbox::RunnerAgent::new(
            worker_id.clone(),
            labels,
            allowed_tiers,
            Region(region.clone()),
            lease_ttl_secs,
            adapter,
            &backend,
            &firehose,
            &reporter,
            hooks,
        );

        eprintln!(
            "ci-runner[{worker_id}]: started (region `{region}`, lease TTL {lease_ttl_secs}s) - \
             claiming from the durable job_queue + executing in gVisor (AG-D4)"
        );
        loop {
            if runner_shutdown_requested(&mut shutdown) {
                return CiRunnerLoopExit::Shutdown;
            }
            match agent.run_one_cycle(now_secs()) {
                Ok(myelin_ci_sandbox::RunnerCycleOutcome::Workload(o)) => {
                    eprintln!(
                        "ci-runner[{worker_id}]: ran job {} for run {} (passed={}, job.done={:?})",
                        o.job_id, o.run_id, o.report.passed, o.signal_outcome
                    );
                }
                Ok(myelin_ci_sandbox::RunnerCycleOutcome::PreparationTerminal {
                    job_id,
                    run_id,
                    signal_outcome,
                    diagnostic,
                }) => match diagnostic {
                    Some(diagnostic) => eprintln!(
                        "ci-runner[{worker_id}]: preparation terminalized job {job_id} for run \
                             {run_id} (job.done={signal_outcome:?}, diagnostic={diagnostic})"
                    ),
                    None => eprintln!(
                        "ci-runner[{worker_id}]: preparation terminalized job {job_id} for run \
                             {run_id} (job.done={signal_outcome:?})"
                    ),
                },
                Ok(myelin_ci_sandbox::RunnerCycleOutcome::PreparationRetryable {
                    job_id,
                    report,
                }) => {
                    eprintln!(
                        "ci-runner[{worker_id}]: preparation requeued job {job_id} ({report:?})"
                    );
                }
                Err(RunnerError::NoWork) => {
                    if runner_sleep_until_shutdown(&mut shutdown, idle_backoff) {
                        return CiRunnerLoopExit::Shutdown;
                    }
                }
                Err(RunnerError::LeaseLost { job_id }) => {
                    eprintln!(
                        "ci-runner[{worker_id}]: lease LOST for job {job_id} mid-claim - retrying \
                         (no double-run)"
                    );
                }
                Err(e @ RunnerError::LaunchFailed(_)) => {
                    eprintln!(
                        "ci-runner[{worker_id}]: launch FAILED (fail-closed, no terminal report; \
                         the dispatch retries): {e}"
                    );
                    if runner_sleep_until_shutdown(&mut shutdown, error_backoff) {
                        return CiRunnerLoopExit::Shutdown;
                    }
                }
                Err(e @ RunnerError::RetryableAttemptRecorded { .. }) => {
                    eprintln!("ci-runner[{worker_id}]: {e}");
                    if runner_sleep_until_shutdown(&mut shutdown, error_backoff) {
                        return CiRunnerLoopExit::Shutdown;
                    }
                }
                Err(e @ RunnerError::ReportFailed(_)) => {
                    eprintln!(
                        "ci-runner[{worker_id}]: terminal report FAILED; stopping host intake so the \
                         durable claim can be recovered: {e}"
                    );
                    return CiRunnerLoopExit::TerminalReportFailed;
                }
                Err(e @ RunnerError::SettlementOwnerMismatch { .. }) => {
                    eprintln!("ci-runner[{worker_id}]: CONFIGURATION REFUSED: {e}");
                    return CiRunnerLoopExit::SettlementOwnerMismatch;
                }
                Err(
                    e @ (RunnerError::PreparationRoutingFailed { .. }
                    | RunnerError::ReconciliationRequired { .. }),
                ) => {
                    eprintln!(
                        "ci-runner[{worker_id}]: checkout recovery REQUIRED; stopping host intake: {e}"
                    );
                    return CiRunnerLoopExit::TerminalReportFailed;
                }
            }
        }
    }
}

pub const RUNNER_SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(50);

fn runner_shutdown_requested(shutdown: &mut tokio::sync::watch::Receiver<bool>) -> bool {
    if *shutdown.borrow_and_update() {
        return true;
    }
    match shutdown.has_changed() {
        Ok(true) => *shutdown.borrow_and_update(),
        Ok(false) => false,
        Err(_) => true,
    }
}

fn runner_sleep_until_shutdown(
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    duration: Duration,
) -> bool {
    let started = std::time::Instant::now();
    loop {
        if runner_shutdown_requested(shutdown) {
            return true;
        }
        let remaining = duration.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return false;
        }
        std::thread::sleep(remaining.min(RUNNER_SHUTDOWN_POLL_INTERVAL));
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn spec_store_unavailable_resolver() -> JobSpecResolver {
    Arc::new(|leased: &LeasedJob| {
        Err(format!(
            "no durable JobSpec store yet (CT-004d) for job {}; runner cannot launch an unresolved \
             job - leaving it leased for the reaper",
            leased.job_id
        ))
    })
}

pub fn durable_spec_resolver(
    store: CiJobSpecStore,
    region: impl Into<String>,
    rt: tokio::runtime::Handle,
    token_issuer: LockedManifestCiJobTokenIssuer,
    secrets: CiJobSecretResolver,
    secret_terminal_reporter: CiPipelineReporterRouter,
) -> JobSpecResolver {
    durable_spec_resolver_with_issuer(
        store,
        region,
        rt,
        Arc::new(token_issuer),
        secrets,
        secret_terminal_reporter,
    )
}

#[cfg(any(test, feature = "test-support"))]
pub fn durable_spec_resolver_test_support(
    store: CiJobSpecStore,
    region: impl Into<String>,
    rt: tokio::runtime::Handle,
    token_issuer: Arc<dyn CiJobTokenIssuer>,
    secrets: CiJobSecretResolver,
    secret_terminal_reporter: CiPipelineReporterRouter,
) -> JobSpecResolver {
    durable_spec_resolver_with_issuer(
        store,
        region,
        rt,
        token_issuer,
        secrets,
        secret_terminal_reporter,
    )
}

fn durable_spec_resolver_with_issuer(
    store: CiJobSpecStore,
    region: impl Into<String>,
    rt: tokio::runtime::Handle,
    token_issuer: Arc<dyn CiJobTokenIssuer>,
    secrets: CiJobSecretResolver,
    secret_terminal_reporter: CiPipelineReporterRouter,
) -> JobSpecResolver {
    let region = region.into();
    Arc::new(move |leased: &LeasedJob| {
        let launch = bridge(
            &rt,
            store.get_launch_template(&leased.tenant_id, &region, &leased.job_id.to_string()),
        )
        .map_err(|e| e.to_string())?;
        if launch.spec.trust_tier != leased.trust_tier {
            return Err("claimed trust tier differs from the durable launch template".into());
        }
        if leased.claim_window_secs.is_none()
            && crate::ci_claim_window::is_checkout_bearing(launch.spec.kind, &launch.spec.workspace)
                .map_err(|e| e.to_string())?
        {
            return Err(
                "checkout-bearing job was claimed on a legacy row with no durable claim window; \
                 refusing before mint (its claim would expire mid-preparation)"
                    .into(),
            );
        }
        let request = CiJobTokenRequest {
            tenant_id: leased.tenant_id.clone(),
            region: region.clone(),
            project_id: launch.project_id.clone(),
            wf_run_id: leased.run_id.to_string(),
            ci_run_id: launch.ci_run_id,
            job_id: leased.job_id.to_string(),
            token_authority_handle: launch.token_authority_handle.clone(),
            idem_token: launch.spec.idem_token.0.clone(),
            lease_owner: leased.lease_owner.clone(),
            lease_epoch: leased.lease_epoch,
            claim_nonce: leased.claim_nonce.clone(),
            claim_started_at_epoch_secs: leased.claim_started_at_epoch_secs,
            claim_expires_at_epoch_secs: leased.claim_expires_at_epoch_secs,
        };
        request.validate().map_err(|e| e.to_string())?;
        let checkout = derive_checkout_authorization_scope(JobKind::Ci, &launch.spec.workspace)
            .map_err(|e| e.to_string())?;
        let authorization = ci_job_authorization_context(
            &request,
            &launch.spec.meter_to.reserve_id,
            checkout.as_ref(),
        );
        let run_token =
            bridge(&rt, token_issuer.mint(request.clone())).map_err(|e| e.to_string())?;
        validate_run_token(&run_token, &launch.token_authority_handle).map_err(|e| e.0)?;
        let resolution = resolve_claim_launch_secrets(
            &TenantId(leased.tenant_id.clone()),
            launch.spec,
            run_token,
            authorization,
            &secrets,
        );
        finish_v1_claim_secret_resolution(resolution, &request, &secret_terminal_reporter)
    })
}

pub fn durable_v2_spec_resolver(
    store: CiJobSpecStore,
    region: impl Into<String>,
    rt: tokio::runtime::Handle,
    checkout_composition: crate::ci_checkout_composition::V2CheckoutComposition,
    secrets: CiJobSecretResolver,
    secret_terminal_reporter: CiPipelineReporterRouter,
) -> JobSpecResolver {
    let region = region.into();
    Arc::new(move |leased: &LeasedJob| {
        let launch = bridge(
            &rt,
            store.get_launch_template(&leased.tenant_id, &region, &leased.job_id.to_string()),
        )
        .map_err(|e| e.to_string())?;
        if launch.spec.trust_tier != leased.trust_tier {
            return Err("claimed trust tier differs from the durable launch template".into());
        }
        if leased.claim_window_secs.is_none()
            && crate::ci_claim_window::is_checkout_bearing(launch.spec.kind, &launch.spec.workspace)
                .map_err(|e| e.to_string())?
        {
            return Err(
                "checkout-bearing job was claimed on a legacy row with no durable claim window; \
                 refusing before mint (its claim would expire mid-preparation)"
                    .into(),
            );
        }
        let request = CiJobTokenRequest {
            tenant_id: leased.tenant_id.clone(),
            region: region.clone(),
            project_id: launch.project_id.clone(),
            wf_run_id: leased.run_id.to_string(),
            ci_run_id: launch.ci_run_id,
            job_id: leased.job_id.to_string(),
            token_authority_handle: launch.token_authority_handle.clone(),
            idem_token: launch.spec.idem_token.0.clone(),
            lease_owner: leased.lease_owner.clone(),
            lease_epoch: leased.lease_epoch,
            claim_nonce: leased.claim_nonce.clone(),
            claim_started_at_epoch_secs: leased.claim_started_at_epoch_secs,
            claim_expires_at_epoch_secs: leased.claim_expires_at_epoch_secs,
        };
        request.validate().map_err(|e| e.to_string())?;
        let checkout_scope =
            derive_checkout_authorization_scope(JobKind::Ci, &launch.spec.workspace)
                .map_err(|e| e.to_string())?;
        let (minted, authorization) = checkout_composition
            .mint_initial_phase_credential(
                &request,
                &launch.spec.meter_to.reserve_id,
                checkout_scope.as_ref(),
            )
            .map_err(|e| e.to_string())?;
        validate_run_token(&minted.credential, &launch.token_authority_handle).map_err(|e| e.0)?;
        let resolution = resolve_claim_launch_secrets(
            &TenantId(leased.tenant_id.clone()),
            launch.spec,
            minted.credential,
            authorization,
            &secrets,
        );
        finish_claim_secret_resolution(resolution, &request, &secret_terminal_reporter)
    })
}

pub struct DurablePreparationLeaseCheckpoint {
    store: CiJobQueueStore,
    claim: crate::job_queue_store::CiJobLaunchClaim,
    rt: tokio::runtime::Handle,
}

impl DurablePreparationLeaseCheckpoint {
    pub fn new(
        store: CiJobQueueStore,
        claim: crate::job_queue_store::CiJobLaunchClaim,
        rt: tokio::runtime::Handle,
    ) -> DurablePreparationLeaseCheckpoint {
        DurablePreparationLeaseCheckpoint { store, claim, rt }
    }
}

impl myelin_ci_sandbox::PreparationLeaseCheckpoint for DurablePreparationLeaseCheckpoint {
    fn renew(&self) -> Result<(), myelin_ci_sandbox::PreparationLeaseLost> {
        match bridge(&self.rt, self.store.renew_preparation_lease(&self.claim)) {
            Ok(true) => Ok(()),
            Ok(false) => Err(myelin_ci_sandbox::PreparationLeaseLost(format!(
                "no live leased generation matched job {} epoch {} nonce {}",
                self.claim.job_id, self.claim.lease_epoch, self.claim.claim_nonce
            ))),
            Err(error) => Err(myelin_ci_sandbox::PreparationLeaseLost(format!(
                "renewal query failed (treated as lost ownership, fail-closed): {error}"
            ))),
        }
    }
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;

    #[test]
    fn pre_signalled_shutdown_interrupts_backoff_without_sleeping() {
        let (_sender, mut receiver) = tokio::sync::watch::channel(true);
        let started = std::time::Instant::now();
        assert!(runner_sleep_until_shutdown(
            &mut receiver,
            Duration::from_secs(2)
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn sender_closure_is_shutdown() {
        let (sender, mut receiver) = tokio::sync::watch::channel(false);
        drop(sender);
        assert!(runner_shutdown_requested(&mut receiver));
    }
}

#[cfg(test)]
mod secret_withhold_terminal_tests {
    use super::*;
    use crate::{SecretLaunchError, WithheldSecret, WithholdReason};
    use myelin_ci_sandbox::{
        CiJobAuthorizationContext, EgressPolicy, IdemToken, JobSpecTemplate, MeterTarget,
        ResourceLimits, RunTokenAuthorizationContext, RunTokenCredential, SecretRef, WorkspaceSpec,
    };
    use std::sync::Mutex;

    struct RecordingTerminalizer {
        terminal: Mutex<bool>,
        failed_job_done: Mutex<bool>,
        accounting_settled: Mutex<bool>,
        reaper_eligible: Mutex<bool>,
        diagnostic: Mutex<Option<String>>,
    }

    impl Default for RecordingTerminalizer {
        fn default() -> Self {
            Self {
                terminal: Mutex::new(false),
                failed_job_done: Mutex::new(false),
                accounting_settled: Mutex::new(false),
                reaper_eligible: Mutex::new(true),
                diagnostic: Mutex::new(None),
            }
        }
    }

    impl SecretWithholdTerminalizer for RecordingTerminalizer {
        fn terminalize(&self, _claim: &CiJobTokenRequest, diagnostic: &str) -> Result<(), String> {
            *self.terminal.lock().unwrap() = true;
            *self.failed_job_done.lock().unwrap() = true;
            *self.accounting_settled.lock().unwrap() = true;
            *self.reaper_eligible.lock().unwrap() = false;
            *self.diagnostic.lock().unwrap() = Some(diagnostic.to_owned());
            Ok(())
        }
    }

    fn claim() -> CiJobTokenRequest {
        CiJobTokenRequest {
            tenant_id: "acme".into(),
            region: "fr-par".into(),
            project_id: "55555555-5555-4555-8555-555555555555".into(),
            wf_run_id: "10000000-0000-0000-0000-000000000001".into(),
            ci_run_id: "20000000-0000-0000-0000-000000000001".into(),
            job_id: "30000000-0000-0000-0000-000000000001".into(),
            token_authority_handle: "authority:job".into(),
            idem_token: "idem:job".into(),
            lease_owner: "runner:1".into(),
            lease_epoch: 1,
            claim_nonce: "40000000-0000-0000-0000-000000000001".into(),
            claim_started_at_epoch_secs: 1_000,
            claim_expires_at_epoch_secs: 1_300,
        }
    }

    #[test]
    fn unavailable_secret_resolution_is_terminal_and_not_claimable_again() {
        let terminalizer = RecordingTerminalizer::default();
        let resolution = Err(SecretLaunchError::Withheld(vec![WithheldSecret {
            name: "DEPLOY_KEY".into(),
            reason: WithholdReason::CapabilityUnavailable,
        }]));

        let error = finish_claim_secret_resolution(resolution, &claim(), &terminalizer)
            .expect_err("withhold settles failed and never produces a launch spec");
        assert!(error.contains("settled terminally"));
        assert_eq!(
            terminalizer.diagnostic.lock().unwrap().as_deref(),
            Some("secret_withheld:DEPLOY_KEY=capability_unavailable")
        );
        assert!(
            *terminalizer.terminal.lock().unwrap(),
            "the exact claim is terminal, so a lease/reaper predicate cannot select it again"
        );

        let query = crate::scheduler::CONSUME_SECRET_WITHHELD_CLAIM_QUERY;
        assert!(query.contains("SET state = 'terminal'"));
        assert!(query.contains("q.state = 'leased'"));
        assert!(query.contains("q.lease_epoch = $7"));
        assert!(query.contains("q.claim_nonce = $8::uuid"));
        assert!(query.contains("AND NOT EXISTS ("));
    }

    #[test]
    fn v1_unavailable_secret_job_is_terminally_settled_and_not_re_leased_or_reaped() {
        let claim = claim();
        let template = JobSpecTemplate::new(
            JobKind::Ci,
            ImageRef::pinned(format!("registry.example/job@sha256:{}", "a".repeat(64))).unwrap(),
            vec!["/bin/true".into()],
            Vec::new(),
            vec![SecretRef {
                name: "DEPLOY_KEY".into(),
                handle: "myelin://acme/ci/secret/opaque-handle".into(),
            }],
            EgressPolicy::deny_all(),
            ResourceLimits {
                cpu_millis: 1_000,
                mem_bytes: 256 * 1024 * 1024,
                disk_bytes: 1024 * 1024 * 1024,
                tmpfs_bytes: 64 * 1024 * 1024,
                pids_max: 64,
                timeout_secs: 30,
            },
            WorkspaceSpec::default(),
            TrustTier::Trusted,
            MeterTarget {
                reserve_id: "reserve:secret-test".into(),
            },
            IdemToken(claim.idem_token.clone()),
        )
        .unwrap();
        let authorization = RunTokenAuthorizationContext::CiJob(CiJobAuthorizationContext {
            tenant_id: claim.tenant_id.clone(),
            region: claim.region.clone(),
            principal_id: "ci-job".into(),
            project_id: claim.project_id.clone(),
            wf_run_id: claim.wf_run_id.clone(),
            job_id: claim.job_id.clone(),
            lease_owner: claim.lease_owner.clone(),
            lease_epoch: claim.lease_epoch,
            claim_nonce: claim.claim_nonce.clone(),
            claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
            claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
            reserve_id: "reserve:secret-test".into(),
            required_capabilities: Vec::new(),
            checkout_scope: None,
            credential_binding: None,
        });
        let resolution = resolve_claim_launch_secrets(
            &TenantId(claim.tenant_id.clone()),
            template,
            RunTokenCredential::new("bearer", "jti:v1-secret-test", 60).unwrap(),
            authorization,
            &crate::unavailable_ci_job_secret_resolver(),
        );
        let terminalizer = RecordingTerminalizer::default();

        let error = finish_v1_claim_secret_resolution(resolution, &claim, &terminalizer)
            .expect_err("V1 withhold must settle terminally and never produce a launch spec");
        assert!(error.contains("settled terminally"));
        assert!(*terminalizer.terminal.lock().unwrap());
        assert!(*terminalizer.failed_job_done.lock().unwrap());
        assert!(*terminalizer.accounting_settled.lock().unwrap());
        assert!(!*terminalizer.reaper_eligible.lock().unwrap());
        let diagnostic = terminalizer.diagnostic.lock().unwrap();
        assert_eq!(
            diagnostic.as_deref(),
            Some("secret_withheld:DEPLOY_KEY=capability_unavailable")
        );
        assert!(!diagnostic.as_deref().unwrap().contains("opaque-handle"));
    }
}

#[cfg(test)]
mod production_gvisor_registry_tests {
    use super::*;

    #[test]
    fn constructs_and_resolves_all_real_images_to_their_expected_paths() {
        let base_dir = resolved_gvisor_rootfs();
        let rust_dir = resolved_gvisor_rust_rootfs();
        let git_dir = myelin_ci_sandbox::resolved_gvisor_git_rootfs();
        let cargo_vendor_dir = resolved_gvisor_cargo_vendor();
        if !base_dir.is_dir()
            || !rust_dir.is_dir()
            || !git_dir.is_dir()
            || !cargo_vendor_dir.is_dir()
        {
            eprintln!(
                "constructs_and_resolves_all_real_images_to_their_expected_paths: SKIPPED - a \
                 staged base ({}) / rust ({}) / git ({}) rootfs or Cargo vendor ({}) is absent on \
                 this machine",
                base_dir.display(),
                rust_dir.display(),
                git_dir.display(),
                cargo_vendor_dir.display()
            );
            return;
        }

        let registry = production_gvisor_registry().expect("production registry verifies");

        let small_image = ImageRef::pinned(format!(
            "myelin.local/linux-small-v1-rootfs@sha256:{LINUX_SMALL_V1_ROOTFS_SHA256}"
        ))
        .unwrap();
        let rust_image = ImageRef::pinned(format!(
            "myelin.local/linux-rust-v1-rootfs@sha256:{LINUX_RUST_V1_ROOTFS_SHA256}"
        ))
        .unwrap();
        let git_image = ImageRef::pinned(format!(
            "myelin.local/git-v1-rootfs@sha256:{GVISOR_GIT_ROOTFS_SHA256}"
        ))
        .unwrap();
        let cargo_vendor_reference = ImageRef::pinned(format!(
            "myelin.local/cargo-vendor-smoke-v1@sha256:{CARGO_VENDOR_SMOKE_TREE_SHA256}"
        ))
        .unwrap();

        let verified_small = registry
            .resolve(&small_image)
            .expect("the production registry must resolve linux-small-v1");
        let verified_rust = registry
            .resolve(&rust_image)
            .expect("the production registry must resolve linux-rust-v1");
        let verified_git = registry
            .resolve(&git_image)
            .expect("the production registry must resolve git-v1");
        let verified_cargo_vendor = registry
            .resolve_cargo_vendor(&cargo_vendor_reference)
            .expect("the production registry must resolve cargo-vendor-smoke-v1");

        assert_eq!(
            verified_small.path(),
            std::fs::canonicalize(&base_dir).unwrap(),
            "linux-small-v1 must resolve to the SAME canonicalized path resolved_gvisor_rootfs() names"
        );
        assert_eq!(
            verified_rust.path(),
            std::fs::canonicalize(&rust_dir).unwrap(),
            "linux-rust-v1 must resolve to the SAME canonicalized path resolved_gvisor_rust_rootfs() names"
        );
        assert_eq!(
            verified_git.path(),
            std::fs::canonicalize(&git_dir).unwrap(),
            "git-v1 must resolve to the same verified canonical path used by checkout"
        );
        assert_eq!(
            verified_cargo_vendor.path(),
            std::fs::canonicalize(&cargo_vendor_dir).unwrap(),
            "cargo-vendor-smoke-v1 must resolve to the manifest-selected canonical path"
        );
        assert_eq!(
            verified_cargo_vendor.digest_hex(),
            CARGO_VENDOR_SMOKE_TREE_SHA256,
            "cargo-vendor-smoke-v1 must round-trip its canonical-tree pin"
        );
        assert_eq!(
            verified_cargo_vendor.cargo_lock_sha256(),
            CARGO_VENDOR_SMOKE_LOCK_SHA256,
            "cargo-vendor-smoke-v1 must round-trip its exact Cargo.lock key"
        );
    }
}

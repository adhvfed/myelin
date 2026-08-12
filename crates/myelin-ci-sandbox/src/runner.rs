use crate::{
    CompletionSettlementOwner, JobSpec, ResourceUsage, RunnerHooks, SandboxBackend,
    SandboxCancellation, SandboxLaunch, SandboxLaunchError, SandboxOutputSink, SandboxOutputStream,
};
use myelin_flow::{
    DurableExecutor, ExecutorError, RunId, SignalOutcome, SignalSpec, JOB_DONE_SIGNAL,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::collections::BTreeMap;
use std::sync::{mpsc::SyncSender, Arc, Mutex};

#[derive(Clone, Debug)]
pub struct QueuedJob {
    pub tenant: TenantId,
    pub region: Region,
    pub run_id: String,
    pub job_id: String,
    pub labels: Vec<String>,
    pub spec: JobSpec,
    pub lease_owner: Option<String>,
    pub lease_expires: Option<i64>,
    pub lease_epoch: i64,
    pub claim_nonce: String,
}

impl QueuedJob {
    pub fn new(
        tenant: TenantId,
        region: Region,
        run_id: impl Into<String>,
        job_id: impl Into<String>,
        labels: Vec<String>,
        spec: JobSpec,
    ) -> Self {
        Self {
            tenant,
            region,
            run_id: run_id.into(),
            job_id: job_id.into(),
            labels,
            spec,
            lease_owner: None,
            lease_expires: None,
            lease_epoch: 0,
            claim_nonce: String::new(),
        }
    }
}

#[derive(Clone, Default)]
pub struct JobLeaseStore {
    inner: Arc<Mutex<BTreeMap<(String, String), QueuedJob>>>,
}

impl JobLeaseStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<(String, String), QueuedJob>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn key(j: &QueuedJob) -> (String, String) {
        (j.tenant.0.clone(), j.job_id.clone())
    }

    pub fn enqueue(&self, job: QueuedJob) {
        self.lock().insert(Self::key(&job), job);
    }

    pub fn get(&self, tenant: &TenantId, job_id: &str) -> Option<QueuedJob> {
        self.lock()
            .get(&(tenant.0.clone(), job_id.to_string()))
            .cloned()
    }

    pub fn claim_for_labels(
        &self,
        worker: &str,
        runner_labels: &[String],
        allowed_tiers: &[crate::TrustTier],
        region: &Region,
        now: i64,
        lease_ttl_secs: i64,
    ) -> Option<QueuedJob> {
        let mut q = self.lock();
        for job in q.values_mut() {
            if &job.region != region {
                continue;
            }
            if !job.labels.iter().all(|l| runner_labels.contains(l)) {
                continue;
            }
            if !allowed_tiers.contains(&job.spec.trust_tier) {
                continue;
            }
            let lease_free = match job.lease_expires {
                None => true,
                Some(exp) => exp <= now,
            };
            if lease_free {
                job.lease_owner = Some(worker.to_string());
                job.lease_expires = Some(now + lease_ttl_secs);
                job.lease_epoch += 1;
                job.claim_nonce = format!("memory:{worker}:{}", job.lease_epoch);
                return Some(job.clone());
            }
        }
        None
    }

    pub fn heartbeat(
        &self,
        worker: &str,
        tenant: &TenantId,
        job_id: &str,
        now: i64,
        lease_ttl_secs: i64,
    ) -> bool {
        let mut q = self.lock();
        match q.get_mut(&(tenant.0.clone(), job_id.to_string())) {
            Some(job) if job.lease_owner.as_deref() == Some(worker) => {
                job.lease_expires = Some(now + lease_ttl_secs);
                true
            }
            _ => false,
        }
    }

    pub fn settle(&self, tenant: &TenantId, job_id: &str) {
        self.lock().remove(&(tenant.0.clone(), job_id.to_string()));
    }

    pub fn claimable_depth(&self, runner_labels: &[String], region: &Region, now: i64) -> usize {
        self.lock()
            .values()
            .filter(|j| {
                &j.region == region
                    && j.labels.iter().all(|l| runner_labels.contains(l))
                    && j.lease_expires.map(|e| e <= now).unwrap_or(true)
            })
            .count()
    }
}

pub trait LeaseStore {
    fn claim_for_labels(
        &self,
        worker: &str,
        runner_labels: &[String],
        allowed_tiers: &[crate::TrustTier],
        region: &Region,
        now: i64,
        lease_ttl_secs: i64,
    ) -> Option<QueuedJob>;

    fn heartbeat(
        &self,
        worker: &str,
        tenant: &TenantId,
        job_id: &str,
        now: i64,
        lease_ttl_secs: i64,
    ) -> bool;

    fn settle(&self, tenant: &TenantId, job_id: &str);
}

impl LeaseStore for JobLeaseStore {
    fn claim_for_labels(
        &self,
        worker: &str,
        runner_labels: &[String],
        allowed_tiers: &[crate::TrustTier],
        region: &Region,
        now: i64,
        lease_ttl_secs: i64,
    ) -> Option<QueuedJob> {
        self.claim_for_labels(
            worker,
            runner_labels,
            allowed_tiers,
            region,
            now,
            lease_ttl_secs,
        )
    }

    fn heartbeat(
        &self,
        worker: &str,
        tenant: &TenantId,
        job_id: &str,
        now: i64,
        lease_ttl_secs: i64,
    ) -> bool {
        self.heartbeat(worker, tenant, job_id, now, lease_ttl_secs)
    }

    fn settle(&self, tenant: &TenantId, job_id: &str) {
        self.settle(tenant, job_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparationLeaseLost(pub String);

impl std::fmt::Display for PreparationLeaseLost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "preparation lease renewal refused: {}", self.0)
    }
}

impl std::error::Error for PreparationLeaseLost {}

pub trait PreparationLeaseCheckpoint: Send + Sync {
    fn renew(&self) -> Result<(), PreparationLeaseLost>;
}

pub trait FirehoseSink {
    fn ship_frame(
        &self,
        run_id: &str,
        job_id: &str,
        tenant: &TenantId,
        frame: &[u8],
    ) -> Result<(), String>;
    fn finish(
        &self,
        run_id: &str,
        job_id: &str,
        tenant: &TenantId,
        passed: bool,
    ) -> Result<(), String>;
}

#[derive(Clone, Default)]
pub struct CountingFirehose {
    count: Arc<Mutex<u64>>,
    finished: Arc<Mutex<u64>>,
}

impl CountingFirehose {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn frames_shipped(&self) -> u64 {
        *self.count.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn jobs_finished(&self) -> u64 {
        *self.finished.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl FirehoseSink for CountingFirehose {
    fn ship_frame(
        &self,
        _run_id: &str,
        _job_id: &str,
        _tenant: &TenantId,
        _frame: &[u8],
    ) -> Result<(), String> {
        *self.count.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        Ok(())
    }
    fn finish(
        &self,
        _run_id: &str,
        _job_id: &str,
        _tenant: &TenantId,
        _passed: bool,
    ) -> Result<(), String> {
        *self.finished.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        Ok(())
    }
}

struct OutputChannelSink {
    tx: SyncSender<Vec<u8>>,
}

impl SandboxOutputSink for OutputChannelSink {
    fn emit(&self, _stream: SandboxOutputStream, frame: &[u8]) -> Result<(), String> {
        self.tx
            .send(frame.to_vec())
            .map_err(|_| "runner durable log consumer stopped".to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalReport {
    pub passed: bool,
    pub timed_out: bool,
    pub usage: ResourceUsage,
    pub result_refs: Vec<ArtifactRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparationPhase {
    SecretResolution,
    CheckoutTransport,
    CheckoutMaterialization,
}

impl PreparationPhase {
    pub fn as_storage_token(self) -> &'static str {
        match self {
            Self::SecretResolution => "secret_resolution",
            Self::CheckoutTransport => "checkout_transport",
            Self::CheckoutMaterialization => "checkout_materialization",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparationTerminalDisposition {
    Failed { phase: PreparationPhase },
    TimedOut { phase: PreparationPhase },
    AttemptsExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparationAttemptDisposition {
    Terminal(PreparationTerminalDisposition),
    RefusedBeforeExecution {
        phase: PreparationPhase,
    },
    RetryableInfrastructure {
        phase: PreparationPhase,
    },
    ReconciliationRequired {
        phase: PreparationPhase,
        teardown_unproven: bool,
        usage_unrepresentable: bool,
        quarantine_required: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionClaim {
    pub tenant: TenantId,
    pub run: RunId,
    pub job_id: String,
    pub idem_token: String,
    pub lease_owner: String,
    pub lease_epoch: i64,
    pub claim_nonce: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparationReportClaim {
    pub tenant_id: String,
    pub region: String,
    pub project_id: String,
    pub wf_run_id: String,
    pub ci_run_id: String,
    pub job_id: String,
    pub token_authority_handle: String,
    pub idem_token: String,
    pub lease_owner: String,
    pub lease_epoch: i64,
    pub claim_nonce: String,
    pub claim_started_at_epoch_secs: i64,
    pub claim_expires_at_epoch_secs: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryableAttemptFailure {
    pub cause: RetryableAttemptCause,
    pub usage: ResourceUsage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryableAttemptCause {
    OutputPersistence,
    SandboxInfrastructure,
}

impl RetryableAttemptCause {
    pub fn as_storage_token(&self) -> &'static str {
        match self {
            RetryableAttemptCause::OutputPersistence => "output_persistence",
            RetryableAttemptCause::SandboxInfrastructure => "sandbox_infrastructure",
        }
    }

    pub fn from_storage_token(token: &str) -> Option<Self> {
        match token {
            "output_persistence" => Some(RetryableAttemptCause::OutputPersistence),
            "sandbox_infrastructure" => Some(RetryableAttemptCause::SandboxInfrastructure),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryableAttemptOutcome {
    Requeued,
    Cancelled,
    ExactReplay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparationRetryReport {
    Requeued,
    NoOp,
}

pub trait TerminalReporter {
    fn completion_settlement_owner(&self) -> CompletionSettlementOwner;

    fn report_done(
        &self,
        claim: &CompletionClaim,
        report: &TerminalReport,
    ) -> Result<SignalOutcome, ExecutorError>;

    fn report_retryable_attempt(
        &self,
        claim: &CompletionClaim,
        failure: &RetryableAttemptFailure,
    ) -> Result<RetryableAttemptOutcome, ExecutorError>;

    fn report_preparation_terminal(
        &self,
        claim: &PreparationReportClaim,
        disposition: PreparationTerminalDisposition,
        diagnostic: Option<&str>,
    ) -> Result<SignalOutcome, ExecutorError>;

    fn report_preparation_retry(
        &self,
        claim: &PreparationReportClaim,
    ) -> Result<PreparationRetryReport, ExecutorError>;
}

pub struct EngineTerminalReporter<E: DurableExecutor> {
    executor: E,
}

impl<E: DurableExecutor> EngineTerminalReporter<E> {
    pub fn new(executor: E) -> Self {
        Self { executor }
    }
}

impl<E: DurableExecutor> TerminalReporter for EngineTerminalReporter<E> {
    fn completion_settlement_owner(&self) -> CompletionSettlementOwner {
        CompletionSettlementOwner::Hook
    }

    fn report_done(
        &self,
        claim: &CompletionClaim,
        report: &TerminalReport,
    ) -> Result<SignalOutcome, ExecutorError> {
        let mut payload = Vec::with_capacity(report.result_refs.len() + 1);
        payload.push(ArtifactRef(format!(
            "myelin://job-done/passed-{}",
            report.passed
        )));
        payload.extend(report.result_refs.iter().cloned());
        self.executor.signal(SignalSpec {
            run: claim.run.clone(),
            signal_name: JOB_DONE_SIGNAL.to_string(),
            idem_key: claim.idem_token.clone(),
            payload,
            payload_key_ref: None,
        })
    }

    fn report_retryable_attempt(
        &self,
        _claim: &CompletionClaim,
        _failure: &RetryableAttemptFailure,
    ) -> Result<RetryableAttemptOutcome, ExecutorError> {
        Err(ExecutorError::InvalidInput(
            "retryable measured CI attempts require a claim-aware durable reporter".into(),
        ))
    }

    fn report_preparation_terminal(
        &self,
        _claim: &PreparationReportClaim,
        _disposition: PreparationTerminalDisposition,
        _diagnostic: Option<&str>,
    ) -> Result<SignalOutcome, ExecutorError> {
        Err(ExecutorError::InvalidInput(
            "checkout preparation terminals require the CI pipeline reporter, not the engine reporter"
                .into(),
        ))
    }

    fn report_preparation_retry(
        &self,
        _claim: &PreparationReportClaim,
    ) -> Result<PreparationRetryReport, ExecutorError> {
        Err(ExecutorError::InvalidInput(
            "checkout preparation retries require the CI pipeline reporter, not the engine reporter"
                .into(),
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunOutcome {
    pub job_id: String,
    pub run_id: String,
    pub report: TerminalReport,
    pub signal_outcome: SignalOutcome,
    pub lease_epoch: i64,
    pub claim_nonce: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunnerCycleOutcome {
    Workload(RunOutcome),
    PreparationTerminal {
        job_id: String,
        run_id: String,
        signal_outcome: SignalOutcome,
        diagnostic: Option<String>,
    },
    PreparationRetryable {
        job_id: String,
        report: PreparationRetryReport,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparationOutcomeDispatch {
    Terminalized(SignalOutcome),
    Retried(PreparationRetryReport),
    NotAPreparationReport,
}

#[derive(Debug)]
pub enum RunnerError {
    NoWork,
    LaunchFailed(String),
    LeaseLost {
        job_id: String,
    },
    ReportFailed(ExecutorError),
    RetryableAttemptRecorded {
        job_id: String,
        message: String,
    },
    SettlementOwnerMismatch {
        hooks: CompletionSettlementOwner,
        reporter: CompletionSettlementOwner,
    },
    PreparationRoutingFailed {
        job_id: String,
        message: String,
    },
    ReconciliationRequired {
        job_id: String,
        message: String,
    },
}

impl std::fmt::Display for RunnerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunnerError::NoWork => write!(f, "no claimable job for the runner's labels/region"),
            RunnerError::LaunchFailed(m) => write!(f, "sandbox launch failed (fail-closed): {m}"),
            RunnerError::LeaseLost { job_id } => write!(
                f,
                "lease LOST mid-run for job {job_id} - another worker reclaimed it (the reaper \
                 re-queued it; this runner must not report terminal)"
            ),
            RunnerError::ReportFailed(e) => write!(f, "terminal job.done report failed: {e}"),
            RunnerError::RetryableAttemptRecorded { job_id, message } => write!(
                f,
                "retryable measured attempt recorded for job {job_id}; no terminal verdict \
                 emitted ({message})"
            ),
            RunnerError::SettlementOwnerMismatch { hooks, reporter } => write!(
                f,
                "terminal settlement owner mismatch: hooks={hooks:?}, reporter={reporter:?}"
            ),
            RunnerError::PreparationRoutingFailed { job_id, message } => write!(
                f,
                "checkout preparation routing failed for job {job_id}: {message}"
            ),
            RunnerError::ReconciliationRequired { job_id, message } => write!(
                f,
                "checkout reconciliation required for job {job_id}: {message}"
            ),
        }
    }
}

impl std::error::Error for RunnerError {}

pub struct RunnerAgent<
    'a,
    B: SandboxBackend,
    F: FirehoseSink,
    T: TerminalReporter,
    L: LeaseStore = JobLeaseStore,
> {
    worker_id: String,
    labels: Vec<String>,
    allowed_tiers: Vec<crate::TrustTier>,
    region: Region,
    lease_ttl_secs: i64,
    leases: L,
    backend: &'a B,
    firehose: &'a F,
    reporter: &'a T,
    hooks: RunnerHooks,
}

#[allow(clippy::too_many_arguments)]
impl<'a, B: SandboxBackend, F: FirehoseSink, T: TerminalReporter, L: LeaseStore>
    RunnerAgent<'a, B, F, T, L>
{
    pub fn new(
        worker_id: impl Into<String>,
        labels: Vec<String>,
        allowed_tiers: Vec<crate::TrustTier>,
        region: Region,
        lease_ttl_secs: i64,
        leases: L,
        backend: &'a B,
        firehose: &'a F,
        reporter: &'a T,
        hooks: RunnerHooks,
    ) -> Self {
        Self {
            worker_id: worker_id.into(),
            labels,
            allowed_tiers,
            region,
            lease_ttl_secs,
            leases,
            backend,
            firehose,
            reporter,
            hooks,
        }
    }

    pub fn worker_id(&self) -> &str {
        &self.worker_id
    }

    pub fn report_preparation_outcome(
        &self,
        outcome: &crate::SandboxCycleOutcome,
    ) -> Result<PreparationOutcomeDispatch, ExecutorError> {
        use crate::SandboxCycleOutcome;
        match outcome {
            SandboxCycleOutcome::PreparationTerminal {
                claim,
                disposition,
                diagnostic,
            } => Ok(PreparationOutcomeDispatch::Terminalized(
                self.reporter.report_preparation_terminal(
                    claim,
                    *disposition,
                    diagnostic.as_deref(),
                )?,
            )),
            SandboxCycleOutcome::PreparationRetryable { claim, .. } => Ok(
                PreparationOutcomeDispatch::Retried(self.reporter.report_preparation_retry(claim)?),
            ),
            SandboxCycleOutcome::WorkloadLaunched(_)
            | SandboxCycleOutcome::WorkloadRetryable { .. }
            | SandboxCycleOutcome::ReconciliationRequired { .. } => {
                Ok(PreparationOutcomeDispatch::NotAPreparationReport)
            }
        }
    }

    pub fn run_one(&self, now: i64) -> Result<RunOutcome, RunnerError> {
        match self.run_one_cycle(now)? {
            RunnerCycleOutcome::Workload(outcome) => Ok(outcome),
            RunnerCycleOutcome::PreparationTerminal { job_id, .. }
            | RunnerCycleOutcome::PreparationRetryable { job_id, .. } => {
                Err(RunnerError::PreparationRoutingFailed {
                    job_id,
                    message: "typed preparation outcome requires the runner-cycle caller".into(),
                })
            }
        }
    }

    pub fn run_one_cycle(&self, now: i64) -> Result<RunnerCycleOutcome, RunnerError> {
        let hook_owner = self.hooks.completion_settlement_owner();
        let reporter_owner = self.reporter.completion_settlement_owner();
        if hook_owner != reporter_owner {
            return Err(RunnerError::SettlementOwnerMismatch {
                hooks: hook_owner,
                reporter: reporter_owner,
            });
        }
        let job = self
            .leases
            .claim_for_labels(
                &self.worker_id,
                &self.labels,
                &self.allowed_tiers,
                &self.region,
                now,
                self.lease_ttl_secs,
            )
            .ok_or(RunnerError::NoWork)?;

        let held = self.leases.heartbeat(
            &self.worker_id,
            &job.tenant,
            &job.job_id,
            now,
            self.lease_ttl_secs,
        );
        if !held {
            return Err(RunnerError::LeaseLost {
                job_id: job.job_id.clone(),
            });
        }

        let claim = CompletionClaim {
            tenant: job.tenant.clone(),
            run: RunId(job.run_id.clone()),
            job_id: job.job_id.clone(),
            idem_token: job.spec.idem_token.0.clone(),
            lease_owner: self.worker_id.clone(),
            lease_epoch: job.lease_epoch,
            claim_nonce: job.claim_nonce.clone(),
        };

        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(8);
        let output_run_id = job.run_id.clone();
        let output_job_id = job.job_id.clone();
        let output_tenant = job.tenant.clone();
        let launch_result = std::thread::scope(|scope| {
            let output: Arc<dyn SandboxOutputSink> = Arc::new(OutputChannelSink { tx });
            let cancellation = SandboxCancellation::new();
            let backend_cancellation = cancellation.clone();
            let backend = self.backend;
            let hooks = &self.hooks;
            let spec = &job.spec;
            let launch_thread =
                scope.spawn(move || backend.run_cycle(spec, hooks, output, backend_cancellation));
            let consumed = (|| -> Result<(), String> {
                while let Ok(frame) = rx.recv() {
                    self.firehose.ship_frame(
                        &output_run_id,
                        &output_job_id,
                        &output_tenant,
                        &frame,
                    )?;
                }
                Ok(())
            })();
            if consumed.is_err() {
                cancellation.cancel();
            }
            drop(rx);
            let launch = launch_thread.join().map_err(|_| {
                RunnerError::LaunchFailed("sandbox launch thread panicked".to_string())
            })?;
            match (launch, consumed) {
                (Ok(outcome), Ok(())) => Ok((outcome, None)),
                (Ok(outcome), Err(error)) => {
                    Ok((outcome, Some(format!("durable log stream failed: {error}"))))
                }
                (Err(error), _) => match error {
                    SandboxLaunchError::Failed(source) => {
                        Err(RunnerError::LaunchFailed(source.to_string()))
                    }
                    SandboxLaunchError::DurableOutcomeUnknown(source) => {
                        Err(RunnerError::LaunchFailed(format!(
                            "durable launch commit outcome UNKNOWN for job {} - neither released \
                             nor settled; the durable lease/claim reaper must reconcile it: {source}",
                            claim.job_id,
                        )))
                    }
                    SandboxLaunchError::RetryableAttempt {
                        source,
                        cause,
                        usage,
                    } => {
                        let message = source.to_string();
                        self.reporter
                            .report_retryable_attempt(
                                &claim,
                                &RetryableAttemptFailure { cause, usage },
                            )
                            .map_err(RunnerError::ReportFailed)?;
                        Err(RunnerError::RetryableAttemptRecorded {
                            job_id: claim.job_id.clone(),
                            message,
                        })
                    }
                },
            }
        })?;
        let (cycle_outcome, stream_failure) = launch_result;
        let launch = match cycle_outcome {
            crate::SandboxCycleOutcome::WorkloadRetryable {
                cause,
                usage,
                message,
            } => {
                self.reporter
                    .report_retryable_attempt(&claim, &RetryableAttemptFailure { cause, usage })
                    .map_err(RunnerError::ReportFailed)?;
                return Err(RunnerError::RetryableAttemptRecorded {
                    job_id: job.job_id,
                    message,
                });
            }
            outcome @ crate::SandboxCycleOutcome::PreparationTerminal { .. } => {
                if let Some(error) = stream_failure {
                    return self.retry_preparation_after_log_failure(&job, &outcome, error);
                }
                if let Err(error) =
                    self.firehose
                        .finish(&job.run_id, &job.job_id, &job.tenant, false)
                {
                    return self.retry_preparation_after_log_failure(
                        &job,
                        &outcome,
                        format!("durable log finish failed: {error}"),
                    );
                }
                return match self
                    .report_preparation_outcome(&outcome)
                    .map_err(RunnerError::ReportFailed)?
                {
                    PreparationOutcomeDispatch::Terminalized(signal_outcome) => {
                        let diagnostic = match outcome {
                            crate::SandboxCycleOutcome::PreparationTerminal {
                                diagnostic, ..
                            } => diagnostic,
                            _ => unreachable!("the matched outcome is preparation-terminal"),
                        };
                        Ok(RunnerCycleOutcome::PreparationTerminal {
                            job_id: job.job_id,
                            run_id: job.run_id,
                            signal_outcome,
                            diagnostic,
                        })
                    }
                    _ => unreachable!("a preparation-terminal outcome has one reporter route"),
                };
            }
            outcome @ crate::SandboxCycleOutcome::PreparationRetryable { .. } => {
                return self.finish_preparation_retry(&job, &outcome);
            }
            crate::SandboxCycleOutcome::ReconciliationRequired {
                phase,
                teardown_unproven,
                usage_unrepresentable,
                quarantine_required,
            } => {
                return Err(RunnerError::ReconciliationRequired {
                    job_id: job.job_id,
                    message: format!(
                        "phase={phase:?}, teardown_unproven={teardown_unproven}, \
                         usage_unrepresentable={usage_unrepresentable}, \
                         quarantine_required={quarantine_required}"
                    ),
                });
            }
            crate::SandboxCycleOutcome::WorkloadLaunched(launch) => launch,
        };
        let SandboxLaunch {
            handle,
            result,
            output_complete: backend_output_complete,
        } = launch;

        let command_passed = result.passed();
        let mut output_failure = stream_failure;
        if !backend_output_complete && output_failure.is_none() {
            output_failure = Some("sandbox output delivery did not complete".into());
        }
        if output_failure.is_none() {
            if let Err(error) =
                self.firehose
                    .finish(&job.run_id, &job.job_id, &job.tenant, command_passed)
            {
                output_failure = Some(format!("durable log finish failed: {error}"));
            }
        }
        self.leases.heartbeat(
            &self.worker_id,
            &job.tenant,
            &job.job_id,
            now,
            self.lease_ttl_secs,
        );

        self.backend
            .kill(&handle)
            .map_err(|e| RunnerError::LaunchFailed(e.to_string()))?;

        if let Some(message) = output_failure {
            self.reporter
                .report_retryable_attempt(
                    &claim,
                    &RetryableAttemptFailure {
                        cause: RetryableAttemptCause::OutputPersistence,
                        usage: result.usage,
                    },
                )
                .map_err(RunnerError::ReportFailed)?;
            return Err(RunnerError::RetryableAttemptRecorded {
                job_id: job.job_id,
                message,
            });
        }

        let report = TerminalReport {
            passed: command_passed,
            timed_out: result.timed_out,
            usage: result.usage,
            result_refs: vec![],
        };

        let outcome = self
            .reporter
            .report_done(&claim, &report)
            .map_err(RunnerError::ReportFailed)?;

        self.leases.settle(&job.tenant, &job.job_id);

        Ok(RunnerCycleOutcome::Workload(RunOutcome {
            job_id: job.job_id,
            run_id: job.run_id,
            report,
            signal_outcome: outcome,
            lease_epoch: job.lease_epoch,
            claim_nonce: job.claim_nonce,
        }))
    }

    fn retry_preparation_after_log_failure(
        &self,
        job: &QueuedJob,
        terminal_outcome: &crate::SandboxCycleOutcome,
        message: String,
    ) -> Result<RunnerCycleOutcome, RunnerError> {
        let claim = match terminal_outcome {
            crate::SandboxCycleOutcome::PreparationTerminal { claim, .. } => claim,
            _ => unreachable!("only a preparation terminal can fail its terminal log flush"),
        };
        match self.reporter.report_preparation_retry(claim) {
            Ok(PreparationRetryReport::Requeued) => Ok(RunnerCycleOutcome::PreparationRetryable {
                job_id: job.job_id.clone(),
                report: PreparationRetryReport::Requeued,
            }),
            Ok(PreparationRetryReport::NoOp) => Err(RunnerError::PreparationRoutingFailed {
                job_id: job.job_id.clone(),
                message: format!(
                    "{message}; preparation retry CAS returned no-op and requires reconciliation"
                ),
            }),
            Err(error) => Err(RunnerError::PreparationRoutingFailed {
                job_id: job.job_id.clone(),
                message: format!("{message}; retry report failed: {error}"),
            }),
        }
    }

    fn finish_preparation_retry(
        &self,
        job: &QueuedJob,
        outcome: &crate::SandboxCycleOutcome,
    ) -> Result<RunnerCycleOutcome, RunnerError> {
        match self
            .report_preparation_outcome(outcome)
            .map_err(RunnerError::ReportFailed)?
        {
            PreparationOutcomeDispatch::Retried(PreparationRetryReport::Requeued) => {
                Ok(RunnerCycleOutcome::PreparationRetryable {
                    job_id: job.job_id.clone(),
                    report: PreparationRetryReport::Requeued,
                })
            }
            PreparationOutcomeDispatch::Retried(PreparationRetryReport::NoOp) => {
                Err(RunnerError::PreparationRoutingFailed {
                    job_id: job.job_id.clone(),
                    message:
                        "preparation retry CAS returned no-op; claim state requires reconciliation"
                            .into(),
                })
            }
            _ => unreachable!("a preparation-retry outcome has one reporter route"),
        }
    }

    pub fn report_done_again(
        &self,
        claim: &CompletionClaim,
        report: &TerminalReport,
    ) -> Result<SignalOutcome, RunnerError> {
        self.reporter
            .report_done(claim, report)
            .map_err(RunnerError::ReportFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EgressPolicy, HookError, IdemToken, ImageRef, JobKind, MeterTarget, ReserveHandle,
        ResourceLimits, ResourceUsage, RunTokenCredential, SandboxHandle, SandboxLaunch,
        SandboxResult, TrustTier, WorkspaceSpec,
    };
    use myelin_events::MonotonicMinter;
    use myelin_flow::{job_idem_token, FlowExecutor, StartSpec};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }

    fn pinned() -> ImageRef {
        ImageRef::pinned("registry.example/runner@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef").unwrap()
    }
    fn limits() -> ResourceLimits {
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 256 << 20,
            disk_bytes: 1 << 30,
            tmpfs_bytes: 1 << 30,
            pids_max: 128,
            timeout_secs: 600,
        }
    }

    fn ci_spec(idem: &str) -> JobSpec {
        JobSpec::new(
            JobKind::Ci,
            pinned(),
            vec!["cargo".into(), "test".into()],
            vec![],
            vec![],
            EgressPolicy::deny_all(),
            limits(),
            WorkspaceSpec::default(),
            TrustTier::Trusted,
            RunTokenCredential::new("test-bearer", "jti-1", 300).unwrap(),
            MeterTarget {
                reserve_id: "res-1".into(),
            },
            IdemToken(idem.into()),
        )
        .unwrap()
    }

    fn test_hooks() -> RunnerHooks {
        RunnerHooks::new(
            CompletionSettlementOwner::Hook,
            Box::new(|spec| {
                Ok(ReserveHandle(format!(
                    "reserved:{}",
                    spec.meter_to.reserve_id
                )))
            }),
            Box::new(|_spec, _h, _u| Ok(())),
            Box::new(|_t| Ok(())),
            Box::new(|_s| Ok(())),
        )
    }

    struct RecordingBackend {
        launches: AtomicUsize,
        kills: AtomicUsize,
        fail_launch: bool,
        forced_launch_error: Option<SandboxLaunchError<HookError>>,
        result: SandboxResult,
    }
    impl Default for RecordingBackend {
        fn default() -> Self {
            Self {
                launches: AtomicUsize::new(0),
                kills: AtomicUsize::new(0),
                fail_launch: false,
                forced_launch_error: None,
                result: SandboxResult {
                    exit_code: Some(0),
                    timed_out: false,
                    usage: ResourceUsage {
                        cpu_seconds: 1,
                        mem_byte_seconds: 1,
                    },
                    stdout: b"<stub guest stdout>".to_vec(),
                    stderr: Vec::new(),
                },
            }
        }
    }
    impl SandboxBackend for RecordingBackend {
        type Error = HookError;
        fn launch(
            &self,
            spec: &JobSpec,
            hooks: &RunnerHooks,
        ) -> Result<SandboxLaunch, SandboxLaunchError<Self::Error>> {
            if let Some(forced) = &self.forced_launch_error {
                return Err(forced.clone());
            }
            (|| -> Result<SandboxLaunch, HookError> {
                if self.fail_launch {
                    return Err(HookError("backend refused".into()));
                }
                hooks.enforce_isolation_floor(spec)?;
                let res = hooks.reserve(spec)?;
                if let Err(error) = hooks.attribute(spec) {
                    hooks.release_unused(spec, &res)?;
                    return Err(error);
                }
                hooks.settle_completed(spec, &res, self.result.usage)?;
                self.launches.fetch_add(1, Ordering::SeqCst);
                Ok(SandboxLaunch {
                    handle: SandboxHandle {
                        guest_id: format!("guest-{}", spec.idem_token.0),
                    },
                    result: self.result.clone(),
                    output_complete: true,
                })
            })()
            .map_err(SandboxLaunchError::Failed)
        }
        fn kill(&self, _h: &SandboxHandle) -> Result<(), Self::Error> {
            self.kills.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[derive(Clone, Copy)]
    enum CycleMode {
        Workload,
        PreparationTerminal,
        PreparationRetryable,
    }

    struct CycleOnlyBackend {
        mode: CycleMode,
        cycle_calls: AtomicUsize,
        kills: AtomicUsize,
    }

    impl CycleOnlyBackend {
        fn new(mode: CycleMode) -> Self {
            Self {
                mode,
                cycle_calls: AtomicUsize::new(0),
                kills: AtomicUsize::new(0),
            }
        }
    }

    impl SandboxBackend for CycleOnlyBackend {
        type Error = HookError;

        fn launch(
            &self,
            _spec: &JobSpec,
            _hooks: &RunnerHooks,
        ) -> Result<SandboxLaunch, SandboxLaunchError<Self::Error>> {
            panic!("Stage-B RunnerAgent must never call launch; it must call run_cycle")
        }

        fn launch_streaming(
            &self,
            _spec: &JobSpec,
            _hooks: &RunnerHooks,
            _output: Arc<dyn SandboxOutputSink>,
            _cancellation: SandboxCancellation,
        ) -> Result<SandboxLaunch, SandboxLaunchError<Self::Error>> {
            panic!("Stage-B RunnerAgent must never call launch_streaming; it must call run_cycle")
        }

        fn run_cycle(
            &self,
            spec: &JobSpec,
            _hooks: &RunnerHooks,
            _output: Arc<dyn SandboxOutputSink>,
            _cancellation: SandboxCancellation,
        ) -> Result<crate::SandboxCycleOutcome, SandboxLaunchError<Self::Error>> {
            self.cycle_calls.fetch_add(1, Ordering::SeqCst);
            Ok(match self.mode {
                CycleMode::Workload => {
                    crate::SandboxCycleOutcome::WorkloadLaunched(SandboxLaunch {
                        handle: SandboxHandle {
                            guest_id: format!("cycle-{}", spec.idem_token.0),
                        },
                        result: SandboxResult::stub_ok(ResourceUsage {
                            cpu_seconds: 2,
                            mem_byte_seconds: 3,
                        }),
                        output_complete: true,
                    })
                }
                CycleMode::PreparationTerminal => crate::SandboxCycleOutcome::PreparationTerminal {
                    claim: prep_report_claim(),
                    disposition: PreparationTerminalDisposition::Failed {
                        phase: PreparationPhase::CheckoutMaterialization,
                    },
                    diagnostic: Some("injected materialization diagnostic".into()),
                },
                CycleMode::PreparationRetryable => {
                    crate::SandboxCycleOutcome::PreparationRetryable {
                        claim: prep_report_claim(),
                        phase: PreparationPhase::CheckoutMaterialization,
                    }
                }
            })
        }

        fn kill(&self, _h: &SandboxHandle) -> Result<(), Self::Error> {
            self.kills.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    type RecordedPreparationTerminal = (
        PreparationReportClaim,
        PreparationTerminalDisposition,
        Option<String>,
    );

    struct RecordingTerminalReporter {
        owner: CompletionSettlementOwner,
        reports: AtomicUsize,
        retry_reports: AtomicUsize,
        settlements: Arc<Mutex<Vec<ResourceUsage>>>,
        retry_causes: Arc<Mutex<Vec<RetryableAttemptCause>>>,
        prep_terminals: Arc<Mutex<Vec<RecordedPreparationTerminal>>>,
        prep_retries: Arc<Mutex<Vec<PreparationReportClaim>>>,
        fail: bool,
    }

    impl RecordingTerminalReporter {
        fn reporter_owned(settlements: Arc<Mutex<Vec<ResourceUsage>>>, fail: bool) -> Self {
            Self {
                owner: CompletionSettlementOwner::TerminalReporter,
                reports: AtomicUsize::new(0),
                retry_reports: AtomicUsize::new(0),
                settlements,
                retry_causes: Arc::new(Mutex::new(Vec::new())),
                prep_terminals: Arc::new(Mutex::new(Vec::new())),
                prep_retries: Arc::new(Mutex::new(Vec::new())),
                fail,
            }
        }
    }

    impl TerminalReporter for RecordingTerminalReporter {
        fn completion_settlement_owner(&self) -> CompletionSettlementOwner {
            self.owner
        }

        fn report_done(
            &self,
            _claim: &CompletionClaim,
            report: &TerminalReport,
        ) -> Result<SignalOutcome, ExecutorError> {
            self.reports.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(ExecutorError::Storage(
                    "injected terminal transaction rollback".into(),
                ));
            }
            self.settlements.lock().unwrap().push(report.usage);
            Ok(SignalOutcome::Buffered)
        }

        fn report_retryable_attempt(
            &self,
            _claim: &CompletionClaim,
            failure: &RetryableAttemptFailure,
        ) -> Result<RetryableAttemptOutcome, ExecutorError> {
            self.retry_reports.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(ExecutorError::Storage(
                    "injected retryable-attempt transaction rollback".into(),
                ));
            }
            self.settlements.lock().unwrap().push(failure.usage);
            self.retry_causes.lock().unwrap().push(failure.cause);
            Ok(RetryableAttemptOutcome::Requeued)
        }

        fn report_preparation_terminal(
            &self,
            claim: &PreparationReportClaim,
            disposition: PreparationTerminalDisposition,
            diagnostic: Option<&str>,
        ) -> Result<SignalOutcome, ExecutorError> {
            if self.fail {
                return Err(ExecutorError::Storage(
                    "injected preparation-terminal transaction rollback".into(),
                ));
            }
            self.prep_terminals.lock().unwrap().push((
                claim.clone(),
                disposition,
                diagnostic.map(str::to_owned),
            ));
            Ok(SignalOutcome::Buffered)
        }

        fn report_preparation_retry(
            &self,
            claim: &PreparationReportClaim,
        ) -> Result<PreparationRetryReport, ExecutorError> {
            if self.fail {
                return Err(ExecutorError::Storage(
                    "injected preparation-retry transaction rollback".into(),
                ));
            }
            self.prep_retries.lock().unwrap().push(claim.clone());
            Ok(PreparationRetryReport::Requeued)
        }
    }

    struct FailingFirehose;
    impl FirehoseSink for FailingFirehose {
        fn ship_frame(
            &self,
            _run_id: &str,
            _job_id: &str,
            _tenant: &TenantId,
            _frame: &[u8],
        ) -> Result<(), String> {
            Err("injected durable log outage".into())
        }

        fn finish(
            &self,
            _run_id: &str,
            _job_id: &str,
            _tenant: &TenantId,
            _passed: bool,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct BlockingStreamingBackend {
        cancellation_observed: std::sync::atomic::AtomicBool,
    }

    impl SandboxBackend for BlockingStreamingBackend {
        type Error = HookError;

        fn launch(
            &self,
            _spec: &JobSpec,
            _hooks: &RunnerHooks,
        ) -> Result<SandboxLaunch, SandboxLaunchError<Self::Error>> {
            Err(SandboxLaunchError::Failed(HookError(
                "blocking backend must use launch_streaming".into(),
            )))
        }

        fn launch_streaming(
            &self,
            spec: &JobSpec,
            hooks: &RunnerHooks,
            output: Arc<dyn SandboxOutputSink>,
            cancellation: SandboxCancellation,
        ) -> Result<SandboxLaunch, SandboxLaunchError<Self::Error>> {
            (|| -> Result<SandboxLaunch, HookError> {
                hooks.enforce_isolation_floor(spec)?;
                let reserve = hooks.reserve(spec)?;
                hooks.attribute(spec)?;
                output
                    .emit(SandboxOutputStream::Stdout, b"live-frame")
                    .map_err(HookError)?;

                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while !cancellation.is_cancelled() {
                    if std::time::Instant::now() >= deadline {
                        return Err(HookError(
                            "runner never cancelled the live backend after sink failure".into(),
                        ));
                    }
                    std::thread::yield_now();
                }
                self.cancellation_observed.store(true, Ordering::SeqCst);
                hooks.settle_completed(
                    spec,
                    &reserve,
                    ResourceUsage {
                        cpu_seconds: 1,
                        mem_byte_seconds: 1,
                    },
                )?;
                Ok(SandboxLaunch {
                    handle: SandboxHandle {
                        guest_id: format!("cancelled-{}", spec.idem_token.0),
                    },
                    result: SandboxResult {
                        exit_code: None,
                        timed_out: false,
                        usage: ResourceUsage {
                            cpu_seconds: 1,
                            mem_byte_seconds: 1,
                        },
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                    },
                    output_complete: false,
                })
            })()
            .map_err(SandboxLaunchError::Failed)
        }

        fn kill(&self, _h: &SandboxHandle) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    fn hooks_with_owner(
        owner: CompletionSettlementOwner,
        hook_settlements: Arc<Mutex<Vec<ResourceUsage>>>,
    ) -> RunnerHooks {
        RunnerHooks::new(
            owner,
            Box::new(|spec| {
                Ok(ReserveHandle(format!(
                    "reserved:{}",
                    spec.meter_to.reserve_id
                )))
            }),
            Box::new(move |_spec, _handle, usage| {
                hook_settlements.lock().unwrap().push(usage);
                Ok(())
            }),
            Box::new(|_spec| Ok(())),
            Box::new(|_spec| Ok(())),
        )
    }

    fn started_run() -> (FlowExecutor, RunId) {
        let ex = FlowExecutor::new(Arc::new(MonotonicMinter::new()), tenant(), region());
        ex.register_definition("ci.pipeline");
        let run = ex
            .start(StartSpec {
                wf_type: "ci.pipeline".into(),
                input: vec![],
                budget: None,
                idem_key: "ci:run-1".into(),
            })
            .expect("start the ci.pipeline run");
        (ex, run)
    }

    #[test]
    fn lease_claim_heartbeat_renew_and_expiry_reclaim() {
        let q = JobLeaseStore::new();
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            "run-1",
            "job-1",
            vec!["linux".into()],
            ci_spec("idem-1"),
        ));
        let tiers = [TrustTier::Trusted];

        let claimed = q
            .claim_for_labels("worker-1", &["linux".into()], &tiers, &region(), 1000, 30)
            .expect("worker-1 claims the eligible job");
        assert_eq!(claimed.job_id, "job-1");
        assert_eq!(q.get(&tenant(), "job-1").unwrap().lease_expires, Some(1030));

        assert!(q.heartbeat("worker-1", &tenant(), "job-1", 1020, 30));
        assert_eq!(q.get(&tenant(), "job-1").unwrap().lease_expires, Some(1050));

        assert!(
            q.claim_for_labels("worker-2", &["linux".into()], &tiers, &region(), 1040, 30)
                .is_none(),
            "a live lease is skipped - no two runners run the same job"
        );

        assert!(
            !q.heartbeat("worker-2", &tenant(), "job-1", 1040, 30),
            "a non-owner cannot heartbeat the lease"
        );

        let reclaimed = q
            .claim_for_labels("worker-2", &["linux".into()], &tiers, &region(), 1100, 30)
            .expect("worker-2 reclaims the EXPIRED lease");
        assert_eq!(reclaimed.job_id, "job-1");
        assert_eq!(reclaimed.lease_owner.as_deref(), Some("worker-2"));
        assert_eq!(q.get(&tenant(), "job-1").unwrap().lease_expires, Some(1130));
    }

    #[test]
    fn claim_respects_region_affinity_and_trust() {
        let q = JobLeaseStore::new();
        q.enqueue(QueuedJob::new(
            tenant(),
            Region("de-fra".into()),
            "run-x",
            "job-region",
            vec!["linux".into()],
            ci_spec("i1"),
        ));
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            "run-y",
            "job-label",
            vec!["gpu".into()],
            ci_spec("i2"),
        ));
        let mut fork_spec = ci_spec("i3");
        fork_spec.trust_tier = TrustTier::UntrustedFork;
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            "run-z",
            "job-trust",
            vec!["linux".into()],
            fork_spec,
        ));

        let none = q.claim_for_labels(
            "worker-1",
            &["linux".into()],
            &[TrustTier::Trusted],
            &region(),
            1000,
            30,
        );
        assert!(
            none.is_none(),
            "no eligible job: out-of-region / wrong-label / untrusted are all skipped"
        );

        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            "run-ok",
            "job-ok",
            vec!["linux".into()],
            ci_spec("i4"),
        ));
        let ok = q
            .claim_for_labels(
                "worker-2",
                &["linux".into(), "gpu".into()],
                &[TrustTier::Trusted, TrustTier::UntrustedFork],
                &region(),
                1000,
                30,
            )
            .expect("an eligible job is claimable");
        assert!(["job-label", "job-ok", "job-trust"].contains(&ok.job_id.as_str()));
    }

    fn prep_report_claim() -> PreparationReportClaim {
        PreparationReportClaim {
            tenant_id: "acme".into(),
            region: "fr-par".into(),
            project_id: "00000000-0000-0000-0000-000000000001".into(),
            wf_run_id: "11111111-1111-1111-1111-111111111111".into(),
            ci_run_id: "44444444-4444-4444-4444-444444444444".into(),
            job_id: "22222222-2222-2222-2222-222222222222".into(),
            token_authority_handle: "tah-xyz".into(),
            idem_token: "11111111-1111-1111-1111-111111111111/build".into(),
            lease_owner: "worker-1".into(),
            lease_epoch: 7,
            claim_nonce: "33333333-3333-3333-3333-333333333333".into(),
            claim_started_at_epoch_secs: 1_000,
            claim_expires_at_epoch_secs: 1_300,
        }
    }

    fn cycle_agent<'a>(
        leases: JobLeaseStore,
        backend: &'a CycleOnlyBackend,
        firehose: &'a CountingFirehose,
        reporter: &'a RecordingTerminalReporter,
    ) -> RunnerAgent<'a, CycleOnlyBackend, CountingFirehose, RecordingTerminalReporter> {
        RunnerAgent::new(
            "worker-1",
            vec!["linux".into()],
            vec![TrustTier::Trusted],
            region(),
            30,
            leases,
            backend,
            firehose,
            reporter,
            hooks_with_owner(
                CompletionSettlementOwner::TerminalReporter,
                Arc::new(Mutex::new(Vec::new())),
            ),
        )
    }

    #[test]
    fn stage_b_runner_edge_calls_run_cycle_and_never_launch_streaming() {
        let leases = JobLeaseStore::new();
        leases.enqueue(QueuedJob::new(
            tenant(),
            region(),
            "run-1",
            "job-1",
            vec!["linux".into()],
            ci_spec("cycle-edge"),
        ));
        let backend = CycleOnlyBackend::new(CycleMode::Workload);
        let firehose = CountingFirehose::new();
        let reporter =
            RecordingTerminalReporter::reporter_owned(Arc::new(Mutex::new(Vec::new())), false);
        let agent = cycle_agent(leases.clone(), &backend, &firehose, &reporter);

        let outcome = agent
            .run_one_cycle(1_000)
            .expect("typed workload cycle succeeds");
        assert!(matches!(outcome, RunnerCycleOutcome::Workload(_)));
        assert_eq!(backend.cycle_calls.load(Ordering::SeqCst), 1);
        assert_eq!(backend.kills.load(Ordering::SeqCst), 1);
        assert_eq!(reporter.reports.load(Ordering::SeqCst), 1);
        assert!(
            leases.get(&tenant(), "job-1").is_none(),
            "workload completion settles the lease"
        );
    }

    #[test]
    fn stage_b_runner_routes_each_preparation_outcome_once_without_workload_completion() {
        for (mode, terminal) in [
            (CycleMode::PreparationTerminal, true),
            (CycleMode::PreparationRetryable, false),
        ] {
            let leases = JobLeaseStore::new();
            leases.enqueue(QueuedJob::new(
                tenant(),
                region(),
                "run-1",
                "job-1",
                vec!["linux".into()],
                ci_spec("prep-edge"),
            ));
            let backend = CycleOnlyBackend::new(mode);
            let firehose = CountingFirehose::new();
            let reporter =
                RecordingTerminalReporter::reporter_owned(Arc::new(Mutex::new(Vec::new())), false);
            let agent = cycle_agent(leases.clone(), &backend, &firehose, &reporter);

            let outcome = agent
                .run_one_cycle(1_000)
                .expect("preparation route succeeds");
            if terminal {
                assert!(matches!(
                    outcome,
                    RunnerCycleOutcome::PreparationTerminal { .. }
                ));
                assert_eq!(reporter.prep_terminals.lock().unwrap().len(), 1);
                assert_eq!(reporter.prep_retries.lock().unwrap().len(), 0);
            } else {
                assert!(matches!(
                    outcome,
                    RunnerCycleOutcome::PreparationRetryable { .. }
                ));
                assert_eq!(reporter.prep_terminals.lock().unwrap().len(), 0);
                assert_eq!(reporter.prep_retries.lock().unwrap().len(), 1);
            }
            assert_eq!(backend.cycle_calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                backend.kills.load(Ordering::SeqCst),
                0,
                "no workload handle existed"
            );
            assert_eq!(
                reporter.reports.load(Ordering::SeqCst),
                0,
                "no job.done workload report"
            );
            assert_eq!(
                reporter.retry_reports.load(Ordering::SeqCst),
                0,
                "no workload retry report"
            );
            assert!(
                leases.get(&tenant(), "job-1").is_some(),
                "preparation reporter owns queue settlement/requeue"
            );
        }
    }

    #[test]
    fn report_preparation_outcome_dispatches_terminal_retry_and_ignores_non_preparation() {
        use crate::SandboxCycleOutcome;
        let backend = RecordingBackend::default();
        let firehose = CountingFirehose::new();
        let reporter =
            RecordingTerminalReporter::reporter_owned(Arc::new(Mutex::new(Vec::new())), false);
        let agent = RunnerAgent::new(
            "worker-1",
            vec!["linux".into()],
            vec![TrustTier::Trusted],
            region(),
            30,
            JobLeaseStore::new(),
            &backend,
            &firehose,
            &reporter,
            test_hooks(),
        );
        let claim = prep_report_claim();

        let disposition = PreparationTerminalDisposition::Failed {
            phase: PreparationPhase::CheckoutTransport,
        };
        let diagnostic = "host-side HEAD re-verification disagreed: injected mismatch";
        let dispatched = agent
            .report_preparation_outcome(&SandboxCycleOutcome::PreparationTerminal {
                claim: claim.clone(),
                disposition,
                diagnostic: Some(diagnostic.to_string()),
            })
            .expect("dispatches the terminal");
        assert_eq!(
            dispatched,
            PreparationOutcomeDispatch::Terminalized(SignalOutcome::Buffered)
        );
        {
            let recorded = reporter.prep_terminals.lock().unwrap();
            assert_eq!(recorded.len(), 1);
            assert_eq!(
                recorded[0].0, claim,
                "the reporter got the outcome's exact claim"
            );
            assert_eq!(recorded[0].1, disposition);
            assert_eq!(recorded[0].2.as_deref(), Some(diagnostic));
        }

        let dispatched = agent
            .report_preparation_outcome(&SandboxCycleOutcome::PreparationRetryable {
                claim: claim.clone(),
                phase: PreparationPhase::CheckoutMaterialization,
            })
            .expect("dispatches the retry");
        assert_eq!(
            dispatched,
            PreparationOutcomeDispatch::Retried(PreparationRetryReport::Requeued)
        );
        assert_eq!(*reporter.prep_retries.lock().unwrap(), vec![claim.clone()]);

        let dispatched = agent
            .report_preparation_outcome(&SandboxCycleOutcome::WorkloadRetryable {
                cause: RetryableAttemptCause::SandboxInfrastructure,
                usage: ResourceUsage {
                    cpu_seconds: 0,
                    mem_byte_seconds: 0,
                },
                message: "post-settle failure".into(),
            })
            .expect("classifies the non-preparation outcome");
        assert_eq!(
            dispatched,
            PreparationOutcomeDispatch::NotAPreparationReport
        );
        assert_eq!(reporter.prep_terminals.lock().unwrap().len(), 1);
        assert_eq!(reporter.prep_retries.lock().unwrap().len(), 1);
    }

    #[test]
    fn engine_terminal_reporter_refuses_both_preparation_reports() {
        let (ex, _run) = started_run();
        let reporter = EngineTerminalReporter::new(ex);
        let claim = prep_report_claim();
        assert!(
            reporter
                .report_preparation_terminal(
                    &claim,
                    PreparationTerminalDisposition::AttemptsExhausted,
                    None,
                )
                .is_err(),
            "the engine reporter must refuse a preparation terminal"
        );
        assert!(
            reporter.report_preparation_retry(&claim).is_err(),
            "the engine reporter must refuse a preparation retry"
        );
    }

    #[test]
    fn runner_agent_claims_launches_and_reports_terminal() {
        let (ex, run) = started_run();
        let idem = job_idem_token(&run.0, "ci.pipeline:0");
        let q = JobLeaseStore::new();
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            &run.0,
            "job-1",
            vec!["linux".into()],
            ci_spec(&idem),
        ));

        let backend = RecordingBackend::default();
        let firehose = CountingFirehose::new();
        let reporter = EngineTerminalReporter::new(ex.clone());
        let agent = RunnerAgent::new(
            "worker-1",
            vec!["linux".into()],
            vec![TrustTier::Trusted],
            region(),
            30,
            q.clone(),
            &backend,
            &firehose,
            &reporter,
            test_hooks(),
        );

        let outcome = agent
            .run_one(1000)
            .expect("the runner runs the job and reports terminal");

        assert_eq!(outcome.job_id, "job-1");
        assert_eq!(outcome.run_id, run.0);
        assert!(
            outcome.report.passed,
            "a clean exit (0, not timed out) derives passed=true"
        );
        assert!(outcome.report.result_refs.is_empty());
        assert_eq!(
            outcome.signal_outcome,
            SignalOutcome::Buffered,
            "the FIRST job.done delivery wakes the parked workflow"
        );
        assert_eq!(backend.launches.load(Ordering::SeqCst), 1);
        assert_eq!(backend.kills.load(Ordering::SeqCst), 1);
        assert_eq!(firehose.frames_shipped(), 1);
        assert!(
            q.get(&tenant(), "job-1").is_none(),
            "the lease is settled on terminal"
        );
        assert_eq!(ex.signals().count_for_run(&tenant(), &run.0), 1);
    }

    #[test]
    fn durable_log_failure_kills_the_guest_and_refuses_a_terminal_verdict() {
        let q = JobLeaseStore::new();
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            "run-log-fail",
            "job-log-fail",
            vec!["linux".into()],
            ci_spec("idem-log-fail"),
        ));
        let backend = RecordingBackend::default();
        let firehose = FailingFirehose;
        let reporter_settlements = Arc::new(Mutex::new(Vec::new()));
        let reporter =
            RecordingTerminalReporter::reporter_owned(reporter_settlements.clone(), false);
        let hook_settlements = Arc::new(Mutex::new(Vec::new()));
        let agent = RunnerAgent::new(
            "worker-log-fail",
            vec!["linux".into()],
            vec![TrustTier::Trusted],
            region(),
            30,
            q,
            &backend,
            &firehose,
            &reporter,
            hooks_with_owner(
                CompletionSettlementOwner::TerminalReporter,
                hook_settlements.clone(),
            ),
        );

        let error = agent
            .run_one(1000)
            .expect_err("output failure is retryable and cannot become job.done");
        assert_eq!(backend.kills.load(Ordering::SeqCst), 1);
        assert!(matches!(
            error,
            RunnerError::RetryableAttemptRecorded { ref job_id, .. }
                if job_id == "job-log-fail"
        ));
        assert_eq!(reporter.reports.load(Ordering::SeqCst), 0);
        assert_eq!(
            reporter.retry_reports.load(Ordering::SeqCst),
            1,
            "one retryable measured attempt is handed to the claim-aware reporter"
        );
        assert!(hook_settlements.lock().unwrap().is_empty());
    }

    #[test]
    fn run_one_routes_a_launch_level_retryable_attempt_to_the_reporter_with_exact_cause_and_usage()
    {
        let q = JobLeaseStore::new();
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            "run-launch-retry",
            "job-launch-retry",
            vec!["linux".into()],
            ci_spec("idem-launch-retry"),
        ));
        let forced_usage = ResourceUsage {
            cpu_seconds: 9,
            mem_byte_seconds: 900,
        };
        let backend = RecordingBackend {
            forced_launch_error: Some(SandboxLaunchError::RetryableAttempt {
                source: HookError("injected sandbox infrastructure failure".into()),
                cause: RetryableAttemptCause::SandboxInfrastructure,
                usage: forced_usage,
            }),
            ..RecordingBackend::default()
        };
        let firehose = CountingFirehose::new();
        let reporter_settlements = Arc::new(Mutex::new(Vec::new()));
        let reporter =
            RecordingTerminalReporter::reporter_owned(reporter_settlements.clone(), false);
        let hook_settlements = Arc::new(Mutex::new(Vec::new()));
        let agent = RunnerAgent::new(
            "worker-launch-retry",
            vec!["linux".into()],
            vec![TrustTier::Trusted],
            region(),
            30,
            q,
            &backend,
            &firehose,
            &reporter,
            hooks_with_owner(
                CompletionSettlementOwner::TerminalReporter,
                hook_settlements.clone(),
            ),
        );

        let error = agent
            .run_one(1000)
            .expect_err("a launch-level retryable attempt is never a terminal success");
        assert!(matches!(
            error,
            RunnerError::RetryableAttemptRecorded { ref job_id, ref message }
                if job_id == "job-launch-retry"
                    && message == "runner hook failed: injected sandbox infrastructure failure"
        ));
        assert_eq!(
            reporter.reports.load(Ordering::SeqCst),
            0,
            "no job.done emitted"
        );
        assert_eq!(reporter.retry_reports.load(Ordering::SeqCst), 1);
        assert_eq!(*reporter_settlements.lock().unwrap(), vec![forced_usage]);
        assert_eq!(
            *reporter.retry_causes.lock().unwrap(),
            vec![RetryableAttemptCause::SandboxInfrastructure]
        );
        assert!(
            hook_settlements.lock().unwrap().is_empty(),
            "the Hook-side settle closure must never fire directly for a reporter-routed attempt"
        );
        assert_eq!(
            backend.kills.load(Ordering::SeqCst),
            0,
            "no SandboxLaunch/handle ever existed to kill - the backend already tore itself down"
        );
    }

    #[test]
    fn run_one_surfaces_reporter_failure_as_report_failed_for_a_launch_level_retryable_attempt() {
        let q = JobLeaseStore::new();
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            "run-launch-retry-fail",
            "job-launch-retry-fail",
            vec!["linux".into()],
            ci_spec("idem-launch-retry-fail"),
        ));
        let backend = RecordingBackend {
            forced_launch_error: Some(SandboxLaunchError::RetryableAttempt {
                source: HookError("injected sandbox infrastructure failure".into()),
                cause: RetryableAttemptCause::SandboxInfrastructure,
                usage: ResourceUsage {
                    cpu_seconds: 1,
                    mem_byte_seconds: 1,
                },
            }),
            ..RecordingBackend::default()
        };
        let firehose = CountingFirehose::new();
        let reporter_settlements = Arc::new(Mutex::new(Vec::new()));
        let reporter = RecordingTerminalReporter::reporter_owned(reporter_settlements, true);
        let hook_settlements = Arc::new(Mutex::new(Vec::new()));
        let agent = RunnerAgent::new(
            "worker-launch-retry-fail",
            vec!["linux".into()],
            vec![TrustTier::Trusted],
            region(),
            30,
            q,
            &backend,
            &firehose,
            &reporter,
            hooks_with_owner(
                CompletionSettlementOwner::TerminalReporter,
                hook_settlements,
            ),
        );

        let error = agent
            .run_one(1000)
            .expect_err("a rolled-back retryable-attempt report must surface, not succeed");
        assert!(
            matches!(error, RunnerError::ReportFailed(_)),
            "got {error:?}"
        );
    }

    #[test]
    fn run_one_never_reports_or_settles_a_durable_outcome_unknown_launch_failure() {
        let q = JobLeaseStore::new();
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            "run-outcome-unknown",
            "job-outcome-unknown",
            vec!["linux".into()],
            ci_spec("idem-outcome-unknown"),
        ));
        let backend = RecordingBackend {
            forced_launch_error: Some(SandboxLaunchError::DurableOutcomeUnknown(HookError(
                "injected ambiguous commit outcome".into(),
            ))),
            ..RecordingBackend::default()
        };
        let firehose = CountingFirehose::new();
        let reporter_settlements = Arc::new(Mutex::new(Vec::new()));
        let reporter =
            RecordingTerminalReporter::reporter_owned(reporter_settlements.clone(), false);
        let hook_settlements = Arc::new(Mutex::new(Vec::new()));
        let agent = RunnerAgent::new(
            "worker-outcome-unknown",
            vec!["linux".into()],
            vec![TrustTier::Trusted],
            region(),
            30,
            q,
            &backend,
            &firehose,
            &reporter,
            hooks_with_owner(
                CompletionSettlementOwner::TerminalReporter,
                hook_settlements.clone(),
            ),
        );

        let error = agent
            .run_one(1000)
            .expect_err("an outcome-unknown launch failure is never a terminal success");
        assert!(
            matches!(error, RunnerError::LaunchFailed(ref msg) if msg.contains("reconcil")),
            "expected a loud LaunchFailed naming reconciliation ownership, got {error:?}"
        );
        assert_eq!(reporter.reports.load(Ordering::SeqCst), 0);
        assert_eq!(
            reporter.retry_reports.load(Ordering::SeqCst),
            0,
            "an outcome-unknown failure must never be reported as a retryable attempt"
        );
        assert!(reporter_settlements.lock().unwrap().is_empty());
        assert!(hook_settlements.lock().unwrap().is_empty());
    }

    #[test]
    fn reporter_owned_log_failure_cancels_live_execution_and_accounts_exactly_once() {
        let q = JobLeaseStore::new();
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            "run-live-log-fail",
            "job-live-log-fail",
            vec!["linux".into()],
            ci_spec("idem-live-log-fail"),
        ));
        let backend = BlockingStreamingBackend::default();
        let firehose = FailingFirehose;
        let reporter_settlements = Arc::new(Mutex::new(Vec::new()));
        let reporter =
            RecordingTerminalReporter::reporter_owned(reporter_settlements.clone(), false);
        let hook_settlements = Arc::new(Mutex::new(Vec::new()));
        let agent = RunnerAgent::new(
            "worker-live-log-fail",
            vec!["linux".into()],
            vec![TrustTier::Trusted],
            region(),
            30,
            q,
            &backend,
            &firehose,
            &reporter,
            hooks_with_owner(
                CompletionSettlementOwner::TerminalReporter,
                hook_settlements.clone(),
            ),
        );

        let started = std::time::Instant::now();
        let error = agent
            .run_one(1000)
            .expect_err("a failed durable frame is requeued without job.done");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "cancellation is prompt rather than waiting for the command timeout"
        );
        assert!(
            backend.cancellation_observed.load(Ordering::SeqCst),
            "the live backend watchdog observed the runner cancellation"
        );
        assert!(matches!(
            error,
            RunnerError::RetryableAttemptRecorded { ref job_id, .. }
                if job_id == "job-live-log-fail"
        ));
        assert_eq!(
            reporter.reports.load(Ordering::SeqCst),
            0,
            "a retryable infrastructure failure cannot emit job.done"
        );
        assert_eq!(
            reporter.retry_reports.load(Ordering::SeqCst),
            1,
            "the reporter-owned topology receives one retryable-attempt transaction"
        );
        assert_eq!(
            *reporter_settlements.lock().unwrap(),
            vec![ResourceUsage {
                cpu_seconds: 1,
                mem_byte_seconds: 1,
            }],
            "the terminal reporter owns the measured failed-attempt accrual"
        );
        assert!(
            hook_settlements.lock().unwrap().is_empty(),
            "the backend hook cannot double-settle reporter-owned completion"
        );
    }

    #[test]
    fn settlement_owner_mismatch_refuses_before_claim_or_launch() {
        let q = JobLeaseStore::new();
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            "run-1",
            "job-1",
            vec!["linux".into()],
            ci_spec("idem-1"),
        ));
        let backend = RecordingBackend::default();
        let firehose = CountingFirehose::new();
        let reporter_settlements = Arc::new(Mutex::new(Vec::new()));
        let reporter =
            RecordingTerminalReporter::reporter_owned(reporter_settlements.clone(), false);
        let hook_settlements = Arc::new(Mutex::new(Vec::new()));
        let agent = RunnerAgent::new(
            "worker-1",
            vec!["linux".into()],
            vec![TrustTier::Trusted],
            region(),
            30,
            q.clone(),
            &backend,
            &firehose,
            &reporter,
            hooks_with_owner(CompletionSettlementOwner::Hook, hook_settlements.clone()),
        );

        assert!(matches!(
            agent.run_one(1000),
            Err(RunnerError::SettlementOwnerMismatch {
                hooks: CompletionSettlementOwner::Hook,
                reporter: CompletionSettlementOwner::TerminalReporter,
            })
        ));
        let queued = q.get(&tenant(), "job-1").expect("job remains queued");
        assert_eq!(
            queued.lease_owner, None,
            "the mismatch cannot claim the job"
        );
        assert_eq!(backend.launches.load(Ordering::SeqCst), 0);
        assert_eq!(reporter.reports.load(Ordering::SeqCst), 0);
        assert!(hook_settlements.lock().unwrap().is_empty());
        assert!(reporter_settlements.lock().unwrap().is_empty());

        let hook_reporter = RecordingTerminalReporter {
            owner: CompletionSettlementOwner::Hook,
            reports: AtomicUsize::new(0),
            retry_reports: AtomicUsize::new(0),
            settlements: reporter_settlements.clone(),
            retry_causes: Arc::new(Mutex::new(Vec::new())),
            prep_terminals: Arc::new(Mutex::new(Vec::new())),
            prep_retries: Arc::new(Mutex::new(Vec::new())),
            fail: false,
        };
        let unowned = RunnerAgent::new(
            "worker-1",
            vec!["linux".into()],
            vec![TrustTier::Trusted],
            region(),
            30,
            q.clone(),
            &backend,
            &firehose,
            &hook_reporter,
            hooks_with_owner(
                CompletionSettlementOwner::TerminalReporter,
                hook_settlements.clone(),
            ),
        );
        assert!(matches!(
            unowned.run_one(1000),
            Err(RunnerError::SettlementOwnerMismatch {
                hooks: CompletionSettlementOwner::TerminalReporter,
                reporter: CompletionSettlementOwner::Hook,
            })
        ));
        assert_eq!(
            q.get(&tenant(), "job-1")
                .expect("job remains queued")
                .lease_owner,
            None,
            "the inverse mismatch also cannot claim the job"
        );
        assert_eq!(backend.launches.load(Ordering::SeqCst), 0);
        assert_eq!(hook_reporter.reports.load(Ordering::SeqCst), 0);
        assert!(hook_settlements.lock().unwrap().is_empty());
        assert!(reporter_settlements.lock().unwrap().is_empty());
    }

    #[test]
    fn reporter_owned_completion_is_single_owner_across_rollback_and_retry() {
        let q = JobLeaseStore::new();
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            "run-1",
            "job-1",
            vec!["linux".into()],
            ci_spec("idem-1"),
        ));
        let backend = RecordingBackend::default();
        let firehose = CountingFirehose::new();
        let hook_settlements = Arc::new(Mutex::new(Vec::new()));
        let reporter_settlements = Arc::new(Mutex::new(Vec::new()));
        let failing_reporter =
            RecordingTerminalReporter::reporter_owned(reporter_settlements.clone(), true);
        let first = RunnerAgent::new(
            "worker-1",
            vec!["linux".into()],
            vec![TrustTier::Trusted],
            region(),
            30,
            q.clone(),
            &backend,
            &firehose,
            &failing_reporter,
            hooks_with_owner(
                CompletionSettlementOwner::TerminalReporter,
                hook_settlements.clone(),
            ),
        );

        assert!(matches!(
            first.run_one(1000),
            Err(RunnerError::ReportFailed(ExecutorError::Storage(_)))
        ));
        let claimed = q
            .get(&tenant(), "job-1")
            .expect("rolled-back terminal report leaves the claim for retry");
        assert_eq!(claimed.lease_owner.as_deref(), Some("worker-1"));
        assert!(hook_settlements.lock().unwrap().is_empty());
        assert!(reporter_settlements.lock().unwrap().is_empty());

        let successful_reporter =
            RecordingTerminalReporter::reporter_owned(reporter_settlements.clone(), false);
        let retry = RunnerAgent::new(
            "worker-2",
            vec!["linux".into()],
            vec![TrustTier::Trusted],
            region(),
            30,
            q.clone(),
            &backend,
            &firehose,
            &successful_reporter,
            hooks_with_owner(
                CompletionSettlementOwner::TerminalReporter,
                hook_settlements.clone(),
            ),
        );
        let outcome = retry
            .run_one(1031)
            .expect("the expired claim is reclaimed and terminal accounting retries");
        assert_eq!(outcome.job_id, "job-1");
        assert_eq!(backend.launches.load(Ordering::SeqCst), 2);
        assert!(hook_settlements.lock().unwrap().is_empty());
        assert_eq!(
            *reporter_settlements.lock().unwrap(),
            vec![ResourceUsage {
                cpu_seconds: 1,
                mem_byte_seconds: 1,
            }],
            "only the successful terminal transaction settles measured usage"
        );
        assert!(q.get(&tenant(), "job-1").is_none());
    }

    #[test]
    fn double_delivered_job_done_wakes_the_workflow_exactly_once() {
        let (ex, run) = started_run();
        let idem = job_idem_token(&run.0, "ci.pipeline:0");
        let q = JobLeaseStore::new();
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            &run.0,
            "job-1",
            vec!["linux".into()],
            ci_spec(&idem),
        ));

        let backend = RecordingBackend::default();
        let firehose = CountingFirehose::new();
        let reporter = EngineTerminalReporter::new(ex.clone());
        let agent = RunnerAgent::new(
            "worker-1",
            vec!["linux".into()],
            vec![TrustTier::Trusted],
            region(),
            30,
            q.clone(),
            &backend,
            &firehose,
            &reporter,
            test_hooks(),
        );

        let first = agent.run_one(1000).expect("first cycle");
        assert_eq!(first.signal_outcome, SignalOutcome::Buffered);

        let again = agent
            .report_done_again(
                &CompletionClaim {
                    tenant: tenant(),
                    run: run.clone(),
                    job_id: "job-1".into(),
                    idem_token: idem.clone(),
                    lease_owner: "worker-1".into(),
                    lease_epoch: first.lease_epoch,
                    claim_nonce: first.claim_nonce.clone(),
                },
                &first.report,
            )
            .expect("re-delivery is the idempotency working, not an error");
        assert_eq!(
            again,
            SignalOutcome::Duplicate,
            "the SECOND job.done is a no-op (ON CONFLICT DO NOTHING - double-effect = 0)"
        );

        assert_eq!(
            ex.signals().count_for_run(&tenant(), &run.0),
            1,
            "double-effect = 0: a doubly-delivered job.done buffers ONCE (the workflow wakes once)"
        );
    }

    #[test]
    fn a_launch_refusal_fails_closed_with_no_terminal_report() {
        let (ex, run) = started_run();
        let idem = job_idem_token(&run.0, "ci.pipeline:0");
        let q = JobLeaseStore::new();
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            &run.0,
            "job-1",
            vec!["linux".into()],
            ci_spec(&idem),
        ));

        let backend = RecordingBackend {
            fail_launch: true,
            ..Default::default()
        };
        let firehose = CountingFirehose::new();
        let reporter = EngineTerminalReporter::new(ex.clone());
        let agent = RunnerAgent::new(
            "worker-1",
            vec!["linux".into()],
            vec![TrustTier::Trusted],
            region(),
            30,
            q.clone(),
            &backend,
            &firehose,
            &reporter,
            test_hooks(),
        );

        let err = agent
            .run_one(1000)
            .expect_err("a launch refusal fails closed");
        assert!(matches!(err, RunnerError::LaunchFailed(_)));
        assert_eq!(
            ex.signals().count_for_run(&tenant(), &run.0),
            0,
            "a failed launch reports NO terminal - the dispatch activity retries (no false wake)"
        );
    }

    #[test]
    fn no_claimable_job_surfaces_no_work() {
        let (ex, _run) = started_run();
        let q = JobLeaseStore::new();
        let backend = RecordingBackend::default();
        let firehose = CountingFirehose::new();
        let reporter = EngineTerminalReporter::new(ex.clone());
        let agent = RunnerAgent::new(
            "worker-1",
            vec!["linux".into()],
            vec![TrustTier::Trusted],
            region(),
            30,
            q,
            &backend,
            &firehose,
            &reporter,
            test_hooks(),
        );
        let err = agent
            .run_one(1000)
            .expect_err("an empty queue surfaces NoWork");
        assert!(matches!(err, RunnerError::NoWork));
        assert_eq!(
            backend.launches.load(Ordering::SeqCst),
            0,
            "nothing launched on NoWork"
        );
    }

    #[test]
    fn claimable_depth_counts_free_eligible_leases() {
        let q = JobLeaseStore::new();
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            "r",
            "j1",
            vec!["linux".into()],
            ci_spec("a"),
        ));
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            "r",
            "j2",
            vec!["linux".into()],
            ci_spec("b"),
        ));
        assert_eq!(q.claimable_depth(&["linux".into()], &region(), 1000), 2);

        q.claim_for_labels(
            "w1",
            &["linux".into()],
            &[TrustTier::Trusted],
            &region(),
            1000,
            30,
        );
        assert_eq!(q.claimable_depth(&["linux".into()], &region(), 1010), 1);
        assert_eq!(q.claimable_depth(&["linux".into()], &region(), 2000), 2);
    }

    #[test]
    fn terminal_report_is_references_not_payloads_keyed_on_idem_token() {
        let (ex, run) = started_run();
        let idem = job_idem_token(&run.0, "ci.pipeline:0");
        let q = JobLeaseStore::new();
        q.enqueue(QueuedJob::new(
            tenant(),
            region(),
            &run.0,
            "job-1",
            vec!["linux".into()],
            ci_spec(&idem),
        ));
        let backend = RecordingBackend {
            result: SandboxResult {
                exit_code: Some(2),
                timed_out: false,
                usage: ResourceUsage {
                    cpu_seconds: 1,
                    mem_byte_seconds: 1,
                },
                stdout: Vec::new(),
                stderr: b"compile error: E0001".to_vec(),
            },
            ..Default::default()
        };
        let firehose = CountingFirehose::new();
        let reporter = EngineTerminalReporter::new(ex.clone());
        let agent = RunnerAgent::new(
            "worker-1",
            vec!["linux".into()],
            vec![TrustTier::Trusted],
            region(),
            30,
            q,
            &backend,
            &firehose,
            &reporter,
            test_hooks(),
        );
        agent.run_one(1000).expect("run");

        assert_eq!(firehose.frames_shipped(), 1);

        let row = ex
            .signals()
            .get(&tenant(), &run.0, JOB_DONE_SIGNAL, &idem)
            .expect("the job.done buffered under the echoed idem_token");
        assert_eq!(
            row.payload[0],
            ArtifactRef("myelin://job-done/passed-false".into())
        );
        assert_eq!(row.payload.len(), 1);
        assert_eq!(row.payload_key_ref, None, "no inline PII payload");
        for r in &row.payload {
            assert!(
                !r.0.contains("compile error"),
                "captured stream bytes must NEVER enter the engine signal payload"
            );
        }
    }

    #[test]
    fn runner_derives_terminal_report_from_the_sandbox_result() {
        fn run_with(result: SandboxResult) -> TerminalReport {
            let (ex, run) = started_run();
            let idem = job_idem_token(&run.0, "ci.pipeline:0");
            let q = JobLeaseStore::new();
            q.enqueue(QueuedJob::new(
                tenant(),
                region(),
                &run.0,
                "job-1",
                vec!["linux".into()],
                ci_spec(&idem),
            ));
            let backend = RecordingBackend {
                result,
                ..Default::default()
            };
            let firehose = CountingFirehose::new();
            let reporter = EngineTerminalReporter::new(ex.clone());
            let agent = RunnerAgent::new(
                "worker-1",
                vec!["linux".into()],
                vec![TrustTier::Trusted],
                region(),
                30,
                q,
                &backend,
                &firehose,
                &reporter,
                test_hooks(),
            );
            agent.run_one(1000).expect("run").report
        }

        fn usage() -> ResourceUsage {
            ResourceUsage {
                cpu_seconds: 1,
                mem_byte_seconds: 1,
            }
        }

        let passed = run_with(SandboxResult {
            exit_code: Some(0),
            timed_out: false,
            usage: usage(),
            stdout: Vec::new(),
            stderr: Vec::new(),
        });
        assert!(passed.passed);
        assert!(!passed.timed_out);
        assert_eq!(passed.usage, usage());
        assert!(
            !run_with(SandboxResult {
                exit_code: Some(1),
                timed_out: false,
                usage: usage(),
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
            .passed
        );
        let timed_out = run_with(SandboxResult {
            exit_code: None,
            timed_out: true,
            usage: usage(),
            stdout: Vec::new(),
            stderr: Vec::new(),
        });
        assert!(!timed_out.passed);
        assert!(timed_out.timed_out);
        assert_eq!(timed_out.usage, usage());
        assert!(
            !run_with(SandboxResult {
                exit_code: Some(0),
                timed_out: true,
                usage: usage(),
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
            .passed
        );
    }
}

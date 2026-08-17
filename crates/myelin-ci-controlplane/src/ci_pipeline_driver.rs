use std::collections::BTreeMap;
#[cfg(any(test, feature = "test-support"))]
use std::collections::HashMap;
use std::sync::Arc;
#[cfg(any(test, feature = "test-support"))]
use std::sync::Mutex;

use myelin_ci_sandbox::{
    derive_checkout_authorization_scope, CompletionClaim, CompletionSettlementOwner, IdemToken,
    JobKind, JobSpec as SandboxJobSpec, PreparationPhase, PreparationReportClaim,
    PreparationRetryReport, PreparationTerminalDisposition, ResourceUsage, RetryableAttemptCause,
    RetryableAttemptFailure, RetryableAttemptOutcome, TerminalReport, TerminalReporter,
};
#[cfg(any(test, feature = "test-support"))]
use myelin_ci_sandbox::{
    EgressPolicy, ImageRef, JobKind as SandboxJobKind, MeterTarget, ResourceLimits,
    RunTokenCredential, TrustTier, WorkspaceSpec,
};
#[cfg(any(test, feature = "test-support"))]
use myelin_events::{Actor, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp};
#[cfg(any(test, feature = "test-support"))]
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_storage::{
    with_tenant_tx_error, DurableCostLedger, DurableSettleError, MeteredUnit, MicroUsd, PgError,
    RunId as CostRunId, TenantScope,
};
use myelin_tenancy::Region;
use myelin_tenancy::TenantId;
use serde::{Deserialize, Serialize};
use sqlx::types::Uuid;
use sqlx::Row;

use myelin_flow::{
    ActivityError, ExecutorError, JobRunner, JobSpec as FlowJobSpec, PgFlowExecutor, RunId,
    SignalOutcome, SignalPayload, TypedSignalSpec, JOB_DONE_SIGNAL,
};
#[cfg(any(test, feature = "test-support"))]
use myelin_flow::{
    DriveOutcome, DurableExecutor, FlowDispatcher, FlowExecutor, FlowTelemetry, StartSpec,
    TimerStore, WfCtx, WfJournal, WorkflowBody, CI_PIPELINE_WF_TYPE, PARTITION_COUNT,
};

use crate::ci_drive_manifest::CiDriveManifestStore;
use crate::ci_manifest_job_runner::CiJobTokenRequest;
use crate::ci_pipeline::PipelineStage;
#[cfg(any(test, feature = "test-support"))]
use crate::ci_pipeline::{run_ci_pipeline_body, PipelineRun, RunVerdict};
use crate::ci_prelaunch_usage_journal::{
    resolve_prelaunch_usage_on_conn, CiPrelaunchParentExpectation, CiPrelaunchSettlementIdentity,
    CiPrelaunchUnresolvedPolicy, CiPrelaunchUsageJournalError,
};
#[cfg(any(test, feature = "test-support"))]
use crate::ci_run_store::CiRunRecord;
use crate::cost_store::{CiCostEventStore, CiCostStoreError};
use crate::job_accounting_store::{
    disposition_receipt_v4, versioned_accounting_receipt, CiJobAccountingError,
    CiJobAccountingRecord, CiJobAccountingStore, CiJobAccountingWriteVersion,
    CiJobTerminalDisposition,
};
#[cfg(any(test, feature = "test-support"))]
use crate::job_queue_store::{trust_from_token, JobQueueStoreError};
use crate::job_queue_store::{
    CiJobQueueStore, ClaimConsumeOutcome, ClaimConsumeSpec, DurableEnqueue,
    PreparationClaimConsumeSpec, PreparationRequeueOutcome, PreparationRequeueSpec,
};
use crate::job_schedule::JobScheduleTerms;
use crate::job_spec_store::{
    CiJobSpecStore, CiJobSpecStoreError, ClaimedDispatchIdentity, DurableCiJobLaunchTemplate,
    MAX_JOB_TIMEOUT_SECS,
};
use crate::metering::{CostEventRow, CostKind, Meter};
#[cfg(any(test, feature = "test-support"))]
use crate::scheduler::Lane;

pub(crate) const MAX_CI_DIAGNOSTIC_BYTES: usize = 2_048;

pub(crate) fn bounded_ci_diagnostic(value: &str) -> String {
    let mut output = String::with_capacity(value.len().min(MAX_CI_DIAGNOSTIC_BYTES));
    for character in value.chars() {
        let character = if character.is_control() || matches!(character, '\u{2028}' | '\u{2029}') {
            '�'
        } else {
            character
        };
        if output.len() + character.len_utf8() > MAX_CI_DIAGNOSTIC_BYTES {
            break;
        }
        output.push(character);
    }
    output
}

fn bridge<F: std::future::Future>(rt: &tokio::runtime::Handle, fut: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(_) => tokio::task::block_in_place(|| rt.block_on(fut)),
        Err(_) => rt.block_on(fut),
    }
}

pub type StageSpecBuilder =
    Arc<dyn Fn(&FlowJobSpec) -> Result<SandboxJobSpec, String> + Send + Sync>;

pub fn unresolved_stage_spec_builder() -> StageSpecBuilder {
    Arc::new(|spec: &FlowJobSpec| {
        Err(format!(
            "no pinned-snapshot → JobSpec resolver yet (CT-004d follow-on) for stage target `{}`; \
             the driver cannot fabricate an executable spec - dispatch refused fail-closed",
            spec.target
        ))
    })
}

pub struct DurableJobRunner {
    store: CiJobSpecStore,
    rt: tokio::runtime::Handle,
    terms: JobScheduleTerms,
    build_spec: StageSpecBuilder,
    targets: Vec<(String, String)>,
}

impl DurableJobRunner {
    pub fn new(
        store: CiJobSpecStore,
        rt: tokio::runtime::Handle,
        terms: JobScheduleTerms,
        build_spec: StageSpecBuilder,
        stages: &[PipelineStage],
    ) -> DurableJobRunner {
        let targets = stages
            .iter()
            .map(|s| (s.engine.target.clone(), s.engine.name.clone()))
            .collect();
        DurableJobRunner {
            store,
            rt,
            terms,
            build_spec,
            targets,
        }
    }

    fn stage_job_id(idem_token: &str) -> String {
        deterministic_uuid(&format!("jobq:{idem_token}"))
    }

    fn build_dispatch(
        &self,
        flow_spec: &FlowJobSpec,
    ) -> Result<(DurableEnqueue, SandboxJobSpec), ActivityError> {
        build_dispatch_parts(&self.terms, &self.build_spec, flow_spec)
    }
}

fn build_dispatch_parts(
    terms: &JobScheduleTerms,
    build_spec: &StageSpecBuilder,
    flow_spec: &FlowJobSpec,
) -> Result<(DurableEnqueue, SandboxJobSpec), ActivityError> {
    let mut spec = (build_spec)(flow_spec).map_err(ActivityError)?;

    spec.trust_tier = terms.trust_tier;
    spec.idem_token = IdemToken(flow_spec.idem_token.clone());
    if spec.limits.timeout_secs > MAX_JOB_TIMEOUT_SECS {
        spec.limits.timeout_secs = MAX_JOB_TIMEOUT_SECS;
    }

    let claim_window_secs = crate::ci_claim_window::claim_window_secs(
        spec.kind,
        &spec.workspace,
        spec.limits.timeout_secs,
    )
    .map_err(|error| ActivityError(error.to_string()))?;

    let enq = DurableEnqueue {
        tenant_id: terms.tenant_id.clone(),
        region: terms.region.clone(),
        job_id: DurableJobRunner::stage_job_id(&flow_spec.idem_token),
        run_id: terms.run_id.clone(),
        lane: terms.lane,
        labels: terms.labels.clone(),
        trust_tier: terms.trust_tier,
        concurrency_group: terms.concurrency_group.clone(),
        fair_key: terms.fair_key.clone(),
        idem_token: flow_spec.idem_token.clone(),
        stage: flow_spec.target.clone(),
        claim_window_secs,
        reservation_write_version: crate::ReservationWriteVersionMarker::derive_from_reserve_handle(
            &spec.meter_to.reserve_id,
        ),
    };
    Ok((enq, spec))
}

impl JobRunner for DurableJobRunner {
    fn dispatch(&self, flow_spec: &FlowJobSpec) -> Result<(), ActivityError> {
        let (mut enq, spec) = self.build_dispatch(flow_spec)?;

        let stage = self
            .targets
            .iter()
            .find(|(t, _)| t == &flow_spec.target)
            .map(|(_, name)| name.clone())
            .ok_or_else(|| {
                ActivityError(format!(
                    "ci.pipeline dispatch refused: target `{}` is not a known pipeline stage - the \
                     verdict could not be durably attributed (fail-closed)",
                    flow_spec.target
                ))
            })?;
        enq.stage = stage.clone();
        let authority = format!("legacy-test-authority:{}", spec.run_token.jti);
        let (spec, _previous_token) = spec.into_template();
        let launch = DurableCiJobLaunchTemplate {
            ci_run_id: enq.run_id.clone(),
            project_id: "00000000-0000-0000-0000-000000000000".into(),
            spec,
            token_authority_handle: authority,
        };

        bridge(
            &self.rt,
            self.store.co_persist_dispatch(&enq, &launch, &stage),
        )
        .map_err(|e| ActivityError(format!("durable co_persist_dispatch refused: {e}")))?;
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ClaimRefusal {
    TenantMismatch { reporter: String, claimed: String },
    NoDispatchRecord { job_id: String },
    RunMismatch { durable: String, claimed: String },
    IdemMismatch { durable: String, claimed: String },
}

impl std::fmt::Display for ClaimRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClaimRefusal::TenantMismatch { reporter, claimed } => write!(
                f,
                "claimed tenant `{claimed}` is not this reporter's tenant `{reporter}`"
            ),
            ClaimRefusal::NoDispatchRecord { job_id } => write!(
                f,
                "no durable ci_job_spec dispatch record for job `{job_id}` (unclaimed/forged completion)"
            ),
            ClaimRefusal::RunMismatch { durable, claimed } => write!(
                f,
                "durable dispatch run_id `{durable}` does not match the claimed run `{claimed}`"
            ),
            ClaimRefusal::IdemMismatch { durable, claimed } => write!(
                f,
                "durable dispatch idem_token `{durable}` does not match the claimed `{claimed}`"
            ),
        }
    }
}

fn verify_claimed_identity(
    reporter_tenant: &TenantId,
    claimed_tenant: &TenantId,
    presented_run: &str,
    presented_job_id: &str,
    presented_idem_token: &str,
    durable: Option<ClaimedDispatchIdentity>,
) -> Result<String, ClaimRefusal> {
    if claimed_tenant != reporter_tenant {
        return Err(ClaimRefusal::TenantMismatch {
            reporter: reporter_tenant.0.clone(),
            claimed: claimed_tenant.0.clone(),
        });
    }
    let Some(identity) = durable else {
        return Err(ClaimRefusal::NoDispatchRecord {
            job_id: presented_job_id.to_string(),
        });
    };
    if identity.run_id != presented_run {
        return Err(ClaimRefusal::RunMismatch {
            durable: identity.run_id,
            claimed: presented_run.to_string(),
        });
    }
    if identity.idem_token != presented_idem_token {
        return Err(ClaimRefusal::IdemMismatch {
            durable: identity.idem_token,
            claimed: presented_idem_token.to_string(),
        });
    }
    Ok(identity.stage)
}

#[derive(Clone, Copy)]
struct CompletionReceiptInput<'a> {
    tenant: &'a TenantId,
    region: &'a str,
    run: &'a RunId,
    job_id: &'a str,
    idem_token: &'a str,
    stage: &'a str,
    passed: bool,
    timed_out: bool,
    usage: ResourceUsage,
    result_refs: &'a [ArtifactRef],
    lease_owner: &'a str,
    lease_epoch: i64,
    claim_nonce: &'a str,
}

fn completion_receipt(input: CompletionReceiptInput<'_>) -> String {
    let key = blake3::derive_key(
        "myelin.ci.completion-receipt.v3",
        input.claim_nonce.as_bytes(),
    );
    let mut hasher = blake3::Hasher::new_keyed(&key);
    for frame in [
        input.tenant.0.as_bytes(),
        input.region.as_bytes(),
        input.run.0.as_bytes(),
        input.job_id.as_bytes(),
        input.idem_token.as_bytes(),
        input.stage.as_bytes(),
        &[input.passed as u8],
        &[input.timed_out as u8],
        &input.usage.cpu_seconds.to_be_bytes(),
        &input.usage.mem_byte_seconds.to_be_bytes(),
        input.lease_owner.as_bytes(),
        &input.lease_epoch.to_be_bytes(),
        input.claim_nonce.as_bytes(),
    ] {
        hasher.update(&(frame.len() as u64).to_be_bytes());
        hasher.update(frame);
    }
    hasher.update(&(input.result_refs.len() as u64).to_be_bytes());
    for result_ref in input.result_refs {
        hasher.update(&(result_ref.0.len() as u64).to_be_bytes());
        hasher.update(result_ref.0.as_bytes());
    }
    format!("v3:{}", hasher.finalize().to_hex())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompletionReceipts {
    current_v4: String,
    legacy_v3: String,
}

fn completion_receipts_v4(
    input: CompletionReceiptInput<'_>,
    disposition: CiJobTerminalDisposition,
) -> CompletionReceipts {
    let legacy_v3 = completion_receipt(input);
    CompletionReceipts {
        current_v4: disposition_receipt_v4(&legacy_v3, disposition),
        legacy_v3,
    }
}

#[derive(Clone, Copy)]
struct PreparationCompletionReceiptInput<'a> {
    tenant: &'a TenantId,
    region: &'a str,
    wf_run_id: &'a str,
    ci_run_id: &'a str,
    job_id: &'a str,
    idem_token: &'a str,
    stage: &'a str,
    reserve_handle: &'a str,
    usage: ResourceUsage,
    lease_owner: &'a str,
    lease_epoch: i64,
    claim_nonce: &'a str,
    claim_started_at_epoch_secs: i64,
    claim_expires_at_epoch_secs: i64,
}

fn preparation_completion_receipts(
    input: PreparationCompletionReceiptInput<'_>,
    disposition: PreparationTerminalDisposition,
) -> CompletionReceipts {
    let key = blake3::derive_key(
        "myelin.ci.preparation-completion-receipt.v3",
        input.claim_nonce.as_bytes(),
    );
    let mut hasher = blake3::Hasher::new_keyed(&key);
    for frame in [
        input.tenant.as_str().as_bytes(),
        input.region.as_bytes(),
        input.wf_run_id.as_bytes(),
        input.ci_run_id.as_bytes(),
        input.job_id.as_bytes(),
        input.idem_token.as_bytes(),
        input.stage.as_bytes(),
        input.reserve_handle.as_bytes(),
        &input.usage.cpu_seconds.to_be_bytes(),
        &input.usage.mem_byte_seconds.to_be_bytes(),
        input.lease_owner.as_bytes(),
        &input.lease_epoch.to_be_bytes(),
        input.claim_nonce.as_bytes(),
        &input.claim_started_at_epoch_secs.to_be_bytes(),
        &input.claim_expires_at_epoch_secs.to_be_bytes(),
    ] {
        hasher.update(&(frame.len() as u64).to_be_bytes());
        hasher.update(frame);
    }
    let legacy_v3 = format!("v3:{}", hasher.finalize().to_hex());
    let disposition = CiJobTerminalDisposition::Preparation(disposition);
    CompletionReceipts {
        current_v4: disposition_receipt_v4(&legacy_v3, disposition),
        legacy_v3,
    }
}

fn workload_disposition(report: &TerminalReport) -> CiJobTerminalDisposition {
    if report.timed_out {
        CiJobTerminalDisposition::WorkloadTimedOut
    } else if report.passed {
        CiJobTerminalDisposition::WorkloadPassed
    } else {
        CiJobTerminalDisposition::WorkloadFailed
    }
}

const RETRY_ATTEMPT_RECORD_VERSION: u8 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetryAttemptRecord {
    lease_epoch: i64,
    claim_nonce: String,
    lease_owner: String,
    cause: String,
    cpu_seconds: u64,
    mem_byte_seconds: u64,
    receipt: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetryAttemptAccrual {
    version: u8,
    attempts: u64,
    cpu_seconds: u64,
    mem_byte_seconds: u64,
    last: RetryAttemptRecord,
}

fn retry_attempt_receipt(
    claim: &CompletionClaim,
    region: &str,
    failure: &RetryableAttemptFailure,
) -> String {
    let cause = failure.cause.as_storage_token();
    let key = blake3::derive_key(
        "myelin.ci.retry-attempt-receipt.v1",
        claim.claim_nonce.as_bytes(),
    );
    let mut hasher = blake3::Hasher::new_keyed(&key);
    for frame in [
        claim.tenant.0.as_bytes(),
        region.as_bytes(),
        claim.run.0.as_bytes(),
        claim.job_id.as_bytes(),
        claim.idem_token.as_bytes(),
        claim.lease_owner.as_bytes(),
        &claim.lease_epoch.to_be_bytes(),
        claim.claim_nonce.as_bytes(),
        cause.as_bytes(),
        &failure.usage.cpu_seconds.to_be_bytes(),
        &failure.usage.mem_byte_seconds.to_be_bytes(),
    ] {
        hasher.update(&(frame.len() as u64).to_be_bytes());
        hasher.update(frame);
    }
    format!("retry-v1:{}", hasher.finalize().to_hex())
}

fn expected_retry_attempt_record(
    claim: &CompletionClaim,
    region: &str,
    failure: &RetryableAttemptFailure,
) -> RetryAttemptRecord {
    RetryAttemptRecord {
        lease_epoch: claim.lease_epoch,
        claim_nonce: claim.claim_nonce.clone(),
        lease_owner: claim.lease_owner.clone(),
        cause: failure.cause.as_storage_token().to_string(),
        cpu_seconds: failure.usage.cpu_seconds,
        mem_byte_seconds: failure.usage.mem_byte_seconds,
        receipt: retry_attempt_receipt(claim, region, failure),
    }
}

fn decode_retry_attempts(
    value: serde_json::Value,
) -> Result<Option<RetryAttemptAccrual>, CompletionTxError> {
    if value.as_object().is_some_and(serde_json::Map::is_empty)
        || value.as_array().is_some_and(Vec::is_empty)
    {
        return Ok(None);
    }
    let accrual: RetryAttemptAccrual =
        serde_json::from_value(value).map_err(|_| CompletionTxError::RetryCorrupt)?;
    let valid = accrual.version == RETRY_ATTEMPT_RECORD_VERSION
        && accrual.attempts > 0
        && accrual.last.lease_epoch > 0
        && accrual.attempts <= accrual.last.lease_epoch as u64
        && Uuid::parse_str(&accrual.last.claim_nonce).is_ok()
        && !accrual.last.lease_owner.is_empty()
        && RetryableAttemptCause::from_storage_token(&accrual.last.cause).is_some()
        && accrual.last.receipt.starts_with("retry-v1:")
        && accrual.last.receipt.len() == "retry-v1:".len() + 64;
    if valid {
        Ok(Some(accrual))
    } else {
        Err(CompletionTxError::RetryCorrupt)
    }
}

pub(crate) fn decode_retry_attempt_usage(
    value: serde_json::Value,
) -> Result<Option<ResourceUsage>, ()> {
    decode_retry_attempts(value)
        .map(|attempts| {
            attempts.map(|attempts| ResourceUsage {
                cpu_seconds: attempts.cpu_seconds,
                mem_byte_seconds: attempts.mem_byte_seconds,
            })
        })
        .map_err(|_| ())
}

fn aggregate_usage(
    attempts: Option<&RetryAttemptAccrual>,
    current: ResourceUsage,
) -> Result<ResourceUsage, CompletionTxError> {
    let Some(attempts) = attempts else {
        return checked_accounting_usage(current).map_err(CompletionTxError::Usage);
    };
    checked_add_accounting_usage(
        current,
        ResourceUsage {
            cpu_seconds: attempts.cpu_seconds,
            mem_byte_seconds: attempts.mem_byte_seconds,
        },
    )
    .map_err(CompletionTxError::Usage)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiUsageAggregationError {
    Overflow,
    DurableRange,
}

impl core::fmt::Display for CiUsageAggregationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Overflow => f.write_str("CI usage aggregation overflowed"),
            Self::DurableRange => {
                f.write_str("CI usage aggregation exceeds the durable bigint range")
            }
        }
    }
}

impl std::error::Error for CiUsageAggregationError {}

pub(crate) fn checked_accounting_usage(
    usage: ResourceUsage,
) -> Result<ResourceUsage, CiUsageAggregationError> {
    if i64::try_from(usage.cpu_seconds).is_err() || i64::try_from(usage.mem_byte_seconds).is_err() {
        return Err(CiUsageAggregationError::DurableRange);
    }
    Ok(usage)
}

pub(crate) fn checked_add_accounting_usage(
    left: ResourceUsage,
    right: ResourceUsage,
) -> Result<ResourceUsage, CiUsageAggregationError> {
    checked_accounting_usage(ResourceUsage {
        cpu_seconds: left
            .cpu_seconds
            .checked_add(right.cpu_seconds)
            .ok_or(CiUsageAggregationError::Overflow)?,
        mem_byte_seconds: left
            .mem_byte_seconds
            .checked_add(right.mem_byte_seconds)
            .ok_or(CiUsageAggregationError::Overflow)?,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PricedCiJobUsage {
    pub pricing_revision: String,
    pub memory_gb_seconds: u64,
    pub cpu_wholesale: MicroUsd,
    pub cpu_markup: MicroUsd,
    pub memory_wholesale: MicroUsd,
    pub memory_markup: MicroUsd,
}

pub const TIER_P_OPERATIONAL_PRICING_REVISION: &str = "tier-p-operational:v1";
pub const MICRO_USD_PER_CPU_SECOND: u64 = 10_000;
pub const MICRO_USD_PER_GB_SECOND: u64 = 10_000;
pub(crate) const TIER_P_OPERATIONAL_RESERVATION_PREFIX: &str = "ci-reserve:v1:";
pub(crate) const TIER_P_OPERATIONAL_RESERVATION_V2_PREFIX: &str = "ci-reserve:v2:";
const PRICING_GIB_BYTES: u64 = 1_073_741_824;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiJobPricingError {
    Unavailable,
    InvalidOutput,
}

impl core::fmt::Display for CiJobPricingError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Unavailable => f.write_str("CI job pricing authority is unavailable"),
            Self::InvalidOutput => f.write_str("CI job pricing authority returned invalid output"),
        }
    }
}

impl std::error::Error for CiJobPricingError {}

pub trait CiJobAccountingPricer: Send + Sync {
    fn price(&self, usage: ResourceUsage) -> Result<PricedCiJobUsage, CiJobPricingError>;
}

pub(crate) fn validate_reservation_pricing_policy(
    reserve_handle: &str,
    usage: ResourceUsage,
    priced: &PricedCiJobUsage,
) -> Result<(), CiJobPricingError> {
    if !reserve_handle.starts_with(TIER_P_OPERATIONAL_RESERVATION_PREFIX)
        && !reserve_handle.starts_with(TIER_P_OPERATIONAL_RESERVATION_V2_PREFIX)
    {
        return Ok(());
    }
    let memory_gb_seconds = usage.mem_byte_seconds.div_ceil(PRICING_GIB_BYTES);
    let exact_operational_policy = priced.pricing_revision == TIER_P_OPERATIONAL_PRICING_REVISION
        && priced.memory_gb_seconds == memory_gb_seconds
        && priced.cpu_wholesale == MicroUsd(usage.cpu_seconds * MICRO_USD_PER_CPU_SECOND)
        && priced.cpu_markup == MicroUsd::ZERO
        && priced.memory_wholesale == MicroUsd(memory_gb_seconds * MICRO_USD_PER_GB_SECOND)
        && priced.memory_markup == MicroUsd::ZERO;
    if exact_operational_policy {
        Ok(())
    } else {
        Err(CiJobPricingError::InvalidOutput)
    }
}

#[derive(Clone)]
pub struct DurableCiJobAccounting {
    scope: TenantScope,
    manifest_store: CiDriveManifestStore,
    money_ledger: DurableCostLedger,
    cost_store: CiCostEventStore,
    receipt_store: CiJobAccountingStore,
    pricer: Arc<dyn CiJobAccountingPricer>,
}

impl DurableCiJobAccounting {
    pub fn new(
        scope: TenantScope,
        manifest_store: CiDriveManifestStore,
        money_ledger: DurableCostLedger,
        cost_store: CiCostEventStore,
        receipt_store: CiJobAccountingStore,
        pricer: Arc<dyn CiJobAccountingPricer>,
    ) -> Self {
        Self {
            scope,
            manifest_store,
            money_ledger,
            cost_store,
            receipt_store,
            pricer,
        }
    }
}

pub(crate) fn priced_cost_rows(
    tenant: &TenantId,
    ci_run_id: &str,
    job_id: &str,
    usage: ResourceUsage,
    priced: &PricedCiJobUsage,
) -> Result<Vec<CostEventRow>, CiJobPricingError> {
    if priced.pricing_revision.is_empty() || priced.pricing_revision.len() > 512 {
        return Err(CiJobPricingError::InvalidOutput);
    }
    Ok(vec![
        CostEventRow {
            tenant: tenant.clone(),
            run_id: ci_run_id.to_owned(),
            job_id: job_id.to_owned(),
            meter: Meter::CpuSeconds,
            amount: usage.cpu_seconds,
            wholesale: priced.cpu_wholesale,
            markup: priced.cpu_markup,
            kind: CostKind::Ci,
        },
        CostEventRow {
            tenant: tenant.clone(),
            run_id: ci_run_id.to_owned(),
            job_id: job_id.to_owned(),
            meter: Meter::MemGbSeconds,
            amount: priced.memory_gb_seconds,
            wholesale: priced.memory_wholesale,
            markup: priced.memory_markup,
            kind: CostKind::Ci,
        },
    ])
}

#[derive(Clone)]
enum ReporterAccounting {
    Durable(Arc<DurableCiJobAccounting>),
    #[cfg(any(test, feature = "test-support"))]
    TestBypass,
}

struct TerminalAccountingInput<'a> {
    tenant: &'a TenantId,
    wf_run: &'a RunId,
    ci_run_id: &'a str,
    job_id: &'a str,
    reserve_handle: &'a str,
    report: &'a TerminalReport,
    receipts: &'a CompletionReceipts,
    disposition: CiJobTerminalDisposition,
    diagnostic: Option<&'a str>,
    replay: bool,
}

struct TerminalSurfaceInput<'a> {
    report: &'a TerminalReport,
    disposition: Option<CiJobTerminalDisposition>,
    diagnostic: Option<&'a str>,
}

#[derive(Debug)]
pub(crate) enum CompletionTxError {
    Scope(PgError),
    Spec(CiJobSpecStoreError),
    Manifest,
    Pricing(CiJobPricingError),
    Money(DurableSettleError),
    Projection(CiCostStoreError),
    Accounting(CiJobAccountingError),
    CancelledClosure,
    Signal(ExecutorError),
    RetryStore,
    RetryCorrupt,
    Prelaunch(CiPrelaunchUsageJournalError),
    Usage(CiUsageAggregationError),
    Refused,
}

impl From<PgError> for CompletionTxError {
    fn from(error: PgError) -> Self {
        Self::Scope(error)
    }
}

async fn record_retryable_attempt_on_conn(
    conn: &mut sqlx::PgConnection,
    region: &str,
    claim: &CompletionClaim,
    failure: &RetryableAttemptFailure,
    requeue: bool,
) -> Result<RetryableAttemptOutcome, CompletionTxError> {
    let job_id = Uuid::parse_str(&claim.job_id).map_err(|_| CompletionTxError::Refused)?;
    let row = sqlx::query(
        "SELECT run_id::text AS run_id, idem_token, state, lease_owner, lease_epoch,
                claim_nonce::text AS claim_nonce, completion_receipt, retry_attempts
         FROM job_queue
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3
         FOR UPDATE",
    )
    .bind(&claim.tenant.0)
    .bind(region)
    .bind(job_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| CompletionTxError::RetryStore)?
    .ok_or(CompletionTxError::Refused)?;
    let durable_run: String = row.get("run_id");
    let durable_idem: String = row.get("idem_token");
    let state: String = row.get("state");
    let lease_owner: Option<String> = row.get("lease_owner");
    let lease_epoch: i64 = row.get("lease_epoch");
    let claim_nonce: Option<String> = row.get("claim_nonce");
    let completion_receipt: Option<String> = row.get("completion_receipt");
    let retry_attempts: serde_json::Value = row.get("retry_attempts");
    let attempts = decode_retry_attempts(retry_attempts.clone())?;
    let expected = expected_retry_attempt_record(claim, region, failure);

    if let Some(recorded) = attempts
        .as_ref()
        .filter(|attempts| attempts.last.lease_epoch == claim.lease_epoch)
    {
        return if recorded.last == expected {
            Ok(RetryableAttemptOutcome::ExactReplay)
        } else {
            Err(CompletionTxError::Refused)
        };
    }
    let exact_live_generation = durable_run == claim.run.0
        && durable_idem == claim.idem_token
        && state == "running"
        && lease_owner.as_deref() == Some(claim.lease_owner.as_str())
        && lease_epoch == claim.lease_epoch
        && claim_nonce.as_deref() == Some(claim.claim_nonce.as_str())
        && completion_receipt.is_none();
    if !exact_live_generation
        || attempts
            .as_ref()
            .is_some_and(|prior| prior.last.lease_epoch >= claim.lease_epoch)
    {
        return Err(CompletionTxError::Refused);
    }
    let prior_attempts = attempts.as_ref().map_or(0, |prior| prior.attempts);
    let prior_cpu = attempts.as_ref().map_or(0, |prior| prior.cpu_seconds);
    let prior_memory = attempts.as_ref().map_or(0, |prior| prior.mem_byte_seconds);
    let encoded = serde_json::to_value(RetryAttemptAccrual {
        version: RETRY_ATTEMPT_RECORD_VERSION,
        attempts: prior_attempts
            .checked_add(1)
            .ok_or(CompletionTxError::Refused)?,
        cpu_seconds: prior_cpu
            .checked_add(failure.usage.cpu_seconds)
            .ok_or(CompletionTxError::Refused)?,
        mem_byte_seconds: prior_memory
            .checked_add(failure.usage.mem_byte_seconds)
            .ok_or(CompletionTxError::Refused)?,
        last: expected,
    })
    .map_err(|_| CompletionTxError::Refused)?;
    let next_state = if requeue { "queued" } else { "terminal" };
    let updated = sqlx::query(
        "UPDATE job_queue
         SET retry_attempts = $10, state = $11, lease_owner = NULL, lease_expires = NULL,
             claim_nonce = NULL
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3 AND run_id = $4::uuid
           AND idem_token = $5 AND state = 'running' AND lease_owner = $6
           AND lease_epoch = $7 AND claim_nonce = $8::uuid AND completion_receipt IS NULL
           AND retry_attempts = $9
         RETURNING job_id",
    )
    .bind(&claim.tenant.0)
    .bind(region)
    .bind(job_id)
    .bind(&claim.run.0)
    .bind(&claim.idem_token)
    .bind(&claim.lease_owner)
    .bind(claim.lease_epoch)
    .bind(&claim.claim_nonce)
    .bind(retry_attempts)
    .bind(encoded)
    .bind(next_state)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| CompletionTxError::RetryStore)?;
    if requeue && updated.is_some() {
        sqlx::query(
            "UPDATE ci_job SET state = 'queued' \
             WHERE tenant_id = $1 AND job_id = $2 AND state = 'running'",
        )
        .bind(&claim.tenant.0)
        .bind(job_id)
        .execute(&mut *conn)
        .await
        .map_err(|_| CompletionTxError::RetryStore)?;
    }
    if updated.is_some() {
        Ok(if requeue {
            RetryableAttemptOutcome::Requeued
        } else {
            RetryableAttemptOutcome::Cancelled
        })
    } else {
        Err(CompletionTxError::Refused)
    }
}

async fn retry_attempts_for_terminal_on_conn(
    conn: &mut sqlx::PgConnection,
    tenant: &TenantId,
    region: &str,
    job_id: Uuid,
) -> Result<Option<RetryAttemptAccrual>, CompletionTxError> {
    let value: serde_json::Value = sqlx::query_scalar(
        "SELECT retry_attempts FROM job_queue
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3
         FOR UPDATE",
    )
    .bind(&tenant.0)
    .bind(region)
    .bind(job_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| CompletionTxError::RetryStore)?
    .ok_or(CompletionTxError::Refused)?;
    decode_retry_attempts(value)
}

async fn co_commit_terminal_accounting(
    conn: &mut sqlx::PgConnection,
    accounting: &DurableCiJobAccounting,
    input: TerminalAccountingInput<'_>,
) -> Result<(), CompletionTxError> {
    let surface_disposition = if input.replay {
        let existing = accounting
            .receipt_store
            .load_in_tx(conn, &accounting.scope, input.job_id)
            .await
            .map_err(CompletionTxError::Accounting)?
            .ok_or(CompletionTxError::Refused)?;
        let common_exact = existing.tenant == *input.tenant
            && existing.job_id == input.job_id
            && existing.wf_run_id == input.wf_run.0
            && existing.ci_run_id == input.ci_run_id
            && existing.reserve_handle == input.reserve_handle
            && existing.passed == input.report.passed
            && existing.timed_out == input.report.timed_out
            && existing.usage == input.report.usage;
        let receipt_exact = match existing.disposition {
            None => existing.completion_receipt == input.receipts.legacy_v3,
            Some(disposition) => {
                disposition == input.disposition
                    && existing.completion_receipt == input.receipts.current_v4
            }
        };
        if !common_exact || !receipt_exact {
            return Err(CompletionTxError::Refused);
        }
        existing.disposition
    } else {
        let priced = accounting
            .pricer
            .price(input.report.usage)
            .map_err(CompletionTxError::Pricing)?;
        validate_reservation_pricing_policy(input.reserve_handle, input.report.usage, &priced)
            .map_err(CompletionTxError::Pricing)?;
        let rows = priced_cost_rows(
            input.tenant,
            input.ci_run_id,
            input.job_id,
            input.report.usage,
            &priced,
        )
        .map_err(CompletionTxError::Pricing)?;
        let units: Vec<MeteredUnit> = rows
            .iter()
            .map(|row| MeteredUnit {
                unit: row.meter.token(),
                wholesale: row.wholesale,
                markup: row.markup,
            })
            .collect();
        let settled = accounting
            .money_ledger
            .settle_in_tx(
                conn,
                input.tenant,
                &CostRunId(input.reserve_handle.to_owned()),
                &units,
            )
            .await
            .map_err(CompletionTxError::Money)?;
        accounting
            .cost_store
            .settle_in_tx(conn, &accounting.scope, &rows)
            .await
            .map_err(CompletionTxError::Projection)?;
        let receipt = versioned_accounting_receipt(
            accounting.receipt_store.write_version(),
            input.receipts.legacy_v3.clone(),
            input.disposition,
        );
        accounting
            .receipt_store
            .record_in_tx(
                conn,
                &accounting.scope,
                &CiJobAccountingRecord {
                    tenant: input.tenant.clone(),
                    job_id: input.job_id.to_owned(),
                    wf_run_id: input.wf_run.0.clone(),
                    ci_run_id: input.ci_run_id.to_owned(),
                    reserve_handle: input.reserve_handle.to_owned(),
                    passed: input.report.passed,
                    timed_out: input.report.timed_out,
                    skipped: false,
                    usage: input.report.usage,
                    pricing_revision: priced.pricing_revision,
                    billed: settled.billed_total,
                    refunded: settled.refunded,
                    disposition: receipt.disposition,
                    completion_receipt: receipt.completion_receipt,
                    legacy_completion_receipt_v3: receipt.legacy_completion_receipt_v3,
                },
            )
            .await
            .map_err(CompletionTxError::Accounting)?;
        (accounting.receipt_store.write_version() == CiJobAccountingWriteVersion::V4)
            .then_some(input.disposition)
    };
    settle_ci_job_surface_on_conn(
        conn,
        input.tenant,
        accounting.scope.region(),
        input.ci_run_id,
        input.job_id,
        TerminalSurfaceInput {
            report: input.report,
            disposition: surface_disposition,
            diagnostic: input.diagnostic,
        },
    )
    .await?;
    Ok(())
}

struct TerminalUsageResolutionInput<'a> {
    tenant: &'a TenantId,
    wf_run_id: &'a str,
    job_id: &'a str,
    reserve_handle: &'a str,
    base_usage: ResourceUsage,
    parent_expectation: CiPrelaunchParentExpectation,
    unresolved_policy: CiPrelaunchUnresolvedPolicy,
}

async fn resolve_terminal_usage_on_conn(
    conn: &mut sqlx::PgConnection,
    accounting: &DurableCiJobAccounting,
    input: TerminalUsageResolutionInput<'_>,
) -> Result<(String, ResourceUsage), CompletionTxError> {
    let (manifest, _) = accounting
        .manifest_store
        .load_by_wf_run_on_conn(conn, input.wf_run_id)
        .await
        .map_err(|_| CompletionTxError::Manifest)?
        .ok_or(CompletionTxError::Refused)?;
    let granted_job = manifest
        .jobs
        .iter()
        .find(|job| job.job_id == input.job_id)
        .ok_or(CompletionTxError::Refused)?;
    if manifest.tenant_id != input.tenant.0 || granted_job.reserve_handle != input.reserve_handle {
        return Err(CompletionTxError::Refused);
    }
    let prelaunch = resolve_prelaunch_usage_on_conn(
        conn,
        CiPrelaunchSettlementIdentity {
            tenant_id: input.tenant.as_str(),
            region: accounting.scope.region().as_str(),
            job_id: input.job_id,
            wf_run_id: input.wf_run_id,
            ci_run_id: &manifest.ci_run_id,
            reserve_handle: input.reserve_handle,
        },
        input.parent_expectation,
        input.unresolved_policy,
    )
    .await
    .map_err(CompletionTxError::Prelaunch)?;
    let usage = checked_add_accounting_usage(input.base_usage, prelaunch.usage)
        .map_err(CompletionTxError::Usage)?;
    Ok((manifest.ci_run_id, usage))
}

async fn verify_preparation_disposition_on_conn(
    conn: &mut sqlx::PgConnection,
    claim: &CiJobTokenRequest,
    reserve_handle: &str,
    disposition: PreparationTerminalDisposition,
) -> Result<(), CompletionTxError> {
    let current = sqlx::query(
        "SELECT budget_revision, max_parent_attempts
         FROM ci_job_parent_attempt
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
           AND wf_run_id = $4::uuid AND ci_run_id = $5::uuid
           AND reserve_handle = $6 AND lease_owner = $7
           AND lease_epoch = $8 AND claim_nonce = $9::uuid
           AND claim_started_at_epoch_secs = $10
           AND claim_expires_at_epoch_secs = $11",
    )
    .bind(&claim.tenant_id)
    .bind(&claim.region)
    .bind(&claim.job_id)
    .bind(&claim.wf_run_id)
    .bind(&claim.ci_run_id)
    .bind(reserve_handle)
    .bind(&claim.lease_owner)
    .bind(claim.lease_epoch)
    .bind(&claim.claim_nonce)
    .bind(claim.claim_started_at_epoch_secs)
    .bind(claim.claim_expires_at_epoch_secs)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| CompletionTxError::Prelaunch(CiPrelaunchUsageJournalError::Database))?;

    match disposition {
        PreparationTerminalDisposition::Failed {
            phase: PreparationPhase::SecretResolution,
        } => {
            if current.is_some() {
                return Err(CompletionTxError::Refused);
            }
        }
        PreparationTerminalDisposition::TimedOut {
            phase: PreparationPhase::SecretResolution,
        } => return Err(CompletionTxError::Refused),
        PreparationTerminalDisposition::Failed { phase }
        | PreparationTerminalDisposition::TimedOut { phase } => {
            current.ok_or(CompletionTxError::Refused)?;
            let terminal = sqlx::query_scalar::<_, i32>(
                "SELECT 1
                 FROM ci_job_prelaunch_usage
                 WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
                   AND lease_epoch = $4 AND claim_nonce = $5::uuid AND phase = $6
                   AND status IN ('measured', 'sealed_ceiling')",
            )
            .bind(&claim.tenant_id)
            .bind(&claim.region)
            .bind(&claim.job_id)
            .bind(claim.lease_epoch)
            .bind(&claim.claim_nonce)
            .bind(phase.as_storage_token())
            .fetch_optional(&mut *conn)
            .await
            .map_err(|_| CompletionTxError::Prelaunch(CiPrelaunchUsageJournalError::Database))?;
            if terminal.is_none() {
                return Err(CompletionTxError::Refused);
            }
        }
        PreparationTerminalDisposition::AttemptsExhausted => {
            let (revision, maximum) = match &current {
                Some(row) => (
                    row.try_get::<i16, _>("budget_revision")
                        .map_err(|_| CompletionTxError::Refused)?,
                    row.try_get::<i64, _>("max_parent_attempts")
                        .map_err(|_| CompletionTxError::Refused)?,
                ),
                None => {
                    let policies = sqlx::query(
                        "SELECT budget_revision, max_parent_attempts
                         FROM ci_job_parent_attempt
                         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
                           AND wf_run_id = $4::uuid AND ci_run_id = $5::uuid
                           AND reserve_handle = $6
                         GROUP BY budget_revision, max_parent_attempts",
                    )
                    .bind(&claim.tenant_id)
                    .bind(&claim.region)
                    .bind(&claim.job_id)
                    .bind(&claim.wf_run_id)
                    .bind(&claim.ci_run_id)
                    .bind(reserve_handle)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(|_| {
                        CompletionTxError::Prelaunch(CiPrelaunchUsageJournalError::Database)
                    })?;
                    if policies.len() != 1 {
                        return Err(CompletionTxError::Refused);
                    }
                    (
                        policies[0]
                            .try_get::<i16, _>("budget_revision")
                            .map_err(|_| CompletionTxError::Refused)?,
                        policies[0]
                            .try_get::<i64, _>("max_parent_attempts")
                            .map_err(|_| CompletionTxError::Refused)?,
                    )
                }
            };
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*)
                 FROM ci_job_parent_attempt
                 WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
                   AND wf_run_id = $4::uuid AND ci_run_id = $5::uuid
                   AND reserve_handle = $6 AND budget_revision = $7
                   AND max_parent_attempts = $8",
            )
            .bind(&claim.tenant_id)
            .bind(&claim.region)
            .bind(&claim.job_id)
            .bind(&claim.wf_run_id)
            .bind(&claim.ci_run_id)
            .bind(reserve_handle)
            .bind(revision)
            .bind(maximum)
            .fetch_one(&mut *conn)
            .await
            .map_err(|_| CompletionTxError::Prelaunch(CiPrelaunchUsageJournalError::Database))?;
            if count != maximum {
                return Err(CompletionTxError::Refused);
            }
        }
    }
    Ok(())
}

enum PreparationRetryGate {
    Proceed,
    NotLive,
}

async fn verify_preparation_retry_permitted_on_conn(
    conn: &mut sqlx::PgConnection,
    claim: &CiJobTokenRequest,
    reserve_handle: &str,
) -> Result<PreparationRetryGate, CompletionTxError> {
    let live = sqlx::query_scalar::<_, String>(
        "SELECT q.job_id::text
         FROM job_queue AS q
         WHERE q.tenant_id = $1 AND q.region = $2 AND q.job_id = $3::uuid AND q.run_id = $4::uuid
           AND q.state = 'leased' AND q.lease_owner = $5 AND q.lease_epoch = $6
           AND q.claim_nonce = $7::uuid
           AND FLOOR(EXTRACT(EPOCH FROM q.claim_started_at))::bigint = $8
           AND FLOOR(EXTRACT(EPOCH FROM q.claim_expires_at))::bigint = $9
           AND q.claim_expires_at > statement_timestamp()
           AND q.completion_receipt IS NULL
         FOR UPDATE",
    )
    .bind(&claim.tenant_id)
    .bind(&claim.region)
    .bind(&claim.job_id)
    .bind(&claim.wf_run_id)
    .bind(&claim.lease_owner)
    .bind(claim.lease_epoch)
    .bind(&claim.claim_nonce)
    .bind(claim.claim_started_at_epoch_secs)
    .bind(claim.claim_expires_at_epoch_secs)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| CompletionTxError::Prelaunch(CiPrelaunchUsageJournalError::Database))?;
    if live.is_none() {
        return Ok(PreparationRetryGate::NotLive);
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "myelin.ci.parent-attempt.v1:{}:{}:{}",
            claim.tenant_id, claim.region, claim.job_id
        ))
        .execute(&mut *conn)
        .await
        .map_err(|_| CompletionTxError::Prelaunch(CiPrelaunchUsageJournalError::Database))?;

    let current = sqlx::query(
        "SELECT budget_revision, max_parent_attempts
         FROM ci_job_parent_attempt
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
           AND wf_run_id = $4::uuid AND ci_run_id = $5::uuid
           AND reserve_handle = $6 AND lease_owner = $7
           AND lease_epoch = $8 AND claim_nonce = $9::uuid
           AND claim_started_at_epoch_secs = $10
           AND claim_expires_at_epoch_secs = $11",
    )
    .bind(&claim.tenant_id)
    .bind(&claim.region)
    .bind(&claim.job_id)
    .bind(&claim.wf_run_id)
    .bind(&claim.ci_run_id)
    .bind(reserve_handle)
    .bind(&claim.lease_owner)
    .bind(claim.lease_epoch)
    .bind(&claim.claim_nonce)
    .bind(claim.claim_started_at_epoch_secs)
    .bind(claim.claim_expires_at_epoch_secs)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| CompletionTxError::Prelaunch(CiPrelaunchUsageJournalError::Database))?
    .ok_or(CompletionTxError::Refused)?;
    let revision: i16 = current
        .try_get("budget_revision")
        .map_err(|_| CompletionTxError::Refused)?;
    let maximum: i64 = current
        .try_get("max_parent_attempts")
        .map_err(|_| CompletionTxError::Refused)?;

    let unresolved = sqlx::query_scalar::<_, i32>(
        "SELECT 1 FROM ci_job_prelaunch_usage
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
           AND lease_epoch = $4 AND claim_nonce = $5::uuid AND status = 'started'
         LIMIT 1",
    )
    .bind(&claim.tenant_id)
    .bind(&claim.region)
    .bind(&claim.job_id)
    .bind(claim.lease_epoch)
    .bind(&claim.claim_nonce)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| CompletionTxError::Prelaunch(CiPrelaunchUsageJournalError::Database))?;
    if unresolved.is_some() {
        return Err(CompletionTxError::Refused);
    }

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM ci_job_parent_attempt
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
           AND wf_run_id = $4::uuid AND ci_run_id = $5::uuid
           AND reserve_handle = $6 AND budget_revision = $7 AND max_parent_attempts = $8",
    )
    .bind(&claim.tenant_id)
    .bind(&claim.region)
    .bind(&claim.job_id)
    .bind(&claim.wf_run_id)
    .bind(&claim.ci_run_id)
    .bind(reserve_handle)
    .bind(revision)
    .bind(maximum)
    .fetch_one(&mut *conn)
    .await
    .map_err(|_| CompletionTxError::Prelaunch(CiPrelaunchUsageJournalError::Database))?;
    if count >= maximum {
        return Err(CompletionTxError::Refused);
    }
    Ok(PreparationRetryGate::Proceed)
}

async fn settle_ci_job_surface_on_conn(
    conn: &mut sqlx::PgConnection,
    tenant: &TenantId,
    region: &Region,
    ci_run_id: &str,
    job_id: &str,
    surface: TerminalSurfaceInput<'_>,
) -> Result<(), CompletionTxError> {
    let preparation = matches!(
        surface.disposition,
        Some(CiJobTerminalDisposition::Preparation(_))
    );
    let state = if !preparation && surface.report.passed && !surface.report.timed_out {
        "succeeded"
    } else {
        "failed"
    };
    let summary = terminal_result_summary(surface.report, surface.disposition, surface.diagnostic);
    let state_predicate = if preparation {
        "state IN ('queued','leased') OR (state=$5 AND result_summary=$6)"
    } else {
        "state IN ('queued','leased','running') OR (state=$5 AND result_summary=$6)"
    };
    let query = format!(
        "UPDATE ci_job
         SET state=$5, result_summary=$6
         WHERE tenant_id=$1 AND region=$2 AND run_id=$3::uuid AND job_id=$4::uuid
           AND ({state_predicate})
         RETURNING job_id"
    );
    let updated = sqlx::query_scalar::<_, Uuid>(&query)
        .bind(tenant.as_str())
        .bind(region.as_str())
        .bind(ci_run_id)
        .bind(job_id)
        .bind(state)
        .bind(summary)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|_| {
            CompletionTxError::Accounting(CiJobAccountingError::Db("job surface update"))
        })?;
    if updated.is_some() {
        Ok(())
    } else {
        Err(CompletionTxError::Refused)
    }
}

fn terminal_result_summary(
    report: &TerminalReport,
    disposition: Option<CiJobTerminalDisposition>,
    diagnostic: Option<&str>,
) -> serde_json::Value {
    match disposition {
        Some(disposition) => {
            let mut summary = serde_json::json!({
                "passed": report.passed,
                "timed_out": report.timed_out,
                "disposition": disposition.as_storage_token(),
                "workload_started": disposition.workload_started(),
            });
            if let Some(diagnostic) = diagnostic {
                let diagnostic = bounded_ci_diagnostic(diagnostic);
                if !diagnostic.is_empty() {
                    summary["diagnostic"] = serde_json::Value::String(diagnostic);
                }
            }
            summary
        }
        None => serde_json::json!({
            "passed": report.passed,
            "timed_out": report.timed_out,
        }),
    }
}

pub(crate) async fn close_cancelled_run_if_accounted(
    conn: &mut sqlx::PgConnection,
    accounting: &DurableCiJobAccounting,
    wf_run_id: &str,
) -> Result<(), CompletionTxError> {
    let (manifest, _) = accounting
        .manifest_store
        .load_by_wf_run_on_conn(conn, wf_run_id)
        .await
        .map_err(|_| CompletionTxError::CancelledClosure)?
        .ok_or(CompletionTxError::CancelledClosure)?;
    let run = sqlx::query(
        "SELECT state, cost_settled, finished_at IS NOT NULL AS finished \
         FROM ci_run \
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid \
           AND wf_run_id = $4::uuid FOR UPDATE",
    )
    .bind(accounting.scope.tenant().as_str())
    .bind(accounting.scope.region().as_str())
    .bind(&manifest.ci_run_id)
    .bind(&manifest.wf_run_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| CompletionTxError::CancelledClosure)?
    .ok_or(CompletionTxError::CancelledClosure)?;
    let state: String = run.get("state");
    let settled: bool = run.get("cost_settled");
    let finished: bool = run.get("finished");
    if state != "cancelled" {
        return Ok(());
    }
    if !finished {
        return Err(CompletionTxError::CancelledClosure);
    }

    let rows = sqlx::query(
        "SELECT job_id::text AS job_id, reserve_handle \
         FROM ci_job_accounting \
         WHERE tenant_id = $1 AND region = $2 AND ci_run_id = $3::uuid \
           AND wf_run_id = $4::uuid ORDER BY job_id FOR SHARE",
    )
    .bind(accounting.scope.tenant().as_str())
    .bind(accounting.scope.region().as_str())
    .bind(&manifest.ci_run_id)
    .bind(&manifest.wf_run_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(|_| CompletionTxError::CancelledClosure)?;
    let expected: BTreeMap<&str, &str> = manifest
        .jobs
        .iter()
        .map(|job| (job.job_id.as_str(), job.reserve_handle.as_str()))
        .collect();
    if rows.len() < expected.len() {
        return Ok(());
    }
    if rows.len() != expected.len()
        || rows.iter().any(|row| {
            let job_id: String = row.get("job_id");
            let reserve_handle: String = row.get("reserve_handle");
            expected.get(job_id.as_str()).copied() != Some(reserve_handle.as_str())
        })
    {
        return Err(CompletionTxError::CancelledClosure);
    }
    if settled {
        return Ok(());
    }
    let updated = sqlx::query(
        "UPDATE ci_run SET cost_settled = true \
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid \
           AND wf_run_id = $4::uuid AND state = 'cancelled' \
           AND cost_settled = false AND finished_at IS NOT NULL",
    )
    .bind(accounting.scope.tenant().as_str())
    .bind(accounting.scope.region().as_str())
    .bind(&manifest.ci_run_id)
    .bind(&manifest.wf_run_id)
    .execute(&mut *conn)
    .await
    .map_err(|_| CompletionTxError::CancelledClosure)?;
    if updated.rows_affected() != 1 {
        return Err(CompletionTxError::CancelledClosure);
    }
    crate::ci_run_supersession::emit_settled_cancelled_checks_on_conn(
        conn,
        accounting.scope.tenant(),
        accounting.scope.region(),
        &manifest,
    )
    .await
    .map_err(|_| CompletionTxError::CancelledClosure)?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparationRetryOutcome {
    Requeued,
    NoOp,
}

#[derive(Clone)]
pub struct CiPipelineReporter {
    pg_executor: PgFlowExecutor,
    spec_store: CiJobSpecStore,
    queue_store: CiJobQueueStore,
    rt: tokio::runtime::Handle,
    tenant: TenantId,
    region: String,
    accounting: ReporterAccounting,
    #[cfg(any(test, feature = "test-support"))]
    test_executor: Option<FlowExecutor>,
}

impl CiPipelineReporter {
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub fn new_accounted(
        pg_executor: PgFlowExecutor,
        spec_store: CiJobSpecStore,
        queue_store: CiJobQueueStore,
        rt: tokio::runtime::Handle,
        accounting: DurableCiJobAccounting,
    ) -> CiPipelineReporter {
        let tenant = accounting.scope.tenant().clone();
        let region = accounting.scope.region().as_str().to_owned();
        CiPipelineReporter {
            pg_executor,
            spec_store,
            queue_store,
            rt,
            tenant,
            region,
            accounting: ReporterAccounting::Durable(Arc::new(accounting)),
            #[cfg(any(test, feature = "test-support"))]
            test_executor: None,
        }
    }

    pub fn report_preparation_terminal(
        &self,
        claim: &CiJobTokenRequest,
        disposition: PreparationTerminalDisposition,
    ) -> Result<SignalOutcome, ExecutorError> {
        self.report_preparation_terminal_with_diagnostic(claim, disposition, None)
    }

    pub fn report_preparation_terminal_with_diagnostic(
        &self,
        claim: &CiJobTokenRequest,
        disposition: PreparationTerminalDisposition,
        diagnostic: Option<&str>,
    ) -> Result<SignalOutcome, ExecutorError> {
        claim.validate().map_err(|error| {
            ExecutorError::InvalidInput(format!(
                "ci.pipeline preparation completion refused: {}",
                error.0
            ))
        })?;
        if claim.tenant_id != self.tenant.0 || claim.region != self.region {
            return Err(ExecutorError::InvalidInput(
                "ci.pipeline preparation completion refused: reporter scope mismatch".into(),
            ));
        }
        let accounting = match &self.accounting {
            ReporterAccounting::Durable(accounting)
                if accounting.receipt_store.write_version() == CiJobAccountingWriteVersion::V4 =>
            {
                accounting.clone()
            }
            ReporterAccounting::Durable(_) => {
                return Err(ExecutorError::InvalidInput(
                    "ci.pipeline preparation completion refused: v4 accounting writer is not \
                     activated"
                        .into(),
                ))
            }
            #[cfg(any(test, feature = "test-support"))]
            ReporterAccounting::TestBypass => {
                return Err(ExecutorError::InvalidInput(
                    "ci.pipeline preparation completion requires durable v4 accounting".into(),
                ))
            }
        };

        let job_uuid = Uuid::parse_str(&claim.job_id)
            .map_err(|_| ExecutorError::InvalidInput("invalid job UUID".into()))?;
        let wf_run_uuid = Uuid::parse_str(&claim.wf_run_id)
            .map_err(|_| ExecutorError::InvalidInput("invalid workflow run UUID".into()))?;
        let ci_run_uuid = Uuid::parse_str(&claim.ci_run_id)
            .map_err(|_| ExecutorError::InvalidInput("invalid CI run UUID".into()))?;
        let nonce_uuid = Uuid::parse_str(&claim.claim_nonce)
            .map_err(|_| ExecutorError::InvalidInput("invalid claim nonce UUID".into()))?;
        let claim = claim.clone();
        let diagnostic = diagnostic.map(str::to_owned);
        let refusal_job = claim.job_id.clone();
        let refusal_owner = claim.lease_owner.clone();
        let refusal_epoch = claim.lease_epoch;
        let tenant = self.tenant.clone();
        let region = self.region.clone();
        let transaction_tenant = tenant.0.clone();
        let transaction_region = region.clone();
        let spec_store = self.spec_store.clone();
        let pg_executor = self.pg_executor.clone();

        let durable = bridge(
            &self.rt,
            with_tenant_tx_error(
                self.queue_store.pool(),
                &transaction_tenant,
                &transaction_region,
                move |conn| {
                    Box::pin(async move {
                        let identity = spec_store
                            .get_dispatch_identity_on_conn(
                                conn,
                                tenant.as_str(),
                                job_uuid,
                                &claim.job_id,
                            )
                            .await
                            .map_err(CompletionTxError::Spec)?;
                        let reserve_handle = identity
                            .as_ref()
                            .map(|identity| identity.reserve_handle.clone())
                            .ok_or(CompletionTxError::Refused)?;
                        let stage = verify_claimed_identity(
                            &tenant,
                            &tenant,
                            &claim.wf_run_id,
                            &claim.job_id,
                            &claim.idem_token,
                            identity,
                        )
                        .map_err(|_| CompletionTxError::Refused)?;
                        let launch = spec_store
                            .get_launch_template_on_conn(conn, tenant.as_str(), &claim.job_id)
                            .await
                            .map_err(CompletionTxError::Spec)?;
                        let checkout_scope = derive_checkout_authorization_scope(
                            JobKind::Ci,
                            &launch.spec.workspace,
                        )
                        .map_err(|_| CompletionTxError::Refused)?;
                        if launch.ci_run_id != claim.ci_run_id
                            || launch.token_authority_handle != claim.token_authority_handle
                            || launch.spec.idem_token.0 != claim.idem_token
                            || launch.spec.meter_to.reserve_id != reserve_handle
                            || (checkout_scope.is_none()
                                && !matches!(
                                    disposition,
                                    PreparationTerminalDisposition::Failed {
                                        phase: PreparationPhase::SecretResolution
                                    }
                                ))
                        {
                            return Err(CompletionTxError::Refused);
                        }

                        let signal = TypedSignalSpec {
                            run: RunId(claim.wf_run_id.clone()),
                            signal_name: JOB_DONE_SIGNAL.to_string(),
                            idem_key: claim.idem_token.clone(),
                            payload: SignalPayload::CiJobDone {
                                stage: stage.clone(),
                                passed: false,
                                result_refs: Vec::new(),
                            },
                            payload_key_ref: None,
                        };
                        let signal_outcome = pg_executor
                            .signal_typed_on_conn(conn, signal)
                            .await
                            .map_err(CompletionTxError::Signal)?;
                        let attempts =
                            retry_attempts_for_terminal_on_conn(conn, &tenant, &region, job_uuid)
                                .await?;
                        let base_usage = aggregate_usage(
                            attempts.as_ref(),
                            ResourceUsage {
                                cpu_seconds: 0,
                                mem_byte_seconds: 0,
                            },
                        )?;
                        let (ci_run_id, usage) = resolve_terminal_usage_on_conn(
                            conn,
                            &accounting,
                            TerminalUsageResolutionInput {
                                tenant: &tenant,
                                wf_run_id: &claim.wf_run_id,
                                job_id: &claim.job_id,
                                reserve_handle: &reserve_handle,
                                base_usage,
                                parent_expectation: if matches!(
                                    disposition,
                                    PreparationTerminalDisposition::Failed {
                                        phase: PreparationPhase::SecretResolution
                                    }
                                ) {
                                    CiPrelaunchParentExpectation::OptionalBeforeLaunch
                                } else {
                                    CiPrelaunchParentExpectation::Required
                                },
                                unresolved_policy: CiPrelaunchUnresolvedPolicy::Refuse,
                            },
                        )
                        .await?;
                        if ci_run_id != claim.ci_run_id {
                            return Err(CompletionTxError::Refused);
                        }
                        verify_preparation_disposition_on_conn(
                            conn,
                            &claim,
                            &reserve_handle,
                            disposition,
                        )
                        .await?;
                        let receipts = preparation_completion_receipts(
                            PreparationCompletionReceiptInput {
                                tenant: &tenant,
                                region: &region,
                                wf_run_id: &claim.wf_run_id,
                                ci_run_id: &claim.ci_run_id,
                                job_id: &claim.job_id,
                                idem_token: &claim.idem_token,
                                stage: &stage,
                                reserve_handle: &reserve_handle,
                                usage,
                                lease_owner: &claim.lease_owner,
                                lease_epoch: claim.lease_epoch,
                                claim_nonce: &claim.claim_nonce,
                                claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
                                claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
                            },
                            disposition,
                        );
                        let consume_spec = PreparationClaimConsumeSpec {
                            tenant_id: tenant.as_str(),
                            region: &region,
                            job_id: job_uuid,
                            wf_run_id: wf_run_uuid,
                            ci_run_id: ci_run_uuid,
                            idem_token: &claim.idem_token,
                            lease_owner: &claim.lease_owner,
                            lease_epoch: claim.lease_epoch,
                            claim_nonce: nonce_uuid,
                            stage: &stage,
                            claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
                            claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
                            reserve_handle: &reserve_handle,
                            completion_receipt: &receipts.current_v4,
                        };
                        let claim_outcome = match disposition {
                            PreparationTerminalDisposition::Failed {
                                phase: PreparationPhase::SecretResolution,
                            } => {
                                CiJobQueueStore::consume_secret_withheld_claim_on_conn(
                                    conn,
                                    consume_spec,
                                )
                                .await?
                            }
                            PreparationTerminalDisposition::AttemptsExhausted => {
                                CiJobQueueStore::consume_preparation_claim_exhausted_on_conn(
                                    conn,
                                    consume_spec,
                                )
                                .await?
                            }
                            PreparationTerminalDisposition::Failed { .. }
                            | PreparationTerminalDisposition::TimedOut { .. } => {
                                CiJobQueueStore::consume_preparation_claim_on_conn(
                                    conn,
                                    consume_spec,
                                )
                                .await?
                            }
                        };
                        if claim_outcome == ClaimConsumeOutcome::Refused {
                            return Err(CompletionTxError::Refused);
                        }
                        let report = TerminalReport {
                            passed: false,
                            timed_out: matches!(
                                disposition,
                                PreparationTerminalDisposition::TimedOut { .. }
                            ),
                            usage,
                            result_refs: Vec::new(),
                        };
                        co_commit_terminal_accounting(
                            conn,
                            &accounting,
                            TerminalAccountingInput {
                                tenant: &tenant,
                                wf_run: &RunId(claim.wf_run_id.clone()),
                                ci_run_id: &claim.ci_run_id,
                                job_id: &claim.job_id,
                                reserve_handle: &reserve_handle,
                                report: &report,
                                receipts: &receipts,
                                disposition: CiJobTerminalDisposition::Preparation(disposition),
                                diagnostic: diagnostic.as_deref(),
                                replay: claim_outcome == ClaimConsumeOutcome::AlreadyConsumed,
                            },
                        )
                        .await?;
                        close_cancelled_run_if_accounted(conn, &accounting, &claim.wf_run_id)
                            .await?;
                        Ok(signal_outcome)
                    })
                },
            ),
        );
        match durable {
            Ok(outcome) => Ok(outcome),
            Err(CompletionTxError::Refused) => Err(ExecutorError::InvalidInput(format!(
                "ci.pipeline preparation completion refused (stale or divergent generation): job \
                 `{}` owner `{}` epoch `{}`",
                refusal_job, refusal_owner, refusal_epoch
            ))),
            Err(CompletionTxError::Spec(error)) => Err(ExecutorError::Storage(format!(
                "durable preparation dispatch read refused: {error}"
            ))),
            Err(CompletionTxError::Signal(error)) => Err(error),
            Err(CompletionTxError::Prelaunch(error)) => Err(ExecutorError::Storage(format!(
                "preparation usage resolution refused: {error}"
            ))),
            Err(CompletionTxError::Usage(error)) => Err(ExecutorError::Storage(format!(
                "preparation usage aggregation refused: {error}"
            ))),
            Err(CompletionTxError::Pricing(error)) => Err(ExecutorError::Storage(format!(
                "preparation accounting pricing refused: {error}"
            ))),
            Err(CompletionTxError::Money(error)) => Err(ExecutorError::Storage(format!(
                "preparation money settlement refused: {error}"
            ))),
            Err(CompletionTxError::Projection(error)) => Err(ExecutorError::Storage(format!(
                "preparation cost projection refused: {error}"
            ))),
            Err(CompletionTxError::Accounting(error)) => Err(ExecutorError::Storage(format!(
                "preparation accounting receipt refused: {error}"
            ))),
            Err(error) => Err(ExecutorError::Storage(format!(
                "preparation completion transaction failed: {error:?}"
            ))),
        }
    }

    pub fn report_preparation_retry(
        &self,
        claim: &CiJobTokenRequest,
    ) -> Result<PreparationRetryOutcome, ExecutorError> {
        claim.validate().map_err(|error| {
            ExecutorError::InvalidInput(format!(
                "ci.pipeline preparation retry refused: {}",
                error.0
            ))
        })?;
        if claim.tenant_id != self.tenant.0 || claim.region != self.region {
            return Err(ExecutorError::InvalidInput(
                "ci.pipeline preparation retry refused: reporter scope mismatch".into(),
            ));
        }
        let job_uuid = Uuid::parse_str(&claim.job_id)
            .map_err(|_| ExecutorError::InvalidInput("invalid job UUID".into()))?;
        let claim = claim.clone();
        let refusal_job = claim.job_id.clone();
        let refusal_owner = claim.lease_owner.clone();
        let refusal_epoch = claim.lease_epoch;
        let tenant = self.tenant.clone();
        let region = self.region.clone();
        let transaction_tenant = tenant.0.clone();
        let transaction_region = region.clone();
        let spec_store = self.spec_store.clone();

        let durable = bridge(
            &self.rt,
            with_tenant_tx_error(
                self.queue_store.pool(),
                &transaction_tenant,
                &transaction_region,
                move |conn| {
                    Box::pin(async move {
                        let identity = spec_store
                            .get_dispatch_identity_on_conn(
                                conn,
                                tenant.as_str(),
                                job_uuid,
                                &claim.job_id,
                            )
                            .await
                            .map_err(CompletionTxError::Spec)?;
                        let reserve_handle = identity
                            .as_ref()
                            .map(|identity| identity.reserve_handle.clone())
                            .ok_or(CompletionTxError::Refused)?;
                        let stage = verify_claimed_identity(
                            &tenant,
                            &tenant,
                            &claim.wf_run_id,
                            &claim.job_id,
                            &claim.idem_token,
                            identity,
                        )
                        .map_err(|_| CompletionTxError::Refused)?;
                        let launch = spec_store
                            .get_launch_template_on_conn(conn, tenant.as_str(), &claim.job_id)
                            .await
                            .map_err(CompletionTxError::Spec)?;
                        if launch.ci_run_id != claim.ci_run_id
                            || launch.token_authority_handle != claim.token_authority_handle
                            || launch.spec.idem_token.0 != claim.idem_token
                            || launch.spec.meter_to.reserve_id != reserve_handle
                            || derive_checkout_authorization_scope(
                                JobKind::Ci,
                                &launch.spec.workspace,
                            )
                            .map_err(|_| CompletionTxError::Refused)?
                            .is_none()
                        {
                            return Err(CompletionTxError::Refused);
                        }
                        if let PreparationRetryGate::NotLive =
                            verify_preparation_retry_permitted_on_conn(
                                conn,
                                &claim,
                                &reserve_handle,
                            )
                            .await?
                        {
                            return Ok(PreparationRequeueOutcome::NoOp);
                        }
                        let outcome = CiJobQueueStore::requeue_preparation_claim_on_conn(
                            conn,
                            PreparationRequeueSpec {
                                tenant_id: tenant.as_str(),
                                region: &region,
                                job_id: &claim.job_id,
                                wf_run_id: &claim.wf_run_id,
                                ci_run_id: &claim.ci_run_id,
                                idem_token: &claim.idem_token,
                                lease_owner: &claim.lease_owner,
                                lease_epoch: claim.lease_epoch,
                                claim_nonce: &claim.claim_nonce,
                                stage: &stage,
                                claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
                                claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
                                reserve_handle: &reserve_handle,
                            },
                        )
                        .await?;
                        Ok(outcome)
                    })
                },
            ),
        );
        match durable {
            Ok(PreparationRequeueOutcome::Requeued) => Ok(PreparationRetryOutcome::Requeued),
            Ok(PreparationRequeueOutcome::NoOp) => Ok(PreparationRetryOutcome::NoOp),
            Err(CompletionTxError::Refused) => Err(ExecutorError::InvalidInput(format!(
                "ci.pipeline preparation retry refused (stale or divergent generation): job `{}` \
                 owner `{}` epoch `{}`",
                refusal_job, refusal_owner, refusal_epoch
            ))),
            Err(CompletionTxError::Spec(error)) => Err(ExecutorError::Storage(format!(
                "durable preparation dispatch read refused: {error}"
            ))),
            Err(CompletionTxError::Prelaunch(error)) => Err(ExecutorError::Storage(format!(
                "preparation retry verification refused: {error}"
            ))),
            Err(error) => Err(ExecutorError::Storage(format!(
                "preparation retry transaction failed: {error:?}"
            ))),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn new(
        pg_executor: PgFlowExecutor,
        spec_store: CiJobSpecStore,
        queue_store: CiJobQueueStore,
        rt: tokio::runtime::Handle,
        tenant: TenantId,
        region: impl Into<String>,
    ) -> CiPipelineReporter {
        CiPipelineReporter {
            pg_executor,
            spec_store,
            queue_store,
            rt,
            tenant,
            region: region.into(),
            accounting: ReporterAccounting::TestBypass,
            #[cfg(any(test, feature = "test-support"))]
            test_executor: None,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    fn with_test_executor(mut self, executor: FlowExecutor) -> Self {
        self.test_executor = Some(executor);
        self
    }
}

pub(crate) fn token_request_from_preparation_report_claim(
    claim: &PreparationReportClaim,
) -> CiJobTokenRequest {
    CiJobTokenRequest {
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
    }
}

impl TerminalReporter for CiPipelineReporter {
    fn completion_settlement_owner(&self) -> CompletionSettlementOwner {
        match self.accounting {
            ReporterAccounting::Durable(_) => CompletionSettlementOwner::TerminalReporter,
            #[cfg(any(test, feature = "test-support"))]
            ReporterAccounting::TestBypass => CompletionSettlementOwner::Hook,
        }
    }

    fn report_done(
        &self,
        claim: &CompletionClaim,
        report: &TerminalReport,
    ) -> Result<SignalOutcome, ExecutorError> {
        let CompletionClaim {
            tenant,
            run,
            job_id,
            idem_token,
            lease_owner,
            lease_epoch,
            claim_nonce,
        } = claim;
        let lease_epoch = *lease_epoch;
        if tenant != &self.tenant {
            return Err(ExecutorError::InvalidInput(format!(
                "ci.pipeline job.done refused (unverified claim, fail-closed): {}",
                ClaimRefusal::TenantMismatch {
                    reporter: self.tenant.0.clone(),
                    claimed: tenant.0.clone(),
                }
            )));
        }
        if report.passed && report.timed_out {
            return Err(ExecutorError::InvalidInput(
                "ci.pipeline job.done refused: a timed-out job cannot pass".into(),
            ));
        }

        let job_uuid = Uuid::parse_str(job_id)
            .map_err(|_| ExecutorError::InvalidInput(format!("invalid job_id UUID `{job_id}`")))?;
        let nonce_uuid = Uuid::parse_str(claim_nonce).map_err(|_| {
            ExecutorError::InvalidInput("invalid claim_nonce UUID (completion refused)".into())
        })?;
        let tenant_owned = self.tenant.0.clone();
        let region_owned = self.region.clone();
        let run_owned = run.clone();
        let job_owned = job_id.to_string();
        let idem_owned = idem_token.to_string();
        let owner_owned = lease_owner.to_string();
        let nonce_owned = claim_nonce.to_string();
        let report_owned = report.clone();
        let spec_store = self.spec_store.clone();
        let pg_executor = self.pg_executor.clone();
        let accounting = self.accounting.clone();

        let durable = bridge(
            &self.rt,
            with_tenant_tx_error(
                self.queue_store.pool(),
                &self.tenant.0,
                &self.region,
                move |conn| {
                    Box::pin(async move {
                        let identity = spec_store
                            .get_dispatch_identity_on_conn(
                                conn,
                                &tenant_owned,
                                job_uuid,
                                &job_owned,
                            )
                            .await
                            .map_err(CompletionTxError::Spec)?;
                        let reserve_handle = identity
                            .as_ref()
                            .map(|identity| identity.reserve_handle.clone())
                            .ok_or(CompletionTxError::Refused)?;
                        let stage = verify_claimed_identity(
                            &TenantId(tenant_owned.clone()),
                            &TenantId(tenant_owned.clone()),
                            &run_owned.0,
                            &job_owned,
                            &idem_owned,
                            identity,
                        )
                        .map_err(|_| CompletionTxError::Refused)?;
                        let signal = TypedSignalSpec {
                            run: run_owned.clone(),
                            signal_name: JOB_DONE_SIGNAL.to_string(),
                            idem_key: idem_owned.clone(),
                            payload: SignalPayload::CiJobDone {
                                stage: stage.clone(),
                                passed: report_owned.passed,
                                result_refs: report_owned.result_refs.clone(),
                            },
                            payload_key_ref: None,
                        };
                        let outcome = pg_executor
                            .signal_typed_on_conn(conn, signal.clone())
                            .await
                            .map_err(CompletionTxError::Signal)?;
                        let attempts = retry_attempts_for_terminal_on_conn(
                            conn,
                            &TenantId(tenant_owned.clone()),
                            &region_owned,
                            job_uuid,
                        )
                        .await?;
                        let mut accounted_report = report_owned.clone();
                        accounted_report.usage =
                            aggregate_usage(attempts.as_ref(), report_owned.usage)?;
                        let (ci_run_id, usage) = match &accounting {
                            ReporterAccounting::Durable(accounting) => {
                                resolve_terminal_usage_on_conn(
                                    conn,
                                    accounting,
                                    TerminalUsageResolutionInput {
                                        tenant: &TenantId(tenant_owned.clone()),
                                        wf_run_id: &run_owned.0,
                                        job_id: &job_owned,
                                        reserve_handle: &reserve_handle,
                                        base_usage: accounted_report.usage,
                                        parent_expectation: CiPrelaunchParentExpectation::Required,
                                        unresolved_policy: CiPrelaunchUnresolvedPolicy::Refuse,
                                    },
                                )
                                .await?
                            }
                            #[cfg(any(test, feature = "test-support"))]
                            ReporterAccounting::TestBypass => {
                                (String::new(), accounted_report.usage)
                            }
                        };
                        accounted_report.usage = usage;
                        let disposition = workload_disposition(&accounted_report);
                        let receipts = completion_receipts_v4(
                            CompletionReceiptInput {
                                tenant: &TenantId(tenant_owned.clone()),
                                region: &region_owned,
                                run: &run_owned,
                                job_id: &job_owned,
                                idem_token: &idem_owned,
                                stage: &stage,
                                passed: accounted_report.passed,
                                timed_out: accounted_report.timed_out,
                                usage: accounted_report.usage,
                                result_refs: &accounted_report.result_refs,
                                lease_owner: &owner_owned,
                                lease_epoch,
                                claim_nonce: &nonce_owned,
                            },
                            disposition,
                        );
                        let write_version = match &accounting {
                            ReporterAccounting::Durable(accounting) => {
                                accounting.receipt_store.write_version()
                            }
                            #[cfg(any(test, feature = "test-support"))]
                            ReporterAccounting::TestBypass => CiJobAccountingWriteVersion::V3,
                        };
                        let (completion_receipt, alternate_replay_receipt) = match write_version {
                            CiJobAccountingWriteVersion::V3 => (
                                receipts.legacy_v3.as_str(),
                                Some(receipts.current_v4.as_str()),
                            ),
                            CiJobAccountingWriteVersion::V4 => (
                                receipts.current_v4.as_str(),
                                Some(receipts.legacy_v3.as_str()),
                            ),
                        };
                        let claim = CiJobQueueStore::consume_claim_on_conn(
                            conn,
                            ClaimConsumeSpec {
                                tenant_id: &tenant_owned,
                                job_id: job_uuid,
                                lease_owner: &owner_owned,
                                lease_epoch,
                                claim_nonce: nonce_uuid,
                                stage: &stage,
                                completion_receipt,
                                alternate_replay_receipt,
                            },
                        )
                        .await?;
                        if claim == ClaimConsumeOutcome::Refused {
                            return Err(CompletionTxError::Refused);
                        }
                        match &accounting {
                            ReporterAccounting::Durable(accounting) => {
                                co_commit_terminal_accounting(
                                    conn,
                                    accounting,
                                    TerminalAccountingInput {
                                        tenant: &TenantId(tenant_owned.clone()),
                                        wf_run: &run_owned,
                                        ci_run_id: &ci_run_id,
                                        job_id: &job_owned,
                                        reserve_handle: &reserve_handle,
                                        report: &accounted_report,
                                        receipts: &receipts,
                                        disposition,
                                        diagnostic: None,
                                        replay: claim == ClaimConsumeOutcome::AlreadyConsumed,
                                    },
                                )
                                .await?;
                                close_cancelled_run_if_accounted(conn, accounting, &run_owned.0)
                                    .await?;
                            }
                            #[cfg(any(test, feature = "test-support"))]
                            ReporterAccounting::TestBypass => {}
                        }
                        Ok((outcome, signal))
                    })
                },
            ),
        );
        let (outcome, signal) = match durable {
            Ok(value) => value,
            Err(CompletionTxError::Refused) => {
                return Err(ExecutorError::InvalidInput(format!(
                    "ci.pipeline job.done refused (unverified, stale, or divergent claim): job \
                     `{job_id}` owner `{lease_owner}` epoch `{lease_epoch}`"
                )))
            }
            Err(CompletionTxError::Spec(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "durable claimed-job read refused: {error}"
                )))
            }
            Err(CompletionTxError::Manifest) => {
                return Err(ExecutorError::Storage(
                    "durable CI launch authority could not be verified".into(),
                ))
            }
            Err(CompletionTxError::Pricing(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "terminal CI accounting refused: {error}"
                )))
            }
            Err(CompletionTxError::Money(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "terminal CI money settlement refused: {error}"
                )))
            }
            Err(CompletionTxError::Projection(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "terminal CI cost projection refused: {error}"
                )))
            }
            Err(CompletionTxError::Accounting(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "terminal CI accounting receipt refused: {error}"
                )))
            }
            Err(CompletionTxError::CancelledClosure) => {
                return Err(ExecutorError::Storage(
                    "cancelled CI run accounting closure was refused".into(),
                ))
            }
            Err(CompletionTxError::Signal(error)) => return Err(error),
            Err(CompletionTxError::RetryStore) => {
                return Err(ExecutorError::Storage(
                    "durable retry-attempt store failed".into(),
                ))
            }
            Err(CompletionTxError::RetryCorrupt) => {
                return Err(ExecutorError::Storage(
                    "durable retry-attempt state is corrupt".into(),
                ))
            }
            Err(CompletionTxError::Prelaunch(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "terminal CI prelaunch usage resolution refused: {error}"
                )))
            }
            Err(CompletionTxError::Usage(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "terminal CI usage aggregation refused: {error}"
                )))
            }
            Err(CompletionTxError::Scope(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "atomic completion transaction failed: {error}"
                )))
            }
        };

        #[cfg(any(test, feature = "test-support"))]
        if let Some(executor) = &self.test_executor {
            executor.signal_typed(signal)?;
            executor.runs().wake(&self.tenant, &run.0);
        }
        #[cfg(not(any(test, feature = "test-support")))]
        let _ = signal;
        Ok(outcome)
    }

    fn report_retryable_attempt(
        &self,
        claim: &CompletionClaim,
        failure: &RetryableAttemptFailure,
    ) -> Result<RetryableAttemptOutcome, ExecutorError> {
        if claim.tenant != self.tenant {
            return Err(ExecutorError::InvalidInput(
                "ci.pipeline retryable attempt refused: reporter tenant mismatch".into(),
            ));
        }
        let job_uuid = Uuid::parse_str(&claim.job_id).map_err(|_| {
            ExecutorError::InvalidInput("invalid job_id UUID in retryable attempt".into())
        })?;
        Uuid::parse_str(&claim.claim_nonce).map_err(|_| {
            ExecutorError::InvalidInput("invalid claim_nonce UUID in retryable attempt".into())
        })?;
        let tenant_owned = self.tenant.0.clone();
        let region_owned = self.region.clone();
        let claim_owned = claim.clone();
        let failure_owned = *failure;
        let spec_store = self.spec_store.clone();
        let accounting = self.accounting.clone();
        let durable = bridge(
            &self.rt,
            with_tenant_tx_error(
                self.queue_store.pool(),
                &self.tenant.0,
                &self.region,
                move |conn| {
                    Box::pin(async move {
                        let flow_state: String = sqlx::query_scalar(
                            "SELECT state FROM workflow_run
                             WHERE tenant_id = $1 AND region = $2 AND run_id = $3
                             FOR UPDATE",
                        )
                        .bind(&tenant_owned)
                        .bind(&region_owned)
                        .bind(&claim_owned.run.0)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|_| CompletionTxError::RetryStore)?
                        .ok_or(CompletionTxError::Refused)?;
                        let requeue = matches!(flow_state.as_str(), "running" | "waiting");
                        let cancelled_ci_run = if !requeue {
                            if flow_state != "terminated" {
                                return Err(CompletionTxError::Refused);
                            }
                            let ci_run: Option<(String, String)> = sqlx::query_as(
                                "SELECT run_id::text, state FROM ci_run
                                 WHERE tenant_id = $1 AND region = $2 AND wf_run_id = $3::uuid",
                            )
                            .bind(&tenant_owned)
                            .bind(&region_owned)
                            .bind(&claim_owned.run.0)
                            .fetch_optional(&mut *conn)
                            .await
                            .map_err(|_| CompletionTxError::RetryStore)?;
                            let (ci_run_id, ci_state) = ci_run.ok_or(CompletionTxError::Refused)?;
                            if ci_state != "cancelled" {
                                return Err(CompletionTxError::Refused);
                            }
                            Some(ci_run_id)
                        } else {
                            None
                        };
                        let identity = spec_store
                            .get_dispatch_identity_on_conn(
                                conn,
                                &tenant_owned,
                                job_uuid,
                                &claim_owned.job_id,
                            )
                            .await
                            .map_err(CompletionTxError::Spec)?;
                        let reserve_handle = identity
                            .as_ref()
                            .map(|identity| identity.reserve_handle.clone())
                            .ok_or(CompletionTxError::Refused)?;
                        verify_claimed_identity(
                            &TenantId(tenant_owned.clone()),
                            &claim_owned.tenant,
                            &claim_owned.run.0,
                            &claim_owned.job_id,
                            &claim_owned.idem_token,
                            identity,
                        )
                        .map_err(|_| CompletionTxError::Refused)?;
                        let outcome = record_retryable_attempt_on_conn(
                            conn,
                            &region_owned,
                            &claim_owned,
                            &failure_owned,
                            requeue,
                        )
                        .await?;
                        if !requeue {
                            let attempts = retry_attempts_for_terminal_on_conn(
                                conn,
                                &TenantId(tenant_owned.clone()),
                                &region_owned,
                                job_uuid,
                            )
                            .await?
                            .ok_or(CompletionTxError::RetryCorrupt)?;
                            let report = TerminalReport {
                                passed: false,
                                timed_out: false,
                                usage: ResourceUsage {
                                    cpu_seconds: attempts.cpu_seconds,
                                    mem_byte_seconds: attempts.mem_byte_seconds,
                                },
                                result_refs: Vec::new(),
                            };
                            match &accounting {
                                ReporterAccounting::Durable(accounting) => {
                                    let (ci_run_id, usage) = resolve_terminal_usage_on_conn(
                                        conn,
                                        accounting,
                                        TerminalUsageResolutionInput {
                                            tenant: &TenantId(tenant_owned.clone()),
                                            wf_run_id: &claim_owned.run.0,
                                            job_id: &claim_owned.job_id,
                                            reserve_handle: &reserve_handle,
                                            base_usage: report.usage,
                                            parent_expectation:
                                                CiPrelaunchParentExpectation::Required,
                                            unresolved_policy: CiPrelaunchUnresolvedPolicy::Refuse,
                                        },
                                    )
                                    .await?;
                                    let report = TerminalReport { usage, ..report };
                                    let legacy_v3 = crate::ci_run_supersession::superseded_receipt(
                                        &accounting.scope,
                                        cancelled_ci_run
                                            .as_deref()
                                            .ok_or(CompletionTxError::Refused)?,
                                        &claim_owned.run.0,
                                        &claim_owned.job_id,
                                        &reserve_handle,
                                        report.usage,
                                        false,
                                    );
                                    let disposition =
                                        CiJobTerminalDisposition::CancelledAfterWorkloadLaunch;
                                    let receipts = CompletionReceipts {
                                        current_v4: disposition_receipt_v4(&legacy_v3, disposition),
                                        legacy_v3,
                                    };
                                    co_commit_terminal_accounting(
                                        conn,
                                        accounting,
                                        TerminalAccountingInput {
                                            tenant: &TenantId(tenant_owned.clone()),
                                            wf_run: &claim_owned.run,
                                            ci_run_id: &ci_run_id,
                                            job_id: &claim_owned.job_id,
                                            reserve_handle: &reserve_handle,
                                            report: &report,
                                            receipts: &receipts,
                                            disposition,
                                            diagnostic: None,
                                            replay: outcome == RetryableAttemptOutcome::ExactReplay,
                                        },
                                    )
                                    .await?;
                                    close_cancelled_run_if_accounted(
                                        conn,
                                        accounting,
                                        &claim_owned.run.0,
                                    )
                                    .await?;
                                }
                                #[cfg(any(test, feature = "test-support"))]
                                ReporterAccounting::TestBypass => {
                                    return Err(CompletionTxError::Refused);
                                }
                            }
                        }
                        Ok(outcome)
                    })
                },
            ),
        );
        match durable {
            Ok(outcome) => Ok(outcome),
            Err(CompletionTxError::Refused) => Err(ExecutorError::InvalidInput(format!(
                "ci.pipeline retryable attempt refused (unverified, stale, or divergent claim): \
                 job `{}` owner `{}` epoch `{}`",
                claim.job_id, claim.lease_owner, claim.lease_epoch
            ))),
            Err(CompletionTxError::Spec(error)) => Err(ExecutorError::Storage(format!(
                "durable retryable-attempt dispatch read refused: {error}"
            ))),
            Err(CompletionTxError::Scope(error)) => Err(ExecutorError::Storage(format!(
                "atomic retryable-attempt transaction failed: {error}"
            ))),
            Err(CompletionTxError::RetryStore) => Err(ExecutorError::Storage(
                "durable retry-attempt store failed".into(),
            )),
            Err(CompletionTxError::RetryCorrupt) => Err(ExecutorError::Storage(
                "durable retry-attempt state is corrupt".into(),
            )),
            Err(CompletionTxError::Prelaunch(error)) => Err(ExecutorError::Storage(format!(
                "terminal retry prelaunch usage resolution refused: {error}"
            ))),
            Err(CompletionTxError::Usage(error)) => Err(ExecutorError::Storage(format!(
                "terminal retry usage aggregation refused: {error}"
            ))),
            Err(_) => Err(ExecutorError::Storage(
                "retryable-attempt transaction reached an invalid accounting path".into(),
            )),
        }
    }

    fn report_preparation_terminal(
        &self,
        claim: &PreparationReportClaim,
        disposition: PreparationTerminalDisposition,
        diagnostic: Option<&str>,
    ) -> Result<SignalOutcome, ExecutorError> {
        CiPipelineReporter::report_preparation_terminal_with_diagnostic(
            self,
            &token_request_from_preparation_report_claim(claim),
            disposition,
            diagnostic,
        )
    }

    fn report_preparation_retry(
        &self,
        claim: &PreparationReportClaim,
    ) -> Result<PreparationRetryReport, ExecutorError> {
        match CiPipelineReporter::report_preparation_retry(
            self,
            &token_request_from_preparation_report_claim(claim),
        )? {
            PreparationRetryOutcome::Requeued => Ok(PreparationRetryReport::Requeued),
            PreparationRetryOutcome::NoOp => Ok(PreparationRetryReport::NoOp),
        }
    }
}

#[derive(Clone)]
#[cfg(any(test, feature = "test-support"))]
struct RunPlan {
    pipeline: PipelineRun,
    terms: JobScheduleTerms,
}

#[cfg(any(test, feature = "test-support"))]
pub struct CiPipelineDriver {
    executor: FlowExecutor,
    pg_executor: PgFlowExecutor,
    tenant: TenantId,
    region: String,
    journal: WfJournal,
    outbox: OutboxStore,
    telemetry: FlowTelemetry,
    timers: TimerStore,
    minter: Arc<dyn IdMinter>,
    ctx_base: EmitContextBase,
    spec_store: CiJobSpecStore,
    rt: tokio::runtime::Handle,
    build_spec: StageSpecBuilder,
    plans: Arc<Mutex<HashMap<String, RunPlan>>>,
    started: Arc<Mutex<Vec<String>>>,
}

#[cfg(any(test, feature = "test-support"))]
impl CiPipelineDriver {
    pub fn new(
        tenant: TenantId,
        region: impl Into<String>,
        spec_store: CiJobSpecStore,
        rt: tokio::runtime::Handle,
        build_spec: StageSpecBuilder,
        outbox: OutboxStore,
    ) -> CiPipelineDriver {
        let region = region.into();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        let executor = FlowExecutor::new(minter.clone(), tenant.clone(), Region(region.clone()));
        let pg_executor = PgFlowExecutor::new(
            spec_store.pool().clone(),
            rt.clone(),
            minter.clone(),
            tenant.clone(),
            Region(region.clone()),
        );
        executor.register_definition(CI_PIPELINE_WF_TYPE);
        CiPipelineDriver {
            executor,
            pg_executor,
            tenant: tenant.clone(),
            region: region.clone(),
            journal: WfJournal::new(),
            outbox,
            telemetry: FlowTelemetry::new(),
            timers: TimerStore::new(),
            minter,
            ctx_base: service_ctx_base(&tenant, &region),
            spec_store,
            rt,
            build_spec,
            plans: Arc::new(Mutex::new(HashMap::new())),
            started: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn executor(&self) -> FlowExecutor {
        self.executor.clone()
    }

    pub fn reporter(&self) -> CiPipelineReporter {
        CiPipelineReporter::new(
            self.pg_executor.clone(),
            self.spec_store.clone(),
            CiJobQueueStore::with_pg(self.spec_store.pool().clone()),
            self.rt.clone(),
            self.tenant.clone(),
            self.region.clone(),
        )
        .with_test_executor(self.executor.clone())
    }

    pub fn outbox(&self) -> &OutboxStore {
        &self.outbox
    }

    pub fn start_run(
        &self,
        record: &CiRunRecord,
        pipeline: PipelineRun,
        labels: Vec<String>,
    ) -> Result<RunId, StartRunError> {
        validate_driver_tenant(&self.tenant, record)?;
        let trust_tier = trust_from_token(&record.trust_tier).map_err(StartRunError::TrustTier)?;
        let terms = JobScheduleTerms {
            tenant_id: record.tenant_id.clone(),
            region: record.region.clone(),
            run_id: record.wf_run_id.clone(),
            lane: Lane::Interactive,
            labels,
            trust_tier,
            concurrency_group: None,
            fair_key: record.tenant_id.clone(),
        };
        self.plans
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(record.wf_run_id.clone(), RunPlan { pipeline, terms });
        {
            let mut started = self.started.lock().unwrap_or_else(|e| e.into_inner());
            if !started.contains(&record.wf_run_id) {
                started.push(record.wf_run_id.clone());
            }
        }
        self.pg_executor
            .register_definition(CI_PIPELINE_WF_TYPE, 1, "blake3:ci-pipeline-driver-v1")
            .map_err(StartRunError::Start)?;
        let durable = self
            .pg_executor
            .start_with_id(
                StartSpec {
                    wf_type: CI_PIPELINE_WF_TYPE.into(),
                    input: vec![],
                    budget: None,
                    idem_key: format!("ci:{}", record.run_id),
                },
                Some(RunId(record.wf_run_id.clone())),
            )
            .map_err(StartRunError::Start)?;
        let memory = self
            .executor
            .start_with_id(
                StartSpec {
                    wf_type: CI_PIPELINE_WF_TYPE.into(),
                    input: vec![],
                    budget: None,
                    idem_key: format!("ci:{}", record.run_id),
                },
                Some(RunId(record.wf_run_id.clone())),
            )
            .map_err(StartRunError::Start)?;
        if durable != memory {
            return Err(StartRunError::Start(ExecutorError::RunIdConflict(
                record.wf_run_id.clone(),
            )));
        }
        Ok(durable)
    }

    fn body(&self) -> Box<WorkflowBody> {
        let plans = self.plans.clone();
        let spec_store = self.spec_store.clone();
        let rt = self.rt.clone();
        let build_spec = self.build_spec.clone();
        Box::new(move |ctx: &mut WfCtx| {
            let run_id = ctx.run_id().to_string();
            let plan = plans
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&run_id)
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "no PipelineRun registered for ci.pipeline run `{run_id}` - the starter must \
                         register the plan before start_with_id (CT-004d.2 chunk 3)"
                    )
                })?;
            let runner = DurableJobRunner::new(
                spec_store.clone(),
                rt.clone(),
                plan.terms.clone(),
                build_spec.clone(),
                &plan.pipeline.stages,
            );
            let verdict =
                run_ci_pipeline_body(ctx, &plan.pipeline, &runner).map_err(|e| format!("{e:?}"))?;
            Ok(match verdict {
                RunVerdict::Succeeded { stages_completed } => {
                    vec![ArtifactRef(format!("outcome:succeeded:{stages_completed}"))]
                }
                RunVerdict::Failed { stage } => {
                    vec![ArtifactRef(format!("outcome:failed:{stage}"))]
                }
                RunVerdict::Rejected { stage } => {
                    vec![ArtifactRef(format!("outcome:rejected:{stage}"))]
                }
                RunVerdict::Parked => vec![],
            })
        })
    }

    fn dispatcher(&self, partition: i16) -> FlowDispatcher {
        let mut disp = FlowDispatcher::new(
            self.executor.runs().clone(),
            self.outbox.clone(),
            self.journal.clone(),
            self.telemetry.clone(),
            self.minter.clone(),
            self.ctx_base.clone(),
            partition,
            "ci-pipeline-driver",
            30,
        )
        .with_signals(self.executor.signals().clone())
        .with_timers(self.timers.clone());
        disp.register(CI_PIPELINE_WF_TYPE, self.body());
        disp
    }

    pub fn drive_once(&self, now: i64, now_clock: &str) -> Vec<DriveOutcome> {
        for run_id in self
            .started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
        {
            self.executor.runs().wake(&self.tenant, run_id);
        }
        let mut outcomes = Vec::new();
        for p in 0..PARTITION_COUNT as i16 {
            let disp = self.dispatcher(p);
            if let Some(o) = disp.tick(now, now_clock, 7) {
                outcomes.push(o);
            }
        }
        outcomes
    }

    pub fn is_terminal(&self, run: &RunId) -> Option<bool> {
        self.executor
            .describe(run)
            .ok()
            .map(|status| status.terminal)
    }

    pub fn run_state(&self, run: &RunId) -> Option<String> {
        self.executor.describe(run).ok().map(|s| s.state)
    }

    pub fn region(&self) -> &str {
        &self.region
    }
}

#[derive(Debug)]
#[cfg(any(test, feature = "test-support"))]
pub enum StartRunError {
    TenantMismatch {
        driver_tenant: String,
        record_tenant: String,
    },
    TrustTier(JobQueueStoreError),
    Start(ExecutorError),
}

#[cfg(any(test, feature = "test-support"))]
impl std::fmt::Display for StartRunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartRunError::TenantMismatch {
                driver_tenant,
                record_tenant,
            } => write!(
                f,
                "ci.pipeline start refused: driver tenant `{driver_tenant}` does not match durable ci_run tenant `{record_tenant}`"
            ),
            StartRunError::TrustTier(e) => {
                write!(f, "ci.pipeline start refused: corrupt trust_tier token: {e}")
            }
            StartRunError::Start(e) => write!(f, "ci.pipeline start_with_id failed: {e}"),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl std::error::Error for StartRunError {}

#[cfg(any(test, feature = "test-support"))]
fn validate_driver_tenant(
    driver_tenant: &TenantId,
    record: &CiRunRecord,
) -> Result<(), StartRunError> {
    if driver_tenant.0 == record.tenant_id {
        Ok(())
    } else {
        Err(StartRunError::TenantMismatch {
            driver_tenant: driver_tenant.0.clone(),
            record_tenant: record.tenant_id.clone(),
        })
    }
}

#[cfg(any(test, feature = "test-support"))]
fn service_ctx_base(tenant: &TenantId, region: &str) -> EmitContextBase {
    EmitContextBase {
        tenant: tenant.clone(),
        region: Region(region.to_string()),
        actor: Actor(Principal::stub(
            PrincipalId("ci-controlplane".into()),
            PrincipalKind::Service,
            tenant.clone(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-07-17T00:00:00Z".into()),
        recorded_at: Timestamp("2026-07-17T00:00:00Z".into()),
        caused_by: None,
    }
}

fn deterministic_uuid(seed: &str) -> String {
    let fill = |salt: u64| -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ salt;
        for b in seed.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    };
    let a = fill(0);
    let b = fill(0x00ff_00ff_00ff_00ff);
    let bytes = [a.to_be_bytes(), b.to_be_bytes()].concat();
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8],
        bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

#[cfg(any(test, feature = "test-support"))]
pub fn fixed_command_spec_builder(
    image: &str,
    command: Vec<String>,
    timeout_secs: u32,
) -> Result<StageSpecBuilder, String> {
    let image = ImageRef::pinned(image).map_err(|e| e.to_string())?;
    Ok(Arc::new(move |_flow_spec: &FlowJobSpec| {
        SandboxJobSpec::new(
            SandboxJobKind::Ci,
            image.clone(),
            command.clone(),
            vec![],
            vec![],
            EgressPolicy::deny_all(),
            ResourceLimits {
                cpu_millis: 1000,
                mem_bytes: 256 * 1024 * 1024,
                disk_bytes: 1 << 30,
                tmpfs_bytes: 1 << 30,
                pids_max: 128,
                timeout_secs,
            },
            WorkspaceSpec::default(),
            TrustTier::Trusted,
            RunTokenCredential::new("ci-pipeline-driver-bearer", "ci-pipeline-driver-jti", 300)
                .expect("static driver credential is valid"),
            MeterTarget {
                reserve_id: "ci-pipeline-driver-reserve".into(),
            },
            IdemToken(String::new()),
        )
        .map_err(|e| e.to_string())
    }))
}

#[cfg(test)]
#[path = "ci_pipeline_driver_tests.rs"]
mod tests;

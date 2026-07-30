//! # Durable CI dispatch and claim-bound completion
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §3.1 (the pipeline IS a durable workflow — `run_ci_pipeline_body`), §3.3 (the
//! `SCHEDULE_AND_RUN_JOB` dispatch handshake → the durable `job_queue` row), §2.1 (the pull-lease claim
//! the CT-004c.2 runner drives) + arch 01 §3.1 (`ci_run` is the thin index over the myelin-flow run).
//!
//! CT-004b armed a durable `ci_run` and pre-minted `wf_run_id`; CT-004c made the runner claim a
//! durable `job_queue` row and execute it in gVisor; CT-004d.1 co-persisted `job_queue` and
//! `ci_job_spec`. This module owns the remaining production-safe components at that boundary:
//!
//! - **Chunk 5 — [`DurableJobRunner`]:** the [`myelin_flow::JobRunner`] the pipeline body dispatches
//!   each stage through. Instead of [`crate::SchedulerJobRunner`]'s in-memory `SchedulerState`, it
//!   builds a [`DurableEnqueue`] + the digest-pinned sandbox [`SandboxJobSpec`] and calls
//!   [`CiJobSpecStore::co_persist_dispatch`] — so each stage lands a DURABLE `job_queue` row (+ its
//!   `ci_job_spec`) the CT-004c.2 runner claims. **THE SECURITY INVARIANT:** the enqueue's `trust_tier`
//!   + `region` come from the run's real [`JobScheduleTerms`] (stamped from `ci_run.trust_tier` /
//!   `ci_run.region` at trigger time), forwarded UNCHANGED, and the SAME tier is stamped onto the
//!   sandbox spec — so `co_persist_dispatch`'s `enq.trust_tier == spec.trust_tier` gate holds by
//!   construction and an `untrusted_fork` stage can NEVER be enqueued behind a widened `trusted` gate.
//! - **Claim-bound completion — [`CiPipelineReporter`]:** verifies and consumes the exact live queue
//!   claim, settles Storage money truth, writes CI's cost projection and immutable accounting
//!   receipt, and inserts the typed `job.done` signal in one PostgreSQL transaction. A production
//!   [`myelin_flow::PgFlowWorker`] wakes from and consumes that durable signal directly. Exact
//!   redelivery reads the stored pricing outcome; historical work is never repriced.
//!
//! ## The verdict-vocabulary bridge (why a bespoke reporter, not `EngineTerminalReporter`)
//! The real sandbox runner ([`myelin_ci_sandbox::RunnerAgent::run_one`]) DERIVES `passed` from the guest
//! exit code and reports it as a `myelin://job-done/passed-<bool>` marker; the pipeline body
//! ([`myelin_flow::WfCtx::run_ci_pipeline`]) decodes the stage verdict from a
//! [`myelin_flow::stage_verdict_marker`] (`ci.stage.verdict:<pass|fail>:<stage>`). Neither frozen body
//! is touched (the sandbox `run_one` security body, the engine fixture). The bridge is the
//! [`myelin_ci_sandbox::TerminalReporter`] seam — a legitimate injection point the runner already
//! depends on abstractly: [`CiPipelineReporter`] re-encodes the runner's derived `passed` into the
//! stage-verdict marker the body decodes. The stage name is co-persisted on `job_queue` and
//! `ci_job_spec`; completion reads that durable identity in the same transaction that consumes the
//! claim and buffers the typed PostgreSQL workflow signal.
//!
//! ## The durable-RunStore FLOOR (named, not silently skipped)
//! The former same-process `CiPipelineDriver` is compiled only with `test-support`. It remains a
//! compatibility harness for historical tests and is absent from the default production build. The
//! production activation floor is now explicit: define a restart-safe CI body-input manifest and
//! DAG-aware execution semantics before composing `PgFlowWorker`; V2 launch authority remains
//! refused until its resource, egress, workspace, token, metering, and check capabilities exist.

use std::collections::BTreeMap;
#[cfg(any(test, feature = "test-support"))]
use std::collections::HashMap;
use std::sync::Arc;
#[cfg(any(test, feature = "test-support"))]
use std::sync::Mutex;

use myelin_ci_sandbox::{
    CompletionClaim, CompletionSettlementOwner, IdemToken, JobSpec as SandboxJobSpec,
    ResourceUsage, RetryableAttemptCause, RetryableAttemptFailure, RetryableAttemptOutcome,
    TerminalReport, TerminalReporter,
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
    with_tenant_tx_error, DurableCostLedger, DurableSettleError, MeteredUnit, MinorUnits, PgError,
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
};
use crate::job_spec_store::{
    CiJobSpecStore, CiJobSpecStoreError, ClaimedDispatchIdentity, DurableCiJobLaunchTemplate,
    MAX_JOB_TIMEOUT_SECS,
};
use crate::metering::{CostEventRow, CostKind, Meter};
use crate::schedule_and_run_job::JobScheduleTerms;
#[cfg(any(test, feature = "test-support"))]
use crate::scheduler::Lane;

/// Bridge one async durable-store call to a sync body on a dedicated OFF-runtime thread (the SAME
/// convention [`crate::runner_bind`] + `myelin_storage::kms_durable` use). The pipeline `tick` runs on
/// its own thread; the `try_current` guard falls back to `block_in_place` if ever driven on a
/// multi-thread worker.
fn bridge<F: std::future::Future>(rt: &tokio::runtime::Handle, fut: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(_) => tokio::task::block_in_place(|| rt.block_on(fut)),
        Err(_) => rt.block_on(fut),
    }
}

/// **The seam that resolves a dispatched stage to its digest-pinned sandbox [`SandboxJobSpec`] template
/// (the `.myelin/ci.toml` resolved-snapshot → executable-spec resolution).** Given the flow
/// [`FlowJobSpec`] the pipeline body dispatched (its opaque `target` names the pipeline step), it
/// returns the image/command/limits/egress/workspace the sandbox launches. `Err` is a fail-closed
/// resolve (the stage never becomes a launchable durable job). **The builder does NOT set the
/// security-load-bearing `trust_tier` or the `idem_token`** — [`DurableJobRunner::dispatch`] STAMPS
/// those from the run's terms + the dispatch, so a builder can never widen the trust tier.
///
/// In production the impl resolves the pinned snapshot's per-stage command; the CT-004d.2 integration
/// test injects a real compute spec that runs in a `runsc` guest. Until the snapshot resolver lands
/// (the named follow-on), [`unresolved_stage_spec_builder`] is the fail-closed default.
pub type StageSpecBuilder =
    Arc<dyn Fn(&FlowJobSpec) -> Result<SandboxJobSpec, String> + Send + Sync>;

/// **The fail-closed default stage-spec builder (the snapshot→spec resolver is the named follow-on).**
/// Returns `Err` for every stage — a driver wired with this dispatches NOTHING (the activity retries +
/// the run fails loud), never a fabricated spec. The real resolver (the pinned `.myelin/ci.toml`
/// snapshot → per-stage command/image) is CT-004d.3+; the integration test injects a real builder.
pub fn unresolved_stage_spec_builder() -> StageSpecBuilder {
    Arc::new(|spec: &FlowJobSpec| {
        Err(format!(
            "no pinned-snapshot → JobSpec resolver yet (CT-004d follow-on) for stage target `{}`; \
             the driver cannot fabricate an executable spec — dispatch refused fail-closed",
            spec.target
        ))
    })
}

// =================================================================================================
// Chunk 5 — the DURABLE JobRunner.
// =================================================================================================

/// **Chunk 5 — the DURABLE [`JobRunner`] the pipeline body dispatches each stage through.** Replaces
/// [`crate::SchedulerJobRunner`]'s in-memory `SchedulerState`: on [`JobRunner::dispatch`] it builds a
/// [`DurableEnqueue`] + the sandbox [`SandboxJobSpec`] and calls [`CiJobSpecStore::co_persist_dispatch`]
/// — one atomic tenant-scoped tx writes the `job_queue` row (the claim gate) + the `ci_job_spec` row
/// (what EXECUTES), idempotent on the engine-minted `idem_token`. Constructed PER RUN (it holds the
/// run's [`JobScheduleTerms`]); the pipeline body closure builds it fresh for each drive.
///
/// **THE SECURITY INVARIANT (the adversarial-verifier surface).** The `trust_tier` + `region` the
/// enqueue gates the claim on come from `self.terms` (the run's real facts, stamped from
/// `ci_run.trust_tier` / `ci_run.region` at trigger time), forwarded UNCHANGED — never widened,
/// defaulted, or dropped. The SAME `terms.trust_tier` is stamped onto the sandbox spec, so
/// `co_persist_dispatch`'s `enq.trust_tier == spec.trust_tier` assertion holds BY CONSTRUCTION (it is
/// not bypassed — it is fed the truth). The [`StageSpecBuilder`] never sets the tier, so it cannot
/// widen it. An `untrusted_fork` run therefore enqueues every stage as `untrusted_fork`, and the
/// CT-004c.2 trusted-only runner never claims it (the durable predicate) — the poisoned-pipeline
/// defence, closed at the dispatch.
pub struct DurableJobRunner {
    store: CiJobSpecStore,
    rt: tokio::runtime::Handle,
    /// the run's real scheduling terms — tenant/region/run_id/lane/labels/trust_tier/fair_key, a PURE
    /// function of the resolved snapshot (the trust tier stamped at trigger time). Forwarded UNCHANGED.
    terms: JobScheduleTerms,
    build_spec: StageSpecBuilder,
    /// `(stage target → stage name)` for THIS run's pipeline — so a dispatched flow `JobSpec` (which
    /// carries the opaque `target`, not the stage name) maps back to the stage name persisted DURABLY
    /// onto the `ci_job_spec` row (the reporter reads it back at `job.done`, restart-safe). Built from
    /// the [`PipelineRun`]'s stages at construction. A dispatch whose target is not a known stage fails
    /// closed (never a durable row the reporter cannot attribute a verdict to).
    targets: Vec<(String, String)>,
}

impl DurableJobRunner {
    /// Build the durable runner for one run: the durable `ci_job_spec` store, the runtime handle the
    /// async co-persist bridges onto, the run's [`JobScheduleTerms`] (the security-load-bearing tier +
    /// region), the [`StageSpecBuilder`], and the run's pipeline stages (for the target → name map).
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

    /// The deterministic `job_queue.job_id` (a `uuid`) for a dispatched stage — derived PURELY from the
    /// engine-minted `idem_token` so a re-dispatch (control-plane replay) re-derives the SAME id and the
    /// `(tenant_id, job_id)` PK collapses it to one row. (The `idem_token` itself — `<run_id>/…:<n>/job`
    /// — is NOT a uuid, so it can not be the durable `job_id` directly; it stays the `jq_idem` key.)
    fn stage_job_id(idem_token: &str) -> String {
        deterministic_uuid(&format!("jobq:{idem_token}"))
    }

    /// **The PURE (DB-free) half of a dispatch** — delegates to [`build_dispatch_parts`] (a free fn so
    /// the SECURITY invariant is unit-testable with NO store/pool at all). Returns the enqueue + the
    /// spec whose `trust_tier` equals `enq.trust_tier` by construction (`co_persist_dispatch` re-asserts).
    fn build_dispatch(
        &self,
        flow_spec: &FlowJobSpec,
    ) -> Result<(DurableEnqueue, SandboxJobSpec), ActivityError> {
        build_dispatch_parts(&self.terms, &self.build_spec, flow_spec)
    }
}

/// **The pure (store-free) dispatch builder — the SECURITY-load-bearing half.** Builds the
/// [`DurableEnqueue`] + the sandbox [`SandboxJobSpec`] the co-persist writes, forwarding the run's
/// `trust_tier` + `region` from `terms` UNCHANGED and STAMPING the SAME tier + the echo `idem_token`
/// onto the spec — so `co_persist_dispatch`'s `enq.trust_tier == spec.trust_tier` gate holds BY
/// CONSTRUCTION and the [`StageSpecBuilder`] can never widen the tier. A free fn (no `self`, no store)
/// so the invariant is provable with zero DB/pool surface.
fn build_dispatch_parts(
    terms: &JobScheduleTerms,
    build_spec: &StageSpecBuilder,
    flow_spec: &FlowJobSpec,
) -> Result<(DurableEnqueue, SandboxJobSpec), ActivityError> {
    // Resolve the stage's executable template (image/command/limits/egress/workspace).
    let mut spec = (build_spec)(flow_spec).map_err(ActivityError)?;

    // SECURITY — stamp the run's REAL trust_tier onto the spec (forwarded UNCHANGED from
    // terms.trust_tier), and the engine-minted idem_token the runner echoes on job.done. So the
    // enqueue + the spec carry the SAME tier by construction — never widened by the builder.
    spec.trust_tier = terms.trust_tier;
    spec.idem_token = IdemToken(flow_spec.idem_token.clone());
    // Belt-and-suspenders: clamp the wall-clock timeout to the store's ceiling so a legitimate stage
    // never trips the fail-closed TimeoutTooLong (the lease-outliving double-run guard).
    if spec.limits.timeout_secs > MAX_JOB_TIMEOUT_SECS {
        spec.limits.timeout_secs = MAX_JOB_TIMEOUT_SECS;
    }

    // The DURABLE enqueue — trust_tier + region FROM the run's terms (forwarded UNCHANGED);
    // idem_token = the engine's dispatch token (the jq_idem key + the job.done echo key).
    let enq = DurableEnqueue {
        tenant_id: terms.tenant_id.clone(),
        region: terms.region.clone(),
        job_id: DurableJobRunner::stage_job_id(&flow_spec.idem_token),
        run_id: terms.run_id.clone(),
        lane: terms.lane,
        labels: terms.labels.clone(),
        trust_tier: terms.trust_tier, // == spec.trust_tier (both terms.trust_tier)
        concurrency_group: terms.concurrency_group.clone(),
        fair_key: terms.fair_key.clone(),
        idem_token: flow_spec.idem_token.clone(),
        stage: flow_spec.target.clone(),
    };
    Ok((enq, spec))
}

impl JobRunner for DurableJobRunner {
    fn dispatch(&self, flow_spec: &FlowJobSpec) -> Result<(), ActivityError> {
        let (mut enq, spec) = self.build_dispatch(flow_spec)?;

        // Resolve the dispatched stage's NAME (the reporter attributes the verdict to it). It is
        // persisted DURABLY onto the ci_job_spec row (not an in-memory map), so a fresh reporter after
        // a restart reads it back. A target that is not a known pipeline stage FAILS CLOSED — a durable
        // job the reporter could never attribute a verdict to must never be enqueued.
        let stage = self
            .targets
            .iter()
            .find(|(t, _)| t == &flow_spec.target)
            .map(|(_, name)| name.clone())
            .ok_or_else(|| {
                ActivityError(format!(
                    "ci.pipeline dispatch refused: target `{}` is not a known pipeline stage — the \
                     verdict could not be durably attributed (fail-closed)",
                    flow_spec.target
                ))
            })?;
        enq.stage = stage.clone();
        let authority = format!("legacy-test-authority:{}", spec.run_token.jti);
        let (spec, _previous_token) = spec.into_template();
        let launch = DurableCiJobLaunchTemplate {
            ci_run_id: enq.run_id.clone(),
            spec,
            token_authority_handle: authority,
        };

        // Co-persist the job_queue row + the ci_job_spec row (carrying the durable stage) in ONE
        // tenant-scoped tx (bridged onto the runtime). A dispatch failure surfaces as an ActivityError
        // the engine retries (reusing the SAME idem_token — the durable ON CONFLICT dedups the re-dispatch).
        bridge(
            &self.rt,
            self.store.co_persist_dispatch(&enq, &launch, &stage),
        )
        .map_err(|e| ActivityError(format!("durable co_persist_dispatch refused: {e}")))?;
        Ok(())
    }
}

// =================================================================================================
// The durable-completion-authority reporter (the external-reviewer blocker: verify the claim first).
// =================================================================================================

/// **Why a refused completion is fail-closed** — the reasons [`CiPipelineReporter::report_done`] rejects
/// a `job.done` BEFORE any verdict is signalled (nothing durable changes on a refusal). Each is a
/// forged / mis-keyed / unclaimed completion: the caller does not own the durable job it claims.
#[derive(Debug, PartialEq, Eq)]
pub enum ClaimRefusal {
    /// The completion's claimed `tenant` is not the tenant this reporter's executor is bound to. A
    /// region runner claims cross-tenant; a reporter is tenant-bound, so a mis-routed completion is
    /// refused rather than signalled against the wrong tenant's run.
    TenantMismatch { reporter: String, claimed: String },
    /// No durable `ci_job_spec` dispatch record exists for `(tenant, job_id)` — the job was never
    /// dispatched/claimed under this identity (a fabricated completion), so it is refused.
    NoDispatchRecord { job_id: String },
    /// The durable dispatch record's `run_id` does not match the completion's `run` — the claim names a
    /// different run than the one it was dispatched for.
    RunMismatch { durable: String, claimed: String },
    /// The durable dispatch record's `idem_token` does not match the echoed one — the claim was keyed
    /// to a different dispatch.
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

/// **The PURE (DB-free) claimed-job verification — the security core, unit-testable with NO pool.**
/// Given the completion's claimed authority `(claimed_tenant, presented_run, presented_job_id,
/// presented_idem_token)`, the reporter's bound `reporter_tenant`, and the durable dispatch record read for
/// `(tenant, job_id)`, returns the durable stage the verdict attributes to — or a [`ClaimRefusal`]
/// (fail-closed, nothing signalled). Every field of the durable claimed-job identity must match; a
/// forged / mis-keyed / unclaimed completion resolves no matching record and is refused.
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

/// **The deterministic, nonce-keyed completion receipt the CAS records.** It length-frames tenant,
/// region, run, job, idem token, durable stage, verdict, timeout status, actual usage, ordered result
/// refs, owner, epoch, and the fresh claim nonce. Exact at-least-once redelivery recomputes the same
/// receipt; any authority, verdict, accounting, or payload divergence is refused. The row CAS still
/// proves live ownership; the keyed receipt is its tamper-evident idempotency evidence.
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

/// Mint the new disposition-bound receipt while retaining the exact historical v3 encoder above.
/// The v3 twin is needed both for old-row replay and for the byte-frozen accounting column during a
/// rolling deployment.
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

/// Last immutable measured-attempt receipt retained beside fixed-size cumulative usage on
/// `job_queue` until a later terminal generation settles the aggregate. The JSON object is PII-free
/// and constant-size across arbitrarily many retries.
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

/// Build the exact durable `RetryAttemptRecord` a real attempt would persist — a small PURE helper
/// (no DB access) extracted from `record_retryable_attempt_on_conn` so the write-side cause binding
/// is directly unit-testable. This is the exact site Sol's review caught hardcoding
/// `OUTPUT_PERSISTENCE_CAUSE` regardless of `failure.cause` — a unit test against THIS function,
/// not just `decode_retry_attempts`, is what would actually catch a regression back to that bug.
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

/// A typed refusal before raw usage reaches bigint-backed accounting or pricing.
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

/// Immutable-pricing output for the two raw resource dimensions a sandbox reports. There is no
/// built-in or permissive production price: an adapter must name the pricing revision and provide
/// the wholesale/markup split for both CPU and memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PricedCiJobUsage {
    pub pricing_revision: String,
    pub memory_gb_seconds: u64,
    pub cpu_wholesale: MinorUnits,
    pub cpu_markup: MinorUnits,
    pub memory_wholesale: MinorUnits,
    pub memory_markup: MinorUnits,
}

/// Frozen Tier-P settlement policy. Paired with both `ci-reserve:v1:` and (CT-007 slice 5b.3-4a.1b)
/// `ci-reserve:v2:` reservation authority -- `v2` only changes the reservation-amount topology and
/// durable handle shape, never the zero-markup settlement policy itself.
pub const TIER_P_OPERATIONAL_PRICING_REVISION: &str = "tier-p-operational:v1";
pub(crate) const TIER_P_OPERATIONAL_RESERVATION_PREFIX: &str = "ci-reserve:v1:";
/// CT-007 slice 5b.3-4a.1b: the `v2` reservation-handle prefix (parent-attempt budget authority,
/// design locked with Sol 2026-07-29). Same Tier-P pricing revision and zero-markup settlement
/// policy as `v1` -- only the reservation-amount topology and durable handle shape differ.
pub(crate) const TIER_P_OPERATIONAL_RESERVATION_V2_PREFIX: &str = "ci-reserve:v2:";
const PRICING_GIB_BYTES: u64 = 1_073_741_824;

/// A fail-closed pricing refusal. Values and authority handles are deliberately absent.
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

/// Immutable completion-accounting lookup. Tier P uses an explicitly revisioned operational-unit
/// adapter with zero markup; Tier B may later compose a Commercial-owned price lookup. Either way,
/// the resulting revision and split amounts are persisted so replay never reprices, and the adapter
/// must use the same unit policy as the reservation that admitted the job.
pub trait CiJobAccountingPricer: Send + Sync {
    fn price(&self, usage: ResourceUsage) -> Result<PricedCiJobUsage, CiJobPricingError>;
}

/// Bind a versioned reservation authority to its exact settlement unit policy. Generic and future
/// Commercial handles remain governed by their own revisioned adapters, but a Tier-P operational
/// handle cannot be settled by an arbitrary monetary pricer merely because it implements the trait.
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
        && priced.cpu_wholesale == MinorUnits(usage.cpu_seconds)
        && priced.cpu_markup == MinorUnits::ZERO
        && priced.memory_wholesale == MinorUnits(memory_gb_seconds)
        && priced.memory_markup == MinorUnits::ZERO;
    if exact_operational_policy {
        Ok(())
    } else {
        Err(CiJobPricingError::InvalidOutput)
    }
}

/// All durable authorities required by the production terminal reporter. Constructing an accounted
/// reporter is impossible without the money ledger, CI projection, immutable receipt store,
/// canonical manifest authority, verified tenant scope, and an explicit pricing adapter.
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
    replay: bool,
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
    // BUG FIX (investigation, 2026-07-25): a retryable-attempt requeue moves `job_queue` back to
    // `queued` here, but historically never touched the `ci_job` DAG surface
    // `AUTHORIZE_JOB_LAUNCH_QUERY` also crosses to `running` in the SAME statement as the launch
    // CAS. Without this reset, `ci_job.state` stayed stuck at `'running'` from the FIRST attempt, so
    // a fresh runner generation claiming the requeued row could never re-win the launch fence
    // (`surface.state IN ('queued', 'leased')`, deliberately pinned in `job_queue_store.rs`'s unit
    // tests) — the retry would be permanently stranded. Mirrors the same reset added to the
    // dead-runner reaper (`job_queue_region.rs::RESET_REAPED_CI_JOB_SURFACE_QUERY`) for the analogous
    // crash-recovery path.
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
        input.report,
        surface_disposition,
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

async fn settle_ci_job_surface_on_conn(
    conn: &mut sqlx::PgConnection,
    tenant: &TenantId,
    region: &Region,
    ci_run_id: &str,
    job_id: &str,
    report: &TerminalReport,
    disposition: Option<CiJobTerminalDisposition>,
) -> Result<(), CompletionTxError> {
    let state = if report.passed && !report.timed_out {
        "succeeded"
    } else {
        "failed"
    };
    let summary = terminal_result_summary(report, disposition);
    let updated = sqlx::query_scalar::<_, Uuid>(
        "UPDATE ci_job
         SET state=$5, result_summary=$6
         WHERE tenant_id=$1 AND region=$2 AND run_id=$3::uuid AND job_id=$4::uuid
           AND (
             state IN ('queued','leased','running')
             OR (state=$5 AND result_summary=$6)
           )
         RETURNING job_id",
    )
    .bind(tenant.as_str())
    .bind(region.as_str())
    .bind(ci_run_id)
    .bind(job_id)
    .bind(state)
    .bind(summary)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| CompletionTxError::Accounting(CiJobAccountingError::Db("job surface update")))?;
    if updated.is_some() {
        Ok(())
    } else {
        Err(CompletionTxError::Refused)
    }
}

fn terminal_result_summary(
    report: &TerminalReport,
    disposition: Option<CiJobTerminalDisposition>,
) -> serde_json::Value {
    match disposition {
        Some(disposition) => serde_json::json!({
            "passed": report.passed,
            "timed_out": report.timed_out,
            "disposition": disposition.as_storage_token(),
            "workload_started": disposition.workload_started(),
        }),
        None => serde_json::json!({
            "passed": report.passed,
            "timed_out": report.timed_out,
        }),
    }
}

/// Canonical surface summary for 5b's future preparation-only terminal CAS. It is intentionally
/// pure and cannot accept caller-supplied pass/usage/text fields.
#[allow(dead_code)]
pub(crate) fn preparation_terminal_result_summary(
    disposition: myelin_ci_sandbox::PreparationTerminalDisposition,
) -> serde_json::Value {
    let timed_out = matches!(
        disposition,
        myelin_ci_sandbox::PreparationTerminalDisposition::TimedOut { .. }
    );
    let disposition = CiJobTerminalDisposition::Preparation(disposition);
    serde_json::json!({
        "passed": false,
        "timed_out": timed_out,
        "disposition": disposition.as_storage_token(),
        "workload_started": false,
    })
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

/// **The [`TerminalReporter`] that VERIFIES durable claimed-job identity before signalling a verdict
/// (the external reviewer's blocker).** The runner ([`myelin_ci_sandbox::RunnerAgent`]) derives `passed`
/// from the real guest exit code and calls `report_done` carrying the CLAIMED row's authority
/// `(tenant, run, job_id, idem_token, owner, epoch, nonce)`. This reporter:
///
/// 1. **Verifies the claim.** It reads the durable `ci_job_spec` dispatch record for `(tenant, job_id)`
///    and refuses fail-closed ([`ClaimRefusal`]) unless the claimed `tenant`, `job_id`
///    `run_id`, and `idem_token` ALL match the durable record — so a
///    caller cannot forge a completion for a job it does not own, and the idem token is no longer a
///    predictable `(run_id, command_id)` free pass (it must match a real claimed row).
/// 2. **Resolves the stage DURABLY.** The verdict-attribution stage name comes from the `ci_job_spec.stage`
///    column (persisted at dispatch) — a restart-safe read, never an in-memory map. So a fresh reporter
///    over the same PG resolves the verdict exactly.
/// 3. **Accounts, consumes, and signals in one transaction.** The queue CAS binds the fresh nonce,
///    durable stage, and a receipt over every canonical completion field. The same transaction
///    settles Storage's reservation, writes CI's per-meter projection and immutable receipt, and then
///    [`PgFlowExecutor::signal_typed_on_conn`] buffers `job.done`. Exact replay reuses the persisted
///    pricing revision instead of repricing. A late completion returns
///    [`SignalOutcome::TerminalNoOp`] (an acknowledged no-op), so the runner settles its lease instead
///    of retrying forever.
#[derive(Clone)]
pub struct CiPipelineReporter {
    pg_executor: PgFlowExecutor,
    spec_store: CiJobSpecStore,
    queue_store: CiJobQueueStore,
    rt: tokio::runtime::Handle,
    tenant: TenantId,
    region: String,
    accounting: ReporterAccounting,
    /// Compatibility-only mirror for the historical in-memory culmination harness. This field and
    /// all code that touches it are absent from normal production builds.
    #[cfg(any(test, feature = "test-support"))]
    test_executor: Option<FlowExecutor>,
}

impl CiPipelineReporter {
    /// Exact tenant scope this reporter verifies before any durable completion work.
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    /// Exact residency region used for every reporter transaction.
    pub fn region(&self) -> &str {
        &self.region
    }

    /// Build the production reporter with all terminal-accounting authorities present. There is no
    /// default pricer and no production constructor that bypasses the atomic accounting co-commit.
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

    /// Compatibility constructor for historical test fixtures. It does not exist in a production
    /// build, so a composition root cannot accidentally activate a reporter without accounting.
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

    /// Attach the legacy in-memory executor only for the test-support culmination harness. The
    /// production reporter has no such field or method in a default build.
    #[cfg(any(test, feature = "test-support"))]
    fn with_test_executor(mut self, executor: FlowExecutor) -> Self {
        self.test_executor = Some(executor);
        self
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
        // ── BLOCKER 2: the tenant check is FIRST, before ANY database access. The reporter is
        // tenant-bound; a cross-tenant completion is refused BEFORE the caller-supplied tenant can reach
        // the RLS GUC. Every durable query below uses self.tenant (the verified value), never the caller's.
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
                        // Lock Flow before the scheduler/accounting rows. Run supersession uses the
                        // same order, so a canceller cannot hold a queue row while waiting for Flow
                        // as this reporter holds Flow while waiting for that queue row.
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
                        // The sandbox report is workload-only. Checkout preparation is authoritative
                        // in `ci_job_prelaunch_usage` and is added exactly once below; folding it
                        // into `TerminalReport` as well would double-account it.
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
            // Compatibility for the historical test harness only. Production builds contain no
            // process-local mirror: PgFlowWorker consumes the PostgreSQL signal directly.
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
                                // `retry_attempts` contains workload attempts only. Prelaunch usage
                                // is resolved from its journal immediately below.
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
}

// =================================================================================================
// Chunk 2 + 3 — the pipeline driver (register + drive the body; start with the pre-minted id).
// =================================================================================================

/// One run's plan the registered body resolves by `run_id`: its [`PipelineRun`] (the ordered stages +
/// the X-1 producer facts) + its [`JobScheduleTerms`] (the security-load-bearing tier/region the
/// durable runner forwards). Populated by [`CiPipelineDriver::start_run`] BEFORE the run is started.
#[derive(Clone)]
#[cfg(any(test, feature = "test-support"))]
struct RunPlan {
    pipeline: PipelineRun,
    terms: JobScheduleTerms,
}

/// **Chunks 2 + 3 — the CI pipeline DRIVER (the same-process engine over the shared executor).** Owns
/// the [`FlowExecutor`] the runner's `job.done` wakes, registers `run_ci_pipeline_body` under
/// [`CI_PIPELINE_WF_TYPE`] (with a per-run [`DurableJobRunner`] injected), and `tick`s a background
/// dispatcher over the SHARED `RunStore`/`SignalStore`. [`start_run`](Self::start_run) reads a durable
/// `ci_run` (queued) row and starts the parked run under the pre-minted `wf_run_id` — so the parked
/// run's id EQUALS the `job_queue` row's `run_id` the runner reports to.
///
/// **Durable-drive FLOOR (named):** start and typed completion signal are PostgreSQL-durable, but body
/// dispatch still mirrors through the in-memory executor. Production activation remains refused until
/// `PgFlowDriveStore` owns lease/replay end to end.
#[cfg(any(test, feature = "test-support"))]
pub struct CiPipelineDriver {
    executor: FlowExecutor,
    pg_executor: PgFlowExecutor,
    tenant: TenantId,
    region: String,
    // the shared durable-workflow substrate the dispatcher drives over (RunStore/SignalStore come from
    // the executor; the rest are the driver's).
    journal: WfJournal,
    outbox: OutboxStore,
    telemetry: FlowTelemetry,
    timers: TimerStore,
    minter: Arc<dyn IdMinter>,
    ctx_base: EmitContextBase,
    // the chunk-5 wiring the registered body composes per run.
    spec_store: CiJobSpecStore,
    rt: tokio::runtime::Handle,
    build_spec: StageSpecBuilder,
    // run_id → RunPlan (the per-run pipeline + terms the registered body resolves).
    plans: Arc<Mutex<HashMap<String, RunPlan>>>,
    // the run ids this driver started (so drive_once can wake any parked run robustly).
    started: Arc<Mutex<Vec<String>>>,
}

#[cfg(any(test, feature = "test-support"))]
impl CiPipelineDriver {
    /// Build the driver for a cell `(tenant, region)`. Constructs the shared [`FlowExecutor`] +
    /// registers [`CI_PIPELINE_WF_TYPE`]; the `spec_store` + `rt` + `build_spec` are the chunk-5 durable
    /// dispatch seam the registered body composes.
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

    /// The SHARED [`FlowExecutor`] the parked pipeline runs on — the runner loop's reporter signals
    /// THIS executor (one signal path). A cloneable handle (shared `Arc<Mutex<…>>` state).
    pub fn executor(&self) -> FlowExecutor {
        self.executor.clone()
    }

    /// Build the [`CiPipelineReporter`] the runner loop drives (over this driver's shared executor +
    /// durable spec store). The runner's `job.done` is VERIFIED against the durable claimed-job identity
    /// (tenant/run/job_id/idem_token), the stage is resolved from the durable `ci_job_spec.stage`
    /// column, and the typed verdict wakes the parked run.
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

    /// The outbox the pipeline body's X-1 producer emits (`ci.run.succeeded` / `ci.check.updated` /
    /// `ci.result`) co-commit into. Shared so a test/driver can read the emitted terminal facts.
    pub fn outbox(&self) -> &OutboxStore {
        &self.outbox
    }

    /// **Chunk 3 — start the parked `ci.pipeline` run under the pre-minted `wf_run_id`.** Registers the
    /// run's [`RunPlan`] (so the registered body resolves it by `run_id`), then calls
    /// [`DurableExecutor::start_with_id`] with `Some(RunId(record.wf_run_id))`. Idempotent on the
    /// `idem_key` (`ci:<run_id>`): a re-drive (a restart re-reading the queued `ci_run`) returns the
    /// EXISTING run — never a second run. `record.trust_tier` / `record.region` are forwarded UNCHANGED
    /// into the run's [`JobScheduleTerms`] (the durable runner's security-load-bearing source).
    ///
    /// `labels` are the runner-affinity labels the stage jobs require (a job is claimable iff
    /// `labels ⊆ runner_labels`) — from the resolved snapshot (the CT-004d follow-on); the caller
    /// supplies them here.
    pub fn start_run(
        &self,
        record: &CiRunRecord,
        pipeline: PipelineRun,
        labels: Vec<String>,
    ) -> Result<RunId, StartRunError> {
        validate_driver_tenant(&self.tenant, record)?;
        // Forward the run's STAMPED trust tier UNCHANGED (parse the ci_run.trust_tier CHECK token). A
        // corrupt token is a loud refusal — never a silent widen/default.
        let trust_tier = trust_from_token(&record.trust_tier).map_err(StartRunError::TrustTier)?;
        let terms = JobScheduleTerms {
            tenant_id: record.tenant_id.clone(),
            region: record.region.clone(),
            run_id: record.wf_run_id.clone(),
            lane: Lane::Interactive, // a PR/push CI check is the interactive lane (arch 02 §2.3)
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

    /// The registered `ci.pipeline` body: resolve the run's [`RunPlan`] by `run_id`, build a per-run
    /// [`DurableJobRunner`] (chunk 5), and drive [`run_ci_pipeline_body`] (which dispatches each stage
    /// through the durable queue + emits the X-1 producer facts). The body is FLOW-DETERMINISTIC: the
    /// plan/terms are fixed at start (no clock/RNG/IO), the dispatch rides the journaled activity, the
    /// verdict rides the journaled `job.done`.
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
                        "no PipelineRun registered for ci.pipeline run `{run_id}` — the starter must \
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

    /// Build a fresh [`FlowDispatcher`] over the SHARED substrate for a partition (the dogfood
    /// per-tick-worker shape). The `RunStore`/`SignalStore` are the executor's (so a `start_with_id`
    /// seeds a run this dispatcher leases + drives, and the runner's `job.done` signal is the one this
    /// consumes); the journal/timers/outbox/telemetry are the driver's persistent shared handles.
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

    /// **One drive pass: wake every started run, then `tick` every partition.** The wake is the robust
    /// re-drive (idempotent — it only flips `waiting → running`): a run with no new `job.done` replays
    /// to its park point and re-parks (cheap); a run whose `job.done` arrived advances. This closes the
    /// report-before-park race the one-shot reporter wake alone could miss. Each `tick` leases + drives
    /// at most one runnable run per partition; the driver loop calls this repeatedly. Returns the
    /// non-idle drive outcomes this pass observed (a test reads `Completed`/`Failed`; the loop ignores).
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

    /// Whether a started run has reached a TERMINAL engine state (completed/failed/terminated/
    /// nondeterministic) — the `ci_run` is the thin index over this myelin-flow run (arch 01 §3.1).
    /// `None` for an unknown run.
    pub fn is_terminal(&self, run: &RunId) -> Option<bool> {
        self.executor
            .describe(run)
            .ok()
            .map(|status| status.terminal)
    }

    /// The engine `state` of a started run (running/waiting/completed/failed/…), for the driver loop /
    /// a test to poll. `None` for an unknown run.
    pub fn run_state(&self, run: &RunId) -> Option<String> {
        self.executor.describe(run).ok().map(|s| s.state)
    }

    /// The cell region this driver runs in.
    pub fn region(&self) -> &str {
        &self.region
    }
}

/// Why [`CiPipelineDriver::start_run`] refused — a corrupt stamped trust token, or an executor start
/// failure (unknown workflow / a pre-minted-id collision with a DIFFERENT run). Surfaced, never swallowed.
#[derive(Debug)]
#[cfg(any(test, feature = "test-support"))]
pub enum StartRunError {
    /// The durable run belongs to a different tenant than this per-tenant driver. Refused before a
    /// plan is registered or an engine run/job is created, so a region-wide starter cannot stamp
    /// one tenant's authority or fair-queue key onto another tenant's run.
    TenantMismatch {
        /// Tenant this driver was composed for.
        driver_tenant: String,
        /// Authoritative tenant read from `ci_run.tenant_id`.
        record_tenant: String,
    },
    /// The `ci_run.trust_tier` token was outside the frozen CHECK vocabulary (a corrupt run-of-record) —
    /// refused loudly rather than defaulting the tier the durable dispatch gates on.
    TrustTier(JobQueueStoreError),
    /// The executor `start_with_id` failed (unknown workflow, or a pre-minted `wf_run_id` collision with
    /// a DIFFERENT run — fail-closed, never a silent clobber).
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

/// Enforce the per-tenant driver boundary before any mutable in-memory/durable orchestration state
/// is touched. A future region-wide queued-run poller must route each record to a driver composed for
/// exactly this authoritative tenant; it may never reuse a synthetic service tenant.
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

// =================================================================================================
// Helpers.
// =================================================================================================

/// The service emit context the driver's dispatcher stamps onto the co-committed X-1 producer events
/// (`ci.run.succeeded` / `ci.check.updated` / `ci.result`). A platform-service principal (no PII), the
/// cell `(tenant, region)`. The deterministic timestamps keep the body replay-stable (the body reads no
/// clock outside `WfCtx`).
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

/// **A deterministic uuid-shaped string from a seed (2×-salted FNV-1a fill).** Mirrors
/// `myelin_ci_dispatch::deterministic_uuid` (the leaf crate can not be a dependency of this one) so a
/// re-dispatch derives the SAME `job_queue.job_id` (the `(tenant_id, job_id)` PK idempotency anchor).
/// Non-cryptographic — it keys a DEDUP boundary (a collision would merge two stages' durable rows), not
/// an auth boundary; the trust gate is the forwarded `trust_tier`, not this id.
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

/// **A digest-pinned compute [`SandboxJobSpec`] builder for a fixed test command.** Produces a
/// `kind=ci` spec running `command` in a `runsc` guest,
/// default-deny egress, a read-only workspace. The `trust_tier` + `idem_token` are placeholders the
/// [`DurableJobRunner`] OVERWRITES from the run's terms + the dispatch (so this builder can never widen
/// the tier). `image` MUST be digest-pinned (fail-closed via [`ImageRef::pinned`]).
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
            // placeholders — DurableJobRunner::dispatch overwrites both from the run's terms + dispatch.
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

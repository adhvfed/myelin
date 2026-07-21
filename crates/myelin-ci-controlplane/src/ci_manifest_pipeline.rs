//! Manifest-backed production body for the durable `ci.pipeline` workflow.
//!
//! The starter persists exactly two workflow input references. This module resolves those references
//! under Flow's fenced lease to the insert-only manifest, verifies the workflow identity/code pin,
//! and then drives only the manifest DAG. The synchronous body performs no database or clock reads
//! outside `WfCtx`.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use myelin_events::check_seam::{check_updated_draft, ci_result_draft, rollup_ci_result};
use myelin_events::{AggregateKey, ArtifactRef, DataRole, EventDraft, EventType, Visibility};
use myelin_flow::{
    read_stage_verdict, ActivityError, DispatchedJob, JobKind, JobOutcome, JobRunner, JobSpec,
    PgClaimedDriveInput, PgFlowWorker, PgInputResolveError, PgResolvedDriveInput, PgWorkerError,
    PgWorkflowInputResolver, RetryPolicy, WaitOutcome, WfCtx, WfError, WfResult, JOB_DONE_SIGNAL,
};
use myelin_tenancy::{Region, TenantId};
use sqlx::PgPool;

use crate::check_emitter::{
    check_status_payload, CheckEmitContext, CheckProvider, CheckState, CostPosture, TrustTier,
};
use crate::ci_drive_manifest::{
    CiDriveManifestError, CiDriveManifestStore, CiDriveManifestV1, CiManifestTrustTierV1,
    GrantedCiJobV1,
};
use crate::ci_run_store::{
    CiRunFinalization, CiRunFinalizationJob, CiRunFinalizer, CiRunTerminalState,
};
use crate::pg_pipeline_starter::{decode_ci_claimed_input, CiWorkflowDefinitionPin};
use crate::CI_PIPELINE_WF_TYPE;

/// Scope- and code-pinned resolver installed alongside the production `ci.pipeline` body.
#[derive(Clone)]
pub struct CiManifestInputResolver {
    store: CiDriveManifestStore,
    tenant: TenantId,
    region: Region,
    definition: CiWorkflowDefinitionPin,
}

impl CiManifestInputResolver {
    pub fn new(
        pool: PgPool,
        tenant: TenantId,
        region: Region,
        definition: CiWorkflowDefinitionPin,
    ) -> Result<Self, CiDriveManifestError> {
        let store = CiDriveManifestStore::new(pool, tenant.clone(), region.clone())?;
        Ok(Self {
            store,
            tenant,
            region,
            definition,
        })
    }

    pub fn definition(&self) -> &CiWorkflowDefinitionPin {
        &self.definition
    }

    async fn resolve_manifest(
        &self,
        input: PgClaimedDriveInput,
    ) -> Result<Vec<u8>, PgInputResolveError> {
        if input.tenant != self.tenant
            || input.region != self.region
            || input.wf_type != CI_PIPELINE_WF_TYPE
            || input.wf_version != self.definition.version()
        {
            return Err(PgInputResolveError::Permanent(
                "claimed workflow scope or definition does not match the registered CI body".into(),
            ));
        }
        let claimed = decode_ci_claimed_input(&self.tenant, &input.input)
            .map_err(|error| PgInputResolveError::Permanent(error.to_string()))?;
        let manifest = self
            .store
            .load_expected(
                &input.run_id,
                claimed.ci_run_id(),
                claimed.manifest_digest(),
            )
            .await
            .map_err(map_manifest_load_error)?;
        if manifest.tenant_id != self.tenant.0
            || manifest.region != self.region.0
            || manifest.wf_run_id != input.run_id
            || manifest.ci_run_id != claimed.ci_run_id()
            || manifest.workflow_type != input.wf_type
            || manifest.workflow_definition_version != input.wf_version
            || manifest.workflow_code_hash != self.definition.code_hash()
        {
            return Err(PgInputResolveError::Permanent(
                "loaded CI manifest does not match the claimed workflow identity and code pin"
                    .into(),
            ));
        }
        manifest
            .canonical_bytes()
            .map_err(|error| PgInputResolveError::Permanent(error.to_string()))
    }
}

impl PgWorkflowInputResolver for CiManifestInputResolver {
    fn resolve(
        &self,
        input: PgClaimedDriveInput,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, PgInputResolveError>> + Send + '_>> {
        Box::pin(self.resolve_manifest(input))
    }
}

/// Install the code-pinned resolver and manifest-native body as one definition. The runner capture
/// is an effect adapter only; all workflow decisions and job targets come from resolved manifest
/// bytes and journaled `WfCtx` reads.
pub fn register_ci_manifest_pipeline<R>(
    worker: &mut PgFlowWorker,
    resolver: CiManifestInputResolver,
    runner: Arc<R>,
    finalizer: Arc<dyn CiRunFinalizer>,
) -> Result<(), PgWorkerError>
where
    R: JobRunner + Send + Sync + 'static,
{
    let version = resolver.definition.version();
    let code_hash = resolver.definition.code_hash().to_owned();
    worker.register_definition_with_input_resolver(
        CI_PIPELINE_WF_TYPE,
        version,
        &code_hash,
        resolver,
        move |input, ctx| {
            drive_resolved_ci_manifest_pipeline(input, ctx, runner.as_ref(), finalizer.as_ref())
        },
    )
}

fn map_manifest_load_error(error: CiDriveManifestError) -> PgInputResolveError {
    match error {
        CiDriveManifestError::Database { operation, .. } => PgInputResolveError::Retry(format!(
            "CI drive manifest database operation `{operation}` is unavailable"
        )),
        other => PgInputResolveError::Permanent(other.to_string()),
    }
}

/// Terminal or parked result of one manifest-native DAG drive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiManifestPipelineOutcome {
    Parked,
    Succeeded { jobs_completed: usize },
    Failed { job: String, timed_out: bool },
}

/// Decode the resolver's canonical bytes, re-bind them to the claimed drive, and execute the DAG.
/// This is the body installed by [`register_ci_manifest_pipeline`].
pub fn drive_resolved_ci_manifest_pipeline<R>(
    input: &PgResolvedDriveInput,
    ctx: &mut WfCtx,
    runner: &R,
    finalizer: &dyn CiRunFinalizer,
) -> Result<Vec<ArtifactRef>, String>
where
    R: JobRunner,
{
    let manifest = decode_resolved_ci_manifest(input)?;
    run_ci_manifest_pipeline(ctx, &manifest, runner, finalizer)
        .map_err(|error| format!("manifest CI workflow failed: {error:?}"))?;
    Ok(vec![ArtifactRef(manifest.run_ref)])
}

/// Decode canonical resolver output and bind it again to the claimed workflow identity.
pub fn decode_resolved_ci_manifest(
    input: &PgResolvedDriveInput,
) -> Result<CiDriveManifestV1, String> {
    let manifest = CiDriveManifestV1::decode_canonical(&input.material)
        .map_err(|error| format!("resolved CI manifest refused: {error}"))?;
    if manifest.tenant_id != input.claimed.tenant.0
        || manifest.region != input.claimed.region.0
        || manifest.wf_run_id != input.claimed.run_id
        || manifest.workflow_type != input.claimed.wf_type
        || manifest.workflow_definition_version != input.claimed.wf_version
    {
        return Err("resolved CI manifest changed claimed workflow identity".into());
    }
    Ok(manifest)
}

/// Execute the validated manifest DAG through Flow's split dispatch/join surface.
///
/// Every ready node is dispatched before any join. Joins use the engine-required
/// `(deadline, idem_token)` order. Once a frontier contains a failure, all already-dispatched
/// siblings are drained before the workflow emits its terminal facts; descendants are never
/// dispatched.
pub fn run_ci_manifest_pipeline<R>(
    ctx: &mut WfCtx,
    manifest: &CiDriveManifestV1,
    runner: &R,
    finalizer: &dyn CiRunFinalizer,
) -> WfResult<CiManifestPipelineOutcome>
where
    R: JobRunner,
{
    manifest
        .validate()
        .map_err(|error| WfError::Nondeterministic(error.to_string()))?;
    let mut completed = BTreeSet::new();
    let mut flow_timed_out_jobs = BTreeSet::new();
    while completed.len() < manifest.jobs.len() {
        let frontier: Vec<&GrantedCiJobV1> = manifest
            .jobs
            .iter()
            .filter(|job| {
                !completed.contains(&job.job_id)
                    && job
                        .needs
                        .iter()
                        .all(|dependency| completed.contains(dependency))
            })
            .collect();
        if frontier.is_empty() {
            return Err(WfError::Nondeterministic(
                "validated CI manifest produced no runnable DAG frontier".into(),
            ));
        }

        let mut dispatched = Vec::with_capacity(frontier.len());
        for job in frontier {
            let handle = ctx.dispatch_job(
                JobSpec::new(JobKind::Ci, job.job_id.clone()),
                runner,
                Some(i64::from(job.limits.timeout_secs)),
            )?;
            dispatched.push((job, handle));
        }
        dispatched.sort_by(|left, right| join_key(&left.1).cmp(&join_key(&right.1)));

        let mut failed: Option<(String, bool)> = None;
        let mut late_accounting = Vec::new();
        for (job, handle) in dispatched {
            match ctx.join_dispatched_job(&handle)? {
                JobOutcome::Parked => return Ok(CiManifestPipelineOutcome::Parked),
                JobOutcome::TimedOut => {
                    failed.get_or_insert_with(|| (job.name.clone(), true));
                    flow_timed_out_jobs.insert(job.job_id.clone());
                    late_accounting.push((job.name.as_str(), handle.idem_token().to_owned()));
                    completed.insert(job.job_id.clone());
                }
                JobOutcome::Completed { result, .. } => {
                    let valid_pass = matches!(
                        read_stage_verdict(&result),
                        Some((ref concrete_name, true)) if concrete_name == &job.name
                    );
                    let exact_verdict = matches!(
                        read_stage_verdict(&result),
                        Some((ref concrete_name, _)) if concrete_name == &job.name
                    );
                    if !valid_pass {
                        failed.get_or_insert_with(|| (job.name.clone(), false));
                    }
                    if !exact_verdict {
                        failed.get_or_insert_with(|| (job.name.clone(), false));
                    }
                    completed.insert(job.job_id.clone());
                }
            }
        }

        // A Flow dispatch deadline is a terminal workflow verdict, not permission to abandon money
        // truth. The sandbox job is never interrupted by this timer. Park on the same exact signal
        // without a second deadline until the runner co-commits measured usage and `job.done`.
        for (job_name, idem_token) in late_accounting {
            match ctx.wait_for_signal_exact(JOB_DONE_SIGNAL, &idem_token, None)? {
                WaitOutcome::Parked => return Ok(CiManifestPipelineOutcome::Parked),
                WaitOutcome::Signalled { payload, .. } => {
                    if !matches!(
                        read_stage_verdict(&payload),
                        Some((ref concrete_name, _)) if concrete_name == job_name
                    ) {
                        return Err(WfError::Nondeterministic(
                            "late CI accounting signal changed the timed-out job identity".into(),
                        ));
                    }
                }
                WaitOutcome::TimedOut => {
                    return Err(WfError::Nondeterministic(
                        "an unbounded late-accounting wait unexpectedly timed out".into(),
                    ));
                }
            }
        }

        if let Some((job, timed_out)) = failed {
            let terminal_state = if timed_out {
                CiRunTerminalState::TimedOut
            } else {
                CiRunTerminalState::Failed
            };
            finalize_and_emit_terminal_facts(
                ctx,
                manifest,
                terminal_state,
                false,
                Some(&job),
                &flow_timed_out_jobs,
                finalizer,
            )?;
            return Ok(CiManifestPipelineOutcome::Failed { job, timed_out });
        }
    }

    finalize_and_emit_terminal_facts(
        ctx,
        manifest,
        CiRunTerminalState::Succeeded,
        true,
        None,
        &flow_timed_out_jobs,
        finalizer,
    )?;
    Ok(CiManifestPipelineOutcome::Succeeded {
        jobs_completed: completed.len(),
    })
}

fn join_key(job: &DispatchedJob) -> (i64, &str) {
    (
        job.deadline_unix_secs().unwrap_or(i64::MAX),
        job.idem_token(),
    )
}

fn finalize_and_emit_terminal_facts(
    ctx: &mut WfCtx,
    manifest: &CiDriveManifestV1,
    terminal_state: CiRunTerminalState,
    success: bool,
    failed_job: Option<&str>,
    flow_timed_out_jobs: &BTreeSet<String>,
    finalizer: &dyn CiRunFinalizer,
) -> WfResult<()> {
    let completed_at = ctx.now();
    let finalization = CiRunFinalization {
        tenant_id: manifest.tenant_id.clone(),
        region: manifest.region.clone(),
        run_id: manifest.ci_run_id.clone(),
        wf_run_id: manifest.wf_run_id.clone(),
        terminal_state,
        completed_at: completed_at.clone(),
        jobs: manifest
            .jobs
            .iter()
            .map(|job| CiRunFinalizationJob {
                job_id: job.job_id.clone(),
                reserve_handle: job.reserve_handle.clone(),
                flow_timed_out: flow_timed_out_jobs.contains(&job.job_id),
            })
            .collect(),
    };
    let finalization_marker = ArtifactRef(format!(
        "ci.run.finalized:{}:{}",
        terminal_state.as_str(),
        manifest.ci_run_id
    ));
    let expected_marker = finalization_marker.clone();
    let result = ctx.activity(RetryPolicy::default_policy(), move |_idem, _attempt| {
        finalizer
            .finalize(&finalization)
            .map_err(|error| ActivityError(error.to_string()))?;
        Ok(vec![finalization_marker.clone()])
    })?;
    if result != vec![expected_marker] {
        return Err(WfError::Nondeterministic(
            "CI run finalization journal changed terminal identity".into(),
        ));
    }
    emit_terminal_facts(
        ctx,
        manifest,
        &completed_at,
        terminal_state,
        success,
        failed_job,
    )
}

fn emit_terminal_facts(
    ctx: &mut WfCtx,
    manifest: &CiDriveManifestV1,
    completed_at: &str,
    terminal_state: CiRunTerminalState,
    success: bool,
    failed_job: Option<&str>,
) -> WfResult<()> {
    let state = if success {
        CheckState::Success
    } else {
        CheckState::Failure
    };
    for (context, attempt) in &manifest.check_attempts {
        let emit_context = CheckEmitContext {
            tenant: manifest.tenant_id.clone(),
            repo: manifest.repo_ref.clone(),
            commit_oid: manifest.commit_oid.clone(),
            run_ref: manifest.run_ref.clone(),
            run_attempt: *attempt,
            trust_tier: manifest_check_trust_tier(manifest.trust_tier),
            started_at: manifest.started_at.clone(),
            completed_at: Some(completed_at.to_owned()),
        };
        let payload = check_status_payload(
            &emit_context,
            CheckProvider::Ci,
            context,
            state,
            true,
            CostPosture::Settled,
            None,
        );
        ctx.emit(
            check_updated_draft(&manifest.repo_ref, &manifest.commit_oid, context, payload),
            None,
        )?;
    }

    let event_type = match terminal_state {
        CiRunTerminalState::Succeeded => myelin_ci_sandbox::events::CI_RUN_SUCCEEDED,
        CiRunTerminalState::Failed => myelin_ci_sandbox::events::CI_RUN_FAILED,
        CiRunTerminalState::TimedOut => myelin_ci_sandbox::events::CI_RUN_TIMED_OUT,
    };
    let mut payload = serde_json::json!({
        "run": manifest.run_ref,
        "commit_oid": manifest.commit_oid,
    });
    if let Some(job) = failed_job {
        payload["structured_failure"] = serde_json::json!({"failed_stage": job});
    }
    ctx.emit(run_event(manifest, event_type, payload), None)?;

    if let Some(waiter) = &manifest.merge_waiter {
        let current = manifest
            .check_attempts
            .keys()
            .map(|context| (context.clone(), success))
            .collect();
        let result = rollup_ci_result(
            &manifest.commit_oid,
            &current,
            &waiter.required_contexts,
            &waiter.idem_token,
        );
        ctx.emit(ci_result_draft(&manifest.repo_ref, &result), None)?;
    }
    Ok(())
}

fn manifest_check_trust_tier(tier: CiManifestTrustTierV1) -> TrustTier {
    match tier {
        CiManifestTrustTierV1::Trusted | CiManifestTrustTierV1::SelfHosted => TrustTier::Trusted,
        CiManifestTrustTierV1::UntrustedFork => TrustTier::UntrustedFork,
    }
}

fn run_event(
    manifest: &CiDriveManifestV1,
    event_type: &str,
    payload: serde_json::Value,
) -> EventDraft {
    EventDraft {
        type_: EventType(event_type.into()),
        subject: ArtifactRef(manifest.run_ref.clone()),
        aggregate: AggregateKey(format!("ci/run/{}", manifest.run_ref)),
        payload,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

#[cfg(test)]
#[path = "ci_manifest_pipeline_tests.rs"]
mod tests;

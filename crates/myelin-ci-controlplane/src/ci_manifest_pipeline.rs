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
    read_stage_verdict, DispatchedJob, JobKind, JobOutcome, JobRunner, JobSpec,
    PgClaimedDriveInput, PgFlowWorker, PgInputResolveError, PgResolvedDriveInput, PgWorkerError,
    PgWorkflowInputResolver, WfCtx, WfError, WfResult,
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
        move |input, ctx| drive_resolved_ci_manifest_pipeline(input, ctx, runner.as_ref()),
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
) -> Result<Vec<ArtifactRef>, String>
where
    R: JobRunner,
{
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
    run_ci_manifest_pipeline(ctx, &manifest, runner)
        .map_err(|error| format!("manifest CI workflow failed: {error:?}"))?;
    Ok(vec![ArtifactRef(manifest.run_ref)])
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
) -> WfResult<CiManifestPipelineOutcome>
where
    R: JobRunner,
{
    manifest
        .validate()
        .map_err(|error| WfError::Nondeterministic(error.to_string()))?;
    let mut completed = BTreeSet::new();
    while completed.len() < manifest.jobs.len() {
        let frontier: Vec<&GrantedCiJobV1> = manifest
            .jobs
            .iter()
            .filter(|job| {
                !completed.contains(&job.job_id)
                    && job.needs.iter().all(|dependency| completed.contains(dependency))
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
        for (job, handle) in dispatched {
            match ctx.join_dispatched_job(&handle)? {
                JobOutcome::Parked => return Ok(CiManifestPipelineOutcome::Parked),
                JobOutcome::TimedOut => {
                    failed.get_or_insert_with(|| (job.name.clone(), true));
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

        if let Some((job, timed_out)) = failed {
            emit_terminal_facts(ctx, manifest, false, Some(&job))?;
            return Ok(CiManifestPipelineOutcome::Failed { job, timed_out });
        }
    }

    emit_terminal_facts(ctx, manifest, true, None)?;
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

fn emit_terminal_facts(
    ctx: &mut WfCtx,
    manifest: &CiDriveManifestV1,
    success: bool,
    failed_job: Option<&str>,
) -> WfResult<()> {
    let completed_at = ctx.now();
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
            completed_at: Some(completed_at.clone()),
        };
        let payload = check_status_payload(
            &emit_context,
            CheckProvider::Ci,
            context,
            state,
            true,
            CostPosture::Unsettled,
            None,
        );
        ctx.emit(
            check_updated_draft(&manifest.repo_ref, &manifest.commit_oid, context, payload),
            None,
        )?;
    }

    let event_type = if success {
        myelin_ci_sandbox::events::CI_RUN_SUCCEEDED
    } else {
        myelin_ci_sandbox::events::CI_RUN_FAILED
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

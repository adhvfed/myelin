use myelin_events::check_seam::{check_updated_draft, ci_result_draft, rollup_ci_result, CiResult};
use myelin_events::{ArtifactRef, EventDraft, EventType};
use myelin_events::{DataRole, Visibility};
use myelin_flow::{CiPipelineSpec, JobRunner, PipelineOutcome, WfCtx, WfResult};

pub const CI_PIPELINE_WF_TYPE: &str = myelin_flow::CI_PIPELINE_WF_TYPE;

use myelin_ci_sandbox::events::{CI_DEPLOYMENT_REJECTED, CI_RUN_FAILED, CI_RUN_SUCCEEDED};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PipelineStage {
    pub engine: myelin_flow::CiStage,
    pub gate: bool,
}

impl PipelineStage {
    pub fn job(engine: myelin_flow::CiStage) -> PipelineStage {
        PipelineStage {
            engine,
            gate: false,
        }
    }

    pub fn gate(engine: myelin_flow::CiStage) -> PipelineStage {
        PipelineStage { engine, gate: true }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PipelineRun {
    pub stages: Vec<PipelineStage>,
    pub contexts: Vec<String>,
    pub facts: CheckFacts,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckFacts {
    pub repo: String,
    pub commit_oid: String,
    pub run_ref: String,
    pub run_attempt: u32,
    pub trust_tier: String,
    pub merge_idem_token: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunVerdict {
    Succeeded {
        stages_completed: usize,
    },
    Failed {
        stage: String,
    },
    Rejected {
        stage: String,
    },
    Parked,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct StructuredFailure {
    pub failed_stage: String,
    pub failed_step: Option<u32>,
    pub failed_test: Option<String>,
    pub log_excerpt_ref: Option<String>,
}

impl StructuredFailure {
    pub fn for_stage(stage: impl Into<String>) -> StructuredFailure {
        StructuredFailure {
            failed_stage: stage.into(),
            ..StructuredFailure::default()
        }
    }

    pub fn to_payload(&self) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "failed_stage".to_string(),
            serde_json::Value::String(self.failed_stage.clone()),
        );
        if let Some(step) = self.failed_step {
            obj.insert("failed_step".to_string(), serde_json::json!(step));
        }
        if let Some(test) = &self.failed_test {
            obj.insert(
                "failed_test".to_string(),
                serde_json::Value::String(test.clone()),
            );
        }
        if let Some(log_ref) = &self.log_excerpt_ref {
            obj.insert(
                "log_excerpt_ref".to_string(),
                serde_json::Value::String(log_ref.clone()),
            );
        }
        serde_json::Value::Object(obj)
    }
}

pub fn structured_failure(
    failed_stage: &str,
    failed_step: Option<u32>,
    failed_test: Option<&str>,
    log_excerpt_ref: Option<&str>,
) -> StructuredFailure {
    StructuredFailure {
        failed_stage: failed_stage.to_string(),
        failed_step,
        failed_test: failed_test.map(str::to_string),
        log_excerpt_ref: log_excerpt_ref.map(str::to_string),
    }
}

fn terminal_check_status(facts: &CheckFacts, context: &str, success: bool) -> serde_json::Value {
    let state = if success {
        crate::check_emitter::CheckState::Success
    } else {
        crate::check_emitter::CheckState::Failure
    };
    let emit_ctx = crate::check_emitter::CheckEmitContext {
        tenant: tenant_of(&facts.run_ref),
        repo: facts.repo.clone(),
        commit_oid: facts.commit_oid.clone(),
        run_ref: facts.run_ref.clone(),
        run_attempt: facts.run_attempt,
        trust_tier: crate::check_emitter::TrustTier::from_stamp(&facts.trust_tier),
        started_at: "1970-01-01T00:00:00Z".to_string(),
        completed_at: Some("1970-01-01T00:00:00Z".to_string()),
    };
    crate::check_emitter::check_status_payload(
        &emit_ctx,
        crate::check_emitter::CheckProvider::Ci,
        context,
        state,
        true,
        crate::check_emitter::CostPosture::Unsettled,
        None,
    )
}

fn tenant_of(run_ref: &str) -> String {
    run_ref
        .strip_prefix("myelin://")
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(run_ref)
        .to_string()
}

pub fn run_ci_pipeline_body<R>(
    ctx: &mut WfCtx,
    run: &PipelineRun,
    runner: &R,
) -> WfResult<RunVerdict>
where
    R: JobRunner,
{
    for stage in &run.stages {
        if !stage.gate {
            break;
        }
        match gate_stage(ctx, &stage.engine.name, stage.engine.timeout_secs)? {
            GateOutcome::Approved => {  }
            GateOutcome::Rejected => {
                emit_deployment_rejected(ctx, &run.facts, &stage.engine.name)?;
                return Ok(RunVerdict::Rejected {
                    stage: stage.engine.name.clone(),
                });
            }
            GateOutcome::Parked => return Ok(RunVerdict::Parked),
        }
    }

    let engine_spec = CiPipelineSpec::new(
        run.stages
            .iter()
            .filter(|s| !s.gate)
            .map(|s| s.engine.clone())
            .collect(),
    );
    let outcome = ctx.run_ci_pipeline(&engine_spec, runner)?;

    match outcome {
        PipelineOutcome::Succeeded { stages_completed } => {
            emit_terminal_checks(ctx, &run.facts, &run.contexts, true)?;
            emit_run_terminal(ctx, &run.facts, true, None)?;
            emit_ci_result(ctx, &run.facts, &run.contexts, true)?;
            Ok(RunVerdict::Succeeded { stages_completed })
        }
        PipelineOutcome::Failed { stage } => {
            emit_terminal_checks(ctx, &run.facts, &run.contexts, false)?;
            emit_run_terminal(ctx, &run.facts, false, Some(&stage))?;
            emit_ci_result(ctx, &run.facts, &run.contexts, false)?;
            Ok(RunVerdict::Failed { stage })
        }
        PipelineOutcome::TimedOut { stage } => {
            emit_terminal_checks(ctx, &run.facts, &run.contexts, false)?;
            emit_run_terminal(ctx, &run.facts, false, Some(&stage))?;
            emit_ci_result(ctx, &run.facts, &run.contexts, false)?;
            Ok(RunVerdict::Failed { stage })
        }
        PipelineOutcome::Parked => {
            Ok(RunVerdict::Parked)
        }
    }
}

fn gate_stage(ctx: &mut WfCtx, stage: &str, window_secs: Option<i64>) -> WfResult<GateOutcome> {
    let name = myelin_flow::approval_wait_name(stage);
    match ctx.wait_for_signal(&name, window_secs)? {
        myelin_flow::WaitOutcome::Signalled { payload, .. } => {
            let declined = payload
                .iter()
                .any(|r| r.0.contains(myelin_flow::DECLINE_MARKER));
            if declined {
                Ok(GateOutcome::Rejected)
            } else {
                Ok(GateOutcome::Approved)
            }
        }
        myelin_flow::WaitOutcome::TimedOut => Ok(GateOutcome::Rejected),
        myelin_flow::WaitOutcome::Parked => Ok(GateOutcome::Parked),
    }
}

fn emit_terminal_checks(
    ctx: &mut WfCtx,
    facts: &CheckFacts,
    contexts: &[String],
    success: bool,
) -> WfResult<()> {
    for context in contexts {
        let status = terminal_check_status(facts, context, success);
        let draft = check_updated_draft(&facts.repo, &facts.commit_oid, context, status);
        ctx.emit(draft, None)?;
    }
    Ok(())
}

fn emit_run_terminal(
    ctx: &mut WfCtx,
    facts: &CheckFacts,
    success: bool,
    failed_stage: Option<&str>,
) -> WfResult<()> {
    let type_ = if success {
        CI_RUN_SUCCEEDED
    } else {
        CI_RUN_FAILED
    };
    let mut payload = serde_json::json!({
        "run": facts.run_ref,
        "repo_ref": facts.repo,
        "commit_oid": facts.commit_oid,
    });
    if let Some(stage) = failed_stage {
        payload["structured_failure"] = StructuredFailure::for_stage(stage).to_payload();
    }
    let draft = run_aggregate_draft(type_, &facts.run_ref, payload);
    ctx.emit(draft, None)?;
    Ok(())
}

fn emit_deployment_rejected(ctx: &mut WfCtx, facts: &CheckFacts, stage: &str) -> WfResult<()> {
    let payload = serde_json::json!({
        "run": facts.run_ref,
        "commit_oid": facts.commit_oid,
        "stage": stage,
    });
    let draft = run_aggregate_draft(CI_DEPLOYMENT_REJECTED, &facts.run_ref, payload);
    ctx.emit(draft, None)?;
    Ok(())
}

fn emit_ci_result(
    ctx: &mut WfCtx,
    facts: &CheckFacts,
    contexts: &[String],
    success: bool,
) -> WfResult<()> {
    let current: std::collections::BTreeMap<String, bool> =
        contexts.iter().map(|c| (c.clone(), success)).collect();
    let required: Vec<String> = contexts.to_vec();
    let result: CiResult = rollup_ci_result(
        &facts.commit_oid,
        &current,
        &required,
        &facts.merge_idem_token,
    );
    let draft = ci_result_draft(&facts.repo, &result);
    ctx.emit(draft, None)?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GateOutcome {
    Approved,
    Rejected,
    Parked,
}

fn run_aggregate_draft(type_: &str, run_ref: &str, payload: serde_json::Value) -> EventDraft {
    let subject = ArtifactRef(run_ref.to_string());
    let aggregate = myelin_events::AggregateKey(format!("ci/run/{run_ref}"));
    EventDraft {
        type_: EventType(type_.to_string()),
        subject,
        aggregate,
        payload,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

#[cfg(test)]
#[path = "ci_pipeline_tests.rs"]
mod tests;

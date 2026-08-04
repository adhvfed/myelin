use myelin_events::check_seam::check_updated_draft;
use myelin_events::EventDraft;
use std::collections::BTreeMap;

pub const BUMP_CHECK_ATTEMPT_SQL: &str = "\
INSERT INTO check_attempt (tenant_id, region, repo_ref, commit_oid, context, next_attempt, current_run)
VALUES ($1, $2, $3, $4, $5, 2, $6)
ON CONFLICT (tenant_id, repo_ref, commit_oid, context)
DO UPDATE SET
  next_attempt = CASE
    WHEN check_attempt.current_run IS NOT DISTINCT FROM EXCLUDED.current_run
      THEN check_attempt.next_attempt
    ELSE check_attempt.next_attempt + 1
  END,
  current_run = EXCLUDED.current_run
RETURNING next_attempt - 1 AS run_attempt";

#[derive(Debug, Default, Clone)]
pub struct CheckAttemptCounter {
    issued: BTreeMap<(String, String), u32>,
}

impl CheckAttemptCounter {
    pub fn new() -> CheckAttemptCounter {
        CheckAttemptCounter::default()
    }

    pub fn bump(&mut self, commit_oid: &str, context: &str) -> u32 {
        let key = (commit_oid.to_string(), context.to_string());
        let slot = self.issued.entry(key).or_insert(0);
        *slot += 1;
        *slot
    }

    pub fn current(&self, commit_oid: &str, context: &str) -> u32 {
        self.issued
            .get(&(commit_oid.to_string(), context.to_string()))
            .copied()
            .unwrap_or(0)
    }

    pub fn is_stale(&self, commit_oid: &str, context: &str, incoming: u32) -> bool {
        incoming < self.current(commit_oid, context)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckProvider {
    Ci,
    External,
}

impl CheckProvider {
    pub fn token(self) -> &'static str {
        match self {
            CheckProvider::Ci => "ci",
            CheckProvider::External => "external",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckState {
    Queued,
    InProgress,
    Success,
    Failure,
    Error,
    Neutral,
    Cancelled,
}

impl CheckState {
    pub fn token(self) -> &'static str {
        match self {
            CheckState::Queued => "queued",
            CheckState::InProgress => "in_progress",
            CheckState::Success => "success",
            CheckState::Failure => "failure",
            CheckState::Error => "error",
            CheckState::Neutral => "neutral",
            CheckState::Cancelled => "cancelled",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            CheckState::Success
                | CheckState::Failure
                | CheckState::Error
                | CheckState::Neutral
                | CheckState::Cancelled
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustTier {
    Trusted,
    UntrustedFork,
}

impl TrustTier {
    pub fn token(self) -> &'static str {
        match self {
            TrustTier::Trusted => "trusted",
            TrustTier::UntrustedFork => "untrusted_fork",
        }
    }

    pub fn from_stamp(stamp: &str) -> TrustTier {
        match stamp {
            "trusted" => TrustTier::Trusted,
            _ => TrustTier::UntrustedFork,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CostPosture {
    Unsettled,
    Settled,
}

impl CostPosture {
    pub fn is_settled(self) -> bool {
        matches!(self, CostPosture::Settled)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckEmitContext {
    pub tenant: String,
    pub repo: String,
    pub commit_oid: String,
    pub run_ref: String,
    pub run_attempt: u32,
    pub trust_tier: TrustTier,
    pub started_at: String,
    pub completed_at: Option<String>,
}

pub fn summary_for(state: CheckState, context: &str) -> (String, BTreeMap<String, String>) {
    let template_key = match state {
        CheckState::Queued => "ci.check.queued",
        CheckState::InProgress => "ci.check.in_progress",
        CheckState::Success => "ci.check.success",
        CheckState::Failure => "ci.check.failure",
        CheckState::Error => "ci.check.error",
        CheckState::Neutral => "ci.check.neutral",
        CheckState::Cancelled => "ci.check.cancelled",
    };
    let mut args = BTreeMap::new();
    args.insert("context".to_string(), context.to_string());
    (template_key.to_string(), args)
}

pub fn details_ref(run_ref: &str, state: CheckState, fail_step: Option<u32>) -> String {
    match (state, fail_step) {
        (CheckState::Failure, Some(n)) | (CheckState::Error, Some(n)) => {
            format!("{run_ref}#step-{n}")
        }
        _ => run_ref.to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn check_status_payload(
    ctx: &CheckEmitContext,
    provider: CheckProvider,
    context: &str,
    state: CheckState,
    required: bool,
    cost: CostPosture,
    fail_step: Option<u32>,
) -> serde_json::Value {
    let (template_key, args) = summary_for(state, context);
    serde_json::json!({
        "tenant": ctx.tenant,
        "repo": ctx.repo,
        "commit_oid": ctx.commit_oid,
        "context": { "provider": provider.token(), "name": context },
        "state": state.token(),
        "required": required,
        "run": ctx.run_ref,
        "run_attempt": ctx.run_attempt,
        "trust_tier": ctx.trust_tier.token(),
        "details_ref": details_ref(&ctx.run_ref, state, fail_step),
        "summary": { "template_key": template_key, "args": args },
        "started_at": ctx.started_at,
        "completed_at": ctx.completed_at,
        "cost_settled": cost.is_settled(),
    })
}

#[allow(clippy::too_many_arguments)]
pub fn assemble_check_status(
    ctx: &CheckEmitContext,
    provider: CheckProvider,
    context: &str,
    state: CheckState,
    required: bool,
    cost: CostPosture,
    fail_step: Option<u32>,
) -> EventDraft {
    let payload = check_status_payload(ctx, provider, context, state, required, cost, fail_step);
    check_updated_draft(&ctx.repo, &ctx.commit_oid, context, payload)
}

#[cfg(test)]
#[path = "check_emitter_tests.rs"]
mod tests;

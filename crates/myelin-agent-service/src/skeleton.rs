use crate::effect_api::validate_call;
use crate::metering::{price, LUNA_RATES};
use crate::tool_exec::{ToolExecError, ToolExecutionContext, ToolExecutor};
use myelin_agent::{
    Agent, AgentRuntime, Conversation, InboxEvent, MeteredRuntime, MeteredStep, RunOutcome,
    RuntimeStepError, StepOutcome, Submission, TokenUsage, ToolOutcome, ToolSurface, Turn,
};
use myelin_content::{Block, Inline, Span};
use myelin_events::{
    Actor, AggregateKey, DataRole, EmitContextBase, EventDraft, EventType, Timestamp, Visibility,
};
use myelin_flow::{
    DelegationCaveats, RetryPolicy, RunTokenError, RunTokenHandle, RunTokenMinter, WfCtx, WfJournal,
};
use myelin_identity::{Principal, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_storage::agent_run_gate::{AgentRunGate, DispatchError};
use myelin_storage::agent_wallet::{AgentWallet, DebitOutcome, MicroUsd, WalletError};
use myelin_storage::reserve_settle::{CostLedger, RunId as StorageRunId};
use myelin_storage::{AgentTraceWrite, AgentTraceWriter};
use myelin_tenancy::{Region, TenantId};

pub const AGENT_RUN_TRACED_EVENT: &str = "agent.run.traced";

pub const SKELETON_STEP_UNIT: &str = "skeleton.step";

pub const DEFAULT_MAX_TURNS: usize = 16;

pub const WALLET_MIN_BALANCE_FLOOR: MicroUsd = MicroUsd::ZERO;

pub fn requesting_subject(agent: &Principal) -> &str {
    match &agent.kind {
        PrincipalKind::Agent {
            on_behalf_of: Some(principal),
            ..
        } => &principal.0,
        _ => &agent.principal_id.0,
    }
}

pub trait RunWallet {
    fn balance(&self, tenant: &TenantId) -> MicroUsd;
    fn debit_once(
        &self,
        tenant: &TenantId,
        amount: MicroUsd,
        run_id: &str,
        charge_key: &str,
    ) -> Result<DebitOutcome, WalletError>;
}

impl RunWallet for AgentWallet {
    fn balance(&self, tenant: &TenantId) -> MicroUsd {
        AgentWallet::balance(self, tenant)
    }

    fn debit_once(
        &self,
        tenant: &TenantId,
        amount: MicroUsd,
        run_id: &str,
        charge_key: &str,
    ) -> Result<DebitOutcome, WalletError> {
        AgentWallet::debit_once(self, tenant, amount, run_id, charge_key)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpendCapStage {
    PreStepGate,
    PostDebit,
}

impl core::fmt::Display for SpendCapStage {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SpendCapStage::PreStepGate => write!(f, "pre-step balance gate"),
            SpendCapStage::PostDebit => write!(f, "post-debit insufficient balance"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SkeletonAgentRuntime;

impl SkeletonAgentRuntime {
    pub fn new() -> SkeletonAgentRuntime {
        SkeletonAgentRuntime
    }
}

impl AgentRuntime for SkeletonAgentRuntime {
    fn step(&self, _conv: &Conversation) -> StepOutcome {
        StepOutcome::Submit(Submission(
            "skeleton: no model, no tools - immediate submit".into(),
        ))
    }
}

impl MeteredRuntime for SkeletonAgentRuntime {}

pub trait RunTokenRevoker {
    fn revoke(&self, jti: &str, now_secs: i64, teardown_secs: i64) -> Result<u64, String>;

    fn is_dead(&self, jti: &str, now_secs: i64) -> bool;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildEnv {
    pub run_token_jti: String,
    pub shared_platform_token: Option<String>,
}

impl ChildEnv {
    pub fn for_run(run_token_jti: impl Into<String>) -> ChildEnv {
        ChildEnv {
            run_token_jti: run_token_jti.into(),
            shared_platform_token: None,
        }
    }

    pub fn leaked_shared_token(&self) -> bool {
        self.shared_platform_token.is_some()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkeletonTelemetry {
    reserved: u64,
    settled: u64,
    traces_written: u64,
    max_revocation_lag: u64,
    tokens_revoked: u64,
    runs_completed: u64,
    runs_killed: u64,
    tokens_input: u64,
    tokens_cached_input: u64,
    tokens_output: u64,
    turns_usage_not_reported: u64,
    charged_micro: u64,
}

impl SkeletonTelemetry {
    pub fn new() -> SkeletonTelemetry {
        SkeletonTelemetry::default()
    }
    pub fn reserved(&self) -> u64 {
        self.reserved
    }
    pub fn settled(&self) -> u64 {
        self.settled
    }
    pub fn ledger_balanced(&self) -> bool {
        self.reserved == self.settled
    }
    pub fn traces_written(&self) -> u64 {
        self.traces_written
    }
    pub fn max_revocation_lag(&self) -> u64 {
        self.max_revocation_lag
    }
    pub fn tokens_revoked(&self) -> u64 {
        self.tokens_revoked
    }
    pub fn runs_completed(&self) -> u64 {
        self.runs_completed
    }
    pub fn runs_killed(&self) -> u64 {
        self.runs_killed
    }
    pub fn tokens_input(&self) -> u64 {
        self.tokens_input
    }
    pub fn tokens_cached_input(&self) -> u64 {
        self.tokens_cached_input
    }
    pub fn tokens_output(&self) -> u64 {
        self.tokens_output
    }
    pub fn turns_usage_not_reported(&self) -> u64 {
        self.turns_usage_not_reported
    }
    pub fn charged_micro(&self) -> u64 {
        self.charged_micro
    }

    fn record_charge(&mut self, amount: MicroUsd) {
        self.charged_micro = self.charged_micro.saturating_add(amount.0);
    }

    fn record_token_usage(&mut self, usage: &TokenUsage) {
        match usage {
            TokenUsage::Reported {
                input,
                cached_input,
                output,
            } => {
                self.tokens_input = self.tokens_input.saturating_add(*input);
                self.tokens_cached_input = self.tokens_cached_input.saturating_add(*cached_input);
                self.tokens_output = self.tokens_output.saturating_add(*output);
            }
            TokenUsage::NotReported => {
                self.turns_usage_not_reported = self.turns_usage_not_reported.saturating_add(1);
            }
        }
    }

    fn record_reserve(&mut self, amount: u64) {
        self.reserved = self.reserved.saturating_add(amount);
    }
    fn record_settle(&mut self, amount: u64) {
        self.settled = self.settled.saturating_add(amount);
    }
    fn record_trace(&mut self) {
        self.traces_written = self.traces_written.saturating_add(1);
    }
    fn record_revoke(&mut self, lag: u64) {
        self.tokens_revoked = self.tokens_revoked.saturating_add(1);
        if lag > self.max_revocation_lag {
            self.max_revocation_lag = lag;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunOutcomeKind {
    Completed,
    KilledMidFlight,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkeletonError {
    DispatchRefused(String),
    MintFailed(String),
    CoCommit(String),
    ToolValidationRejected(String),
    ToolExecFailed(String),
    ApprovalRequired {
        gate_id: String,
    },
    MaxTurnsExhausted {
        run_id: String,
        turns: usize,
    },
    WalletSpendCapReached {
        run_id: String,
        stage: SpendCapStage,
    },
    RuntimeStepFailed {
        run_id: String,
        error: RuntimeStepError,
    },
    MeteringUsageNotReported {
        run_id: String,
    },
    MeteringOverflow {
        run_id: String,
        reason: String,
    },
    CostSettlementFailed {
        run_id: String,
        reason: String,
    },
    TokenRevocationFailed {
        run_id: String,
        reason: String,
    },
}

impl SkeletonError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::DispatchRefused(_) => "dispatch_refused",
            Self::MintFailed(_) => "run_token_mint_failed",
            Self::CoCommit(_) => "co_commit_failed",
            Self::ToolValidationRejected(_) => "tool_validation_rejected",
            Self::ToolExecFailed(_) => "tool_execution_failed",
            Self::ApprovalRequired { .. } => "approval_required",
            Self::MaxTurnsExhausted { .. } => "max_turns_exhausted",
            Self::WalletSpendCapReached { .. } => "wallet_spend_cap_reached",
            Self::RuntimeStepFailed { error, .. } => error.code(),
            Self::MeteringUsageNotReported { .. } => "metering_usage_not_reported",
            Self::MeteringOverflow { .. } => "metering_overflow",
            Self::CostSettlementFailed { .. } => "cost_settlement_failed",
            Self::TokenRevocationFailed { .. } => "run_token_revocation_failed",
        }
    }
}

impl core::fmt::Display for SkeletonError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SkeletonError::DispatchRefused(m) => write!(f, "SKELETON dispatch refused: {m}"),
            SkeletonError::MintFailed(m) => write!(f, "SKELETON mint failed: {m}"),
            SkeletonError::CoCommit(m) => write!(f, "SKELETON co-commit failed: {m}"),
            SkeletonError::ToolValidationRejected(m) => {
                write!(f, "SKELETON tool-call validation rejected (fail-closed): {m}")
            }
            SkeletonError::ToolExecFailed(m) => write!(f, "SKELETON tool execution failed: {m}"),
            SkeletonError::ApprovalRequired { gate_id } => write!(
                f,
                "SKELETON run is waiting for human approval at gate `{gate_id}`"
            ),
            SkeletonError::MaxTurnsExhausted { run_id, turns } => write!(
                f,
                "SKELETON bounded loop exhausted: run={run_id} reached max_turns={turns} without a submit"
            ),
            SkeletonError::WalletSpendCapReached { run_id, stage } => write!(
                f,
                "SKELETON metering spend cap reached: run={run_id} halted gracefully at the {stage} \
                 (no overspend, reservation left in-flight)"
            ),
            SkeletonError::RuntimeStepFailed { run_id, error } => write!(
                f,
                "SKELETON runtime step failed: run={run_id} code={}: {error} (fail-closed)",
                error.code(),
            ),
            SkeletonError::MeteringUsageNotReported { run_id } => write!(
                f,
                "SKELETON metering failed closed: run={run_id} had a paid turn with NO reported token \
                 usage - billing never guesses (fail-closed)"
            ),
            SkeletonError::MeteringOverflow { run_id, reason } => write!(
                f,
                "SKELETON metering arithmetic overflowed: run={run_id}: {reason} (loud, never a wrap)"
            ),
            SkeletonError::CostSettlementFailed { run_id, reason } => write!(
                f,
                "SKELETON cost settlement failed: run={run_id}: {reason} (the durable reservation remains retryable)"
            ),
            SkeletonError::TokenRevocationFailed { run_id, reason } => write!(
                f,
                "SKELETON run-token teardown failed: run={run_id}: {reason} (the token remains bounded by its fail-static lifetime)"
            ),
        }
    }
}

impl std::error::Error for SkeletonError {}

impl From<DispatchError> for SkeletonError {
    fn from(e: DispatchError) -> SkeletonError {
        SkeletonError::DispatchRefused(e.to_string())
    }
}

impl From<RunTokenError> for SkeletonError {
    fn from(e: RunTokenError) -> SkeletonError {
        SkeletonError::MintFailed(e.to_string())
    }
}

pub struct RunSubstrate<'a> {
    pub tenant: TenantId,
    pub region: Region,
    pub agent: Principal,
    pub run_id: String,
    pub minter_token: std::sync::Arc<dyn RunTokenMinter + Send + Sync>,
    pub agent_id: String,
    pub caveats: DelegationCaveats,
    pub token_ttl_secs: u64,
    pub revoker: &'a dyn RunTokenRevoker,
    pub catalogue: &'a dyn ToolSurface,
    pub executor: &'a dyn ToolExecutor,
    pub wallet: Option<&'a dyn RunWallet>,
    pub gate: &'a mut AgentRunGate,
    pub ledger: &'a mut CostLedger,
    pub available: MicroUsd,
    pub estimate: MicroUsd,
    pub outbox: &'a myelin_events::OutboxStore,
    pub minter: std::sync::Arc<dyn myelin_events::IdMinter>,
    pub journal: WfJournal,
    pub trace_writer: std::sync::Arc<dyn AgentTraceWriter>,
    pub now_secs: i64,
}

pub struct SkeletonAgent;

impl SkeletonAgent {
    pub fn new() -> SkeletonAgent {
        SkeletonAgent
    }

    pub fn handle_run(
        &self,
        runtime: &dyn MeteredRuntime,
        sub: &mut RunSubstrate<'_>,
        telemetry: &mut SkeletonTelemetry,
        kill: RunOutcomeKind,
    ) -> Result<RunOutcome, SkeletonError> {
        let mut caveats = sub.caveats.clone();
        caveats.0.push(format!("run:{}", sub.run_id));
        let token: RunTokenHandle = sub
            .minter_token
            .mint_run_token(&sub.agent_id, &sub.run_id, &caveats, sub.token_ttl_secs)
            .map_err(SkeletonError::from)?;
        let teardown_at = sub.now_secs;
        let result = self.handle_minted_run(runtime, sub, telemetry, kill, &token);
        match sub.revoker.revoke(&token.jti, sub.now_secs, teardown_at) {
            Ok(lag) => {
                telemetry.record_revoke(lag);
                if result.is_ok() {
                    match kill {
                        RunOutcomeKind::Completed => {
                            telemetry.runs_completed = telemetry.runs_completed.saturating_add(1)
                        }
                        RunOutcomeKind::KilledMidFlight => {
                            telemetry.runs_killed = telemetry.runs_killed.saturating_add(1)
                        }
                    }
                }
            }
            Err(reason) => {
                return Err(SkeletonError::TokenRevocationFailed {
                    run_id: sub.run_id.clone(),
                    reason,
                })
            }
        }
        result
    }

    fn handle_minted_run(
        &self,
        runtime: &dyn MeteredRuntime,
        sub: &mut RunSubstrate<'_>,
        telemetry: &mut SkeletonTelemetry,
        kill: RunOutcomeKind,
        token: &RunTokenHandle,
    ) -> Result<RunOutcome, SkeletonError> {
        let storage_run = StorageRunId::new(sub.run_id.clone());
        let in_flight = sub
            .gate
            .dispatch_or_resume_workflow(
                sub.ledger,
                sub.tenant.clone(),
                storage_run.clone(),
                sub.estimate,
                sub.available,
            )
            .map_err(SkeletonError::from)?;
        telemetry.record_reserve(in_flight.reserved().0);

        let _child_env = ChildEnv::for_run(&token.jti);

        let ctx_base = EmitContextBase {
            tenant: sub.tenant.clone(),
            region: sub.region.clone(),
            actor: Actor(sub.agent.clone()),
            schema_ver: 1,
            occurred_at: Timestamp(format!("skeleton-now:{}", sub.now_secs)),
            recorded_at: Timestamp(format!("skeleton-now:{}", sub.now_secs)),
            caused_by: None,
        };
        let mut ctx = WfCtx::begin(
            sub.outbox,
            sub.minter.clone(),
            sub.journal.clone(),
            ctx_base,
            sub.run_id.clone(),
            "agent.run",
            format!("skeleton-now:{}", sub.now_secs),
            0,
        );

        let mut conv = Conversation::default();

        let mut submission: Option<Submission> = None;
        for turn in 0..DEFAULT_MAX_TURNS {
            if let Some(wallet) = sub.wallet {
                if wallet.balance(&sub.tenant) <= WALLET_MIN_BALANCE_FLOOR {
                    return Err(SkeletonError::WalletSpendCapReached {
                        run_id: sub.run_id.clone(),
                        stage: SpendCapStage::PreStepGate,
                    });
                }
            }

            let MeteredStep { outcome, usage } =
                runtime
                    .step_metered(&conv)
                    .map_err(|error| SkeletonError::RuntimeStepFailed {
                        run_id: sub.run_id.clone(),
                        error,
                    })?;
            telemetry.record_token_usage(&usage);

            if let Some(wallet) = sub.wallet {
                self.meter_turn(
                    wallet,
                    &sub.tenant,
                    &usage,
                    &sub.run_id,
                    &format!("{}/model-turn/{turn}", sub.run_id),
                    telemetry,
                )?;
            }

            match outcome {
                StepOutcome::Submit(s) => {
                    conv.turns.push(Turn::Model(StepOutcome::Submit(s.clone())));
                    submission = Some(s);
                    break;
                }
                StepOutcome::UseTools(calls) => {
                    conv.turns
                        .push(Turn::Model(StepOutcome::UseTools(calls.clone())));
                    let mut outcomes: Vec<ToolOutcome> = Vec::with_capacity(calls.len());
                    for (call_index, call) in calls.iter().enumerate() {
                        if let Err(reason) = validate_call(sub.catalogue, call) {
                            return Err(SkeletonError::ToolValidationRejected(reason));
                        }
                        let def = match sub.catalogue.resolve(&call.name) {
                            Some(def) => def,
                            None => {
                                return Err(SkeletonError::ToolValidationRejected(format!(
                                    "tool `{}` vanished from the catalogue after validation",
                                    call.name.0
                                )))
                            }
                        };
                        let effect_key =
                            crate::tool_exec::logical_tool_effect_key(turn, call_index);
                        let tool_context = ToolExecutionContext {
                            run_id: &sub.run_id,
                            run_token: token,
                            effect_key: &effect_key,
                        };
                        match sub.executor.execute(&tool_context, def, call) {
                            Ok(result) => outcomes.push(ToolOutcome {
                                call_id: call.id.clone(),
                                result,
                            }),
                            Err(ToolExecError::ApprovalRequired { gate_id }) => {
                                return Err(SkeletonError::ApprovalRequired { gate_id });
                            }
                            Err(ToolExecError::Failed(reason)) => {
                                return Err(SkeletonError::ToolExecFailed(reason));
                            }
                        }
                    }
                    conv.turns.push(Turn::ToolResults(outcomes));
                }
            }
        }

        let submission = match submission {
            Some(s) => s,
            None => {
                return Err(SkeletonError::MaxTurnsExhausted {
                    run_id: sub.run_id.clone(),
                    turns: DEFAULT_MAX_TURNS,
                });
            }
        };

        if kill == RunOutcomeKind::KilledMidFlight {
            return Ok(RunOutcome(format!(
                "killed-mid-flight: run={} token-revoked (no trace, reservation left in-flight)",
                sub.run_id
            )));
        }

        let trace = completed_trace(sub, &submission, telemetry);
        let trace_artifact = sub
            .trace_writer
            .write(&sub.tenant, trace)
            .map_err(|error| SkeletonError::CoCommit(error.to_string()))?
            .artifact_ref;
        let trace_ref = trace_artifact.0.clone();
        ctx.activity(RetryPolicy::default_policy(), {
            let tr = trace_artifact.clone();
            move |_id: &str, _attempt: u32| Ok(vec![tr.clone()])
        })
        .map_err(|e| SkeletonError::CoCommit(format!("{e:?}")))?;

        let draft = EventDraft {
            type_: EventType(AGENT_RUN_TRACED_EVENT.into()),
            subject: ArtifactRef(trace_ref.clone()),
            aggregate: AggregateKey(format!("run:{}", sub.run_id)),
            payload: serde_json::json!({ "trace_ref": trace_ref }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        };
        ctx.emit(draft, None)
            .map_err(|e| SkeletonError::CoCommit(format!("{e:?}")))?;

        ctx.commit()
            .map_err(|e| SkeletonError::CoCommit(format!("{e:?}")))?;
        telemetry.record_trace();

        let settle = in_flight.settle(sub.ledger, &[]).map_err(|error| {
            SkeletonError::CostSettlementFailed {
                run_id: sub.run_id.clone(),
                reason: error.to_string(),
            }
        })?;
        let settled_total = settle.billed_total.0.saturating_add(settle.refunded.0);
        telemetry.record_settle(settled_total);

        Ok(RunOutcome(format!(
            "completed: run={} trace={} reserved={} settled={} token-revoked",
            sub.run_id,
            trace_ref,
            in_flight.reserved().0,
            settled_total
        )))
    }

    fn meter_turn(
        &self,
        wallet: &dyn RunWallet,
        tenant: &TenantId,
        usage: &TokenUsage,
        run_id: &str,
        charge_key: &str,
        telemetry: &mut SkeletonTelemetry,
    ) -> Result<(), SkeletonError> {
        let reported = match usage {
            TokenUsage::NotReported => {
                return Err(SkeletonError::MeteringUsageNotReported {
                    run_id: run_id.to_string(),
                })
            }
            reported => reported,
        };
        let priced = price(reported, &LUNA_RATES).map_err(|e| SkeletonError::MeteringOverflow {
            run_id: run_id.to_string(),
            reason: e.to_string(),
        })?;
        let charge = priced
            .total()
            .ok_or_else(|| SkeletonError::MeteringOverflow {
                run_id: run_id.to_string(),
                reason: "priced wholesale + markup overflowed u64".into(),
            })?;
        match wallet.debit_once(tenant, charge, run_id, charge_key) {
            Ok(DebitOutcome::Applied(_new_balance) | DebitOutcome::Replayed(_new_balance)) => {
                telemetry.record_charge(charge);
                Ok(())
            }
            Err(WalletError::InsufficientBalance { .. }) => {
                Err(SkeletonError::WalletSpendCapReached {
                    run_id: run_id.to_string(),
                    stage: SpendCapStage::PostDebit,
                })
            }
            Err(other) => Err(SkeletonError::MeteringOverflow {
                run_id: run_id.to_string(),
                reason: other.to_string(),
            }),
        }
    }
}

fn completed_trace(
    sub: &RunSubstrate<'_>,
    submission: &Submission,
    telemetry: &SkeletonTelemetry,
) -> AgentTraceWrite {
    let requested_by = requesting_subject(&sub.agent).to_string();
    let blocks = vec![Block::Paragraph {
        inline: Inline {
            spans: vec![Span::Text {
                text: submission.0.clone(),
                marks: vec![],
                link: None,
            }],
            nodes: vec![],
        },
    }];
    let trace_body = serde_json::json!({
        "schema": "myelin.agent_trace.v1",
        "run_id": sub.run_id,
        "actor": sub.agent.principal_id.0,
        "requested_by": requested_by,
        "answer": submission.0,
        "charged_micro": telemetry.charged_micro(),
        "blocks": blocks,
    });
    AgentTraceWrite {
        run_id: sub.run_id.clone(),
        agent_principal: sub.agent.principal_id.0.clone(),
        requested_by,
        answer: submission.0.clone(),
        trace_body,
        charged_micro: telemetry.charged_micro(),
    }
}

impl Default for SkeletonAgent {
    fn default() -> Self {
        SkeletonAgent::new()
    }
}

impl Agent for SkeletonAgent {
    fn handle(&self, inbox: InboxEvent, runtime: &dyn AgentRuntime) -> RunOutcome {
        let _ = runtime.step(&Conversation::default());
        RunOutcome(format!(
            "skeleton handle: delivered={} (chained path → handle_run)",
            inbox.0
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_exec::{MockToolExecutor, MockToolSurface, ToolExecError};
    use myelin_agent::{EffectKind, ToolCall, ToolCallId, ToolDef, ToolName, ToolResult};
    use myelin_identity::{PrincipalId, PrincipalKind};
    use std::sync::Arc;

    #[derive(Default)]
    struct FakeMinter;
    impl RunTokenMinter for FakeMinter {
        fn mint_run_token(
            &self,
            agent_id: &str,
            run_id: &str,
            caveats: &myelin_flow::DelegationCaveats,
            ttl_secs: u64,
        ) -> Result<RunTokenHandle, RunTokenError> {
            assert!(
                caveats.0.iter().any(|c| c == &format!("run:{run_id}")),
                "the mint must carry the per-run attenuation caveat"
            );
            Ok(RunTokenHandle {
                token: format!("tok:{agent_id}:{run_id}"),
                jti: format!("jti:{agent_id}:{run_id}"),
                ttl_secs,
            })
        }
    }

    #[derive(Default)]
    struct FakeRevoker {
        revoked: std::sync::Mutex<std::collections::HashMap<String, i64>>,
        ttl_w: i64,
        minted_at: i64,
    }
    impl RunTokenRevoker for FakeRevoker {
        fn revoke(&self, jti: &str, now_secs: i64, teardown_secs: i64) -> Result<u64, String> {
            let mut g = self.revoked.lock().unwrap();
            if g.contains_key(jti) {
                return Ok(0);
            }
            g.insert(jti.to_string(), now_secs);
            Ok(now_secs.saturating_sub(teardown_secs).max(0) as u64)
        }
        fn is_dead(&self, jti: &str, now_secs: i64) -> bool {
            self.revoked.lock().unwrap().contains_key(jti)
                || now_secs >= self.minted_at + self.ttl_w
        }
    }

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn agent() -> Principal {
        Principal::stub(
            PrincipalId("psn:agent-7".into()),
            PrincipalKind::Agent {
                runtime_ref: myelin_identity::RuntimeRef("skeleton".into()),
                on_behalf_of: None,
            },
            tenant(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn substrate<'a>(
        run_id: &str,
        revoker: &'a dyn RunTokenRevoker,
        catalogue: &'a dyn ToolSurface,
        executor: &'a dyn ToolExecutor,
        gate: &'a mut AgentRunGate,
        ledger: &'a mut CostLedger,
        outbox: &'a myelin_events::OutboxStore,
        available: u64,
        estimate: u64,
        now_secs: i64,
    ) -> RunSubstrate<'a> {
        RunSubstrate {
            tenant: tenant(),
            region: region(),
            agent: agent(),
            run_id: run_id.into(),
            minter_token: Arc::new(FakeMinter),
            agent_id: "psn:agent-7".into(),
            caveats: DelegationCaveats(vec!["delegated:human-x".into()]),
            token_ttl_secs: 300,
            revoker,
            catalogue,
            executor,
            wallet: None,
            gate,
            ledger,
            available: MicroUsd(available),
            estimate: MicroUsd(estimate),
            outbox,
            minter: Arc::new(myelin_events::MonotonicMinter::new()),
            journal: WfJournal::new(),
            trace_writer: Arc::new(myelin_storage::InMemoryAgentTraceStore::new()),
            now_secs,
        }
    }

    #[test]
    fn skeleton_runtime_submits_immediately() {
        let rt = SkeletonAgentRuntime::new();
        assert!(matches!(
            rt.step(&Conversation::default()),
            StepOutcome::Submit(_)
        ));
    }

    #[test]
    fn skeleton_chains_the_whole_substrate_path() {
        let rt = SkeletonAgentRuntime::new();
        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker {
            ttl_w: 300,
            minted_at: 1000,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        let cat = MockToolSurface::new();
        let exec = MockToolExecutor::new();
        let mut sub = substrate(
            "R1",
            &revoker,
            &cat,
            &exec,
            &mut gate,
            &mut ledger,
            &outbox,
            100,
            10,
            1000,
        );

        let out = agent_loop
            .handle_run(&rt, &mut sub, &mut tele, RunOutcomeKind::Completed)
            .expect("the SKELETON chain completes");
        assert!(
            out.0.contains("completed"),
            "the run completed the chain: {out:?}"
        );

        assert_eq!(tele.traces_written(), 1, "exactly one trace row written");
        assert!(
            tele.ledger_balanced(),
            "reserved {} == settled {}",
            tele.reserved(),
            tele.settled()
        );
        assert_eq!(tele.reserved(), 10, "reserved the estimate");
        assert_eq!(
            tele.settled(),
            10,
            "settled (billed 0 + refunded 10) == reserved"
        );
        assert_eq!(
            tele.tokens_revoked(),
            1,
            "the per-run token revoked on teardown"
        );
        assert_eq!(tele.runs_completed(), 1);
        assert_eq!(tele.runs_killed(), 0);
        let mut caveats = sub.caveats.clone();
        caveats.0.push("run:R1".into());
        let token = sub
            .minter_token
            .mint_run_token(&sub.agent_id, "R1", &caveats, sub.token_ttl_secs)
            .unwrap();
        assert_eq!(
            token.jti, "jti:psn:agent-7:R1",
            "the token jti is bound to (agent, run)"
        );
        assert_eq!(
            sub.agent_id, "psn:agent-7",
            "the run's agent principal == the token's principal"
        );
    }

    #[test]
    fn settlement_outage_returns_a_retryable_error_without_crashing_the_worker() {
        let runtime = SkeletonAgentRuntime::new();
        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker {
            ttl_w: 300,
            minted_at: 1000,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        ledger.fail_next_settlement_for_test();
        let outbox = myelin_events::OutboxStore::new();
        let mut telemetry = SkeletonTelemetry::new();
        let catalogue = MockToolSurface::new();
        let executor = MockToolExecutor::new();
        let mut substrate = substrate(
            "Rsettlement-outage",
            &revoker,
            &catalogue,
            &executor,
            &mut gate,
            &mut ledger,
            &outbox,
            100,
            10,
            1000,
        );

        let error = agent_loop
            .handle_run(
                &runtime,
                &mut substrate,
                &mut telemetry,
                RunOutcomeKind::Completed,
            )
            .expect_err("the settlement outage is a controlled run failure");

        assert!(matches!(
            error,
            SkeletonError::CostSettlementFailed { ref run_id, .. }
                if run_id == "Rsettlement-outage"
        ));
        assert_eq!(
            telemetry.traces_written(),
            1,
            "the durable answer was committed"
        );
        assert_eq!(
            telemetry.settled(),
            0,
            "the failed settlement was not invented"
        );
        assert_eq!(
            telemetry.runs_completed(),
            0,
            "the run was not called complete"
        );
        assert_eq!(
            telemetry.tokens_revoked(),
            1,
            "the run credential was torn down"
        );
        assert_eq!(
            substrate
                .ledger
                .state_of(&tenant(), &StorageRunId::new("Rsettlement-outage"))
                .expect("the test ledger remains readable"),
            Some(myelin_storage::ReservationState::InFlight),
            "the durable reservation remains available to the workflow retry"
        );
    }

    #[test]
    fn ag_d8_killed_run_revokes_token_and_leaks_nothing() {
        let rt = SkeletonAgentRuntime::new();
        let agent_loop = SkeletonAgent::new();
        let w = 300i64;
        let minted_at = 1000i64;
        let revoker = FakeRevoker {
            ttl_w: w,
            minted_at,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        let cat = MockToolSurface::new();
        let exec = MockToolExecutor::new();
        let mut sub = substrate(
            "R2",
            &revoker,
            &cat,
            &exec,
            &mut gate,
            &mut ledger,
            &outbox,
            100,
            10,
            minted_at,
        );

        let out = agent_loop
            .handle_run(&rt, &mut sub, &mut tele, RunOutcomeKind::KilledMidFlight)
            .expect("a killed run still tears down cleanly");
        assert!(
            out.0.contains("killed-mid-flight"),
            "the run was killed: {out:?}"
        );

        assert_eq!(
            tele.tokens_revoked(),
            1,
            "killed run STILL revoked its token on teardown"
        );
        assert_eq!(tele.runs_killed(), 1);
        assert_eq!(
            tele.traces_written(),
            0,
            "a killed run wrote no trace (0 ghost - co-commit abandoned)"
        );

        let jti = "jti:psn:agent-7:R2";
        assert!(
            revoker.is_dead(jti, minted_at),
            "revoked-on-teardown → dead now"
        );
        let fresh = FakeRevoker {
            ttl_w: w,
            minted_at,
            ..Default::default()
        };
        assert!(!fresh.is_dead(jti, minted_at), "not yet expired before W");
        assert!(
            fresh.is_dead(jti, minted_at + w),
            "auto-expires by minted_at + W (≤ W window)"
        );

        let child = ChildEnv::for_run(jti);
        assert!(
            !child.leaked_shared_token(),
            "0 shared platform token leaked into the child env"
        );
        assert_eq!(
            child.shared_platform_token, None,
            "the child env's shared-token slot is UNSET"
        );
        assert_eq!(
            child.run_token_jti, jti,
            "the child's ONLY credential is the per-run jti"
        );
        assert!(
            tele.max_revocation_lag() <= w as u64,
            "revocation lag within bound W"
        );
    }

    #[test]
    fn no_balance_no_run_but_token_still_torn_down() {
        let rt = SkeletonAgentRuntime::new();
        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker {
            ttl_w: 300,
            minted_at: 1000,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        let cat = MockToolSurface::new();
        let exec = MockToolExecutor::new();
        let mut sub = substrate(
            "R3",
            &revoker,
            &cat,
            &exec,
            &mut gate,
            &mut ledger,
            &outbox,
            1,
            10,
            1000,
        );

        let err = agent_loop
            .handle_run(&rt, &mut sub, &mut tele, RunOutcomeKind::Completed)
            .expect_err("an unfunded dispatch is refused");
        assert!(
            matches!(err, SkeletonError::DispatchRefused(_)),
            "no balance → no run: {err}"
        );
        assert_eq!(tele.traces_written(), 0);
        assert_eq!(
            tele.reserved(),
            0,
            "nothing reserved (the reserve was refused)"
        );
        assert_eq!(tele.settled(), 0);
        assert_eq!(
            tele.tokens_revoked(),
            1,
            "the minted token is torn down even on a refused dispatch"
        );
        assert_eq!(
            gate.reserve_refusals(),
            1,
            "the gate counted the refusal (AG-D11 telemetry)"
        );
    }

    #[test]
    fn agent_handle_frozen_shape_drives_the_seam() {
        let agent_loop = SkeletonAgent::new();
        let rt = SkeletonAgentRuntime::new();
        let out = agent_loop.handle(InboxEvent("issue.created".into()), &rt);
        assert!(
            out.0.contains("skeleton handle"),
            "the frozen 8.5 shape returns the loop outcome"
        );
    }

    #[test]
    fn ledger_balanced_predicate_is_exact() {
        let mut t = SkeletonTelemetry::new();
        assert!(t.ledger_balanced(), "an empty ledger is balanced");
        t.record_reserve(10);
        assert!(!t.ledger_balanced(), "reserved-not-settled is UNbalanced");
        t.record_settle(10);
        assert!(t.ledger_balanced(), "reserved == settled is balanced");
    }

    #[test]
    fn telemetry_signal_accessors_are_exact() {
        let mut t = SkeletonTelemetry::new();
        assert_eq!(t.tokens_revoked(), 0);
        assert_eq!(t.runs_completed(), 0);
        assert_eq!(t.runs_killed(), 0);
        assert_eq!(t.max_revocation_lag(), 0);
        assert_eq!(t.traces_written(), 0);

        t.record_revoke(7);
        t.record_revoke(3);
        t.record_revoke(9);
        t.record_trace();
        t.runs_completed = 2;
        t.runs_killed = 5;
        assert_eq!(
            t.tokens_revoked(),
            3,
            "tokens_revoked counts every revoke (kills -> 1)"
        );
        assert_eq!(
            t.max_revocation_lag(),
            9,
            "max lag is the MAXIMUM (7, then 3 ignored, then 9)"
        );
        assert_eq!(t.traces_written(), 1, "traces_written counts each trace");
        assert_eq!(
            t.runs_completed(),
            2,
            "runs_completed returns its field (kills -> 1)"
        );
        assert_eq!(t.runs_killed(), 5, "runs_killed returns its field");
        assert_eq!(
            t.reserved(),
            0,
            "reserved is independent (kills cross-field constant mutants)"
        );
    }

    #[test]
    fn record_revoke_keeps_the_maximum_lag() {
        let mut t = SkeletonTelemetry::new();
        t.record_revoke(5);
        assert_eq!(t.max_revocation_lag(), 5);
        t.record_revoke(5);
        assert_eq!(t.max_revocation_lag(), 5);
        t.record_revoke(2);
        assert_eq!(t.max_revocation_lag(), 5);
        t.record_revoke(8);
        assert_eq!(t.max_revocation_lag(), 8);
    }

    #[test]
    fn child_env_leak_predicate_is_exact() {
        let clean = ChildEnv::for_run("jti:R1");
        assert!(
            !clean.leaked_shared_token(),
            "a clean child env does not leak (the anti-leak unset)"
        );
        let leaked = ChildEnv {
            run_token_jti: "jti:R1".into(),
            shared_platform_token: Some("PLATFORM-TOKEN".into()),
        };
        assert!(
            leaked.leaked_shared_token(),
            "a leaked shared token IS a leak (kills -> false)"
        );
    }

    #[test]
    fn skeleton_error_display_is_loud_and_distinct() {
        let refused = SkeletonError::DispatchRefused("no balance".into()).to_string();
        let mint = SkeletonError::MintFailed("id down".into()).to_string();
        let cc = SkeletonError::CoCommit("journal".into()).to_string();
        assert!(
            refused.contains("dispatch refused"),
            "Display renders the refusal: {refused}"
        );
        assert!(
            mint.contains("mint failed"),
            "Display renders the mint failure: {mint}"
        );
        assert!(
            cc.contains("co-commit failed"),
            "Display renders the co-commit failure: {cc}"
        );
        let val = SkeletonError::ToolValidationRejected("bad args".into()).to_string();
        let exec = SkeletonError::ToolExecFailed("subsystem down".into()).to_string();
        let maxt = SkeletonError::MaxTurnsExhausted {
            run_id: "Rx".into(),
            turns: 16,
        }
        .to_string();
        assert!(
            val.contains("validation rejected"),
            "Display renders the validation rejection: {val}"
        );
        assert!(
            exec.contains("tool execution failed"),
            "Display renders the executor failure: {exec}"
        );
        assert!(
            maxt.contains("max_turns=16"),
            "Display renders the max-turns exhaustion: {maxt}"
        );
        assert_ne!(refused, mint);
        assert_ne!(mint, cc);
        assert_ne!(val, exec);
        assert_ne!(exec, maxt);
        assert!(
            !refused.is_empty(),
            "the error message is non-empty (kills fmt -> Ok(default))"
        );
    }

    #[test]
    fn skeleton_failures_have_stable_operator_codes() {
        assert_eq!(
            SkeletonError::ToolExecFailed("upstream detail".into()).code(),
            "tool_execution_failed",
        );
        assert_eq!(
            SkeletonError::RuntimeStepFailed {
                run_id: "Rprovider".into(),
                error: RuntimeStepError::Rejected { status: Some(503) },
            }
            .code(),
            "runtime_rejected",
        );
        assert_eq!(
            SkeletonError::MeteringUsageNotReported {
                run_id: "Runreported".into(),
            }
            .code(),
            "metering_usage_not_reported",
        );
        assert_eq!(
            SkeletonError::CostSettlementFailed {
                run_id: "Rsettle".into(),
                reason: "storage unavailable".into(),
            }
            .code(),
            "cost_settlement_failed",
        );
    }

    fn tool_def(name: &str) -> ToolDef {
        ToolDef {
            name: ToolName(name.into()),
            subsystem: "test".into(),
            version: 1,
            input_schema: "{}".into(),
            required_caps: vec![],
            effect_kind: EffectKind::Read,
            side_effecting: false,
            requires_approval: false,
            exposed_over_mcp: false,
        }
    }

    fn tool_call(name: &str) -> ToolCall {
        ToolCall {
            id: ToolCallId(format!("call:{name}")),
            name: ToolName(name.into()),
            arguments: serde_json::json!({}),
        }
    }

    #[test]
    fn loop_drives_tool_turns_then_submits() {
        #[derive(Default)]
        struct DriveBrain {
            tool_result_turns_seen: std::sync::Mutex<Vec<usize>>,
            outcomes_at_submit: std::sync::Mutex<Vec<ToolOutcome>>,
        }
        impl AgentRuntime for DriveBrain {
            fn step(&self, conv: &Conversation) -> StepOutcome {
                let model_turns = conv
                    .turns
                    .iter()
                    .filter(|t| matches!(t, Turn::Model(_)))
                    .count();
                let tr_turns = conv
                    .turns
                    .iter()
                    .filter(|t| matches!(t, Turn::ToolResults(_)))
                    .count();
                self.tool_result_turns_seen.lock().unwrap().push(tr_turns);
                match model_turns {
                    0 => StepOutcome::UseTools(vec![tool_call("search")]),
                    1 => StepOutcome::UseTools(vec![tool_call("read")]),
                    _ => {
                        let outcomes: Vec<ToolOutcome> = conv
                            .turns
                            .iter()
                            .flat_map(|t| match t {
                                Turn::ToolResults(rs) => rs.clone(),
                                _ => Vec::new(),
                            })
                            .collect();
                        *self.outcomes_at_submit.lock().unwrap() = outcomes;
                        StepOutcome::Submit(Submission("done".into()))
                    }
                }
            }
        }
        impl MeteredRuntime for DriveBrain {}

        let brain = DriveBrain::default();
        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker {
            ttl_w: 300,
            minted_at: 1000,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        let cat = MockToolSurface::with([tool_def("search"), tool_def("read")]);
        let exec = MockToolExecutor::new();
        let mut sub = substrate(
            "Rtools",
            &revoker,
            &cat,
            &exec,
            &mut gate,
            &mut ledger,
            &outbox,
            100,
            10,
            1000,
        );

        let out = agent_loop
            .handle_run(&brain, &mut sub, &mut tele, RunOutcomeKind::Completed)
            .expect("the tool-driving run completes");
        assert!(out.0.contains("completed"), "the run completed: {out:?}");

        assert_eq!(exec.call_count(), 2, "one execute per tool turn");
        let seen = exec.calls();
        assert_eq!(seen[0].name, ToolName("search".into()));
        assert_eq!(seen[1].name, ToolName("read".into()));

        assert_eq!(
            *brain.tool_result_turns_seen.lock().unwrap(),
            vec![0, 1, 2],
            "each tool turn appended a ToolResults turn the next step reads"
        );
        let submit_outcomes = brain.outcomes_at_submit.lock().unwrap().clone();
        assert_eq!(
            submit_outcomes.len(),
            2,
            "both tool round-trips accumulated"
        );
        assert_eq!(submit_outcomes[0].call_id, ToolCallId("call:search".into()));
        assert_eq!(
            submit_outcomes[0].result,
            ToolResult::Succeeded("mock-exec:search:ok".into()),
            "the executor's result was threaded back, keyed to its call"
        );
        assert_eq!(submit_outcomes[1].call_id, ToolCallId("call:read".into()));

        assert_eq!(tele.traces_written(), 1);
        assert!(tele.ledger_balanced(), "reserved == settled");
        assert_eq!(tele.tokens_revoked(), 1, "torn down on the completed path");
        assert_eq!(tele.runs_completed(), 1);
    }

    struct AlwaysUseTool(ToolName);
    impl AgentRuntime for AlwaysUseTool {
        fn step(&self, _conv: &Conversation) -> StepOutcome {
            StepOutcome::UseTools(vec![ToolCall {
                id: ToolCallId("c".into()),
                name: self.0.clone(),
                arguments: serde_json::json!({}),
            }])
        }
    }
    impl MeteredRuntime for AlwaysUseTool {}

    #[test]
    fn loop_hits_max_turns_and_terminates_gracefully() {
        let brain = AlwaysUseTool(ToolName("loop".into()));
        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker {
            ttl_w: 300,
            minted_at: 1000,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        let cat = MockToolSurface::with([tool_def("loop")]);
        let exec = MockToolExecutor::new();
        let mut sub = substrate(
            "Rmax",
            &revoker,
            &cat,
            &exec,
            &mut gate,
            &mut ledger,
            &outbox,
            100,
            10,
            1000,
        );

        let err = agent_loop
            .handle_run(&brain, &mut sub, &mut tele, RunOutcomeKind::Completed)
            .expect_err("a never-submitting brain trips the bounded ceiling");
        match err {
            SkeletonError::MaxTurnsExhausted { run_id, turns } => {
                assert_eq!(run_id, "Rmax");
                assert_eq!(turns, DEFAULT_MAX_TURNS, "the exact ceiling that tripped");
            }
            other => panic!("expected MaxTurnsExhausted, got {other:?}"),
        }
        assert_eq!(
            exec.call_count(),
            DEFAULT_MAX_TURNS,
            "one execute per bounded turn, then graceful termination"
        );
        assert_eq!(tele.tokens_revoked(), 1, "torn down on the max-turns path");
        assert_eq!(tele.traces_written(), 0, "no trace on an exhausted run");
        assert_eq!(tele.runs_completed(), 0);
        assert_eq!(tele.runs_killed(), 0);
    }

    #[test]
    fn loop_validation_failure_aborts_fail_closed_without_dispatch() {
        let brain = AlwaysUseTool(ToolName("ghost".into()));
        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker {
            ttl_w: 300,
            minted_at: 1000,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        let cat = MockToolSurface::new();
        let exec = MockToolExecutor::new();
        let mut sub = substrate(
            "Rbad",
            &revoker,
            &cat,
            &exec,
            &mut gate,
            &mut ledger,
            &outbox,
            100,
            10,
            1000,
        );

        let err = agent_loop
            .handle_run(&brain, &mut sub, &mut tele, RunOutcomeKind::Completed)
            .expect_err("an unvalidated tool call aborts the run");
        assert!(
            matches!(err, SkeletonError::ToolValidationRejected(_)),
            "fail-closed on validation: {err}"
        );
        assert_eq!(exec.call_count(), 0, "0 dispatch on a validation failure");
        assert_eq!(
            tele.tokens_revoked(),
            1,
            "torn down on the validation-abort path"
        );
        assert_eq!(tele.traces_written(), 0);
        assert_eq!(tele.runs_completed(), 0);
    }

    #[test]
    fn loop_executor_error_aborts_and_tears_down() {
        let brain = AlwaysUseTool(ToolName("read".into()));
        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker {
            ttl_w: 300,
            minted_at: 1000,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        let cat = MockToolSurface::with([tool_def("read")]);
        let exec =
            MockToolExecutor::with_results([Err(ToolExecError::Failed("subsystem down".into()))]);
        let mut sub = substrate(
            "Rerr",
            &revoker,
            &cat,
            &exec,
            &mut gate,
            &mut ledger,
            &outbox,
            100,
            10,
            1000,
        );

        let err = agent_loop
            .handle_run(&brain, &mut sub, &mut tele, RunOutcomeKind::Completed)
            .expect_err("an executor error aborts the run");
        assert!(
            matches!(err, SkeletonError::ToolExecFailed(_)),
            "loud executor failure: {err}"
        );
        assert_eq!(exec.call_count(), 1, "the failing call was attempted once");
        assert_eq!(
            tele.tokens_revoked(),
            1,
            "torn down on the executor-error path"
        );
        assert_eq!(tele.traces_written(), 0);
        assert_eq!(tele.runs_completed(), 0);
    }

    #[test]
    fn a_human_gate_parks_the_effect_without_becoming_a_model_tool_result() {
        let brain = AlwaysUseTool(ToolName("merge".into()));
        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker {
            ttl_w: 300,
            minted_at: 1000,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        let cat = MockToolSurface::with([tool_def("merge")]);
        let exec = MockToolExecutor::with_results([Err(ToolExecError::ApprovalRequired {
            gate_id: "gate-merge-42".into(),
        })]);
        let mut sub = substrate(
            "Rgate",
            &revoker,
            &cat,
            &exec,
            &mut gate,
            &mut ledger,
            &outbox,
            100,
            10,
            1000,
        );

        let error = agent_loop
            .handle_run(&brain, &mut sub, &mut tele, RunOutcomeKind::Completed)
            .expect_err("the host must yield as soon as a human decision is required");

        assert_eq!(
            error,
            SkeletonError::ApprovalRequired {
                gate_id: "gate-merge-42".into(),
            }
        );
        assert_eq!(exec.call_count(), 1, "the proposed effect was planned once");
        assert_eq!(tele.reserved(), 10, "the run remains admitted for a resume");
        assert_eq!(tele.settled(), 0, "a parked run is not called complete");
        assert_eq!(
            tele.tokens_revoked(),
            1,
            "the runtime credential is torn down"
        );
        assert_eq!(
            tele.traces_written(),
            0,
            "the unfinished run has no final trace"
        );
        assert_eq!(tele.runs_completed(), 0);
    }

    #[test]
    fn skeleton_run_token_totals_are_zero_and_not_reported() {
        let rt = SkeletonAgentRuntime::new();
        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker {
            ttl_w: 300,
            minted_at: 1000,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        let cat = MockToolSurface::new();
        let exec = MockToolExecutor::new();
        let mut sub = substrate(
            "Rtok0",
            &revoker,
            &cat,
            &exec,
            &mut gate,
            &mut ledger,
            &outbox,
            100,
            10,
            1000,
        );

        agent_loop
            .handle_run(&rt, &mut sub, &mut tele, RunOutcomeKind::Completed)
            .expect("the SKELETON chain completes");

        assert_eq!(tele.tokens_input(), 0, "no model → 0 input tokens");
        assert_eq!(tele.tokens_cached_input(), 0, "no model → 0 cached tokens");
        assert_eq!(tele.tokens_output(), 0, "no model → 0 output tokens");
        assert_eq!(
            tele.turns_usage_not_reported(),
            1,
            "the one submit turn reported no usage (fail-closed signal)"
        );
        assert!(tele.ledger_balanced(), "reserved == settled is unaffected");
        assert_eq!(tele.reserved(), 10);
        assert_eq!(tele.settled(), 10);
    }

    #[test]
    fn run_accumulates_per_turn_token_usage_into_telemetry() {
        #[derive(Default)]
        struct MeteredBrain;
        impl AgentRuntime for MeteredBrain {
            fn step(&self, conv: &Conversation) -> StepOutcome {
                match model_turns(conv) {
                    0 => StepOutcome::UseTools(vec![tool_call("search")]),
                    1 => StepOutcome::UseTools(vec![tool_call("read")]),
                    _ => StepOutcome::Submit(Submission("done".into())),
                }
            }
        }
        impl MeteredRuntime for MeteredBrain {
            fn step_metered(&self, conv: &Conversation) -> Result<MeteredStep, RuntimeStepError> {
                Ok(MeteredStep {
                    outcome: self.step(conv),
                    usage: TokenUsage::Reported {
                        input: 100,
                        cached_input: 20,
                        output: 5,
                    },
                })
            }
        }
        fn model_turns(conv: &Conversation) -> usize {
            conv.turns
                .iter()
                .filter(|t| matches!(t, Turn::Model(_)))
                .count()
        }

        let brain = MeteredBrain;
        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker {
            ttl_w: 300,
            minted_at: 1000,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        let cat = MockToolSurface::with([tool_def("search"), tool_def("read")]);
        let exec = MockToolExecutor::new();
        let mut sub = substrate(
            "Rtok",
            &revoker,
            &cat,
            &exec,
            &mut gate,
            &mut ledger,
            &outbox,
            100,
            10,
            1000,
        );

        let out = agent_loop
            .handle_run(&brain, &mut sub, &mut tele, RunOutcomeKind::Completed)
            .expect("the metered run completes");
        assert!(out.0.contains("completed"), "the run completed: {out:?}");

        assert_eq!(tele.tokens_input(), 300, "3 turns × 100 input");
        assert_eq!(tele.tokens_cached_input(), 60, "3 turns × 20 cached");
        assert_eq!(tele.tokens_output(), 15, "3 turns × 5 output");
        assert_eq!(
            tele.turns_usage_not_reported(),
            0,
            "every turn reported usage"
        );
        assert!(tele.ledger_balanced(), "reserved == settled is unaffected");
        assert_eq!(tele.reserved(), 10);
        assert_eq!(tele.settled(), 10);
    }

    #[test]
    fn record_token_usage_sums_reported_and_counts_not_reported() {
        let mut t = SkeletonTelemetry::new();
        t.record_token_usage(&TokenUsage::Reported {
            input: 10,
            cached_input: 2,
            output: 3,
        });
        t.record_token_usage(&TokenUsage::NotReported);
        t.record_token_usage(&TokenUsage::Reported {
            input: 5,
            cached_input: 1,
            output: 4,
        });
        assert_eq!(t.tokens_input(), 15);
        assert_eq!(t.tokens_cached_input(), 3);
        assert_eq!(t.tokens_output(), 7);
        assert_eq!(t.turns_usage_not_reported(), 1);
    }

    struct FakeWallet {
        balance: std::sync::Mutex<u64>,
        debits: std::sync::Mutex<Vec<(u64, String)>>,
        charge_keys: std::sync::Mutex<Vec<String>>,
    }
    impl FakeWallet {
        fn new(initial: u64) -> FakeWallet {
            FakeWallet {
                balance: std::sync::Mutex::new(initial),
                debits: std::sync::Mutex::new(Vec::new()),
                charge_keys: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn balance_now(&self) -> u64 {
            *self.balance.lock().unwrap()
        }
        fn debit_rows(&self) -> Vec<(u64, String)> {
            self.debits.lock().unwrap().clone()
        }
        fn charge_keys(&self) -> Vec<String> {
            self.charge_keys.lock().unwrap().clone()
        }
    }
    impl RunWallet for FakeWallet {
        fn balance(&self, _tenant: &TenantId) -> MicroUsd {
            MicroUsd(*self.balance.lock().unwrap())
        }
        fn debit_once(
            &self,
            _tenant: &TenantId,
            amount: MicroUsd,
            run_id: &str,
            charge_key: &str,
        ) -> Result<DebitOutcome, WalletError> {
            let mut b = self.balance.lock().unwrap();
            match b.checked_sub(amount.0) {
                Some(new_balance) => {
                    *b = new_balance;
                    self.debits
                        .lock()
                        .unwrap()
                        .push((amount.0, run_id.to_string()));
                    self.charge_keys
                        .lock()
                        .unwrap()
                        .push(charge_key.to_string());
                    Ok(DebitOutcome::Applied(MicroUsd(new_balance)))
                }
                None => Err(WalletError::InsufficientBalance {
                    requested: amount,
                    available: MicroUsd(*b),
                }),
            }
        }
    }

    fn count_model_turns(conv: &Conversation) -> usize {
        conv.turns
            .iter()
            .filter(|t| matches!(t, Turn::Model(_)))
            .count()
    }

    struct MeteredScriptBrain {
        usage: TokenUsage,
        steps: std::sync::atomic::AtomicUsize,
    }
    impl MeteredScriptBrain {
        fn new(usage: TokenUsage) -> MeteredScriptBrain {
            MeteredScriptBrain {
                usage,
                steps: std::sync::atomic::AtomicUsize::new(0),
            }
        }
        fn steps_taken(&self) -> usize {
            self.steps.load(std::sync::atomic::Ordering::SeqCst)
        }
    }
    impl AgentRuntime for MeteredScriptBrain {
        fn step(&self, conv: &Conversation) -> StepOutcome {
            match count_model_turns(conv) {
                0 => StepOutcome::UseTools(vec![tool_call("search")]),
                1 => StepOutcome::UseTools(vec![tool_call("read")]),
                _ => StepOutcome::Submit(Submission("done".into())),
            }
        }
    }
    impl MeteredRuntime for MeteredScriptBrain {
        fn step_metered(&self, conv: &Conversation) -> Result<MeteredStep, RuntimeStepError> {
            self.steps.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(MeteredStep {
                outcome: self.step(conv),
                usage: self.usage,
            })
        }
    }

    const TEST_USAGE: TokenUsage = TokenUsage::Reported {
        input: 1_000,
        cached_input: 500,
        output: 200,
    };
    const TEST_CHARGE_PER_TURN: u64 = 459;

    #[test]
    fn metered_run_debits_wallet_per_turn_and_ledger_stays_balanced() {
        let brain = MeteredScriptBrain::new(TEST_USAGE);
        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker {
            ttl_w: 300,
            minted_at: 1000,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        let cat = MockToolSurface::with([tool_def("search"), tool_def("read")]);
        let exec = MockToolExecutor::new();
        let wallet = FakeWallet::new(10_000);
        let mut sub = substrate(
            "Rmeter",
            &revoker,
            &cat,
            &exec,
            &mut gate,
            &mut ledger,
            &outbox,
            100,
            10,
            1000,
        );
        sub.wallet = Some(&wallet);

        let out = agent_loop
            .handle_run(&brain, &mut sub, &mut tele, RunOutcomeKind::Completed)
            .expect("the metered run completes");
        assert!(out.0.contains("completed"), "the run completed: {out:?}");

        let rows = wallet.debit_rows();
        assert_eq!(
            rows.len(),
            3,
            "one debit per turn (no double-charge, no skip)"
        );
        for (amount, run_id) in &rows {
            assert_eq!(
                *amount, TEST_CHARGE_PER_TURN,
                "each turn debits wholesale+markup"
            );
            assert_eq!(run_id, "Rmeter", "every debit is run_id-linked");
        }
        assert_eq!(
            wallet.charge_keys(),
            [
                "Rmeter/model-turn/0",
                "Rmeter/model-turn/1",
                "Rmeter/model-turn/2",
            ],
            "each turn receives a deterministic identity that survives workflow replay",
        );
        assert_eq!(
            wallet.balance_now(),
            10_000 - 3 * TEST_CHARGE_PER_TURN,
            "balance dropped by exactly the sum of the per-turn charges"
        );
        assert_eq!(tele.charged_micro(), 3 * TEST_CHARGE_PER_TURN);
        assert!(tele.ledger_balanced(), "reserved == settled is unaffected");
        assert_eq!(tele.reserved(), 10);
        assert_eq!(tele.settled(), 10);
        assert_eq!(tele.traces_written(), 1);
        assert_eq!(tele.tokens_revoked(), 1);
        assert_eq!(tele.runs_completed(), 1);
    }

    #[test]
    fn metered_run_dry_wallet_halts_gracefully_no_negative_balance() {
        let brain = MeteredScriptBrain::new(TEST_USAGE);
        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker {
            ttl_w: 300,
            minted_at: 1000,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        let cat = MockToolSurface::with([tool_def("search"), tool_def("read")]);
        let exec = MockToolExecutor::new();
        let wallet = FakeWallet::new(1_000);
        let mut sub = substrate(
            "Rdry",
            &revoker,
            &cat,
            &exec,
            &mut gate,
            &mut ledger,
            &outbox,
            100,
            10,
            1000,
        );
        sub.wallet = Some(&wallet);

        let err = agent_loop
            .handle_run(&brain, &mut sub, &mut tele, RunOutcomeKind::Completed)
            .expect_err("the dry wallet halts the run mid-loop");
        match err {
            SkeletonError::WalletSpendCapReached { run_id, stage } => {
                assert_eq!(run_id, "Rdry");
                assert_eq!(
                    stage,
                    SpendCapStage::PostDebit,
                    "the mid-run debit was refused"
                );
            }
            other => panic!("expected WalletSpendCapReached, got {other:?}"),
        }
        assert_eq!(
            wallet.debit_rows().len(),
            2,
            "only the funded turns debited"
        );
        assert_eq!(
            wallet.balance_now(),
            1_000 - 2 * TEST_CHARGE_PER_TURN,
            "balance = 1000 − 2×459 = 82 (never negative; the refused turn left it untouched)"
        );
        assert_eq!(tele.charged_micro(), 2 * TEST_CHARGE_PER_TURN);
        assert_eq!(tele.tokens_revoked(), 1, "torn down on the spend-cap path");
        assert_eq!(tele.traces_written(), 0, "no trace on a capped run");
        assert_eq!(tele.runs_completed(), 0);
    }

    #[test]
    fn metered_run_not_reported_turn_fails_closed_with_teardown() {
        let rt = SkeletonAgentRuntime::new();
        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker {
            ttl_w: 300,
            minted_at: 1000,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        let cat = MockToolSurface::new();
        let exec = MockToolExecutor::new();
        let wallet = FakeWallet::new(10_000);
        let mut sub = substrate(
            "Rnr",
            &revoker,
            &cat,
            &exec,
            &mut gate,
            &mut ledger,
            &outbox,
            100,
            10,
            1000,
        );
        sub.wallet = Some(&wallet);

        let err = agent_loop
            .handle_run(&rt, &mut sub, &mut tele, RunOutcomeKind::Completed)
            .expect_err("an unmetered paid turn fails closed");
        match err {
            SkeletonError::MeteringUsageNotReported { run_id } => assert_eq!(run_id, "Rnr"),
            other => panic!("expected MeteringUsageNotReported, got {other:?}"),
        }
        assert!(
            wallet.debit_rows().is_empty(),
            "0 debit on a NotReported turn"
        );
        assert_eq!(wallet.balance_now(), 10_000, "balance untouched");
        assert_eq!(tele.charged_micro(), 0);
        assert_eq!(
            tele.tokens_revoked(),
            1,
            "torn down on the fail-closed path"
        );
        assert_eq!(tele.traces_written(), 0);
        assert_eq!(tele.runs_completed(), 0);
    }

    #[test]
    fn failed_runtime_step_keeps_its_cause_and_never_becomes_a_metering_error() {
        struct UnavailableRuntime;

        impl AgentRuntime for UnavailableRuntime {
            fn step(&self, _conv: &Conversation) -> StepOutcome {
                unreachable!("paid runs use the fallible metered boundary")
            }
        }

        impl MeteredRuntime for UnavailableRuntime {
            fn step_metered(&self, _conv: &Conversation) -> Result<MeteredStep, RuntimeStepError> {
                Err(RuntimeStepError::Unavailable)
            }
        }

        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker {
            ttl_w: 300,
            minted_at: 1000,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        let cat = MockToolSurface::new();
        let exec = MockToolExecutor::new();
        let wallet = FakeWallet::new(10_000);
        let mut sub = substrate(
            "Rprovider",
            &revoker,
            &cat,
            &exec,
            &mut gate,
            &mut ledger,
            &outbox,
            100,
            10,
            1000,
        );
        sub.wallet = Some(&wallet);

        let error = agent_loop
            .handle_run(
                &UnavailableRuntime,
                &mut sub,
                &mut tele,
                RunOutcomeKind::Completed,
            )
            .expect_err("an unavailable runtime fails the run");

        assert_eq!(
            error,
            SkeletonError::RuntimeStepFailed {
                run_id: "Rprovider".into(),
                error: RuntimeStepError::Unavailable,
            },
        );
        assert!(
            wallet.debit_rows().is_empty(),
            "a failed call has no usage to debit"
        );
        assert_eq!(tele.turns_usage_not_reported(), 0);
        assert_eq!(
            tele.tokens_revoked(),
            1,
            "the failed run still tears down its token"
        );
    }

    #[test]
    fn metered_run_pre_step_zero_balance_gate_blocks_the_paid_call() {
        let brain = MeteredScriptBrain::new(TEST_USAGE);
        let agent_loop = SkeletonAgent::new();
        let revoker = FakeRevoker {
            ttl_w: 300,
            minted_at: 1000,
            ..Default::default()
        };
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut tele = SkeletonTelemetry::new();
        let cat = MockToolSurface::with([tool_def("search"), tool_def("read")]);
        let exec = MockToolExecutor::new();
        let wallet = FakeWallet::new(0);
        let mut sub = substrate(
            "Rzero",
            &revoker,
            &cat,
            &exec,
            &mut gate,
            &mut ledger,
            &outbox,
            100,
            10,
            1000,
        );
        sub.wallet = Some(&wallet);

        let err = agent_loop
            .handle_run(&brain, &mut sub, &mut tele, RunOutcomeKind::Completed)
            .expect_err("a zero-balance wallet blocks the run");
        match err {
            SkeletonError::WalletSpendCapReached { run_id, stage } => {
                assert_eq!(run_id, "Rzero");
                assert_eq!(
                    stage,
                    SpendCapStage::PreStepGate,
                    "halted at the pre-step gate"
                );
            }
            other => panic!("expected WalletSpendCapReached(PreStepGate), got {other:?}"),
        }
        assert_eq!(brain.steps_taken(), 0, "the paid model call was never made");
        assert!(
            wallet.debit_rows().is_empty(),
            "0 debit - the call was blocked"
        );
        assert_eq!(tele.charged_micro(), 0);
        assert_eq!(
            tele.tokens_revoked(),
            1,
            "torn down on the pre-step-gate path"
        );
        assert_eq!(tele.traces_written(), 0);
        assert_eq!(tele.runs_completed(), 0);
    }

    #[test]
    fn metering_error_display_is_loud_and_distinct() {
        let cap = SkeletonError::WalletSpendCapReached {
            run_id: "Rx".into(),
            stage: SpendCapStage::PostDebit,
        }
        .to_string();
        let nr = SkeletonError::MeteringUsageNotReported {
            run_id: "Rx".into(),
        }
        .to_string();
        let ov = SkeletonError::MeteringOverflow {
            run_id: "Rx".into(),
            reason: "boom".into(),
        }
        .to_string();
        let settle = SkeletonError::CostSettlementFailed {
            run_id: "Rx".into(),
            reason: "database unavailable".into(),
        }
        .to_string();
        assert!(cap.contains("spend cap"), "renders the cap: {cap}");
        assert!(cap.contains("post-debit"), "renders the stage: {cap}");
        assert!(
            nr.contains("fail-closed"),
            "renders the fail-closed abort: {nr}"
        );
        assert!(
            ov.contains("overflow") && ov.contains("boom"),
            "renders the overflow: {ov}"
        );
        assert!(
            settle.contains("settlement failed") && settle.contains("remains retryable"),
            "renders the recoverable settlement failure: {settle}"
        );
        assert_ne!(cap, nr);
        assert_ne!(nr, ov);
        assert_ne!(ov, settle);
    }

    #[test]
    fn every_run_exit_revokes_its_token_exactly_once() {
        let agent_loop = SkeletonAgent::new();
        let w: i64 = 300;
        let now: i64 = 1_000;

        let run = |run_id: &str,
                   runtime: &dyn MeteredRuntime,
                   kill: RunOutcomeKind,
                   wallet: Option<&dyn RunWallet>|
         -> SkeletonTelemetry {
            let revoker = FakeRevoker {
                ttl_w: w,
                minted_at: now,
                ..Default::default()
            };
            let mut gate = AgentRunGate::new();
            let mut ledger = CostLedger::new();
            let outbox = myelin_events::OutboxStore::new();
            let mut tele = SkeletonTelemetry::new();
            let cat = MockToolSurface::with([tool_def("search"), tool_def("read")]);
            let exec = MockToolExecutor::new();
            let mut sub = substrate(
                run_id,
                &revoker,
                &cat,
                &exec,
                &mut gate,
                &mut ledger,
                &outbox,
                100,
                10,
                now,
            );
            sub.wallet = wallet;
            let _ = agent_loop.handle_run(runtime, &mut sub, &mut tele, kill);
            tele
        };

        let done = run(
            "Ronce-done",
            &SkeletonAgentRuntime::new(),
            RunOutcomeKind::Completed,
            None,
        );
        assert_eq!(done.runs_completed(), 1, "the run completed");
        assert_eq!(
            done.tokens_revoked(),
            1,
            "completion: revoked EXACTLY once (never zero, never twice)"
        );
        assert!(done.max_revocation_lag() <= w as u64, "lag within bound W");

        let killed = run(
            "Ronce-kill",
            &SkeletonAgentRuntime::new(),
            RunOutcomeKind::KilledMidFlight,
            None,
        );
        assert_eq!(killed.runs_killed(), 1, "the run was killed mid-flight");
        assert_eq!(
            killed.tokens_revoked(),
            1,
            "kill path: revoked EXACTLY once (never zero, never twice)"
        );
        assert!(
            killed.max_revocation_lag() <= w as u64,
            "lag within bound W"
        );

        let brain = MeteredScriptBrain::new(TEST_USAGE);
        let wallet = FakeWallet::new(1_000);
        let capped = run(
            "Ronce-cap",
            &brain,
            RunOutcomeKind::Completed,
            Some(&wallet),
        );
        assert_eq!(
            capped.runs_completed(),
            0,
            "the capped run did NOT complete (fail-closed abort)"
        );
        assert_eq!(
            capped.tokens_revoked(),
            1,
            "insufficient-balance abort: revoked EXACTLY once (never zero, never twice)"
        );
        assert!(
            capped.max_revocation_lag() <= w as u64,
            "lag within bound W"
        );
    }

    struct FailingRevoker;

    impl RunTokenRevoker for FailingRevoker {
        fn revoke(&self, _jti: &str, _now_secs: i64, _teardown_secs: i64) -> Result<u64, String> {
            Err("revocation store unavailable".into())
        }

        fn is_dead(&self, _jti: &str, _now_secs: i64) -> bool {
            true
        }
    }

    #[test]
    fn a_teardown_outage_fails_the_run_without_panicking() {
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let outbox = myelin_events::OutboxStore::new();
        let mut telemetry = SkeletonTelemetry::new();
        let catalogue = MockToolSurface::new();
        let executor = MockToolExecutor::new();
        let mut sub = substrate(
            "R-revoke-outage",
            &FailingRevoker,
            &catalogue,
            &executor,
            &mut gate,
            &mut ledger,
            &outbox,
            100,
            10,
            1_000,
        );

        let error = SkeletonAgent::new()
            .handle_run(
                &SkeletonAgentRuntime::new(),
                &mut sub,
                &mut telemetry,
                RunOutcomeKind::Completed,
            )
            .expect_err("a run cannot claim success when its token remains live");

        assert_eq!(error.code(), "run_token_revocation_failed");
        assert_eq!(telemetry.tokens_revoked(), 0);
        assert_eq!(telemetry.runs_completed(), 0);
    }
}

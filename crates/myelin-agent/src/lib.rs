use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Conversation {
    pub system: SystemContext,
    pub turns: Vec<Turn>,
    pub tools: Vec<ToolSchema>,
    pub budget: BudgetView,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SystemContext(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolSchema {
    pub name: ToolName,
    pub description: String,
    pub input_schema: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BudgetView(pub u64);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Turn {
    Model(StepOutcome),
    ToolResults(Vec<ToolOutcome>),
    Approval(ApprovalNote),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalNote(pub String);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: ToolCallId,
    pub name: ToolName,
    pub arguments: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submission(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepOutcome {
    UseTools(Vec<ToolCall>),
    Submit(Submission),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokenUsage {
    Reported {
        input: u64,
        cached_input: u64,
        output: u64,
    },
    #[default]
    NotReported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOutcome {
    pub call_id: ToolCallId,
    pub result: ToolResult,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolName(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EffectKind {
    Read,
    Compute,
    Mutate,
    External,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: ToolName,
    pub subsystem: String,
    pub version: u32,
    pub input_schema: String,
    pub required_caps: Vec<String>,
    pub effect_kind: EffectKind,
    pub side_effecting: bool,
    pub requires_approval: bool,
    pub exposed_over_mcp: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RunCtx(pub String);

#[derive(Clone, PartialEq, Eq)]
pub struct EffectAuthority {
    pub run_token: myelin_identity::RunToken,
    pub principal_id: myelin_identity::PrincipalId,
    pub tool: String,
    pub idempotency_key: String,
}

impl core::fmt::Debug for EffectAuthority {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EffectAuthority")
            .field("jti", &"<redacted>")
            .field("principal_id", &self.principal_id)
            .field("tool", &self.tool)
            .field("idempotency_key", &"<redacted>")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposedEffect(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EffectResult {
    Applied(EventId),
    Gated(GateId),
    Denied(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxEvent(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunOutcome(pub String);

pub trait AgentRuntime {
    fn step(&self, conv: &Conversation) -> StepOutcome;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeteredStep {
    pub outcome: StepOutcome,
    pub usage: TokenUsage,
}

pub trait MeteredRuntime: AgentRuntime {
    fn step_metered(&self, conv: &Conversation) -> MeteredStep {
        MeteredStep {
            outcome: self.step(conv),
            usage: TokenUsage::NotReported,
        }
    }
}

pub trait Agent {
    fn handle(&self, inbox: InboxEvent, runtime: &dyn AgentRuntime) -> RunOutcome;
}

pub trait ToolHands {
    fn exec(&self, cmd: Command) -> ToolResult;
}

pub trait ToolSurface {
    fn register_tool(&mut self, def: ToolDef);
    fn resolve(&self, name: &ToolName) -> Option<&ToolDef>;
}

pub trait EventInbox {
    fn deliver(&self, ev: InboxEvent);
}

pub trait EffectApi {
    fn apply(&self, run: &RunCtx, effect: ProposedEffect) -> EffectResult;

    fn apply_authorized(
        &self,
        _run: &RunCtx,
        _authority: &EffectAuthority,
        _effect: ProposedEffect,
    ) -> EffectResult {
        EffectResult::Denied(
            "effect adapter does not implement signed run-token authority verification - denied"
                .into(),
        )
    }
}

pub trait DryRun {
    fn dry_run(&self, inbox: InboxEvent) -> Vec<ProposedEffect>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_authority_debug_redacts_every_credential_and_replay_handle() {
        let authority = EffectAuthority {
            run_token: myelin_identity::RunToken {
                token: "secret-bearer".into(),
                jti: "secret-jti".into(),
            },
            principal_id: myelin_identity::PrincipalId("principal".into()),
            tool: "issue.close".into(),
            idempotency_key: "secret-idempotency-key".into(),
        };
        let rendered = format!("{authority:?}");
        for secret in ["secret-bearer", "secret-jti", "secret-idempotency-key"] {
            assert!(!rendered.contains(secret));
        }
    }

    struct Mock {
        catalogue: Vec<ToolDef>,
    }

    impl AgentRuntime for Mock {
        fn step(&self, _conv: &Conversation) -> StepOutcome {
            StepOutcome::Submit(Submission("ok".into()))
        }
    }

    impl MeteredRuntime for Mock {}

    impl Agent for Mock {
        fn handle(&self, _inbox: InboxEvent, runtime: &dyn AgentRuntime) -> RunOutcome {
            let _ = runtime.step(&Conversation::default());
            RunOutcome("done".into())
        }
    }

    impl ToolHands for Mock {
        fn exec(&self, _cmd: Command) -> ToolResult {
            ToolResult("sim:executed".into())
        }
    }

    impl ToolSurface for Mock {
        fn register_tool(&mut self, def: ToolDef) {
            self.catalogue.push(def);
        }
        fn resolve(&self, name: &ToolName) -> Option<&ToolDef> {
            self.catalogue.iter().find(|d| &d.name == name)
        }
    }

    impl EventInbox for Mock {
        fn deliver(&self, _ev: InboxEvent) {
        }
    }

    impl EffectApi for Mock {
        fn apply(&self, _run: &RunCtx, _effect: ProposedEffect) -> EffectResult {
            EffectResult::Applied(EventId("evt-1".into()))
        }
    }

    impl DryRun for Mock {
        fn dry_run(&self, _inbox: InboxEvent) -> Vec<ProposedEffect> {
            vec![ProposedEffect("planned".into())]
        }
    }

    fn def(name: &str) -> ToolDef {
        ToolDef {
            name: ToolName(name.into()),
            subsystem: "issues".into(),
            version: 1,
            input_schema: "{}".into(),
            required_caps: vec!["issue.write".into()],
            effect_kind: EffectKind::Mutate,
            side_effecting: true,
            requires_approval: false,
            exposed_over_mcp: false,
        }
    }

    #[test]
    fn agent_runtime_step_signature_is_frozen() {
        let m = Mock { catalogue: vec![] };
        assert!(matches!(
            m.step(&Conversation::default()),
            StepOutcome::Submit(_)
        ));
    }

    #[test]
    fn metered_runtime_default_step_is_not_reported() {
        let m = Mock { catalogue: vec![] };
        let metered = m.step_metered(&Conversation::default());
        assert_eq!(metered.outcome, StepOutcome::Submit(Submission("ok".into())));
        assert_eq!(metered.usage, TokenUsage::NotReported);
        assert_eq!(metered.outcome, m.step(&Conversation::default()));
    }

    #[test]
    fn token_usage_carries_raw_counts_and_defaults_not_reported() {
        assert_eq!(TokenUsage::default(), TokenUsage::NotReported);
        let reported = TokenUsage::Reported {
            input: 50,
            cached_input: 8,
            output: 12,
        };
        assert_ne!(reported, TokenUsage::NotReported);
        assert!(matches!(
            reported,
            TokenUsage::Reported {
                input: 50,
                cached_input: 8,
                output: 12
            }
        ));
        let json = serde_json::to_string(&reported).unwrap();
        assert_eq!(reported, serde_json::from_str(&json).unwrap());
    }

    #[test]
    fn agent_handle_signature_is_frozen() {
        let m = Mock { catalogue: vec![] };
        let out = m.handle(InboxEvent("mention".into()), &m);
        assert_eq!(out, RunOutcome("done".into()));
    }

    #[test]
    fn tool_hands_exec_signature_is_frozen() {
        let m = Mock { catalogue: vec![] };
        assert_eq!(
            m.exec(Command("cargo test".into())),
            ToolResult("sim:executed".into())
        );
    }

    #[test]
    fn tool_surface_register_resolve_signatures_are_frozen() {
        let mut m = Mock { catalogue: vec![] };
        m.register_tool(def("issue.transition"));
        let resolved = m.resolve(&ToolName("issue.transition".into()));
        assert!(resolved.is_some());
        let d = resolved.unwrap();
        assert_eq!(d.subsystem, "issues");
        assert_eq!(d.effect_kind, EffectKind::Mutate);
        assert!(d.side_effecting);
        assert!(m.resolve(&ToolName("nope".into())).is_none());
    }

    #[test]
    fn event_inbox_deliver_signature_is_frozen() {
        let m = Mock { catalogue: vec![] };
        m.deliver(InboxEvent("issue.created".into()));
    }

    #[test]
    fn effect_api_apply_signature_is_frozen() {
        let m = Mock { catalogue: vec![] };
        match m.apply(&RunCtx::default(), ProposedEffect("close-issue".into())) {
            EffectResult::Applied(EventId(id)) => assert_eq!(id, "evt-1"),
            other => panic!("expected Applied, got {other:?}"),
        }
    }

    #[test]
    fn dry_run_signature_is_frozen() {
        let m = Mock { catalogue: vec![] };
        let plan = m.dry_run(InboxEvent("mention".into()));
        assert_eq!(plan, vec![ProposedEffect("planned".into())]);
    }

    #[test]
    fn step_outcome_variants_are_distinct() {
        let use_tools = StepOutcome::UseTools(vec![ToolCall {
            id: ToolCallId("call-1".into()),
            name: ToolName("t".into()),
            arguments: serde_json::Value::Null,
        }]);
        let submit = StepOutcome::Submit(Submission("a".into()));
        assert_ne!(use_tools, submit);
        assert!(matches!(use_tools, StepOutcome::UseTools(ref v) if v.len() == 1));
        assert!(matches!(submit, StepOutcome::Submit(ref s) if s.0 == "a"));
    }

    #[test]
    fn effect_result_variants_are_distinct() {
        let applied = EffectResult::Applied(EventId("e".into()));
        let gated = EffectResult::Gated(GateId("g".into()));
        let denied = EffectResult::Denied("nope".into());
        assert_ne!(applied, gated);
        assert_ne!(gated, denied);
        assert_ne!(applied, denied);
        assert!(matches!(applied, EffectResult::Applied(EventId(ref id)) if id == "e"));
        assert!(matches!(gated, EffectResult::Gated(GateId(ref id)) if id == "g"));
        assert!(matches!(denied, EffectResult::Denied(ref r) if r == "nope"));
    }

    #[test]
    fn effect_kind_variants_are_distinct() {
        let all = [
            EffectKind::Read,
            EffectKind::Compute,
            EffectKind::Mutate,
            EffectKind::External,
        ];
        for (i, a) in all.iter().enumerate() {
            for (j, b) in all.iter().enumerate() {
                assert_eq!(i == j, a == b, "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn tool_def_field_list_is_frozen() {
        let d = ToolDef {
            name: ToolName("ci.deploy".into()),
            subsystem: "ci".into(),
            version: 7,
            input_schema: "{\"x\":1}".into(),
            required_caps: vec!["ci.deploy".into(), "secret.read".into()],
            effect_kind: EffectKind::External,
            side_effecting: true,
            requires_approval: true,
            exposed_over_mcp: true,
        };
        assert_eq!(d.name, ToolName("ci.deploy".into()));
        assert_eq!(d.subsystem, "ci");
        assert_eq!(d.version, 7);
        assert_eq!(d.input_schema, "{\"x\":1}");
        assert_eq!(d.required_caps.len(), 2);
        assert_eq!(d.effect_kind, EffectKind::External);
        assert!(d.side_effecting);
        assert!(d.requires_approval);
        assert!(d.exposed_over_mcp);
    }

    #[test]
    fn value_types_serde_round_trip() {
        let d = def("issue.close");
        let json = serde_json::to_string(&d).unwrap();
        let back: ToolDef = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);

        let r = EffectResult::Gated(GateId("card:1:0".into()));
        let rj = serde_json::to_string(&r).unwrap();
        let rb: EffectResult = serde_json::from_str(&rj).unwrap();
        assert_eq!(r, rb);
    }
}

use myelin_agent::{
    BudgetView, ProposedEffect, StepOutcome, Submission, SystemContext, ToolCall, ToolCallId,
    ToolDef, ToolName, ToolSchema, ToolSurface,
};
use myelin_issues::rebac_fragment::object_types as issue_objects;

use crate::defaults::{cap, mutate_tool_def, register_tool_defs, LooseningViolation};
use crate::dry_run::proposed_effect_sequence;
use crate::effect_api::{EffectCost, PlannedEffect};
use crate::issues_tools::{issues_tool_defs, ISSUES_SUBSYSTEM, ISSUES_TOOL_VERSION};
use crate::mock::{MockAgentRuntime, MockScript};

pub const CREATE_TOOL: &str = "create";
pub const UPDATE_TOOL: &str = "update";
pub const COMMENT_TOOL: &str = "comment";
pub const LINK_TOOL: &str = "link";
pub const ESTIMATE_TOOL: &str = "estimate";
pub const REORDER_TOOL: &str = "reorder";
pub const ASSIGN_TOOL: &str = "assign";
pub const CLOSE_TOOL: &str = "close";

pub fn create_required_caps() -> Vec<String> {
    cap(issue_objects::ISSUE, "create")
}

pub fn update_required_caps() -> Vec<String> {
    cap(issue_objects::ISSUE, "update")
}

pub fn comment_required_caps() -> Vec<String> {
    cap(issue_objects::ISSUE, "comment")
}

pub fn assign_required_caps() -> Vec<String> {
    cap(issue_objects::ISSUE, "transition")
}

fn crud_tool_def(name: &str, caps: Vec<String>, input_schema: &str) -> ToolDef {
    mutate_tool_def(ISSUES_SUBSYSTEM, name, ISSUES_TOOL_VERSION, input_schema, caps)
}

pub fn create_tool_def() -> ToolDef {
    crud_tool_def(
        CREATE_TOOL,
        create_required_caps(),
        r#"{"type":"object","required":["project","title"],"properties":{"project":{"type":"string"},"title":{"type":"string"},"type":{"type":"string"}}}"#,
    )
}

pub fn update_tool_def() -> ToolDef {
    crud_tool_def(
        UPDATE_TOOL,
        update_required_caps(),
        r#"{"type":"object","required":["issue"],"properties":{"issue":{"type":"string"},"fields":{"type":"object"}}}"#,
    )
}

pub fn comment_tool_def() -> ToolDef {
    crud_tool_def(
        COMMENT_TOOL,
        comment_required_caps(),
        r#"{"type":"object","required":["issue","body"],"properties":{"issue":{"type":"string"},"body":{"type":"string"}}}"#,
    )
}

pub fn link_tool_def() -> ToolDef {
    crud_tool_def(
        LINK_TOOL,
        update_required_caps(),
        r#"{"type":"object","required":["issue","target","relation"],"properties":{"issue":{"type":"string"},"target":{"type":"string"},"relation":{"type":"string"}}}"#,
    )
}

pub fn estimate_tool_def() -> ToolDef {
    crud_tool_def(
        ESTIMATE_TOOL,
        update_required_caps(),
        r#"{"type":"object","required":["issue","points"],"properties":{"issue":{"type":"string"},"points":{"type":"number"}}}"#,
    )
}

pub fn reorder_tool_def() -> ToolDef {
    crud_tool_def(
        REORDER_TOOL,
        update_required_caps(),
        r#"{"type":"object","required":["issue","order_key"],"properties":{"issue":{"type":"string"},"order_key":{"type":"string"}}}"#,
    )
}

pub fn assign_tool_def() -> ToolDef {
    crud_tool_def(
        ASSIGN_TOOL,
        assign_required_caps(),
        r#"{"type":"object","required":["issue","assignee"],"properties":{"issue":{"type":"string"},"assignee":{"type":"string"}}}"#,
    )
}

pub fn close_tool_def() -> ToolDef {
    crud_tool_def(
        CLOSE_TOOL,
        assign_required_caps(),
        r#"{"type":"object","required":["issue"],"properties":{"issue":{"type":"string"},"reason":{"type":"string"}}}"#,
    )
}

pub fn full_issues_tool_defs() -> Vec<ToolDef> {
    let mut defs = vec![
        create_tool_def(),
        update_tool_def(),
        comment_tool_def(),
        link_tool_def(),
        estimate_tool_def(),
        reorder_tool_def(),
        assign_tool_def(),
        close_tool_def(),
    ];
    defs.extend(issues_tool_defs());
    defs
}

pub fn register_full_issues_tools<S: ToolSurface>(
    surface: &mut S,
) -> Result<Vec<ToolDef>, LooseningViolation> {
    register_tool_defs(surface, full_issues_tool_defs())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForecastInput {
    pub remaining: u64,
    pub velocity_per_period: u64,
    pub at_risk_threshold_periods: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForecastOutput {
    pub periods_to_completion: Option<u64>,
    pub at_risk: bool,
}

pub struct LinearForecast;

impl LinearForecast {
    pub fn forecast(input: &ForecastInput) -> ForecastOutput {
        let periods = if input.velocity_per_period == 0 {
            None
        } else {
            Some(input.remaining.div_ceil(input.velocity_per_period))
        };
        let at_risk = match periods {
            Some(p) => p > input.at_risk_threshold_periods,
            None => true,
        };
        ForecastOutput {
            periods_to_completion: periods,
            at_risk,
        }
    }
}

pub fn mock_forecast_agent(input: &ForecastInput) -> MockAgentRuntime {
    let out = LinearForecast::forecast(input);
    let summary = match out.periods_to_completion {
        Some(p) => format!(
            "forecast(linear): ~{p} period(s) to completion (remaining={}, velocity={}/period); at_risk={}",
            input.remaining, input.velocity_per_period, out.at_risk
        ),
        None => format!(
            "forecast(linear): no defensible date (velocity=0, remaining={}); at_risk=true",
            input.remaining
        ),
    };
    MockAgentRuntime::new(MockScript::submit_only(
        "issues.forecast agent (mock, linear; labelled as an agent)",
        summary,
    ))
}

pub fn mock_triage_agent(issue_ref: &str) -> MockAgentRuntime {
    let triage_tool = crate::issues_tools::TRIAGE_TOOL;
    let script = MockScript::new(
        SystemContext("issues.triage agent (mock; labelled as an agent)".into()),
        vec![ToolSchema {
            name: ToolName(triage_tool.to_string()),
            description: String::new(),
            input_schema: "{}".into(),
        }],
        BudgetView(0),
        vec![
            StepOutcome::UseTools(vec![ToolCall {
                id: ToolCallId(format!("call:{triage_tool}")),
                name: ToolName(triage_tool.to_string()),
                arguments: serde_json::Value::Null,
            }]),
            StepOutcome::Submit(Submission(format!(
                "triage(suggestion strip): proposed triage for {issue_ref} (S9 - the human accepts)"
            ))),
        ],
    );
    MockAgentRuntime::new(script)
}

pub fn triage_effect_for(name: &ToolName, issue_ref: &str) -> Option<PlannedEffect> {
    if name.0 == crate::issues_tools::TRIAGE_TOOL {
        Some(PlannedEffect {
            tool: name.clone(),
            object: myelin_tenancy::ArtifactRef(format!("myelin://acme/issue/issue/{issue_ref}")),
            input_json: format!(r#"{{"issue":"{issue_ref}","priority":"high"}}"#),
            field: None,
            transition: None,
            cost: EffectCost {
                unit: "issue.transition",
                wholesale: 1,
                markup: 0,
            },
        })
    } else {
        None
    }
}

pub fn triage_suggestion_strip(issue_ref: &str) -> Vec<ProposedEffect> {
    let brain = mock_triage_agent(issue_ref);
    let script = brain.script().clone();
    proposed_effect_sequence(
        &brain,
        &script,
        &|name: &ToolName| triage_effect_for(name, issue_ref),
        crate::mock::MOCK_MAX_STEPS,
    )
}

pub fn replay_forecast_agent(input: &ForecastInput) -> crate::mock::ReplayRecord {
    let brain = mock_forecast_agent(input);
    let script = brain.script().clone();
    crate::mock::replay_bounded(&brain, &script, crate::mock::MOCK_MAX_STEPS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defaults::{assert_no_silent_loosening, requires_approval_default};
    use myelin_agent::EffectKind;

    struct Catalogue {
        defs: Vec<ToolDef>,
    }
    impl ToolSurface for Catalogue {
        fn register_tool(&mut self, def: ToolDef) {
            self.defs.push(def);
        }
        fn resolve(&self, name: &ToolName) -> Option<&ToolDef> {
            self.defs.iter().find(|d| &d.name == name)
        }
    }

    #[test]
    fn full_catalogue_registers_all_twelve_arch_8_tools() {
        let defs = full_issues_tool_defs();
        let names: Vec<&str> = defs.iter().map(|d| d.name.0.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "create",
                "update",
                "comment",
                "link",
                "estimate",
                "reorder",
                "assign",
                "close",
                "forecast",
                "triage",
                "sla_draft",
                "transition",
            ],
            "the FULL arch-§8 Issues catalogue, in order (8 CRUD + 4 agent)"
        );

        let mut cat = Catalogue { defs: vec![] };
        let registered = register_full_issues_tools(&mut cat).expect("seeded defs admit");
        assert_eq!(registered.len(), 12);
        for name in &names {
            assert!(
                cat.resolve(&ToolName(name.to_string())).is_some(),
                "{name} registered into the ONE surface"
            );
        }
        assert!(cat.resolve(&ToolName("delete".into())).is_none());
    }

    #[test]
    fn exactly_close_and_transition_are_gated_by_the_frozen_default() {
        let defs = full_issues_tool_defs();
        let gated: Vec<&str> = defs
            .iter()
            .filter(|d| d.requires_approval)
            .map(|d| d.name.0.as_str())
            .collect();
        assert_eq!(
            gated,
            vec!["close", "transition"],
            "only close + the SLA-bound transition are gated; the rest are advisory/reversible"
        );
        for d in &defs {
            assert_eq!(
                d.requires_approval,
                requires_approval_default(&d.subsystem, &d.name.0),
                "{}.{} gating IS the frozen §6.3 seed",
                d.subsystem,
                d.name.0
            );
            assert_eq!(d.effect_kind, EffectKind::Mutate);
            assert!(d.side_effecting);
            assert_eq!(d.version, ISSUES_TOOL_VERSION);
            assert!(!d.exposed_over_mcp, "v1 Issues tools are not MCP-exposed");
        }
    }

    #[test]
    fn crud_caps_are_the_issues_rebac_fragment_permissions() {
        assert_eq!(create_tool_def().required_caps, vec!["issue.create"]);
        assert_eq!(update_tool_def().required_caps, vec!["issue.update"]);
        assert_eq!(comment_tool_def().required_caps, vec!["issue.comment"]);
        assert_eq!(link_tool_def().required_caps, vec!["issue.update"]);
        assert_eq!(estimate_tool_def().required_caps, vec!["issue.update"]);
        assert_eq!(reorder_tool_def().required_caps, vec!["issue.update"]);
        assert_eq!(assign_tool_def().required_caps, vec!["issue.transition"]);
        assert_eq!(close_tool_def().required_caps, vec!["issue.transition"]);
        assert_eq!(issue_objects::ISSUE, "issue");
    }

    #[test]
    fn a_hand_loosened_close_registration_is_rejected_loud() {
        let mut loosened = close_tool_def();
        loosened.requires_approval = false;
        let err = assert_no_silent_loosening(&loosened, &[]).unwrap_err();
        assert_eq!(err.subsystem, "issues");
        assert_eq!(err.tool, "close");
        assert!(err.to_string().contains("WITHOUT a written deviation"));
    }

    #[test]
    fn linear_forecast_is_ceil_remaining_over_velocity() {
        let f = LinearForecast::forecast(&ForecastInput {
            remaining: 100,
            velocity_per_period: 10,
            at_risk_threshold_periods: 12,
        });
        assert_eq!(f.periods_to_completion, Some(10));
        assert!(!f.at_risk, "10 ≤ 12 → not at-risk");

        let f2 = LinearForecast::forecast(&ForecastInput {
            remaining: 101,
            velocity_per_period: 10,
            at_risk_threshold_periods: 10,
        });
        assert_eq!(
            f2.periods_to_completion,
            Some(11),
            "a partial period rounds up"
        );
        assert!(f2.at_risk, "11 > 10 → at-risk");

        let f3 = LinearForecast::forecast(&ForecastInput {
            remaining: 50,
            velocity_per_period: 0,
            at_risk_threshold_periods: 5,
        });
        assert_eq!(
            f3.periods_to_completion, None,
            "velocity 0 → no defensible date"
        );
        assert!(f3.at_risk, "no velocity → at-risk (the worst case)");
    }

    #[test]
    fn ag_d9_forecast_agent_replay_is_byte_identical() {
        let input = ForecastInput {
            remaining: 100,
            velocity_per_period: 10,
            at_risk_threshold_periods: 12,
        };
        let a = replay_forecast_agent(&input);
        let b = replay_forecast_agent(&input);
        assert_eq!(a, b, "AG-D9: two forecast-agent replays are byte-identical");
        assert!(
            a.terminated,
            "the forecast agent terminates (a single Submit)"
        );
        let s = a.submission.expect("the forecast agent submits");
        assert!(
            s.0.contains("forecast(linear): ~10 period(s)"),
            "the submission carries the linear forecast: {}",
            s.0
        );
    }

    #[test]
    fn ag_d9_triage_suggestion_strip_is_byte_identical_and_proposes_one_effect() {
        let a = triage_suggestion_strip("ENG-42");
        let b = triage_suggestion_strip("ENG-42");
        assert_eq!(
            a, b,
            "AG-D9: two triage dry-run strips are byte-identical (effect-sequence determinism)"
        );
        assert_eq!(a.len(), 1, "the triage agent proposes one advisory effect");
        let carrier = &a[0].0;
        assert!(
            carrier.contains("tool=triage"),
            "the proposed effect is triage: {carrier}"
        );
        assert!(carrier.contains("ENG-42"), "for the named issue: {carrier}");
    }
}

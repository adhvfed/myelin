use myelin_agent::{ToolDef, ToolSurface};
use myelin_identity::{CaveatContext, TransitionId};
use myelin_issues::rebac_fragment::object_types as issue_objects;
use myelin_tenancy::ArtifactRef;

use crate::defaults::{cap, mutate_tool_def, register_tool_defs, LooseningViolation};

pub const ISSUES_SUBSYSTEM: &str = "issues";

pub const FORECAST_TOOL: &str = "forecast";

pub const TRIAGE_TOOL: &str = "triage";

pub const SLA_DRAFT_TOOL: &str = "sla_draft";

pub const TRANSITION_TOOL: &str = "transition";

pub const ISSUES_TOOL_VERSION: u32 = 1;

pub fn advisory_required_caps() -> Vec<String> {
    cap(issue_objects::ISSUE, "transition")
}

pub fn transition_required_caps() -> Vec<String> {
    cap(issue_objects::ISSUE_TRANSITION, "perform_transition")
}

fn advisory_tool_def(name: &str, input_schema: &str) -> ToolDef {
    mutate_tool_def(
        ISSUES_SUBSYSTEM,
        name,
        ISSUES_TOOL_VERSION,
        input_schema,
        advisory_required_caps(),
    )
}

pub fn forecast_tool_def() -> ToolDef {
    advisory_tool_def(
        FORECAST_TOOL,
        r#"{"type":"object","required":["issue"],"properties":{"issue":{"type":"string"},"horizon_days":{"type":"integer"}}}"#,
    )
}

pub fn triage_tool_def() -> ToolDef {
    advisory_tool_def(
        TRIAGE_TOOL,
        r#"{"type":"object","required":["issue"],"properties":{"issue":{"type":"string"},"priority":{"type":"string"},"labels":{"type":"array"}}}"#,
    )
}

pub fn sla_draft_tool_def() -> ToolDef {
    advisory_tool_def(
        SLA_DRAFT_TOOL,
        r#"{"type":"object","required":["issue"],"properties":{"issue":{"type":"string"},"sla_class":{"type":"string"}}}"#,
    )
}

pub fn transition_tool_def() -> ToolDef {
    mutate_tool_def(
        ISSUES_SUBSYSTEM,
        TRANSITION_TOOL,
        ISSUES_TOOL_VERSION,
        r#"{"type":"object","required":["issue","to_state"],"properties":{"issue":{"type":"string"},"to_state":{"type":"string"},"approver":{"type":"string"}}}"#,
        transition_required_caps(),
    )
}

pub fn transition_caveat(
    issue_object: ArtifactRef,
    transition_id: impl Into<String>,
) -> CaveatContext {
    CaveatContext {
        object: issue_object,
        field: None,
        transition: Some(TransitionId(transition_id.into())),
        attrs: std::collections::BTreeMap::new(),
    }
}

pub fn issues_tool_defs() -> Vec<ToolDef> {
    vec![
        forecast_tool_def(),
        triage_tool_def(),
        sla_draft_tool_def(),
        transition_tool_def(),
    ]
}

pub fn register_issues_tools<S: ToolSurface>(
    surface: &mut S,
) -> Result<Vec<ToolDef>, LooseningViolation> {
    register_tool_defs(surface, issues_tool_defs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defaults::{assert_no_silent_loosening, requires_approval_default};
    use myelin_agent::{EffectKind, ToolName};

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
    fn forecast_triage_sla_draft_are_advisory_not_gated() {
        for (def, tool) in [
            (forecast_tool_def(), FORECAST_TOOL),
            (triage_tool_def(), TRIAGE_TOOL),
            (sla_draft_tool_def(), SLA_DRAFT_TOOL),
        ] {
            assert!(
                !def.requires_approval,
                "issues.{tool} is advisory → NOT gated (§6.3)"
            );
            assert_eq!(
                def.requires_approval,
                requires_approval_default(ISSUES_SUBSYSTEM, tool),
                "issues.{tool}'s (non-)gating IS the frozen §6.3 default (seeded, not hand-set)"
            );
            assert_eq!(def.effect_kind, EffectKind::Mutate);
            assert!(def.side_effecting);
        }
    }

    #[test]
    fn transition_is_the_gated_floor_by_the_frozen_default() {
        let def = transition_tool_def();
        assert!(
            def.requires_approval,
            "issues.transition is the gated floor (§6.3 SLA-bound transition)"
        );
        assert_eq!(
            def.requires_approval,
            requires_approval_default(ISSUES_SUBSYSTEM, TRANSITION_TOOL),
            "issues.transition's gating IS the frozen §6.3 default (the conservative floor)"
        );
        assert_eq!(def.effect_kind, EffectKind::Mutate);
        assert!(def.side_effecting);
    }

    #[test]
    fn required_caps_are_the_issues_rebac_fragment_permissions() {
        assert_eq!(
            forecast_tool_def().required_caps,
            vec!["issue.transition".to_string()]
        );
        assert_eq!(
            transition_tool_def().required_caps,
            vec!["issue_transition.perform_transition".to_string()]
        );
        assert_eq!(issue_objects::ISSUE, "issue");
        assert_eq!(issue_objects::ISSUE_TRANSITION, "issue_transition");
    }

    #[test]
    fn transition_caveat_carries_the_transition_id_not_a_field() {
        let caveat = transition_caveat(
            ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
            "issue:42:open->done",
        );
        assert_eq!(
            caveat.transition,
            Some(TransitionId("issue:42:open->done".into())),
            "the caveat carries the SLA-bound transition the approver-edge ABAC gates"
        );
        assert!(
            caveat.field.is_none(),
            "this is a transition-level caveat, not a field-level one"
        );
        assert_eq!(
            caveat.object,
            ArtifactRef("myelin://acme/issues/issue/PROJ-1".into()),
            "the caveat carries the target issue object the check resolves against"
        );
        assert!(
            caveat.attrs.is_empty(),
            "attrs is empty at this seam (the predicate evaluator is Identity's)"
        );
    }

    #[test]
    fn register_issues_tools_registers_all_four_into_the_one_surface() {
        let mut cat = Catalogue { defs: vec![] };
        let registered = register_issues_tools(&mut cat).expect("seeded defs always admit");
        assert_eq!(
            registered.len(),
            4,
            "forecast + triage + sla_draft + transition"
        );

        let transition = cat
            .resolve(&ToolName(TRANSITION_TOOL.into()))
            .expect("transition registered");
        assert_eq!(transition.subsystem, ISSUES_SUBSYSTEM);
        assert!(
            transition.requires_approval,
            "the registered transition is the gated floor"
        );
        assert_eq!(
            transition.required_caps,
            vec!["issue_transition.perform_transition".to_string()]
        );

        let forecast = cat
            .resolve(&ToolName(FORECAST_TOOL.into()))
            .expect("forecast registered");
        assert!(
            !forecast.requires_approval,
            "the registered forecast is advisory (NOT gated)"
        );

        assert!(cat.resolve(&ToolName("issues.delete".into())).is_none());
    }

    #[test]
    fn a_hand_loosened_transition_registration_is_rejected_loud() {
        let mut loosened = transition_tool_def();
        loosened.requires_approval = false;
        let err = assert_no_silent_loosening(&loosened, &[]).unwrap_err();
        assert_eq!(err.subsystem, "issues");
        assert_eq!(err.tool, "transition");
        assert!(
            err.to_string().contains("WITHOUT a written deviation"),
            "the loosening is surfaced LOUD: {err}"
        );
    }

    #[test]
    fn the_issues_tools_are_a_projection_not_a_new_engine() {
        let defs = issues_tool_defs();
        assert_eq!(defs.len(), 4);
        for d in &defs {
            assert_eq!(
                d.effect_kind,
                EffectKind::Mutate,
                "every Issues consumer tool routes through EffectApi - no new path"
            );
            assert!(d.side_effecting);
            assert_eq!(
                d.requires_approval,
                requires_approval_default(&d.subsystem, &d.name.0),
                "{}.{} gating is the frozen §6.3 seed",
                d.subsystem,
                d.name.0
            );
        }
        let gated: Vec<&str> = defs
            .iter()
            .filter(|d| d.requires_approval)
            .map(|d| d.name.0.as_str())
            .collect();
        assert_eq!(
            gated,
            vec!["transition"],
            "only the SLA-bound transition is gated; forecast/triage/sla_draft are advisory"
        );
    }
}

use myelin_agent::{ToolDef, ToolSurface};
use myelin_issues::rebac_fragment::object_types as issue_objects;

use crate::defaults::{cap, mutate_tool_def, register_tool_defs, LooseningViolation};
use crate::issues_tools::{issues_tool_defs, ISSUES_SUBSYSTEM, ISSUES_TOOL_VERSION};

pub const CREATE_TOOL: &str = "create";
pub const UPDATE_TOOL: &str = "update";
pub const COMMENT_TOOL: &str = "comment";
pub const LINK_TOOL: &str = "link";
pub const ESTIMATE_TOOL: &str = "estimate";
pub const REORDER_TOOL: &str = "reorder";
pub const ASSIGN_TOOL: &str = "assign";
pub const CLOSE_TOOL: &str = "close";
pub const CREATE_TOOL_VERSION: u32 = 2;

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
    mutate_tool_def(
        ISSUES_SUBSYSTEM,
        name,
        ISSUES_TOOL_VERSION,
        input_schema,
        caps,
    )
}

fn create_tool_def_v1() -> ToolDef {
    let mut definition = crud_tool_def(
        CREATE_TOOL,
        create_required_caps(),
        r#"{"type":"object","required":["project_id","title"],"properties":{"project_id":{"type":"string","pattern":"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"},"type_id":{"type":"string","pattern":"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"},"prefix":{"type":"string","pattern":"^[A-Z][A-Z0-9]{1,9}$"},"title":{"type":"string","minLength":1,"maxLength":512}},"additionalProperties":false}"#,
    );
    definition.exposed_over_mcp = true;
    definition
}

pub fn create_tool_def() -> ToolDef {
    let mut definition = mutate_tool_def(
        ISSUES_SUBSYSTEM,
        CREATE_TOOL,
        CREATE_TOOL_VERSION,
        r#"{"type":"object","required":["project_ref","title"],"properties":{"project_ref":{"type":"string","pattern":"^myelin://[^/]+/identity/project/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"},"title":{"type":"string","minLength":1,"maxLength":512}},"additionalProperties":false}"#,
        create_required_caps(),
    );
    definition.exposed_over_mcp = true;
    definition
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
    let mut definition = crud_tool_def(
        CLOSE_TOOL,
        assign_required_caps(),
        r#"{"type":"object","required":["issue_ref"],"properties":{"issue_ref":{"type":"string","pattern":"^myelin://[^/]+/issue/issue/[A-Z][A-Z0-9]{1,9}-[1-9][0-9]*$"}},"additionalProperties":false}"#,
    );
    definition.exposed_over_mcp = true;
    definition
}

pub fn full_issues_tool_defs() -> Vec<ToolDef> {
    let mut defs = vec![
        create_tool_def_v1(),
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
    fn full_catalogue_keeps_old_contracts_beside_the_twelve_arch_8_tools() {
        let defs = full_issues_tool_defs();
        let cursors = defs
            .iter()
            .map(|definition| format!("{}.v{}", definition.canonical_name(), definition.version))
            .collect::<Vec<_>>();
        assert_eq!(
            cursors,
            vec![
                "issues.create.v1",
                "issues.create.v2",
                "issues.update.v1",
                "issues.comment.v1",
                "issues.link.v1",
                "issues.estimate.v1",
                "issues.reorder.v1",
                "issues.assign.v1",
                "issues.close.v1",
                "issues.forecast.v1",
                "issues.triage.v1",
                "issues.sla_draft.v1",
                "issues.transition.v1",
            ],
            "the full arch-§8 Issues catalogue, plus the durable create v1 contract"
        );

        let mut cat = Catalogue { defs: vec![] };
        let registered = register_full_issues_tools(&mut cat).expect("seeded defs admit");
        assert_eq!(registered.len(), 13);
        for name in cursors.iter().filter_map(|cursor| cursor.split('.').nth(1)) {
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
            if d.canonical_name() == "issues.create" {
                assert!([ISSUES_TOOL_VERSION, CREATE_TOOL_VERSION].contains(&d.version));
            } else {
                assert_eq!(d.version, ISSUES_TOOL_VERSION);
            }
            assert_eq!(
                d.exposed_over_mcp,
                matches!(
                    d.canonical_name().as_str(),
                    "issues.create" | "issues.close"
                ),
                "only the implemented Issues mutations are MCP-exposed"
            );
        }
    }

    #[test]
    fn create_uses_a_canonical_project_reference_without_hidden_ids() {
        let definition = create_tool_def();
        let schema: serde_json::Value = serde_json::from_str(&definition.input_schema).unwrap();
        assert_eq!(definition.canonical_name(), "issues.create");
        assert_eq!(definition.version, CREATE_TOOL_VERSION);
        assert!(definition.exposed_over_mcp);
        assert_eq!(
            schema["required"],
            serde_json::json!(["project_ref", "title"])
        );
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["title"]["maxLength"], 512);
        assert!(schema["properties"].get("project_id").is_none());
        assert!(schema["properties"].get("type_id").is_none());
        assert!(schema["properties"].get("prefix").is_none());
    }

    #[test]
    fn create_v1_remains_available_for_already_activated_agents() {
        let definition = create_tool_def_v1();
        let schema: serde_json::Value = serde_json::from_str(&definition.input_schema).unwrap();
        assert_eq!(definition.canonical_name(), "issues.create");
        assert_eq!(definition.version, ISSUES_TOOL_VERSION);
        assert!(definition.exposed_over_mcp);
        assert_eq!(
            schema["required"],
            serde_json::json!(["project_id", "title"])
        );
        assert!(schema["properties"].get("project_ref").is_none());
    }

    #[test]
    fn close_accepts_one_canonical_issue_reference_and_requires_approval() {
        let definition = close_tool_def();
        let schema: serde_json::Value = serde_json::from_str(&definition.input_schema).unwrap();
        assert_eq!(definition.canonical_name(), "issues.close");
        assert!(definition.exposed_over_mcp);
        assert!(definition.requires_approval);
        assert_eq!(schema["required"], serde_json::json!(["issue_ref"]));
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(
            schema["properties"]["issue_ref"]["pattern"],
            "^myelin://[^/]+/issue/issue/[A-Z][A-Z0-9]{1,9}-[1-9][0-9]*$"
        );
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
}

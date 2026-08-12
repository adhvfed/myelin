use myelin_agent::{EffectKind, ToolDef, ToolName};

use crate::issues_tools::{ISSUES_SUBSYSTEM, ISSUES_TOOL_VERSION};

pub const LIST_ISSUES_TOOL: &str = "list";
pub const VIEW_ISSUE_TOOL: &str = "view";
pub const VIEW_ISSUE_TOOL_VERSION: u32 = 2;

fn read_tool(name: &str, version: u32, input_schema: &str) -> ToolDef {
    ToolDef {
        name: ToolName(name.into()),
        subsystem: ISSUES_SUBSYSTEM.into(),
        version,
        input_schema: input_schema.into(),
        required_caps: vec!["issue.view".into()],
        effect_kind: EffectKind::Read,
        side_effecting: false,
        requires_approval: false,
        exposed_over_mcp: true,
    }
}

pub fn issues_read_tool_defs() -> Vec<ToolDef> {
    vec![
        read_tool(
            LIST_ISSUES_TOOL,
            ISSUES_TOOL_VERSION,
            r#"{"type":"object","properties":{"state":{"type":"string","enum":["open","closed","all"]},"key":{"type":"string","minLength":1,"maxLength":32,"pattern":"^[A-Za-z0-9-]+$"},"limit":{"type":"integer","minimum":1,"maximum":100},"cursor":{"type":"string","minLength":1,"maxLength":192}},"additionalProperties":false}"#,
        ),
        read_tool(
            VIEW_ISSUE_TOOL,
            ISSUES_TOOL_VERSION,
            r#"{"type":"object","required":["issue_id"],"properties":{"issue_id":{"type":"string","pattern":"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"}},"additionalProperties":false}"#,
        ),
        read_tool(
            VIEW_ISSUE_TOOL,
            VIEW_ISSUE_TOOL_VERSION,
            r#"{"type":"object","required":["issue_ref"],"properties":{"issue_ref":{"type":"string","pattern":"^myelin://[^/]+/issue/issue/[A-Z][A-Z0-9]{1,9}-[1-9][0-9]*$"}},"additionalProperties":false}"#,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_reads_keep_old_contracts_beside_the_reference_native_surface() {
        let definitions = issues_read_tool_defs();
        assert_eq!(
            definitions
                .iter()
                .map(|definition| {
                    format!("{}.v{}", definition.canonical_name(), definition.version)
                })
                .collect::<Vec<_>>(),
            ["issues.list.v1", "issues.view.v1", "issues.view.v2"]
        );
        for definition in &definitions {
            definition.validate().unwrap();
            assert_eq!(definition.required_caps, ["issue.view"]);
            assert_eq!(definition.effect_kind, EffectKind::Read);
            assert!(!definition.side_effecting);
            assert!(!definition.requires_approval);
            assert!(definition.exposed_over_mcp);
        }
        let current_view = definitions.last().unwrap();
        let schema: serde_json::Value = serde_json::from_str(&current_view.input_schema).unwrap();
        assert_eq!(current_view.version, VIEW_ISSUE_TOOL_VERSION);
        assert_eq!(schema["required"], serde_json::json!(["issue_ref"]));
        assert!(schema["properties"].get("issue_id").is_none());
    }
}

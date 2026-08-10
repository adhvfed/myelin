use myelin_agent::{EffectKind, ToolDef, ToolName};

use crate::issues_tools::{ISSUES_SUBSYSTEM, ISSUES_TOOL_VERSION};

pub const LIST_ISSUES_TOOL: &str = "list";
pub const VIEW_ISSUE_TOOL: &str = "view";

fn read_tool(name: &str, input_schema: &str) -> ToolDef {
    ToolDef {
        name: ToolName(name.into()),
        subsystem: ISSUES_SUBSYSTEM.into(),
        version: ISSUES_TOOL_VERSION,
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
            r#"{"type":"object","properties":{"state":{"type":"string","enum":["open","closed","all"]},"key":{"type":"string","minLength":1,"maxLength":32,"pattern":"^[A-Za-z0-9-]+$"},"limit":{"type":"integer","minimum":1,"maximum":100},"cursor":{"type":"string","minLength":1,"maxLength":192}},"additionalProperties":false}"#,
        ),
        read_tool(
            VIEW_ISSUE_TOOL,
            r#"{"type":"object","required":["issue_id"],"properties":{"issue_id":{"type":"string","pattern":"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"}},"additionalProperties":false}"#,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_reads_are_exactly_the_real_mcp_surface() {
        let definitions = issues_read_tool_defs();
        assert_eq!(
            definitions
                .iter()
                .map(ToolDef::canonical_name)
                .collect::<Vec<_>>(),
            ["issues.list", "issues.view"]
        );
        for definition in definitions {
            definition.validate().unwrap();
            assert_eq!(definition.required_caps, ["issue.view"]);
            assert_eq!(definition.effect_kind, EffectKind::Read);
            assert!(!definition.side_effecting);
            assert!(!definition.requires_approval);
            assert!(definition.exposed_over_mcp);
        }
    }
}

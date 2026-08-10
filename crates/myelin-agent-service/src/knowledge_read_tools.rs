use myelin_agent::{EffectKind, ToolDef, ToolName};

use crate::knowledge_tools::{KNOWLEDGE_SUBSYSTEM, KNOWLEDGE_TOOL_VERSION};

pub const LIST_PAGES_TOOL: &str = "list_pages";
pub const READ_PAGE_TOOL: &str = "read_page";

fn read_tool(name: &str, input_schema: &str) -> ToolDef {
    ToolDef {
        name: ToolName(name.into()),
        subsystem: KNOWLEDGE_SUBSYSTEM.into(),
        version: KNOWLEDGE_TOOL_VERSION,
        input_schema: input_schema.into(),
        required_caps: vec!["knowledge.read".into()],
        effect_kind: EffectKind::Read,
        side_effecting: false,
        requires_approval: false,
        exposed_over_mcp: true,
    }
}

pub fn knowledge_read_tool_defs() -> Vec<ToolDef> {
    vec![
        read_tool(
            LIST_PAGES_TOOL,
            r#"{"type":"object","properties":{"limit":{"type":"integer","minimum":1,"maximum":100},"cursor":{"type":"string","pattern":"^[0-9A-HJKMNP-TV-Z]{26}$"}},"additionalProperties":false}"#,
        ),
        read_tool(
            READ_PAGE_TOOL,
            r#"{"type":"object","required":["page_id"],"properties":{"page_id":{"type":"string","pattern":"^[0-9A-HJKMNP-TV-Z]{26}$"}},"additionalProperties":false}"#,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knowledge_reads_are_the_small_permission_checked_mcp_surface() {
        let definitions = knowledge_read_tool_defs();
        assert_eq!(
            definitions
                .iter()
                .map(ToolDef::canonical_name)
                .collect::<Vec<_>>(),
            ["knowledge.list_pages", "knowledge.read_page"]
        );
        for definition in definitions {
            definition.validate().unwrap();
            assert_eq!(definition.required_caps, ["knowledge.read"]);
            assert_eq!(definition.effect_kind, EffectKind::Read);
            assert!(!definition.side_effecting);
            assert!(!definition.requires_approval);
            assert!(definition.exposed_over_mcp);
        }
    }
}

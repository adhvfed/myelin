use myelin_agent::{EffectKind, ToolDef, ToolName};

use crate::chat_tools::{CHAT_SUBSYSTEM, CHAT_TOOL_VERSION};

pub const LIST_CONVERSATIONS_TOOL: &str = "list_conversations";
pub const READ_MESSAGES_TOOL: &str = "read_messages";

fn read_tool(name: &str, input_schema: &str) -> ToolDef {
    ToolDef {
        name: ToolName(name.into()),
        subsystem: CHAT_SUBSYSTEM.into(),
        version: CHAT_TOOL_VERSION,
        input_schema: input_schema.into(),
        required_caps: vec!["chat.read".into()],
        effect_kind: EffectKind::Read,
        side_effecting: false,
        requires_approval: false,
        exposed_over_mcp: true,
    }
}

pub fn chat_read_tool_defs() -> Vec<ToolDef> {
    vec![
        read_tool(
            LIST_CONVERSATIONS_TOOL,
            r#"{"type":"object","properties":{"limit":{"type":"integer","minimum":1,"maximum":100},"cursor":{"type":"string","pattern":"^[0-9A-HJKMNP-TV-Z]{26}$"}},"additionalProperties":false}"#,
        ),
        read_tool(
            READ_MESSAGES_TOOL,
            r#"{"type":"object","required":["conversation_id"],"properties":{"conversation_id":{"type":"string","pattern":"^[0-9A-HJKMNP-TV-Z]{26}$"},"limit":{"type":"integer","minimum":1,"maximum":100},"before":{"type":"string","pattern":"^[0-9A-HJKMNP-TV-Z]{26}$"}},"additionalProperties":false}"#,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_reads_are_the_small_permission_checked_mcp_surface() {
        let definitions = chat_read_tool_defs();
        assert_eq!(
            definitions
                .iter()
                .map(ToolDef::canonical_name)
                .collect::<Vec<_>>(),
            ["chat.list_conversations", "chat.read_messages"]
        );
        for definition in definitions {
            definition.validate().unwrap();
            assert_eq!(definition.required_caps, ["chat.read"]);
            assert_eq!(definition.effect_kind, EffectKind::Read);
            assert!(!definition.side_effecting);
            assert!(!definition.requires_approval);
            assert!(definition.exposed_over_mcp);
        }
    }
}

use myelin_agent::{EffectKind, ToolDef, ToolName};

pub const CHAT_SUBSYSTEM: &str = "chat";

pub const CHAT_TOOL_VERSION: u32 = 1;

pub const POST_TOOL: &str = "post";
pub const REPLY_TOOL: &str = "reply";

pub fn requires_approval_default(tool: &str) -> bool {
    !matches!(tool, POST_TOOL | REPLY_TOOL)
}

fn post_tool_def() -> ToolDef {
    ToolDef {
        name: ToolName(POST_TOOL.to_string()),
        subsystem: CHAT_SUBSYSTEM.to_string(),
        version: CHAT_TOOL_VERSION,
        input_schema: r#"{"type":"object","required":["conversation_id","content"],"properties":{"conversation_id":{"type":"string","pattern":"^[0-9A-HJKMNP-TV-Z]{26}$"},"content":{"type":"string","minLength":1,"maxLength":32768},"references":{"type":"array","maxItems":32,"items":{"type":"string","minLength":1,"maxLength":1024}}},"additionalProperties":false}"#.to_string(),
        // The Edge adapter also proves that the human delegator can see the
        // target conversation before it writes there.
        required_caps: vec!["chat.post".to_string()],
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        requires_approval: requires_approval_default(POST_TOOL),
        exposed_over_mcp: true,
    }
}

fn reply_tool_def() -> ToolDef {
    ToolDef {
        name: ToolName(REPLY_TOOL.to_string()),
        subsystem: CHAT_SUBSYSTEM.to_string(),
        version: CHAT_TOOL_VERSION,
        input_schema: r#"{"type":"object","required":["root_message_id","content"],"properties":{"root_message_id":{"type":"string","pattern":"^[0-9A-HJKMNP-TV-Z]{26}$"},"content":{"type":"string","minLength":1,"maxLength":32768},"references":{"type":"array","maxItems":32,"items":{"type":"string","minLength":1,"maxLength":1024}}},"additionalProperties":false}"#.to_string(),
        // A reply is another Chat post. Edge additionally proves that the
        // human delegator can see and post in the root's conversation.
        required_caps: vec!["chat.post".to_string()],
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        requires_approval: requires_approval_default(REPLY_TOOL),
        exposed_over_mcp: true,
    }
}

pub fn chat_tool_defs() -> Vec<ToolDef> {
    vec![post_tool_def(), reply_tool_def()]
}

#[cfg(test)]
mod tests;

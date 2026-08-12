use myelin_agent::{EffectKind, ToolDef, ToolName};

use crate::knowledge_tools::{KNOWLEDGE_SUBSYSTEM, KNOWLEDGE_TOOL_VERSION};

pub const LIST_PAGES_TOOL: &str = "list_pages";
pub const READ_PAGE_TOOL: &str = "read_page";
pub const LINK_WORK_TOOL: &str = "link_work";

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

pub fn link_work_tool_def() -> ToolDef {
    ToolDef {
        name: ToolName(LINK_WORK_TOOL.into()),
        subsystem: KNOWLEDGE_SUBSYSTEM.into(),
        version: KNOWLEDGE_TOOL_VERSION,
        input_schema: r#"{"type":"object","required":["page_id","reference"],"properties":{"page_id":{"type":"string","pattern":"^[0-9A-HJKMNP-TV-Z]{26}$"},"reference":{"type":"string","minLength":1,"maxLength":1024,"description":"Canonical myelin:// reference to related work"},"note":{"type":"string","minLength":1,"maxLength":4096,"description":"Short context shown before the related-work link"}},"additionalProperties":false}"#.into(),
        required_caps: vec!["knowledge.edit".into()],
        effect_kind: EffectKind::Mutate,
        side_effecting: true,
        requires_approval: false,
        exposed_over_mcp: true,
    }
}

pub fn knowledge_mcp_tool_defs() -> Vec<ToolDef> {
    vec![
        read_tool(
            LIST_PAGES_TOOL,
            r#"{"type":"object","properties":{"limit":{"type":"integer","minimum":1,"maximum":100},"cursor":{"type":"string","pattern":"^[0-9A-HJKMNP-TV-Z]{26}$"}},"additionalProperties":false}"#,
        ),
        read_tool(
            READ_PAGE_TOOL,
            r#"{"type":"object","required":["page_id"],"properties":{"page_id":{"type":"string","pattern":"^[0-9A-HJKMNP-TV-Z]{26}$"}},"additionalProperties":false}"#,
        ),
        link_work_tool_def(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defaults::requires_approval_default;

    #[test]
    fn knowledge_mcp_offers_small_reads_and_one_reversible_context_write() {
        let definitions = knowledge_mcp_tool_defs();
        assert_eq!(
            definitions
                .iter()
                .map(ToolDef::canonical_name)
                .collect::<Vec<_>>(),
            [
                "knowledge.list_pages",
                "knowledge.read_page",
                "knowledge.link_work",
            ]
        );
        for definition in &definitions {
            definition.validate().unwrap();
            assert!(definition.exposed_over_mcp);
            assert_eq!(
                definition.requires_approval,
                requires_approval_default(&definition.subsystem, &definition.name.0)
            );
        }

        let reads = &definitions[..2];
        assert!(reads.iter().all(|definition| {
            definition.required_caps == ["knowledge.read"]
                && definition.effect_kind == EffectKind::Read
                && !definition.side_effecting
        }));

        let link = &definitions[2];
        assert_eq!(link.required_caps, ["knowledge.edit"]);
        assert_eq!(link.effect_kind, EffectKind::Mutate);
        assert!(link.side_effecting);
        assert!(!link.requires_approval, "adding a link is reversible");
    }
}

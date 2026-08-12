use myelin_agent::{EffectKind, ToolDef, ToolName};

use crate::knowledge_tools::{KNOWLEDGE_SUBSYSTEM, KNOWLEDGE_TOOL_VERSION};

pub const LIST_PAGES_TOOL: &str = "list_pages";
pub const READ_PAGE_TOOL: &str = "read_page";
pub const READ_PAGE_TOOL_VERSION: u32 = 2;
pub const LINK_WORK_TOOL: &str = "link_work";

fn read_tool(name: &str, version: u32, input_schema: &str) -> ToolDef {
    ToolDef {
        name: ToolName(name.into()),
        subsystem: KNOWLEDGE_SUBSYSTEM.into(),
        version,
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
            KNOWLEDGE_TOOL_VERSION,
            r#"{"type":"object","properties":{"limit":{"type":"integer","minimum":1,"maximum":100},"cursor":{"type":"string","pattern":"^[0-9A-HJKMNP-TV-Z]{26}$"}},"additionalProperties":false}"#,
        ),
        read_tool(
            READ_PAGE_TOOL,
            KNOWLEDGE_TOOL_VERSION,
            r#"{"type":"object","required":["page_id"],"properties":{"page_id":{"type":"string","pattern":"^[0-9A-HJKMNP-TV-Z]{26}$"}},"additionalProperties":false}"#,
        ),
        read_tool(
            READ_PAGE_TOOL,
            READ_PAGE_TOOL_VERSION,
            r#"{"type":"object","required":["page_ref"],"properties":{"page_ref":{"type":"string","pattern":"^myelin://[^/]+/knowledge/page/[0-9A-HJKMNP-TV-Z]{26}$"}},"additionalProperties":false}"#,
        ),
        link_work_tool_def(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defaults::requires_approval_default;

    #[test]
    fn knowledge_reads_keep_old_ids_beside_the_reference_native_surface() {
        let definitions = knowledge_mcp_tool_defs();
        assert_eq!(
            definitions
                .iter()
                .map(|definition| {
                    format!("{}.v{}", definition.canonical_name(), definition.version)
                })
                .collect::<Vec<_>>(),
            [
                "knowledge.list_pages.v1",
                "knowledge.read_page.v1",
                "knowledge.read_page.v2",
                "knowledge.link_work.v1",
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

        let reads = &definitions[..3];
        assert!(reads.iter().all(|definition| {
            definition.required_caps == ["knowledge.read"]
                && definition.effect_kind == EffectKind::Read
                && !definition.side_effecting
        }));

        let current_read = &definitions[2];
        let schema: serde_json::Value = serde_json::from_str(&current_read.input_schema).unwrap();
        assert_eq!(current_read.version, READ_PAGE_TOOL_VERSION);
        assert_eq!(schema["required"], serde_json::json!(["page_ref"]));
        assert!(schema["properties"].get("page_id").is_none());

        let link = &definitions[3];
        assert_eq!(link.required_caps, ["knowledge.edit"]);
        assert_eq!(link.effect_kind, EffectKind::Mutate);
        assert!(link.side_effecting);
        assert!(!link.requires_approval, "adding a link is reversible");
    }
}

use myelin_agent::{EffectKind, ToolDef, ToolName};

pub const PROJECTS_SUBSYSTEM: &str = "projects";
pub const LIST_PROJECTS_TOOL: &str = "list";
pub const PROJECTS_TOOL_VERSION: u32 = 1;

pub fn project_read_tool_defs() -> Vec<ToolDef> {
    vec![ToolDef {
        name: ToolName(LIST_PROJECTS_TOOL.into()),
        subsystem: PROJECTS_SUBSYSTEM.into(),
        version: PROJECTS_TOOL_VERSION,
        input_schema: r#"{"type":"object","properties":{"limit":{"type":"integer","minimum":1,"maximum":100},"cursor":{"type":"string","pattern":"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"}},"additionalProperties":false}"#.into(),
        required_caps: vec!["project.view".into()],
        effect_kind: EffectKind::Read,
        side_effecting: false,
        requires_approval: false,
        exposed_over_mcp: true,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_can_discover_projects_without_hidden_identifiers() {
        let definitions = project_read_tool_defs();
        assert_eq!(definitions.len(), 1);
        let definition = &definitions[0];
        assert_eq!(definition.canonical_name(), "projects.list");
        assert_eq!(definition.required_caps, ["project.view"]);
        assert_eq!(definition.effect_kind, EffectKind::Read);
        assert!(!definition.side_effecting);
        assert!(!definition.requires_approval);
        assert!(definition.exposed_over_mcp);
        definition.validate().unwrap();
    }
}

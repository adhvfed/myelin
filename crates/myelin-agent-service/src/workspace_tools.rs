use myelin_agent::{EffectKind, ToolDef, ToolName};

use crate::defaults::seed_requires_approval;

pub const WORKSPACE_SUBSYSTEM: &str = "workspace";
pub const WORKSPACE_TOOL_VERSION: u32 = 1;
pub const READ_WORKSPACE_FILE_TOOL: &str = "read_file";
pub const WRITE_WORKSPACE_FILE_TOOL: &str = "write_file";

pub fn workspace_tool_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: ToolName(READ_WORKSPACE_FILE_TOOL.into()),
            subsystem: WORKSPACE_SUBSYSTEM.into(),
            version: WORKSPACE_TOOL_VERSION,
            input_schema: r#"{"type":"object","description":"Reads one bounded UTF-8 file from this private thread's durable workspace.","required":["path"],"properties":{"path":{"type":"string","minLength":1,"maxLength":1024}},"additionalProperties":false}"#.into(),
            required_caps: vec!["agent.run".into()],
            effect_kind: EffectKind::Read,
            side_effecting: false,
            requires_approval: false,
            exposed_over_mcp: true,
        },
        seed_requires_approval(ToolDef {
            name: ToolName(WRITE_WORKSPACE_FILE_TOOL.into()),
            subsystem: WORKSPACE_SUBSYSTEM.into(),
            version: WORKSPACE_TOOL_VERSION,
            input_schema: r#"{"type":"object","description":"Atomically writes one bounded UTF-8 file in this private thread's durable workspace.","required":["path","content"],"properties":{"path":{"type":"string","minLength":1,"maxLength":1024},"content":{"type":"string","maxLength":262144}},"additionalProperties":false}"#.into(),
            required_caps: vec!["agent.run".into()],
            effect_kind: EffectKind::Mutate,
            side_effecting: true,
            requires_approval: false,
            exposed_over_mcp: true,
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_tools_are_explicitly_selected_and_bound_to_agent_runs() {
        let definitions = workspace_tool_defs();
        assert_eq!(
            definitions
                .iter()
                .map(ToolDef::canonical_name)
                .collect::<Vec<_>>(),
            ["workspace.read_file", "workspace.write_file"]
        );
        for definition in definitions {
            definition.validate().unwrap();
            assert_eq!(definition.required_caps, ["agent.run"]);
            assert!(!definition.requires_approval);
            assert!(definition.exposed_over_mcp);
        }
    }
}

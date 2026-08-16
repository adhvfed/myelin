use myelin_agent::{EffectKind, ToolDef, ToolName};

use crate::git_tools::{GIT_SUBSYSTEM, GIT_TOOL_VERSION};

pub const LIST_REPOSITORIES_TOOL: &str = "list_repositories";
pub const READ_FILE_TOOL: &str = "read_file";
pub const SEARCH_CODE_TOOL: &str = "search_code";

fn read_tool(name: &str, input_schema: &str) -> ToolDef {
    ToolDef {
        name: ToolName(name.into()),
        subsystem: GIT_SUBSYSTEM.into(),
        version: GIT_TOOL_VERSION,
        input_schema: input_schema.into(),
        required_caps: vec!["repo.pull".into()],
        effect_kind: EffectKind::Read,
        side_effecting: false,
        requires_approval: false,
        exposed_over_mcp: true,
    }
}

pub fn git_read_tool_defs() -> Vec<ToolDef> {
    vec![
        read_tool(
            LIST_REPOSITORIES_TOOL,
            r#"{"type":"object","description":"Lists visible repositories from newest to oldest.","properties":{"limit":{"type":"integer","minimum":1,"maximum":100},"cursor":{"type":"string","description":"Opaque next_cursor from the previous page.","maxLength":512}},"additionalProperties":false}"#,
        ),
        read_tool(
            READ_FILE_TOOL,
            r#"{"type":"object","required":["repo","ref","path"],"properties":{"repo":{"type":"string","minLength":1,"maxLength":1024},"ref":{"type":"string","minLength":1,"maxLength":1024},"path":{"type":"string","minLength":1,"maxLength":4096}},"additionalProperties":false}"#,
        ),
        read_tool(
            SEARCH_CODE_TOOL,
            r#"{"type":"object","required":["query"],"properties":{"query":{"type":"string","minLength":1,"maxLength":4096},"repo":{"type":"string","minLength":1,"maxLength":1024}},"additionalProperties":false}"#,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_reads_are_the_small_permission_checked_mcp_surface() {
        let definitions = git_read_tool_defs();
        assert_eq!(
            definitions
                .iter()
                .map(ToolDef::canonical_name)
                .collect::<Vec<_>>(),
            ["git.list_repositories", "git.read_file", "git.search_code"]
        );
        assert!(
            definitions[0].input_schema.contains("newest to oldest"),
            "the agent-facing contract explains repository ordering"
        );
        for definition in definitions {
            definition.validate().unwrap();
            assert_eq!(definition.required_caps, ["repo.pull"]);
            assert_eq!(definition.effect_kind, EffectKind::Read);
            assert!(!definition.side_effecting);
            assert!(!definition.requires_approval);
            assert!(definition.exposed_over_mcp);
        }
    }
}

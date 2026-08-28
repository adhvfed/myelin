use myelin_agent::{ToolDef, ToolSurface};
use myelin_issues::rebac_fragment::object_types as issue_objects;

use crate::defaults::{cap, mutate_tool_def, register_tool_defs, LooseningViolation};

pub const ISSUES_SUBSYSTEM: &str = "issues";
pub const ISSUES_TOOL_VERSION: u32 = 1;
pub const CREATE_TOOL: &str = "create";
pub const CLOSE_TOOL: &str = "close";
pub const CREATE_TOOL_VERSION: u32 = 2;

pub fn create_required_caps() -> Vec<String> {
    cap(issue_objects::ISSUE, "create")
}

fn close_required_caps() -> Vec<String> {
    cap(issue_objects::ISSUE, "transition")
}

fn create_tool_def_v1() -> ToolDef {
    let mut definition = mutate_tool_def(
        ISSUES_SUBSYSTEM,
        CREATE_TOOL,
        ISSUES_TOOL_VERSION,
        r#"{"type":"object","required":["project_id","title"],"properties":{"project_id":{"type":"string","pattern":"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"},"type_id":{"type":"string","pattern":"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"},"prefix":{"type":"string","pattern":"^[A-Z][A-Z0-9]{1,9}$"},"title":{"type":"string","minLength":1,"maxLength":512}},"additionalProperties":false}"#,
        create_required_caps(),
    );
    definition.exposed_over_mcp = true;
    definition
}

pub fn create_tool_def() -> ToolDef {
    let mut definition = mutate_tool_def(
        ISSUES_SUBSYSTEM,
        CREATE_TOOL,
        CREATE_TOOL_VERSION,
        r#"{"type":"object","required":["project_ref","title"],"properties":{"project_ref":{"type":"string","pattern":"^myelin://[^/]+/identity/project/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"},"title":{"type":"string","minLength":1,"maxLength":512}},"additionalProperties":false}"#,
        create_required_caps(),
    );
    definition.exposed_over_mcp = true;
    definition
}

pub fn close_tool_def() -> ToolDef {
    let mut definition = mutate_tool_def(
        ISSUES_SUBSYSTEM,
        CLOSE_TOOL,
        ISSUES_TOOL_VERSION,
        r#"{"type":"object","required":["issue_ref"],"properties":{"issue_ref":{"type":"string","pattern":"^myelin://[^/]+/issue/issue/[A-Z][A-Z0-9]{1,9}-[1-9][0-9]*$"}},"additionalProperties":false}"#,
        close_required_caps(),
    );
    definition.exposed_over_mcp = true;
    definition
}

pub fn issues_mutation_tool_defs() -> Vec<ToolDef> {
    vec![create_tool_def_v1(), create_tool_def(), close_tool_def()]
}

pub fn register_issues_mutation_tools<S: ToolSurface>(
    surface: &mut S,
) -> Result<Vec<ToolDef>, LooseningViolation> {
    register_tool_defs(surface, issues_mutation_tool_defs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defaults::requires_approval_default;
    use myelin_agent::{EffectKind, ToolName};

    struct Catalogue {
        defs: Vec<ToolDef>,
    }

    impl ToolSurface for Catalogue {
        fn register_tool(&mut self, def: ToolDef) {
            self.defs.push(def);
        }

        fn resolve(&self, name: &ToolName) -> Option<&ToolDef> {
            self.defs
                .iter()
                .rev()
                .find(|definition| &definition.name == name)
        }
    }

    #[test]
    fn catalogue_contains_only_effects_with_a_running_executor() {
        let definitions = issues_mutation_tool_defs();
        assert_eq!(
            definitions
                .iter()
                .map(|definition| format!(
                    "{}.v{}",
                    definition.canonical_name(),
                    definition.version
                ))
                .collect::<Vec<_>>(),
            ["issues.create.v1", "issues.create.v2", "issues.close.v1"]
        );
        for definition in &definitions {
            definition.validate().unwrap();
            assert_eq!(definition.effect_kind, EffectKind::Mutate);
            assert!(definition.side_effecting);
            assert!(definition.exposed_over_mcp);
            assert_eq!(
                definition.requires_approval,
                requires_approval_default(&definition.subsystem, &definition.name.0)
            );
        }
    }

    #[test]
    fn registration_resolves_the_latest_create_and_close() {
        let mut catalogue = Catalogue { defs: Vec::new() };
        let registered = register_issues_mutation_tools(&mut catalogue).unwrap();
        assert_eq!(registered.len(), 3);
        assert_eq!(
            catalogue
                .resolve(&ToolName(CREATE_TOOL.into()))
                .unwrap()
                .version,
            CREATE_TOOL_VERSION
        );
        assert_eq!(
            catalogue
                .resolve(&ToolName(CLOSE_TOOL.into()))
                .unwrap()
                .version,
            ISSUES_TOOL_VERSION
        );
        assert!(catalogue.resolve(&ToolName("reorder".into())).is_none());
        assert!(catalogue.resolve(&ToolName("transition".into())).is_none());
    }

    #[test]
    fn reference_native_schemas_reject_legacy_or_surplus_coordinates() {
        let create: serde_json::Value =
            serde_json::from_str(&create_tool_def().input_schema).unwrap();
        assert_eq!(
            create["required"],
            serde_json::json!(["project_ref", "title"])
        );
        assert!(create["properties"].get("project_id").is_none());
        assert_eq!(create["additionalProperties"], false);

        let close: serde_json::Value =
            serde_json::from_str(&close_tool_def().input_schema).unwrap();
        assert_eq!(close["required"], serde_json::json!(["issue_ref"]));
        assert!(close["properties"].get("issue_id").is_none());
        assert_eq!(close["additionalProperties"], false);
    }
}

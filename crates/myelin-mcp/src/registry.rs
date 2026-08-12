use std::collections::BTreeSet;

use myelin_agent::{EffectKind, ToolDef};
use myelin_agent_service::{catalogue_cursor, PlatformToolCatalogue};
use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct RegisteredTool {
    name: String,
    definition: ToolDef,
}

impl RegisteredTool {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn cursor(&self) -> String {
        catalogue_cursor(&self.definition)
    }

    pub fn definition(&self) -> &ToolDef {
        &self.definition
    }

    pub fn requires_approval(&self) -> bool {
        self.definition.requires_approval
    }

    pub fn required_caps(&self) -> Vec<&str> {
        self.definition
            .required_caps
            .iter()
            .map(String::as_str)
            .collect()
    }

    pub fn effect_kind(&self) -> EffectKind {
        self.definition.effect_kind
    }

    pub fn side_effecting(&self) -> bool {
        self.definition.side_effecting
    }

    pub fn to_mcp_json(&self) -> Value {
        self.definition
            .mcp_projection()
            .expect("the platform catalogue stores only validated ToolDefs")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryError {
    Platform(String),
    DuplicateCursor(String),
    UnavailableCursor(String),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Platform(reason) => {
                write!(formatter, "platform tool catalogue refused: {reason}")
            }
            Self::UnavailableCursor(cursor) => {
                write!(
                    formatter,
                    "selected MCP tool cursor `{cursor}` is unavailable"
                )
            }
            Self::DuplicateCursor(cursor) => {
                write!(
                    formatter,
                    "selected MCP tool cursor `{cursor}` is duplicated"
                )
            }
        }
    }
}

impl std::error::Error for RegistryError {}

#[derive(Clone, Debug, Default)]
pub struct ToolRegistry {
    tools: Vec<RegisteredTool>,
}

impl ToolRegistry {
    pub fn platform() -> Result<Self, RegistryError> {
        let catalogue = platform_catalogue()?;
        Ok(Self::from_definitions(
            catalogue
                .latest_definitions()
                .into_iter()
                .filter(|definition| definition.exposed_over_mcp)
                .cloned(),
        ))
    }

    /// Materialize precisely the versioned tools selected when the external agent was activated.
    /// Capability equivalence is intentionally irrelevant: two tools sharing one grant remain two
    /// distinct user choices.
    pub fn for_cursors(cursors: &[String]) -> Result<Self, RegistryError> {
        let catalogue = platform_catalogue()?;
        let mut selected = Vec::with_capacity(cursors.len());
        let mut seen = BTreeSet::new();
        for cursor in cursors {
            if !seen.insert(cursor) {
                return Err(RegistryError::DuplicateCursor(cursor.clone()));
            }
            let definition = catalogue
                .definitions()
                .iter()
                .find(|definition| catalogue_cursor(definition) == *cursor)
                .filter(|definition| definition.exposed_over_mcp)
                .ok_or_else(|| RegistryError::UnavailableCursor(cursor.clone()))?;
            selected.push(definition.clone());
        }
        Ok(Self::from_definitions(selected))
    }

    pub fn with_git() -> ToolRegistry {
        Self::filtered_platform(|definition| definition.subsystem == "git")
            .expect("the built-in Git MCP catalogue must be valid")
    }

    pub fn with_git_and_ci_reads() -> Result<ToolRegistry, RegistryError> {
        Self::filtered_platform(|definition| {
            definition.subsystem == "git" || definition.subsystem == "ci"
        })
    }

    fn filtered_platform(
        include: impl Fn(&ToolDef) -> bool,
    ) -> Result<ToolRegistry, RegistryError> {
        let catalogue = platform_catalogue()?;
        Ok(Self::from_definitions(
            catalogue
                .latest_definitions()
                .into_iter()
                .filter(|definition| definition.exposed_over_mcp && include(definition))
                .cloned(),
        ))
    }

    fn from_definitions(definitions: impl IntoIterator<Item = ToolDef>) -> Self {
        let mut tools = definitions
            .into_iter()
            .map(|definition| RegisteredTool {
                name: definition.canonical_name(),
                definition,
            })
            .collect::<Vec<_>>();
        tools.sort_by(|left, right| left.name.cmp(&right.name));
        Self { tools }
    }

    pub fn tools(&self) -> &[RegisteredTool] {
        &self.tools
    }

    pub fn resolve(&self, name: &str) -> Option<&RegisteredTool> {
        self.tools.iter().find(|tool| tool.name() == name)
    }

    pub fn list_result(&self) -> Value {
        json!({ "tools": self.tools.iter().map(RegisteredTool::to_mcp_json).collect::<Vec<_>>() })
    }

    pub fn list_result_for(&self, permitted_names: &BTreeSet<String>) -> Value {
        json!({
            "tools": self
                .tools
                .iter()
                .filter(|tool| permitted_names.contains(tool.name()))
                .map(RegisteredTool::to_mcp_json)
                .collect::<Vec<_>>()
        })
    }
}

fn platform_catalogue() -> Result<PlatformToolCatalogue, RegistryError> {
    PlatformToolCatalogue::platform().map_err(|error| RegistryError::Platform(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_is_the_mcp_projection_of_the_shared_platform_catalogue() {
        let registry = ToolRegistry::platform().unwrap();
        let names = registry
            .tools()
            .iter()
            .map(RegisteredTool::name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "chat.list_conversations",
                "chat.post",
                "chat.read_messages",
                "ci.read_log",
                "ci.read_run",
                "git.endorse_fork_ci",
                "git.list_repositories",
                "git.merge",
                "git.open_pr",
                "git.read_file",
                "git.search_code",
                "git.submit_review",
                "git.write_file",
                "issues.close",
                "issues.create",
                "issues.list",
                "issues.view",
                "knowledge.link_work",
                "knowledge.list_pages",
                "knowledge.read_page",
                "projects.list",
            ]
        );
        assert!(registry.resolve("git.merge").unwrap().requires_approval());
        assert_eq!(
            registry.resolve("git.open_pr").unwrap().required_caps(),
            ["repo.push"]
        );

        let read_names = registry
            .tools()
            .iter()
            .filter(|tool| tool.effect_kind() == EffectKind::Read)
            .map(RegisteredTool::name)
            .collect::<Vec<_>>();
        assert_eq!(
            read_names,
            crate::governance::GOVERNED_DIRECT_READ_TOOLS,
            "adding a direct read also requires a durable governance-audit route"
        );
    }

    #[test]
    fn a_versioned_selection_never_expands_to_a_capability_equivalent_tool() {
        let registry = ToolRegistry::for_cursors(&["ci.read_run.v1".into()]).unwrap();
        assert!(registry.resolve("ci.read_run").is_some());
        assert!(
            registry.resolve("ci.read_log").is_none(),
            "sharing `run.view` is not consent to a second tool"
        );
        assert!(matches!(
            ToolRegistry::for_cursors(&["ci.missing.v1".into()]),
            Err(RegistryError::UnavailableCursor(_))
        ));
    }

    #[test]
    fn old_agents_keep_uuid_contracts_while_new_selections_receive_reference_contracts() {
        let old = ToolRegistry::for_cursors(&["issues.create.v1".into(), "issues.view.v1".into()])
            .unwrap();
        let current =
            ToolRegistry::for_cursors(&["issues.create.v2".into(), "issues.view.v2".into()])
                .unwrap();

        let old_create = old.resolve("issues.create").unwrap();
        let current_create = current.resolve("issues.create").unwrap();
        assert_eq!(old_create.cursor(), "issues.create.v1");
        assert_eq!(current_create.cursor(), "issues.create.v2");
        assert!(old_create.definition().input_schema.contains("project_id"));
        assert!(current_create
            .definition()
            .input_schema
            .contains("project_ref"));
        let old_view = old.resolve("issues.view").unwrap();
        let current_view = current.resolve("issues.view").unwrap();
        assert_eq!(old_view.cursor(), "issues.view.v1");
        assert_eq!(current_view.cursor(), "issues.view.v2");
        assert!(old_view.definition().input_schema.contains("issue_id"));
        assert!(current_view.definition().input_schema.contains("issue_ref"));
    }

    #[test]
    fn git_and_ci_contracts_preserve_the_provider_schemas_exactly() {
        let registry = ToolRegistry::with_git_and_ci_reads().unwrap();
        let catalogue = PlatformToolCatalogue::platform().unwrap();
        for tool in registry.tools() {
            let provider = catalogue.resolve(tool.name()).unwrap();
            assert_eq!(tool.cursor(), catalogue_cursor(provider));
            assert_eq!(
                tool.to_mcp_json()["inputSchema"],
                serde_json::from_str::<Value>(&provider.input_schema).unwrap()
            );
        }
    }

    #[test]
    fn a_corrupt_durable_selection_with_duplicate_cursors_fails_closed() {
        assert!(matches!(
            ToolRegistry::for_cursors(&["ci.read_run.v1".into(), "ci.read_run.v1".into()]),
            Err(RegistryError::DuplicateCursor(cursor)) if cursor == "ci.read_run.v1"
        ));
    }
}

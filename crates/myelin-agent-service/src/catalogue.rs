use std::collections::{BTreeMap, BTreeSet};

use myelin_agent::{ToolDef, ToolDefValidationError};
use myelin_tenancy::{ArtifactRef, TenantId};
use serde_json::{json, Value};

use crate::defaults::{assert_no_silent_loosening, LooseningViolation};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolCatalogueError {
    InvalidDefinition(ToolDefValidationError),
    UnsafeDefault(LooseningViolation),
    DuplicateVersion { name: String, version: u32 },
}

impl core::fmt::Display for ToolCatalogueError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidDefinition(error) => write!(f, "{error}"),
            Self::UnsafeDefault(error) => write!(f, "{error}"),
            Self::DuplicateVersion { name, version } => {
                write!(f, "duplicate tool definition `{name}` version {version}")
            }
        }
    }
}

impl std::error::Error for ToolCatalogueError {}

#[derive(Clone, Debug)]
pub struct PlatformToolCatalogue {
    definitions: Vec<ToolDef>,
}

impl PlatformToolCatalogue {
    pub fn platform() -> Result<Self, ToolCatalogueError> {
        let git_mcp = myelin_git::api::agent_tool_defs();
        let mcp_names = git_mcp
            .iter()
            .map(ToolDef::canonical_name)
            .collect::<BTreeSet<_>>();

        let mut definitions = crate::git_tool_defs()
            .into_iter()
            .filter(|definition| !mcp_names.contains(&definition.canonical_name()))
            .collect::<Vec<_>>();
        definitions.extend(git_mcp);
        definitions.extend(crate::git_read_tool_defs());
        definitions.extend(myelin_ci_controlplane::ci_tool_defs());
        definitions.extend(crate::issues_read_tool_defs());
        definitions.extend(crate::full_issues_tool_defs());
        definitions.extend(crate::knowledge_read_tool_defs());
        definitions.extend(crate::knowledge_tool_defs());
        definitions.extend(crate::chat_read_tool_defs());
        definitions.extend(myelin_chat::tools::chat_tool_defs());
        Self::try_from_definitions(definitions)
    }

    pub fn try_from_definitions(
        definitions: impl IntoIterator<Item = ToolDef>,
    ) -> Result<Self, ToolCatalogueError> {
        let mut by_version = BTreeMap::new();
        for definition in definitions {
            definition
                .validate()
                .map_err(ToolCatalogueError::InvalidDefinition)?;
            assert_no_silent_loosening(&definition, &[])
                .map_err(ToolCatalogueError::UnsafeDefault)?;
            let name = definition.canonical_name();
            let version = definition.version;
            let key = (name.clone(), version);
            if by_version.insert(key, definition).is_some() {
                return Err(ToolCatalogueError::DuplicateVersion { name, version });
            }
        }
        Ok(Self {
            definitions: by_version.into_values().collect(),
        })
    }

    pub fn definitions(&self) -> &[ToolDef] {
        &self.definitions
    }

    pub fn resolve(&self, canonical_name: &str) -> Option<&ToolDef> {
        self.definitions
            .iter()
            .rev()
            .find(|definition| definition.canonical_name() == canonical_name)
    }

    pub fn latest_definitions(&self) -> Vec<&ToolDef> {
        let mut seen = BTreeSet::new();
        let mut latest = self
            .definitions
            .iter()
            .rev()
            .filter(|definition| seen.insert(definition.canonical_name()))
            .collect::<Vec<_>>();
        latest.reverse();
        latest
    }

    pub fn mcp_manifest(&self) -> Value {
        self.mcp_manifest_for(|_| true)
    }

    pub fn mcp_manifest_for(&self, permitted: impl Fn(&ToolDef) -> bool) -> Value {
        json!({
            "tools": self
                .latest_definitions()
                .into_iter()
                .filter(|definition| definition.exposed_over_mcp)
                .filter(|definition| permitted(definition))
                .map(|definition| {
                    definition
                        .mcp_projection()
                        .expect("the platform catalogue stores only validated ToolDefs")
                })
                .collect::<Vec<_>>()
        })
    }
}

pub fn catalogue_cursor(definition: &ToolDef) -> String {
    format!("{}.v{}", definition.canonical_name(), definition.version)
}

pub fn tool_ref(tenant: &TenantId, definition: &ToolDef) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{}/agent/tool/{}/v{}",
        tenant.0,
        definition.canonical_name(),
        definition.version
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_platform_catalogue_is_one_sorted_validated_cross_subsystem_surface() {
        let catalogue = PlatformToolCatalogue::platform().unwrap();
        assert_eq!(catalogue.definitions().len(), 54);
        let keys = catalogue
            .definitions()
            .iter()
            .map(catalogue_cursor)
            .collect::<Vec<_>>();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        assert_eq!(keys.iter().collect::<BTreeSet<_>>().len(), keys.len());
        for definition in catalogue.definitions() {
            definition.validate().unwrap();
        }
    }

    #[test]
    fn subsystem_providers_drive_the_catalogue_and_the_mcp_projection() {
        let catalogue = PlatformToolCatalogue::platform().unwrap();
        let merge = catalogue.resolve("git.merge").unwrap();
        assert!(merge.exposed_over_mcp);
        assert!(merge.requires_approval);
        let chat_archive = catalogue.resolve("chat.archive_channel").unwrap();
        assert!(chat_archive.requires_approval);
        let ci_read = catalogue.resolve("ci.read_run").unwrap();
        assert_eq!(ci_read, &myelin_ci_controlplane::ci_tool_def("read_run"));

        let manifest = catalogue.mcp_manifest();
        let names = manifest["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<BTreeSet<_>>();
        for expected in [
            "chat.list_conversations",
            "chat.post",
            "chat.read_messages",
            "git.list_repositories",
            "git.read_file",
            "git.search_code",
            "git.merge",
            "git.open_pr",
            "git.write_file",
            "ci.read_run",
            "ci.read_log",
            "issues.list",
            "issues.view",
            "knowledge.list_pages",
            "knowledge.read_page",
        ] {
            assert!(names.contains(expected), "manifest omitted {expected}");
        }
        assert!(!names.contains("knowledge.publish"));
    }

    #[test]
    fn the_mcp_projection_publishes_only_the_latest_exposed_version_of_a_tool() {
        let version_one = myelin_git::api::agent_tool_defs().remove(0);
        let canonical_name = version_one.canonical_name();
        let mut version_two = version_one.clone();
        version_two.version = 2;
        let catalogue =
            PlatformToolCatalogue::try_from_definitions([version_one, version_two]).unwrap();

        let tools = catalogue.mcp_manifest()["tools"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(tools.len(), 1);
        assert!(tools[0]["description"].as_str().unwrap().contains(" v2;"));
        assert_eq!(
            catalogue.latest_definitions(),
            vec![catalogue.resolve(&canonical_name).unwrap()]
        );

        let version_one = myelin_git::api::agent_tool_defs().remove(0);
        let mut hidden_version_two = version_one.clone();
        hidden_version_two.version = 2;
        hidden_version_two.exposed_over_mcp = false;
        let catalogue =
            PlatformToolCatalogue::try_from_definitions([version_one, hidden_version_two]).unwrap();
        assert!(catalogue.mcp_manifest()["tools"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(catalogue.mcp_manifest_for(|definition| definition
            .required_caps
            .iter()
            .any(|cap| cap == "never"))["tools"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn duplicate_or_unsafe_definitions_fail_loudly() {
        let definition = myelin_git::api::agent_tool_defs().remove(0);
        assert!(matches!(
            PlatformToolCatalogue::try_from_definitions([definition.clone(), definition.clone()]),
            Err(ToolCatalogueError::DuplicateVersion { .. })
        ));

        let mut loosened = definition;
        loosened.requires_approval = false;
        assert!(matches!(
            PlatformToolCatalogue::try_from_definitions([loosened]),
            Err(ToolCatalogueError::UnsafeDefault(_))
        ));
    }

    #[test]
    fn tools_are_re_addressable_without_embedding_tenant_in_the_definition() {
        let catalogue = PlatformToolCatalogue::platform().unwrap();
        let merge = catalogue.resolve("git.merge").unwrap();
        assert_eq!(catalogue_cursor(merge), "git.merge.v1");
        assert_eq!(
            tool_ref(&TenantId("acme".into()), merge).0,
            "myelin://acme/agent/tool/git.merge/v1"
        );
    }
}

use myelin_agent::{EffectKind, ToolDef};
use myelin_git::api::AgentToolDef;
use serde_json::{json, Value};
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
enum ToolSource {
    Shared(ToolDef),
    Git(AgentToolDef),
}

#[derive(Clone, Debug)]
pub struct RegisteredTool {
    mcp_name: String,
    source: ToolSource,
}

impl RegisteredTool {
    pub fn name(&self) -> &str {
        &self.mcp_name
    }

    pub fn requires_approval(&self) -> bool {
        match &self.source {
            ToolSource::Shared(def) => def.requires_approval,
            ToolSource::Git(def) => def.requires_approval,
        }
    }

    pub fn required_caps(&self) -> Vec<&str> {
        match &self.source {
            ToolSource::Shared(def) => def.required_caps.iter().map(String::as_str).collect(),
            ToolSource::Git(def) => def.required_caps.to_vec(),
        }
    }

    pub fn effect_kind(&self) -> EffectKind {
        match &self.source {
            ToolSource::Shared(def) => def.effect_kind,
            ToolSource::Git(_) => EffectKind::Mutate,
        }
    }

    pub fn side_effecting(&self) -> bool {
        match &self.source {
            ToolSource::Shared(def) => def.side_effecting,
            ToolSource::Git(_) => true,
        }
    }

    pub fn to_mcp_json(&self) -> Value {
        match &self.source {
            ToolSource::Shared(def) => def
                .mcp_projection()
                .expect("shared ToolDef schemas are validated at registration"),
            ToolSource::Git(def) => def
                .to_tool_def()
                .mcp_projection()
                .expect("the frozen Git ToolDefs are validated by their provider"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryError {
    InvalidDefinition(String),
    DuplicateName(String),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::InvalidDefinition(reason) => write!(f, "{reason}"),
            RegistryError::DuplicateName(name) => {
                write!(f, "duplicate MCP tool name `{name}`")
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ToolRegistry {
    tools: Vec<RegisteredTool>,
}

impl ToolRegistry {
    pub fn new() -> ToolRegistry {
        ToolRegistry { tools: Vec::new() }
    }

    pub fn register_legacy_git(&mut self) -> Result<(), RegistryError> {
        for def in myelin_git::api::agent_tools() {
            self.push(RegisteredTool {
                mcp_name: def.name.to_string(),
                source: ToolSource::Git(def),
            })?;
        }
        Ok(())
    }

    pub fn register_tool_defs(
        &mut self,
        defs: impl IntoIterator<Item = ToolDef>,
    ) -> Result<(), RegistryError> {
        for def in defs.into_iter().filter(|def| def.exposed_over_mcp) {
            validate_shared_def(&def)?;
            let mcp_name = format!("{}.{}", def.subsystem, def.name.0);
            self.push(RegisteredTool {
                mcp_name,
                source: ToolSource::Shared(def),
            })?;
        }
        Ok(())
    }

    fn push(&mut self, tool: RegisteredTool) -> Result<(), RegistryError> {
        if self
            .tools
            .iter()
            .any(|existing| existing.name() == tool.name())
        {
            return Err(RegistryError::DuplicateName(tool.name().to_string()));
        }
        self.tools.push(tool);
        Ok(())
    }

    pub fn with_git() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry
            .register_legacy_git()
            .expect("the frozen Git catalogue has unique names");
        registry
    }

    pub fn with_git_and_ci_reads() -> Result<ToolRegistry, RegistryError> {
        let mut registry = ToolRegistry::with_git();
        registry.register_tool_defs(myelin_ci_controlplane::ci_tool_defs())?;
        Ok(registry)
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

fn validate_shared_def(def: &ToolDef) -> Result<(), RegistryError> {
    def.validate()
        .map_err(|error| RegistryError::InvalidDefinition(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_provider_catalogue_remains_verbatim() {
        let registry = ToolRegistry::with_git();
        let names = registry
            .tools()
            .iter()
            .map(RegisteredTool::name)
            .collect::<Vec<_>>();
        let expected = myelin_git::api::agent_tools()
            .iter()
            .map(|def| def.name)
            .collect::<Vec<_>>();
        assert_eq!(names, expected);
        assert!(registry.resolve("git.merge").unwrap().requires_approval());
        assert_eq!(
            registry.resolve("git.open_pr").unwrap().required_caps(),
            ["repo.push"]
        );
    }

    #[test]
    fn ci_projection_is_exact_and_only_exposes_implemented_reads() {
        let registry = ToolRegistry::with_git_and_ci_reads().unwrap();
        let read_run = registry.resolve("ci.read_run").expect("read_run exposed");
        let read_log = registry.resolve("ci.read_log").expect("read_log exposed");
        assert_eq!(read_run.effect_kind(), EffectKind::Read);
        assert!(!read_run.side_effecting());
        assert_eq!(read_run.required_caps(), ["run.view"]);
        assert_eq!(
            read_run.to_mcp_json()["inputSchema"],
            serde_json::from_str::<Value>(
                &myelin_ci_controlplane::ci_tool_def("read_run").input_schema
            )
            .unwrap()
        );
        assert!(read_log.to_mcp_json()["inputSchema"]["properties"]["limit"]["maximum"] == 262_144);
        assert!(registry.resolve("ci.run").is_none());
        assert!(registry.resolve("ci.validate").is_none());
    }

    #[test]
    fn shared_registration_rejects_malformed_or_duplicate_contracts() {
        let mut registry = ToolRegistry::new();
        let mut def = myelin_ci_controlplane::ci_tool_def("read_run");
        def.input_schema = "not json".into();
        assert!(matches!(
            registry.register_tool_defs([def]),
            Err(RegistryError::InvalidDefinition(_))
        ));

        let def = myelin_ci_controlplane::ci_tool_def("read_run");
        registry.register_tool_defs([def.clone()]).unwrap();
        assert_eq!(
            registry.register_tool_defs([def]),
            Err(RegistryError::DuplicateName("ci.read_run".into()))
        );
    }
}

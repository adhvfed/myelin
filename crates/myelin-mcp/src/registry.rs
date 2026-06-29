//! # `registry` — the tool-registration framework (REUSE `agent_tools()`, don't re-declare).
//!
//! The headline of MR-021's framework half: the MCP server's tool catalogue is a **thin projection**
//! of each subsystem's OWN `agent_tools()` (the frozen MCP tool shape — name + `requires_approval` +
//! the already-built handler). A subsystem's tool SET is **never re-declared here** — it is sourced
//! verbatim from the subsystem crate, so a NEW git tool added to [`myelin_git::api::agent_tools`]
//! flows to `tools/list` (and is callable via `tools/call`) with **no change to this crate**.
//!
//! ## How a subsystem adds MCP tools (the plug-in convention — mirrors the MR-014 edge + MR-020 CLI)
//! The MR-014 edge convention is "a subsystem adds ONLY its routes + handlers; the gateway owns
//! auth/scope/error". The MR-020 CLI mirror is "a subsystem exposes its command grammar; the shell
//! owns auth/render". The MCP mirror is:
//!   1. **Catalogue** — the subsystem exposes `agent_tools() -> Vec<AgentToolDef>` (its frozen tool
//!      names + `requires_approval` defaults + handlers). Git ships [`myelin_git::api::agent_tools`].
//!   2. **Register** — the server calls [`ToolRegistry::register_subsystem`] with that catalogue. The
//!      framework keys each tool by its `name` and projects it into the MCP `tools/list` shape.
//!   3. **Route** — `tools/call` resolves the name back to its [`RegisteredTool`] and routes its
//!      effect through the ONE governance chokepoint (see [`crate::governance`]).
//!
//! Adding a subsystem is steps 1+2 only — the protocol, the governance routing, the audit, and the
//! HITL gate are owned ONCE by the server (this crate), for everyone.

use myelin_git::api::AgentToolDef;
use serde_json::{json, Value};

/// One tool registered into the MCP catalogue — the subsystem token, the frozen [`AgentToolDef`]
/// (its name + `requires_approval` + handler, sourced VERBATIM from the subsystem's `agent_tools()`),
/// kept exactly as the subsystem declared it. No field is re-declared here.
#[derive(Clone, Debug)]
pub struct RegisteredTool {
    /// The contributing subsystem (`"git"`, …) — the event-bus token, for attribution + the
    /// `(subsystem, name)` key.
    pub subsystem: &'static str,
    /// The frozen tool definition from the subsystem's `agent_tools()` (REUSED, not forked).
    pub def: AgentToolDef,
}

impl RegisteredTool {
    /// The MCP tool name (the catalogue key — e.g. `"git.merge"`). The subsystem owns it.
    pub fn name(&self) -> &'static str {
        self.def.name
    }

    /// Whether the tool is HITL-gated by default — the FROZEN `requires_approval` flag from the
    /// subsystem's `agent_tools()` (git: `git.merge = true`, everything else `false`). This is the
    /// flag the governance router gates on BEFORE `EffectApi::apply` (the MR-006 HITL leg).
    pub fn requires_approval(&self) -> bool {
        self.def.requires_approval
    }

    /// Project this tool into the MCP `tools/list` entry shape. The `inputSchema` is a permissive
    /// object schema here (the rich per-tool JSON Schema is the `myelin_agent::ToolDef.input_schema`
    /// seam, seeded in the agent-fabric catalogue — AG-P8; the MCP surface projects the names +
    /// frozen `requires_approval` from `agent_tools()` today). `annotations.requiresApproval`
    /// surfaces the frozen HITL default so the calling Claude knows a confirm is coming.
    pub fn to_mcp_json(&self) -> Value {
        json!({
            "name": self.name(),
            "description": format!(
                "{} tool `{}` (handler: {:?}). Routes through mint_run_token -> EffectApi::apply \
                 under the same governance a human is.{}",
                self.subsystem,
                self.name(),
                self.def.handler,
                if self.requires_approval() { " Requires HITL approval before apply." } else { "" }
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "repo": { "type": "string", "description": "the repository slug (tenant is from the token, never here)" },
                    "number": { "type": "integer", "description": "the PR number, when the tool acts on a PR" }
                },
                "additionalProperties": true
            },
            "annotations": {
                "requiresApproval": self.requires_approval()
            }
        })
    }
}

/// **The MCP tool registry** — the framework that holds the registered tools (sourced from the
/// subsystems' `agent_tools()`) and serves `tools/list` + resolves `tools/call`.
#[derive(Clone, Debug, Default)]
pub struct ToolRegistry {
    tools: Vec<RegisteredTool>,
}

impl ToolRegistry {
    /// An empty registry.
    pub fn new() -> ToolRegistry {
        ToolRegistry { tools: Vec::new() }
    }

    /// **Register a subsystem's tools** by providing its `(subsystem, agent_tools())` — the plug-in
    /// step. Each [`AgentToolDef`] is kept VERBATIM (no re-declaration). A new tool in the
    /// subsystem's `agent_tools()` is registered automatically the next time the server starts.
    pub fn register_subsystem(&mut self, subsystem: &'static str, defs: Vec<AgentToolDef>) {
        for def in defs {
            self.tools.push(RegisteredTool { subsystem, def });
        }
    }

    /// Build a registry pre-loaded with git's frozen `agent_tools()` (the first registered subsystem
    /// — MR-021 plugs git first, the same order MR-015/MR-020 plugged git into the edge/CLI).
    pub fn with_git() -> ToolRegistry {
        let mut r = ToolRegistry::new();
        r.register_subsystem("git", myelin_git::api::agent_tools());
        r
    }

    /// The registered tools (for `tools/list`).
    pub fn tools(&self) -> &[RegisteredTool] {
        &self.tools
    }

    /// Resolve a tool by its MCP name (for `tools/call`). `None` ⇒ an unknown tool (the server
    /// returns an Invalid-params JSON-RPC error, never a panic / a faked call).
    pub fn resolve(&self, name: &str) -> Option<&RegisteredTool> {
        self.tools.iter().find(|t| t.name() == name)
    }

    /// The `tools/list` result body: the projected MCP tool entries.
    pub fn list_result(&self) -> Value {
        json!({ "tools": self.tools.iter().map(RegisteredTool::to_mcp_json).collect::<Vec<_>>() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **REUSE proof:** every tool git's `agent_tools()` declares is exposed by the registry with NO
    /// re-declaration — a rename/add in `api.rs` flows here automatically.
    #[test]
    fn registry_reuses_git_agent_tools_verbatim() {
        let reg = ToolRegistry::with_git();
        let names: Vec<&str> = reg.tools().iter().map(|t| t.name()).collect();
        let git_names: Vec<&str> =
            myelin_git::api::agent_tools().iter().map(|d| d.name).collect();
        assert_eq!(names, git_names, "the registry is a verbatim projection of agent_tools()");
        assert!(names.contains(&"git.merge"));
        assert!(names.contains(&"git.submit_review"));
    }

    /// The FROZEN `requires_approval` defaults flow through: git.merge is HITL-gated, open_pr is not.
    #[test]
    fn frozen_requires_approval_flows_through() {
        let reg = ToolRegistry::with_git();
        assert!(reg.resolve("git.merge").unwrap().requires_approval(), "git.merge is HITL-gated");
        assert!(!reg.resolve("git.open_pr").unwrap().requires_approval(), "open_pr is not");
    }

    /// `tools/list` projects the MCP shape incl. the `requiresApproval` annotation; an unknown tool
    /// resolves to None (no panic, no fake).
    #[test]
    fn list_result_projects_mcp_shape_and_unknown_resolves_none() {
        let reg = ToolRegistry::with_git();
        let list = reg.list_result();
        let tools = list["tools"].as_array().unwrap();
        assert!(!tools.is_empty());
        let merge = tools.iter().find(|t| t["name"] == "git.merge").unwrap();
        assert_eq!(merge["annotations"]["requiresApproval"], json!(true));
        assert!(reg.resolve("git.nonexistent").is_none());
    }
}

//! # `server` — the MCP JSON-RPC server: handshake + `tools/list` + governed `tools/call`.
//!
//! Ties the three halves together: the [`crate::protocol`] framing, the [`crate::registry`] tool
//! catalogue (sourced from `agent_tools()`), and the [`crate::governance`] chokepoint (the MR-006
//! `mint_run_token → EffectApi::apply` routing). Total over malformed input — a bad line yields a
//! JSON-RPC error, never a panic.
//!
//! ## The MCP methods
//! - `initialize` → the server's protocol version + capabilities (`tools`) + server info.
//! - `notifications/initialized` → a notification (no response).
//! - `tools/list` → the registered tools (names + input schemas + the frozen `requiresApproval`).
//! - `tools/call` → resolve the tool, route through the governance chokepoint, return the outcome.
//!   With NO governance router wired (the standalone binary), `tools/call` returns an HONEST
//!   "not wired" JSON-RPC error — the per-run minter + the `EffectApi` body are injected by the
//!   composition root (myelin-agent-service), never constructed in the protocol shell.

use std::io::{BufRead, Write};

use serde_json::{json, Value};

use crate::governance::{CallOutcome, GovernedRouter};
use crate::protocol::{
    error_response, parse_request, success, RpcError, GOVERNANCE_NOT_WIRED, INVALID_PARAMS,
    METHOD_NOT_FOUND,
};
use crate::registry::ToolRegistry;
use myelin_events::Timestamp;

/// The MCP protocol version this server advertises (the MCP revision string).
pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

/// The MCP server — the tool registry + an OPTIONAL governance router. With a router wired,
/// `tools/call` routes through `mint_run_token → EffectApi::apply`; without one, the protocol +
/// catalogue (`initialize` / `tools/list`) are fully live and `tools/call` is honestly "not wired".
pub struct McpServer {
    registry: ToolRegistry,
    router: Option<GovernedRouter>,
    /// The clock the governed call mints/consults under. Injected so a test is deterministic; the
    /// real wall-clock source is the substrate clock binding (the SAME convention the mint uses).
    now: Timestamp,
}

impl McpServer {
    /// A server over git's tool catalogue with NO governance router (the standalone protocol shell).
    pub fn new_catalogue_only() -> McpServer {
        McpServer {
            registry: ToolRegistry::with_git(),
            router: None,
            now: Timestamp(default_now()),
        }
    }

    /// A server with a governance router wired (the governed path — the composition root / tests).
    pub fn with_router(registry: ToolRegistry, router: GovernedRouter, now: Timestamp) -> McpServer {
        McpServer { registry, router: Some(router), now }
    }

    /// The tool registry (so a host/test can inspect the catalogue).
    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    /// The governance router, if wired (so a test can read the audit trail / revoke the run token).
    pub fn router(&self) -> Option<&GovernedRouter> {
        self.router.as_ref()
    }

    /// **Handle ONE input line, returning the response line to write — or `None` for a notification.**
    /// TOTAL: any malformed input yields a JSON-RPC error string, never a panic.
    pub fn handle_line(&self, line: &str) -> Option<String> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let req = match parse_request(line) {
            Ok(r) => r,
            Err((id, err)) => return Some(write_value(&error_response(id, err))),
        };
        // A notification (no id) NEVER gets a response (JSON-RPC §4.1), even on error.
        if req.is_notification {
            return None;
        }
        let response = match req.method.as_str() {
            "initialize" => success(req.id, self.initialize_result()),
            "tools/list" => success(req.id, self.registry.list_result()),
            "tools/call" => match self.tools_call(&req.params) {
                Ok(result) => success(req.id, result),
                Err(err) => error_response(req.id, err),
            },
            _ => error_response(
                req.id,
                RpcError::new(METHOD_NOT_FOUND, format!("method not found: {}", req.method)),
            ),
        };
        Some(write_value(&response))
    }

    /// **Run the stdio loop** — read newline-delimited JSON-RPC from `reader`, write responses to
    /// `writer` (one message per line). Returns on EOF. Never panics on a malformed line.
    pub fn run(&self, reader: impl BufRead, mut writer: impl Write) -> std::io::Result<()> {
        for line in reader.lines() {
            let line = line?;
            if let Some(resp) = self.handle_line(&line) {
                writeln!(writer, "{resp}")?;
                writer.flush()?;
            }
        }
        Ok(())
    }

    /// The `initialize` result — the protocol version + capabilities (tools) + server info.
    fn initialize_result(&self) -> Value {
        json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "myelin-mcp", "version": env!("CARGO_PKG_VERSION") }
        })
    }

    /// `tools/call` — resolve the tool + route through the governance chokepoint.
    fn tools_call(&self, params: &Value) -> Result<Value, RpcError> {
        let name = params.get("name").and_then(Value::as_str).ok_or_else(|| {
            RpcError::new(INVALID_PARAMS, "tools/call requires a string `name`")
        })?;
        let args = params.get("arguments").cloned().unwrap_or(json!({}));

        // Resolve against the catalogue (sourced from agent_tools()). Unknown ⇒ Invalid params,
        // never a panic / a faked call.
        let tool = self
            .registry
            .resolve(name)
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, format!("unknown tool: {name}")))?
            .clone();

        // The governance router is the chokepoint. Without one, this is honestly "not wired" — the
        // per-run minter + the EffectApi body are injected by the composition root, never here.
        let router = self.router.as_ref().ok_or_else(|| {
            RpcError::new(
                GOVERNANCE_NOT_WIRED,
                "governed routing not wired into this server instance: a tools/call mints a per-run \
                 token and routes through EffectApi::apply (MR-006); the minter + the EffectApi body \
                 are injected by the composition root (myelin-agent-service) — not the standalone \
                 protocol shell. See the governed_routing integration test for the end-to-end path.",
            )
        })?;

        // HITL (R2.4): a re-drive after a human approved the card PRESENTS the server-issued
        // opaque gate id (`approval.gateId`); the router looks it up in the SERVER-SIDE verdict
        // store and proceeds only if THAT gate is Approved there by a distinct principal. The
        // legacy caller-supplied `approval.granted` boolean is deliberately NOT read — it is inert
        // on the wire and never an enforcement input (the 2026-07-06 HIGH finding).
        let presented_gate_id = params
            .get("approval")
            .and_then(|a| a.get("gateId"))
            .and_then(Value::as_str);

        let outcome = router.call(&tool, &args, &self.now, presented_gate_id);
        Ok(call_result_json(name, &outcome))
    }
}

/// Map a governed [`CallOutcome`] to the MCP `tools/call` result body. `isError` is set for a denied
/// effect; a gated effect surfaces the gate id (the human must approve). Every result carries the
/// run-token `jti` under `_meta` — the attribution that makes the call auditable to the run.
fn call_result_json(tool: &str, outcome: &CallOutcome) -> Value {
    let (text, is_error) = match outcome {
        CallOutcome::Applied { event_id, .. } => {
            (format!("`{tool}` applied through EffectApi (event {event_id})."), false)
        }
        CallOutcome::Gated { gate_id, .. } => (
            format!("`{tool}` is withheld pending HITL approval (gate {gate_id}); not applied."),
            false,
        ),
        CallOutcome::Denied { reason, .. } => (format!("`{tool}` denied: {reason}"), true),
    };
    let mut meta = json!({ "runToken": outcome.jti(), "tool": tool });
    match outcome {
        CallOutcome::Applied { event_id, .. } => meta["eventId"] = json!(event_id),
        CallOutcome::Gated { gate_id, .. } => meta["gateId"] = json!(gate_id),
        CallOutcome::Denied { reason, .. } => meta["reason"] = json!(reason),
    }
    json!({
        "content": [ { "type": "text", "text": text } ],
        "isError": is_error,
        "_meta": meta
    })
}

/// Serialise a response `Value` to a single line (no embedded newline — the stdio framing rule).
fn write_value(v: &Value) -> String {
    // `serde_json::to_string` never emits a newline; this is the on-the-wire frame.
    serde_json::to_string(v).unwrap_or_else(|_| {
        // Unreachable for a well-formed Value; stay TOTAL rather than panic.
        r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"internal serialize error"}}"#
            .to_string()
    })
}

/// A floor RFC-3339 `now` for the catalogue-only shell (no mint happens there). The real wall-clock
/// source is the substrate clock binding (the SAME named floor the mint uses).
fn default_now() -> String {
    "2026-06-26T00:00:00Z".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_returns_capabilities_and_server_info() {
        let s = McpServer::new_catalogue_only();
        let resp = s.handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
            .expect("response");
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["result"]["protocolVersion"], json!(MCP_PROTOCOL_VERSION));
        assert!(v["result"]["capabilities"]["tools"].is_object());
        assert_eq!(v["result"]["serverInfo"]["name"], json!("myelin-mcp"));
    }

    #[test]
    fn tools_list_exposes_the_git_tools_with_requires_approval() {
        let s = McpServer::new_catalogue_only();
        let resp = s.handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).expect("resp");
        let v: Value = serde_json::from_str(&resp).unwrap();
        let tools = v["result"]["tools"].as_array().unwrap();
        let merge = tools.iter().find(|t| t["name"] == "git.merge").unwrap();
        assert_eq!(merge["annotations"]["requiresApproval"], json!(true));
    }

    #[test]
    fn a_notification_yields_no_response() {
        let s = McpServer::new_catalogue_only();
        assert!(s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    }

    #[test]
    fn malformed_line_is_a_jsonrpc_error_not_a_panic() {
        let s = McpServer::new_catalogue_only();
        let resp = s.handle_line("{ this is not json").expect("error response");
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["error"]["code"], json!(crate::protocol::PARSE_ERROR));
    }

    #[test]
    fn unknown_method_is_method_not_found() {
        let s = McpServer::new_catalogue_only();
        let resp = s.handle_line(r#"{"jsonrpc":"2.0","id":3,"method":"frobnicate"}"#).unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["error"]["code"], json!(METHOD_NOT_FOUND));
    }

    #[test]
    fn tools_call_without_a_router_is_honestly_not_wired() {
        let s = McpServer::new_catalogue_only();
        let resp = s
            .handle_line(r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"git.merge"}}"#)
            .unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["error"]["code"], json!(GOVERNANCE_NOT_WIRED));
    }

    #[test]
    fn tools_call_unknown_tool_is_invalid_params() {
        let s = McpServer::new_catalogue_only();
        let resp = s
            .handle_line(r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"git.nope"}}"#)
            .unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["error"]["code"], json!(INVALID_PARAMS));
    }
}

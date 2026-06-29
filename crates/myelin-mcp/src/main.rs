//! # `myelin-mcp` binary — the MCP stdio server entry point.
//!
//! A thin shell: build the tool registry (git's `agent_tools()` first) and run the JSON-RPC stdio
//! loop. `initialize` + `tools/list` are fully live (the protocol + the catalogue). `tools/call`
//! routes through the governance chokepoint when a [`myelin_mcp::GovernedRouter`] is injected by the
//! composition root (the per-run `RunTokenMinter` + the `EffectApi` body from
//! `myelin-agent-service`); the standalone binary has neither (it constructs no crypto / no minter),
//! so `tools/call` returns an HONEST "governed routing not wired" JSON-RPC error. The end-to-end
//! governed path (`mint_run_token → EffectApi::apply`, HITL, revocation) is proven by the
//! `governed_routing` integration test, which wires a real minter + a reference `EffectApi`.
//!
//! Local Claude (Claude Code) launches this binary as an MCP server over stdio; the operator's MCP
//! client config points at it. Reads newline-delimited JSON-RPC from stdin, writes responses to
//! stdout. Logs/diagnostics, if any, must go to stderr (stdout is the protocol channel).

use std::io::{self, BufReader};

use myelin_mcp::McpServer;

fn main() -> io::Result<()> {
    let server = McpServer::new_catalogue_only();
    let stdin = io::stdin();
    let stdout = io::stdout();
    server.run(BufReader::new(stdin.lock()), stdout.lock())
}

//! # `myelin-mcp` — governed MCP server + tool-registration framework (E0.9 / MR-021)
//!
//! The JSON-RPC-2.0-over-stdio surface a **local Claude** (Claude Code on the operator's machine)
//! drives Myelin's git/issues/chat/docs/CI through as native MCP tools — under the **same governance
//! a human is**. This is the near-term answer to "agents, but not hosted yet": agent **governance**
//! (who/what did what, with what authority) is real from day one, even though agent **hosting** (the
//! real `LlmAgentRuntime`) stays deferred. The mock→real-runtime seam (MR-006) is unchanged.
//!
//! ## The three halves
//! - **Protocol** ([`protocol`]) — hand-built JSON-RPC 2.0 framing (newline-delimited JSON over
//!   stdio). There is no MCP/rmcp crate in `Cargo.lock`, so it is built on `serde_json` (minimal-deps
//!   ethos). Total over malformed input (a bad line → a JSON-RPC error, never a panic).
//! - **Registry** ([`registry`]) — a direct projection of shared [`myelin_agent::ToolDef`] values;
//!   Git's older catalogue stays behind an explicit compatibility adapter.
//! - **Governance** ([`governance`]) — every `tools/call` acts under a minted run token. Reads go
//!   directly to a permission-checked subsystem API; mutations go through
//!   [`myelin_agent::EffectApi`] and HITL where declared. Never a bare human PAT or direct mutation.
//!
//! ## Production composition
//! The binary composes the protocol and registry with cryptographically authenticated trigger
//! identity, snapshot-bound per-run authority, PostgreSQL-backed governance stores, object-scoped
//! Git ReBAC, a durable Git effect adapter, and durable audit intent. The generic `EffectApi` remains
//! injected so tests can use [`governance::SkeletonEffectApi`] and later runtimes can reuse the same
//! governed boundary. Agent hosting (a real `LlmAgentRuntime`) remains outside this crate.
//!
//! ## DAG position
//! A LEAF BINARY (like `myelin-cli` / `myelin-edge`): it REUSES git's `agent_tools()` + the agent
//! `EffectApi` governance contract + the `RunTokenMinter`, and NOTHING in the production crate DAG
//! depends back on it. It is NOT a node in the eleven-crate library DAG modelled by
//! `myelin_substrate::crate_graph` (substrate_is_root() / identity_is_sink() are unaffected).

pub mod ci_read;
pub mod governance;
pub mod protocol;
pub mod registry;
pub mod server;

pub use ci_read::CiDirectReadExecutor;
pub use governance::{
    git_merge_repo_from_effect_key, AuditPhase, CallOutcome, GateApproverPolicy, GovernanceAudit,
    GovernanceAuditRecord, GovernedRouter, OutboxGovernanceAudit, ReadAuthorization, RunPrincipal,
    SkeletonEffectApi,
};
pub use registry::{RegisteredTool, ToolRegistry};
pub use server::{Clock, DirectReadError, DirectReadExecutor, McpServer, MAX_FRAME_BYTES};

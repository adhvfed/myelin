//! # `myelin-mcp` — the MCP server skeleton + tool-registration framework (E0.9 / MR-021)
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
//! - **Registry** ([`registry`]) — the tool-registration framework. The catalogue is a thin
//!   projection of each subsystem's OWN `agent_tools()` (git first). A NEW git tool in
//!   [`myelin_git::api::agent_tools`] flows to the MCP server with NO re-declaration.
//! - **Governance** ([`governance`]) — THE MR-006 BINDING. Every `tools/call` routes through
//!   `mint_run_token → EffectApi::apply` under a per-run [`myelin_agent::RunCtx`] (the platform-owned
//!   plan-then-apply chokepoint), HITL-gated where `requires_approval`, audited/attributed to the run.
//!   NEVER a bare human PAT; NEVER a direct mutation.
//!
//! ## What is REAL vs deferred (honesty)
//! - **REAL:** the protocol + the tool framework + the GOVERNANCE pipeline (per-run token mint, the
//!   durable-revocation consult, the HITL gate on the frozen flag, the routing through the `EffectApi`
//!   trait, and the per-run audit attribution).
//! - **INJECTED:** the concrete `EffectApi` body — the eight-step `PlanThenApply` pipeline lives in
//!   `myelin-agent-service`; the composition root injects it. [`governance::SkeletonEffectApi`] is a
//!   reference chokepoint used to prove the routing.
//! - **DEFERRED:** the durable git-backend EFFECT (a real merge/PR write) is the Git track **E1.1**;
//!   the full per-subsystem tool coverage beyond git; and agent HOSTING (the real `LlmAgentRuntime`).
//!
//! ## DAG position
//! A LEAF BINARY (like `myelin-cli` / `myelin-edge`): it REUSES git's `agent_tools()` + the agent
//! `EffectApi` governance contract + the `RunTokenMinter`, and NOTHING in the production crate DAG
//! depends back on it. It is NOT a node in the eleven-crate library DAG modelled by
//! `myelin_substrate::crate_graph` (substrate_is_root() / identity_is_sink() are unaffected).

pub mod governance;
pub mod protocol;
pub mod registry;
pub mod server;

pub use governance::{CallOutcome, GovernedRouter, RunPrincipal, SkeletonEffectApi};
pub use registry::{RegisteredTool, ToolRegistry};
pub use server::McpServer;

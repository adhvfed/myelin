//! # `myelin-agent` — runtime / agent / tool traits (the strategy-pattern boundary)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §2.4 (`myelin-agent` — runtime/agent/tool traits).
//!
//! **Contract-index cluster:** 8 — Agent fabric
//! (`planning/05-refined-shared-systems-architecture/contract-index.md` rows 8.3
//! `AgentRuntime::step`, 8.4 `ToolHands::exec`, 8.1 `ToolSurface::register_tool`).
//!
//! ## What crosses the crate boundary here (the frozen surface)
//! The small trait set behind which `MockAgentRuntime` lives now and `LlmAgentRuntime`
//! lives later (the strategy seam, VISION §3): **NO LLM SDK / prompt / model name appears
//! in platform code** (ADR-08.2; the `no-llm-in-platform` lint, P-S11, makes this
//! structural).
//! - `AgentRuntime::step` (8.3) — the STATELESS brain (AG-1); platform owns history.
//! - `ToolHands::exec` (8.4) — the hands; **no host-execution bypass** (AG-2, X-6);
//!   `exec` IS the CI runner's `kind=agent` job on the unified sandbox. The
//!   `no-host-exec` lint (P-S10) enforces no path bypasses this.
//! - `ToolSurface::register_tool` (8.1) — one permissioned catalogue (ADR-08.4).
//!
//! ## Floors named (stubbed bodies → filling prompt)
//! All bodies are `todo!()`. The trait SHAPES are frozen here (P-001 skeleton); the Agent
//! Fabric roadmap fills them:
//! - `AgentRuntime::step` strategy seam (skeleton/mock/llm; `--use-mock`) → Agent M-stage
//!   (8.3).
//! - `ToolHands::exec` sandbox + the four uniform guarantees (cost gate, per-run-token
//!   attribution, HITL withhold, isolation floor + the real-kernel escape drill) →
//!   Agent M-stage (8.4, X-6). The `no-host-exec` lint that guards it is P-S10.
//! - `ToolSurface::register_tool` + the frozen `requires_approval` defaults table →
//!   Agent M-stage (8.1, X-6).

use serde::{Deserialize, Serialize};

/// The agent conversation the stateless brain reads (architecture §2.4; contract 8.3).
/// Platform owns history — the runtime is stateless. Opaque in the skeleton; the
/// conversation/message model lands with the Agent Fabric (8.5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation(pub String);

/// A proposed tool call the brain emits (architecture §2.4; contract 8.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall(pub String);

/// A final submission from the brain (architecture §2.4; contract 8.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submission(pub String);

/// The brain's per-step outcome (architecture §2.4; contract 8.3): use tools, or submit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepOutcome {
    UseTools(Vec<ToolCall>),
    Submit(Submission),
}

/// A sandboxed command for the hands (architecture §2.4; contract 8.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command(pub String);

/// The result of a sandboxed `exec` (architecture §2.4; contract 8.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult(pub String);

/// A tool definition registered into the one catalogue (architecture §2.4; contract 8.1).
/// The full `ToolDef{name, input_schema, required_caps, effect_kind, side_effecting,
/// requires_approval, exposed_over_mcp}` + the frozen `requires_approval` defaults are
/// the Agent Fabric's (8.1); the skeleton carries an opaque def so the trait compiles.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDef(pub String);

/// The STATELESS brain (architecture §2.4; contract 8.3; AG-1). Strategy seam — the
/// runtime behind which mock/skeleton/llm live; NO LLM SDK in platform code
/// (`no-llm-in-platform`, P-S11).
///
/// **Floor:** `step` body is `todo!()`; the Agent Fabric roadmap fills it (8.3).
pub trait AgentRuntime {
    fn step(&self, conv: &Conversation) -> StepOutcome;
}

/// The hands (architecture §2.4; contract 8.4; AG-2). `exec` is sandboxed computation
/// with **no host-execution bypass** (X-6; the `no-host-exec` lint, P-S10, enforces it).
///
/// **Floor:** `exec` body is `todo!()`; the sandbox + four uniform guarantees land in the
/// Agent Fabric roadmap (8.4).
pub trait ToolHands {
    fn exec(&self, cmd: Command) -> ToolResult;
}

/// The one permissioned tool catalogue (architecture §2.4; contract 8.1; ADR-08.4).
///
/// **Floor:** `register_tool` body is `todo!()`; the catalogue + MCP exposure + the
/// frozen `requires_approval` defaults land in the Agent Fabric roadmap (8.1).
pub trait ToolSurface {
    fn register_tool(&mut self, def: ToolDef);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-asserting test: the three agent traits exist with their frozen signatures
    /// (architecture §2.4; contract 8.1/8.3/8.4). A stub proves `step -> StepOutcome`,
    /// `exec(Command) -> ToolResult`, `register_tool(&mut self, ToolDef)`. Bodies deferred
    /// to the Agent Fabric roadmap; the strategy seam (no LLM in platform) is the point.
    #[test]
    fn agent_runtime_hands_surface_shapes_are_frozen() {
        struct Mock;
        impl AgentRuntime for Mock {
            fn step(&self, _conv: &Conversation) -> StepOutcome {
                // A mock runtime is a real flag (`--use-mock`, 8.3); a deterministic
                // submit is a valid skeleton body.
                StepOutcome::Submit(Submission("ok".into()))
            }
        }
        impl ToolHands for Mock {
            fn exec(&self, _cmd: Command) -> ToolResult {
                todo!("sandboxed exec + four uniform guarantees land in Agent Fabric (8.4)")
            }
        }
        impl ToolSurface for Mock {
            fn register_tool(&mut self, _def: ToolDef) {
                todo!("the one catalogue lands in Agent Fabric (8.1)")
            }
        }
        let m = Mock;
        assert!(matches!(
            m.step(&Conversation("hi".into())),
            StepOutcome::Submit(_)
        ));
    }
}

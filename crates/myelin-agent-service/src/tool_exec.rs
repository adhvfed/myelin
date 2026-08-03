//! # `tool_exec` — the `ToolExecutor` seam: turn a VALIDATED tool call into a result (the driving
//! loop's dependency)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/agent-fabric.md`
//! §5.0 (the routing point — the platform loop routes a `UseTools` per `effect_kind`), §2.2 (the
//! hands), §5.2 (plan-then-apply). **Contract-index:** the loop half of 8.5 (`Agent::handle`) —
//! this is the small seam the bounded multi-turn driver ([`crate::skeleton::SkeletonAgent::handle_run`])
//! depends on to turn one *validated* [`ToolCall`] into a [`ToolResult`].
//!
//! ## What this slice ships — the SEAM + a test double (the real per-route impls are NOT wired)
//! The driving loop is generic/`dyn` over [`ToolExecutor`]; this slice ships ONLY the trait + a
//! deterministic [`MockToolExecutor`] test double. The **security checkpoint** the loop runs BEFORE
//! calling `execute` is [`validate_call`](crate::effect_api::validate_call) — the untrusted model
//! arguments are validated against the tool's schema and the tool is confirmed registered; a call
//! that fails validation is NEVER handed to `execute` (fail-closed, §2.1 plan-then-apply survives).
//!
//! **Metering is a SEPARATE, decision-gated follow-on.** This seam adds NO per-call token cost,
//! pricing, or wallet debit; the runaway guard for the loop is the max-turns bound
//! ([`crate::skeleton::DEFAULT_MAX_TURNS`]). The reserve/settle cost gate is untouched.

use myelin_agent::{ToolCall, ToolDef, ToolResult};

/// **An error from the [`ToolExecutor`].** Surfaced LOUD (never swallowed, EI-01 §2): an executor
/// failure aborts the run with a typed value — the driving loop tears the run down (revoke the
/// per-run token) and returns fail-closed, never a silent half-run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolExecError {
    /// The tool could not be executed (a routing/subsystem/sandbox/apply failure, or the tool ran
    /// and reported failure). Carries a machine reason so the refusal is self-describing.
    Failed(String),
}

impl core::fmt::Display for ToolExecError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ToolExecError::Failed(m) => write!(f, "tool execution failed: {m}"),
        }
    }
}

impl std::error::Error for ToolExecError {}

/// **The `ToolExecutor` seam — turn a VALIDATED [`ToolCall`] into a [`ToolResult`] (the driving
/// loop's dependency).** The bounded multi-turn driver
/// ([`crate::skeleton::SkeletonAgent::handle_run`]) is generic/`dyn` over this seam: on a
/// [`StepOutcome::UseTools`](myelin_agent::StepOutcome::UseTools) it validates each call
/// ([`validate_call`](crate::effect_api::validate_call), the security checkpoint — fail-closed on
/// `Err`) and then calls [`execute`](ToolExecutor::execute) with the resolved [`ToolDef`].
///
/// **The three intended REAL per-route impls (OUT OF SCOPE for this slice) — a real executor picks
/// one by [`route_of`](crate::exec::route_of)`(def.effect_kind)`:**
/// - `Read` → [`ToolRoute::Direct`](crate::exec::ToolRoute::Direct) — a permission-filtered
///   SUBSYSTEM READ (no mutation, no sandbox);
/// - `Compute` → [`ToolRoute::Sandbox`](crate::exec::ToolRoute::Sandbox) — the existing
///   [`SandboxToolHands::exec`](crate::exec::SandboxToolHands) on the unified sandbox (AG-P15);
/// - `Mutate` / `External` → [`ToolRoute::EffectApi`](crate::exec::ToolRoute::EffectApi) — the
///   existing plan-then-apply [`EffectApi::apply`](myelin_agent::EffectApi) + HITL withhold path
///   (AG-P6 / AG-P9).
///
/// This slice wires NONE of the three; it ships the seam + the [`MockToolExecutor`] double so the
/// loop is exercised end-to-end deterministically. NO metering lives here (a decision-gated
/// follow-on) — the loop's runaway guard is the max-turns bound.
pub trait ToolExecutor {
    /// Execute a **already-validated** tool call (`def` resolved from the catalogue, `call.arguments`
    /// checked against `def.input_schema`) and return its result, or a [`ToolExecError`].
    ///
    /// The caller (the driving loop) guarantees `def` is the [`ToolDef`] `call.name` resolves to and
    /// that `validate_call` passed — an impl need not re-run schema validation, but a mutation-capable
    /// real impl MUST still re-verify authority at its own final boundary (plan-then-apply).
    fn execute(&self, def: &ToolDef, call: &ToolCall) -> Result<ToolResult, ToolExecError>;
}

// ───────────────────────── the test doubles (test-support gated, mirroring the crate's mocks) ────

/// **A deterministic in-memory [`ToolSurface`](myelin_agent::ToolSurface) for tests.** The driving
/// loop validates each [`ToolCall`] against a catalogue; this is the minimal register/resolve
/// catalogue the unit tests + the CDC/drill integration tests build a [`crate::RunSubstrate`] over
/// (the SKELETON registers no tools → an empty catalogue → the loop body is never entered).
///
/// Gated `#[cfg(any(test, feature = "test-support"))]` — the same gate the crate's other in-process
/// doubles use, so the `tests/` integration targets (which enable `test-support` via the self
/// dev-dependency) can construct it. NEVER in the production DAG.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Debug, Default)]
pub struct MockToolSurface {
    defs: Vec<ToolDef>,
}

#[cfg(any(test, feature = "test-support"))]
impl MockToolSurface {
    /// A fresh, EMPTY catalogue (the SKELETON's — it registers no tools).
    pub fn new() -> MockToolSurface {
        MockToolSurface::default()
    }

    /// A catalogue seeded with `defs` (the tools a tool-driving run may call).
    pub fn with(defs: impl IntoIterator<Item = ToolDef>) -> MockToolSurface {
        MockToolSurface {
            defs: defs.into_iter().collect(),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl myelin_agent::ToolSurface for MockToolSurface {
    fn register_tool(&mut self, def: ToolDef) {
        self.defs.push(def);
    }
    fn resolve(&self, name: &myelin_agent::ToolName) -> Option<&ToolDef> {
        self.defs.iter().find(|d| &d.name == name)
    }
}

/// **A deterministic [`ToolExecutor`] test double: scripted results + a record of the calls it
/// received.** Proves the driving loop hands the executor exactly the VALIDATED calls, in order, and
/// threads each [`ToolResult`] back into the conversation. Interior-mutable ([`execute`] takes
/// `&self`) via a mutex, so a `&dyn ToolExecutor` can record across turns.
///
/// Gated `#[cfg(any(test, feature = "test-support"))]` — the crate's mock-gating convention; the
/// `tests/` integration targets reach it through the self `test-support` dev-dependency.
#[cfg(any(test, feature = "test-support"))]
pub struct MockToolExecutor {
    /// The scripted results, popped front-to-back per `execute` call. When exhausted, a deterministic
    /// default `mock-exec:<tool>:ok` is returned (so a run with more turns than scripted results still
    /// drives deterministically).
    scripted: std::sync::Mutex<std::collections::VecDeque<Result<ToolResult, ToolExecError>>>,
    /// Every [`ToolCall`] the loop dispatched, in order — the "the executor saw the validated calls"
    /// witness a test asserts on.
    seen: std::sync::Mutex<Vec<ToolCall>>,
}

#[cfg(any(test, feature = "test-support"))]
impl MockToolExecutor {
    /// A fresh executor with NO scripted results — every call returns the deterministic default
    /// `mock-exec:<tool>:ok`. Records the calls it receives.
    pub fn new() -> MockToolExecutor {
        MockToolExecutor {
            scripted: std::sync::Mutex::new(std::collections::VecDeque::new()),
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// An executor that replays `results` in order (then falls back to the default). A scripted `Err`
    /// drives the executor-error abort path.
    pub fn with_results(
        results: impl IntoIterator<Item = Result<ToolResult, ToolExecError>>,
    ) -> MockToolExecutor {
        MockToolExecutor {
            scripted: std::sync::Mutex::new(results.into_iter().collect()),
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// The calls the executor received, in dispatch order (the validated-calls witness).
    pub fn calls(&self) -> Vec<ToolCall> {
        self.seen.lock().unwrap().clone()
    }

    /// How many calls the executor received.
    pub fn call_count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for MockToolExecutor {
    fn default() -> MockToolExecutor {
        MockToolExecutor::new()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl ToolExecutor for MockToolExecutor {
    fn execute(&self, _def: &ToolDef, call: &ToolCall) -> Result<ToolResult, ToolExecError> {
        self.seen.lock().unwrap().push(call.clone());
        match self.scripted.lock().unwrap().pop_front() {
            Some(result) => result,
            None => Ok(ToolResult(format!("mock-exec:{}:ok", call.name.0))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_agent::{EffectKind, ToolCallId, ToolName, ToolSurface};

    fn def(name: &str) -> ToolDef {
        ToolDef {
            name: ToolName(name.into()),
            subsystem: "test".into(),
            version: 1,
            input_schema: "{}".into(),
            required_caps: vec![],
            effect_kind: EffectKind::Read,
            side_effecting: false,
            requires_approval: false,
            exposed_over_mcp: false,
        }
    }

    fn call(name: &str) -> ToolCall {
        ToolCall {
            id: ToolCallId(format!("call:{name}")),
            name: ToolName(name.into()),
            arguments: serde_json::Value::Null,
        }
    }

    /// **`MockToolSurface` register/resolve round-trips + an unknown name is `None`.**
    #[test]
    fn mock_tool_surface_resolves_registered_tools() {
        let mut cat = MockToolSurface::new();
        assert!(cat.resolve(&ToolName("search".into())).is_none());
        cat.register_tool(def("search"));
        assert!(cat.resolve(&ToolName("search".into())).is_some());
        assert!(cat.resolve(&ToolName("nope".into())).is_none());

        // `with` seeds the catalogue up front.
        let seeded = MockToolSurface::with([def("a"), def("b")]);
        assert!(seeded.resolve(&ToolName("a".into())).is_some());
        assert!(seeded.resolve(&ToolName("b".into())).is_some());
    }

    /// **`MockToolExecutor` records every call + replays scripted results, then the default.**
    #[test]
    fn mock_tool_executor_records_calls_and_replays_results() {
        let exec = MockToolExecutor::with_results([
            Ok(ToolResult("first".into())),
            Err(ToolExecError::Failed("boom".into())),
        ]);
        let d = def("t");

        assert_eq!(exec.execute(&d, &call("t")), Ok(ToolResult("first".into())));
        assert_eq!(
            exec.execute(&d, &call("t")),
            Err(ToolExecError::Failed("boom".into()))
        );
        // scripted exhausted → the deterministic default, keyed by tool name.
        assert_eq!(
            exec.execute(&d, &call("search")),
            Ok(ToolResult("mock-exec:search:ok".into()))
        );

        // it recorded all three calls, in order.
        assert_eq!(exec.call_count(), 3);
        let seen = exec.calls();
        assert_eq!(seen[0].name, ToolName("t".into()));
        assert_eq!(seen[2].name, ToolName("search".into()));
    }

    /// **`ToolExecError` Display is loud + non-empty (kills the `fmt -> Ok(default)` mutant).**
    #[test]
    fn tool_exec_error_display_is_loud() {
        let e = ToolExecError::Failed("subsystem down".into()).to_string();
        assert!(e.contains("tool execution failed"), "loud reason: {e}");
        assert!(e.contains("subsystem down"));
    }
}

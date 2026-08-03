//! # `mock` — the `MockAgentRuntime`: a deterministic scripted brain on the `--use-mock` real
//! code path (AG-P5 → P-217, M2-B)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/agent-fabric.md` §3.2 (the **MockAgentRuntime**
//! — *built; deterministic scripted `StepOutcome`s; shipped as a real `--use-mock` runtime flag on
//! the SAME code path users hit; the lever for golden + `cargo-mutants` testing of the
//! event→trigger→effect→event loop*, AG-4), §2.1 (the brain is **stateless**; the platform owns the
//! [`Conversation`] history — a `PersonalDataHolder`, residency-pinned, the trace), §2.3 (the
//! `Agent::handle` bounded driven multi-turn loop: `build_conversation` from the trace → step →
//! route → append → step again).
//!
//! **Contract-index:** OWNS the `--use-mock` half of 8.3 (`AgentRuntime::step` — the **Mock** impl +
//! the real `--use-mock` flag). CONSUMES nothing new (it plugs into the SAME `&dyn AgentRuntime`
//! seam the SKELETON loop ([`crate::skeleton::SkeletonAgent`]) already drives — the brain is the
//! ONLY swappable part of the loop; §2.3).
//!
//! ## What this prompt (AG-P5) ships — the deterministic scripted brain on the real path
//!
//! EI-03 §3 binds the build order: prove the WHOLE agent story on a **mock brain** first — *"a mock
//! provider replays a scripted queue of steps — deterministic, zero cost, used by both unit tests
//! AND a developer `--use-mock` runtime flag (the same path)"*. VISION §3 binds the swap to real:
//! the [`MockAgentRuntime`] is THE v1 floor; `LlmAgentRuntime` is the post-M5 follow-on (AG-P25), a
//! config/impl swap behind the frozen [`AgentRuntime`] seam, **never a rewrite**.
//!
//! Three pieces:
//! - [`MockAgentRuntime`] (8.3) — an [`AgentRuntime`] whose [`step`](AgentRuntime::step) returns the
//!   next scripted [`StepOutcome`] from a fixture [`MockScript`]. It is **stateless** (§2.1): it does
//!   NOT hold a cursor — it reads the WHOLE [`Conversation`] and replays the script entry for the
//!   conversation's current position (the count of model turns already taken). The platform owns
//!   history; the brain is a pure function of the conversation. NO model / SDK / prompt / model-name
//!   string appears (the `no-llm-in-platform` ratchet, contract 1.6).
//! - [`build_conversation`] (§2.1/§2.3) — reconstructs the [`Conversation`] from the
//!   platform-owned history ([`TraceHistory`]): the system context + prior model steps + tool
//!   results + the running transcript + the budget view + the scoped tool list. The brain is passed
//!   a `&Conversation`; it holds NO state of its own. This is the loop's `build_conversation` seam.
//! - [`RuntimeFlag`] / [`select_runtime`] — the real `--use-mock` flag: it selects
//!   [`MockAgentRuntime`] vs the SKELETON brain ([`SkeletonAgentRuntime`]) on the SAME
//!   gateway/identity/dispatch/reserve/trace path (NOT a test-only stub). `--use-mock` drives the
//!   full [`crate::skeleton::SkeletonAgent::handle_run`] substrate path unchanged — the mock is on
//!   the real code path, not a bypass.
//!
//! ## The determinism property (AG-D9, the step-determinism leg)
//! Given the SAME [`MockScript`] + the SAME inbound history, two runs produce **byte-identical**
//! [`StepOutcome`] streams + identical [`Conversation`] reconstructions. This is the AG-4 lever: the
//! stateless scripted brain makes the whole event→trigger→effect loop golden- and
//! `cargo-mutants`-testable. [`replay`] drives a script to completion against a growing conversation
//! and returns the recorded [`StepOutcome`] stream a golden test asserts byte-identical across runs.
//!
//! ## FLOORS named (this is the MOCK brain — a mock that masquerades as a real brain is the failure;
//! VISION §3, EI-01 §1)
//! - **The MockAgentRuntime is THE named v1 floor.** The real vendor brain is `LlmAgentRuntime`
//!   (**AG-P25, post-M5**, designed-not-built — the only place a model/SDK/prompt/model-name string
//!   ever appears; `no-llm-in-platform`, contract 1.6), swapped in after the safety drills
//!   (AG-D4/D2/D3/D5) are green, a config/impl swap behind the frozen [`AgentRuntime`] seam.
//! - **The full proposed-effect-sequence determinism is re-asserted in AG-P8** (→ P-220) once
//!   `EffectApi::apply` (AG-P6 → P-218) produces effects. THIS prompt greens the **step-sequence**
//!   half of AG-D9 (identical `StepOutcome`/conversation streams) and names that completion.
//! - **The trace HISTORY this builds the conversation from is the platform-owned trace holder.** The
//!   content-addressed write of the trace document into Knowledge is AG-P19 (→ P-268). Here the
//!   [`TraceHistory`] is the in-memory shape the loop reconstructs from; the durable trace row +
//!   `run.trace_ref` `ArtifactRef` are the SKELETON's (AG-P4) + the schema's (AG-P2).

use myelin_agent::{
    AgentRuntime, BudgetView, Conversation, MeteredRuntime, StepOutcome, Submission, SystemContext,
    ToolCall, ToolOutcome, ToolResult, ToolSchema, Turn,
};

// ───────────────────────── §3.2 — the scripted brain's fixture ─────────────────────────

/// **A fixture script the [`MockAgentRuntime`] replays (§3.2 — *a scripted queue of steps*).** An
/// ORDERED sequence of [`StepOutcome`]s: the brain's decision at each model turn. The mock replays
/// `steps[n]` at the `n`-th model turn (0-indexed), so a script
/// `[UseTools([search]), UseTools([read]), Submit("done")]` drives a three-turn run
/// (two tool turns, then the terminal submit).
///
/// **Deterministic by construction:** the same script + the same conversation position always
/// yields the same outcome. The script is a value (clonable, serde — a golden fixture is a recorded
/// `MockScript` + its replayed [`StepOutcome`] stream).
///
/// **Well-formed scripts END in [`StepOutcome::Submit`]** — a script that runs off its end without
/// a terminal `Submit` is a bug the loop surfaces (the brain MUST terminate the bounded loop). The
/// mock replays a defensive terminal `Submit` past the end so a malformed/over-stepped script never
/// hangs the loop — but [`MockScript::is_well_formed`] flags it so a drill catches the authoring bug.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockScript {
    /// The system framing the rebuilt conversation opens with (the task / role / labelled-as-agent
    /// notice, §2.1). Part of the script so a golden fixture pins the whole conversation, not just
    /// the decisions.
    system: SystemContext,
    /// The scoped tool list the rebuilt conversation exposes (already permission/delegation-scoped,
    /// §5.2 — the N+1-free push-down that computes the subset is AG-P7 → P-219; here the script
    /// names them so the conversation reconstruction is complete).
    tools: Vec<ToolSchema>,
    /// The remaining-reserve view the conversation carries (so a golden fixture pins the budget the
    /// brain reads, §2.1).
    budget: BudgetView,
    /// The ordered brain decisions — `steps[n]` is replayed at the `n`-th model turn.
    steps: Vec<StepOutcome>,
}

impl MockScript {
    /// Build a script from its `system` framing, `tools` scope, `budget`, and the ordered `steps`.
    pub fn new(
        system: SystemContext,
        tools: Vec<ToolSchema>,
        budget: BudgetView,
        steps: Vec<StepOutcome>,
    ) -> MockScript {
        MockScript {
            system,
            tools,
            budget,
            steps,
        }
    }

    /// A minimal single-turn script: a system framing + one terminal [`StepOutcome::Submit`] with
    /// `answer` (no tools). The simplest well-formed script — drives a one-turn run.
    pub fn submit_only(system: impl Into<String>, answer: impl Into<String>) -> MockScript {
        MockScript {
            system: SystemContext(system.into()),
            tools: Vec::new(),
            budget: BudgetView(0),
            steps: vec![StepOutcome::Submit(Submission(answer.into()))],
        }
    }

    /// The scripted step at model-turn index `n` (0-indexed). Past the end of the script the mock
    /// replays a defensive terminal [`StepOutcome::Submit`] (so an over-stepped loop never hangs);
    /// [`is_well_formed`](MockScript::is_well_formed) flags such a script as a bug.
    fn step_at(&self, n: usize) -> StepOutcome {
        match self.steps.get(n) {
            Some(step) => step.clone(),
            None => StepOutcome::Submit(Submission(
                // The defensive terminal submit — a well-formed script never reaches here. Loud
                // marker so a golden/drill assertion that the script ran off its end is catchable.
                "mock: script exhausted — defensive terminal submit (script not well-formed)"
                    .into(),
            )),
        }
    }

    /// The number of scripted steps.
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Whether the script is empty (no steps — never terminates on its own).
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// **Whether the script is WELL-FORMED: non-empty AND its LAST step is a terminal
    /// [`StepOutcome::Submit`].** A well-formed script terminates the bounded loop without the
    /// defensive fallback. A drill asserts every committed fixture is well-formed (the brain MUST
    /// terminate — a script that only ever `UseTools` would loop to the `max_steps` ceiling).
    pub fn is_well_formed(&self) -> bool {
        matches!(self.steps.last(), Some(StepOutcome::Submit(_)))
    }

    /// The system framing the rebuilt conversation opens with.
    pub fn system(&self) -> &SystemContext {
        &self.system
    }

    /// The scoped tool list the rebuilt conversation exposes.
    pub fn tools(&self) -> &[ToolSchema] {
        &self.tools
    }

    /// The budget view the rebuilt conversation carries.
    pub fn budget(&self) -> &BudgetView {
        &self.budget
    }
}

// ───────────────────────── §3.2 — the deterministic scripted brain ─────────────────────────

/// **8.3 — the [`MockAgentRuntime`] (a deterministic scripted brain).** Its
/// [`step`](AgentRuntime::step) returns the next scripted [`StepOutcome`] for the conversation's
/// current position. **Stateless** (§2.1): it holds NO cursor — it reads the WHOLE [`Conversation`]
/// and counts the model turns already taken ([`model_turns_taken`]), then replays
/// `script.step_at(n)`. The platform owns history; the brain is a pure function of the conversation
/// (so two runs over the same history are byte-identical — AG-D9).
///
/// This is the §3.2 mock on the real `--use-mock` path: the SAME `&dyn AgentRuntime` seam the
/// SKELETON brain ([`SkeletonAgentRuntime`](crate::skeleton::SkeletonAgentRuntime)) and the future
/// `LlmAgentRuntime` (AG-P25) plug into. NO model/SDK/prompt/model-name string appears here (the
/// `no-llm-in-platform` ratchet, contract 1.6) — the brain is a scripted queue, nothing more.
///
/// **Floor (named):** this is the MOCK brain — THE v1 floor (VISION §3). The real vendor brain is
/// `LlmAgentRuntime` (AG-P25, post-M5), a config/impl swap behind this frozen seam.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MockAgentRuntime {
    /// The scripted decisions this brain replays (deterministic; the platform owns the history the
    /// brain reads to decide its position).
    script: MockScript,
}

impl MockAgentRuntime {
    /// Build a mock brain over a fixture `script`.
    pub fn new(script: MockScript) -> MockAgentRuntime {
        MockAgentRuntime { script }
    }

    /// The script this brain replays (for a golden fixture / a drill to inspect).
    pub fn script(&self) -> &MockScript {
        &self.script
    }
}

/// **Count the model turns already taken in `conv` (the stateless brain's position cursor, §2.1).**
/// The brain holds NO cursor of its own — it derives its position from the platform-owned history:
/// the number of [`Turn::Model`] turns already in the conversation IS the index of the NEXT decision
/// to replay. (A run that has taken `n` model steps has `n` `Model` turns; the next step is the
/// `n`-th, 0-indexed.) Pure function of the conversation → deterministic.
pub fn model_turns_taken(conv: &Conversation) -> usize {
    conv.turns
        .iter()
        .filter(|t| matches!(t, Turn::Model(_)))
        .count()
}

impl AgentRuntime for MockAgentRuntime {
    /// Replay the scripted decision for the conversation's current position. **Stateless**: the
    /// position is [`model_turns_taken`] (the count of prior model turns in the platform-owned
    /// history), NOT an internal cursor. Two `step` calls over the SAME conversation return the SAME
    /// outcome (idempotent — the brain is a pure function; AG-D9).
    fn step(&self, conv: &Conversation) -> StepOutcome {
        let n = model_turns_taken(conv);
        self.script.step_at(n)
    }
}

/// The Mock is a scripted brain with no model, so it has no usage source: it inherits the default
/// [`MeteredRuntime::step_metered`] (the scripted decision + [`myelin_agent::TokenUsage::NotReported`]).
/// Explicit (not blanket) so the vendor `LlmAgentRuntime` override does not collide under coherence.
impl MeteredRuntime for MockAgentRuntime {}

// ───────────────────────── §2.1 — the platform-owned history (the trace) ─────────────────────────

/// **The platform-owned conversation history a run reconstructs from (§2.1 — the trace; a
/// `PersonalDataHolder`, residency-pinned).** The brain is **stateless**: it never holds history;
/// the platform reconstructs the [`Conversation`] from THIS log via [`build_conversation`]. Each
/// [`HistoryEntry`] is one prior model step or its tool results — the running transcript.
///
/// **Floor (named):** the durable, content-addressed trace document + its erasure are the trace
/// holder's (the schema is AG-P2; the Knowledge write is AG-P19 → P-268). Here [`TraceHistory`] is
/// the in-memory shape the loop's `build_conversation` reconstructs from; the SKELETON (AG-P4)
/// writes the durable `trace` row + the `run.trace_ref` `ArtifactRef`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TraceHistory {
    entries: Vec<HistoryEntry>,
}

/// One entry in the platform-owned history (§2.1): a prior model step the brain emitted, or the tool
/// results the platform routed back. The transcript the [`Conversation`] is rebuilt from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryEntry {
    /// A prior model step (the brain's recorded decision for that turn).
    Model(StepOutcome),
    /// The results of the tool calls the platform routed for the prior step, each linked back to its
    /// call by id.
    ToolResults(Vec<ToolOutcome>),
}

impl TraceHistory {
    /// A fresh, empty history (an opening run — no prior turns).
    pub fn new() -> TraceHistory {
        TraceHistory::default()
    }

    /// Append a model step the brain emitted (record it into the platform-owned history).
    pub fn push_model(&mut self, step: StepOutcome) {
        self.entries.push(HistoryEntry::Model(step));
    }

    /// Append the tool results the platform routed for the prior step (each linked to its call).
    pub fn push_tool_results(&mut self, results: Vec<ToolOutcome>) {
        self.entries.push(HistoryEntry::ToolResults(results));
    }

    /// The number of recorded entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the history is empty (an opening run).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// **`build_conversation` (§2.1/§2.3) — reconstruct the [`Conversation`] the stateless brain reads
/// from the platform-owned history.** The loop's `build_conversation` seam: the system framing + the
/// scoped tool list + the budget view (from the [`MockScript`]) + the running transcript (the prior
/// model steps + tool results, from [`TraceHistory`]). The brain is passed the `&Conversation` and
/// holds NO state — the platform owns truth.
///
/// **Deterministic:** the same `(script, history)` always rebuilds a byte-identical [`Conversation`]
/// (the AG-D9 conversation-reconstruction leg). The transcript order is the history order: a `Model`
/// entry becomes a [`Turn::Model`], a `ToolResults` entry becomes a [`Turn::ToolResults`].
pub fn build_conversation(script: &MockScript, history: &TraceHistory) -> Conversation {
    let turns = history
        .entries
        .iter()
        .map(|e| match e {
            HistoryEntry::Model(step) => Turn::Model(step.clone()),
            HistoryEntry::ToolResults(results) => Turn::ToolResults(results.clone()),
        })
        .collect();
    Conversation {
        system: script.system.clone(),
        turns,
        tools: script.tools.clone(),
        budget: script.budget.clone(),
    }
}

// ───────────────────────── AG-D9 — the determinism lever (golden + mutants) ─────────────────────

/// **The recorded outcome of a full scripted replay (the AG-D9 golden artifact).** The
/// [`StepOutcome`] stream the brain emitted, the reconstructed [`Conversation`]s it read at each
/// turn, and how the run terminated. A golden test asserts two replays produce a **byte-identical**
/// [`ReplayRecord`] (the step-determinism leg of AG-D9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayRecord {
    /// The ordered [`StepOutcome`] stream the brain emitted (one per turn). The golden stream.
    pub outcomes: Vec<StepOutcome>,
    /// The reconstructed [`Conversation`] the brain read at each turn (the conversation-reconstruction
    /// leg — a renamed/dropped field or reordered transcript flips this).
    pub conversations: Vec<Conversation>,
    /// The terminal submission (the run's final answer) — `Some` for a run that ran to a `Submit`.
    pub submission: Option<Submission>,
    /// Whether the run terminated within the `max_steps` ceiling (a well-formed script always does;
    /// a non-terminating script trips the ceiling — surfaced, never hung).
    pub terminated: bool,
}

/// The default bounded `max_steps` ceiling the replay drives under (§2.3 — the bounded loop has
/// three independent ceilings; this is the step ceiling; the reserve/settle budget + the
/// causal-depth ceiling are the others, enforced by the SKELETON loop + the flow runtime). A
/// well-formed [`MockScript`] terminates far below this.
pub const MOCK_MAX_STEPS: usize = 64;

/// **Drive a [`MockScript`] to completion against a growing conversation and RECORD the
/// outcome (the AG-D9 step-determinism lever).** This is the loop body's brain-driving core
/// (§2.3 — `build_conversation` → step → append → step again), distilled so a golden test asserts
/// determinism: the same `script` always yields a byte-identical [`ReplayRecord`].
///
/// On each turn it: rebuilds the [`Conversation`] from the running [`TraceHistory`]
/// ([`build_conversation`]), steps the brain ([`MockAgentRuntime::step`]), records the outcome +
/// the conversation, appends the model step (and, on [`StepOutcome::UseTools`], appends DETERMINISTIC
/// scripted tool results so the next reconstruction is reproducible), and loops until a
/// [`StepOutcome::Submit`] or the `max_steps` ceiling. Bounded — a non-terminating script trips the
/// ceiling (`terminated == false`), it never hangs.
///
/// **Tool results are deterministic by construction:** the mock fabricates `tool:<name>:result` per
/// requested [`ToolCall`] — pure-function of the call, so the reconstruction is reproducible. The
/// REAL tool execution (the hands / the apply pipeline) is AG-P6/AG-P15 — here the scripted result
/// keeps the conversation-reconstruction deterministic without a sandbox.
pub fn replay(script: &MockScript) -> ReplayRecord {
    let brain = MockAgentRuntime::new(script.clone());
    replay_bounded(&brain, script, MOCK_MAX_STEPS)
}

/// **Drive ANY `&dyn AgentRuntime` brain to completion under an explicit `max_steps` ceiling (the
/// loop body's brain-driving core).** Generic over the brain so the SAME loop drives the mock, the
/// SKELETON, or (later) the LLM brain — the only swappable part is the brain (§2.3). `framing`
/// supplies the conversation's system/tools/budget (the platform-owned framing); the brain decides.
/// A non-terminating brain (one that never `Submit`s) trips the ceiling (`terminated == false`) — the
/// bound is a real, provable second ceiling, NOT a convention; the loop never hangs.
pub fn replay_bounded(
    brain: &dyn AgentRuntime,
    framing: &MockScript,
    max_steps: usize,
) -> ReplayRecord {
    let script = framing;
    let mut history = TraceHistory::new();
    let mut outcomes = Vec::new();
    let mut conversations = Vec::new();
    let mut submission = None;
    let mut terminated = false;

    for _ in 0..max_steps {
        let conv = build_conversation(script, &history);
        let outcome = brain.step(&conv);
        outcomes.push(outcome.clone());
        conversations.push(conv);
        match &outcome {
            StepOutcome::Submit(s) => {
                // The brain terminated the bounded loop (a well-formed script ends here).
                history.push_model(outcome.clone());
                submission = Some(s.clone());
                terminated = true;
                break;
            }
            StepOutcome::UseTools(calls) => {
                // Record the model step, then route DETERMINISTIC scripted tool results back into
                // the history so the next conversation reconstruction is reproducible. The REAL
                // routing (read direct / compute|external → hands / mutate → EffectApi) is §5.0,
                // AG-P6/AG-P15 — here a pure-function result keeps the loop deterministic.
                history.push_model(outcome.clone());
                let results = scripted_tool_results(calls);
                history.push_tool_results(results);
            }
        }
    }

    ReplayRecord {
        outcomes,
        conversations,
        submission,
        terminated,
    }
}

/// Fabricate DETERMINISTIC tool results for the brain's requested calls (`tool:<name>:result` per
/// [`ToolCall`]) — a pure function of the calls so the conversation reconstruction is reproducible.
/// The REAL execution is the hands / the apply pipeline (AG-P6/AG-P15); this keeps AG-D9 sandbox-free.
fn scripted_tool_results(calls: &[ToolCall]) -> Vec<ToolOutcome> {
    calls
        .iter()
        .map(|call| ToolOutcome {
            // Link each scripted result back to its call by the model-minted id.
            call_id: call.id.clone(),
            result: ToolResult(format!("tool:{}:result", call.name.0)),
        })
        .collect()
}

// ───────────────────────── the real `--use-mock` runtime flag (8.3) ─────────────────────────

/// **The real `--use-mock` runtime flag (8.3 — a real flag, NOT a test-only stub).** Selects which
/// brain plugs into the SAME `&dyn AgentRuntime` seam the SKELETON loop drives. `--use-mock` is a
/// developer/dev runtime flag (EI-03 §3 — *the same path*); absent it, the SKELETON brain runs.
/// `LlmAgentRuntime` (AG-P25) becomes a third arm here, a config/impl swap behind the frozen seam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeFlag {
    /// The SKELETON brain (no model, no tools — immediate submit). The default when `--use-mock` is
    /// absent. Exercises the substrate at ~zero cost (AG-P4).
    Skeleton,
    /// The deterministic scripted MOCK brain (`--use-mock`). The v1 floor (VISION §3).
    UseMock,
}

impl RuntimeFlag {
    /// **Parse the `--use-mock` flag from a process arg list (the real flag, 8.3).** Returns
    /// [`RuntimeFlag::UseMock`] iff `--use-mock` is present, else [`RuntimeFlag::Skeleton`] (the
    /// default). A real flag on the same code path — not a `#[cfg(test)]` bypass.
    pub fn from_args<I, S>(args: I) -> RuntimeFlag
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        if args.into_iter().any(|a| a.as_ref() == "--use-mock") {
            RuntimeFlag::UseMock
        } else {
            RuntimeFlag::Skeleton
        }
    }

    /// Whether this flag selects the mock brain.
    pub fn is_mock(self) -> bool {
        matches!(self, RuntimeFlag::UseMock)
    }
}

/// **`select_runtime` — build the brain the `--use-mock` flag selects, behind the frozen
/// `&dyn AgentRuntime` seam (8.3).** Returns the [`AgentRuntime`] the SKELETON loop
/// ([`crate::skeleton::SkeletonAgent::handle_run`]) drives UNCHANGED — the mock is on the SAME
/// gateway/identity/dispatch/reserve/trace path, not a bypass (the AG-P5 gate: `--use-mock` drives
/// the full AG-P4 substrate path). When [`RuntimeFlag::UseMock`], the `script` is the brain's queue;
/// when [`RuntimeFlag::Skeleton`], the SKELETON's immediate-submit brain runs (the `script` is
/// unused).
///
/// The return is a `Box<dyn AgentRuntime + Send + Sync>` so the dispatch tier holds ONE brain handle
/// regardless of which runtime the flag selected — the swap is a value, never a code change (VISION
/// §3). `LlmAgentRuntime` (AG-P25) slots in here as a third arm with no caller change.
pub fn select_runtime(
    flag: RuntimeFlag,
    script: MockScript,
) -> Box<dyn AgentRuntime + Send + Sync> {
    match flag {
        RuntimeFlag::UseMock => Box::new(MockAgentRuntime::new(script)),
        RuntimeFlag::Skeleton => Box::new(crate::skeleton::SkeletonAgentRuntime::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_agent::{Submission, ToolCallId, ToolName};

    /// A name-only scoped tool schema (empty description + permissive schema) — the fields the
    /// widened seam carries; these tests only exercise the tool name.
    fn schema(name: &str) -> ToolSchema {
        ToolSchema {
            name: ToolName(name.into()),
            description: String::new(),
            input_schema: "{}".into(),
        }
    }

    /// A tool call with a deterministic id and null arguments — the scripted brain chooses no real
    /// arguments; the id links its later [`ToolOutcome`] back (see [`outcome`]).
    fn call(name: &str) -> ToolCall {
        ToolCall {
            id: ToolCallId(format!("call:{name}")),
            name: ToolName(name.into()),
            arguments: serde_json::Value::Null,
        }
    }

    /// The deterministic scripted [`ToolOutcome`] `scripted_tool_results` produces for [`call(name)`]
    /// — the result `tool:<name>:result` keyed back to that call's id.
    fn outcome(name: &str) -> ToolOutcome {
        ToolOutcome {
            call_id: ToolCallId(format!("call:{name}")),
            result: ToolResult(format!("tool:{name}:result")),
        }
    }

    fn search_then_read_then_submit() -> MockScript {
        // A three-turn script: search, then read, then submit — the canonical multi-turn run.
        MockScript::new(
            SystemContext("you are agent-7; you are labelled as an agent".into()),
            vec![schema("search"), schema("read")],
            BudgetView(100),
            vec![
                StepOutcome::UseTools(vec![call("search")]),
                StepOutcome::UseTools(vec![call("read")]),
                StepOutcome::Submit(Submission("the answer".into())),
            ],
        )
    }

    /// **8.3 — the mock brain is STATELESS: `step` is a pure function of the conversation (§2.1).**
    /// Two `step` calls over the SAME conversation return the SAME outcome (no internal cursor); the
    /// outcome is the script entry for the conversation's model-turn position.
    #[test]
    fn mock_step_is_stateless_pure_function_of_the_conversation() {
        let brain = MockAgentRuntime::new(search_then_read_then_submit());

        // an opening conversation (0 model turns) → the FIRST scripted step (search).
        let opening = Conversation::default();
        let a = brain.step(&opening);
        let b = brain.step(&opening); // same conversation → SAME outcome (stateless).
        assert_eq!(
            a, b,
            "the brain holds no cursor — same conversation, same outcome"
        );
        assert_eq!(
            a,
            StepOutcome::UseTools(vec![call("search")]),
            "the opening turn replays step[0] (search)"
        );

        // after ONE model turn → step[1] (read).
        let mut later = Conversation::default();
        later.turns.push(Turn::Model(a.clone()));
        assert_eq!(
            brain.step(&later),
            StepOutcome::UseTools(vec![call("read")]),
            "one model turn taken → replay step[1] (read)"
        );
    }

    /// **`model_turns_taken` counts ONLY model turns (the stateless position cursor).** Tool-result
    /// + approval turns do NOT advance the brain's position — only `Turn::Model` does.
    #[test]
    fn model_turns_taken_counts_only_model_turns() {
        let mut conv = Conversation::default();
        assert_eq!(
            model_turns_taken(&conv),
            0,
            "an opening conversation is at position 0"
        );
        conv.turns
            .push(Turn::Model(StepOutcome::Submit(Submission("a".into()))));
        assert_eq!(model_turns_taken(&conv), 1, "one model turn → position 1");
        conv.turns.push(Turn::ToolResults(vec![ToolOutcome {
            call_id: ToolCallId("call:r".into()),
            result: ToolResult("r".into()),
        }]));
        assert_eq!(
            model_turns_taken(&conv),
            1,
            "a tool-result turn does NOT advance the position"
        );
        conv.turns
            .push(Turn::Model(StepOutcome::Submit(Submission("b".into()))));
        assert_eq!(
            model_turns_taken(&conv),
            2,
            "a second model turn → position 2"
        );
    }

    /// **AG-D9 (the step-determinism leg) — the GOLDEN test: the SAME script replays to a
    /// byte-identical [`ReplayRecord`] across two runs.** Two `replay`s produce identical
    /// `StepOutcome` streams + identical conversation reconstructions (the AG-4 lever; a mutant that
    /// perturbs the brain/loop seam flips this).
    #[test]
    fn ag_d9_replay_is_byte_identical_across_two_runs() {
        let script = search_then_read_then_submit();
        let first = replay(&script);
        let second = replay(&script);
        assert_eq!(
            first, second,
            "AG-D9: two replays of the same script are byte-identical"
        );

        // the recorded stream is exactly the script (search, read, submit), in order.
        assert_eq!(
            first.outcomes,
            vec![
                StepOutcome::UseTools(vec![call("search")]),
                StepOutcome::UseTools(vec![call("read")]),
                StepOutcome::Submit(Submission("the answer".into())),
            ],
            "the replayed StepOutcome stream IS the script, in order"
        );
        assert!(
            first.terminated,
            "a well-formed script terminates the bounded loop"
        );
        assert_eq!(
            first.submission,
            Some(Submission("the answer".into())),
            "the terminal submission is the script's final answer"
        );
    }

    /// **AG-D9 (the conversation-reconstruction leg) — the rebuilt conversations GROW the platform-
    /// owned transcript deterministically.** At each turn `build_conversation` reconstructs the same
    /// conversation; the model steps + the routed tool results are appended in order.
    #[test]
    fn ag_d9_conversation_reconstruction_grows_the_transcript_deterministically() {
        let script = search_then_read_then_submit();
        let rec = replay(&script);

        // three turns recorded (search, read, submit).
        assert_eq!(
            rec.conversations.len(),
            3,
            "three turns → three reconstructed conversations"
        );

        // turn 0: an opening conversation (no prior turns), the script's system + tools + budget.
        let c0 = &rec.conversations[0];
        assert!(c0.turns.is_empty(), "turn 0 opens with an empty transcript");
        assert_eq!(
            c0.system,
            SystemContext("you are agent-7; you are labelled as an agent".into())
        );
        assert_eq!(
            c0.tools.len(),
            2,
            "the scoped tool list is rebuilt from the script"
        );
        assert_eq!(
            c0.budget,
            BudgetView(100),
            "the budget view is rebuilt from the script"
        );

        // turn 1: the transcript now holds [Model(search), ToolResults(tool:search:result)].
        let c1 = &rec.conversations[1];
        assert_eq!(
            c1.turns.len(),
            2,
            "turn 1 sees the search step + its routed tool result"
        );
        assert_eq!(
            c1.turns[0],
            Turn::Model(StepOutcome::UseTools(vec![call("search")]))
        );
        assert_eq!(
            c1.turns[1],
            Turn::ToolResults(vec![outcome("search")]),
            "the routed tool result is DETERMINISTIC (tool:<name>:result), linked to its call id"
        );

        // turn 2 (the submit turn): the transcript holds search+result, read+result (4 turns).
        let c2 = &rec.conversations[2];
        assert_eq!(c2.turns.len(), 4, "turn 2 sees both prior tool round-trips");
    }

    /// **A non-terminating brain trips the bounded ceiling (§2.3 — the loop is bounded; it never
    /// hangs).** A brain that ALWAYS returns `UseTools` (never `Submit`) runs to `max_steps` and
    /// records `terminated == false` — the bound is a real, provable second ceiling. A well-formed
    /// scripted brain terminates BELOW the ceiling.
    #[test]
    fn non_terminating_brain_trips_the_bounded_ceiling() {
        // A genuinely non-terminating brain: it ALWAYS proposes a tool, regardless of position. The
        // positional mock cannot loop forever (its defensive submit terminates it), so the ceiling is
        // proven with a real always-UseTools brain — the bound holds for ANY &dyn AgentRuntime.
        struct NeverSubmits;
        impl AgentRuntime for NeverSubmits {
            fn step(&self, _conv: &Conversation) -> StepOutcome {
                StepOutcome::UseTools(vec![call("loop")])
            }
        }
        let framing = MockScript::new(SystemContext("sys".into()), vec![], BudgetView(0), vec![]);
        let rec = replay_bounded(&NeverSubmits, &framing, /* max_steps */ 5);
        assert!(
            !rec.terminated,
            "a non-terminating brain trips the ceiling (never hangs)"
        );
        assert_eq!(
            rec.outcomes.len(),
            5,
            "it ran exactly max_steps turns, then stopped (bounded)"
        );
        assert_eq!(
            rec.submission, None,
            "no terminal submission (it never submitted)"
        );

        // a well-formed scripted brain terminates BELOW the ceiling.
        let good = search_then_read_then_submit();
        assert!(
            good.is_well_formed(),
            "search→read→submit ends in a terminal Submit"
        );
        let good_brain = MockAgentRuntime::new(good.clone());
        let good_rec = replay_bounded(&good_brain, &good, 64);
        assert!(
            good_rec.terminated,
            "a well-formed scripted brain terminates the bounded loop"
        );
    }

    /// **An over-stepped script replays a defensive terminal Submit past its end (never hangs), and
    /// `is_well_formed` flags the authoring bug.** `step_at(n)` past the end yields a loud terminal
    /// Submit — the bounded loop still terminates even on a malformed fixture.
    #[test]
    fn script_exhaustion_is_a_defensive_terminal_submit() {
        let script = MockScript::submit_only("sys", "done");
        // step 0 is the submit; step 1 (past the end) is the defensive terminal submit.
        assert!(matches!(
            script.step_at(0),
            StepOutcome::Submit(Submission(ref s)) if s == "done"
        ));
        match script.step_at(1) {
            StepOutcome::Submit(Submission(s)) => {
                assert!(
                    s.contains("script exhausted"),
                    "past-the-end is a LOUD defensive submit: {s}"
                );
            }
            other => panic!("expected a defensive terminal submit, got {other:?}"),
        }
        assert!(
            script.is_well_formed(),
            "submit_only IS well-formed (a single terminal Submit)"
        );
    }

    /// **8.3 — the real `--use-mock` flag selects the mock brain on the SAME `&dyn AgentRuntime`
    /// seam (NOT a test-only stub).** `--use-mock` present → the mock; absent → the SKELETON brain.
    #[test]
    fn use_mock_flag_selects_the_mock_brain_on_the_same_seam() {
        // absent → SKELETON (the default; immediate submit).
        let flag = RuntimeFlag::from_args(["myelin-agent", "serve"]);
        assert_eq!(
            flag,
            RuntimeFlag::Skeleton,
            "no --use-mock → the SKELETON brain (default)"
        );
        assert!(!flag.is_mock());

        // present → the mock.
        let flag = RuntimeFlag::from_args(["myelin-agent", "serve", "--use-mock"]);
        assert_eq!(
            flag,
            RuntimeFlag::UseMock,
            "--use-mock → the deterministic mock brain"
        );
        assert!(flag.is_mock());

        // select_runtime returns the brain behind the FROZEN &dyn AgentRuntime seam — the mock runs
        // on the SAME code path the SKELETON loop drives (not a bypass).
        let brain = select_runtime(RuntimeFlag::UseMock, MockScript::submit_only("sys", "ok"));
        assert_eq!(
            brain.step(&Conversation::default()),
            StepOutcome::Submit(Submission("ok".into())),
            "the selected mock brain replays its script through the &dyn seam"
        );

        // the SKELETON arm returns the SKELETON's immediate submit (the script is unused).
        let skel = select_runtime(
            RuntimeFlag::Skeleton,
            MockScript::submit_only("sys", "ignored"),
        );
        assert!(
            matches!(skel.step(&Conversation::default()), StepOutcome::Submit(_)),
            "the SKELETON arm submits immediately (no model, no tools)"
        );
    }

    /// **The mock brain drives the SAME SKELETON substrate path UNCHANGED (the AG-P5 gate:
    /// `--use-mock` is on the real code path, mint/reserve/trace/settle unchanged).** The mock plugs
    /// into [`crate::skeleton::SkeletonAgent::handle`] behind the `&dyn AgentRuntime` seam exactly as
    /// the SKELETON brain does — the loop body is identical, only the decision differs.
    #[test]
    fn mock_brain_drives_the_skeleton_loop_seam_unchanged() {
        use crate::skeleton::SkeletonAgent;
        use myelin_agent::{Agent, InboxEvent};
        let loop_body = SkeletonAgent::new();
        let brain = MockAgentRuntime::new(MockScript::submit_only("sys", "answer"));
        // the SAME Agent::handle the SKELETON brain drives — the brain is the only swapped part.
        let out = loop_body.handle(InboxEvent("issue.created".into()), &brain);
        assert!(
            out.0.contains("skeleton handle"),
            "the mock brain drives the SAME platform-owned loop seam (only the brain swapped): {out:?}"
        );
    }

    /// **`build_conversation` is deterministic: the same `(script, history)` rebuilds a byte-identical
    /// conversation (the AG-D9 reconstruction leg, isolated).** A renamed/dropped field or a reordered
    /// transcript flips the equality.
    #[test]
    fn build_conversation_is_deterministic() {
        let script = search_then_read_then_submit();
        let mut history = TraceHistory::new();
        history.push_model(StepOutcome::UseTools(vec![call("search")]));
        history.push_tool_results(vec![outcome("search")]);

        let a = build_conversation(&script, &history);
        let b = build_conversation(&script, &history);
        assert_eq!(
            a, b,
            "the same (script, history) rebuilds a byte-identical conversation"
        );
        assert_eq!(
            a.turns.len(),
            2,
            "the transcript carries the model step + the tool result"
        );
        assert_eq!(
            a.system,
            *script.system(),
            "the system framing is rebuilt from the script"
        );
        assert_eq!(
            a.tools,
            script.tools(),
            "the scoped tool list is rebuilt from the script"
        );
    }

    /// **`MockScript::is_well_formed` is exact (mutation-floor — a committed fixture MUST terminate).**
    /// non-empty + last == Submit ⇒ well-formed; empty or last == UseTools ⇒ NOT.
    #[test]
    fn is_well_formed_predicate_is_exact() {
        assert!(
            !MockScript::new(SystemContext("s".into()), vec![], BudgetView(0), vec![])
                .is_well_formed(),
            "an empty script is NOT well-formed (it never terminates)"
        );
        assert!(
            MockScript::submit_only("s", "x").is_well_formed(),
            "a single Submit IS well-formed"
        );
        let trailing_tools = MockScript::new(
            SystemContext("s".into()),
            vec![],
            BudgetView(0),
            vec![
                StepOutcome::Submit(Submission("early".into())),
                StepOutcome::UseTools(vec![call("t")]),
            ],
        );
        assert!(
            !trailing_tools.is_well_formed(),
            "a script ending in UseTools is NOT well-formed"
        );
    }

    /// **The `MockScript` / `TraceHistory` accessors are EXACT (mutation-floor — kills the
    /// `len -> 0/1`, `is_empty -> true/false`, `budget -> default` constant mutants).** Each accessor
    /// returns ITS value distinguishably across non-trivial inputs, so a constant-return mutant flips
    /// an assertion. The brain/loop seam reads these (the loop counts turns; the well-formed gate
    /// reads len); a constant accessor would silently corrupt the loop, so it must be killed.
    #[test]
    fn script_and_history_accessors_are_exact() {
        // MockScript::len / is_empty — distinct non-trivial values kill `-> 0`, `-> 1`, `-> true/false`.
        let empty = MockScript::new(SystemContext("s".into()), vec![], BudgetView(0), vec![]);
        assert_eq!(empty.len(), 0, "an empty script has len 0 (kills -> 1)");
        assert!(
            empty.is_empty(),
            "an empty script is_empty (kills -> false)"
        );
        let three = search_then_read_then_submit();
        assert_eq!(
            three.len(),
            3,
            "a three-step script has len 3 (kills -> 0 / -> 1)"
        );
        assert!(
            !three.is_empty(),
            "a non-empty script is NOT is_empty (kills -> true)"
        );

        // MockScript::budget — a non-default value kills the `-> default()` mutant.
        assert_eq!(
            *three.budget(),
            BudgetView(100),
            "budget() returns its field (kills -> default)"
        );
        assert_ne!(
            *three.budget(),
            BudgetView::default(),
            "the budget is non-default (kills -> default)"
        );

        // TraceHistory::len / is_empty — distinct non-trivial values kill the constant mutants.
        let mut h = TraceHistory::new();
        assert_eq!(h.len(), 0, "a fresh history has len 0 (kills -> 1)");
        assert!(h.is_empty(), "a fresh history is_empty (kills -> false)");
        h.push_model(StepOutcome::Submit(Submission("a".into())));
        h.push_tool_results(vec![outcome("r")]);
        assert_eq!(h.len(), 2, "two entries → len 2 (kills -> 0 / -> 1)");
        assert!(
            !h.is_empty(),
            "a non-empty history is NOT is_empty (kills -> true)"
        );
    }
}

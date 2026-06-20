//! # The CDC pair for contract 8.5 — `Agent::handle(InboxEvent, &dyn AgentRuntime) -> RunOutcome`
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 8.5
//! (`Agent::handle(InboxEvent, &dyn AgentRuntime) → RunOutcome` — platform-owned bounded multi-turn
//! loop; nested causality; a run is a durable workflow). Owning architecture: `agent-fabric.md`
//! §2.3. AG-P1 / P-130 ships the SIGNATURE half; the loop body (build_conversation → reserve →
//! repeatedly `step` → route → settle) lands with the SKELETON runtime (AG-P4 → P-216).
//!
//! ## What this pair pins (the signature half of 8.5)
//! - the **PROVIDER** is the agent fabric's platform-owned loop: `handle` drives the brain through
//!   the `&dyn AgentRuntime` seam (the strategy boundary — the brain is dynamically dispatched and
//!   swappable for mock/real) and returns a `RunOutcome`.
//! - the **CONSUMER** is the dispatch tier (the Bus): it delivers an `InboxEvent` and a runtime
//!   choice (mock or real) and reads the run outcome. The loop is platform-owned, NOT a strategy.

use myelin_agent::{
    Agent, AgentRuntime, Conversation, InboxEvent, RunOutcome, StepOutcome, Submission,
};

/// A deterministic runtime the loop drives (the swappable brain behind the `&dyn` seam).
struct SubmitRuntime;
impl AgentRuntime for SubmitRuntime {
    fn step(&self, _conv: &Conversation) -> StepOutcome {
        StepOutcome::Submit(Submission("done".into()))
    }
}

/// **PROVIDER side of 8.5 (agent fabric).** The platform-owned loop: it drives the brain (here one
/// `step`) through the `&dyn AgentRuntime` seam and returns a `RunOutcome`. The bounded multi-turn
/// body lands in AG-P4 (→ P-216); this pins that the brain is dynamically dispatched + swappable.
struct ProviderLoop;
impl Agent for ProviderLoop {
    fn handle(&self, _inbox: InboxEvent, runtime: &dyn AgentRuntime) -> RunOutcome {
        match runtime.step(&Conversation::default()) {
            StepOutcome::Submit(Submission(s)) => RunOutcome(format!("submitted:{s}")),
            StepOutcome::UseTools(_) => RunOutcome("used-tools".into()),
        }
    }
}

#[test]
fn cdc_8_5_handle_drives_the_brain_through_the_dyn_seam() {
    let loop_provider = ProviderLoop;
    // CONSUMER (the dispatch tier): deliver an event + a chosen runtime (the brain is swappable).
    let runtime = SubmitRuntime;
    let out = loop_provider.handle(InboxEvent("mention".into()), &runtime);
    assert_eq!(out, RunOutcome("submitted:done".into()));
}

//! # The CDC pair for contract 8.2 — `EffectApi::apply(run, ProposedEffect) -> EffectResult`
//!
//! **Contract:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 8.2
//! (`EffectApi::apply(run, ProposedEffect) → Applied(event_id) | Gated(gate_id) | Denied(reason)` —
//! plan-then-apply: schema → capability → delegation → tenant → budget → HITL gate → apply via the
//! public endpoint → meter; Denied = ordinary tool error; a withheld gated tool does not mutate,
//! AG-8). Owning architecture: `agent-fabric.md` §5.2. AG-P1 / P-130 ships the SIGNATURE half; the
//! eight-step pipeline body (AG-D1/D2/D3) is AG-P6 (→ P-218).
//!
//! ## What this pair pins (the signature half of 8.2)
//! - the **PROVIDER** is the agent fabric's platform-owned write-back path: `apply` returns exactly
//!   one of the three frozen outcomes; a gated effect does NOT mutate (it returns `Gated`).
//! - the **CONSUMER** is the loop / external MCP / a workflow activity: it hands a `ProposedEffect`
//!   under a `RunCtx` and branches on `Applied | Gated | Denied` — agents NEVER mutate directly.

use myelin_agent::{EffectApi, EffectResult, EventId, GateId, ProposedEffect, RunCtx};

/// **PROVIDER side of 8.2 (agent fabric).** A scripted plan-then-apply that routes a mutate effect
/// to `Applied`, a gated effect to `Gated` (does not mutate), and a forbidden effect to `Denied`.
/// The eight-step pipeline body lands in AG-P6 (→ P-218); this is the outcome-shape provider.
struct ProviderEffectApi;

impl EffectApi for ProviderEffectApi {
    fn apply(&self, _run: &RunCtx, effect: ProposedEffect) -> EffectResult {
        match effect.0.as_str() {
            "gated" => EffectResult::Gated(GateId("card:1:0".into())),
            "denied" => EffectResult::Denied("missing capability".into()),
            other => EffectResult::Applied(EventId(format!("evt-{other}"))),
        }
    }
}

#[test]
fn cdc_8_2_apply_returns_one_of_three_frozen_outcomes() {
    let provider = ProviderEffectApi;
    let run = RunCtx::default();

    // CONSUMER: an applied mutation carries the emitted event id.
    match provider.apply(&run, ProposedEffect("close".into())) {
        EffectResult::Applied(EventId(id)) => assert_eq!(id, "evt-close"),
        other => panic!("expected Applied, got {other:?}"),
    }

    // CONSUMER: a gated effect is WITHHELD (carries a gate id, does not mutate, AG-8).
    match provider.apply(&run, ProposedEffect("gated".into())) {
        EffectResult::Gated(GateId(id)) => assert_eq!(id, "card:1:0"),
        other => panic!("expected Gated, got {other:?}"),
    }

    // CONSUMER: a denied effect is an ordinary tool error (carries a reason).
    match provider.apply(&run, ProposedEffect("denied".into())) {
        EffectResult::Denied(reason) => assert_eq!(reason, "missing capability"),
        other => panic!("expected Denied, got {other:?}"),
    }
}

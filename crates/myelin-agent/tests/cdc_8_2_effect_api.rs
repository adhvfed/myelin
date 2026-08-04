use myelin_agent::{EffectApi, EffectResult, EventId, GateId, ProposedEffect, RunCtx};

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

    match provider.apply(&run, ProposedEffect("close".into())) {
        EffectResult::Applied(EventId(id)) => assert_eq!(id, "evt-close"),
        other => panic!("expected Applied, got {other:?}"),
    }

    match provider.apply(&run, ProposedEffect("gated".into())) {
        EffectResult::Gated(GateId(id)) => assert_eq!(id, "card:1:0"),
        other => panic!("expected Gated, got {other:?}"),
    }

    match provider.apply(&run, ProposedEffect("denied".into())) {
        EffectResult::Denied(reason) => assert_eq!(reason, "missing capability"),
        other => panic!("expected Denied, got {other:?}"),
    }
}

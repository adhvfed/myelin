use myelin_agent::{EffectApi, EffectAuthority, EffectResult, ProposedEffect, RunCtx};

pub struct ApplyingEffectApi;

impl EffectApi for ApplyingEffectApi {
    fn apply(&self, run: &RunCtx, _effect: ProposedEffect) -> EffectResult {
        EffectResult::Applied(myelin_agent::EventId(format!("evt:{}", run.0)))
    }

    fn apply_authorized(
        &self,
        run: &RunCtx,
        _authority: &EffectAuthority,
        effect: ProposedEffect,
    ) -> EffectResult {
        self.apply(run, effect)
    }
}

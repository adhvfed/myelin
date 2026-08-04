use myelin_agent::{EffectApi, EffectResult, EventId, ProposedEffect, RunCtx};
use myelin_events::{IdMinter, MonotonicMinter};
use myelin_flow::{
    apply_approved_effects, per_effect_idem_key, ApprovalCard, ApprovalDecision, DurableExecutor,
    EffectOutcome, FlowExecutor, GatedEffect, RunBudget, RunId, SignalSpec, StartSpec,
    APPROVAL_SIGNAL_NAME, DECLINE_MARKER,
};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::cell::RefCell;
use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
}

fn executor() -> FlowExecutor {
    let ex = FlowExecutor::new(minter(), tenant(), region());
    ex.register_definition("agent.run");
    ex
}

fn start_a_run(ex: &FlowExecutor) -> RunId {
    ex.start(StartSpec {
        wf_type: "agent.run".into(),
        input: vec![],
        budget: Some(RunBudget { minor_units: 1_000 }),
        idem_key: "k".into(),
    })
    .expect("start")
}

struct RecordingEffectApi {
    applied: RefCell<Vec<String>>,
}

impl EffectApi for RecordingEffectApi {
    fn apply(&self, _run: &RunCtx, effect: ProposedEffect) -> EffectResult {
        self.applied.borrow_mut().push(effect.0.clone());
        EffectResult::Applied(EventId(format!("evt-for-{}", effect.0)))
    }
}

fn approve(ex: &FlowExecutor, run: &RunId, card_id: &str, idx: usize, total: usize) {
    ex.signal(SignalSpec {
        run: run.clone(),
        signal_name: APPROVAL_SIGNAL_NAME.into(),
        idem_key: per_effect_idem_key(card_id, idx, total),
        payload: vec![ArtifactRef(format!(
            "myelin://acme/agent/effect/{card_id}-{idx}"
        ))],
        payload_key_ref: None,
    })
    .expect("approve");
}

fn decline(ex: &FlowExecutor, run: &RunId, card_id: &str, idx: usize, total: usize) {
    ex.signal(SignalSpec {
        run: run.clone(),
        signal_name: APPROVAL_SIGNAL_NAME.into(),
        idem_key: per_effect_idem_key(card_id, idx, total),
        payload: vec![],
        payload_key_ref: Some(DECLINE_MARKER.into()),
    })
    .expect("decline");
}

fn card(run: &RunId, decisions: [ApprovalDecision; 3]) -> ApprovalCard {
    ApprovalCard {
        run_id: run.0.clone(),
        card_id: "card-7".into(),
        effects: decisions
            .iter()
            .enumerate()
            .map(|(i, d)| GatedEffect {
                effect_ref: ArtifactRef(format!("myelin://acme/agent/effect/e{i}")),
                decision: *d,
            })
            .collect(),
    }
}

#[test]
fn partial_approval_maps_each_approved_effect_to_exactly_one_apply() {
    let ex = executor();
    let run = start_a_run(&ex);
    approve(&ex, &run, "card-7", 0, 3);
    decline(&ex, &run, "card-7", 1, 3);
    approve(&ex, &run, "card-7", 2, 3);
    assert_eq!(ex.signals().count_for_run(&tenant(), &run.0), 3);

    let consumer = RecordingEffectApi {
        applied: RefCell::new(vec![]),
    };
    let run_ctx = RunCtx::default();
    let outcomes = apply_approved_effects(
        ex.signals(),
        &tenant(),
        &card(
            &run,
            [
                ApprovalDecision::Approve,
                ApprovalDecision::Decline,
                ApprovalDecision::Approve,
            ],
        ),
        &|eff: &ArtifactRef| {
            match consumer.apply(&run_ctx, ProposedEffect(eff.0.clone())) {
                EffectResult::Applied(EventId(id)) => Ok(id),
                EffectResult::Gated(g) => Err(format!("gated:{}", g.0)),
                EffectResult::Denied(r) => Err(r),
            }
        },
    );

    assert!(matches!(outcomes[0], Some(Ok(EffectOutcome::Applied(_)))));
    assert_eq!(
        outcomes[1],
        Some(Ok(EffectOutcome::Withheld(DECLINE_MARKER.to_string())))
    );
    assert!(matches!(outcomes[2], Some(Ok(EffectOutcome::Applied(_)))));

    let applied = consumer.applied.into_inner();
    assert_eq!(
        applied.len(),
        2,
        "exactly two applies (effects 0 and 2); the declined effect 1 made 0 mutation"
    );
    assert!(
        !applied.contains(&"myelin://acme/agent/effect/e1".to_string()),
        "the DECLINED effect never reached EffectApi::apply (AG-8)"
    );
}

#[test]
fn double_click_drives_the_real_effect_api_exactly_once_per_effect() {
    let ex = executor();
    let run = start_a_run(&ex);
    for idx in 0..3 {
        approve(&ex, &run, "card-7", idx, 3);
    }
    for idx in 0..3 {
        approve(&ex, &run, "card-7", idx, 3);
    }
    assert_eq!(
        ex.signals().count_for_run(&tenant(), &run.0),
        3,
        "the double-click buffered nothing new"
    );

    let consumer = RecordingEffectApi {
        applied: RefCell::new(vec![]),
    };
    let run_ctx = RunCtx::default();
    apply_approved_effects(
        ex.signals(),
        &tenant(),
        &card(
            &run,
            [
                ApprovalDecision::Approve,
                ApprovalDecision::Approve,
                ApprovalDecision::Approve,
            ],
        ),
        &|eff: &ArtifactRef| match consumer.apply(&run_ctx, ProposedEffect(eff.0.clone())) {
            EffectResult::Applied(EventId(id)) => Ok(id),
            other => Err(format!("{other:?}")),
        },
    );
    assert_eq!(
        consumer.applied.into_inner().len(),
        3,
        "the real EffectApi::apply ran exactly 3 times (the double-click did not double-apply)"
    );
}

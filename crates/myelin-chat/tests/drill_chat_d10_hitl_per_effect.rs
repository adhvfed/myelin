//! # CHAT-D10 — the multi-effect HITL card per-effect drill (CHAT-P18 → P-413, M4-C6)
//!
//! **Drill (testing-strategy/01 row CHAT-D10):** a multi-effect card approved 2-of-3 → the 2 resume
//! APPROVED, the 1 WITHHELD, each independent `idem_key = card_id:<idx>`; no effect runs twice; the
//! withheld never mutates. CI; the per-effect duplicate + withheld-mutation signals = 0.
//!
//! **A CHAINED scenario (EI-01 §4):** the card service posts three INDEPENDENT per-effect decisions
//! onto the REAL engine; a double-click on "approve all" re-posts the SAME keys (the engine dedups);
//! the engine's gated loop (`apply_approved_effects`) applies the 2 EXACTLY once each and WITHHOLDS
//! the 1 (0 mutation). The CHAT face of FLOW-D4's per-effect half — chat owns the card; the apply +
//! the dedup are the ENGINE's.

use myelin_chat::hitl::{
    build_card_signal, CardClick, CardDecision, CardEffect, CardSignal, ChatApprovalCard,
    SignalDelivery, SignalPort, SignalPostError, DECLINE_MARKER,
};
use myelin_events::{IdMinter, MonotonicMinter};
use myelin_flow::{
    apply_approved_effects, ApprovalCard, ApprovalDecision, DurableExecutor, EffectOutcome,
    FlowExecutor, GatedEffect, RunBudget, RunId as FlowRunId, SignalOutcome, SignalSpec, StartSpec,
    APPROVAL_SIGNAL_NAME,
};
use myelin_identity::RunId as IdRunId;
use myelin_refs::ArtifactRef as RefArtifactRef;
use myelin_tenancy::{ArtifactRef, Region, TenantId};
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

/// chat's [`SignalPort`] over the REAL engine, posting the per-effect approval signals under the
/// engine's `approval` signal name (the §6.4 per-effect keys ride the `idem_key`).
struct FlowSignalPort {
    ex: FlowExecutor,
}
impl SignalPort for FlowSignalPort {
    fn post_signal(&self, signal: &CardSignal) -> Result<SignalDelivery, SignalPostError> {
        let outcome = self
            .ex
            .signal(SignalSpec {
                run: FlowRunId(signal.run_id.0.clone()),
                signal_name: signal.signal_name.clone(),
                idem_key: signal.idem_key.clone(),
                payload: signal.payload.clone(),
                payload_key_ref: signal.payload_key_ref.clone(),
            })
            .map_err(|e| SignalPostError {
                reason: format!("{e}"),
            })?;
        Ok(match outcome {
            SignalOutcome::Buffered => SignalDelivery::Buffered,
            SignalOutcome::Duplicate => SignalDelivery::Duplicate,
            SignalOutcome::TerminalNoOp => {
                unreachable!("the in-memory FlowExecutor buffers signals to terminal runs")
            }
        })
    }
}

fn executor() -> FlowExecutor {
    let ex = FlowExecutor::new(minter(), tenant(), region());
    ex.register_definition("agent.run");
    ex
}

fn start_run(ex: &FlowExecutor) -> FlowRunId {
    ex.start(StartSpec {
        wf_type: "agent.run".into(),
        input: vec![],
        budget: Some(RunBudget { minor_units: 1_000 }),
        idem_key: "k".into(),
    })
    .expect("start")
}

fn effect(refs: &str) -> CardEffect {
    CardEffect {
        subject: ArtifactRef(format!("myelin://acme/git/pr/{refs}")),
        action: format!("apply {refs}"),
        risk: "irreversible".into(),
        cost: "$0.10".into(),
        effect_refs: vec![ArtifactRef(format!("myelin://acme/agent/effect/{refs}"))],
    }
}

fn three_effect_card(run: &FlowRunId) -> ChatApprovalCard {
    ChatApprovalCard {
        run_id: IdRunId(run.0.clone()),
        card_id: "card-7".into(),
        // post under the engine's `approval` per-effect signal name (apply_approved_effects reads it).
        effects: vec![effect("e0"), effect("e1"), effect("e2")],
    }
}

/// chat posts the per-effect decision under the engine's `approval` signal name (the per-effect keys
/// ride the `idem_key` — the loop reads the buffered per-effect signals).
fn post_per_effect(
    port: &FlowSignalPort,
    card: &ChatApprovalCard,
    click: &CardClick,
) -> SignalDelivery {
    let mut sig: CardSignal = build_card_signal(card, click);
    sig.signal_name = APPROVAL_SIGNAL_NAME.to_string();
    port.post_signal(&sig).unwrap()
}

/// **CHAT-D10: a multi-effect card approved 2-of-3 → the 2 resume APPROVED, the 1 WITHHELD, each
/// independent `card-7:<idx>`; no effect runs twice; the withheld never mutates.** Chat posts three
/// independent per-effect decisions; a DOUBLE-CLICK on "approve all" re-posts the same keys (the
/// engine dedups); the engine's gated loop applies effects 0 + 2 EXACTLY once and WITHHOLDS effect 1.
#[test]
fn chat_d10_partial_approval_two_of_three_per_effect_independent_zero_double_apply() {
    let ex = executor();
    let run = start_run(&ex);
    let card = three_effect_card(&run);
    let port = FlowSignalPort { ex: ex.clone() };

    // chat posts: approve 0, decline 1, approve 2 — three INDEPENDENT per-effect signals.
    let d0 = post_per_effect(
        &port,
        &card,
        &CardClick {
            effect_idx: 0,
            decision: CardDecision::Approve,
            decline_reason: String::new(),
        },
    );
    let d1 = post_per_effect(
        &port,
        &card,
        &CardClick {
            effect_idx: 1,
            decision: CardDecision::Decline,
            decline_reason: DECLINE_MARKER.into(),
        },
    );
    let d2 = post_per_effect(
        &port,
        &card,
        &CardClick {
            effect_idx: 2,
            decision: CardDecision::Approve,
            decline_reason: String::new(),
        },
    );
    assert_eq!(d0, SignalDelivery::Buffered);
    assert_eq!(d1, SignalDelivery::Buffered);
    assert_eq!(d2, SignalDelivery::Buffered);

    // DOUBLE-CLICK "approve all": re-post the SAME per-effect keys → the engine DEDUPS (0 double).
    assert_eq!(
        post_per_effect(
            &port,
            &card,
            &CardClick {
                effect_idx: 0,
                decision: CardDecision::Approve,
                decline_reason: String::new()
            }
        ),
        SignalDelivery::Duplicate,
        "re-clicking effect 0 is a no-op (the per-effect key dedups)"
    );
    assert_eq!(
        post_per_effect(
            &port,
            &card,
            &CardClick {
                effect_idx: 2,
                decision: CardDecision::Approve,
                decline_reason: String::new()
            }
        ),
        SignalDelivery::Duplicate
    );

    // three distinct buffered signals (one per per-effect key) — the §6.4 anchor.
    assert_eq!(
        ex.signals().count_for_run(&tenant(), &run.0),
        3,
        "three independent per-effect signals (card-7:0/1/2); the double-click buffered nothing new"
    );
    // each keyed independently.
    assert!(ex
        .signals()
        .get(&tenant(), &run.0, APPROVAL_SIGNAL_NAME, "card-7:0")
        .is_some());
    assert!(ex
        .signals()
        .get(&tenant(), &run.0, APPROVAL_SIGNAL_NAME, "card-7:1")
        .is_some());
    assert!(ex
        .signals()
        .get(&tenant(), &run.0, APPROVAL_SIGNAL_NAME, "card-7:2")
        .is_some());

    // --- the ENGINE's gated loop resumes the buffered decisions (apply 0+2 once, withhold 1). ---
    let engine_card = ApprovalCard {
        run_id: run.0.clone(),
        card_id: "card-7".into(),
        effects: vec![
            GatedEffect {
                effect_ref: RefArtifactRef("myelin://acme/agent/effect/e0".into()),
                decision: ApprovalDecision::Approve,
            },
            GatedEffect {
                effect_ref: RefArtifactRef("myelin://acme/agent/effect/e1".into()),
                decision: ApprovalDecision::Decline,
            },
            GatedEffect {
                effect_ref: RefArtifactRef("myelin://acme/agent/effect/e2".into()),
                decision: ApprovalDecision::Approve,
            },
        ],
    };
    let applied = RefCell::new(Vec::<String>::new());
    let outcomes = apply_approved_effects(
        ex.signals(),
        &tenant(),
        &engine_card,
        &|eff: &RefArtifactRef| {
            applied.borrow_mut().push(eff.0.clone());
            Ok(format!("evt-{}", eff.0))
        },
    );

    // effect 0 applied; effect 1 WITHHELD (0 mutation, AG-8); effect 2 applied.
    assert!(
        matches!(outcomes[0], Some(Ok(EffectOutcome::Applied(_)))),
        "effect 0 approved → applied"
    );
    assert_eq!(
        outcomes[1],
        Some(Ok(EffectOutcome::Withheld(DECLINE_MARKER.to_string()))),
        "effect 1 declined → WITHHELD (0 mutation, AG-8)"
    );
    assert!(
        matches!(outcomes[2], Some(Ok(EffectOutcome::Applied(_)))),
        "effect 2 approved → applied"
    );

    // GATE: exactly TWO applies (0 and 2); the withheld effect 1 made ZERO mutation (the
    // per-effect duplicate + withheld-mutation signals = 0).
    let applied = applied.into_inner();
    assert_eq!(
        applied.len(),
        2,
        "exactly two effects applied (no effect runs twice)"
    );
    assert!(
        !applied.contains(&"myelin://acme/agent/effect/e1".to_string()),
        "the WITHHELD effect 1 never reached apply (0 mutation, AG-8)"
    );
}

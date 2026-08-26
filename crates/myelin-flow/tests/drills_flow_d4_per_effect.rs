use myelin_events::{Actor, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp};
use myelin_flow::{
    apply_approved_effects, partition_for_run_id, run_state, ApprovalCard, ApprovalDecision,
    DurableExecutor, EffectOutcome, FlowDispatcher, FlowExecutor, FlowTelemetry, GatedEffect,
    RunStore, SignalSpec, SignalStore, WaitOutcome, WfCtx, WfJournal, WorkflowBody,
    APPROVAL_SIGNAL_NAME, DECLINE_MARKER,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::cell::RefCell;
use std::rc::Rc;
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
fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: None,
    }
}

const CARD_ID: &str = "card-7";
const CARD_DECISION_WAKE: &str = "card_decision";
const EFFECT_APPLIED_EVENT: &str = "agent.effect.applied";

#[derive(Clone, Debug, PartialEq, Eq)]
enum LedgerEntry {
    Apply(String),
    Decline(String),
}

type Ledger = Rc<RefCell<Vec<LedgerEntry>>>;
type ApplyCount = Rc<RefCell<usize>>;

fn three_effect_card(run_id: &str) -> ApprovalCard {
    ApprovalCard {
        run_id: run_id.to_string(),
        card_id: CARD_ID.into(),
        effects: vec![
            GatedEffect {
                effect_ref: ArtifactRef("myelin://acme/agent/effect/merge-a".into()),
                decision: ApprovalDecision::Approve,
            },
            GatedEffect {
                effect_ref: ArtifactRef("myelin://acme/agent/effect/merge-b".into()),
                decision: ApprovalDecision::Decline,
            },
            GatedEffect {
                effect_ref: ArtifactRef("myelin://acme/agent/effect/merge-c".into()),
                decision: ApprovalDecision::Approve,
            },
        ],
    }
}

fn gated_multi_effect_body(
    ledger: Ledger,
    apply_count: ApplyCount,
    signals: SignalStore,
) -> Box<WorkflowBody> {
    Box::new(move |ctx: &mut WfCtx| {
        let emitted = RefCell::new(false);
        ctx.activity(
            myelin_flow::RetryPolicy { max_attempts: 1 },
            |_idem, _attempt| {
                *emitted.borrow_mut() = true;
                Ok(vec![])
            },
        )
        .map_err(|e| format!("{e:?}"))?;
        if *emitted.borrow() {
            ctx.emit(
                myelin_events::EventDraft {
                    type_: myelin_events::EventType("agent.approval.requested".into()),
                    subject: myelin_events::ArtifactRef("myelin://acme/agent/run/R1".into()),
                    aggregate: myelin_events::AggregateKey("run:R1".into()),
                    payload: serde_json::json!({ "card_id": CARD_ID, "effects": 3 }),
                    data_role: myelin_events::DataRole::Controller,
                    visibility: myelin_events::Visibility::Internal,
                    contains_personal_data: false,
                    pii_key_ref: None,
                },
                None,
            )
            .map_err(|e| format!("{e:?}"))?;
        }

        let outcome = ctx
            .wait_for_signal(CARD_DECISION_WAKE, Some(7 * 86_400))
            .map_err(|e| format!("{e:?}"))?;
        match outcome {
            WaitOutcome::Parked => return Ok(vec![]),
            WaitOutcome::TimedOut => {
                for eff in &three_effect_card(ctx.run_id()).effects {
                    ledger
                        .borrow_mut()
                        .push(LedgerEntry::Decline(eff.effect_ref.0.clone()));
                }
                return Ok(vec![]);
            }
            WaitOutcome::Signalled { .. } => {}
        }

        let card = three_effect_card(ctx.run_id());
        let outcomes = apply_approved_effects(&signals, &tenant(), &card, &|eff: &ArtifactRef| {
            *apply_count.borrow_mut() += 1;
            Ok(format!("evt-for-{}", eff.0))
        });

        let mut applied_refs = Vec::new();
        for (idx, outcome) in outcomes.iter().enumerate() {
            let eff_ref = card.effects[idx].effect_ref.0.clone();
            match outcome {
                Some(Ok(EffectOutcome::Applied(_))) => {
                    ledger
                        .borrow_mut()
                        .push(LedgerEntry::Apply(eff_ref.clone()));
                    ctx.emit(
                        myelin_events::EventDraft {
                            type_: myelin_events::EventType(EFFECT_APPLIED_EVENT.into()),
                            subject: myelin_events::ArtifactRef(eff_ref.clone()),
                            aggregate: myelin_events::AggregateKey(format!("effect:{idx}")),
                            payload: serde_json::json!({ "card_id": CARD_ID, "effect_idx": idx }),
                            data_role: myelin_events::DataRole::Controller,
                            visibility: myelin_events::Visibility::Internal,
                            contains_personal_data: false,
                            pii_key_ref: None,
                        },
                        None,
                    )
                    .map_err(|e| format!("{e:?}"))?;
                    applied_refs.push(ArtifactRef(eff_ref));
                }
                Some(Ok(EffectOutcome::Withheld(_))) => {
                    ledger.borrow_mut().push(LedgerEntry::Decline(eff_ref));
                }
                Some(Err(e)) => return Err(format!("effect {idx} apply failed: {e:?}")),
                None => return Err(format!("effect {idx} had no buffered decision on resume")),
            }
        }
        Ok(applied_refs)
    })
}

struct Substrate {
    runs: RunStore,
    journal: WfJournal,
    signals: SignalStore,
    timers: myelin_flow::TimerStore,
    outbox: OutboxStore,
    tele: FlowTelemetry,
}

fn fresh_worker(
    sub: &Substrate,
    worker: &str,
    partition: i16,
    ledger: Ledger,
    apply_count: ApplyCount,
    minter: Arc<dyn IdMinter>,
) -> FlowDispatcher {
    let mut disp = FlowDispatcher::new(
        sub.runs.clone(),
        sub.outbox.clone(),
        sub.journal.clone(),
        sub.tele.clone(),
        minter,
        ctx_base(),
        partition,
        worker,
        30,
    )
    .with_signals(sub.signals.clone())
    .with_timers(sub.timers.clone());
    disp.register(
        "agent.run",
        gated_multi_effect_body(ledger, apply_count, sub.signals.clone()),
    );
    disp
}

fn approve(ex: &FlowExecutor, run: &myelin_flow::RunId, idx: usize) {
    let key = myelin_flow::per_effect_idem_key(CARD_ID, idx, 3);
    ex.signal(SignalSpec {
        run: run.clone(),
        signal_name: APPROVAL_SIGNAL_NAME.into(),
        idem_key: key,
        payload: vec![ArtifactRef(format!(
            "myelin://acme/agent/effect/{CARD_ID}-{idx}"
        ))],
        payload_key_ref: None,
    })
    .expect("approve");
}

fn decline(ex: &FlowExecutor, run: &myelin_flow::RunId, idx: usize) {
    let key = myelin_flow::per_effect_idem_key(CARD_ID, idx, 3);
    ex.signal(SignalSpec {
        run: run.clone(),
        signal_name: APPROVAL_SIGNAL_NAME.into(),
        idem_key: key,
        payload: vec![],
        payload_key_ref: Some(DECLINE_MARKER.into()),
    })
    .expect("decline");
}

fn wake_signal(ex: &FlowExecutor, run: &myelin_flow::RunId) -> myelin_flow::SignalOutcome {
    ex.signal(SignalSpec {
        run: run.clone(),
        signal_name: CARD_DECISION_WAKE.into(),
        idem_key: CARD_ID.into(),
        payload: vec![],
        payload_key_ref: None,
    })
    .expect("wake")
}

#[test]
fn flow_d4_per_effect_partial_approval_across_restart_and_deploy_double_click() {
    let ex = FlowExecutor::new(minter(), tenant(), region());
    ex.register_definition("agent.run");
    let run = ex
        .start(myelin_flow::StartSpec {
            wf_type: "agent.run".into(),
            input: vec![],
            budget: None,
            idem_key: "rule:evt-partial-1".into(),
        })
        .expect("start the gated multi-effect workflow");

    let sub = Substrate {
        runs: ex.runs().clone(),
        journal: WfJournal::new(),
        signals: ex.signals().clone(),
        timers: myelin_flow::TimerStore::new(),
        outbox: OutboxStore::new(),
        tele: FlowTelemetry::new(),
    };
    let ledger: Ledger = Rc::new(RefCell::new(Vec::new()));
    let apply_count: ApplyCount = Rc::new(RefCell::new(0));
    let part = partition_for_run_id(&run.0);
    let id_source = minter();

    let w1 = fresh_worker(
        &sub,
        "worker-1",
        part,
        ledger.clone(),
        apply_count.clone(),
        id_source.clone(),
    );
    let o1 = w1
        .tick(1_000, "2026-06-21T00:00:00Z", 7)
        .expect("worker-1 drives the run");
    assert_eq!(
        o1,
        myelin_flow::DriveOutcome::Waiting,
        "the run PARKED on the card-decision wait (the multi-effect card request was emitted)"
    );
    assert_eq!(
        sub.runs.get(&tenant(), &run.0).unwrap().state,
        run_state::WAITING,
        "state=waiting - the multi-day HITL wait holds no runtime (FLOW-D4)"
    );
    assert_eq!(
        sub.outbox.committed_count(),
        1,
        "the agent.approval.requested card (gating 3 effects) was emitted once"
    );
    assert_eq!(
        *apply_count.borrow(),
        0,
        "no effect applied while the run is parked"
    );

    drop(w1);

    approve(&ex, &run, 0);
    decline(&ex, &run, 1);
    approve(&ex, &run, 2);
    approve(&ex, &run, 0);
    approve(&ex, &run, 2);
    assert_eq!(
        sub.signals.count_for_run(&tenant(), &run.0),
        3,
        "the partial approval + double-click buffered EXACTLY THREE per-effect signals (0/1/2) - the \
         double-click on approve-all re-sent the same keys → ON CONFLICT DO NOTHING"
    );

    assert_eq!(
        wake_signal(&ex, &run),
        myelin_flow::SignalOutcome::Buffered,
        "the card-decision wake buffered"
    );
    assert_eq!(
        wake_signal(&ex, &run),
        myelin_flow::SignalOutcome::Duplicate,
        "the double-clicked wake is a no-op (ON CONFLICT DO NOTHING)"
    );
    sub.runs.wake(&tenant(), &run.0);

    let w2 = fresh_worker(
        &sub,
        "worker-2",
        part,
        ledger.clone(),
        apply_count.clone(),
        id_source.clone(),
    );
    let o2 = w2
        .tick(7 * 86_400 + 2_000, "2026-06-28T00:00:00Z", 7)
        .expect("worker-2 resumes the run");
    match o2 {
        myelin_flow::DriveOutcome::Completed(refs) => assert_eq!(
            refs,
            vec![
                ArtifactRef("myelin://acme/agent/effect/merge-a".into()),
                ArtifactRef("myelin://acme/agent/effect/merge-c".into()),
            ],
            "the resumed run applied effects 0 and 2 (the approved ones) - effect 1 was WITHHELD"
        ),
        other => panic!("expected the run to resume + complete, got {other:?}"),
    }

    let ledger = ledger.borrow().clone();
    assert_eq!(
        ledger,
        vec![
            LedgerEntry::Apply("myelin://acme/agent/effect/merge-a".into()),
            LedgerEntry::Decline("myelin://acme/agent/effect/merge-b".into()),
            LedgerEntry::Apply("myelin://acme/agent/effect/merge-c".into()),
        ],
        "the per-effect ledger is exactly [apply, decline, apply] - each effect decided independently (§6.4)"
    );

    assert_eq!(
        *apply_count.borrow(),
        2,
        "exactly 2 applies (effects 0 and 2) - the double-click on approve-all did NOT double-apply (0 double-apply)"
    );

    assert_eq!(
        sub.outbox.committed_count(),
        3,
        "the outbox holds the card request + EXACTLY TWO effect-applied emits - the declined effect made \
         0 MUTATION (AG-8); no third effect-applied row"
    );
    assert!(
        !ledger.contains(&LedgerEntry::Apply(
            "myelin://acme/agent/effect/merge-b".into()
        )),
        "the DECLINED effect 1 was NEVER applied - 0 mutation on decline across the restart (AG-8)"
    );

    assert_eq!(
        sub.runs.get(&tenant(), &run.0).unwrap().state,
        run_state::COMPLETED,
        "the run completed (terminal) - it will never be driven again"
    );

    println!(
        "[2026-06-21] PASS  drill=FLOW-D4(per-effect-extended)  partial approval across restart+deploy  \
         park->state=waiting  partial={{0=approve,1=decline,2=approve}}  double-click->buffered=3  \
         ledger=[apply,decline,apply]  applies=2  double-apply=0  decline-mutation=0(AG-8)"
    );
}

#[test]
fn partial_approval_ledger_has_exactly_three_entries() {
    let ex = FlowExecutor::new(minter(), tenant(), region());
    ex.register_definition("agent.run");
    let run = ex
        .start(myelin_flow::StartSpec {
            wf_type: "agent.run".into(),
            input: vec![],
            budget: None,
            idem_key: "rule:ledger".into(),
        })
        .expect("start");

    approve(&ex, &run, 0);
    decline(&ex, &run, 1);
    approve(&ex, &run, 2);
    assert_eq!(
        ex.signals().count_for_run(&tenant(), &run.0),
        3,
        "three independently-keyed per-effect signals"
    );

    let applies = RefCell::new(0usize);
    let card = three_effect_card(&run.0);
    let outcomes = apply_approved_effects(ex.signals(), &tenant(), &card, &|_e: &ArtifactRef| {
        *applies.borrow_mut() += 1;
        Ok("evt".into())
    });

    assert_eq!(outcomes.len(), 3);
    assert!(matches!(outcomes[0], Some(Ok(EffectOutcome::Applied(_)))));
    assert_eq!(
        outcomes[1],
        Some(Ok(EffectOutcome::Withheld(DECLINE_MARKER.to_string()))),
        "effect 1 declined → WITHHELD (0 mutation, AG-8)"
    );
    assert!(matches!(outcomes[2], Some(Ok(EffectOutcome::Applied(_)))));
    assert_eq!(
        *applies.borrow(),
        2,
        "exactly two applies (effects 0 and 2)"
    );
}

#[test]
fn double_click_adds_no_fourth_apply() {
    let ex = FlowExecutor::new(minter(), tenant(), region());
    ex.register_definition("agent.run");
    let run = ex
        .start(myelin_flow::StartSpec {
            wf_type: "agent.run".into(),
            input: vec![],
            budget: None,
            idem_key: "rule:double".into(),
        })
        .expect("start");

    approve(&ex, &run, 0);
    decline(&ex, &run, 1);
    approve(&ex, &run, 2);
    approve(&ex, &run, 0);
    approve(&ex, &run, 2);
    assert_eq!(
        ex.signals().count_for_run(&tenant(), &run.0),
        3,
        "the double-click buffered 0 new signals (ON CONFLICT DO NOTHING) - still three per-effect rows"
    );

    let applies = RefCell::new(0usize);
    let card = three_effect_card(&run.0);
    let _ = apply_approved_effects(ex.signals(), &tenant(), &card, &|_e: &ArtifactRef| {
        *applies.borrow_mut() += 1;
        Ok("evt".into())
    });
    assert_eq!(
        *applies.borrow(),
        2,
        "exactly two applies - the double-click did NOT add a fourth apply (0 double-apply)"
    );
}

//! # FLOW-D4 extended — the per-effect partial-approval drill across a restart + deploy (P-FLOW-12 → P-209)
//!
//! The headline drill the P-FLOW-12 GATE requires (architecture §8 / testing-strategy FLOW-D4 per-effect
//! extended form): a gated workflow with a **multi-effect approval card** parks on the durable wait
//! (`state=waiting`, holding NO runtime) across a worker **restart** + a **deploy**; the **partial
//! approval** — `{card_id:0 = approve, card_id:1 = decline, card_id:2 = approve}` — arrives **days later**
//! WITH a **double-click on "approve all"**; the resumed run applies/withholds EACH effect **EXACTLY
//! ONCE**, the **declined effect never mutates** (AG-8), and the double-click is **absorbed** (0
//! double-apply). The exact threshold (architecture §8): **3 per-effect ledger entries** (apply / decline
//! / apply), a **0-double-apply** counter, and a **0-mutation-on-decline** assertion across the restart.
//!
//! **No new engine primitive.** This drills P-FLOW-10's per-effect `idem_key` rule
//! ([`per_effect_idem_key`] + [`apply_approved_effects`], contract 9.1) together with P-FLOW-11's durable
//! wait ([`WfCtx::wait_for_signal`], contract 9.4) under failure injection — exactly the F-4-extended
//! assertion of architecture §8.b. A red drill is information — never weaken it to pass (EI-01 §3).
//!
//! **What "restart" + "deploy" model** (identical to the FLOW-D4 base drill,
//! `tests/drills_flow_d4_multiday_hitl.rs`): the engine drives runs through a [`FlowDispatcher`] (one
//! per-partition worker). A RESTART is a FRESH dispatcher over the SAME run store + journal + signal
//! buffer + outbox (the durable state survives the worker death). A DEPLOY is a re-registration of the
//! workflow body (here the SAME version 1 — a deploy that does not change the workflow shape; a deploy
//! that DOES bump the version is the FLOW-D2 divergence guard, P-FLOW-07). The durability is the point:
//! the partial approval arrives across both, days later, and each effect is still applied/withheld
//! exactly once.
//!
//! **The per-effect ledger.** Each APPLIED effect emits an `agent.effect.applied` event via the outbox
//! (one ledger row per apply); a DECLINED effect is WITHHELD — it emits NOTHING (0 mutation, AG-8). So
//! the per-effect ledger across the run is exactly three entries — `apply` (effect 0), `decline` (effect
//! 1, recorded as a withhold with NO emit), `apply` (effect 2) — and the outbox holds the card request +
//! exactly TWO effect-applied emits.

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

/// The card identity gated by this drill — a three-effect batch card (the §6.4 multi-effect form: each
/// effect keys on `card-7:0` / `card-7:1` / `card-7:2`).
const CARD_ID: &str = "card-7";
/// The signal name the body parks on for the human's "I have decided the whole card" wake. The per-effect
/// decisions ride the separate [`APPROVAL_SIGNAL_NAME`] signal under the §6.4 per-effect `idem_key`s; this
/// wake is what un-parks the run once the partial approval has been recorded.
const CARD_DECISION_WAKE: &str = "card_decision";
/// The FROZEN per-effect ledger event a single APPLIED effect emits via the outbox (one row per apply).
const EFFECT_APPLIED_EVENT: &str = "agent.effect.applied";

/// One entry in the per-effect ledger the drill asserts: `Apply(effect_ref)` (the effect mutated exactly
/// once) or `Decline(effect_ref)` (WITHHELD — 0 mutation, AG-8). Across the three-effect card the ledger
/// must be exactly `[Apply, Decline, Apply]`.
#[derive(Clone, Debug, PartialEq, Eq)]
enum LedgerEntry {
    Apply(String),
    Decline(String),
}

/// The per-effect ledger the body records as it walks the card (shared with the test so it can assert the
/// three entries). A real engine derives this from the emitted `agent.effect.applied` events + the
/// withheld set; here we record it directly so the drill can assert apply/decline/apply.
type Ledger = Rc<RefCell<Vec<LedgerEntry>>>;
/// A double-apply tripwire: incremented by the `EffectApi::apply` closure on EVERY call. The gate asserts
/// it equals the number of APPROVED effects (2) — never more (a double-click MUST NOT double-apply).
type ApplyCount = Rc<RefCell<usize>>;

/// The three-effect approval card the partial approval decides (approve 0, decline 1, approve 2 — the
/// §8.b shape). The decisions on the card mirror what the human submits; the engine reads the BUFFERED
/// per-effect signals as the truth.
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

/// The gated multi-effect workflow body: emit the approval card request (once, replay-guarded), park on
/// the durable wait, then on resume run the §6.4 gated loop over the three buffered per-effect signals —
/// applying each APPROVED effect EXACTLY once (one `agent.effect.applied` emit per apply) and WITHHOLDING
/// the DECLINED effect (no emit, 0 mutation, AG-8). Deterministic over its journal.
fn gated_multi_effect_body(
    ledger: Ledger,
    apply_count: ApplyCount,
    signals: SignalStore,
) -> Box<WorkflowBody> {
    Box::new(move |ctx: &mut WfCtx| {
        // 1. Emit the card request ONCE (guarded by an activity so a re-drive short-circuits, §4.1) — the
        //    card gates three effects; Notif/Chat renders it (contract 7.3, NOT this engine).
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

        // 2. PARK on the durable wait until the human has decided the card (the `card_decision` wake). The
        //    run holds NO runtime (state=waiting) across the restart + deploy, days later (FLOW-D4).
        let outcome = ctx
            .wait_for_signal(CARD_DECISION_WAKE, Some(7 * 86_400))
            .map_err(|e| format!("{e:?}"))?;
        match outcome {
            // still parked (the wake has not arrived) — the run stays waiting.
            WaitOutcome::Parked => return Ok(vec![]),
            // the auto-deny window elapsed → the whole card is withheld (0 mutation across all effects).
            WaitOutcome::TimedOut => {
                for eff in &three_effect_card(ctx.run_id()).effects {
                    ledger
                        .borrow_mut()
                        .push(LedgerEntry::Decline(eff.effect_ref.0.clone()));
                }
                return Ok(vec![]);
            }
            // 3. RESUMED — the wake arrived. Run the §6.4 gated loop over the buffered per-effect signals.
            WaitOutcome::Signalled { .. } => {}
        }

        // 3a. Read the per-effect decisions off the buffered `approval` signals + record the ledger. The
        //     apply closure is a pure recorder (it bumps the double-apply tripwire); the actual mutating
        //     emit is done OUTSIDE the closure (it cannot borrow `ctx`, which the closure does not hold).
        let card = three_effect_card(ctx.run_id());
        let outcomes = apply_approved_effects(&signals, &tenant(), &card, &|eff: &ArtifactRef| {
            *apply_count.borrow_mut() += 1;
            Ok(format!("evt-for-{}", eff.0))
        });

        // 3b. Emit ONE `agent.effect.applied` ledger event per APPLIED effect (the mutation); record the
        //     ledger entry per effect (apply | decline). A DECLINED effect emits NOTHING (0 mutation,
        //     AG-8). Each emit rides through the outbox (the ONLY emit path, §4.5).
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
                    // WITHHELD (AG-8): NO emit, 0 mutation — the ledger records the decline.
                    ledger.borrow_mut().push(LedgerEntry::Decline(eff_ref));
                }
                Some(Err(e)) => return Err(format!("effect {idx} apply failed: {e:?}")),
                None => return Err(format!("effect {idx} had no buffered decision on resume")),
            }
        }
        Ok(applied_refs)
    })
}

/// The shared durable substrate a worker drives over (run store + journal + signal buffer + outbox +
/// telemetry). Restarts share THIS substrate (the durable state survives a worker death).
struct Substrate {
    runs: RunStore,
    journal: WfJournal,
    signals: SignalStore,
    outbox: OutboxStore,
    tele: FlowTelemetry,
}

/// A FRESH dispatcher over the shared substrate (a "restart" / a "redeploy" — a new worker process), on
/// the run's partition so its lease scan finds it. The deploy re-registers the SAME-version body.
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
        // A SHARED globally-unique ULID source across workers (production mints ULIDs from a
        // process-global source; a restart/deploy never resets event-id uniqueness). Using one
        // monotonic minter across both workers models that: the resume drive's effect-applied emits
        // get FRESH event_ids, never colliding with the card request the prior worker committed.
        minter,
        ctx_base(),
        partition,
        worker,
        30,
    )
    .with_signals(sub.signals.clone());
    disp.register(
        "agent.run",
        gated_multi_effect_body(ledger, apply_count, sub.signals.clone()),
    );
    disp
}

/// Deliver an APPROVE signal for effect `idx` of the three-effect card (the payload carries the effect
/// ref; keyed on the §6.4 per-effect `card-7:idx`).
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

/// Deliver a DECLINE signal for effect `idx` (empty payload + the DECLINE_MARKER, §3.4 — keyed on
/// `card-7:idx`).
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

/// Deliver the `card_decision` wake that un-parks the run once the partial approval has been recorded.
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

/// **FLOW-D4 EXTENDED — the per-effect partial-approval drill across a restart + deploy with a
/// double-click (the P-FLOW-12 GATE).** A three-effect card parks on the durable wait across a worker
/// restart + a deploy; the partial approval `{0=approve, 1=decline, 2=approve}` arrives days later WITH a
/// double-click on "approve all"; the resumed run applies effects 0 and 2 EXACTLY once each and WITHHOLDS
/// effect 1 (0 mutation). The threshold: 3 per-effect ledger entries (apply/decline/apply), a
/// 0-double-apply counter, a 0-mutation-on-decline assertion across the restart.
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
        outbox: OutboxStore::new(),
        tele: FlowTelemetry::new(),
    };
    let ledger: Ledger = Rc::new(RefCell::new(Vec::new()));
    let apply_count: ApplyCount = Rc::new(RefCell::new(0));
    let part = partition_for_run_id(&run.0);
    // One process-global ULID source shared across the restart/deploy (event-id uniqueness is global).
    let id_source = minter();

    // WORKER 1 ticks: the body emits the three-effect card request + PARKS (state=waiting holds no runtime).
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
        "state=waiting — the multi-day HITL wait holds no runtime (FLOW-D4)"
    );
    assert_eq!(
        sub.outbox.committed_count(),
        1,
        "the agent.approval.requested card (gating 3 effects) was emitted once"
    );
    // nothing applied yet — the partial approval has not arrived.
    assert_eq!(
        *apply_count.borrow(),
        0,
        "no effect applied while the run is parked"
    );

    // --- WORKER 1 CRASHES (restart) + the service is REDEPLOYED while the run is parked (days pass). ---
    drop(w1);

    // DAYS LATER: the human submits the PARTIAL approval — approve 0, decline 1, approve 2 — three
    // independently-keyed per-effect signals (the §6.4 partial-approval shape).
    approve(&ex, &run, 0);
    decline(&ex, &run, 1);
    approve(&ex, &run, 2);
    // THE DOUBLE-CLICK on "approve all": re-send the SAME per-effect APPROVE keys (0 and 2) → ON CONFLICT
    // DO NOTHING → 0 new buffered signals (the double-click is absorbed).
    approve(&ex, &run, 0);
    approve(&ex, &run, 2);
    assert_eq!(
        sub.signals.count_for_run(&tenant(), &run.0),
        3,
        "the partial approval + double-click buffered EXACTLY THREE per-effect signals (0/1/2) — the \
         double-click on approve-all re-sent the same keys → ON CONFLICT DO NOTHING"
    );

    // the human's submit posts the `card_decision` wake — also double-clicked; the wake buffers once.
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
    // the signal-wake flips the parked run waiting → running so the NEW worker re-leases it.
    sub.runs.wake(&tenant(), &run.0);

    // --- WORKER 2 (the redeployed process) re-leases + resumes the run DAYS later. ---
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
            "the resumed run applied effects 0 and 2 (the approved ones) — effect 1 was WITHHELD"
        ),
        other => panic!("expected the run to resume + complete, got {other:?}"),
    }

    // THE THRESHOLD 1 — three per-effect ledger entries: apply / decline / apply.
    let ledger = ledger.borrow().clone();
    assert_eq!(
        ledger,
        vec![
            LedgerEntry::Apply("myelin://acme/agent/effect/merge-a".into()),
            LedgerEntry::Decline("myelin://acme/agent/effect/merge-b".into()),
            LedgerEntry::Apply("myelin://acme/agent/effect/merge-c".into()),
        ],
        "the per-effect ledger is exactly [apply, decline, apply] — each effect decided independently (§6.4)"
    );

    // THE THRESHOLD 2 — 0 double-apply: exactly TWO applies (effects 0 and 2), never four (the double-click
    // was absorbed).
    assert_eq!(
        *apply_count.borrow(),
        2,
        "exactly 2 applies (effects 0 and 2) — the double-click on approve-all did NOT double-apply (0 double-apply)"
    );

    // THE THRESHOLD 3 — 0 mutation on decline: the declined effect (1) emitted NOTHING. The outbox holds
    // the card request (1) + exactly TWO effect-applied emits (effects 0 and 2) = 3 — never a third
    // effect-applied row for the declined effect (AG-8).
    assert_eq!(
        sub.outbox.committed_count(),
        3,
        "the outbox holds the card request + EXACTLY TWO effect-applied emits — the declined effect made \
         0 MUTATION (AG-8); no third effect-applied row"
    );
    assert!(
        !ledger.contains(&LedgerEntry::Apply(
            "myelin://acme/agent/effect/merge-b".into()
        )),
        "the DECLINED effect 1 was NEVER applied — 0 mutation on decline across the restart (AG-8)"
    );

    // the per-effect approval signals were all consumed (the buffered approval depth dropped; the wake
    // was consumed too). The run is terminal.
    assert_eq!(
        sub.runs.get(&tenant(), &run.0).unwrap().state,
        run_state::COMPLETED,
        "the run completed (terminal) — it will never be driven again"
    );

    println!(
        "[2026-06-21] PASS  drill=FLOW-D4(per-effect-extended)  partial approval across restart+deploy  \
         park->state=waiting  partial={{0=approve,1=decline,2=approve}}  double-click->buffered=3  \
         ledger=[apply,decline,apply]  applies=2  double-apply=0  decline-mutation=0(AG-8)"
    );
}

/// **Unit: the partial-approval ledger has EXACTLY three entries (apply/decline/apply).** A focused
/// assertion over [`apply_approved_effects`] (the §6.4 gated loop) — the engine half of the §8.b drill,
/// independent of the dispatcher: three per-effect signals → three outcomes, exactly two applies, one
/// withhold.
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

    // exactly three outcomes — apply / decline (withheld) / apply.
    assert_eq!(outcomes.len(), 3);
    assert!(matches!(outcomes[0], Some(Ok(EffectOutcome::Applied(_)))));
    assert_eq!(
        outcomes[1],
        Some(Ok(EffectOutcome::Withheld(DECLINE_MARKER.to_string()))),
        "effect 1 declined → WITHHELD (0 mutation, AG-8)"
    );
    assert!(matches!(outcomes[2], Some(Ok(EffectOutcome::Applied(_)))));
    // the ledger has three entries; exactly two are applies.
    assert_eq!(
        *applies.borrow(),
        2,
        "exactly two applies (effects 0 and 2)"
    );
}

/// **Unit: the double-click adds NO fourth apply.** Re-sending the same per-effect approve keys buffers no
/// new signal (ON CONFLICT DO NOTHING); the loop over the buffered set applies each effect once — the
/// double-click never produces a fourth apply.
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

    // partial approval: approve 0, decline 1, approve 2.
    approve(&ex, &run, 0);
    decline(&ex, &run, 1);
    approve(&ex, &run, 2);
    // DOUBLE-CLICK on "approve all": re-send the same approve keys (0 and 2).
    approve(&ex, &run, 0);
    approve(&ex, &run, 2);
    assert_eq!(
        ex.signals().count_for_run(&tenant(), &run.0),
        3,
        "the double-click buffered 0 new signals (ON CONFLICT DO NOTHING) — still three per-effect rows"
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
        "exactly two applies — the double-click did NOT add a fourth apply (0 double-apply)"
    );
}

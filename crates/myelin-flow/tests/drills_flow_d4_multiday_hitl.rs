//! # FLOW-D4 drill — the multi-day HITL approval-card round-trip (P-FLOW-11 → P-208)
//!
//! The headline drill the P-FLOW-11 GATE requires (testing-strategy FLOW-D4): a gated workflow
//! `wait_for_signal("approval:<call>", timeout)` PARKS (`state=waiting`, holding NO runtime) across a
//! worker **restart** + a **deploy**; the approval is delivered **days later** with a **double-click**;
//! the workflow resumes, consumes the approval **EXACTLY ONCE**, and runs (approved) or withholds
//! (denied → **0 mutation**, AG-8). The exact threshold (testing-strategy FLOW-D4): **1 consume** on
//! the signal-buffer-depth ledger; **withhold = 0 mutation**. A red drill is information — never weaken
//! it to pass (EI-01 §3).
//!
//! **What "restart" + "deploy" model:** the engine drives runs through a [`FlowDispatcher`] (one
//! per-partition worker). A RESTART is a FRESH dispatcher over the SAME run store + journal + signal
//! buffer (the durable state survives the worker death). A DEPLOY is a re-registration of the
//! workflow body (here the SAME version 1 — a deploy that does not change the workflow shape; a deploy
//! that DOES bump the version is the FLOW-D2 divergence guard, drilled at P-FLOW-07). The durability is
//! the point: the approval arrives across both, days later, and is still consumed exactly once.

use myelin_events::{Actor, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp};
use myelin_flow::{
    approval_wait_name, request_approval_and_wait, run_state, DurableExecutor, FlowDispatcher,
    FlowExecutor, FlowTelemetry, RetryPolicy, RunStore, SignalSpec, SignalStore, WaitOutcome,
    WfCtx, WfJournal, WorkflowBody, DECLINE_MARKER,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
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

/// The gated-tool workflow body: request approval (`agent.approval.requested` via the outbox) + wait;
/// on approve → run the merge tool (one mutating activity → one effect ref); on decline/timeout →
/// WITHHELD (0 mutation, AG-8). Deterministic over its journal (the flow-determinism contract).
fn gated_merge_body() -> Box<WorkflowBody> {
    Box::new(|ctx: &mut WfCtx| {
        let outcome = request_approval_and_wait(
            ctx,
            "call-1",
            vec![ArtifactRef("myelin://acme/agent/tool/merge".into())],
            Some(7 * 86_400), // a one-week approval window.
            |refs| myelin_events::EventDraft {
                type_: myelin_events::EventType("agent.approval.requested".into()),
                subject: myelin_events::ArtifactRef("myelin://acme/agent/run/R1".into()),
                aggregate: myelin_events::AggregateKey("run:R1".into()),
                payload: serde_json::json!({ "refs": refs.iter().map(|r| r.0.clone()).collect::<Vec<_>>() }),
                data_role: myelin_events::DataRole::Controller,
                visibility: myelin_events::Visibility::Internal,
                contains_personal_data: false,
                pii_key_ref: None,
            },
        )
        .map_err(|e| format!("{e:?}"))?;
        match outcome {
            WaitOutcome::Signalled {
                payload_key_ref, ..
            } if payload_key_ref.as_deref() == Some(DECLINE_MARKER) => {
                Ok(vec![]) // DENY → WITHHELD: 0 mutation (AG-8).
            }
            WaitOutcome::Signalled { .. } => {
                // APPROVE → run the merge tool (one mutating activity → one effect).
                let eff = ctx
                    .activity(RetryPolicy { max_attempts: 1 }, |_i, _a| {
                        Ok(vec![ArtifactRef(
                            "myelin://acme/agent/effect/merged".into(),
                        )])
                    })
                    .map_err(|e| format!("{e:?}"))?;
                Ok(eff)
            }
            WaitOutcome::TimedOut => Ok(vec![]), // auto-deny → 0 mutation.
            WaitOutcome::Parked => Ok(vec![]),   // still waiting.
        }
    })
}

/// Build the shared durable substrate (run store + journal + signal buffer + outbox + telemetry) a
/// worker drives over. Restarts share THIS substrate (the durable state survives a worker death).
struct Substrate {
    runs: RunStore,
    journal: WfJournal,
    signals: SignalStore,
    outbox: OutboxStore,
    tele: FlowTelemetry,
}

/// The deterministic partition a run is hashed into (`hash(run_id) % PARTITION_COUNT`, §7.2) — the
/// same hash `FlowExecutor::start` stamps, so the worker built on this partition leases the run.
fn partition_for(run_id: &str) -> i16 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    run_id.hash(&mut h);
    (h.finish() % myelin_flow::PARTITION_COUNT as u64) as i16
}

/// A FRESH dispatcher over the shared substrate (a "restart" / a "redeploy" — a new worker process),
/// on the run's partition so its lease scan finds it.
fn fresh_worker(sub: &Substrate, worker: &str, partition: i16) -> FlowDispatcher {
    let mut disp = FlowDispatcher::new(
        sub.runs.clone(),
        sub.outbox.clone(),
        sub.journal.clone(),
        sub.tele.clone(),
        minter(),
        ctx_base(),
        partition,
        worker,
        30,
    )
    .with_signals(sub.signals.clone());
    disp.register("agent.run", gated_merge_body()); // the deploy re-registers the body.
    disp
}

/// **FLOW-D4 (APPROVE): park across a restart + a deploy, approve days later with a double-click,
/// resume + run, consume EXACTLY ONCE.** The threshold: 1 consume; the merge runs once.
#[test]
fn flow_d4_multiday_hitl_approve_across_restart_and_deploy_consumes_once() {
    let ex = FlowExecutor::new(minter(), tenant(), region());
    ex.register_definition("agent.run");
    let run = ex
        .start(myelin_flow::StartSpec {
            wf_type: "agent.run".into(),
            input: vec![],
            budget: None,
            idem_key: "rule:evt-1".into(),
        })
        .expect("start the gated workflow");

    let sub = Substrate {
        runs: ex.runs().clone(),
        journal: WfJournal::new(),
        signals: ex.signals().clone(),
        outbox: OutboxStore::new(),
        tele: FlowTelemetry::new(),
    };

    // WORKER 1 ticks: the body requests the approval card + PARKS (state=waiting holds no runtime).
    let part = partition_for(&run.0);
    let w1 = fresh_worker(&sub, "worker-1", part);
    let o1 = w1
        .tick(1_000, "2026-06-21T00:00:00Z", 7)
        .expect("worker-1 drives the run");
    assert_eq!(
        o1,
        myelin_flow::DriveOutcome::Waiting,
        "the run PARKED on the approval wait"
    );
    assert_eq!(
        sub.runs.get(&tenant(), &run.0).unwrap().state,
        run_state::WAITING,
        "state=waiting — the multi-day HITL wait holds no runtime (FLOW-D4)"
    );
    assert_eq!(
        sub.outbox.committed_count(),
        1,
        "the agent.approval.requested card was emitted once"
    );

    // --- WORKER 1 CRASHES (restart) + the service is REDEPLOYED while the run is parked (days pass). ---
    drop(w1);

    // DAYS LATER: a human clicks Approve — and DOUBLE-CLICKS (two deliveries under the SAME idem_key).
    let approve = || {
        ex.signal(SignalSpec {
            run: run.clone(),
            signal_name: approval_wait_name("call-1"),
            idem_key: "card-7".into(),
            payload: vec![ArtifactRef("myelin://acme/agent/decision/approve".into())],
            payload_key_ref: None,
        })
        .expect("approve")
    };
    let first = approve();
    let second = approve(); // the DOUBLE-CLICK.
    assert_eq!(
        first,
        myelin_flow::SignalOutcome::Buffered,
        "the first click buffered the approval"
    );
    assert_eq!(
        second,
        myelin_flow::SignalOutcome::Duplicate,
        "the double-click is a no-op (ON CONFLICT DO NOTHING)"
    );
    assert_eq!(
        sub.signals.count_for_run(&tenant(), &run.0),
        1,
        "the double-click buffered EXACTLY ONE approval (the workflow wakes once)"
    );

    // the signal-wake flips the parked run waiting → running so the NEW worker re-leases it.
    sub.runs.wake(&tenant(), &run.0);

    // --- WORKER 2 (the redeployed process) re-leases + resumes the run DAYS later. ---
    let w2 = fresh_worker(&sub, "worker-2", part);
    let o2 = w2
        .tick(7 * 86_400 + 2_000, "2026-06-28T00:00:00Z", 7)
        .expect("worker-2 resumes the run");
    match o2 {
        myelin_flow::DriveOutcome::Completed(refs) => assert_eq!(
            refs,
            vec![ArtifactRef("myelin://acme/agent/effect/merged".into())],
            "the resumed run RAN the approved merge tool (the approve branch)"
        ),
        other => panic!("expected the run to resume + complete, got {other:?}"),
    }

    // THE THRESHOLD: 1 consume on the signal-buffer-depth ledger (the approval was consumed ONCE).
    assert_eq!(
        sub.signals.buffered_depth(),
        0,
        "the approval was consumed EXACTLY ONCE — the buffered depth dropped to 0 (FLOW-D4: 1 consume)"
    );
    // the card request was emitted exactly once across BOTH drives (NOT re-emitted on the resume).
    assert_eq!(
        sub.outbox.committed_count(),
        1,
        "the card request was emitted ONCE (no re-emit on resume)"
    );
    // the run is terminal (completed) — it will never be driven again.
    assert!(sub.runs.get(&tenant(), &run.0).unwrap().state == run_state::COMPLETED);

    println!(
        "[2026-06-21] PASS  drill=FLOW-D4  multi-day HITL approve across restart+deploy  \
         park->state=waiting  double-click->buffered=1  consume=1  card-emit=1  merge=ran(approve)"
    );
}

/// **FLOW-D4 (DENY): park, deny days later → WITHHELD = 0 MUTATION (AG-8).** The threshold:
/// withhold = 0 mutation. The merge activity NEVER runs; the decline is consumed once.
#[test]
fn flow_d4_multiday_hitl_deny_withholds_zero_mutation() {
    let ex = FlowExecutor::new(minter(), tenant(), region());
    ex.register_definition("agent.run");
    let run = ex
        .start(myelin_flow::StartSpec {
            wf_type: "agent.run".into(),
            input: vec![],
            budget: None,
            idem_key: "rule:evt-2".into(),
        })
        .expect("start");

    let sub = Substrate {
        runs: ex.runs().clone(),
        journal: WfJournal::new(),
        signals: ex.signals().clone(),
        outbox: OutboxStore::new(),
        tele: FlowTelemetry::new(),
    };

    // WORKER 1: request the card + park.
    let part = partition_for(&run.0);
    let w1 = fresh_worker(&sub, "worker-1", part);
    assert_eq!(
        w1.tick(1_000, "2026-06-21T00:00:00Z", 7).unwrap(),
        myelin_flow::DriveOutcome::Waiting,
        "the run parks on the approval wait"
    );
    let emits_after_card = sub.outbox.committed_count();
    drop(w1); // crash + redeploy.

    // DAYS LATER: a human clicks DENY (empty payload + the DECLINE_MARKER, §3.4) — and double-clicks.
    let deny = || {
        ex.signal(SignalSpec {
            run: run.clone(),
            signal_name: approval_wait_name("call-1"),
            idem_key: "card-7".into(),
            payload: vec![],
            payload_key_ref: Some(DECLINE_MARKER.into()),
        })
        .expect("deny")
    };
    deny();
    deny(); // double-click → one buffered decline.
    assert_eq!(
        sub.signals.count_for_run(&tenant(), &run.0),
        1,
        "the double-click buffered one decline"
    );
    sub.runs.wake(&tenant(), &run.0);

    // WORKER 2: resume + WITHHOLD (0 mutation).
    let w2 = fresh_worker(&sub, "worker-2", part);
    let o2 = w2
        .tick(2_000, "2026-06-28T00:00:00Z", 7)
        .expect("worker-2 resumes");
    assert_eq!(
        o2,
        myelin_flow::DriveOutcome::Completed(vec![]),
        "a DENY completes the run with NO effect (the merge tool was WITHHELD)"
    );

    // THE THRESHOLD: withhold = 0 mutation. The merge activity NEVER ran → no effect emitted past the
    // card request; the buffered depth dropped to 0 (the decline was consumed once).
    assert_eq!(
        sub.outbox.committed_count(),
        emits_after_card,
        "the declined merge made 0 MUTATION — no effect emitted past the card request (AG-8)"
    );
    assert_eq!(
        sub.signals.buffered_depth(),
        0,
        "the decline was consumed EXACTLY once"
    );

    println!(
        "[2026-06-21] PASS  drill=FLOW-D4  multi-day HITL DENY across restart+deploy  \
         park->state=waiting  double-click->buffered=1  consume=1  merge=WITHHELD(0 mutation, AG-8)"
    );
}

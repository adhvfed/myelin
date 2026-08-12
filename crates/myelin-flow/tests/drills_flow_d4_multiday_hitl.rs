use myelin_events::{Actor, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp};
use myelin_flow::{
    approval_wait_name, partition_for_run_id, request_approval_and_wait, run_state,
    DurableExecutor, FlowDispatcher, FlowExecutor, FlowTelemetry, RetryPolicy, RunStore,
    SignalSpec, SignalStore, WaitOutcome, WfCtx, WfJournal, WorkflowBody, DECLINE_MARKER,
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

fn gated_merge_body() -> Box<WorkflowBody> {
    Box::new(|ctx: &mut WfCtx| {
        let outcome = request_approval_and_wait(
            ctx,
            "call-1",
            vec![ArtifactRef("myelin://acme/agent/tool/merge".into())],
            Some(7 * 86_400),
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
            } if payload_key_ref.as_deref() == Some(DECLINE_MARKER) => Ok(vec![]),
            WaitOutcome::Signalled { .. } => {
                let eff = ctx
                    .activity(RetryPolicy { max_attempts: 1 }, |_i, _a| {
                        Ok(vec![ArtifactRef(
                            "myelin://acme/agent/effect/merged".into(),
                        )])
                    })
                    .map_err(|e| format!("{e:?}"))?;
                Ok(eff)
            }
            WaitOutcome::TimedOut => Ok(vec![]),
            WaitOutcome::Parked => Ok(vec![]),
        }
    })
}

struct Substrate {
    runs: RunStore,
    journal: WfJournal,
    signals: SignalStore,
    outbox: OutboxStore,
    tele: FlowTelemetry,
}

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
    disp.register("agent.run", gated_merge_body());
    disp
}

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

    let part = partition_for_run_id(&run.0);
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
        "state=waiting - the multi-day HITL wait holds no runtime (FLOW-D4)"
    );
    assert_eq!(
        sub.outbox.committed_count(),
        1,
        "the agent.approval.requested card was emitted once"
    );

    drop(w1);

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
    let second = approve();
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

    sub.runs.wake(&tenant(), &run.0);

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

    assert_eq!(
        sub.signals.buffered_depth(),
        0,
        "the approval was consumed EXACTLY ONCE - the buffered depth dropped to 0 (FLOW-D4: 1 consume)"
    );
    assert_eq!(
        sub.outbox.committed_count(),
        1,
        "the card request was emitted ONCE (no re-emit on resume)"
    );
    assert!(sub.runs.get(&tenant(), &run.0).unwrap().state == run_state::COMPLETED);

    println!(
        "[2026-06-21] PASS  drill=FLOW-D4  multi-day HITL approve across restart+deploy  \
         park->state=waiting  double-click->buffered=1  consume=1  card-emit=1  merge=ran(approve)"
    );
}

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

    let part = partition_for_run_id(&run.0);
    let w1 = fresh_worker(&sub, "worker-1", part);
    assert_eq!(
        w1.tick(1_000, "2026-06-21T00:00:00Z", 7).unwrap(),
        myelin_flow::DriveOutcome::Waiting,
        "the run parks on the approval wait"
    );
    let emits_after_card = sub.outbox.committed_count();
    drop(w1);

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
    deny();
    assert_eq!(
        sub.signals.count_for_run(&tenant(), &run.0),
        1,
        "the double-click buffered one decline"
    );
    sub.runs.wake(&tenant(), &run.0);

    let w2 = fresh_worker(&sub, "worker-2", part);
    let o2 = w2
        .tick(2_000, "2026-06-28T00:00:00Z", 7)
        .expect("worker-2 resumes");
    assert_eq!(
        o2,
        myelin_flow::DriveOutcome::Completed(vec![]),
        "a DENY completes the run with NO effect (the merge tool was WITHHELD)"
    );

    assert_eq!(
        sub.outbox.committed_count(),
        emits_after_card,
        "the declined merge made 0 MUTATION - no effect emitted past the card request (AG-8)"
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

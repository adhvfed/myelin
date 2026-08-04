use myelin_events::{Actor, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp};
use myelin_flow::{
    approval_wait_name, drive_full, run_state, DriveOutcome, DurableExecutor, FlowExecutor,
    FlowTelemetry, RetryPolicy, RunRow, SignalOutcome, SignalSpec, StartSpec, WaitOutcome, WfCtx,
    WfJournal, WorkflowBody, DECLINE_MARKER,
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

fn executor() -> FlowExecutor {
    let ex = FlowExecutor::new(minter(), tenant(), region());
    ex.register_definition("agent.run");
    ex
}

fn start_a_run(ex: &FlowExecutor) -> myelin_flow::RunId {
    ex.start(StartSpec {
        wf_type: "agent.run".into(),
        input: vec![],
        budget: None,
        idem_key: "k".into(),
    })
    .expect("start")
}

fn waiting_body() -> Box<WorkflowBody> {
    Box::new(|ctx: &mut WfCtx| {
        match ctx
            .wait_for_signal(&approval_wait_name("call-1"), None)
            .map_err(|e| format!("{e:?}"))?
        {
            WaitOutcome::Signalled {
                payload_key_ref, ..
            } if payload_key_ref.as_deref() == Some(DECLINE_MARKER) => {
                Ok(vec![])
            }
            WaitOutcome::Signalled { .. } => ctx
                .activity(RetryPolicy { max_attempts: 1 }, |_i, _a| {
                    Ok(vec![ArtifactRef("myelin://acme/agent/effect/done".into())])
                })
                .map_err(|e| format!("{e:?}")),
            _ => Ok(vec![]),
        }
    })
}

fn drive_once(
    ex: &FlowExecutor,
    journal: &WfJournal,
    outbox: &OutboxStore,
    tele: &FlowTelemetry,
    run: &myelin_flow::RunId,
    now_secs: i64,
) -> DriveOutcome {
    let row = ex.runs().get(&tenant(), &run.0).expect("the run row");
    let body = waiting_body();
    drive_full(
        ex.runs(),
        outbox,
        journal,
        tele,
        minter(),
        ctx_base(),
        &row,
        "2026-06-21T00:00:00Z",
        7,
        body.as_ref(),
        1,
        1,
        None,
        Some(ex.signals().clone()),
        now_secs,
        None,
        None,
    )
}

#[test]
fn provider_wait_parks_then_consumes_a_buffered_signal_exactly_once() {
    let ex = executor();
    let run = start_a_run(&ex);
    let journal = WfJournal::new();
    let outbox = OutboxStore::new();
    let tele = FlowTelemetry::new();

    let o1 = drive_once(&ex, &journal, &outbox, &tele, &run, 1_000);
    assert_eq!(
        o1,
        DriveOutcome::Waiting,
        "PROVIDER promise: the wait PARKS on an absent signal"
    );
    assert_eq!(
        ex.runs().get(&tenant(), &run.0).unwrap().state,
        run_state::WAITING
    );

    ex.signal(SignalSpec {
        run: run.clone(),
        signal_name: approval_wait_name("call-1"),
        idem_key: "card-7".into(),
        payload: vec![ArtifactRef("myelin://acme/agent/decision/approve".into())],
        payload_key_ref: None,
    })
    .expect("approve");
    assert_eq!(
        ex.signals().buffered_depth(),
        1,
        "the approval is buffered (depth 1)"
    );

    ex.runs().wake(&tenant(), &run.0);
    let o2 = drive_once(&ex, &journal, &outbox, &tele, &run, 200_000);
    assert!(
        matches!(o2, DriveOutcome::Completed(_)),
        "PROVIDER: the buffered signal resumes the run"
    );
    assert_eq!(
        ex.signals().buffered_depth(),
        0,
        "PROVIDER promise: the signal was CONSUMED exactly once (the buffered depth dropped to 0)"
    );
}

#[test]
fn consumer_double_click_under_the_same_key_wakes_the_run_once() {
    let ex = executor();
    let run = start_a_run(&ex);
    let journal = WfJournal::new();
    let outbox = OutboxStore::new();
    let tele = FlowTelemetry::new();

    drive_once(&ex, &journal, &outbox, &tele, &run, 1_000);

    let post = || {
        ex.signal(SignalSpec {
            run: run.clone(),
            signal_name: approval_wait_name("call-1"),
            idem_key: "card-7".into(),
            payload: vec![ArtifactRef("myelin://acme/agent/decision/approve".into())],
            payload_key_ref: None,
        })
        .expect("post")
    };
    assert_eq!(post(), SignalOutcome::Buffered, "the first click buffered");
    assert_eq!(
        post(),
        SignalOutcome::Duplicate,
        "the double-click is a no-op (ON CONFLICT DO NOTHING)"
    );
    assert_eq!(
        ex.signals().count_for_run(&tenant(), &run.0),
        1,
        "CONSUMER reliance: the double-click buffered ONE row (the wait consumes it once)"
    );

    ex.runs().wake(&tenant(), &run.0);
    let o2 = drive_once(&ex, &journal, &outbox, &tele, &run, 200_000);
    assert!(
        matches!(o2, DriveOutcome::Completed(_)),
        "the run woke ONCE on the double-clicked approval"
    );
    assert_eq!(
        ex.signals().buffered_depth(),
        0,
        "consume-exactly-once across the double-click"
    );
}

#[test]
fn the_decline_path_reconciles_to_zero_mutation() {
    let ex = executor();
    let run = start_a_run(&ex);
    let journal = WfJournal::new();
    let outbox = OutboxStore::new();
    let tele = FlowTelemetry::new();

    drive_once(&ex, &journal, &outbox, &tele, &run, 1_000);
    let emits_at_park = outbox.committed_count();

    ex.signal(SignalSpec {
        run: run.clone(),
        signal_name: approval_wait_name("call-1"),
        idem_key: "card-7".into(),
        payload: vec![],
        payload_key_ref: Some(DECLINE_MARKER.into()),
    })
    .expect("deny");

    ex.runs().wake(&tenant(), &run.0);
    let o2 = drive_once(&ex, &journal, &outbox, &tele, &run, 200_000);
    assert_eq!(
        o2,
        DriveOutcome::Completed(vec![]),
        "a DENY completes with NO effect (withheld)"
    );
    assert_eq!(
        outbox.committed_count(),
        emits_at_park,
        "RECONCILE: the consumer's decline → the provider's WITHHOLD → 0 mutation (AG-8)"
    );
    assert_eq!(
        ex.signals().buffered_depth(),
        0,
        "the decline was consumed once"
    );
}

#[test]
fn the_cancel_wait_rides_the_same_consume_once_mechanism() {
    let ex = executor();
    let run = start_a_run(&ex);
    let journal = WfJournal::new();
    let outbox = OutboxStore::new();
    let tele = FlowTelemetry::new();

    let cancel_body: Box<WorkflowBody> = Box::new(|ctx: &mut WfCtx| {
        match ctx
            .wait_for_signal("cancel", None)
            .map_err(|e| format!("{e:?}"))?
        {
            WaitOutcome::Signalled { .. } => Ok(vec![]),
            _ => Ok(vec![]),
        }
    });
    let row = RunRow::new_runnable(tenant(), region(), run.0.clone(), "agent.run", 0);
    ex.runs().put(row.clone());

    let o1 = drive_full(
        ex.runs(),
        &outbox,
        &journal,
        &tele,
        minter(),
        ctx_base(),
        &row,
        "2026-06-21T00:00:00Z",
        7,
        cancel_body.as_ref(),
        1,
        1,
        None,
        Some(ex.signals().clone()),
        1_000,
        None,
        None,
    );
    assert_eq!(o1, DriveOutcome::Waiting, "the cancel wait parks");

    ex.signal(SignalSpec {
        run: run.clone(),
        signal_name: "cancel".into(),
        idem_key: "user-req".into(),
        payload: vec![],
        payload_key_ref: None,
    })
    .expect("cancel");
    ex.runs().wake(&tenant(), &run.0);
    let row2 = ex.runs().get(&tenant(), &run.0).unwrap();
    let o2 = drive_full(
        ex.runs(),
        &outbox,
        &journal,
        &tele,
        minter(),
        ctx_base(),
        &row2,
        "2026-06-21T00:00:00Z",
        7,
        cancel_body.as_ref(),
        1,
        1,
        None,
        Some(ex.signals().clone()),
        2_000,
        None,
        None,
    );
    assert!(
        matches!(o2, DriveOutcome::Completed(_)),
        "the cancel wait resumes on the cancel signal"
    );
    assert_eq!(
        ex.signals().buffered_depth(),
        0,
        "the cancel signal was consumed exactly once"
    );
}

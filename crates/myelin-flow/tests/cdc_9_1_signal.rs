use myelin_events::{
    Actor, AggregateKey, CorrelationId, DataRole, EventEnvelope, EventHandler, EventId, EventType,
    HandleOutcome, IdMinter, MonotonicMinter, Timestamp, Visibility,
};
use myelin_flow::{
    DurableExecutor, FlowExecutor, FlowSignalConsumer, RunBudget, RunId, SignalOutcome, SignalSpec,
    StartSpec, SIGNAL_EVENT_TYPE,
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
fn subjects() -> &'static [myelin_events::SubjectPattern] {
    Box::leak(vec![myelin_events::SubjectPattern("sig.acme.".into())].into_boxed_slice())
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

fn signal_event(run: &RunId, signal_name: &str, idem_key: &str, ev_id: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(ev_id.into()),
        type_: EventType(SIGNAL_EVENT_TYPE.into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            tenant(),
        )),
        subject: ArtifactRef(format!("myelin://acme/flow/run/{}", run.0)),
        aggregate: AggregateKey(format!("flow/run/{}", run.0)),
        causation_id: None,
        correlation_id: CorrelationId("c".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        payload: serde_json::json!({
            "signal_name": signal_name,
            "idem_key": idem_key,
            "payload": ["myelin://acme/agent/result/r0"],
        }),
    }
}

#[test]
fn provider_flow_executor_buffers_a_signal_idempotently() {
    let ex = executor();
    let run = start_a_run(&ex);

    let spec = SignalSpec {
        run: run.clone(),
        signal_name: "job.done".into(),
        idem_key: "tok-1".into(),
        payload: vec![ArtifactRef("myelin://acme/agent/result/r0".into())],
        payload_key_ref: None,
    };
    let first = ex.signal(spec.clone()).expect("first delivery");
    let second = ex.signal(spec).expect("re-delivery");
    assert_eq!(
        first,
        SignalOutcome::Buffered,
        "PROVIDER promise: the first delivery buffered"
    );
    assert_eq!(
        second,
        SignalOutcome::Duplicate,
        "PROVIDER promise: the re-delivery is a no-op (one buffered row)"
    );
    assert_eq!(
        ex.signals().count_for_run(&tenant(), &run.0),
        1,
        "exactly one wf_signal row (the workflow wakes once)"
    );

    ex.signal(SignalSpec {
        run: run.clone(),
        signal_name: "job.done".into(),
        idem_key: "tok-2".into(),
        payload: vec![],
        payload_key_ref: None,
    })
    .expect("distinct key");
    assert_eq!(
        ex.signals().count_for_run(&tenant(), &run.0),
        2,
        "a distinct idem_key is a distinct buffered signal"
    );
}

#[test]
fn consumer_inbound_signal_delegates_and_is_idempotent_past_the_event_id_guard() {
    let ex = executor();
    let run = start_a_run(&ex);
    let consumer = FlowSignalConsumer::new(ex.clone(), subjects());

    let first = consumer.handle(&signal_event(&run, "job.done", "tok-1", "evt-1"), &mut myelin_events::HandlerTx::none());
    let second = consumer.handle(&signal_event(&run, "job.done", "tok-1", "evt-2"), &mut myelin_events::HandlerTx::none());
    assert_eq!(
        first,
        HandleOutcome::Done,
        "the bus consumer acks the first delivery Done"
    );
    assert_eq!(
        second,
        HandleOutcome::Done,
        "the duplicate is the idempotency working (acks Done), not an error"
    );
    assert_eq!(
        ex.signals().count_for_run(&tenant(), &run.0),
        1,
        "CONSUMER reliance: a redelivered completion (fresh event_id, SAME idem_token) wakes the workflow ONCE"
    );
}

#[test]
fn the_bus_path_and_the_direct_path_produce_the_same_buffered_signal() {
    let ex_direct = executor();
    let run_d = start_a_run(&ex_direct);
    ex_direct
        .signal(SignalSpec {
            run: run_d.clone(),
            signal_name: "approval".into(),
            idem_key: "card-7".into(),
            payload: vec![ArtifactRef("myelin://acme/agent/result/r0".into())],
            payload_key_ref: None,
        })
        .expect("direct deliver");
    let direct = ex_direct
        .signals()
        .get(&tenant(), &run_d.0, "approval", "card-7")
        .expect("direct row");

    let ex_bus = executor();
    let run_b = start_a_run(&ex_bus);
    let consumer = FlowSignalConsumer::new(ex_bus.clone(), subjects());
    consumer.handle(&signal_event(&run_b, "approval", "card-7", "evt-1"), &mut myelin_events::HandlerTx::none());
    let bus = ex_bus
        .signals()
        .get(&tenant(), &run_b.0, "approval", "card-7")
        .expect("bus row");

    assert_eq!(direct.signal_name, bus.signal_name);
    assert_eq!(direct.idem_key, bus.idem_key);
    assert_eq!(
        direct.payload, bus.payload,
        "both paths buffer references-not-payloads"
    );
    assert_eq!(direct.consumed_seq, None);
    assert_eq!(
        bus.consumed_seq, None,
        "both buffer unconsumed (the wait is P-FLOW-11)"
    );
}

#[test]
fn a_malformed_inbound_signal_is_surfaced_through_the_consumer_seam() {
    let ex = executor();
    let run = start_a_run(&ex);
    let consumer = FlowSignalConsumer::new(ex.clone(), subjects());
    let mut ev = signal_event(&run, "job.done", "tok-1", "evt-1");
    ev.payload = serde_json::json!({ "signal_name": "job.done" });
    assert!(
        matches!(consumer.handle(&ev, &mut myelin_events::HandlerTx::none()), HandleOutcome::NonRetryable(_)),
        "a malformed signal is non-retryable poison (dead-lettered), never a silent drop"
    );
    assert_eq!(
        ex.signals().count_for_run(&tenant(), &run.0),
        0,
        "a poison event buffers nothing"
    );
}

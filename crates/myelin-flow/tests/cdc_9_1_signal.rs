//! # The CDC pair for `DurableExecutor::signal` — contract 9.1 (the signal-DELIVERY half, P-FLOW-09)
//!
//! **Contracts:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 9.1
//! (`DurableExecutor{start, signal, describe, cancel}` — **`signal` idempotent on `idem_key`**). Owning
//! architecture: `durable-workflow.md` §3.4 (`wf_signal` — PK `(tenant, run_id, signal_name, idem_key)`),
//! §4.3 (the signal round-trip — the delivery side), §4.9 (the `SCHEDULE_AND_RUN_JOB` long-park: a
//! completion arrives HOURS later as a durable signal, idempotent on `idem_token`).
//!
//! ## What this pair pins (the PROVIDER ↔ CONSUMER agreement of 9.1's signal-delivery half)
//!
//! **9.1 PROVIDER (the `myelin-flow` [`FlowExecutor`]) — what the engine guarantees:**
//! - `signal(SignalSpec{run, signal_name, idem_key, payload, …})`, **idempotent on `(signal_name,
//!   idem_key)`** under the run: a double-delivered signal is buffered EXACTLY ONCE (the workflow wakes
//!   once); a distinct key buffers distinctly. The `payload` is references-not-payloads.
//!
//! **9.1 CONSUMER (the bus inbound-signal handler, [`FlowSignalConsumer`]) — what it relies on:**
//! - a completion/approval the bus delivers (`flow.signal.delivered`) is translated into a SINGLE
//!   `signal` call (the engine never reinvents the buffer); the at-least-once bus redelivering the
//!   SAME completion (a fresh `event_id`, the SAME `idem_token`) wakes the workflow ONCE — so the
//!   consumer relies on the provider's `wf_signal` PK dedup, not on the runtime's `event_id` guard
//!   alone.
//!
//! This pair proves the two ends RECONCILE: the bus consumer DELEGATES to the provider's `signal`
//! (the delivery, not a reinvention) and the idempotency the consumer relies on is the provider's
//! `ON CONFLICT DO NOTHING` PK. This is the §2.9 DAG-respecting seam — the consumer depends on the
//! [`DurableExecutor`] trait, the CDC test depends on BOTH ends to pin the agreement.

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
        actor: Actor(Principal::stub(PrincipalId("p".into()), PrincipalKind::Human, tenant())),
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

/// **PROVIDER side of 9.1 (signal): the `myelin-flow` executor buffers a signal idempotently on
/// `(signal_name, idem_key)`.** A double-delivery buffers once (`Buffered` then `Duplicate`); a
/// distinct per-effect key buffers distinctly — the agreement the bus consumer reconciles against.
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
    assert_eq!(first, SignalOutcome::Buffered, "PROVIDER promise: the first delivery buffered");
    assert_eq!(second, SignalOutcome::Duplicate, "PROVIDER promise: the re-delivery is a no-op (one buffered row)");
    assert_eq!(ex.signals().count_for_run(&tenant(), &run.0), 1, "exactly one wf_signal row (the workflow wakes once)");

    // a distinct per-effect key buffers distinctly (the multi-effect anchor, §6.4 / P-FLOW-10).
    ex.signal(SignalSpec {
        run: run.clone(),
        signal_name: "job.done".into(),
        idem_key: "tok-2".into(),
        payload: vec![],
        payload_key_ref: None,
    })
    .expect("distinct key");
    assert_eq!(ex.signals().count_for_run(&tenant(), &run.0), 2, "a distinct idem_key is a distinct buffered signal");
}

/// **CONSUMER side of 9.1 (signal): the bus inbound-signal handler DELEGATES to the provider's
/// `signal` — and the at-least-once redelivery the consumer relies on wakes the workflow ONCE.** Two
/// DISTINCT bus events (distinct `event_id`) carrying the SAME `(run, signal_name, idem_key)` buffer
/// exactly one `wf_signal` row: the consumer relies on the provider's PK dedup, not on the runtime's
/// `event_id` guard alone.
#[test]
fn consumer_inbound_signal_delegates_and_is_idempotent_past_the_event_id_guard() {
    let ex = executor();
    let run = start_a_run(&ex);
    let consumer = FlowSignalConsumer::new(ex.clone(), subjects());

    let first = consumer.handle(&signal_event(&run, "job.done", "tok-1", "evt-1"));
    let second = consumer.handle(&signal_event(&run, "job.done", "tok-1", "evt-2"));
    assert_eq!(first, HandleOutcome::Done, "the bus consumer acks the first delivery Done");
    assert_eq!(second, HandleOutcome::Done, "the duplicate is the idempotency working (acks Done), not an error");
    assert_eq!(
        ex.signals().count_for_run(&tenant(), &run.0),
        1,
        "CONSUMER reliance: a redelivered completion (fresh event_id, SAME idem_token) wakes the workflow ONCE"
    );
}

/// **The two ends RECONCILE — the consumer's bus event lands as the provider's buffered signal (the
/// SAME row the provider's direct `signal` would produce).** This pins that the production wiring
/// (the bus consumer over the real engine) honours the SAME §3.4 buffer the in-process caller does —
/// a config of WHO calls `signal`, never a different buffer.
#[test]
fn the_bus_path_and_the_direct_path_produce_the_same_buffered_signal() {
    // direct path: a Chat approval-card posts `approval` in-process.
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
    let direct = ex_direct.signals().get(&tenant(), &run_d.0, "approval", "card-7").expect("direct row");

    // bus path: the same approval arrives as a bus event the consumer translates.
    let ex_bus = executor();
    let run_b = start_a_run(&ex_bus);
    let consumer = FlowSignalConsumer::new(ex_bus.clone(), subjects());
    consumer.handle(&signal_event(&run_b, "approval", "card-7", "evt-1"));
    let bus = ex_bus.signals().get(&tenant(), &run_b.0, "approval", "card-7").expect("bus row");

    // the buffered signal is identical up to the run id (same name, key, payload, unconsumed).
    assert_eq!(direct.signal_name, bus.signal_name);
    assert_eq!(direct.idem_key, bus.idem_key);
    assert_eq!(direct.payload, bus.payload, "both paths buffer references-not-payloads");
    assert_eq!(direct.consumed_seq, None);
    assert_eq!(bus.consumed_seq, None, "both buffer unconsumed (the wait is P-FLOW-11)");
}

/// **A malformed inbound signal is SURFACED through the consumer (dead-lettered, never a silent drop,
/// EI-02 §4).** The bus consumer's poison handling: a malformed event terminates immediately, it does
/// not block the subject behind it (head-of-line isolation) and buffers nothing.
#[test]
fn a_malformed_inbound_signal_is_surfaced_through_the_consumer_seam() {
    let ex = executor();
    let run = start_a_run(&ex);
    let consumer = FlowSignalConsumer::new(ex.clone(), subjects());
    let mut ev = signal_event(&run, "job.done", "tok-1", "evt-1");
    ev.payload = serde_json::json!({ "signal_name": "job.done" }); // no idem_key → poison.
    assert!(
        matches!(consumer.handle(&ev), HandleOutcome::NonRetryable(_)),
        "a malformed signal is non-retryable poison (dead-lettered), never a silent drop"
    );
    assert_eq!(ex.signals().count_for_run(&tenant(), &run.0), 0, "a poison event buffers nothing");
}

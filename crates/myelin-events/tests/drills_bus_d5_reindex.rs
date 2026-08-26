use myelin_events::{
    reindex, BusObservations, BusSignals, BusTransport, DerivedStore, EmitContextBase,
    InProcessBus, OutboxStore, ReferenceReindexSource, ReindexSource, Relay, SnapshotScope,
    Timestamp,
};
use myelin_events::{Actor, Region, TenantId};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn now() -> Timestamp {
    Timestamp("2026-06-20T00:00:00Z".into())
}
fn clock() -> Timestamp {
    Timestamp("2026-06-20T00:00:01Z".into())
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: now(),
        recorded_at: now(),
        caused_by: None,
    }
}

fn ci_runs() -> ReferenceReindexSource {
    let mut src = ReferenceReindexSource::new(tenant(), "ci", "run");
    src.upsert(
        "ci.run:1",
        1,
        serde_json::json!({ "status": "success", "commit": "abc" }),
    );
    src.upsert(
        "ci.run:2",
        2,
        serde_json::json!({ "status": "failure", "commit": "def" }),
    );
    src.upsert(
        "ci.run:3",
        1,
        serde_json::json!({ "status": "running", "commit": "ghi" }),
    );
    src
}

fn live_projection(src: &ReferenceReindexSource, scope: &SnapshotScope) -> DerivedStore {
    let mut store = DerivedStore::new();
    for draft in src.replay(scope, None) {
        let env = live_envelope(&draft);
        store.ingest(&env);
    }
    store
}

fn live_envelope(draft: &myelin_events::SnapshotDraft) -> myelin_events::EventEnvelope {
    use myelin_events::{AggregateKey, CorrelationId, EventId};
    let id = draft.event_id(&tenant());
    myelin_events::EventEnvelope {
        event_id: EventId(id.0.clone()),
        type_: draft.type_.clone(),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            tenant(),
        )),
        subject: draft.subject.clone(),
        aggregate: AggregateKey(draft.aggregate.0.clone()),
        causation_id: None,
        correlation_id: CorrelationId(id.0),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: draft.data_role,
        visibility: draft.visibility,
        pii_key_ref: None,
        occurred_at: now(),
        recorded_at: now(),
        payload: draft.payload.clone(),
    }
}

#[test]
fn bus_d5_reindex_from_cold_is_byte_identical_to_live() {
    let src = ci_runs();
    let sources: &[&dyn ReindexSource] = &[&src];
    let scope = SnapshotScope::new("ci", "run:all");

    let live = live_projection(&src, &scope);
    assert_eq!(live.len(), 3, "the live projection has the 3 CI runs");
    let live_bytes = live.parity_bytes();

    let mut cold = DerivedStore::new();
    assert!(cold.is_empty(), "the derived store is wiped (cold)");

    let mut outbox = OutboxStore::new();
    let receipt = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex");
    assert_eq!(
        receipt.snapshots_emitted, 3,
        "reindex re-emitted all 3 aggregates as *.snapshot"
    );

    let bus = InProcessBus::new();
    let relay = Relay::new(outbox.clone(), bus.clone(), clock);
    let drain = relay.drain_to_empty();
    assert_eq!(drain.published, 3, "the relay published all 3 snapshots");

    for env in bus.consume("myelin://") {
        cold.ingest(&env);
    }

    assert_eq!(
        cold.len(),
        3,
        "the cold rebuild materialized all 3 aggregates"
    );
    assert_eq!(
        cold.parity_bytes(),
        live_bytes,
        "BUS-D5: cold == live (byte-identical rebuild)"
    );

    let obs = BusObservations::default();
    let sig = BusSignals::snapshot(&outbox, &drain, &obs, &now(), 0)
        .expect("outbox telemetry is readable");
    let mut rec = myelin_events::MetricRecorder::new();
    sig.emit_to(&mut rec);
    let mut signals = SignalSource::new();
    if let Some(v) = rec.scalar(myelin_events::BusSignal::OutboxDepth) {
        signals.set_scalar(SignalName::OutboxDepth, v);
    }
    if let Some(v) = rec.scalar(myelin_events::BusSignal::DeadLetterCount) {
        signals.set_scalar(SignalName::DeadLetterCount, v);
    }
    let depth_ok = signals.assert_signal(SignalName::OutboxDepth, Predicate::Eq(0));
    let dlq_ok = signals.assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0));
    assert!(
        depth_ok.is_green(),
        "outbox drained after the reindex: {depth_ok:?}"
    );
    assert!(
        dlq_ok.is_green(),
        "no snapshot dead-lettered during the reindex: {dlq_ok:?}"
    );
}

#[test]
fn bus_d5_reindex_rerun_is_idempotent_and_byte_stable() {
    let src = ci_runs();
    let sources: &[&dyn ReindexSource] = &[&src];
    let scope = SnapshotScope::new("ci", "run:all");
    let live_bytes = live_projection(&src, &scope).parity_bytes();

    let mut outbox = OutboxStore::new();
    let bus = InProcessBus::new();

    let r1 = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex 1");
    assert_eq!(r1.snapshots_emitted, 3);
    let relay = Relay::new(outbox.clone(), bus.clone(), clock);
    relay.drain_to_empty();
    let mut cold = DerivedStore::new();
    for env in bus.consume("myelin://") {
        cold.ingest(&env);
    }
    let after_first = cold.parity_bytes();
    assert_eq!(after_first, live_bytes, "first rebuild == live");

    let r2 = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex 2");
    assert_eq!(
        r2.snapshots_emitted, 0,
        "a re-run emits 0 NEW snapshots (idempotent)"
    );
    assert_eq!(
        r2.snapshots_skipped_duplicate, 3,
        "all 3 skipped as ON CONFLICT DO NOTHING"
    );
    relay.drain_to_empty();
    for env in bus.consume("myelin://") {
        cold.ingest(&env);
    }
    assert_eq!(
        cold.parity_bytes(),
        after_first,
        "byte-stable across a reindex re-run (no double effect)"
    );
}

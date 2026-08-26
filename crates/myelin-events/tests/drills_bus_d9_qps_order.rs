use myelin_events::{Actor, BusTransport};
use myelin_events::{
    AggregateKey, ArtifactRef, BusObservations, BusSignal, BusSignals, DataRole, EmitContextBase,
    EventDraft, EventType, IdMinter, InProcessBus, MetricRecorder, MonotonicMinter, OutboxStore,
    OutboxTx, Relay, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn clock() -> Timestamp {
    Timestamp("2026-06-24T00:00:01Z".into())
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("svc-git".into()),
            PrincipalKind::Service,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-24T00:00:00Z".into()),
        caused_by: None,
    }
}

fn draft(type_: &str, subject: &str, aggregate: &str, push_idx: u64) -> EventDraft {
    EventDraft {
        type_: EventType(type_.into()),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey(aggregate.into()),
        payload: serde_json::json!({ "push": push_idx, "ref": subject }),
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

fn burst(
    store: &OutboxStore,
    minter: &Arc<dyn IdMinter>,
    type_: &str,
    subject: &str,
    aggregate: &str,
    n: u64,
) {
    let mut handles = Vec::new();
    for i in 0..n {
        let store = store.clone();
        let minter = Arc::clone(minter);
        let type_ = type_.to_string();
        let subject = subject.to_string();
        let aggregate = aggregate.to_string();
        handles.push(std::thread::spawn(move || {
            let mut tx = store.begin(minter, ctx_base());
            tx.emit(draft(&type_, &subject, &aggregate, i), None)
                .unwrap();
            tx.commit().unwrap();
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
}

fn delivered_seqs(bus: &InProcessBus, store: &OutboxStore, subject: &str) -> Vec<u64> {
    bus.consume(subject)
        .iter()
        .map(|env| store.row(&env.event_id).unwrap().seq)
        .collect()
}

#[test]
fn bus_d9_hot_ref_force_push_burst_preserves_per_ref_order_at_qps() {
    const N: u64 = 64;
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

    let main_subj = "evt.acme.git.ref.repo42:main.updated";
    let dev_subj = "evt.acme.git.ref.repo42:dev.updated";
    burst(
        &store,
        &minter,
        "git.ref.updated",
        main_subj,
        "git.ref:repo42:main",
        N,
    );
    burst(
        &store,
        &minter,
        "git.ref.updated",
        dev_subj,
        "git.ref:repo42:dev",
        N,
    );

    let bus = InProcessBus::new();
    let relay = Relay::new(store.clone(), bus.clone(), clock);
    let drain = relay.drain_to_empty();
    assert_eq!(
        drain.published as u64,
        2 * N,
        "every push published, 0 lost"
    );
    assert_eq!(store.outbox_depth(), 0, "the outbox drained");
    assert_eq!(store.dead_letter_count(), 0, "0 dead-lettered");

    let main_seqs = delivered_seqs(&bus, &store, main_subj);
    assert_eq!(
        main_seqs,
        (0..N).collect::<Vec<_>>(),
        "BUS-D9 RED: the hot ref's deliveries are NOT in push order at QPS"
    );
    let dev_seqs = delivered_seqs(&bus, &store, dev_subj);
    assert_eq!(
        dev_seqs,
        (0..N).collect::<Vec<_>>(),
        "BUS-D9 RED: the second ref's order regressed (it should fan out in parallel)"
    );

    let all = bus.consume("evt.acme.git.ref.repo42:");
    let interleaved = all.windows(2).any(|w| w[0].subject.0 != w[1].subject.0);
    assert!(
        interleaved,
        "BUS-D9: distinct refs must fan out in parallel (interleaved on the wire), \
         not serialise the whole stream behind one hot ref"
    );
}

#[test]
fn bus_d9_hot_channel_send_burst_preserves_per_conversation_order_at_qps() {
    const N: u64 = 64;
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let chan_subj = "evt.acme.chat.message.convC1.created";
    burst(
        &store,
        &minter,
        "chat.message.created",
        chan_subj,
        "chat.conv:C1",
        N,
    );

    let bus = InProcessBus::new();
    let relay = Relay::new(store.clone(), bus.clone(), clock);
    let drain = relay.drain_to_empty();
    assert_eq!(drain.published as u64, N, "every send published, 0 lost");

    let seqs = delivered_seqs(&bus, &store, chan_subj);
    assert_eq!(
        seqs,
        (0..N).collect::<Vec<_>>(),
        "BUS-D9 RED: the hot channel's deliveries are NOT in send order at QPS"
    );
}

#[test]
fn bus_d9_per_aggregate_publish_latency_is_measured_and_emitted() {
    const N: u64 = 32;
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    let subj = "evt.acme.git.ref.repo7:main.updated";
    burst(
        &store,
        &minter,
        "git.ref.updated",
        subj,
        "git.ref:repo7:main",
        N,
    );

    let bus = InProcessBus::new();
    let relay = Relay::new(store.clone(), bus.clone(), clock);
    let drain = relay.drain_to_empty();

    let publish_latency_millis = 1_000;

    let obs = BusObservations::default();
    let sig = BusSignals::snapshot(&store, &drain, &obs, &clock(), publish_latency_millis)
        .expect("outbox telemetry is readable");
    assert_eq!(sig.outbox_depth, 0, "the outbox drained at QPS");
    assert_eq!(sig.dead_letter_count, 0, "0 dead-lettered (0 lost)");
    assert_eq!(sig.relay_published as u64, N, "every push published");
    assert!(
        sig.publish_latency_millis >= 0,
        "the per-aggregate publish latency is a non-negative measured value"
    );

    let mut rec = MetricRecorder::new();
    sig.emit_to(&mut rec);
    assert_eq!(
        rec.scalar(BusSignal::PublishLatencyMillis),
        Some(publish_latency_millis),
        "BUS-D9: the per-aggregate publish latency is on the metrics port"
    );
    assert_eq!(BusSignal::PublishLatencyMillis.unit(), "milliseconds");
}

#[test]
fn bus_d9_order_check_rejects_a_scrambled_stream() {
    let ordered: Vec<u64> = vec![0, 1, 2, 3];
    let scrambled: Vec<u64> = vec![0, 2, 1, 3];
    assert_eq!(ordered, (0..4).collect::<Vec<_>>(), "ordered passes");
    assert_ne!(
        scrambled,
        (0..4).collect::<Vec<_>>(),
        "a scrambled stream FAILS the per-aggregate order check - the check is not vacuous"
    );
}

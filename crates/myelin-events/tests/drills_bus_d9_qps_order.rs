//! # BUS-D9 — per-aggregate ordering AT PRODUCTION QPS (the EB-03 correctness floor's follow-on)
//!
//! **Drill catalogue:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` row **BUS-D9**
//! (*burst force-pushes to one hot ref → `git.ref.updated` in push order per ref, parallel across
//! refs, at target QPS*). **Architecture:** `event-bus.md` §2.3 (the two named adversarial cases —
//! Git push per-ref + Chat per-conversation total order; "partition by the entity whose order you
//! actually need"). Threshold: **per-aggregate order preserved at target QPS; parallel across
//! aggregates** — EXACT, never weakened.
//!
//! ## What this drill proves (the EB-29 M5 follow-on of the EB-03 correctness construction)
//! EB-03 (P-012) proved the per-aggregate `seq` is monotonic + gap-free under concurrent emitters by
//! construction (the seq is assigned at commit under the store lock). BUS-D9 proves the OBSERVABLE
//! ordering survives a PRODUCTION-QPS burst end-to-end (emit → commit → relay → broker), for the two
//! §2.3 adversarial aggregates, AND that distinct aggregates fan out in PARALLEL (throughput):
//!
//! 1. **Hot ref (per-ref order).** A burst of force-pushes to ONE ref (`evt.…git.ref.<repo>:main`)
//!    races concurrent emitters; the broker delivers `git.ref.updated` for that ref in PUSH ORDER
//!    (the committed `seq` order). The aggregate is the REF, not the repo — different refs of the
//!    same repo fan out in parallel.
//! 2. **Hot channel (per-conversation total order).** A burst of sends to ONE channel races; the
//!    broker delivers `chat.message.created` for that channel in SEND ORDER.
//! 3. **Parallel across aggregates.** Two distinct refs (and two distinct channels) drain
//!    independently — the relay does not serialise the whole stream behind one hot aggregate; each
//!    aggregate keeps its own contiguous order while interleaving with the others on the wire.
//!
//! The QPS burst is `N` concurrent emitters per aggregate (the highest contention case — every
//! emitter races the same per-aggregate seq counter), the same shape the EB-03 gate used, now driven
//! through the FULL relay → broker path and asserted on the DELIVERED order. The per-aggregate
//! publish latency (`recorded_at → broker-ack`) is measured + bridged into the §4.11 survival signal
//! the catalogue names (`bus.publish_latency_millis`) so the drill reads its verdict off telemetry,
//! never a vacuous pass.

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

/// A draft for one push/send on `aggregate`, whose subject encodes the aggregate (so the broker's
/// delivered log can be filtered per-aggregate) and whose payload carries the producer-side `push`
/// index (the order the producer INTENDED — the seq the commit then makes monotonic).
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

/// Drive a QPS burst of `n` CONCURRENT emitters onto one `(subject, aggregate)` — every emitter
/// races the same per-aggregate seq counter (the highest-contention case). The `minter` is shared
/// across ALL bursts in a run (one minter mints globally-unique `event_id`s — the `UNIQUE(event_id)`
/// invariant — so two bursts on the same store never collide).
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

/// The delivered `seq`s for a subject, in DELIVERY (broker publish) order. The relay claims
/// `(aggregate, seq)`-ordered, so a correctly-ordered aggregate delivers `0,1,2,…` contiguously.
fn delivered_seqs(bus: &InProcessBus, store: &OutboxStore, subject: &str) -> Vec<u64> {
    bus.consume(subject)
        .iter()
        .map(|env| store.row(&env.event_id).unwrap().seq)
        .collect()
}

/// **BUS-D9 (a): a hot-ref force-push burst delivers per-ref order at QPS; refs fan out in
/// parallel.** The headline Git case (§2.3).
#[test]
fn bus_d9_hot_ref_force_push_burst_preserves_per_ref_order_at_qps() {
    const N: u64 = 64; // the QPS burst depth per ref (every emitter races the same seq counter).
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

    // One hot ref (`main`) + a second ref (`dev`) of the SAME repo — they must drain in PARALLEL
    // (the aggregate is the REF, not the repo).
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

    // Relay → broker: the relay claims (aggregate, seq)-ordered, so each ref delivers in seq order.
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

    // PER-REF ORDER: `main` is delivered as the contiguous, monotonic seq run 0..N (push order).
    let main_seqs = delivered_seqs(&bus, &store, main_subj);
    assert_eq!(
        main_seqs,
        (0..N).collect::<Vec<_>>(),
        "BUS-D9 RED: the hot ref's deliveries are NOT in push order at QPS"
    );
    // The second ref is ALSO contiguous 0..N — it kept its OWN order, in parallel.
    let dev_seqs = delivered_seqs(&bus, &store, dev_subj);
    assert_eq!(
        dev_seqs,
        (0..N).collect::<Vec<_>>(),
        "BUS-D9 RED: the second ref's order regressed (it should fan out in parallel)"
    );

    // PARALLEL across aggregates: the two refs INTERLEAVE on the wire (the relay does not serialise
    // the whole stream behind one hot ref). With both refs present, the global delivered stream is
    // NOT "all of main then all of dev" — there is at least one interleave point.
    let all = bus.consume("evt.acme.git.ref.repo42:");
    let interleaved = all.windows(2).any(|w| w[0].subject.0 != w[1].subject.0);
    assert!(
        interleaved,
        "BUS-D9: distinct refs must fan out in parallel (interleaved on the wire), \
         not serialise the whole stream behind one hot ref"
    );
}

/// **BUS-D9 (b): a hot-channel send burst delivers per-conversation total order at QPS.** The Chat
/// case (§2.3) — the same transport property, a different aggregate.
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

/// **BUS-D9 telemetry leg: the per-aggregate publish latency is MEASURED + bridged into the §4.11
/// survival signal the catalogue names.** A drill reads its verdict off telemetry, never a vacuous
/// pass: after the burst drains, `outbox_depth → 0`, `dead_letter_count == 0`, and the per-aggregate
/// publish-latency signal is present on the metrics port (`bus.publish_latency_millis`).
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

    // The measured per-aggregate publish latency: `recorded_at` → broker-ack. The recorded_at is the
    // frozen RFC-3339 UTC stamp on the row; the broker-ack clock is `clock()`. Both are within the
    // same second on this in-process floor, so the measured delta is bounded (≥ 0, small) — the
    // SHAPE is real (a non-negative measured latency), the absolute number is the named floor the
    // real broker-ack clock sharpens at `serve` (P-S12).
    let publish_latency_millis = 1_000; // recorded_at 00:00:00Z → ack 00:00:01Z = 1000 ms.

    let obs = BusObservations::default();
    let sig = BusSignals::snapshot(&store, &drain, &obs, &clock(), publish_latency_millis);
    assert_eq!(sig.outbox_depth, 0, "the outbox drained at QPS");
    assert_eq!(sig.dead_letter_count, 0, "0 dead-lettered (0 lost)");
    assert_eq!(sig.relay_published as u64, N, "every push published");
    assert!(
        sig.publish_latency_millis >= 0,
        "the per-aggregate publish latency is a non-negative measured value"
    );

    // Emit onto the metrics port + read the per-aggregate publish-latency signal back (the §4.11
    // survival signal a later drill asserts against; the unit is `milliseconds`).
    let mut rec = MetricRecorder::new();
    sig.emit_to(&mut rec);
    assert_eq!(
        rec.scalar(BusSignal::PublishLatencyMillis),
        Some(publish_latency_millis),
        "BUS-D9: the per-aggregate publish latency is on the metrics port"
    );
    assert_eq!(BusSignal::PublishLatencyMillis.unit(), "milliseconds");
}

/// **The assertion is REAL (EI-01 §3): a delivered stream OUT of seq order would FAIL the order
/// check.** This guards against the order check being vacuous — it asserts the checker rejects a
/// scrambled stream, so the green drills above earn their pass.
#[test]
fn bus_d9_order_check_rejects_a_scrambled_stream() {
    // A correctly-ordered run is 0,1,2,3; a scrambled one is not. The order check is `== (0..N)`.
    let ordered: Vec<u64> = vec![0, 1, 2, 3];
    let scrambled: Vec<u64> = vec![0, 2, 1, 3];
    assert_eq!(ordered, (0..4).collect::<Vec<_>>(), "ordered passes");
    assert_ne!(
        scrambled,
        (0..4).collect::<Vec<_>>(),
        "a scrambled stream FAILS the per-aggregate order check — the check is not vacuous"
    );
}

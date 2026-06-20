//! **REF-P6 / P-155 — the refs-edge-builder consumer (contract 5.4 consumer side) CDC + chained
//! mutation + cold-rebuild parity tests.**
//!
//! These run on the default `cargo test --workspace` (DB-free): they exercise the builder through
//! the REAL [`myelin_events::Consumer`] runtime + the REAL [`myelin_events::DedupLedger`] (contracts
//! 2.4/2.5) — the in-memory edge projection models the §3.2 `edge` table's semantics. The REAL
//! Postgres `INSERT … ON CONFLICT` + the REF-D7 producer-crash / outbox emit-iff-committed half are
//! proven against the live dev stack in `tests/integration_ref_p6_edge_builder.rs` (the `integration`
//! feature).
//!
//! What is proven here:
//! - **CDC 5.4 (consumer side):** the builder consumes `refs.edge.created`/`.removed` (the frozen
//!   subjects) and projects them — there is NO standalone edge-write API; the only way a row lands is
//!   the consumer path.
//! - **Idempotent rebuild:** replaying `refs.edge.created` twice through the runtime upserts ONE row
//!   (the deterministic `edge_id`), and the `consumer_dedup` ledger drops the duplicate delivery.
//! - **Chained mutation across a simulated consumer RESTART:** created → removed → created again,
//!   re-binding the consumer by name (rule 4) with the SAME dedup ledger — asserting
//!   exactly-once-in-effect (the final state is one LIVE edge; no duplicate rows; redeliveries are
//!   deduped).
//! - **Steady-state == cold-rebuild (REF-D4, one code path):** the SAME log replayed into a FRESH
//!   projection rebuilds the byte-identical live-edge set — the handler never branched cold-vs-live.

use myelin_events::{
    consume, Actor, AggregateKey, ConsumerName, ConsumerSpec, CorrelationId, DataRole, DedupLedger,
    Delivered, EventEnvelope, EventId, EventType, Message, Reason, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_refs_service::{edge_id, EdgeProjection, RefsEdgeBuilder, EDGE_BUILDER_SUBJECT_PREFIXES};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}

fn edge_event(id: &str, type_: &str, source: &str, target: &str, rel: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType(type_.into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("p-opaque-1".into()),
            PrincipalKind::Human,
            tenant(),
        )),
        subject: ArtifactRef(source.into()),
        aggregate: AggregateKey(format!("edge:{source}->{target}")),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 1,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload: serde_json::json!({ "source": source, "target": target, "rel": rel, "zookie": "zk-1" }),
    }
}

fn msg(ev: &EventEnvelope) -> Message {
    // The broker subject the message arrives on — a `refs.*` subject the builder whitelists.
    Message { subject: "refs.edge.created".into(), envelope: ev.clone() }
}

/// **The refs-edge-builder is a valid consumer: it binds through the ONE sanctioned entry-point with
/// a `*`-free whitelist (BUS-3/BUS-4).** `consume(...)` admits the edge + typed-lifecycle subject
/// prefixes (never `*`).
#[test]
fn edge_builder_binds_through_the_sanctioned_entrypoint_no_wildcard() {
    let builder = RefsEdgeBuilder::new(EdgeProjection::new());
    let spec = ConsumerSpec::new(
        ConsumerName("refs-edge-builder".into()),
        EDGE_BUILDER_SUBJECT_PREFIXES,
    );
    let consumer = consume(spec, builder, DedupLedger::new());
    assert!(consumer.is_ok(), "the builder binds with a *-free whitelist (one of the reviewed BUS-4 consumers)");
}

/// **CDC 5.4 (consumer side) + idempotent rebuild through the runtime.** Delivering
/// `refs.edge.created` projects the edge; a REDELIVERY (same `event_id`) is deduped by the ledger
/// (the handler does not re-run), and even if it did, the deterministic `edge_id` upsert is one row.
#[test]
fn created_is_consumed_and_redelivery_is_deduped_one_row() {
    let projection = EdgeProjection::new();
    let builder = RefsEdgeBuilder::new(projection.clone());
    let spec = ConsumerSpec::new(ConsumerName("refs-edge-builder".into()), EDGE_BUILDER_SUBJECT_PREFIXES);
    let consumer = consume(spec, builder, DedupLedger::new()).expect("bind the builder");

    let src = "myelin://acme/chat/message/m1";
    let tgt = "myelin://acme/issue/issue/ENG-1";
    let ev = edge_event("01J-1", "refs.edge.created", src, tgt, "mentions");

    assert_eq!(consumer.deliver(&msg(&ev)), Delivered::Acked, "first delivery projects the edge");
    assert_eq!(consumer.deliver(&msg(&ev)), Delivered::Deduplicated, "redelivery is deduped (0 dup)");
    assert_eq!(projection.live_count(&tenant(), &region()), 1, "exactly one row (idempotent rebuild)");

    let id = edge_id(&tenant(), src, tgt, "mentions");
    assert!(projection.get(&tenant(), &region(), &id).is_some(), "the edge row exists at the deterministic id");
}

/// **Chained mutation across a simulated consumer RESTART — exactly-once-in-effect (the prompt's
/// required chained test).** created → removed → created again, with the consumer "reconnecting"
/// (re-bound by the SAME name + SAME dedup ledger, rule 4) between steps. The broker redelivers
/// already-handled events on reconnect (at-least-once); the ledger absorbs them (0 dup). The final
/// state is ONE LIVE edge — the create→remove→create sequence is applied exactly once in effect.
#[test]
fn chained_created_removed_created_across_restart_is_exactly_once_in_effect() {
    let projection = EdgeProjection::new();
    let ledger = DedupLedger::new();
    let src = "myelin://acme/chat/message/m1#b9";
    let tgt = "myelin://acme/knowledge/page/7c2";
    let id = edge_id(&tenant(), src, tgt, "embeds");

    let created = edge_event("01J-c", "refs.edge.created", src, tgt, "embeds");
    let removed = {
        let mut e = edge_event("01J-r", "refs.edge.removed", src, tgt, "embeds");
        e.payload = serde_json::json!({ "source": src, "target": tgt, "rel": "embeds" });
        e
    };
    let recreated = edge_event("01J-c2", "refs.edge.created", src, tgt, "embeds");

    // ── Connection 1: deliver created, then removed; broker "drops" before re-create. ──
    {
        let builder = RefsEdgeBuilder::new(projection.clone());
        let spec = ConsumerSpec::new(ConsumerName("refs-edge-builder".into()), EDGE_BUILDER_SUBJECT_PREFIXES);
        let c = consume(spec, builder, ledger.clone()).expect("bind");
        assert_eq!(c.deliver(&msg(&created)), Delivered::Acked);
        assert_eq!(projection.live_count(&tenant(), &region()), 1, "created → one live edge");
        assert_eq!(c.deliver(&msg(&removed)), Delivered::Acked);
        assert_eq!(projection.live_count(&tenant(), &region()), 0, "removed → tombstoned, hidden");
        assert!(projection.get(&tenant(), &region(), &id).unwrap().tombstoned);
        // broker drops here.
    }

    // ── Reconnect: SAME name + SAME ledger. The broker REDELIVERS created + removed (at-least-once),
    //    then delivers the new re-create. The redeliveries are deduped; the re-create revives the edge. ──
    let builder = RefsEdgeBuilder::new(projection.clone());
    let spec = ConsumerSpec::new(ConsumerName("refs-edge-builder".into()), EDGE_BUILDER_SUBJECT_PREFIXES);
    let c2 = consume(spec, builder, ledger.clone()).expect("re-bind by name");
    assert_eq!(c2.deliver(&msg(&created)), Delivered::Deduplicated, "redelivered created → 0 dup");
    assert_eq!(c2.deliver(&msg(&removed)), Delivered::Deduplicated, "redelivered removed → 0 dup");
    assert_eq!(c2.deliver(&msg(&recreated)), Delivered::Acked, "the re-create revives the edge");

    // Exactly-once-in-effect: ONE live edge, ONE row (the deterministic edge_id never duplicated).
    assert_eq!(projection.live_count(&tenant(), &region()), 1, "final state: one LIVE edge");
    assert_eq!(projection.total_count(&tenant(), &region()), 1, "no duplicate rows (deterministic edge_id)");
    assert!(!projection.get(&tenant(), &region(), &id).unwrap().tombstoned, "the edge is live again");
}

/// **Steady-state == cold-rebuild (REF-D4): the SAME log replayed into a FRESH projection rebuilds
/// the byte-identical state — ONE code path, no drift.** This is the no-cross-db floor in action:
/// the cold rebuild ingests the SAME events through the SAME `handle`, never reading an owner DB.
#[test]
fn steady_state_equals_cold_rebuild_one_code_path() {
    let src1 = "myelin://acme/chat/message/m1";
    let tgt1 = "myelin://acme/issue/issue/ENG-1";
    let src2 = "myelin://acme/chat/message/m2";
    let tgt2 = "myelin://acme/knowledge/page/7c2";
    let log = vec![
        edge_event("01J-1", "refs.edge.created", src1, tgt1, "mentions"),
        edge_event("01J-2", "refs.edge.created", src2, tgt2, "embeds"),
        {
            let mut e = edge_event("01J-3", "refs.edge.removed", src1, tgt1, "mentions");
            e.payload = serde_json::json!({ "source": src1, "target": tgt1, "rel": "mentions" });
            e
        },
    ];

    // Steady-state: feed the log live.
    let steady = EdgeProjection::new();
    let sb = RefsEdgeBuilder::new(steady.clone());
    for ev in &log {
        sb.project(ev).expect("steady-state ingest");
    }

    // Cold rebuild: a FRESH projection, the SAME builder code path (project), the SAME log replayed.
    let cold = EdgeProjection::new();
    let cb = RefsEdgeBuilder::new(cold.clone());
    for ev in &log {
        cb.project(ev).expect("cold-rebuild ingest");
    }

    // Byte-parity on the observable state: same live count, same total count, same per-edge rows.
    assert_eq!(
        steady.live_count(&tenant(), &region()),
        cold.live_count(&tenant(), &region()),
        "cold rebuild reproduces the live-edge set"
    );
    assert_eq!(steady.live_count(&tenant(), &region()), 1, "one live edge (m2→7c2; m1→ENG-1 removed)");
    assert_eq!(
        steady.total_count(&tenant(), &region()),
        cold.total_count(&tenant(), &region()),
        "cold rebuild reproduces every row (incl. tombstones)"
    );
    let id2 = edge_id(&tenant(), src2, tgt2, "embeds");
    assert_eq!(
        steady.get(&tenant(), &region(), &id2),
        cold.get(&tenant(), &region(), &id2),
        "the rebuilt edge row is byte-identical (REF-D4 parity)"
    );
    let id1 = edge_id(&tenant(), src1, tgt1, "mentions");
    assert_eq!(
        steady.get(&tenant(), &region(), &id1),
        cold.get(&tenant(), &region(), &id1),
        "the tombstoned row is byte-identical too"
    );
}

/// **A malformed edge event poisons (non-retryable) through the runtime — surfaced, not silently
/// dropped (fail-closed).** The runtime dead-letters it; the index is never corrupted.
#[test]
fn malformed_edge_event_dead_letters_through_the_runtime() {
    let projection = EdgeProjection::new();
    let builder = RefsEdgeBuilder::new(projection.clone());
    let spec = ConsumerSpec::new(ConsumerName("refs-edge-builder".into()), EDGE_BUILDER_SUBJECT_PREFIXES);
    let consumer = consume(spec, builder, DedupLedger::new()).expect("bind");

    let mut bad = edge_event("01J-bad", "refs.edge.created", "s", "t", "mentions");
    bad.payload = serde_json::json!({ "target": "t", "rel": "mentions" }); // no source.
    match consumer.deliver(&msg(&bad)) {
        Delivered::DeadLettered(Reason(r)) => assert!(r.contains("source"), "the poison names the field: {r}"),
        other => panic!("a malformed edge event must dead-letter, got {other:?}"),
    }
    assert_eq!(projection.total_count(&tenant(), &region()), 0, "the index is never corrupted");
}

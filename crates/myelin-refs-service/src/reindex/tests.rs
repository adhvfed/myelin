use super::*;

use myelin_events::{Actor, CorrelationId, EventId, OutboxStore, Timestamp};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn principal() -> Principal {
    Principal::stub(
        PrincipalId("platform".into()),
        PrincipalKind::Service,
        tenant(),
    )
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(principal()),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
        caused_by: None,
    }
}

fn source_edge(agg: &str, version: u64, source: &str, target: &str, rel: &str) -> SourceEdge {
    SourceEdge {
        aggregate: agg.into(),
        version,
        source: ArtifactRef(source.into()),
        target: ArtifactRef(target.into()),
        rel: rel.into(),
        origin_actor: "p-opaque-1".into(),
        zookie: Some("zk-1".into()),
    }
}

fn live_edge_event(
    id: &str,
    source: &str,
    target: &str,
    rel: &str,
) -> myelin_events::EventEnvelope {
    use myelin_events::{AggregateKey, DataRole, EventType, Visibility};
    myelin_events::EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("refs.edge.created".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("p-opaque-1".into()),
            PrincipalKind::Human,
            tenant(),
        )),
        subject: ArtifactRef(source.into()),
        aggregate: AggregateKey(format!("refs.edge:{source}->{target}")),
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

fn scope() -> SnapshotScope {
    SnapshotScope::new(REFS_OWNER_TOKEN, "edge:all")
}

#[test]
fn refs_source_replays_source_of_truth_as_snapshot_drafts() {
    let mut src = RefsReindexSource::new();
    src.record(source_edge("refs.edge:a", 1, "s1", "t1", "mentions"));
    src.record(source_edge("refs.edge:b", 1, "s2", "t2", "embeds"));
    assert_eq!(src.owner_token(), "refs");

    let drafts = src.replay(&scope(), None);
    assert_eq!(drafts.len(), 2, "the whole scope replays (since=None)");
    assert_eq!(drafts[0].type_.0, REFS_EDGE_SNAPSHOT_TYPE);
    assert_eq!(
        drafts[0].payload.get("source").and_then(|v| v.as_str()),
        Some("s1")
    );
    assert_eq!(
        drafts[0].payload.get("rel").and_then(|v| v.as_str()),
        Some("mentions")
    );
}

#[test]
fn refs_source_since_cursor_replays_only_newer_versions() {
    let mut src = RefsReindexSource::new();
    src.record(source_edge("refs.edge:a", 1, "s1", "t1", "mentions"));
    src.record(source_edge("refs.edge:b", 5, "s2", "t2", "embeds"));
    let drafts = src.replay(&scope(), Some(3));
    assert_eq!(
        drafts.len(),
        1,
        "only the version-5 edge replays past since=3"
    );
    assert_eq!(drafts[0].version, 5);
}

#[test]
fn erased_aggregate_is_not_re_snapshotted_x7() {
    let mut src = RefsReindexSource::new();
    src.record(source_edge("refs.edge:a", 1, "s1", "t1", "mentions"));
    src.record(source_edge("refs.edge:b", 1, "s2", "t2", "embeds"));
    assert!(
        src.erase("refs.edge:a"),
        "erase reports it removed the aggregate"
    );
    assert!(
        !src.erase("refs.edge:a"),
        "erasing an absent aggregate is an idempotent no-op"
    );

    let drafts = src.replay(&scope(), None);
    assert_eq!(drafts.len(), 1, "the erased aggregate is NOT replayed");
    assert_eq!(drafts[0].aggregate.0, "refs.edge:b");
}

#[test]
fn reindex_from_source_rebuilds_byte_parity_cold_equals_live() {
    let live_builder = RefsEdgeBuilder::new(EdgeProjection::new());
    live_builder.handle(&live_edge_event("01J-1", "s1", "t1", "mentions"), &mut myelin_events::HandlerTx::none());
    live_builder.handle(&live_edge_event("01J-2", "s2", "t2", "embeds"), &mut myelin_events::HandlerTx::none());
    live_builder.handle(&live_edge_event(
        "01J-3",
        "s3#block-9",
        "t3#block-3",
        "embeds",
    ), &mut myelin_events::HandlerTx::none());
    let live = live_builder.projection().clone();
    assert_eq!(
        live.live_count(&tenant(), &region()),
        3,
        "the live index holds 3 edges"
    );

    let reindexer = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));
    reindexer
        .builder()
        .handle(&live_edge_event("stale-1", "GONE", "GONE2", "links"), &mut myelin_events::HandlerTx::none());
    assert_eq!(
        reindexer.projection().live_count(&tenant(), &region()),
        1,
        "stale pre-state"
    );

    let mut src = RefsReindexSource::new();
    src.record(source_edge("refs.edge:1", 1, "s1", "t1", "mentions"));
    src.record(source_edge("refs.edge:2", 1, "s2", "t2", "embeds"));
    src.record(source_edge(
        "refs.edge:3",
        1,
        "s3#block-9",
        "t3#block-3",
        "embeds",
    ));

    let mut outbox = OutboxStore::new();
    let receipt = reindexer
        .reindex(&scope(), None, &src, &mut outbox, ctx_base())
        .expect("reindex succeeds");
    assert_eq!(receipt.snapshots_emitted, 3, "3 snapshots emitted");
    assert_eq!(
        receipt.ingested, 3,
        "3 snapshots ingested through the live consumer"
    );

    assert!(
        reindexer.verify_parity(&live, &tenant(), &region()),
        "the rebuilt index byte-matches the live index (cold == live)"
    );
    assert_eq!(
        receipt.parity_hash,
        live.parity_hash(&tenant(), &region()),
        "the parity HASH matches"
    );
    assert_eq!(
        reindexer.reindex_parity(),
        1,
        "the {} telemetry reads 1 (recovery succeeded)",
        RefsReindexer::REINDEX_PARITY_SIGNAL,
    );
    assert_eq!(
        reindexer.projection().live_count(&tenant(), &region()),
        3,
        "exactly the live set"
    );
}

#[test]
fn reindex_rerun_emits_zero_new_and_stays_byte_parity() {
    let live_builder = RefsEdgeBuilder::new(EdgeProjection::new());
    live_builder.handle(&live_edge_event("01J-1", "s1", "t1", "mentions"), &mut myelin_events::HandlerTx::none());
    let live = live_builder.projection().clone();

    let reindexer = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));
    let mut src = RefsReindexSource::new();
    src.record(source_edge("refs.edge:1", 1, "s1", "t1", "mentions"));
    let mut outbox = OutboxStore::new();

    let r1 = reindexer
        .reindex(&scope(), None, &src, &mut outbox, ctx_base())
        .expect("first reindex");
    assert_eq!(r1.snapshots_emitted, 1);
    assert_eq!(r1.snapshots_skipped_duplicate, 0);

    let r2 = reindexer
        .reindex(&scope(), None, &src, &mut outbox, ctx_base())
        .expect("second reindex");
    assert_eq!(
        r2.snapshots_emitted, 0,
        "a re-run emits 0 NEW (idempotent on the deterministic id)"
    );
    assert_eq!(r2.snapshots_skipped_duplicate, 1);
    assert!(
        reindexer.verify_parity(&live, &tenant(), &region()),
        "still byte-parity after the re-run"
    );
}

#[test]
fn reindex_parity_telemetry_is_zero_on_drift() {
    let live_builder = RefsEdgeBuilder::new(EdgeProjection::new());
    live_builder.handle(&live_edge_event("01J-1", "s1", "t1", "mentions"), &mut myelin_events::HandlerTx::none());
    live_builder.handle(&live_edge_event("01J-2", "s2", "t2", "embeds"), &mut myelin_events::HandlerTx::none());
    let live = live_builder.projection().clone();

    let reindexer = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));
    let mut src = RefsReindexSource::new();
    src.record(source_edge("refs.edge:1", 1, "s1", "t1", "mentions"));
    let mut outbox = OutboxStore::new();
    reindexer
        .reindex(&scope(), None, &src, &mut outbox, ctx_base())
        .expect("reindex");

    assert!(
        !reindexer.verify_parity(&live, &tenant(), &region()),
        "the rebuild DRIFTED"
    );
    assert_eq!(
        reindexer.reindex_parity(),
        0,
        "the telemetry reads 0 (failed recovery - LOUD)"
    );
}

#[test]
fn malformed_snapshot_fails_the_rebuild_loudly() {
    let reindexer = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));

    let mut malformed = live_edge_event("01J-bad", "s", "t", "mentions");
    malformed.type_ = myelin_events::EventType("refs.edge.snapshot".into());
    malformed.payload = serde_json::json!({ "target": "t", "rel": "mentions" });
    match reindexer.builder().handle(&malformed, &mut myelin_events::HandlerTx::none()) {
        myelin_events::HandleOutcome::NonRetryable(myelin_events::Reason(r)) => {
            assert!(
                r.contains("source"),
                "the poison names the missing field: {r}"
            );
        }
        other => panic!("a malformed snapshot must be a non-retryable poison, got {other:?}"),
    }
}

#[test]
fn te7_drift_reconverges_to_typed_table_typed_wins() {
    let reindexer = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));
    let proj = reindexer.projection();

    let spurious = crate::mirror::SyntheticTypedEvent {
        source: ArtifactRef("myelin://acme/issue/issue/ENG-9".into()),
        target: ArtifactRef("myelin://acme/issue/issue/ENG-2".into()),
        rel: crate::mirror::LifecycleRel::Blocks,
        origin_event: "drift".into(),
        origin_actor: "p-opaque-1".into(),
        zookie: None,
    };
    crate::mirror::project_typed_event(proj, &tenant(), &region(), &spurious).expect("seed drift");

    let typed_truth = vec![crate::mirror::SyntheticTypedEvent {
        source: ArtifactRef("myelin://acme/issue/issue/ENG-1".into()),
        target: ArtifactRef("myelin://acme/issue/issue/ENG-2".into()),
        rel: crate::mirror::LifecycleRel::Blocks,
        origin_event: "typed".into(),
        origin_actor: "p-opaque-1".into(),
        zookie: None,
    }];
    let covered = vec![ArtifactRef("myelin://acme/issue/issue/ENG-2".into())];

    let (reprojected, tombstoned) = reindexer
        .reconverge_typed(&tenant(), &region(), &typed_truth, &covered, "reindex-1")
        .expect("reconverge");
    assert_eq!(
        reprojected, 2,
        "the typed event projects forward + inverse (blocks + blocked_by)"
    );
    assert_eq!(
        tombstoned, 1,
        "the spurious drift edge is tombstoned (typed wins)"
    );

    let inbound = proj.inbound_live(&tenant(), &region(), &covered[0]);
    assert_eq!(
        inbound.len(),
        1,
        "exactly the typed-backed inbound edge survives"
    );
    assert_eq!(
        inbound[0].source.0, "myelin://acme/issue/issue/ENG-1",
        "the typed source wins"
    );
}

#[test]
fn reconverge_leaves_reference_class_edges_untouched() {
    let reindexer = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));
    reindexer.builder().handle(&live_edge_event(
        "01J-ref",
        "myelin://acme/chat/message/m1",
        "myelin://acme/issue/issue/ENG-2",
        "mentions",
    ), &mut myelin_events::HandlerTx::none());
    let covered = vec![ArtifactRef("myelin://acme/issue/issue/ENG-2".into())];

    let (_re, tombstoned) = reindexer
        .reconverge_typed(&tenant(), &region(), &[], &covered, "reindex-2")
        .expect("reconverge");
    assert_eq!(tombstoned, 0, "no lifecycle drift to tombstone");
    assert_eq!(
        reindexer
            .projection()
            .inbound_live(&tenant(), &region(), &covered[0])
            .len(),
        1,
        "the reference-class edge is Refs-authoritative - untouched by reconvergence",
    );
}

#[test]
fn incremental_backfill_extends_does_not_wipe() {
    let reindexer = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));
    reindexer
        .builder()
        .handle(&live_edge_event("01J-existing", "s0", "t0", "links"), &mut myelin_events::HandlerTx::none());
    assert_eq!(reindexer.projection().live_count(&tenant(), &region()), 1);

    let mut src = RefsReindexSource::new();
    src.record(source_edge("refs.edge:new", 5, "s9", "t9", "embeds"));
    let mut outbox = OutboxStore::new();
    reindexer
        .reindex(&scope(), Some(3), &src, &mut outbox, ctx_base())
        .expect("incremental backfill");

    assert_eq!(
        reindexer.projection().live_count(&tenant(), &region()),
        2,
        "a backfill EXTENDS - the existing edge survives",
    );
}

#[test]
fn reindex_parity_signal_is_named() {
    assert_eq!(
        RefsReindexer::REINDEX_PARITY_SIGNAL,
        "refs.reindex_parity",
        "contract-1.8 signal name"
    );
    let r = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));
    assert_eq!(r.reindex_parity(), 1, "a fresh reindexer has not drifted");
}

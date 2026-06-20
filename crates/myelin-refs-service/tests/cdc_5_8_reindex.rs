//! **REF-P16 / P-165 — reindex-from-source (contract 5.8 OWNED `reindex(scope)`; contract 2.6
//! CONSUMED the re-emit + replay) CDC + chained byte-parity + typed-wins tests.**
//!
//! These run on the default `cargo test --workspace` (DB-free). They exercise the Refs `reindex(scope)`
//! surface through the REAL [`myelin_events::reindex`] bus seam + the REAL [`myelin_events::Consumer`]
//! runtime + [`myelin_events::DedupLedger`] (contracts 2.4/2.5) — the in-memory edge projection models
//! the §3.2 `edge` table's semantics. The REAL Postgres reindex (wipe the partition → re-drive the
//! upserts from the replayed snapshots → byte-match the live table) is proven against the live dev
//! stack in `tests/integration_ref_p16_reindex_parity.rs` (the `integration` feature).
//!
//! What is proven here:
//! - **CDC 5.8 (`reindex(scope)`):** the Refs `reindex` drives the §5.6 bus re-emit (contract 2.6) →
//!   the snapshots land in the outbox at their deterministic ids → they ingest through the SAME live
//!   consumer `handle` (cold == live, no owner-DB backdoor) → the rebuilt index byte-matches live.
//! - **The snapshots route through the REAL Consumer runtime (dedup'd):** a `refs.edge.snapshot`
//!   delivered through [`myelin_events::Consumer`] projects the edge; a redelivery is deduped.
//! - **REF-D4 (CI variant) byte-parity + typed-wins:** wipe → reindex → the parity hash matches; a
//!   synthetic TE-7 drift reconverges to the typed table (typed wins, the drifted edge tombstoned).
//! - **X-7:** an erased aggregate is not re-snapshotted (the erasure stays erased across a rebuild).

use myelin_events::{
    consume, snapshot_event_id, Actor, ConsumerName, ConsumerSpec, DedupLedger, Delivered,
    EmitContextBase, EventHandler, Message, OutboxStore, ReindexSource, SnapshotScope, Timestamp,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_refs_service::{
    project_typed_event, EdgeProjection, LifecycleRel, RefsEdgeBuilder, RefsReindexSource,
    RefsReindexer, SourceEdge, SyntheticTypedEvent, EDGE_BUILDER_SUBJECT_PREFIXES,
    REFS_EDGE_SNAPSHOT_TYPE, REFS_OWNER_TOKEN,
};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
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

fn scope() -> SnapshotScope {
    SnapshotScope::new(REFS_OWNER_TOKEN, "edge:all")
}

/// **CDC 5.8: `reindex(scope)` rebuilds the edge index byte-parity through the bus re-emit — ONE code
/// path, no owner-DB backdoor.** Build a live projection; wipe + rebuild a second projection ONLY from
/// the reindex-from-source snapshots; assert the parity hashes match (the §4.7 green artifact) and the
/// `reindex_parity` telemetry reads 1.
#[test]
fn reindex_scope_rebuilds_byte_parity_cold_equals_live() {
    // Live index (steady-state).
    let live = RefsEdgeBuilder::new(EdgeProjection::new());
    let mut truth = RefsReindexSource::new();
    for (i, (s, t, r)) in [
        ("s1", "t1", "mentions"),
        ("s2#b9", "t2#b3", "embeds"),
        ("s3", "t3", "links"),
    ]
    .iter()
    .enumerate()
    {
        // Mirror the live log into the owner's source of truth (cold == live MEANS these are equal).
        truth.record(source_edge(&format!("refs.edge:{i}"), 1, s, t, r));
        // and ingest the live event into the live index.
        live.handle(&snapshot_to_live_event(&truth, i));
    }
    let live_proj = live.projection().clone();
    assert_eq!(live_proj.live_count(&tenant(), &region()), 3);

    // Cold rebuild.
    let reindexer = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));
    let mut outbox = OutboxStore::new();
    let receipt = reindexer
        .reindex(&scope(), None, &truth, &mut outbox, ctx_base())
        .expect("reindex");
    assert_eq!(receipt.snapshots_emitted, 3);
    assert_eq!(receipt.ingested, 3);
    assert!(reindexer.verify_parity(&live_proj, &tenant(), &region()), "byte-parity (cold == live)");
    assert_eq!(receipt.parity_hash, live_proj.parity_hash(&tenant(), &region()));
    assert_eq!(reindexer.reindex_parity(), 1, "the reindex_parity telemetry reads 1");
}

/// Build the live `refs.edge.created` envelope that corresponds to the i-th source edge (the SAME
/// payload the snapshot replay carries — that IS the cold==live invariant).
fn snapshot_to_live_event(truth: &RefsReindexSource, i: usize) -> myelin_events::EventEnvelope {
    let drafts = truth.replay(&scope(), None);
    let d = &drafts[i];
    use myelin_events::{AggregateKey, DataRole, EventId, EventType, Visibility};
    myelin_events::EventEnvelope {
        event_id: EventId(format!("live-{i}")),
        type_: EventType("refs.edge.created".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("p-opaque-1".into()),
            PrincipalKind::Human,
            tenant(),
        )),
        subject: d.subject.clone(),
        aggregate: AggregateKey(d.aggregate.0.clone()),
        causation_id: None,
        correlation_id: myelin_events::CorrelationId(format!("live-{i}")),
        caused_by: None,
        depth: 1,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload: d.payload.clone(),
    }
}

/// **The reindex snapshots route through the REAL Consumer runtime (the live consumer path), dedup'd.**
/// A `refs.edge.snapshot` delivered through [`myelin_events::Consumer`] projects the edge (the builder's
/// `"snapshot"` branch → `apply_created`, cold == live); a redelivery is deduped by the ledger. This is
/// the no-cross-db floor: the ONLY way a row lands on a rebuild is the live consumer path.
#[test]
fn reindex_snapshots_ingest_through_the_real_consumer_runtime_deduped() {
    let mut truth = RefsReindexSource::new();
    truth.record(source_edge("refs.edge:1", 1, "s1", "t1", "mentions"));
    let mut outbox = OutboxStore::new();
    let reindexer = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));
    // (Reindex into a throwaway reindexer just to populate the outbox with the snapshot row.)
    reindexer
        .reindex(&scope(), None, &truth, &mut outbox, ctx_base())
        .expect("reindex emits the snapshot");

    let snap_id = snapshot_event_id(&myelin_events::AggregateKey("refs.edge:1".into()), 1);
    let row = outbox.row(&snap_id).expect("snapshot row in the outbox at its deterministic id");
    assert_eq!(row.envelope.type_.0, REFS_EDGE_SNAPSHOT_TYPE, "the snapshot type");

    // Drive the snapshot envelope through the REAL Consumer runtime over a FRESH projection.
    let projection = EdgeProjection::new();
    let spec = ConsumerSpec::new(
        ConsumerName("refs-edge-builder".into()),
        EDGE_BUILDER_SUBJECT_PREFIXES,
    );
    let consumer = consume(spec, RefsEdgeBuilder::new(projection.clone()), DedupLedger::new())
        .expect("bind the builder");
    let msg = Message { subject: "refs.edge.snapshot".into(), envelope: row.envelope.clone() };

    assert_eq!(consumer.deliver(&msg), Delivered::Acked, "the snapshot projects the edge");
    assert_eq!(consumer.deliver(&msg), Delivered::Deduplicated, "a redelivered snapshot is deduped");
    assert_eq!(projection.live_count(&tenant(), &region()), 1, "exactly one row (idempotent rebuild)");
}

/// **REF-D4 (TE-7 half): a scoped reindex reconverges Refs to the typed table — typed wins.** A
/// spurious lifecycle edge the typed table does not back is tombstoned; the typed snapshot's edges
/// (forward + inverse) become live.
#[test]
fn scoped_reindex_reconverges_te7_drift_typed_wins() {
    let reindexer = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));
    let proj = reindexer.projection();

    // Seed a spurious lifecycle edge (ENG-9 blocks ENG-2) the typed table does not back.
    let spurious = SyntheticTypedEvent {
        source: ArtifactRef("myelin://acme/issue/issue/ENG-9".into()),
        target: ArtifactRef("myelin://acme/issue/issue/ENG-2".into()),
        rel: LifecycleRel::Blocks,
        origin_event: "drift".into(),
        origin_actor: "p-opaque-1".into(),
        zookie: None,
    };
    project_typed_event(proj, &tenant(), &region(), &spurious).expect("seed drift");

    // The authoritative typed snapshot: ENG-1 blocks ENG-2.
    let typed = vec![SyntheticTypedEvent {
        source: ArtifactRef("myelin://acme/issue/issue/ENG-1".into()),
        target: ArtifactRef("myelin://acme/issue/issue/ENG-2".into()),
        rel: LifecycleRel::Blocks,
        origin_event: "typed".into(),
        origin_actor: "p-opaque-1".into(),
        zookie: None,
    }];
    let covered = vec![ArtifactRef("myelin://acme/issue/issue/ENG-2".into())];
    let (reprojected, tombstoned) = reindexer
        .reconverge_typed(&tenant(), &region(), &typed, &covered, "reindex-1")
        .expect("reconverge");
    assert_eq!(reprojected, 2, "forward + inverse (blocks + blocked_by)");
    assert_eq!(tombstoned, 1, "the spurious drift is tombstoned (typed wins)");

    let inbound = proj.inbound_live(&tenant(), &region(), &covered[0]);
    assert_eq!(inbound.len(), 1, "exactly the typed-backed inbound edge survives");
    assert_eq!(inbound[0].source.0, "myelin://acme/issue/issue/ENG-1");
}

/// **X-7: an erased aggregate is NOT re-snapshotted — the erasure stays erased across a reindex.**
#[test]
fn erased_aggregate_stays_erased_across_reindex_x7() {
    let mut truth = RefsReindexSource::new();
    truth.record(source_edge("refs.edge:keep", 1, "s1", "t1", "mentions"));
    truth.record(source_edge("refs.edge:gone", 1, "s2", "t2", "embeds"));
    assert!(truth.erase("refs.edge:gone"), "erase the aggregate from the source of truth");

    let reindexer = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));
    let mut outbox = OutboxStore::new();
    let receipt = reindexer
        .reindex(&scope(), None, &truth, &mut outbox, ctx_base())
        .expect("reindex");
    assert_eq!(receipt.snapshots_emitted, 1, "only the kept aggregate is re-snapshotted");
    assert_eq!(
        reindexer.projection().live_count(&tenant(), &region()),
        1,
        "the erased aggregate does NOT resurrect on a rebuild (X-7)",
    );
}

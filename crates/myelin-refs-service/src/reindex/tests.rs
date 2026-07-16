//! Unit tests for **reindex-from-source: rebuild byte-parity** (REF-P16 / P-165; contract 5.8/2.6).
//! The mutation-tested core: the deterministic-id replay (a re-run emits 0 new), the
//! WIPE-then-rebuild-from-snapshots-only path (no owner-DB backdoor — cold == live, ONE `handle`), the
//! byte-parity verdict (`parity_hash` equality), the TE-7 typed-wins reconvergence (the drifted edge is
//! tombstoned), the X-7 erased-stays-erased discipline, and the `reindex_parity` telemetry (1 on match,
//! 0 on drift). The chained drill + the CDC pair for 5.8 live in `tests/cdc_5_8_reindex.rs`.

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

/// A live edge event (the SAME shape the producer emits live) — used to build the LIVE reference
/// projection the cold rebuild must byte-match.
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

// =====================================================================================================
// The Refs ReindexSource (contract 2.6) — deterministic replay, reads the source of truth, X-7 erasure.
// =====================================================================================================

/// **The Refs source owns the `refs` token + replays its source of truth as `refs.edge.snapshot`
/// drafts.** The drafts carry the SAME `created`-shaped payload a live event carries (cold == live).
#[test]
fn refs_source_replays_source_of_truth_as_snapshot_drafts() {
    let mut src = RefsReindexSource::new();
    src.record(source_edge("refs.edge:a", 1, "s1", "t1", "mentions"));
    src.record(source_edge("refs.edge:b", 1, "s2", "t2", "embeds"));
    assert_eq!(src.owner_token(), "refs");

    let drafts = src.replay(&scope(), None);
    assert_eq!(drafts.len(), 2, "the whole scope replays (since=None)");
    // the snapshot type + the edge payload shape (cold == live).
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

/// **The `since` cursor replays only newer versions (the incremental backfill path).**
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

/// **An ERASED aggregate is dropped from the source of truth → never re-snapshotted (X-7).** The
/// erasure stays erased across a reindex — a mutant that re-snapshots an erased aggregate is caught.
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

// =====================================================================================================
// REF-D4 byte-parity (CI variant) — wipe → reindex → the rebuilt index byte-matches the live index.
// =====================================================================================================

/// **THE byte-parity drill (REF-D4 CI variant, §4.7).** Build a LIVE projection by ingesting the live
/// edge log; build a second projection, WIPE it, then rebuild it ONLY from the reindex-from-source
/// `*.snapshot` replay through the SAME live consumer; assert the rebuilt partition byte-matches the
/// live partition (the parity hash is IDENTICAL — the green artifact). The `reindex_parity` telemetry
/// reads 1 (the recovery succeeded).
#[test]
fn reindex_from_source_rebuilds_byte_parity_cold_equals_live() {
    // ── LIVE projection: ingest the live edge log. ──
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

    // ── COLD rebuild: wipe + rebuild ONLY from the snapshot replay (no owner-DB backdoor). ──
    let reindexer = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));
    // Seed the cold index with STALE rows first, so the WIPE is load-bearing (a mutant that skips the
    // wipe would leave these and break byte-parity).
    reindexer
        .builder()
        .handle(&live_edge_event("stale-1", "GONE", "GONE2", "links"), &mut myelin_events::HandlerTx::none());
    assert_eq!(
        reindexer.projection().live_count(&tenant(), &region()),
        1,
        "stale pre-state"
    );

    // The owner's source of truth (mirrors the live log — that is what cold==live MEANS).
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

    // The rebuilt partition byte-matches the live partition (the §4.7 equality).
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
    // The stale pre-state row is GONE (the wipe was load-bearing).
    assert_eq!(
        reindexer.projection().live_count(&tenant(), &region()),
        3,
        "exactly the live set"
    );
}

/// **A re-run of the reindex emits 0 NEW snapshots (idempotent on the deterministic id).** The second
/// run reports the snapshots as skipped-duplicate; the rebuilt index still byte-matches.
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

/// **The `reindex_parity` telemetry reads 0 on DRIFT (a failed recovery is LOUD + observable).** If the
/// rebuilt partition does NOT match the live one (a missing edge), the verdict is 0 — never a silent
/// partial rebuild. A mutant that inverts the verdict is caught.
#[test]
fn reindex_parity_telemetry_is_zero_on_drift() {
    let live_builder = RefsEdgeBuilder::new(EdgeProjection::new());
    live_builder.handle(&live_edge_event("01J-1", "s1", "t1", "mentions"), &mut myelin_events::HandlerTx::none());
    live_builder.handle(&live_edge_event("01J-2", "s2", "t2", "embeds"), &mut myelin_events::HandlerTx::none());
    let live = live_builder.projection().clone();

    // The owner's truth is MISSING an edge (a corrupt / incomplete source) → the rebuild drifts.
    let reindexer = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));
    let mut src = RefsReindexSource::new();
    src.record(source_edge("refs.edge:1", 1, "s1", "t1", "mentions")); // only ONE of the two.
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
        "the telemetry reads 0 (failed recovery — LOUD)"
    );
}

/// **A structurally-malformed snapshot is a LOUD poison on rebuild (fail-closed), never a silent
/// corruption of the rebuilt index.** The builder's poison surfaces as a [`ReindexError::Poison`].
#[test]
fn malformed_snapshot_fails_the_rebuild_loudly() {
    // A source whose replay yields a snapshot with no `source` field — force it by hand through the
    // outbox (the source-of-truth model always emits well-formed; this tests the rebuild's fail-closed
    // path if a corrupt snapshot ever reaches the consumer).
    let reindexer = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));

    // Build a one-edge source, but POISON the outbox row's payload after emit by re-driving a hand-built
    // malformed snapshot through the builder directly (the rebuild ingest path).
    let mut malformed = live_edge_event("01J-bad", "s", "t", "mentions");
    malformed.type_ = myelin_events::EventType("refs.edge.snapshot".into());
    malformed.payload = serde_json::json!({ "target": "t", "rel": "mentions" }); // no source.
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

// =====================================================================================================
// TE-7 drift reconvergence — the typed table always wins (§3.3/§4.7), driven on the SAME reindex pass.
// =====================================================================================================

/// **A synthetic TE-7 drift reconverges to the typed table — typed wins (REF-D4 TE-7 half).** A
/// spurious lifecycle edge the typed table does NOT back is tombstoned on a scoped reindex; the typed
/// snapshot's edges (forward + inverse) become live. A mutant that lets the drifted edge survive is
/// caught.
#[test]
fn te7_drift_reconverges_to_typed_table_typed_wins() {
    let reindexer = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));
    let proj = reindexer.projection();

    // Seed a SPURIOUS lifecycle edge (a stale projection the typed table does not back): ENG-9 blocks
    // ENG-2 — but the typed truth (below) only has ENG-1 blocks ENG-2.
    let spurious = crate::mirror::SyntheticTypedEvent {
        source: ArtifactRef("myelin://acme/issue/issue/ENG-9".into()),
        target: ArtifactRef("myelin://acme/issue/issue/ENG-2".into()),
        rel: crate::mirror::LifecycleRel::Blocks,
        origin_event: "drift".into(),
        origin_actor: "p-opaque-1".into(),
        zookie: None,
    };
    crate::mirror::project_typed_event(proj, &tenant(), &region(), &spurious).expect("seed drift");

    // The AUTHORITATIVE typed snapshot the reindex re-emits for the scope: ENG-1 blocks ENG-2.
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

    // The live inbound set to ENG-2 is exactly the typed-backed `blocks` edge (ENG-1→ENG-2); the
    // spurious ENG-9→ENG-2 is gone.
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

/// **`reference`-class edges are NEVER touched by the TE-7 reconvergence (they are Refs-authoritative).**
/// Only `lifecycle` drift is tombstoned; a `reference` edge to a covered root survives.
#[test]
fn reconverge_leaves_reference_class_edges_untouched() {
    let reindexer = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));
    // A reference-class edge inbound to ENG-2 (a chat message mentions it).
    reindexer.builder().handle(&live_edge_event(
        "01J-ref",
        "myelin://acme/chat/message/m1",
        "myelin://acme/issue/issue/ENG-2",
        "mentions",
    ), &mut myelin_events::HandlerTx::none());
    let covered = vec![ArtifactRef("myelin://acme/issue/issue/ENG-2".into())];

    // An EMPTY typed snapshot for the scope (no lifecycle edges) — reconverge tombstones lifecycle
    // drift only; the reference edge must survive.
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
        "the reference-class edge is Refs-authoritative — untouched by reconvergence",
    );
}

/// **An incremental backfill (`since = Some`) EXTENDS the index — it does NOT wipe.** The full-rebuild
/// recovery path is `since = None`; the backfill path leaves existing rows. A mutant that wipes on a
/// backfill is caught.
#[test]
fn incremental_backfill_extends_does_not_wipe() {
    let reindexer = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));
    // An existing edge in the index (from steady-state).
    reindexer
        .builder()
        .handle(&live_edge_event("01J-existing", "s0", "t0", "links"), &mut myelin_events::HandlerTx::none());
    assert_eq!(reindexer.projection().live_count(&tenant(), &region()), 1);

    let mut src = RefsReindexSource::new();
    src.record(source_edge("refs.edge:new", 5, "s9", "t9", "embeds")); // a newer-version edge.
    let mut outbox = OutboxStore::new();
    reindexer
        .reindex(&scope(), Some(3), &src, &mut outbox, ctx_base())
        .expect("incremental backfill");

    // BOTH the existing row AND the backfilled one are present (no wipe on a `since` backfill).
    assert_eq!(
        reindexer.projection().live_count(&tenant(), &region()),
        2,
        "a backfill EXTENDS — the existing edge survives",
    );
}

/// **The `reindex_parity` signal NAME is the named constant (drills assert against the name).**
#[test]
fn reindex_parity_signal_is_named() {
    assert_eq!(
        RefsReindexer::REINDEX_PARITY_SIGNAL,
        "refs.reindex_parity",
        "contract-1.8 signal name"
    );
    // a fresh reindexer starts un-drifted (parity = 1).
    let r = RefsReindexer::new(RefsEdgeBuilder::new(EdgeProjection::new()));
    assert_eq!(r.reindex_parity(), 1, "a fresh reindexer has not drifted");
}

//! # CDC: Knowledge's FULL `replay(scope)` reindex-from-source (contract 2.6) — KN-P20 / P-310, M3
//!
//! **Contract:** `contract-index.md` row **2.6** (`events::reindex(scope)` → owner `replay(scope,
//! since)` emits `*.snapshot` via the OUTBOX through the LIVE consumer; sub-artifact-granular). The
//! Bus's reference cold==live (BUS-D5) + the content-crate page/block body (P-246) are pinned by
//! `myelin-content`'s `cdc_2_9_2_6_...`; this CDC pins the SERVICE crate's FULL surface (KN-D6): the
//! `knowledge.row.snapshot` row leg + the `refs.edge.snapshot` TE-7 drift-correction (§3.1) the
//! content crate could not ship, off the real service stores.
//!
//! **The contract pair:** PROVIDER (producer) = `myelin_knowledge::replay::KnowledgeReindexSource`
//! (the owner's `replay`); CONSUMER = `myelin_events::DerivedStore::ingest` (the live consumer path Search/Refs/
//! OLAP read-models are). The pair proves: (a) the rebuild uses the LIVE consumer path ONLY — never
//! an owner-DB read; (b) cold == live byte-for-byte; (c) a re-run is idempotent on the deterministic
//! `(aggregate, version)` id; (d) the `refs.edge.snapshot` token is the FROZEN wire string the Refs
//! reindexer ingests.

use myelin_knowledge::replay::{KnowledgeReindexSource, REFS_EDGE_SNAPSHOT};
use myelin_events::{
    reindex, snapshot_event_id, validate_event_type, Actor, AggregateKey, CorrelationId,
    DerivedStore, EmitContextBase, EventEnvelope, OutboxStore, Region, ReindexSource, SnapshotDraft,
    SnapshotScope, TenantId, Timestamp,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            TenantId("acme".into()),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-22T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-22T00:00:00Z".into()),
        caused_by: None,
    }
}

fn snapshot_envelope(draft: &SnapshotDraft) -> EventEnvelope {
    EventEnvelope {
        event_id: draft.event_id(),
        type_: draft.type_.clone(),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            TenantId("acme".into()),
        )),
        subject: draft.subject.clone(),
        aggregate: draft.aggregate.clone(),
        causation_id: None,
        correlation_id: CorrelationId(draft.event_id().0),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: draft.data_role,
        visibility: draft.visibility,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-22T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-22T00:00:00Z".into()),
        payload: draft.payload.clone(),
    }
}

/// A full Knowledge derived-state surface: a page (with blocks), rows, and TE-7 typed edges.
fn full_source() -> KnowledgeReindexSource {
    let mut s = KnowledgeReindexSource::new();
    s.upsert_page(
        "home",
        4,
        &[
            ("b1", 3, serde_json::json!({ "kind": "heading", "text_ref": "h1" })),
            ("b2", 9, serde_json::json!({ "kind": "paragraph", "text_ref": "p1" })),
        ],
    );
    s.upsert_row("task-1", 2, serde_json::json!({ "title": "Ship KN-P20", "status": "open" }));
    s.upsert_row("task-2", 5, serde_json::json!({ "title": "Wire replay", "status": "done" }));
    s.upsert_edge(
        "myelin://acme/knowledge/page/home",
        "myelin://acme/knowledge/page/space",
        "parent",
        1,
    );
    s.upsert_edge(
        "myelin://acme/knowledge/row/task-1",
        "myelin://acme/knowledge/row/task-2",
        "relates",
        2,
    );
    s
}

/// **CDC 2.6 (a): the rebuild is THROUGH the live consumer path only, and cold == live byte-for-byte
/// over the FULL surface (page+block+row+edge).** The producer's `replay` re-emits `*.snapshot`
/// drafts; the consumer (`DerivedStore::ingest`) materialises them; a wiped store rebuilt ONLY from
/// the reindex outbox re-emit matches the live projection byte-for-byte (the reindex-parity hash).
#[test]
fn cdc_2_6_full_surface_rebuilds_cold_equals_live_via_the_live_consumer_only() {
    let s = full_source();
    let scope = SnapshotScope::new("knowledge", "all");

    // LIVE projection (the consumer ingesting the owner's events).
    let mut live = DerivedStore::new();
    for draft in s.replay(&scope, None) {
        live.ingest(&snapshot_envelope(&draft));
    }

    // COLD: wiped, rebuilt ONLY from the reindex re-emit through the OUTBOX (the live path).
    let sources: &[&dyn ReindexSource] = &[&s];
    let mut outbox = OutboxStore::new();
    reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex");
    let mut cold = DerivedStore::new();
    for draft in s.replay(&scope, None) {
        let row = outbox.row(&draft.event_id()).expect("snapshot row present in the outbox");
        cold.ingest(&row.envelope);
    }

    assert!(live.len() >= 5, "page + 2 blocks + 2 rows + 2 edges materialised");
    assert_eq!(
        cold.parity_bytes(),
        live.parity_bytes(),
        "CDC 2.6: cold == live (the reindex-parity hash matches) — one code path, no drift"
    );
}

/// **CDC 2.6 (b): the re-emit is idempotent on the deterministic `(aggregate, version)` id.** A
/// second reindex of the same scope at the same versions emits 0 NEW snapshots (the outbox
/// `UNIQUE(event_id)` no-ops the duplicate) — a partial reindex that is retried converges.
#[test]
fn cdc_2_6_reindex_rerun_is_idempotent() {
    let s = full_source();
    let scope = SnapshotScope::new("knowledge", "all");
    let sources: &[&dyn ReindexSource] = &[&s];
    let mut outbox = OutboxStore::new();

    let r1 = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("first");
    assert!(r1.snapshots_emitted > 0, "first run emits the snapshots");
    let after_first = outbox.committed_count();

    let r2 = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("re-run");
    assert_eq!(r2.snapshots_emitted, 0, "a re-run emits 0 NEW (idempotent)");
    assert_eq!(r2.snapshots_skipped_duplicate, r1.snapshots_emitted, "all reported as duplicate");
    assert_eq!(outbox.committed_count(), after_first, "no duplicate effect in the outbox");
}

/// **CDC 2.6 (c): the `refs.edge.snapshot` TE-7 drift-correction token is the FROZEN wire string the
/// Refs reindexer ingests + grammatical.** Knowledge produces the SAME `refs.edge.snapshot` token
/// `myelin_refs_service::reindex::REFS_EDGE_SNAPSHOT_TYPE` ingests — it does NOT author a second
/// vocabulary. (The const is mirrored, not depended-on, per the refs_glue `REFS_EDGE_CREATED`
/// precedent; this CDC pins the byte-equivalence.)
#[test]
fn cdc_2_6_refs_edge_snapshot_token_is_the_frozen_wire_string() {
    assert_eq!(REFS_EDGE_SNAPSHOT, "refs.edge.snapshot", "the frozen Refs snapshot wire token");
    assert!(validate_event_type(REFS_EDGE_SNAPSHOT).is_ok(), "grammatical under the Bus §6 grammar");

    // The drift-correction re-emits the typed edges under this exact token.
    let s = full_source();
    let drafts = s.drift_correct_edges(None);
    assert_eq!(drafts.len(), 2, "both TE-7 typed edges re-emitted");
    assert!(
        drafts.iter().all(|d| d.type_.0 == REFS_EDGE_SNAPSHOT),
        "every drift-correction draft carries the frozen token"
    );
}

/// **CDC 2.6 (d): the deterministic snapshot id is a pure function of `(aggregate, version)`** — the
/// idempotency key both the outbox and the consumer dedup off, identical to the Bus seam's.
#[test]
fn cdc_2_6_snapshot_id_is_deterministic_matching_the_bus_seam() {
    let a = AggregateKey("myelin://acme/knowledge/row/task-1".into());
    assert_eq!(snapshot_event_id(&a, 2), snapshot_event_id(&a, 2));
    assert_ne!(snapshot_event_id(&a, 2), snapshot_event_id(&a, 3), "a row edit re-snapshots");
    assert!(snapshot_event_id(&a, 2).0.starts_with("snap-"), "the snapshot id is prefixed");
}

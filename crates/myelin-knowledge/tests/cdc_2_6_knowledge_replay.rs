use myelin_events::{
    reindex, snapshot_event_id, validate_event_type, Actor, AggregateKey, CorrelationId,
    DerivedStore, EmitContextBase, EventEnvelope, OutboxStore, Region, ReindexSource,
    SnapshotDraft, SnapshotScope, TenantId, Timestamp,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_knowledge::replay::{KnowledgeReindexSource, REFS_EDGE_SNAPSHOT};

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-22T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-22T00:00:00Z".into()),
        caused_by: None,
    }
}

fn snapshot_envelope(draft: &SnapshotDraft) -> EventEnvelope {
    let event_id = draft.event_id(&tenant());
    EventEnvelope {
        event_id: event_id.clone(),
        type_: draft.type_.clone(),
        schema_ver: 1,
        tenant: tenant(),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("platform".into()),
            PrincipalKind::Service,
            tenant(),
        )),
        subject: draft.subject.clone(),
        aggregate: draft.aggregate.clone(),
        causation_id: None,
        correlation_id: CorrelationId(event_id.0),
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

fn full_source() -> KnowledgeReindexSource {
    let mut s = KnowledgeReindexSource::new();
    s.upsert_page(
        "home",
        4,
        &[
            (
                "b1",
                3,
                serde_json::json!({ "kind": "heading", "text_ref": "h1" }),
            ),
            (
                "b2",
                9,
                serde_json::json!({ "kind": "paragraph", "text_ref": "p1" }),
            ),
        ],
    );
    s.upsert_row(
        "task-1",
        2,
        serde_json::json!({ "title": "Ship KN-P20", "status": "open" }),
    );
    s.upsert_row(
        "task-2",
        5,
        serde_json::json!({ "title": "Wire replay", "status": "done" }),
    );
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

#[test]
fn cdc_2_6_full_surface_rebuilds_cold_equals_live_via_the_live_consumer_only() {
    let s = full_source();
    let scope = SnapshotScope::new("knowledge", "all");

    let mut live = DerivedStore::new();
    for draft in s.replay(&scope, None) {
        live.ingest(&snapshot_envelope(&draft));
    }

    let sources: &[&dyn ReindexSource] = &[&s];
    let mut outbox = OutboxStore::new();
    reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex");
    let mut cold = DerivedStore::new();
    for draft in s.replay(&scope, None) {
        let row = outbox
            .row(&draft.event_id(&tenant()))
            .expect("snapshot row present in the outbox");
        cold.ingest(&row.envelope);
    }

    assert!(
        live.len() >= 5,
        "page + 2 blocks + 2 rows + 2 edges materialised"
    );
    assert_eq!(
        cold.parity_bytes(),
        live.parity_bytes(),
        "CDC 2.6: cold == live (the reindex-parity hash matches) - one code path, no drift"
    );
}

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
    assert_eq!(
        r2.snapshots_skipped_duplicate, r1.snapshots_emitted,
        "all reported as duplicate"
    );
    assert_eq!(
        outbox.committed_count(),
        after_first,
        "no duplicate effect in the outbox"
    );
}

#[test]
fn cdc_2_6_refs_edge_snapshot_token_is_the_frozen_wire_string() {
    assert_eq!(
        REFS_EDGE_SNAPSHOT, "refs.edge.snapshot",
        "the frozen Refs snapshot wire token"
    );
    assert!(
        validate_event_type(REFS_EDGE_SNAPSHOT).is_ok(),
        "grammatical under the Bus §6 grammar"
    );

    let s = full_source();
    let drafts = s.drift_correct_edges(None);
    assert_eq!(drafts.len(), 2, "both TE-7 typed edges re-emitted");
    assert!(
        drafts.iter().all(|d| d.type_.0 == REFS_EDGE_SNAPSHOT),
        "every drift-correction draft carries the frozen token"
    );
}

#[test]
fn cdc_2_6_snapshot_id_is_deterministic_matching_the_bus_seam() {
    let a = AggregateKey("myelin://acme/knowledge/row/task-1".into());
    assert_eq!(
        snapshot_event_id(&tenant(), &a, 2),
        snapshot_event_id(&tenant(), &a, 2)
    );
    assert_ne!(
        snapshot_event_id(&tenant(), &a, 2),
        snapshot_event_id(&tenant(), &a, 3),
        "a row edit re-snapshots"
    );
    assert!(
        snapshot_event_id(&tenant(), &a, 2).0.starts_with("snap-"),
        "the snapshot id is prefixed"
    );
}

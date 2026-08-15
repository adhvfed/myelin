use myelin_events::{
    reindex, Actor, CorrelationId, DerivedStore, EmitContextBase, EventEnvelope, OutboxStore,
    Region, ReindexSource, SnapshotDraft, SnapshotScope, TenantId, Timestamp,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_knowledge::replay::KnowledgeReindexSource;

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

fn drill_source() -> KnowledgeReindexSource {
    let mut s = KnowledgeReindexSource::new();
    s.upsert_page(
        "runbook",
        6,
        &[
            (
                "b1",
                4,
                serde_json::json!({ "kind": "heading", "text_ref": "h" }),
            ),
            (
                "b2",
                8,
                serde_json::json!({ "kind": "paragraph", "text_ref": "p" }),
            ),
            (
                "b3",
                2,
                serde_json::json!({ "kind": "code", "text_ref": "c" }),
            ),
        ],
    );
    s.upsert_page(
        "notes",
        1,
        &[("n1", 1, serde_json::json!({ "kind": "paragraph" }))],
    );
    s.upsert_row(
        "inc-1",
        3,
        serde_json::json!({ "title": "Incident", "sev": 1 }),
    );
    s.upsert_row(
        "inc-2",
        7,
        serde_json::json!({ "title": "Follow-up", "sev": 3 }),
    );
    s.upsert_edge(
        "myelin://acme/knowledge/page/runbook",
        "myelin://acme/knowledge/page/space",
        "parent",
        1,
    );
    s.upsert_edge(
        "myelin://acme/knowledge/row/inc-1",
        "myelin://acme/knowledge/row/inc-2",
        "relates",
        4,
    );
    s
}

fn build_live(s: &KnowledgeReindexSource, scope: &SnapshotScope) -> DerivedStore {
    let mut live = DerivedStore::new();
    for draft in s.replay(scope, None) {
        live.ingest(&snapshot_envelope(&draft));
    }
    live
}

#[test]
fn kn_d6_wipe_replay_cold_equals_live() {
    let s = drill_source();
    let scope = SnapshotScope::new("knowledge", "all");

    let live = build_live(&s, &scope);

    let mut cold = DerivedStore::new();
    assert!(
        cold.is_empty(),
        "the derived store is wiped before the rebuild"
    );

    let sources: &[&dyn ReindexSource] = &[&s];
    let mut outbox = OutboxStore::new();
    reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex replay");
    for draft in s.replay(&scope, None) {
        let row = outbox
            .row(&draft.event_id(&tenant()))
            .expect("snapshot row present");
        cold.ingest(&row.envelope);
    }

    assert_eq!(cold.len(), live.len(), "the same aggregate count rebuilt");
    assert_eq!(
        cold.parity_bytes(),
        live.parity_bytes(),
        "KN-D6 parity hash: cold == live (byte-identical) - one code path, no drift"
    );
}

#[test]
fn kn_d6_crash_mid_rebuild_then_resume_converges_idempotently() {
    let s = drill_source();
    let scope = SnapshotScope::new("knowledge", "all");
    let live = build_live(&s, &scope);

    let sources: &[&dyn ReindexSource] = &[&s];
    let mut outbox = OutboxStore::new();
    reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex");

    let all_drafts = s.replay(&scope, None);
    let half = all_drafts.len() / 2;

    let mut store = DerivedStore::new();
    for draft in &all_drafts[..half] {
        let row = outbox.row(&draft.event_id(&tenant())).expect("row present");
        store.ingest(&row.envelope);
    }
    let after_crash_len = store.len();
    assert!(
        after_crash_len <= live.len(),
        "a partial rebuild has at most the live count"
    );

    let mut reapplied_no_ops = 0usize;
    for draft in &all_drafts {
        let row = outbox.row(&draft.event_id(&tenant())).expect("row present");
        if !store.ingest(&row.envelope) {
            reapplied_no_ops += 1;
        }
    }
    assert!(
        reapplied_no_ops >= half,
        "the already-applied snapshots are idempotent no-ops on resume"
    );
    assert_eq!(
        store.parity_bytes(),
        live.parity_bytes(),
        "after the crash + resume, the rebuild converges to live (0 double-apply, idempotent)"
    );
}

#[test]
fn kn_d6_rebuild_does_not_resurrect_erased_state() {
    let mut s = drill_source();
    let scope = SnapshotScope::new("knowledge", "all");

    assert!(
        s.erase_page("notes"),
        "the page is erased (its derived state shredded)"
    );
    assert!(s.erase_row("inc-2"), "the row is erased");

    let live = build_live(&s, &scope);

    let sources: &[&dyn ReindexSource] = &[&s];
    let mut outbox = OutboxStore::new();
    reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex");
    let mut cold = DerivedStore::new();
    for draft in s.replay(&scope, None) {
        let row = outbox.row(&draft.event_id(&tenant())).expect("row present");
        cold.ingest(&row.envelope);
    }

    assert_eq!(
        cold.parity_bytes(),
        live.parity_bytes(),
        "cold == live AFTER erasure too"
    );
    let bytes = String::from_utf8_lossy(&cold.parity_bytes()).to_string();
    assert!(
        !bytes.contains("page/notes"),
        "the erased page is not resurrected by the rebuild"
    );
    assert!(
        !bytes.contains("row/inc-2"),
        "the erased row is not resurrected by the rebuild"
    );
    assert!(
        bytes.contains("page/runbook"),
        "the surviving page rebuilds"
    );
    assert!(bytes.contains("row/inc-1"), "the surviving row rebuilds");
}

#[test]
fn kn_d6_te7_drift_correction_typed_table_wins() {
    let s = drill_source();
    let edge_scope = SnapshotScope::new("knowledge", "edges:all");

    let typed = s.drift_correct_edges(None);
    assert_eq!(typed.len(), 2, "two typed edges in the authority");

    let sources: &[&dyn ReindexSource] = &[&s];
    let mut outbox = OutboxStore::new();
    let receipt =
        reindex(&edge_scope, None, sources, &mut outbox, ctx_base()).expect("drift reindex");
    assert_eq!(
        receipt.snapshots_emitted, 2,
        "both typed edges re-emitted as refs.edge.snapshot"
    );

    let mut refs_projection = DerivedStore::new();
    for draft in s.drift_correct_edges(None) {
        let row = outbox
            .row(&draft.event_id(&tenant()))
            .expect("edge snapshot present");
        refs_projection.ingest(&row.envelope);
    }
    assert_eq!(
        refs_projection.len(),
        2,
        "Refs reconverged to exactly the typed edges (typed wins)"
    );

    let r2 = reindex(&edge_scope, None, sources, &mut outbox, ctx_base()).expect("re-run");
    assert_eq!(
        r2.snapshots_emitted, 0,
        "the drift-correction re-run is idempotent"
    );
}

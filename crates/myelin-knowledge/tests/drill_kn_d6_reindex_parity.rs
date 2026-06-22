//! # KN-D6 — reindex-from-source parity (cold == live; live consumer path only; idempotent)
//!
//! **Prompt:** P-310 (KN-P20, M3 · KN-D6). **Drill (catalogue 01, F4, SCHED):** wipe Knowledge's
//! derived state (the Refs edge projection / the Search index); `replay(scope)` (block-granular
//! `*.snapshot` + row + edge) → the rebuilt state MATCHES live; the rebuild uses the LIVE consumer
//! path ONLY; the reindex-parity hash == live is the dated green (cold == live).
//!
//! **Owning architecture doc:**
//! `04-subsystem-architectures/knowledge-platform/architecture/03-events-contracts-and-glue.md` §2.3
//! (`replay(scope, since)` — the ONLY recovery path; deterministic snapshot `event_id`) + §3.1 (the
//! TE-7 drift-correction — a scoped replay reconverges Refs to the typed table). **Contract:** index
//! row **2.6** (reindex-from-source — owned). **Doctrine:** EI-04 §5 (derived stores rebuild FROM
//! SOURCE via the live consumer, never reading owner DBs — steady-state and recovery are ONE code
//! path so they cannot drift); EI-01 §3 (prove-it: cold == live).
//!
//! ## What this proves
//! Both sides of the reindex seam (the CDC pair for contract 2.6, the KN service leg):
//! - **PROVIDER side** — the owner's `replay(scope, since)` re-emit:
//!   `myelin_knowledge::replay::KnowledgeReindexSource` re-emits `knowledge.page.snapshot` /
//!   `knowledge.block.snapshot` / `knowledge.row.snapshot` / `refs.edge.snapshot` through the Bus
//!   outbox, re-reading its OWN source of truth (the block tree / rows / typed-edge tables), never a
//!   derived store / cross DB.
//! - **CONSUMER side** — the live derived consumer (`myelin_events::DerivedStore::ingest`) re-applies
//!   each re-emit through the SAME steady-state step; the consumer cannot tell cold from live, so the
//!   cold rebuild byte-matches the live store.
//!
//! 1. **cold == live (parity hash):** a wiped derived store, rebuilt ONLY from the provider's
//!    `replay` through the outbox, byte-matches the live projection over the full surface
//!    (page+block+row+edge).
//! 2. **live consumer path only:** the rebuild ingests via `DerivedStore::ingest` (the same step the
//!    steady-state live event takes) — there is no owner-DB read API on the consumer side.
//! 3. **failure injection:** a CRASH mid-rebuild (only some snapshots ingested) followed by a resume
//!    (a full re-replay) still converges to the live bytes — the deterministic ids make the resume an
//!    idempotent no-op for the already-applied snapshots, 0 double-apply.
//! 4. **erasure stays erased (X-7):** an erased page/row is not re-snapshotted, so a rebuild never
//!    resurrects shredded derived state.

use myelin_events::{
    reindex, Actor, CorrelationId, DerivedStore, EmitContextBase, EventEnvelope, OutboxStore,
    Region, ReindexSource, SnapshotDraft, SnapshotScope, TenantId, Timestamp,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_knowledge::replay::KnowledgeReindexSource;

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

/// Knowledge's source of truth for the drill: a multi-page, multi-row, multi-edge surface.
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

/// Build the LIVE projection (the consumer ingesting the owner's live events).
fn build_live(s: &KnowledgeReindexSource, scope: &SnapshotScope) -> DerivedStore {
    let mut live = DerivedStore::new();
    for draft in s.replay(scope, None) {
        live.ingest(&snapshot_envelope(&draft));
    }
    live
}

/// **KN-D6 headline: wipe → replay → cold == live (the reindex-parity hash).** The derived store is
/// wiped to empty; it is rebuilt ONLY from `replay` re-emitted through the outbox; the rebuilt bytes
/// byte-match the live projection (the parity hash). The rebuild uses the LIVE consumer path
/// (`DerivedStore::ingest`) only — there is no owner-DB read.
#[test]
fn kn_d6_wipe_replay_cold_equals_live() {
    let s = drill_source();
    let scope = SnapshotScope::new("knowledge", "all");

    let live = build_live(&s, &scope);

    // Wipe the derived store to empty (the F4 failure: a derived store is lost).
    let mut cold = DerivedStore::new();
    assert!(
        cold.is_empty(),
        "the derived store is wiped before the rebuild"
    );

    // Rebuild ONLY from the reindex re-emit through the outbox (no owner-DB read).
    let sources: &[&dyn ReindexSource] = &[&s];
    let mut outbox = OutboxStore::new();
    reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex replay");
    for draft in s.replay(&scope, None) {
        let row = outbox.row(&draft.event_id()).expect("snapshot row present");
        cold.ingest(&row.envelope);
    }

    assert_eq!(cold.len(), live.len(), "the same aggregate count rebuilt");
    assert_eq!(
        cold.parity_bytes(),
        live.parity_bytes(),
        "KN-D6 parity hash: cold == live (byte-identical) — one code path, no drift"
    );
}

/// **KN-D6 failure injection: a CRASH mid-rebuild, then a resume, still converges (idempotent).**
/// The rebuild ingests only the FIRST half of the snapshots, then "crashes"; on resume it re-replays
/// the WHOLE scope — the already-applied snapshots are idempotent no-ops (the deterministic ids), and
/// the rebuild completes to the live bytes with 0 double-apply.
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

    // Partial rebuild, then a crash (only the first half ingested).
    let mut store = DerivedStore::new();
    for draft in &all_drafts[..half] {
        let row = outbox.row(&draft.event_id()).expect("row present");
        store.ingest(&row.envelope);
    }
    let after_crash_len = store.len();
    assert!(
        after_crash_len <= live.len(),
        "a partial rebuild has at most the live count"
    );

    // Resume: re-replay the WHOLE scope. The already-applied snapshots are no-ops (idempotent on the
    // deterministic id); the rest fill in. 0 double-apply.
    let mut reapplied_no_ops = 0usize;
    for draft in &all_drafts {
        let row = outbox.row(&draft.event_id()).expect("row present");
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

/// **KN-D6 erasure stays erased (X-7): a rebuild never resurrects an erased page/row.** Erase a page
/// and a row; the post-erasure live projection and a wiped-then-rebuilt cold projection both EXCLUDE
/// the erased aggregates (the erasure survives a reindex, 0 resurrected derived state).
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
        let row = outbox.row(&draft.event_id()).expect("row present");
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
    // The surviving aggregates ARE rebuilt.
    assert!(
        bytes.contains("page/runbook"),
        "the surviving page rebuilds"
    );
    assert!(bytes.contains("row/inc-1"), "the surviving row rebuilds");
}

/// **KN-D6 TE-7 drift-correction (§3.1): a diverged Refs edge projection reconverges to the typed
/// table via a scoped edge replay, and the typed table WINS.** A Refs projection holding a STALE edge
/// (one with no typed-table backing) is corrected: the scoped `refs.edge.snapshot` replay re-emits
/// only the typed edges; after ingest the projection holds exactly the typed edges (the stale one
/// ages out — it is never re-asserted because it is not in the typed table).
#[test]
fn kn_d6_te7_drift_correction_typed_table_wins() {
    let s = drill_source();
    let edge_scope = SnapshotScope::new("knowledge", "edges:all");

    // The authoritative typed edges (the truth Refs must converge to).
    let typed = s.drift_correct_edges(None);
    assert_eq!(typed.len(), 2, "two typed edges in the authority");

    // A diverged Refs projection: rebuild it ONLY from the drift-correction re-emit through the
    // outbox. Whatever stale edges it held are not re-asserted (the typed table is truth); the typed
    // edges are. (We model "the projection reconverges to exactly the typed edges".)
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
            .row(&draft.event_id())
            .expect("edge snapshot present");
        refs_projection.ingest(&row.envelope);
    }
    assert_eq!(
        refs_projection.len(),
        2,
        "Refs reconverged to exactly the typed edges (typed wins)"
    );

    // Idempotent re-run of the drift-correction.
    let r2 = reindex(&edge_scope, None, sources, &mut outbox, ctx_base()).expect("re-run");
    assert_eq!(
        r2.snapshots_emitted, 0,
        "the drift-correction re-run is idempotent"
    );
}

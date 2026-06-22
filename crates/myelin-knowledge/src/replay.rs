//! # `replay` — Knowledge's FULL reindex-from-source `replay(scope)` body (KN-P20 / P-310, M3 · KN-D6)
//!
//! **Owning architecture doc:**
//! `04-subsystem-architectures/knowledge-platform/architecture/03-events-contracts-and-glue.md`
//! §2.3 (`replay(scope, since)` — the ONLY recovery path; deterministic snapshot `event_id` from
//! `(aggregate, version)`) + §3.1 (the TE-7 drift-correction — a scoped replay reconverges Refs to
//! the typed table; the typed table always wins). **Contract:** index row **2.6**
//! (`events::reindex(scope)` → owner `replay(scope, since)` emits `*.snapshot` via the OUTBOX through
//! the live consumer; sub-artifact-granular — **KN page-subtree at BLOCK granularity** + rows +
//! edges). Doctrine: `external-insights/04-hard-problems.md` §5 (the derived stores — Search/Refs —
//! are rebuilt via the LIVE consumer path, never by reading the owner DB; steady-state and recovery
//! are ONE code path so they cannot drift) + `external-insights/01-process-and-quality-doctrine.md`
//! §3 (prove-it: cold == live).
//!
//! ## Why this exists ALONGSIDE `myelin_content::replay` (EI-01 §7 reconciliation)
//! The Bus's reindex seam ([`myelin_events::reindex`]) named the per-owner `replay` bodies as EB-26
//! (P-246, the `BUS-D5` reference cold==live proof). That floor lives in `myelin_content::replay`
//! and covers the **page + block** snapshots only, off an in-memory `BTreeMap` truth — it is the
//! taxonomy crate's reference body. **This module is the SERVICE crate's body (KN-D6, P-310):** it
//! adds the two derived-store legs the content crate could not (it has no `database`/`refs_glue`):
//! `knowledge.row.snapshot` (off the real [`crate::database`] rows) and `refs.edge.snapshot` (off the
//! real TE-7 typed tables — [`crate::block_tree::PageTree`] `page_parent` + [`crate::database::RelationStore`]
//! `db_relation`), **plus the TE-7 drift-correction** (§3.1). It reads Knowledge's OWN source of
//! truth (its block tree / rows / typed-edge tables), NEVER a derived index, so the cold rebuild and
//! the live ingest are one code path. The content-crate body is NOT duplicated here — page/block
//! snapshots use the SAME [`myelin_content::events`] tokens; this is the full-coverage superset the
//! KN-D6 drill (vs BUS-D5) proves.
//!
//! ## Sub-artifact granularity (contract 2.6)
//! A `page:<id>` scope replays the page's whole block subtree as one `knowledge.block.snapshot` per
//! block (plus the page-level `knowledge.page.snapshot`); a `db:<id>` scope replays one
//! `knowledge.row.snapshot` per row; an `edges:<scope>` (or any scope) re-emits the typed edges as
//! `refs.edge.snapshot` so Refs re-derives its projection at edge granularity. The
//! deterministic snapshot `event_id` (from `(aggregate, version)`) makes a re-run an idempotent no-op
//! (the outbox `UNIQUE(event_id)` + the consumer `consumer_dedup` ledger both absorb the duplicate).
//!
//! ## An erased page/row/edge is SKIPPED (X-7)
//! A tombstoned page (or its block subtree), an erased row, or a removed edge is NOT re-snapshotted —
//! the erasure stays erased across a reindex (the structural-floor erasure of KN-D4 is never undone
//! by a rebuild). The in-memory truth here models that by simply not holding an erased aggregate.

use std::collections::BTreeMap;

use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventType, ReindexSource, SnapshotDraft, SnapshotScope,
    Visibility,
};

use myelin_content::events::{
    KNOWLEDGE_BLOCK_SNAPSHOT, KNOWLEDGE_PAGE_SNAPSHOT, KNOWLEDGE_ROW_SNAPSHOT,
};

/// **The frozen `refs.edge.snapshot` event type (contract 2.6 / Refs §4.7 — the TE-7
/// drift-correction re-emit).** A scoped replay re-emits Knowledge's TYPED-table edges (the source
/// of truth, §3.1) under this token so the Refs edge projection reconverges to the typed table. The
/// string is **byte-identical** to `myelin_refs_service::reindex::REFS_EDGE_SNAPSHOT_TYPE` — the ONE
/// frozen wire token (the CDC pins the equivalence; Knowledge does not author a second vocabulary,
/// and does NOT take a `myelin-refs-service` dependency to read a string — the same const-mirror
/// discipline as [`crate::refs_glue::REFS_EDGE_CREATED`]).
pub const REFS_EDGE_SNAPSHOT: &str = "refs.edge.snapshot";

/// The `rel_class = 'lifecycle'` token a TE-7 typed-edge snapshot carries (byte-identical to
/// [`crate::refs_glue::REL_CLASS_LIFECYCLE`]). The TE-7 drift-correction re-emits lifecycle edges
/// (the `page_parent` `parent` + the `db_relation` `relates`/`rollup_source`) — never the
/// `reference`-class content edges (those re-derive from the block snapshots).
const REL_CLASS_LIFECYCLE: &str = "lifecycle";

// =================================================================================================
// The owner's SOURCE OF TRUTH, as the replay reads it (fed from the live block-tree / row / typed
// tables — never a derived index). Each is the minimal `(aggregate, version, payload)` shape the
// derived store ingests; the live store swaps in behind the SAME `replay` signature.
// =================================================================================================

/// One block in a page subtree: its aggregate (`<page>#block-<id>`), version, and the live payload
/// the index reads (references-not-payloads — structural ids + the rendered-text ref, never the raw
/// body bytes, which live behind the page's per-subject DEK).
#[derive(Clone, Debug, PartialEq, Eq)]
struct BlockTruth {
    aggregate: String,
    version: u64,
    payload: serde_json::Value,
}

/// One page in Knowledge's source of truth: its page-level version + its ordered block subtree.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PageTruth {
    page_aggregate: String,
    page_version: u64,
    /// The block subtree, in document order (a `BTreeMap` so replay order is deterministic).
    blocks: BTreeMap<String, BlockTruth>,
}

/// One row in a flexible database: its aggregate (`row/<id>`), version (the `DbRow.version` CAS
/// token), and the live props payload (the JSONB Search indexes — references-not-payloads).
#[derive(Clone, Debug, PartialEq, Eq)]
struct RowTruth {
    aggregate: String,
    version: u64,
    payload: serde_json::Value,
}

/// One TE-7 typed edge (the `page_parent` `parent` or the `db_relation` `relates`/`rollup_source`)
/// — the SOURCE OF TRUTH the Refs projection mirrors (§3.1). The version is the edge's revision (a
/// re-parent / re-relate bumps it) so a drift-correcting re-emit of a CHANGED edge lands a fresh
/// deterministic id (and the typed table wins). PII-free.
#[derive(Clone, Debug, PartialEq, Eq)]
struct EdgeTruth {
    /// The shared `edge:<source>-><target>` aggregate (the per-edge ordering partition — the SAME
    /// key [`crate::refs_glue::edge_aggregate_key`] mints, so the typed-edge live event and this
    /// drift-correcting snapshot share an aggregate and converge).
    aggregate: String,
    version: u64,
    source: String,
    target: String,
    rel: String,
    payload: serde_json::Value,
}

/// **Knowledge's FULL [`ReindexSource`] body (KN-P20 / P-310, M3 — KN-D6).** Holds Knowledge's OWN
/// source of truth (pages + block subtrees, flexible-DB rows, and the TE-7 typed edges) and replays
/// a scope at sub-artifact granularity → `knowledge.page.snapshot` + per-block
/// `knowledge.block.snapshot` + per-row `knowledge.row.snapshot` + per-edge `refs.edge.snapshot`. A
/// real wiring reads the live [`crate::block_tree`] / [`crate::database`] / typed tables; this reads
/// its in-memory truth — the SAME shape (the live store swaps in behind this `replay` signature).
#[derive(Debug, Default)]
pub struct KnowledgeReindexSource {
    /// `page-id → PageTruth` (deterministic replay order via the `BTreeMap`).
    pages: BTreeMap<String, PageTruth>,
    /// `row-id → RowTruth`.
    rows: BTreeMap<String, RowTruth>,
    /// `edge-key → EdgeTruth` — the TE-7 typed-edge source of truth.
    edges: BTreeMap<String, EdgeTruth>,
}

impl KnowledgeReindexSource {
    /// A fresh, empty source.
    pub fn new() -> KnowledgeReindexSource {
        KnowledgeReindexSource::default()
    }

    /// Record/update a page's truth: its page-level version + its ordered `(block_id, block_version,
    /// payload)` subtree (the live state a collab session settled to). Each block payload gets a
    /// `version` field stamped so the derived store reads it for LWW.
    pub fn upsert_page(
        &mut self,
        page_id: &str,
        page_version: u64,
        blocks: &[(&str, u64, serde_json::Value)],
    ) {
        let page_aggregate = format!("myelin://acme/knowledge/page/{page_id}");
        let mut subtree = BTreeMap::new();
        for (block_id, version, payload) in blocks {
            let aggregate = format!("{page_aggregate}#block-{block_id}");
            subtree.insert(
                (*block_id).to_string(),
                BlockTruth {
                    aggregate,
                    version: *version,
                    payload: stamp_version(payload, *version),
                },
            );
        }
        self.pages.insert(
            page_id.to_string(),
            PageTruth {
                page_aggregate,
                page_version,
                blocks: subtree,
            },
        );
    }

    /// Record/update a flexible-DB row's truth: its [`crate::database::DbRow`] version + its props
    /// JSONB payload (the value Search re-indexes). The `version` field is stamped for LWW.
    pub fn upsert_row(&mut self, row_id: &str, version: u64, props: serde_json::Value) {
        let aggregate = format!("myelin://acme/knowledge/row/{row_id}");
        self.rows.insert(
            row_id.to_string(),
            RowTruth {
                aggregate,
                version,
                payload: stamp_version(&props, version),
            },
        );
    }

    /// **Record/update a TE-7 typed edge (the source of truth Refs mirrors, §3.1).** `source`/`target`
    /// are the artifact URNs; `rel` is a frozen lifecycle token (`parent`/`relates`/`rollup_source`);
    /// `version` is the edge revision (bump it on a re-parent / re-relate so the drift-correcting
    /// re-emit lands a fresh deterministic id). The aggregate is the shared `edge:<src>-><tgt>` key
    /// (the same partition the live `knowledge.page.parent_set` / `knowledge.relation.created` event
    /// uses), so the typed-edge live event and this snapshot converge on one aggregate.
    pub fn upsert_edge(&mut self, source: &str, target: &str, rel: &str, version: u64) {
        let aggregate = format!("edge:{source}->{target}");
        let payload = serde_json::json!({
            "source": source,
            "target": target,
            "rel": rel,
            "rel_class": REL_CLASS_LIFECYCLE,
            "version": version,
        });
        self.edges.insert(
            aggregate.clone(),
            EdgeTruth {
                aggregate,
                version,
                source: source.to_string(),
                target: target.to_string(),
                rel: rel.to_string(),
                payload,
            },
        );
    }

    /// Mark a page erased (a tombstone) — it is REMOVED from the truth, so a subsequent replay SKIPS
    /// it AND its whole block subtree AND its outbound TE-7 typed edges (e.g. its `page_parent`),
    /// so the erasure stays erased across a reindex (X-7). The Refs projection's inbound edges to the
    /// erased page tombstone via the `*.erased` consumer (KN-P19), not via replay.
    pub fn erase_page(&mut self, page_id: &str) -> bool {
        let page_urn = format!("myelin://acme/knowledge/page/{page_id}");
        self.edges.retain(|_, e| e.source != page_urn);
        self.pages.remove(page_id).is_some()
    }

    /// Mark a row erased — removed from the truth so a subsequent replay SKIPS it AND its TE-7 typed
    /// edges (both directions of a `db_relation` touching it), so the erasure stays erased across a
    /// reindex (X-7).
    pub fn erase_row(&mut self, row_id: &str) -> bool {
        let row_urn = format!("myelin://acme/knowledge/row/{row_id}");
        self.edges
            .retain(|_, e| e.source != row_urn && e.target != row_urn);
        self.rows.remove(row_id).is_some()
    }

    /// Remove a TE-7 typed edge (an unrelate / re-parent removes the old edge) — a subsequent replay
    /// does NOT re-emit it (the removed edge stays removed; the typed table is truth, §3.1).
    pub fn remove_edge(&mut self, source: &str, target: &str) -> bool {
        self.edges
            .remove(&format!("edge:{source}->{target}"))
            .is_some()
    }

    /// Parse a `page:<id>` (or `page:all`) selector into the page id target. The whole-platform
    /// `all` selector replays EVERY leg, so it maps to the `all` page target too.
    fn page_target(selector: &str) -> Option<&str> {
        if selector == "all" {
            return Some("all");
        }
        selector.strip_prefix("page:")
    }

    /// Parse a `db:<id>` (or `db:all` / `row:all`) row selector. `db:all` (or `row:all`) replays
    /// every row; a `db:<id>` is treated as "all rows" in the in-memory model (the live binding
    /// scopes by the row's parent db).
    fn row_scope(selector: &str) -> bool {
        selector == "db:all"
            || selector == "row:all"
            || selector.starts_with("db:")
            || selector.starts_with("row:")
            || selector == "all"
    }

    /// Whether this scope re-emits the TE-7 typed edges (the drift-correction, §3.1). `edges:<...>`
    /// is an explicit edge-only reconverge; `all` re-emits edges alongside content.
    fn edge_scope(selector: &str) -> bool {
        selector.starts_with("edges:") || selector == "all"
    }

    /// **The TE-7 drift-correction (§3.1) — re-emit JUST the typed edges so Refs reconverges to the
    /// typed table.** A focused `replay` used when the Refs projection is observed to DISAGREE with
    /// Knowledge's `page_parent` / `db_relation` truth: it re-emits every typed edge as a
    /// `refs.edge.snapshot`; Refs ingests idempotently and the typed table WINS (a stale projected
    /// edge with no typed-table backing is not re-emitted, so it ages out / is tombstoned; a typed
    /// edge missing from the projection is re-asserted). Deterministic order + ids → idempotent
    /// re-run. This is `replay(edges:all, since)` exposed as a named verb.
    pub fn drift_correct_edges(&self, since: Option<u64>) -> Vec<SnapshotDraft> {
        self.edges
            .values()
            .filter(|e| since.is_none_or(|s| e.version > s))
            .map(|e| self.edge_draft(e))
            .collect()
    }

    /// Build the `refs.edge.snapshot` draft for one typed edge.
    fn edge_draft(&self, edge: &EdgeTruth) -> SnapshotDraft {
        let _ = (&edge.source, &edge.target, &edge.rel); // carried in the payload (refs-not-payloads)
        SnapshotDraft {
            aggregate: AggregateKey(edge.aggregate.clone()),
            version: edge.version,
            type_: EventType(REFS_EDGE_SNAPSHOT.to_string()),
            subject: ArtifactRef(edge.source.clone()),
            payload: edge.payload.clone(),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
        }
    }
}

/// Stamp a `version` field into a JSON object payload (so the derived store reads it for LWW). A
/// non-object payload is returned unchanged (the version still rides the [`SnapshotDraft`]).
fn stamp_version(payload: &serde_json::Value, version: u64) -> serde_json::Value {
    let mut payload = payload.clone();
    if let serde_json::Value::Object(map) = &mut payload {
        map.insert("version".into(), serde_json::json!(version));
    }
    payload
}

impl ReindexSource for KnowledgeReindexSource {
    fn owner_token(&self) -> &str {
        "knowledge"
    }

    fn replay(&self, scope: &SnapshotScope, since: Option<u64>) -> Vec<SnapshotDraft> {
        let mut drafts = Vec::new();

        // ── page-subtree at BLOCK granularity (contract 2.6) ──────────────────────────────────
        if let Some(target) = Self::page_target(&scope.selector) {
            for (page_id, page) in &self.pages {
                if target != "all" && page_id.as_str() != target {
                    continue;
                }
                if since.is_none_or(|s| page.page_version > s) {
                    drafts.push(SnapshotDraft {
                        aggregate: AggregateKey(page.page_aggregate.clone()),
                        version: page.page_version,
                        type_: EventType(KNOWLEDGE_PAGE_SNAPSHOT.to_string()),
                        subject: ArtifactRef(page.page_aggregate.clone()),
                        payload: serde_json::json!({ "page": page_id, "version": page.page_version }),
                        data_role: DataRole::Controller,
                        visibility: Visibility::Internal,
                    });
                }
                for block in page.blocks.values() {
                    if since.is_none_or(|s| block.version > s) {
                        drafts.push(SnapshotDraft {
                            aggregate: AggregateKey(block.aggregate.clone()),
                            version: block.version,
                            type_: EventType(KNOWLEDGE_BLOCK_SNAPSHOT.to_string()),
                            subject: ArtifactRef(block.aggregate.clone()),
                            payload: block.payload.clone(),
                            data_role: DataRole::Controller,
                            visibility: Visibility::Internal,
                        });
                    }
                }
            }
        }

        // ── flexible-DB ROW snapshots (contract 2.6 — Search re-indexes rows) ─────────────────
        if Self::row_scope(&scope.selector) {
            for row in self.rows.values() {
                if since.is_none_or(|s| row.version > s) {
                    drafts.push(SnapshotDraft {
                        aggregate: AggregateKey(row.aggregate.clone()),
                        version: row.version,
                        type_: EventType(KNOWLEDGE_ROW_SNAPSHOT.to_string()),
                        subject: ArtifactRef(row.aggregate.clone()),
                        payload: row.payload.clone(),
                        data_role: DataRole::Controller,
                        visibility: Visibility::Internal,
                    });
                }
            }
        }

        // ── TE-7 typed-edge drift-correction (§3.1 — Refs reconverges to the typed table) ─────
        if Self::edge_scope(&scope.selector) {
            drafts.extend(self.drift_correct_edges(since));
        }

        drafts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        reindex, snapshot_event_id, validate_event_type, Actor, CorrelationId, DerivedStore,
        EmitContextBase, EventEnvelope, OutboxStore, Region, TenantId, Timestamp,
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

    /// A source with a page (3 blocks), two rows, and two TE-7 typed edges — Knowledge's full
    /// derived-state surface.
    fn source() -> KnowledgeReindexSource {
        let mut s = KnowledgeReindexSource::new();
        s.upsert_page(
            "home",
            5,
            &[
                (
                    "b1",
                    2,
                    serde_json::json!({ "kind": "heading", "text_ref": "r1" }),
                ),
                (
                    "b2",
                    7,
                    serde_json::json!({ "kind": "paragraph", "text_ref": "r2" }),
                ),
                (
                    "b3",
                    1,
                    serde_json::json!({ "kind": "code", "text_ref": "r3" }),
                ),
            ],
        );
        s.upsert_row(
            "row-1",
            3,
            serde_json::json!({ "title": "Task A", "status": "open" }),
        );
        s.upsert_row(
            "row-2",
            1,
            serde_json::json!({ "title": "Task B", "status": "done" }),
        );
        // page_parent (the TE-7 `parent` typed edge) + a db_relation (`relates`).
        s.upsert_edge(
            "myelin://acme/knowledge/page/home",
            "myelin://acme/knowledge/page/root",
            "parent",
            1,
        );
        s.upsert_edge(
            "myelin://acme/knowledge/row/row-1",
            "myelin://acme/knowledge/row/row-2",
            "relates",
            1,
        );
        s
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

    /// **Every snapshot token this replay emits is GRAMMATICAL (the Bus §6 grammar) + the frozen
    /// wire string.** The four owner tokens (page/block/row snapshot + the `refs.edge.snapshot`
    /// drift-correction) parse the one Bus validator and are byte-identical to the frozen constants.
    #[test]
    fn snapshot_tokens_are_grammatical_and_frozen() {
        for t in [
            KNOWLEDGE_PAGE_SNAPSHOT,
            KNOWLEDGE_BLOCK_SNAPSHOT,
            KNOWLEDGE_ROW_SNAPSHOT,
            REFS_EDGE_SNAPSHOT,
        ] {
            assert!(
                validate_event_type(t).is_ok(),
                "`{t}` must be grammatical: {:?}",
                validate_event_type(t)
            );
        }
        assert_eq!(
            REFS_EDGE_SNAPSHOT, "refs.edge.snapshot",
            "the frozen Refs snapshot token"
        );
    }

    /// **`replay(page:home)` is block-granular: page snapshot + one block snapshot per block.**
    #[test]
    fn replay_page_scope_is_block_granular() {
        let s = source();
        let drafts = s.replay(&SnapshotScope::new("knowledge", "page:home"), None);
        let pages = drafts
            .iter()
            .filter(|d| d.type_.0 == KNOWLEDGE_PAGE_SNAPSHOT)
            .count();
        let blocks = drafts
            .iter()
            .filter(|d| d.type_.0 == KNOWLEDGE_BLOCK_SNAPSHOT)
            .count();
        assert_eq!(pages, 1, "one page snapshot");
        assert_eq!(
            blocks, 3,
            "one snapshot per block (block granularity, contract 2.6)"
        );
        assert!(drafts.iter().all(|d| d.aggregate.0.contains("page/home")));
    }

    /// **`replay(db:all)` re-emits one `knowledge.row.snapshot` per flexible-DB row (the row leg the
    /// content-crate body lacks).**
    #[test]
    fn replay_row_scope_emits_one_snapshot_per_row() {
        let s = source();
        let drafts = s.replay(&SnapshotScope::new("knowledge", "db:all"), None);
        let rows: Vec<&str> = drafts
            .iter()
            .filter(|d| d.type_.0 == KNOWLEDGE_ROW_SNAPSHOT)
            .map(|d| d.aggregate.0.as_str())
            .collect();
        assert_eq!(rows.len(), 2, "one snapshot per row");
        assert!(rows.iter().all(|a| a.contains("knowledge/row/")));
    }

    /// **The TE-7 drift-correction (§3.1): a scoped replay re-emits the typed edges as
    /// `refs.edge.snapshot` so Refs reconverges to the typed table (the typed table is truth).**
    #[test]
    fn te7_drift_correction_re_emits_typed_edges() {
        let s = source();
        let drafts = s.drift_correct_edges(None);
        assert_eq!(drafts.len(), 2, "both typed edges re-emitted");
        assert!(drafts.iter().all(|d| d.type_.0 == REFS_EDGE_SNAPSHOT));
        // Each carries the lifecycle rel_class + the typed rel (parent / relates) — the truth Refs
        // mirrors.
        let rels: Vec<&str> = drafts
            .iter()
            .map(|d| d.payload.get("rel").and_then(|v| v.as_str()).unwrap_or(""))
            .collect();
        assert!(rels.contains(&"parent"), "the page_parent typed edge");
        assert!(rels.contains(&"relates"), "the db_relation typed edge");
        assert!(drafts
            .iter()
            .all(|d| d.payload.get("rel_class").and_then(|v| v.as_str())
                == Some(REL_CLASS_LIFECYCLE)));
    }

    /// **A REMOVED typed edge is NOT re-emitted (the typed table wins — a stale projected edge with
    /// no typed-table backing is never re-asserted by the drift-correction, §3.1).**
    #[test]
    fn drift_correction_does_not_resurrect_a_removed_edge() {
        let mut s = source();
        assert!(s.remove_edge(
            "myelin://acme/knowledge/row/row-1",
            "myelin://acme/knowledge/row/row-2",
        ));
        let drafts = s.drift_correct_edges(None);
        assert_eq!(drafts.len(), 1, "only the surviving typed edge re-emits");
        assert!(drafts
            .iter()
            .all(|d| d.payload.get("rel").and_then(|v| v.as_str()) == Some("parent")));
    }

    /// **`since` is the incremental cursor across ALL legs (block/row/edge granular).**
    #[test]
    fn replay_since_cursor_is_sub_artifact_granular() {
        let s = source();
        // since=2: page(v5)✓, b2(v7)✓ but b1(v2)✗ b3(v1)✗; row-1(v3)✓ row-2(v1)✗.
        let drafts = s.replay(&SnapshotScope::new("knowledge", "all"), Some(2));
        let block_vs: Vec<u64> = drafts
            .iter()
            .filter(|d| d.type_.0 == KNOWLEDGE_BLOCK_SNAPSHOT)
            .map(|d| d.version)
            .collect();
        assert_eq!(block_vs, vec![7], "only the block past the cursor replays");
        let row_aggs: Vec<&str> = drafts
            .iter()
            .filter(|d| d.type_.0 == KNOWLEDGE_ROW_SNAPSHOT)
            .map(|d| d.aggregate.0.as_str())
            .collect();
        assert_eq!(
            row_aggs,
            vec!["myelin://acme/knowledge/row/row-1"],
            "only row past the cursor"
        );
    }

    /// An ERASED page / row is SKIPPED — its derived state stays erased across a reindex (X-7).
    #[test]
    fn replay_skips_erased_page_and_row() {
        let mut s = source();
        assert!(s.erase_page("home"));
        assert!(s.erase_row("row-2"));
        let drafts = s.replay(&SnapshotScope::new("knowledge", "all"), None);
        assert!(
            drafts.iter().all(|d| !d.aggregate.0.contains("page/home")),
            "erased page skipped"
        );
        assert!(
            drafts.iter().all(|d| !d.aggregate.0.contains("row/row-2")),
            "erased row skipped"
        );
        assert!(
            drafts.iter().any(|d| d.aggregate.0.contains("row/row-1")),
            "surviving row replays"
        );
    }

    /// **The deterministic snapshot `event_id` from `(aggregate, version)` (contract 2.6).**
    #[test]
    fn snapshot_event_id_is_deterministic() {
        let a = AggregateKey("myelin://acme/knowledge/row/row-1".into());
        assert_eq!(
            snapshot_event_id(&a, 3),
            snapshot_event_id(&a, 3),
            "same inputs → same id"
        );
        assert_ne!(
            snapshot_event_id(&a, 3),
            snapshot_event_id(&a, 4),
            "version bumps the id"
        );
    }

    /// KN-D6 cold == live and idempotent re-run (the headline drill, block/row/edge granular). Build
    /// the LIVE projection by ingesting every snapshot the owner would have emitted live, then wipe
    /// and rebuild a SECOND store ONLY from the reindex replay through the outbox-relay path, and
    /// assert byte-identical (the reindex-parity hash). Then re-run reindex (0 new, the deterministic
    /// ids no-op it). The rebuild uses the LIVE consumer (`DerivedStore::ingest`) path only, never an
    /// owner-DB read.
    #[test]
    fn kn_d6_cold_equals_live_and_idempotent_full_surface() {
        let s = source();
        let scope = SnapshotScope::new("knowledge", "all");

        // LIVE: ingest the same drafts the owner emits live (cold==live is precisely that these are
        // the same shape). The `all` scope covers every leg (page+block+row+edge).
        let mut live = DerivedStore::new();
        for draft in s.replay(&scope, None) {
            live.ingest(&snapshot_envelope(&draft));
        }

        // COLD: wiped; rebuilt ONLY from the reindex re-emit through the outbox→relay path.
        let mut cold = DerivedStore::new();
        let sources: &[&dyn ReindexSource] = &[&s];
        let mut outbox = OutboxStore::new();
        reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("full-surface reindex");
        for draft in s.replay(&scope, None) {
            let row = outbox
                .row(&draft.event_id())
                .expect("snapshot row present (live path only)");
            cold.ingest(&row.envelope);
        }

        assert_eq!(cold.len(), live.len(), "same aggregate count");
        assert_eq!(
            cold.parity_bytes(),
            live.parity_bytes(),
            "KN-D6: cold == live (the reindex-parity hash matches)"
        );

        // Idempotent re-run: every snapshot's deterministic id is already in the outbox → 0 new.
        let r = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("re-reindex");
        assert_eq!(
            r.snapshots_emitted, 0,
            "a re-run emits 0 NEW (idempotent — deterministic ids)"
        );
        assert!(
            r.snapshots_skipped_duplicate > 0,
            "the duplicates are reported, not re-emitted"
        );
    }

    /// **The drift-correction re-emit goes through the LIVE outbox→consumer path + is idempotent.**
    #[test]
    fn drift_correction_reconverges_refs_via_the_live_path_idempotently() {
        let s = source();
        let edge_scope = SnapshotScope::new("knowledge", "edges:all");
        let sources: &[&dyn ReindexSource] = &[&s];
        let mut outbox = OutboxStore::new();

        // A wiped/diverged Refs projection rebuilt ONLY from the typed-edge re-emit.
        let mut refs_projection = DerivedStore::new();
        let r1 =
            reindex(&edge_scope, None, sources, &mut outbox, ctx_base()).expect("drift reindex");
        assert_eq!(r1.snapshots_emitted, 2, "both typed edges re-emitted");
        for draft in s.drift_correct_edges(None) {
            let row = outbox
                .row(&draft.event_id())
                .expect("edge snapshot present");
            assert!(row.envelope.type_.0 == REFS_EDGE_SNAPSHOT);
            refs_projection.ingest(&row.envelope);
        }
        assert_eq!(
            refs_projection.len(),
            2,
            "Refs reconverged to the two typed edges"
        );

        // Re-run → idempotent (0 new).
        let r2 = reindex(&edge_scope, None, sources, &mut outbox, ctx_base()).expect("re-run");
        assert_eq!(
            r2.snapshots_emitted, 0,
            "drift-correction re-run is idempotent"
        );
    }
}

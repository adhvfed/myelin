use std::collections::BTreeMap;

use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventType, ReindexSource, SnapshotDraft, SnapshotScope,
    Visibility,
};

use myelin_content::events::{
    KNOWLEDGE_BLOCK_SNAPSHOT, KNOWLEDGE_PAGE_SNAPSHOT, KNOWLEDGE_ROW_SNAPSHOT,
};

pub const REFS_EDGE_SNAPSHOT: &str = "refs.edge.snapshot";

const REL_CLASS_LIFECYCLE: &str = "lifecycle";

#[derive(Clone, Debug, PartialEq, Eq)]
struct BlockTruth {
    aggregate: String,
    version: u64,
    payload: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PageTruth {
    page_aggregate: String,
    page_version: u64,
    blocks: BTreeMap<String, BlockTruth>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RowTruth {
    aggregate: String,
    version: u64,
    payload: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EdgeTruth {
    aggregate: String,
    version: u64,
    source: String,
    target: String,
    rel: String,
    payload: serde_json::Value,
}

#[derive(Debug, Default)]
pub struct KnowledgeReindexSource {
    pages: BTreeMap<String, PageTruth>,
    rows: BTreeMap<String, RowTruth>,
    edges: BTreeMap<String, EdgeTruth>,
}

impl KnowledgeReindexSource {
    pub fn new() -> KnowledgeReindexSource {
        KnowledgeReindexSource::default()
    }

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

    pub fn erase_page(&mut self, page_id: &str) -> bool {
        let page_urn = format!("myelin://acme/knowledge/page/{page_id}");
        self.edges.retain(|_, e| e.source != page_urn);
        self.pages.remove(page_id).is_some()
    }

    pub fn erase_row(&mut self, row_id: &str) -> bool {
        let row_urn = format!("myelin://acme/knowledge/row/{row_id}");
        self.edges
            .retain(|_, e| e.source != row_urn && e.target != row_urn);
        self.rows.remove(row_id).is_some()
    }

    pub fn remove_edge(&mut self, source: &str, target: &str) -> bool {
        self.edges
            .remove(&format!("edge:{source}->{target}"))
            .is_some()
    }

    fn page_target(selector: &str) -> Option<&str> {
        if selector == "all" {
            return Some("all");
        }
        selector.strip_prefix("page:")
    }

    fn row_scope(selector: &str) -> bool {
        selector == "db:all"
            || selector == "row:all"
            || selector.starts_with("db:")
            || selector.starts_with("row:")
            || selector == "all"
    }

    fn edge_scope(selector: &str) -> bool {
        selector.starts_with("edges:") || selector == "all"
    }

    pub fn drift_correct_edges(&self, since: Option<u64>) -> Vec<SnapshotDraft> {
        self.edges
            .values()
            .filter(|e| since.is_none_or(|s| e.version > s))
            .map(|e| self.edge_draft(e))
            .collect()
    }

    fn edge_draft(&self, edge: &EdgeTruth) -> SnapshotDraft {
        let _ = (&edge.source, &edge.target, &edge.rel);
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

    #[test]
    fn te7_drift_correction_re_emits_typed_edges() {
        let s = source();
        let drafts = s.drift_correct_edges(None);
        assert_eq!(drafts.len(), 2, "both typed edges re-emitted");
        assert!(drafts.iter().all(|d| d.type_.0 == REFS_EDGE_SNAPSHOT));
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

    #[test]
    fn replay_since_cursor_is_sub_artifact_granular() {
        let s = source();
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

    #[test]
    fn kn_d6_cold_equals_live_and_idempotent_full_surface() {
        let s = source();
        let scope = SnapshotScope::new("knowledge", "all");

        let mut live = DerivedStore::new();
        for draft in s.replay(&scope, None) {
            live.ingest(&snapshot_envelope(&draft));
        }

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

        let r = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("re-reindex");
        assert_eq!(
            r.snapshots_emitted, 0,
            "a re-run emits 0 NEW (idempotent - deterministic ids)"
        );
        assert!(
            r.snapshots_skipped_duplicate > 0,
            "the duplicates are reported, not re-emitted"
        );
    }

    #[test]
    fn drift_correction_reconverges_refs_via_the_live_path_idempotently() {
        let s = source();
        let edge_scope = SnapshotScope::new("knowledge", "edges:all");
        let sources: &[&dyn ReindexSource] = &[&s];
        let mut outbox = OutboxStore::new();

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

        let r2 = reindex(&edge_scope, None, sources, &mut outbox, ctx_base()).expect("re-run");
        assert_eq!(
            r2.snapshots_emitted, 0,
            "drift-correction re-run is idempotent"
        );
    }
}

use std::collections::BTreeMap;

use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventType, ReindexSource, SnapshotDraft, SnapshotScope,
    Visibility,
};

use crate::events;

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

#[derive(Debug, Default)]
pub struct KnowledgeReindexSource {
    pages: BTreeMap<String, PageTruth>,
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
            let mut payload = payload.clone();
            if let serde_json::Value::Object(map) = &mut payload {
                map.insert("version".into(), serde_json::json!(version));
            }
            let aggregate = format!("{page_aggregate}#block-{block_id}");
            subtree.insert(
                (*block_id).to_string(),
                BlockTruth {
                    aggregate,
                    version: *version,
                    payload,
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

    pub fn erase_page(&mut self, page_id: &str) -> bool {
        self.pages.remove(page_id).is_some()
    }

    fn page_target(selector: &str) -> Option<&str> {
        selector.strip_prefix("page:")
    }
}

impl ReindexSource for KnowledgeReindexSource {
    fn owner_token(&self) -> &str {
        "knowledge"
    }

    fn replay(&self, scope: &SnapshotScope, since: Option<u64>) -> Vec<SnapshotDraft> {
        let Some(target) = Self::page_target(&scope.selector) else {
            return Vec::new();
        };
        let mut drafts = Vec::new();
        for (page_id, page) in &self.pages {
            if target != "all" && page_id.as_str() != target {
                continue;
            }
            if since.is_none_or(|s| page.page_version > s) {
                drafts.push(SnapshotDraft {
                    aggregate: AggregateKey(page.page_aggregate.clone()),
                    version: page.page_version,
                    type_: EventType(events::KNOWLEDGE_PAGE_SNAPSHOT.to_string()),
                    subject: ArtifactRef(page.page_aggregate.clone()),
                    payload: serde_json::json!({ "page": page_id, "version": page.page_version }),
                    data_role: DataRole::Processor,
                    visibility: Visibility::Internal,
                });
            }
            for block in page.blocks.values() {
                if since.is_none_or(|s| block.version > s) {
                    drafts.push(SnapshotDraft {
                        aggregate: AggregateKey(block.aggregate.clone()),
                        version: block.version,
                        type_: EventType(events::KNOWLEDGE_BLOCK_SNAPSHOT.to_string()),
                        subject: ArtifactRef(block.aggregate.clone()),
                        payload: block.payload.clone(),
                        data_role: DataRole::Processor,
                        visibility: Visibility::Internal,
                    });
                }
            }
        }
        drafts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        reindex, snapshot_event_id, Actor, CorrelationId, DerivedStore, EmitContextBase,
        EventEnvelope, OutboxStore, Region, TenantId, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

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
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:00Z".into()),
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
        s.upsert_page(
            "notes",
            1,
            &[("n1", 1, serde_json::json!({ "kind": "paragraph" }))],
        );
        s
    }

    #[test]
    fn replay_page_scope_emits_one_snapshot_per_block() {
        let s = source();
        let drafts = s.replay(&SnapshotScope::new("knowledge", "page:home"), None);
        assert_eq!(drafts.len(), 4);
        let page = drafts
            .iter()
            .filter(|d| d.type_.0 == "knowledge.page.snapshot")
            .count();
        let blocks = drafts
            .iter()
            .filter(|d| d.type_.0 == "knowledge.block.snapshot")
            .count();
        assert_eq!(page, 1, "one page snapshot");
        assert_eq!(blocks, 3, "one snapshot per block (block granularity)");
        assert!(drafts.iter().all(|d| d.aggregate.0.contains("page/home")));
    }

    #[test]
    fn replay_since_cursor_is_block_granular() {
        let s = source();
        let drafts = s.replay(&SnapshotScope::new("knowledge", "page:home"), Some(3));
        let block_versions: Vec<u64> = drafts
            .iter()
            .filter(|d| d.type_.0 == "knowledge.block.snapshot")
            .map(|d| d.version)
            .collect();
        assert_eq!(
            block_versions,
            vec![7],
            "only the block past the cursor replays"
        );
    }

    #[test]
    fn replay_skips_an_erased_page_and_its_subtree() {
        let mut s = source();
        assert!(s.erase_page("home"));
        let drafts = s.replay(&SnapshotScope::new("knowledge", "page:all"), None);
        assert!(
            drafts.iter().all(|d| !d.aggregate.0.contains("page/home")),
            "the erased page + its blocks are not re-snapshotted"
        );
        assert!(drafts.iter().any(|d| d.aggregate.0.contains("page/notes")));
    }

    #[test]
    fn kn_replay_rebuilds_byte_identically_and_is_idempotent() {
        let s = source();
        let scope = SnapshotScope::new("knowledge", "page:home");

        let mut live = DerivedStore::new();
        for draft in s.replay(&scope, None) {
            live.ingest(&snapshot_envelope(&draft));
        }

        let mut cold = DerivedStore::new();
        let sources: &[&dyn ReindexSource] = &[&s];
        let mut outbox = OutboxStore::new();
        let r1 = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("reindex");
        assert_eq!(r1.snapshots_emitted, 4, "page + 3 blocks");
        for draft in s.replay(&scope, None) {
            let row = outbox
                .row(&draft.event_id(&tenant()))
                .expect("snapshot row present");
            cold.ingest(&row.envelope);
        }
        assert_eq!(
            cold.parity_bytes(),
            live.parity_bytes(),
            "cold == live (byte-identical)"
        );

        let r2 = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("re-reindex");
        assert_eq!(r2.snapshots_emitted, 0, "a re-run emits 0 new (idempotent)");
        assert_eq!(r2.snapshots_skipped_duplicate, 4);
    }

    #[test]
    fn block_snapshot_id_is_deterministic() {
        let a = AggregateKey("myelin://acme/knowledge/page/home#block-b2".into());
        assert_eq!(
            snapshot_event_id(&tenant(), &a, 7),
            snapshot_event_id(&tenant(), &a, 7)
        );
        assert_ne!(
            snapshot_event_id(&tenant(), &a, 7),
            snapshot_event_id(&tenant(), &a, 8)
        );
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
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:00Z".into()),
            payload: draft.payload.clone(),
        }
    }
}

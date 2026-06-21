//! # `replay` — Knowledge's per-owner reindex-from-source `replay` body (EB-26 / P-246, M3)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/event-bus.md` §4.9 (reindex-from-source re-emit),
//! contract-index row **2.6** (`events::reindex(scope)` → owner `replay(scope, since)` emits
//! `*.snapshot`; **sub-artifact-granular — KN page-subtree at BLOCK granularity**). **Floor filled:**
//! the Bus's `myelin_events::reindex` named the per-OWNER `replay` bodies as EB-26 (P-246, M3); this
//! is KNOWLEDGE's.
//!
//! ## What this is — page-subtree at BLOCK granularity
//! Knowledge is an OWNING subsystem of reindex-from-source. When Search's content index (or any
//! derived store) is wiped/bootstrapped, the Bus asks Knowledge to `replay(scope, since)` → the
//! `*.snapshot` drafts it re-emits through the SAME outbox→bus→live-consumer path (no backdoor —
//! EI-04 §5.3). [`KnowledgeReindexSource`] is Knowledge's [`myelin_events::ReindexSource`] body. The
//! KN replay is **sub-artifact-granular at BLOCK granularity** (contract 2.6): a `page:<id>` scope
//! replays the page's whole block subtree as one `knowledge.block.snapshot` per block (plus the
//! page-level `knowledge.page.snapshot`), so Search re-indexes / the `#sub` block-anchor ladder
//! (contract 5.7) re-derives at block granularity — exactly the granularity the live collab op-stream
//! (`knowledge.block.op`, firehose) updates at.
//!
//! Each block's snapshot carries the live block's `(version, payload)`; the deterministic snapshot
//! `event_id` (from `(aggregate, version)`) makes a re-run idempotent (cold == live, BUS-D5). The
//! collab op-stream is the FIREHOSE half (KN-D1 — `resume(scope=doc, last_seq)` loses 0 ops); the
//! durable bus carries only these pointer snapshots, never the per-keystroke ops.
//!
//! ## An erased page/block is SKIPPED (X-7)
//! A tombstoned page (or block) is NOT re-snapshotted — the erasure stays erased across a reindex.

use std::collections::BTreeMap;

use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventType, ReindexSource, SnapshotDraft, SnapshotScope,
    Visibility,
};

use crate::events;

/// One block in a page's subtree: its aggregate key (`page#block-<id>`), version, and the live
/// payload the index reads (references-not-payloads — the rendered-text ref + structural ids, never
/// the raw body bytes, which live behind the page's per-subject DEK).
#[derive(Clone, Debug, PartialEq, Eq)]
struct BlockTruth {
    /// The block's aggregate key (`<page>#block-<id>`) — the per-block ordering partition + half the
    /// deterministic snapshot id.
    aggregate: String,
    /// The block version (the OTHER half of the deterministic id — an edited block re-snapshots).
    version: u64,
    /// The live block payload (refs/ids — references-not-payloads).
    payload: serde_json::Value,
}

/// One page in Knowledge's source of truth: its page-level version + its ORDERED block subtree.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PageTruth {
    /// The page aggregate key (`myelin://<tenant>/knowledge/page/<id>`).
    page_aggregate: String,
    /// The page-level version (drives the `knowledge.page.snapshot`).
    page_version: u64,
    /// The page's block subtree, in document order (a `BTreeMap` so replay order is deterministic).
    blocks: BTreeMap<String, BlockTruth>,
}

/// **Knowledge's [`ReindexSource`] body (EB-26 / P-246, M3 — the named floor filled).** Holds
/// Knowledge's OWN source of truth (its pages + block subtrees) and replays a `page:<id>` scope at
/// BLOCK granularity → the `knowledge.page.snapshot` + per-block `knowledge.block.snapshot` drafts. A
/// real wiring reads KN's content store; this reads its in-memory truth — the SAME shape (the live
/// store swaps in behind this same `replay` signature).
#[derive(Debug, Default)]
pub struct KnowledgeReindexSource {
    /// `page-id → PageTruth`. A `BTreeMap` so the replay order is deterministic (a rebuild is
    /// byte-reproducible).
    pages: BTreeMap<String, PageTruth>,
}

impl KnowledgeReindexSource {
    /// A fresh, empty source.
    pub fn new() -> KnowledgeReindexSource {
        KnowledgeReindexSource::default()
    }

    /// Record/update a page's truth: its page-level version + its ordered `(block_id, version,
    /// payload)` subtree (the live state a collab session settled to). Each block payload gets a
    /// `version` field stamped in so the derived store reads it for LWW.
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
                BlockTruth { aggregate, version: *version, payload },
            );
        }
        self.pages.insert(
            page_id.to_string(),
            PageTruth { page_aggregate, page_version, blocks: subtree },
        );
    }

    /// Mark a page erased (a tombstone) — it is REMOVED from the truth, so a subsequent replay SKIPS
    /// it AND its whole block subtree (the erasure stays erased across a reindex, X-7).
    pub fn erase_page(&mut self, page_id: &str) -> bool {
        self.pages.remove(page_id).is_some()
    }

    /// Parse the page id out of a `page:<id>` selector (or `page:all` for every page).
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
            return Vec::new(); // an unparseable / non-page selector matches nothing in KN's truth.
        };
        let mut drafts = Vec::new();
        for (page_id, page) in &self.pages {
            if target != "all" && page_id.as_str() != target {
                continue;
            }
            // The page-level snapshot (if past the cursor).
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
            // The BLOCK-granular subtree: one snapshot per block (the KN granularity, contract 2.6).
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
                ("b1", 2, serde_json::json!({ "kind": "heading", "text_ref": "r1" })),
                ("b2", 7, serde_json::json!({ "kind": "paragraph", "text_ref": "r2" })),
                ("b3", 1, serde_json::json!({ "kind": "code", "text_ref": "r3" })),
            ],
        );
        s.upsert_page("notes", 1, &[("n1", 1, serde_json::json!({ "kind": "paragraph" }))]);
        s
    }

    /// KN's `replay` is sub-artifact-granular at BLOCK granularity: a `page:home` scope re-emits the
    /// page snapshot + ONE `knowledge.block.snapshot` per block in the subtree.
    #[test]
    fn replay_page_scope_emits_one_snapshot_per_block() {
        let s = source();
        let drafts = s.replay(&SnapshotScope::new("knowledge", "page:home"), None);
        // 1 page snapshot + 3 block snapshots.
        assert_eq!(drafts.len(), 4);
        let page = drafts.iter().filter(|d| d.type_.0 == "knowledge.page.snapshot").count();
        let blocks = drafts.iter().filter(|d| d.type_.0 == "knowledge.block.snapshot").count();
        assert_eq!(page, 1, "one page snapshot");
        assert_eq!(blocks, 3, "one snapshot per block (block granularity)");
        // Only the home page (not notes) — the scope is page-specific.
        assert!(drafts.iter().all(|d| d.aggregate.0.contains("page/home")));
    }

    /// `since` is the incremental cursor at block granularity: only blocks past the cursor replay.
    #[test]
    fn replay_since_cursor_is_block_granular() {
        let s = source();
        // since=3: page (v5) replays; b2 (v7) replays; b1 (v2) + b3 (v1) do NOT.
        let drafts = s.replay(&SnapshotScope::new("knowledge", "page:home"), Some(3));
        let block_versions: Vec<u64> = drafts
            .iter()
            .filter(|d| d.type_.0 == "knowledge.block.snapshot")
            .map(|d| d.version)
            .collect();
        assert_eq!(block_versions, vec![7], "only the block past the cursor replays");
    }

    /// An ERASED page is SKIPPED — its whole block subtree stays erased across a reindex (X-7).
    #[test]
    fn replay_skips_an_erased_page_and_its_subtree() {
        let mut s = source();
        assert!(s.erase_page("home"));
        let drafts = s.replay(&SnapshotScope::new("knowledge", "page:all"), None);
        assert!(
            drafts.iter().all(|d| !d.aggregate.0.contains("page/home")),
            "the erased page + its blocks are not re-snapshotted"
        );
        // The other page is still replayed.
        assert!(drafts.iter().any(|d| d.aggregate.0.contains("page/notes")));
    }

    /// **cold == live + idempotent re-run (BUS-D5 for the KN owner, block-granular).** Build a LIVE
    /// projection from KN's block snapshots; wipe + rebuild from the reindex replay through the
    /// outbox; assert byte-identical. Then re-run — 0 new (the deterministic ids no-op it).
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
            let row = outbox.row(&draft.event_id()).expect("snapshot row present");
            cold.ingest(&row.envelope);
        }
        assert_eq!(cold.parity_bytes(), live.parity_bytes(), "cold == live (byte-identical)");

        let r2 = reindex(&scope, None, sources, &mut outbox, ctx_base()).expect("re-reindex");
        assert_eq!(r2.snapshots_emitted, 0, "a re-run emits 0 new (idempotent)");
        assert_eq!(r2.snapshots_skipped_duplicate, 4);
    }

    /// The deterministic block snapshot id is stable for a block aggregate@version.
    #[test]
    fn block_snapshot_id_is_deterministic() {
        let a = AggregateKey("myelin://acme/knowledge/page/home#block-b2".into());
        assert_eq!(snapshot_event_id(&a, 7), snapshot_event_id(&a, 7));
        assert_ne!(snapshot_event_id(&a, 7), snapshot_event_id(&a, 8));
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
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:00Z".into()),
            payload: draft.payload.clone(),
        }
    }
}

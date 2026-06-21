//! # Integration — SRCH-P19 (P-262, M3): sub-artifact-granular + content-anchored projections
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` SRCH-D5 (Git+KN
//! corpus, the reindex-parity proving the projection re-derives). **Architecture:**
//! `search-and-indexing.md` §4.1 tail / §4.9 ask (sub-artifact-granular projections; Git line-ranges
//! content-anchored — the searchable span re-derived from the owner's resolve, never a stale raw line
//! number; KN replay page-subtree at block granularity; Git replay per-blob/ref), §3.1 (the `doc_id`
//! may carry a frozen `#sub`). **Reconciliation:** change #7 / X-4 (the unified `#sub` grammar +
//! content-anchoring). **Contracts:** 5.7 (the `#sub` kinds on real sub-anchors), 2.6 (sub-artifact-
//! granular replay).
//!
//! ## What this proves (the dated green artifact, 2026-06-21)
//! The sub-artifact GRAIN classifier ([`myelin_search::SubGrain`], keyed off the FROZEN
//! `myelin_refs::sub_kind` grammar) + the sub-doc projection builders ([`block_subdoc_projection`],
//! [`db_row_subdoc_projection`], [`db_field_subdoc_projection`], [`line_range_subdoc_projection`])
//! drive the LIVE [`IncrementalIndexer`] per-event pipeline so that:
//!
//! 1. **Sub-anchors resolve at the right grain** — a doc block (`#b<id>`), a KN db row (`#row-<id>`),
//!    a KN field (`#field-<id>`), and a Git content-anchored line-range (`#L<a>-L<b>`) each index as a
//!    sub-precise `doc_id` whose ACL pins on the `#sub`-stripped parent (§3.1). A query hits the
//!    sub-doc, not the whole artifact.
//! 2. **Content-anchoring re-derives (the chained force-push test)** — a Git line-range another
//!    artifact embeds is force-pushed; the owner's `project` re-derives the span to its shifted
//!    position; a SCOPED reindex re-drives `project`; the indexed line-range carries the NEW position
//!    (never the stale raw line number).
//! 3. **SRCH-D5 Git+KN reindex-parity** — a live sub-artifact corpus (KN blocks + Git line-ranges) is
//!    wiped + reindexed-from-source; cold == live (doc set + searchability + the re-derived
//!    content-anchored line-ranges).
//!
//! The ENGINE is UNCHANGED — this is producer-corpus + grain wiring (the prompt's DoD). No new
//! mutation-core module is added; the SRCH-P16 reindex mutation floor still holds on the real corpora.
//! The M4 producer sub-anchors (`comment-`/`thread-`/`message-`/`check-`/`step-`) are the named floor.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_events::{
    Actor, AggregateKey, CorrelationId, DataRole, EmitContextBase, EventEnvelope, EventId, EventType,
    OutboxStore, ReindexSource, SnapshotScope, Timestamp, Visibility,
};
use myelin_events::reindex::ReferenceReindexSource;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_query::{FieldValue, OrderKey};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

use myelin_search::{
    block_subdoc_projection, db_field_subdoc_projection, db_row_subdoc_projection,
    line_range_subdoc_projection, AclFilter, AnchorState, ContentAnchoredSpan, IncrementalIndexer,
    IndexSpec, MockEmbeddingAdapter, ProjectFetchError, ProjectFetcher, SearchProjection,
    SearchReindexer, SubGrain, FACET_ANCHOR_STATE, FACET_LINE_START,
};
use myelin_content::{parse_inline, Block, HeadingLevel};

// ----------------------------------------------------------------------------------------------
// fixtures
// ----------------------------------------------------------------------------------------------

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn principal() -> Principal {
    Principal::stub(PrincipalId("p-1".into()), PrincipalKind::Human, tenant())
}

/// A scripted [`ProjectFetcher`] over a `ref → SearchProjection` map — the owner's `project(ref,
/// viewer)` (5.6 / 5.7). For a sub-anchored ref the OWNER resolved the `#sub` (and for a Git
/// line-range re-derived the content-anchored span); this fetcher serves what the owner returned (the
/// no-cross-db floor — Search never reads the owner DB). The map is MUTABLE so the force-push test can
/// swap the owner's re-derived span (the owner's `project` now returns the shifted span).
#[derive(Default)]
struct OwnerFetcher {
    projections: Mutex<BTreeMap<String, SearchProjection>>,
}
impl OwnerFetcher {
    fn put(&self, ref_: &str, p: SearchProjection) {
        self.projections.lock().unwrap().insert(ref_.to_string(), p);
    }
}
impl ProjectFetcher for OwnerFetcher {
    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
    ) -> Result<SearchProjection, ProjectFetchError> {
        match self.projections.lock().unwrap().get(&ref_.0) {
            Some(p) => Ok(p.clone()),
            None => Err(ProjectFetchError::Gone),
        }
    }
}

/// The index specs the sub-artifact corpus declares: KN page (block/heading sub-docs ride the page
/// spec's facet union) + KN db_row (row/field sub-docs) + Git blob (line-range sub-docs ride the blob
/// facet union + the re-derived line-range facets). The facet union is the union of all specs.
fn corpus_specs() -> Vec<IndexSpec> {
    let mut specs = myelin_search::kn_index_specs();
    specs.push(git_line_range_spec());
    specs
}

/// A `git`/`blob` spec whose struct_fields are the line-range sub-doc facet union (path/language/
/// blob_oid + the re-derived line_start/line_end/anchor_state). `acl_object_type = repo`.
fn git_line_range_spec() -> IndexSpec {
    IndexSpec::new("git", "blob", myelin_search::line_range_subdoc_facets())
        .with_acl_object_type("repo")
}

/// Build a live indexer over the sub-artifact corpus specs + the owner fetcher.
fn indexer() -> (Arc<IncrementalIndexer>, Arc<OwnerFetcher>) {
    let fetcher = Arc::new(OwnerFetcher::default());
    let ix = Arc::new(IncrementalIndexer::new(
        corpus_specs(),
        fetcher.clone(),
        Arc::new(MockEmbeddingAdapter::new(8)),
    ));
    (ix, fetcher)
}

/// A `*.created` event for a (sub-precise) ref. The subsystem/type is read from the ref; a `#sub`
/// makes the doc sub-precise (the indexer keys it sub-precisely + pins the ACL on the parent).
fn created_event(type_: &str, ref_: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(format!("ev:{ref_}")),
        type_: EventType(type_.into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(principal()),
        subject: ArtifactRef(ref_.into()),
        aggregate: AggregateKey(format!("agg:{ref_}")),
        causation_id: None,
        correlation_id: CorrelationId(ref_.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        payload: serde_json::json!({ "zookie": "zk-1", "version": 1 }),
    }
}

// ----------------------------------------------------------------------------------------------
// 1. sub-anchors resolve at the right grain (doc blocks, KN rows/fields, Git line-ranges)
// ----------------------------------------------------------------------------------------------

/// **A doc BLOCK sub-anchor (`#b<id>`) indexes at block granularity — the doc_id is sub-precise, the
/// ACL pins on the parent page, and a query hits the BLOCK (not the whole page).**
#[test]
fn doc_block_sub_anchor_resolves_at_block_grain() {
    let (ix, fetcher) = indexer();
    let block_ref = "myelin://acme/knowledge/page/42#b9";
    // The owner resolved the `#b9` sub-anchor → that block's projection.
    let block = Block::Paragraph {
        inline: parse_inline("the deadlock detection block only", &[]),
    };
    fetcher.put(block_ref, block_subdoc_projection(&block, Some("en")));

    ix.index(&created_event("knowledge.page.updated", block_ref)).expect("index block sub-doc");
    assert_eq!(ix.live_count(&tenant(), &region()), 1, "the block sub-doc is indexed");

    // A query hits the block sub-doc; the doc_id is the SUB-PRECISE ref (the `#b9` is kept).
    let hits = ix.search_ft(&tenant(), &region(), &AclFilter::All, "deadlock", 10).expect("ft");
    assert_eq!(hits.len(), 1, "the block is searchable at block grain");
    assert_eq!(hits[0].doc_id, block_ref, "the doc_id keeps the #b9 (sub-precise, §3.1)");

    // The classifier confirms the grain (the frozen grammar).
    assert_eq!(
        SubGrain::classify(&ArtifactRef(block_ref.into())),
        SubGrain::Block("9".into())
    );
}

/// **A KN db ROW sub-anchor (`#row-<id>`) indexes at row granularity — the row's typed facets + its
/// full-text are searchable, the doc_id is sub-precise.**
#[test]
fn kn_db_row_sub_anchor_resolves_at_row_grain() {
    let (ix, fetcher) = indexer();
    let row_ref = "myelin://acme/knowledge/db_row/tasks:r7#row-r7";
    let mut fields: BTreeMap<String, FieldValue> = BTreeMap::new();
    fields.insert("priority".into(), FieldValue::Select("P0".into()));
    let ok = OrderKey::parse("hmmmm").expect("a base-62 key");
    fetcher.put(
        row_ref,
        db_row_subdoc_projection(&fields, "a row about the P0 incident", Some(ok)),
    );

    ix.index(&created_event("knowledge.db_row.created", row_ref)).expect("index row sub-doc");
    assert_eq!(ix.live_count(&tenant(), &region()), 1);

    // The structured facet (priority == P0) hits the row sub-doc (the GIN-scan path), ACL-filtered.
    let hits = ix
        .search_structured(
            &tenant(),
            &region(),
            &AclFilter::All,
            "priority",
            &FieldValue::Select("P0".into()),
            10,
        )
        .expect("structured");
    assert_eq!(hits.len(), 1, "the row is found by its typed facet at row grain");
    assert_eq!(hits[0].doc_id, row_ref, "the doc_id keeps the #row-r7 (sub-precise)");
    // The row's full-text is searchable too.
    let ft = ix.search_ft(&tenant(), &region(), &AclFilter::All, "incident", 10).expect("ft");
    assert_eq!(ft.len(), 1);
}

/// **A KN FIELD sub-anchor (`#field-<id>`) indexes at field granularity (the finest KN grain) — the
/// one resolved field + its rendered text, sub-precise doc_id.**
#[test]
fn kn_field_sub_anchor_resolves_at_field_grain() {
    let (ix, fetcher) = indexer();
    let field_ref = "myelin://acme/knowledge/db_row/tasks:r7#field-priority";
    fetcher.put(
        field_ref,
        db_field_subdoc_projection("priority", FieldValue::Select("P0".into()), "priority is P0"),
    );

    ix.index(&created_event("knowledge.db_row.updated", field_ref)).expect("index field sub-doc");
    let hits = ix
        .search_structured(
            &tenant(),
            &region(),
            &AclFilter::All,
            "priority",
            &FieldValue::Select("P0".into()),
            10,
        )
        .expect("structured");
    assert_eq!(hits.len(), 1, "the field sub-doc is found at field grain");
    assert_eq!(hits[0].doc_id, field_ref, "the doc_id keeps the #field-priority");
    assert_eq!(
        SubGrain::classify(&ArtifactRef(field_ref.into())),
        SubGrain::Field("priority".into())
    );
}

/// **A Git content-anchored LINE-RANGE sub-anchor (`#L<a>-L<b>`) indexes the RE-DERIVED span — the
/// span is code-searchable, the doc_id is sub-precise, and the re-derived endpoints + anchor state are
/// stamped (never a raw line number).**
#[test]
fn git_line_range_sub_anchor_resolves_at_span_grain_content_anchored() {
    let (ix, fetcher) = indexer();
    let lr_ref = "myelin://acme/git/blob/repo:main:src/scheduler/deadlock.rs#L42-L45";
    // The owner's `project` re-derived the content-anchored span (exact match here).
    let span = ContentAnchoredSpan {
        path: "src/scheduler/deadlock.rs".into(),
        language: "rust".into(),
        blob_oid: "oid-v1".into(),
        resolved_start: 42,
        resolved_end: 45,
        span_text: "fn detectDeadlock(graph: &WaitForGraph) -> bool { graph.has_cycle() }".into(),
        anchor_state: AnchorState::Exact,
    };
    fetcher.put(lr_ref, line_range_subdoc_projection(&span));

    ix.index(&created_event("git.blob.indexed", lr_ref)).expect("index line-range sub-doc");
    assert_eq!(ix.live_count(&tenant(), &region()), 1);

    // The span is code-searchable (a symbol query hits the line-range sub-doc).
    let hits = ix.search_ft(&tenant(), &region(), &AclFilter::All, "detectdeadlock", 10).expect("ft");
    assert_eq!(hits.len(), 1, "the line-range span is code-searchable at span grain");
    assert_eq!(hits[0].doc_id, lr_ref, "the doc_id keeps the #L42-L45 (sub-precise)");

    // The re-derived endpoint is stamped (the owner's resolve, never a stored raw line).
    let by_state = ix
        .search_structured(
            &tenant(),
            &region(),
            &AclFilter::All,
            FACET_ANCHOR_STATE,
            &FieldValue::Text("exact".into()),
            10,
        )
        .expect("structured");
    assert_eq!(by_state.len(), 1, "the anchor state is a typed facet (exact)");
}

// ----------------------------------------------------------------------------------------------
// 2. content-anchoring: a force-pushed line-range re-derives through a scoped reindex
// ----------------------------------------------------------------------------------------------

const REGION: &str = "fr-par";

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
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:00Z".into()),
        caused_by: None,
    }
}

/// **CHAINED (the prompt's required content-anchoring test): a Git line-range another artifact embeds
/// is FORCE-PUSHED; the owner's `project` re-derives the span to its shifted position; a SCOPED
/// reindex re-drives `project`; the indexed line-range carries the NEW position — never the stale raw
/// line number (content-anchored, §3.1/§4.9).**
#[test]
fn chained_force_push_line_range_re_derives_through_a_scoped_reindex() {
    // The owner truth: a single Git blob aggregate at the line-range scope. The reference owner replays
    // the sub-doc as `myelin://t/git/blob/<agg>` — mirror that for the fetcher key.
    let mut src = ReferenceReindexSource::new("git", "blob");
    src.upsert("repo:main:src/scheduler/deadlock.rs#L42-L45", 1, serde_json::json!({ "kind": "blob" }));
    let snapshot_ref = "myelin://t/git/blob/repo:main:src/scheduler/deadlock.rs#L42-L45";

    let (ix, fetcher) = indexer();

    // BEFORE the force-push: the owner resolves the span at lines 42–45 (exact, oid-v1).
    let before = ContentAnchoredSpan {
        path: "src/scheduler/deadlock.rs".into(),
        language: "rust".into(),
        blob_oid: "oid-v1".into(),
        resolved_start: 42,
        resolved_end: 45,
        span_text: "fn detectDeadlock(graph: &WaitForGraph) -> bool { graph.has_cycle() }".into(),
        anchor_state: AnchorState::Exact,
    };
    fetcher.put(snapshot_ref, line_range_subdoc_projection(&before));

    // Initial index-from-source (the live consumer path).
    let reindexer = SearchReindexer::new(ix.clone(), region());
    let scope = SnapshotScope::new("git", "blob:all");
    let mut outbox = OutboxStore::new();
    reindexer
        .reindex(&tenant(), &scope, None, &[&src], &mut outbox, ctx_base())
        .expect("initial index");
    assert_eq!(ix.live_count(&tenant(), &region()), 1, "the line-range sub-doc is indexed");

    // The indexed line-range starts at the minted position (42) — the BEFORE state.
    let pre = ix
        .search_structured(
            &tenant(),
            &region(),
            &AclFilter::All,
            FACET_LINE_START,
            &FieldValue::Text("42".into()),
            10,
        )
        .expect("structured before");
    assert_eq!(pre.len(), 1, "before the force-push the span is at line 42");

    // ---- THE FORCE-PUSH ----
    // A force-push rewrote the blob; the fingerprinted lines moved to 60–63 (the owner's
    // `resolve_line_range` returned `Rebased { new_start: 60, .. }` against the new blob oid). The owner's
    // `project` NOW returns the re-derived span at the shifted position. The index holds NO raw line
    // number — it re-derives on the owner's resolve.
    let after = ContentAnchoredSpan {
        blob_oid: "oid-v2-after-force-push".into(),
        resolved_start: 60,
        resolved_end: 63,
        anchor_state: AnchorState::Rebased,
        ..before.clone()
    };
    fetcher.put(snapshot_ref, line_range_subdoc_projection(&after));

    // ---- THE SCOPED REINDEX ----
    // A scoped reindex (the §4.9 scoped reindex, driven at line-range grain) re-drives the owner's
    // `project` — which re-derives the content-anchored span. The blob aggregate is re-emitted at a new
    // version (the force-push bumped it). A full re-scope wipes + rebuilds; the rebuilt projection carries
    // the NEW position.
    let mut src_after = ReferenceReindexSource::new("git", "blob");
    src_after.upsert(
        "repo:main:src/scheduler/deadlock.rs#L42-L45",
        2,
        serde_json::json!({ "kind": "blob" }),
    );
    let mut outbox2 = OutboxStore::new();
    let job = reindexer
        .reindex(&tenant(), &scope, None, &[&src_after], &mut outbox2, ctx_base())
        .expect("scoped reindex after force-push");
    assert!(job.is_done());
    assert_eq!(ix.live_count(&tenant(), &region()), 1, "still exactly one line-range sub-doc");

    // ---- THE CONTENT-ANCHORING INVARIANT ----
    // The re-derived line-range is at the NEW position (60) — the stale 42 is GONE (never stored).
    let stale = ix
        .search_structured(
            &tenant(),
            &region(),
            &AclFilter::All,
            FACET_LINE_START,
            &FieldValue::Text("42".into()),
            10,
        )
        .expect("structured stale");
    assert!(stale.is_empty(), "the STALE raw line number (42) is GONE — content-anchored, not positional");

    let fresh = ix
        .search_structured(
            &tenant(),
            &region(),
            &AclFilter::All,
            FACET_LINE_START,
            &FieldValue::Text("60".into()),
            10,
        )
        .expect("structured fresh");
    assert_eq!(fresh.len(), 1, "the span RE-DERIVES to the shifted position (60) through the owner's resolve");
    assert_eq!(fresh[0].doc_id, snapshot_ref, "the same sub-precise doc_id, re-derived span");

    // The anchor state now flags `rebased` (the hit renders the `moved` flag from the CURRENT resolve).
    let moved = ix
        .search_structured(
            &tenant(),
            &region(),
            &AclFilter::All,
            FACET_ANCHOR_STATE,
            &FieldValue::Text("rebased".into()),
            10,
        )
        .expect("structured moved");
    assert_eq!(moved.len(), 1, "the re-derived span is flagged `rebased` (moved)");
    // The span content is still code-searchable after the re-derive.
    let ft = ix.search_ft(&tenant(), &region(), &AclFilter::All, "detectdeadlock", 10).expect("ft");
    assert_eq!(ft.len(), 1, "the span content is still searchable post-force-push");
}

// ----------------------------------------------------------------------------------------------
// 3. SRCH-D5 (Git+KN sub-artifact corpus): cold == live reindex-parity
// ----------------------------------------------------------------------------------------------

/// **SRCH-D5 (Git+KN sub-artifact corpus): build a LIVE index of a KN block sub-doc + a Git
/// line-range sub-doc; wipe; reindex-from-source; the rebuilt index is identical (doc set +
/// searchability + the re-derived content-anchored line-range).** The reindex re-drives the SAME live
/// consumer path (cold == live); the projection re-derives correctly through the owner's resolve.
#[test]
fn srch_d5_git_kn_subartifact_reindex_parity_cold_equals_live() {
    let _ = REGION;
    // The KN block sub-doc + the Git line-range sub-doc (one source per owner).
    let block_agg = "page/42#b9";
    let lr_agg = "repo:main:src/x.rs#L10-L12";
    let block_snap = format!("myelin://t/knowledge/page/{block_agg}");
    let lr_snap = format!("myelin://t/git/blob/{lr_agg}");

    let mut kn_src = ReferenceReindexSource::new("knowledge", "page");
    kn_src.upsert(block_agg, 1, serde_json::json!({ "kind": "page" }));
    let mut git_src = ReferenceReindexSource::new("git", "blob");
    git_src.upsert(lr_agg, 1, serde_json::json!({ "kind": "blob" }));

    let (ix, fetcher) = indexer();
    let block = Block::Heading {
        level: HeadingLevel::new(2).unwrap(),
        inline: parse_inline("raft consensus block", &[]),
    };
    fetcher.put(&block_snap, block_subdoc_projection(&block, Some("en")));
    let span = ContentAnchoredSpan {
        path: "src/x.rs".into(),
        language: "rust".into(),
        blob_oid: "oid-1".into(),
        resolved_start: 10,
        resolved_end: 12,
        span_text: "fn paxosRound() -> Ballot { Ballot::next() }".into(),
        anchor_state: AnchorState::Exact,
    };
    fetcher.put(&lr_snap, line_range_subdoc_projection(&span));

    // LIVE: ingest both sub-docs through the ordinary `*.created`/`*.indexed` path.
    ix.index(&created_event("knowledge.page.updated", &block_snap)).expect("live block");
    ix.index(&created_event("git.blob.indexed", &lr_snap)).expect("live line-range");
    let live_count = ix.live_count(&tenant(), &region());
    let live_raft = ix.search_ft(&tenant(), &region(), &AclFilter::All, "raft", 10).expect("live raft");
    let live_paxos = ix.search_ft(&tenant(), &region(), &AclFilter::All, "paxosround", 10).expect("live paxos");
    assert_eq!(live_count, 2, "the live index holds both sub-docs");
    assert_eq!(live_raft.len(), 1, "the KN block sub-doc is searchable");
    assert_eq!(live_paxos.len(), 1, "the Git line-range sub-doc is searchable");

    // COLD: wipe + reindex-from-source (both owners) through the live consumer path. A full reindex
    // (`since = None`) WIPES the whole per-tenant index, so we wipe ONCE on the first owner (knowledge),
    // then append the second owner (git) with an incremental backfill (`since = Some(0)`, no re-wipe) —
    // the multi-owner cold rebuild the §4.9 scope drives (each owner's `replay` feeds the live consumer).
    let reindexer = SearchReindexer::new(ix.clone(), region());
    let sources: &[&dyn ReindexSource] = &[&kn_src, &git_src];
    let kn_scope = SnapshotScope::new("knowledge", "page:all");
    let git_scope = SnapshotScope::new("git", "blob:all");
    let mut kn_outbox = OutboxStore::new();
    let kn_job = reindexer
        .reindex(&tenant(), &kn_scope, None, sources, &mut kn_outbox, ctx_base())
        .expect("reindex KN (wipes once)");
    assert!(kn_job.is_done());
    let mut git_outbox = OutboxStore::new();
    let git_job = reindexer
        .reindex(&tenant(), &git_scope, Some(0), sources, &mut git_outbox, ctx_base())
        .expect("backfill git (appends, no re-wipe)");
    assert!(git_job.is_done());

    // PARITY: cold == live (doc count + the same sub-docs searchable + the re-derived line-range).
    let cold_count = ix.live_count(&tenant(), &region());
    let cold_raft = ix.search_ft(&tenant(), &region(), &AclFilter::All, "raft", 10).expect("cold raft");
    let cold_paxos = ix.search_ft(&tenant(), &region(), &AclFilter::All, "paxosround", 10).expect("cold paxos");
    assert_eq!(cold_count, live_count, "cold-rebuilt doc count == live (SRCH-D5)");
    assert_eq!(cold_raft.len(), live_raft.len(), "the KN block sub-doc is searchable in the cold rebuild");
    assert_eq!(cold_paxos.len(), live_paxos.len(), "the Git line-range sub-doc is searchable in the cold rebuild");
    // The re-derived content-anchored line-range survives the rebuild (still at its resolved position).
    let lr = ix
        .search_structured(
            &tenant(),
            &region(),
            &AclFilter::All,
            FACET_LINE_START,
            &FieldValue::Text("10".into()),
            10,
        )
        .expect("structured cold line-range");
    assert_eq!(lr.len(), 1, "the content-anchored line-range re-derives in the cold rebuild (SRCH-D5)");
}

/// **The Search half of E2E-1 behaviour proven in-context: a hit on a confidential sub-doc resolves to
/// a tombstone with 0 title/count leak.** A private block sub-doc under a viewer with no grant NEVER
/// appears in any result (incl. counts) — the ACL pre-filter (pinned on the parent) fires; the
/// sub-precise doc_id never leaks. (The DoD's named in-context E2E-1 Search behaviour.)
#[test]
fn e2e1_confidential_sub_doc_is_a_tombstone_zero_leak() {
    let (ix, fetcher) = indexer();
    let block_ref = "myelin://acme/knowledge/page/secret#b1";
    let block = Block::Paragraph { inline: parse_inline("the confidential merger terms", &[]) };
    fetcher.put(block_ref, block_subdoc_projection(&block, Some("en")));
    ix.index(&created_event("knowledge.page.updated", block_ref)).expect("index secret block");

    // A viewer with NO grant: the ACL pre-filter admits NOTHING (the parent page is unreachable). The
    // confidential block sub-doc never appears in any result, incl. the count.
    let denied = AclFilter::None;
    let hits = ix.search_ft(&tenant(), &region(), &denied, "merger", 10).expect("ft denied");
    assert!(hits.is_empty(), "0 leak: the confidential sub-doc never appears (incl. count) without a grant");

    // A grant on the PARENT page (the ACL pins on the #sub-stripped parent, §3.1) ⇒ the block appears.
    let granted = AclFilter::ids(["myelin://acme/knowledge/page/secret"]);
    let ok = ix.search_ft(&tenant(), &region(), &granted, "merger", 10).expect("ft granted");
    assert_eq!(ok.len(), 1, "a grant on the parent page makes the block sub-doc reachable (the ACL, not a deny)");
    assert_eq!(ok[0].doc_id, block_ref, "the sub-precise doc_id resolves once reachable");
}

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_events::reindex::ReferenceReindexSource;
use myelin_events::{
    Actor, AggregateKey, CorrelationId, DataRole, EmitContextBase, EventEnvelope, EventId,
    EventType, OutboxStore, ReindexSource, SnapshotScope, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_query::{FieldValue, OrderKey};
use myelin_tenancy::{ArtifactRef, Region, TenantId};

use myelin_content::{parse_inline, Block, HeadingLevel};
use myelin_search::{
    block_subdoc_projection, db_field_subdoc_projection, db_row_subdoc_projection,
    line_range_subdoc_projection, AclFilter, AnchorState, ContentAnchoredSpan, EmbeddingAdapter,
    IncrementalIndexer, IndexSpec, MockEmbeddingAdapter, ProjectFetchError, ProjectFetcher,
    SearchProjection, SearchReindexer, SubGrain, FACET_ANCHOR_STATE, FACET_LINE_START,
};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn principal() -> Principal {
    Principal::stub(PrincipalId("p-1".into()), PrincipalKind::Human, tenant())
}

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

fn corpus_specs() -> Vec<IndexSpec> {
    let mut specs = myelin_search::kn_index_specs();
    specs.push(git_line_range_spec());
    specs
}

fn git_line_range_spec() -> IndexSpec {
    IndexSpec::new("git", "blob", myelin_search::line_range_subdoc_facets())
        .with_parent_acl_object_type("repo", "repo")
}

fn indexer() -> (Arc<IncrementalIndexer>, Arc<OwnerFetcher>) {
    let fetcher = Arc::new(OwnerFetcher::default());
    let ix = Arc::new(IncrementalIndexer::new(
        corpus_specs(),
        fetcher.clone(),
        Arc::new(MockEmbeddingAdapter::new(8)),
    ));
    (ix, fetcher)
}

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

#[test]
fn doc_block_sub_anchor_resolves_at_block_grain() {
    let (ix, fetcher) = indexer();
    let block_ref = "myelin://acme/knowledge/page/42#b9";
    let block = Block::Paragraph {
        inline: parse_inline("the deadlock detection block only", &[]),
    };
    fetcher.put(block_ref, block_subdoc_projection(&block, Some("en")));

    ix.index(&created_event("knowledge.page.updated", block_ref))
        .expect("index block sub-doc");
    assert_eq!(
        ix.live_count(&tenant(), &region()),
        1,
        "the block sub-doc is indexed"
    );

    let hits = ix
        .search_ft(&tenant(), &region(), &AclFilter::All, "deadlock", 10)
        .expect("ft");
    assert_eq!(hits.len(), 1, "the block is searchable at block grain");
    assert_eq!(
        hits[0].doc_id, block_ref,
        "the doc_id keeps the #b9 (sub-precise, §3.1)"
    );

    assert_eq!(
        SubGrain::classify(&ArtifactRef(block_ref.into())),
        SubGrain::Block("9".into())
    );
}

#[test]
fn kn_db_row_sub_anchor_resolves_at_row_grain() {
    let (ix, fetcher) = indexer();
    let row_ref = "myelin://acme/knowledge/row/tasks:r7#row-r7";
    let mut fields: BTreeMap<String, FieldValue> = BTreeMap::new();
    fields.insert("priority".into(), FieldValue::Select("P0".into()));
    let ok = OrderKey::parse("hmmmm").expect("a base-62 key");
    fetcher.put(
        row_ref,
        db_row_subdoc_projection(&fields, "a row about the P0 incident", Some(ok)),
    );

    ix.index(&created_event("knowledge.row.created", row_ref))
        .expect("index row sub-doc");
    assert_eq!(ix.live_count(&tenant(), &region()), 1);

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
    assert_eq!(
        hits.len(),
        1,
        "the row is found by its typed facet at row grain"
    );
    assert_eq!(
        hits[0].doc_id, row_ref,
        "the doc_id keeps the #row-r7 (sub-precise)"
    );
    let ft = ix
        .search_ft(&tenant(), &region(), &AclFilter::All, "incident", 10)
        .expect("ft");
    assert_eq!(ft.len(), 1);
}

#[test]
fn kn_field_sub_anchor_resolves_at_field_grain() {
    let (ix, fetcher) = indexer();
    let field_ref = "myelin://acme/knowledge/row/tasks:r7#field-priority";
    fetcher.put(
        field_ref,
        db_field_subdoc_projection(
            "priority",
            FieldValue::Select("P0".into()),
            "priority is P0",
        ),
    );

    ix.index(&created_event("knowledge.row.updated", field_ref))
        .expect("index field sub-doc");
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
    assert_eq!(
        hits[0].doc_id, field_ref,
        "the doc_id keeps the #field-priority"
    );
    assert_eq!(
        SubGrain::classify(&ArtifactRef(field_ref.into())),
        SubGrain::Field("priority".into())
    );
}

#[test]
fn git_line_range_sub_anchor_resolves_at_span_grain_content_anchored() {
    let (ix, fetcher) = indexer();
    let lr_ref = "myelin://acme/git/blob/repo:main:src/scheduler/deadlock.rs#L42-L45";
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

    ix.index(&created_event("git.blob.indexed", lr_ref))
        .expect("index line-range sub-doc");
    assert_eq!(ix.live_count(&tenant(), &region()), 1);

    let hits = ix
        .search_ft(&tenant(), &region(), &AclFilter::All, "detectdeadlock", 10)
        .expect("ft");
    assert_eq!(
        hits.len(),
        1,
        "the line-range span is code-searchable at span grain"
    );
    assert_eq!(
        hits[0].doc_id, lr_ref,
        "the doc_id keeps the #L42-L45 (sub-precise)"
    );

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
    assert_eq!(
        by_state.len(),
        1,
        "the anchor state is a typed facet (exact)"
    );
}

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

#[test]
fn chained_force_push_line_range_re_derives_through_a_scoped_reindex() {
    let mut src = ReferenceReindexSource::new(tenant(), "git", "blob");
    src.upsert(
        "repo:main:src/scheduler/deadlock.rs#L42-L45",
        1,
        serde_json::json!({ "kind": "blob" }),
    );
    let snapshot_ref = "myelin://acme/git/blob/repo:main:src/scheduler/deadlock.rs#L42-L45";

    let (ix, fetcher) = indexer();

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

    let reindexer = SearchReindexer::new(ix.clone(), region());
    let scope = SnapshotScope::new("git", "blob:all");
    let mut outbox = OutboxStore::new();
    reindexer
        .reindex(&tenant(), &scope, None, &[&src], &mut outbox, ctx_base())
        .expect("initial index");
    assert_eq!(
        ix.live_count(&tenant(), &region()),
        1,
        "the line-range sub-doc is indexed"
    );

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

    let after = ContentAnchoredSpan {
        blob_oid: "oid-v2-after-force-push".into(),
        resolved_start: 60,
        resolved_end: 63,
        anchor_state: AnchorState::Rebased,
        ..before.clone()
    };
    fetcher.put(snapshot_ref, line_range_subdoc_projection(&after));

    let mut src_after = ReferenceReindexSource::new(tenant(), "git", "blob");
    src_after.upsert(
        "repo:main:src/scheduler/deadlock.rs#L42-L45",
        2,
        serde_json::json!({ "kind": "blob" }),
    );
    let mut outbox2 = OutboxStore::new();
    let job = reindexer
        .reindex(
            &tenant(),
            &scope,
            None,
            &[&src_after],
            &mut outbox2,
            ctx_base(),
        )
        .expect("scoped reindex after force-push");
    assert!(job.is_done());
    assert_eq!(
        ix.live_count(&tenant(), &region()),
        1,
        "still exactly one line-range sub-doc"
    );

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
    assert!(
        stale.is_empty(),
        "the STALE raw line number (42) is GONE - content-anchored, not positional"
    );

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
    assert_eq!(
        fresh.len(),
        1,
        "the span RE-DERIVES to the shifted position (60) through the owner's resolve"
    );
    assert_eq!(
        fresh[0].doc_id, snapshot_ref,
        "the same sub-precise doc_id, re-derived span"
    );

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
    assert_eq!(
        moved.len(),
        1,
        "the re-derived span is flagged `rebased` (moved)"
    );
    let ft = ix
        .search_ft(&tenant(), &region(), &AclFilter::All, "detectdeadlock", 10)
        .expect("ft");
    assert_eq!(
        ft.len(),
        1,
        "the span content is still searchable post-force-push"
    );
}

#[test]
fn srch_d5_git_kn_subartifact_reindex_parity_cold_equals_live() {
    let _ = REGION;
    let block_agg = "42#b9";
    let lr_agg = "repo:main:src/x.rs#L10-L12";
    let block_snap = format!("myelin://acme/knowledge/page/{block_agg}");
    let lr_snap = format!("myelin://acme/git/blob/{lr_agg}");

    let mut kn_src = ReferenceReindexSource::new(tenant(), "knowledge", "page");
    kn_src.upsert(block_agg, 1, serde_json::json!({ "kind": "page" }));
    let mut git_src = ReferenceReindexSource::new(tenant(), "git", "blob");
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

    ix.index(&created_event("knowledge.page.updated", &block_snap))
        .expect("live block");
    ix.index(&created_event("git.blob.indexed", &lr_snap))
        .expect("live line-range");
    let live_count = ix.live_count(&tenant(), &region());
    let live_raft = ix
        .search_ft(&tenant(), &region(), &AclFilter::All, "raft", 10)
        .expect("live raft");
    let live_paxos = ix
        .search_ft(&tenant(), &region(), &AclFilter::All, "paxosround", 10)
        .expect("live paxos");
    assert_eq!(live_count, 2, "the live index holds both sub-docs");
    assert_eq!(live_raft.len(), 1, "the KN block sub-doc is searchable");
    assert_eq!(
        live_paxos.len(),
        1,
        "the Git line-range sub-doc is searchable"
    );

    let reindexer = SearchReindexer::new(ix.clone(), region());
    let sources: &[&dyn ReindexSource] = &[&kn_src, &git_src];
    let kn_scope = SnapshotScope::new("knowledge", "page:all");
    let git_scope = SnapshotScope::new("git", "blob:all");
    let mut kn_outbox = OutboxStore::new();
    let kn_job = reindexer
        .reindex(
            &tenant(),
            &kn_scope,
            None,
            sources,
            &mut kn_outbox,
            ctx_base(),
        )
        .expect("reindex KN (wipes once)");
    assert!(kn_job.is_done());
    let mut git_outbox = OutboxStore::new();
    let git_job = reindexer
        .reindex(
            &tenant(),
            &git_scope,
            Some(0),
            sources,
            &mut git_outbox,
            ctx_base(),
        )
        .expect("backfill git (appends, no re-wipe)");
    assert!(git_job.is_done());

    let cold_count = ix.live_count(&tenant(), &region());
    let cold_raft = ix
        .search_ft(&tenant(), &region(), &AclFilter::All, "raft", 10)
        .expect("cold raft");
    let cold_paxos = ix
        .search_ft(&tenant(), &region(), &AclFilter::All, "paxosround", 10)
        .expect("cold paxos");
    assert_eq!(
        cold_count, live_count,
        "cold-rebuilt doc count == live (SRCH-D5)"
    );
    assert_eq!(
        cold_raft.len(),
        live_raft.len(),
        "the KN block sub-doc is searchable in the cold rebuild"
    );
    assert_eq!(
        cold_paxos.len(),
        live_paxos.len(),
        "the Git line-range sub-doc is searchable in the cold rebuild"
    );
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
    assert_eq!(
        lr.len(),
        1,
        "the content-anchored line-range re-derives in the cold rebuild (SRCH-D5)"
    );
}

#[test]
fn e2e1_confidential_sub_doc_is_a_tombstone_zero_leak() {
    let (ix, fetcher) = indexer();
    let block_ref = "myelin://acme/knowledge/page/secret#b1";
    let block = Block::Paragraph {
        inline: parse_inline("the confidential merger terms", &[]),
    };
    fetcher.put(block_ref, block_subdoc_projection(&block, Some("en")));
    ix.index(&created_event("knowledge.page.updated", block_ref))
        .expect("index secret block");

    let denied = AclFilter::None;
    let hits = ix
        .search_ft(&tenant(), &region(), &denied, "merger", 10)
        .expect("ft denied");
    assert!(
        hits.is_empty(),
        "0 leak: the confidential sub-doc never appears (incl. count) without a grant"
    );

    let granted = AclFilter::ids(["myelin://acme/knowledge/page/secret"]);
    let ok = ix
        .search_ft(&tenant(), &region(), &granted, "merger", 10)
        .expect("ft granted");
    assert_eq!(
        ok.len(),
        1,
        "a grant on the parent page makes the block sub-doc reachable (the ACL, not a deny)"
    );
    assert_eq!(
        ok[0].doc_id, block_ref,
        "the sub-precise doc_id resolves once reachable"
    );
}

#[test]
fn e2e1_confidential_sub_doc_semantic_path_admits_via_parent_acl_object() {
    let (ix, fetcher) = indexer();
    let block_ref = "myelin://acme/knowledge/page/secret#b1";
    let parent = "myelin://acme/knowledge/page/secret";
    let text = "the confidential merger terms";
    let block = Block::Paragraph {
        inline: parse_inline(text, &[]),
    };
    fetcher.put(block_ref, block_subdoc_projection(&block, Some("en")));
    ix.index(&created_event("knowledge.page.updated", block_ref))
        .expect("index secret block (embedded)");

    let query = MockEmbeddingAdapter::new(8)
        .embed(text)
        .expect("embed query");

    let denied = ix
        .search_semantic(&tenant(), &region(), &AclFilter::None, &query, 10)
        .expect("semantic denied");
    assert!(
        denied.is_empty(),
        "0 vector leak: the confidential sub-doc's embedding never surfaces without a grant"
    );

    let granted = AclFilter::ids([parent]);
    let ok = ix
        .search_semantic(&tenant(), &region(), &granted, &query, 10)
        .expect("semantic granted");
    assert_eq!(
        ok.len(),
        1,
        "a grant on the parent acl_object makes the sub-doc's vector reachable (acl_object arm)"
    );
    assert_eq!(
        ok[0].doc_id, block_ref,
        "the sub-precise doc_id resolves on the vector path once reachable"
    );
}

#[test]
fn e2e1_confidential_sub_doc_semantic_deny_set_both_directions() {
    let (ix, fetcher) = indexer();
    let block_ref = "myelin://acme/knowledge/page/secret#b1";
    let parent = "myelin://acme/knowledge/page/secret";
    let text = "the confidential merger terms";
    let block = Block::Paragraph {
        inline: parse_inline(text, &[]),
    };
    fetcher.put(block_ref, block_subdoc_projection(&block, Some("en")));
    ix.index(&created_event("knowledge.page.updated", block_ref))
        .expect("index secret block (embedded)");

    let query = MockEmbeddingAdapter::new(8)
        .embed(text)
        .expect("embed query");

    let unrelated_deny = AclFilter::not_ids(["myelin://acme/knowledge/page/other"]);
    let admitted = ix
        .search_semantic(&tenant(), &region(), &unrelated_deny, &query, 10)
        .expect("semantic unrelated deny");
    assert_eq!(
        admitted
            .iter()
            .map(|h| h.doc_id.as_str())
            .collect::<Vec<_>>(),
        vec![block_ref],
        "a deny naming neither identifier admits the sub-doc (control)"
    );

    let deny_parent = AclFilter::not_ids([parent]);
    assert!(
        ix.search_semantic(&tenant(), &region(), &deny_parent, &query, 10)
            .expect("semantic deny parent")
            .is_empty(),
        "R2.7: a deny on the parent acl_object excludes the sub-doc's vector hit (no semantic leak)"
    );

    let deny_docid = AclFilter::not_ids([block_ref]);
    assert!(
        ix.search_semantic(&tenant(), &region(), &deny_docid, &query, 10)
            .expect("semantic deny doc_id")
            .is_empty(),
        "a deny on the sub-precise doc_id excludes the sub-doc's vector hit (doc_id arm enforced)"
    );
}

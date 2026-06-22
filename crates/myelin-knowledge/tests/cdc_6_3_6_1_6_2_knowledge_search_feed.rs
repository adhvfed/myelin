//! # CDC — the Knowledge Search feed (rows 6.3 declare_indexable + 6.1 query / 6.2 semantic)
//!
//! KN-P21 / P-311 (M3). This file carries BOTH the **provider** and **consumer** sides of the
//! Knowledge search-feed seam (the coverage scanner requires both markers per file):
//!
//! - **6.3 `declare_indexable(IndexSpec)`** — Knowledge is the **PROVIDER** (it OWNS + declares its
//!   page + db_row index specs, re-homed verbatim from `myelin-search`); Search is the **CONSUMER**
//!   (it ADMITS them into a live indexer's per-tenant facet union).
//! - **6.1 `query` / 6.2 `semantic`** — Search is the **PROVIDER** (the frozen permission-aware
//!   engine that conjoins the `list_objects` `Filter` before scoring); Knowledge is the **CONSUMER**
//!   (it drives `query`/`semantic` ALWAYS through the conjoining entry, pinned to the `page` object
//!   type — the `search-requires-acl-filter` discipline; the KN-D5 re-confirm: a confidential page
//!   appears in NEITHER the result NOR the COUNT, across the FT and the semantic/RAG paths).
//!
//! The point of a CDC is that PROVIDER and CONSUMER agree on the SAME shape: the spec Knowledge
//! declares is byte-equal to the set Search admits, and the conjoin Knowledge routes through is the
//! engine's frozen pre-filter (no parallel un-ACL'd path, EI-01 §7).

use myelin_identity::{
    Consistency, ConsistencyMode, ListObjectsResult, Literal, ObjectId, ObjectType, Permission,
    Principal, PrincipalId, PrincipalKind, Result as AuthzResult, Zookie,
};
use myelin_knowledge::search_feed::{
    feed_project, kn_declared_index_specs, kn_read_permission, kn_search_query, kn_search_semantic,
    register_kn_index_specs, FeedGrain, KN_SEARCH_OBJECT_TYPE,
};
use myelin_query::{CmpOp, Expr, FieldType, FieldValue, OrderKey, Predicate, QueryAst};
use myelin_search::{
    ConsistencyStats, EmbeddingAdapter, FieldDecl, FieldSchema, IndexBackend, IndexDocument,
    IndexSpec, IncrementalIndexer, ListObjectsPort, MockEmbeddingAdapter, Page, QueryStats,
    ScopedEngine, TantivyBackend, VectorQuery, FT_BODY_FIELD, ORDER_KEY_FIELD, SEMANTIC_FIELD,
};
use myelin_tenancy::TenantId;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

// ─────────────────────────── shared fixtures ───────────────────────────

fn viewer() -> Principal {
    Principal::stub(PrincipalId("p:alice".into()), PrincipalKind::Human, TenantId("acme".into()))
}

fn consistency() -> Consistency {
    Consistency { at_least: Zookie("z0".into()), mode: ConsistencyMode::BoundedStale }
}

fn ast_body(term: &str) -> QueryAst {
    QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var(FT_BODY_FIELD.into()),
        rhs: Expr::Lit(Literal::Str(term.into())),
    })
    .expect("within cost bounds")
}

fn semantic_ast(term: &str) -> QueryAst {
    QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var(SEMANTIC_FIELD.into()),
        rhs: Expr::Lit(Literal::Str(term.into())),
    })
    .expect("within cost bounds")
}

/// The scripted `list_objects` port — the leak-free pre-filter the CONSUMER conjoins. Counts the
/// calls (the no-N+1 GATE asserts exactly one per query).
struct FakeAuthz {
    answer: ListObjectsResult,
    calls: AtomicU64,
}
impl FakeAuthz {
    fn ids(ids: &[&str]) -> FakeAuthz {
        FakeAuthz {
            answer: ListObjectsResult::Ids {
                ids: ids.iter().map(|i| ObjectId((*i).into())).collect(),
                zookie: Zookie("z-acl".into()),
            },
            calls: AtomicU64::new(0),
        }
    }
}
impl ListObjectsPort for FakeAuthz {
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _t: &ObjectType,
        _a: &Consistency,
    ) -> AuthzResult<ListObjectsResult> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(self.answer.clone())
    }
}

fn facet_decl() -> BTreeMap<String, FieldType> {
    let mut m = BTreeMap::new();
    m.insert(ORDER_KEY_FIELD.to_string(), FieldType::OrderKey);
    m
}

fn schema() -> FieldSchema {
    FieldSchema::new()
        .with(FT_BODY_FIELD, FieldDecl::stored(FieldType::Text))
        .with(ORDER_KEY_FIELD, FieldDecl::stored(FieldType::OrderKey))
}

// ─────────────────────────── 6.3 declare_indexable ───────────────────────────

/// **PROVIDER (6.3) — Knowledge OWNS + declares its index specs.** The owned set is the page +
/// db_row specs (the block doc rides the page object type); it is byte-equal to the re-homed Search
/// shape (no parallel second shape — EI-01 §7).
#[test]
fn provider_knowledge_declares_its_index_specs() {
    let specs: Vec<IndexSpec> = kn_declared_index_specs();
    assert_eq!(specs.len(), 2, "exactly the page + db_row spec TYPES");
    assert!(specs.iter().any(|s| s.type_ == "page" && s.semantic), "the semantic page doc");
    assert!(specs.iter().any(|s| s.type_ == "db_row" && !s.semantic), "the non-semantic db_row doc");
    for s in &specs {
        assert_eq!(s.subsystem, "knowledge", "owned under the knowledge subsystem token");
    }
}

/// **CONSUMER (6.3) — Search ADMITS the declared specs.** A live indexer takes the SAME declared set
/// into its per-tenant facet union without a schema mismatch (the agreed shape both sides hold).
#[test]
fn consumer_search_admits_the_declared_specs() {
    let accepted = register_kn_index_specs();
    assert_eq!(accepted, kn_declared_index_specs(), "Search admits exactly the declared KN specs");
    // And a live indexer over the owned set opens (the facet union is consistent across the specs).
    let _ix = IncrementalIndexer::new(
        kn_declared_index_specs(),
        std::sync::Arc::new(NullFetcher),
        std::sync::Arc::new(MockEmbeddingAdapter::new(16)),
    );
}

struct NullFetcher;
impl myelin_search::ProjectFetcher for NullFetcher {
    fn project(
        &self,
        _t: &myelin_tenancy::TenantId,
        _r: &myelin_tenancy::Region,
        _ref_: &myelin_tenancy::ArtifactRef,
    ) -> Result<myelin_search::SearchProjection, myelin_search::ProjectFetchError> {
        Err(myelin_search::ProjectFetchError::Gone)
    }
}

// ─────────────────────────── 6.1 query / 6.2 semantic (the KN-D5 conjoin) ───────────────────────────

/// Index a KN page corpus, projecting each page via [`feed_project`] (the `project` feed Search
/// consumes off the bus — the PROVIDER text source, no DB read).
fn kn_page_corpus() -> TantivyBackend {
    let mut be = TantivyBackend::open(&facet_decl()).expect("open");
    let mut upsert = |id: &str, body: &str| {
        let blocks = vec![myelin_content::Block::Paragraph {
            inline: myelin_content::parse_inline(body, &[]),
        }];
        let proj = feed_project(&blocks, FeedGrain::Page, Some("en"));
        let k = OrderKey::bisect(None, None);
        let mut d =
            IndexDocument::new(id, &proj.text).with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k));
        for (name, value) in proj.fields {
            d = d.with_field(&name, value);
        }
        be.upsert(&d).unwrap();
    };
    upsert("myelin://acme/knowledge/page/PUB-1", "deadlock in the scheduler runbook");
    upsert("myelin://acme/knowledge/page/SECRET-9", "deadlock secret incident postmortem");
    be
}

/// **CONSUMER (6.1) — Knowledge drives `query` with the `list_objects` `Filter` ALWAYS conjoined;
/// the PROVIDER (Search engine) conjoins it before scoring → a confidential page is in NEITHER the
/// result NOR the COUNT (KN-D5 re-confirm, 0 leak incl. COUNT).** Pinned to the `page` object type.
#[test]
fn consumer_knowledge_query_conjoins_filter_no_count_leak() {
    let be = kn_page_corpus();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
    let unauth = FakeAuthz::ids(&["myelin://acme/knowledge/page/PUB-1"]);
    let stats = QueryStats::new();
    let res = kn_search_query(&eng, &unauth, &ast_body("deadlock"), &viewer(), &consistency(), Page::FIRST, &stats)
        .expect("query");
    let ids: Vec<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
    assert_eq!(ids, ["myelin://acme/knowledge/page/PUB-1"], "the confidential page is pre-filtered out");
    assert_eq!(res.hits.len(), 1, "the COUNT reveals neither the existence nor the number of forbidden pages");
    assert_eq!(unauth.calls.load(Ordering::Relaxed), 1, "exactly ONE list_objects (no N+1)");
}

/// **CONSUMER (6.2) — Knowledge drives `semantic`/RAG with the conjoin; the PROVIDER returns k
/// VISIBLE neighbours (filter-during-traversal) → a confidential page never enters the candidate
/// set (KN-D5 vector half, the agent-RAG path).** The query text is the exact secret-page text.
#[test]
fn consumer_knowledge_semantic_conjoins_filter() {
    let embedder = MockEmbeddingAdapter::new(16);
    let mut be = TantivyBackend::open(&facet_decl()).expect("open");
    let mut emb = |id: &str, body: &str| {
        let v = embedder.embed(body).expect("embeds");
        let k = OrderKey::bisect(None, None);
        let d = IndexDocument::new(id, body)
            .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k))
            .with_embedding(v, embedder.model_ref());
        be.upsert(&d).unwrap();
    };
    emb("myelin://acme/knowledge/page/PUB-1", "deadlock in the scheduler runbook");
    emb("myelin://acme/knowledge/page/SECRET-9", "deadlock secret ops postmortem");

    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
    let vq = VectorQuery::Text { text: "deadlock secret ops postmortem".into(), embedder: &embedder };
    let unauth = FakeAuthz::ids(&["myelin://acme/knowledge/page/PUB-1"]);
    let stats = QueryStats::new();
    let cstats = ConsistencyStats::new();
    let res = kn_search_semantic(
        &eng,
        &unauth,
        &semantic_ast("deadlock secret ops postmortem"),
        &viewer(),
        &consistency(),
        &vq,
        Page::FIRST,
        &stats,
        &cstats,
    )
    .expect("semantic");
    let ids: std::collections::BTreeSet<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
    assert!(
        !ids.contains("myelin://acme/knowledge/page/SECRET-9"),
        "the confidential page never surfaces through the semantic/RAG path (KN-D5: 0 leak)"
    );
    assert_eq!(stats.list_objects_calls(), 1, "exactly ONE list_objects (no N+1 on the semantic path)");
}

/// **The CONSUMER pins the conjoin to the agreed (object-type, permission) the PROVIDER expects.**
/// Knowledge always `list_objects(viewer, knowledge.read, page)` — the `page` reachability the page
/// + block docs are ACL'd under (a block is never more visible than its page).
#[test]
fn consumer_pins_the_agreed_object_type_and_permission() {
    assert_eq!(KN_SEARCH_OBJECT_TYPE, "page");
    assert_eq!(kn_read_permission(), Permission("knowledge.read".into()));
}

//! # CDC — contract 6.2 `semantic(text|vec, viewer, k, filter_ast?) → k visible NN` (SRCH-P11 →
//! P-174, the PUBLIC query-path provider side)
//!
//! **Contract:** `contract-index.md` row 6.2 — `semantic(text|vec, viewer, k, filter_ast?) → k
//! visible NN` — ACL-filtered-during-traversal k-NN; **agent RAG**, dedup. **Architecture:**
//! `search-and-indexing.md` §4.5 (vector k-NN over the per-tenant HNSW, ACL-filtered DURING
//! traversal; RRF hybrid fusion — both branches carry the same ACL filter so fusion can never
//! introduce a hidden doc) / §4.2.2 (filter-during-traversal — k VISIBLE neighbours, brute-force
//! fallback for very selective filters).
//!
//! - **PROVIDER** = the public [`myelin_search::semantic`] query entry (the pipeline that runs
//!   `list_objects` → lowers the ACL `SetExpr` → executes the vector branch filter-during-traversal
//!   → fuses with the lexical branch via RRF → applies the no-stale-grant zookie pass).
//! - **CONSUMER** = **agent RAG** (VISION §3): an agent retrieves the top-k VISIBLE passages for its
//!   delegated principal; it NEVER retrieves a doc that principal cannot see (RAG is
//!   permission-correct by the same pre-filter, contract 6.2).
//!
//! The dated green artifact: the consumer (an agent's RAG retrieval) supplies a query (`text` form,
//! embedded through the SAME mock adapter the corpus was embedded under), and the provider returns
//! ONLY the visible nearest passages — the nearest CONFIDENTIAL passage never enters the candidate
//! set. If the 6.2 shape drifts (the viewer-driven ACL pre-filter is dropped, the text|vec form
//! changes, k stops bounding the visible neighbours), this stops compiling/passing — the contract.
//! Floors: the TUNED filtered-ANN strategy + recall@k-at-scale is SRCH-P26/D8; the real EU-hostable
//! embedding model is post-M5 (the mock adapter is the named v1 floor).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use myelin_identity::{
    Consistency, ConsistencyMode, ListObjectsResult, Literal, ObjectId, ObjectType, Permission,
    Principal, PrincipalId, PrincipalKind, Result as AuthzResult, Zookie,
};
use myelin_query::{CmpOp, Expr, FieldType, FieldValue, OrderKey, Predicate, QueryAst};
use myelin_tenancy::TenantId;

use myelin_search::{
    semantic, ConsistencyStats, EmbeddingAdapter, FieldDecl, FieldSchema, IndexBackend,
    IndexDocument, ListObjectsPort, MockEmbeddingAdapter, Page, QueryStats, ScopedEngine,
    TantivyBackend, VectorQuery, FT_BODY_FIELD, ORDER_KEY_FIELD, SEMANTIC_FIELD,
};

fn facet_decl() -> BTreeMap<String, FieldType> {
    let mut m = BTreeMap::new();
    m.insert("status".to_string(), FieldType::Select);
    m.insert(ORDER_KEY_FIELD.to_string(), FieldType::OrderKey);
    m
}

fn schema() -> FieldSchema {
    FieldSchema::new()
        .with(FT_BODY_FIELD, FieldDecl::stored(FieldType::Text))
        .with("status", FieldDecl::stored(FieldType::Select))
        .with(ORDER_KEY_FIELD, FieldDecl::stored(FieldType::OrderKey))
}

fn viewer() -> Principal {
    Principal::stub(
        PrincipalId("agent:delegated".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn consistency() -> Consistency {
    Consistency {
        at_least: Zookie("z0".into()),
        mode: ConsistencyMode::BoundedStale,
    }
}

/// The PROVIDER-facing authz port: `list_objects` returns the viewer's reachable allow-set (the S4
/// materialised `Ids` path) — the SAME pre-filter the vector branch is conjoined with.
struct RagAuthz {
    reachable: Vec<String>,
    calls: AtomicU64,
}
impl ListObjectsPort for RagAuthz {
    fn list_objects(
        &self,
        _subject: &Principal,
        permission: &Permission,
        ty: &ObjectType,
        _at: &Consistency,
    ) -> AuthzResult<ListObjectsResult> {
        assert_eq!(
            permission,
            &Permission("read".into()),
            "RAG reads under `read`, never a wider perm"
        );
        assert_eq!(ty, &ObjectType("issue".into()));
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(ListObjectsResult::Ids {
            ids: self.reachable.iter().map(|s| ObjectId(s.clone())).collect(),
            zookie: Zookie("z-acl".into()),
        })
    }
}

/// Index a doc with an embedding under the SAME adapter the query will use (one vector space, §3.3).
fn rag_corpus(embedder: &MockEmbeddingAdapter) -> TantivyBackend {
    let mut be = TantivyBackend::open(&facet_decl()).expect("open");
    let mut emb = |id: &str, body: &str| {
        let v = embedder.embed(body).expect("body embeds");
        let k = OrderKey::bisect(None, None);
        let d = IndexDocument::new(id, body)
            .with_field("status", FieldValue::Select("open".into()))
            .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k))
            .with_embedding(v, embedder.model_ref());
        be.upsert(&d).unwrap();
    };
    emb("acme/issue/PUB-1", "how the scheduler avoids deadlock");
    emb("acme/issue/PUB-2", "indexer backpressure design");
    emb(
        "acme/issue/SECRET-PLAN",
        "confidential acquisition plan deadlock",
    );
    be
}

fn ast(p: Predicate) -> QueryAst {
    QueryAst::compiled(p).expect("within cost bounds")
}

/// **The 6.2 CDC pair: agent RAG retrieves the top-k VISIBLE passages; the confidential passage —
/// even when it is the nearest neighbour — never enters the candidate set.** The provider runs the
/// viewer-driven ACL pre-filter FIRST (the `list_objects` allow-set), then the vector branch
/// filter-during-traversal. The text form embeds the query through the corpus's adapter.
#[test]
fn semantic_6_2_agent_rag_gets_only_visible_passages() {
    let embedder = MockEmbeddingAdapter::new(16);
    let be = rag_corpus(&embedder);
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());

    // The agent's delegated principal can read the two PUBLIC issues, NOT the confidential plan.
    let authz = RagAuthz {
        reachable: vec!["acme/issue/PUB-1".into(), "acme/issue/PUB-2".into()],
        calls: AtomicU64::new(0),
    };
    let stats = QueryStats::new();
    let cstats = ConsistencyStats::new();

    // The RAG query text is the EXACT confidential-plan text — so SECRET-PLAN is its nearest vector.
    let q = ast(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var(SEMANTIC_FIELD.into()),
        rhs: Expr::Lit(Literal::Str(
            "confidential acquisition plan deadlock".into(),
        )),
    });
    let vq = VectorQuery::Text {
        text: "confidential acquisition plan deadlock".into(),
        embedder: &embedder,
    };

    let res = semantic(
        &eng,
        &authz,
        None,
        &q,
        &viewer(),
        &ObjectType("issue".into()),
        &consistency(),
        &vq,
        Page {
            offset: 0,
            limit: 10,
        },
        &stats,
        &cstats,
    )
    .expect("the 6.2 semantic entry answers");

    let ids: std::collections::BTreeSet<&str> =
        res.hits.iter().map(|h| h.doc_id.as_str()).collect();
    assert!(
        !ids.contains("acme/issue/SECRET-PLAN"),
        "the confidential passage — though the NEAREST neighbour — never reaches the agent (6.2 RAG: 0 leak)"
    );
    assert!(
        ids.contains("acme/issue/PUB-1") && ids.contains("acme/issue/PUB-2"),
        "the visible passages surface"
    );
    // The viewer-driven ACL pre-filter is exactly ONE list_objects (no per-result authz, no N+1).
    assert_eq!(
        authz.calls.load(Ordering::Relaxed),
        1,
        "exactly one list_objects for the RAG retrieval"
    );
}

/// **6.2 honours `k`: the result is bounded by the requested k VISIBLE neighbours (the page limit
/// here drives k).** With k=1 only the single nearest VISIBLE passage is returned.
#[test]
fn semantic_6_2_bounds_to_k_visible_neighbours() {
    let embedder = MockEmbeddingAdapter::new(16);
    let be = rag_corpus(&embedder);
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
    let authz = RagAuthz {
        reachable: vec!["acme/issue/PUB-1".into(), "acme/issue/PUB-2".into()],
        calls: AtomicU64::new(0),
    };
    let q = ast(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var(SEMANTIC_FIELD.into()),
        rhs: Expr::Lit(Literal::Str("how the scheduler avoids deadlock".into())),
    });
    let vq = VectorQuery::Text {
        text: "how the scheduler avoids deadlock".into(),
        embedder: &embedder,
    };
    let res = semantic(
        &eng,
        &authz,
        None,
        &q,
        &viewer(),
        &ObjectType("issue".into()),
        &consistency(),
        &vq,
        Page {
            offset: 0,
            limit: 1,
        }, // k = 1
        &QueryStats::new(),
        &ConsistencyStats::new(),
    )
    .expect("semantic k=1");
    assert_eq!(
        res.hits.len(),
        1,
        "k=1 ⇒ exactly the single nearest VISIBLE neighbour"
    );
    assert_eq!(
        res.hits[0].doc_id, "acme/issue/PUB-1",
        "the nearest visible passage to the query"
    );
}

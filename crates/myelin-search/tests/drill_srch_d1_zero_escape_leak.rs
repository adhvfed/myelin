use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use myelin_identity::{
    AuthzIndexRef, Consistency, ConsistencyMode, ListObjectsResult, Literal, ObjectType,
    Permission, Principal, PrincipalId, PrincipalKind, Result as AuthzResult, SetExpr, Zookie,
};
use myelin_query::{CmpOp, Expr, FieldType, FieldValue, OrderKey, Predicate, QueryAst};
use myelin_tenancy::TenantId;

use myelin_search::{
    query, semantic, AclFilter, Embedding, FieldDecl, FieldSchema, IndexBackend, IndexDocument,
    ListObjectsPort, Page, QueryStats, RelationalLeaf, ReverseIndexAnswer, RevisionWatermark,
    ScopedEngine, TantivyBackend, VectorQuery, FT_BODY_FIELD, ORDER_KEY_FIELD, SEMANTIC_FIELD,
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
        PrincipalId("p:alice".into()),
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

fn ast(p: Predicate) -> QueryAst {
    QueryAst::compiled(p).expect("within cost bounds")
}
fn var(n: &str) -> Expr {
    Expr::Var(n.into())
}
fn lit(v: &str) -> Expr {
    Expr::Lit(Literal::Str(v.into()))
}

struct ScriptedAuthz {
    set_expr: SetExpr,
    zookie: String,
    reverse: ReverseIndexAnswer,
    calls: AtomicU64,
    resolve_calls: AtomicU64,
}
impl ScriptedAuthz {
    fn new(visible: &[&str], zookie: &str, revision: u64) -> ScriptedAuthz {
        ScriptedAuthz {
            set_expr: SetExpr::TupleSet {
                index: AuthzIndexRef("authz_visible".into()),
            },
            zookie: zookie.into(),
            reverse: ReverseIndexAnswer {
                object_ids: visible.iter().map(|s| (*s).to_string()).collect(),
                revision: RevisionWatermark(revision),
            },
            calls: AtomicU64::new(0),
            resolve_calls: AtomicU64::new(0),
        }
    }
}
impl ListObjectsPort for ScriptedAuthz {
    fn list_objects(
        &self,
        _subject: &Principal,
        permission: &Permission,
        ty: &ObjectType,
        _at: &Consistency,
    ) -> AuthzResult<ListObjectsResult> {
        assert_eq!(permission, &Permission("read".into()));
        assert_eq!(ty, &ObjectType("issue".into()));
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(ListObjectsResult::Filter {
            set_expr: self.set_expr.clone(),
            zookie: Zookie(self.zookie.clone()),
        })
    }
    fn resolve_relation(
        &self,
        _subject: &Principal,
        form: &RelationalLeaf,
        required: &RevisionWatermark,
    ) -> AuthzResult<ReverseIndexAnswer> {
        assert!(
            matches!(form, RelationalLeaf::TupleSet { .. }),
            "the big-result path is a TupleSet JOIN"
        );
        assert!(
            self.reverse.revision >= *required,
            "the reverse index serves a fresh-enough revision"
        );
        self.resolve_calls.fetch_add(1, Ordering::Relaxed);
        Ok(self.reverse.clone())
    }
}

fn adversarial_corpus() -> TantivyBackend {
    let mut be = TantivyBackend::open(&facet_decl()).expect("open");
    let k = OrderKey::bisect(None, None);
    let doc = |id: &str, text: &str, embed: Vec<f32>| {
        IndexDocument::new(id, text)
            .with_field("status", FieldValue::Select("open".into()))
            .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k.clone()))
            .with_embedding(Embedding::new(embed), "text-embed@1")
    };
    be.upsert(&doc(
        "acme/issue/PUB-1",
        "a deadlock in the scheduler",
        vec![0.6, 0.4, 0.0],
    ))
    .unwrap();
    for i in 0..20 {
        be.upsert(&doc(
            &format!("acme/issue/SECRET-{i}"),
            "deadlock secret incident",
            vec![0.9, 0.1, 0.0],
        ))
        .unwrap();
    }
    be.upsert(&doc(
        "acme/issue/SECRET-STRONG",
        "deadlock deadlock deadlock deadlock everywhere",
        vec![1.0, 0.0, 0.0],
    ))
    .unwrap();
    be.upsert(
        &doc(
            SUB_DOC_ID,
            "deadlock in the confidential merger sub-block",
            vec![1.0, 0.0, 0.0],
        )
        .with_acl_object(SUB_DOC_PARENT),
    )
    .unwrap();
    be
}

const SUB_DOC_ID: &str = "acme/issue/SECRET-SUB#b1";
const SUB_DOC_PARENT: &str = "acme/issue/SECRET-SUB";

const VISIBLE: [&str; 1] = ["acme/issue/PUB-1"];

fn confidential_ids() -> Vec<String> {
    let mut v: Vec<String> = (0..20).map(|i| format!("acme/issue/SECRET-{i}")).collect();
    v.push("acme/issue/SECRET-STRONG".into());
    v
}

#[test]
fn srch_d1_zero_escape_leak_relational_big_result_path() {
    let be = adversarial_corpus();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
    let confidential = confidential_ids();

    let authz = ScriptedAuthz::new(&VISIBLE, "z@10", 10);
    let stats = QueryStats::new();

    let res = query(
        &eng,
        &authz,
        &ast(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var(FT_BODY_FIELD),
            rhs: lit("deadlock"),
        }),
        &viewer(),
        &ObjectType("issue".into()),
        &consistency(),
        Page {
            offset: 0,
            limit: 1000,
        },
        &stats,
    )
    .expect("query");

    let ids: Vec<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
    for c in &confidential {
        assert!(
            !ids.contains(&c.as_str()),
            "LEAK: confidential `{c}` surfaced in the FT result"
        );
    }
    assert_eq!(
        res.hits.len(),
        1,
        "0 count-leak: only the visible doc is counted"
    );
    assert_eq!(ids, ["acme/issue/PUB-1"], "only the visible doc surfaces");

    assert_eq!(
        authz.calls.load(Ordering::Relaxed),
        1,
        "exactly one list_objects (no N+1)"
    );
    assert_eq!(
        authz.resolve_calls.load(Ordering::Relaxed),
        1,
        "exactly one reverse-index JOIN"
    );

    let st = query(
        &eng,
        &authz,
        &ast(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var("status"),
            rhs: lit("open"),
        }),
        &viewer(),
        &ObjectType("issue".into()),
        &consistency(),
        Page {
            offset: 0,
            limit: 1000,
        },
        &QueryStats::new(),
    )
    .expect("structured query");
    let st_ids: Vec<&str> = st.hits.iter().map(|h| h.doc_id.as_str()).collect();
    assert_eq!(
        st_ids,
        ["acme/issue/PUB-1"],
        "0 leak on the structured branch (status==open)"
    );
    assert_eq!(st.hits.len(), 1, "0 count-leak on the structured branch");
}

#[test]
fn srch_d1_zero_escape_leak_rag_vector_half() {
    let be = adversarial_corpus();
    let confidential = confidential_ids();

    let acl = AclFilter::ids(VISIBLE);
    let hits = be
        .semantic(&acl, &Embedding::new(vec![1.0, 0.0, 0.0]), 5)
        .expect("semantic");
    let ids: Vec<&str> = hits.iter().map(|h| h.doc_id.as_str()).collect();
    for c in &confidential {
        assert!(
            !ids.contains(&c.as_str()),
            "RAG LEAK: confidential `{c}` surfaced as a neighbour"
        );
    }
    assert_eq!(
        ids,
        ["acme/issue/PUB-1"],
        "only the visible neighbour; the nearest hidden one never surfaces"
    );
}

#[test]
fn srch_d1_vector_deny_set_both_directions_doc_id_or_acl_object() {
    let be = adversarial_corpus();
    let q = Embedding::new(vec![1.0, 0.0, 0.0]);

    let admit = be
        .semantic(&AclFilter::ids([SUB_DOC_PARENT]), &q, 10)
        .expect("semantic grant on parent");
    assert!(
        admit.iter().any(|h| h.doc_id == SUB_DOC_ID),
        "a grant on the parent acl_object admits the sub-doc's vector (acl_object arm)"
    );

    let deny_parent = be
        .semantic(&AclFilter::not_ids([SUB_DOC_PARENT]), &q, 20)
        .expect("semantic deny on parent");
    assert!(
        !deny_parent.iter().any(|h| h.doc_id == SUB_DOC_ID),
        "R2.7 leak direction: a deny on the parent acl_object excludes the sub-doc's vector hit"
    );
    assert!(
        !deny_parent.is_empty(),
        "the deny is selective (other docs still surface) - not a blanket empty result"
    );

    let deny_docid = be
        .semantic(&AclFilter::not_ids([SUB_DOC_ID]), &q, 20)
        .expect("semantic deny on doc_id");
    assert!(
        !deny_docid.iter().any(|h| h.doc_id == SUB_DOC_ID),
        "a deny on the sub-precise doc_id excludes the sub-doc's vector hit (doc_id arm)"
    );
}

#[test]
fn srch_d1_rag_vector_half_through_the_public_semantic_entry() {
    let be = adversarial_corpus();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
    let confidential = confidential_ids();

    let authz = ScriptedAuthz::new(&VISIBLE, "z@10", 10);
    let stats = QueryStats::new();
    let cstats = myelin_search::ConsistencyStats::new();
    let q = ast(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: var(SEMANTIC_FIELD),
        rhs: lit("deadlock"),
    });
    let vq = VectorQuery::Vec(Embedding::new(vec![1.0, 0.0, 0.0]));

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
            limit: 1000,
        },
        &stats,
        &cstats,
    )
    .expect("semantic");

    let ids: Vec<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
    for c in &confidential {
        assert!(
            !ids.contains(&c.as_str()),
            "RAG LEAK through the public semantic entry: confidential `{c}` surfaced as a neighbour"
        );
    }
    assert_eq!(
        ids,
        ["acme/issue/PUB-1"],
        "only the visible neighbour; the nearest hidden one never surfaces"
    );
    assert_eq!(res.hits.len(), 1, "0 count-leak on the RAG/vector path");
    assert_eq!(
        authz.calls.load(Ordering::Relaxed),
        1,
        "exactly one list_objects (no N+1 on the RAG path)"
    );
    assert_eq!(
        authz.resolve_calls.load(Ordering::Relaxed),
        1,
        "exactly one reverse-index JOIN"
    );
}

#[test]
fn srch_d1_chained_grant_makes_the_relation_doc_visible() {
    let be = adversarial_corpus();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
    let q = ast(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: var(FT_BODY_FIELD),
        rhs: lit("deadlock"),
    });

    let before = ScriptedAuthz::new(&VISIBLE, "z@10", 10);
    let r0 = query(
        &eng,
        &before,
        &q,
        &viewer(),
        &ObjectType("issue".into()),
        &consistency(),
        Page {
            offset: 0,
            limit: 1000,
        },
        &QueryStats::new(),
    )
    .expect("before");
    assert_eq!(r0.hits.len(), 1, "only PUB-1 before the grant");

    let after = ScriptedAuthz::new(&["acme/issue/PUB-1", "acme/issue/SECRET-0"], "z@11", 11);
    let r1 = query(
        &eng,
        &after,
        &q,
        &viewer(),
        &ObjectType("issue".into()),
        &consistency(),
        Page {
            offset: 0,
            limit: 1000,
        },
        &QueryStats::new(),
    )
    .expect("after grant");
    let ids: std::collections::BTreeSet<&str> = r1.hits.iter().map(|h| h.doc_id.as_str()).collect();
    assert!(
        ids.contains("acme/issue/SECRET-0"),
        "after the grant the relation doc is visible"
    );
    assert!(ids.contains("acme/issue/PUB-1"));
    assert_eq!(
        ids.len(),
        2,
        "exactly the two now-reachable docs (still no leak of the other 20)"
    );
}

#[test]
fn srch_d1_deep_page_never_spills_a_hidden_doc() {
    let be = adversarial_corpus();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
    let authz = ScriptedAuthz::new(&VISIBLE, "z@10", 10);
    let res = query(
        &eng,
        &authz,
        &ast(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var(FT_BODY_FIELD),
            rhs: lit("deadlock"),
        }),
        &viewer(),
        &ObjectType("issue".into()),
        &consistency(),
        Page {
            offset: 1,
            limit: 50,
        },
        &QueryStats::new(),
    )
    .expect("query");
    assert!(
        res.hits.is_empty(),
        "no 'more results' tail of hidden docs (0 leak past the visible page)"
    );
}

//! # Drill — SRCH-D1 the cardinal zero-escape leak = 0 (F1), the relational big-result path
//! (SRCH-P09 → P-172)
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` SRCH-D1 (F1, the
//! cardinal sin — the zero-escape leak: a confidential issue / overridden page / private channel /
//! private repo file NEVER appears in any query result — including counts, IDF, "more results", and
//! RAG — for an unauthorized viewer, across an adversarial corpus, when the reachable set is a
//! relational `Filter`/`TupleSet`, the big-result path). **Architecture:** `search-and-indexing.md`
//! §4.2 (the relational `SetExpr` lowering → the reverse-index JOIN; pre-filter not post-filter,
//! §4.2.1 — count/IDF leakage + N+1 melt; hidden docs never enter the candidate set) + §4.2.3 (the
//! revision watermark the JOIN honours).
//!
//! ## What this drill proves (the dated green artifact, 2026-06-20)
//! An ADVERSARIAL corpus (many confidential docs sharing the rare term the leak would exploit for
//! IDF/count inference, alongside a few visible ones) is indexed. An unauthorized viewer's reachable
//! set is a **relational `TupleSet`** (the big-result path) the reverse-index JOIN resolves to ONLY
//! the visible doc-ids. The drill asserts, across the leak vectors:
//! - **0 leaked docs:** no confidential doc-id ever appears in any FT / structured / pure-ACL /
//!   semantic-RAG result for the unauthorized viewer.
//! - **0 count-leak:** the visible result COUNT is exactly the visible-set count — the hidden docs
//!   never contribute to the count (a post-filter would have shown a larger pre-filter count).
//! - **0 IDF/ranking leak:** a confidential doc that is a STRONGER textual match than the visible
//!   one never out-ranks (never surfaces at all) — the hidden doc never enters the scored candidate
//!   set, so it cannot perturb the visible doc's rank or the IDF.
//! - **0 RAG/vector leak:** the nearest semantic neighbour is confidential, yet it never surfaces —
//!   filter-during-traversal returns k VISIBLE neighbours (the SRCH-D1 vector/RAG half).
//! - **the chained grant:** grant the relation → the reverse-index JOIN now resolves the formerly
//!   confidential doc → it becomes visible (the rejection was the ACL firing, not a blanket deny).
//! - **no N+1:** exactly ONE `list_objects` + ONE reverse-index JOIN per query (the big-result path
//!   is one JOIN, not one check per candidate).
//!
//! ## Floors named
//! - The **full no-stale-grant + fail-static** mechanism (SRCH-D2: revoke → re-search → the
//!   fail-static cache bypass) is **SRCH-P10** (P-173). Here the watermark mechanism is wired so the
//!   JOIN never READS a stale reverse-index revision; the wait/bounded-recheck/fail-static is
//!   downstream.
//! - **BM25** is the default ranking; the learning-to-rank / semantic re-rank is **SRCH-P26**.
//! - The synthetic per-tenant facet schema is the named M3/M4 floor (real per-subsystem IndexSpecs).

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

/// A scripted authz port: `list_objects` returns a relational `TupleSet` Filter (the big-result
/// path), and `resolve_relation` JOINs the reverse index to a canned visible-id set + revision.
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
        // The JOIN is over the TupleSet leaf (the big-result path); the pipeline passes the watermark
        // derived from the list_objects zookie.
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

/// **THE ADVERSARIAL CORPUS.** One VISIBLE doc and MANY confidential docs, all sharing the rare term
/// `deadlock` (so a post-filter would leak via IDF/count) — plus a confidential doc that is a
/// STRONGER textual match (more occurrences) than the visible one (so a post-filter would let it
/// out-rank), and a confidential doc that is the NEAREST semantic neighbour (the RAG leak vector).
fn adversarial_corpus() -> TantivyBackend {
    let mut be = TantivyBackend::open(&facet_decl()).expect("open");
    let k = OrderKey::bisect(None, None);
    let doc = |id: &str, text: &str, embed: Vec<f32>| {
        IndexDocument::new(id, text)
            .with_field("status", FieldValue::Select("open".into()))
            .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k.clone()))
            .with_embedding(Embedding::new(embed), "text-embed@1")
    };
    // The ONE visible doc — a single `deadlock`.
    be.upsert(&doc(
        "acme/issue/PUB-1",
        "a deadlock in the scheduler",
        vec![0.6, 0.4, 0.0],
    ))
    .unwrap();
    // Many confidential docs all matching `deadlock` (the count/IDF adversary).
    for i in 0..20 {
        be.upsert(&doc(
            &format!("acme/issue/SECRET-{i}"),
            "deadlock secret incident",
            vec![0.9, 0.1, 0.0],
        ))
        .unwrap();
    }
    // A confidential doc that is a STRONGER textual match (repeats `deadlock`) — the ranking adversary.
    be.upsert(&doc(
        "acme/issue/SECRET-STRONG",
        "deadlock deadlock deadlock deadlock everywhere",
        vec![1.0, 0.0, 0.0],
    ))
    .unwrap();
    // R2.7 — a confidential SUB-ARTIFACT doc whose `doc_id` is `#sub`-precise while its `acl_object`
    // pins on the `#sub`-stripped parent (doc_id ≠ acl_object). It carries an embedding AT the query
    // direction (the nearest neighbour), so a vector path that only checked `doc_id` would let a deny
    // expressed on the PARENT `acl_object` leak this hit. The ACL parity fix denies it on either arm.
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

/// The R2.7 sub-artifact fixture: a `#sub`-precise `doc_id` whose ACL pins on the parent `acl_object`.
const SUB_DOC_ID: &str = "acme/issue/SECRET-SUB#b1";
const SUB_DOC_PARENT: &str = "acme/issue/SECRET-SUB";

/// The ONLY visible doc the reverse-index JOIN resolves for the unauthorized viewer.
const VISIBLE: [&str; 1] = ["acme/issue/PUB-1"];

/// Every confidential doc-id — none may EVER appear in any result for the unauthorized viewer.
fn confidential_ids() -> Vec<String> {
    let mut v: Vec<String> = (0..20).map(|i| format!("acme/issue/SECRET-{i}")).collect();
    v.push("acme/issue/SECRET-STRONG".into());
    v
}

/// **SRCH-D1 (F1) — the cardinal zero-escape leak across the FT/structured/pure-ACL surfaces, the
/// relational big-result path. 0 leaked docs, 0 count-leak, 0 IDF/ranking leak.**
#[test]
fn srch_d1_zero_escape_leak_relational_big_result_path() {
    let be = adversarial_corpus();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
    let confidential = confidential_ids();

    // The unauthorized viewer's reachable set is a TupleSet the JOIN resolves to ONLY PUB-1.
    let authz = ScriptedAuthz::new(&VISIBLE, "z@10", 10);
    let stats = QueryStats::new();

    // (a) The FT branch over the rare term `deadlock` — every confidential doc matches it (incl. the
    // STRONGER match). Only the visible doc surfaces.
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
        }, // a generous page so a count-leak would show up
        &stats,
    )
    .expect("query");

    let ids: Vec<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
    // 0 leaked docs.
    for c in &confidential {
        assert!(
            !ids.contains(&c.as_str()),
            "LEAK: confidential `{c}` surfaced in the FT result"
        );
    }
    // 0 count-leak: exactly ONE visible doc (not 22 — the hidden docs never entered the candidate set).
    assert_eq!(
        res.hits.len(),
        1,
        "0 count-leak: only the visible doc is counted"
    );
    assert_eq!(ids, ["acme/issue/PUB-1"], "only the visible doc surfaces");

    // 0 N+1: one list_objects + one reverse-index JOIN (the big-result path is ONE JOIN).
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

    // (b) The structured branch (status == open matches ALL 22 docs) — still only PUB-1.
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

/// **SRCH-D1 (the RAG/vector half) — the nearest semantic neighbour is confidential, yet it never
/// surfaces (filter-during-traversal returns k VISIBLE neighbours).** Proven directly against the
/// engine `semantic` entry with the SAME relational `Ids` ACL filter the JOIN resolves to.
#[test]
fn srch_d1_zero_escape_leak_rag_vector_half() {
    let be = adversarial_corpus();
    let confidential = confidential_ids();

    // The reverse-index JOIN resolves the unauthorized viewer to ONLY PUB-1 — the SAME visible-id set
    // the pipeline lowers a TupleSet to. The query vector is NEAREST to the confidential STRONG doc.
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

/// **SRCH-D1 / R2.7 — the vector-path deny-set leak test, BOTH directions, over a `doc_id ≠
/// acl_object` sub-artifact fixture.** The confidential sub-doc (`doc_id` = `…SECRET-SUB#b1`,
/// `acl_object` = the parent `…SECRET-SUB`) carries an embedding AT the query direction — it is the
/// nearest neighbour, the strongest leak vector. The vector filter-during-traversal predicate matches
/// `doc_id` OR `acl_object` exactly like the lexical clause, so:
///  - a GRANT on the parent `acl_object` ADMITS the sub-doc's vector (the acl_object arm);
///  - a DENY on the parent `acl_object` EXCLUDES the sub-doc's vector hit — **the leak direction**
///    that fails RED on pre-fix code (`NotIds::admits` only compared the sub-precise `doc_id`, so a
///    deny expressed on the parent never matched and the DENIED sub-doc leaked into semantic/RAG);
///  - a DENY on the sub-precise `doc_id` ALSO excludes it (the `doc_id` arm stays enforced).
#[test]
fn srch_d1_vector_deny_set_both_directions_doc_id_or_acl_object() {
    let be = adversarial_corpus();
    let q = Embedding::new(vec![1.0, 0.0, 0.0]); // AT the sub-doc's embedding direction.

    // ALLOW via the parent acl_object arm: a grant on the parent admits the sub-doc's vector.
    let admit = be
        .semantic(&AclFilter::ids([SUB_DOC_PARENT]), &q, 10)
        .expect("semantic grant on parent");
    assert!(
        admit.iter().any(|h| h.doc_id == SUB_DOC_ID),
        "a grant on the parent acl_object admits the sub-doc's vector (acl_object arm)"
    );

    // LEAK DIRECTION: a deny on the parent acl_object must EXCLUDE the sub-doc's vector hit.
    let deny_parent = be
        .semantic(&AclFilter::not_ids([SUB_DOC_PARENT]), &q, 20)
        .expect("semantic deny on parent");
    assert!(
        !deny_parent.iter().any(|h| h.doc_id == SUB_DOC_ID),
        "R2.7 leak direction: a deny on the parent acl_object excludes the sub-doc's vector hit"
    );
    assert!(
        !deny_parent.is_empty(),
        "the deny is selective (other docs still surface) — not a blanket empty result"
    );

    // DOC_ID DIRECTION: a deny on the sub-precise doc_id also excludes it (the doc_id arm intact).
    let deny_docid = be
        .semantic(&AclFilter::not_ids([SUB_DOC_ID]), &q, 20)
        .expect("semantic deny on doc_id");
    assert!(
        !deny_docid.iter().any(|h| h.doc_id == SUB_DOC_ID),
        "a deny on the sub-precise doc_id excludes the sub-doc's vector hit (doc_id arm)"
    );
}

/// **SRCH-D1 (the RAG/vector half) through the PUBLIC `semantic` pipeline entry (contract 6.2 /
/// SRCH-P11).** The full path: `list_objects` → a relational `TupleSet` → the reverse-index JOIN
/// resolves ONLY PUB-1 → the vector branch runs filter-during-traversal under that conjoined ACL
/// filter. The query vector is NEAREST the confidential STRONG doc, yet it (and every other
/// confidential doc) NEVER surfaces in the semantic/RAG result — the agent-RAG retrieval is
/// permission-correct by the same pre-filter (an agent never retrieves a doc its delegated principal
/// cannot see). One list_objects + one reverse-index JOIN (no N+1).
#[test]
fn srch_d1_rag_vector_half_through_the_public_semantic_entry() {
    let be = adversarial_corpus();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
    let confidential = confidential_ids();

    // The unauthorized viewer's reachable set is a TupleSet the JOIN resolves to ONLY PUB-1.
    let authz = ScriptedAuthz::new(&VISIBLE, "z@10", 10);
    let stats = QueryStats::new();
    let cstats = myelin_search::ConsistencyStats::new();
    // The pure-semantic AST (the vector branch); the query vector is supplied directly (the agent-RAG
    // `vec` form), nearest the confidential STRONG doc `[1.0, 0.0, 0.0]`.
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
        }, // a generous page so a count-leak would show up
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

/// **SRCH-D1 — the chained grant: grant the relation → the reverse-index JOIN now resolves the
/// formerly-confidential doc → it becomes visible.** Confirms the rejection was the ACL firing, not
/// a blanket deny (a real query engine that simply returned nothing would falsely pass the leak
/// drill).
#[test]
fn srch_d1_chained_grant_makes_the_relation_doc_visible() {
    let be = adversarial_corpus();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
    let q = ast(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: var(FT_BODY_FIELD),
        rhs: lit("deadlock"),
    });

    // BEFORE: only PUB-1 reachable.
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

    // GRANT SECRET-0 (the relation now reaches it), at a FRESHER revision (11 > 10).
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

/// **SRCH-D1 — a more-results / pagination probe: a deep page never spills a hidden doc.** Even if
/// the caller pages past the visible result, no confidential doc fills the tail (the hidden docs are
/// not in the candidate set, so there is no "more" to leak).
#[test]
fn srch_d1_deep_page_never_spills_a_hidden_doc() {
    let be = adversarial_corpus();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
    let authz = ScriptedAuthz::new(&VISIBLE, "z@10", 10);
    // Page well past the single visible result.
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
        }, // skip the one visible doc — the tail must be EMPTY, not hidden docs
        &QueryStats::new(),
    )
    .expect("query");
    assert!(
        res.hits.is_empty(),
        "no 'more results' tail of hidden docs (0 leak past the visible page)"
    );
}

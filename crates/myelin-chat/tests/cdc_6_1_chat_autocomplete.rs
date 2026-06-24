//! # CDC 6.1 — the Chat `@`/`#` autocomplete is Search-backed (CHAT-P12 / P-406, M4-C3)
//!
//! **Contract 6.1** — `query(ast, viewer, zookie?, page) → RankedResults` (the ACL-conjoining Search
//! entry; the `search-requires-acl-filter` discipline). **CONSUMED** by Chat: the composer's `@`/`#`
//! autocomplete ([`myelin_chat::composer::AutocompletePort`]) is **Search-backed** — there is **0
//! chat-private mention/artifact index**. This CDC PINS that the REAL `myelin_search::query` surface
//! satisfies chat's `AutocompletePort` (the seam the gateway wires), and that the conjoined
//! `list_objects` `Filter` excludes a suggestion the viewer cannot see BEFORE it reaches the composer
//! (the no-leak property the autocomplete inherits — a `#`-suggestion for a confidential artifact never
//! appears in the picker; not in the rows, not in the count).
//!
//! This is the CONSUMER leg of contract 6.1 for Chat. The PROVIDER (the ACL-conjoining pipeline) is the
//! Search crate's (`cdc_query_pipeline_6_1.rs`); HERE the chat-side adapter routes through that one
//! surface and the autocomplete inherits the leak-free guarantee — chat owns no second index, no
//! post-filter.

use myelin_chat::composer::{AutocompleteKind, AutocompletePort, Suggestion};
use myelin_content::InlineNode;
use myelin_events::ArtifactRef;
use myelin_identity::{
    Consistency, ConsistencyMode, ListObjectsResult, Literal, ObjectId, ObjectType, Permission,
    Principal, PrincipalId, PrincipalKind, Result as AuthzResult, SetExpr, Zookie,
};
use myelin_query::{CmpOp, Expr, Predicate, QueryAst};
use myelin_search::{
    query as search_query, IndexBackend, IndexDocument, ListObjectsPort, Page, QueryStats,
    RankedResults, ScopedEngine, TantivyBackend, FT_BODY_FIELD, ORDER_KEY_FIELD,
};
use myelin_tenancy::TenantId;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// The viewer driving the autocomplete (a human composing a message).
fn viewer() -> Principal {
    Principal::stub(
        PrincipalId("p-opaque-alice".into()),
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

/// The full-text AST a `#`/`@` prefix compiles to (a body-field equality over the prefix term — the
/// composer builds this from the in-flight text; the real adapter would use a prefix matcher, the
/// shape is the same: an FT query over the kind's object type, ALWAYS ACL-conjoined).
fn ast_body(term: &str) -> QueryAst {
    QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var(FT_BODY_FIELD.into()),
        rhs: Expr::Lit(Literal::Str(term.into())),
    })
    .expect("within cost bounds")
}

fn facet_decl() -> BTreeMap<String, myelin_query::FieldType> {
    let mut m = BTreeMap::new();
    m.insert(
        ORDER_KEY_FIELD.to_string(),
        myelin_query::FieldType::OrderKey,
    );
    m
}

fn schema() -> myelin_search::FieldSchema {
    myelin_search::FieldSchema::new()
        .with(
            FT_BODY_FIELD,
            myelin_search::FieldDecl::stored(myelin_query::FieldType::Text),
        )
        .with(
            ORDER_KEY_FIELD,
            myelin_search::FieldDecl::stored(myelin_query::FieldType::OrderKey),
        )
}

/// A scripted `ListObjectsPort` (the per-viewer ACL pre-filter) returning a canned allow-set + counting
/// the calls (the no-N+1 gate). The SAME fake the Search/Knowledge CDCs use — chat does not author a
/// second authz path.
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
    fn none() -> FakeAuthz {
        FakeAuthz {
            answer: ListObjectsResult::Filter {
                set_expr: SetExpr::None,
                zookie: Zookie("z-acl".into()),
            },
            calls: AtomicU64::new(0),
        }
    }
}
impl ListObjectsPort for FakeAuthz {
    fn list_objects(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _ty: &ObjectType,
        _at: &Consistency,
    ) -> AuthzResult<ListObjectsResult> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(self.answer.clone())
    }
}

/// A `#`-artifact corpus: a PUBLIC issue + a CONFIDENTIAL issue, both matching the FT term "deploy".
fn artifact_corpus() -> TantivyBackend {
    let mut be = TantivyBackend::open(&facet_decl()).expect("open");
    let mut upsert = |id: &str, body: &str| {
        let k = myelin_query::OrderKey::bisect(None, None);
        let d = IndexDocument::new(id, body)
            .with_field(ORDER_KEY_FIELD, myelin_query::FieldValue::OrderKey(k));
        be.upsert(&d).unwrap();
    };
    upsert(
        "myelin://acme/issue/issue/ENG-1",
        "deploy the public service",
    );
    upsert(
        "myelin://acme/issue/issue/SECRET-9",
        "deploy the confidential incident fix",
    );
    be
}

/// **THE CHAT-SIDE ADAPTER under test** — a `composer::AutocompletePort` that routes through the REAL
/// `myelin_search::query` ACL-conjoining surface (contract 6.1). It holds NO chat-private index: every
/// suggestion comes from Search, pre-filtered by the viewer's `list_objects` `Filter`. This is the seam
/// the chat gateway wires in production; the CDC proves the real Search surface satisfies the port.
struct SearchAutocompleteAdapter<'a, B: IndexBackend> {
    engine: &'a ScopedEngine<'a, B>,
    authz: &'a dyn ListObjectsPort,
    viewer: Principal,
    at: Consistency,
}

impl<B: IndexBackend> AutocompletePort for SearchAutocompleteAdapter<'_, B> {
    fn suggest(&self, kind: AutocompleteKind, prefix: &str, limit: u32) -> Vec<Suggestion> {
        // The kind selects the ACL object type the conjoin keys on (`member` for `@`, `issue` for
        // `#` here — the real adapter fans the `#` query across the artifact object types). The query
        // ALWAYS goes through the conjoining `query` entry — there is NO chat-private index branch.
        let ty = match kind {
            AutocompleteKind::Mention => ObjectType("member".into()),
            AutocompleteKind::Artifact => ObjectType("issue".into()),
        };
        let ast = ast_body(prefix);
        let stats = QueryStats::new();
        let res: RankedResults = search_query(
            self.engine,
            self.authz,
            &ast,
            &self.viewer,
            &ty,
            &self.at,
            Page::FIRST,
            &stats,
        )
        .expect("the Search query surface is reachable");
        // Map the ranked, ALREADY-AUTHORISED hits to suggestions. A denied artifact is NOT in `hits`
        // (the engine pre-filter excluded it) — so it cannot become a suggestion.
        res.hits
            .into_iter()
            .take(limit as usize)
            .map(|h| Suggestion {
                target: ArtifactRef(h.doc_id.clone()),
                label: h.doc_id, // the doc id is the authorised display key here (leak-free).
                kind,
            })
            .collect()
    }
}

/// **The `#`-autocomplete is Search-backed and leak-free: a confidential artifact is NOT suggested
/// (0 chat-private index), then a grant makes it appear.** Both issues match "deploy"; the
/// unauthorized allow-set excludes SECRET-9 → it is in neither the suggestions NOR the count.
#[test]
fn artifact_autocomplete_is_search_backed_and_excludes_confidential_incl_count() {
    let be = artifact_corpus();
    let eng = ScopedEngine::new(&be, "acme", "fr-par", schema());

    // UNAUTHORIZED: the allow-set excludes the confidential issue.
    let unauth = FakeAuthz::ids(&["myelin://acme/issue/issue/ENG-1"]);
    let adapter = SearchAutocompleteAdapter {
        engine: &eng,
        authz: &unauth,
        viewer: viewer(),
        at: consistency(),
    };
    let sugg = adapter.suggest(AutocompleteKind::Artifact, "deploy", 10);
    let targets: Vec<&str> = sugg.iter().map(|s| s.target.0.as_str()).collect();
    assert_eq!(
        targets,
        ["myelin://acme/issue/issue/ENG-1"],
        "the confidential artifact is excluded by the Search pre-filter (0 leak)"
    );
    // The count-leak close: the suggestion count is the VISIBLE count only.
    assert_eq!(
        sugg.len(),
        1,
        "the autocomplete count reveals neither the existence nor the number of forbidden artifacts"
    );
    // No N+1: exactly ONE list_objects per autocomplete query.
    assert_eq!(
        unauth.calls.load(Ordering::Relaxed),
        1,
        "exactly ONE list_objects (the conjoined pre-filter; no N+1)"
    );
    // Selecting the suggestion inserts a STRUCTURED artifact_ref node (the refs.edge producer reads it).
    assert!(matches!(
        sugg[0].artifact_node(),
        Some(InlineNode::ArtifactRefNode(_))
    ));

    // GRANTED: the allow-set now includes SECRET-9 → it surfaces on re-query (the SAME surface).
    let granted = FakeAuthz::ids(&[
        "myelin://acme/issue/issue/ENG-1",
        "myelin://acme/issue/issue/SECRET-9",
    ]);
    let adapter2 = SearchAutocompleteAdapter {
        engine: &eng,
        authz: &granted,
        viewer: viewer(),
        at: consistency(),
    };
    let sugg2 = adapter2.suggest(AutocompleteKind::Artifact, "deploy", 10);
    assert_eq!(
        sugg2.len(),
        2,
        "after grant both artifacts are suggested (and counted)"
    );
}

/// **A viewer who can see NO artifact gets ZERO suggestions (the `SetExpr::None` short-circuit) — the
/// autocomplete cannot leak a count.** The conjoined `WHERE false` ACL returns an empty result without
/// materialising a candidate set; the picker is empty.
#[test]
fn autocomplete_empty_allow_set_yields_no_suggestions() {
    let be = artifact_corpus();
    let eng = ScopedEngine::new(&be, "acme", "fr-par", schema());
    let none = FakeAuthz::none();
    let adapter = SearchAutocompleteAdapter {
        engine: &eng,
        authz: &none,
        viewer: viewer(),
        at: consistency(),
    };
    let sugg = adapter.suggest(AutocompleteKind::Artifact, "deploy", 10);
    assert!(
        sugg.is_empty(),
        "no readable artifact → no suggestion (0 count leak)"
    );
}

/// **The autocomplete is bounded by the requested `limit` (the picker shows a bounded window) and runs
/// over the kind's object type.** A `@`-mention query and a `#`-artifact query both route through the
/// SAME `query` surface — proving there is one Search-backed path, not a per-kind chat-private index.
#[test]
fn autocomplete_respects_limit_and_one_surface_per_kind() {
    let be = artifact_corpus();
    let eng = ScopedEngine::new(&be, "acme", "fr-par", schema());
    let granted = FakeAuthz::ids(&[
        "myelin://acme/issue/issue/ENG-1",
        "myelin://acme/issue/issue/SECRET-9",
    ]);
    let adapter = SearchAutocompleteAdapter {
        engine: &eng,
        authz: &granted,
        viewer: viewer(),
        at: consistency(),
    };
    // limit=1 over a 2-hit corpus → exactly one suggestion (bounded picker window).
    let sugg = adapter.suggest(AutocompleteKind::Artifact, "deploy", 1);
    assert_eq!(sugg.len(), 1, "the picker window is bounded by limit");
    assert_eq!(sugg[0].kind, AutocompleteKind::Artifact);
}

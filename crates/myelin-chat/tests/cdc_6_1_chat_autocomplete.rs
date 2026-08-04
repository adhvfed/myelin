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

struct SearchAutocompleteAdapter<'a, B: IndexBackend> {
    engine: &'a ScopedEngine<'a, B>,
    authz: &'a dyn ListObjectsPort,
    viewer: Principal,
    at: Consistency,
}

impl<B: IndexBackend> AutocompletePort for SearchAutocompleteAdapter<'_, B> {
    fn suggest(&self, kind: AutocompleteKind, prefix: &str, limit: u32) -> Vec<Suggestion> {
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
        res.hits
            .into_iter()
            .take(limit as usize)
            .map(|h| Suggestion {
                target: ArtifactRef(h.doc_id.clone()),
                label: h.doc_id,
                kind,
            })
            .collect()
    }
}

#[test]
fn artifact_autocomplete_is_search_backed_and_excludes_confidential_incl_count() {
    let be = artifact_corpus();
    let eng = ScopedEngine::new(&be, "acme", "fr-par", schema());

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
    assert_eq!(
        sugg.len(),
        1,
        "the autocomplete count reveals neither the existence nor the number of forbidden artifacts"
    );
    assert_eq!(
        unauth.calls.load(Ordering::Relaxed),
        1,
        "exactly ONE list_objects (the conjoined pre-filter; no N+1)"
    );
    assert!(matches!(
        sugg[0].artifact_node(),
        Some(InlineNode::ArtifactRefNode(_))
    ));

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
    let sugg = adapter.suggest(AutocompleteKind::Artifact, "deploy", 1);
    assert_eq!(sugg.len(), 1, "the picker window is bounded by limit");
    assert_eq!(sugg[0].kind, AutocompleteKind::Artifact);
}

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use myelin_identity::{
    Consistency, ConsistencyMode, ListObjectsResult, Literal, ObjectType, Permission, Principal,
    PrincipalId, PrincipalKind, Result as AuthzResult, SetExpr, Zookie,
};
use myelin_query::{CmpOp, Expr, FieldType, FieldValue, OrderKey, Predicate, QueryAst};
use myelin_tenancy::TenantId;

use myelin_search::{
    query, FieldDecl, FieldSchema, IndexBackend, IndexDocument, ListObjectsPort, Page, QueryError,
    QueryStats, ScopedEngine, TantivyBackend, FT_BODY_FIELD, ORDER_KEY_FIELD,
};

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

fn tenant_index(tenant: &str) -> TantivyBackend {
    let mut be = TantivyBackend::open(&facet_decl()).expect("open");
    let k = OrderKey::bisect(None, None);
    be.upsert(
        &IndexDocument::new(
            format!("{tenant}/issue/ENG-1"),
            "confidential deadlock note",
        )
        .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k)),
    )
    .unwrap();
    be
}

fn viewer(tenant: &str) -> Principal {
    Principal::stub(
        PrincipalId("p:mallory".into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    )
}

fn consistency() -> Consistency {
    Consistency {
        at_least: Zookie("z0".into()),
        mode: ConsistencyMode::BoundedStale,
    }
}

fn ast() -> QueryAst {
    QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var(FT_BODY_FIELD.into()),
        rhs: Expr::Lit(Literal::Str("deadlock".into())),
    })
    .expect("within cost bounds")
}

struct AllowAllAuthz {
    calls: AtomicU64,
}
impl ListObjectsPort for AllowAllAuthz {
    fn list_objects(
        &self,
        _s: &Principal,
        _p: &Permission,
        _t: &ObjectType,
        _a: &Consistency,
    ) -> AuthzResult<ListObjectsResult> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(ListObjectsResult::Filter {
            set_expr: SetExpr::All,
            zookie: Zookie("z".into()),
        })
    }
}

#[test]
fn srch_d3_cross_tenant_idor_is_zero() {
    let acme = tenant_index("acme");
    let acme_engine = ScopedEngine::new(&acme, "acme", "eu-west", schema());

    let mallory = viewer("evil");
    let authz = AllowAllAuthz {
        calls: AtomicU64::new(0),
    };
    let stats = QueryStats::new();
    let res = query(
        &acme_engine,
        &authz,
        &ast(),
        &mallory,
        &ObjectType("issue".into()),
        &consistency(),
        Page::FIRST,
        &stats,
    );
    let err = res.expect_err("a cross-tenant query MUST be rejected (SRCH-D3)");
    assert!(
        matches!(err, QueryError::TenantMismatch { .. }),
        "cross-tenant ⇒ TenantMismatch: {err}"
    );
    assert_eq!(
        authz.calls.load(Ordering::Relaxed),
        0,
        "0 list_objects calls (rejected first)"
    );
    assert_eq!(
        stats.list_objects_calls(),
        0,
        "the no-N+1 counter saw 0 (no authz consulted)"
    );
    assert_eq!(
        stats.engine_branches(),
        0,
        "0 engine branches - acme's index is never touched"
    );

    let evil = tenant_index("evil");
    let evil_engine = ScopedEngine::new(&evil, "evil", "eu-west", schema());
    let authz2 = AllowAllAuthz {
        calls: AtomicU64::new(0),
    };
    let stats2 = QueryStats::new();
    let res2 = query(
        &evil_engine,
        &authz2,
        &ast(),
        &mallory,
        &ObjectType("issue".into()),
        &consistency(),
        Page::FIRST,
        &stats2,
    )
    .expect("the same-tenant viewer is admitted");
    assert_eq!(
        res2.hits.iter().map(|h| h.doc_id.as_str()).collect::<Vec<_>>(),
        ["evil/issue/ENG-1"],
        "the same-tenant viewer sees ONLY its own tenant's doc (never acme's colliding-namespace doc)"
    );
    assert!(
        !res2.hits.iter().any(|h| h.doc_id.starts_with("acme/")),
        "0 acme docs reachable from the evil tenant's query"
    );
}

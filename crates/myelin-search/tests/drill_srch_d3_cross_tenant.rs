//! # Drill — SRCH-D3 cross-tenant IDOR = 0 (F2) (SRCH-P08 → P-171)
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` SRCH-D3 (F2,
//! cross-tenant IDOR — spoof the path-tenant ⇒ 0 cross-tenant results; tenant from the verified
//! token, the partition key (tenant, region) enforced). **Architecture:** `search-and-indexing.md`
//! §4.2 (the permission-aware query pipeline; the tenant from the verified token, never the URL
//! path; the partition key (tenant, region)) + §3.4 (the per-tenant index, partition-keyed).
//!
//! ## What this drill proves (the dated green artifact, 2026-06-20)
//! Two tenants (`acme`, `evil`) each index a document under a colliding doc-id namespace. A viewer
//! whose **verified** tenant is `evil` queries against `acme`'s per-tenant index. The query pipeline
//! REJECTS it (`TenantMismatch`) — **0 cross-tenant results**, with **0** `list_objects` calls and
//! **0** engine touches (rejected at the partition-key check, before any authz or engine work).
//! Crucially: there is **no path/tenant parameter** to spoof — the tenant is `viewer.tenant` (the
//! verified principal), and the engine is the wrong tenant's index. Spoofing cannot reach another
//! tenant's documents.
//!
//! And the positive control: the SAME viewer querying its OWN tenant's index sees its own doc — so
//! the rejection is the cross-tenant guard firing, not a blanket deny that would mask a real bug.
//!
//! ## Floors named
//! - The **relational** `SetExpr` reverse-index JOIN forms + the full zero-escape leak drill SRCH-D1
//!   across an adversarial corpus are the sibling slice **SRCH-P09** (P-172).
//! - The **zookie/consistency** no-stale-grant + fail-static bypass is **SRCH-P10** (P-173).
//! - The synthetic per-tenant facet schema is the named M3/M4 floor (real per-subsystem IndexSpecs).

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

/// A per-tenant index holding a single doc whose doc-id namespace COLLIDES across tenants
/// (`<tenant>/issue/ENG-1`) — so the only thing keeping them apart is the partition key, not a
/// lucky id difference.
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

/// An "allow-everything" authz double — the WORST case for the guard: even if authz says "see all",
/// the partition-key check must still confine the query to one tenant.
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

/// **SRCH-D3 (F2): the cross-tenant query yields 0 results — rejected at the partition-key check,
/// with 0 authz calls and 0 engine touches; and the same-tenant control sees its own doc.**
#[test]
fn srch_d3_cross_tenant_idor_is_zero() {
    let acme = tenant_index("acme");
    let acme_engine = ScopedEngine::new(&acme, "acme", "eu-west", schema());

    // --- the attack: a viewer verified as tenant `evil` queries acme's index ---
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
        matches!(err, QueryError::TenantMismatch),
        "cross-tenant ⇒ TenantMismatch: {err}"
    );
    // THE DRILL ARTIFACT: 0 cross-tenant results, AND the guard fired before any authz/engine work.
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
        "0 engine branches — acme's index is never touched"
    );

    // --- the positive control: the SAME id namespace, but a viewer of the RIGHT tenant sees it ---
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
        &mallory, // tenant `evil` — matches the evil index
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
    // And acme's doc-id with the same suffix is NOT in the evil viewer's results (the partition
    // confines the index — the colliding namespace does not leak across tenants).
    assert!(
        !res2.hits.iter().any(|h| h.doc_id.starts_with("acme/")),
        "0 acme docs reachable from the evil tenant's query"
    );
}

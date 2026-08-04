use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use myelin_identity::{
    Consistency, ConsistencyMode, ListObjectsResult, Literal, ObjectId, ObjectType, Permission,
    Principal, PrincipalId, PrincipalKind, Result as AuthzResult, Zookie,
};
use myelin_query::{CmpOp, Expr, FieldType, FieldValue, OrderKey, Predicate, QueryAst};
use myelin_tenancy::TenantId;

use myelin_search::{
    query_consistent, BoundedCheckPort, ConsistencyStats, FieldDecl, FieldSchema, IndexDocument,
    Page, QueryStats, ScopedEngine, TantivyBackend, FT_BODY_FIELD, ORDER_KEY_FIELD,
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

fn doc(id: &str, text: &str) -> IndexDocument {
    let k = OrderKey::bisect(None, None);
    IndexDocument::new(id, text).with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k))
}

fn viewer() -> Principal {
    Principal::stub(
        PrincipalId("p:alice".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn strong_at(rev: u64) -> Consistency {
    Consistency {
        at_least: Zookie(format!("z@{rev}")),
        mode: ConsistencyMode::Strong,
    }
}

fn bounded_at(rev: u64) -> Consistency {
    Consistency {
        at_least: Zookie(format!("z@{rev}")),
        mode: ConsistencyMode::BoundedStale,
    }
}

fn ast(p: Predicate) -> QueryAst {
    QueryAst::compiled(p).expect("within cost bounds")
}
fn var(name: &str) -> Expr {
    Expr::Var(name.into())
}
fn s(v: &str) -> Expr {
    Expr::Lit(Literal::Str(v.into()))
}

struct FixedAuthz {
    ids: Vec<&'static str>,
    zookie: &'static str,
    calls: AtomicU64,
}
impl FixedAuthz {
    fn new(ids: &[&'static str], zookie: &'static str) -> FixedAuthz {
        FixedAuthz {
            ids: ids.to_vec(),
            zookie,
            calls: AtomicU64::new(0),
        }
    }
}
impl myelin_search::ListObjectsPort for FixedAuthz {
    fn list_objects(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _ty: &ObjectType,
        _at: &Consistency,
    ) -> AuthzResult<ListObjectsResult> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(ListObjectsResult::Ids {
            ids: self.ids.iter().map(|i| ObjectId((*i).into())).collect(),
            zookie: Zookie(self.zookie.into()),
        })
    }
}

struct Revoker {
    revoked: Vec<&'static str>,
    checks: AtomicU64,
}
impl Revoker {
    fn new(revoked: &[&'static str]) -> Revoker {
        Revoker {
            revoked: revoked.to_vec(),
            checks: AtomicU64::new(0),
        }
    }
}
impl BoundedCheckPort for Revoker {
    fn check(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        object: &ObjectId,
        _at: &Consistency,
    ) -> AuthzResult<bool> {
        self.checks.fetch_add(1, Ordering::Relaxed);
        Ok(!self.revoked.iter().any(|r| *r == object.0))
    }
}

#[test]
fn srch_d2_zero_stale_allow_with_zookie() {
    let mut be = TantivyBackend::open(&facet_decl()).expect("open");
    be.upsert_stamped(
        &doc("acme/issue/SECRET-9", "deadlock secret incident"),
        "z@5",
        5,
    )
    .unwrap();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());

    let authz = FixedAuthz::new(&["acme/issue/SECRET-9"], "z-acl");
    let revoker = Revoker::new(&["acme/issue/SECRET-9"]);
    let q = ast(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: var(FT_BODY_FIELD),
        rhs: s("deadlock"),
    });

    let stats = QueryStats::new();
    let cstats = ConsistencyStats::new();
    let res = query_consistent(
        &eng,
        &authz,
        Some(&revoker),
        &q,
        &viewer(),
        &ObjectType("issue".into()),
        &strong_at(9),
        Page::FIRST,
        &stats,
        &cstats,
    )
    .expect("query");

    assert!(
        res.hits.is_empty(),
        "0 stale-allow: the revoked doc is EXCLUDED, never served stale"
    );
    assert_eq!(
        cstats.revalidated(),
        1,
        "exactly the ONE stale candidate was re-validated (no N+1)"
    );
    assert_eq!(
        cstats.excluded_stale(),
        1,
        "the zero-escape-under-staleness counter: 1 excluded"
    );
    assert_eq!(
        revoker.checks.load(Ordering::Relaxed),
        1,
        "exactly one bounded check (affected set)"
    );
    assert_eq!(
        stats.list_objects_calls(),
        1,
        "still exactly one list_objects (no N+1)"
    );
    assert_eq!(
        cstats.fail_static_bypassed(),
        1,
        "a zookie-stamped read BYPASSES the fail-static cache"
    );
}

#[test]
fn srch_d2_chained_grant_then_revoke_excludes() {
    let mut be = TantivyBackend::open(&facet_decl()).expect("open");
    be.upsert_stamped(
        &doc("acme/issue/SECRET-9", "deadlock secret incident"),
        "z@5",
        5,
    )
    .unwrap();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
    let q = ast(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: var(FT_BODY_FIELD),
        rhs: s("deadlock"),
    });
    let authz = FixedAuthz::new(&["acme/issue/SECRET-9"], "z-acl");

    let granted = Revoker::new(&[]);
    let cstats1 = ConsistencyStats::new();
    let res1 = query_consistent(
        &eng,
        &authz,
        Some(&granted),
        &q,
        &viewer(),
        &ObjectType("issue".into()),
        &strong_at(9),
        Page::FIRST,
        &QueryStats::new(),
        &cstats1,
    )
    .expect("q1");
    assert_eq!(
        res1.hits
            .iter()
            .map(|h| h.doc_id.as_str())
            .collect::<Vec<_>>(),
        ["acme/issue/SECRET-9"],
        "a stale-but-STILL-granted candidate is re-validated and surfaces"
    );
    assert_eq!(
        cstats1.revalidated(),
        1,
        "the stale candidate was re-validated"
    );
    assert_eq!(
        cstats1.excluded_stale(),
        0,
        "and admitted (the grant survived the snapshot)"
    );

    let revoked = Revoker::new(&["acme/issue/SECRET-9"]);
    let cstats2 = ConsistencyStats::new();
    let res2 = query_consistent(
        &eng,
        &authz,
        Some(&revoked),
        &q,
        &viewer(),
        &ObjectType("issue".into()),
        &strong_at(9),
        Page::FIRST,
        &QueryStats::new(),
        &cstats2,
    )
    .expect("q2");
    assert!(
        res2.hits.is_empty(),
        "after the revoke the doc is EXCLUDED (the new-enemy is kept out)"
    );
    assert_eq!(cstats2.excluded_stale(), 1, "the zero-escape counter fired");
}

#[test]
fn srch_d2_only_stale_candidates_are_revalidated() {
    let mut be = TantivyBackend::open(&facet_decl()).expect("open");
    be.upsert_stamped(&doc("acme/issue/FRESH-1", "deadlock fresh"), "z@9", 9)
        .unwrap();
    be.upsert_stamped(&doc("acme/issue/STALE-2", "deadlock stale"), "z@4", 4)
        .unwrap();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());

    let authz = FixedAuthz::new(&["acme/issue/FRESH-1", "acme/issue/STALE-2"], "z-acl");
    let allow = Revoker::new(&[]);
    let q = ast(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: var(FT_BODY_FIELD),
        rhs: s("deadlock"),
    });
    let cstats = ConsistencyStats::new();
    let res = query_consistent(
        &eng,
        &authz,
        Some(&allow),
        &q,
        &viewer(),
        &ObjectType("issue".into()),
        &strong_at(9),
        Page::FIRST,
        &QueryStats::new(),
        &cstats,
    )
    .expect("query");

    let ids: std::collections::BTreeSet<&str> =
        res.hits.iter().map(|h| h.doc_id.as_str()).collect();
    assert!(
        ids.contains("acme/issue/FRESH-1") && ids.contains("acme/issue/STALE-2"),
        "both visible: {ids:?}"
    );
    assert_eq!(
        cstats.revalidated(),
        1,
        "ONLY the stale doc was re-validated (the fresh one is served as-is)"
    );
    assert_eq!(
        allow.checks.load(Ordering::Relaxed),
        1,
        "exactly one bounded check - the affected set, no N+1"
    );
}

#[test]
fn srch_d2_fail_static_bounded_degrades_strong_bypasses() {
    let mut be = TantivyBackend::open(&facet_decl()).expect("open");
    be.upsert_stamped(&doc("acme/issue/PUB-1", "deadlock public"), "z@5", 5)
        .unwrap();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
    let authz = FixedAuthz::new(&["acme/issue/PUB-1"], "z-acl");
    let q = ast(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: var(FT_BODY_FIELD),
        rhs: s("deadlock"),
    });

    let revoker = Revoker::new(&["acme/issue/PUB-1"]);
    let cstats_b = ConsistencyStats::new();
    let res_b = query_consistent(
        &eng,
        &authz,
        Some(&revoker),
        &q,
        &viewer(),
        &ObjectType("issue".into()),
        &bounded_at(5),
        Page::FIRST,
        &QueryStats::new(),
        &cstats_b,
    )
    .expect("bounded");
    assert_eq!(
        res_b
            .hits
            .iter()
            .map(|h| h.doc_id.as_str())
            .collect::<Vec<_>>(),
        ["acme/issue/PUB-1"],
        "default-consistency uses the cached/indexed filter (degrade-not-cascade)"
    );
    assert_eq!(
        cstats_b.fail_static_served(),
        1,
        "the BoundedStale read used the fail-static path"
    );
    assert_eq!(
        cstats_b.fail_static_bypassed(),
        0,
        "a default-consistency read does NOT bypass"
    );
    assert_eq!(
        cstats_b.revalidated(),
        0,
        "nothing stale at the indexed watermark - no re-check"
    );

    let revoker2 = Revoker::new(&["acme/issue/PUB-1"]);
    let cstats_s = ConsistencyStats::new();
    let res_s = query_consistent(
        &eng,
        &authz,
        Some(&revoker2),
        &q,
        &viewer(),
        &ObjectType("issue".into()),
        &strong_at(9),
        Page::FIRST,
        &QueryStats::new(),
        &cstats_s,
    )
    .expect("strong");
    assert!(
        res_s.hits.is_empty(),
        "the strong read sees the revocation (the new-enemy is excluded)"
    );
    assert_eq!(
        cstats_s.fail_static_bypassed(),
        1,
        "the zookie-stamped read BYPASSED the fail-static cache"
    );
    assert_eq!(
        cstats_s.fail_static_served(),
        0,
        "and did NOT degrade on a stale grant"
    );
}

#[test]
fn srch_d2_no_check_port_excludes_stale_fail_closed() {
    let mut be = TantivyBackend::open(&facet_decl()).expect("open");
    be.upsert_stamped(&doc("acme/issue/SECRET-9", "deadlock secret"), "z@5", 5)
        .unwrap();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
    let authz = FixedAuthz::new(&["acme/issue/SECRET-9"], "z-acl");
    let q = ast(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: var(FT_BODY_FIELD),
        rhs: s("deadlock"),
    });

    let cstats = ConsistencyStats::new();
    let res = query_consistent(
        &eng,
        &authz,
        None,
        &q,
        &viewer(),
        &ObjectType("issue".into()),
        &strong_at(9),
        Page::FIRST,
        &QueryStats::new(),
        &cstats,
    )
    .expect("query");
    assert!(
        res.hits.is_empty(),
        "fail CLOSED: the stale candidate is excluded pending re-index"
    );
    assert_eq!(
        cstats.excluded_stale(),
        1,
        "the zero-escape counter fired without a check port"
    );
    assert_eq!(
        cstats.revalidated(),
        0,
        "no bounded check could run (none wired) - excluded, not admitted"
    );
}

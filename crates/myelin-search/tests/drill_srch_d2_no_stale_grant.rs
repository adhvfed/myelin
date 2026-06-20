//! # Drill — SRCH-D2 the no-stale-grant zero-escape-under-staleness = 0 (F1/F8) (SRCH-P10 → P-173)
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` SRCH-D2 (F1/F8):
//! *Revoke a grant, re-search with the post-revoke zookie → excluded (the zookie bypasses
//! fail-static + honours the reverse-index revision watermark); default-consistency search excludes
//! within W ≤ revocation SLA. Gate: 0 stale-allow with zookie; ≤ W without.* **Architecture:**
//! `search-and-indexing.md` §4.2.3 (the consistency clause: a candidate whose `indexed_zookie` is
//! older than the passed zookie is re-validated via a bounded `check` on the affected candidates
//! only, or excluded pending re-index — NEVER served stale-allow; zookie-stamped queries bypass the
//! fail-static cache; default-consistency may use the cached filter during an Id hiccup, bounded
//! staleness ≤ W). Contracts 4.10 (zookie/consistency + revision watermark), 1.10 (FailStatic), 4.2
//! (the bounded `check`), 6.1 (query forwards the zookie).
//!
//! ## What this drill proves (the dated green artifact, 2026-06-20)
//! 1. **0 stale-allow with a zookie (the new-enemy):** a doc indexed under an OLD ACL projection
//!    (its `indexed_zookie` predates the post-revoke query zookie) is re-validated by the bounded
//!    `check` at the demanded snapshot — the `check` DENIES (the grant is revoked) → the doc is
//!    EXCLUDED. It NEVER surfaces stale-allow, even though the index projection still carries it.
//! 2. **the chained grant→search→revoke→re-search ladder:** with the grant the doc is visible;
//!    after the revoke (re-search with a newer zookie + a denying bounded check) it is excluded —
//!    proving the exclusion is the new-enemy guard firing, not a blanket deny.
//! 3. **a STILL-granted stale candidate is admitted:** a doc indexed under an old zookie whose grant
//!    SURVIVES the snapshot is re-validated and surfaces (the re-validation admits as well as
//!    excludes — it is a real consistency check, not a deny-everything-stale).
//! 4. **the bounded affected set (no N+1):** only the STALE candidates are re-validated (the
//!    `revalidated` counter == the stale count, never every hit); fresh candidates are served as-is.
//! 5. **fail-static degrade-not-cascade:** a default-consistency (BoundedStale) query during an Id
//!    hiccup uses the cached/indexed filter (no cascade); a zookie-stamped (Strong) query BYPASSES
//!    the fail-static cache (it must see the revocation). The fail-static ratio telemetry fires.
//!
//! ## Floors named
//! - The full at-scale assertion (the revocation-SLA load drill) is unchanged at M5 (the prompt's
//!   "FLOOR named: none new").
//! - The hybrid/vector RRF fusion (SRCH-P11 / P-174) reuses this SAME zookie path for the semantic
//!   surface (no-stale-grant for RAG too) — named, not duplicated here.
//! - The synthetic per-tenant facet schema is the named M3/M4 floor (real per-subsystem IndexSpecs).

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

// ---- fixtures --------------------------------------------------------------

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
    Principal::stub(PrincipalId("p:alice".into()), PrincipalKind::Human, TenantId("acme".into()))
}

/// A zookie-stamped STRONG read at watermark `rev` (read-your-writes — bypasses fail-static).
fn strong_at(rev: u64) -> Consistency {
    Consistency { at_least: Zookie(format!("z@{rev}")), mode: ConsistencyMode::Strong }
}

/// A default-consistency BoundedStale read at watermark `rev` (fail-static eligible).
fn bounded_at(rev: u64) -> Consistency {
    Consistency { at_least: Zookie(format!("z@{rev}")), mode: ConsistencyMode::BoundedStale }
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

/// A `list_objects` port that returns a fixed allow-set (the STALE ACL projection — it still
/// carries the doc the source has since revoked). Counts calls (the no-N+1 GATE).
struct FixedAuthz {
    ids: Vec<&'static str>,
    zookie: &'static str,
    calls: AtomicU64,
}
impl FixedAuthz {
    fn new(ids: &[&'static str], zookie: &'static str) -> FixedAuthz {
        FixedAuthz { ids: ids.to_vec(), zookie, calls: AtomicU64::new(0) }
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

/// The bounded re-validation port (contract 4.2): a per-object `check` at the demanded snapshot. It
/// holds the set of objects REVOKED at the snapshot — a revoked object `check`s DENY (the
/// new-enemy), every other object `check`s ALLOW. Counts re-validations so the bounded-affected-set
/// (no N+1) property is provable.
struct Revoker {
    revoked: Vec<&'static str>,
    checks: AtomicU64,
}
impl Revoker {
    fn new(revoked: &[&'static str]) -> Revoker {
        Revoker { revoked: revoked.to_vec(), checks: AtomicU64::new(0) }
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

// ---- the drill -------------------------------------------------------------

/// **SRCH-D2 (F1/F8): 0 stale-allow with a zookie — the new-enemy is excluded.** The doc is indexed
/// under an OLD `indexed_zookie` (rev 5) and the STALE ACL projection still lists it; the post-revoke
/// query carries a NEWER zookie (rev 9). The candidate is stale (5 < 9) → re-validated by the bounded
/// `check`, which DENIES (revoked) → EXCLUDED. 0 stale-allow.
#[test]
fn srch_d2_zero_stale_allow_with_zookie() {
    let mut be = TantivyBackend::open(&facet_decl()).expect("open");
    // The doc was indexed under the OLD ACL projection (zookie z@5) — before the revoke.
    be.upsert_stamped(&doc("acme/issue/SECRET-9", "deadlock secret incident"), "z@5", 5)
        .unwrap();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());

    // The STALE list_objects projection STILL lists SECRET-9 (the revoke has not re-indexed yet).
    let authz = FixedAuthz::new(&["acme/issue/SECRET-9"], "z-acl");
    // The source has REVOKED the grant — the bounded check denies at the demanded snapshot.
    let revoker = Revoker::new(&["acme/issue/SECRET-9"]);
    let q = ast(Predicate::Cmp { op: CmpOp::Eq, lhs: var(FT_BODY_FIELD), rhs: s("deadlock") });

    let stats = QueryStats::new();
    let cstats = ConsistencyStats::new();
    // The post-revoke read carries a NEWER zookie (rev 9 > the doc's indexed rev 5).
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

    assert!(res.hits.is_empty(), "0 stale-allow: the revoked doc is EXCLUDED, never served stale");
    assert_eq!(cstats.revalidated(), 1, "exactly the ONE stale candidate was re-validated (no N+1)");
    assert_eq!(cstats.excluded_stale(), 1, "the zero-escape-under-staleness counter: 1 excluded");
    assert_eq!(revoker.checks.load(Ordering::Relaxed), 1, "exactly one bounded check (affected set)");
    assert_eq!(stats.list_objects_calls(), 1, "still exactly one list_objects (no N+1)");
    assert_eq!(cstats.fail_static_bypassed(), 1, "a zookie-stamped read BYPASSES the fail-static cache");
}

/// **The chained grant→search→revoke→re-search ladder.** Before the revoke (the bounded check
/// ALLOWS), the doc is visible; after the revoke (the bounded check DENIES at the newer zookie), it
/// is excluded — proving the rejection is the new-enemy guard firing, not a blanket deny.
#[test]
fn srch_d2_chained_grant_then_revoke_excludes() {
    let mut be = TantivyBackend::open(&facet_decl()).expect("open");
    be.upsert_stamped(&doc("acme/issue/SECRET-9", "deadlock secret incident"), "z@5", 5)
        .unwrap();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
    let q = ast(Predicate::Cmp { op: CmpOp::Eq, lhs: var(FT_BODY_FIELD), rhs: s("deadlock") });
    let authz = FixedAuthz::new(&["acme/issue/SECRET-9"], "z-acl");

    // GRANTED: the bounded check ALLOWS (nothing revoked) → the stale candidate is re-validated and
    // SURFACES (a stale-but-still-granted doc is admitted, not deny-everything-stale).
    let granted = Revoker::new(&[]);
    let cstats1 = ConsistencyStats::new();
    let res1 = query_consistent(
        &eng, &authz, Some(&granted), &q, &viewer(), &ObjectType("issue".into()),
        &strong_at(9), Page::FIRST, &QueryStats::new(), &cstats1,
    )
    .expect("q1");
    assert_eq!(
        res1.hits.iter().map(|h| h.doc_id.as_str()).collect::<Vec<_>>(),
        ["acme/issue/SECRET-9"],
        "a stale-but-STILL-granted candidate is re-validated and surfaces"
    );
    assert_eq!(cstats1.revalidated(), 1, "the stale candidate was re-validated");
    assert_eq!(cstats1.excluded_stale(), 0, "and admitted (the grant survived the snapshot)");

    // REVOKED: the bounded check now DENIES → the SAME stale candidate is excluded.
    let revoked = Revoker::new(&["acme/issue/SECRET-9"]);
    let cstats2 = ConsistencyStats::new();
    let res2 = query_consistent(
        &eng, &authz, Some(&revoked), &q, &viewer(), &ObjectType("issue".into()),
        &strong_at(9), Page::FIRST, &QueryStats::new(), &cstats2,
    )
    .expect("q2");
    assert!(res2.hits.is_empty(), "after the revoke the doc is EXCLUDED (the new-enemy is kept out)");
    assert_eq!(cstats2.excluded_stale(), 1, "the zero-escape counter fired");
}

/// **The bounded affected set (no N+1): ONLY stale candidates are re-validated.** A corpus with one
/// FRESH doc (indexed at the query zookie) + one STALE doc (indexed before it): exactly ONE bounded
/// check runs (the stale one); the fresh doc is served as-is without any check.
#[test]
fn srch_d2_only_stale_candidates_are_revalidated() {
    let mut be = TantivyBackend::open(&facet_decl()).expect("open");
    // FRESH: indexed at z@9 (== the query zookie) — its ACL projection reflects the snapshot.
    be.upsert_stamped(&doc("acme/issue/FRESH-1", "deadlock fresh"), "z@9", 9).unwrap();
    // STALE: indexed at z@4 (< the query zookie) — re-validated.
    be.upsert_stamped(&doc("acme/issue/STALE-2", "deadlock stale"), "z@4", 4).unwrap();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());

    let authz = FixedAuthz::new(&["acme/issue/FRESH-1", "acme/issue/STALE-2"], "z-acl");
    // Nothing revoked — the stale candidate's grant survives, so both surface.
    let allow = Revoker::new(&[]);
    let q = ast(Predicate::Cmp { op: CmpOp::Eq, lhs: var(FT_BODY_FIELD), rhs: s("deadlock") });
    let cstats = ConsistencyStats::new();
    let res = query_consistent(
        &eng, &authz, Some(&allow), &q, &viewer(), &ObjectType("issue".into()),
        &strong_at(9), Page::FIRST, &QueryStats::new(), &cstats,
    )
    .expect("query");

    let ids: std::collections::BTreeSet<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
    assert!(ids.contains("acme/issue/FRESH-1") && ids.contains("acme/issue/STALE-2"), "both visible: {ids:?}");
    assert_eq!(cstats.revalidated(), 1, "ONLY the stale doc was re-validated (the fresh one is served as-is)");
    assert_eq!(allow.checks.load(Ordering::Relaxed), 1, "exactly one bounded check — the affected set, no N+1");
}

/// **Fail-static degrade-not-cascade: a default-consistency (BoundedStale) read does NOT bypass the
/// fail-static cache and finds nothing stale (it uses the indexed filter as-is, bounded staleness ≤
/// W); a zookie-stamped (Strong) read DOES bypass.** Proves the consistency-mode split (4.10/1.10).
#[test]
fn srch_d2_fail_static_bounded_degrades_strong_bypasses() {
    let mut be = TantivyBackend::open(&facet_decl()).expect("open");
    be.upsert_stamped(&doc("acme/issue/PUB-1", "deadlock public"), "z@5", 5).unwrap();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
    let authz = FixedAuthz::new(&["acme/issue/PUB-1"], "z-acl");
    let q = ast(Predicate::Cmp { op: CmpOp::Eq, lhs: var(FT_BODY_FIELD), rhs: s("deadlock") });

    // DEFAULT-CONSISTENCY (BoundedStale) at rev 5: the doc indexed at z@5 is NOT stale (5 == 5) — it
    // is served from the indexed filter (degrade-not-cascade, no re-validation, no cascade). No
    // bounded check needed even though one is wired.
    let revoker = Revoker::new(&["acme/issue/PUB-1"]);
    let cstats_b = ConsistencyStats::new();
    let res_b = query_consistent(
        &eng, &authz, Some(&revoker), &q, &viewer(), &ObjectType("issue".into()),
        &bounded_at(5), Page::FIRST, &QueryStats::new(), &cstats_b,
    )
    .expect("bounded");
    assert_eq!(
        res_b.hits.iter().map(|h| h.doc_id.as_str()).collect::<Vec<_>>(),
        ["acme/issue/PUB-1"],
        "default-consistency uses the cached/indexed filter (degrade-not-cascade)"
    );
    assert_eq!(cstats_b.fail_static_served(), 1, "the BoundedStale read used the fail-static path");
    assert_eq!(cstats_b.fail_static_bypassed(), 0, "a default-consistency read does NOT bypass");
    assert_eq!(cstats_b.revalidated(), 0, "nothing stale at the indexed watermark — no re-check");

    // ZOOKIE-STAMPED (Strong) at a NEWER rev 9: the SAME doc (indexed at z@5) is now stale (5 < 9) →
    // re-validated → the revoke DENIES → excluded. The strong read BYPASSES the fail-static cache.
    let revoker2 = Revoker::new(&["acme/issue/PUB-1"]);
    let cstats_s = ConsistencyStats::new();
    let res_s = query_consistent(
        &eng, &authz, Some(&revoker2), &q, &viewer(), &ObjectType("issue".into()),
        &strong_at(9), Page::FIRST, &QueryStats::new(), &cstats_s,
    )
    .expect("strong");
    assert!(res_s.hits.is_empty(), "the strong read sees the revocation (the new-enemy is excluded)");
    assert_eq!(cstats_s.fail_static_bypassed(), 1, "the zookie-stamped read BYPASSED the fail-static cache");
    assert_eq!(cstats_s.fail_static_served(), 0, "and did NOT degrade on a stale grant");
}

/// **No check port wired → a stale candidate is EXCLUDED pending re-index (fail CLOSED, ADR-03).**
/// The safe default the plain `query` entry uses: never served stale-allow even without a `check`.
#[test]
fn srch_d2_no_check_port_excludes_stale_fail_closed() {
    let mut be = TantivyBackend::open(&facet_decl()).expect("open");
    be.upsert_stamped(&doc("acme/issue/SECRET-9", "deadlock secret"), "z@5", 5).unwrap();
    let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
    let authz = FixedAuthz::new(&["acme/issue/SECRET-9"], "z-acl");
    let q = ast(Predicate::Cmp { op: CmpOp::Eq, lhs: var(FT_BODY_FIELD), rhs: s("deadlock") });

    let cstats = ConsistencyStats::new();
    let res = query_consistent(
        &eng,
        &authz,
        None, // NO bounded-check port
        &q,
        &viewer(),
        &ObjectType("issue".into()),
        &strong_at(9), // stale relative to the doc's z@5
        Page::FIRST,
        &QueryStats::new(),
        &cstats,
    )
    .expect("query");
    assert!(res.hits.is_empty(), "fail CLOSED: the stale candidate is excluded pending re-index");
    assert_eq!(cstats.excluded_stale(), 1, "the zero-escape counter fired without a check port");
    assert_eq!(cstats.revalidated(), 0, "no bounded check could run (none wired) — excluded, not admitted");
}

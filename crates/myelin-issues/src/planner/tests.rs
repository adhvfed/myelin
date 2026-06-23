//! Unit tests for the ISS-P13 AST→OLTP-store compiler — the `SetExpr` → SQL JOIN over `issue.id`,
//! the one-query / no-post-filter conjoin, the CONFIDENTIAL set-difference exclusion (the ISS-D3 leak
//! crux), the leak-free evaluation, and the new-enemy zookie guard. The chained-mutation e2e + the
//! ISS-D3 drill scenario are the `tests/` integration files; the live-Postgres proof is the
//! `--features integration` test (`tests/integration_iss_p13_setexpr_pushdown.rs`). These are the
//! deterministic, DB-free unit drills (the `SetExpr`-lowering path is mandatory-core — a leak is the
//! failure; the cargo-mutants floor is stated in the module doc / report).

use super::*;
use myelin_identity::{AuthzIndexRef, PrincipalId, PrincipalKind, RelName};

fn viewer(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn oid(s: &str) -> ObjectId {
    ObjectId(s.into())
}

// ───────────────────────────── the lowering (§3 — via_column = issue.id) ──────────────────────────

/// **`All` → `TRUE`**; **`None` → `FALSE`** (admin sees all; deny is `WHERE false`, never permissive).
#[test]
fn all_and_none_lower_to_true_and_false() {
    let v = viewer("u");
    assert_eq!(lower_over_issue_id(&SetExpr::All, &v).sql_predicate, "TRUE");
    assert_eq!(
        lower_over_issue_id(&SetExpr::None, &v).sql_predicate,
        "FALSE"
    );
}

/// **`Ids` → `issue.id IN (:p0, …)`** with the ids BOUND (never interpolated literals).
#[test]
fn ids_lowers_to_in_over_issue_id_with_bound_params() {
    let v = viewer("u");
    let lowered = lower_over_issue_id(&SetExpr::Ids(vec![oid("ENG-1"), oid("ENG-2")]), &v);
    assert_eq!(lowered.sql_predicate, "issue.id IN (:id_0, :id_1)");
    // The literal ids live in params, bound — never in the SQL text (injection-safe).
    assert_eq!(lowered.params.len(), 2);
    assert_eq!(lowered.params[0].value, "ENG-1");
    assert_eq!(lowered.params[1].value, "ENG-2");
    assert!(!lowered.sql_predicate.contains("ENG-1"));
    assert_eq!(lowered.filter_mode(), FilterMode::Ids);
    assert!(!lowered.depends_on_reverse_index());
}

/// **An empty `Ids` allow-set → `FALSE`** (NEVER a permissive `TRUE` — the leak-free identity).
#[test]
fn empty_ids_lowers_to_false_never_permissive() {
    let v = viewer("u");
    assert_eq!(
        lower_over_issue_id(&SetExpr::Ids(vec![]), &v).sql_predicate,
        "FALSE"
    );
}

/// **`NotIds` → `issue.id NOT IN (…)`**; an empty deny-set excludes nothing → `TRUE`.
#[test]
fn not_ids_lowers_to_not_in_over_issue_id() {
    let v = viewer("u");
    let lowered = lower_over_issue_id(&SetExpr::NotIds(vec![oid("ENG-9")]), &v);
    assert_eq!(lowered.sql_predicate, "issue.id NOT IN (:id_0)");
    assert_eq!(
        lower_over_issue_id(&SetExpr::NotIds(vec![]), &v).sql_predicate,
        "TRUE"
    );
}

/// **`InRelation` → the `authz_visible` JOIN keyed on `issue.id`** (the reverse-index pattern), with
/// the subject + relation BOUND and the JOIN watermark-dependent.
#[test]
fn in_relation_lowers_to_authz_visible_join_over_issue_id() {
    let v = viewer("alice");
    let lowered = lower_over_issue_id(
        &SetExpr::InRelation {
            relation: RelName("view".into()),
            via_column: issue_id_colref(),
        },
        &v,
    );
    assert_eq!(lowered.sql_predicate, "av0.object_id IS NOT NULL");
    assert_eq!(lowered.joins.len(), 1);
    assert!(lowered.joins[0]
        .clause
        .contains("JOIN authz_visible av0 ON av0.object_id = issue.id"));
    assert_eq!(lowered.joins[0].relation, "view");
    // subject + relation bound, never interpolated.
    assert!(lowered.params.iter().any(|p| p.value == "alice"));
    assert!(lowered.params.iter().any(|p| p.value == "view"));
    assert!(lowered.depends_on_reverse_index());
    assert_eq!(lowered.filter_mode(), FilterMode::PushedDown);
}

/// **`TupleSet` → the same `authz_visible` JOIN** (the big-result path).
#[test]
fn tuple_set_lowers_to_the_authz_visible_join() {
    let v = viewer("u");
    let lowered = lower_over_issue_id(
        &SetExpr::TupleSet {
            index: AuthzIndexRef("view".into()),
        },
        &v,
    );
    assert_eq!(lowered.sql_predicate, "av0.object_id IS NOT NULL");
    assert_eq!(lowered.joins.len(), 1);
}

/// **`Union`/`Intersect`/`Difference` → `(a OR b)` / `(a AND b)` / `(a AND NOT b)`.**
#[test]
fn boolean_composition_lowers_to_or_and_and_not() {
    let v = viewer("u");
    let union = SetExpr::Union(vec![
        SetExpr::Ids(vec![oid("a")]),
        SetExpr::Ids(vec![oid("b")]),
    ]);
    assert_eq!(
        lower_over_issue_id(&union, &v).sql_predicate,
        "(issue.id IN (:id_0) OR issue.id IN (:id_1))"
    );
    let inter = SetExpr::Intersect(vec![SetExpr::All, SetExpr::Ids(vec![oid("a")])]);
    assert_eq!(
        lower_over_issue_id(&inter, &v).sql_predicate,
        "(TRUE AND issue.id IN (:id_0))"
    );
    let diff = SetExpr::Difference(
        Box::new(SetExpr::All),
        Box::new(SetExpr::Ids(vec![oid("a")])),
    );
    assert_eq!(
        lower_over_issue_id(&diff, &v).sql_predicate,
        "(TRUE AND NOT issue.id IN (:id_0))"
    );
}

/// **The CONFIDENTIAL set-difference (ISS-D3 crux): `view = (read − confidential) + grant`** lowers a
/// confidential issue OUT by construction — `(((read) AND NOT (confidential)) OR (grant))` — never a
/// post-filter, never an "N hidden" count leak.
#[test]
fn confidential_set_difference_lowers_to_and_not_no_count_leak() {
    let v = viewer("alice");
    // The frozen `view` rewrite shape (rebac_fragment): (parent_project->read − confidential) + grant.
    let set_expr = SetExpr::Union(vec![
        SetExpr::Difference(
            Box::new(SetExpr::InRelation {
                relation: RelName("read".into()),
                via_column: issue_id_colref(),
            }),
            Box::new(SetExpr::InRelation {
                relation: RelName("confidential".into()),
                via_column: issue_id_colref(),
            }),
        ),
        SetExpr::InRelation {
            relation: RelName("confidential_grant".into()),
            via_column: issue_id_colref(),
        },
    ]);
    let lowered = lower_over_issue_id(&set_expr, &v);
    assert_eq!(
        lowered.sql_predicate,
        "((av0.object_id IS NOT NULL AND NOT av1.object_id IS NOT NULL) OR av2.object_id IS NOT NULL)"
    );
    // Three distinct relations → three JOINs (read / confidential / confidential_grant), no N+1.
    assert_eq!(lowered.joins.len(), 3);
    // There is NO count/post-filter clause — the confidential set is excluded IN the predicate.
    assert!(!lowered.sql_predicate.to_lowercase().contains("count"));
    assert!(!lowered.sql_predicate.to_lowercase().contains("hidden"));
}

/// **No N+1: a relation repeated across the tree emits ONE JOIN** (deduplicated by `(viewer,
/// relation)`, however nested).
#[test]
fn repeated_relation_emits_one_join_no_n_plus_1() {
    let v = viewer("u");
    let r = || SetExpr::InRelation {
        relation: RelName("view".into()),
        via_column: issue_id_colref(),
    };
    let set_expr = SetExpr::Union(vec![r(), SetExpr::Intersect(vec![r(), r()])]);
    let lowered = lower_over_issue_id(&set_expr, &v);
    // Three textual references, ONE JOIN (the no-N+1 guarantee).
    assert_eq!(lowered.joins.len(), 1);
    assert_eq!(
        lowered
            .sql_predicate
            .matches("av0.object_id IS NOT NULL")
            .count(),
        3
    );
}

// ───────────────────────────── the board composer (one query) ────────────────────────────────────

/// **The board composes to ONE leak-free statement** with the tenant predicate + the ACL conjoined
/// BEFORE `ORDER BY issue.rank` / `LIMIT` (pre-filter, never post-filter).
#[test]
fn board_composes_to_one_leak_free_query() {
    let v = viewer("alice");
    let set_expr = SetExpr::InRelation {
        relation: RelName("view".into()),
        via_column: issue_id_colref(),
    };
    let q = compose_board_query(&set_expr, &v, &tenant(), &region());
    assert_eq!(q.statement_count(), 1, "one query — no N+1");
    assert!(q.sql.contains("FROM issue JOIN authz_visible av0"));
    assert!(q
        .sql
        .contains("WHERE issue.tenant_id = :tenant AND issue.region = :region"));
    // The ACL conjoin precedes the ORDER BY (pre-filter) — NOT a post-filter over scored rows.
    let acl_pos = q.sql.find("av0.object_id IS NOT NULL").unwrap();
    let order_pos = q.sql.find("ORDER BY issue.rank").unwrap();
    assert!(acl_pos < order_pos, "ACL pre-filter precedes ORDER BY");
    assert!(q.sql.find("LIMIT :page").unwrap() > order_pos);
    assert_eq!(q.filter_mode, FilterMode::PushedDown);
    // tenant/region bound.
    assert!(q
        .params
        .iter()
        .any(|p| p.placeholder == ":tenant" && p.value == "acme"));
    assert!(q
        .params
        .iter()
        .any(|p| p.placeholder == ":region" && p.value == "fr-par"));
}

/// **`None` composes to a `WHERE (FALSE)` board** — no rows, never permissive.
#[test]
fn none_set_composes_to_false_no_rows() {
    let v = viewer("u");
    let q = compose_board_query(&SetExpr::None, &v, &tenant(), &region());
    assert!(q.sql.contains("AND (FALSE)"));
    assert_eq!(q.statement_count(), 1);
}

// ───────────────────────────── leak-free evaluation (ISS-D3) ──────────────────────────────────────

/// **Partial visibility, 0 leak over the JOIN:** the viewer sees only the issues they hold a `view`
/// tuple for; the rest are ABSENT (never a post-filter over a wider set).
#[test]
fn partial_visibility_zero_leak_over_the_join() {
    let idx = AuthzVisibleIndex::new();
    let v = viewer("alice");
    // alice may view ENG-1 and ENG-3, not ENG-2.
    idx.grant(&tenant(), &region(), "alice", "view", "ENG-1", "zk-001");
    idx.grant(&tenant(), &region(), "alice", "view", "ENG-3", "zk-001");
    let set_expr = SetExpr::InRelation {
        relation: RelName("view".into()),
        via_column: issue_id_colref(),
    };
    let lowered = lower_over_issue_id(&set_expr, &v);
    let candidates = vec![oid("ENG-1"), oid("ENG-2"), oid("ENG-3")];
    let visible = idx.evaluate(&tenant(), &region(), &v, &lowered, &candidates);
    assert_eq!(visible, vec![oid("ENG-1"), oid("ENG-3")]);
    assert!(!visible.contains(&oid("ENG-2")), "ENG-2 absent — 0 leak");
}

/// **The confidential issue is ABSENT for a non-grantee** (the set-difference excludes; ISS-D3). With
/// a grant, it re-appears — leak-free either way.
#[test]
fn confidential_absent_for_non_grantee_present_for_grantee() {
    let idx = AuthzVisibleIndex::new();
    let v = viewer("alice");
    // alice reads ENG-1 (normal) and ENG-2 (confidential); only ENG-2 is marked confidential.
    idx.grant(&tenant(), &region(), "alice", "read", "ENG-1", "zk-001");
    idx.grant(&tenant(), &region(), "alice", "read", "ENG-2", "zk-001");
    idx.grant(
        &tenant(),
        &region(),
        "alice",
        "confidential",
        "ENG-2",
        "zk-001",
    );

    let view = || {
        SetExpr::Union(vec![
            SetExpr::Difference(
                Box::new(SetExpr::InRelation {
                    relation: RelName("read".into()),
                    via_column: issue_id_colref(),
                }),
                Box::new(SetExpr::InRelation {
                    relation: RelName("confidential".into()),
                    via_column: issue_id_colref(),
                }),
            ),
            SetExpr::InRelation {
                relation: RelName("confidential_grant".into()),
                via_column: issue_id_colref(),
            },
        ])
    };
    let lowered = lower_over_issue_id(&view(), &v);
    let candidates = vec![oid("ENG-1"), oid("ENG-2")];

    // Non-grantee: ENG-2 (confidential) is ABSENT — never a count leak.
    let visible = idx.evaluate(&tenant(), &region(), &v, &lowered, &candidates);
    assert_eq!(visible, vec![oid("ENG-1")]);

    // Now grant alice the explicit confidential_grant → ENG-2 re-appears (the `+ grant` arm).
    idx.grant(
        &tenant(),
        &region(),
        "alice",
        "confidential_grant",
        "ENG-2",
        "zk-002",
    );
    let lowered2 = lower_over_issue_id(&view(), &v);
    let visible2 = idx.evaluate(&tenant(), &region(), &v, &lowered2, &candidates);
    assert_eq!(visible2, vec![oid("ENG-1"), oid("ENG-2")]);
}

/// **No cross-tenant leak:** a viewer with a `view` tuple in tenant `acme` sees NOTHING when the scan
/// is scoped to a different tenant (the per-tenant index key — EI-02 §1).
#[test]
fn no_cross_tenant_leak() {
    let idx = AuthzVisibleIndex::new();
    let v = viewer("alice");
    idx.grant(&tenant(), &region(), "alice", "view", "ENG-1", "zk-001");
    let set_expr = SetExpr::InRelation {
        relation: RelName("view".into()),
        via_column: issue_id_colref(),
    };
    let lowered = lower_over_issue_id(&set_expr, &v);
    let other = TenantId("globex".into());
    let visible = idx.evaluate(&other, &region(), &v, &lowered, &[oid("ENG-1")]);
    assert!(visible.is_empty(), "no cross-tenant leak");
}

/// **`Difference(All, deny)` evaluates leak-free** (the `(TRUE AND NOT …)` form).
#[test]
fn difference_all_except_deny_evaluates_correctly() {
    let idx = AuthzVisibleIndex::new();
    let v = viewer("alice");
    idx.grant(
        &tenant(),
        &region(),
        "alice",
        "confidential",
        "ENG-2",
        "zk-001",
    );
    let set_expr = SetExpr::Difference(
        Box::new(SetExpr::All),
        Box::new(SetExpr::InRelation {
            relation: RelName("confidential".into()),
            via_column: issue_id_colref(),
        }),
    );
    let lowered = lower_over_issue_id(&set_expr, &v);
    let visible = idx.evaluate(
        &tenant(),
        &region(),
        &v,
        &lowered,
        &[oid("ENG-1"), oid("ENG-2")],
    );
    assert_eq!(
        visible,
        vec![oid("ENG-1")],
        "the confidential ENG-2 is excepted"
    );
}

/// **`NotIds` deny-set evaluates leak-free** over the `NOT IN` leaf.
#[test]
fn not_ids_deny_set_evaluates_leak_free() {
    let idx = AuthzVisibleIndex::new();
    let v = viewer("u");
    let lowered = lower_over_issue_id(&SetExpr::NotIds(vec![oid("ENG-2")]), &v);
    let visible = idx.evaluate(
        &tenant(),
        &region(),
        &v,
        &lowered,
        &[oid("ENG-1"), oid("ENG-2"), oid("ENG-3")],
    );
    assert_eq!(visible, vec![oid("ENG-1"), oid("ENG-3")]);
}

// ───────────────────────────── the new-enemy zookie guard (4.10) ──────────────────────────────────

/// **The new-enemy guard: a just-revoked grant drops out of the next zookie-bounded read.** The
/// revoke advances the watermark; the read carrying the post-revoke zookie reflects it (read-your-
/// writes) — the revoked issue is ABSENT.
#[test]
fn just_revoked_grant_drops_out_of_the_read() {
    let idx = AuthzVisibleIndex::new();
    let v = viewer("alice");
    idx.grant(&tenant(), &region(), "alice", "view", "ENG-1", "zk-001");
    let set_expr = SetExpr::InRelation {
        relation: RelName("view".into()),
        via_column: issue_id_colref(),
    };

    // Before the revoke: alice sees ENG-1.
    let lowered = lower_over_issue_id(&set_expr, &v);
    assert_eq!(
        idx.evaluate(&tenant(), &region(), &v, &lowered, &[oid("ENG-1")]),
        vec![oid("ENG-1")]
    );

    // Revoke → watermark advances to zk-002.
    idx.revoke(&tenant(), &region(), "alice", "view", "ENG-1", "zk-002");
    let post_revoke = idx.watermark(&tenant(), &region());
    assert_eq!(post_revoke.0, "zk-002");

    // The next read carrying the post-revoke zookie: the index serves (at-or-after) and ENG-1 is gone.
    assert!(idx.serves(&tenant(), &region(), &post_revoke));
    let lowered2 = lower_over_issue_id(&set_expr, &v);
    assert!(
        idx.evaluate(&tenant(), &region(), &v, &lowered2, &[oid("ENG-1")])
            .is_empty(),
        "the revoked grant is absent — 0 leak under the zookie"
    );
}

/// **`serves` is exactly at-or-after; behind → fall back to `check`** (never serve a stale grant).
#[test]
fn new_enemy_guard_serves_at_or_after_falls_back_behind() {
    let idx = AuthzVisibleIndex::new();
    idx.advance_watermark(&tenant(), &region(), "zk-005");
    assert!(
        idx.serves(&tenant(), &region(), &Zookie("zk-004".into())),
        "behind required → serves"
    );
    assert!(
        idx.serves(&tenant(), &region(), &Zookie("zk-005".into())),
        "exactly at → serves"
    );
    assert!(
        !idx.serves(&tenant(), &region(), &Zookie("zk-006".into())),
        "ahead of watermark → falls back to check (never a stale grant)"
    );
    // An empty required (default consistency) always serves.
    assert!(idx.serves(&tenant(), &region(), &Zookie(String::new())));
}

/// **The watermark is monotone — a stale/older revision never regresses it.**
#[test]
fn watermark_is_monotone_stale_never_regresses() {
    let idx = AuthzVisibleIndex::new();
    idx.advance_watermark(&tenant(), &region(), "zk-005");
    idx.advance_watermark(&tenant(), &region(), "zk-003"); // stale — ignored.
    assert_eq!(idx.watermark(&tenant(), &region()).0, "zk-005");
    idx.advance_watermark(&tenant(), &region(), "zk-009"); // newer — advances.
    assert_eq!(idx.watermark(&tenant(), &region()).0, "zk-009");
}

/// **`statement_count` counts statements (not a hardcoded `1`)** — a defensive guard that a future
/// composer change to two statements is caught (the no-N+1 property is structural).
#[test]
fn statement_count_counts_statements_not_constant_one() {
    let mut q = compose_board_query(&SetExpr::All, &viewer("u"), &tenant(), &region());
    assert_eq!(q.statement_count(), 1);
    q.sql = "SELECT 1; SELECT 2".into();
    assert_eq!(
        q.statement_count(),
        2,
        "counts real statements, not a constant"
    );
}

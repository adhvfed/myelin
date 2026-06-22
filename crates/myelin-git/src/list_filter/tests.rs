//! Unit tests for the GIT-P26 list-filter push-down (the SetExpr → SQL JOIN over `repo.id`/`pr.id`,
//! the one-query/no-post-filter conjoin, the code-search pre-filter, the leak-free evaluation, and
//! the new-enemy zookie guard). The chained-e2e / live-Postgres proof is the `--features
//! integration` test (`tests/integration_git_p26_list_pushdown.rs`); these are the deterministic,
//! DB-free unit + model drills (the SetExpr-lowering path is mandatory-core — a leak is the failure).

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

// ───────────────────────────── the lowering (§5.3 — via_column = repo.id / pr.id) ────────────────

/// **`All` → `TRUE`**; **`None` → `FALSE`** (admin sees all; deny is `WHERE false`, never permissive).
#[test]
fn all_and_none_lower_to_true_and_false() {
    let all = lower_over_repo_id(&SetExpr::All, &viewer("p:a"));
    assert_eq!(all.sql_predicate, "TRUE");
    assert!(all.joins.is_empty() && all.params.is_empty());
    let none = lower_over_repo_id(&SetExpr::None, &viewer("p:a"));
    assert_eq!(none.sql_predicate, "FALSE");
}

/// **`Ids` → `repo.id IN (…)` with BOUND params** (never interpolated literals — injection-safe).
#[test]
fn ids_lowers_to_in_over_repo_id_with_bound_params() {
    let l = lower_over_repo_id(
        &SetExpr::Ids(vec![ObjectId("repo:a".into()), ObjectId("repo:b".into())]),
        &viewer("p:a"),
    );
    assert_eq!(l.sql_predicate, "repo.id IN (:id_0, :id_1)");
    assert_eq!(
        l.params,
        vec![
            BoundParam {
                placeholder: ":id_0".into(),
                value: "repo:a".into()
            },
            BoundParam {
                placeholder: ":id_1".into(),
                value: "repo:b".into()
            },
        ],
        "the ids are BOUND params, never interpolated into the SQL"
    );
    assert!(
        l.joins.is_empty(),
        "an Ids lowering needs no reverse-index JOIN"
    );
    assert_eq!(l.filter_mode(), FilterMode::Ids);
}

/// **An empty `Ids` is `FALSE`** (the empty allow-set sees nothing — never `IN ()`, never `TRUE`).
#[test]
fn empty_ids_lowers_to_false_never_permissive() {
    let l = lower_over_pr_id(&SetExpr::Ids(vec![]), &viewer("p:a"));
    assert_eq!(l.sql_predicate, "FALSE", "an empty allow-set sees nothing");
}

/// **`NotIds` → `<via> NOT IN (…)`** over the PR id column; an empty deny-set is `TRUE`.
#[test]
fn not_ids_lowers_to_not_in_over_pr_id() {
    let l = lower_over_pr_id(
        &SetExpr::NotIds(vec![ObjectId("pr:secret".into())]),
        &viewer("p:a"),
    );
    assert_eq!(l.sql_predicate, "pr.id NOT IN (:id_0)");
    let empty = lower_over_pr_id(&SetExpr::NotIds(vec![]), &viewer("p:a"));
    assert_eq!(
        empty.sql_predicate, "TRUE",
        "an empty deny-set excludes nothing"
    );
}

/// **`InRelation` → the `authz_visible` JOIN keyed on Git's OWN id column (§5.3).** The JOIN is ONE
/// clause over `pr.id`; the predicate references its alias — no per-row subquery, no N+1.
#[test]
fn in_relation_lowers_to_authz_visible_join_over_pr_id() {
    let l = lower_over_pr_id(
        &SetExpr::InRelation {
            relation: RelName(PR_LIST_PERMISSION.into()),
            via_column: pr_id_colref(),
        },
        &viewer("p:alice"),
    );
    assert_eq!(l.joins.len(), 1, "exactly one reverse-index JOIN (no N+1)");
    let j = &l.joins[0];
    assert!(
        j.clause
            .contains("JOIN authz_visible av0 ON av0.object_id = pr.id"),
        "the JOIN keys on Git's own pr.id column: {}",
        j.clause
    );
    assert!(
        j.clause.contains("av0.subject = :subject_0"),
        "binds the subject: {}",
        j.clause
    );
    assert!(
        j.clause.contains("av0.relation = :rel_for_view"),
        "binds the relation: {}",
        j.clause
    );
    assert_eq!(l.sql_predicate, "av0.object_id IS NOT NULL");
    assert!(l
        .params
        .iter()
        .any(|p| p.placeholder == ":subject_0" && p.value == "p:alice"));
    assert!(
        l.depends_on_reverse_index(),
        "an InRelation lowering depends on the watermark"
    );
    assert_eq!(l.filter_mode(), FilterMode::PushedDown);
}

/// **`TupleSet` → the same `authz_visible` JOIN** (the big-result materialised path).
#[test]
fn tuple_set_lowers_to_the_authz_visible_join() {
    let l = lower_over_repo_id(
        &SetExpr::TupleSet {
            index: AuthzIndexRef("pull".into()),
        },
        &viewer("p:a"),
    );
    assert_eq!(l.joins.len(), 1);
    assert!(l.joins[0].clause.contains("av0.relation = :rel_for_pull"));
    assert!(l.depends_on_reverse_index());
}

/// **Boolean composition → `(a OR b)` / `(a AND b)` / `(a AND NOT b)` (§5.3).**
#[test]
fn boolean_composition_lowers_to_or_and_and_not() {
    let u = lower_over_repo_id(
        &SetExpr::Union(vec![
            SetExpr::Ids(vec![ObjectId("repo:a".into())]),
            SetExpr::Ids(vec![ObjectId("repo:b".into())]),
        ]),
        &viewer("p:a"),
    );
    assert_eq!(
        u.sql_predicate,
        "(repo.id IN (:id_0) OR repo.id IN (:id_1))"
    );

    let d = lower_over_repo_id(
        &SetExpr::Difference(
            Box::new(SetExpr::All),
            Box::new(SetExpr::Ids(vec![ObjectId("repo:secret".into())])),
        ),
        &viewer("p:a"),
    );
    assert_eq!(d.sql_predicate, "(TRUE AND NOT repo.id IN (:id_0))");
}

/// **No N+1: the SAME `(viewer, relation)` JOIN is emitted ONCE even in two branches (§5.3).**
#[test]
fn repeated_relation_emits_one_join_no_n_plus_1() {
    let l = lower_over_pr_id(
        &SetExpr::Union(vec![
            SetExpr::InRelation {
                relation: RelName("view".into()),
                via_column: pr_id_colref(),
            },
            SetExpr::InRelation {
                relation: RelName("view".into()),
                via_column: pr_id_colref(),
            },
        ]),
        &viewer("p:alice"),
    );
    assert_eq!(
        l.joins.len(),
        1,
        "the same (viewer, relation) JOIN once, however nested — no N+1"
    );
    assert_eq!(
        l.sql_predicate,
        "(av0.object_id IS NOT NULL OR av0.object_id IS NOT NULL)"
    );
}

// ───────────────────────────── the list-query composers (ONE query) ──────────────────────────────

/// **The PR-list composes to ONE leak-free query** — the `authz_visible` JOIN in the `FROM`, the
/// tenant/region predicate + the ACL predicate in the `WHERE` (the ACL conjoined BEFORE ORDER/LIMIT
/// — pre-filter, never post-filter), the page bound bound, exactly ONE statement (no N+1).
#[test]
fn pr_list_composes_to_one_leak_free_query() {
    let set_expr = SetExpr::InRelation {
        relation: RelName(PR_LIST_PERMISSION.into()),
        via_column: pr_id_colref(),
    };
    let q = compose_pr_list_query(&set_expr, &viewer("p:alice"), &tenant(), &region());
    assert_eq!(
        q.statement_count(),
        1,
        "ONE SQL statement — no N+1, no per-row check loop"
    );
    assert!(
        q.sql
            .contains("JOIN authz_visible av0 ON av0.object_id = pr.id"),
        "{}",
        q.sql
    );
    assert!(
        q.sql
            .contains("pr.tenant_id = :tenant AND pr.region = :region"),
        "tenant predicate: {}",
        q.sql
    );
    // The ACL predicate is conjoined BEFORE the ORDER BY / LIMIT (pre-filter, never post-filter).
    let acl_pos = q
        .sql
        .find("av0.object_id IS NOT NULL")
        .expect("acl predicate present");
    let order_pos = q.sql.find("ORDER BY").expect("order present");
    assert!(
        acl_pos < order_pos,
        "the ACL is conjoined BEFORE scoring/pagination (pre-filter): {}",
        q.sql
    );
    assert!(
        q.sql.contains("LIMIT :page"),
        "the page bound is bound: {}",
        q.sql
    );
    assert_eq!(q.filter_mode, FilterMode::PushedDown);
    assert!(q
        .params
        .iter()
        .any(|p| p.placeholder == ":tenant" && p.value == "acme"));
    assert!(q
        .params
        .iter()
        .any(|p| p.placeholder == ":region" && p.value == "fr-par"));
}

/// **The repo-list composes to ONE query over `repo.id`** with the tenant predicate ALWAYS emitted
/// (a tenant-less list is unconstructable — the `tenant-predicate` discipline).
#[test]
fn repo_list_composes_to_one_query_with_tenant_predicate() {
    let set_expr = SetExpr::Ids(vec![ObjectId("repo:core".into())]);
    let q = compose_repo_list_query(&set_expr, &viewer("p:alice"), &tenant(), &region());
    assert_eq!(q.statement_count(), 1);
    assert!(q.sql.contains("FROM repo"), "{}", q.sql);
    assert!(
        q.sql
            .contains("repo.tenant_id = :tenant AND repo.region = :region"),
        "{}",
        q.sql
    );
    assert!(q.sql.contains("repo.id IN (:id_0)"), "{}", q.sql);
    assert_eq!(
        q.filter_mode,
        FilterMode::Ids,
        "a materialised Ids allow-set is the Ids mode"
    );
}

/// **A `None` deny set composes to a `WHERE … AND (FALSE)` — the leak-free empty list (no rows).**
#[test]
fn none_set_composes_to_a_false_predicate_no_rows() {
    let q = compose_pr_list_query(&SetExpr::None, &viewer("p:nobody"), &tenant(), &region());
    assert_eq!(q.statement_count(), 1);
    assert!(
        q.sql.contains("AND (FALSE)"),
        "a denied viewer's list is WHERE false (0 rows): {}",
        q.sql
    );
}

// ───────────────────────────── the code-search pre-filter (6.1) ──────────────────────────────────

/// **The code-search pre-filter lowers `list_objects(viewer, read, repo)` over the doc's `repo_id`
/// facet and names the binder `acl_filter`** (the `search-requires-acl-filter` lint fingerprint).
#[test]
fn code_search_pre_filter_keys_on_repo_over_code_doc_repo_id() {
    let set_expr = SetExpr::InRelation {
        relation: RelName(CODE_SEARCH_PERMISSION.into()),
        via_column: code_search_repo_colref(),
    };
    let pf = code_search_pre_filter(&set_expr, &viewer("p:alice"));
    assert!(
        pf.acl_filter.joins[0]
            .clause
            .contains("av0.object_id = code_doc.repo_id"),
        "the code-search pre-filter keys on the doc's parent-repo id (GIT-P5 acl_object=repo): {}",
        pf.acl_filter.joins[0].clause
    );
    assert!(pf.acl_filter.joins[0]
        .clause
        .contains("av0.relation = :rel_for_read"));
    assert!(pf.acl_filter.depends_on_reverse_index());
}

// ───────────────────────────── leak-free evaluation (the model = the SQL) ─────────────────────────

/// **A PARTIAL-visibility PR list returns ONLY visible rows (0 leak) — the SetExpr JOIN evaluated
/// against the reverse index keeps exactly the granted PRs.** alice may `view` two PRs; a third PR
/// (granted to bob) NEVER survives the predicate.
#[test]
fn partial_pr_visibility_zero_leak_over_the_join() {
    let ix = AuthzVisibleIndex::new();
    ix.grant(
        &tenant(),
        &region(),
        "p:alice",
        "view",
        "pr:1",
        "zk-00000000000000000001",
    );
    ix.grant(
        &tenant(),
        &region(),
        "p:alice",
        "view",
        "pr:2",
        "zk-00000000000000000002",
    );
    ix.grant(
        &tenant(),
        &region(),
        "p:bob",
        "view",
        "pr:3",
        "zk-00000000000000000003",
    );

    let set_expr = SetExpr::InRelation {
        relation: RelName("view".into()),
        via_column: pr_id_colref(),
    };
    let lowered = lower_over_pr_id(&set_expr, &viewer("p:alice"));
    let candidates = vec![
        ObjectId("pr:1".into()),
        ObjectId("pr:2".into()),
        ObjectId("pr:3".into()),
    ];
    let visible = ix.evaluate(
        &tenant(),
        &region(),
        &viewer("p:alice"),
        &lowered,
        &candidates,
    );
    assert_eq!(
        visible,
        vec![ObjectId("pr:1".into()), ObjectId("pr:2".into())],
        "only alice's two visible PRs survive — pr:3 (bob's) is 0-leak absent"
    );
}

/// **No cross-tenant leak.** A grant in `acme` does not make `pr:1` visible to a viewer scanning
/// under `globex` (the reverse index is keyed `(tenant, region, subject, relation)`).
#[test]
fn no_cross_tenant_leak() {
    let ix = AuthzVisibleIndex::new();
    ix.grant(
        &tenant(),
        &region(),
        "p:alice",
        "view",
        "pr:1",
        "zk-00000000000000000001",
    );
    let globex = TenantId("globex".into());
    let set_expr = SetExpr::InRelation {
        relation: RelName("view".into()),
        via_column: pr_id_colref(),
    };
    let lowered = lower_over_pr_id(&set_expr, &viewer("p:alice"));
    let visible = ix.evaluate(
        &globex,
        &region(),
        &viewer("p:alice"),
        &lowered,
        &[ObjectId("pr:1".into())],
    );
    assert!(
        visible.is_empty(),
        "an acme grant does not list under globex (no cross-tenant query path)"
    );
}

/// **The `Difference` (All EXCEPT a deny-set) evaluates leak-free** — admin sees all repos except
/// the explicitly-denied one (the `(TRUE AND NOT repo.id IN (…))` lowering).
#[test]
fn difference_all_except_deny_evaluates_correctly() {
    let ix = AuthzVisibleIndex::new();
    let set_expr = SetExpr::Difference(
        Box::new(SetExpr::All),
        Box::new(SetExpr::Ids(vec![ObjectId("repo:secret".into())])),
    );
    let lowered = lower_over_repo_id(&set_expr, &viewer("p:admin"));
    let candidates = vec![ObjectId("repo:a".into()), ObjectId("repo:secret".into())];
    let visible = ix.evaluate(
        &tenant(),
        &region(),
        &viewer("p:admin"),
        &lowered,
        &candidates,
    );
    assert_eq!(
        visible,
        vec![ObjectId("repo:a".into())],
        "admin sees all except the denied repo"
    );
}

/// **A `Union` of a JOIN arm and an explicit `Ids` allow arm evaluates as OR.** A PR visible via
/// either the reviewer relation OR the explicit allow-set survives.
#[test]
fn union_of_join_and_ids_evaluates_as_or() {
    let ix = AuthzVisibleIndex::new();
    ix.grant(
        &tenant(),
        &region(),
        "p:alice",
        "view",
        "pr:joined",
        "zk-00000000000000000001",
    );
    let set_expr = SetExpr::Union(vec![
        SetExpr::InRelation {
            relation: RelName("view".into()),
            via_column: pr_id_colref(),
        },
        SetExpr::Ids(vec![ObjectId("pr:explicit".into())]),
    ]);
    let lowered = lower_over_pr_id(&set_expr, &viewer("p:alice"));
    let candidates = vec![
        ObjectId("pr:joined".into()),
        ObjectId("pr:explicit".into()),
        ObjectId("pr:neither".into()),
    ];
    let visible = ix.evaluate(
        &tenant(),
        &region(),
        &viewer("p:alice"),
        &lowered,
        &candidates,
    );
    assert_eq!(
        visible,
        vec![ObjectId("pr:joined".into()), ObjectId("pr:explicit".into())],
        "either arm of the Union survives; the unrelated PR does not"
    );
}

// ───────────────────────────── the new-enemy zookie guard (4.10 / §5.3) ───────────────────────────

/// **A just-revoked grant is reflected — the revoked PR drops out of the evaluated list.** After
/// `revoke`, the reverse index no longer carries the row, so the JOIN (and thus the list) excludes it.
#[test]
fn just_revoked_grant_drops_out_of_the_list() {
    let ix = AuthzVisibleIndex::new();
    ix.grant(
        &tenant(),
        &region(),
        "p:alice",
        "view",
        "pr:1",
        "zk-00000000000000000001",
    );
    ix.grant(
        &tenant(),
        &region(),
        "p:alice",
        "view",
        "pr:2",
        "zk-00000000000000000002",
    );
    let set_expr = SetExpr::InRelation {
        relation: RelName("view".into()),
        via_column: pr_id_colref(),
    };
    let lowered = lower_over_pr_id(&set_expr, &viewer("p:alice"));
    let candidates = vec![ObjectId("pr:1".into()), ObjectId("pr:2".into())];

    // Revoke pr:1 at a later revision.
    ix.revoke(
        &tenant(),
        &region(),
        "p:alice",
        "view",
        "pr:1",
        "zk-00000000000000000003",
    );
    let after = ix.evaluate(
        &tenant(),
        &region(),
        &viewer("p:alice"),
        &lowered,
        &candidates,
    );
    assert_eq!(
        after,
        vec![ObjectId("pr:2".into())],
        "the just-revoked pr:1 drops out (zookie reflected)"
    );
}

/// **The new-enemy guard: a scan requiring a revision AT-OR-AFTER the watermark serves the JOIN; a
/// scan requiring a FRESHER revision than the watermark does NOT serve (fall back to per-row check
/// rather than serve a stale grant).**
#[test]
fn new_enemy_guard_serves_at_or_after_falls_back_behind() {
    let ix = AuthzVisibleIndex::new();
    ix.grant(
        &tenant(),
        &region(),
        "p:alice",
        "view",
        "pr:1",
        "zk-00000000000000000005",
    );
    // The watermark is at rev 5.
    assert_eq!(
        ix.watermark(&tenant(), &region()),
        Zookie("zk-00000000000000000005".into())
    );
    // A scan requiring rev 3 (<= 5) → serves.
    assert!(ix.serves(
        &tenant(),
        &region(),
        &Zookie("zk-00000000000000000003".into())
    ));
    // A scan requiring exactly rev 5 → serves (at-or-after is inclusive).
    assert!(ix.serves(
        &tenant(),
        &region(),
        &Zookie("zk-00000000000000000005".into())
    ));
    // A scan requiring rev 7 (> 5) → does NOT serve (fall back to check, never serve stale).
    assert!(!ix.serves(
        &tenant(),
        &region(),
        &Zookie("zk-00000000000000000007".into())
    ));
    // A default-consistency read (no pinned revision) always serves.
    assert!(ix.serves(&tenant(), &region(), &Zookie(String::new())));
}

/// **The watermark is monotone — a stale (older) revision never regresses it.**
#[test]
fn watermark_is_monotone_stale_never_regresses() {
    let ix = AuthzVisibleIndex::new();
    ix.advance_watermark(&tenant(), &region(), "zk-00000000000000000010");
    ix.advance_watermark(&tenant(), &region(), "zk-00000000000000000005"); // stale — ignored
    assert_eq!(
        ix.watermark(&tenant(), &region()),
        Zookie("zk-00000000000000000010".into())
    );
}

/// **A `NotIds` deny-set evaluates leak-free: a denied id is excluded, an otherwise-visible id
/// survives** (the `<via> NOT IN (…)` leaf branch — kills the `==`/`!`/`NOT IN` mutants on the
/// inline deny path). The deny-set is a single explicit `NotIds` (the `(TRUE AND NOT …)` Difference
/// composition routes through the `IN` leaf + the parser's `AND NOT`; THIS exercises the direct
/// `NOT IN` leaf).
#[test]
fn not_ids_deny_set_evaluates_leak_free_over_the_not_in_leaf() {
    let ix = AuthzVisibleIndex::new();
    let lowered = lower_over_repo_id(
        &SetExpr::NotIds(vec![ObjectId("repo:denied".into())]),
        &viewer("p:a"),
    );
    assert_eq!(lowered.sql_predicate, "repo.id NOT IN (:id_0)");
    let candidates = vec![
        ObjectId("repo:visible".into()),
        ObjectId("repo:denied".into()),
    ];
    let visible = ix.evaluate(&tenant(), &region(), &viewer("p:a"), &lowered, &candidates);
    assert_eq!(
        visible,
        vec![ObjectId("repo:visible".into())],
        "the denied id is excluded by NOT IN; the otherwise-visible id survives"
    );
}

/// **`statement_count` actually counts statements** (kills the `replace -> 1` constant mutant): a
/// composed query is one statement; a (hypothetical) two-statement string counts two — so the
/// function is not a constant `1`.
#[test]
fn statement_count_counts_statements_not_constant_one() {
    let one = compose_pr_list_query(&SetExpr::All, &viewer("p:a"), &tenant(), &region());
    assert_eq!(one.statement_count(), 1);
    // A two-statement variant (defensive — proves the count is computed, not the constant 1).
    let two = ComposedListQuery {
        sql: "SELECT 1; SELECT 2".into(),
        params: vec![],
        filter_mode: FilterMode::Ids,
    };
    assert_eq!(
        two.statement_count(),
        2,
        "statement_count computes the count, it is not constant 1"
    );
}

/// **The new-enemy guard's `>=` is a STRICT at-or-after** (kills the `>`/`>=` boundary mutants on
/// `serves`): a scan requiring EXACTLY the watermark serves; one strictly above does not.
#[test]
fn serves_boundary_is_exactly_at_or_after() {
    let ix = AuthzVisibleIndex::new();
    ix.advance_watermark(&tenant(), &region(), "zk-00000000000000000005");
    // Exactly the watermark → serves (the `>=` boundary, not `>`).
    assert!(ix.serves(
        &tenant(),
        &region(),
        &Zookie("zk-00000000000000000005".into())
    ));
    // One unit above → does not serve.
    assert!(!ix.serves(
        &tenant(),
        &region(),
        &Zookie("zk-00000000000000000006".into())
    ));
}

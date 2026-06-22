//! Unit tests for the KN-P16 `SetExpr` lowering over `db_row.id` / `page.id` + the permission-correct
//! COUNT + the in-memory `authz_visible` evaluator. These are the **mutation-tested core** (a leak is
//! catastrophic — KN-D5): the no-leak property (a forbidden row is absent from the view AND uncounted)
//! must survive mutation. The lowering shape is byte-identical to the sibling consumers
//! (`myelin_identity_service::lowering`, `myelin_git::list_filter`) — the column is `db_row.id`.
//!
//! ## The cargo-mutants mutation-score FLOOR (mandatory-core; KN-P16 TESTS)
//! The LEAK-CRITICAL surface — the [`lower_expr`] `SetExpr`→SQL table (the All/None/Ids/NotIds/
//! InRelation/TupleSet/Union/Intersect/Difference lowering) AND the leak-deciding leaves of the
//! in-memory evaluator ([`AuthzVisibleIndex::frag_holds`] — the `IN`/`NOT IN`/reverse-index-JOIN
//! membership + the deny defaults), the COUNT==view structural equality, and the watermark
//! monotonicity — is the mutation floor: **every mutant that would let a forbidden row LEAK into the
//! view OR be COUNTED (the no-leak property) must be CAUGHT.** Run:
//!
//!   cargo mutants -p myelin-knowledge --file crates/myelin-knowledge/src/list_filter.rs -- --lib
//!
//! Documented surviving (NON-leak) mutants — these do NOT weaken the no-leak gate:
//! - the `advance_watermark` `> → >=` is an EQUIVALENT mutant (equal-revision assigns the same value;
//!   the monotonicity that MATTERS — a stale advance never regresses — IS asserted by
//!   `watermark_is_monotone_stale_never_regresses`), the same documented equivalent as the sibling
//!   `myelin_git::list_filter`.
//! - the recursive-descent boolean parser ([`eval_predicate`]/`tokenize`/`parse_*`) is TEST/MODEL
//!   machinery (the in-memory mirror of the SQL `WHERE`), NOT the production path — the database
//!   evaluates the real predicate, proven by the `--features integration` test
//!   (`integration_kn_d5_list_pushdown.rs`) running the lowered SQL against live Postgres. Arithmetic
//!   mutants of the tokenizer's index/depth counters (`+= → -=`/`*=`) either time out (an infinite
//!   loop the harness flags) or mis-parse a fragment the leak tests would still reject closed; they do
//!   not affect the production leak property. The leak-DECIDING leaves are pinned by the direct
//!   evaluator tests below (`not_ids_deny_set_*`, `intersect_all_with_not_ids_*`, `union_admits_*`).

use super::*;
use myelin_identity::{AuthzIndexRef, ObjectId, PrincipalId, PrincipalKind, RelName, SetExpr};

fn viewer(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn db_via() -> ColRef {
    db_row_id_colref()
}

// ───────────────────────────── the FROZEN §4.1 lowering table (each variant) ──────────────────────

/// **`All` → `TRUE`** (the viewer reads the whole db via page-level inheritance — no ACL conjunct).
#[test]
fn all_lowers_to_true_no_conjunct() {
    let l = lower_over(&SetExpr::All, &viewer("p:a"), &db_via());
    assert_eq!(l.sql_predicate, "TRUE");
    assert!(l.joins.is_empty() && l.params.is_empty());
}

/// **`None` → `FALSE`** (`WHERE false` — the deny set, never a permissive default).
#[test]
fn none_lowers_to_where_false() {
    let l = lower_over(&SetExpr::None, &viewer("p:a"), &db_via());
    assert_eq!(l.sql_predicate, "FALSE");
}

/// **`Ids` → `db_row.id IN (…)` with BOUND params** (never interpolated literals — injection-safe).
#[test]
fn ids_lowers_to_in_with_bound_params_over_db_row_id() {
    let l = lower_over(
        &SetExpr::Ids(vec![ObjectId("row:1".into()), ObjectId("row:2".into())]),
        &viewer("p:a"),
        &db_via(),
    );
    assert_eq!(l.sql_predicate, "db_row.id IN (:id_0, :id_1)");
    assert_eq!(
        l.params,
        vec![
            BoundParam {
                placeholder: ":id_0".into(),
                value: "row:1".into()
            },
            BoundParam {
                placeholder: ":id_1".into(),
                value: "row:2".into()
            },
        ],
        "the ids are BOUND params over the FROZEN db_row.id column, never interpolated"
    );
    assert!(l.joins.is_empty());
}

/// **An empty `Ids` is `FALSE`** (the empty allow-set sees nothing — never `IN ()`, never a
/// permissive `TRUE`). The leak-critical identity element.
#[test]
fn empty_ids_lowers_to_false_never_permissive() {
    let l = lower_over(&SetExpr::Ids(vec![]), &viewer("p:a"), &db_via());
    assert_eq!(l.sql_predicate, "FALSE", "an empty allow-set sees nothing");
}

/// **`NotIds` → `db_row.id NOT IN (…)`**; an empty deny-set is `TRUE`.
#[test]
fn not_ids_lowers_to_not_in() {
    let l = lower_over(
        &SetExpr::NotIds(vec![ObjectId("row:secret".into())]),
        &viewer("p:a"),
        &db_via(),
    );
    assert_eq!(l.sql_predicate, "db_row.id NOT IN (:id_0)");
    let empty = lower_over(&SetExpr::NotIds(vec![]), &viewer("p:a"), &db_via());
    assert_eq!(
        empty.sql_predicate, "TRUE",
        "an empty deny-set excludes nothing"
    );
}

/// **`InRelation{row_reader, db_row.id}` → the `authz_visible` JOIN keyed on `db_row.id` (§4.1
/// /§5.1).** The row-restricted case — ONE JOIN, the predicate references its alias, no per-row
/// subquery, no N+1.
#[test]
fn in_relation_row_reader_lowers_to_authz_visible_join_over_db_row_id() {
    let l = lower_over(
        &SetExpr::InRelation {
            relation: RelName("read".into()),
            via_column: db_via(),
        },
        &viewer("p:alice"),
        &db_via(),
    );
    assert_eq!(l.joins.len(), 1, "exactly one reverse-index JOIN (no N+1)");
    let j = &l.joins[0];
    assert!(
        j.clause
            .contains("JOIN authz_visible av0 ON av0.object_id = db_row.id"),
        "the JOIN keys on the FROZEN db_row.id column: {}",
        j.clause
    );
    assert!(
        j.clause.contains("av0.subject = :subject_0"),
        "binds the subject: {}",
        j.clause
    );
    assert!(
        j.clause.contains("av0.relation = :rel_for_read"),
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
fn tuple_set_lowers_to_authz_visible_join() {
    let l = lower_over(
        &SetExpr::TupleSet {
            index: AuthzIndexRef("row_reader".into()),
        },
        &viewer("p:alice"),
        &db_via(),
    );
    assert_eq!(l.joins.len(), 1);
    assert!(l.joins[0]
        .clause
        .contains("av0.relation = :rel_for_row_reader"));
    assert!(l.depends_on_reverse_index());
}

/// **`Union` → `(a OR b)`, `Intersect` → `(a AND b)`, `Difference` → `(a AND NOT b)` (§4.1).**
#[test]
fn boolean_composition_lowers_to_or_and_and_not() {
    let u = lower_over(
        &SetExpr::Union(vec![
            SetExpr::Ids(vec![ObjectId("row:a".into())]),
            SetExpr::Ids(vec![ObjectId("row:b".into())]),
        ]),
        &viewer("p:a"),
        &db_via(),
    );
    assert_eq!(
        u.sql_predicate,
        "(db_row.id IN (:id_0) OR db_row.id IN (:id_1))"
    );

    let i = lower_over(
        &SetExpr::Intersect(vec![
            SetExpr::All,
            SetExpr::NotIds(vec![ObjectId("row:x".into())]),
        ]),
        &viewer("p:a"),
        &db_via(),
    );
    assert_eq!(i.sql_predicate, "(TRUE AND db_row.id NOT IN (:id_0))");

    let d = lower_over(
        &SetExpr::Difference(
            Box::new(SetExpr::All),
            Box::new(SetExpr::Ids(vec![ObjectId("row:secret".into())])),
        ),
        &viewer("p:a"),
        &db_via(),
    );
    assert_eq!(d.sql_predicate, "(TRUE AND NOT db_row.id IN (:id_0))");
}

/// **No N+1: the SAME `(viewer, relation)` JOIN is emitted ONCE even across two branches (§4.1).**
#[test]
fn repeated_relation_emits_one_join_no_n_plus_1() {
    let l = lower_over(
        &SetExpr::Union(vec![
            SetExpr::InRelation {
                relation: RelName("read".into()),
                via_column: db_via(),
            },
            SetExpr::InRelation {
                relation: RelName("read".into()),
                via_column: db_via(),
            },
        ]),
        &viewer("p:alice"),
        &db_via(),
    );
    assert_eq!(
        l.joins.len(),
        1,
        "the same (viewer, relation) JOIN is emitted once — no N+1"
    );
    assert_eq!(
        l.sql_predicate,
        "(av0.object_id IS NOT NULL OR av0.object_id IS NOT NULL)"
    );
}

/// **The page-tree list lowers over `page.id`** (the §5 inherited-with-overrides node).
#[test]
fn page_list_lowers_over_page_id() {
    let l = lower_over_page_id(
        &SetExpr::InRelation {
            relation: RelName("read".into()),
            via_column: page_id_colref(),
        },
        &viewer("p:a"),
    );
    assert!(
        l.joins[0].clause.contains("av0.object_id = page.id"),
        "{}",
        l.joins[0].clause
    );
}

// ───────────────────────────── the composed ONE query (view + COUNT) ──────────────────────────────

/// **The db VIEW query is ONE statement, ACL conjoined BEFORE the ORDER BY/LIMIT, tenant+db_id
/// confined (no-cross-db / tenant-predicate).** Pre-filter, never post-filter.
#[test]
fn view_query_is_one_statement_acl_pre_filtered_tenant_and_db_confined() {
    let q = compose_db_view_query(
        &SetExpr::InRelation {
            relation: RelName("read".into()),
            via_column: db_via(),
        },
        &viewer("p:alice"),
        &TenantId("acme".into()),
        "db:projects",
    );
    assert_eq!(
        q.statement_count(),
        1,
        "ONE query (no N+1, no post-filter second pass)"
    );
    assert!(
        q.sql
            .contains("JOIN authz_visible av0 ON av0.object_id = db_row.id"),
        "{}",
        q.sql
    );
    // The tenant + db_id predicates are ALWAYS present (the lints) and the ACL is BEFORE ORDER BY.
    assert!(
        q.sql.contains("db_row.tenant = :tenant"),
        "tenant predicate present: {}",
        q.sql
    );
    assert!(
        q.sql.contains("db_row.db_id = :db_id"),
        "db_id (no-cross-db) predicate present: {}",
        q.sql
    );
    let acl_pos = q.sql.find("av0.object_id IS NOT NULL").unwrap();
    let order_pos = q.sql.find("ORDER BY").unwrap();
    assert!(
        acl_pos < order_pos,
        "the ACL is conjoined BEFORE ORDER BY/LIMIT — pre-filter: {}",
        q.sql
    );
    assert!(!q.is_count);
    assert!(q
        .params
        .iter()
        .any(|p| p.placeholder == ":tenant" && p.value == "acme"));
    assert!(q
        .params
        .iter()
        .any(|p| p.placeholder == ":db_id" && p.value == "db:projects"));
}

/// **The COUNT query conjoins the ACL INSIDE the aggregate (the KN-D5 count-leak-closed shape).** The
/// `COUNT(*)` is over the JOINed/filtered set — NOT a post-count over a wider scan.
#[test]
fn count_query_conjoins_acl_inside_the_aggregate() {
    let q = compose_db_count_query(
        &SetExpr::InRelation {
            relation: RelName("read".into()),
            via_column: db_via(),
        },
        &viewer("p:alice"),
        &TenantId("acme".into()),
        "db:projects",
    );
    assert_eq!(q.statement_count(), 1);
    assert!(q.is_count);
    assert!(
        q.sql.starts_with("SELECT COUNT(*) FROM db_row"),
        "an aggregate COUNT: {}",
        q.sql
    );
    // The ACL JOIN + predicate are INSIDE the COUNT query (the count-leak is closed by construction).
    assert!(
        q.sql
            .contains("JOIN authz_visible av0 ON av0.object_id = db_row.id"),
        "{}",
        q.sql
    );
    assert!(
        q.sql.contains("AND (av0.object_id IS NOT NULL)"),
        "the ACL is conjoined into the COUNT: {}",
        q.sql
    );
    assert!(
        !q.sql.contains("ORDER BY"),
        "a COUNT has no ORDER BY/LIMIT: {}",
        q.sql
    );
}

// ───────────────────────────── the in-memory leak / COUNT proof (KN-D5 nucleus) ───────────────────

fn region() -> Region {
    Region("fr-par".into())
}

/// Build a row-restricted-db scenario: a viewer granted `read` of rows 1+2; rows secret+3 NOT granted
/// (granted to someone else). Returns the index + the candidate row set.
fn row_restricted_scenario() -> (AuthzVisibleIndex, Vec<&'static str>) {
    let idx = AuthzVisibleIndex::new();
    let candidates = vec!["row:1", "row:2", "row:secret", "row:3"];
    idx.grant(
        &TenantId("acme".into()),
        &region(),
        "p:viewer",
        "read",
        "row:1",
        "zk-0000000001",
    );
    idx.grant(
        &TenantId("acme".into()),
        &region(),
        "p:viewer",
        "read",
        "row:2",
        "zk-0000000002",
    );
    // row:secret + row:3 are granted to OTHER subjects (the leak witnesses).
    idx.grant(
        &TenantId("acme".into()),
        &region(),
        "p:other",
        "read",
        "row:secret",
        "zk-0000000003",
    );
    idx.grant(
        &TenantId("acme".into()),
        &region(),
        "p:other",
        "read",
        "row:3",
        "zk-0000000004",
    );
    (idx, candidates)
}

/// **KN-D5 nucleus: a row-restricted db never leaks a forbidden row in the VIEW — and the COUNT is
/// permission-correct (0 leak, 0 count-leak).** The viewer sees exactly rows 1+2; the COUNT is 2, NOT
/// 4 (the count cannot reveal the 2 hidden rows' existence).
#[test]
fn kn_d5_row_restricted_view_and_count_zero_leak_zero_count_leak() {
    let (idx, candidates) = row_restricted_scenario();
    let v = viewer("p:viewer");
    let lowered = lower_over_db_row_id(
        &SetExpr::InRelation {
            relation: RelName("read".into()),
            via_column: db_via(),
        },
        &v,
    );

    let visible = idx.evaluate(
        &TenantId("acme".into()),
        &region(),
        &v,
        &lowered,
        &candidates,
    );
    assert_eq!(
        visible,
        vec!["row:1".to_string(), "row:2".to_string()],
        "0 leak: only the granted rows"
    );
    assert!(
        !visible.iter().any(|r| r == "row:secret"),
        "the confidential row is ABSENT"
    );

    let count = idx.count_visible(
        &TenantId("acme".into()),
        &region(),
        &v,
        &lowered,
        &candidates,
    );
    assert_eq!(
        count, 2,
        "0 count-leak: the COUNT is 2 (the granted rows), NOT 4 — the hidden rows are uncounted"
    );
    assert_eq!(
        count,
        visible.len(),
        "the COUNT equals the listed cardinality — no second path can diverge"
    );
}

/// **An unauthorized viewer (no grants) sees an EMPTY view and a COUNT of 0** — `InRelation` with no
/// reverse-index tuple admits nothing (fail-closed, never a permissive default).
#[test]
fn unauthorized_viewer_sees_nothing_and_counts_zero() {
    let (idx, candidates) = row_restricted_scenario();
    let stranger = viewer("p:stranger");
    let lowered = lower_over_db_row_id(
        &SetExpr::InRelation {
            relation: RelName("read".into()),
            via_column: db_via(),
        },
        &stranger,
    );
    let visible = idx.evaluate(
        &TenantId("acme".into()),
        &region(),
        &stranger,
        &lowered,
        &candidates,
    );
    assert!(
        visible.is_empty(),
        "an ungranted viewer sees no rows: {visible:?}"
    );
    let count = idx.count_visible(
        &TenantId("acme".into()),
        &region(),
        &stranger,
        &lowered,
        &candidates,
    );
    assert_eq!(count, 0, "0 count-leak: an ungranted viewer counts 0");
}

/// **`None` → `WHERE false`: the view is empty and the COUNT is 0 regardless of the candidate set**
/// (the deny set is the leak-critical floor; a mutation flipping `None`→`All` is caught here).
#[test]
fn none_deny_set_empties_view_and_count() {
    let (idx, candidates) = row_restricted_scenario();
    let v = viewer("p:viewer");
    let lowered = lower_over_db_row_id(&SetExpr::None, &v);
    assert_eq!(lowered.sql_predicate, "FALSE");
    assert!(idx
        .evaluate(
            &TenantId("acme".into()),
            &region(),
            &v,
            &lowered,
            &candidates
        )
        .is_empty());
    assert_eq!(
        idx.count_visible(
            &TenantId("acme".into()),
            &region(),
            &v,
            &lowered,
            &candidates
        ),
        0
    );
}

/// **`Ids` allow-set: exactly the inlined rows survive (view + COUNT).** The materialised path.
#[test]
fn ids_allow_set_admits_exactly_those_rows() {
    let (idx, candidates) = row_restricted_scenario();
    let v = viewer("p:viewer");
    let lowered = lower_over_db_row_id(
        &SetExpr::Ids(vec![ObjectId("row:2".into()), ObjectId("row:3".into())]),
        &v,
    );
    let visible = idx.evaluate(
        &TenantId("acme".into()),
        &region(),
        &v,
        &lowered,
        &candidates,
    );
    assert_eq!(
        visible,
        vec!["row:2".to_string(), "row:3".to_string()],
        "exactly the allow-set"
    );
    assert_eq!(
        idx.count_visible(
            &TenantId("acme".into()),
            &region(),
            &v,
            &lowered,
            &candidates
        ),
        2
    );
    assert_eq!(
        lowered.filter_mode(),
        FilterMode::Ids,
        "a materialised Ids set is the Ids mode"
    );
}

/// **`Difference(All, Ids[secret])`: the otherwise-visible space MINUS an explicit deny — the secret
/// row is excluded from both view and COUNT** (the §4.1 overridden sub-page case).
#[test]
fn difference_excludes_the_overridden_row_from_view_and_count() {
    let (idx, candidates) = row_restricted_scenario();
    let v = viewer("p:viewer");
    let lowered = lower_over_db_row_id(
        &SetExpr::Difference(
            Box::new(SetExpr::All),
            Box::new(SetExpr::Ids(vec![ObjectId("row:secret".into())])),
        ),
        &v,
    );
    let visible = idx.evaluate(
        &TenantId("acme".into()),
        &region(),
        &v,
        &lowered,
        &candidates,
    );
    assert!(
        !visible.iter().any(|r| r == "row:secret"),
        "the overridden row is excluded: {visible:?}"
    );
    assert_eq!(visible.len(), 3, "All minus the one denied row");
    assert_eq!(
        idx.count_visible(
            &TenantId("acme".into()),
            &region(),
            &v,
            &lowered,
            &candidates
        ),
        3
    );
}

/// **`NotIds` deny-set evaluated through the in-memory model: exactly the NON-denied candidates
/// survive (view + COUNT).** Exercises the `<via> NOT IN (…)` leaf in the evaluator DIRECTLY (the
/// `Difference` path produces `AND NOT (IN …)`, a different parse) — a mutation that flips the `NOT IN`
/// membership (`== → !=`, `delete !`) would let a denied row LEAK / a permitted row vanish; both are
/// caught here. The mutation-critical leaf for the explicit-deny case.
#[test]
fn not_ids_deny_set_excludes_exactly_the_denied_rows_view_and_count() {
    let (idx, candidates) = row_restricted_scenario();
    let v = viewer("p:viewer");
    // Deny row:secret + row:3 directly via a NOT IN leaf (the otherwise-visible space is All-modelled
    // by the candidate universe here — NotIds alone evaluates membership of the deny-set per row).
    let lowered = lower_over_db_row_id(
        &SetExpr::NotIds(vec![
            ObjectId("row:secret".into()),
            ObjectId("row:3".into()),
        ]),
        &v,
    );
    assert_eq!(lowered.sql_predicate, "db_row.id NOT IN (:id_0, :id_1)");
    let visible = idx.evaluate(
        &TenantId("acme".into()),
        &region(),
        &v,
        &lowered,
        &candidates,
    );
    assert_eq!(
        visible,
        vec!["row:1".to_string(), "row:2".to_string()],
        "exactly the NON-denied rows survive the NOT IN leaf (a flipped membership would leak row:secret)"
    );
    assert!(
        !visible.iter().any(|r| r == "row:secret"),
        "the explicitly-denied row never survives NOT IN"
    );
    assert_eq!(
        idx.count_visible(
            &TenantId("acme".into()),
            &region(),
            &v,
            &lowered,
            &candidates
        ),
        2
    );
}

/// **`Intersect(All, NotIds[secret])` through the model: the `AND` composition denies the secret row
/// AND the COUNT is correct.** Exercises the evaluator's `AND` path (the `parse_and` boolean) so a
/// `&& → ||` / negation mutation in the boolean evaluator that would admit the denied row is caught.
#[test]
fn intersect_all_with_not_ids_denies_and_counts_correctly() {
    let (idx, candidates) = row_restricted_scenario();
    let v = viewer("p:viewer");
    let lowered = lower_over_db_row_id(
        &SetExpr::Intersect(vec![
            SetExpr::All,
            SetExpr::NotIds(vec![ObjectId("row:secret".into())]),
        ]),
        &v,
    );
    assert_eq!(lowered.sql_predicate, "(TRUE AND db_row.id NOT IN (:id_0))");
    let visible = idx.evaluate(
        &TenantId("acme".into()),
        &region(),
        &v,
        &lowered,
        &candidates,
    );
    assert!(
        !visible.iter().any(|r| r == "row:secret"),
        "the AND-denied row is excluded: {visible:?}"
    );
    assert_eq!(visible.len(), 3, "All AND NOT secret = 3 rows");
    assert_eq!(
        idx.count_visible(
            &TenantId("acme".into()),
            &region(),
            &v,
            &lowered,
            &candidates
        ),
        3
    );
}

/// **`Union(Ids[1], Ids[3])` through the model: the `OR` composition admits EITHER set (view +
/// COUNT).** Exercises the evaluator's `OR` path (`parse_or`) so a `|| → &&` mutation — which would
/// turn the union into an intersection and HIDE rows that should be visible (a different failure than a
/// leak, but a correctness break the no-leak gate must still catch) — is killed.
#[test]
fn union_admits_either_id_set_view_and_count() {
    let (idx, candidates) = row_restricted_scenario();
    let v = viewer("p:viewer");
    let lowered = lower_over_db_row_id(
        &SetExpr::Union(vec![
            SetExpr::Ids(vec![ObjectId("row:1".into())]),
            SetExpr::Ids(vec![ObjectId("row:3".into())]),
        ]),
        &v,
    );
    assert_eq!(
        lowered.sql_predicate,
        "(db_row.id IN (:id_0) OR db_row.id IN (:id_1))"
    );
    let visible = idx.evaluate(
        &TenantId("acme".into()),
        &region(),
        &v,
        &lowered,
        &candidates,
    );
    assert_eq!(
        visible,
        vec!["row:1".to_string(), "row:3".to_string()],
        "the union admits EITHER set (a `|| -> &&` mutation would wrongly intersect to the empty set)"
    );
    assert_eq!(
        idx.count_visible(
            &TenantId("acme".into()),
            &region(),
            &v,
            &lowered,
            &candidates
        ),
        2
    );
}

/// **`statement_count()` is exactly 1 for a JOIN-bearing composed query (not a hardcoded return).**
/// Pins the `ComposedQuery::statement_count` mutant (`-> 1`): a multi-statement SQL (a `;`-joined
/// second pass — the post-filter anti-pattern) would return 2, so the one-query guarantee is a REAL
/// count over the SQL, never a constant.
#[test]
fn statement_count_is_a_real_count_not_a_constant() {
    let one = compose_db_view_query(
        &SetExpr::InRelation {
            relation: RelName("read".into()),
            via_column: db_via(),
        },
        &viewer("p:a"),
        &TenantId("acme".into()),
        "db:projects",
    );
    assert_eq!(one.statement_count(), 1);
    // A hand-built two-statement query (the post-filter anti-pattern) counts as 2 — proving the method
    // counts the SQL, it is not the constant `1` the mutant would substitute.
    let two = ComposedQuery {
        sql: "SELECT 1; SELECT 2".into(),
        params: vec![],
        filter_mode: FilterMode::Ids,
        is_count: false,
    };
    assert_eq!(
        two.statement_count(),
        2,
        "statement_count is a real `;`-split count, never a constant 1"
    );
}

// ───────────────────────────── the read-your-writes / new-enemy watermark ─────────────────────────

/// **write_tuples → zookie read-your-writes: a just-revoked grant is reflected in the view + COUNT.**
/// After a revoke (advancing the watermark), the SAME lowered query drops the revoked row and the
/// COUNT decrements — the new-enemy guard (the index is at-or-after the revoke's revision).
#[test]
fn just_revoked_grant_drops_from_view_and_count_read_your_writes() {
    let (idx, candidates) = row_restricted_scenario();
    let v = viewer("p:viewer");
    let lowered = lower_over_db_row_id(
        &SetExpr::InRelation {
            relation: RelName("read".into()),
            via_column: db_via(),
        },
        &v,
    );
    // Before: rows 1+2 visible, count 2.
    assert_eq!(
        idx.count_visible(
            &TenantId("acme".into()),
            &region(),
            &v,
            &lowered,
            &candidates
        ),
        2
    );

    // Revoke row:1 (a knowledge.access.* change writes tuples; the reverse index projects it,
    // advancing the watermark — the page.acl_zookie the read carries is at-or-after this revision).
    idx.revoke(
        &TenantId("acme".into()),
        &region(),
        "p:viewer",
        "read",
        "row:1",
        "zk-0000000099",
    );

    let after = idx.evaluate(
        &TenantId("acme".into()),
        &region(),
        &v,
        &lowered,
        &candidates,
    );
    assert_eq!(
        after,
        vec!["row:2".to_string()],
        "the just-revoked row:1 is gone (read-your-writes)"
    );
    assert_eq!(
        idx.count_visible(
            &TenantId("acme".into()),
            &region(),
            &v,
            &lowered,
            &candidates
        ),
        1,
        "the COUNT decremented — a revoked grant cannot be counted stale"
    );
}

/// **The watermark serves at-or-after the required revision; falls behind otherwise (the new-enemy
/// guard — the caller falls back to per-row check rather than serving stale).**
#[test]
fn watermark_serves_at_or_after_else_behind() {
    let idx = AuthzVisibleIndex::new();
    idx.advance_watermark(&TenantId("acme".into()), &region(), "zk-0000000005");
    // A read requiring rev 3 (<= 5) → the JOIN serves.
    assert!(idx.serves(
        &TenantId("acme".into()),
        &region(),
        &Zookie("zk-0000000003".into())
    ));
    // Exactly rev 5 → still serves (at-or-after is inclusive).
    assert!(idx.serves(
        &TenantId("acme".into()),
        &region(),
        &Zookie("zk-0000000005".into())
    ));
    // Rev 7 (> 5) → behind → do NOT serve (fall back to per-row check).
    assert!(!idx.serves(
        &TenantId("acme".into()),
        &region(),
        &Zookie("zk-0000000007".into())
    ));
    // No pinned revision → always serves (default-consistency, no freshness floor).
    assert!(idx.serves(&TenantId("acme".into()), &region(), &Zookie(String::new())));
}

/// **The watermark advances monotonically; a stale (older) advance never regresses it.**
#[test]
fn watermark_is_monotone_stale_never_regresses() {
    let idx = AuthzVisibleIndex::new();
    idx.advance_watermark(&TenantId("acme".into()), &region(), "zk-0000000005");
    idx.advance_watermark(&TenantId("acme".into()), &region(), "zk-0000000002"); // stale — ignored
    assert_eq!(
        idx.watermark(&TenantId("acme".into()), &region()),
        Zookie("zk-0000000005".into())
    );
    idx.advance_watermark(&TenantId("acme".into()), &region(), "zk-0000000009"); // newer — advances
    assert_eq!(
        idx.watermark(&TenantId("acme".into()), &region()),
        Zookie("zk-0000000009".into())
    );
}

/// **Tenant isolation: a viewer's grant in tenant acme is INVISIBLE under tenant evilcorp** (no
/// cross-tenant query path — the index is keyed `(tenant, region, subject, relation)`).
#[test]
fn tenant_isolation_no_cross_tenant_read() {
    let (idx, candidates) = row_restricted_scenario();
    let v = viewer("p:viewer");
    let lowered = lower_over_db_row_id(
        &SetExpr::InRelation {
            relation: RelName("read".into()),
            via_column: db_via(),
        },
        &v,
    );
    // The viewer's acme grants do NOT carry over to evilcorp — 0 rows, 0 count.
    let cross = idx.evaluate(
        &TenantId("evilcorp".into()),
        &region(),
        &v,
        &lowered,
        &candidates,
    );
    assert!(cross.is_empty(), "no cross-tenant read: {cross:?}");
    assert_eq!(
        idx.count_visible(
            &TenantId("evilcorp".into()),
            &region(),
            &v,
            &lowered,
            &candidates
        ),
        0
    );
}

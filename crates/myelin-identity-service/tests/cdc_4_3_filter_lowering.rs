//! # The CDC pair for contract 4.3 — the `Filter` SetExpr→SQL lowering (P-ID-12 / P-070)
//!
//! **Contract-index row 4.3** (the `Filter` push-down half — the load-bearing no-N+1/no-post-filter
//! lowering). This is the dedicated provider+consumer pair the P-ID-12 TESTS field names: a
//! **board/list consumer pushing down a `Filter` against its OWN id column**.
//!
//! - the **PROVIDER** (Identity's [`lower`]) turns a `SetExpr` into the consumer-composable
//!   `(sql_predicate, joins, params)` over the consumer's own `via_column` — the §7.2 lowering
//!   (`InRelation` → the `authz_visible` JOIN; `Union`/`Intersect`/`Difference` → `OR`/`AND`/
//!   `AND NOT`; `Ids` → `IN`), one query, no N+1, no post-filter, bound params (injection-safe);
//! - the **CONSUMER** is a board/list query (e.g. an Issues board over `issue.id`, a Git PR list over
//!   `pr.id`): it takes the [`Lowered`] and assembles `SELECT … FROM <its table> <joins> WHERE
//!   (<sql_predicate>) AND <its own filters>` — it NEVER post-filters a wider set, NEVER receives an
//!   opaque blob, and the JOINs key on ITS OWN id column.
//!
//! The provider's promise (a composable predicate + JOINs + bound params over the consumer's own
//! column) and the consumer's promise (it conjoins the predicate into one query, no N+1, no
//! post-filter) are pinned here so a change to either side fails in the same CI job.

use myelin_identity::{
    AuthzIndexRef, ColRef, ObjectId, Principal, PrincipalId, PrincipalKind, RelName, SetExpr,
};
use myelin_identity_service::{lower, Lowered};
use myelin_tenancy::TenantId;

fn subject(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

/// The CONSUMER half: a board/list query that assembles ONE SQL statement from the lowered Filter,
/// conjoining the predicate over its OWN id column. Returns the assembled SQL so the CDC can pin the
/// exact one-query, no-post-filter shape. It asserts (the consumer's promise): the JOINs key on its
/// own id column, the predicate is composable boolean SQL, and every literal is a bound param.
fn consumer_assembles_one_query(table: &str, lowered: &Lowered) -> String {
    // The consumer's own additional filter (e.g. "the board's project = :board") — the point is the
    // authz predicate is ANDed into ONE WHERE, never applied as a post-filter over a wider result.
    let own_filter = format!("{table}.deleted_at IS NULL");
    let joins = lowered
        .joins
        .iter()
        .map(|j| j.clause.clone())
        .collect::<Vec<_>>()
        .join(" ");
    // The consumer's promise: every JOIN keys on ITS OWN id column.
    for j in &lowered.joins {
        assert!(
            j.clause.contains(&format!("object_id = {table}.id")),
            "the JOIN keys on the consumer's own id column: {}",
            j.clause
        );
    }
    // The consumer's promise: no interpolated literals — every param is bound.
    for p in &lowered.params {
        assert!(
            p.placeholder.starts_with(':'),
            "every param is a bound placeholder, never an interpolated literal: {p:?}"
        );
    }
    format!(
        "SELECT {table}.id FROM {table} {joins} WHERE ({pred}) AND {own}",
        pred = lowered.sql_predicate,
        own = own_filter,
    )
}

/// **The Issues-board consumer pushes down a `Filter` against `issue.id` (the §7.3 mapping) — one
/// JOIN, conjoined, no post-filter.** The provider lowers `InRelation{read}`; the consumer assembles
/// one query JOINing `authz_visible` on its own `issue.id`.
#[test]
fn cdc_4_3_board_consumer_pushes_down_filter_against_its_own_id_column() {
    let via = ColRef {
        table: "issue".into(),
        column: "id".into(),
    };
    let set_expr = SetExpr::InRelation {
        relation: RelName("read".into()),
        via_column: via.clone(),
    };

    // PROVIDER: lower the Filter.
    let lowered = lower(&set_expr, &subject("p:alice"), &via);
    assert_eq!(lowered.joins.len(), 1, "one reverse-index JOIN (no N+1)");

    // CONSUMER: assemble ONE query.
    let sql = consumer_assembles_one_query("issue", &lowered);
    assert_eq!(
        sql,
        "SELECT issue.id FROM issue \
         JOIN authz_visible av0 ON av0.object_id = issue.id \
         AND av0.subject = :subject_0 AND av0.relation = :rel_for_read \
         WHERE (av0.object_id IS NOT NULL) AND issue.deleted_at IS NULL",
        "the board conjoins the authz JOIN into ONE query over its own id column (no post-filter)"
    );
}

/// **A boolean `Union` lowers to one query with `OR` — no N+1 even across branches.** A PR-list
/// consumer sees the union of two reverse-index relations as a single JOIN-per-distinct-relation,
/// ORed in the WHERE.
#[test]
fn cdc_4_3_union_lowers_to_one_query_no_n_plus_1() {
    let via = ColRef {
        table: "pr".into(),
        column: "id".into(),
    };
    let set_expr = SetExpr::Union(vec![
        SetExpr::InRelation {
            relation: RelName("reader".into()),
            via_column: via.clone(),
        },
        SetExpr::InRelation {
            relation: RelName("writer".into()),
            via_column: via.clone(),
        },
    ]);
    let lowered = lower(&set_expr, &subject("p:alice"), &via);
    assert_eq!(
        lowered.joins.len(),
        2,
        "two distinct relations → two JOINs (still one query)"
    );
    let sql = consumer_assembles_one_query("pr", &lowered);
    assert!(
        sql.contains("WHERE ((av0.object_id IS NOT NULL OR av1.object_id IS NOT NULL))"),
        "the union ORs the two reverse-index branches in ONE WHERE: {sql}"
    );
}

/// **A `Difference` (allow EXCEPT deny) lowers to `AND NOT` — the deny is conjoined, never a
/// post-filter.** A Knowledge db-view consumer sees "all rows EXCEPT the confidential ids".
#[test]
fn cdc_4_3_difference_conjoins_the_deny_set() {
    let via = ColRef {
        table: "database_row".into(),
        column: "id".into(),
    };
    let set_expr = SetExpr::Difference(
        Box::new(SetExpr::All),
        Box::new(SetExpr::Ids(vec![ObjectId("database_row:secret".into())])),
    );
    let lowered = lower(&set_expr, &subject("p:alice"), &via);
    let sql = consumer_assembles_one_query("database_row", &lowered);
    assert!(
        sql.contains("WHERE ((TRUE AND NOT database_row.id IN (:id_0)))"),
        "the deny set is conjoined as AND NOT (one query, no post-filter): {sql}"
    );
}

/// **A `TupleSet` (the big-result materialised path) lowers to the same `authz_visible` JOIN** — the
/// consumer JOINs against the server-materialised tuple set keyed on its own id column.
#[test]
fn cdc_4_3_tuple_set_lowers_to_the_join() {
    let via = ColRef {
        table: "channel".into(),
        column: "id".into(),
    };
    let set_expr = SetExpr::TupleSet {
        index: AuthzIndexRef("watcher".into()),
    };
    let lowered = lower(&set_expr, &subject("p:alice"), &via);
    assert_eq!(lowered.joins.len(), 1);
    let sql = consumer_assembles_one_query("channel", &lowered);
    assert!(
        sql.contains("JOIN authz_visible av0 ON av0.object_id = channel.id")
            && sql.contains("av0.relation = :rel_for_watcher"),
        "the big-result path JOINs the materialised tuple set on the consumer's own id column: {sql}"
    );
}

/// **`None` lowers to `FALSE` — a denied subject's board renders nothing (leak-free).**
#[test]
fn cdc_4_3_none_renders_empty() {
    let via = ColRef {
        table: "issue".into(),
        column: "id".into(),
    };
    let lowered = lower(&SetExpr::None, &subject("p:nobody"), &via);
    let sql = consumer_assembles_one_query("issue", &lowered);
    assert!(
        sql.contains("WHERE (FALSE)"),
        "a denied subject's board is WHERE false (renders nothing): {sql}"
    );
}

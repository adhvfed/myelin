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

fn consumer_assembles_one_query(table: &str, lowered: &Lowered) -> String {
    let own_filter = format!("{table}.deleted_at IS NULL");
    let joins = lowered
        .joins
        .iter()
        .map(|j| j.clause.clone())
        .collect::<Vec<_>>()
        .join(" ");
    for j in &lowered.joins {
        assert!(
            j.clause.contains(&format!("object_id = {table}.id")),
            "the JOIN keys on the consumer's own id column: {}",
            j.clause
        );
    }
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

    let lowered = lower(&set_expr, &subject("p:alice"), &via);
    assert_eq!(lowered.joins.len(), 1, "one reverse-index JOIN (no N+1)");

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

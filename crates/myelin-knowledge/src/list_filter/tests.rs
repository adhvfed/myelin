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

#[test]
fn all_lowers_to_true_no_conjunct() {
    let l = lower_over(&SetExpr::All, &viewer("p:a"), &db_via());
    assert_eq!(l.sql_predicate, "TRUE");
    assert!(l.joins.is_empty() && l.params.is_empty());
}

#[test]
fn none_lowers_to_where_false() {
    let l = lower_over(&SetExpr::None, &viewer("p:a"), &db_via());
    assert_eq!(l.sql_predicate, "FALSE");
}

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

#[test]
fn empty_ids_lowers_to_false_never_permissive() {
    let l = lower_over(&SetExpr::Ids(vec![]), &viewer("p:a"), &db_via());
    assert_eq!(l.sql_predicate, "FALSE", "an empty allow-set sees nothing");
}

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
        "the same (viewer, relation) JOIN is emitted once - no N+1"
    );
    assert_eq!(
        l.sql_predicate,
        "(av0.object_id IS NOT NULL OR av0.object_id IS NOT NULL)"
    );
}

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
        "the ACL is conjoined BEFORE ORDER BY/LIMIT - pre-filter: {}",
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

fn region() -> Region {
    Region("fr-par".into())
}

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
        "0 count-leak: the COUNT is 2 (the granted rows), NOT 4 - the hidden rows are uncounted"
    );
    assert_eq!(
        count,
        visible.len(),
        "the COUNT equals the listed cardinality - no second path can diverge"
    );
}

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

#[test]
fn not_ids_deny_set_excludes_exactly_the_denied_rows_view_and_count() {
    let (idx, candidates) = row_restricted_scenario();
    let v = viewer("p:viewer");
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
        "the COUNT decremented - a revoked grant cannot be counted stale"
    );
}

#[test]
fn watermark_serves_at_or_after_else_behind() {
    let idx = AuthzVisibleIndex::new();
    idx.advance_watermark(&TenantId("acme".into()), &region(), "zk-0000000005");
    assert!(idx.serves(
        &TenantId("acme".into()),
        &region(),
        &Zookie("zk-0000000003".into())
    ));
    assert!(idx.serves(
        &TenantId("acme".into()),
        &region(),
        &Zookie("zk-0000000005".into())
    ));
    assert!(!idx.serves(
        &TenantId("acme".into()),
        &region(),
        &Zookie("zk-0000000007".into())
    ));
    assert!(idx.serves(&TenantId("acme".into()), &region(), &Zookie(String::new())));
}

#[test]
fn watermark_is_monotone_stale_never_regresses() {
    let idx = AuthzVisibleIndex::new();
    idx.advance_watermark(&TenantId("acme".into()), &region(), "zk-0000000005");
    idx.advance_watermark(&TenantId("acme".into()), &region(), "zk-0000000002");
    assert_eq!(
        idx.watermark(&TenantId("acme".into()), &region()),
        Zookie("zk-0000000005".into())
    );
    idx.advance_watermark(&TenantId("acme".into()), &region(), "zk-0000000009");
    assert_eq!(
        idx.watermark(&TenantId("acme".into()), &region()),
        Zookie("zk-0000000009".into())
    );
}

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

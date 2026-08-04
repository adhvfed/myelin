use myelin_ci_controlplane::{
    ci_run_id_colref, compose_run_list_query, lower_over_run_id, AuthzVisibleIndex,
};
use myelin_identity::{ObjectId, Principal, PrincipalId, PrincipalKind, RelName, SetExpr};
use myelin_tenancy::{Region, TenantId};

fn viewer(id: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId::from_token("acme"),
    )
}

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

fn region() -> Region {
    Region("fr-par".into())
}

fn provider_set_expr() -> SetExpr {
    SetExpr::InRelation {
        relation: RelName("read".into()),
        via_column: ci_run_id_colref(),
    }
}

#[test]
fn consumer_lowers_the_provider_filter_into_one_leak_free_query() {
    let q = compose_run_list_query(&provider_set_expr(), &viewer("alice"), &tenant(), &region());

    assert_eq!(q.statement_count(), 1);
    assert!(q
        .sql
        .contains("JOIN authz_visible av0 ON av0.object_id = ci_run.run_id"));
    assert!(q.sql.contains("ci_run.tenant_id = :tenant"));
    assert!(q.sql.find("WHERE").unwrap() < q.sql.find("ORDER BY").unwrap());
    assert!(q.params.iter().any(|p| p.value == "alice"));
}

#[test]
fn consumer_pushdown_is_leak_free_and_revoke_reflected() {
    let idx = AuthzVisibleIndex::new();
    let alice = viewer("alice");
    idx.grant(&tenant(), &region(), "alice", "read", "r1");
    idx.grant(&tenant(), &region(), "alice", "read", "r3");

    let lowered = lower_over_run_id(&provider_set_expr(), &alice);
    let candidates = vec![
        ObjectId("r1".into()),
        ObjectId("r2".into()),
        ObjectId("r3".into()),
    ];
    let visible = idx.evaluate(&tenant(), &region(), &alice, &lowered, &candidates);
    assert_eq!(
        visible,
        vec![ObjectId("r1".into()), ObjectId("r3".into())],
        "0 leaked rows - the confidential r2 never survives the JOIN"
    );

    idx.revoke(&tenant(), &region(), "alice", "read", "r3");
    let after = idx.evaluate(&tenant(), &region(), &alice, &lowered, &candidates);
    assert_eq!(after, vec![ObjectId("r1".into())]);
}

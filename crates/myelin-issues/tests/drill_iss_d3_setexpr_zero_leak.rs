use myelin_identity::{ObjectId, Principal, PrincipalId, PrincipalKind, RelName, SetExpr, Zookie};
use myelin_issues::planner::{issue_id_colref, lower_over_issue_id, AuthzVisibleIndex};
use myelin_tenancy::{Region, TenantId};

fn viewer(id: &str, tenant: &str) -> Principal {
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId(tenant.into()),
    )
}
fn region() -> Region {
    Region("fr-par".into())
}
fn oid(s: &str) -> ObjectId {
    ObjectId(s.into())
}

fn view_set_expr() -> SetExpr {
    let in_rel = |r: &str| SetExpr::InRelation {
        relation: RelName(r.into()),
        via_column: issue_id_colref(),
    };
    SetExpr::Union(vec![
        SetExpr::Difference(Box::new(in_rel("read")), Box::new(in_rel("confidential"))),
        in_rel("confidential_grant"),
    ])
}

#[test]
fn iss_d3_zero_escape_counter_is_zero() {
    let idx = AuthzVisibleIndex::new();
    let acme = TenantId("acme".into());
    let globex = TenantId("globex".into());

    idx.grant(&acme, &region(), "alice", "read", "ENG-1", "zk-001");
    idx.grant(&acme, &region(), "alice", "read", "ENG-2", "zk-001");
    idx.grant(&acme, &region(), "alice", "confidential", "ENG-2", "zk-001");
    idx.grant(&acme, &region(), "alice", "read", "ENG-3", "zk-001");
    idx.grant(&acme, &region(), "alice", "confidential", "ENG-3", "zk-001");
    idx.grant(
        &acme,
        &region(),
        "alice",
        "confidential_grant",
        "ENG-3",
        "zk-001",
    );

    let alice = viewer("alice", "acme");
    let lowered = lower_over_issue_id(&view_set_expr(), &alice);
    let universe = vec![oid("ENG-1"), oid("ENG-2"), oid("ENG-3"), oid("ENG-4")];

    let mut escapes = 0usize;

    let visible = idx.evaluate(&acme, &region(), &alice, &lowered, &universe);

    if visible.contains(&oid("ENG-2")) {
        escapes += 1;
    }
    if visible.contains(&oid("ENG-4")) {
        escapes += 1;
    }
    assert_eq!(
        visible,
        vec![oid("ENG-1"), oid("ENG-3")],
        "the board shows exactly the legitimately-visible issues"
    );

    let cross = idx.evaluate(&globex, &region(), &alice, &lowered, &universe);
    escapes += cross.len();
    assert!(cross.is_empty(), "no cross-tenant leak");

    assert_eq!(escapes, 0, "ISS-D3 zero-escape counter MUST be 0 (0 leak)");
}

#[test]
fn iss_d3_grant_then_revoke_reflects_under_zookie_zero_leak() {
    let idx = AuthzVisibleIndex::new();
    let acme = TenantId("acme".into());
    let bob = viewer("bob", "acme");

    idx.grant(&acme, &region(), "bob", "read", "ENG-7", "zk-010");
    idx.grant(&acme, &region(), "bob", "confidential", "ENG-7", "zk-010");
    let universe = [oid("ENG-7")];

    let before = idx.evaluate(
        &acme,
        &region(),
        &bob,
        &lower_over_issue_id(&view_set_expr(), &bob),
        &universe,
    );
    assert!(
        before.is_empty(),
        "confidential, ungranted → absent (0 leak)"
    );

    idx.grant(
        &acme,
        &region(),
        "bob",
        "confidential_grant",
        "ENG-7",
        "zk-011",
    );
    let granted_zookie = idx.watermark(&acme, &region());
    assert_eq!(granted_zookie.0, "zk-011");
    let after_grant = idx.evaluate(
        &acme,
        &region(),
        &bob,
        &lower_over_issue_id(&view_set_expr(), &bob),
        &universe,
    );
    assert_eq!(after_grant, vec![oid("ENG-7")], "the grant re-admits ENG-7");

    idx.revoke(
        &acme,
        &region(),
        "bob",
        "confidential_grant",
        "ENG-7",
        "zk-012",
    );
    let post_revoke_zookie = idx.watermark(&acme, &region());
    assert_eq!(post_revoke_zookie.0, "zk-012");

    assert!(
        idx.serves(&acme, &region(), &post_revoke_zookie),
        "the index is at-or-after the post-revoke zookie → serves"
    );
    let after_revoke = idx.evaluate(
        &acme,
        &region(),
        &bob,
        &lower_over_issue_id(&view_set_expr(), &bob),
        &universe,
    );
    assert!(
        after_revoke.is_empty(),
        "ISS-D3: the just-revoked grant is absent in the next zookie-bounded read (0 leak)"
    );

    assert!(
        !idx.serves(&acme, &region(), &Zookie("zk-013".into())),
        "a scan needing a fresher revision than the watermark falls back to check (never stale)"
    );
}

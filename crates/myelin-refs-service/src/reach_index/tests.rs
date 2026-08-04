use super::*;
use crate::backlinks::{ids_result, source_root_colref, BacklinkRead};
use crate::edge_builder::RelClass;
use myelin_identity::{
    ConsistencyMode, Principal, PrincipalId, PrincipalKind, RelName, SetExpr, Zookie,
};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn viewer(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}
fn aref(s: &str) -> ArtifactRef {
    ArtifactRef(s.into())
}
fn latest() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::BoundedStale,
    }
}

fn target_root() -> ArtifactRef {
    aref("myelin://acme/issue/issue/VIRAL-1")
}

fn edge_row(eid: &str, src: &ArtifactRef) -> EdgeRow {
    EdgeRow {
        edge_id: eid.into(),
        source: src.clone(),
        source_root: src.clone(),
        target: target_root(),
        target_root: target_root(),
        rel: "mentions".into(),
        rel_class: RelClass::Reference,
        origin_event: format!("evt-{eid}"),
        origin_actor: "principal-opaque-1".into(),
        zookie: Some("zk-1".into()),
        tombstoned: false,
    }
}

fn seed_r1(n_secret: usize, n_public: usize) -> EdgeProjection {
    let r1 = EdgeProjection::new();
    for i in 0..n_secret {
        let src = aref(&format!("myelin://acme/issue/issue/SECRET-{i}"));
        r1.upsert(&tenant(), &region(), edge_row(&format!("s-{i}"), &src));
    }
    for i in 0..n_public {
        let src = aref(&format!("myelin://acme/issue/issue/OPEN-{i}"));
        r1.upsert(&tenant(), &region(), edge_row(&format!("p-{i}"), &src));
    }
    r1
}

fn public_only_filter(
    authz: &AuthzVisibleIndex,
    viewer: &Principal,
    n_public: usize,
) -> ListObjectsResult {
    for i in 0..n_public {
        let src = format!("myelin://acme/issue/issue/OPEN-{i}");
        authz.grant(
            &tenant(),
            &region(),
            &viewer.principal_id.0,
            "view",
            &src,
            "zk-1",
        );
    }
    ListObjectsResult::Filter {
        set_expr: SetExpr::InRelation {
            relation: RelName("view".into()),
            via_column: source_root_colref(),
        },
        zookie: Zookie("zk-1".into()),
    }
}

#[test]
fn r4_is_rebuilt_from_r1() {
    let r1 = seed_r1(0, 5);
    let r4 = R4ReachIndex::new(AuthzVisibleIndex::new(), R4_READ_BUDGET_FANOUT);
    assert_eq!(
        r4.measured_fanout(&tenant(), &region(), &target_root()),
        0,
        "R4 starts empty (derived, not its own source of truth)"
    );
    r4.rebuild_from_r1(&r1, &tenant(), &region(), &target_root());
    assert_eq!(
        r4.measured_fanout(&tenant(), &region(), &target_root()),
        5,
        "R4 flattens R1's 5 live inbound edges"
    );
}

#[test]
fn r4_tracks_r1_under_incremental_upsert_and_tombstone() {
    let r4 = R4ReachIndex::new(AuthzVisibleIndex::new(), R4_READ_BUDGET_FANOUT);
    let src = aref("myelin://acme/issue/issue/OPEN-0");
    let row = edge_row("e-1", &src);

    r4.on_edge_upsert(&tenant(), &region(), &row);
    assert_eq!(r4.measured_fanout(&tenant(), &region(), &target_root()), 1);
    r4.on_edge_upsert(&tenant(), &region(), &row);
    assert_eq!(
        r4.measured_fanout(&tenant(), &region(), &target_root()),
        1,
        "a redelivered upsert is idempotent on edge_id"
    );
    r4.on_edge_tombstone(&tenant(), &region(), "e-1", &target_root());
    assert_eq!(
        r4.measured_fanout(&tenant(), &region(), &target_root()),
        0,
        "a tombstone drops R4's entry (lock-step with R1)"
    );
    r4.on_edge_tombstone(&tenant(), &region(), "e-1", &target_root());
    assert_eq!(r4.measured_fanout(&tenant(), &region(), &target_root()), 0);
}

#[test]
fn r4_upsert_of_a_tombstoned_row_is_a_drop_not_an_add() {
    let r4 = R4ReachIndex::new(AuthzVisibleIndex::new(), R4_READ_BUDGET_FANOUT);
    let src = aref("myelin://acme/issue/issue/OPEN-0");
    let mut row = edge_row("e-1", &src);
    row.tombstoned = true;
    r4.on_edge_upsert(&tenant(), &region(), &row);
    assert_eq!(
        r4.measured_fanout(&tenant(), &region(), &target_root()),
        0,
        "a tombstoned-row upsert never lands a live R4 entry"
    );
}

#[test]
fn r4_promotes_strictly_above_the_read_budget_not_at_it() {
    let budget = 3;
    let r4_at = R4ReachIndex::new(AuthzVisibleIndex::new(), budget);
    let r1_at = seed_r1(0, budget as usize);
    r4_at.rebuild_from_r1(&r1_at, &tenant(), &region(), &target_root());
    let verdict_at = r4_at.promotion_verdict(&tenant(), &region(), &target_root());
    assert!(
        !verdict_at.is_promoted(),
        "a target AT the budget serves from the CTE floor (strict >, never >=): {verdict_at:?}"
    );
    assert_eq!(verdict_at.measured_fanout(), budget);

    let r4_over = R4ReachIndex::new(AuthzVisibleIndex::new(), budget);
    let r1_over = seed_r1(0, budget as usize + 1);
    r4_over.rebuild_from_r1(&r1_over, &tenant(), &region(), &target_root());
    let verdict_over = r4_over.promotion_verdict(&tenant(), &region(), &target_root());
    assert!(
        verdict_over.is_promoted(),
        "a target OVER the budget promotes R4 (the measured-trigger): {verdict_over:?}"
    );
    assert_eq!(verdict_over.measured_fanout(), budget + 1);
}

#[test]
fn hot_artifact_fanout_telemetry_fires_and_is_named() {
    let r4 = R4ReachIndex::new(AuthzVisibleIndex::new(), 3);
    let r1 = seed_r1(0, 7);
    r4.rebuild_from_r1(&r1, &tenant(), &region(), &target_root());
    assert_eq!(r4.last_fanout_sample(), 0, "no read considered yet");
    let _ = r4.promotion_verdict(&tenant(), &region(), &target_root());
    assert_eq!(
        r4.last_fanout_sample(),
        7,
        "the hot_artifact_fanout sample reads the measured fanout"
    );
    assert_eq!(
        R4ReachIndex::HOT_ARTIFACT_FANOUT_SIGNAL,
        "refs.hot_artifact_fanout",
        "the contract-1.8 signal name"
    );
}

#[test]
fn a_cold_target_is_never_promoted() {
    let r4 = R4ReachIndex::new(AuthzVisibleIndex::new(), 3);
    assert!(
        !r4.is_promoted(&tenant(), &region(), &target_root()),
        "a target with 0 inbound edges is cold - R4 never serves it (measured, not predicted)"
    );
}

#[test]
fn r4_returns_the_same_leak_free_paginated_set_as_the_cte_floor() {
    let n_secret = 4;
    let n_public = 6;
    let r1 = seed_r1(n_secret, n_public);
    let v = viewer("viewer-1");

    let authz_cte = AuthzVisibleIndex::new();
    let lo_cte = public_only_filter(&authz_cte, &v, n_public);
    let cte = BacklinkRead::new(r1.clone(), authz_cte);
    let cte_page = cte
        .backlinks(
            &tenant(),
            &region(),
            &target_root(),
            &v,
            &lo_cte,
            &latest(),
            100,
        )
        .expect("the CTE floor read");

    let authz_r4 = AuthzVisibleIndex::new();
    let lo_r4 = public_only_filter(&authz_r4, &v, n_public);
    let r4 = R4ReachIndex::new(authz_r4, 5);
    r4.rebuild_from_r1(&r1, &tenant(), &region(), &target_root());
    assert!(
        r4.is_promoted(&tenant(), &region(), &target_root()),
        "the 10-inbound hot artifact promotes R4 over the budget of 5"
    );
    let r4_page = r4
        .backlinks(
            &tenant(),
            &region(),
            &target_root(),
            &v,
            &lo_r4,
            &latest(),
            100,
        )
        .expect("the R4 read");

    assert_eq!(
        r4_page.edges, cte_page.edges,
        "R4 must return the SAME leak-free, paginated result set as the CTE floor (REF-D3 parity)"
    );
    assert_eq!(
        r4_page.edges.len(),
        n_public,
        "only the public sources are admitted (the secret referrers are absent)"
    );
    for edge in &r4_page.edges {
        assert!(
            edge.source_root.0.contains("OPEN-"),
            "no SECRET referrer leaks through R4: {}",
            edge.source_root.0
        );
    }
    assert!(r4.r4_served_count() >= 1, "R4 served the read");
}

#[test]
fn r4_pagination_matches_the_cte_floor() {
    let n_public = 8;
    let r1 = seed_r1(0, n_public);
    let v = viewer("viewer-1");

    let authz_cte = AuthzVisibleIndex::new();
    let lo_cte = public_only_filter(&authz_cte, &v, n_public);
    let cte = BacklinkRead::new(r1.clone(), authz_cte);
    let cte_page = cte
        .backlinks(
            &tenant(),
            &region(),
            &target_root(),
            &v,
            &lo_cte,
            &latest(),
            3,
        )
        .expect("CTE read");

    let authz_r4 = AuthzVisibleIndex::new();
    let lo_r4 = public_only_filter(&authz_r4, &v, n_public);
    let r4 = R4ReachIndex::new(authz_r4, 2);
    r4.rebuild_from_r1(&r1, &tenant(), &region(), &target_root());
    let r4_page = r4
        .backlinks(
            &tenant(),
            &region(),
            &target_root(),
            &v,
            &lo_r4,
            &latest(),
            3,
        )
        .expect("R4 read");

    assert_eq!(cte_page.edges.len(), 3, "the CTE floor pages to 3");
    assert_eq!(
        r4_page.edges, cte_page.edges,
        "R4 pages to the SAME first 3 edges in the SAME order"
    );
}

#[test]
fn r4_rejects_a_zero_page() {
    let r4 = R4ReachIndex::new(AuthzVisibleIndex::new(), R4_READ_BUDGET_FANOUT);
    let v = viewer("viewer-1");
    let err = r4
        .backlinks(
            &tenant(),
            &region(),
            &target_root(),
            &v,
            &ids_result(&[], "zk-1"),
            &latest(),
            0,
        )
        .expect_err("a 0 page is malformed");
    assert_eq!(err, BacklinkError::InvalidPage);
}

#[test]
fn r4_is_tenant_first_no_cross_tenant_path() {
    let r1 = seed_r1(0, 5);
    let r4 = R4ReachIndex::new(AuthzVisibleIndex::new(), R4_READ_BUDGET_FANOUT);
    r4.rebuild_from_r1(&r1, &tenant(), &region(), &target_root());
    let other = TenantId("other-tenant".into());
    assert_eq!(
        r4.measured_fanout(&other, &region(), &target_root()),
        0,
        "another tenant's R4 partition is empty (no cross-tenant reach)"
    );
}

#[test]
fn r4_read_budget_is_the_constructed_budget() {
    let r4 = R4ReachIndex::new(AuthzVisibleIndex::new(), 1234);
    assert_eq!(r4.read_budget_fanout(), 1234);
    assert_eq!(R4_READ_BUDGET_FANOUT, 1000, "the §6.3 seed default-to-beat");
}

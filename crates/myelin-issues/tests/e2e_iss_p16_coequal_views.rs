use myelin_identity::{Principal, PrincipalId, PrincipalKind, RelName, SetExpr};
use myelin_issues::{
    board_and_roadmap_share_row, edit_on_board_reflects_on_roadmap, issue_id_colref,
    type_rank_split_is_partition, CostBudget, FacetCatalog, IssueView, PlanOutcome, RowProjection,
    BOARD_TYPE_RANK_MAX, ROADMAP_TYPE_RANK_MIN,
};
use myelin_tenancy::{Region, TenantId};

fn viewer() -> Principal {
    Principal::stub(
        PrincipalId("p:eng".into()),
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
fn zk() -> myelin_identity::Zookie {
    myelin_identity::Zookie("zk-0000000042".into())
}

fn view_acl() -> SetExpr {
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
fn edit_on_board_reflects_the_same_row_on_the_roadmap() {
    let row = RowProjection::new("ENG-1421", 0);
    assert_eq!(
        row.current_lens(),
        IssueView::Board,
        "starts on the board lens"
    );

    let roadmap_read = edit_on_board_reflects_on_roadmap(row, |r| {
        r.earliest_start = Some("2026-07-01".into());
        r.type_rank = ROADMAP_TYPE_RANK_MIN;
    });

    assert_eq!(
        roadmap_read,
        Some("ENG-1421".to_string()),
        "the roadmap reflects the SAME row id the board edited (0 drift) - ISS-D1"
    );
}

#[test]
fn edit_on_roadmap_reflects_the_same_row_on_the_board() {
    let epic = RowProjection::new("ENG-2000", ROADMAP_TYPE_RANK_MIN);
    assert_eq!(
        epic.current_lens(),
        IssueView::Roadmap,
        "an epic is on the roadmap lens"
    );

    let board_read = board_and_roadmap_share_row(&epic, &epic);
    assert_eq!(
        board_read,
        Some("ENG-2000".to_string()),
        "board and roadmap agree on the row id (co-equal, symmetric)"
    );
}

#[test]
fn all_seven_views_conjoin_the_acl_filter() {
    let acl = view_acl();
    for view in IssueView::all() {
        let outcome = view.plan(
            &acl,
            &viewer(),
            &tenant(),
            &region(),
            &zk(),
            &FacetCatalog::new(),
            &CostBudget::DEFAULT,
            10,
        );
        let PlanOutcome::ServeOltp(q) = outcome else {
            panic!(
                "{} should serve a small typed-core view on OLTP",
                view.wire_id()
            );
        };
        assert!(
            q.composed.sql.contains("authz_visible") && q.composed.sql.contains(":tenant"),
            "{} conjoins the ACL reverse-index JOIN + is tenant-scoped (no leak)",
            view.wire_id()
        );
        assert!(
            q.is_bounded(),
            "{} is paginated + statement-timeout'd",
            view.wire_id()
        );
    }
}

#[test]
fn the_type_rank_split_is_a_clean_partition() {
    assert!(type_rank_split_is_partition());
    assert_eq!(ROADMAP_TYPE_RANK_MIN, BOARD_TYPE_RANK_MAX + 1);
    for rank in -2..=8 {
        let board = IssueView::Board.keeps_type_rank(rank);
        let roadmap = IssueView::Roadmap.keeps_type_rank(rank);
        assert!(
            board ^ roadmap,
            "type_rank {rank} is in exactly one of board/roadmap"
        );
    }
}

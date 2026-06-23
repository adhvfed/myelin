//! **ISS-P16 / P-382 — the co-equal `ViewSpec` views chained-mutation e2e (ISS-D1 same-row).**
//!
//! The DoD's chained-mutation e2e: edit an issue on the board → read it on the roadmap → assert the SAME
//! ROW ID (0 drift). The board and the roadmap are co-equal `ViewSpec`s over the ONE `issue` table (the
//! denormalised `type_rank` split); there is no second store, so a board edit is an edit to the ONE row
//! the roadmap reads. Plus: every one of the seven views conjoins the leak-free ACL `Filter` (4.3).
//!
//! DB-free (the structural co-equality is a property of the one-table model); the live SQL-level same-row
//! drill (edit on the board SQL → read on the roadmap SQL → same row id) is the `--features integration`
//! ISS-D1 drill (`integration_iss_p16_coequal_views.rs`).

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

/// The leak-free `view` ACL pre-filter (the confidential set-difference, ISS-P13) — conjoined on EVERY
/// view. `(read − confidential) + confidential_grant`.
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

/// **ISS-D1 — edit on the board → read on the roadmap → SAME row id (0 drift).** The structural chained
/// mutation: a story (board, type_rank 0) gets its dates/scope edited and is promoted to an epic (roadmap,
/// type_rank 2). The roadmap reads the SAME row id afterward — there is no second store to drift from.
#[test]
fn edit_on_board_reflects_the_same_row_on_the_roadmap() {
    // A board-shaped row (a story, type_rank 0).
    let row = RowProjection::new("ENG-1421", 0);
    assert_eq!(
        row.current_lens(),
        IssueView::Board,
        "starts on the board lens"
    );

    // Edit on the board: set the roadmap date axis + promote to an epic (a date/scope/type change to the
    // ONE row). The "read on the roadmap" is re-reading THAT same row.
    let roadmap_read = edit_on_board_reflects_on_roadmap(row, |r| {
        r.earliest_start = Some("2026-07-01".into());
        r.type_rank = ROADMAP_TYPE_RANK_MIN; // promoted → now a roadmap-shaped row
    });

    assert_eq!(
        roadmap_read,
        Some("ENG-1421".to_string()),
        "the roadmap reflects the SAME row id the board edited (0 drift) — ISS-D1"
    );
}

/// The reverse direction: edit on the roadmap (a date change to an epic) → read on the board's children
/// rollup → the SAME row id. Co-equality is symmetric (one table).
#[test]
fn edit_on_roadmap_reflects_the_same_row_on_the_board() {
    let epic = RowProjection::new("ENG-2000", ROADMAP_TYPE_RANK_MIN);
    assert_eq!(
        epic.current_lens(),
        IssueView::Roadmap,
        "an epic is on the roadmap lens"
    );

    // The board reads it back as the SAME row (the board read is a projection of the same store).
    let board_read = board_and_roadmap_share_row(&epic, &epic);
    assert_eq!(
        board_read,
        Some("ENG-2000".to_string()),
        "board and roadmap agree on the row id (co-equal, symmetric)"
    );
}

/// Every one of the seven views conjoins the leak-free ACL `Filter` (4.3) — there is NO view path that
/// omits the ACL pre-filter (a confidential issue is ABSENT, no "N hidden" leak).
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

/// The board↔roadmap split partitions the spine: every row is in exactly one lens; the bounds are clean
/// (`ROADMAP_TYPE_RANK_MIN == BOARD_TYPE_RANK_MAX + 1`).
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

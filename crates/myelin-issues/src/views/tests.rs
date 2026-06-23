//! Unit tests for the co-equal `ViewSpec` views (ISS-P16 / P-382).
//!
//! These prove: (1) every view is a `ViewSpec` projection over the one table; (2) every view conjoins
//! the leak-free ACL `Filter` (4.3 — a confidential issue is ABSENT, no "N hidden" leak); (3) the
//! board↔roadmap share the row (the denormalised `type_rank` partition; ISS-D1 by row id). The live
//! same-row drill + the `<1s` board proof are the integration drills; the e2e chained mutation is
//! `tests/e2e_iss_p16_coequal_views.rs`.

use super::*;
use myelin_identity::{PrincipalId, PrincipalKind, RelName};

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
fn zk() -> Zookie {
    Zookie("zk-0000000042".into())
}

/// The leak-free `view` ACL pre-filter (the confidential set-difference, ISS-P13) — the SAME `set_expr`
/// `list_objects(viewer, "view", "issue")` returns. `(read − confidential) + confidential_grant`.
fn view_acl() -> SetExpr {
    let in_rel = |r: &str| SetExpr::InRelation {
        relation: RelName(r.into()),
        via_column: crate::planner::issue_id_colref(),
    };
    SetExpr::Union(vec![
        SetExpr::Difference(Box::new(in_rel("read")), Box::new(in_rel("confidential"))),
        in_rel("confidential_grant"),
    ])
}

// ───────────────────────────── (1) every view is a ViewSpec projection over one table ─────────────────

#[test]
fn the_seven_views_are_frozen_and_distinct() {
    let all = IssueView::all();
    assert_eq!(all.len(), 7, "the seven canonical Issues views");
    let ids: Vec<&str> = all.iter().map(|v| v.wire_id()).collect();
    assert_eq!(
        ids,
        ["board", "roadmap", "backlog", "list", "table", "cycle", "calendar"],
        "the frozen view catalogue (the persona-default landing views)"
    );
    // Every view id is distinct.
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len(), "no duplicate view ids");
}

#[test]
fn every_view_is_a_viewspec_with_the_frozen_order_field() {
    for view in IssueView::all() {
        let spec = view.spec();
        // The render kind matches the view's declared kind (13.3 ViewKind).
        assert_eq!(spec.kind, view.kind(), "{} kind", view.wire_id());
        // The order_field is ALWAYS the frozen LexoRank order_key (the last-resort tiebreak — two rows
        // never ambiguously ordered).
        assert_eq!(
            spec.order_field,
            FieldId::new(ORDER_KEY_FIELD),
            "{} carries the frozen order_key tiebreak",
            view.wire_id()
        );
        // The filter is the ONE frozen QueryAst grammar (a compiled predicate tree, never a raw stub).
        assert!(
            spec.filter.predicate().is_some(),
            "{} filter is a compiled QueryAst (one grammar)",
            view.wire_id()
        );
        // Title is always a visible field.
        assert!(
            spec.visible.contains(&FieldId::new("title")),
            "{} shows the title",
            view.wire_id()
        );
    }
}

#[test]
fn board_and_roadmap_filter_the_same_denormalised_type_rank_column() {
    // The structural co-equality seam: BOTH the board and the roadmap filter on the SAME denormalised
    // `type_rank` column — they are two slices of one table, not two object graphs.
    let board = IssueView::Board.view_filter();
    let roadmap = IssueView::Roadmap.view_filter();
    for (view, ast) in [("board", &board), ("roadmap", &roadmap)] {
        let pred = ast.predicate().expect("compiled");
        let reads_type_rank = match pred {
            Predicate::Cmp { lhs, .. } => matches!(lhs, Expr::Var(v) if v == TYPE_RANK_FIELD),
            _ => false,
        };
        assert!(reads_type_rank, "{view} filters on the type_rank column");
    }
}

// ───────────────────────────── (2) every view conjoins the leak-free Filter (4.3) ────────────────────

#[test]
fn every_view_plan_conjoins_the_acl_filter() {
    // EVERY view's executor plan goes through plan_board_query, which lowers the ACL SetExpr FIRST and
    // conjoins it. The serve-OLTP composed SQL must reference the authz_visible JOIN (the ACL pre-filter)
    // — there is NO view path that omits it.
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
        match outcome {
            PlanOutcome::ServeOltp(q) => {
                // The leak-free pre-filter is conjoined: the board SQL JOINs the authz_visible reverse
                // index AND the tenant predicate is present (no cross-tenant query path).
                assert!(
                    q.composed.sql.contains("authz_visible"),
                    "{} conjoins the ACL reverse-index JOIN (no leak)",
                    view.wire_id()
                );
                assert!(
                    q.composed.sql.contains("issue.tenant_id = :tenant"),
                    "{} is tenant-scoped",
                    view.wire_id()
                );
                // The ACL predicate is conjoined BEFORE the ORDER BY / LIMIT — never a post-filter.
                let where_pos = q.composed.sql.find("WHERE").unwrap();
                let order_pos = q.composed.sql.find("ORDER BY").unwrap();
                assert!(
                    where_pos < order_pos,
                    "{} ACL pre-filter, not post",
                    view.wire_id()
                );
                // Bounded: paginated + statement-timeout'd, never an unbounded scan.
                assert!(q.is_bounded(), "{} is bounded", view.wire_id());
            }
            other => panic!(
                "{} should serve on OLTP for a small typed-core view, got {other:?}",
                view.wire_id()
            ),
        }
    }
}

#[test]
fn a_confidential_row_is_absent_not_counted() {
    // The leak-free family: a confidential issue with no grant is ABSENT from the board (the
    // set-difference lowers it OUT) — never an "N hidden" count leak. We drive the in-memory authz model
    // exactly as the SQL JOIN would.
    use crate::planner::{lower_over_issue_id, AuthzVisibleIndex};
    use myelin_identity::ObjectId;

    let acl = view_acl();
    let lowered = lower_over_issue_id(&acl, &viewer());
    let idx = AuthzVisibleIndex::new();
    // Two readable rows; one is also confidential (no grant) → it must be ABSENT.
    idx.grant(
        &tenant(),
        &region(),
        "p:eng",
        "read",
        "ENG-1",
        "zk-0000000001",
    );
    idx.grant(
        &tenant(),
        &region(),
        "p:eng",
        "read",
        "ENG-2",
        "zk-0000000002",
    );
    idx.grant(
        &tenant(),
        &region(),
        "p:eng",
        "confidential",
        "ENG-2",
        "zk-0000000003",
    );

    let candidates = [ObjectId("ENG-1".into()), ObjectId("ENG-2".into())];
    let visible = idx.evaluate(&tenant(), &region(), &viewer(), &lowered, &candidates);
    let ids: Vec<&str> = visible.iter().map(|o| o.0.as_str()).collect();
    assert_eq!(
        ids,
        ["ENG-1"],
        "the confidential row is absent — no N-hidden leak"
    );
}

// ───────────────────────────── (3) board↔roadmap share the row (ISS-D1) ──────────────────────────────

#[test]
fn type_rank_split_partitions_the_spine() {
    // The board (type_rank ≤ 1) and the roadmap (type_rank ≥ 2) are disjoint + exhaustive: every row is
    // in exactly one lens; no row is in both.
    assert!(
        type_rank_split_is_partition(),
        "the split is a clean partition"
    );
    for rank in -1..=5 {
        let in_board = IssueView::Board.keeps_type_rank(rank);
        let in_roadmap = IssueView::Roadmap.keeps_type_rank(rank);
        assert!(
            in_board ^ in_roadmap,
            "type_rank {rank} is in exactly one lens (board={in_board}, roadmap={in_roadmap})"
        );
    }
}

#[test]
fn board_and_roadmap_resolve_the_same_row_id() {
    // ISS-D1 by row id: the SAME issue row, read through the board projection and the roadmap projection,
    // resolves to the SAME id (0 drift). There is no second store.
    let row_board = RowProjection::new("ENG-1421", 0);
    let row_roadmap = row_board.clone(); // the SAME row, projected by the roadmap lens
    let shared = board_and_roadmap_share_row(&row_board, &row_roadmap);
    assert_eq!(
        shared,
        Some("ENG-1421".to_string()),
        "board and roadmap agree on the row id (co-equal)"
    );
}

#[test]
fn an_edit_crossing_the_type_rank_boundary_moves_the_same_row_between_lenses() {
    // A story (type_rank 0, board) promoted to an epic (type_rank 2, roadmap): the SAME row id moves
    // between the two lenses — never a copy, never a drift.
    let row = RowProjection::new("ENG-1421", 0);
    assert_eq!(row.current_lens(), IssueView::Board, "starts on the board");
    assert!(row.shown_in(IssueView::Board) && !row.shown_in(IssueView::Roadmap));

    let read_back = edit_on_board_reflects_on_roadmap(row, |r| {
        // The board edit: promote the type (a denormalised type_rank change to the ONE row).
        r.type_rank = 2;
        r.earliest_start = Some("2026-07-01".into());
    });
    assert_eq!(
        read_back,
        Some("ENG-1421".to_string()),
        "the roadmap reads the SAME row id after the board edit (0 drift)"
    );
}

#[test]
fn edit_on_board_reflects_on_roadmap_same_row() {
    // The chained mutation at the model level: edit the date/scope on the board → read on the roadmap →
    // SAME row id. (The full SQL-level chained mutation is the e2e in tests/.)
    let row = RowProjection::new("ENG-2000", 2); // a roadmap-shaped epic
    let after = edit_on_board_reflects_on_roadmap(row, |r| {
        r.earliest_start = Some("2026-08-15".into());
    });
    assert_eq!(after, Some("ENG-2000".to_string()), "same row, 0 drift");
}

#[test]
fn board_and_roadmap_kinds_are_co_equal_not_two_object_graphs() {
    // The board renders as a `board`, the roadmap as a `timeline` — different RENDERINGS of the same
    // item model (the falsifiable rule: switch projection → same rows). Both project the one issue table.
    assert_eq!(IssueView::Board.kind(), ViewKind::Board);
    assert_eq!(IssueView::Roadmap.kind(), ViewKind::Timeline);
    // The cycle view shares the board rendering (a board over a cycle slice) — still one component.
    assert_eq!(IssueView::Cycle.kind(), ViewKind::Board);
}

#[test]
fn cycle_and_calendar_bind_the_frozen_membership_column() {
    assert_eq!(IssueView::Cycle.cycle_bind_column(), Some(CYCLE_FIELD));
    assert_eq!(IssueView::Calendar.cycle_bind_column(), Some(CYCLE_FIELD));
    assert_eq!(IssueView::Board.cycle_bind_column(), None);
}

#[test]
fn floors_are_named() {
    assert_eq!(ViewFloors::CROSS_CELL_ROLLUP, "ISS-P32");
    assert_eq!(ViewFloors::REALTIME_SYNC, "ISS-P30");
}

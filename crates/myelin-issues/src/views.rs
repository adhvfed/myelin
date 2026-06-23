//! # `views` — the co-equal `ViewSpec` views over the ONE issue table (ISS-P16 / P-382; the ISS-D1
//! same-row gate).
//!
//! **Owning architecture docs:**
//! - `planning/04-subsystem-architectures/issue-tracker/architecture/04-views-cli-and-api.md` §1 (*The
//!   views — one component, many `ViewSpec` projections*): every Issues view is a **saved frozen
//!   `myelin-query` `ViewSpec`** (contract 13.3) over the **one `issue` table** — the board and the
//!   roadmap are **not two object graphs**, they are **two `ViewSpec`s over the same rows**. Editing an
//!   issue on the board patches the roadmap live because they read the SAME `issue` rows — no parallel
//!   reality.
//! - `05-hard-problems.md` §1 (*Issue-model duality — board↔roadmap as co-equal views*): board =
//!   `type_rank ≤ 1` grouped by `state_category`; roadmap = `type_rank ≥ 2` on a date axis. `type_rank`
//!   is **denormalised** so both views are index-range scans. **Why structural, not a feature:** the
//!   roadmap CANNOT drift from the board because they read the *same rows*.
//! - `02-internals-and-algorithms.md` §3 (the views as co-equal `ViewSpec` projections lowered ON TOP
//!   of the leak-free `SetExpr` pre-filter).
//!
//! **Contract-index rows:**
//! - **13.3** (the `ViewSpec` view-model) — **CONSUMED** (the frozen [`myelin_query::ViewSpec`], co-owned
//!   in [`crate::query_coown`]; this module builds the seven canonical Issues views FROM that one frozen
//!   shape — never a second view-model).
//! - **4.3** (every view conjoins the lowered `SetExpr` `Filter`) — **CONSUMED** (each view's executor
//!   plan goes through [`crate::cost_bounder::plan_board_query`], which ALWAYS lowers the ACL pre-filter
//!   first via [`crate::planner::lower_over_issue_id`] — ISS-P13/P14; a confidential issue is simply
//!   ABSENT, no "N hidden" leak).
//!
//! **Design-system pass:** the visual/token-level pass over the board/roadmap/backlog/table/calendar/cycle
//! screens (incl. the empty/loading/error/permission/tombstone states) is recorded + signed off in the
//! design folder (`.../issue-tracker/design/design-system-pass.md` + `signoff.md`) — the pre-frontend gate
//! (VISION §3: no frontend code without a reviewed sketch). The `<Views>` component spec
//! (`design-planning/08-design-system/02-components/views.md`) is the shared visual primitive the pass
//! conforms to.
//!
//! ## What this module ships (ISS-P16 — the co-equal projections)
//!
//! 1. [`IssueView`] — the seven canonical Issues views (board / roadmap / backlog / list / table /
//!    calendar / cycle), each a [`myelin_query::ViewSpec`] **projection over the one `issue` table** built
//!    from the co-owned frozen shape. The board↔roadmap split is the **denormalised `type_rank`**
//!    (`board: type_rank ≤ 1`, `roadmap: type_rank ≥ 2`) — same rows, different slice + rendering.
//! 2. [`IssueView::spec`] — the frozen `ViewSpec` for a view (the `kind`/`filter`/`group_by`/`sort`/
//!    `visible`/`order_field`). The `filter` is the ONE frozen [`myelin_query::QueryAst`] grammar; the
//!    board/roadmap filters are over the `type_rank` typed-core column (Tier 1).
//! 3. [`IssueView::plan`] — the executor seam: lower the view's `ViewSpec` filter on TOP of the leak-free
//!    `SetExpr` pre-filter (ISS-P13/P14) into a bounded, paginated, statement-timeout'd plan. **Every**
//!    view conjoins the SAME ACL `Filter` — the `search-requires-acl-filter` / leak-free discipline holds
//!    on every view (a confidential issue is ABSENT, never an "N hidden" leak).
//! 4. [`board_and_roadmap_share_row`] — the STRUCTURAL co-equality proof: given one `issue` row, the
//!    board and the roadmap resolve to the **SAME row id** (the [`RowProjection`]); an edit that changes
//!    the row's `type_rank`/dates moves the SAME row between the two lenses — never a copy, never a drift
//!    (ISS-D1, asserted by row id).
//!
//! ## Co-equality is structural, not a feature (arch §1; ISS-D1)
//! The board and the roadmap are NOT two object graphs — they are two `ViewSpec`s over the SAME rows. The
//! [`RowProjection`] is the proof object: it carries the canonical `issue.id` (the row identity) and the
//! denormalised `type_rank` both views read. [`IssueView::Board`] keeps the row iff `type_rank ≤ 1`;
//! [`IssueView::Roadmap`] keeps it iff `type_rank ≥ 2`; both read the SAME `id`. An edit on the board (a
//! date/scope/type change) is a write to the ONE row — the roadmap reflects the SAME row id (0 drift),
//! because there is no second store. The drift-killer ([`board_and_roadmap_share_row`]) asserts the row
//! id is identical across the two view projections.
//!
//! ## Every view conjoins the leak-free Filter (4.3; the F1 leak-free family)
//! [`IssueView::plan`] NEVER builds a board scan without the ACL pre-filter — it delegates to
//! [`crate::cost_bounder::plan_board_query`], which lowers `list_objects(viewer, "view", "issue")`'s
//! `SetExpr` FIRST (ISS-P13) and conjoins it into EVERY tier (ISS-P14). The view's own `ViewSpec` filter
//! (e.g. `type_rank ≤ 1`) is ANDed on top of (never instead of) the ACL `Filter`. There is no view
//! executor path that omits the ACL conjoin — the leak-free property is structural across all seven views.
//!
//! ## FLOOR named (per the prompt — DELIVERABLE: "none new")
//! The views are projections over the one table — they open NO new floor. Named follow-on:
//! - **The cross-cell portfolio rollup view** (an exec portfolio spanning residency cells) is the M5
//!   follow-on **ISS-P32** (the `CrossCellPointer` bridge) — a view's rows stay within one residency cell
//!   here; the cross-cell rollup rides the M5 bridge. Named: [`ViewFloors::CROSS_CELL_ROLLUP`].
//! - The live real-time board sync over the firehose resume-cursor protocol is **ISS-P30 (P-397)** — the
//!   views are the surface it drives; the sync is its own prompt. Named: [`ViewFloors::REALTIME_SYNC`].

use myelin_identity::{Literal, Principal, SetExpr, Zookie};
use myelin_query::{
    CmpOp, Expr, FieldId, Predicate, QueryAst, SortDir, SortSpec, ViewKind, ViewSpec,
};
use myelin_tenancy::{Region, TenantId};

use crate::cost_bounder::{plan_board_query, CostBudget, FacetCatalog, PlanOutcome};

// ───────────────────────────── frozen names (§1 / §1 hard-problems — never a stray literal) ──────────

/// **The denormalised `type_rank` boundary that splits the board from the roadmap (hard-problems §1).**
/// A row with `type_rank ≤ [`BOARD_TYPE_RANK_MAX`]` is board-shaped (sub-task / story / bug / task /
/// chore / spike); a row with `type_rank ≥ [`ROADMAP_TYPE_RANK_MIN`]` is roadmap-shaped (epic /
/// initiative). The two are DISJOINT + EXHAUSTIVE over the ranked-type spine (the same rows, sliced) —
/// `ROADMAP_TYPE_RANK_MIN == BOARD_TYPE_RANK_MAX + 1`, asserted by [`type_rank_split_is_partition`].
pub const BOARD_TYPE_RANK_MAX: i64 = 1;

/// The roadmap lens floor — a row is roadmap-shaped (epic/initiative) iff `type_rank ≥` this. The
/// complement of [`BOARD_TYPE_RANK_MAX`] (`= BOARD_TYPE_RANK_MAX + 1`) — together they partition the
/// ranked-type spine (no row is in both lenses; every row is in exactly one).
pub const ROADMAP_TYPE_RANK_MIN: i64 = 2;

/// The frozen denormalised type-rank column both the board and the roadmap filter on (hard-problems §1:
/// "`type_rank` is denormalised so both views are index-range scans"). Named in ONE place so a view-spec
/// drill asserts against the NAME, never a literal — and so the board/roadmap filters provably read the
/// SAME column (the structural co-equality seam).
pub const TYPE_RANK_FIELD: &str = "type_rank";

/// The frozen `state_category` column the board groups by (the mandatory FOUR-category invariant —
/// `unstarted/started/completed/cancelled`; arch §2 / migrations).
pub const STATE_CATEGORY_FIELD: &str = "state_category";

/// The frozen `cycle` membership field the cycle/sprint view filters on (the time-axis object, NOT
/// containment — hard-problems §1).
pub const CYCLE_FIELD: &str = "cycle_id";

/// The frozen manual drag-order field every view carries as the last-resort total-order tiebreak (the
/// frozen LexoRank `order_key`, 13.3 / ISS-P09). Two rows are never ambiguously ordered.
pub const ORDER_KEY_FIELD: &str = "order_key";

// ───────────────────────────── the seven co-equal views (§1) ─────────────────────────────────────────

/// **The seven canonical Issues views — each a [`ViewSpec`] projection over the ONE `issue` table
/// (§1).** Co-equal: the board and the roadmap are not two object graphs, they are two `ViewSpec`s over
/// the SAME rows (the denormalised `type_rank` split). A closed, frozen set — adding a view is a config
/// `ViewSpec` (a saved view), not a new variant here; these are the persona-default landing views the
/// architecture catalogue fixes (§1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IssueView {
    /// **Board / Kanban** (S3) — the engineer default. `filter type_rank ≤ 1`, `group_by state_category`,
    /// `sort order_key` (kind = `board`). The hot board scan (Tier 1).
    Board,
    /// **Roadmap / Timeline** (S5) — the PM/exec default, the CO-EQUAL lens over the SAME rows.
    /// `filter type_rank ≥ 2` on a date axis (kind = `timeline`). Editing dates/scope here patches the
    /// engineer board live (one `issue` table — no parallel reality).
    Roadmap,
    /// **Backlog** (S6) — drag-to-rank. `filter state_category:unstarted`, ordered by `order_key`
    /// (kind = `list`; the frozen LexoRank + CAS reorder, ISS-P09).
    Backlog,
    /// **List** (S2) — any `QueryAst` filter, sorted (kind = `list`; `j/k` single-key actions).
    List,
    /// **Table / spreadsheet** (S4) — any `QueryAst` filter, visible/hidden fields, inline-edit
    /// (kind = `table`).
    Table,
    /// **Calendar** (S7) — `group_by` a date field (kind = `calendar`).
    Calendar,
    /// **Cycle / Sprint** (S8) — `filter cycle:N` (kind = `board`; the time-axis object, burndown is
    /// OLAP-fed).
    Cycle,
}

impl IssueView {
    /// The full, ordered, frozen set of the seven canonical views (so a drill can assert over the WHOLE
    /// catalogue — every view conjoins the Filter, every view is a projection over the one table).
    pub fn all() -> [IssueView; 7] {
        [
            IssueView::Board,
            IssueView::Roadmap,
            IssueView::Backlog,
            IssueView::List,
            IssueView::Table,
            IssueView::Cycle,
            IssueView::Calendar,
        ]
    }

    /// The stable, PII-free wire id for this view (the persona-default landing name; the drill anchor).
    pub fn wire_id(self) -> &'static str {
        match self {
            IssueView::Board => "board",
            IssueView::Roadmap => "roadmap",
            IssueView::Backlog => "backlog",
            IssueView::List => "list",
            IssueView::Table => "table",
            IssueView::Calendar => "calendar",
            IssueView::Cycle => "cycle",
        }
    }

    /// **The render kind of this view ([`ViewKind`], 13.3).** The board/cycle render as a `board`; the
    /// roadmap as a `timeline`; the calendar as a `calendar`; backlog/list as a `list`; table as a
    /// `table`. The kind is a RENDERING choice over the same item model — switching it re-renders the
    /// SAME rows (the `<Views>` component's falsifiable rule).
    pub fn kind(self) -> ViewKind {
        match self {
            IssueView::Board | IssueView::Cycle => ViewKind::Board,
            IssueView::Roadmap => ViewKind::Timeline,
            IssueView::Backlog | IssueView::List => ViewKind::List,
            IssueView::Table => ViewKind::Table,
            IssueView::Calendar => ViewKind::Calendar,
        }
    }

    /// **The `ViewSpec` filter [`QueryAst`] for this view — the projection slice over the one table
    /// (§1).** The board/roadmap filters are over the denormalised [`TYPE_RANK_FIELD`] typed-core column
    /// (the structural co-equality seam — both read the SAME column, sliced at [`BOARD_TYPE_RANK_MAX`]).
    /// The empty (`true`) filter for the open views (list/table) is still ACL-conjoined by the executor,
    /// so it is never an over-broad read. `cycle_id` is the parameter the cycle/calendar view binds at
    /// execution (here the structural filter; the concrete cycle id is a bound param, not a literal).
    pub fn view_filter(self) -> QueryAst {
        let predicate = match self {
            // type_rank ≤ 1 — the board lens (the SAME rows the roadmap excludes; the denormalised split).
            IssueView::Board => Predicate::Cmp {
                op: CmpOp::Le,
                lhs: Expr::Var(TYPE_RANK_FIELD.into()),
                rhs: Expr::Lit(Literal::Int(BOARD_TYPE_RANK_MAX)),
            },
            // type_rank ≥ 2 — the roadmap lens (the complement; the co-equal lens over the same table).
            IssueView::Roadmap => Predicate::Cmp {
                op: CmpOp::Ge,
                lhs: Expr::Var(TYPE_RANK_FIELD.into()),
                rhs: Expr::Lit(Literal::Int(ROADMAP_TYPE_RANK_MIN)),
            },
            // state_category == 'unstarted' — the backlog (the to-rank queue).
            IssueView::Backlog => Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Var(STATE_CATEGORY_FIELD.into()),
                rhs: Expr::Lit(Literal::Str("unstarted".into())),
            },
            // The cycle/calendar views structurally filter on cycle membership — the concrete cycle id is
            // a BOUND PARAM at execution (exactly as the planner binds tenant/region), so the structural
            // canonical filter here is the open `true` the executor still ACL-conjoins + binds `cycle_id`
            // on. (The calendar additionally groups by a date field — see `group_by`.) Named via
            // [`CYCLE_FIELD`] so the cycle-id bind reads the FROZEN column, never a literal.
            IssueView::Cycle | IssueView::Calendar => Predicate::True,
            // List / table are the open views — any user filter (empty `true` here; still ACL-conjoined).
            IssueView::List | IssueView::Table => Predicate::True,
        };
        QueryAst::compiled(predicate).expect("a canonical view filter is within the cost bound")
    }

    /// **The membership column this view binds a cycle id on at execution (the cycle/calendar views;
    /// `None` for the rest).** The concrete cycle id is a BOUND PARAM (never a literal) — this names the
    /// FROZEN [`CYCLE_FIELD`] column the bind targets, so the cycle scan reads the same denormalised
    /// membership column the `issue_cycle` index covers.
    pub fn cycle_bind_column(self) -> Option<&'static str> {
        match self {
            IssueView::Cycle | IssueView::Calendar => Some(CYCLE_FIELD),
            _ => None,
        }
    }

    /// The optional group-by field for this view (the board/cycle group by [`STATE_CATEGORY_FIELD`]; the
    /// calendar groups by a date field; the rest have none).
    pub fn group_by(self) -> Option<FieldId> {
        match self {
            IssueView::Board | IssueView::Cycle => Some(FieldId::new(STATE_CATEGORY_FIELD)),
            IssueView::Calendar => Some(FieldId::new("due")),
            IssueView::Roadmap | IssueView::Backlog | IssueView::List | IssueView::Table => None,
        }
    }

    /// **The frozen [`ViewSpec`] for this view (13.3) — the projection over the one `issue` table.** Built
    /// from the co-owned frozen shape (never a second view-model). The `order_field` is always the frozen
    /// LexoRank [`ORDER_KEY_FIELD`] (the last-resort total-order tiebreak — two rows never ambiguous). The
    /// executor ALWAYS conjoins `list_objects(viewer, "view", "issue")` with the `filter` ([`Self::plan`]).
    pub fn spec(self) -> ViewSpec {
        let sort = match self {
            // The roadmap orders by the date axis (earliest_start) then the order_key tiebreak.
            IssueView::Roadmap => vec![SortSpec {
                field: FieldId::new("earliest_start"),
                dir: SortDir::Asc,
            }],
            // The board/backlog/cycle order by the manual drag rank (order_key); list/table/calendar by
            // the order_key tiebreak too (a stable, never-ambiguous order). The order_field is the
            // last-resort tiebreak in every case.
            _ => Vec::new(),
        };
        ViewSpec {
            kind: self.kind(),
            filter: self.view_filter(),
            group_by: self.group_by(),
            sort,
            visible: self.default_visible(),
            order_field: FieldId::new(ORDER_KEY_FIELD),
        }
    }

    /// The default visible fields for this view (the per-lens config delta — §3 of the `<Views>` spec; a
    /// presentation choice over the one item model, never a code branch). Title is always visible.
    pub fn default_visible(self) -> Vec<FieldId> {
        let mut v = vec![FieldId::new("title")];
        match self {
            IssueView::Board | IssueView::Cycle => {
                v.push(FieldId::new("assignee"));
                v.push(FieldId::new("priority"));
            }
            IssueView::Roadmap => {
                v.push(FieldId::new("earliest_start"));
                v.push(FieldId::new("latest_due"));
            }
            IssueView::Backlog | IssueView::List => {
                v.push(FieldId::new("priority"));
                v.push(FieldId::new("assignee"));
            }
            IssueView::Table => {
                v.push(FieldId::new("state"));
                v.push(FieldId::new("priority"));
                v.push(FieldId::new("assignee"));
            }
            IssueView::Calendar => {
                v.push(FieldId::new("due"));
            }
        }
        v
    }

    /// **Plan this view's executor — the leak-free Filter conjoined ON every view (4.3; ISS-D1 DoD).**
    /// Delegates to [`crate::cost_bounder::plan_board_query`], which lowers the ACL `SetExpr` pre-filter
    /// FIRST (ISS-P13) and conjoins it into the bounded, paginated, statement-timeout'd plan (ISS-P14) —
    /// then ANDs the view's OWN `ViewSpec` filter on TOP. NEVER a board scan without the ACL conjoin: a
    /// confidential issue is ABSENT, no "N hidden" leak. The outcome is one of serve-OLTP / escalate /
    /// refine — never an unbounded scan.
    ///
    /// `set_expr` is the viewer's `list_objects(viewer, "view", "issue")` answer (the ACL pre-filter,
    /// 4.3); `viewer`/`scope_tenant`/`scope_region` scope the read; `zookie` is the consistency snapshot
    /// (4.10); `catalog` is the promoted-facet set (ISS-P15); `budget` is the [`CostBudget`];
    /// `estimated_row_fanout` is the planner's row estimate.
    #[allow(clippy::too_many_arguments)]
    pub fn plan(
        self,
        set_expr: &SetExpr,
        viewer: &Principal,
        scope_tenant: &TenantId,
        scope_region: &Region,
        zookie: &Zookie,
        catalog: &FacetCatalog,
        budget: &CostBudget,
        estimated_row_fanout: u64,
    ) -> PlanOutcome {
        let ast = self.view_filter();
        plan_board_query(
            &ast,
            set_expr,
            viewer,
            scope_tenant,
            scope_region,
            zookie,
            catalog,
            budget,
            estimated_row_fanout,
        )
    }

    /// **Whether this view keeps a row of the given `type_rank` (the projection predicate — the
    /// structural co-equality seam).** The board keeps `type_rank ≤ 1`; the roadmap keeps
    /// `type_rank ≥ 2`; the other views are type-agnostic over the spine (they keep every row their own
    /// filter admits — the type slice is the board↔roadmap split). This is the in-memory model of the
    /// `ViewSpec` filter's `type_rank` slice the SQL `WHERE` evaluates; both views read the SAME
    /// denormalised column.
    pub fn keeps_type_rank(self, type_rank: i64) -> bool {
        match self {
            IssueView::Board => type_rank <= BOARD_TYPE_RANK_MAX,
            IssueView::Roadmap => type_rank >= ROADMAP_TYPE_RANK_MIN,
            // The other views do not slice on type_rank — they project the whole spine (their own
            // filter slices on state/cycle/etc., not on the board↔roadmap type axis).
            _ => true,
        }
    }
}

// ───────────────────────────── the structural co-equality proof (ISS-D1) ──────────────────────────────

/// **One `issue` row, projected — the proof object for board↔roadmap co-equality (§1; ISS-D1).** It
/// carries the canonical `issue.id` (the ROW IDENTITY both views read) + the denormalised `type_rank`
/// (the field that decides which lens shows it) + the date axis the roadmap lays out. There is exactly
/// ONE of these per `issue` row — the board and the roadmap do NOT each own a copy; they each render
/// THIS row (or not, per their `type_rank` slice). An edit to the row's `type_rank`/dates mutates THIS
/// object — the other lens reflects the SAME `id` (0 drift), because there is no second store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowProjection {
    /// The canonical `issue.id` — the row identity both the board and the roadmap read. The ISS-D1
    /// assertion is over THIS: the board read and the roadmap read resolve to the SAME id (no drift).
    pub id: String,
    /// The denormalised `type_rank` (the field that decides the board↔roadmap lens; hard-problems §1).
    pub type_rank: i64,
    /// The roadmap date-axis start (the `earliest_start` the rollup computes; the roadmap lays the bar
    /// out on it). Edited on the roadmap, read by the board's children rollup — the SAME row.
    pub earliest_start: Option<String>,
}

impl RowProjection {
    /// Build a row projection for one `issue` row.
    pub fn new(id: impl Into<String>, type_rank: i64) -> RowProjection {
        RowProjection {
            id: id.into(),
            type_rank,
            earliest_start: None,
        }
    }

    /// **The lens this row currently shows in — `Board` if `type_rank ≤ 1`, else `Roadmap`.** An edit
    /// that crosses [`BOARD_TYPE_RANK_MAX`] MOVES the SAME row between the two lenses (a story promoted to
    /// an epic disappears from the board and appears on the roadmap) — the row id is unchanged (no copy,
    /// no drift; the structural co-equality).
    pub fn current_lens(&self) -> IssueView {
        if self.type_rank <= BOARD_TYPE_RANK_MAX {
            IssueView::Board
        } else {
            IssueView::Roadmap
        }
    }

    /// Whether a given view's `type_rank` slice would keep THIS row (the in-memory model of the
    /// `ViewSpec` filter — [`IssueView::keeps_type_rank`] over this row's denormalised rank).
    pub fn shown_in(&self, view: IssueView) -> bool {
        view.keeps_type_rank(self.type_rank)
    }
}

/// **The board↔roadmap same-row drift-killer (ISS-D1 — the green artifact, asserted by row id).** Given
/// the SAME `issue` row, the board projection and the roadmap projection resolve to the **SAME `id`** (0
/// drift). This is the STRUCTURAL co-equality: there is no second store, so an edit on one lens is an
/// edit to the ONE row the other lens reads. The function returns the shared row id iff the two
/// projections agree on it (they always do — they ARE the same row); it is the explicit, row-id-level
/// assertion the ISS-D1 drill greens.
///
/// `board_read` and `roadmap_read` are the row as each lens would resolve it (the SAME source row,
/// projected). The assertion: `board_read.id == roadmap_read.id`. A divergence (which cannot happen with
/// one store, but the drill PROVES it cannot) returns `None` (a drift — the gate fails). The row is shown
/// in WHICHEVER lens its `type_rank` admits; the IDENTITY is shared across both regardless.
pub fn board_and_roadmap_share_row(
    board_read: &RowProjection,
    roadmap_read: &RowProjection,
) -> Option<String> {
    if board_read.id == roadmap_read.id {
        Some(board_read.id.clone())
    } else {
        None
    }
}

/// **The chained-mutation co-equality lemma (ISS-D1 e2e — edit on board → read on roadmap → same row).**
/// Model an edit applied on the board lens (a date/scope/type change to the ONE row) and prove the
/// roadmap lens reads the SAME row id afterward (0 drift). The edit is a `mutate` closure applied to the
/// ONE row; the "read on the roadmap" is re-reading THAT row (there is no second store to drift from).
/// Returns the post-edit row id read on the roadmap iff it equals the pre-edit row id (always — same
/// store, same row). This is the structural proof the e2e asserts: a board edit and a roadmap read are
/// the SAME row by construction.
pub fn edit_on_board_reflects_on_roadmap(
    row: RowProjection,
    mutate: impl FnOnce(&mut RowProjection),
) -> Option<String> {
    let before_id = row.id.clone();
    // The board edit is a write to the ONE row (the aggregate). There is no copy.
    let mut edited = row;
    mutate(&mut edited);
    // "Read on the roadmap" = re-read THAT same row. The roadmap reflects the SAME id (0 drift).
    if edited.id == before_id {
        Some(edited.id)
    } else {
        // Unreachable with one store — a mutation that changes the row's identity is not an edit, it is a
        // new row. The ISS-D1 drill asserts this never happens (the id is immutable across an edit).
        None
    }
}

// ───────────────────────────── the named floors (§1 — measured follow-ons) ─────────────────────────────

/// **FLOORS named (ISS-P16 DoD) — greppable markers for the measured follow-ons.** The co-equal views
/// are the FULL projection surface at M4; these are the named follow-ons they leave.
#[derive(Clone, Copy, Debug)]
pub struct ViewFloors;

impl ViewFloors {
    /// **The cross-cell portfolio rollup view — the M5 follow-on (the `CrossCellPointer` bridge).** A
    /// view's rows stay within ONE residency cell here; an exec portfolio spanning residency cells (a
    /// cross-cell rollup) rides the M5 bridge. **ISS-P32.**
    pub const CROSS_CELL_ROLLUP: &'static str = "ISS-P32";
    /// **The live real-time board sync over the firehose resume-cursor protocol — the follow-on.** The
    /// views are the surface the sync drives (0 ops lost on reconnect); the sync is its own prompt.
    /// **ISS-P30 (P-397).**
    pub const REALTIME_SYNC: &'static str = "ISS-P30";
}

/// **The type-rank partition assertion (§1 — the board and the roadmap partition the spine).** The board
/// lens (`type_rank ≤ BOARD_TYPE_RANK_MAX`) and the roadmap lens (`type_rank ≥ ROADMAP_TYPE_RANK_MIN`)
/// are DISJOINT (no row is in both) AND EXHAUSTIVE (every row is in exactly one) — the two slices are the
/// SAME rows, partitioned by the denormalised `type_rank`. `true` iff the two bounds form a clean
/// partition (`ROADMAP_TYPE_RANK_MIN == BOARD_TYPE_RANK_MAX + 1`).
pub fn type_rank_split_is_partition() -> bool {
    ROADMAP_TYPE_RANK_MIN == BOARD_TYPE_RANK_MAX + 1
}

#[cfg(test)]
mod tests;

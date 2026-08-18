use myelin_identity::{Literal, Principal, SetExpr, Zookie};
use myelin_query::{
    CmpOp, Expr, FieldId, Predicate, QueryAst, SortDir, SortSpec, ViewKind, ViewSpec,
};
use myelin_tenancy::{Region, TenantId};

use crate::cost_bounder::{plan_board_query, CostBudget, FacetCatalog, PlanOutcome};

pub const BOARD_TYPE_RANK_MAX: i64 = 1;

pub const ROADMAP_TYPE_RANK_MIN: i64 = 2;

pub const TYPE_RANK_FIELD: &str = "type_rank";

pub const STATE_CATEGORY_FIELD: &str = "state_category";

pub const CYCLE_FIELD: &str = "cycle_id";

pub const ORDER_KEY_FIELD: &str = "order_key";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IssueView {
    Board,
    Roadmap,
    Backlog,
    List,
    Table,
    Calendar,
    Cycle,
}

impl IssueView {
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

    pub fn kind(self) -> ViewKind {
        match self {
            IssueView::Board | IssueView::Cycle => ViewKind::Board,
            IssueView::Roadmap => ViewKind::Timeline,
            IssueView::Backlog | IssueView::List => ViewKind::List,
            IssueView::Table => ViewKind::Table,
            IssueView::Calendar => ViewKind::Calendar,
        }
    }

    pub fn view_filter(self) -> QueryAst {
        let predicate = match self {
            IssueView::Board => Predicate::Cmp {
                op: CmpOp::Le,
                lhs: Expr::Var(TYPE_RANK_FIELD.into()),
                rhs: Expr::Lit(Literal::Int(BOARD_TYPE_RANK_MAX)),
            },
            IssueView::Roadmap => Predicate::Cmp {
                op: CmpOp::Ge,
                lhs: Expr::Var(TYPE_RANK_FIELD.into()),
                rhs: Expr::Lit(Literal::Int(ROADMAP_TYPE_RANK_MIN)),
            },
            IssueView::Backlog => Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: Expr::Var(STATE_CATEGORY_FIELD.into()),
                rhs: Expr::Lit(Literal::Str("unstarted".into())),
            },
            IssueView::Cycle | IssueView::Calendar => Predicate::True,
            IssueView::List | IssueView::Table => Predicate::True,
        };
        QueryAst::compiled(predicate).expect("a canonical view filter is within the cost bound")
    }

    pub fn cycle_bind_column(self) -> Option<&'static str> {
        match self {
            IssueView::Cycle | IssueView::Calendar => Some(CYCLE_FIELD),
            _ => None,
        }
    }

    pub fn group_by(self) -> Option<FieldId> {
        match self {
            IssueView::Board | IssueView::Cycle => Some(FieldId::new(STATE_CATEGORY_FIELD)),
            IssueView::Calendar => Some(FieldId::new("due")),
            IssueView::Roadmap | IssueView::Backlog | IssueView::List | IssueView::Table => None,
        }
    }

    pub fn spec(self) -> ViewSpec {
        let sort = match self {
            IssueView::Roadmap => vec![SortSpec {
                field: FieldId::new("earliest_start"),
                dir: SortDir::Asc,
            }],
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

    pub fn keeps_type_rank(self, type_rank: i64) -> bool {
        match self {
            IssueView::Board => type_rank <= BOARD_TYPE_RANK_MAX,
            IssueView::Roadmap => type_rank >= ROADMAP_TYPE_RANK_MIN,
            _ => true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowProjection {
    pub id: String,
    pub type_rank: i64,
    pub earliest_start: Option<String>,
}

impl RowProjection {
    pub fn new(id: impl Into<String>, type_rank: i64) -> RowProjection {
        RowProjection {
            id: id.into(),
            type_rank,
            earliest_start: None,
        }
    }

    pub fn current_lens(&self) -> IssueView {
        if self.type_rank <= BOARD_TYPE_RANK_MAX {
            IssueView::Board
        } else {
            IssueView::Roadmap
        }
    }

    pub fn shown_in(&self, view: IssueView) -> bool {
        view.keeps_type_rank(self.type_rank)
    }
}

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

pub fn edit_on_board_reflects_on_roadmap(
    row: RowProjection,
    mutate: impl FnOnce(&mut RowProjection),
) -> Option<String> {
    let before_id = row.id.clone();
    let mut edited = row;
    mutate(&mut edited);
    if edited.id == before_id {
        Some(edited.id)
    } else {
        None
    }
}

pub fn type_rank_split_is_partition() -> bool {
    ROADMAP_TYPE_RANK_MIN == BOARD_TYPE_RANK_MAX + 1
}

#[cfg(test)]
mod tests;

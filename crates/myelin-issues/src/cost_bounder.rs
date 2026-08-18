use myelin_identity::{Principal, SetExpr, Zookie};
use myelin_query::{Expr, Predicate, QueryAst};
use myelin_tenancy::{Region, TenantId};

use crate::planner::{compose_board_query, lower_over_issue_id, BoundParam, ComposedBoardQuery};
use crate::schemes::IndexPosture;

pub const TYPED_CORE_FIELDS: &[&str] = &[
    "state",
    "state_category",
    "priority",
    "assignee",
    "reporter",
    "type",
    "type_id",
    "type_rank",
    "parent",
    "parent_id",
    "project",
    "project_id",
    "cycle",
    "cycle_id",
    "rank",
    "created_at",
    "updated_at",
    "state_changed_at",
];

pub const TIER3_FIELDS: &[&str] = &["text", "body", "fulltext", "semantic", "any_artifact"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    TypedCore,
    GeneratedFacet,
    GinProbe,
    Search,
}

impl Tier {
    pub fn cost_weight(self) -> u64 {
        match self {
            Tier::TypedCore => 1,
            Tier::GeneratedFacet => 2,
            Tier::GinProbe => 8,
            Tier::Search => 1,
        }
    }

    pub fn is_oltp(self) -> bool {
        !matches!(self, Tier::Search)
    }
}

#[derive(Clone, Debug, Default)]
pub struct FacetCatalog {
    promoted: Vec<String>,
}

impl FacetCatalog {
    pub fn new() -> FacetCatalog {
        FacetCatalog::default()
    }

    pub fn promote(&mut self, field_id: impl Into<String>) -> &mut FacetCatalog {
        let id = field_id.into();
        if !self.promoted.iter().any(|f| f == &id) {
            self.promoted.push(id);
        }
        self
    }

    pub fn posture(&self, field_id: &str) -> IndexPosture {
        if self.promoted.iter().any(|f| f == field_id) {
            IndexPosture::GeneratedIndex
        } else {
            IndexPosture::Gin
        }
    }
}

pub fn classify_field(field: &str, catalog: &FacetCatalog) -> Tier {
    if TYPED_CORE_FIELDS.contains(&field) {
        return Tier::TypedCore;
    }
    if TIER3_FIELDS.contains(&field) {
        return Tier::Search;
    }
    match catalog.posture(field) {
        IndexPosture::GeneratedIndex => Tier::GeneratedFacet,
        IndexPosture::Gin => Tier::GinProbe,
    }
}

fn predicate_fields(pred: &Predicate) -> Vec<String> {
    fn walk(p: &Predicate, out: &mut Vec<String>) {
        match p {
            Predicate::True | Predicate::False => {}
            Predicate::Cmp { lhs, rhs, .. } => {
                if let Expr::Var(v) = lhs {
                    out.push(v.clone());
                }
                if let Expr::Var(v) = rhs {
                    out.push(v.clone());
                }
            }
            Predicate::And(ps) | Predicate::Or(ps) => ps.iter().for_each(|p| walk(p, out)),
            Predicate::Not(p) => walk(p, out),
        }
    }
    let mut out = Vec::new();
    walk(pred, &mut out);
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CostBudget {
    pub max_scanned_cost: u64,
    pub statement_timeout_ms: u64,
    pub page_limit: u32,
    pub refine_cost_ceiling: u64,
}

impl CostBudget {
    pub const DEFAULT: CostBudget = CostBudget {
        max_scanned_cost: 50_000,
        statement_timeout_ms: 900,
        page_limit: 50,
        refine_cost_ceiling: 5_000_000,
    };

    pub fn new(
        max_scanned_cost: u64,
        statement_timeout_ms: u64,
        page_limit: u32,
        refine_cost_ceiling: u64,
    ) -> CostBudget {
        CostBudget {
            max_scanned_cost,
            statement_timeout_ms,
            page_limit,
            refine_cost_ceiling,
        }
    }
}

impl Default for CostBudget {
    fn default() -> CostBudget {
        CostBudget::DEFAULT
    }
}

pub fn estimate_cost(tiers: &[Tier], estimated_row_fanout: u64) -> u64 {
    let bottleneck_weight: u64 = tiers
        .iter()
        .filter(|t| t.is_oltp())
        .map(|t| t.cost_weight())
        .max()
        .unwrap_or(0);
    estimated_row_fanout.saturating_mul(bottleneck_weight)
}

#[derive(Clone, Debug)]
pub struct SearchEscalation {
    pub ast: QueryAst,
    pub set_expr: SetExpr,
    pub zookie: Zookie,
    pub page_limit: u32,
}

impl SearchEscalation {
    pub fn new(
        ast: QueryAst,
        set_expr: SetExpr,
        zookie: Zookie,
        page_limit: u32,
    ) -> SearchEscalation {
        SearchEscalation {
            ast,
            set_expr,
            zookie,
            page_limit,
        }
    }

    pub fn to_board_query(&self) -> myelin_search::BoardQuery {
        myelin_search::BoardQuery::new(self.ast.clone(), self.set_expr.clone(), self.zookie.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefineHint {
    pub hint: String,
    pub estimated_cost: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundedBoardQuery {
    pub composed: ComposedBoardQuery,
    pub statement_timeout_ms: u64,
    pub page_limit: u32,
    pub tier: Tier,
}

impl BoundedBoardQuery {
    pub fn params(&self) -> Vec<BoundParam> {
        let mut params = self.composed.params.clone();
        params.push(BoundParam {
            placeholder: ":page".into(),
            value: self.page_limit.to_string(),
        });
        params.push(BoundParam {
            placeholder: ":statement_timeout_ms".into(),
            value: self.statement_timeout_ms.to_string(),
        });
        params
    }

    pub fn is_bounded(&self) -> bool {
        self.composed.sql.contains("LIMIT :page")
            && self.page_limit > 0
            && self.statement_timeout_ms > 0
    }
}

#[derive(Clone, Debug)]
pub enum PlanOutcome {
    ServeOltp(BoundedBoardQuery),
    EscalateToSearch(SearchEscalation),
    Refine(RefineHint),
}

impl PlanOutcome {
    pub fn is_serve_oltp(&self) -> bool {
        matches!(self, PlanOutcome::ServeOltp(_))
    }
    pub fn is_escalate(&self) -> bool {
        matches!(self, PlanOutcome::EscalateToSearch(_))
    }
    pub fn is_refine(&self) -> bool {
        matches!(self, PlanOutcome::Refine(_))
    }

    pub fn assert_no_unbounded_scan(&self) -> bool {
        match self {
            PlanOutcome::ServeOltp(q) => q.is_bounded(),
            PlanOutcome::EscalateToSearch(e) => e.page_limit > 0,
            PlanOutcome::Refine(_) => true,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn plan_board_query(
    ast: &QueryAst,
    set_expr: &SetExpr,
    viewer: &Principal,
    scope_tenant: &TenantId,
    scope_region: &Region,
    zookie: &Zookie,
    catalog: &FacetCatalog,
    budget: &CostBudget,
    estimated_row_fanout: u64,
) -> PlanOutcome {
    let fields = ast.predicate().map(predicate_fields).unwrap_or_default();
    let tiers: Vec<Tier> = if fields.is_empty() {
        vec![Tier::TypedCore]
    } else {
        fields.iter().map(|f| classify_field(f, catalog)).collect()
    };

    let oltp_cost = estimate_cost(&tiers, estimated_row_fanout);
    let has_tier3_leg = tiers.iter().any(|t| matches!(t, Tier::Search));
    let over_budget = oltp_cost > budget.max_scanned_cost;

    if has_tier3_leg || over_budget {
        if estimated_row_fanout > budget.refine_cost_ceiling {
            return PlanOutcome::Refine(RefineHint {
                hint: format!(
                    "this query would scan ~{estimated_row_fanout} rows - narrow it (add a project, \
                     assignee, or date filter) so it fits the board budget",
                ),
                estimated_cost: oltp_cost.max(estimated_row_fanout),
            });
        }
        return PlanOutcome::EscalateToSearch(SearchEscalation::new(
            ast.clone(),
            set_expr.clone(),
            zookie.clone(),
            budget.page_limit,
        ));
    }

    let tier = tiers
        .iter()
        .copied()
        .filter(|t| t.is_oltp())
        .max_by_key(|t| t.cost_weight())
        .unwrap_or(Tier::TypedCore);
    let composed = compose_board_query(set_expr, viewer, scope_tenant, scope_region);
    PlanOutcome::ServeOltp(BoundedBoardQuery {
        composed,
        statement_timeout_ms: budget.statement_timeout_ms,
        page_limit: budget.page_limit,
        tier,
    })
}

pub use crate::planner::LoweredFilter;

pub fn lower_acl(set_expr: &SetExpr, viewer: &Principal) -> LoweredFilter {
    lower_over_issue_id(set_expr, viewer)
}

#[cfg(test)]
mod tests;

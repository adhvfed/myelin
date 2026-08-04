use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use myelin_identity::{Literal, Principal, SetExpr};
use myelin_query::{FieldId, FieldValue};
use myelin_tenancy::{Region, TenantId};

use crate::database::{PropertyBag, RelationKind, RelationStore};
use crate::list_filter::{lower_over_db_row_id, AuthzVisibleIndex};

pub const MAX_FORMULA_NODES: usize = 256;

pub const MAX_FORMULA_DEPTH: usize = 32;

pub const MAX_DEPENDENCY_DEPTH: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RollupFn {
    Count,
    Sum,
    Min,
    Max,
    Avg,
}

impl RollupFn {
    pub fn wire_id(self) -> &'static str {
        match self {
            RollupFn::Count => "count",
            RollupFn::Sum => "sum",
            RollupFn::Min => "min",
            RollupFn::Max => "max",
            RollupFn::Avg => "avg",
        }
    }

    fn apply(self, target_values: &[i64], visible_count: usize) -> RollupOutcome {
        match self {
            RollupFn::Count => RollupOutcome::Value(visible_count as i64),
            RollupFn::Sum => RollupOutcome::Value(target_values.iter().sum()),
            RollupFn::Avg => {
                if target_values.is_empty() {
                    RollupOutcome::Value(0)
                } else {
                    let sum: i64 = target_values.iter().sum();
                    RollupOutcome::Value(sum / target_values.len() as i64)
                }
            }
            RollupFn::Min => match target_values.iter().min() {
                Some(m) => RollupOutcome::Value(*m),
                None => RollupOutcome::Empty,
            },
            RollupFn::Max => match target_values.iter().max() {
                Some(m) => RollupOutcome::Value(*m),
                None => RollupOutcome::Empty,
            },
        }
    }
}

enum RollupOutcome {
    Value(i64),
    Empty,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormulaExpr {
    Lit(Literal),
    Prop(FieldId),
    Rollup {
        func: RollupFn,
        target: FieldId,
    },
    FormulaRef(FieldId),
    Add(Box<FormulaExpr>, Box<FormulaExpr>),
    Sub(Box<FormulaExpr>, Box<FormulaExpr>),
    Mul(Box<FormulaExpr>, Box<FormulaExpr>),
    Div(Box<FormulaExpr>, Box<FormulaExpr>),
}

impl FormulaExpr {
    fn node_count(&self) -> usize {
        match self {
            FormulaExpr::Lit(_)
            | FormulaExpr::Prop(_)
            | FormulaExpr::Rollup { .. }
            | FormulaExpr::FormulaRef(_) => 1,
            FormulaExpr::Add(a, b)
            | FormulaExpr::Sub(a, b)
            | FormulaExpr::Mul(a, b)
            | FormulaExpr::Div(a, b) => 1 + a.node_count() + b.node_count(),
        }
    }

    fn depth(&self) -> usize {
        match self {
            FormulaExpr::Lit(_)
            | FormulaExpr::Prop(_)
            | FormulaExpr::Rollup { .. }
            | FormulaExpr::FormulaRef(_) => 1,
            FormulaExpr::Add(a, b)
            | FormulaExpr::Sub(a, b)
            | FormulaExpr::Mul(a, b)
            | FormulaExpr::Div(a, b) => 1 + a.depth().max(b.depth()),
        }
    }

    pub fn formula_refs(&self, out: &mut BTreeSet<FieldId>) {
        match self {
            FormulaExpr::FormulaRef(f) => {
                out.insert(f.clone());
            }
            FormulaExpr::Add(a, b)
            | FormulaExpr::Sub(a, b)
            | FormulaExpr::Mul(a, b)
            | FormulaExpr::Div(a, b) => {
                a.formula_refs(out);
                b.formula_refs(out);
            }
            FormulaExpr::Lit(_) | FormulaExpr::Prop(_) | FormulaExpr::Rollup { .. } => {}
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormulaField {
    pub field: FieldId,
    pub expr: FormulaExpr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormulaSchemaError {
    DuplicateField(String),
    TooLarge {
        field: String,
        nodes: usize,
    },
    TooDeep {
        field: String,
        depth: usize,
    },
}

impl std::fmt::Display for FormulaSchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormulaSchemaError::DuplicateField(id) => {
                write!(f, "duplicate formula field definition `{id}`")
            }
            FormulaSchemaError::TooLarge { field, nodes } => write!(
                f,
                "formula `{field}` exceeds the static node ceiling ({nodes} > {MAX_FORMULA_NODES})"
            ),
            FormulaSchemaError::TooDeep { field, depth } => write!(
                f,
                "formula `{field}` exceeds the static depth ceiling ({depth} > {MAX_FORMULA_DEPTH})"
            ),
        }
    }
}

impl std::error::Error for FormulaSchemaError {}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FormulaSchema {
    fields: Vec<FormulaField>,
}

impl FormulaSchema {
    pub fn of(
        fields: impl IntoIterator<Item = FormulaField>,
    ) -> Result<FormulaSchema, FormulaSchemaError> {
        let fields: Vec<FormulaField> = fields.into_iter().collect();
        let mut seen = BTreeSet::new();
        for ff in &fields {
            if !seen.insert(ff.field.clone()) {
                return Err(FormulaSchemaError::DuplicateField(ff.field.to_string()));
            }
            Self::validate(&ff.field, &ff.expr)?;
        }
        Ok(FormulaSchema { fields })
    }

    pub fn validate(field: &FieldId, expr: &FormulaExpr) -> Result<(), FormulaSchemaError> {
        let nodes = expr.node_count();
        if nodes > MAX_FORMULA_NODES {
            return Err(FormulaSchemaError::TooLarge {
                field: field.to_string(),
                nodes,
            });
        }
        let depth = expr.depth();
        if depth > MAX_FORMULA_DEPTH {
            return Err(FormulaSchemaError::TooDeep {
                field: field.to_string(),
                depth,
            });
        }
        Ok(())
    }

    pub fn fields(&self) -> &[FormulaField] {
        &self.fields
    }

    pub fn formula(&self, field: &FieldId) -> Option<&FormulaExpr> {
        self.fields
            .iter()
            .find(|ff| &ff.field == field)
            .map(|ff| &ff.expr)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CellValue {
    Int(i64),
    Str(String),
    Bool(bool),
    Cycle,
    Error,
    Empty,
}

impl CellValue {
    pub fn display(&self) -> String {
        match self {
            CellValue::Int(n) => n.to_string(),
            CellValue::Str(s) => s.clone(),
            CellValue::Bool(b) => b.to_string(),
            CellValue::Cycle => "#CYCLE".to_string(),
            CellValue::Error => "#ERROR".to_string(),
            CellValue::Empty => "#EMPTY".to_string(),
        }
    }

    pub fn is_diagnostic(&self) -> bool {
        matches!(self, CellValue::Cycle | CellValue::Error | CellValue::Empty)
    }
}

pub struct RollupResolver<'a> {
    tenant: &'a TenantId,
    region: &'a Region,
    relations: &'a RelationStore,
    authz: &'a AuthzVisibleIndex,
    target_props: &'a BTreeMap<String, PropertyBag>,
}

impl<'a> RollupResolver<'a> {
    pub fn new(
        tenant: &'a TenantId,
        region: &'a Region,
        relations: &'a RelationStore,
        authz: &'a AuthzVisibleIndex,
        target_props: &'a BTreeMap<String, PropertyBag>,
    ) -> RollupResolver<'a> {
        RollupResolver {
            tenant,
            region,
            relations,
            authz,
            target_props,
        }
    }

    fn visible_target_ids(&self, viewer: &Principal, src_row: &str) -> Vec<String> {
        let edges = self
            .relations
            .relations_from(self.tenant, src_row, RelationKind::RollupSource);
        let candidate_ids: Vec<String> = edges.iter().map(|e| e.dst_ref.0.clone()).collect();
        let candidate_refs: Vec<&str> = candidate_ids.iter().map(|s| s.as_str()).collect();
        let lowered = lower_over_db_row_id(
            &SetExpr::InRelation {
                relation: myelin_identity::RelName("read".into()),
                via_column: crate::list_filter::db_row_id_colref(),
            },
            viewer,
        );
        self.authz
            .evaluate(self.tenant, self.region, viewer, &lowered, &candidate_refs)
    }

    fn compute(
        &self,
        viewer: &Principal,
        src_row: &str,
        func: RollupFn,
        target: &FieldId,
    ) -> CellValue {
        let visible = self.visible_target_ids(viewer, src_row);
        let mut target_values: Vec<i64> = Vec::new();
        if func != RollupFn::Count {
            for id in &visible {
                if let Some(props) = self.target_props.get(id) {
                    if let Some(FieldValue::Int(n)) = props.get(target) {
                        target_values.push(*n);
                    }
                }
            }
        }
        match func.apply(&target_values, visible.len()) {
            RollupOutcome::Value(v) => CellValue::Int(v),
            RollupOutcome::Empty => CellValue::Empty,
        }
    }
}

pub fn compute_row(
    viewer: &Principal,
    src_row: &str,
    field: &FieldId,
    props: &PropertyBag,
    formulas: &FormulaSchema,
    rollups: &RollupResolver<'_>,
) -> CellValue {
    let Some(expr) = formulas.formula(field) else {
        return match props.get(field) {
            Some(v) => field_value_to_cell(v),
            None => CellValue::Error,
        };
    };
    let mut visiting: BTreeSet<FieldId> = BTreeSet::new();
    visiting.insert(field.clone());
    eval_expr(
        viewer,
        src_row,
        expr,
        props,
        formulas,
        rollups,
        &mut visiting,
        0,
    )
}

#[allow(clippy::too_many_arguments)]
fn eval_expr(
    viewer: &Principal,
    src_row: &str,
    expr: &FormulaExpr,
    props: &PropertyBag,
    formulas: &FormulaSchema,
    rollups: &RollupResolver<'_>,
    visiting: &mut BTreeSet<FieldId>,
    depth: usize,
) -> CellValue {
    if depth > MAX_DEPENDENCY_DEPTH {
        return CellValue::Cycle;
    }
    match expr {
        FormulaExpr::Lit(lit) => literal_to_cell(lit),
        FormulaExpr::Prop(field) => match props.get(field) {
            Some(v) => field_value_to_cell(v),
            None => CellValue::Error,
        },
        FormulaExpr::Rollup { func, target } => rollups.compute(viewer, src_row, *func, target),
        FormulaExpr::FormulaRef(field) => {
            let Some(dep_expr) = formulas.formula(field) else {
                return CellValue::Error;
            };
            if !visiting.insert(field.clone()) {
                return CellValue::Cycle;
            }
            let v = eval_expr(
                viewer,
                src_row,
                dep_expr,
                props,
                formulas,
                rollups,
                visiting,
                depth + 1,
            );
            visiting.remove(field);
            v
        }
        FormulaExpr::Add(a, b) => arith(
            viewer,
            src_row,
            a,
            b,
            props,
            formulas,
            rollups,
            visiting,
            depth,
            |x, y| Some(x.wrapping_add(y)),
        ),
        FormulaExpr::Sub(a, b) => arith(
            viewer,
            src_row,
            a,
            b,
            props,
            formulas,
            rollups,
            visiting,
            depth,
            |x, y| Some(x.wrapping_sub(y)),
        ),
        FormulaExpr::Mul(a, b) => arith(
            viewer,
            src_row,
            a,
            b,
            props,
            formulas,
            rollups,
            visiting,
            depth,
            |x, y| Some(x.wrapping_mul(y)),
        ),
        FormulaExpr::Div(a, b) => arith(
            viewer,
            src_row,
            a,
            b,
            props,
            formulas,
            rollups,
            visiting,
            depth,
            |x, y| {
                if y == 0 {
                    None
                } else {
                    Some(x / y)
                }
            },
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn arith(
    viewer: &Principal,
    src_row: &str,
    a: &FormulaExpr,
    b: &FormulaExpr,
    props: &PropertyBag,
    formulas: &FormulaSchema,
    rollups: &RollupResolver<'_>,
    visiting: &mut BTreeSet<FieldId>,
    depth: usize,
    op: impl Fn(i64, i64) -> Option<i64>,
) -> CellValue {
    let lv = eval_expr(
        viewer, src_row, a, props, formulas, rollups, visiting, depth,
    );
    if matches!(lv, CellValue::Cycle) {
        return CellValue::Cycle;
    }
    let rv = eval_expr(
        viewer, src_row, b, props, formulas, rollups, visiting, depth,
    );
    if matches!(rv, CellValue::Cycle) {
        return CellValue::Cycle;
    }
    match (lv, rv) {
        (CellValue::Int(x), CellValue::Int(y)) => match op(x, y) {
            Some(v) => CellValue::Int(v),
            None => CellValue::Error,
        },
        _ => CellValue::Error,
    }
}

fn literal_to_cell(lit: &Literal) -> CellValue {
    match lit {
        Literal::Int(n) => CellValue::Int(*n),
        Literal::Bool(b) => CellValue::Bool(*b),
        Literal::Str(s) => CellValue::Str(s.clone()),
    }
}

fn field_value_to_cell(value: &FieldValue) -> CellValue {
    match value {
        FieldValue::Int(n) => CellValue::Int(*n),
        FieldValue::Bool(b) => CellValue::Bool(*b),
        FieldValue::Text(s) | FieldValue::Date(s) | FieldValue::Select(s) => {
            CellValue::Str(s.clone())
        }
        FieldValue::Relation(r) => CellValue::Str(r.clone()),
        FieldValue::Principal(p) => CellValue::Str(p.clone()),
        FieldValue::OrderKey(k) => CellValue::Str(k.as_str().to_string()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterialisationHint {
    pub db_id: String,
    pub field: FieldId,
    pub measured_p99_ms: u64,
}

#[derive(Clone, Default)]
pub struct RollupLatencyTelemetry {
    samples: BTreeMap<(String, String), Vec<f64>>,
}

impl RollupLatencyTelemetry {
    pub fn new() -> RollupLatencyTelemetry {
        RollupLatencyTelemetry::default()
    }

    pub fn record(&mut self, db_id: &str, field: &FieldId, dur: Duration) {
        self.samples
            .entry((db_id.to_string(), field.to_string()))
            .or_default()
            .push(dur.as_secs_f64() * 1000.0);
    }

    pub fn p99_ms(&self, db_id: &str, field: &FieldId) -> f64 {
        match self.samples.get(&(db_id.to_string(), field.to_string())) {
            Some(s) if !s.is_empty() => {
                let mut sorted = s.clone();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let idx = ((sorted.len() as f64 * 0.99).ceil() as usize).saturating_sub(1);
                sorted[idx.min(sorted.len() - 1)]
            }
            _ => 0.0,
        }
    }

    pub fn materialisation_candidates(&self, budget_ms: u64) -> Vec<MaterialisationHint> {
        let mut out = Vec::new();
        for (db_id, field) in self.samples.keys() {
            let field_id = FieldId::new(field.clone());
            let p99 = self.p99_ms(db_id, &field_id);
            if p99 > budget_ms as f64 {
                out.push(MaterialisationHint {
                    db_id: db_id.clone(),
                    field: field_id,
                    measured_p99_ms: p99.ceil() as u64,
                });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests;

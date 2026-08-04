use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_identity::{Principal, SetExpr};
use myelin_query::{FieldId, FieldType, FieldValue, OrderKey, Predicate, QueryAst, ViewSpec};
use myelin_tenancy::{ArtifactRef, TenantId};

use crate::list_filter::{
    compose_db_count_query, compose_db_view_query, BoundParam, ComposedQuery,
};
use crate::rebac_fragment::DB_ROW_TABLE;

pub const FACET_PROMOTION_THRESHOLD: f64 = 0.05;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldDef {
    pub field_id: FieldId,
    pub field_type: FieldType,
    pub personal_data: bool,
}

impl FieldDef {
    pub fn new(field_id: impl Into<String>, field_type: FieldType) -> FieldDef {
        FieldDef {
            field_id: FieldId::new(field_id),
            field_type,
            personal_data: false,
        }
    }

    pub fn personal(field_id: impl Into<String>, field_type: FieldType) -> FieldDef {
        FieldDef {
            field_id: FieldId::new(field_id),
            field_type,
            personal_data: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FieldSchema {
    defs: Vec<FieldDef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaError {
    DuplicateField(String),
    UnknownField(String),
    TypeMismatch {
        field: String,
        declared: FieldType,
        supplied: FieldType,
    },
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchemaError::DuplicateField(id) => {
                write!(f, "duplicate field definition `{id}` (a JSONB key collision)")
            }
            SchemaError::UnknownField(id) => {
                write!(f, "row property `{id}` names a field the schema does not declare")
            }
            SchemaError::TypeMismatch { field, declared, supplied } => write!(
                f,
                "field `{field}` is declared `{}` but the value is `{}` (type mismatch, no coercion)",
                declared.wire_id(),
                supplied.wire_id()
            ),
        }
    }
}

impl std::error::Error for SchemaError {}

impl FieldSchema {
    pub fn of(defs: impl IntoIterator<Item = FieldDef>) -> Result<FieldSchema, SchemaError> {
        let defs: Vec<FieldDef> = defs.into_iter().collect();
        let mut seen = std::collections::BTreeSet::new();
        for d in &defs {
            if !seen.insert(d.field_id.clone()) {
                return Err(SchemaError::DuplicateField(d.field_id.to_string()));
            }
        }
        Ok(FieldSchema { defs })
    }

    pub fn fields(&self) -> &[FieldDef] {
        &self.defs
    }

    pub fn field_type(&self, field: &FieldId) -> Option<FieldType> {
        self.defs
            .iter()
            .find(|d| &d.field_id == field)
            .map(|d| d.field_type)
    }

    pub fn validate_props(&self, props: &PropertyBag) -> Result<(), SchemaError> {
        for (field, value) in props.iter() {
            let declared = self
                .field_type(field)
                .ok_or_else(|| SchemaError::UnknownField(field.to_string()))?;
            let supplied = value.field_type();
            if declared != supplied {
                return Err(SchemaError::TypeMismatch {
                    field: field.to_string(),
                    declared,
                    supplied,
                });
            }
        }
        Ok(())
    }
}

pub type PropertyBag = BTreeMap<FieldId, FieldValue>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DbRow {
    pub row_id: String,
    pub props: PropertyBag,
    pub order_key: OrderKey,
    pub version: u64,
    pub body_page: Option<ArtifactRef>,
}

impl DbRow {
    pub fn new(row_id: impl Into<String>, props: PropertyBag, order_key: OrderKey) -> DbRow {
        DbRow {
            row_id: row_id.into(),
            props,
            order_key,
            version: 1,
            body_page: None,
        }
    }

    pub fn props_json(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for (field, value) in &self.props {
            map.insert(field.as_str().to_string(), field_value_to_json(value));
        }
        serde_json::Value::Object(map)
    }
}

fn field_value_to_json(value: &FieldValue) -> serde_json::Value {
    match value {
        FieldValue::Text(s) | FieldValue::Date(s) | FieldValue::Select(s) => {
            serde_json::Value::String(s.clone())
        }
        FieldValue::Relation(r) => serde_json::Value::String(r.clone()),
        FieldValue::Principal(p) => serde_json::Value::String(p.clone()),
        FieldValue::Int(n) => serde_json::Value::Number((*n).into()),
        FieldValue::Bool(b) => serde_json::Value::Bool(*b),
        FieldValue::OrderKey(k) => serde_json::Value::String(k.as_str().to_string()),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FacetPath {
    GinScan,
    GeneratedColumn,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoweredViewFilter {
    pub sql_predicate: String,
    pub params: Vec<BoundParam>,
    pub facet_paths: BTreeMap<FieldId, FacetPath>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FacetIndexHint {
    pub field_id: FieldId,
    pub field_type: FieldType,
    pub personal_data: bool,
}

pub fn lower_view_filter(filter: &QueryAst, hot_facets: &[FieldId]) -> Option<LoweredViewFilter> {
    let predicate = filter.predicate()?;
    let mut ctx = FilterLowerCtx {
        hot_facets,
        params: Vec::new(),
        facet_paths: BTreeMap::new(),
        next_id: 0,
    };
    let sql_predicate = lower_predicate(predicate, &mut ctx);
    Some(LoweredViewFilter {
        sql_predicate,
        params: ctx.params,
        facet_paths: ctx.facet_paths,
    })
}

struct FilterLowerCtx<'a> {
    hot_facets: &'a [FieldId],
    params: Vec<BoundParam>,
    facet_paths: BTreeMap<FieldId, FacetPath>,
    next_id: usize,
}

impl FilterLowerCtx<'_> {
    fn bind(&mut self, value: &str) -> String {
        let placeholder = format!(":f{}", self.next_id);
        self.next_id += 1;
        self.params.push(BoundParam {
            placeholder: placeholder.clone(),
            value: value.to_string(),
        });
        placeholder
    }

    fn facet_access(&mut self, field: &str) -> String {
        let field_id = FieldId::new(field);
        let is_hot = self.hot_facets.iter().any(|h| h.as_str() == field);
        let path = if is_hot {
            FacetPath::GeneratedColumn
        } else {
            FacetPath::GinScan
        };
        self.facet_paths.insert(field_id, path);
        if is_hot {
            format!("{DB_ROW_TABLE}.{}__col", sanitize_ident(field))
        } else {
            format!("{DB_ROW_TABLE}.props ->> '{}'", sanitize_ident(field))
        }
    }
}

fn lower_predicate(predicate: &Predicate, ctx: &mut FilterLowerCtx<'_>) -> String {
    match predicate {
        Predicate::True => "TRUE".to_string(),
        Predicate::False => "FALSE".to_string(),
        Predicate::Cmp { op, lhs, rhs } => lower_cmp(*op, lhs, rhs, ctx),
        Predicate::And(ps) => {
            if ps.is_empty() {
                return "TRUE".to_string();
            }
            let frags: Vec<String> = ps.iter().map(|p| lower_predicate(p, ctx)).collect();
            format!("({})", frags.join(" AND "))
        }
        Predicate::Or(ps) => {
            if ps.is_empty() {
                return "FALSE".to_string();
            }
            let frags: Vec<String> = ps.iter().map(|p| lower_predicate(p, ctx)).collect();
            format!("({})", frags.join(" OR "))
        }
        Predicate::Not(p) => format!("(NOT {})", lower_predicate(p, ctx)),
    }
}

fn lower_cmp(
    op: myelin_query::CmpOp,
    lhs: &myelin_query::Expr,
    rhs: &myelin_query::Expr,
    ctx: &mut FilterLowerCtx<'_>,
) -> String {
    use myelin_query::{CmpOp, Expr};
    let sql_op = match op {
        CmpOp::Eq => "=",
        CmpOp::Ne => "<>",
        CmpOp::Lt => "<",
        CmpOp::Le => "<=",
        CmpOp::Gt => ">",
        CmpOp::Ge => ">=",
    };
    match (lhs, rhs) {
        (Expr::Var(field), Expr::Lit(lit)) => {
            let access = ctx.facet_access(field);
            let ph = ctx.bind(&literal_text(lit));
            format!("{access} {sql_op} {ph}")
        }
        (Expr::Lit(lit), Expr::Var(field)) => {
            let access = ctx.facet_access(field);
            let ph = ctx.bind(&literal_text(lit));
            format!("{ph} {sql_op} {access}")
        }
        (Expr::Lit(a), Expr::Lit(b)) => {
            let pa = ctx.bind(&literal_text(a));
            let pb = ctx.bind(&literal_text(b));
            format!("{pa} {sql_op} {pb}")
        }
        (Expr::Var(fa), Expr::Var(fb)) => {
            let aa = ctx.facet_access(fa);
            let ab = ctx.facet_access(fb);
            format!("{aa} {sql_op} {ab}")
        }
    }
}

fn literal_text(lit: &myelin_identity::Literal) -> String {
    use myelin_identity::Literal;
    match lit {
        Literal::Str(s) => s.clone(),
        Literal::Int(n) => n.to_string(),
        Literal::Bool(b) => b.to_string(),
    }
}

fn sanitize_ident(field: &str) -> String {
    field
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewQuery {
    pub sql: String,
    pub params: Vec<BoundParam>,
    pub facet_paths: BTreeMap<FieldId, FacetPath>,
    pub is_count: bool,
}

impl ViewQuery {
    pub fn statement_count(&self) -> usize {
        self.sql.split(';').filter(|s| !s.trim().is_empty()).count()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageBound {
    pub limit: u32,
    pub statement_timeout_ms: u32,
}

impl PageBound {
    pub const DEFAULT: PageBound = PageBound {
        limit: 50,
        statement_timeout_ms: 5_000,
    };
    pub const MAX: u32 = 500;

    pub fn new(limit: u32, statement_timeout_ms: u32) -> PageBound {
        PageBound {
            limit: limit.clamp(1, PageBound::MAX),
            statement_timeout_ms,
        }
    }
}

impl Default for PageBound {
    fn default() -> Self {
        PageBound::DEFAULT
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewError {
    FilterNotCompiled,
}

impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ViewError::FilterNotCompiled => write!(
                f,
                "the view filter is an un-parsed placeholder (no compiled tree) - refused, never run as match-all"
            ),
        }
    }
}

impl std::error::Error for ViewError {}

#[allow(clippy::too_many_arguments)]
pub fn execute_view_query(
    view: &ViewSpec,
    acl: &SetExpr,
    viewer: &Principal,
    scope_tenant: &TenantId,
    db_id: &str,
    hot_facets: &[FieldId],
    page: PageBound,
) -> Result<ViewQuery, ViewError> {
    let lowered =
        lower_view_filter(&view.filter, hot_facets).ok_or(ViewError::FilterNotCompiled)?;
    let acl_query = compose_db_view_query(acl, viewer, scope_tenant, db_id);
    let sql = splice_view_filter(
        &acl_query,
        &lowered,
        &order_by_clause(view),
        page.limit,
        false,
    );
    let params = merge_params(acl_query.params, lowered.params);
    Ok(ViewQuery {
        sql,
        params,
        facet_paths: lowered.facet_paths,
        is_count: false,
    })
}

pub fn execute_view_count(
    view: &ViewSpec,
    acl: &SetExpr,
    viewer: &Principal,
    scope_tenant: &TenantId,
    db_id: &str,
    hot_facets: &[FieldId],
) -> Result<ViewQuery, ViewError> {
    let lowered =
        lower_view_filter(&view.filter, hot_facets).ok_or(ViewError::FilterNotCompiled)?;
    let acl_query = compose_db_count_query(acl, viewer, scope_tenant, db_id);
    let sql = splice_view_filter(&acl_query, &lowered, "", 0, true);
    let params = merge_params(acl_query.params, lowered.params);
    Ok(ViewQuery {
        sql,
        params,
        facet_paths: lowered.facet_paths,
        is_count: true,
    })
}

fn order_by_clause(view: &ViewSpec) -> String {
    use myelin_query::SortDir;
    let mut parts: Vec<String> = view
        .sort
        .iter()
        .map(|s| {
            let dir = match s.dir {
                SortDir::Asc => "ASC",
                SortDir::Desc => "DESC",
            };
            format!(
                "{DB_ROW_TABLE}.props ->> '{}' {dir}",
                sanitize_ident(s.field.as_str())
            )
        })
        .collect();
    parts.push(format!("{DB_ROW_TABLE}.order_key ASC"));
    parts.join(", ")
}

fn splice_view_filter(
    acl_query: &ComposedQuery,
    lowered: &LoweredViewFilter,
    order_by: &str,
    limit: u32,
    is_count: bool,
) -> String {
    let acl_sql = &acl_query.sql;
    if is_count {
        format!("{acl_sql} AND ({})", lowered.sql_predicate)
    } else {
        let acl_marker = " ORDER BY ";
        let (head, _tail) = acl_sql
            .split_once(acl_marker)
            .expect("the ACL view composer always emits an ORDER BY clause");
        format!(
            "{head} AND ({filter}) ORDER BY {order_by} LIMIT {limit}",
            filter = lowered.sql_predicate,
        )
    }
}

fn merge_params(
    mut acl_params: Vec<BoundParam>,
    filter_params: Vec<BoundParam>,
) -> Vec<BoundParam> {
    acl_params.extend(filter_params);
    acl_params
}

pub fn row_matches_filter(
    filter: &QueryAst,
    props: &PropertyBag,
) -> Result<bool, myelin_query::EvalError> {
    let mut ctx = myelin_query::EvalContext::new();
    for (field, value) in props {
        if let Some(lit) = field_value_to_literal(value) {
            ctx = ctx.bind(field.as_str().to_string(), lit);
        }
    }
    filter.eval(&ctx)
}

fn field_value_to_literal(value: &FieldValue) -> Option<myelin_identity::Literal> {
    use myelin_identity::Literal;
    Some(match value {
        FieldValue::Text(s) | FieldValue::Date(s) | FieldValue::Select(s) => {
            Literal::Str(s.clone())
        }
        FieldValue::Relation(r) => Literal::Str(r.clone()),
        FieldValue::Principal(p) => Literal::Str(p.clone()),
        FieldValue::OrderKey(k) => Literal::Str(k.as_str().to_string()),
        FieldValue::Int(n) => Literal::Int(*n),
        FieldValue::Bool(b) => Literal::Bool(*b),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RelationKind {
    Relates,
    RollupSource,
}

impl RelationKind {
    pub fn wire_id(self) -> &'static str {
        match self {
            RelationKind::Relates => "relates",
            RelationKind::RollupSource => "rollup_source",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DbRelation {
    pub relation_id: String,
    pub src_row: String,
    pub dst_ref: ArtifactRef,
    pub rel: RelationKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelationEdgeEvent {
    pub created: bool,
    pub relation: DbRelation,
}

#[derive(Clone, Default)]
pub struct RelationStore {
    rows: Arc<Mutex<Vec<ScopedRelation>>>,
    edge_events: Arc<Mutex<Vec<RelationEdgeEvent>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ScopedRelation {
    tenant: String,
    relation: DbRelation,
}

impl RelationStore {
    pub fn new() -> RelationStore {
        RelationStore::default()
    }

    pub fn relate(&self, tenant: &TenantId, relation: DbRelation) -> bool {
        let mut rows = self.rows.lock().unwrap();
        let exists = rows.iter().any(|r| {
            r.tenant == tenant.0
                && r.relation.src_row == relation.src_row
                && r.relation.dst_ref == relation.dst_ref
                && r.relation.rel == relation.rel
        });
        if exists {
            return false;
        }
        rows.push(ScopedRelation {
            tenant: tenant.0.clone(),
            relation: relation.clone(),
        });
        drop(rows);
        self.edge_events.lock().unwrap().push(RelationEdgeEvent {
            created: true,
            relation,
        });
        true
    }

    pub fn unrelate(
        &self,
        tenant: &TenantId,
        src_row: &str,
        dst_ref: &ArtifactRef,
        rel: RelationKind,
    ) -> bool {
        let mut rows = self.rows.lock().unwrap();
        let before = rows.len();
        let mut removed: Option<DbRelation> = None;
        rows.retain(|r| {
            let matches = r.tenant == tenant.0
                && r.relation.src_row == src_row
                && &r.relation.dst_ref == dst_ref
                && r.relation.rel == rel;
            if matches {
                removed = Some(r.relation.clone());
            }
            !matches
        });
        let did_remove = rows.len() != before;
        drop(rows);
        if let Some(relation) = removed {
            self.edge_events.lock().unwrap().push(RelationEdgeEvent {
                created: false,
                relation,
            });
        }
        did_remove
    }

    pub fn relations_from(
        &self,
        tenant: &TenantId,
        src_row: &str,
        rel: RelationKind,
    ) -> Vec<DbRelation> {
        self.rows
            .lock()
            .unwrap()
            .iter()
            .filter(|r| {
                r.tenant == tenant.0 && r.relation.src_row == src_row && r.relation.rel == rel
            })
            .map(|r| r.relation.clone())
            .collect()
    }

    pub fn drain_edge_events(&self) -> Vec<RelationEdgeEvent> {
        std::mem::take(&mut *self.edge_events.lock().unwrap())
    }
}

#[derive(Clone, Default)]
pub struct FacetTelemetry {
    counters: Arc<Mutex<BTreeMap<String, DbCounters>>>,
}

#[derive(Clone, Debug, Default)]
struct DbCounters {
    total: u64,
    facet_uses: BTreeMap<String, u64>,
}

impl FacetTelemetry {
    pub fn new() -> FacetTelemetry {
        FacetTelemetry::default()
    }

    pub fn record_execution(&self, db_id: &str, facets: &[FieldId]) {
        let mut c = self.counters.lock().unwrap();
        let entry = c.entry(db_id.to_string()).or_default();
        entry.total += 1;
        let mut counted = std::collections::BTreeSet::new();
        for f in facets {
            if counted.insert(f.as_str().to_string()) {
                *entry.facet_uses.entry(f.as_str().to_string()).or_insert(0) += 1;
            }
        }
    }

    pub fn facet_frequency(&self, db_id: &str, facet: &FieldId) -> f64 {
        let c = self.counters.lock().unwrap();
        match c.get(db_id) {
            Some(counters) if counters.total > 0 => {
                let uses = counters
                    .facet_uses
                    .get(facet.as_str())
                    .copied()
                    .unwrap_or(0);
                uses as f64 / counters.total as f64
            }
            _ => 0.0,
        }
    }

    pub fn promotion_candidates(&self, db_id: &str, schema: &FieldSchema) -> Vec<FacetIndexHint> {
        let c = self.counters.lock().unwrap();
        let Some(counters) = c.get(db_id) else {
            return Vec::new();
        };
        if counters.total == 0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (field, uses) in &counters.facet_uses {
            let freq = *uses as f64 / counters.total as f64;
            if freq > FACET_PROMOTION_THRESHOLD {
                let field_id = FieldId::new(field.clone());
                if let Some(def) = schema.fields().iter().find(|d| d.field_id == field_id) {
                    out.push(FacetIndexHint {
                        field_id,
                        field_type: def.field_type,
                        personal_data: def.personal_data,
                    });
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests;

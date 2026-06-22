//! # The flexible database — JSONB property bag + GIN-indexed projection + views + relations
//! (KN-P17 / P-307 — the KN-D9 flex-DB latency crux)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/knowledge-platform/architecture/`
//! `01-tech-and-data-model.md` §1.2 (the flexible DB is JSONB + derived projections — NOT
//! per-tenant DDL: JSONB property-bag rows (source of truth) + GIN/expression indexes + generated
//! columns for the **measured-hot** facets) + §4.2 (the `db_collection` field defs / the `db_row`
//! property bag / `db_row_props_gin`) + §4.3 (the `db_relation` two-way relation — the TE-7
//! source of truth) + §4.4 (the frozen `ViewSpec`); and `02-internals-and-algorithms.md` §4.1 (the
//! `VIEW_QUERY` with the `SetExpr` conjoin — measured-hot facets → the generated/expression-column
//! index, cold → a bounded paginated GIN `jsonb_path_ops` scan; paginated, row-capped,
//! statement-timeout).
//!
//! **Contract-index:** rows **13.3** (the `FieldType`/`ViewSpec`/`QueryAst` — **Knowledge owns its
//! EXECUTOR**: the JSONB query lowering over the frozen shared shapes; the definitions live in
//! [`myelin_query`]), **4.3** (the `list_objects` `SetExpr` push-down — **CONSUMED**: the
//! [`crate::list_filter`] lowering this module conjoins into every view/board/COUNT query, so a
//! viewer never sees an un-permitted row — KN-D5 composes with KN-D9), **6.3** (the **> 5%
//! facet-promotion threshold** — **CONSUMED**: [`FacetTelemetry`] measures the per-facet
//! view-execution frequency and reports which facets cross the Search-owned threshold; the
//! promotion ACT — provisioning the generated/expression-column index off the bus — is **KN-P31
//! (M5)**, named below).
//!
//! ## What this module ships (KN-P17 — the executor over the frozen shapes)
//! 1. [`FieldDef`] / [`FieldSchema`] — typed field definitions over the frozen [`FieldType`] enum
//!    (the `db_collection.field_defs`, §4.2). [`FieldSchema::validate_props`] type-checks a row's
//!    property bag against the declared types — a declared-type/value mismatch is REJECTED, never
//!    silently coerced (the typed-`FieldType` validation gate).
//! 2. [`DbRow`] — a JSONB property-bag row (the source of truth, §4.2): `{ field_id → FieldValue }`
//!    plus the frozen LexoRank `order_key` and the CAS `version`. [`DbRow::props_json`] is the
//!    `props` JSONB column the GIN index covers.
//! 3. [`lower_view_filter`] — the §4.1 step 4: lower a view's [`QueryAst`] filter into JSONB ops
//!    over `props`, picking the **measured-hot generated/expression-column index** path for a hot
//!    facet and the **cold GIN `jsonb_path_ops` scan** path otherwise (the derived projection).
//! 4. [`execute_view_query`] — the §4.1 `VIEW_QUERY`: the lowered filter **conjoined** with the
//!    `list_objects` `SetExpr` ACL (via [`crate::list_filter::compose_db_view_query`]) — paginated,
//!    row-capped, statement-timeout. Permission by construction: 0 post-filter, 0 leak (composes
//!    with KN-D5). [`execute_view_count`] is the permission-correct `COUNT(*)`.
//! 5. [`RelationStore`] — the two-way relation (`db_relation`, the TE-7 source of truth, §4.3):
//!    [`RelationStore::relate`] / [`RelationStore::unrelate`] maintain the forward edge
//!    transactionally and emit the typed lifecycle edge the Refs edge-builder mirrors (the inverse
//!    projection is eventually-consistent in Refs — KN-P19, named below).
//! 6. [`FacetTelemetry`] — the 6.3 measured-promotion telemetry: per-(db, facet) view-execution
//!    counters + the [`FacetTelemetry::promotion_candidates`] report (facets crossing the frozen
//!    `> 5%` threshold). Measured here; ACTED on (the generated-column index provisioned off the
//!    bus) in KN-P31.
//!
//! ## FLOOR 1 named (VISION §3 / EI-01 §4 — stubbed/deferred + the filling prompt)
//! - **Floor 1 — JSONB bag + GIN-indexed projection (read-time facets).** This module ships the
//!   JSONB property bag (source of truth) + the GIN `jsonb_path_ops` scan path + the *measurement*
//!   of the `> 5%` facet-promotion trigger. The **per-facet generated/expression-column index**
//!   (promoted when a facet crosses the threshold, provisioned off the bus via the
//!   expand→backfill→contract online migration) is **KN-P31 (M5)** — [`FacetIndexHint`] is the
//!   names-only hint the promotion path consumes; this module does NOT build the live generated
//!   column. Named here in writing.
//! - **The read-time formula/rollup over these rows is KN-P18 (M3)** — the bounded `FormulaAst`
//!   evaluator that reads over the property bag / the `rollup_source` relations, never stored. This
//!   module ships the rows + relations it reads; the evaluator itself is KN-P18. Named.
//! - **The Refs inverse-edge projection is KN-P19 (M3)** — [`RelationStore::relate`] writes the
//!   forward `db_relation` row (transactional) + records the typed edge event to mirror; the Refs
//!   `#sub` mint + the inverse `backlinks` projection is KN-P19. The forward edge is the source of
//!   truth here; the inverse is eventually-consistent (EI-04 §2 / contract 5.5). Named.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_identity::{Principal, SetExpr};
use myelin_query::{FieldId, FieldType, FieldValue, OrderKey, Predicate, QueryAst, ViewSpec};
use myelin_tenancy::{ArtifactRef, TenantId};

use crate::list_filter::{compose_db_count_query, compose_db_view_query, BoundParam, ComposedQuery};
use crate::rebac_fragment::DB_ROW_TABLE;

/// The frozen `> 5%` facet-promotion threshold (contract 6.3 / OQ-C, architecture §1.2): a facet
/// used in MORE than this fraction of a collection's view executions over a rolling window promotes
/// from a cold GIN scan to a per-facet generated/expression-column index. The threshold is the
/// **Search-owned tunable**; it is measured HERE ([`FacetTelemetry`]) and ACTED on in KN-P31 (M5).
/// Expressed as a ratio (0.05 == 5%); a facet at EXACTLY 5% does NOT promote (the trigger is
/// strictly `> 5%`, the frozen 6.3 wording).
pub const FACET_PROMOTION_THRESHOLD: f64 = 0.05;

// ───────────────────────────── typed field definitions (§4.2 / 13.3) ──────────────────────────────

/// **A typed field definition** — one column of a flexible-database collection (the
/// `db_collection.field_defs`, §4.2). It pairs a stable [`FieldId`] with its frozen [`FieldType`]
/// and the personal-data classification (`#[personal_data]`, contract 10.2 — a PII facet is tagged
/// at the schema level so field-level erasure + the field-level ABAC caveat find it, §5). The
/// definitions are the frozen shared shapes (13.3); Knowledge owns the EXECUTOR over them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldDef {
    /// The stable field id (the JSONB `props` key + the `FieldId` a view references).
    pub field_id: FieldId,
    /// The frozen field type the value space is validated against.
    pub field_type: FieldType,
    /// Whether this field carries personal data (contract 10.2 — the field-level erasure / ABAC
    /// caveat target). A PII facet is NEVER promoted to a plaintext generated column without the
    /// caveat gate (KN-P31 honours this hint; here it is recorded).
    pub personal_data: bool,
}

impl FieldDef {
    /// A non-PII field definition.
    pub fn new(field_id: impl Into<String>, field_type: FieldType) -> FieldDef {
        FieldDef {
            field_id: FieldId::new(field_id),
            field_type,
            personal_data: false,
        }
    }

    /// A field definition carrying personal data (contract 10.2).
    pub fn personal(field_id: impl Into<String>, field_type: FieldType) -> FieldDef {
        FieldDef {
            field_id: FieldId::new(field_id),
            field_type,
            personal_data: true,
        }
    }
}

/// **A collection's typed field schema** — the ordered set of [`FieldDef`]s a `db_collection`
/// declares (§4.2). It type-checks a row's property bag ([`FieldSchema::validate_props`]) and is the
/// authority a view's field references resolve against. Field ids are unique (a duplicate id is a
/// schema error — two columns cannot share a JSONB key).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FieldSchema {
    defs: Vec<FieldDef>,
}

/// A flexible-database schema/validation error — every variant is a typed rejection, never a silent
/// coercion (the typed-`FieldType` validation gate; EI-01 §3 fail-closed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaError {
    /// Two field definitions declared the same [`FieldId`] (a JSONB key collision).
    DuplicateField(String),
    /// A row's property names a field the schema does not declare (an undeclared column — the row
    /// is rejected, never stored with an unknown facet).
    UnknownField(String),
    /// A property value's [`FieldType`] does not match the field's declared type (e.g. an `Int`
    /// value in a `Text` field). The exact frozen wording surfaces the declared vs supplied type.
    TypeMismatch {
        /// The offending field id.
        field: String,
        /// The declared field type (the schema's).
        declared: FieldType,
        /// The supplied value's field type.
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
    /// Build a schema from an ordered set of field definitions. Returns [`SchemaError::DuplicateField`]
    /// if two definitions share a [`FieldId`] (a JSONB key cannot map to two columns).
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

    /// The declared field definitions, in order.
    pub fn fields(&self) -> &[FieldDef] {
        &self.defs
    }

    /// Look up a field's declared type (`None` if the schema does not declare it).
    pub fn field_type(&self, field: &FieldId) -> Option<FieldType> {
        self.defs.iter().find(|d| &d.field_id == field).map(|d| d.field_type)
    }

    /// **Type-check a row's property bag against the declared field types (the typed-`FieldType`
    /// validation gate, §4.2).** Every property must name a declared field AND carry a value of the
    /// declared type — a mismatch is [`SchemaError::TypeMismatch`], an undeclared field is
    /// [`SchemaError::UnknownField`]. Never a silent coercion (a `"5"` text in an `Int` field is a
    /// rejection, not a parse). A subset of fields is allowed (a row may omit a field — the property
    /// bag is sparse); only PRESENT properties are validated.
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

// ───────────────────────────── the JSONB property bag + rows (§4.2) ───────────────────────────────

/// **The JSONB property bag** — `{ field_id → FieldValue }`, the source-of-truth `db_row.props`
/// column (§4.2). A `BTreeMap` so the JSONB serialization is deterministic (stable key order — the
/// GIN index + the golden tests see byte-stable JSON). Sparse: a row carries only the fields it sets.
pub type PropertyBag = BTreeMap<FieldId, FieldValue>;

/// **A flexible-database row — the JSONB property bag is the source of truth (§4.2).** Carries the
/// `props` bag + the frozen LexoRank `order_key` (the manual drag order) + the CAS `version` (the
/// row-edit optimistic-concurrency token). The body-page pointer (a row IS a page — open-as-page) is
/// the `body_page` block subtree (the block tree is [`crate::block_tree`]); it is an optional
/// [`ArtifactRef`] here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DbRow {
    /// The row's stable id (the consumer id column for the `SetExpr` push-down, OQ-E — `db_row.id`).
    pub row_id: String,
    /// The JSONB property bag (source of truth).
    pub props: PropertyBag,
    /// The frozen LexoRank `order_key` (the manual drag-order field, §2.5 / 13.3).
    pub order_key: OrderKey,
    /// The CAS version (optimistic-concurrency token for a row edit).
    pub version: u64,
    /// The body-page pointer (a row IS a page — its body block subtree), if opened as a page.
    pub body_page: Option<ArtifactRef>,
}

impl DbRow {
    /// Build a row from its id, property bag and order key (version starts at 1, no body page).
    pub fn new(row_id: impl Into<String>, props: PropertyBag, order_key: OrderKey) -> DbRow {
        DbRow {
            row_id: row_id.into(),
            props,
            order_key,
            version: 1,
            body_page: None,
        }
    }

    /// **The `props` JSONB column the GIN `jsonb_path_ops` index covers (§4.2).** The deterministic
    /// JSON object the source-of-truth row stores; the derived GIN projection indexes THIS. Field
    /// values render to their JSON form (a `FieldValue::Int(5)` → `5`, a `Text("x")` → `"x"`, an
    /// `OrderKey` → its base-62 string). Stable key order (the `BTreeMap`) so the serialization is
    /// byte-stable.
    pub fn props_json(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for (field, value) in &self.props {
            map.insert(field.as_str().to_string(), field_value_to_json(value));
        }
        serde_json::Value::Object(map)
    }
}

/// Render a typed [`FieldValue`] to its JSONB form (the value the `props` bag stores + the GIN index
/// covers). The mapping is total and lossless over the frozen [`FieldType`] value space.
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

// ───────────────────────── the view-filter JSONB lowering (§4.1 step 4) ──────────────────────────

/// **Which derived-projection path a facet's predicate lowers to (the §4.1 step-4 split).** A
/// **measured-hot** facet (one that crossed the frozen `> 5%` view-execution threshold, 6.3) lowers
/// to the per-facet **generated/expression-column index** ([`FacetPath::GeneratedColumn`], KN-P31);
/// a **cold** facet lowers to the bounded paginated **GIN `jsonb_path_ops` scan** over `props`
/// ([`FacetPath::GinScan`], the Floor-1 path this module ships).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FacetPath {
    /// The cold path: a GIN `jsonb_path_ops` scan over `props` (Floor 1 — shipped here).
    GinScan,
    /// The measured-hot path: a per-facet generated/expression-column index (KN-P31, M5).
    GeneratedColumn,
}

/// **The lowered view filter — the JSONB SQL fragment + the per-facet projection paths it chose
/// (§4.1 step 4).** The `sql_predicate` is the boolean over `db_row.props` the view scan ANDs into
/// its `WHERE` (BEFORE the ACL conjoin, which [`execute_view_query`] adds); `facet_paths` records,
/// per referenced facet, whether the cold GIN scan or the hot generated-column index served it (the
/// 1.8 filter-mode-style telemetry). `params` are bound, never interpolated (a user-controlled
/// filter literal can never become SQL — the same injection-safe discipline as the ACL lowering).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoweredViewFilter {
    /// The boolean SQL over `db_row.props` (the JSONB ops). `TRUE` for the empty (`Predicate::True`)
    /// filter — the executor STILL conjoins the ACL, so an empty filter is never an over-broad read.
    pub sql_predicate: String,
    /// The bound filter literals (`:f0`, `:f1`, …) — bound, never interpolated.
    pub params: Vec<BoundParam>,
    /// Per referenced facet, the projection path that served it (cold GIN vs hot generated column).
    pub facet_paths: BTreeMap<FieldId, FacetPath>,
}

/// A names-only hint the KN-P31 (M5) promotion path consumes: which facet to promote to a per-facet
/// generated/expression-column index, and whether it carries personal data (a PII facet is gated by
/// the field-level caveat before it is materialised as a plaintext column). This module RECORDS the
/// hint (from [`FacetTelemetry::promotion_candidates`] + [`FieldSchema`]); KN-P31 acts on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FacetIndexHint {
    /// The facet to promote.
    pub field_id: FieldId,
    /// Its declared type (the generated-column type KN-P31 provisions).
    pub field_type: FieldType,
    /// Whether the facet carries personal data (gated by the field-level caveat, contract 10.2).
    pub personal_data: bool,
}

/// **Lower a view's [`QueryAst`] filter into JSONB ops over `db_row.props` (§4.1 step 4).** Each
/// `Cmp { field, value }` becomes a `props ->> '<field>' <op> :literal` predicate over the property
/// bag; the boolean connectives compose with `AND`/`OR`/`NOT`. For each referenced facet, the
/// `hot_facets` set decides the projection path: a hot facet lowers to the generated-column index
/// (KN-P31), a cold one to the GIN `jsonb_path_ops` scan (Floor 1). The filter LITERALS are bound
/// (never interpolated). The empty (`Predicate::True`) filter is `TRUE` — the executor still
/// ACL-conjoins it (never an over-broad read).
///
/// Returns `None` if the [`QueryAst`] is the un-parsed placeholder surface (no compiled tree — the
/// fail-closed posture: an un-parsed filter is uncertainty, never a silent match-all).
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

/// Internal accumulator threaded through the recursive view-filter lowering.
struct FilterLowerCtx<'a> {
    hot_facets: &'a [FieldId],
    params: Vec<BoundParam>,
    facet_paths: BTreeMap<FieldId, FacetPath>,
    next_id: usize,
}

impl FilterLowerCtx<'_> {
    /// Bind a filter literal, returning its `:placeholder` (bound, never interpolated).
    fn bind(&mut self, value: &str) -> String {
        let placeholder = format!(":f{}", self.next_id);
        self.next_id += 1;
        self.params.push(BoundParam {
            placeholder: placeholder.clone(),
            value: value.to_string(),
        });
        placeholder
    }

    /// The JSONB access expression for a facet, recording the projection path it took. A hot facet
    /// reads its generated/expression column (`db_row.<field>__col`, the KN-P31 index target); a
    /// cold facet reads `db_row.props ->> '<field>'` (the GIN-covered property-bag path).
    fn facet_access(&mut self, field: &str) -> String {
        let field_id = FieldId::new(field);
        let is_hot = self.hot_facets.iter().any(|h| h.as_str() == field);
        let path = if is_hot { FacetPath::GeneratedColumn } else { FacetPath::GinScan };
        self.facet_paths.insert(field_id, path);
        if is_hot {
            // The measured-hot generated/expression-column index (KN-P31 provisions the column; the
            // lowering NAMES it so the promoted read targets the column, not the GIN scan).
            format!("{DB_ROW_TABLE}.{}__col", sanitize_ident(field))
        } else {
            // The cold path: the GIN `jsonb_path_ops`-covered property-bag access (`->>` = text).
            format!("{DB_ROW_TABLE}.props ->> '{}'", sanitize_ident(field))
        }
    }
}

/// Lower one predicate node into a boolean SQL fragment over `props`. The leaf `Cmp` reads the facet
/// access (hot column or cold GIN path) and binds the literal; the connectives compose.
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

/// Lower a comparison into a JSONB predicate. The frozen `Expr` shape is `Var(field)` on one side
/// and a `Lit` on the other; a comparison of two literals or two vars (no field reference) lowers to
/// a constant-fold leaf so the SQL is always well-formed.
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
        // The canonical view-filter shape: `field <op> literal`.
        (Expr::Var(field), Expr::Lit(lit)) => {
            let access = ctx.facet_access(field);
            let ph = ctx.bind(&literal_text(lit));
            format!("{access} {sql_op} {ph}")
        }
        // The mirrored shape: `literal <op> field` (flip the operator's sense).
        (Expr::Lit(lit), Expr::Var(field)) => {
            let access = ctx.facet_access(field);
            let ph = ctx.bind(&literal_text(lit));
            format!("{ph} {sql_op} {access}")
        }
        // Two literals — a constant comparison the planner folds (rare; keeps the SQL well-formed).
        (Expr::Lit(a), Expr::Lit(b)) => {
            let pa = ctx.bind(&literal_text(a));
            let pb = ctx.bind(&literal_text(b));
            format!("{pa} {sql_op} {pb}")
        }
        // Two field references (a field-to-field comparison) — both access `props`.
        (Expr::Var(fa), Expr::Var(fb)) => {
            let aa = ctx.facet_access(fa);
            let ab = ctx.facet_access(fb);
            format!("{aa} {sql_op} {ab}")
        }
    }
}

/// The bound text form of a literal (the value the parameter binds — never interpolated).
fn literal_text(lit: &myelin_identity::Literal) -> String {
    use myelin_identity::Literal;
    match lit {
        Literal::Str(s) => s.clone(),
        Literal::Int(n) => n.to_string(),
        Literal::Bool(b) => b.to_string(),
    }
}

/// Strip any character that is not a safe SQL identifier byte from a facet name (defence in depth —
/// the field id is a schema token, not user free-text, but the lowering NEVER lets a facet name
/// reach the SQL string unsanitised; the literal VALUES are always bound, this guards the column
/// IDENTIFIER). Keeps `[A-Za-z0-9_]`; anything else is dropped.
fn sanitize_ident(field: &str) -> String {
    field.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect()
}

// ───────────────────────────── the VIEW_QUERY executor (§4.1) ─────────────────────────────────────

/// **A composed, leak-free flexible-database view query (the §4.1 `VIEW_QUERY`).** The single SQL
/// statement is `SELECT … FROM db_row <acl-joins> WHERE tenant = :tenant AND db_id = :db_id AND
/// (<view-filter>) AND (<acl>) ORDER BY <sort>, db_row.order_key LIMIT :page` — the view filter
/// (JSONB ops) AND the `list_objects` `SetExpr` ACL are BOTH conjoined BEFORE pagination (pre-filter,
/// never post-filter, ADR-03 / §5). One query; the `order_key` is the always-present last-resort
/// tiebreak (13.3). The `params` are bound. [`ViewQuery::statement_count`] is always 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewQuery {
    /// The single SQL statement (no trailing `;`).
    pub sql: String,
    /// The bound parameters (tenant, db_id, the view-filter literals, the ACL ids/subject/relation).
    pub params: Vec<BoundParam>,
    /// Per referenced facet, the projection path (cold GIN vs hot generated column) — the 6.3
    /// measured-promotion telemetry the [`FacetTelemetry`] records.
    pub facet_paths: BTreeMap<FieldId, FacetPath>,
    /// `true` iff this is the permission-correct `COUNT(*)` aggregate (the KN-D5 count-leak-closed
    /// shape) rather than a row-projecting view scan.
    pub is_count: bool,
}

impl ViewQuery {
    /// The number of SQL statements — ALWAYS 1 (the §4.1 no-N+1 guarantee: the view filter + the ACL
    /// are conjoined into ONE statement the planner resolves, never a per-row check loop, never a
    /// post-filter second pass).
    pub fn statement_count(&self) -> usize {
        self.sql.split(';').filter(|s| !s.trim().is_empty()).count()
    }
}

/// The page bound + statement-timeout a view read carries (§4.1 step 5 — paginated; row-capped;
/// statement-timeout). A read is ALWAYS bounded: a missing/zero page is the [`PageBound::DEFAULT`]
/// cap (never an unbounded scan). The statement timeout is the belt-and-braces DoS guard a crafted
/// filter cannot exceed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageBound {
    /// The maximum rows a single view read returns (the row cap). Clamped to [`PageBound::MAX`].
    pub limit: u32,
    /// The statement timeout (ms) the read runs under (the §4.1 statement-timeout guard).
    pub statement_timeout_ms: u32,
}

impl PageBound {
    /// The default page bound: 50 rows, a 5 s statement timeout (the §4.1 row-cap + timeout floor).
    pub const DEFAULT: PageBound = PageBound { limit: 50, statement_timeout_ms: 5_000 };
    /// The hard maximum page size — a request for more is clamped to this (never an unbounded scan).
    pub const MAX: u32 = 500;

    /// Build a page bound, clamping the limit to `[1, MAX]` (a 0 / over-large request is clamped —
    /// a view read is ALWAYS row-capped).
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

/// A view executor error (the un-buildable cases — fail-closed, never a silent match-all).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewError {
    /// The view's [`QueryAst`] filter is the un-parsed placeholder surface (no compiled tree). An
    /// un-parsed filter is uncertainty — the read is refused, never run as match-all (fail-closed).
    FilterNotCompiled,
}

impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ViewError::FilterNotCompiled => write!(
                f,
                "the view filter is an un-parsed placeholder (no compiled tree) — refused, never run as match-all"
            ),
        }
    }
}

impl std::error::Error for ViewError {}

/// **Execute the §4.1 `VIEW_QUERY` — the flexible-database rows the `viewer` may `read`, leak-free,
/// in ONE query.** Lowers the `view`'s filter into JSONB ops over `props` (the hot/cold facet split)
/// AND conjoins the `list_objects` `SetExpr` ACL ([`compose_db_view_query`]) — both BEFORE the
/// `ORDER BY`/`LIMIT` (pre-filter, never post-filter). The sort is the view's `sort` criteria with
/// the frozen LexoRank `order_key` as the always-present last-resort tiebreak (13.3). Paginated +
/// row-capped + statement-timeout (`page`).
///
/// Permission by construction (composes with KN-D5): a row the viewer cannot read never survives the
/// ACL conjunct, so it is ABSENT from the view AND (via [`execute_view_count`]) uncounted.
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
    let lowered = lower_view_filter(&view.filter, hot_facets).ok_or(ViewError::FilterNotCompiled)?;
    // The ACL composer builds the tenant + db_id scope + the lowered SetExpr ACL over `db_row.id`
    // (one query, no N+1) — we reuse it (EI-01 §7, one primitive) and SPLICE the view filter +
    // ORDER BY in, so the ACL and the view filter are conjoined into the SAME WHERE.
    let acl_query = compose_db_view_query(acl, viewer, scope_tenant, db_id);
    let sql = splice_view_filter(&acl_query, &lowered, &order_by_clause(view), page.limit, false);
    let params = merge_params(acl_query.params, lowered.params);
    Ok(ViewQuery {
        sql,
        params,
        facet_paths: lowered.facet_paths,
        is_count: false,
    })
}

/// **Execute the §4.1 permission-correct `COUNT(*)` (the KN-D5 count-leak-closed shape) over a
/// view.** The SAME view filter + the SAME `SetExpr` ACL conjoined INSIDE a `SELECT COUNT(*)` — so
/// the aggregate counts ONLY rows the viewer may read AND match the filter; a `COUNT` over a
/// row-restricted db reveals neither the existence nor the number of forbidden rows. NOT a
/// post-count subtraction (which would itself leak the hidden count) — the count IS the conjoined
/// query's cardinality.
pub fn execute_view_count(
    view: &ViewSpec,
    acl: &SetExpr,
    viewer: &Principal,
    scope_tenant: &TenantId,
    db_id: &str,
    hot_facets: &[FieldId],
) -> Result<ViewQuery, ViewError> {
    let lowered = lower_view_filter(&view.filter, hot_facets).ok_or(ViewError::FilterNotCompiled)?;
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

/// The `ORDER BY` clause for a view (§4.4): the view's `sort` criteria, with the frozen LexoRank
/// `order_field` (`db_row.order_key`) as the always-present LAST-resort tiebreak so two rows are
/// never ambiguously ordered (13.3).
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
            // A sort field reads its property-bag value (the GIN-covered path); the order_field
            // tiebreak reads the dedicated order_key column.
            format!("{DB_ROW_TABLE}.props ->> '{}' {dir}", sanitize_ident(s.field.as_str()))
        })
        .collect();
    // The always-present last-resort tiebreak: the manual drag-order LexoRank column.
    parts.push(format!("{DB_ROW_TABLE}.order_key ASC"));
    parts.join(", ")
}

/// Splice the view filter (`AND (<filter>)`) and the `ORDER BY`/`LIMIT` into the ACL-composed query.
/// The ACL composer already emits `… WHERE tenant = :tenant AND db_id = :db_id AND (<acl>) ORDER BY
/// db_row.id LIMIT :page` (or `SELECT COUNT(*) … AND (<acl>)`); we conjoin the view filter into the
/// SAME WHERE — so the filter AND the ACL are one conjunction (pre-filter, never post-filter) — and
/// replace the ACL composer's placeholder `ORDER BY`/`LIMIT` with the view's sort + page bound.
fn splice_view_filter(
    acl_query: &ComposedQuery,
    lowered: &LoweredViewFilter,
    order_by: &str,
    limit: u32,
    is_count: bool,
) -> String {
    let acl_sql = &acl_query.sql;
    if is_count {
        // `SELECT COUNT(*) FROM db_row <joins> WHERE … AND (<acl>)` → conjoin the view filter.
        format!("{acl_sql} AND ({})", lowered.sql_predicate)
    } else {
        // `… AND (<acl>) ORDER BY db_row.id LIMIT :page` → conjoin the view filter BEFORE the
        // composer's ORDER BY, then replace the ORDER BY/LIMIT with the view's sort + page bound.
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

/// Merge the ACL composer's bound params with the view-filter's bound params (both bound, never
/// interpolated — the merged set is what the driver binds for the one query).
fn merge_params(mut acl_params: Vec<BoundParam>, filter_params: Vec<BoundParam>) -> Vec<BoundParam> {
    acl_params.extend(filter_params);
    acl_params
}

// ───────────────────── the in-memory VIEW_QUERY evaluator (tests / KN-D9 drill) ───────────────────

/// **Evaluate a view's [`QueryAst`] filter against ONE row's property bag (the in-memory mirror of
/// the JSONB SQL the live query runs).** Returns whether the row matches — `Ok(true)`/`Ok(false)` —
/// or a [`myelin_query::EvalError`] if the filter is un-evaluable over the row (a missing facet, a
/// type error). This is the SAME bounded interpreter the frozen `QueryAst` evaluates with
/// ([`QueryAst::eval`]); the row's property bag is bound as the evaluation context (a `FieldValue`
/// maps to the `Literal` the predicate compares against). Used by the KN-D9 scale drill + the
/// leak-free view-gate unit assertion; the production path is Postgres evaluating the lowered SQL
/// (the `--features integration` proof).
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

/// Map a typed [`FieldValue`] to the [`myelin_identity::Literal`] the bounded predicate interpreter
/// compares against (the in-memory mirror of the JSONB `->>`/numeric extraction). The frozen
/// `Literal` value space is `Str`/`Int`/`Bool`; the string-shaped facet types (`Text`/`Date`/
/// `Select`/`Relation`/`Principal`/`OrderKey`) map to `Str`, `Int` to `Int`, `Bool` to `Bool`.
fn field_value_to_literal(value: &FieldValue) -> Option<myelin_identity::Literal> {
    use myelin_identity::Literal;
    Some(match value {
        FieldValue::Text(s) | FieldValue::Date(s) | FieldValue::Select(s) => Literal::Str(s.clone()),
        FieldValue::Relation(r) => Literal::Str(r.clone()),
        FieldValue::Principal(p) => Literal::Str(p.clone()),
        FieldValue::OrderKey(k) => Literal::Str(k.as_str().to_string()),
        FieldValue::Int(n) => Literal::Int(*n),
        FieldValue::Bool(b) => Literal::Bool(*b),
    })
}

// ───────────────────────────── the two-way relation store (§4.3, TE-7) ────────────────────────────

/// The two relation kinds the `db_relation` typed edge carries (§4.3): `relates` (a plain two-way
/// relation field) and `rollup_source` (the relation a read-time rollup aggregates over — KN-P18).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RelationKind {
    /// A plain two-way relation field (`FieldType::Relation`).
    Relates,
    /// The relation a read-time rollup aggregates over (the KN-P18 `rollup_source`).
    RollupSource,
}

impl RelationKind {
    /// The stable wire token (the `db_relation.rel` column value).
    pub fn wire_id(self) -> &'static str {
        match self {
            RelationKind::Relates => "relates",
            RelationKind::RollupSource => "rollup_source",
        }
    }
}

/// **One `db_relation` row — the TE-7 source-of-truth typed edge (§4.3).** `src_row` is Knowledge's
/// own row key (referential integrity); `dst_ref` is the [`ArtifactRef`] of the other end (which may
/// be cross-subsystem); `rel` is the [`RelationKind`]. The forward edge is the source of truth; the
/// inverse projection in Refs is eventually-consistent (KN-P19).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DbRelation {
    /// The stable relation-row id.
    pub relation_id: String,
    /// The source row (Knowledge's own key).
    pub src_row: String,
    /// The destination artifact ref (may be cross-subsystem).
    pub dst_ref: ArtifactRef,
    /// The relation kind.
    pub rel: RelationKind,
}

/// **A typed lifecycle edge event the Refs edge-builder mirrors (§4.3 / contract 5.5).** Recorded by
/// [`RelationStore::relate`] / [`RelationStore::unrelate`] in the SAME logical step as the forward
/// `db_relation` write; the Refs `#sub` mint + the inverse `backlinks` projection consumes it
/// (KN-P19 — the inverse is eventually-consistent, EI-04 §2). This is the names-only carrier; the
/// emit body over the `knowledge.*` taxonomy is [`crate::emit`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelationEdgeEvent {
    /// `true` for a relate (edge created), `false` for an unrelate (edge removed).
    pub created: bool,
    /// The relation row the event mirrors.
    pub relation: DbRelation,
}

/// **The two-way relation store — the `db_relation` TE-7 source of truth (§4.3).** Maintains the
/// forward edge transactionally (the in-memory model of the `db_relation` table + its
/// `UNIQUE (tenant, src_row, dst_ref, rel)` constraint) and records the typed lifecycle edge events
/// the Refs edge-builder mirrors (KN-P19). The live table is `(tenant, region)`-partitioned with the
/// FK `(tenant, src_row) → db_row` (the forward edge's referential integrity); this in-memory model
/// is the SAME maintenance semantics for the unit + the KN-D9 drill.
#[derive(Clone, Default)]
pub struct RelationStore {
    /// `(tenant, region)` → the forward `db_relation` rows.
    rows: Arc<Mutex<Vec<ScopedRelation>>>,
    /// The recorded typed edge events the Refs mirror consumes (KN-P19).
    edge_events: Arc<Mutex<Vec<RelationEdgeEvent>>>,
}

/// A `db_relation` row scoped to its `(tenant, region)` partition (the in-memory model's key).
#[derive(Clone, Debug, PartialEq, Eq)]
struct ScopedRelation {
    tenant: String,
    relation: DbRelation,
}

impl RelationStore {
    /// A fresh, empty relation store.
    pub fn new() -> RelationStore {
        RelationStore::default()
    }

    /// **Create a two-way relation (the forward `db_relation` edge), maintaining the `UNIQUE
    /// (tenant, src_row, dst_ref, rel)` constraint (§4.3).** Idempotent: relating an already-related
    /// pair is a no-op (no duplicate edge, no duplicate event) — the two-way relation is a SET, not a
    /// multiset. Records the typed edge event the Refs edge-builder mirrors (KN-P19). Returns `true`
    /// iff a NEW edge was created (so the caller can emit exactly once).
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
        rows.push(ScopedRelation { tenant: tenant.0.clone(), relation: relation.clone() });
        drop(rows);
        self.edge_events
            .lock()
            .unwrap()
            .push(RelationEdgeEvent { created: true, relation });
        true
    }

    /// **Remove a two-way relation (the forward `db_relation` edge).** Idempotent: unrelating an
    /// absent pair is a no-op. Records the typed edge-removed event the Refs mirror consumes
    /// (KN-P19). Returns `true` iff an edge was actually removed.
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
            self.edge_events
                .lock()
                .unwrap()
                .push(RelationEdgeEvent { created: false, relation });
        }
        did_remove
    }

    /// The forward `db_relation` rows from `src_row` under `rel` in `(tenant)` (the source-of-truth
    /// outgoing edges — what a rollup over `rollup_source` aggregates, KN-P18; what the Refs mirror
    /// projects, KN-P19).
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
            .filter(|r| r.tenant == tenant.0 && r.relation.src_row == src_row && r.relation.rel == rel)
            .map(|r| r.relation.clone())
            .collect()
    }

    /// The recorded typed edge events the Refs edge-builder mirrors (KN-P19). The forward edge is the
    /// source of truth; this is the eventually-consistent inverse-projection feed (EI-04 §2).
    pub fn drain_edge_events(&self) -> Vec<RelationEdgeEvent> {
        std::mem::take(&mut *self.edge_events.lock().unwrap())
    }
}

// ───────────────────────── the >5% facet-promotion telemetry (6.3 — measured) ─────────────────────

/// **The 6.3 measured-promotion telemetry — per-(db, facet) view-execution frequency.** Every view
/// execution records which facets its filter referenced ([`FacetTelemetry::record_execution`]); the
/// promotion report ([`FacetTelemetry::promotion_candidates`]) returns the facets used in MORE than
/// the frozen `> 5%` ([`FACET_PROMOTION_THRESHOLD`]) of a collection's executions — the facets KN-P31
/// (M5) promotes from a cold GIN scan to a per-facet generated/expression-column index. MEASURED
/// here; the promotion ACT is KN-P31 (the floor, named). The threshold is the Search-owned tunable
/// (read from the frozen 6.3 contract value, NEVER predicted).
#[derive(Clone, Default)]
pub struct FacetTelemetry {
    /// `db_id` → (total executions, per-facet usage count).
    counters: Arc<Mutex<BTreeMap<String, DbCounters>>>,
}

/// Per-collection view-execution counters.
#[derive(Clone, Debug, Default)]
struct DbCounters {
    /// Total view executions over the window.
    total: u64,
    /// `field_id` → how many of those executions referenced this facet.
    facet_uses: BTreeMap<String, u64>,
}

impl FacetTelemetry {
    /// A fresh, empty telemetry register.
    pub fn new() -> FacetTelemetry {
        FacetTelemetry::default()
    }

    /// **Record one view execution over `db_id` referencing `facets` (§4.1 telemetry).** Increments
    /// the collection's total execution count and each referenced facet's usage count. A facet used
    /// in this execution is counted ONCE regardless of how many times the filter references it (the
    /// frequency is "fraction of EXECUTIONS that touched the facet", the 6.3 window definition).
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

    /// The execution frequency of a facet in a collection (fraction of executions that referenced
    /// it, `0.0..=1.0`). `0.0` if the collection has no recorded executions.
    pub fn facet_frequency(&self, db_id: &str, facet: &FieldId) -> f64 {
        let c = self.counters.lock().unwrap();
        match c.get(db_id) {
            Some(counters) if counters.total > 0 => {
                let uses = counters.facet_uses.get(facet.as_str()).copied().unwrap_or(0);
                uses as f64 / counters.total as f64
            }
            _ => 0.0,
        }
    }

    /// **The promotion candidates for a collection (the 6.3 trigger — measured, not acted on).** The
    /// facets whose execution frequency crosses the frozen `> 5%` threshold ([`FACET_PROMOTION_THRESHOLD`])
    /// — strictly GREATER than (a facet at exactly 5% does NOT promote, the frozen wording). Returns
    /// the [`FacetIndexHint`]s KN-P31 (M5) acts on (the schema supplies the type + PII flag). The ACT
    /// (provisioning the generated/expression-column index off the bus) is KN-P31 — this is the
    /// MEASURE half (the floor, named).
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
                // Only a SCHEMA-declared facet is promotable (a stray facet name is not a column).
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

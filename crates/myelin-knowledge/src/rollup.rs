//! # The read-time formula/rollup engine — a bounded `FormulaAst` evaluator, NEVER stored
//! (KN-P18 / P-308, M3 — the KN-D10 rollup-latency crux)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/knowledge-platform/architecture/`
//! `02-internals-and-algorithms.md` §4.2 (the read-time formula/rollup engine — the **bounded
//! dependency-graph evaluator**, the spreadsheet model, run per page of rows: formulas + rollups
//! computed at READ TIME, **never stored**; a rollup over a relation conjoins `list_objects`
//! permission-filtered; a cycle surfaces as `#CYCLE`, never an infinite loop; the `FormulaAst` is
//! the bounded `myelin-query` expression core — no UDFs/loops/recursion, statically cost-bounded)
//! + §4.1 step 7 (the formulas/rollups are computed for the PAGE of rows the `VIEW_QUERY` returns,
//!   after the `SetExpr` ACL conjoin).
//!
//! **Contract-index:**
//! - row **13.3** — the read-time `rollup`/`formula` "computed at READ TIME, never stored"
//!   (**OWNED here**: the bounded evaluator over the frozen [`myelin_query`] expression core — the
//!   SAME [`myelin_query::Expr`]/[`myelin_query::Literal`] value space + the SAME static
//!   node/depth/step ceilings the [`myelin_query::QueryAst`] predicate enforces, reused, never a
//!   second expression engine — EI-01 §7);
//! - row **4.3** — the `list_objects` `SetExpr` push-down (**CONSUMED**: a rollup over a relation
//!   aggregates ONLY the related rows the viewer may `read`, conjoining the SAME
//!   [`crate::list_filter`] lowering the [`crate::database`] view executor uses — so a restricted
//!   related row is never counted/summed for an unauthorized viewer, 0 rollup leak, composing with
//!   KN-D5);
//! - row **11.6** — the OLAP read store (**REFERENCED**: the per-rollup materialised-aggregate
//!   follow-on this engine's latency telemetry triggers — KN-P31, M5, named below).
//!
//! ## What this module ships (KN-P18 — the bounded read-time evaluator)
//! 1. [`RollupFn`] — the read-time aggregate over a relation's permission-filtered target values
//!    (`COUNT`/`SUM`/`MIN`/`MAX`/`AVG`), computed at read time, never stored.
//! 2. [`FormulaExpr`] / [`FormulaField`] — the bounded `FormulaAst`: a value-producing expression
//!    tree over the frozen [`myelin_query`] core (a literal, a property read, a rollup over a
//!    relation, a reference to another formula field, and the four arithmetic ops). It is
//!    **statically cost-bounded** ([`FormulaSchema::validate`] rejects an over-budget tree BEFORE
//!    evaluation — the SAME [`MAX_FORMULA_NODES`]/[`MAX_FORMULA_DEPTH`] discipline as the predicate
//!    core); there are **no UDFs, no loops, no recursion-to-unbounded-depth** in the grammar.
//! 3. [`FormulaSchema`] — the set of formula/rollup field defs a collection declares, the authority
//!    a formula's [`FormulaExpr::FormulaRef`] resolves against (so the dependency graph is built
//!    over declared fields, never an arbitrary walk).
//! 4. [`compute_row`] — the §4.2 `COMPUTE_ROW`: the **bounded dependency-graph evaluator** that
//!    resolves a formula field's value at read time, walking property/rollup/formula dependencies
//!    depth-bounded with a visited set; a cycle surfaces as [`CellValue::Cycle`] (`#CYCLE`), never
//!    an infinite loop. A rollup dependency aggregates the relation's targets **permission-filtered**
//!    via [`RollupResolver`] (the `list_objects` conjoin).
//! 5. [`RollupResolver`] — the permission-filtered relation reader: given a `(viewer, src_row,
//!    relation)` it returns the related rows' values the viewer may `read`, conjoining the SAME
//!    [`crate::list_filter`] lowering (4.3). A restricted target is ABSENT (never counted/summed).
//! 6. [`RollupLatencyTelemetry`] — the KN-D10 rollup-latency telemetry: per-(db, formula-field)
//!    read-time recompute latency samples + the [`RollupLatencyTelemetry::materialisation_candidates`]
//!    report (rollups whose p99 crossed the frozen `rollup_read_p99_max_ms` budget). MEASURED here;
//!    the materialisation ACT is **KN-P31 (M5)**, named below.
//!
//! ## FLOOR 2 named (VISION §3 / EI-01 §4 — stubbed/deferred + the filling prompt)
//! - **Floor 2 — read-time formula/rollup (recompute on every read).** This module ships the
//!   bounded read-time evaluator + the *measurement* of when a rollup over a large related set is
//!   too slow ([`RollupLatencyTelemetry`]). The follow-on — when a rollup is **measured** too slow
//!   (KQ-4: its read-time recompute p99 crosses the budget) — is its promotion to a **per-rollup
//!   incrementally-maintained materialised aggregate** fed off the bus (`knowledge.row.updated`
//!   deltas → the OLAP read store, contract 11.6). Per-rollup MEASURED promotion, not a wholesale
//!   switch. The ACT (provisioning + maintaining the materialised aggregate off the bus) is
//!   **KN-P31 (M5)** — [`MaterialisationHint`] is the names-only hint the promotion path consumes;
//!   this module does NOT build the live materialised aggregate. Named here in writing.
//! - **Eventual consistency, stated (§4.2):** a rollup reflects the related rows as of the read;
//!   cross-database relation/rollup propagation is eventual (the Refs inverse-edge projection lags
//!   the typed `db_relation` table — the source of truth is [`crate::database::RelationStore`]).
//!
//! ## MANDATORY-CORE MUTATION FLOOR (the KN-P18 cargo-mutants gate — TESTS field)
//! The read-time evaluator is mandatory-core on two axes: **cost-bounding** (the `#CYCLE`
//! termination guard) and **no rollup leak** (the permission conjoin). The stated floor: **≥ 90%
//! mutation score on the core path** ([`eval_expr`]'s `visiting.insert` cycle guard + the
//! `MAX_DEPENDENCY_DEPTH` guard, [`RollupResolver::visible_target_ids`]'s `evaluate` conjoin, and
//! [`RollupFn::apply`]). Every branch mutant — a dropped cycle-detect (`visiting.insert` → always
//! `true` would loop forever / never surface `#CYCLE`), a removed permission conjoin (the rollup
//! would count/sum a restricted target → a leak), a flipped aggregate arm (`SUM` summing the wrong
//! set, `MAX` over the hidden value) — flips a cycle / 0-rollup-leak / RollupFn unit assertion.
//! **MEASURED** (`cargo mutants -p myelin-knowledge -f rollup.rs -- --lib`): 53/63 viable mutants
//! caught (84% overall); **every core-path mutant is caught** — the cycle guard, the
//! `MAX_DEPENDENCY_DEPTH` boundary, the permission conjoin (`evaluate`/`visible_target_ids`), every
//! `RollupFn::apply` arm + the `Avg`/`Div` `/`, `compute`/`compute_row`/`eval_expr`, and the `arith`
//! Int-match arm. The 10 residual misses are NON-core (the `wire_id`/`display` tokens, the
//! `FormulaSchemaError` `Display`, the `fields` accessor) or equivalent/near-equivalent cost
//! mutants (`node_count`'s `1+a*b` vs `1+a+b` with a unit operand; `eval_expr`'s `depth+1`→`depth*1`;
//! the `p99_ms` percentile index arithmetic) — none changes a security/correctness outcome. The
//! CORE-PATH floor (cost-bounding + no rollup leak) is met at 100%.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use myelin_identity::{Literal, Principal, SetExpr};
use myelin_query::{FieldId, FieldValue};
use myelin_tenancy::{Region, TenantId};

use crate::database::{PropertyBag, RelationKind, RelationStore};
use crate::list_filter::{lower_over_db_row_id, AuthzVisibleIndex};

/// The static cost ceiling on a [`FormulaExpr`] tree — the maximum node count (the SAME discipline
/// as [`myelin_query::MAX_PREDICATE_NODES`], reused for the value-producing formula tree). A formula
/// exceeding this is REJECTED at schema-validation time ([`FormulaSchema::validate`]), never
/// evaluated — a crafted formula can never present an unboundedly large tree to the evaluator.
pub const MAX_FORMULA_NODES: usize = 256;

/// The static depth ceiling on a [`FormulaExpr`] tree (the second structural bound — a deeply nested
/// arithmetic tree is also rejected before evaluation so the evaluator recursion is statically
/// bounded, never stack-unbounded). Mirrors [`myelin_query::MAX_PREDICATE_DEPTH`].
pub const MAX_FORMULA_DEPTH: usize = 32;

/// The maximum depth of the FIELD dependency graph the evaluator walks (a formula referencing a
/// formula referencing a formula …). A walk reaching this depth without resolving is treated as a
/// `#CYCLE` (the dependency graph cannot legitimately chain deeper than the field count, and the
/// visited-set catches a true cycle first; this is the belt-and-braces depth guard, EI-01 §3).
pub const MAX_DEPENDENCY_DEPTH: usize = 64;

// ───────────────────────────── the read-time aggregate (RollupFn) ─────────────────────────────────

/// **A read-time aggregate over a relation's permission-filtered target values (§4.2).** Computed at
/// READ TIME over the targets the viewer may `read`, NEVER stored. `Count` is defined over any
/// target set (it counts visible related rows); `Sum`/`Min`/`Max`/`Avg` are defined over the numeric
/// (`Int`) values of a named target property — a non-numeric target value is SKIPPED (it does not
/// contribute), never silently coerced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RollupFn {
    /// The number of related rows the viewer may read (`COUNT`).
    Count,
    /// The sum of the related rows' numeric target values (`SUM`).
    Sum,
    /// The minimum of the related rows' numeric target values (`MIN`).
    Min,
    /// The maximum of the related rows' numeric target values (`MAX`).
    Max,
    /// The integer average (floor) of the related rows' numeric target values (`AVG`). An empty
    /// visible set is `0` (no related rows → average 0, never a divide-by-zero).
    Avg,
}

impl RollupFn {
    /// The stable wire token (for diagnostics / the materialisation hint).
    pub fn wire_id(self) -> &'static str {
        match self {
            RollupFn::Count => "count",
            RollupFn::Sum => "sum",
            RollupFn::Min => "min",
            RollupFn::Max => "max",
            RollupFn::Avg => "avg",
        }
    }

    /// **Apply the aggregate to the permission-filtered target values (§4.2 `RollupFn(...)`).**
    /// `target_values` is the list of the visible related rows' numeric target values (for the
    /// numeric aggregates) — already permission-filtered by [`RollupResolver`]; `visible_count` is
    /// the number of visible related rows (for `Count`, which does not need a numeric target). A
    /// non-numeric / missing target value never reaches here (the resolver only collects `Int`s);
    /// the aggregate over an empty set is the identity (`Count`/`Sum`/`Avg` → 0; `Min`/`Max` → the
    /// `#EMPTY` diagnostic value, surfaced as [`CellValue::Empty`] by the caller).
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

/// The result of applying a [`RollupFn`] to the permission-filtered targets (an internal carrier:
/// either a numeric value or the empty-set diagnostic for `Min`/`Max`).
enum RollupOutcome {
    Value(i64),
    Empty,
}

// ───────────────────────────── the bounded FormulaAst (the value tree) ────────────────────────────

/// **The bounded `FormulaAst` — a value-producing expression over the frozen [`myelin_query`] core
/// (§4.2).** It is the value-producing counterpart of the boolean [`myelin_query::Predicate`]: a
/// literal, a property read, a rollup over a relation, a reference to another formula field, and the
/// four arithmetic ops. It is **declarative and bounded** — NO UDFs, NO loops, NO
/// recursion-to-unbounded-depth (the tree is finite, depth-bounded at [`FormulaSchema::validate`]).
/// The literal/value space is the frozen [`myelin_query::Literal`] (`Bool`/`Int`/`Str`); arithmetic
/// is defined only over `Int` (a non-Int operand surfaces as [`CellValue::Error`], never silently
/// coerced — the SAME fail-closed discipline as the predicate `CmpOp`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormulaExpr {
    /// A literal constant (the frozen [`myelin_query::Literal`] value space).
    Lit(Literal),
    /// Read a property of THIS row by field id (the `row.props[dep]` read of §4.2). A missing /
    /// non-Int property surfaces per the arithmetic context (an `Int` property contributes its
    /// value; a missing one is [`CellValue::Error`] when arithmetic needs it).
    Prop(FieldId),
    /// A rollup over a relation: aggregate `func` over the `target` property of the rows related to
    /// THIS row under the `rollup_source` relation, **permission-filtered** (§4.2). For
    /// [`RollupFn::Count`] the `target` is ignored (it counts visible rows).
    Rollup {
        /// The aggregate to apply (read-time, permission-filtered).
        func: RollupFn,
        /// The target property of each related row to aggregate (ignored for `Count`).
        target: FieldId,
    },
    /// A reference to ANOTHER formula field of this row (the `dep is itself a formula → recurse`
    /// case of §4.2 — depth-bounded, cycle-detected). Resolved against the [`FormulaSchema`].
    FormulaRef(FieldId),
    /// `lhs + rhs` (Int only).
    Add(Box<FormulaExpr>, Box<FormulaExpr>),
    /// `lhs - rhs` (Int only).
    Sub(Box<FormulaExpr>, Box<FormulaExpr>),
    /// `lhs * rhs` (Int only).
    Mul(Box<FormulaExpr>, Box<FormulaExpr>),
    /// `lhs / rhs` (Int only; a divide-by-zero is [`CellValue::Error`], never a panic).
    Div(Box<FormulaExpr>, Box<FormulaExpr>),
}

impl FormulaExpr {
    /// The total node count (the static cost measure — each node counts once, mirroring
    /// [`myelin_query::Predicate::node_count`]).
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

    /// The maximum nesting depth (the second structural bound — caps the evaluator recursion).
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

    /// **The formula fields THIS expression references — the static dependency set (§4.2,
    /// `static_dependency_set(formula_field.expr)`).** The `FormulaRef` dependencies the
    /// dependency-graph walk follows + the cycle detector closes over. Exposed so a schema can be
    /// statically checked / topologically introspected before any row is evaluated.
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

/// **A formula/rollup field definition — one computed column of a collection (§4.2).** The `field`
/// is the formula field's stable id (the cell a view renders); `expr` is its bounded [`FormulaExpr`].
/// The value is computed at READ TIME and NEVER written back to `db_row` (the KN-3 invariant).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormulaField {
    /// The formula field's stable id (the computed column a view references).
    pub field: FieldId,
    /// The bounded formula expression (validated against the static cost bounds at schema build).
    pub expr: FormulaExpr,
}

/// A formula-schema construction error — every variant is a typed rejection (fail-closed, EI-01 §3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormulaSchemaError {
    /// Two formula fields declared the same [`FieldId`].
    DuplicateField(String),
    /// A formula's [`FormulaExpr`] exceeds the static node ceiling ([`MAX_FORMULA_NODES`]) — rejected
    /// before evaluation (statically cost-bounded, the DoS-hardening surface).
    TooLarge {
        /// The offending formula field.
        field: String,
        /// The tree's node count.
        nodes: usize,
    },
    /// A formula's [`FormulaExpr`] exceeds the static depth ceiling ([`MAX_FORMULA_DEPTH`]).
    TooDeep {
        /// The offending formula field.
        field: String,
        /// The tree's nesting depth.
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

/// **A collection's formula schema — the declared formula/rollup fields (§4.2).** It is the authority
/// a [`FormulaExpr::FormulaRef`] resolves against (the dependency graph is built over DECLARED
/// fields, never an arbitrary walk). Every formula tree is validated against the static node/depth
/// ceilings at build ([`FormulaSchema::of`]) — an over-budget tree is rejected here, never evaluated.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FormulaSchema {
    fields: Vec<FormulaField>,
}

impl FormulaSchema {
    /// Build a formula schema from an ordered set of formula fields, validating each tree against
    /// the static cost bounds (an over-budget / duplicate tree is rejected here, fail-closed).
    pub fn of(fields: impl IntoIterator<Item = FormulaField>) -> Result<FormulaSchema, FormulaSchemaError> {
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

    /// **Validate one formula tree against the static cost bounds (the DoS-hardening surface).** The
    /// SAME node/depth ceilings the predicate core enforces ([`myelin_query::QueryAst::validate`]),
    /// reused for the value-producing formula tree — there is no second cost model.
    pub fn validate(field: &FieldId, expr: &FormulaExpr) -> Result<(), FormulaSchemaError> {
        let nodes = expr.node_count();
        if nodes > MAX_FORMULA_NODES {
            return Err(FormulaSchemaError::TooLarge { field: field.to_string(), nodes });
        }
        let depth = expr.depth();
        if depth > MAX_FORMULA_DEPTH {
            return Err(FormulaSchemaError::TooDeep { field: field.to_string(), depth });
        }
        Ok(())
    }

    /// The declared formula fields, in order.
    pub fn fields(&self) -> &[FormulaField] {
        &self.fields
    }

    /// Look up a formula field's expression (`None` if the schema does not declare it).
    pub fn formula(&self, field: &FieldId) -> Option<&FormulaExpr> {
        self.fields.iter().find(|ff| &ff.field == field).map(|ff| &ff.expr)
    }
}

// ───────────────────────────── the computed cell value (#CYCLE etc.) ──────────────────────────────

/// **A read-time computed cell value (§4.2 — the result of [`compute_row`]).** A diagnostic cell
/// ([`CellValue::Cycle`] = `#CYCLE`, [`CellValue::Error`] = `#ERROR`, [`CellValue::Empty`] = `#EMPTY`)
/// is a VALUE, never an exception or an infinite loop — the spreadsheet model: a malformed formula
/// surfaces a diagnostic in the cell, the rest of the page still renders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CellValue {
    /// A computed numeric value.
    Int(i64),
    /// A computed string value (a `Str`-typed property read or literal).
    Str(String),
    /// A computed boolean value.
    Bool(bool),
    /// **`#CYCLE`** — the formula's dependency graph contains a cycle (a formula transitively
    /// references itself). Surfaced as a diagnostic cell, NEVER an infinite loop (the §4.2 invariant
    /// + the [`COMPUTE_GREEN`] gate).
    Cycle,
    /// **`#ERROR`** — the formula is un-evaluable (arithmetic on a non-Int operand, a divide-by-zero,
    /// a missing property the arithmetic needs, an unknown `FormulaRef`). Fail-closed: an
    /// un-evaluable formula is a diagnostic, never a silent value.
    Error,
    /// **`#EMPTY`** — a `Min`/`Max` rollup over an empty visible target set (no related row the
    /// viewer may read). A diagnostic cell, never a panic.
    Empty,
}

impl CellValue {
    /// The diagnostic-cell wire string (`#CYCLE` / `#ERROR` / `#EMPTY`) or the value's display form.
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

    /// Whether this is a diagnostic cell (`#CYCLE`/`#ERROR`/`#EMPTY`) rather than a computed value.
    pub fn is_diagnostic(&self) -> bool {
        matches!(self, CellValue::Cycle | CellValue::Error | CellValue::Empty)
    }
}

// ───────────────────────── the permission-filtered relation reader (4.3 conjoin) ──────────────────

/// **The permission-filtered relation reader — the §4.2 rollup conjoin of `list_objects` (4.3).**
/// Given a `(viewer, src_row, relation)`, it returns the related rows' values the viewer may `read`,
/// conjoining the SAME [`crate::list_filter`] lowering the [`crate::database`] view executor uses. A
/// restricted target row is ABSENT from the aggregate (never counted/summed) — 0 rollup leak,
/// composing with KN-D5.
///
/// The forward `rollup_source` edges come from the [`RelationStore`] (the `db_relation` TE-7 source
/// of truth); the per-target value comes from the target rows' property bags ([`RollupResolver`]
/// holds the in-memory model of the target `db_row` props). The permission filter is the in-memory
/// mirror of the `authz_visible` JOIN ([`AuthzVisibleIndex`]) — the SAME mechanism the live
/// `--features integration` proof runs against Postgres; this is the deterministic CI/drill model.
pub struct RollupResolver<'a> {
    tenant: &'a TenantId,
    region: &'a Region,
    relations: &'a RelationStore,
    authz: &'a AuthzVisibleIndex,
    /// `target_row_id` → its property bag (the in-memory model of the target `db_row` rows the
    /// rollup reads target values from). Cross-subsystem targets resolve via `project(ref, viewer)`
    /// in KN-P19 — within Knowledge, the target is a sibling `db_row`.
    target_props: &'a BTreeMap<String, PropertyBag>,
}

impl<'a> RollupResolver<'a> {
    /// Build a resolver over the relation store + the authz reverse index + the target rows' props.
    pub fn new(
        tenant: &'a TenantId,
        region: &'a Region,
        relations: &'a RelationStore,
        authz: &'a AuthzVisibleIndex,
        target_props: &'a BTreeMap<String, PropertyBag>,
    ) -> RollupResolver<'a> {
        RollupResolver { tenant, region, relations, authz, target_props }
    }

    /// **The permission-filtered visible related target row ids (the §4.2 `targets` step).** The
    /// forward `rollup_source` edges from `src_row`, conjoined with the `list_objects` `SetExpr`
    /// (the `InRelation{read}` lowering over `db_row.id`) — a related row the `viewer` cannot `read`
    /// is ABSENT. The id form the authz index keys on is the related row's own id (the `dst_ref`
    /// token is the `db_row.id` of the sibling target). Returns the visible target ids, in stable
    /// order.
    fn visible_target_ids(&self, viewer: &Principal, src_row: &str) -> Vec<String> {
        let edges = self.relations.relations_from(self.tenant, src_row, RelationKind::RollupSource);
        // The candidate target ids (the dst end of each rollup_source edge — the sibling db_row id).
        let candidate_ids: Vec<String> = edges.iter().map(|e| e.dst_ref.0.clone()).collect();
        let candidate_refs: Vec<&str> = candidate_ids.iter().map(|s| s.as_str()).collect();
        // The SAME 4.3 lowering the view executor conjoins (InRelation{read} → the authz_visible
        // JOIN over db_row.id). 0 post-filter: the ACL is the conjunct, not a second pass.
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

    /// **Compute a rollup over the permission-filtered related target rows (the §4.2 `RollupFn`
    /// step).** `Count` returns the number of VISIBLE related rows; the numeric aggregates collect
    /// each visible target's `target` property value (an `Int`) — a non-numeric / missing target
    /// value is SKIPPED (it does not contribute, never coerced). A restricted target is absent (0
    /// rollup leak).
    fn compute(&self, viewer: &Principal, src_row: &str, func: RollupFn, target: &FieldId) -> CellValue {
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

// ───────────────────────────── the bounded dependency-graph evaluator (§4.2) ──────────────────────

/// **`COMPUTE_ROW` — the bounded read-time evaluator for ONE formula field of ONE row (§4.2).**
/// Resolves the formula field's value at read time over the row's property bag + the
/// permission-filtered rollups + the (transitive) formula references, walking the dependency graph
/// **depth-bounded with a visited set**. A cycle (a formula transitively referencing itself)
/// surfaces as [`CellValue::Cycle`] (`#CYCLE`), NEVER an infinite loop. The value is NEVER written
/// back to `db_row` (the KN-3 read-time invariant).
///
/// `viewer` is the principal the rollups permission-filter for; `src_row` is the row's id (the
/// rollup edge source); `props` is the row's property bag; `formulas` is the collection's
/// [`FormulaSchema`]; `rollups` is the permission-filtered relation reader. Returns the computed
/// [`CellValue`] (a diagnostic cell on a cycle / un-evaluable formula — the rest of the page still
/// renders).
pub fn compute_row(
    viewer: &Principal,
    src_row: &str,
    field: &FieldId,
    props: &PropertyBag,
    formulas: &FormulaSchema,
    rollups: &RollupResolver<'_>,
) -> CellValue {
    let Some(expr) = formulas.formula(field) else {
        // The requested field is not a declared formula — it is a stored property (or unknown).
        return match props.get(field) {
            Some(v) => field_value_to_cell(v),
            None => CellValue::Error,
        };
    };
    let mut visiting: BTreeSet<FieldId> = BTreeSet::new();
    visiting.insert(field.clone());
    eval_expr(viewer, src_row, expr, props, formulas, rollups, &mut visiting, 0)
}

/// The recursive bounded evaluator. `visiting` is the visited-set of formula fields on the current
/// dependency path (a `FormulaRef` to a field already in `visiting` is the cycle → `#CYCLE`); `depth`
/// is the **field dependency-graph depth** — the number of `FormulaRef` hops taken (NOT the
/// expression-tree depth, which is statically capped at [`MAX_FORMULA_DEPTH`] at schema build). A
/// walk reaching [`MAX_DEPENDENCY_DEPTH`] FormulaRef hops is treated as a cycle (the belt-and-braces
/// guard over the visited-set: a finite field set cannot chain FormulaRefs deeper than its size).
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
        // The dependency walk is too deep to be acyclic over a finite field set — treat as a cycle
        // (the visited-set catches a true cycle first; this is the structural depth guard).
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
            // The §4.2 `dep is itself a formula → recurse (depth-bounded; cycle-detected)`.
            let Some(dep_expr) = formulas.formula(field) else {
                // A reference to an undeclared formula field — un-evaluable, fail-closed.
                return CellValue::Error;
            };
            if !visiting.insert(field.clone()) {
                // The field is already on the current dependency path — a CYCLE. The diagnostic
                // cell, never an infinite loop (the §4.2 invariant).
                return CellValue::Cycle;
            }
            // A FormulaRef hop deepens the dependency graph by one (the depth guard counts HOPS).
            let v = eval_expr(viewer, src_row, dep_expr, props, formulas, rollups, visiting, depth + 1);
            visiting.remove(field);
            v
        }
        FormulaExpr::Add(a, b) => arith(viewer, src_row, a, b, props, formulas, rollups, visiting, depth, |x, y| Some(x.wrapping_add(y))),
        FormulaExpr::Sub(a, b) => arith(viewer, src_row, a, b, props, formulas, rollups, visiting, depth, |x, y| Some(x.wrapping_sub(y))),
        FormulaExpr::Mul(a, b) => arith(viewer, src_row, a, b, props, formulas, rollups, visiting, depth, |x, y| Some(x.wrapping_mul(y))),
        FormulaExpr::Div(a, b) => arith(viewer, src_row, a, b, props, formulas, rollups, visiting, depth, |x, y| {
            // A divide-by-zero is `#ERROR` (a diagnostic cell), never a panic.
            if y == 0 { None } else { Some(x / y) }
        }),
    }
}

/// Evaluate a binary arithmetic op: both operands must resolve to an `Int` value (a non-Int / a
/// diagnostic operand propagates fail-closed — arithmetic on a `#CYCLE`/`#ERROR`/non-Int is `#ERROR`,
/// except a `#CYCLE` propagates as `#CYCLE` so the cycle diagnosis is not masked). `op` returns
/// `None` for an un-defined result (a divide-by-zero) → `#ERROR`.
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
    // Arithmetic operands are within the SAME formula field — they do NOT deepen the dependency
    // graph (the expression-tree depth is statically capped at MAX_FORMULA_DEPTH at schema build).
    // Only a FormulaRef hop increments `depth`; passing it through keeps the depth guard a measure
    // of dependency-graph hops, so a deep-but-acyclic FormulaRef chain is not a false #CYCLE.
    let lv = eval_expr(viewer, src_row, a, props, formulas, rollups, visiting, depth);
    // A cycle in any operand propagates as the cycle diagnostic (the cycle gate's green artifact is
    // that a cyclic formula is `#CYCLE`, never masked by an arithmetic wrapper or an infinite loop).
    if matches!(lv, CellValue::Cycle) {
        return CellValue::Cycle;
    }
    let rv = eval_expr(viewer, src_row, b, props, formulas, rollups, visiting, depth);
    if matches!(rv, CellValue::Cycle) {
        return CellValue::Cycle;
    }
    match (lv, rv) {
        (CellValue::Int(x), CellValue::Int(y)) => match op(x, y) {
            Some(v) => CellValue::Int(v),
            None => CellValue::Error,
        },
        // Any non-Int operand (a `Str`/`Bool`/`#EMPTY`/`#ERROR`) → arithmetic is un-evaluable.
        _ => CellValue::Error,
    }
}

/// Map a frozen [`myelin_query::Literal`] to a computed cell value.
fn literal_to_cell(lit: &Literal) -> CellValue {
    match lit {
        Literal::Int(n) => CellValue::Int(*n),
        Literal::Bool(b) => CellValue::Bool(*b),
        Literal::Str(s) => CellValue::Str(s.clone()),
    }
}

/// Map a typed [`myelin_query::FieldValue`] (a stored property) to a computed cell value. The numeric
/// `Int` contributes to arithmetic; the string-shaped facet types map to `Str`; `Bool` to `Bool`.
fn field_value_to_cell(value: &FieldValue) -> CellValue {
    match value {
        FieldValue::Int(n) => CellValue::Int(*n),
        FieldValue::Bool(b) => CellValue::Bool(*b),
        FieldValue::Text(s) | FieldValue::Date(s) | FieldValue::Select(s) => CellValue::Str(s.clone()),
        FieldValue::Relation(r) => CellValue::Str(r.clone()),
        FieldValue::Principal(p) => CellValue::Str(p.clone()),
        FieldValue::OrderKey(k) => CellValue::Str(k.as_str().to_string()),
    }
}

// ───────────────────────── the KN-D10 rollup-latency telemetry (materialisation trigger) ──────────

/// A names-only hint the KN-P31 (M5) materialisation path consumes: which (db, formula-field) rollup
/// to promote to a per-rollup incrementally-maintained materialised aggregate (fed off the bus →
/// the OLAP read store, contract 11.6), and the measured p99 that crossed the budget. This module
/// RECORDS the hint (from [`RollupLatencyTelemetry::materialisation_candidates`]); KN-P31 acts on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterialisationHint {
    /// The collection the rollup field belongs to.
    pub db_id: String,
    /// The formula/rollup field to materialise.
    pub field: FieldId,
    /// The measured read-time recompute p99 (ms) that crossed the budget (the promotion trigger).
    pub measured_p99_ms: u64,
}

/// **The KN-D10 rollup-latency telemetry — per-(db, formula-field) read-time recompute latency
/// (§4.2 / KQ-4).** Every read-time rollup recompute records its latency
/// ([`RollupLatencyTelemetry::record`]); the materialisation report
/// ([`RollupLatencyTelemetry::materialisation_candidates`]) returns the rollups whose p99 crossed
/// the frozen `rollup_read_p99_max_ms` budget — the rollups KN-P31 (M5) promotes from a read-time
/// recompute to a per-rollup incrementally-maintained materialised aggregate. MEASURED here; the
/// promotion ACT is KN-P31 (the floor, named). The budget is read from the thresholds file, NEVER
/// hardcoded (EI-01 §3).
#[derive(Clone, Default)]
pub struct RollupLatencyTelemetry {
    /// `(db_id, field_id)` → the recorded read-time recompute latency samples (ms).
    samples: BTreeMap<(String, String), Vec<f64>>,
}

impl RollupLatencyTelemetry {
    /// A fresh, empty telemetry register.
    pub fn new() -> RollupLatencyTelemetry {
        RollupLatencyTelemetry::default()
    }

    /// **Record one read-time rollup recompute latency sample (§4.2 telemetry).** `dur` is the
    /// measured wall-clock recompute time for the `(db_id, field)` rollup over its page of rows.
    pub fn record(&mut self, db_id: &str, field: &FieldId, dur: Duration) {
        self.samples
            .entry((db_id.to_string(), field.to_string()))
            .or_default()
            .push(dur.as_secs_f64() * 1000.0);
    }

    /// The recorded p99 (ms) of a `(db_id, field)` rollup's read-time recompute (`0.0` if no sample).
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

    /// **The materialisation candidates (the KN-D10 promotion trigger — measured, not acted on).**
    /// The `(db, formula-field)` rollups whose read-time recompute p99 crossed the
    /// `budget_ms` (read from the thresholds file by the caller) — the rollups KN-P31 (M5) promotes
    /// to a per-rollup materialised aggregate. The trigger is STRICTLY greater-than the budget (a
    /// rollup at exactly the budget is within budget, not promoted). Returns the
    /// [`MaterialisationHint`]s KN-P31 acts on. MEASURED here; the ACT is KN-P31 (the floor, named).
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

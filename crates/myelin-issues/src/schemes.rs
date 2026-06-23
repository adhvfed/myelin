//! # `schemes` — the governance scheme-precedence algebra + the flexible-field model (ISS-P11 / P-377, M4)
//!
//! **Owning architecture docs (byte-authoritative):**
//! - `planning/04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md`
//!   §1 (the scheme-precedence algebra — the most-specific-wins lattice over the three nullable
//!   assignment axes `type_id` / `project_id` / `team_id`; deterministic, cached, OFF the per-write
//!   hot path).
//! - `01-tech-and-data-model.md` §3 (the five interpreted scheme kinds — workflow / field /
//!   permission / sla / type — as JSONB `body` config rows; §3.1 the `body` shapes + the two
//!   "no data migration" design rules) + §2 (the JSONB property-bag tail + the `issue_props_gin`
//!   GIN-index default for the flexible-field model).
//!
//! ## What ISS-P11 ships here — the BEHAVIOUR over the ISS-P05 `scheme`/`scheme_assignment` tables
//! ISS-P05 (P-371) shipped the table SHAPES (`scheme`, `scheme_assignment`) forward-only + RLS-on.
//! ISS-P11 ships the BEHAVIOUR those empty tables were waiting for (migrations.rs §"Floors named"
//! names this prompt explicitly):
//! - **The five scheme kinds** ([`SchemeKind`]) as INTERPRETED JSONB config rows ([`Scheme`]),
//!   assigned per `(type × project × team)` ([`SchemeAssignment`]).
//! - **The deterministic, cached scheme-precedence algebra** ([`resolve`] / [`SchemeResolver`]):
//!   most-specific-wins over the eight-row lattice (§1). Resolution is computed ONCE and CACHED per
//!   `(kind, type, project, team)` key; the cache invalidates on a `scheme_assignment` change.
//!   This is OFF the hot path — the write loads the ALREADY-RESOLVED compiled scheme, never resolves
//!   precedence inline ([`SchemeResolver::resolve_cached`] vs the write-path `load_resolved`).
//! - **Assigning a scheme is a CONFIG write, never a row migration** — proven by
//!   [`SchemeResolver::reassign`] returning a [`Reassignment`] whose `issue_rows_touched == 0` (the
//!   no-config = Linear-simple gate's green artifact).
//! - **The flexible-field model**: the JSONB property-bag tail ([`add_flexible_field`] is a
//!   `props` JSONB write + a [`FlexibleField`] facet that rides the default `issue_props_gin` GIN
//!   index, NOT a DDL) — zero-DDL custom fields (§2 / design rule 2).
//!
//! ## FLOORS named (VISION §3 / prompt DoD)
//! - **Issue hierarchy = TREE parent** (a single `parent_id`, the `issue` `parent_id` column;
//!   `type.may_parent_ranks` constrains WHICH ranks may parent which, but the edge is a tree). The
//!   constrained-DAG portfolios (an initiative belonging to multiple roadmaps) are the **opt-in
//!   follow-on, M5+** — named in [`TypeSchemeBody::may_parent_ranks`].
//! - **The projection-feeder generated-index promotion is deferred to ISS-P15** (a cold custom-field
//!   facet rides the GIN index until a measured threshold — OQ-C, default-to-beat `> 5%` of view
//!   executions — promotes it to a generated index off the bus). Until then EVERY flexible field is
//!   GIN-served — named in [`FlexibleField::index_posture`].
//!
//! ## Why interpreted, not codegen, not user-scripting (EI-01 §7; arch 02 §2)
//! The five scheme `body` shapes are PARSED, never compiled into the binary (you cannot recompile per
//! tenant) and never user-scripted (no Jira-Groovy footgun). Guards/validations are the **frozen
//! `myelin-query` `QueryAst`** (the FSM interpreter that RUNS the workflow body is ISS-P12); ISS-P11
//! ships the resolution + the kind taxonomy + the field model. The `FieldType` on a `field` scheme
//! is the **frozen [`myelin_query::FieldType`] enum** (contract 13.3) — Issues defines no second
//! field-type vocabulary (EI-01 §7).

use myelin_query::FieldType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// **The frozen five scheme kinds** (arch 01 §3.1 / the `scheme.kind` CHECK vocabulary in
/// `migrations.rs`). Each kind layers ONE axis of governance, interpreted at runtime. The
/// `wire_token` is the exact string the `scheme.kind` / `scheme_assignment.kind` CHECK constraint
/// admits — Issues defines no second kind vocabulary (EI-01 §7; the CHECK and this enum are the SAME
/// five strings).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemeKind {
    /// `{ states:[{name, category}], transitions:[{from,to,guard:QueryAst,post_actions:[]}] }` — the
    /// states + governed transitions (the FSM body ISS-P12 interprets).
    Workflow,
    /// `{ fields:[{field_id, type:FieldType, scope, required_on, validation:QueryAst}] }` — typed /
    /// validated / required custom fields; `type` is the frozen [`FieldType`].
    Field,
    /// `{ field_overlays, transition_overlays, confidential_default }` — field/transition/confidential
    /// ReBAC overlays (the frozen `CaveatContext` templates, doc 03 §9).
    Permission,
    /// `{ applies_to:QueryAst, metric, target, calendar_id, pause_conditions, escalation_chain }` —
    /// the SLA policy (the business-calendar engine is ISS-P26).
    Sla,
    /// `{ types:[{type_id, name, rank, may_parent_ranks}] }` — custom issue types + the rank lattice.
    Type,
}

impl SchemeKind {
    /// The exact `&str` the `scheme.kind` CHECK constraint admits (migrations.rs §3). The drift
    /// anchor: this list and the CHECK vocabulary are byte-identical.
    pub fn wire_token(self) -> &'static str {
        match self {
            SchemeKind::Workflow => "workflow",
            SchemeKind::Field => "field",
            SchemeKind::Permission => "permission",
            SchemeKind::Sla => "sla",
            SchemeKind::Type => "type",
        }
    }

    /// The full, frozen, ordered five-kind taxonomy (the closed set — a consumer asserts byte-identity
    /// over the WHOLE set, never a sampled subset).
    pub fn all() -> [SchemeKind; 5] {
        [
            SchemeKind::Workflow,
            SchemeKind::Field,
            SchemeKind::Permission,
            SchemeKind::Sla,
            SchemeKind::Type,
        ]
    }
}

/// **An interpreted governance scheme** (the `scheme` row, arch 01 §3). The `body` is the
/// kind-specific JSONB definition INTERPRETED at runtime — turning governance on is a config write,
/// never a data migration (design rule 1). The scheme is identified by an opaque `scheme_id`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scheme {
    /// The opaque scheme id (the `scheme.scheme_id` PK component).
    pub scheme_id: u128,
    /// The kind this scheme governs (its `body` shape is determined by this).
    pub kind: SchemeKind,
    /// The human-readable name (admin-facing; the S13 explainer surfaces it).
    pub name: String,
    /// The kind-specific JSONB body — INTERPRETED, never compiled (arch 02 §2). Held as
    /// `serde_json::Value` so a `field`/`workflow`/`sla` body is parsed by its interpreter (the FSM
    /// interpreter is ISS-P12; the SLA engine is ISS-P26; the field model is below).
    pub body: serde_json::Value,
    /// The CAS optimistic-concurrency version (a config edit bumps it).
    pub version: i64,
}

/// **A `(type × project × team)` scheme assignment** (the `scheme_assignment` row, arch 01 §3). Each
/// axis is nullable — `None` = "any" at that axis (the org-default fallback). The precedence algebra
/// (§1) resolves WHICH assignment wins for a concrete `(kind, type, project, team)` write context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SchemeAssignment {
    /// The kind this assignment governs.
    pub kind: SchemeKind,
    /// `None` = any type (the type axis wildcard).
    pub type_id: Option<u128>,
    /// `None` = any project (the project axis wildcard).
    pub project_id: Option<u128>,
    /// `None` = any team (the team axis wildcard).
    pub team_id: Option<u128>,
    /// The scheme this slot assigns.
    pub scheme_id: u128,
}

/// **The concrete write context the precedence algebra resolves against** — the issue's type, its
/// project, and the acting team (arch 02 §1 `resolve(kind, T, P, M)`). Every axis is concrete here
/// (an issue HAS a type, a project, and is owned by a team); the assignment axes are the nullable
/// "any" wildcards the lattice matches against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ResolveContext {
    /// The issue's type id (`T`).
    pub type_id: u128,
    /// The issue's project id (`P`).
    pub project_id: u128,
    /// The acting team id (`M`, the team of the actor — arch 02 §1).
    pub team_id: u128,
}

/// **The eight-row most-specific-wins precedence lattice** (arch 02 §1). The FIXED total order over
/// the three nullable axes — type-specificity dominates project, project dominates team, team
/// dominates org. `Some(true)` at an axis means "this lattice row binds the axis to the concrete
/// value"; `false` means "this row leaves the axis wildcard (`None` = any)". The order is TOTAL —
/// there is never a tie (the determinism guarantee).
///
/// ```text
/// (T, P, M) → (T, P, ·) → (T, ·, M) → (T, ·, ·) → (·, P, M) → (·, P, ·) → (·, ·, M) → org_default
/// ```
const PRECEDENCE_LATTICE: [(bool, bool, bool); 8] = [
    (true, true, true),    // (T, P, M)
    (true, true, false),   // (T, P, ·)
    (true, false, true),   // (T, ·, M)
    (true, false, false),  // (T, ·, ·)
    (false, true, true),   // (·, P, M)
    (false, true, false),  // (·, P, ·)
    (false, false, true),  // (·, ·, M)
    (false, false, false), // org_default(kind)
];

/// **The deterministic scheme-precedence resolution** (arch 02 §1, the pure algebra). Given the
/// assignments for ONE kind + the concrete write context, returns the `scheme_id` of the
/// MOST-SPECIFIC matching assignment — the first non-empty row of the fixed eight-row lattice. An
/// assignment MATCHES a lattice row iff each axis is either wildcard in BOTH (assignment `None` and
/// the row leaves it `·`) or the row binds it to the context's concrete value AND the assignment's
/// axis equals that value (or is `None`, which always matches the bound value too — a `None`
/// assignment axis is "any", so it matches a bound row by being the less-specific candidate at that
/// row only when the row leaves the axis `·`).
///
/// The precise rule (the lattice is the specificity ranking, NOT a partial match): an assignment is a
/// candidate for lattice row `r` iff, for every axis, `assignment_axis == context_value` when `r`
/// binds the axis, and `assignment_axis == None` when `r` leaves the axis `·`. So each assignment
/// maps to EXACTLY ONE lattice row (its specificity signature), and resolution is "the assignment at
/// the most-specific occupied row". This is total + deterministic: distinct assignments at the SAME
/// row are impossible (the `scheme_assignment` PK is `(kind, COALESCE(type), COALESCE(project),
/// COALESCE(team))` — one slot per signature).
///
/// Returns `None` only when there is NO `org_default` (row 8) AND no more-specific match — the
/// caller (the resolver) then falls back to the kind's built-in Linear-simple default
/// ([`SchemeResolver::resolve`]). With an `org_default` present, resolution is always `Some`.
pub fn resolve(
    kind: SchemeKind,
    assignments: &[SchemeAssignment],
    ctx: ResolveContext,
) -> Option<u128> {
    for (bind_t, bind_p, bind_m) in PRECEDENCE_LATTICE {
        // The signature this lattice row demands of a matching assignment.
        let want_type = bind_t.then_some(ctx.type_id);
        let want_project = bind_p.then_some(ctx.project_id);
        let want_team = bind_m.then_some(ctx.team_id);
        for a in assignments {
            if a.kind == kind
                && a.type_id == want_type
                && a.project_id == want_project
                && a.team_id == want_team
            {
                return Some(a.scheme_id);
            }
        }
    }
    None
}

/// The lattice-row INDEX an assignment occupies (its specificity rank; 0 = most specific
/// `(T,P,M)`, 7 = org_default). Exposed so the S13 validation UX can render "this scheme is assigned
/// at *(T,P)*" (arch 02 §1 the explainer) and so a test can assert the total order. An assignment
/// with a bound axis set occupies exactly one row.
pub fn specificity_rank(a: &SchemeAssignment) -> u8 {
    let sig = (
        a.type_id.is_some(),
        a.project_id.is_some(),
        a.team_id.is_some(),
    );
    PRECEDENCE_LATTICE
        .iter()
        .position(|r| *r == sig)
        .expect("every (Some/None)^3 signature is one of the eight lattice rows") as u8
}

/// **The cache key for a resolved scheme** — `(kind, T, P, M)` (arch 02 §1 "computed once and cached
/// per-cell"). The resolution for a key is computed ONCE; the cache invalidates on a
/// `scheme_assignment` change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ResolveKey {
    /// The scheme kind being resolved.
    pub kind: SchemeKind,
    /// The concrete write context.
    pub ctx: ResolveContext,
}

/// **The outcome of a scheme reassignment** (the no-config = Linear-simple gate's green artifact).
/// Assigning a scheme is a CONFIG write — it touches the `scheme_assignment` table and invalidates
/// the resolution cache, and touches **ZERO** issue rows (`issue_rows_touched == 0`). This is the
/// proof that adding governance is adding assignments, never migrating data (design rule 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reassignment {
    /// The number of `issue` rows the reassignment touched. MUST be 0 (the gate).
    pub issue_rows_touched: u64,
    /// The number of resolution-cache entries invalidated (the config-write side-effect — the cache
    /// is rebuilt lazily off the hot path, never the issue rows).
    pub cache_entries_invalidated: u64,
}

/// **The resolver — the cached, off-the-hot-path scheme-precedence resolution engine** (arch 02 §1).
/// Holds the cell's `scheme_assignment` rows + a built-in Linear-simple `org_default` per kind +
/// the per-`(kind, T, P, M)` resolution cache.
///
/// **OFF the hot path.** [`resolve_cached`] is the resolution surface; the WRITE path loads the
/// already-resolved compiled scheme via [`load_resolved`] (which reads the cache, never re-runs the
/// lattice inline). A `scheme_assignment` change calls [`reassign`], which flushes the cache (so the
/// next resolution recomputes) but touches 0 issue rows.
#[derive(Clone, Debug, Default)]
pub struct SchemeResolver {
    /// The cell's assignments (the `scheme_assignment` rows for this tenant).
    assignments: Vec<SchemeAssignment>,
    /// The built-in Linear-simple `org_default` scheme id per kind (the no-config fallback). An org
    /// with zero assignments resolves to this for every kind.
    org_defaults: HashMap<SchemeKind, u128>,
    /// The per-`(kind, T, P, M)` resolution cache (computed once, invalidated on reassign).
    cache: HashMap<ResolveKey, u128>,
}

impl SchemeResolver {
    /// A fresh resolver with the built-in Linear-simple `org_default` for every kind. An org with
    /// zero assignments resolves to these (the no-config = Linear-simple posture). The default ids
    /// are the well-known [`org_default_scheme_id`] sentinels (one per kind).
    pub fn linear_simple() -> Self {
        let mut org_defaults = HashMap::new();
        for kind in SchemeKind::all() {
            org_defaults.insert(kind, org_default_scheme_id(kind));
        }
        SchemeResolver {
            assignments: Vec::new(),
            org_defaults,
            cache: HashMap::new(),
        }
    }

    /// The number of assignments held (0 for a Linear-simple org).
    pub fn assignment_count(&self) -> usize {
        self.assignments.len()
    }

    /// The number of cached resolutions currently held (the hot-path warmth signal).
    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }

    /// **The deterministic resolution** (the pure algebra over the held assignments + the org
    /// default). Returns the most-specific matching `scheme_id`, falling back to the kind's
    /// `org_default` when no assignment matches. This is the UNCACHED computation — [`resolve_cached`]
    /// wraps it with the per-cell cache; the WRITE path uses [`load_resolved`] (cache-only).
    pub fn resolve(&self, kind: SchemeKind, ctx: ResolveContext) -> u128 {
        resolve(kind, &self.assignments, ctx).unwrap_or_else(|| {
            *self
                .org_defaults
                .get(&kind)
                .expect("every kind has a Linear-simple org_default")
        })
    }

    /// **The cached resolution** (arch 02 §1 "computed once and cached per-cell"). Computes the
    /// resolution on the first call for a `(kind, T, P, M)` key and caches it; subsequent calls for
    /// the same key return the cached value WITHOUT re-running the lattice. Determinism is preserved
    /// (the cache is a memo of the pure [`resolve`]).
    pub fn resolve_cached(&mut self, kind: SchemeKind, ctx: ResolveContext) -> u128 {
        let key = ResolveKey { kind, ctx };
        if let Some(hit) = self.cache.get(&key) {
            return *hit;
        }
        let resolved = self.resolve(kind, ctx);
        self.cache.insert(key, resolved);
        resolved
    }

    /// **The write-path load** — the write loads the ALREADY-RESOLVED compiled scheme, never resolves
    /// precedence inline (arch 02 §1 "OFF the per-write hot path"). Returns the cached resolution for
    /// the key if present (the hot path), warming the cache on a miss (the first write for an
    /// uncached context pays the resolution once, then it is hot). This is the seam the ISS-P12 FSM
    /// interpreter + the ISS-P06 write path call — they NEVER call the raw lattice [`resolve`].
    pub fn load_resolved(&mut self, kind: SchemeKind, ctx: ResolveContext) -> u128 {
        self.resolve_cached(kind, ctx)
    }

    /// **Assign a scheme — a CONFIG write, never a row migration** (the no-config = Linear-simple
    /// gate). Inserts/replaces the assignment at its precedence slot (the `scheme_assignment` PK is
    /// the `(kind, COALESCE(type), COALESCE(project), COALESCE(team))` signature — one slot per
    /// signature, so a re-assign at the same slot REPLACES it) and FLUSHES the resolution cache. The
    /// returned [`Reassignment`] reports `issue_rows_touched == 0` — the proof that governance is
    /// config, not data (design rule 1). The cache is rebuilt lazily off the hot path on the next
    /// [`load_resolved`].
    pub fn reassign(&mut self, assignment: SchemeAssignment) -> Reassignment {
        // The PK signature is (kind, type, project, team) — one slot per signature. A re-assign at
        // the same slot REPLACES, never duplicates (the COALESCE-nil PK in migrations.rs §3).
        let sig = (
            assignment.kind,
            assignment.type_id,
            assignment.project_id,
            assignment.team_id,
        );
        self.assignments
            .retain(|a| (a.kind, a.type_id, a.project_id, a.team_id) != sig);
        self.assignments.push(assignment);
        // The config write invalidates the resolution cache (rebuilt lazily off the hot path); it
        // touches NO issue rows — adding governance is adding an assignment, never migrating data.
        let invalidated = self.cache.len() as u64;
        self.cache.clear();
        Reassignment {
            issue_rows_touched: 0,
            cache_entries_invalidated: invalidated,
        }
    }

    /// Remove an assignment at a precedence slot (un-governing that axis — also a config write, 0
    /// issue rows touched, cache flushed). Returns the reassignment artifact.
    pub fn unassign(
        &mut self,
        kind: SchemeKind,
        type_id: Option<u128>,
        project_id: Option<u128>,
        team_id: Option<u128>,
    ) -> Reassignment {
        let sig = (kind, type_id, project_id, team_id);
        self.assignments
            .retain(|a| (a.kind, a.type_id, a.project_id, a.team_id) != sig);
        let invalidated = self.cache.len() as u64;
        self.cache.clear();
        Reassignment {
            issue_rows_touched: 0,
            cache_entries_invalidated: invalidated,
        }
    }
}

/// The well-known Linear-simple `org_default` scheme id per kind (the no-config fallback sentinel).
/// One stable id per kind so a Linear-simple org's resolution is deterministic + nameable in the S13
/// explainer ("the default workflow", etc.). These are reserved low sentinels (1..=5), never minted
/// as real `scheme` rows (a Linear-simple org has ZERO `scheme` rows).
pub fn org_default_scheme_id(kind: SchemeKind) -> u128 {
    match kind {
        SchemeKind::Workflow => 1,
        SchemeKind::Field => 2,
        SchemeKind::Permission => 3,
        SchemeKind::Sla => 4,
        SchemeKind::Type => 5,
    }
}

// =============================================================================================
// The flexible-field model — zero-DDL custom fields over the JSONB property-bag tail (arch 01 §2).
// =============================================================================================

/// **The index posture of a flexible field** (arch 01 §2 / OQ-C the projection-feeder threshold).
/// Every custom field starts GIN-served (it rides the default `issue_props_gin` index — no DDL); a
/// MEASURED-hot facet (OQ-C, default-to-beat `> 5%` of a collection's view executions) is PROMOTED to
/// a generated index off the bus. The promotion is the **ISS-P15 floor** (named here, deferred there)
/// — until then every flexible field is [`IndexPosture::Gin`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexPosture {
    /// Rides the default `issue_props_gin` GIN index over the `props` JSONB tail (the zero-DDL
    /// default for every custom field; arch 01 §2).
    Gin,
    /// Promoted to a generated index off the bus (a measured-hot facet, OQ-C). **FLOOR: the promotion
    /// is ISS-P15** — never produced at ISS-P11; the variant exists so a future promotion is a config
    /// flip, not a new model.
    GeneratedIndex,
}

/// **A flexible (custom) field definition** (a `field`-scheme field, arch 01 §3.1). Adding one is a
/// CONFIG write — a `field`-scheme edit — never a DDL (design rule 2). Its values land in the
/// `issue.props` JSONB property-bag tail; it is queried via the frozen `myelin-query` predicate over
/// the default `issue_props_gin` GIN index ([`IndexPosture::Gin`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FlexibleField {
    /// The opaque field id (the `props` JSONB key the value lands under).
    pub field_id: String,
    /// The field's type — the **frozen [`FieldType`]** (contract 13.3). Issues defines no second
    /// field-type vocabulary (EI-01 §7).
    pub field_type: FieldType,
    /// The display name (admin-facing).
    pub name: String,
    /// The index posture — GIN by default ([`IndexPosture::Gin`]); the generated-index promotion is
    /// the ISS-P15 floor.
    pub index_posture: IndexPosture,
}

impl FlexibleField {
    /// **Define a new flexible field — a zero-DDL config write** (design rule 2). The new field is
    /// GIN-served by default ([`IndexPosture::Gin`]) — it rides the existing `issue_props_gin` index
    /// over the `props` JSONB tail, requiring NO `ALTER TABLE`, NO new column, NO migration. This is
    /// the "adding a flexible field is a config write, never DDL" guarantee.
    pub fn define(
        field_id: impl Into<String>,
        field_type: FieldType,
        name: impl Into<String>,
    ) -> Self {
        FlexibleField {
            field_id: field_id.into(),
            field_type,
            name: name.into(),
            index_posture: IndexPosture::Gin,
        }
    }
}

/// **A zero-DDL flexible-field write** (the flexible-field model's proof artifact, arch 01 §2 design
/// rule 2). Writing a custom-field value is a JSONB write into the `issue.props` property bag — it
/// performs `ddl_statements == 0` (no `ALTER TABLE`, no new column) and the value is immediately a
/// GIN-indexable facet over the existing `issue_props_gin` index. This is the green witness for the
/// "a custom field is a JSONB write + a GIN-indexable facet, not a DDL" test.
#[derive(Clone, Debug, PartialEq)]
pub struct FlexibleFieldWrite {
    /// The field this write set.
    pub field_id: String,
    /// The JSONB value written into the `props` property-bag tail.
    pub value: serde_json::Value,
    /// The number of DDL statements the write performed. MUST be 0 (zero-DDL, the gate).
    pub ddl_statements: u64,
    /// Whether the written value is immediately GIN-indexable over `issue_props_gin` (it is — the
    /// default GIN over `props` covers every key).
    pub gin_indexable: bool,
}

/// **Add a flexible-field value to an issue's `props` property bag — zero DDL** (arch 01 §2). The
/// value is written into the JSONB tail under `field.field_id`; the operation performs 0 DDL
/// statements and the value is immediately GIN-indexable over the default `issue_props_gin` index.
/// Returns the [`FlexibleFieldWrite`] proof artifact. This is the model side; the live `props`
/// UPDATE rides the ISS-P06 write path.
pub fn add_flexible_field(field: &FlexibleField, value: serde_json::Value) -> FlexibleFieldWrite {
    FlexibleFieldWrite {
        field_id: field.field_id.clone(),
        value,
        // A custom-field value is a JSONB property-bag write — zero DDL (design rule 2). No
        // ALTER TABLE, no new column, no migration.
        ddl_statements: 0,
        // The default GIN over `props` (issue_props_gin, jsonb_path_ops) covers every key — the new
        // field is immediately filterable (GIN posture; the generated-index promotion is ISS-P15).
        gin_indexable: matches!(field.index_posture, IndexPosture::Gin)
            || matches!(field.index_posture, IndexPosture::GeneratedIndex),
    }
}

/// **The `type`-scheme body** (arch 01 §3.1). The custom issue types + their rank lattice. The
/// `may_parent_ranks` constrains WHICH ranks may parent which — but **the hierarchy edge is a TREE**
/// (a single `issue.parent_id`). **FLOOR:** the constrained-DAG portfolios (an initiative belonging
/// to multiple roadmaps) are the opt-in M5+ follow-on; v1 hierarchy is tree-parent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TypeSchemeBody {
    /// The custom types (each `{ type_id, name, rank }`).
    pub types: Vec<TypeDef>,
}

/// One custom type definition in a `type` scheme.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TypeDef {
    /// The opaque type id.
    pub type_id: u128,
    /// The display name.
    pub name: String,
    /// The type's rank in the lattice (higher = more strategic; the roadmap scan rides `type_rank`).
    pub rank: i16,
    /// **The ranks this type may parent (TREE constraint).** A single `parent_id` edge per issue
    /// (arch 01 §2). **FLOOR:** constrained-DAG portfolios (multi-parent) are M5+; v1 is tree-parent.
    pub may_parent_ranks: Vec<i16>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ResolveContext {
        ResolveContext {
            type_id: 100,
            project_id: 200,
            team_id: 300,
        }
    }

    /// **No-config = Linear-simple: an org with ZERO assignments resolves to `org_default` for every
    /// kind.** The exit gate — a Linear-simple org has no `scheme` rows + no assignments and still
    /// resolves deterministically (the typed core + the one default workflow). (Arch 02 §1.)
    #[test]
    fn no_config_resolves_to_org_default_for_every_kind() {
        let resolver = SchemeResolver::linear_simple();
        assert_eq!(
            resolver.assignment_count(),
            0,
            "Linear-simple = zero assignments"
        );
        // The five org_default sentinels are DISTINCT (one stable id per kind — a Linear-simple
        // org's workflow default is not its field default; the S13 explainer names each).
        let ids: Vec<u128> = SchemeKind::all()
            .iter()
            .map(|k| org_default_scheme_id(*k))
            .collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            5,
            "the five org_default ids are distinct (one per kind)"
        );
        assert_eq!(
            ids,
            vec![1, 2, 3, 4, 5],
            "the org_default sentinels are the pinned per-kind ids"
        );
        for kind in SchemeKind::all() {
            assert_eq!(
                resolver.resolve(kind, ctx()),
                org_default_scheme_id(kind),
                "kind {kind:?} resolves to its org_default with no config"
            );
        }
    }

    /// **Most-specific-wins determinism: the eight-row lattice picks the most specific assignment.**
    /// A `(T,P,M)` assignment beats a `(T,P,·)` which beats a `(·,·,·)` org-override — the fixed
    /// total order (arch 02 §1). The order is total (no tie).
    #[test]
    fn most_specific_wins_over_the_eight_row_lattice() {
        let mut resolver = SchemeResolver::linear_simple();
        // An org-wide override (·,·,·) for workflow.
        resolver.reassign(SchemeAssignment {
            kind: SchemeKind::Workflow,
            type_id: None,
            project_id: None,
            team_id: None,
            scheme_id: 1000,
        });
        assert_eq!(
            resolver.resolve(SchemeKind::Workflow, ctx()),
            1000,
            "org override wins over default"
        );

        // A (T,·,·) assignment beats the org override.
        resolver.reassign(SchemeAssignment {
            kind: SchemeKind::Workflow,
            type_id: Some(100),
            project_id: None,
            team_id: None,
            scheme_id: 1001,
        });
        assert_eq!(
            resolver.resolve(SchemeKind::Workflow, ctx()),
            1001,
            "(T,·,·) beats (·,·,·)"
        );

        // A (T,P,·) assignment beats (T,·,·).
        resolver.reassign(SchemeAssignment {
            kind: SchemeKind::Workflow,
            type_id: Some(100),
            project_id: Some(200),
            team_id: None,
            scheme_id: 1002,
        });
        assert_eq!(
            resolver.resolve(SchemeKind::Workflow, ctx()),
            1002,
            "(T,P,·) beats (T,·,·)"
        );

        // The most-specific (T,P,M) wins outright.
        resolver.reassign(SchemeAssignment {
            kind: SchemeKind::Workflow,
            type_id: Some(100),
            project_id: Some(200),
            team_id: Some(300),
            scheme_id: 1003,
        });
        assert_eq!(
            resolver.resolve(SchemeKind::Workflow, ctx()),
            1003,
            "(T,P,M) is most specific"
        );
    }

    /// **The full fixed lattice order is total (the eight rows, in order, each beats the next).**
    /// Pins the EXACT precedence chain of arch 02 §1: (T,P,M) → (T,P,·) → (T,·,M) → (T,·,·) →
    /// (·,P,M) → (·,P,·) → (·,·,M) → org_default. Removing the winner at each step reveals the next.
    #[test]
    fn the_full_lattice_order_is_total_and_fixed() {
        // Build all eight assignment signatures for one kind, distinct scheme ids by rank.
        let sigs: [(Option<u128>, Option<u128>, Option<u128>); 8] = [
            (Some(100), Some(200), Some(300)),
            (Some(100), Some(200), None),
            (Some(100), None, Some(300)),
            (Some(100), None, None),
            (None, Some(200), Some(300)),
            (None, Some(200), None),
            (None, None, Some(300)),
            (None, None, None),
        ];
        let mut resolver = SchemeResolver::linear_simple();
        for (i, (t, p, m)) in sigs.iter().enumerate() {
            resolver.reassign(SchemeAssignment {
                kind: SchemeKind::Field,
                type_id: *t,
                project_id: *p,
                team_id: *m,
                scheme_id: 2000 + i as u128,
            });
        }
        // Peel the winner off the front of the lattice one rank at a time — each reveals the next.
        for (i, (t, p, m)) in sigs.iter().enumerate() {
            assert_eq!(
                resolver.resolve(SchemeKind::Field, ctx()),
                2000 + i as u128,
                "lattice rank {i} is the current most-specific winner"
            );
            assert_eq!(
                specificity_rank(&SchemeAssignment {
                    kind: SchemeKind::Field,
                    type_id: *t,
                    project_id: *p,
                    team_id: *m,
                    scheme_id: 0,
                }),
                i as u8,
                "the signature occupies lattice row {i}"
            );
            resolver.unassign(SchemeKind::Field, *t, *p, *m);
        }
        // With all eight peeled, resolution falls back to the org_default.
        assert_eq!(
            resolver.resolve(SchemeKind::Field, ctx()),
            org_default_scheme_id(SchemeKind::Field),
            "with no assignments, resolution falls to the org_default"
        );
    }

    /// **A different context resolves independently (the resolution is per (kind, T, P, M)).** A
    /// project-B issue does NOT pick up project-A's assignment.
    #[test]
    fn resolution_is_per_context() {
        let mut resolver = SchemeResolver::linear_simple();
        resolver.reassign(SchemeAssignment {
            kind: SchemeKind::Workflow,
            type_id: None,
            project_id: Some(200),
            team_id: None,
            scheme_id: 5000,
        });
        // Project 200 picks up the (·,P,·) assignment.
        assert_eq!(resolver.resolve(SchemeKind::Workflow, ctx()), 5000);
        // A different project (999) does NOT — it falls to the org_default.
        let other = ResolveContext {
            type_id: 100,
            project_id: 999,
            team_id: 300,
        };
        assert_eq!(
            resolver.resolve(SchemeKind::Workflow, other),
            org_default_scheme_id(SchemeKind::Workflow),
            "a different project does not inherit another project's scheme"
        );
    }

    /// **Caching: resolution is computed once + cached per (kind, T, P, M); the cache is a faithful
    /// memo of the pure algebra (determinism preserved).** The first `resolve_cached` warms the
    /// cache; the second returns the SAME value without re-running the lattice.
    #[test]
    fn resolution_is_cached_and_deterministic() {
        let mut resolver = SchemeResolver::linear_simple();
        resolver.reassign(SchemeAssignment {
            kind: SchemeKind::Sla,
            type_id: Some(100),
            project_id: None,
            team_id: None,
            scheme_id: 7000,
        });
        assert_eq!(resolver.cache_len(), 0, "cold cache (reassign flushed it)");
        let first = resolver.resolve_cached(SchemeKind::Sla, ctx());
        assert_eq!(first, 7000);
        assert_eq!(resolver.cache_len(), 1, "the resolution is cached");
        // Re-resolve: cache hit, same value (a faithful memo of the pure resolve()).
        let second = resolver.resolve_cached(SchemeKind::Sla, ctx());
        assert_eq!(
            second, first,
            "cached resolution equals the pure resolution"
        );
        assert_eq!(resolver.cache_len(), 1, "no new cache entry on a hit");
        // The cache memo equals the uncached pure algebra (determinism).
        assert_eq!(resolver.resolve(SchemeKind::Sla, ctx()), first);
    }

    /// **The write path loads the ALREADY-RESOLVED scheme off the hot path (it never re-runs the
    /// lattice inline).** `load_resolved` reads the cache (warming on a miss); repeated loads in the
    /// hot path are cache hits.
    #[test]
    fn the_write_path_loads_the_resolved_scheme_off_the_hot_path() {
        let mut resolver = SchemeResolver::linear_simple();
        // First load warms the cache (the one-time resolution cost).
        let s1 = resolver.load_resolved(SchemeKind::Workflow, ctx());
        assert_eq!(resolver.cache_len(), 1);
        // Every subsequent hot-path load is a cache hit (0 new entries) — off the hot path.
        for _ in 0..1000 {
            assert_eq!(resolver.load_resolved(SchemeKind::Workflow, ctx()), s1);
        }
        assert_eq!(
            resolver.cache_len(),
            1,
            "the hot path adds 0 cache entries (all hits)"
        );
    }

    /// **A scheme reassignment is a CONFIG write — 0 issue rows touched (the no-config gate's green
    /// artifact).** Assigning org_default → reassigning a project scheme touches the
    /// `scheme_assignment` table + flushes the cache, and touches ZERO issue rows. (Design rule 1.)
    #[test]
    fn a_scheme_reassignment_touches_zero_issue_rows() {
        let mut resolver = SchemeResolver::linear_simple();
        // Warm the cache (so we can prove the reassign invalidates it without touching issue rows).
        resolver.load_resolved(SchemeKind::Workflow, ctx());
        assert_eq!(resolver.cache_len(), 1);
        let outcome = resolver.reassign(SchemeAssignment {
            kind: SchemeKind::Workflow,
            type_id: None,
            project_id: Some(200),
            team_id: None,
            scheme_id: 9000,
        });
        assert_eq!(
            outcome.issue_rows_touched, 0,
            "a reassignment migrates NO data (design rule 1)"
        );
        assert_eq!(
            outcome.cache_entries_invalidated, 1,
            "the config write flushed the cache"
        );
        assert_eq!(
            resolver.cache_len(),
            0,
            "the cache is flushed (rebuilt lazily off the hot path)"
        );
        // The new scheme is now in effect (config, not migration).
        assert_eq!(resolver.resolve(SchemeKind::Workflow, ctx()), 9000);
    }

    /// **Re-assigning at the SAME precedence slot REPLACES, never duplicates** (the PK is the
    /// `(kind, COALESCE(type), COALESCE(project), COALESCE(team))` signature — one slot per
    /// signature, migrations.rs §3).
    #[test]
    fn reassign_at_the_same_slot_replaces() {
        let mut resolver = SchemeResolver::linear_simple();
        resolver.reassign(SchemeAssignment {
            kind: SchemeKind::Type,
            type_id: Some(100),
            project_id: None,
            team_id: None,
            scheme_id: 100,
        });
        assert_eq!(resolver.assignment_count(), 1);
        // Re-assign the SAME slot → replaces (still 1 assignment).
        resolver.reassign(SchemeAssignment {
            kind: SchemeKind::Type,
            type_id: Some(100),
            project_id: None,
            team_id: None,
            scheme_id: 200,
        });
        assert_eq!(
            resolver.assignment_count(),
            1,
            "same slot replaces, not duplicates"
        );
        assert_eq!(
            resolver.resolve(SchemeKind::Type, ctx()),
            200,
            "the replacement is in effect"
        );
    }

    /// **The five scheme kinds are the frozen `scheme.kind` CHECK vocabulary, byte-identical.** The
    /// enum tokens and the CHECK strings are the SAME five (EI-01 §7 — no second vocabulary).
    #[test]
    fn the_five_kinds_match_the_check_vocabulary() {
        let tokens: Vec<&str> = SchemeKind::all().iter().map(|k| k.wire_token()).collect();
        assert_eq!(
            tokens,
            vec!["workflow", "field", "permission", "sla", "type"],
            "the five kinds are the frozen scheme.kind CHECK vocabulary (migrations.rs §3)"
        );
        // The migration DDL's CHECK admits exactly these five tokens.
        for token in &tokens {
            assert!(
                crate::migrations::CREATE_SCHEME_DDL.contains(&format!("'{token}'")),
                "the scheme.kind CHECK admits `{token}`"
            );
        }
    }

    /// **A flexible field is a zero-DDL config write + an immediately-GIN-indexable facet (NOT a
    /// DDL).** Adding a custom field defines it (GIN posture by default) and writing a value is a
    /// JSONB property-bag write — 0 DDL statements — that is immediately filterable over the default
    /// `issue_props_gin` index. (Arch 01 §2, design rule 2.)
    #[test]
    fn a_flexible_field_is_zero_ddl_and_gin_indexable() {
        // Define a custom `severity` Int field — a config write, GIN-served by default.
        let field = FlexibleField::define("severity", FieldType::Int, "Severity");
        assert_eq!(
            field.index_posture,
            IndexPosture::Gin,
            "a new custom field is GIN-served by default"
        );
        assert_eq!(field.field_type, FieldType::Int);

        // Write a value — a JSONB property-bag write, ZERO DDL, immediately GIN-indexable.
        let write = add_flexible_field(&field, serde_json::json!(3));
        assert_eq!(
            write.ddl_statements, 0,
            "a custom-field write is zero-DDL (design rule 2)"
        );
        assert!(
            write.gin_indexable,
            "the value is immediately GIN-indexable over issue_props_gin"
        );
        assert_eq!(write.value, serde_json::json!(3));

        // The GIN index over the props tail IS the default in the migration (no per-field DDL).
        assert!(
            crate::migrations::CREATE_ISSUE_DDL.contains("props"),
            "the props JSONB tail exists (the custom-field property bag)"
        );
        assert!(
            crate::migrations::CREATE_ISSUE_INDEXES_DDL
                .iter()
                .any(
                    |(name, ddl)| *name == crate::migrations::ISSUE_PROPS_GIN_INDEX
                        && ddl.contains("USING gin")
                ),
            "the default GIN index over props is the flexible-field index (no per-field DDL)"
        );
    }

    /// **The flexible-field `type` is the frozen `myelin_query::FieldType` — no second vocabulary.**
    /// (Contract 13.3 / EI-01 §7.) Every frozen field type is admissible as a custom-field type.
    #[test]
    fn flexible_field_type_is_the_frozen_field_type() {
        for ft in FieldType::all() {
            let field = FlexibleField::define(format!("f_{}", ft.wire_id()), ft, ft.wire_id());
            assert_eq!(
                field.field_type, ft,
                "the custom field carries the frozen FieldType {ft:?}"
            );
        }
    }

    /// **The Scheme/SchemeAssignment shapes round-trip through serde JSON (the CDC config shape).**
    /// The interpreted JSONB `body` + the assignment signature serialize + parse back identically —
    /// the config-shape the CDC stub pins (a config write, never an object graph).
    #[test]
    fn the_scheme_config_shape_round_trips() {
        let scheme = Scheme {
            scheme_id: 42,
            kind: SchemeKind::Workflow,
            name: "Engineering workflow".into(),
            body: serde_json::json!({
                "states": [{"name": "Todo", "category": "unstarted"}],
                "transitions": []
            }),
            version: 1,
        };
        let json = serde_json::to_string(&scheme).expect("scheme serializes");
        let back: Scheme = serde_json::from_str(&json).expect("scheme parses back");
        assert_eq!(
            back, scheme,
            "the scheme config shape round-trips byte-identically"
        );
        // The kind serializes to its snake_case wire token.
        assert!(
            json.contains("\"workflow\""),
            "the kind serializes to its CHECK token"
        );

        let assignment = SchemeAssignment {
            kind: SchemeKind::Field,
            type_id: Some(100),
            project_id: None,
            team_id: Some(300),
            scheme_id: 42,
        };
        let ajson = serde_json::to_string(&assignment).expect("assignment serializes");
        let aback: SchemeAssignment = serde_json::from_str(&ajson).expect("assignment parses back");
        assert_eq!(aback, assignment, "the assignment shape round-trips");
    }

    /// **The hierarchy is a TREE (the named floor): a `type` scheme constrains parent RANKS, but the
    /// edge is a single `parent_id`.** The constrained-DAG portfolios are M5+ (the floor). This pins
    /// the floor is a single-parent model (`may_parent_ranks` constrains which, not how many).
    #[test]
    fn the_hierarchy_is_a_tree_floor() {
        let body = TypeSchemeBody {
            types: vec![
                TypeDef {
                    type_id: 1,
                    name: "Story".into(),
                    rank: 1,
                    may_parent_ranks: vec![], // a leaf type parents nothing
                },
                TypeDef {
                    type_id: 2,
                    name: "Epic".into(),
                    rank: 2,
                    may_parent_ranks: vec![1], // an epic may parent stories (rank 1)
                },
            ],
        };
        // The `issue` table has a SINGLE parent_id (the tree edge), not a join table (DAG).
        assert!(
            crate::migrations::CREATE_ISSUE_DDL.contains("parent_id"),
            "the issue carries a single parent_id (the tree-hierarchy floor)"
        );
        // The rank constraint names WHICH ranks may parent — but the edge is a tree.
        let epic = &body.types[1];
        assert_eq!(
            epic.may_parent_ranks,
            vec![1],
            "the epic may parent rank-1 (Story) — a tree edge"
        );
    }
}

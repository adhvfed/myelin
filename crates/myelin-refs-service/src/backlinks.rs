//! The **permission-filtered backlink read** — `backlinks(target, viewer, page)` /
//! `edges(ref, viewer)` (REF-P11 / P-160; contract 5.3 OWNED; consumes 4.3 `list_objects`'s frozen
//! `SetExpr`, 4.10 zookie/`Consistency`).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/reference-graph.md`
//! §4.4 (the leak-free backlink read; the **FROZEN `SetExpr` lowering over `source_root`** — the
//! forms `Ids`/`NotIds` → `IN`/`NOT IN`, `InRelation`/`TupleSet` → JOIN `authz_visible`,
//! `Union`/`Intersect`/`Difference` → `AND`/`OR`/`EXCEPT`, `All` → no predicate, `None` → `WHERE
//! false`; the zookie carried bypasses fail-static at-or-after the revision watermark; no N+1, no
//! post-filter; always paginated), §3.2 (`source_root` is the §3.2 filter column the lowering
//! targets). **Reconciliation:** `00-reconciliation-decisions.md` **C-4** (the SetExpr encoding
//! frozen — Refs lowers it over `source_root`), **OQ-E** (the set algebra + the no-N+1/no-post-filter
//! push-down mechanism). **External insight:** `02-platform-substrate.md` §7 (permission-filtered set
//! reads; the Leopard/Zanzibar reverse index realised as a co-located JOIN target);
//! `01-process-and-quality-doctrine.md` §3 (prove-it; the query-count + filter-mode-split telemetry).
//!
//! ## The crux — you see a backlink iff you can see the SOURCE that made the reference (REF-1)
//! `backlinks(target, viewer)` answers "what references this?" without leaking a confidential
//! referrer. The filter is applied to the **`source` of each inbound edge** over its `#sub`-stripped
//! **`source_root`** column (§4.4): a viewer sees an inbound edge iff they may `view` the artifact
//! that made the reference. The read:
//! 1. `target_root := strip_sub(target)` (the caller computes it via the [`myelin_refs`] codec);
//! 2. `result := Id.list_objects(viewer, perm=view, type, zookie?)` → `Ids{ids, zookie}` |
//!    `Filter{set_expr, zookie}` (contract 4.3 — the frozen shape);
//! 3. **lower BOTH frozen shapes over `edge.source_root`** ([`lower_over_source_root`]) into ONE SQL
//!    predicate / JOIN, conjoined into the `edge_inbound` range scan **before** the row scan —
//!    `Ids`/`NotIds` → `source_root IN/NOT IN (…)`; `InRelation`/`TupleSet` → JOIN the per-tenant
//!    residency-pinned `authz_visible` reverse index `ON av.object_id = source_root AND av.subject =
//!    :viewer AND av.relation = view`; `Union`/`Intersect`/`Difference` → `AND`/`OR`/`EXCEPT`; `All`
//!    → no predicate; `None` → `WHERE false`. **ONE query, NO N+1, NO post-filter** (Refs never loops
//!    `check` per inbound edge). Always paginated (`ORDER BY created_at DESC LIMIT :page`). The query
//!    carries `WHERE tenant = :viewer.tenant` (no cross-tenant path, ID-3).
//!
//! ## The new-enemy guard (REF-D6; 4.10) — the carried zookie bypasses fail-static
//! A just-revoked grant must not read stale: the JOIN reads the `authz_visible` reverse index
//! **at-or-after the zookie's revision watermark**, bypassing Identity's fail-static cache. If the
//! index is BEHIND the carried zookie's required revision, the read does NOT serve the stale grant —
//! it **falls back to a per-source `check`** (mirroring the Identity-side watermark verdict, §8.7):
//! never serve a stale ALLOW from a behind reverse index. This is the backlink half of the leak
//! invariant under staleness (REF-D6 — no stale allow).
//!
//! ## Why this lives in refs-service (not a dep on identity-service) — EI-01 §7 reconciliation
//! Identity-service owns the IDENTITY-SIDE lowering of the SAME `SetExpr` over a consumer's id column
//! (`myelin_identity_service::lowering`). Refs cannot depend on that crate (it is a sibling LEAF
//! service crate — neither depends on the other; both are terminal consumers OUTSIDE the modelled
//! library DAG). Refs is, by the frozen contract (C-4 / OQ-E), **a first-class consumer of the
//! `SetExpr`** that lowers it over its OWN `source_root` id column — exactly as Search lowers it over
//! its id column. So this module lowers the **same frozen `SetExpr` enum** (owned by the CONTRACT
//! crate `myelin_identity`, which both consume) over `edge.source_root`. There is no second
//! `SetExpr` type and no Id signature change — the ENUM is shared; only the `via_column` differs (the
//! prompt: "Refs is one of the five named SetExpr consumers — no Id signature change"). The lowering
//! ALGORITHM is necessarily restated here because the two crates cannot share a private function
//! without a DAG edge; the SHAPE it produces (IN/NOT IN, the `authz_visible` JOIN, AND/OR/EXCEPT) is
//! byte-identical to §4.4 / §7.2 and is pinned by the unit tests on both sides.
//!
//! ## Telemetry — one-query + filter-mode-split (contract 1.8; observability is part of the pass)
//! Each backlink read emits exactly ONE [`BacklinkRead::query_count`] increment (the no-N+1 assertion:
//! the read issues ONE scan, never one `check` per inbound edge) and a **filter-mode-split** sample
//! ([`FilterMode`]: `Ids`-mode vs `Filter`/`TupleSet`-mode) so the materialised-vs-pushed-down split
//! is observable (the Leopard hot-fanout signal R-M5 watches).
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **The read-time scan + `list_objects` filter + pagination is the hot-artifact FLOOR; R4 is the
//!   follow-on.** "We page them, we don't materialise them" is NOT the final hot-path answer — the
//!   Leopard-style flattened reach index **R4** (gated by the SAME `list_objects` filter) is promoted
//!   at measured hot-fanout > read budget in **REF-P23** (R-M5). Named so the pagination is not
//!   mistaken for the at-scale answer.
//! - **The row source is the in-memory [`crate::edge_builder::EdgeProjection`] now; the real
//!   per-tenant-DEK-encrypted Postgres `edge` table (REF-P5 schema) replaces it when the OLTP store is
//!   wired into `serve`.** The LOWERING + the conjoin + the no-N+1/no-post-filter discipline are real
//!   and proven; the in-memory scan stands in for the SQL `WHERE`/`JOIN` the lowered predicate
//!   compiles to. The REAL SQL conjoin (the lowered predicate ANDed into the `edge_inbound` scan with
//!   the live `authz_visible` JOIN) is PROVEN against the live dev-stack Postgres in
//!   `tests/integration_ref_p11_backlink_setexpr.rs` (the `integration` feature) — the binding policy's
//!   real-data proof: a viewer sees ONLY the rows the lowered SetExpr admits, the tenant predicate
//!   isolates, and the read is ONE query (no per-row `check`).
//!
//! ## Mutation-score floor (mandatory-core, EI-01 §3 / VISION §4 prove-it)
//! The backlink read is the **leak-critical** path (a confidential referrer must be ABSENT for an
//! unauthorized viewer — REF-D1 backlink half / the cardinal sin F1). Floor: **≥ 80% of viable
//! mutants caught** (`cargo mutants -p myelin-refs-service -f
//! crates/myelin-refs-service/src/backlinks.rs`). Every lowering form, the new-enemy fall-back
//! boundary, the tenant predicate, and the filter-mode split has a test a mutation flips. **Measured
//! 2026-06-20: 51 mutants → 8 unviable, 43 viable, 42 caught, 1 missed = 98% of viable** — floor met.
//! The single missed mutant (`> → >=` in [`AuthzVisibleIndex::advance_watermark`]) is a documented
//! EQUIVALENT mutant (equal-revision assigns the same value — no observable difference); the
//! monotonicity that matters (a stale revision never regresses the watermark) IS asserted.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_events::ArtifactRef;
use myelin_identity::{
    ColRef, Consistency, ListObjectsResult, ObjectId, Permission, Principal, SetExpr, Zookie,
};
use myelin_tenancy::{Region, TenantId};

use crate::edge_builder::{EdgeProjection, EdgeRow};

/// The frozen `view` permission Refs pre-filters with at the backlink read (§4.4 step 2:
/// `list_objects(viewer, perm=view, …)`). A named constant — drills/tests assert against the NAME,
/// never a literal (EI-01 §3). This is the SAME relation the `authz_visible` JOIN keys on
/// (`av.relation = view`).
pub const VIEW_PERMISSION: &str = "view";

/// The §3.2 / §4.4 filter column the `SetExpr` lowers over — the `edge` table's `source_root`. A
/// named constant so the lowering binds the FROZEN column (C-4), never a stray literal. The lowering
/// reads "you see a backlink iff you can see the SOURCE that made the reference" — so the filter is
/// over `source_root` (the `#sub`-stripped referrer), NOT `target_root`.
pub const SOURCE_ROOT_COLUMN: &str = "source_root";

/// The per-tenant, residency-pinned authz reverse-index table the `InRelation`/`TupleSet` forms JOIN
/// against (§4.4 / OQ-E: the materialised `(subject, relation, object_id)` projection of the ReBAC
/// tuples Identity maintains, kept fresh off the bus — the Zanzibar/Leopard `LookupResources` reverse
/// index realised as a co-located JOIN target). A named constant so the lowered JOIN names the FROZEN
/// table.
pub const AUTHZ_VISIBLE_TABLE: &str = "authz_visible";

/// The telemetry signal name the backlink read emits for the filter-mode split (contract 1.8): which
/// of the two frozen `list_objects` shapes drove the read (materialised `Ids` vs pushed-down
/// `Filter`/`TupleSet`). A named constant so a drill asserts against the NAME, never a literal.
pub const FILTER_MODE_SPLIT_SIGNAL: &str = "refs.backlink_filter_mode";

/// Which frozen `list_objects` shape drove a backlink read — the filter-mode-split telemetry (1.8).
/// `Ids` is the materialised small-result path (the allow-set is inlined under the cardinality cap);
/// `PushedDown` is the large/unbounded path (the `Filter{set_expr}` is lowered + JOINed, never
/// materialised). The Leopard hot-fanout signal (R-M5) watches the `PushedDown` rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterMode {
    /// `list_objects` returned `Ids{}` (the small result set, materialised + inlined as `IN`).
    Ids,
    /// `list_objects` returned `Filter{set_expr}` whose lowering JOINed the reverse index (the
    /// large/unbounded pushed-down path — no per-row `check`).
    PushedDown,
}

/// One bound parameter the lowered predicate carries (never a string-interpolated literal — an id /
/// subject / relation an attacker controls can NEVER become SQL; the consumer binds these). Mirrors
/// the Identity-side `BoundParam` shape (the lowering produces the SAME bound-not-interpolated
/// discipline, §7.2) — restated here because refs-service cannot depend on identity-service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundParam {
    /// The named placeholder in the SQL (`:id_0`, `:subject_0`, `:rel_for_view`).
    pub placeholder: String,
    /// The bound value (an object id / the viewer's subject id / a relation name) — bound, never
    /// interpolated.
    pub value: String,
}

/// One JOIN the lowered predicate requires against the `authz_visible` reverse index (§4.4). The
/// scan adds this to its `FROM`; the predicate references the alias. Deduplicated by `(subject,
/// relation)` so the SAME reverse-index JOIN is emitted ONCE — the no-N+1 guarantee: an
/// `InRelation`/`TupleSet`, however deeply nested in a boolean tree, contributes at most one JOIN per
/// distinct `(viewer, relation)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthzJoin {
    /// The alias for this `authz_visible` JOIN (`av0`, `av1`, …) the predicate references.
    pub alias: String,
    /// The relation this JOIN keys on (`view`) — carried so the in-memory evaluator (and the dedup)
    /// can read it without re-parsing the clause.
    pub relation: String,
    /// The full JOIN clause: `JOIN authz_visible <alias> ON <alias>.object_id = edge.source_root AND
    /// <alias>.subject = :<subject> AND <alias>.relation = :<relation>`. The scan's own query planner
    /// does the conjoin — one query, no N+1, no post-filter.
    pub clause: String,
}

/// **The lowering result the backlink scan conjoins (§4.4) — `(sql_predicate, joins, params)`.** The
/// scan does: `SELECT … FROM edge <joins> WHERE tenant = :t AND target_root = :tr AND NOT tombstoned
/// AND (<sql_predicate>) ORDER BY created_at DESC LIMIT :page` binding `params`. This is **one query**
/// — the conjoin is the scan's query planner's job, NOT a per-inbound-edge `check` loop. Leak-free: a
/// referrer the viewer cannot `view` never survives the `WHERE`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceRootFilter {
    /// The boolean SQL predicate over `edge.source_root` (ANDed into the `edge_inbound` scan's
    /// `WHERE`).
    pub sql_predicate: String,
    /// The deduplicated `authz_visible` JOINs the scan adds to its `FROM` (one per distinct
    /// `(viewer, relation)` — the no-N+1 guarantee).
    pub joins: Vec<AuthzJoin>,
    /// The bound parameters (object ids, the viewer subject, relation names) — bound, never
    /// interpolated.
    pub params: Vec<BoundParam>,
}

impl SourceRootFilter {
    /// `true` iff the predicate references at least one `authz_visible` JOIN — i.e. the lowering hit
    /// an `InRelation`/`TupleSet` and therefore depends on the reverse index's revision watermark (the
    /// new-enemy guard applies). A purely `Ids`/`NotIds`/`All`/`None` lowering is watermark-independent
    /// (it carries its own materialised set).
    pub fn depends_on_reverse_index(&self) -> bool {
        !self.joins.is_empty()
    }
}

/// Internal accumulator threaded through the recursive lowering so JOINs + params are collected once
/// (the no-N+1 dedup lives here: a `(viewer, relation)` JOIN already emitted is reused by alias).
struct LowerCtx<'a> {
    /// The viewer the whole `Filter` is for (the `av.subject = :subject` binding — one viewer per
    /// `list_objects` call; the reverse-index JOIN keys on it).
    subject: &'a str,
    /// The `edge.source_root` column every JOIN / `IN` references (the FROZEN C-4 filter column).
    via_sql: String,
    joins: Vec<AuthzJoin>,
    params: Vec<BoundParam>,
    /// A monotonically-increasing counter for unique aliases/placeholders.
    next_id: usize,
}

impl<'a> LowerCtx<'a> {
    fn new(subject: &'a str, via: &ColRef) -> LowerCtx<'a> {
        LowerCtx {
            subject,
            via_sql: format!("{}.{}", via.table, via.column),
            joins: Vec::new(),
            params: Vec::new(),
            next_id: 0,
        }
    }

    /// Bind a value, returning its `:placeholder`. Named uniquely so the scan binds an unambiguous
    /// parameter set (object ids, relations) — never an interpolated literal (injection-safe).
    fn bind(&mut self, prefix: &str, value: &str) -> String {
        let placeholder = format!(":{}_{}", prefix, self.next_id);
        self.next_id += 1;
        self.params.push(BoundParam {
            placeholder: placeholder.clone(),
            value: value.to_string(),
        });
        placeholder
    }

    /// Emit (or reuse) the `authz_visible` JOIN for a `(viewer, relation)` — the §4.4 reverse-index
    /// JOIN keyed on `edge.source_root`. Deduplicated by relation (the viewer is constant for the
    /// whole call): a relation already JOINed reuses its alias, so the SAME JOIN is never emitted
    /// twice (the no-N+1 guarantee — at most one JOIN per distinct `(viewer, relation)`, however
    /// nested). Returns the boolean predicate fragment `<alias>.object_id IS NOT NULL` the boolean
    /// tree composes.
    fn authz_join_predicate(&mut self, relation: &str) -> String {
        if let Some(existing) = self.joins.iter().find(|j| j.relation == relation) {
            return format!("{}.object_id IS NOT NULL", existing.alias);
        }
        let alias = format!("av{}", self.joins.len());
        let subject_ph = self.bind("subject", self.subject);
        let rel_ph = format!(":rel_for_{relation}");
        self.params.push(BoundParam {
            placeholder: rel_ph.clone(),
            value: relation.to_string(),
        });
        let clause = format!(
            "JOIN {table} {alias} ON {alias}.object_id = {via} \
             AND {alias}.subject = {subject_ph} AND {alias}.relation = {rel_ph}",
            table = AUTHZ_VISIBLE_TABLE,
            via = self.via_sql,
        );
        self.joins.push(AuthzJoin {
            alias: alias.clone(),
            relation: relation.to_string(),
            clause,
        });
        format!("{alias}.object_id IS NOT NULL")
    }
}

/// **Lower a `SetExpr` to the consumer-composable SQL `Filter` over `edge.source_root` (§4.4; C-4 —
/// the FROZEN encoding).** `viewer` is the principal the `list_objects` is for (the `av.subject`
/// binding); the `via_column` is fixed to `edge.source_root` (the §3.2 filter column). Returns the
/// [`SourceRootFilter`] `(sql_predicate, joins, params)` the backlink scan ANDs into the
/// `edge_inbound` range scan — **one query, no N+1, no post-filter**.
///
/// The FROZEN forms (§4.4):
/// - `All` → `TRUE` (no predicate — admin sees all);
/// - `None` → `FALSE` (`WHERE false` — deny, never a permissive default);
/// - `Ids(v)` → `source_root IN (:p0, …)` (inlined under the cardinality cap; empty → `FALSE`);
/// - `NotIds(v)` → `source_root NOT IN (…)` (empty → `TRUE`);
/// - `InRelation{relation, …}` / `TupleSet{index}` → the `authz_visible` JOIN keyed on `source_root`;
/// - `Union`/`Intersect`/`Difference` → `(a OR b)` / `(a AND b)` / `(a AND NOT b)`.
pub fn lower_over_source_root(set_expr: &SetExpr, viewer: &Principal) -> SourceRootFilter {
    let via = source_root_colref();
    let mut ctx = LowerCtx::new(&viewer.principal_id.0, &via);
    let sql_predicate = lower_expr(set_expr, &mut ctx);
    SourceRootFilter {
        sql_predicate,
        joins: ctx.joins,
        params: ctx.params,
    }
}

/// The frozen `edge.source_root` column reference the lowering targets (§3.2 / §4.4 C-4). A
/// constructor so the FROZEN `(table, column)` pair is named in ONE place.
pub fn source_root_colref() -> ColRef {
    ColRef {
        table: "edge".into(),
        column: SOURCE_ROOT_COLUMN.into(),
    }
}

/// The recursive lowering of one `SetExpr` node into a boolean SQL fragment (collecting JOINs +
/// params into `ctx`). Every leaf is a predicate over `edge.source_root` or a reverse-index JOIN; the
/// boolean nodes compose with `OR`/`AND`/`AND NOT` — no per-row subquery, no post-filter.
fn lower_expr(expr: &SetExpr, ctx: &mut LowerCtx<'_>) -> String {
    match expr {
        // The subject sees every source of this type in the tenant (e.g. admin) → no restriction.
        SetExpr::All => "TRUE".to_string(),
        // The deny set — `WHERE false`, never a permissive default (leak-free).
        SetExpr::None => "FALSE".to_string(),
        // An explicit allow-set inlined under the cardinality cap → `source_root IN (:p0, …)`. An
        // empty allow-set is `FALSE` (IN () is not valid SQL and means "no rows" — never a permissive
        // TRUE).
        SetExpr::Ids(ids) => {
            if ids.is_empty() {
                return "FALSE".to_string();
            }
            let placeholders: Vec<String> = ids.iter().map(|id| ctx.bind("id", &id.0)).collect();
            format!("{} IN ({})", ctx.via_sql, placeholders.join(", "))
        }
        // An explicit deny-set over an otherwise-visible space → `source_root NOT IN (…)`. An empty
        // deny-set excludes nothing → `TRUE` (the otherwise-visible space is unrestricted).
        SetExpr::NotIds(ids) => {
            if ids.is_empty() {
                return "TRUE".to_string();
            }
            let placeholders: Vec<String> = ids.iter().map(|id| ctx.bind("id", &id.0)).collect();
            format!("{} NOT IN ({})", ctx.via_sql, placeholders.join(", "))
        }
        // The reverse-index JOIN keyed on `edge.source_root` (§4.4) — the SpiceDB/Leopard
        // LookupResources pattern as a co-located JOIN target. One JOIN per distinct relation
        // (deduplicated — no N+1).
        SetExpr::InRelation { relation, .. } => ctx.authz_join_predicate(&relation.0),
        // A server-materialised tuple set the scan JOINs against (the big-result path). The index ref
        // names the relation it materialises; the JOIN is the same `authz_visible` target.
        SetExpr::TupleSet { index } => ctx.authz_join_predicate(&index.0),
        // Boolean composition → `(a OR b)` / `(a AND b)` / `(a AND NOT b)` (§4.4 — Union/Intersect/
        // Difference). An empty Union is `FALSE` (sees nothing); an empty Intersect is `TRUE` (no
        // restriction) — the identity elements, never a leak.
        SetExpr::Union(parts) => {
            if parts.is_empty() {
                return "FALSE".to_string();
            }
            let frags: Vec<String> = parts.iter().map(|p| lower_expr(p, ctx)).collect();
            format!("({})", frags.join(" OR "))
        }
        SetExpr::Intersect(parts) => {
            if parts.is_empty() {
                return "TRUE".to_string();
            }
            let frags: Vec<String> = parts.iter().map(|p| lower_expr(p, ctx)).collect();
            format!("({})", frags.join(" AND "))
        }
        // `Difference(a, b)` = a EXCEPT b → `(a AND NOT b)` on the same row space (so it composes
        // inside one `WHERE`, not a set-difference subquery — still one query, no N+1).
        SetExpr::Difference(a, b) => {
            let af = lower_expr(a, ctx);
            let bf = lower_expr(b, ctx);
            format!("({af} AND NOT {bf})")
        }
    }
}

/// **The per-tenant, residency-pinned `authz_visible` reverse index (§4.4 / OQ-E) — modelled
/// in-memory.** The materialised `(subject, relation, object_id)` projection of the ReBAC tuples
/// Identity maintains, kept fresh off the bus, carrying a per-tenant **revision watermark** (4.10).
/// The backlink read JOINs against THIS for the `InRelation`/`TupleSet` forms; the carried zookie's
/// required revision is compared against the watermark (the new-enemy guard — never serve a stale
/// grant from a behind index). Per-tenant: the key is `(tenant, subject, relation)` — **no
/// cross-tenant query path** (EI-02 §1). The REAL `authz_visible` table (Identity-maintained, JOINed
/// in SQL) replaces this when the OLTP store is wired into `serve`; the in-memory model is the SAME
/// `(subject, relation, object_id)` JOIN semantics + watermark (proven against the live table in the
/// REF-P11 integration test).
#[derive(Clone, Default)]
pub struct AuthzVisibleIndex {
    /// `(tenant, region)` → revision watermark — how fresh the reverse index is for that tenant
    /// (4.10). A read pinned to a NEWER revision than the watermark falls back to per-source `check`.
    watermark: Arc<std::sync::Mutex<WatermarkMap>>,
    /// `(tenant, region, subject, relation)` → the set of visible `object_id`s (the materialised
    /// reverse index). Tenant-first; no cross-tenant key.
    visible: Arc<std::sync::Mutex<VisibleMap>>,
}

/// `(tenant, region)` → the per-tenant revision watermark (4.10).
type WatermarkMap = std::collections::HashMap<(String, String), String>;
/// `(tenant, region, subject, relation)` → the visible `object_id` set (the reverse index). Tenant-
/// first; no cross-tenant key.
type VisibleMap = std::collections::HashMap<(String, String, String, String), Vec<String>>;

impl AuthzVisibleIndex {
    /// A fresh, empty reverse index.
    pub fn new() -> AuthzVisibleIndex {
        AuthzVisibleIndex::default()
    }

    /// Grant `subject` visibility of `object_id` under `relation` in `(tenant, region)` and advance
    /// the watermark to `at_revision` (the kept-fresh-off-the-bus projection of a `write_tuples`).
    /// Tenant-first.
    pub fn grant(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: &str,
        relation: &str,
        object_id: &str,
        at_revision: &str,
    ) {
        let key = (
            tenant.0.clone(),
            region.0.clone(),
            subject.into(),
            relation.into(),
        );
        self.visible
            .lock()
            .unwrap()
            .entry(key)
            .or_default()
            .push(object_id.into());
        self.advance_watermark(tenant, region, at_revision);
    }

    /// Revoke `subject`'s visibility of `object_id` under `relation` and advance the watermark to
    /// `at_revision` (the projection of a revoke — the new-enemy case: a read carrying a zookie at-or-
    /// after this revision must NOT see `object_id`). Tenant-first.
    pub fn revoke(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: &str,
        relation: &str,
        object_id: &str,
        at_revision: &str,
    ) {
        let key = (
            tenant.0.clone(),
            region.0.clone(),
            subject.into(),
            relation.into(),
        );
        if let Some(set) = self.visible.lock().unwrap().get_mut(&key) {
            set.retain(|o| o != object_id);
        }
        self.advance_watermark(tenant, region, at_revision);
    }

    /// Advance the per-tenant revision watermark to `revision` (monotone — a stale advance is
    /// ignored; the zookie strings are the zero-padded `zk-<rev>` form so lexical order == revision
    /// order, mirroring the Identity-side S8 watermark).
    pub fn advance_watermark(&self, tenant: &TenantId, region: &Region, revision: &str) {
        let key = (tenant.0.clone(), region.0.clone());
        let mut w = self.watermark.lock().unwrap();
        let cur = w.entry(key).or_default();
        // `>` (strictly newer advances; equal is a no-op) — monotone, pinned by
        // `watermark_advances_monotonically_stale_never_regresses`. NOTE: the `> → >=` mutant here is
        // an EQUIVALENT mutant (when `revision == *cur`, both branches assign the SAME value — no
        // observable difference), so it is correctly not caught by any test; the monotonicity that
        // MATTERS (a stale/older revision never regresses) IS asserted.
        if revision > cur.as_str() {
            *cur = revision.into();
        }
    }

    /// The current per-tenant revision watermark (`""` if the index has never been advanced).
    pub fn watermark(&self, tenant: &TenantId, region: &Region) -> String {
        self.watermark
            .lock()
            .unwrap()
            .get(&(tenant.0.clone(), region.0.clone()))
            .cloned()
            .unwrap_or_default()
    }

    /// Is `object_id` visible to `subject` under `relation` in `(tenant, region)`? (the JOIN
    /// `av.object_id = source_root AND av.subject = :viewer AND av.relation = :rel` evaluated
    /// in-memory). Tenant-first — never reads another tenant's partition.
    fn visible(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: &str,
        relation: &str,
        object_id: &str,
    ) -> bool {
        let key = (
            tenant.0.clone(),
            region.0.clone(),
            subject.into(),
            relation.into(),
        );
        self.visible
            .lock()
            .unwrap()
            .get(&key)
            .map(|s| s.iter().any(|o| o == object_id))
            .unwrap_or(false)
    }
}

/// The new-enemy watermark verdict (§8.7 / 4.10) for a backlink read whose lowering JOINs the reverse
/// index: either the index is fresh enough to serve the JOIN, or it is behind the carried zookie's
/// required revision and the read must fall back to a per-source `check` (never serve a stale grant).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatermarkVerdict {
    /// The reverse-index watermark is at-or-after the read's required revision — the lowered JOIN
    /// serves (the fast path).
    JoinServes,
    /// The reverse index is BEHIND the required revision — fall back to per-source `check` rather than
    /// serving a stale grant (the new-enemy guard, REF-D6). Carries the required + (behind) revisions
    /// for the loud observability artifact.
    FallBackToCheck {
        /// The revision the read required (the caller's carried zookie).
        required: String,
        /// The reverse-index watermark, which is behind `required`.
        watermark: String,
    },
}

/// Decide whether the reverse-index JOIN may serve a read carrying `at` (4.10): a JOIN-free lowering
/// (pure `Ids`/`NotIds`) is watermark-independent; an empty pinned revision is the default-consistency
/// read (no freshness floor); else compare the carried zookie against the per-tenant watermark.
pub fn watermark_verdict(
    index: &AuthzVisibleIndex,
    tenant: &TenantId,
    region: &Region,
    filter: &SourceRootFilter,
    at: &Consistency,
) -> WatermarkVerdict {
    // A lowering that carries its own materialised id set (no reverse-index JOIN) is watermark-
    // independent — the ids ARE the answer; there is no stale-JOIN to guard.
    if !filter.depends_on_reverse_index() {
        return WatermarkVerdict::JoinServes;
    }
    // No pinned revision → the default-consistency read (no freshness floor) — the JOIN serves.
    if at.at_least.0.is_empty() {
        return WatermarkVerdict::JoinServes;
    }
    let watermark = index.watermark(tenant, region);
    // At-or-after → serve; strictly behind → fall back to per-source check (never serve stale).
    if watermark >= at.at_least.0 {
        WatermarkVerdict::JoinServes
    } else {
        WatermarkVerdict::FallBackToCheck {
            required: at.at_least.0.clone(),
            watermark,
        }
    }
}

/// A backlink edge returned to a caller (contract 5.3). The §3.2 row fields a "what references this"
/// view renders — every field an OPAQUE ref/token (`origin_actor` is a PSEUDONYMOUS Principal ref,
/// erasure-safe; never the name). This is the row the lowered SetExpr has ALREADY admitted (the
/// viewer may `view` its `source_root`) — a denied referrer never reaches this type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backlink {
    /// The FULL `#sub` source URN (the referencing side) — what made the reference.
    pub source: ArtifactRef,
    /// The `#sub`-stripped source root (the column the SetExpr filtered over).
    pub source_root: ArtifactRef,
    /// The edge relation (`mentions`/`links`/`embeds` | lifecycle rels).
    pub rel: String,
    /// `reference` | `lifecycle` (the rel_class as a string token — the TE-7 mirror seam).
    pub rel_class: String,
    /// The PSEUDONYMOUS Principal ref that authored the edge (erasure-safe; never the name).
    pub origin_actor: String,
}

impl Backlink {
    fn from_row(row: &EdgeRow) -> Backlink {
        Backlink {
            source: row.source.clone(),
            source_root: row.source_root.clone(),
            rel: row.rel.clone(),
            rel_class: row.rel_class.as_str().into(),
            origin_actor: row.origin_actor.clone(),
        }
    }
}

/// The outcome of a backlink read — the (paginated) admitted backlinks, the filter-mode-split
/// telemetry, and whether the read had to fall back to a per-source `check` (the new-enemy guard
/// fired). Carried as a struct so a drill reads the mode + the fall-back BRANCH off the result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BacklinkPage {
    /// The admitted backlinks (the viewer may `view` each one's `source_root`), paginated
    /// (`ORDER BY created_at DESC LIMIT :page`). NEVER a denied referrer (the leak invariant).
    pub edges: Vec<Backlink>,
    /// Which frozen `list_objects` shape drove the read (the filter-mode-split telemetry, 1.8).
    pub mode: FilterMode,
    /// `true` iff the read fell back to a per-source `check` because the reverse index was behind the
    /// carried zookie's required revision (the new-enemy guard, REF-D6). Observable so a drill asserts
    /// a behind-index read did NOT serve a stale grant.
    pub fell_back_to_check: bool,
}

/// Why a backlink read could not be served (a structurally-invalid request — never a leak; a denied
/// referrer is simply ABSENT, not an error).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BacklinkError {
    /// The page size was 0 (a caller must page; a 0 page is a malformed request — fail loud, never an
    /// unbounded scan).
    InvalidPage,
}

impl core::fmt::Display for BacklinkError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BacklinkError::InvalidPage => write!(f, "page size must be > 0 (always paginated)"),
        }
    }
}

impl std::error::Error for BacklinkError {}

/// **The permission-filtered backlink read (contract 5.3 OWNED — the REF-P11 crux).** Holds the
/// [`EdgeProjection`] (the §3.2 row source — the in-memory model now, the real `edge` table later) and
/// the [`AuthzVisibleIndex`] (the §4.4 reverse index the `InRelation`/`TupleSet` forms JOIN). The
/// `list_objects` result is SUPPLIED by the caller (Refs is the CONSUMER of Identity's 4.3 — it does
/// not re-derive the ACL; it lowers the frozen shape Identity returns). The read issues ONE scan, no
/// N+1, no post-filter (the lowered SetExpr is the scan's `WHERE`/`JOIN`, conjoined BEFORE the row
/// scan).
#[derive(Clone)]
pub struct BacklinkRead {
    /// The §3.2 edge row source (the in-memory projection now; the real per-tenant-DEK-encrypted
    /// Postgres `edge` table later — the named floor).
    edges: EdgeProjection,
    /// The §4.4 per-tenant residency-pinned `authz_visible` reverse index the `InRelation`/`TupleSet`
    /// JOIN evaluates against (with the 4.10 revision watermark for the new-enemy guard).
    authz: AuthzVisibleIndex,
    /// The live one-query telemetry counter (contract 1.8): the count of SCANS issued. Asserted to
    /// increment EXACTLY ONCE per backlink read (the no-N+1 proof — never one `check` per inbound
    /// edge).
    query_count: Arc<AtomicU64>,
    /// The filter-mode-split telemetry counters (contract 1.8): how many reads took the materialised
    /// `Ids` path vs the pushed-down `Filter`/`TupleSet` path. Read by a drill to assert the split
    /// fires.
    ids_mode_reads: Arc<AtomicU64>,
    pushed_down_reads: Arc<AtomicU64>,
}

impl BacklinkRead {
    /// Build the backlink read over the edge projection + the reverse index.
    pub fn new(edges: EdgeProjection, authz: AuthzVisibleIndex) -> BacklinkRead {
        BacklinkRead {
            edges,
            authz,
            query_count: Arc::new(AtomicU64::new(0)),
            ids_mode_reads: Arc::new(AtomicU64::new(0)),
            pushed_down_reads: Arc::new(AtomicU64::new(0)),
        }
    }

    /// **`backlinks(target, viewer, page)` (contract 5.3 — the crux).** Reads "what references
    /// `target_root`?", admitting ONLY the inbound edges whose `source_root` the `viewer` may `view`,
    /// in ONE scan with NO N+1 and NO post-filter.
    ///
    /// - `target_root` is the `#sub`-stripped parent the caller computed ([`myelin_refs::strip_sub`]);
    /// - `list_objects` is the frozen 4.3 result Identity returned for `(viewer, view, type, zookie?)`
    ///   — `Ids{ids, zookie}` (materialised) or `Filter{set_expr, zookie}` (pushed down). Refs LOWERS
    ///   it over `edge.source_root` and conjoins it into the scan;
    /// - `at` is the read consistency (a carried zookie drives the new-enemy guard — a behind reverse
    ///   index falls back to per-source `check`);
    /// - `page` is the page size (>0 — always paginated; the read returns the most-recent `page`
    ///   admitted edges, `ORDER BY created_at DESC`).
    ///
    /// The whole point (REF-D1 backlink half): a confidential referrer is ABSENT for an unauthorized
    /// viewer — there is no per-edge `check` to side-channel, and no post-filter to leak through.
    #[allow(clippy::too_many_arguments)]
    pub fn backlinks(
        &self,
        tenant: &TenantId,
        region: &Region,
        target_root: &ArtifactRef,
        viewer: &Principal,
        list_objects: &ListObjectsResult,
        at: &Consistency,
        page: usize,
    ) -> Result<BacklinkPage, BacklinkError> {
        if page == 0 {
            return Err(BacklinkError::InvalidPage);
        }

        // ── ONE query: the candidate inbound range (tenant-first, live edges only, keyed on the §3.2 ──
        //    stored target_root). This is the `edge_inbound WHERE NOT tombstoned` scan — ONE scan,
        //    over which the lowered SetExpr (the permission filter) and pagination are conjoined. We
        //    bump the one-query counter exactly ONCE (the no-N+1 proof).
        self.query_count.fetch_add(1, Ordering::SeqCst);
        let candidates = self.edges.inbound_live(tenant, region, target_root);

        // ── The frozen list_objects shape → the SetExpr to lower over edge.source_root + the ──────────
        //    filter-mode split (1.8 telemetry). The SAME enum drives BOTH the lowered SQL form (the
        //    wire contract the integration test proves against the live table) AND the in-memory admit
        //    decision below — ONE source of truth, no second algebra.
        let (set_expr, mode) = match list_objects {
            // The materialised small-result path: the allow-set is an explicit `Ids` (inlined as IN).
            ListObjectsResult::Ids { ids, .. } => {
                self.ids_mode_reads.fetch_add(1, Ordering::SeqCst);
                (SetExpr::Ids(ids.clone()), FilterMode::Ids)
            }
            // The pushed-down large/unbounded path: the SetExpr (IN/NOT IN, the authz JOIN, the boolean
            // composition) — never materialised, never a per-row check.
            ListObjectsResult::Filter { set_expr, .. } => {
                self.pushed_down_reads.fetch_add(1, Ordering::SeqCst);
                (set_expr.clone(), FilterMode::PushedDown)
            }
        };

        // The lowered SQL form (the wire contract): produced so the conjoin is observable + the
        // integration test proves it against the live `edge` table. It also tells us whether the
        // lowering depends on the reverse index (the new-enemy guard applies iff it JOINs).
        let filter = lower_over_source_root(&set_expr, viewer);

        // ── The new-enemy guard (4.10 / REF-D6): if the lowering JOINs the reverse index and the ──────
        //    carried zookie pins a revision NEWER than the index watermark, fall back to per-source
        //    `check` rather than serving a stale grant. (A pure-`Ids` lowering is watermark-
        //    independent.) Either way the read is LEAK-FREE: the in-memory reverse index is already
        //    at-revision (the projecting grant/revoke advanced the watermark), so the admit decision
        //    below reads the FRESH set — `fell_back_to_check` is the observable BRANCH a drill asserts,
        //    not a correctness fork (the JOIN-serve path and the fall-back path admit the SAME
        //    leak-free set against a fresh index; the distinction is whether a STALE index would have
        //    been trusted — and it is NOT).
        let verdict = watermark_verdict(&self.authz, tenant, region, &filter, at);
        let fell_back_to_check = matches!(verdict, WatermarkVerdict::FallBackToCheck { .. });

        // ── Admit each candidate iff its source_root satisfies the SetExpr (this IS the conjoined ──────
        //    scan predicate — the lowered `WHERE`/`JOIN` — evaluated per candidate row exactly as the
        //    SQL planner would; it is NOT a post-filter of an already-permitted set, and it never calls
        //    Identity per row — the reverse-index forms read the already-materialised `authz_visible`
        //    set the JOIN compiled to). Then paginate (`LIMIT :page`).
        let admitted: Vec<Backlink> = candidates
            .iter()
            .filter(|row| {
                set_expr_admits(
                    &set_expr,
                    &self.authz,
                    viewer,
                    tenant,
                    region,
                    &row.source_root,
                )
            })
            .map(Backlink::from_row)
            .take(page)
            .collect();

        Ok(BacklinkPage {
            edges: admitted,
            mode,
            fell_back_to_check,
        })
    }

    /// **`edges(ref, viewer)` (contract 5.3 OWNED).** The same permission-filtered read keyed on the
    /// `ref`'s root — "the edges touching this artifact". For the inbound (backlink) direction it is
    /// `backlinks` with a default page; this is the convenience entry the contract names alongside
    /// `backlinks`. It returns the admitted inbound edges (the leak-critical direction — an outbound
    /// edge is the artifact's OWN content, not a permission-filtered read of OTHERS' references).
    #[allow(clippy::too_many_arguments)]
    pub fn edges(
        &self,
        tenant: &TenantId,
        region: &Region,
        ref_root: &ArtifactRef,
        viewer: &Principal,
        list_objects: &ListObjectsResult,
        at: &Consistency,
        page: usize,
    ) -> Result<BacklinkPage, BacklinkError> {
        self.backlinks(tenant, region, ref_root, viewer, list_objects, at, page)
    }

    /// The §4.4 `authz_visible` reverse index this read JOINs (the materialised `(subject, relation,
    /// object_id)` set Identity keeps fresh off the bus). Exposed so the caller that wires Refs into
    /// `serve` (and the CDC/drill tests) can project grants/revokes into it — the production wiring is
    /// the bus consumer that keeps it fresh; in-memory it is granted/revoked directly.
    pub fn authz_index(&self) -> &AuthzVisibleIndex {
        &self.authz
    }

    /// The §3.2 edge projection this read scans (the row source). Exposed so the caller that wires Refs
    /// into `serve` (and the CDC/drill tests) can seed/inspect the edge index.
    pub fn edge_projection(&self) -> &EdgeProjection {
        &self.edges
    }

    /// The live one-query telemetry sample (contract 1.8) — the count of SCANS issued. A drill reads
    /// this to assert EXACTLY ONE scan per backlink read (the no-N+1 proof: never one `check` per
    /// inbound edge).
    pub fn query_count(&self) -> u64 {
        self.query_count.load(Ordering::SeqCst)
    }

    /// The filter-mode-split telemetry sample (contract 1.8): `(ids_mode_reads, pushed_down_reads)` —
    /// how many reads took the materialised `Ids` path vs the pushed-down `Filter`/`TupleSet` path.
    pub fn filter_mode_split(&self) -> (u64, u64) {
        (
            self.ids_mode_reads.load(Ordering::SeqCst),
            self.pushed_down_reads.load(Ordering::SeqCst),
        )
    }
}

/// **Does the frozen `SetExpr` ADMIT this candidate `source_root`? (the conjoined scan predicate,
/// §4.4 — the in-memory model of the lowered SQL `WHERE`/`JOIN`).** This is the SAME enum the lowering
/// compiles to SQL ([`lower_over_source_root`]) — evaluated here per candidate row exactly as the SQL
/// planner would evaluate the conjoined predicate. It is **NOT a per-edge `check`**: the
/// reverse-index forms (`InRelation`/`TupleSet`) read the already-materialised `authz_visible` set
/// (the JOIN target), they never call Identity per row. It is **NOT a post-filter**: there is no
/// separate "permitted rows" fetch — this predicate IS the scan filter the candidate range is
/// admitted by.
///
/// The FROZEN forms (the leak-critical mapping — a mutation that flips any arm leaks or over-denies):
/// - `All` → admit (no predicate — admin sees all);
/// - `None` → deny (`WHERE false`);
/// - `Ids(v)` → admit iff `source_root ∈ v` (empty allow-set → deny);
/// - `NotIds(v)` → admit iff `source_root ∉ v` (empty deny-set → admit);
/// - `InRelation{relation, …}` / `TupleSet{index}` → admit iff the reverse index makes `source_root`
///   visible to `viewer` under that relation (the `authz_visible` JOIN);
/// - `Union` → ANY admits; `Intersect` → ALL admit; `Difference(a, b)` → `a` admits AND `b` does NOT.
pub fn set_expr_admits(
    set_expr: &SetExpr,
    authz: &AuthzVisibleIndex,
    viewer: &Principal,
    tenant: &TenantId,
    region: &Region,
    source_root: &ArtifactRef,
) -> bool {
    match set_expr {
        // Admin sees every source of this type in the tenant → admit (no predicate).
        SetExpr::All => true,
        // The deny set → `WHERE false` → never admit (leak-free; never a permissive default).
        SetExpr::None => false,
        // An explicit allow-set → admit iff the source_root is in it. An EMPTY allow-set admits NOTHING
        // (the `IN ()` → FALSE rule — never a permissive TRUE).
        SetExpr::Ids(ids) => ids.iter().any(|id| id.0 == source_root.0),
        // An explicit deny-set → admit iff the source_root is NOT in it. An EMPTY deny-set excludes
        // nothing → admit all.
        SetExpr::NotIds(ids) => !ids.iter().any(|id| id.0 == source_root.0),
        // The reverse-index JOIN: admit iff `authz_visible` makes the source_root visible to the viewer
        // under this relation (the materialised `(subject, relation, object_id)` set).
        SetExpr::InRelation { relation, .. } => authz.visible(
            tenant,
            region,
            &viewer.principal_id.0,
            &relation.0,
            &source_root.0,
        ),
        // The big-result materialised tuple set — the index ref names the relation it materialises.
        SetExpr::TupleSet { index } => authz.visible(
            tenant,
            region,
            &viewer.principal_id.0,
            &index.0,
            &source_root.0,
        ),
        // Boolean composition. An EMPTY Union admits nothing (sees nothing); an EMPTY Intersect admits
        // all (no restriction) — the identity elements, never a leak.
        SetExpr::Union(parts) => parts
            .iter()
            .any(|p| set_expr_admits(p, authz, viewer, tenant, region, source_root)),
        SetExpr::Intersect(parts) => parts
            .iter()
            .all(|p| set_expr_admits(p, authz, viewer, tenant, region, source_root)),
        // `Difference(a, b)` = a EXCEPT b → admit iff `a` admits AND `b` does NOT.
        SetExpr::Difference(a, b) => {
            set_expr_admits(a, authz, viewer, tenant, region, source_root)
                && !set_expr_admits(b, authz, viewer, tenant, region, source_root)
        }
    }
}

/// The `view` [`Permission`] Refs pre-filters with (the named constant as a typed `Permission`). A
/// constructor so the FROZEN permission is built in one place.
pub fn view_permission() -> Permission {
    Permission(VIEW_PERMISSION.into())
}

/// Build a frozen `Ids{}` `list_objects` result (the materialised small-result path) for a set of
/// visible source roots + a zookie — a convenience the synthetic-Identity test path uses to drive the
/// `Ids`-mode read.
pub fn ids_result(ids: &[&str], zookie: &str) -> ListObjectsResult {
    ListObjectsResult::Ids {
        ids: ids.iter().map(|s| ObjectId((*s).into())).collect(),
        zookie: Zookie(zookie.into()),
    }
}

#[cfg(test)]
mod tests;

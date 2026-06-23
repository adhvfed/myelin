//! # `planner` — the Issues AST→OLTP-store compiler: the `list_objects` `SetExpr` push-down
//! lowered FIRST (leak-free, no N+1, no post-filter) — ISS-P13 / P-379; the ISS-D3 gate.
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md`
//! §3 (*The query planner — AST→store compiler + `SetExpr` push-down*): the planner's FIRST job is
//! **lowering the frozen `list_objects` `SetExpr`** into a SQL predicate over `issue.id` — the
//! no-leak, no-N+1, no-post-filter pre-filter (contract 4.3, OQ-E; Δ1). The `authz_visible` JOIN
//! target is the per-tenant, residency-pinned authz reverse index Identity maintains off the bus
//! (the SpiceDB/Zanzibar `LookupResources` reverse index as a co-located JOIN target). The returned
//! `zookie` bounds staleness; a security-sensitive scan passes the zookie so the read reads the
//! tuple index at-or-after the zookie's revision (read-your-writes for a just-revoked grant). A
//! **confidential issue is absent by construction** — the ReBAC `- confidential` set-difference
//! userset lowers to `(visible AND NOT confidential)`; never a post-filter, never an "N hidden"
//! count leak.
//!
//! **Reconciliation:** `00-reconciliation-decisions.md §OQ-E` (the `SetExpr` push-down — lowered to
//! a SQL predicate / JOIN over the per-tenant authz reverse index; the §7.3 per-consumer `via_column`
//! table — Issues is `issue` over `issue.id`).
//!
//! **Contract-index:** row **4.3** (`list_objects → Ids | Filter{set_expr, zookie}` — the SetExpr
//! lowered to a SQL JOIN over the consumer's own id column via the per-tenant authz reverse index; no
//! N+1, no post-filter) — **CONSUMED** (Issues is THE headline consumer — the board/backlog scan is
//! the single most load-bearing inter-system contract); row **4.10** (zookie — the new-enemy guard;
//! a security-sensitive scan reads at-or-after the zookie's revision) — **CONSUMED**; row **13.3**
//! (the `QueryAst`/`ViewSpec` the compiler lowers ON TOP of the leak-free pre-filter) — **CONSUMED**
//! (co-owned in `query_coown`).
//!
//! ## What this module ships (ISS-P13 — the LOWERING half)
//!
//! Issues OWNS the compiler from the shared `myelin-query` AST to its OLTP store (the ADR-06 line:
//! *"share the schema language and the view model, not the query planner"*). The first job — and the
//! whole of this prompt — is the **leak-free pre-filter**:
//!
//! 1. [`lower_over_issue_id`] — lower the returned `SetExpr` over Issues' OWN id column (`issue.id`,
//!    the §3 / §7.3 mapping) into `(sql_predicate, joins, params)`. Mirrors the Identity-side
//!    `lowering::lower` discipline (bound params never interpolated literals; ONE JOIN per distinct
//!    `(viewer, relation)` — no N+1) restated here because `myelin-issues` is a LEAF CONSUMER that
//!    cannot depend on the Identity SERVICE crate (the §2.9 acyclic DAG). The SHAPE is the wire
//!    contract.
//! 2. [`compose_board_query`] — the board/backlog LIST query: ONE SQL statement that conjoins the
//!    lowered `Filter` predicate + its `authz_visible` JOINs into the board scan's `FROM`/`WHERE`
//!    over the verified `(tenant, region)` partition, the ACL pre-filter conjoined BEFORE the
//!    `ORDER BY rank` / `LIMIT` (never a post-filter). No per-row `check` loop.
//! 3. [`AuthzVisibleIndex`] — an in-memory model of the per-tenant residency-pinned `authz_visible`
//!    reverse index + its revision watermark, so the unit + drill tests drive the JOIN semantics +
//!    the new-enemy guard with zero DB. The REAL `authz_visible` table is JOINed in SQL; the live
//!    proof (one query, 0 leak, tenant-scoped, zookie-bounded) is the `--features integration` test
//!    against the dev-stack Postgres (`tests/integration_iss_p13_setexpr_pushdown.rs`).
//!
//! ## Leak-free is by construction (EI-01 §3 stop-the-bleeding; the ISS-D3 highest-stakes property)
//! A denied / confidential issue NEVER survives the `WHERE` — the ACL predicate is conjoined BEFORE
//! any sort / pagination, never a post-filter over a wider materialised set. The `None` set lowers to
//! `FALSE` (`WHERE false`), an empty `Ids` allow-set lowers to `FALSE` (never a permissive `TRUE`), a
//! cross-tenant row is excluded by the tenant predicate the composer always emits, and the
//! **confidential set-difference** (`view = (parent_project->read − confidential) + confidential_grant`,
//! `rebac_fragment`) lowers to `(... AND NOT <confidential>)` so a confidential issue is **absent**
//! — never counted, never an "N hidden" leak (the F1 leak-free family, ISS-D3).
//!
//! ## The new-enemy guard (4.10 read-your-writes; §3 zookie)
//! A security-sensitive scan passes the post-write zookie so the read reflects a just-revoked grant.
//! [`AuthzVisibleIndex::serves`] compares the scan's required revision against the per-tenant
//! watermark: at-or-after → the JOIN serves; behind → the caller falls back to per-row `check` rather
//! than serving the stale grant. This is the SAME semantics the Identity-side
//! `lowering::watermark_verdict` enforces (restated for the consumer's own decision point).
//!
//! ## FLOOR named (per the prompt — DELIVERABLE: "none new")
//! The `SetExpr` lowering is THE leak seam (mandatory-core, the highest mutation bar). This prompt
//! opens NO new floor:
//! - **Cost-bounding + the three-tier escalation** (Tier-1 typed core / Tier-2 generated facet index
//!   / Tier-3 Search escalation, the <1s flexible-field latency budget) is the named follow-on
//!   **ISS-P14** (P-380) — every tier conjoins THIS same `SetExpr` Filter.
//! - The **projection feeder** (the measured-promotion generated index) is **ISS-P15**.
//! - The co-equal `ViewSpec` views (board/roadmap/backlog/table/calendar/cycle) are **ISS-P16**,
//!   each ALWAYS conjoining this leak-free path.

use myelin_identity::{ColRef, ObjectId, Principal, SetExpr, Zookie};
use myelin_tenancy::{Region, TenantId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ───────────────────────────── frozen names (§3 / §7.3 — never a stray literal) ───────────────────

/// The frozen `view` permission Issues pre-filters the board/backlog with (§3: `list_objects(viewer,
/// view, issue)`). The board is "the issues the viewer may `view`" — the `view` rewrite is the
/// confidential set-difference (`rebac_fragment`). A named constant — drills assert against the NAME,
/// never a literal (EI-01 §3).
pub const ISSUE_VIEW_PERMISSION: &str = "view";

/// Issues' OWN issue id column the board `SetExpr` lowers over (§3 / §7.3: `issue.id`). The FROZEN
/// `(table, column)` pair, named in ONE place.
pub fn issue_id_colref() -> ColRef {
    ColRef {
        table: "issue".into(),
        column: "id".into(),
    }
}

/// The per-tenant, residency-pinned authz reverse-index table the `InRelation`/`TupleSet` forms JOIN
/// against (§3 / OQ-E: Identity's materialised `(subject, relation, object_id)` projection kept fresh
/// off the bus — the Zanzibar/Leopard `LookupResources` reverse index realised as a co-located JOIN
/// target). A named constant so the lowered JOIN names the FROZEN table.
pub const AUTHZ_VISIBLE_TABLE: &str = "authz_visible";

/// Which frozen `list_objects` shape drove a board read — the filter-mode-split telemetry. `Ids` is
/// the materialised small-result path (the allow-set inlined as `IN`); `PushedDown` is the
/// large/unbounded path (the `Filter{set_expr}` lowered + JOINed, never materialised).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterMode {
    /// `list_objects` returned `Ids{}` (the small result set, materialised + inlined as `IN`).
    Ids,
    /// `list_objects` returned `Filter{set_expr}` whose lowering JOINed the reverse index (the
    /// large/unbounded pushed-down path — no per-row `check`).
    PushedDown,
}

// ───────────────────────────── the lowering (§3 — bound, never interpolated) ──────────────────────

/// One bound parameter the lowered predicate carries (never a string-interpolated literal — an id /
/// subject / relation an attacker controls can NEVER become SQL; the consumer binds these). Mirrors
/// the Identity-side `BoundParam` shape (the SAME bound-not-interpolated discipline, §7.2) — restated
/// here because `myelin-issues` (a LEAF consumer) cannot depend on the Identity SERVICE crate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundParam {
    /// The named placeholder in the SQL (`:id_0`, `:subject_0`, `:rel_for_view`).
    pub placeholder: String,
    /// The bound value (an object id / the viewer subject id / a relation name) — bound, never
    /// interpolated.
    pub value: String,
}

/// One JOIN the lowered predicate requires against the `authz_visible` reverse index (§3). The board
/// scan adds this to its `FROM`; the predicate references the alias. Deduplicated by `(viewer,
/// relation)` so the SAME reverse-index JOIN is emitted ONCE — the no-N+1 guarantee: an
/// `InRelation`/`TupleSet`, however deeply nested in a boolean tree, contributes at most one JOIN per
/// distinct `(viewer, relation)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthzJoin {
    /// The alias for this `authz_visible` JOIN (`av0`, `av1`, …) the predicate references.
    pub alias: String,
    /// The relation this JOIN keys on (`view`/`confidential`/…) — carried so the in-memory evaluator
    /// (and the dedup) reads it without re-parsing the clause.
    pub relation: String,
    /// The full JOIN clause: `JOIN authz_visible <alias> ON <alias>.object_id = issue.id AND
    /// <alias>.subject = :<subject> AND <alias>.relation = :<relation>`. The scan's own query planner
    /// does the conjoin — one query, no N+1, no post-filter.
    pub clause: String,
}

/// **The lowering result the Issues board scan conjoins (§3) — `(sql_predicate, joins, params)`.**
/// The scan does: `SELECT … FROM issue <joins> WHERE tenant_id = :t AND region = :r AND
/// (<sql_predicate>) ORDER BY issue.rank LIMIT :page` binding `params`. This is **one query** — the
/// conjoin is the scan's query planner's job, NOT a per-row `check` loop. Leak-free: an issue the
/// viewer cannot see (or a confidential issue) never survives the `WHERE`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoweredFilter {
    /// The boolean SQL predicate over `issue.id` (ANDed into the board `WHERE`).
    pub sql_predicate: String,
    /// The deduplicated `authz_visible` JOINs the scan adds to its `FROM` (one per distinct
    /// `(viewer, relation)` — the no-N+1 guarantee).
    pub joins: Vec<AuthzJoin>,
    /// The bound parameters (object ids, the viewer subject, relation names) — bound, never
    /// interpolated.
    pub params: Vec<BoundParam>,
}

impl LoweredFilter {
    /// `true` iff the predicate references at least one `authz_visible` JOIN — i.e. the lowering hit
    /// an `InRelation`/`TupleSet` and therefore depends on the reverse index's revision watermark
    /// (the new-enemy guard applies). A purely `Ids`/`NotIds`/`All`/`None` lowering is
    /// watermark-independent (it carries its own materialised set).
    pub fn depends_on_reverse_index(&self) -> bool {
        !self.joins.is_empty()
    }

    /// The filter-mode this lowering represents: a JOIN-bearing lowering is `PushedDown`; a
    /// materialised `Ids`/`NotIds`/`All`/`None` lowering is `Ids`.
    pub fn filter_mode(&self) -> FilterMode {
        if self.depends_on_reverse_index() {
            FilterMode::PushedDown
        } else {
            FilterMode::Ids
        }
    }
}

/// Internal accumulator threaded through the recursive lowering so JOINs + params are collected once
/// (the no-N+1 dedup lives here: a `(viewer, relation)` JOIN already emitted is reused by alias).
struct LowerCtx<'a> {
    /// The viewer the whole `Filter` is for (the `av.subject = :subject` binding — one viewer per
    /// `list_objects` call; the reverse-index JOIN keys on it).
    subject: &'a str,
    /// Issues' own id column (`issue.id`) every JOIN/`IN` references.
    via_sql: String,
    joins: Vec<AuthzJoin>,
    params: Vec<BoundParam>,
    /// A monotonically-increasing counter for unique placeholders.
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
    /// parameter set — never an interpolated literal (injection-safe).
    fn bind(&mut self, prefix: &str, value: &str) -> String {
        let placeholder = format!(":{}_{}", prefix, self.next_id);
        self.next_id += 1;
        self.params.push(BoundParam {
            placeholder: placeholder.clone(),
            value: value.to_string(),
        });
        placeholder
    }

    /// Emit (or reuse) the `authz_visible` JOIN for a `(viewer, relation)` — the §3 reverse-index
    /// JOIN keyed on `issue.id`. Deduplicated by relation (the viewer is constant for the whole
    /// call): a relation already JOINed reuses its alias, so the SAME JOIN is never emitted twice
    /// (the no-N+1 guarantee — at most one JOIN per distinct `(viewer, relation)`, however nested).
    /// Returns the boolean predicate fragment `<alias>.object_id IS NOT NULL`.
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

/// **Lower a `SetExpr` to the consumer-composable SQL `Filter` over Issues' own id column `issue.id`
/// (§3; the FROZEN encoding).** `viewer` is the principal the `list_objects` is for (the `av.subject`
/// binding). Returns the [`LoweredFilter`] `(sql_predicate, joins, params)` the board scan ANDs into
/// its query — **one query, no N+1, no post-filter**.
///
/// The FROZEN forms (§3 / §7.2):
/// - `All` → `TRUE` (no predicate — admin sees all);
/// - `None` → `FALSE` (`WHERE false` — deny, never a permissive default);
/// - `Ids(v)` → `issue.id IN (:p0, …)` (inlined under the cardinality cap; empty → `FALSE`);
/// - `NotIds(v)` → `issue.id NOT IN (…)` (empty → `TRUE`);
/// - `InRelation{relation, …}` / `TupleSet{index}` → the `authz_visible` JOIN keyed on `issue.id`;
/// - `Union`/`Intersect`/`Difference` → `(a OR b)` / `(a AND b)` / `(a AND NOT b)` — the
///   confidential set-difference `view = (read − confidential) + grant` lowers through this last form,
///   so a confidential issue is ABSENT by construction (never a post-filter, never a count leak).
pub fn lower_over_issue_id(set_expr: &SetExpr, viewer: &Principal) -> LoweredFilter {
    let via = issue_id_colref();
    let mut ctx = LowerCtx::new(&viewer.principal_id.0, &via);
    let sql_predicate = lower_expr(set_expr, &mut ctx);
    LoweredFilter {
        sql_predicate,
        joins: ctx.joins,
        params: ctx.params,
    }
}

/// The recursive lowering of one `SetExpr` node into a boolean SQL fragment (collecting JOINs +
/// params into `ctx`). Every leaf is a predicate over `issue.id` or a reverse-index JOIN; the boolean
/// nodes compose with `OR`/`AND`/`AND NOT` — no per-row subquery, no post-filter.
fn lower_expr(expr: &SetExpr, ctx: &mut LowerCtx<'_>) -> String {
    match expr {
        // The viewer sees every issue of this type in the tenant (e.g. admin) → no restriction.
        SetExpr::All => "TRUE".to_string(),
        // The deny set — `WHERE false`, never a permissive default (leak-free).
        SetExpr::None => "FALSE".to_string(),
        // An explicit allow-set inlined under the cardinality cap → `issue.id IN (:p0, …)`. An empty
        // allow-set is `FALSE` (`IN ()` is not valid SQL and means "no rows" — never a permissive
        // TRUE; the leak-free identity element).
        SetExpr::Ids(ids) => {
            if ids.is_empty() {
                return "FALSE".to_string();
            }
            let placeholders: Vec<String> = ids.iter().map(|id| ctx.bind("id", &id.0)).collect();
            format!("{} IN ({})", ctx.via_sql, placeholders.join(", "))
        }
        // An explicit deny-set over an otherwise-visible space → `issue.id NOT IN (…)`. An empty
        // deny-set excludes nothing → `TRUE`.
        SetExpr::NotIds(ids) => {
            if ids.is_empty() {
                return "TRUE".to_string();
            }
            let placeholders: Vec<String> = ids.iter().map(|id| ctx.bind("id", &id.0)).collect();
            format!("{} NOT IN ({})", ctx.via_sql, placeholders.join(", "))
        }
        // The reverse-index JOIN keyed on `issue.id` (§3) — the SpiceDB/Leopard LookupResources
        // pattern as a co-located JOIN target. One JOIN per distinct relation (deduplicated — no N+1).
        SetExpr::InRelation { relation, .. } => ctx.authz_join_predicate(&relation.0),
        // A server-materialised tuple set the scan JOINs against (the big-result path). The index
        // ref names the relation it materialises; the JOIN is the same `authz_visible` target.
        SetExpr::TupleSet { index } => ctx.authz_join_predicate(&index.0),
        // Boolean composition → `(a OR b)` / `(a AND b)` / `(a AND NOT b)` (§3 — Union/Intersect/
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
        // inside one `WHERE`, not a set-difference subquery — still one query, no N+1). This is the
        // CONFIDENTIAL crux: `view = (parent_project->read − confidential) + confidential_grant`
        // lowers a confidential issue OUT of the result set by construction — ABSENT, never an "N
        // hidden" count leak (ISS-D3).
        SetExpr::Difference(a, b) => {
            let af = lower_expr(a, ctx);
            let bf = lower_expr(b, ctx);
            format!("({af} AND NOT {bf})")
        }
    }
}

// ───────────────────────────── the board-list composer (one query) ───────────────────────────────

/// **A composed, leak-free board/backlog query (the §3 push-down conjoined into ONE statement).** The
/// `sql` is a single `SELECT issue.id FROM issue <joins> WHERE issue.tenant_id = :tenant AND
/// issue.region = :region AND (<acl_predicate>) ORDER BY issue.rank LIMIT :page` — the ACL pre-filter
/// is conjoined BEFORE the `ORDER BY rank` / pagination (never a post-filter), with the tenant
/// predicate isolating cross-tenant rows. The `params` are bound (the tenant/region + the lowered
/// filter's ids/subject/relation), never interpolated. This is **one query** — verified by
/// [`ComposedBoardQuery::statement_count`] (always 1; the conjoin is the planner's job, not an N+1
/// loop).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposedBoardQuery {
    /// The single SQL statement (no trailing `;` — one statement, exactly).
    pub sql: String,
    /// The bound parameters (tenant, region, the lowered filter's ids/subject/relation) — bound,
    /// never interpolated.
    pub params: Vec<BoundParam>,
    /// Which `list_objects` shape drove this read (the filter-mode split telemetry).
    pub filter_mode: FilterMode,
}

impl ComposedBoardQuery {
    /// The number of SQL statements this read issues — ALWAYS 1 (the §3 no-N+1 guarantee: the conjoin
    /// is a JOIN + `WHERE` the query planner resolves, never a per-row `check` loop). A drill asserts
    /// this is `1`.
    pub fn statement_count(&self) -> usize {
        // One statement by construction — there is no `;`-separated second query and no per-row loop.
        // (A defensive split confirms the SQL is a single statement.)
        self.sql.split(';').filter(|s| !s.trim().is_empty()).count()
    }
}

/// **Compose the board/backlog query (§3): the issues the `viewer` may `view`, leak-free, in ONE
/// query.** Given the `set_expr` Identity returned for `list_objects(viewer, view, issue)`, lower it
/// over `issue.id` and conjoin it into the board scan over the verified `(tenant, region)` partition,
/// the ACL pre-filter conjoined BEFORE `ORDER BY issue.rank` (the typed-core board ordering, §5). The
/// page bound is bound, not interpolated. A confidential issue is ABSENT (the set-difference, §3).
pub fn compose_board_query(
    set_expr: &SetExpr,
    viewer: &Principal,
    scope_tenant: &TenantId,
    scope_region: &Region,
) -> ComposedBoardQuery {
    let lowered = lower_over_issue_id(set_expr, viewer);
    let filter_mode = lowered.filter_mode();
    let joins: String = lowered
        .joins
        .iter()
        .map(|j| format!(" {}", j.clause))
        .collect();
    // The tenant predicate is ALWAYS emitted (a tenant-less board is unconstructable — the
    // `tenant-predicate` lint; EI-02 §1 no cross-tenant query path). The ACL predicate is conjoined
    // BEFORE the ORDER BY / LIMIT — pre-filter, never post-filter (ADR-03 / SC-1). The board orders by
    // the typed-core `rank` (the frozen order_key, §5).
    let sql = format!(
        "SELECT issue.id FROM issue{joins} \
         WHERE issue.tenant_id = :tenant AND issue.region = :region \
         AND ({acl}) ORDER BY issue.rank LIMIT :page",
        acl = lowered.sql_predicate,
    );
    let mut params = vec![
        BoundParam {
            placeholder: ":tenant".into(),
            value: scope_tenant.0.clone(),
        },
        BoundParam {
            placeholder: ":region".into(),
            value: scope_region.0.clone(),
        },
    ];
    params.extend(lowered.params);
    ComposedBoardQuery {
        sql,
        params,
        filter_mode,
    }
}

// ───────────────────────────── the in-memory authz_visible model (tests) ─────────────────────────

/// **The per-tenant, residency-pinned `authz_visible` reverse index (§3 / OQ-E) — modelled in-memory
/// for the unit + drill tests.** The materialised `(subject, relation, object_id)` projection of the
/// ReBAC tuples Identity maintains, kept fresh off the bus, carrying a per-tenant **revision
/// watermark** (4.10). The board read JOINs against THIS for the `InRelation`/`TupleSet` forms; a read
/// carrying a zookie at-or-after a revoke's revision must NOT see the revoked issue (the new-enemy
/// guard — never serve a stale grant from a behind index). Per-tenant: the key is `(tenant, region,
/// subject, relation)` — **no cross-tenant query path** (EI-02 §1). The REAL `authz_visible` table
/// (Identity-maintained, JOINed in SQL) replaces this in the `--features integration` proof; the
/// in-memory model is the SAME JOIN semantics + watermark.
#[derive(Clone, Default)]
pub struct AuthzVisibleIndex {
    watermark: Arc<Mutex<WatermarkMap>>,
    visible: Arc<Mutex<VisibleMap>>,
}

/// `(tenant, region)` → the per-tenant revision watermark (4.10).
type WatermarkMap = HashMap<(String, String), String>;
/// `(tenant, region, subject, relation)` → the visible `object_id` set (the reverse index).
type VisibleMap = HashMap<(String, String, String, String), Vec<String>>;

impl AuthzVisibleIndex {
    /// A fresh, empty reverse index.
    pub fn new() -> AuthzVisibleIndex {
        AuthzVisibleIndex::default()
    }

    /// Grant `subject` the `relation` tuple over `object_id` in `(tenant, region)` and advance the
    /// watermark to `at_revision` (the kept-fresh-off-the-bus projection of a `write_tuples`).
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
        let mut v = self.visible.lock().unwrap();
        let set = v.entry(key).or_default();
        if !set.iter().any(|o| o == object_id) {
            set.push(object_id.into());
        }
        drop(v);
        self.advance_watermark(tenant, region, at_revision);
    }

    /// Revoke `subject`'s `relation` tuple over `object_id` and advance the watermark to
    /// `at_revision` (the projection of a revoke — the new-enemy case: a read carrying a zookie
    /// at-or-after this revision must NOT see `object_id`).
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
        // `watermark_is_monotone_stale_never_regresses`. The `> → >=` mutant here is an EQUIVALENT
        // mutant (when `revision == *cur`, both branches assign the SAME value — no observable
        // difference), so it is correctly not caught; the monotonicity that MATTERS (a stale/older
        // revision never regresses) IS asserted. Mirrors the documented equivalent-mutant in
        // `myelin_git::list_filter::AuthzVisibleIndex::advance_watermark`.
        if revision > cur.as_str() {
            *cur = revision.into();
        }
    }

    /// The current per-tenant revision watermark (`""` if the index has never been advanced).
    pub fn watermark(&self, tenant: &TenantId, region: &Region) -> Zookie {
        let key = (tenant.0.clone(), region.0.clone());
        Zookie(
            self.watermark
                .lock()
                .unwrap()
                .get(&key)
                .cloned()
                .unwrap_or_default(),
        )
    }

    /// **The new-enemy guard (§3 zookie; 4.10 read-your-writes).** Whether the JOIN may serve a scan
    /// that requires revision `required` (the post-write zookie a security-sensitive read passes):
    /// `true` iff the per-tenant watermark is at-or-after `required` (the index reflects the writes the
    /// scan must see). A `required` of `""` (default-consistency, no freshness floor) always serves.
    /// When this is `false` the caller falls back to per-row `check` rather than serving a stale grant.
    pub fn serves(&self, tenant: &TenantId, region: &Region, required: &Zookie) -> bool {
        if required.0.is_empty() {
            return true;
        }
        self.watermark(tenant, region).0 >= required.0
    }

    /// **Evaluate a [`LoweredFilter`] against this in-memory index for the unit/drill: the set of
    /// `candidate` issue ids that survive the JOIN + predicate (the SAME row set the SQL `WHERE`/JOIN
    /// would keep).** Leak-free: a candidate the viewer has no `relation` tuple for (and no inline
    /// `IN`-allow), or one that is `confidential` without a grant, never survives. This models the SQL
    /// the live `--features integration` test proves; it is NOT a per-row `check` (it reads the
    /// already-materialised reverse index, exactly as the JOIN does).
    pub fn evaluate(
        &self,
        tenant: &TenantId,
        region: &Region,
        viewer: &Principal,
        lowered: &LoweredFilter,
        candidates: &[ObjectId],
    ) -> Vec<ObjectId> {
        candidates
            .iter()
            .filter(|c| self.row_survives(tenant, region, viewer, lowered, &c.0))
            .cloned()
            .collect()
    }

    /// Whether one candidate id survives the lowered predicate (the boolean SQL evaluated against the
    /// reverse index for the JOIN forms + the inline `IN` sets for the `Ids`/`NotIds` forms).
    fn row_survives(
        &self,
        tenant: &TenantId,
        region: &Region,
        viewer: &Principal,
        lowered: &LoweredFilter,
        candidate: &str,
    ) -> bool {
        eval_predicate(&lowered.sql_predicate, &mut |frag| {
            self.frag_holds(tenant, region, viewer, lowered, frag, candidate)
        })
    }

    /// Evaluate one LEAF predicate fragment (`TRUE`/`FALSE`/`issue.id IN (…)`/`issue.id NOT IN (…)`/
    /// `avN.object_id IS NOT NULL`) against the reverse index / the bound `IN` sets for `candidate`.
    fn frag_holds(
        &self,
        tenant: &TenantId,
        region: &Region,
        viewer: &Principal,
        lowered: &LoweredFilter,
        frag: &str,
        candidate: &str,
    ) -> bool {
        let f = frag.trim();
        if f == "TRUE" {
            return true;
        }
        if f == "FALSE" {
            return false;
        }
        // `avN.object_id IS NOT NULL` — the reverse-index JOIN for the alias's relation: the viewer
        // has `relation` over `candidate` in the reverse index.
        if let Some(alias) = f.strip_suffix(".object_id IS NOT NULL") {
            let relation = lowered
                .joins
                .iter()
                .find(|j| j.alias == alias)
                .map(|j| j.relation.as_str())
                .unwrap_or("");
            let key = (
                tenant.0.clone(),
                region.0.clone(),
                viewer.principal_id.0.clone(),
                relation.to_string(),
            );
            return self
                .visible
                .lock()
                .unwrap()
                .get(&key)
                .map(|set| set.iter().any(|o| o == candidate))
                .unwrap_or(false);
        }
        // `issue.id IN (…)` / `issue.id NOT IN (…)` — the inline bound allow/deny set: resolve the
        // placeholders to their bound values and test membership.
        if let Some(rest) = f.split_once(" NOT IN (") {
            let in_set = self.bound_in_set(lowered, rest.1);
            return !in_set.iter().any(|v| v == candidate);
        }
        if let Some(rest) = f.split_once(" IN (") {
            let in_set = self.bound_in_set(lowered, rest.1);
            return in_set.iter().any(|v| v == candidate);
        }
        // An unrecognised leaf is treated as a deny (fail-closed — never a permissive default).
        false
    }

    /// Resolve the placeholders inside an `IN (…)` fragment to their bound values (the bound-not-
    /// interpolated discipline means the literal ids live in `params`, not the SQL).
    fn bound_in_set(&self, lowered: &LoweredFilter, in_body: &str) -> Vec<String> {
        let body = in_body.trim_end_matches(')');
        body.split(',')
            .map(|p| p.trim())
            .filter_map(|ph| {
                lowered
                    .params
                    .iter()
                    .find(|p| p.placeholder == ph)
                    .map(|p| p.value.clone())
            })
            .collect()
    }
}

/// A tiny boolean-expression evaluator for the lowered predicate grammar (`TRUE`/`FALSE`, leaf
/// fragments, `AND`/`OR`/`NOT`, parentheses) — enough to evaluate the [`lower_expr`] output against
/// one candidate row (the in-memory model of the SQL `WHERE`). `leaf(frag)` evaluates a single LEAF
/// fragment. This is test/model machinery, not a general SQL engine; the production path is the
/// database evaluating the same predicate (proven in the `--features integration` test).
fn eval_predicate(pred: &str, leaf: &mut dyn FnMut(&str) -> bool) -> bool {
    let tokens = tokenize(pred);
    let mut pos = 0;
    let v = parse_or(&tokens, &mut pos, leaf);
    debug_assert_eq!(pos, tokens.len(), "the predicate parsed fully: {pred}");
    v
}

/// Tokenize the lowered predicate into `(`, `)`, `AND NOT`, `AND`, `OR`, `NOT`, and LEAF fragments.
/// A leaf runs up to the next boolean keyword / structural paren; a leaf's own `IN (…)` parens are
/// kept as part of the leaf (only TOP-LEVEL parens are structural — `depth_in_leaf` tracks the leaf's
/// own `IN (` nesting). The boolean keywords are space-delimited at the top level.
fn tokenize(pred: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut depth_in_leaf = 0usize;
    let mut i = 0;
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        let t = cur.trim();
        if !t.is_empty() {
            out.push(t.to_string());
        }
        cur.clear();
    };
    let chars: Vec<char> = pred.chars().collect();
    while i < chars.len() {
        let rest: String = chars[i..].iter().collect();
        // A leaf's `IN (` opens the leaf's own paren nesting (kept in the leaf, not structural).
        if rest.starts_with("IN (") {
            cur.push_str("IN (");
            i += 4;
            depth_in_leaf += 1;
            continue;
        }
        // Top-level boolean keywords (space-delimited) split leaves. Longest match first.
        if depth_in_leaf == 0 {
            if rest.starts_with(" AND NOT ") {
                flush(&mut cur, &mut out);
                out.push("AND NOT".into());
                i += " AND NOT ".chars().count();
                continue;
            }
            if rest.starts_with(" AND ") {
                flush(&mut cur, &mut out);
                out.push("AND".into());
                i += " AND ".chars().count();
                continue;
            }
            if rest.starts_with(" OR ") {
                flush(&mut cur, &mut out);
                out.push("OR".into());
                i += " OR ".chars().count();
                continue;
            }
            // A leading `NOT ` (the `(TRUE AND NOT …)` lowering emits `AND NOT`, but a defensive
            // standalone `NOT ` is still handled).
            if rest.starts_with("NOT ") && cur.trim().is_empty() {
                out.push("NOT".into());
                i += 4;
                continue;
            }
        }
        let c = chars[i];
        if c == '(' && depth_in_leaf == 0 && cur.trim().is_empty() {
            out.push("(".into());
            i += 1;
            continue;
        }
        if c == ')' {
            if depth_in_leaf > 0 {
                depth_in_leaf -= 1;
                cur.push(')');
                i += 1;
                continue;
            }
            flush(&mut cur, &mut out);
            out.push(")".into());
            i += 1;
            continue;
        }
        cur.push(c);
        i += 1;
    }
    flush(&mut cur, &mut out);
    out
}

fn parse_or(tokens: &[String], pos: &mut usize, leaf: &mut dyn FnMut(&str) -> bool) -> bool {
    let mut v = parse_and(tokens, pos, leaf);
    while *pos < tokens.len() && tokens[*pos] == "OR" {
        *pos += 1;
        let r = parse_and(tokens, pos, leaf);
        v = v || r;
    }
    v
}

fn parse_and(tokens: &[String], pos: &mut usize, leaf: &mut dyn FnMut(&str) -> bool) -> bool {
    let mut v = parse_unary(tokens, pos, leaf);
    while *pos < tokens.len() && (tokens[*pos] == "AND" || tokens[*pos] == "AND NOT") {
        let negate = tokens[*pos] == "AND NOT";
        *pos += 1;
        let mut r = parse_unary(tokens, pos, leaf);
        if negate {
            r = !r;
        }
        v = v && r;
    }
    v
}

fn parse_unary(tokens: &[String], pos: &mut usize, leaf: &mut dyn FnMut(&str) -> bool) -> bool {
    if *pos < tokens.len() && tokens[*pos] == "NOT" {
        *pos += 1;
        return !parse_unary(tokens, pos, leaf);
    }
    parse_primary(tokens, pos, leaf)
}

fn parse_primary(tokens: &[String], pos: &mut usize, leaf: &mut dyn FnMut(&str) -> bool) -> bool {
    if *pos >= tokens.len() {
        return false;
    }
    if tokens[*pos] == "(" {
        *pos += 1;
        let v = parse_or(tokens, pos, leaf);
        if *pos < tokens.len() && tokens[*pos] == ")" {
            *pos += 1;
        }
        return v;
    }
    let frag = tokens[*pos].clone();
    *pos += 1;
    leaf(&frag)
}

#[cfg(test)]
mod tests;

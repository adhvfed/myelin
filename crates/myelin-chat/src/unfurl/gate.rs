//! # `unfurl::gate` — the per-viewer `list_objects` SetExpr → JOIN lowering (contract 4.3, OQ-E)
//!
//! The membership-as-permission class precompute (§4.3): the frozen `list_objects` `SetExpr` (contract
//! 4.3) lowered to a SQL predicate / JOIN over the **unfurl candidate id column** against Identity's
//! per-tenant `authz_visible` reverse index — **one query, no N+1, no post-filter**. The SAME FROZEN
//! lowering every permission-aware consumer runs (`myelin_git::list_filter`,
//! `myelin_knowledge::list_filter`, `myelin_ci_controlplane::surfacing`, `myelin_refs_service::backlinks`)
//! — each over its OWN id column (the architecture §7.3 mapping: a consumer lowers over its own table).
//! Chat is the unfurl-candidate consumer; its column is `unfurl_candidate.object_id`.
//!
//! Chat is a CONSUMER subsystem (it depends on the names-only `myelin-identity`, never the engine
//! crate), so — exactly like `myelin-git` — the `BoundParam`/`AuthzJoin`/`LoweredFilter` shapes are
//! restated here (the SAME bound-not-interpolated discipline, §7.2): a leaf consumer cannot depend on
//! the Identity SERVICE crate's internal lowering. This is NOT a "third copy" abstraction violation —
//! the architecture (§4.3 / §7.3) FREEZES that each consumer lowers the `SetExpr` over its OWN id
//! column; the shared, frozen thing is the `SetExpr` algebra (owned by `myelin-identity`), which chat
//! consumes verbatim.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use myelin_identity::{ColRef, ObjectId, Principal, SetExpr, Zookie};
use myelin_tenancy::{Region, TenantId};

/// The unfurl-candidate ACL column the `SetExpr` lowers over — the candidate ref's authz object id
/// (the channel/artifact the viewer must hold `read` on). The scan JOINs `authz_visible` ON
/// `av.object_id = unfurl_candidate.object_id`. The §7.3 mapping names the consumer's own id column;
/// chat's unfurl candidate scan is `unfurl_candidate` over `object_id`.
pub fn unfurl_candidate_colref() -> ColRef {
    ColRef {
        table: "unfurl_candidate".into(),
        column: "object_id".into(),
    }
}

/// The per-tenant, residency-pinned authz reverse-index table the `InRelation`/`TupleSet` forms JOIN
/// against (§5.3 / OQ-E: Identity's materialised `(subject, relation, object_id)` projection kept
/// fresh off the bus). A named constant so the lowered JOIN names the FROZEN table (the SAME table
/// every consumer's lowering JOINs — one reverse index, never a second).
pub const AUTHZ_VISIBLE_TABLE: &str = "authz_visible";

/// Which frozen `list_objects` shape drove an unfurl-class read (the filter-mode-split telemetry, 1.8).
/// `Ids` is the materialised small-result path (the allow-set inlined as `IN`); `PushedDown` is the
/// large/unbounded path (the `Filter{set_expr}` lowered + JOINed, never materialised — the no-N+1
/// class precompute for a channel full of refs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterMode {
    /// `list_objects` returned `Ids{}` (the small result set, materialised + inlined as `IN`).
    Ids,
    /// `list_objects` returned `Filter{set_expr}` whose lowering JOINed the reverse index (the
    /// large/unbounded pushed-down class path — no per-row `check`).
    PushedDown,
}

/// One bound parameter the lowered predicate carries (never a string-interpolated literal — an
/// attacker-controlled id/subject/relation can NEVER become SQL; the scan binds these). Mirrors the
/// Identity-side `BoundParam` shape (the SAME bound-not-interpolated discipline, §7.2) — restated
/// because `myelin-chat` (a consumer LEAF) cannot depend on the Identity SERVICE crate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundParam {
    /// The named placeholder in the SQL (`:id_0`, `:subject_0`, `:rel_for_read`).
    pub placeholder: String,
    /// The bound value (an object id / the viewer subject id / a relation name) — bound, never
    /// interpolated.
    pub value: String,
}

/// One JOIN the lowered predicate requires against the `authz_visible` reverse index (§5.3). The
/// candidate scan adds this to its `FROM`; the predicate references the alias. Deduplicated by
/// `(viewer, relation)` so the SAME reverse-index JOIN is emitted ONCE — the no-N+1 guarantee: an
/// `InRelation`/`TupleSet`, however deeply nested in a boolean tree, contributes at most one JOIN per
/// distinct `(viewer, relation)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthzJoin {
    /// The alias for this `authz_visible` JOIN (`av0`, `av1`, …) the predicate references.
    pub alias: String,
    /// The relation this JOIN keys on (`read`/`member`) — carried so the in-memory evaluator reads it
    /// without re-parsing the clause.
    pub relation: String,
    /// The full JOIN clause: `JOIN authz_visible <alias> ON <alias>.object_id = <via> AND
    /// <alias>.subject = :<subject> AND <alias>.relation = :<relation>`. The scan's planner does the
    /// conjoin — one query, no N+1, no post-filter.
    pub clause: String,
}

/// **The lowering result the unfurl candidate scan conjoins (§5.3/§4.3) — `(sql_predicate, joins,
/// params)`.** The scan does: `SELECT … FROM unfurl_candidate <joins> WHERE tenant_id = :t AND region
/// = :r AND (<sql_predicate>) …` binding `params`. This is **one query** — the conjoin is the planner's
/// job, NOT a per-candidate `check` loop. Leak-free: a candidate the viewer cannot see never survives
/// the `WHERE`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoweredFilter {
    /// The boolean SQL predicate over the unfurl candidate id column (ANDed into the scan `WHERE`).
    pub sql_predicate: String,
    /// The deduplicated `authz_visible` JOINs the scan adds to its `FROM` (one per distinct
    /// `(viewer, relation)` — the no-N+1 guarantee).
    pub joins: Vec<AuthzJoin>,
    /// The bound parameters (object ids, the viewer subject, relation names) — bound, never
    /// interpolated.
    pub params: Vec<BoundParam>,
}

impl LoweredFilter {
    /// `true` iff the predicate references at least one `authz_visible` JOIN — i.e. the lowering hit an
    /// `InRelation`/`TupleSet` and therefore depends on the reverse index's revision watermark (the
    /// new-enemy guard applies). A purely `Ids`/`NotIds`/`All`/`None` lowering is watermark-independent.
    pub fn depends_on_reverse_index(&self) -> bool {
        !self.joins.is_empty()
    }

    /// The filter-mode this lowering represents (the 1.8 telemetry split): a JOIN-bearing lowering is
    /// `PushedDown`; a materialised `Ids`/`NotIds`/`All`/`None` lowering is `Ids`.
    pub fn filter_mode(&self) -> FilterMode {
        if self.depends_on_reverse_index() {
            FilterMode::PushedDown
        } else {
            FilterMode::Ids
        }
    }

    /// **The no-N+1 GATE: the number of distinct reverse-index JOINs the lowering emits.** However
    /// deeply an `InRelation`/`TupleSet` is nested in a boolean tree, the lowering emits at most one
    /// JOIN per distinct `(viewer, relation)` — a drill asserts a `SetExpr` repeating the SAME relation
    /// N times lowers to ONE JOIN (the no-N+1 guarantee). 0 post-filter passes by construction (the
    /// conjoin is in the `WHERE`).
    pub fn join_count(&self) -> usize {
        self.joins.len()
    }
}

/// Internal accumulator threaded through the recursive lowering so JOINs + params are collected once
/// (the no-N+1 dedup lives here: a `(viewer, relation)` JOIN already emitted is reused by alias).
struct LowerCtx<'a> {
    /// The viewer the whole `Filter` is for (the `av.subject = :subject` binding — one viewer per
    /// `list_objects` call; the reverse-index JOIN keys on it).
    subject: &'a str,
    /// The unfurl candidate id column every JOIN/`IN` references.
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

    /// Bind a value, returning its `:placeholder` (never an interpolated literal — injection-safe).
    fn bind(&mut self, prefix: &str, value: &str) -> String {
        let placeholder = format!(":{}_{}", prefix, self.next_id);
        self.next_id += 1;
        self.params.push(BoundParam {
            placeholder: placeholder.clone(),
            value: value.to_string(),
        });
        placeholder
    }

    /// Emit (or reuse) the `authz_visible` JOIN for a `(viewer, relation)` — deduplicated by relation
    /// (the viewer is constant for the whole call): a relation already JOINed reuses its alias, so the
    /// SAME JOIN is never emitted twice (the no-N+1 guarantee). Returns `<alias>.object_id IS NOT NULL`.
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

/// **Lower a `SetExpr` to the consumer-composable SQL `Filter` over the unfurl candidate id column
/// `via` (§5.3/§4.3; the FROZEN encoding).** `viewer` is the principal the `list_objects` is for (the
/// `av.subject` binding). Returns the [`LoweredFilter`] the candidate scan ANDs into its query — **one
/// query, no N+1, no post-filter**.
///
/// The FROZEN forms (§5.3 / §7.2): `All → TRUE`; `None → FALSE`; `Ids(v) → <via> IN (…)` (empty →
/// `FALSE`, the leak-free identity); `NotIds(v) → <via> NOT IN (…)` (empty → `TRUE`); `InRelation`/
/// `TupleSet → the authz_visible JOIN`; `Union/Intersect/Difference → (a OR b)/(a AND b)/(a AND NOT b)`.
pub fn lower_over(set_expr: &SetExpr, viewer: &Principal, via: &ColRef) -> LoweredFilter {
    let mut ctx = LowerCtx::new(&viewer.principal_id.0, via);
    let sql_predicate = lower_expr(set_expr, &mut ctx);
    LoweredFilter {
        sql_predicate,
        joins: ctx.joins,
        params: ctx.params,
    }
}

/// Lower an unfurl-class `SetExpr` over the unfurl candidate id column (§4.3) — the gate's lowering.
pub fn lower_over_unfurl_candidate(set_expr: &SetExpr, viewer: &Principal) -> LoweredFilter {
    lower_over(set_expr, viewer, &unfurl_candidate_colref())
}

/// The recursive lowering of one `SetExpr` node into a boolean SQL fragment (collecting JOINs + params
/// into `ctx`). Every leaf is a predicate over the candidate id column or a reverse-index JOIN; the
/// boolean nodes compose with `OR`/`AND`/`AND NOT` — no per-row subquery, no post-filter.
fn lower_expr(expr: &SetExpr, ctx: &mut LowerCtx<'_>) -> String {
    match expr {
        // The viewer sees every candidate of this type in the tenant → no restriction.
        SetExpr::All => "TRUE".to_string(),
        // The deny set — `WHERE false`, never a permissive default (leak-free).
        SetExpr::None => "FALSE".to_string(),
        // An explicit allow-set inlined under the cardinality cap → `<via> IN (…)`. An empty allow-set
        // is `FALSE` (never a permissive TRUE — the leak-free identity element).
        SetExpr::Ids(ids) => {
            if ids.is_empty() {
                return "FALSE".to_string();
            }
            let placeholders: Vec<String> = ids.iter().map(|id| ctx.bind("id", &id.0)).collect();
            format!("{} IN ({})", ctx.via_sql, placeholders.join(", "))
        }
        // An explicit deny-set over an otherwise-visible space → `<via> NOT IN (…)`. An empty deny-set
        // excludes nothing → `TRUE`.
        SetExpr::NotIds(ids) => {
            if ids.is_empty() {
                return "TRUE".to_string();
            }
            let placeholders: Vec<String> = ids.iter().map(|id| ctx.bind("id", &id.0)).collect();
            format!("{} NOT IN ({})", ctx.via_sql, placeholders.join(", "))
        }
        // The reverse-index JOIN keyed on the candidate id column (§5.3) — one JOIN per distinct
        // relation (deduplicated — no N+1).
        SetExpr::InRelation { relation, .. } => ctx.authz_join_predicate(&relation.0),
        // A server-materialised tuple set the scan JOINs against (the big-result path). The index ref
        // names the relation it materialises; the JOIN is the same `authz_visible` target.
        SetExpr::TupleSet { index } => ctx.authz_join_predicate(&index.0),
        // Boolean composition. An empty Union is `FALSE` (sees nothing); an empty Intersect is `TRUE`
        // (no restriction) — the identity elements, never a leak.
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
        // `Difference(a, b)` = a EXCEPT b → `(a AND NOT b)` on the same row space (one query, no N+1).
        SetExpr::Difference(a, b) => {
            let af = lower_expr(a, ctx);
            let bf = lower_expr(b, ctx);
            format!("({af} AND NOT {bf})")
        }
    }
}

// ───────────────────────────── the in-memory authz_visible model (tests) ─────────────────────────

/// **The per-tenant, residency-pinned `authz_visible` reverse index (§5.3 / OQ-E) — modelled in-memory
/// for the unit + CDC tests.** The materialised `(subject, relation, object_id)` projection of the
/// ReBAC tuples Identity maintains, kept fresh off the bus, carrying a per-tenant **revision
/// watermark** (4.10). The candidate scan JOINs against THIS for the `InRelation`/`TupleSet` forms; a
/// read carrying a zookie at-or-after a revoke's revision must NOT see the revoked object (the
/// new-enemy guard — never serve a stale grant from a behind index). The REAL `authz_visible` table
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

    /// Grant `subject` visibility of `object_id` under `relation` and advance the watermark (the
    /// kept-fresh-off-the-bus projection of a `write_tuples`).
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

    /// Revoke `subject`'s visibility of `object_id` under `relation` and advance the watermark (the
    /// new-enemy case: a read carrying a zookie at-or-after this revision must NOT see `object_id`).
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

    /// Advance the per-tenant revision watermark to `revision` (monotone — a stale advance is ignored;
    /// the zookie strings are the zero-padded `zk-<rev>` form so lexical order == revision order).
    pub fn advance_watermark(&self, tenant: &TenantId, region: &Region, revision: &str) {
        let key = (tenant.0.clone(), region.0.clone());
        let mut w = self.watermark.lock().unwrap();
        let cur = w.entry(key).or_default();
        // `>` (strictly newer advances; equal is a no-op) — monotone. The `> → >=` mutant here is an
        // EQUIVALENT mutant (when `revision == *cur` both branches assign the SAME value — no
        // observable difference), so it is correctly not caught; the monotonicity that MATTERS (a
        // stale/older revision never regresses) IS asserted. Mirrors the identical documented
        // equivalent-mutant in `myelin_git::list_filter::AuthzVisibleIndex::advance_watermark`.
        if revision > cur.as_str() {
            *cur = revision.into();
        }
    }

    /// The current per-tenant revision watermark (`""` if never advanced).
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

    /// **The new-enemy guard (§5.3 zookie; 4.10 read-your-writes).** Whether the JOIN may serve a scan
    /// requiring revision `required`: `true` iff the per-tenant watermark is at-or-after `required`. A
    /// `required` of `""` always serves. When `false` the caller falls back to per-row `check` rather
    /// than serving a stale grant.
    pub fn serves(&self, tenant: &TenantId, region: &Region, required: &Zookie) -> bool {
        if required.0.is_empty() {
            return true;
        }
        self.watermark(tenant, region).0 >= required.0
    }

    /// **Evaluate a [`LoweredFilter`] against this in-memory index: the candidate object ids that
    /// survive the JOIN + predicate (the SAME row set the SQL `WHERE`/JOIN keeps).** Leak-free: a
    /// candidate the viewer has no `relation` tuple for (and no inline `IN`-allow) never survives. This
    /// models the SQL the live `--features integration` test proves; it is NOT a per-row `check`.
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

    /// Evaluate one LEAF predicate fragment against the reverse index / the bound `IN` sets.
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
        // `avN.object_id IS NOT NULL` — the reverse-index JOIN for the alias's relation.
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
        // `<via> NOT IN (…)` / `<via> IN (…)` — the inline bound allow/deny set.
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

    /// Resolve the placeholders inside an `IN (…)` fragment to their bound values (the literal ids live
    /// in `params`, not the SQL — the bound-not-interpolated discipline).
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
/// fragments, `AND`/`OR`/`AND NOT`, parentheses) — enough to evaluate the [`lower_expr`] output against
/// one candidate row (the in-memory model of the SQL `WHERE`). `leaf(frag)` evaluates a single LEAF
/// fragment. Test/model machinery, not a general SQL engine; the production path is the database
/// evaluating the same predicate (proven in the `--features integration` test).
fn eval_predicate(pred: &str, leaf: &mut dyn FnMut(&str) -> bool) -> bool {
    let tokens = tokenize(pred);
    let mut pos = 0;
    let v = parse_or(&tokens, &mut pos, leaf);
    debug_assert_eq!(pos, tokens.len(), "the predicate parsed fully: {pred}");
    v
}

/// Tokenize the lowered predicate into `(`, `)`, `AND NOT`, `AND`, `OR`, `NOT`, and LEAF fragments. A
/// leaf's own `IN (…)` parens are kept as part of the leaf (only TOP-LEVEL parens are structural).
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
        if rest.starts_with("IN (") {
            cur.push_str("IN (");
            i += 4;
            depth_in_leaf += 1;
            continue;
        }
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
            if rest.starts_with("NOT ") && cur.trim().is_empty() {
                out.push("NOT".into());
                i += "NOT ".chars().count();
                continue;
            }
            let c = chars[i];
            if c == '(' {
                flush(&mut cur, &mut out);
                out.push("(".into());
                i += 1;
                continue;
            }
            if c == ')' {
                flush(&mut cur, &mut out);
                out.push(")".into());
                i += 1;
                continue;
            }
        } else if chars[i] == ')' {
            // The closing paren of the leaf's own `IN (…)` — kept in the leaf.
            cur.push(')');
            i += 1;
            depth_in_leaf -= 1;
            continue;
        }
        cur.push(chars[i]);
        i += 1;
    }
    flush(&mut cur, &mut out);
    out
}

/// `OR` is the lowest-precedence binary operator.
fn parse_or(tokens: &[String], pos: &mut usize, leaf: &mut dyn FnMut(&str) -> bool) -> bool {
    let mut v = parse_and(tokens, pos, leaf);
    while *pos < tokens.len() && tokens[*pos] == "OR" {
        *pos += 1;
        let rhs = parse_and(tokens, pos, leaf);
        v = v || rhs;
    }
    v
}

/// `AND` / `AND NOT` bind tighter than `OR`.
fn parse_and(tokens: &[String], pos: &mut usize, leaf: &mut dyn FnMut(&str) -> bool) -> bool {
    let mut v = parse_not(tokens, pos, leaf);
    while *pos < tokens.len() && (tokens[*pos] == "AND" || tokens[*pos] == "AND NOT") {
        let negate = tokens[*pos] == "AND NOT";
        *pos += 1;
        let rhs = parse_not(tokens, pos, leaf);
        v = v && (if negate { !rhs } else { rhs });
    }
    v
}

/// A leading `NOT` negates the next factor.
fn parse_not(tokens: &[String], pos: &mut usize, leaf: &mut dyn FnMut(&str) -> bool) -> bool {
    if *pos < tokens.len() && tokens[*pos] == "NOT" {
        *pos += 1;
        return !parse_atom(tokens, pos, leaf);
    }
    parse_atom(tokens, pos, leaf)
}

/// An atom is a parenthesised sub-expression or a LEAF fragment.
fn parse_atom(tokens: &[String], pos: &mut usize, leaf: &mut dyn FnMut(&str) -> bool) -> bool {
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
    let frag = &tokens[*pos];
    *pos += 1;
    leaf(frag)
}

// ───────────────────────────── unit tests (the §4.3 SetExpr → JOIN gate) ──────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{AuthzIndexRef, ObjectId, PrincipalId, PrincipalKind, RelName};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn viewer(id: &str) -> Principal {
        Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
    }
    fn in_relation(rel: &str) -> SetExpr {
        SetExpr::InRelation {
            relation: RelName(rel.into()),
            via_column: unfurl_candidate_colref(),
        }
    }

    /// The `InRelation` form lowers to ONE `authz_visible` JOIN over the candidate id column — the
    /// per-viewer gate's pushed-down class (no per-candidate `check`, no post-filter).
    #[test]
    fn in_relation_lowers_to_one_join_over_candidate_column() {
        let lowered = lower_over_unfurl_candidate(&in_relation("read"), &viewer("alice"));
        assert_eq!(lowered.join_count(), 1, "one JOIN for one relation");
        assert!(lowered.depends_on_reverse_index());
        assert_eq!(lowered.filter_mode(), FilterMode::PushedDown);
        // the JOIN keys on the candidate id column + the FROZEN reverse-index table.
        let join = &lowered.joins[0];
        assert!(join.clause.contains("JOIN authz_visible"));
        assert!(join
            .clause
            .contains("ON av0.object_id = unfurl_candidate.object_id"));
        // the predicate references the JOIN alias (the conjoin is in the WHERE, not a post-filter).
        assert_eq!(lowered.sql_predicate, "av0.object_id IS NOT NULL");
    }

    /// **The no-N+1 GATE: a `SetExpr` repeating the SAME relation N times lowers to ONE JOIN.** However
    /// deeply nested in a boolean tree, an `InRelation` of the same `(viewer, relation)` emits at most
    /// one reverse-index JOIN — the no-N+1 guarantee (0 per-candidate JOIN explosion).
    #[test]
    fn repeated_relation_dedups_to_one_join_no_n_plus_1() {
        let expr = SetExpr::Union(vec![
            in_relation("read"),
            SetExpr::Intersect(vec![in_relation("read"), in_relation("read")]),
            in_relation("read"),
        ]);
        let lowered = lower_over_unfurl_candidate(&expr, &viewer("bob"));
        assert_eq!(
            lowered.join_count(),
            1,
            "the same (viewer, relation) JOINs ONCE however nested — no N+1"
        );
    }

    /// Two DISTINCT relations lower to two distinct JOINs (each deduped within itself).
    #[test]
    fn distinct_relations_emit_distinct_joins() {
        let expr = SetExpr::Union(vec![in_relation("read"), in_relation("member")]);
        let lowered = lower_over_unfurl_candidate(&expr, &viewer("carol"));
        assert_eq!(lowered.join_count(), 2);
    }

    /// The frozen leak-free identity elements: `None → FALSE`, empty `Ids → FALSE`, empty `NotIds →
    /// TRUE`, `All → TRUE` — a deny never lowers to a permissive default.
    #[test]
    fn frozen_identity_elements_are_leak_free() {
        let v = viewer("dan");
        assert_eq!(
            lower_over_unfurl_candidate(&SetExpr::None, &v).sql_predicate,
            "FALSE"
        );
        assert_eq!(
            lower_over_unfurl_candidate(&SetExpr::Ids(vec![]), &v).sql_predicate,
            "FALSE"
        );
        assert_eq!(
            lower_over_unfurl_candidate(&SetExpr::NotIds(vec![]), &v).sql_predicate,
            "TRUE"
        );
        assert_eq!(
            lower_over_unfurl_candidate(&SetExpr::All, &v).sql_predicate,
            "TRUE"
        );
    }

    /// **The gate is leak-free against a candidate set: a candidate the viewer has no tuple for never
    /// survives the JOIN.** The in-memory `authz_visible` model evaluates the lowered `InRelation` JOIN;
    /// only the granted candidate survives (0 leak of the ungranted one).
    #[test]
    fn join_filters_candidates_leak_free() {
        let index = AuthzVisibleIndex::new();
        let v = viewer("erin");
        // erin can read channel:c1, not channel:c2.
        index.grant(&tenant(), &region(), "erin", "read", "channel:c1", "zk-01");
        let lowered = lower_over_unfurl_candidate(&in_relation("read"), &v);
        let candidates = vec![ObjectId("channel:c1".into()), ObjectId("channel:c2".into())];
        let visible = index.evaluate(&tenant(), &region(), &v, &lowered, &candidates);
        assert_eq!(visible, vec![ObjectId("channel:c1".into())], "0 leak of c2");
    }

    /// **The new-enemy guard: a revoke drops the candidate from the JOIN result (the reverse index
    /// reflects the revoke, the watermark advances).** A re-evaluation after the revoke yields 0 — the
    /// revoked viewer cannot see the candidate (no stale grant served).
    #[test]
    fn revoke_drops_candidate_new_enemy() {
        let index = AuthzVisibleIndex::new();
        let v = viewer("frank");
        index.grant(&tenant(), &region(), "frank", "read", "channel:c9", "zk-01");
        let lowered = lower_over_unfurl_candidate(&in_relation("read"), &v);
        let candidates = vec![ObjectId("channel:c9".into())];
        assert_eq!(
            index
                .evaluate(&tenant(), &region(), &v, &lowered, &candidates)
                .len(),
            1
        );
        // revoke advances the watermark; the re-eval sees 0.
        index.revoke(&tenant(), &region(), "frank", "read", "channel:c9", "zk-02");
        assert!(index
            .evaluate(&tenant(), &region(), &v, &lowered, &candidates)
            .is_empty());
        // the watermark is at-or-after the revoke revision (the strong read serves the post-revoke set).
        assert!(index.serves(&tenant(), &region(), &Zookie("zk-02".into())));
    }

    /// The watermark is monotone — a stale (older) advance never regresses it.
    #[test]
    fn watermark_is_monotone_stale_never_regresses() {
        let index = AuthzVisibleIndex::new();
        index.advance_watermark(&tenant(), &region(), "zk-05");
        index.advance_watermark(&tenant(), &region(), "zk-02"); // stale — ignored.
        assert_eq!(index.watermark(&tenant(), &region()).0, "zk-05");
    }

    /// `Difference(All, Ids)` → `(TRUE AND NOT <via> IN (…))` excludes exactly the deny ids.
    #[test]
    fn difference_excludes_the_deny_set() {
        let index = AuthzVisibleIndex::new();
        let v = viewer("gwen");
        let expr = SetExpr::Difference(
            Box::new(SetExpr::All),
            Box::new(SetExpr::Ids(vec![ObjectId("channel:secret".into())])),
        );
        let lowered = lower_over_unfurl_candidate(&expr, &v);
        let candidates = vec![
            ObjectId("channel:open".into()),
            ObjectId("channel:secret".into()),
        ];
        let visible = index.evaluate(&tenant(), &region(), &v, &lowered, &candidates);
        assert_eq!(visible, vec![ObjectId("channel:open".into())]);
    }

    /// The `TupleSet` form (the server-materialised big-result path) lowers to a JOIN too.
    #[test]
    fn tuple_set_lowers_to_join() {
        let expr = SetExpr::TupleSet {
            index: AuthzIndexRef("read".into()),
        };
        let lowered = lower_over_unfurl_candidate(&expr, &viewer("hank"));
        assert_eq!(lowered.join_count(), 1);
        assert_eq!(lowered.filter_mode(), FilterMode::PushedDown);
    }
}

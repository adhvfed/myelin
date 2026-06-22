//! # `list_filter` — the `list_objects` `SetExpr` push-down for Knowledge views/lists/COUNTs
//! (KN-P16 / P-306 — the KN-D5 0-leak-incl-COUNT crux)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/knowledge-platform/architecture/`
//! `02-internals-and-algorithms.md` §4.1 (the FROZEN `SetExpr` lowering over `db_row.id` — the
//! `All`/`None`/`Ids`/`InRelation{row_reader, via_column}`/`Union` table; the JOIN against
//! `authz_visible`; **closing the count-leak because the ACL conjunct is INSIDE the query**) + §5
//! (permission-filtered reads everywhere — NEVER post-filter).
//! `03-events-contracts-and-glue.md` §3.2 (the `list_objects`/`check` glue, the `page.acl_zookie`
//! read-your-writes watermark).
//!
//! **Contract-index:** rows **4.3** (`list_objects` + the `SetExpr` push-down — **CONSUMED** here:
//! Knowledge lowers the `Filter{set_expr}` Identity returns over its OWN `db_row.id`/`page.id`
//! column into ONE leak-free list/board/view/COUNT query) and **4.6/4.10** (`write_tuples → zookie`
//! — the ACL-change path stamps `page.acl_zookie`, the read-your-writes watermark; the write half
//! lives in [`crate::authority::AclZookieTable`], the read-side watermark guard is [`AuthzVisibleIndex`]
//! here).
//!
//! ## Why a SECOND lowering and not `myelin_identity_service::lowering`
//! Knowledge is a **leaf consumer** over the frozen `myelin-identity` CONTRACT crate (the
//! `Principal`/`SetExpr`/`Zookie` ABI), NOT the Identity SERVICE engine — exactly as
//! `myelin_git::list_filter` and `myelin_refs_service::backlinks` are. The lowering ALGORITHM is the
//! shared §4.1/§7.2 frozen encoding (`All`→`TRUE`, `None`→`FALSE`, `Ids`→`IN`, `InRelation`→the
//! `authz_visible` JOIN, `Union`/`Intersect`/`Difference`→`OR`/`AND`/`AND NOT`); each consumer owns
//! its lowering over its OWN id column so the production DAG stays acyclic (`identity_is_sink()`).
//! The SHAPE is byte-identical to the sibling consumers (proven by the unit tests here mirroring
//! `myelin_identity_service::lowering::tests`); the column is `db_row.id` / `page.id`.
//!
//! ## What this module ships (KN-P16 — the load-bearing crux)
//! 1. [`lower_over`] / [`lower_over_db_row_id`] / [`lower_over_page_id`] — lower the returned
//!    `SetExpr` over Knowledge's OWN id column. One query, no N+1, no post-filter.
//! 2. [`compose_db_view_query`] — the §4.1 `VIEW_QUERY`: the ACL `SetExpr` conjoined into the db
//!    view/board scan over the verified `(tenant, region)` partition, **before** the `ORDER BY`/`LIMIT`
//!    (pre-filter, never post-filter).
//! 3. [`compose_db_count_query`] — **THE KN-D5 HEADLINE: a permission-correct `COUNT(*)`.** The ACL
//!    conjunct is INSIDE the COUNT query (the same JOIN/`WHERE`), so an aggregate over a row-restricted
//!    db counts ONLY rows the viewer may read — the **count-leak is closed** (a `COUNT` cannot reveal
//!    the existence/number of forbidden rows; §4.1 KD-5).
//! 4. [`AuthzVisibleIndex`] — the in-memory model of the per-tenant residency-pinned `authz_visible`
//!    reverse index (the §4.1 JOIN target) + the 4.10 revision watermark (the new-enemy guard). It
//!    `evaluate`s a lowered filter (the in-memory mirror of the SQL `WHERE`/JOIN) AND `count_visible`s
//!    (the in-memory mirror of the permission-correct `COUNT`) for the unit + KN-D5 drill. The REAL
//!    `authz_visible` table is JOINed in SQL; the live one-query/0-leak/0-count-leak proof is the
//!    `--features integration` test against the dev-stack Postgres.
//!
//! ## The leak-free invariant (ADR-03 / §5 / KN-D5)
//! A row/page the viewer cannot read is **ABSENT** from every view/list AND **uncounted** by every
//! aggregate — because the ACL is conjoined into the scan BEFORE pagination/aggregation, never a
//! post-filter over a wider materialised set. `None` → `FALSE` (`WHERE false`), an empty `Ids`
//! allow-set → `FALSE` (never a permissive `TRUE`), and a cross-tenant row is excluded by the tenant
//! predicate the composer ALWAYS emits (a tenant-less list/count is unconstructable — the
//! `tenant-predicate` lint; the `no-cross-db` reach is structural: the conjoin is one db's `db_id`
//! partition).
//!
//! ## The read-your-writes / new-enemy guard (§4.1 consistency; 4.10)
//! A security-sensitive scan passes the zookie the ACL change stamped on `page.acl_zookie`
//! ([`crate::authority::AclZookieTable`]); [`AuthzVisibleIndex::serves`] compares the scan's required
//! revision against the per-tenant reverse-index watermark — at-or-after → the JOIN serves; behind →
//! the caller falls back to per-row `check` (never serve a just-revoked grant stale). This is the
//! read half of the 4.6/4.10 pair whose write half ([`crate::authority::AclZookieTable::stamp`])
//! KN-P14/KN-P15 shipped.

use myelin_identity::{ColRef, Principal, SetExpr, Zookie};
use myelin_tenancy::{Region, TenantId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::rebac_fragment::{DB_ROW_ID_COLUMN, DB_ROW_TABLE};

/// The per-tenant, residency-pinned authz reverse-index table the `InRelation`/`TupleSet` forms JOIN
/// against (§4.1 / OQ-E — the SpiceDB/Zanzibar `LookupResources` reverse index realised as a
/// co-located JOIN target). A named constant so the lowered JOIN names the FROZEN table.
pub const AUTHZ_VISIBLE_TABLE: &str = "authz_visible";

/// The `page` table id column the page-tree list/board `SetExpr` lowers over (§4.1 / §5 — a page list
/// keyed on `page.id`, the `parent_page->read − direct_block` inheritance the reverse index serves).
pub const PAGE_TABLE: &str = "page";
/// The id column on [`PAGE_TABLE`].
pub const PAGE_ID_COLUMN: &str = "id";

/// Knowledge's OWN `db_row.id` column the row-level `SetExpr` lowers over (§4.1 / §5.1 — the
/// row-restricted db's row id, the `InRelation{row_reader, via_column: db_row.id}` JOIN key).
pub fn db_row_id_colref() -> ColRef {
    ColRef {
        table: DB_ROW_TABLE.into(),
        column: DB_ROW_ID_COLUMN.into(),
    }
}

/// Knowledge's OWN `page.id` column the page-tree list `SetExpr` lowers over (§4.1 / §5).
pub fn page_id_colref() -> ColRef {
    ColRef {
        table: PAGE_TABLE.into(),
        column: PAGE_ID_COLUMN.into(),
    }
}

/// Which frozen `list_objects` shape drove a read (the 1.8 filter-mode-split telemetry): the
/// materialised `Ids`/`NotIds`/`All`/`None` path, or the pushed-down `Filter`/`TupleSet` JOIN path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterMode {
    /// `list_objects` returned a materialised set (or `All`/`None`) — no reverse-index JOIN.
    Ids,
    /// `list_objects` returned `Filter{set_expr}` whose lowering JOINed the reverse index (the
    /// large/unbounded path — the `Filter{set_expr}` lowered + JOINed, never materialised).
    PushedDown,
}

// ───────────────────────────── the lowering (§4.1 — bound, never interpolated) ───────────────────

/// One bound parameter the lowered predicate carries (never a string-interpolated literal — an id /
/// subject / relation an attacker controls can NEVER become SQL; the consumer binds these).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundParam {
    /// The named placeholder in the SQL (`:id_0`, `:subject_0`, `:rel_for_read`).
    pub placeholder: String,
    /// The bound value (an object id / the viewer subject / a relation name) — bound, never
    /// interpolated.
    pub value: String,
}

/// One JOIN the lowered predicate requires against the `authz_visible` reverse index (§4.1). The
/// scan adds this to its `FROM`; the predicate references the alias. Deduplicated by relation (the
/// viewer is constant for the whole call) so the SAME reverse-index JOIN is emitted ONCE — the
/// no-N+1 guarantee: an `InRelation`/`TupleSet`, however deeply nested in a boolean tree, contributes
/// at most one JOIN per distinct `(viewer, relation)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthzJoin {
    /// The alias for this `authz_visible` JOIN (`av0`, `av1`, …) the predicate references.
    pub alias: String,
    /// The relation this JOIN keys on (`read`) — carried so the in-memory evaluator + the dedup read
    /// it without re-parsing the clause.
    pub relation: String,
    /// The full JOIN clause: `JOIN authz_visible <alias> ON <alias>.object_id = <via_column> AND
    /// <alias>.subject = :<subject> AND <alias>.relation = :<relation>`. The scan's own query planner
    /// does the conjoin — one query, no N+1, no post-filter.
    pub clause: String,
}

/// **The lowering result the Knowledge view/list/COUNT scan conjoins (§4.1) — `(sql_predicate,
/// joins, params)`.** The scan does: `SELECT … FROM <table> <joins> WHERE tenant = :t AND db_id =
/// :db AND (<acl>) …` binding `params`. This is **one query** — the conjoin is the planner's job, NOT
/// a per-row `check` loop. Leak-free: a row the viewer cannot read never survives the `WHERE`
/// (including under a `COUNT`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoweredFilter {
    /// The boolean SQL predicate over Knowledge's own id column (ANDed into the view/list/COUNT
    /// `WHERE`).
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
    /// an `InRelation`/`TupleSet` and therefore depends on the reverse index's revision watermark (the
    /// new-enemy guard applies). A purely `Ids`/`NotIds`/`All`/`None` lowering is watermark-independent
    /// (it carries its own materialised set).
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
}

/// Internal accumulator threaded through the recursive lowering so JOINs + params are collected once
/// (the no-N+1 dedup lives here: a `(viewer, relation)` JOIN already emitted is reused by alias).
struct LowerCtx<'a> {
    /// The viewer the whole `Filter` is for (the `av.subject = :subject` binding — one viewer per
    /// `list_objects` call; the reverse-index JOIN keys on it).
    subject: &'a str,
    /// Knowledge's own id column (`db_row.id` / `page.id`) every JOIN/`IN` references.
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

    /// Emit (or reuse) the `authz_visible` JOIN for a `(viewer, relation)` — the §4.1 reverse-index
    /// JOIN keyed on Knowledge's own id column. Deduplicated by relation (the viewer is constant for
    /// the whole call): a relation already JOINed reuses its alias, so the SAME JOIN is never emitted
    /// twice (the no-N+1 guarantee — at most one JOIN per distinct `(viewer, relation)`, however
    /// nested). Returns the boolean predicate fragment `<alias>.object_id IS NOT NULL`.
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

/// **Lower a `SetExpr` to the consumer-composable SQL `Filter` over Knowledge's own id column `via`
/// (§4.1; the FROZEN encoding).** `viewer` is the principal the `list_objects` is for (the
/// `av.subject` binding); `via` is Knowledge's own id column (`db_row.id` / `page.id`). Returns the
/// [`LoweredFilter`] `(sql_predicate, joins, params)` the view/list/COUNT scan ANDs into its query —
/// **one query, no N+1, no post-filter**.
///
/// The FROZEN forms (§4.1):
/// - `All` → `TRUE` (no predicate — the viewer reads the whole db via page-level inheritance);
/// - `None` → `FALSE` (`WHERE false` — deny, never a permissive default);
/// - `Ids(v)` → `<via> IN (:p0, …)` (inlined under the cardinality cap; empty → `FALSE`);
/// - `NotIds(v)` → `<via> NOT IN (…)` (empty → `TRUE`);
/// - `InRelation{relation, …}` / `TupleSet{index}` → the `authz_visible` JOIN keyed on `<via>` (the
///   row-restricted case lowers to `InRelation{row_reader, db_row.id}`);
/// - `Union`/`Intersect`/`Difference` → `(a OR b)` / `(a AND b)` / `(a AND NOT b)`.
pub fn lower_over(set_expr: &SetExpr, viewer: &Principal, via: &ColRef) -> LoweredFilter {
    let mut ctx = LowerCtx::new(&viewer.principal_id.0, via);
    let sql_predicate = lower_expr(set_expr, &mut ctx);
    LoweredFilter {
        sql_predicate,
        joins: ctx.joins,
        params: ctx.params,
    }
}

/// Lower a db-row-list `SetExpr` over Knowledge's `db_row.id` column (§4.1 / §5.1 — the row-restricted
/// db's row id; the `row_reader` GROUP grant JOINs the reverse index here).
pub fn lower_over_db_row_id(set_expr: &SetExpr, viewer: &Principal) -> LoweredFilter {
    lower_over(set_expr, viewer, &db_row_id_colref())
}

/// Lower a page-tree list `SetExpr` over Knowledge's `page.id` column (§4.1 / §5).
pub fn lower_over_page_id(set_expr: &SetExpr, viewer: &Principal) -> LoweredFilter {
    lower_over(set_expr, viewer, &page_id_colref())
}

/// The recursive lowering of one `SetExpr` node into a boolean SQL fragment (collecting JOINs +
/// params into `ctx`). Every leaf is a predicate over Knowledge's own id column or a reverse-index
/// JOIN; the boolean nodes compose with `OR`/`AND`/`AND NOT` — no per-row subquery, no post-filter.
fn lower_expr(expr: &SetExpr, ctx: &mut LowerCtx<'_>) -> String {
    match expr {
        // The viewer reads the whole db of this type in the tenant (page-level inheritance) → no
        // restriction.
        SetExpr::All => "TRUE".to_string(),
        // The deny set — `WHERE false`, never a permissive default (leak-free).
        SetExpr::None => "FALSE".to_string(),
        // An explicit allow-set inlined under the cardinality cap → `<via> IN (:p0, …)`. An empty
        // allow-set is `FALSE` (IN () is not valid SQL and means "no rows" — never a permissive TRUE).
        SetExpr::Ids(ids) => {
            if ids.is_empty() {
                return "FALSE".to_string();
            }
            let placeholders: Vec<String> = ids.iter().map(|id| ctx.bind("id", &id.0)).collect();
            format!("{} IN ({})", ctx.via_sql, placeholders.join(", "))
        }
        // An explicit deny-set over an otherwise-visible space → `<via> NOT IN (…)`. An empty
        // deny-set excludes nothing → `TRUE`.
        SetExpr::NotIds(ids) => {
            if ids.is_empty() {
                return "TRUE".to_string();
            }
            let placeholders: Vec<String> = ids.iter().map(|id| ctx.bind("id", &id.0)).collect();
            format!("{} NOT IN ({})", ctx.via_sql, placeholders.join(", "))
        }
        // The reverse-index JOIN keyed on Knowledge's own id column (§4.1) — the §5.1 row-restricted
        // case (`InRelation{row_reader, db_row.id}`). One JOIN per distinct relation (deduplicated —
        // no N+1).
        SetExpr::InRelation { relation, .. } => ctx.authz_join_predicate(&relation.0),
        // A server-materialised tuple set the scan JOINs against (the big-result path). The index ref
        // names the relation it materialises; the JOIN is the same `authz_visible` target.
        SetExpr::TupleSet { index } => ctx.authz_join_predicate(&index.0),
        // Boolean composition → `(a OR b)` / `(a AND b)` / `(a AND NOT b)` (§4.1 — Union/Intersect/
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

// ───────────────────────────── the view/COUNT composers (§4.1 — one query) ───────────────────────

/// **A composed, leak-free db query (the §4.1 push-down conjoined into ONE statement).** The `sql` is
/// a single `SELECT … FROM <table> <joins> WHERE <table>.tenant = :tenant AND <table>.db_id = :db_id
/// AND (<acl>) …` — the ACL pre-filter is conjoined BEFORE pagination/aggregation (never a
/// post-filter), with the tenant predicate isolating cross-tenant rows and the `db_id` predicate
/// confining the read to the ONE database (the `no-cross-db` structural reach). The `params` are
/// bound (the tenant/db_id + the lowered filter's ids/subject/relation), never interpolated. One
/// query — [`ComposedQuery::statement_count`] is always 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposedQuery {
    /// The single SQL statement (no trailing `;` — one statement, exactly).
    pub sql: String,
    /// The bound parameters (tenant, db_id, the lowered filter's ids/subject/relation) — bound, never
    /// interpolated.
    pub params: Vec<BoundParam>,
    /// Which `list_objects` shape drove this read (the 1.8 filter-mode split telemetry).
    pub filter_mode: FilterMode,
    /// `true` iff this query is an aggregate `COUNT(*)` (the KN-D5 count-leak-closed path) rather than
    /// a row-projecting view scan.
    pub is_count: bool,
}

impl ComposedQuery {
    /// The number of SQL statements this read issues — ALWAYS 1 (the §4.1 no-N+1 guarantee: the
    /// conjoin is a JOIN + `WHERE` the query planner resolves, never a per-row `check` loop, never a
    /// post-filter second pass). A drill asserts this is `1`.
    pub fn statement_count(&self) -> usize {
        self.sql.split(';').filter(|s| !s.trim().is_empty()).count()
    }
}

/// **Compose the §4.1 `VIEW_QUERY` — the db rows the `viewer` may `read`, leak-free, in ONE query.**
/// Given the `set_expr` Identity returned for `list_objects(viewer, read, database_row)`, lower it
/// over `db_row.id` and conjoin it into the db view/board scan over the verified `(tenant, db_id)`
/// partition. The page bound is bound, not interpolated; the ACL is conjoined BEFORE the
/// `ORDER BY`/`LIMIT` (pre-filter, never post-filter).
pub fn compose_db_view_query(
    set_expr: &SetExpr,
    viewer: &Principal,
    scope_tenant: &TenantId,
    db_id: &str,
) -> ComposedQuery {
    let lowered = lower_over_db_row_id(set_expr, viewer);
    let filter_mode = lowered.filter_mode();
    let joins = render_joins(&lowered);
    // The tenant + db_id predicates are ALWAYS emitted: a tenant-less list is unconstructable
    // (`tenant-predicate` lint; EI-02 §1 no cross-tenant query path) and the `db_id` predicate
    // confines the scan to the ONE database (`no-cross-db` — a view never reaches another db's rows).
    // The ACL predicate is conjoined BEFORE the ORDER BY / LIMIT — pre-filter, never post-filter
    // (ADR-03 / §5).
    let sql = format!(
        "SELECT {t}.id FROM {t}{joins} \
         WHERE {t}.tenant = :tenant AND {t}.db_id = :db_id \
         AND ({acl}) ORDER BY {t}.id LIMIT :page",
        t = DB_ROW_TABLE,
        acl = lowered.sql_predicate,
    );
    ComposedQuery {
        sql,
        params: prepend_scope_params(scope_tenant, db_id, lowered.params),
        filter_mode,
        is_count: false,
    }
}

/// **Compose the §4.1 permission-correct `COUNT(*)` — THE KN-D5 HEADLINE (the count-leak closed).**
/// Given the `set_expr` Identity returned for `list_objects(viewer, read, database_row)`, lower it
/// over `db_row.id` and conjoin it into a `SELECT COUNT(*)` over the verified `(tenant, db_id)`
/// partition. The ACL conjunct is INSIDE the COUNT query (the SAME JOIN/`WHERE` as the view), so the
/// aggregate counts ONLY rows the viewer may read — a `COUNT` over a row-restricted db can NOT reveal
/// the existence or number of forbidden rows (§4.1 KD-5). This is NOT a post-count subtraction (which
/// would itself leak the hidden count); the count IS the conjoined query's cardinality.
pub fn compose_db_count_query(
    set_expr: &SetExpr,
    viewer: &Principal,
    scope_tenant: &TenantId,
    db_id: &str,
) -> ComposedQuery {
    let lowered = lower_over_db_row_id(set_expr, viewer);
    let filter_mode = lowered.filter_mode();
    let joins = render_joins(&lowered);
    // The ACL conjunct is INSIDE the aggregate — the COUNT is over the JOINed/filtered row set, NOT a
    // post-filter of a wider count (which would leak the forbidden cardinality). Tenant + db_id
    // confined exactly as the view query.
    let sql = format!(
        "SELECT COUNT(*) FROM {t}{joins} \
         WHERE {t}.tenant = :tenant AND {t}.db_id = :db_id AND ({acl})",
        t = DB_ROW_TABLE,
        acl = lowered.sql_predicate,
    );
    ComposedQuery {
        sql,
        params: prepend_scope_params(scope_tenant, db_id, lowered.params),
        filter_mode,
        is_count: true,
    }
}

/// Render the lowered JOINs into the ` JOIN …` suffix the `FROM` carries (deduplicated upstream — one
/// per distinct `(viewer, relation)`).
fn render_joins(lowered: &LoweredFilter) -> String {
    lowered
        .joins
        .iter()
        .map(|j| format!(" {}", j.clause))
        .collect()
}

/// Prepend the bound `(tenant, db_id)` scope params to the lowered filter's params (the scope
/// predicate the composer always emits; bound, never interpolated).
fn prepend_scope_params(
    scope_tenant: &TenantId,
    db_id: &str,
    lowered_params: Vec<BoundParam>,
) -> Vec<BoundParam> {
    let mut params = vec![
        BoundParam {
            placeholder: ":tenant".into(),
            value: scope_tenant.0.clone(),
        },
        BoundParam {
            placeholder: ":db_id".into(),
            value: db_id.into(),
        },
    ];
    params.extend(lowered_params);
    params
}

// ───────────────────────────── the in-memory authz_visible model (tests/drills) ──────────────────

/// **The per-tenant, residency-pinned `authz_visible` reverse index (§4.1 / OQ-E) — modelled
/// in-memory for the unit + KN-D5 drill.** The materialised `(subject, relation, object_id)`
/// projection of the ReBAC tuples Identity maintains, kept fresh off the bus, carrying a per-tenant
/// **revision watermark** (4.10). The view/list/COUNT read JOINs against THIS for the
/// `InRelation`/`TupleSet` forms; a read carrying a zookie at-or-after a revoke's revision must NOT
/// see (or count) the revoked row (the new-enemy guard — never serve a stale grant from a behind
/// index). Per-tenant: the key is `(tenant, region, subject, relation)` — **no cross-tenant query
/// path** (EI-02 §1). The REAL `authz_visible` table (Identity-maintained, JOINed in SQL) replaces
/// this in the `--features integration` proof; the in-memory model is the SAME JOIN semantics +
/// watermark + COUNT cardinality.
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

    /// Grant `subject` visibility of `object_id` under `relation` in `(tenant, region)` and advance
    /// the watermark to `at_revision` (the kept-fresh-off-the-bus projection of a `write_tuples`).
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

    /// Revoke `subject`'s visibility of `object_id` under `relation` and advance the watermark to
    /// `at_revision` (the projection of a revoke — the new-enemy case: a read carrying a zookie
    /// at-or-after this revision must NOT see/count `object_id`).
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
    /// order, mirroring the Identity-side S8 watermark and [`crate::authority::AclZookieTable`]).
    pub fn advance_watermark(&self, tenant: &TenantId, region: &Region, revision: &str) {
        let key = (tenant.0.clone(), region.0.clone());
        let mut w = self.watermark.lock().unwrap();
        let cur = w.entry(key).or_default();
        // `>` (strictly newer advances; equal is a no-op) — monotone, pinned by
        // `watermark_is_monotone_stale_never_regresses`. NOTE: the `> → >=` mutant here is an
        // EQUIVALENT mutant (when `revision == *cur`, both branches assign the SAME value — no
        // observable difference), so it is correctly not caught; the monotonicity that MATTERS (a
        // stale/older revision never regresses) IS asserted. Mirrors the identical documented
        // equivalent-mutant in `myelin_git::list_filter` / `myelin_refs_service::backlinks`.
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

    /// **The new-enemy guard (§4.1 consistency; 4.10 read-your-writes).** Whether the JOIN may serve
    /// a scan that requires revision `required` (the `page.acl_zookie` a security-sensitive read
    /// passes): `true` iff the per-tenant watermark is at-or-after `required` (the index reflects the
    /// writes the scan must see). A `required` of `""` (default-consistency, no freshness floor)
    /// always serves. When this is `false` the caller falls back to per-row `check` rather than
    /// serving a stale grant.
    pub fn serves(&self, tenant: &TenantId, region: &Region, required: &Zookie) -> bool {
        if required.0.is_empty() {
            return true;
        }
        self.watermark(tenant, region).0 >= required.0
    }

    /// **Evaluate a [`LoweredFilter`] against this in-memory index: the set of `candidate` object ids
    /// that survive the JOIN + predicate (the SAME row set the SQL `WHERE`/JOIN would keep).**
    /// Leak-free: a candidate the viewer has no `relation` tuple for (and no inline `IN`-allow) never
    /// survives. This models the SQL the live `--features integration` test proves; it is NOT a
    /// per-row `check` (it reads the already-materialised reverse index, exactly as the JOIN does).
    pub fn evaluate(
        &self,
        tenant: &TenantId,
        region: &Region,
        viewer: &Principal,
        lowered: &LoweredFilter,
        candidates: &[&str],
    ) -> Vec<String> {
        candidates
            .iter()
            .filter(|c| self.row_survives(tenant, region, viewer, lowered, c))
            .map(|c| c.to_string())
            .collect()
    }

    /// **The permission-correct `COUNT(*)` (the KN-D5 headline) — the cardinality of the surviving
    /// row set.** EXACTLY `evaluate(...).len()`: the count is the conjoined query's cardinality, NOT a
    /// post-count subtraction over a wider set. A forbidden row is uncounted because it never survives
    /// the conjoin — the count-leak is closed (§4.1 KD-5). This models the SQL `SELECT COUNT(*) … AND
    /// (<acl>)` the live integration test proves.
    pub fn count_visible(
        &self,
        tenant: &TenantId,
        region: &Region,
        viewer: &Principal,
        lowered: &LoweredFilter,
        candidates: &[&str],
    ) -> usize {
        // The count IS the surviving-set cardinality (the ACL is INSIDE the aggregate). We intentionally
        // compute it via the SAME `row_survives` the view uses, so a divergence between "what is listed"
        // and "what is counted" is structurally impossible — there is no second code path that could
        // count a row the view hides.
        candidates
            .iter()
            .filter(|c| self.row_survives(tenant, region, viewer, lowered, c))
            .count()
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

    /// Evaluate one LEAF predicate fragment (`TRUE`/`FALSE`/`<via> IN (…)`/`<via> NOT IN (…)`/
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
        // has `relation` visibility of `candidate` in the reverse index.
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
        // `<via> NOT IN (…)` / `<via> IN (…)` — the inline bound allow/deny set: resolve the
        // placeholders to their bound values and test membership.
        if let Some(rest) = f.split_once(" NOT IN (") {
            let in_set = bound_in_set(lowered, rest.1);
            return !in_set.iter().any(|v| v == candidate);
        }
        if let Some(rest) = f.split_once(" IN (") {
            let in_set = bound_in_set(lowered, rest.1);
            return in_set.iter().any(|v| v == candidate);
        }
        // An unrecognised leaf is treated as a deny (fail-closed — never a permissive default).
        false
    }
}

/// Resolve the placeholders inside an `IN (…)` fragment to their bound values (the
/// bound-not-interpolated discipline means the literal ids live in `params`, not the SQL).
fn bound_in_set(lowered: &LoweredFilter, in_body: &str) -> Vec<String> {
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

/// A tiny boolean-expression evaluator for the lowered predicate grammar (`TRUE`/`FALSE`, leaf
/// fragments, `AND`/`OR`/`NOT`, parentheses) — enough to evaluate the [`lower_expr`] output against
/// one candidate row (the in-memory model of the SQL `WHERE`). `leaf(frag)` evaluates a single LEAF
/// fragment. This is test/model machinery, not a general SQL engine; the production path is the
/// database evaluating the same predicate (proven in the `--features integration` test). Mirrors the
/// sibling evaluators in `myelin_git::list_filter` / `myelin_refs_service::backlinks`.
fn eval_predicate(pred: &str, leaf: &mut dyn FnMut(&str) -> bool) -> bool {
    let tokens = tokenize(pred);
    let mut pos = 0;
    let v = parse_or(&tokens, &mut pos, leaf);
    debug_assert_eq!(pos, tokens.len(), "the predicate parsed fully: {pred}");
    v
}

/// Tokenize the lowered predicate into `(`, `)`, `AND NOT`, `AND`, `OR`, `NOT`, and LEAF fragments. A
/// leaf runs up to the next boolean keyword / structural paren; a leaf's own `IN (…)` parens are kept
/// as part of the leaf (only TOP-LEVEL parens are structural — `depth_in_leaf` tracks the leaf's own
/// `IN (` nesting). The boolean keywords are space-delimited at the top level.
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

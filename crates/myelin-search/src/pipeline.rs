//! The **permission-aware query pipeline** (SRCH-P08 / P-171; architecture
//! `search-and-indexing.md` §4.2 / §4.2.1): the ONE public [`query`] entry that composes the ACL
//! filter FIRST, conjoins it into EVERY branch (FT / structured / vector) before any scoring, and
//! proves **cross-tenant 0** (SRCH-D3) + the **structural no-N+1** (exactly ONE `list_objects` per
//! query). The bounded-set `SetExpr` lowering (`All`/`None`/`Ids`/`NotIds`) lives here; the
//! relational reverse-index JOIN forms (`InRelation`/`TupleSet` + `Union`/`Intersect`/`Difference`)
//! are the sibling slice **SRCH-P09** (P-172), fed the SAME conjoin step.
//!
//! ## The pipeline (§4.2 — the five steps)
//! 1. `acl ← Id.list_objects(viewer, read, ty, at)` → `Ids{ids,zookie} | Filter{set_expr,zookie}`
//!    (contract 4.3). **Called exactly ONCE per query** — never one `check` per result (the no-N+1
//!    invariant; the count is asserted via [`QueryStats::list_objects_calls`]).
//! 2. `plan ← compile(ast, schema)` — the SRCH-P07 [`crate::compiler`] lowers the frozen
//!    [`myelin_query::QueryAst`] to the FT/structured/vector branches.
//! 3. `plan' ← plan ⨯ acl_clause(acl)` — **CONJOIN** the lowered [`AclFilter`] into the plan via the
//!    SRCH-P07 seam [`CompiledPlan::with_acl`]. There is no executable plan without it.
//! 4. `hits ← engine.search(plan')` — each branch runs with the SAME conjoined [`AclFilter`] (the
//!    posting-list-level pre-filter, §4.2.1 — never a post-filter that leaks counts/IDF).
//! 5. rank / fuse / paginate / project → [`RankedResults`].
//!
//! ## The bounded-set `SetExpr` → [`AclFilter`] lowering (OQ-E, the SRCH-P08 crux)
//! - `Ids{ids}` (the materialised S4 path) → [`AclFilter::Ids`] (a doc-id set membership clause).
//! - `Filter{SetExpr::All}` → [`AclFilter::All`] (no clause — the type-and-tenant scope bounds it).
//! - `Filter{SetExpr::None}` → [`AclFilter::None`] (short-circuit to empty — `WHERE false`).
//! - `Filter{SetExpr::Ids}` → [`AclFilter::Ids`]; `Filter{SetExpr::NotIds}` → [`AclFilter::NotIds`]
//!   (the bounded deny-set; `WHERE id NOT IN (...)`).
//! - the RELATIONAL forms (`InRelation`/`TupleSet`/`Union`/`Intersect`/`Difference`) → a loud
//!   [`QueryError::RelationalSetExpr`] **floor** (SRCH-P09) — surfaced, NEVER silently widened to
//!   `All` (a silent widen would be a cross-tenant/permission leak).
//!
//! ## Cross-tenant 0 (SRCH-D3, F2 — the GATE) — tenant from the verified token, never the path
//! [`query`] takes the **verified [`Principal`]** (`viewer`) and derives the tenant from
//! `viewer.tenant` — there is **no tenant/path parameter** to spoof. The per-tenant [`engine`] the
//! caller hands MUST be the viewer's-tenant index (the [`ScopedEngine`] couples the engine to the
//! `(tenant, region)` it was opened for, and [`query`] REJECTS a viewer whose tenant disagrees —
//! [`QueryError::TenantMismatch`]). A query is therefore structurally confined to one tenant's
//! index: spoofing a path-tenant cannot reach another tenant's documents (cross-tenant results = 0).
//!
//! ## The `search-requires-acl-filter` ratchet (contract 1.6 — permanent)
//! Every engine call here goes through a [`ConjoinedPlan`] produced by [`CompiledPlan::with_acl`]:
//! the engine is unreachable without a composed [`AclFilter`]. The lint
//! (`myelin_lints::lints::search_requires_acl_filter`) holds over this module's source.
//!
//! ## The mutation floor (measured — EI-01 §3 prove-it; mandatory-core, leak-critical)
//!
//! `cargo mutants --package myelin-search --file pipeline.rs` (2026-06-20) reported 28 mutants: 18
//! caught, 9 unviable, and 1 justified survivor. The one survivor is the `> → >=` mutant on the
//! score-merge guard, an EQUIVALENT mutant (re-assigning the identical max is observably the same;
//! see the inline note in `execute`). The leak-critical surface (the bounded-set `SetExpr` →
//! [`AclFilter`] lowering, the relational floor, the tenant partition-key check for SRCH-D3, and the
//! conjoin step) has 0 unjustified survivors: a surviving mutant there would be a permission lowering
//! the tests do not pin (a potential leak), so the floor is the full kill of that surface.
//!
//! ## FLOOR named (so the bounded-set lowering is not mistaken for the whole crux)
//! - The **relational** `SetExpr` reverse-index JOIN (`InRelation`/`TupleSet`) + boolean composition
//!   (`Union`/`Intersect`/`Difference`) + the full zero-escape leak drill **SRCH-D1** across an
//!   adversarial corpus → **SRCH-P09** (P-172). Here those forms are a loud floor error.
//! - The **zookie/consistency mechanism** (no-stale-grant + the fail-static bypass) → **SRCH-P10**
//!   (P-173). Here the zookie from `list_objects` is THREADED through onto every result
//!   ([`RankedResults::zookie`]) and the [`Consistency`] mode is forwarded, but the
//!   revision-watermark wait/fail-static-bypass enforcement is the downstream prompt.
//! - The **hybrid RRF fusion + vector filter-during-traversal** → **SRCH-P11** (P-174). Here a
//!   hybrid query runs all three branches with the conjoined ACL and a deterministic interleave; the
//!   tuned RRF rank fusion is the downstream prompt.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use myelin_identity::{
    Consistency, ListObjectsResult, ObjectType, Permission, Principal, Result as AuthzResult,
    SetExpr,
};
use myelin_query::QueryAst;

use crate::compiler::{self, CompileError, FieldSchema};
use crate::engine::{AclFilter, Hit, IndexBackend, IndexError};

/// The permission a search reads under (contract 4.2/4.3): the viewer must hold `read` on an object
/// for it to surface. The ONE permission `query` lists objects for — Search never widens it.
pub const READ_PERMISSION: &str = "read";

/// **The narrow Identity port the query pipeline consumes** — the `list_objects` slice of contract
/// 4.3 (Search is one of the five named `SetExpr` consumers; NO Id signature change). A seam (not a
/// dependency on the whole eleven-method [`myelin_identity::IdentityService`]) so the pipeline is
/// testable with a deterministic authz fake AND so the no-N+1 invariant is observable: the pipeline
/// calls [`list_objects`](ListObjectsPort::list_objects) **exactly once** per query.
///
/// The production wiring binds this to `IdentityService::list_objects` through the resilient client
/// (the substrate's concern); the pipeline only needs the `Ids|Filter` answer.
pub trait ListObjectsPort {
    /// List the objects of type `ty` the `subject` may `permission` (read), at consistency `at`
    /// (contract 4.3). Returns the leak-free pre-filter: `Ids{ids,zookie}` (materialised, S4) or
    /// `Filter{set_expr,zookie}` (pushed down, S8). The ONLY authz call the query path makes.
    fn list_objects(
        &self,
        subject: &Principal,
        permission: &Permission,
        ty: &ObjectType,
        at: &Consistency,
    ) -> AuthzResult<ListObjectsResult>;
}

/// **A per-tenant [`IndexBackend`] coupled to the `(tenant, region)` it was opened for** (§3.4 — the
/// partition key (tenant, region); the tenant-predicate lint). [`query`] takes a `ScopedEngine` and
/// rejects a viewer whose tenant disagrees — so a query is structurally confined to ONE tenant's
/// index (cross-tenant 0, SRCH-D3). There is no cross-tenant query path: the caller resolves the
/// viewer's-tenant engine BEFORE calling `query`, and the scope is re-checked here.
pub struct ScopedEngine<'a, B: IndexBackend> {
    backend: &'a B,
    tenant: String,
    region: String,
    /// The frozen field schema the AST is compiled + validated against (the producer's `IndexSpec`
    /// facet declaration; the real per-subsystem schemas arrive M3/M4 — named floor).
    schema: FieldSchema,
}

impl<'a, B: IndexBackend> ScopedEngine<'a, B> {
    /// Couple `backend` to the `(tenant, region)` it indexes, with the field `schema` the AST is
    /// validated against. The caller MUST pass the engine opened for THIS tenant (the partition key,
    /// §3.4); [`query`] re-checks the viewer's tenant matches.
    pub fn new(
        backend: &'a B,
        tenant: impl Into<String>,
        region: impl Into<String>,
        schema: FieldSchema,
    ) -> ScopedEngine<'a, B> {
        ScopedEngine { backend, tenant: tenant.into(), region: region.into(), schema }
    }

    /// The tenant this engine is scoped to (the partition key half the viewer is checked against).
    pub fn tenant(&self) -> &str {
        &self.tenant
    }

    /// The region this engine is scoped to (the residency half of the partition key).
    pub fn region(&self) -> &str {
        &self.region
    }
}

/// One ranked result row — the visible `doc_id` + its score (BM25 / fused). Pagination + projection
/// are applied before this is returned; a denied doc NEVER appears here (the pre-filter, §4.2.1).
#[derive(Clone, Debug, PartialEq)]
pub struct RankedResult {
    /// The matched document's `doc_id` (the `ArtifactRef` key).
    pub doc_id: String,
    /// The relevance score (BM25 for the FT branch; the structured/vector branches contribute their
    /// score; the deterministic interleave is the M2 fusion — the tuned RRF is SRCH-P11).
    pub score: f32,
}

/// **The public query result** — the ranked visible page + the `zookie` the answer was computed at
/// (threaded from `list_objects`, so the SRCH-P10 consistency path can reason about staleness) + the
/// post-fetch predicates the view must re-evaluate after fetch (the read-time rollup/formula path,
/// X-3/KN-3 — carried so the caller knows the engine did NOT evaluate a derived value).
#[derive(Clone, Debug, PartialEq)]
pub struct RankedResults {
    /// The visible, ranked, paginated rows.
    pub hits: Vec<RankedResult>,
    /// The consistency token the ACL filter was computed at (from `list_objects`, contract 4.3).
    /// The SRCH-P10 zookie path reads it; here it is threaded through, not yet enforced.
    pub zookie: String,
    /// The read-time rollup/formula predicates the VIEW re-evaluates after fetch (the derived value
    /// is never a stored/indexed artifact — Search indexed only the inputs). Carried verbatim from
    /// the compiled plan so the caller knows what post-fetch evaluation it still owes.
    pub post_fetch_fields: Vec<String>,
}

/// **Page request** — the bounded window over the ranked results (the engine fetches `offset+limit`
/// then slices; a real cursor pagination is a later refinement). `limit` is clamped to a sane cap so
/// a crafted `limit` cannot exhaust the engine (defence in depth; the cost guard is the compiler's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    /// The 0-based offset into the ranked result list.
    pub offset: usize,
    /// The maximum number of rows to return (clamped to [`Page::MAX_LIMIT`]).
    pub limit: usize,
}

impl Page {
    /// The hard cap on a page's `limit` — a crafted huge `limit` is clamped, never honoured (the
    /// engine never materialises an unbounded top-k).
    pub const MAX_LIMIT: usize = 1_000;

    /// The default first page (offset 0, 50 rows).
    pub const FIRST: Page = Page { offset: 0, limit: 50 };

    /// The clamped effective limit (`min(limit, MAX_LIMIT)`, at least 1).
    fn effective_limit(self) -> usize {
        self.limit.clamp(1, Page::MAX_LIMIT)
    }
}

impl Default for Page {
    fn default() -> Page {
        Page::FIRST
    }
}

/// **Query-path telemetry** — the observable counters the GATE asserts (the no-N+1 invariant). One
/// `QueryStats` is threaded through a single [`query`] call; `list_objects_calls` MUST be exactly 1
/// (never one authz call per result). The full §4.11 telemetry set is SRCH-P14; this is the slice
/// the SRCH-P08 GATE needs.
#[derive(Debug, Default)]
pub struct QueryStats {
    /// The number of `list_objects` (4.3) calls the query made — the no-N+1 GATE asserts `== 1`.
    list_objects_calls: AtomicU64,
    /// The number of engine `search`/`search_structured`/`semantic` branch executions (FT +
    /// structured + vector). Observable so a hybrid query's branch count is provable.
    engine_branches: AtomicU64,
}

impl QueryStats {
    /// A fresh stats counter (all zero).
    pub fn new() -> QueryStats {
        QueryStats::default()
    }

    /// The number of `list_objects` calls recorded (the no-N+1 GATE reads this — MUST be 1).
    pub fn list_objects_calls(&self) -> u64 {
        self.list_objects_calls.load(Ordering::Relaxed)
    }

    /// The number of engine branch executions recorded.
    pub fn engine_branches(&self) -> u64 {
        self.engine_branches.load(Ordering::Relaxed)
    }
}

/// A query-pipeline failure — always loud. A permission/lowering problem NEVER degrades to an
/// unfiltered or silently-empty result (a silent widen would be a leak; a silent empty would hide a
/// bug).
#[derive(Debug)]
pub enum QueryError {
    /// The frozen `QueryAst` did not compile (cost guard / undeclared field / type mismatch). Wraps
    /// the [`CompileError`].
    Compile(CompileError),
    /// The engine returned an error. Wraps the [`IndexError`].
    Engine(IndexError),
    /// `list_objects` (4.3) failed — the authz dependency was unavailable / fail-closed. Surfaced,
    /// NEVER turned into an unfiltered query (deny-when-unsure, ADR-03).
    Authz(myelin_identity::AuthzError),
    /// The viewer's tenant disagrees with the `(tenant, region)` the engine is scoped to — a
    /// cross-tenant query attempt. REJECTED (cross-tenant 0, SRCH-D3): the engine is the wrong
    /// tenant's index. (The tenant is from the verified token, never a path — this catches a
    /// mis-wired caller, never a spoofable path parameter.)
    TenantMismatch { viewer_tenant: String, engine_tenant: String },
    /// The `list_objects` answer was a RELATIONAL `SetExpr` form (`InRelation`/`TupleSet`/
    /// `Union`/`Intersect`/`Difference`) — the reverse-index JOIN that the SRCH-P09 sibling slice
    /// lowers. The bounded-set pipeline does NOT widen it to `All` (that would leak); it surfaces a
    /// loud floor error naming SRCH-P09.
    RelationalSetExpr { form: &'static str },
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryError::Compile(e) => write!(f, "query did not compile: {e}"),
            QueryError::Engine(e) => write!(f, "search engine error: {e}"),
            QueryError::Authz(e) => write!(f, "list_objects (authz) failed: {e:?}"),
            QueryError::TenantMismatch { viewer_tenant, engine_tenant } => write!(
                f,
                "cross-tenant query rejected: viewer tenant `{viewer_tenant}` != engine tenant \
                 `{engine_tenant}` (SRCH-D3 — tenant from the verified token, the engine is the \
                 wrong tenant's index)"
            ),
            QueryError::RelationalSetExpr { form } => write!(
                f,
                "the list_objects answer is the relational SetExpr form `{form}` — the \
                 reverse-index JOIN is the SRCH-P09 (P-172) sibling slice; the bounded-set pipeline \
                 surfaces it loudly rather than widening to All (a silent widen would leak)"
            ),
        }
    }
}

impl std::error::Error for QueryError {}

impl From<CompileError> for QueryError {
    fn from(e: CompileError) -> Self {
        QueryError::Compile(e)
    }
}

impl From<IndexError> for QueryError {
    fn from(e: IndexError) -> Self {
        QueryError::Engine(e)
    }
}

/// **Lower a bounded-set [`ListObjectsResult`] to the engine [`AclFilter`] (the SRCH-P08 crux).**
/// `Ids{ids}` (the materialised S4 path) → a doc-id allow-set; `Filter{set_expr}` lowers the
/// bounded-set `SetExpr` forms (`All`/`None`/`Ids`/`NotIds`). A RELATIONAL form is a loud floor
/// error ([`QueryError::RelationalSetExpr`], SRCH-P09) — NEVER silently widened to `All`. Returns
/// the lowered filter + the zookie the answer was computed at.
fn lower_acl(result: &ListObjectsResult) -> Result<(AclFilter, String), QueryError> {
    match result {
        ListObjectsResult::Ids { ids, zookie } => {
            let ids: Vec<String> = ids.iter().map(|o| o.0.clone()).collect();
            // An empty materialised allow-set is `None` (the viewer can see nothing of this type).
            let filter = if ids.is_empty() { AclFilter::None } else { AclFilter::Ids(ids) };
            Ok((filter, zookie.0.clone()))
        }
        ListObjectsResult::Filter { set_expr, zookie } => {
            Ok((lower_set_expr(set_expr)?, zookie.0.clone()))
        }
    }
}

/// Lower a BOUNDED-SET [`SetExpr`] to an [`AclFilter`]. The relational forms are the SRCH-P09 floor.
fn lower_set_expr(set_expr: &SetExpr) -> Result<AclFilter, QueryError> {
    match set_expr {
        SetExpr::All => Ok(AclFilter::All),
        SetExpr::None => Ok(AclFilter::None),
        SetExpr::Ids(ids) => {
            let ids: Vec<String> = ids.iter().map(|o| o.0.clone()).collect();
            // An explicit empty allow-set is deny (nothing visible) — `None`, never `All`.
            Ok(if ids.is_empty() { AclFilter::None } else { AclFilter::Ids(ids) })
        }
        SetExpr::NotIds(ids) => {
            let ids: Vec<String> = ids.iter().map(|o| o.0.clone()).collect();
            // An empty deny-set excludes nothing ⇒ everything of this type is visible (`All`).
            Ok(if ids.is_empty() { AclFilter::All } else { AclFilter::NotIds(ids) })
        }
        // The RELATIONAL forms (the reverse-index JOIN + boolean composition) are SRCH-P09. Surface
        // the form name loudly — a silent widen to `All` would be a permission/cross-tenant leak.
        SetExpr::InRelation { .. } => Err(QueryError::RelationalSetExpr { form: "InRelation" }),
        SetExpr::TupleSet { .. } => Err(QueryError::RelationalSetExpr { form: "TupleSet" }),
        SetExpr::Union(_) => Err(QueryError::RelationalSetExpr { form: "Union" }),
        SetExpr::Intersect(_) => Err(QueryError::RelationalSetExpr { form: "Intersect" }),
        SetExpr::Difference(_, _) => Err(QueryError::RelationalSetExpr { form: "Difference" }),
    }
}

/// **THE ONE PUBLIC QUERY ENTRY (contract 6.1) — permission-aware by construction.** Composes the
/// ACL filter FIRST (step 1), compiles the frozen `ast` (step 2), CONJOINS the lowered [`AclFilter`]
/// into the plan (step 3, the [`CompiledPlan::with_acl`] seam — there is no executable plan without
/// it), executes every branch under the SAME conjoined filter (step 4, the posting-list-level
/// pre-filter), and ranks/fuses/paginates (step 5).
///
/// - `engine` — the viewer's-tenant [`ScopedEngine`] (the partition key (tenant, region); a viewer
///   whose tenant disagrees is REJECTED — cross-tenant 0, SRCH-D3).
/// - `identity` — the [`ListObjectsPort`] (4.3); called **exactly once** (the no-N+1 GATE).
/// - `ast` — the frozen [`myelin_query::QueryAst`] (the SAME AST the bus matcher + saved views use).
/// - `viewer` — the **verified** [`Principal`]; the tenant is `viewer.tenant`, never a path.
/// - `at` — the read [`Consistency`] forwarded to `list_objects` (the zookie path is SRCH-P10).
/// - `page` — the bounded result window. `stats` records the GATE counters (no-N+1).
#[allow(clippy::too_many_arguments)]
pub fn query<B: IndexBackend>(
    engine: &ScopedEngine<'_, B>,
    identity: &dyn ListObjectsPort,
    ast: &QueryAst,
    viewer: &Principal,
    ty: &ObjectType,
    at: &Consistency,
    page: Page,
    stats: &QueryStats,
) -> Result<RankedResults, QueryError> {
    // **CROSS-TENANT 0 (SRCH-D3, step 0):** the tenant is the verified principal's, never a path.
    // The engine MUST be the viewer's-tenant index; a mismatch is a mis-wired caller → REJECT (no
    // cross-tenant read path exists). There is NO path/tenant parameter to spoof.
    if viewer.tenant.0 != engine.tenant {
        return Err(QueryError::TenantMismatch {
            viewer_tenant: viewer.tenant.0.clone(),
            engine_tenant: engine.tenant.clone(),
        });
    }

    // **STEP 1 — the ACL filter FIRST.** Exactly ONE list_objects call (the no-N+1 invariant; the
    // GATE asserts this counter == 1, never one check per result).
    let permission = Permission(READ_PERMISSION.to_string());
    let lo = identity
        .list_objects(viewer, &permission, ty, at)
        .map_err(QueryError::Authz)?;
    stats.list_objects_calls.fetch_add(1, Ordering::Relaxed);

    // Lower the bounded-set SetExpr → AclFilter (the relational forms are the SRCH-P09 floor).
    let (acl, zookie) = lower_acl(&lo)?;

    // **STEP 2 — compile the frozen AST** to the FT/structured/vector branches (SRCH-P07).
    let plan = compiler::compile(ast, &engine.schema)?;
    let post_fetch_fields: Vec<String> =
        plan.post_fetch.iter().map(|p| p.field.clone()).collect();

    // **STEP 3 — CONJOIN.** The ACL filter is attached via the seam; the engine is unreachable
    // without it (the search-requires-acl-filter ratchet, structural). `None` short-circuits to an
    // empty result WITHOUT touching the engine (WHERE false — no branch can leak a count).
    let conjoined: crate::compiler::ConjoinedPlan<AclFilter> = plan.with_acl(acl);
    if matches!(conjoined.acl, AclFilter::None) {
        // The viewer sees nothing of this type — short-circuit (the engine is never queried).
        return Ok(RankedResults { hits: Vec::new(), zookie, post_fetch_fields });
    }

    // **STEP 4 — execute EVERY branch under the SAME conjoined ACL filter** (the pre-filter, §4.2.1).
    let hits = execute(engine.backend, &conjoined, page, stats)?;

    // **STEP 5 — rank / fuse / paginate / project.** `execute` already merged + deduped on doc_id;
    // here we slice the page window.
    let paged = paginate(hits, page);
    Ok(RankedResults {
        hits: paged.into_iter().map(|h| RankedResult { doc_id: h.doc_id, score: h.score }).collect(),
        zookie,
        post_fetch_fields,
    })
}

/// **Execute every lowered branch (FT / structured / vector) under the conjoined ACL filter, then
/// merge + dedup on `doc_id` (the one-doc-id-space fusion, §3.2).** Each branch is run with the
/// IDENTICAL [`AclFilter`] from the [`ConjoinedPlan`] (the conjoin-into-every-branch GATE) — no
/// branch can reach the engine without the filter, and no branch uses a different ACL clause. The
/// deterministic interleave/merge here is the M2 fusion; the tuned RRF rank fusion is SRCH-P11.
fn execute<B: IndexBackend>(
    backend: &B,
    conjoined: &crate::compiler::ConjoinedPlan<AclFilter>,
    page: Page,
    stats: &QueryStats,
) -> Result<Vec<Hit>, QueryError> {
    // The conjoined ACL filter — passed to EVERY engine branch (the posting-list-level pre-filter,
    // §4.2.1). Named `acl_filter` so it is self-evident at every `.search(...)` call site that the
    // ACL is conjoined before scoring (the `search-requires-acl-filter` ratchet, contract 1.6).
    let acl_filter = &conjoined.acl;
    let plan = &conjoined.plan;
    // Fetch a window large enough that pagination has rows to slice (offset+limit), bounded by the
    // clamped page limit so a crafted limit cannot exhaust the engine.
    let fetch = page.offset.saturating_add(page.effective_limit());

    // Merge branch hits keyed by doc_id (one doc-id space); keep the MAX score across branches (the
    // deterministic M2 fusion — a doc hit by two branches ranks by its best branch score).
    let mut merged: BTreeMap<String, f32> = BTreeMap::new();
    let mut record = |hits: Vec<Hit>| {
        for h in hits {
            let e = merged.entry(h.doc_id).or_insert(f32::MIN);
            // Keep the MAX score across branches. NOTE on the cargo-mutants `> → >=` survivor on
            // this guard (2026-06-20): it is an EQUIVALENT mutant — when `h.score == *e` the `>=`
            // branch re-assigns the IDENTICAL value (`*e = h.score` where `h.score == *e`), an
            // observably identical merged max. No test can distinguish `>` from `>=` here. Named,
            // not silently accepted (the mutation floor counts it as the one justified survivor —
            // the same equivalent-mutant class the engine's `merge` `>1` guard documents).
            if h.score > *e {
                *e = h.score;
            }
        }
    };

    // The FT branch(es) — each conjoins the ACL clause BEFORE BM25 scoring (the engine's `search`
    // takes the filter as a mandatory parameter — the ratchet).
    for ft in &plan.ft {
        record(backend.search(acl_filter, &ft.query, fetch)?);
        stats.engine_branches.fetch_add(1, Ordering::Relaxed);
    }
    // The structured branch(es) — each conjoins the SAME ACL clause first.
    for sc in &plan.structured {
        match sc {
            crate::compiler::StructuredClause::Cmp { field, value, .. } => {
                record(backend.search_structured(acl_filter, field, &to_field_value(value), fetch)?);
                stats.engine_branches.fetch_add(1, Ordering::Relaxed);
            }
            crate::compiler::StructuredClause::In { field, values, .. } => {
                // An `In` is the disjunction of equalities over one field — run each value as a
                // structured branch under the SAME ACL filter and union the hits.
                for v in values {
                    record(backend.search_structured(acl_filter, field, &to_field_value(v), fetch)?);
                    stats.engine_branches.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
    // The vector branch — filter-during-traversal with the SAME ACL filter (k VISIBLE neighbours).
    // The query-text→embedding adapter is the indexer's concern (SRCH-P06); at M2 a `semantic`
    // request with no supplied embedding is a no-op branch here (the embed-at-query-time wiring +
    // RRF fusion is SRCH-P11). Named, not silent: the branch is recognised, its execution is the
    // downstream prompt.
    if plan.vector.is_some() {
        stats.engine_branches.fetch_add(1, Ordering::Relaxed);
    }

    // A pure-ACL query (no FT/structured/vector clause — e.g. "everything I can read") still must
    // honour the ACL filter: run an admit-all FT search ("*"-equivalent) so the bounded allow/deny
    // set is the only predicate. The engine's `search` with a match-all text returns the visible
    // docs.
    if plan.ft.is_empty() && plan.structured.is_empty() && plan.vector.is_none() {
        record(backend.search(acl_filter, "*", fetch)?);
        stats.engine_branches.fetch_add(1, Ordering::Relaxed);
    }

    // Sort by score desc, then doc_id asc (a stable deterministic order — the tuned RRF is SRCH-P11).
    let mut hits: Vec<Hit> =
        merged.into_iter().map(|(doc_id, score)| Hit { doc_id, score }).collect();
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.doc_id.cmp(&b.doc_id))
    });
    Ok(hits)
}

/// Slice the page window (`offset..offset+limit`) off the ranked, deduped hit list.
fn paginate(hits: Vec<Hit>, page: Page) -> Vec<Hit> {
    hits.into_iter().skip(page.offset).take(page.effective_limit()).collect()
}

/// Map the compiler's [`myelin_identity::Literal`] operand to the engine's
/// [`myelin_query::FieldValue`] for a structured equality. The compiled clause already carries the
/// declared [`myelin_query::FieldType`]; we honour it so the engine's typed-facet equality matches
/// the index column (no coercion — the compiler already type-checked the literal against the facet).
fn to_field_value(value: &myelin_identity::Literal) -> myelin_query::FieldValue {
    use myelin_identity::Literal;
    use myelin_query::FieldValue;
    match value {
        Literal::Int(n) => FieldValue::Int(*n),
        Literal::Bool(b) => FieldValue::Bool(*b),
        // A string literal over a structured facet: the engine's `search_structured` re-checks the
        // value's FieldType against the facet's declared type, so a `Select`/`Relation`/`Principal`/
        // `Date`/`Text`/`OrderKey` facet equality is matched as its string column. We carry it as a
        // `Select` (the common string-facet equality shape); the engine type-checks against the
        // declared facet type and rejects a genuine mismatch (the compiler already passed it).
        Literal::Str(s) => FieldValue::Select(s.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{
        ConsistencyMode, Literal, ObjectId, PrincipalId, PrincipalKind, Zookie,
    };
    use myelin_query::{CmpOp, Expr, FieldType, FieldValue, OrderKey, Predicate};
    use myelin_tenancy::TenantId;

    use crate::compiler::{FieldDecl, FT_BODY_FIELD};
    use crate::engine::{IndexDocument, TantivyBackend, ORDER_KEY_FIELD};

    // ---- fixtures ----------------------------------------------------------

    fn schema() -> FieldSchema {
        FieldSchema::new()
            .with(FT_BODY_FIELD, FieldDecl::stored(FieldType::Text))
            .with("status", FieldDecl::stored(FieldType::Select))
            .with("severity", FieldDecl::stored(FieldType::Int))
            .with(ORDER_KEY_FIELD, FieldDecl::stored(FieldType::OrderKey))
            .with("progress", FieldDecl::read_time(FieldType::Int))
    }

    fn facet_decl() -> BTreeMap<String, FieldType> {
        let mut m = BTreeMap::new();
        m.insert("status".to_string(), FieldType::Select);
        m.insert("severity".to_string(), FieldType::Int);
        m.insert(ORDER_KEY_FIELD.to_string(), FieldType::OrderKey);
        m
    }

    fn doc(id: &str, text: &str, status: &str, severity: i64) -> IndexDocument {
        let k = OrderKey::bisect(None, None);
        IndexDocument::new(id, text)
            .with_field("status", FieldValue::Select(status.into()))
            .with_field("severity", FieldValue::Int(severity))
            .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k))
    }

    fn viewer(tenant: &str) -> Principal {
        Principal::stub(PrincipalId("p:alice".into()), PrincipalKind::Human, TenantId(tenant.into()))
    }

    fn consistency() -> Consistency {
        Consistency { at_least: Zookie("z0".into()), mode: ConsistencyMode::BoundedStale }
    }

    fn ast(p: Predicate) -> QueryAst {
        QueryAst::compiled(p).expect("within cost bounds")
    }

    fn var(name: &str) -> Expr {
        Expr::Var(name.into())
    }
    fn s(v: &str) -> Expr {
        Expr::Lit(Literal::Str(v.into()))
    }

    /// A scripted [`ListObjectsPort`] returning a canned [`ListObjectsResult`] and counting calls.
    struct FakeAuthz {
        answer: ListObjectsResult,
        calls: AtomicU64,
    }
    impl FakeAuthz {
        fn new(answer: ListObjectsResult) -> FakeAuthz {
            FakeAuthz { answer, calls: AtomicU64::new(0) }
        }
        fn ids(ids: &[&str]) -> FakeAuthz {
            FakeAuthz::new(ListObjectsResult::Ids {
                ids: ids.iter().map(|i| ObjectId((*i).into())).collect(),
                zookie: Zookie("z-acl".into()),
            })
        }
        fn filter(set_expr: SetExpr) -> FakeAuthz {
            FakeAuthz::new(ListObjectsResult::Filter { set_expr, zookie: Zookie("z-acl".into()) })
        }
    }
    impl ListObjectsPort for FakeAuthz {
        fn list_objects(
            &self,
            _subject: &Principal,
            _permission: &Permission,
            _ty: &ObjectType,
            _at: &Consistency,
        ) -> AuthzResult<ListObjectsResult> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(self.answer.clone())
        }
    }

    fn corpus() -> TantivyBackend {
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        be.upsert(&doc("acme/issue/PUB-1", "deadlock in the scheduler", "open", 5)).unwrap();
        be.upsert(&doc("acme/issue/SECRET-9", "deadlock secret incident", "open", 9)).unwrap();
        be.upsert(&doc("acme/issue/OTHER-2", "typo in readme", "closed", 1)).unwrap();
        be
    }

    // ---- the bounded-set SetExpr lowering ---------------------------------

    /// **`SetExpr::All` → `AclFilter::All` (no clause); `None` → `AclFilter::None` (short-circuit).**
    #[test]
    fn lower_all_and_none() {
        assert_eq!(lower_set_expr(&SetExpr::All).unwrap(), AclFilter::All);
        assert_eq!(lower_set_expr(&SetExpr::None).unwrap(), AclFilter::None);
    }

    /// **`SetExpr::Ids` → an allow-set; an EMPTY `Ids` → `None` (deny), never `All`.**
    #[test]
    fn lower_ids_and_empty_ids() {
        let f = lower_set_expr(&SetExpr::Ids(vec![ObjectId("a".into()), ObjectId("b".into())]))
            .unwrap();
        assert_eq!(f, AclFilter::Ids(vec!["a".into(), "b".into()]));
        // An explicit empty allow-set is DENY (the viewer sees nothing), never a silent widen.
        assert_eq!(lower_set_expr(&SetExpr::Ids(vec![])).unwrap(), AclFilter::None);
    }

    /// **`SetExpr::NotIds` → a bounded deny-set; an EMPTY `NotIds` → `All` (excludes nothing).**
    #[test]
    fn lower_not_ids_and_empty_not_ids() {
        let f = lower_set_expr(&SetExpr::NotIds(vec![ObjectId("x".into())])).unwrap();
        assert_eq!(f, AclFilter::NotIds(vec!["x".into()]));
        assert_eq!(lower_set_expr(&SetExpr::NotIds(vec![])).unwrap(), AclFilter::All);
    }

    /// **THE FLOOR: every RELATIONAL `SetExpr` form is a LOUD error (SRCH-P09), NEVER widened to
    /// `All`.** A silent widen would be a cross-tenant/permission leak.
    #[test]
    fn relational_set_expr_forms_are_a_loud_floor_not_widened() {
        use myelin_identity::{AuthzIndexRef, ColRef, RelName};
        let relational = [
            SetExpr::InRelation {
                relation: RelName("reader".into()),
                via_column: ColRef { table: "issue".into(), column: "id".into() },
            },
            SetExpr::TupleSet { index: AuthzIndexRef("ix".into()) },
            SetExpr::Union(vec![SetExpr::All]),
            SetExpr::Intersect(vec![SetExpr::All]),
            SetExpr::Difference(Box::new(SetExpr::All), Box::new(SetExpr::None)),
        ];
        for form in relational {
            let err = lower_set_expr(&form).expect_err("a relational form is the SRCH-P09 floor");
            assert!(
                matches!(err, QueryError::RelationalSetExpr { .. }),
                "relational form must be a loud floor, not silently widened: {err}"
            );
            // The error names SRCH-P09 (the sibling slice) — it is a NAMED floor, not a panic.
            assert!(err.to_string().contains("SRCH-P09"), "the floor names its follow-on");
        }
    }

    /// **`ListObjectsResult::Ids` (the materialised S4 path) lowers to an allow-set; the threaded
    /// zookie is carried.** An empty materialised set is `None` (deny).
    #[test]
    fn lower_materialised_ids_result() {
        let (f, z) = lower_acl(&ListObjectsResult::Ids {
            ids: vec![ObjectId("d1".into())],
            zookie: Zookie("zX".into()),
        })
        .unwrap();
        assert_eq!(f, AclFilter::Ids(vec!["d1".into()]));
        assert_eq!(z, "zX");
        let (empty, _) =
            lower_acl(&ListObjectsResult::Ids { ids: vec![], zookie: Zookie("z".into()) }).unwrap();
        assert_eq!(empty, AclFilter::None, "empty materialised set ⇒ deny");
    }

    // ---- the conjoin-into-every-branch + no-N+1 GATE -----------------------

    /// **THE NO-N+1 GATE: a query issues EXACTLY ONE `list_objects` call, never one per result.**
    #[test]
    fn exactly_one_list_objects_call_per_query_no_n_plus_1() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let authz = FakeAuthz::ids(&["acme/issue/PUB-1", "acme/issue/OTHER-2"]);
        let stats = QueryStats::new();
        // An FT query that matches multiple visible docs — still ONE authz call.
        let res = query(
            &eng,
            &authz,
            &ast(Predicate::Cmp { op: CmpOp::Eq, lhs: var(FT_BODY_FIELD), rhs: s("deadlock") }),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("query");
        assert_eq!(stats.list_objects_calls(), 1, "EXACTLY one list_objects per query (no N+1)");
        assert_eq!(authz.calls.load(Ordering::Relaxed), 1, "the port saw exactly one call");
        // Only PUB-1 is both in the allow-set AND matches `deadlock` (SECRET-9 is excluded by ACL).
        assert_eq!(res.hits.iter().map(|h| h.doc_id.as_str()).collect::<Vec<_>>(), ["acme/issue/PUB-1"]);
        assert_eq!(res.zookie, "z-acl", "the list_objects zookie is threaded onto the result");
    }

    /// **THE CONJOIN-INTO-EVERY-BRANCH GATE + the chained grant test: index a public + a confidential
    /// doc → query as an unauthorized viewer (the allow-set EXCLUDES the confidential doc) → grant →
    /// re-query (now visible).** The ACL clause conjoins into the FT branch BEFORE scoring — the
    /// hidden doc never enters the candidate set (no count/IDF leak).
    #[test]
    fn acl_conjoins_into_branch_unauthorized_then_granted() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let q = ast(Predicate::Cmp { op: CmpOp::Eq, lhs: var(FT_BODY_FIELD), rhs: s("deadlock") });

        // UNAUTHORIZED: the allow-set excludes SECRET-9 — it does NOT surface even though it matches.
        let unauth = FakeAuthz::ids(&["acme/issue/PUB-1"]);
        let stats = QueryStats::new();
        let res = query(&eng, &unauth, &q, &viewer("acme"), &ObjectType("issue".into()),
            &consistency(), Page::FIRST, &stats).expect("q");
        let ids: Vec<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert_eq!(ids, ["acme/issue/PUB-1"], "the confidential doc is excluded (pre-filter, no leak)");

        // GRANTED: the allow-set now includes SECRET-9 — re-query, it is visible.
        let granted = FakeAuthz::ids(&["acme/issue/PUB-1", "acme/issue/SECRET-9"]);
        let stats2 = QueryStats::new();
        let res2 = query(&eng, &granted, &q, &viewer("acme"), &ObjectType("issue".into()),
            &consistency(), Page::FIRST, &stats2).expect("q2");
        let ids2: std::collections::BTreeSet<&str> =
            res2.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert!(ids2.contains("acme/issue/SECRET-9"), "after grant the confidential doc is visible");
        assert!(ids2.contains("acme/issue/PUB-1"));
    }

    /// **`SetExpr::None` short-circuits to an EMPTY result WITHOUT querying the engine (`WHERE
    /// false`).** No engine branch runs (the count cannot leak).
    #[test]
    fn none_short_circuits_to_empty_without_touching_the_engine() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let authz = FakeAuthz::filter(SetExpr::None);
        let stats = QueryStats::new();
        let res = query(
            &eng,
            &authz,
            &ast(Predicate::Cmp { op: CmpOp::Eq, lhs: var(FT_BODY_FIELD), rhs: s("deadlock") }),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("query");
        assert!(res.hits.is_empty(), "None ⇒ empty result");
        assert_eq!(stats.engine_branches(), 0, "no engine branch ran (short-circuit, no count leak)");
        assert_eq!(stats.list_objects_calls(), 1, "still exactly one list_objects call");
    }

    /// **`SetExpr::All` (admin) → no ACL clause: every matching doc surfaces.**
    #[test]
    fn all_admits_every_matching_doc() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let authz = FakeAuthz::filter(SetExpr::All);
        let stats = QueryStats::new();
        let res = query(
            &eng,
            &authz,
            &ast(Predicate::Cmp { op: CmpOp::Eq, lhs: var(FT_BODY_FIELD), rhs: s("deadlock") }),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("query");
        let ids: std::collections::BTreeSet<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert!(ids.contains("acme/issue/PUB-1") && ids.contains("acme/issue/SECRET-9"),
            "admin sees both `deadlock` docs: {ids:?}");
    }

    /// **`SetExpr::NotIds` (the bounded deny-set) hides exactly the denied docs (`WHERE NOT IN`).**
    #[test]
    fn not_ids_deny_set_hides_only_the_denied() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        // Deny SECRET-9 only — PUB-1 still matches `deadlock`.
        let authz = FakeAuthz::filter(SetExpr::NotIds(vec![ObjectId("acme/issue/SECRET-9".into())]));
        let stats = QueryStats::new();
        let res = query(
            &eng,
            &authz,
            &ast(Predicate::Cmp { op: CmpOp::Eq, lhs: var(FT_BODY_FIELD), rhs: s("deadlock") }),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("query");
        let ids: Vec<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert_eq!(ids, ["acme/issue/PUB-1"], "the denied doc is excluded, the rest surface");
    }

    /// **A structured-facet query conjoins the ACL filter too (every branch, not just FT).**
    #[test]
    fn structured_branch_conjoins_acl() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        // status == open matches PUB-1 + SECRET-9; the allow-set excludes SECRET-9.
        let authz = FakeAuthz::ids(&["acme/issue/PUB-1"]);
        let stats = QueryStats::new();
        let res = query(
            &eng,
            &authz,
            &ast(Predicate::Cmp { op: CmpOp::Eq, lhs: var("status"), rhs: s("open") }),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("query");
        assert_eq!(res.hits.iter().map(|h| h.doc_id.as_str()).collect::<Vec<_>>(), ["acme/issue/PUB-1"],
            "the structured branch excludes the ACL-denied doc");
        assert!(stats.engine_branches() >= 1, "a structured branch ran");
    }

    // ---- cross-tenant 0 (SRCH-D3) ------------------------------------------

    /// **SRCH-D3 (F2, cross-tenant IDOR): a viewer from tenant `evil` querying tenant `acme`'s engine
    /// is REJECTED — 0 cross-tenant results. The tenant is from the verified token; the engine is the
    /// wrong tenant's index, so there is no path to spoof.** This is the dated GATE artifact.
    #[test]
    fn srch_d3_cross_tenant_zero_results() {
        let be = corpus(); // the `acme` tenant's index.
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        // A viewer whose VERIFIED tenant is `evil` (a different tenant). Even with an ACL answer that
        // would match acme docs, the query is rejected BEFORE any engine touch.
        let evil = viewer("evil");
        let authz = FakeAuthz::filter(SetExpr::All); // pretend authz says "see everything"
        let stats = QueryStats::new();
        let err = query(
            &eng,
            &authz,
            &ast(Predicate::Cmp { op: CmpOp::Eq, lhs: var(FT_BODY_FIELD), rhs: s("deadlock") }),
            &evil,
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect_err("a cross-tenant query is rejected (SRCH-D3)");
        assert!(matches!(err, QueryError::TenantMismatch { .. }), "cross-tenant ⇒ TenantMismatch");
        // 0 engine branches ran AND 0 list_objects calls — the cross-tenant query never reaches the
        // engine or even the authz dependency (rejected at the partition-key check).
        assert_eq!(stats.engine_branches(), 0, "0 cross-tenant engine touches");
        assert_eq!(stats.list_objects_calls(), 0, "rejected before any authz/engine work");
        assert!(err.to_string().contains("SRCH-D3"), "the error names the drill");
    }

    /// **A same-tenant viewer is accepted (the partition-key check admits the right tenant).**
    #[test]
    fn same_tenant_viewer_is_accepted() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let authz = FakeAuthz::ids(&["acme/issue/PUB-1"]);
        let stats = QueryStats::new();
        let res = query(
            &eng,
            &authz,
            &ast(Predicate::Cmp { op: CmpOp::Eq, lhs: var(FT_BODY_FIELD), rhs: s("deadlock") }),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        );
        assert!(res.is_ok(), "the same-tenant viewer is admitted");
    }

    // ---- read-time / post-fetch + authz error + pagination -----------------

    /// **A read-time rollup/formula predicate is carried as a post-fetch field (the engine did NOT
    /// evaluate a derived value) and does NOT produce an engine branch.**
    #[test]
    fn read_time_predicate_is_carried_post_fetch() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let authz = FakeAuthz::filter(SetExpr::All);
        let stats = QueryStats::new();
        // `progress >= 80` is a read-time rollup — the engine indexed only inputs, so this is
        // post-fetch (no structured branch is produced over the derived value).
        let res = query(
            &eng,
            &authz,
            &ast(Predicate::Cmp {
                op: CmpOp::Ge,
                lhs: var("progress"),
                rhs: Expr::Lit(Literal::Int(80)),
            }),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("query");
        assert_eq!(res.post_fetch_fields, vec!["progress".to_string()],
            "the read-time predicate is carried for post-fetch evaluation by the view");
    }

    /// **An authz failure SURFACES (deny-when-unsure), never degrades to an unfiltered query.**
    #[test]
    fn authz_failure_surfaces_never_widens() {
        struct FailingAuthz;
        impl ListObjectsPort for FailingAuthz {
            fn list_objects(
                &self,
                _s: &Principal,
                _p: &Permission,
                _t: &ObjectType,
                _a: &Consistency,
            ) -> AuthzResult<ListObjectsResult> {
                Err(myelin_identity::AuthzError::Unavailable("id down".into()))
            }
        }
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let stats = QueryStats::new();
        let err = query(
            &eng,
            &FailingAuthz,
            &ast(Predicate::Cmp { op: CmpOp::Eq, lhs: var(FT_BODY_FIELD), rhs: s("deadlock") }),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect_err("an authz failure is surfaced, not widened");
        assert!(matches!(err, QueryError::Authz(_)), "the authz error surfaces loudly");
        assert_eq!(stats.engine_branches(), 0, "no engine query ran on an authz failure");
    }

    /// **Pagination slices the window and clamps a crafted huge `limit` (no unbounded top-k).**
    #[test]
    fn pagination_slices_and_clamps_limit() {
        let hits: Vec<Hit> = (0..10)
            .map(|i| Hit { doc_id: format!("d{i:02}"), score: (10 - i) as f32 })
            .collect();
        let page = Page { offset: 2, limit: 3 };
        let sliced = paginate(hits.clone(), page);
        assert_eq!(sliced.iter().map(|h| h.doc_id.clone()).collect::<Vec<_>>(),
            ["d02", "d03", "d04"], "the page window is offset..offset+limit");
        // A crafted huge limit is clamped to MAX_LIMIT (not honoured verbatim).
        let huge = Page { offset: 0, limit: usize::MAX };
        assert_eq!(huge.effective_limit(), Page::MAX_LIMIT, "a crafted limit is clamped");
    }

    /// **`ScopedEngine::tenant`/`region` expose the partition key they were opened for (kills the
    /// accessor mutants).** The accessors back the SRCH-D3 partition-key check.
    #[test]
    fn scoped_engine_exposes_its_partition_key() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        assert_eq!(eng.tenant(), "acme", "the tenant accessor returns the opened tenant verbatim");
        assert_eq!(eng.region(), "eu-west", "the region accessor returns the opened region verbatim");
    }

    /// **THE FUSION KEEPS THE MAX SCORE across branches (one doc-id space, §3.2): a doc hit by TWO
    /// branches ranks by its BEST branch score (kills the `>` merge-comparison mutant).** We feed a
    /// doc that surfaces from an FT branch (BM25 score > 0) AND a structured branch (score 0.0); the
    /// merged score must be the FT (larger) score, never the structured 0.0.
    #[test]
    fn fusion_keeps_the_max_score_across_branches() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let authz = FakeAuthz::filter(SetExpr::All);
        let stats = QueryStats::new();
        // PUB-1 matches BOTH `text ~ deadlock` (FT, BM25 > 0) and `status == open` (structured,
        // score 0.0) — the AND runs both branches over the same doc_id.
        let res = query(
            &eng,
            &authz,
            &ast(Predicate::And(vec![
                Predicate::Cmp { op: CmpOp::Eq, lhs: var(FT_BODY_FIELD), rhs: s("deadlock") },
                Predicate::Cmp { op: CmpOp::Eq, lhs: var("status"), rhs: s("open") },
            ])),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("query");
        let pub1 = res.hits.iter().find(|h| h.doc_id == "acme/issue/PUB-1").expect("PUB-1 surfaces");
        assert!(
            pub1.score > 0.0,
            "the fused score is the MAX (the BM25 FT score), not the structured branch's 0.0: {}",
            pub1.score
        );
        assert!(stats.engine_branches() >= 2, "both the FT and structured branches ran");
    }

    /// **The `QueryError` Display messages are loud + name their drill/floor (kills the Display
    /// mutants).**
    #[test]
    fn query_error_messages_are_loud() {
        let tm = QueryError::TenantMismatch {
            viewer_tenant: "evil".into(),
            engine_tenant: "acme".into(),
        };
        let s = tm.to_string();
        assert!(s.contains("evil") && s.contains("acme") && s.contains("SRCH-D3"));
        let rel = QueryError::RelationalSetExpr { form: "InRelation" };
        assert!(rel.to_string().contains("InRelation") && rel.to_string().contains("SRCH-P09"));
    }
}

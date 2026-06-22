//! The **permission-aware query pipeline** (SRCH-P08 / P-171 + SRCH-P09 / P-172; architecture
//! `search-and-indexing.md` §4.2 / §4.2.1 / §4.2.3): the ONE public [`query`] entry that composes
//! the ACL filter FIRST, conjoins it into EVERY branch (FT / structured / vector) before any
//! scoring, and proves **cross-tenant 0** (SRCH-D3) + the **structural no-N+1** (exactly ONE
//! `list_objects` per query). The bounded-set `SetExpr` lowering (`All`/`None`/`Ids`/`NotIds`) is
//! SRCH-P08; the **relational reverse-index JOIN** (`InRelation`/`TupleSet`) + the **boolean
//! composition** (`Union`/`Intersect`/`Difference`) are **SRCH-P09** (P-172) — fed the SAME conjoin
//! step (the big-result path; the cardinal zero-escape leak drill SRCH-D1).
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
//!
//! ## The relational `SetExpr` reverse-index JOIN (the SRCH-P09 crux, §4.2 / §4.2.3)
//! - `Filter{SetExpr::InRelation{relation, via_column}}` / `Filter{SetExpr::TupleSet{index}}` → the
//!   **reverse-index JOIN**: [`ListObjectsPort::resolve_relation`] JOINs against the per-tenant
//!   authz reverse index (Identity's materialised `(subject, relation, object_id)` projection,
//!   replicated per cell) for the visible-id set, which lowers to the SAME [`AclFilter::Ids`]
//!   membership clause as the bounded path (the Zanzibar/Leopard `LookupResources` reverse index as
//!   a conjoinable filter; ONE JOIN per leaf, no N+1, no post-filter). The JOIN **honours the
//!   revision watermark** ([`RevisionWatermark`], contract 4.10): a resolved revision below the
//!   `list_objects` watermark is a loud [`QueryError::StaleReverseIndex`], NEVER read stale.
//! - `Filter{SetExpr::Union/Intersect/Difference}` → [`AclFilter::Or`]/[`AclFilter::And`]/
//!   (`And` + [`AclFilter::Not`]) — the boolean composition of the lowered sub-clauses, composed at
//!   the posting-list level BEFORE scoring (a hidden doc never enters the candidate set under ANY
//!   branch). NO form is EVER silently widened to `All` (that would be a permission/cross-tenant
//!   leak).
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
//! ## FLOOR named (the downstream slices, so SRCH-P09 is not mistaken for the whole consistency story)
//! - The **relational reverse-index JOIN** (`InRelation`/`TupleSet`) + the **boolean composition**
//!   (`Union`/`Intersect`/`Difference`) is IMPLEMENTED HERE (SRCH-P09 / P-172): the JOIN resolves
//!   the visible-id set, honours the revision watermark, and composes at the posting-list level. The
//!   full zero-escape leak drill **SRCH-D1** (the cardinal sin, the big-result path, incl.
//!   counts/IDF/RAG) is proven by this crate's drill test.
//! - The **full no-stale-grant + fail-static mechanism** (SRCH-D2: the new-enemy drill — revoke,
//!   re-search, the fail-static cache bypass) → **SRCH-P10** (P-173). Here the watermark mechanism
//!   is wired so the JOIN never READS a stale reverse-index revision (a stale revision is a loud
//!   [`QueryError::StaleReverseIndex`]); the wait/bounded-recheck/fail-static bypass is downstream.
//! - The **BM25 default ranking** → the post-M5 learning-to-rank / semantic re-rank floor
//!   (**SRCH-P26**). Here scoring is BM25 / the deterministic interleave.
//! - The **hybrid RRF fusion + vector filter-during-traversal** → **SRCH-P11** (P-174). Here a
//!   hybrid query runs all three branches with the conjoined ACL and a deterministic interleave; the
//!   tuned RRF rank fusion is the downstream prompt.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use myelin_identity::{
    ColRef, Consistency, ListObjectsResult, ObjectId, ObjectType, Permission, Principal, RelName,
    Result as AuthzResult, SetExpr,
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

    /// **Resolve a relational `SetExpr` form to the co-located visible-id set (the SRCH-P09
    /// reverse-index JOIN, contract 4.3 / §4.2).** When `list_objects` returns a `Filter` whose
    /// algebra contains the relational forms `InRelation{relation, via_column}` / `TupleSet{index}`,
    /// Search JOINs against the **per-tenant authz reverse index** — Identity's materialised
    /// `(subject, relation, object_id)` projection, replicated/queried per cell, kept fresh off the
    /// bus. This resolves ONE such form for `subject` to the set of `object_id`s the subject reaches,
    /// together with the **revision** the reverse index served the answer at (contract 4.10, the
    /// revision watermark — the JOIN never reads a revision staler than the `required` watermark
    /// derived from the `list_objects` zookie). The Zanzibar/Leopard `LookupResources` reverse index
    /// as a conjoinable filter; ONE resolve per relational leaf, no N+1.
    ///
    /// **Default = unavailable (deny-when-unsure, ADR-03).** A port wired ONLY for the bounded-set
    /// path (the SRCH-P08 fakes) has no reverse index; resolving a relational form against it is a
    /// loud `Unavailable`, never a silent widen. The production wiring + the SRCH-P09 tests provide
    /// a real resolver.
    fn resolve_relation(
        &self,
        _subject: &Principal,
        _form: &RelationalLeaf,
        _required: &RevisionWatermark,
    ) -> AuthzResult<ReverseIndexAnswer> {
        Err(myelin_identity::AuthzError::Unavailable(
            "the authz reverse index is not wired for this query path — a relational SetExpr leaf \
             cannot be resolved (deny-when-unsure, ADR-03; SRCH-P09 needs a reverse-index resolver)"
                .into(),
        ))
    }
}

/// **A relational `SetExpr` leaf the reverse-index JOIN resolves (SRCH-P09).** The two relational
/// forms of the frozen algebra (OQ-E): `InRelation{relation, via_column}` (objects where the
/// doc_id is the object of `relation` for the subject — a JOIN keyed by the consumer's own
/// `via_column`) and `TupleSet{index}` (a server-materialised tuple set to JOIN against — the
/// big-result path). Lifted out of [`myelin_identity::SetExpr`] so the resolver port takes JUST the
/// relational leaf (the boolean composition is resolved by the pipeline, not the port).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelationalLeaf {
    /// `InRelation{relation, via_column}` — objects where the doc_id is the object of `relation`
    /// for the subject, JOINed by the consumer's own `via_column`.
    InRelation {
        relation: RelName,
        via_column: ColRef,
    },
    /// `TupleSet{index}` — a server-materialised `(subject, relation, object_id)` tuple set to
    /// JOIN/semijoin against (the big-result path).
    TupleSet {
        index: myelin_identity::AuthzIndexRef,
    },
}

/// **The authz reverse-index revision watermark (contract 4.10, §4.2.3).** A monotone revision the
/// reverse-index JOIN honours: the JOIN must read at a revision **≥** the watermark the
/// `list_objects` answer was computed at (derived from its zookie), so a JOIN never composes a
/// reverse-index revision OLDER than the ACL snapshot the rest of the filter was computed at (a
/// stale reverse-index revision could re-admit a just-revoked grant — the new-enemy problem). The
/// FULL no-stale-grant + fail-static drill (SRCH-D2) is **SRCH-P10**; here the mechanism is wired so
/// the JOIN never READS a stale revision — a resolver returning a revision below the watermark is a
/// loud [`QueryError::StaleReverseIndex`], resolved by the bounded-check fallback, never served.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RevisionWatermark(pub u64);

/// **The reverse-index JOIN answer (SRCH-P09).** The co-located visible-id set the relational leaf
/// resolved to + the **revision** the reverse index served it at (contract 4.10). The pipeline
/// checks `revision >= required` (the watermark) before composing the set into the ACL filter — a
/// revision below the watermark is rejected loudly (never read stale).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReverseIndexAnswer {
    /// The `object_id`s the subject reaches via this relational leaf (the visible-id set the JOIN
    /// produced — the `LookupResources` reverse index as a conjoinable membership clause).
    pub object_ids: Vec<String>,
    /// The reverse-index revision this answer was served at (contract 4.10). The watermark check
    /// asserts `revision >= required`.
    pub revision: RevisionWatermark,
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
        ScopedEngine {
            backend,
            tenant: tenant.into(),
            region: region.into(),
            schema,
        }
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
    pub const FIRST: Page = Page {
        offset: 0,
        limit: 50,
    };

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
    /// **The number of authz reverse-index JOINs the relational lowering issued (SRCH-P09).** One
    /// resolve per relational `SetExpr` leaf — the no-N+1 GATE on the relational path asserts the
    /// JOIN against the reverse index is ONE query per leaf (a single relational filter ⇒ exactly 1,
    /// never one resolve per candidate doc). The `Ids vs Filter/TupleSet` filter-mode split (1.8).
    reverse_index_joins: AtomicU64,
    /// **The filter-mode split (contract 1.8 / §4.11) — the `Ids` leg.** Incremented once per query
    /// whose `list_objects` answer was the MATERIALISED `Ids{ids,zookie}` form (the S4 path: a
    /// concrete visible doc-id allow-set). Read against [`Self::filter_mode_count`] so the
    /// metrics-health port emits `list_objects` mode = `Ids` vs `Filter`/`TupleSet` (SRCH-P14).
    ids_mode_count: AtomicU64,
    /// **The filter-mode split (contract 1.8 / §4.11) — the `Filter`/`TupleSet` leg.** Incremented
    /// once per query whose `list_objects` answer was the PUSHED-DOWN `Filter{set_expr,zookie}` form
    /// (the S8 path: a `SetExpr` algebra lowered into the engine filter, including the relational
    /// `InRelation`/`TupleSet` reverse-index JOINs). The other half of the 1.8 filter-mode split.
    filter_mode_count: AtomicU64,
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

    /// The number of authz reverse-index JOINs recorded (SRCH-P09 — the relational-path no-N+1 GATE
    /// reads this: ONE resolve per relational `SetExpr` leaf, never one per candidate doc).
    pub fn reverse_index_joins(&self) -> u64 {
        self.reverse_index_joins.load(Ordering::Relaxed)
    }

    /// **The `Ids` leg of the filter-mode split (contract 1.8 / §4.11).** Queries whose
    /// `list_objects` answer was the materialised `Ids{ids}` form. SRCH-P14 reads this onto the
    /// metrics-health port as the `Ids` half of the `Ids vs Filter/TupleSet` split.
    pub fn ids_mode_count(&self) -> u64 {
        self.ids_mode_count.load(Ordering::Relaxed)
    }

    /// **The `Filter`/`TupleSet` leg of the filter-mode split (contract 1.8 / §4.11).** Queries whose
    /// `list_objects` answer was the pushed-down `Filter{set_expr}` form (incl. the relational
    /// `InRelation`/`TupleSet` reverse-index JOINs). The other half of the 1.8 filter-mode split.
    pub fn filter_mode_count(&self) -> u64 {
        self.filter_mode_count.load(Ordering::Relaxed)
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
    TenantMismatch {
        viewer_tenant: String,
        engine_tenant: String,
    },
    /// **The authz reverse-index JOIN read a revision STALER than the required watermark (SRCH-P09 /
    /// contract 4.10).** A relational `SetExpr` leaf resolved to a reverse-index answer whose
    /// revision is BELOW the watermark the `list_objects` zookie required — the JOIN refuses to
    /// compose a stale reverse-index revision (a stale revision could re-admit a just-revoked grant,
    /// the new-enemy problem). Surfaced loudly here; the full no-stale-grant + fail-static bounded
    /// re-check is SRCH-P10. NEVER served stale-allow.
    StaleReverseIndex {
        required: u64,
        served: u64,
        form: &'static str,
    },
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryError::Compile(e) => write!(f, "query did not compile: {e}"),
            QueryError::Engine(e) => write!(f, "search engine error: {e}"),
            QueryError::Authz(e) => write!(f, "list_objects (authz) failed: {e:?}"),
            QueryError::TenantMismatch {
                viewer_tenant,
                engine_tenant,
            } => write!(
                f,
                "cross-tenant query rejected: viewer tenant `{viewer_tenant}` != engine tenant \
                 `{engine_tenant}` (SRCH-D3 — tenant from the verified token, the engine is the \
                 wrong tenant's index)"
            ),
            QueryError::StaleReverseIndex {
                required,
                served,
                form,
            } => write!(
                f,
                "the authz reverse-index JOIN for the relational form `{form}` served revision \
                 {served} but the list_objects watermark requires >= {required} (contract 4.10) — \
                 the JOIN refuses to compose a stale reverse-index revision (SRCH-P09; a stale \
                 revision could re-admit a revoked grant — the new-enemy problem); the full \
                 no-stale-grant + fail-static path is SRCH-P10"
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

/// **Derive the reverse-index revision watermark from a `list_objects` zookie (contract 4.10).** The
/// zookie is the consistency snapshot the ACL answer was computed at; the reverse-index JOIN must
/// read at a revision **≥** this watermark so it never composes a revision older than the rest of the
/// filter. In Search's embedded model the opaque zookie carries a monotone revision suffix
/// `…@<rev>`; a zookie with no suffix carries watermark 0 (any non-stale revision satisfies it). The
/// real zookie→revision mapping is Identity's (contract 4.10); this is the deterministic embedded
/// model the SRCH-P09 watermark mechanism is proven against (the full fail-static path is SRCH-P10).
///
/// `pub(crate)` so the SRCH-P10 consistency path ([`crate::consistency`]) shares the SAME
/// zookie→revision decoding the watermark uses — ONE encoding, no drift between the JOIN watermark
/// and the no-stale-grant candidate comparison.
pub(crate) fn watermark_from_zookie(zookie: &str) -> RevisionWatermark {
    let rev = zookie
        .rsplit_once('@')
        .and_then(|(_, suffix)| suffix.parse::<u64>().ok())
        .unwrap_or(0);
    RevisionWatermark(rev)
}

/// **Lower a [`ListObjectsResult`] to the engine [`AclFilter`] (the SRCH-P08 bounded-set crux +
/// the SRCH-P09 relational reverse-index JOIN).** `Ids{ids}` (the materialised S4 path) → a doc-id
/// allow-set; `Filter{set_expr}` lowers the FULL frozen `SetExpr` algebra: the bounded-set forms
/// (`All`/`None`/`Ids`/`NotIds`) directly, and the **relational** forms (`InRelation`/`TupleSet`)
/// via the reverse-index JOIN (`identity.resolve_relation` against the per-tenant authz reverse
/// index, honouring the revision watermark), composing `Union`/`Intersect`/`Difference` into the
/// `And`/`Or`/`Not` boolean clauses. Returns the lowered filter + the zookie the answer was computed
/// at. No form is EVER silently widened to `All` (that would leak).
fn lower_acl(
    result: &ListObjectsResult,
    subject: &Principal,
    identity: &dyn ListObjectsPort,
    stats: &QueryStats,
) -> Result<(AclFilter, String), QueryError> {
    match result {
        ListObjectsResult::Ids { ids, zookie } => {
            // **Filter-mode split (1.8):** this query used the MATERIALISED `Ids` mode (S4).
            stats.ids_mode_count.fetch_add(1, Ordering::Relaxed);
            let ids: Vec<String> = ids.iter().map(|o| o.0.clone()).collect();
            // An empty materialised allow-set is `None` (the viewer can see nothing of this type).
            let filter = if ids.is_empty() {
                AclFilter::None
            } else {
                AclFilter::Ids(ids)
            };
            Ok((filter, zookie.0.clone()))
        }
        ListObjectsResult::Filter { set_expr, zookie } => {
            // **Filter-mode split (1.8):** this query used the PUSHED-DOWN `Filter`/`TupleSet` mode
            // (S8 — the `SetExpr` algebra, incl. the relational reverse-index JOINs).
            stats.filter_mode_count.fetch_add(1, Ordering::Relaxed);
            // The watermark the reverse-index JOIN must honour is the snapshot the ACL answer was
            // computed at (contract 4.10) — the SAME zookie the rest of the filter rode.
            let required = watermark_from_zookie(&zookie.0);
            let filter = lower_set_expr(set_expr, subject, identity, &required, stats)?;
            Ok((filter, zookie.0.clone()))
        }
    }
}

/// **Lower a [`SetExpr`] to an [`AclFilter`] — the full frozen algebra (OQ-E).** The bounded-set
/// forms lower directly; the relational forms JOIN against the reverse index (honouring `required`,
/// the revision watermark); the boolean forms compose recursively into `And`/`Or`/`Not`. No silent
/// widen to `All` for any form.
fn lower_set_expr(
    set_expr: &SetExpr,
    subject: &Principal,
    identity: &dyn ListObjectsPort,
    required: &RevisionWatermark,
    stats: &QueryStats,
) -> Result<AclFilter, QueryError> {
    match set_expr {
        SetExpr::All => Ok(AclFilter::All),
        SetExpr::None => Ok(AclFilter::None),
        SetExpr::Ids(ids) => {
            let ids: Vec<String> = ids.iter().map(|o| o.0.clone()).collect();
            // An explicit empty allow-set is deny (nothing visible) — `None`, never `All`.
            Ok(if ids.is_empty() {
                AclFilter::None
            } else {
                AclFilter::Ids(ids)
            })
        }
        SetExpr::NotIds(ids) => {
            let ids: Vec<String> = ids.iter().map(|o| o.0.clone()).collect();
            // An empty deny-set excludes nothing ⇒ everything of this type is visible (`All`).
            Ok(if ids.is_empty() {
                AclFilter::All
            } else {
                AclFilter::NotIds(ids)
            })
        }
        // **THE RELATIONAL REVERSE-INDEX JOIN (SRCH-P09).** Resolve the relational leaf to the
        // co-located visible-id set via the per-tenant authz reverse index, honouring the watermark.
        SetExpr::InRelation {
            relation,
            via_column,
        } => resolve_relational_leaf(
            &RelationalLeaf::InRelation {
                relation: relation.clone(),
                via_column: via_column.clone(),
            },
            "InRelation",
            subject,
            identity,
            required,
            stats,
        ),
        SetExpr::TupleSet { index } => resolve_relational_leaf(
            &RelationalLeaf::TupleSet {
                index: index.clone(),
            },
            "TupleSet",
            subject,
            identity,
            required,
            stats,
        ),
        // **THE BOOLEAN COMPOSITION (SRCH-P09).** Union/Intersect/Difference compose the lowered
        // sub-clauses into the engine `Or`/`And`/`Not` — at the posting-list level, before scoring.
        SetExpr::Union(subs) => {
            let mut clauses = Vec::with_capacity(subs.len());
            for s in subs {
                clauses.push(lower_set_expr(s, subject, identity, required, stats)?);
            }
            Ok(AclFilter::Or(clauses))
        }
        SetExpr::Intersect(subs) => {
            let mut clauses = Vec::with_capacity(subs.len());
            for s in subs {
                clauses.push(lower_set_expr(s, subject, identity, required, stats)?);
            }
            Ok(AclFilter::And(clauses))
        }
        SetExpr::Difference(left, right) => {
            // `left EXCEPT right` = `left AND NOT right` — the visible-under-left minus the
            // reachable-under-right, composed at the posting-list level (BEFORE scoring).
            let l = lower_set_expr(left, subject, identity, required, stats)?;
            let r = lower_set_expr(right, subject, identity, required, stats)?;
            Ok(AclFilter::And(vec![l, AclFilter::Not(Box::new(r))]))
        }
    }
}

/// **Resolve ONE relational `SetExpr` leaf to the co-located visible-id set (the SRCH-P09
/// reverse-index JOIN).** JOINs against the per-tenant authz reverse index
/// ([`ListObjectsPort::resolve_relation`]), honours the revision watermark (a served revision below
/// `required` is a loud [`QueryError::StaleReverseIndex`], never read stale — §4.2.3 / contract
/// 4.10), and lowers the resulting visible-id set to the SAME `Ids` membership clause the bounded
/// set path uses (an empty resolved set ⇒ `None` — the subject reaches nothing via this relation,
/// never a silent widen). ONE resolve per leaf (no N+1; the resolve count is recorded so the GATE
/// can assert it).
fn resolve_relational_leaf(
    leaf: &RelationalLeaf,
    form: &'static str,
    subject: &Principal,
    identity: &dyn ListObjectsPort,
    required: &RevisionWatermark,
    stats: &QueryStats,
) -> Result<AclFilter, QueryError> {
    let answer = identity
        .resolve_relation(subject, leaf, required)
        .map_err(QueryError::Authz)?;
    stats.reverse_index_joins.fetch_add(1, Ordering::Relaxed);

    // **THE REVISION WATERMARK CHECK (contract 4.10):** the JOIN must read at a revision >= the
    // watermark the ACL snapshot was computed at. A staler revision is REFUSED — never composed
    // into the filter (a stale reverse-index revision could re-admit a just-revoked grant).
    if answer.revision < *required {
        return Err(QueryError::StaleReverseIndex {
            required: required.0,
            served: answer.revision.0,
            form,
        });
    }

    // Lower the resolved visible-id set to the SAME membership clause as the bounded `Ids` path
    // (one membership-clause meaning — no drift between the bounded path and the relational JOIN).
    // An empty resolved set is deny (the subject reaches nothing via this relation), never `All`.
    Ok(if answer.object_ids.is_empty() {
        AclFilter::None
    } else {
        AclFilter::Ids(answer.object_ids)
    })
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
    // The default-consistency path with NO bounded-check port wired: a candidate whose
    // `indexed_zookie` is stale relative to the passed zookie is EXCLUDED pending re-index (fail
    // CLOSED, ADR-03 — never served stale-allow). The full zookie re-validation (bounded `check` on
    // the affected candidates) is [`query_consistent`] (SRCH-P10); this entry preserves the
    // SRCH-P08/P09 call shape and still honours the no-stale-grant exclusion.
    let consistency_stats = crate::consistency::ConsistencyStats::new();
    query_consistent(
        engine,
        identity,
        None,
        ast,
        viewer,
        ty,
        at,
        page,
        stats,
        &consistency_stats,
    )
}

/// **THE ZOOKIE/CONSISTENCY QUERY ENTRY (SRCH-P10 / P-173; §4.2.3 + contract 4.2/4.10/1.10).** The
/// permission-aware [`query`] PLUS the **no-stale-grant zookie re-validation** + the **fail-static
/// degrade-not-cascade** mechanism (the consistency mechanism). Identical steps 0–4 to [`query`],
/// then — between execution and pagination — the **STEP 4.5 no-stale-grant pass**:
///
/// A candidate doc whose `indexed_zookie` is OLDER than the passed query `at` zookie for an
/// ACL-relevant facet carries a STALE permission projection (the new-enemy problem). Such a
/// candidate is **re-validated** via a bounded `check` on the affected candidate only (contract
/// 4.2, the `check` port) and surfaces iff the check still ALLOWS at the demanded snapshot — or is
/// **excluded pending re-index** when no `check` port is wired (fail CLOSED, ADR-03). A fresh
/// candidate (indexed at-or-after the passed zookie) is served as-is. The re-validation runs over
/// the BOUNDED affected set ONLY (the stale subset, never every hit — no N+1).
///
/// **The fail-static decision (contract 4.10/1.10):** a **zookie-stamped strong** read
/// ([`ConsistencyMode::Strong`]) BYPASSES the fail-static cache (read-your-writes-after-revocation
/// must see the revocation); a **default-consistency** read ([`ConsistencyMode::BoundedStale`]) MAY
/// degrade-not-cascade on an Id hiccup (the substrate `FailStatic<T>` cache; the bypass decision is
/// [`crate::consistency::fail_static_bypass`]). The fail-static ratio telemetry (1.8) is recorded.
///
/// - `check` — the bounded re-validation port (contract 4.2); `None` ⇒ a stale candidate is
///   excluded pending re-index (fail closed — the safe default).
/// - `cstats` — the consistency telemetry (the SRCH-D2 zero-escape + the fail-static ratio).
#[allow(clippy::too_many_arguments)]
pub fn query_consistent<B: IndexBackend>(
    engine: &ScopedEngine<'_, B>,
    identity: &dyn ListObjectsPort,
    check: Option<&dyn crate::consistency::BoundedCheckPort>,
    ast: &QueryAst,
    viewer: &Principal,
    ty: &ObjectType,
    at: &Consistency,
    page: Page,
    stats: &QueryStats,
    cstats: &crate::consistency::ConsistencyStats,
) -> Result<RankedResults, QueryError> {
    // The FT/structured path: NO query-time embedding is supplied, so the vector branch is
    // recognised but not executed (the hybrid/semantic execution is the `semantic`/`hybrid` entries
    // below). All other steps — list_objects, lower, conjoin, no-stale-grant — are identical.
    query_consistent_with_vector(
        engine, identity, check, ast, viewer, ty, at, None, page, stats, cstats,
    )
}

/// **THE HYBRID / SEMANTIC QUERY ENTRY (SRCH-P11 / P-174; contract 6.2 `semantic(text|vec, viewer,
/// k, filter_ast?)`; §4.5).** The full permission-aware + zookie-consistent query path of
/// [`query_consistent`] PLUS the executed **vector branch** (filter-during-traversal — k VISIBLE
/// neighbours) and the **RRF fusion** of the lexical + semantic ranked lists. The semantic surface
/// REUSES the SAME SRCH-P10 zookie path (no-stale-grant for RAG too): a vector hit whose
/// `indexed_zookie` is stale is re-validated / excluded exactly as a lexical hit is — an agent's RAG
/// retrieval is permission-correct AND consistency-correct by the same machinery.
///
/// - `vec` — the query-time embedding source (contract 6.2 `text|vec`): a directly-supplied vector
///   OR query text embedded through the swappable [`crate::indexer::EmbeddingAdapter`] (the
///   model_ref-pinned adapter, §3.3; mock v1, real EU-hostable model post-M5 — the named floor). The
///   query embedding shares the corpus's vector space (same adapter), so the k-NN is meaningful.
/// - `filter_ast` is carried in the `ast` exactly as for [`query_consistent`] (the structured/FT
///   predicates conjoined with the ACL filter); a pure-semantic query passes an `ast` with only the
///   semantic clause. Both branches carry the SAME conjoined ACL filter, so **fusion can never
///   introduce a hidden doc** (§4.5 — the SRCH-D1 vector/RAG half).
///
/// **Agent RAG (contract 6.2 / VISION §3):** an agent's retrieval rides this entry with the agent's
/// DELEGATED principal as `viewer`; the top-k VISIBLE passages are returned, so the agent never
/// retrieves a doc its delegated principal cannot see (RAG is permission-correct by the same
/// pre-filter — not a separate, weaker path).
#[allow(clippy::too_many_arguments)]
pub fn semantic<B: IndexBackend>(
    engine: &ScopedEngine<'_, B>,
    identity: &dyn ListObjectsPort,
    check: Option<&dyn crate::consistency::BoundedCheckPort>,
    ast: &QueryAst,
    viewer: &Principal,
    ty: &ObjectType,
    at: &Consistency,
    vec: &VectorQuery<'_>,
    page: Page,
    stats: &QueryStats,
    cstats: &crate::consistency::ConsistencyStats,
) -> Result<RankedResults, QueryError> {
    query_consistent_with_vector(
        engine,
        identity,
        check,
        ast,
        viewer,
        ty,
        at,
        Some(vec),
        page,
        stats,
        cstats,
    )
}

/// The shared query path for [`query_consistent`] (no vector) and [`semantic`] (executed vector
/// branch + RRF). `vector_query` is the query-time embedding source threaded into [`execute`].
#[allow(clippy::too_many_arguments)]
fn query_consistent_with_vector<B: IndexBackend>(
    engine: &ScopedEngine<'_, B>,
    identity: &dyn ListObjectsPort,
    check: Option<&dyn crate::consistency::BoundedCheckPort>,
    ast: &QueryAst,
    viewer: &Principal,
    ty: &ObjectType,
    at: &Consistency,
    vector_query: Option<&VectorQuery<'_>>,
    page: Page,
    stats: &QueryStats,
    cstats: &crate::consistency::ConsistencyStats,
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

    // **STEP 0.5 — the fail-static bypass decision (contract 4.10/1.10).** A zookie-stamped strong
    // read bypasses the fail-static cache (it must see a just-revoked grant); a default-consistency
    // read may degrade-not-cascade. Recorded for the fail-static ratio (1.8). The cache itself is
    // the substrate `FailStatic<T>` the production `list_objects` client fronts (P-S25) — the bypass
    // flag tells that client whether a stale coarse-grant answer is permissible for THIS read.
    if crate::consistency::fail_static_bypass(at) {
        cstats.record_fail_static_bypass();
    } else {
        cstats.record_fail_static_served();
    }

    // **STEP 1 — the ACL filter FIRST.** Exactly ONE list_objects call (the no-N+1 invariant; the
    // GATE asserts this counter == 1, never one check per result).
    let permission = Permission(READ_PERMISSION.to_string());
    let lo = identity
        .list_objects(viewer, &permission, ty, at)
        .map_err(QueryError::Authz)?;
    stats.list_objects_calls.fetch_add(1, Ordering::Relaxed);

    // Lower the FULL SetExpr algebra → AclFilter: the bounded-set forms directly (SRCH-P08) AND the
    // relational reverse-index JOIN + boolean composition (SRCH-P09), honouring the revision
    // watermark. A relational leaf JOINs against the per-tenant authz reverse index via `identity`.
    let (acl, zookie) = lower_acl(&lo, viewer, identity, stats)?;

    // **STEP 2 — compile the frozen AST** to the FT/structured/vector branches (SRCH-P07).
    let plan = compiler::compile(ast, &engine.schema)?;
    let post_fetch_fields: Vec<String> = plan.post_fetch.iter().map(|p| p.field.clone()).collect();

    // **STEP 3 — CONJOIN.** The ACL filter is attached via the seam; the engine is unreachable
    // without it (the search-requires-acl-filter ratchet, structural). `None` short-circuits to an
    // empty result WITHOUT touching the engine (WHERE false — no branch can leak a count).
    let conjoined: crate::compiler::ConjoinedPlan<AclFilter> = plan.with_acl(acl);
    if matches!(conjoined.acl, AclFilter::None) {
        // The viewer sees nothing of this type — short-circuit (the engine is never queried).
        return Ok(RankedResults {
            hits: Vec::new(),
            zookie,
            post_fetch_fields,
        });
    }

    // **STEP 4 — execute EVERY branch under the SAME conjoined ACL filter** (the pre-filter, §4.2.1).
    // Fetch the full ranked candidate list (not yet paginated) so the no-stale-grant pass can
    // exclude stale candidates BEFORE the page window is sliced (an excluded stale doc must not
    // consume a visible page slot — otherwise the page would be short by the excluded count).
    let hits = execute(engine.backend, &conjoined, vector_query, page, stats)?;

    // **STEP 4.5 — THE NO-STALE-GRANT ZOOKIE PASS (§4.2.3).** A candidate whose `indexed_zookie` is
    // OLDER than the **passed query zookie** (`at.at_least`, the read-your-writes snapshot the
    // CALLER demanded — NOT the list_objects answer's zookie) carries a stale permission projection.
    // Re-validate it (a bounded `check` on the affected candidate only) or exclude pending re-index.
    // NEVER served stale-allow. The re-validation runs over the bounded stale SUBSET, never every
    // hit. A default-consistency query with no real zookie watermark (rev 0) finds nothing stale —
    // it uses the indexed filter as-is (bounded staleness ≤ W), the degrade-not-cascade path.
    let hits = revalidate_stale_candidates(
        engine.backend,
        hits,
        identity_subject(viewer),
        &permission,
        &at.at_least.0,
        at,
        check,
        cstats,
    )?;

    // **STEP 5 — rank / fuse / paginate / project.** `execute` already merged + deduped on doc_id;
    // here we slice the page window (over the post-revalidation visible set).
    let paged = paginate(hits, page);
    Ok(RankedResults {
        hits: paged
            .into_iter()
            .map(|h| RankedResult {
                doc_id: h.doc_id,
                score: h.score,
            })
            .collect(),
        zookie,
        post_fetch_fields,
    })
}

/// The verified principal whose reachable set drives both `list_objects` and the bounded re-check
/// (named so the re-validation's subject is self-evidently the SAME verified viewer, never a
/// re-derived one — no cross-subject drift in the consistency pass).
fn identity_subject(viewer: &Principal) -> &Principal {
    viewer
}

/// **The no-stale-grant re-validation (§4.2.3 / SRCH-P10).** Partition the ranked hits into fresh
/// (indexed at-or-after the passed `zookie`) and stale (indexed before it) by the per-doc
/// `indexed_zookie` point lookup; serve the fresh as-is; re-validate each STALE candidate via the
/// bounded `check` port (contract 4.2) at the demanded consistency `at` — admit iff it still
/// ALLOWS, otherwise EXCLUDE; with NO check port wired, exclude every stale candidate pending
/// re-index (fail CLOSED, ADR-03). NEVER served stale-allow. The bounded affected set is the stale
/// subset only (no N+1 over every hit). The fresh hits keep their ranked order; an admitted stale
/// hit is re-appended after the fresh set (its rank is preserved relative to other admitted-stale).
#[allow(clippy::too_many_arguments)]
fn revalidate_stale_candidates<B: IndexBackend>(
    backend: &B,
    hits: Vec<Hit>,
    subject: &Principal,
    permission: &Permission,
    zookie: &str,
    at: &Consistency,
    check: Option<&dyn crate::consistency::BoundedCheckPort>,
    cstats: &crate::consistency::ConsistencyStats,
) -> Result<Vec<Hit>, QueryError> {
    let mut out: Vec<Hit> = Vec::with_capacity(hits.len());
    for hit in hits {
        // The per-doc staleness anchor — a doc-id POINT LOOKUP (not a scored search).
        let indexed = backend.indexed_zookie_of(&hit.doc_id);
        match crate::consistency::disposition(indexed.as_deref(), zookie) {
            // Fresh: its indexed ACL state already reflects the demanded snapshot — serve as-is.
            crate::consistency::CandidateDisposition::Fresh => out.push(hit),
            // Stale: re-validate via a bounded `check`, or exclude pending re-index (fail closed).
            crate::consistency::CandidateDisposition::StaleNeedsRevalidation => {
                match check {
                    Some(port) => {
                        cstats.record_revalidation();
                        let object = ObjectId(hit.doc_id.clone());
                        let still_allowed = port
                            .check(subject, permission, &object, at)
                            .map_err(QueryError::Authz)?;
                        if still_allowed {
                            // The grant survives the demanded snapshot — surface it.
                            out.push(hit);
                        } else {
                            // The grant is gone at the demanded zookie (the new-enemy) — EXCLUDE.
                            cstats.record_excluded_stale();
                        }
                    }
                    None => {
                        // No bounded-check port → exclude the stale candidate pending re-index
                        // (fail CLOSED — never served stale-allow, ADR-03).
                        cstats.record_excluded_stale();
                    }
                }
            }
        }
    }
    Ok(out)
}

/// **The query-time embedding source for the vector branch (SRCH-P11 / contract 6.2 `text|vec`).**
/// `semantic(text|vec, …)` accepts EITHER a directly-supplied query vector (`Vec`) OR query text to
/// embed through the swappable [`EmbeddingAdapter`] (`Text`) — the §3.3 model_ref-pinned adapter
/// (mock v1, real EU-hostable model post-M5, the named floor). `None` (the [`query`] /
/// [`query_consistent`] path) means NO embedding is supplied: the vector branch is recognised but
/// not executed (the FT/structured query path is unchanged). The adapter is borrowed, never owned.
pub enum VectorQuery<'a> {
    /// A query vector supplied directly (the `vec` form of contract 6.2) — already embedded.
    Vec(crate::vector::Embedding),
    /// Query text to embed through the adapter at query time (the `text` form) — the SAME adapter
    /// that embedded the corpus, so the query and the docs live in one vector space (§3.3).
    Text {
        text: String,
        embedder: &'a dyn crate::indexer::EmbeddingAdapter,
    },
}

impl VectorQuery<'_> {
    /// Resolve to the query embedding (embedding the text through the adapter if needed). `None` if
    /// the text is empty (no embedding for empty text — a vector with no source is meaningless,
    /// §3.3). The directly-supplied `Vec` form is always present.
    fn resolve(&self) -> Option<crate::vector::Embedding> {
        match self {
            VectorQuery::Vec(e) => Some(e.clone()),
            VectorQuery::Text { text, embedder } => embedder.embed(text),
        }
    }
}

/// **Execute every lowered branch (FT / structured / vector) under the conjoined ACL filter, then
/// fuse on `doc_id` (the one-doc-id-space fusion, §3.2).** Each branch is run with the IDENTICAL
/// [`AclFilter`] from the [`ConjoinedPlan`] (the conjoin-into-every-branch GATE) — no branch can
/// reach the engine without the filter, and no branch uses a different ACL clause.
///
/// **Fusion (§4.5 — the SRCH-P11 RRF).** When a query carries BOTH a lexical (FT) and a semantic
/// (vector) branch, the two ranked lists are fused with **Reciprocal Rank Fusion** ([`crate::fusion`])
/// — score-scale-free (no per-corpus calibration), and because BOTH branches carry the SAME conjoined
/// ACL filter (FT via the posting-list pre-filter, vector via filter-during-traversal), fusion can
/// **never introduce a hidden doc** (the leak-safe property; the SRCH-D1 vector/RAG half). Structured
/// equality branches (exact-match filters, not relevance-ranked) keep the deterministic max-score
/// merge and are unioned with the fused relevance ranking. A query with no FT/structured/vector
/// clause runs an admit-all FT search so the ACL allow/deny set is the only predicate.
///
/// `vector_query` is the query-time embedding source (SRCH-P11): `Some` for the `semantic`/`hybrid`
/// entries (the vector branch runs through `backend.semantic`, filter-during-traversal); `None` for
/// the FT-only [`query`]/[`query_consistent`] path (the vector branch is recognised but not executed).
fn execute<B: IndexBackend>(
    backend: &B,
    conjoined: &crate::compiler::ConjoinedPlan<AclFilter>,
    vector_query: Option<&VectorQuery<'_>>,
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

    // The FT branch(es) → ONE ranked list (BM25 order). Each conjoins the ACL clause BEFORE BM25
    // scoring (the engine's `search` takes the filter as a mandatory parameter — the ratchet). The
    // ranked doc-ids feed RRF (rank, not score — score-scale-free).
    let mut ft_ranked: Vec<Hit> = Vec::new();
    for ft in &plan.ft {
        for h in backend.search(acl_filter, &ft.query, fetch)? {
            if !ft_ranked.iter().any(|e| e.doc_id == h.doc_id) {
                ft_ranked.push(h);
            }
        }
        stats.engine_branches.fetch_add(1, Ordering::Relaxed);
    }

    // The VECTOR branch (SRCH-P11 / §4.5) → ONE ranked list (cosine-similarity order), executed via
    // filter-during-traversal with the SAME conjoined ACL filter (k VISIBLE neighbours, never
    // k-then-filtered — `backend.semantic`). It runs ONLY when a query-time embedding is supplied
    // (the `semantic`/`hybrid` entries). With no embedding (the FT-only [`query`] path) the branch is
    // recognised but not executed (the prior counted-no-op behaviour, preserved).
    let mut vector_ranked: Vec<crate::vector::VectorHit> = Vec::new();
    if plan.vector.is_some() {
        stats.engine_branches.fetch_add(1, Ordering::Relaxed);
        if let Some(vq) = vector_query {
            if let Some(query_embedding) = vq.resolve() {
                // k = the fetch window (the page's worth of nearest VISIBLE neighbours). The engine
                // conjoins the ACL filter DURING traversal — a hidden doc never enters the candidate
                // set (the SRCH-D1 vector/RAG half).
                vector_ranked = backend.semantic(acl_filter, &query_embedding, fetch)?;
            }
        }
    }

    // **RRF FUSION (§4.5).** Build the rank lists and fuse. When both branches are present this is
    // the hybrid lexical+semantic ranking; when only one is present RRF degenerates to that branch's
    // order (an empty branch contributes nothing). The fused set is EXACTLY the union of the branch
    // lists — fusion holds no ACL state, so no hidden doc is introduced (leak-safe by construction).
    let mut fusion_inputs: Vec<crate::fusion::RankedList> = Vec::new();
    if !ft_ranked.is_empty() {
        fusion_inputs.push(crate::fusion::RankedList::from_ranked(
            ft_ranked.iter().map(|h| h.doc_id.clone()),
        ));
    }
    if !vector_ranked.is_empty() {
        fusion_inputs.push(crate::fusion::RankedList::from_ranked(
            vector_ranked.iter().map(|h| h.doc_id.clone()),
        ));
    }
    let fused = crate::fusion::reciprocal_rank_fusion(&fusion_inputs);

    // Merge the fused relevance ranking with the structured equality branches (exact-match filters)
    // keyed by doc_id (one doc-id space); keep the MAX score across contributions (a doc surfaced by
    // both relevance fusion and an exact-match facet ranks by its best contribution).
    let mut merged: BTreeMap<String, f32> = BTreeMap::new();
    let mut record = |hits: Vec<Hit>| {
        for h in hits {
            let e = merged.entry(h.doc_id).or_insert(f32::MIN);
            // Keep the MAX score across branches. NOTE on the cargo-mutants `> → >=` survivor on
            // this guard (2026-06-20): it is an EQUIVALENT mutant — when `h.score == *e` the `>=`
            // branch re-assigns the IDENTICAL value, an observably identical merged max. No test can
            // distinguish `>` from `>=` here. Named, not silently accepted (the one justified
            // survivor — the same equivalent-mutant class the engine's `merge` `>1` guard documents).
            if h.score > *e {
                *e = h.score;
            }
        }
    };
    // The fused relevance hits (FT + vector via RRF).
    record(
        fused
            .into_iter()
            .map(|f| Hit {
                doc_id: f.doc_id,
                score: f.score,
            })
            .collect(),
    );

    // The structured branch(es) — each conjoins the SAME ACL clause first (exact-match equality).
    for sc in &plan.structured {
        match sc {
            crate::compiler::StructuredClause::Cmp { field, value, .. } => {
                record(backend.search_structured(
                    acl_filter,
                    field,
                    &to_field_value(value),
                    fetch,
                )?);
                stats.engine_branches.fetch_add(1, Ordering::Relaxed);
            }
            crate::compiler::StructuredClause::In { field, values, .. } => {
                // An `In` is the disjunction of equalities over one field — run each value as a
                // structured branch under the SAME ACL filter and union the hits.
                for v in values {
                    record(backend.search_structured(
                        acl_filter,
                        field,
                        &to_field_value(v),
                        fetch,
                    )?);
                    stats.engine_branches.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    // A pure-ACL query (no FT/structured clause AND no executed vector branch — e.g. "everything I
    // can read") still must honour the ACL filter: run an admit-all FT search ("*"-equivalent) so the
    // bounded allow/deny set is the only predicate. The engine's `search` with a match-all text
    // returns the visible docs.
    // (When all three are absent, nothing was recorded yet, so `merged` is empty — the structural
    // condition below is exactly "no relevance/structured/vector clause".)
    if plan.ft.is_empty() && plan.structured.is_empty() && vector_ranked.is_empty() {
        record(backend.search(acl_filter, "*", fetch)?);
        stats.engine_branches.fetch_add(1, Ordering::Relaxed);
    }

    // Sort by score desc, then doc_id asc (a stable deterministic order — the RRF fusion is applied
    // above; the tuned re-rank is SRCH-P26).
    let mut hits: Vec<Hit> = merged
        .into_iter()
        .map(|(doc_id, score)| Hit { doc_id, score })
        .collect();
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
    hits.into_iter()
        .skip(page.offset)
        .take(page.effective_limit())
        .collect()
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
    use myelin_identity::{ConsistencyMode, Literal, ObjectId, PrincipalId, PrincipalKind, Zookie};
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
        Principal::stub(
            PrincipalId("p:alice".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        )
    }

    fn consistency() -> Consistency {
        Consistency {
            at_least: Zookie("z0".into()),
            mode: ConsistencyMode::BoundedStale,
        }
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
    /// For the SRCH-P09 relational path it ALSO carries a canned reverse-index answer (the visible-id
    /// set + revision the JOIN resolves) and counts `resolve_relation` calls (the no-N+1 GATE).
    struct FakeAuthz {
        answer: ListObjectsResult,
        calls: AtomicU64,
        /// The canned reverse-index JOIN answer (the visible-id set + served revision). `None` ⇒ the
        /// port has no reverse index (the bounded-set fakes) and a relational leaf is `Unavailable`.
        reverse: Option<ReverseIndexAnswer>,
        /// The number of `resolve_relation` (reverse-index JOIN) calls (the relational no-N+1 GATE).
        resolve_calls: AtomicU64,
    }
    impl FakeAuthz {
        fn new(answer: ListObjectsResult) -> FakeAuthz {
            FakeAuthz {
                answer,
                calls: AtomicU64::new(0),
                reverse: None,
                resolve_calls: AtomicU64::new(0),
            }
        }
        fn ids(ids: &[&str]) -> FakeAuthz {
            FakeAuthz::new(ListObjectsResult::Ids {
                ids: ids.iter().map(|i| ObjectId((*i).into())).collect(),
                zookie: Zookie("z-acl".into()),
            })
        }
        fn filter(set_expr: SetExpr) -> FakeAuthz {
            FakeAuthz::new(ListObjectsResult::Filter {
                set_expr,
                zookie: Zookie("z-acl".into()),
            })
        }
        /// A `Filter{set_expr, zookie}` answer with an explicit zookie (carrying a `@<rev>` suffix
        /// so the watermark can be exercised) + a canned reverse-index JOIN answer.
        fn filter_with(set_expr: SetExpr, zookie: &str, reverse: ReverseIndexAnswer) -> FakeAuthz {
            FakeAuthz {
                answer: ListObjectsResult::Filter {
                    set_expr,
                    zookie: Zookie(zookie.into()),
                },
                calls: AtomicU64::new(0),
                reverse: Some(reverse),
                resolve_calls: AtomicU64::new(0),
            }
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
        fn resolve_relation(
            &self,
            _subject: &Principal,
            _form: &RelationalLeaf,
            _required: &RevisionWatermark,
        ) -> AuthzResult<ReverseIndexAnswer> {
            self.resolve_calls.fetch_add(1, Ordering::Relaxed);
            self.reverse
                .clone()
                .ok_or_else(|| myelin_identity::AuthzError::Unavailable("no reverse index".into()))
        }
    }

    fn corpus() -> TantivyBackend {
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        be.upsert(&doc(
            "acme/issue/PUB-1",
            "deadlock in the scheduler",
            "open",
            5,
        ))
        .unwrap();
        be.upsert(&doc(
            "acme/issue/SECRET-9",
            "deadlock secret incident",
            "open",
            9,
        ))
        .unwrap();
        be.upsert(&doc("acme/issue/OTHER-2", "typo in readme", "closed", 1))
            .unwrap();
        be
    }

    // ---- the bounded-set SetExpr lowering ---------------------------------

    /// A no-reverse-index port + a default watermark — the bounded-set lowering needs neither.
    fn no_reverse() -> FakeAuthz {
        FakeAuthz::ids(&[])
    }
    /// Lower a bounded-set `SetExpr` directly (no reverse index needed).
    fn lower_bounded(set_expr: &SetExpr) -> Result<AclFilter, QueryError> {
        let v = viewer("acme");
        let authz = no_reverse();
        lower_set_expr(
            set_expr,
            &v,
            &authz,
            &RevisionWatermark(0),
            &QueryStats::new(),
        )
    }

    /// **`SetExpr::All` → `AclFilter::All` (no clause); `None` → `AclFilter::None` (short-circuit).**
    #[test]
    fn lower_all_and_none() {
        assert_eq!(lower_bounded(&SetExpr::All).unwrap(), AclFilter::All);
        assert_eq!(lower_bounded(&SetExpr::None).unwrap(), AclFilter::None);
    }

    /// **`SetExpr::Ids` → an allow-set; an EMPTY `Ids` → `None` (deny), never `All`.**
    #[test]
    fn lower_ids_and_empty_ids() {
        let f = lower_bounded(&SetExpr::Ids(vec![
            ObjectId("a".into()),
            ObjectId("b".into()),
        ]))
        .unwrap();
        assert_eq!(f, AclFilter::Ids(vec!["a".into(), "b".into()]));
        // An explicit empty allow-set is DENY (the viewer sees nothing), never a silent widen.
        assert_eq!(
            lower_bounded(&SetExpr::Ids(vec![])).unwrap(),
            AclFilter::None
        );
    }

    /// **`SetExpr::NotIds` → a bounded deny-set; an EMPTY `NotIds` → `All` (excludes nothing).**
    #[test]
    fn lower_not_ids_and_empty_not_ids() {
        let f = lower_bounded(&SetExpr::NotIds(vec![ObjectId("x".into())])).unwrap();
        assert_eq!(f, AclFilter::NotIds(vec!["x".into()]));
        assert_eq!(
            lower_bounded(&SetExpr::NotIds(vec![])).unwrap(),
            AclFilter::All
        );
    }

    // ---- SRCH-P09: the relational reverse-index JOIN + boolean composition --

    fn rev_answer(ids: &[&str], revision: u64) -> ReverseIndexAnswer {
        ReverseIndexAnswer {
            object_ids: ids.iter().map(|s| (*s).to_string()).collect(),
            revision: RevisionWatermark(revision),
        }
    }

    /// **`InRelation` lowers via the reverse-index JOIN to the resolved visible-id set (an `Ids`
    /// membership clause), honouring the watermark — ONE resolve, never widened to `All`.**
    #[test]
    fn relational_in_relation_lowers_via_reverse_index_join() {
        use myelin_identity::{ColRef, RelName};
        let authz = FakeAuthz::filter_with(
            SetExpr::InRelation {
                relation: RelName("reader".into()),
                via_column: ColRef {
                    table: "issue".into(),
                    column: "id".into(),
                },
            },
            "z@7",
            rev_answer(&["acme/issue/PUB-1", "acme/issue/PUB-2"], 7),
        );
        let v = viewer("acme");
        let stats = QueryStats::new();
        let (f, z) = lower_acl(&authz.answer.clone(), &v, &authz, &stats).unwrap();
        assert_eq!(
            f,
            AclFilter::Ids(vec!["acme/issue/PUB-1".into(), "acme/issue/PUB-2".into()]),
            "the JOIN resolves to the visible-id set as an Ids membership clause (not All)"
        );
        assert_eq!(z, "z@7", "the list_objects zookie is threaded through");
        assert_eq!(
            stats.reverse_index_joins(),
            1,
            "exactly ONE reverse-index JOIN (no N+1)"
        );
    }

    /// **`TupleSet` lowers via the reverse-index JOIN; an EMPTY resolved set is `None` (the subject
    /// reaches nothing via this relation), NEVER a silent widen to `All`.**
    #[test]
    fn relational_tuple_set_empty_resolved_is_deny_not_widen() {
        use myelin_identity::AuthzIndexRef;
        let authz = FakeAuthz::filter_with(
            SetExpr::TupleSet {
                index: AuthzIndexRef("authz_visible".into()),
            },
            "z@3",
            rev_answer(&[], 3), // the subject reaches NOTHING
        );
        let v = viewer("acme");
        let (f, _) = lower_acl(&authz.answer.clone(), &v, &authz, &QueryStats::new()).unwrap();
        assert_eq!(
            f,
            AclFilter::None,
            "an empty resolved set ⇒ deny, never widened to All"
        );
    }

    /// **THE REVISION WATERMARK (contract 4.10): a reverse-index revision BELOW the watermark is a
    /// loud `StaleReverseIndex`, never read stale (the JOIN refuses the stale revision).**
    #[test]
    fn relational_stale_reverse_index_revision_is_refused() {
        use myelin_identity::AuthzIndexRef;
        // The list_objects zookie requires watermark 9; the reverse index serves only revision 4.
        let authz = FakeAuthz::filter_with(
            SetExpr::TupleSet {
                index: AuthzIndexRef("ix".into()),
            },
            "z@9",
            rev_answer(&["acme/issue/PUB-1"], 4),
        );
        let v = viewer("acme");
        let err = lower_acl(&authz.answer.clone(), &v, &authz, &QueryStats::new())
            .expect_err("a stale reverse-index revision is refused");
        match err {
            QueryError::StaleReverseIndex {
                required, served, ..
            } => {
                assert_eq!(required, 9);
                assert_eq!(served, 4);
            }
            other => panic!("expected StaleReverseIndex, got {other}"),
        }
    }

    /// **A reverse-index revision AT or ABOVE the watermark is accepted (the watermark is `>=`, not
    /// `>`).** Served revision == required watermark is fresh enough.
    #[test]
    fn relational_revision_at_watermark_is_accepted() {
        use myelin_identity::AuthzIndexRef;
        let authz = FakeAuthz::filter_with(
            SetExpr::TupleSet {
                index: AuthzIndexRef("ix".into()),
            },
            "z@5",
            rev_answer(&["acme/issue/PUB-1"], 5), // exactly at the watermark
        );
        let v = viewer("acme");
        let (f, _) = lower_acl(&authz.answer.clone(), &v, &authz, &QueryStats::new()).unwrap();
        assert_eq!(
            f,
            AclFilter::Ids(vec!["acme/issue/PUB-1".into()]),
            "revision == watermark is fresh"
        );
    }

    /// **`Union`/`Intersect`/`Difference` compose into `Or`/`And`/(`And` + `Not`) over the lowered
    /// sub-clauses (the boolean composition, SRCH-P09).**
    #[test]
    fn boolean_composition_lowers_to_engine_and_or_not() {
        // Union(Ids[a], Ids[b]) → Or([Ids[a], Ids[b]]).
        let u = lower_bounded(&SetExpr::Union(vec![
            SetExpr::Ids(vec![ObjectId("a".into())]),
            SetExpr::Ids(vec![ObjectId("b".into())]),
        ]))
        .unwrap();
        assert_eq!(
            u,
            AclFilter::Or(vec![
                AclFilter::Ids(vec!["a".into()]),
                AclFilter::Ids(vec!["b".into()])
            ])
        );

        // Intersect(All, NotIds[x]) → And([All, NotIds[x]]).
        let i = lower_bounded(&SetExpr::Intersect(vec![
            SetExpr::All,
            SetExpr::NotIds(vec![ObjectId("x".into())]),
        ]))
        .unwrap();
        assert_eq!(
            i,
            AclFilter::And(vec![AclFilter::All, AclFilter::NotIds(vec!["x".into()])])
        );

        // Difference(All, Ids[secret]) → And([All, Not(Ids[secret])]) = everything EXCEPT secret.
        let d = lower_bounded(&SetExpr::Difference(
            Box::new(SetExpr::All),
            Box::new(SetExpr::Ids(vec![ObjectId("secret".into())])),
        ))
        .unwrap();
        assert_eq!(
            d,
            AclFilter::And(vec![
                AclFilter::All,
                AclFilter::Not(Box::new(AclFilter::Ids(vec!["secret".into()])))
            ])
        );
    }

    /// **A relational leaf with NO reverse index wired is `Unavailable` (deny-when-unsure), NEVER a
    /// silent widen — the default `resolve_relation` fails closed.**
    #[test]
    fn relational_without_reverse_index_fails_closed() {
        use myelin_identity::AuthzIndexRef;
        let authz = no_reverse(); // no reverse index
        let v = viewer("acme");
        let err = lower_set_expr(
            &SetExpr::TupleSet {
                index: AuthzIndexRef("ix".into()),
            },
            &v,
            &authz,
            &RevisionWatermark(0),
            &QueryStats::new(),
        )
        .expect_err("no reverse index ⇒ fail closed, never widen");
        assert!(
            matches!(err, QueryError::Authz(_)),
            "unavailable surfaces, never widens to All"
        );
    }

    /// **`ListObjectsResult::Ids` (the materialised S4 path) lowers to an allow-set; the threaded
    /// zookie is carried.** An empty materialised set is `None` (deny).
    #[test]
    fn lower_materialised_ids_result() {
        let v = viewer("acme");
        let authz = no_reverse();
        let stats = QueryStats::new();
        let (f, z) = lower_acl(
            &ListObjectsResult::Ids {
                ids: vec![ObjectId("d1".into())],
                zookie: Zookie("zX".into()),
            },
            &v,
            &authz,
            &stats,
        )
        .unwrap();
        assert_eq!(f, AclFilter::Ids(vec!["d1".into()]));
        assert_eq!(z, "zX");
        let (empty, _) = lower_acl(
            &ListObjectsResult::Ids {
                ids: vec![],
                zookie: Zookie("z".into()),
            },
            &v,
            &authz,
            &stats,
        )
        .unwrap();
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
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var(FT_BODY_FIELD),
                rhs: s("deadlock"),
            }),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("query");
        assert_eq!(
            stats.list_objects_calls(),
            1,
            "EXACTLY one list_objects per query (no N+1)"
        );
        assert_eq!(
            authz.calls.load(Ordering::Relaxed),
            1,
            "the port saw exactly one call"
        );
        // A BOUNDED-SET query issues ZERO reverse-index JOINs (the JOIN is only for the relational
        // forms) — distinguishes the join counter's 0 from the relational path's 1 (kills the
        // constant-`1` accessor mutant).
        assert_eq!(
            stats.reverse_index_joins(),
            0,
            "a bounded-set (Ids) query does NO reverse-index JOIN"
        );
        // Only PUB-1 is both in the allow-set AND matches `deadlock` (SECRET-9 is excluded by ACL).
        assert_eq!(
            res.hits
                .iter()
                .map(|h| h.doc_id.as_str())
                .collect::<Vec<_>>(),
            ["acme/issue/PUB-1"]
        );
        assert_eq!(
            res.zookie, "z-acl",
            "the list_objects zookie is threaded onto the result"
        );
    }

    /// **THE CONJOIN-INTO-EVERY-BRANCH GATE + the chained grant test: index a public + a confidential
    /// doc → query as an unauthorized viewer (the allow-set EXCLUDES the confidential doc) → grant →
    /// re-query (now visible).** The ACL clause conjoins into the FT branch BEFORE scoring — the
    /// hidden doc never enters the candidate set (no count/IDF leak).
    #[test]
    fn acl_conjoins_into_branch_unauthorized_then_granted() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let q = ast(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var(FT_BODY_FIELD),
            rhs: s("deadlock"),
        });

        // UNAUTHORIZED: the allow-set excludes SECRET-9 — it does NOT surface even though it matches.
        let unauth = FakeAuthz::ids(&["acme/issue/PUB-1"]);
        let stats = QueryStats::new();
        let res = query(
            &eng,
            &unauth,
            &q,
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("q");
        let ids: Vec<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert_eq!(
            ids,
            ["acme/issue/PUB-1"],
            "the confidential doc is excluded (pre-filter, no leak)"
        );

        // GRANTED: the allow-set now includes SECRET-9 — re-query, it is visible.
        let granted = FakeAuthz::ids(&["acme/issue/PUB-1", "acme/issue/SECRET-9"]);
        let stats2 = QueryStats::new();
        let res2 = query(
            &eng,
            &granted,
            &q,
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats2,
        )
        .expect("q2");
        let ids2: std::collections::BTreeSet<&str> =
            res2.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert!(
            ids2.contains("acme/issue/SECRET-9"),
            "after grant the confidential doc is visible"
        );
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
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var(FT_BODY_FIELD),
                rhs: s("deadlock"),
            }),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("query");
        assert!(res.hits.is_empty(), "None ⇒ empty result");
        assert_eq!(
            stats.engine_branches(),
            0,
            "no engine branch ran (short-circuit, no count leak)"
        );
        assert_eq!(
            stats.list_objects_calls(),
            1,
            "still exactly one list_objects call"
        );
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
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var(FT_BODY_FIELD),
                rhs: s("deadlock"),
            }),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("query");
        let ids: std::collections::BTreeSet<&str> =
            res.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert!(
            ids.contains("acme/issue/PUB-1") && ids.contains("acme/issue/SECRET-9"),
            "admin sees both `deadlock` docs: {ids:?}"
        );
    }

    /// **`SetExpr::NotIds` (the bounded deny-set) hides exactly the denied docs (`WHERE NOT IN`).**
    #[test]
    fn not_ids_deny_set_hides_only_the_denied() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        // Deny SECRET-9 only — PUB-1 still matches `deadlock`.
        let authz = FakeAuthz::filter(SetExpr::NotIds(vec![ObjectId(
            "acme/issue/SECRET-9".into(),
        )]));
        let stats = QueryStats::new();
        let res = query(
            &eng,
            &authz,
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var(FT_BODY_FIELD),
                rhs: s("deadlock"),
            }),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("query");
        let ids: Vec<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert_eq!(
            ids,
            ["acme/issue/PUB-1"],
            "the denied doc is excluded, the rest surface"
        );
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
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("status"),
                rhs: s("open"),
            }),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("query");
        assert_eq!(
            res.hits
                .iter()
                .map(|h| h.doc_id.as_str())
                .collect::<Vec<_>>(),
            ["acme/issue/PUB-1"],
            "the structured branch excludes the ACL-denied doc"
        );
        assert!(stats.engine_branches() >= 1, "a structured branch ran");
    }

    /// **THE SRCH-P09 CHAINED GRANT (the big-result path): a confidential + a public doc reachable
    /// only via a `TupleSet` relation → query as an UNAUTHORIZED viewer (the reverse-index JOIN
    /// resolves to ONLY the public doc) → 0 leak incl. count → grant (the JOIN now resolves the
    /// confidential doc too) → re-query, now visible.** The whole query runs through the engine with
    /// the conjoined relational `Ids` clause; exactly ONE reverse-index JOIN (no N+1).
    #[test]
    fn relational_tuple_set_chained_grant_through_query_no_leak() {
        use myelin_identity::AuthzIndexRef;
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let q = ast(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var(FT_BODY_FIELD),
            rhs: s("deadlock"),
        });
        let tuple_set = SetExpr::TupleSet {
            index: AuthzIndexRef("authz_visible".into()),
        };

        // UNAUTHORIZED: the reverse-index JOIN resolves to ONLY PUB-1 (SECRET-9 unreachable) at
        // revision 5; the watermark from `z@5` is 5 — fresh enough.
        let unauth = FakeAuthz::filter_with(
            tuple_set.clone(),
            "z@5",
            rev_answer(&["acme/issue/PUB-1"], 5),
        );
        let stats = QueryStats::new();
        let res = query(
            &eng,
            &unauth,
            &q,
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("q");
        let ids: Vec<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert_eq!(
            ids,
            ["acme/issue/PUB-1"],
            "the confidential doc is excluded (no leak, big-result path)"
        );
        assert_eq!(
            stats.reverse_index_joins(),
            1,
            "exactly ONE reverse-index JOIN (no N+1)"
        );
        assert_eq!(
            stats.list_objects_calls(),
            1,
            "and exactly one list_objects"
        );

        // GRANTED: the JOIN now resolves SECRET-9 too (at a fresher revision 6, zookie z@6).
        let granted = FakeAuthz::filter_with(
            tuple_set,
            "z@6",
            rev_answer(&["acme/issue/PUB-1", "acme/issue/SECRET-9"], 6),
        );
        let stats2 = QueryStats::new();
        let res2 = query(
            &eng,
            &granted,
            &q,
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats2,
        )
        .expect("q2");
        let ids2: std::collections::BTreeSet<&str> =
            res2.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert!(
            ids2.contains("acme/issue/SECRET-9"),
            "after grant the confidential doc is visible"
        );
        assert!(ids2.contains("acme/issue/PUB-1"));
    }

    /// **A `Difference(All, TupleSet)` through `query`: everything EXCEPT the reachable-via-relation
    /// set — the boolean composition conjoins into the engine branch (`All AND NOT Ids`).** A doc in
    /// the difference's excluded set never surfaces.
    #[test]
    fn relational_difference_through_query_excludes_the_relation_set() {
        use myelin_identity::AuthzIndexRef;
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        let q = ast(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var(FT_BODY_FIELD),
            rhs: s("deadlock"),
        });
        // Difference(All, TupleSet) — everything EXCEPT what the relation reaches; the relation
        // reaches SECRET-9, so SECRET-9 is excluded and PUB-1 (which matches `deadlock`) surfaces.
        let set_expr = SetExpr::Difference(
            Box::new(SetExpr::All),
            Box::new(SetExpr::TupleSet {
                index: AuthzIndexRef("blocked".into()),
            }),
        );
        let authz =
            FakeAuthz::filter_with(set_expr, "z@2", rev_answer(&["acme/issue/SECRET-9"], 2));
        let stats = QueryStats::new();
        let res = query(
            &eng,
            &authz,
            &q,
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("q");
        let ids: Vec<&str> = res.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert_eq!(
            ids,
            ["acme/issue/PUB-1"],
            "the relation-reached doc is excluded by the Difference"
        );
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
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var(FT_BODY_FIELD),
                rhs: s("deadlock"),
            }),
            &evil,
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect_err("a cross-tenant query is rejected (SRCH-D3)");
        assert!(
            matches!(err, QueryError::TenantMismatch { .. }),
            "cross-tenant ⇒ TenantMismatch"
        );
        // 0 engine branches ran AND 0 list_objects calls — the cross-tenant query never reaches the
        // engine or even the authz dependency (rejected at the partition-key check).
        assert_eq!(stats.engine_branches(), 0, "0 cross-tenant engine touches");
        assert_eq!(
            stats.list_objects_calls(),
            0,
            "rejected before any authz/engine work"
        );
        assert!(
            err.to_string().contains("SRCH-D3"),
            "the error names the drill"
        );
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
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var(FT_BODY_FIELD),
                rhs: s("deadlock"),
            }),
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
        assert_eq!(
            res.post_fetch_fields,
            vec!["progress".to_string()],
            "the read-time predicate is carried for post-fetch evaluation by the view"
        );
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
            &ast(Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var(FT_BODY_FIELD),
                rhs: s("deadlock"),
            }),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect_err("an authz failure is surfaced, not widened");
        assert!(
            matches!(err, QueryError::Authz(_)),
            "the authz error surfaces loudly"
        );
        assert_eq!(
            stats.engine_branches(),
            0,
            "no engine query ran on an authz failure"
        );
    }

    /// **Pagination slices the window and clamps a crafted huge `limit` (no unbounded top-k).**
    #[test]
    fn pagination_slices_and_clamps_limit() {
        let hits: Vec<Hit> = (0..10)
            .map(|i| Hit {
                doc_id: format!("d{i:02}"),
                score: (10 - i) as f32,
            })
            .collect();
        let page = Page {
            offset: 2,
            limit: 3,
        };
        let sliced = paginate(hits.clone(), page);
        assert_eq!(
            sliced.iter().map(|h| h.doc_id.clone()).collect::<Vec<_>>(),
            ["d02", "d03", "d04"],
            "the page window is offset..offset+limit"
        );
        // A crafted huge limit is clamped to MAX_LIMIT (not honoured verbatim).
        let huge = Page {
            offset: 0,
            limit: usize::MAX,
        };
        assert_eq!(
            huge.effective_limit(),
            Page::MAX_LIMIT,
            "a crafted limit is clamped"
        );
    }

    /// **`ScopedEngine::tenant`/`region` expose the partition key they were opened for (kills the
    /// accessor mutants).** The accessors back the SRCH-D3 partition-key check.
    #[test]
    fn scoped_engine_exposes_its_partition_key() {
        let be = corpus();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        assert_eq!(
            eng.tenant(),
            "acme",
            "the tenant accessor returns the opened tenant verbatim"
        );
        assert_eq!(
            eng.region(),
            "eu-west",
            "the region accessor returns the opened region verbatim"
        );
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
                Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: var(FT_BODY_FIELD),
                    rhs: s("deadlock"),
                },
                Predicate::Cmp {
                    op: CmpOp::Eq,
                    lhs: var("status"),
                    rhs: s("open"),
                },
            ])),
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            Page::FIRST,
            &stats,
        )
        .expect("query");
        let pub1 = res
            .hits
            .iter()
            .find(|h| h.doc_id == "acme/issue/PUB-1")
            .expect("PUB-1 surfaces");
        assert!(
            pub1.score > 0.0,
            "the fused score is the MAX (the BM25 FT score), not the structured branch's 0.0: {}",
            pub1.score
        );
        assert!(
            stats.engine_branches() >= 2,
            "both the FT and structured branches ran"
        );
    }

    // ---- SRCH-P11 (P-174): hybrid + vector — RRF fusion + filter-during-traversal --------------

    use crate::compiler::SEMANTIC_FIELD;
    use crate::indexer::MockEmbeddingAdapter;
    use crate::vector::Embedding;
    use crate::EmbeddingAdapter;

    /// An embedded corpus: each doc carries a vector under the SAME mock model as the query embedder,
    /// so a query embedding and the docs live in ONE vector space (§3.3). `embed(text)` is the mock
    /// adapter's deterministic embedding — the doc and a query of the same text are identical vectors.
    fn embedded_corpus(embedder: &MockEmbeddingAdapter) -> TantivyBackend {
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let mut emb = |id: &str, body: &str, status: &str| {
            let v = embedder.embed(body).expect("non-empty body embeds");
            let k = OrderKey::bisect(None, None);
            let d = IndexDocument::new(id, body)
                .with_field("status", FieldValue::Select(status.into()))
                .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k))
                .with_embedding(v, embedder.model_ref());
            be.upsert(&d).unwrap();
        };
        emb("acme/issue/PUB-1", "deadlock in the scheduler", "open");
        emb("acme/issue/PUB-2", "deadlock in the indexer", "open");
        emb("acme/issue/SECRET-9", "deadlock secret ops runbook", "open");
        be
    }

    /// A pure-semantic AST (`__semantic__ == query_text`) — lowers to the vector branch only.
    fn semantic_ast(query_text: &str) -> QueryAst {
        ast(Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: var(SEMANTIC_FIELD),
            rhs: s(query_text),
        })
    }

    /// A hybrid AST: an FT clause AND a semantic clause over ONE compiled plan (one doc-id space).
    fn hybrid_ast(ft_text: &str, semantic_text: &str) -> QueryAst {
        ast(Predicate::And(vec![
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var(FT_BODY_FIELD),
                rhs: s(ft_text),
            },
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var(SEMANTIC_FIELD),
                rhs: s(semantic_text),
            },
        ]))
    }

    /// **SRCH-D1 (the vector/RAG leak half): a confidential doc NEVER appears in a semantic result
    /// for an unauthorized viewer — filter-during-traversal returns k VISIBLE neighbours.** The
    /// query text matches the SECRET-9 doc exactly (its nearest neighbour), but the allow-set
    /// excludes it: it never enters the candidate set (no count/rank leak through the vector/RAG
    /// path). Then GRANT → re-search: it surfaces (the visible neighbours grew).
    #[test]
    fn semantic_filter_during_traversal_excludes_confidential_then_grant_makes_visible() {
        let embedder = MockEmbeddingAdapter::new(16);
        let be = embedded_corpus(&embedder);
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        // The query is the EXACT text of the secret doc — so SECRET-9 is its nearest vector.
        let vq = VectorQuery::Text {
            text: "deadlock secret ops runbook".into(),
            embedder: &embedder,
        };
        let q = semantic_ast("deadlock secret ops runbook");

        // UNAUTHORIZED: the allow-set EXCLUDES SECRET-9 (its nearest neighbour).
        let unauth = FakeAuthz::ids(&["acme/issue/PUB-1", "acme/issue/PUB-2"]);
        let stats = QueryStats::new();
        let cstats = crate::consistency::ConsistencyStats::new();
        let res = semantic(
            &eng,
            &unauth,
            None,
            &q,
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            &vq,
            Page::FIRST,
            &stats,
            &cstats,
        )
        .expect("semantic");
        let ids: std::collections::BTreeSet<&str> =
            res.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert!(
            !ids.contains("acme/issue/SECRET-9"),
            "the confidential doc NEVER surfaces in the semantic/RAG result (SRCH-D1 vector half: 0 leak)"
        );
        assert!(
            ids.contains("acme/issue/PUB-1") && ids.contains("acme/issue/PUB-2"),
            "visible neighbours"
        );
        assert_eq!(
            stats.list_objects_calls(),
            1,
            "exactly ONE list_objects (no N+1 on the semantic path)"
        );

        // GRANT SECRET-9 → re-search: now it is one of the visible neighbours (the nearest, in fact).
        let granted = FakeAuthz::ids(&[
            "acme/issue/PUB-1",
            "acme/issue/PUB-2",
            "acme/issue/SECRET-9",
        ]);
        let stats2 = QueryStats::new();
        let cstats2 = crate::consistency::ConsistencyStats::new();
        let res2 = semantic(
            &eng,
            &granted,
            None,
            &q,
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            &vq,
            Page::FIRST,
            &stats2,
            &cstats2,
        )
        .expect("semantic after grant");
        let ids2: std::collections::BTreeSet<&str> =
            res2.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert!(
            ids2.contains("acme/issue/SECRET-9"),
            "after grant the doc is in the visible neighbours"
        );
    }

    /// **The `vec` form of contract 6.2: a directly-supplied query vector is searched
    /// filter-during-traversal (the agent-RAG shape — an agent passes an embedding directly).** The
    /// same leak-free property holds.
    #[test]
    fn semantic_accepts_a_directly_supplied_query_vector() {
        let embedder = MockEmbeddingAdapter::new(16);
        let be = embedded_corpus(&embedder);
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        // Embed the query text OURSELVES and pass the vector (the `vec` form).
        let query_vec: Embedding = embedder.embed("deadlock in the scheduler").unwrap();
        let vq = VectorQuery::Vec(query_vec);
        let q = semantic_ast("ignored — the vec form supplies the embedding");

        let authz = FakeAuthz::ids(&["acme/issue/PUB-1", "acme/issue/PUB-2"]);
        let stats = QueryStats::new();
        let cstats = crate::consistency::ConsistencyStats::new();
        let res = semantic(
            &eng,
            &authz,
            None,
            &q,
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            &vq,
            Page::FIRST,
            &stats,
            &cstats,
        )
        .expect("semantic");
        assert_eq!(
            res.hits[0].doc_id, "acme/issue/PUB-1",
            "the exact-text doc is the nearest visible vector"
        );
    }

    /// **RRF fusion of a HYBRID query introduces no hidden doc + fuses the lexical + semantic ranked
    /// lists (§4.5).** Both branches carry the SAME conjoined ACL filter; the confidential doc is in
    /// neither branch's list, so fusion cannot introduce it. The doc both branches rank surfaces.
    #[test]
    fn hybrid_rrf_fusion_no_hidden_doc_and_fuses_both_branches() {
        let embedder = MockEmbeddingAdapter::new(16);
        let be = embedded_corpus(&embedder);
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());
        // FT "deadlock" matches all three; semantic "deadlock in the scheduler" is nearest PUB-1.
        let vq = VectorQuery::Text {
            text: "deadlock in the scheduler".into(),
            embedder: &embedder,
        };
        let q = hybrid_ast("deadlock", "deadlock in the scheduler");

        // Allow-set EXCLUDES SECRET-9 — it is in NEITHER branch's list, so RRF cannot fuse it in.
        let authz = FakeAuthz::ids(&["acme/issue/PUB-1", "acme/issue/PUB-2"]);
        let stats = QueryStats::new();
        let cstats = crate::consistency::ConsistencyStats::new();
        let res = semantic(
            &eng,
            &authz,
            None,
            &q,
            &viewer("acme"),
            &ObjectType("issue".into()),
            &consistency(),
            &vq,
            Page::FIRST,
            &stats,
            &cstats,
        )
        .expect("hybrid");
        let ids: std::collections::BTreeSet<&str> =
            res.hits.iter().map(|h| h.doc_id.as_str()).collect();
        assert!(
            !ids.contains("acme/issue/SECRET-9"),
            "RRF introduces no hidden doc (SRCH-D1 vector half)"
        );
        // PUB-1 is rank-high in BOTH the FT and the vector branch (exact semantic match) — the RRF
        // agreement boost ranks it first.
        assert_eq!(
            res.hits[0].doc_id, "acme/issue/PUB-1",
            "the doc both branches rank fuses to the top (RRF)"
        );
        // Both branches ran (FT + vector) plus the list_objects is still exactly one.
        assert!(
            stats.engine_branches() >= 2,
            "the FT and the vector branch both executed"
        );
        assert_eq!(
            stats.list_objects_calls(),
            1,
            "ONE list_objects for the hybrid query (no N+1)"
        );
    }

    /// **The semantic surface reuses the SRCH-P10 zookie path (no-stale-grant for RAG too).** A
    /// vector candidate whose `indexed_zookie` is STALE relative to the demanded strong zookie, with
    /// NO bounded-check port wired, is EXCLUDED pending re-index (fail closed) — exactly as a lexical
    /// hit would be. RAG never serves a stale-granted doc.
    #[test]
    fn semantic_reuses_the_no_stale_grant_zookie_path() {
        use myelin_identity::ConsistencyMode;
        let embedder = MockEmbeddingAdapter::new(16);
        // A backend where the docs are indexed at an OLD zookie; the query demands a NEWER strong one.
        let mut be = TantivyBackend::open(&facet_decl()).expect("open");
        let v = embedder.embed("deadlock in the scheduler").unwrap();
        let k = OrderKey::bisect(None, None);
        let d = IndexDocument::new("acme/issue/PUB-1", "deadlock in the scheduler")
            .with_field("status", FieldValue::Select("open".into()))
            .with_field(ORDER_KEY_FIELD, FieldValue::OrderKey(k))
            .with_embedding(v, embedder.model_ref());
        // Stamp it at an OLD zookie revision (rev 1).
        be.upsert_stamped(&d, "z@1", 1).unwrap();
        let eng = ScopedEngine::new(&be, "acme", "eu-west", schema());

        let vq = VectorQuery::Text {
            text: "deadlock in the scheduler".into(),
            embedder: &embedder,
        };
        let q = semantic_ast("deadlock in the scheduler");
        let authz = FakeAuthz::ids(&["acme/issue/PUB-1"]);
        // A STRONG read demanding zookie rev 9 — newer than the doc's indexed rev 1 (stale).
        let strong = Consistency {
            at_least: myelin_identity::Zookie("z@9".into()),
            mode: ConsistencyMode::Strong,
        };
        let stats = QueryStats::new();
        let cstats = crate::consistency::ConsistencyStats::new();
        // NO bounded-check port → the stale candidate is EXCLUDED (fail closed) — RAG serves nothing stale.
        let res = semantic(
            &eng,
            &authz,
            None,
            &q,
            &viewer("acme"),
            &ObjectType("issue".into()),
            &strong,
            &vq,
            Page::FIRST,
            &stats,
            &cstats,
        )
        .expect("semantic strong");
        assert!(
            res.hits.is_empty(),
            "the stale-indexed vector candidate is excluded pending re-index (no-stale-grant for RAG; fail closed)"
        );
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
        let stale = QueryError::StaleReverseIndex {
            required: 9,
            served: 4,
            form: "TupleSet",
        };
        let sm = stale.to_string();
        assert!(
            sm.contains("TupleSet") && sm.contains("SRCH-P09"),
            "the stale-revision error is loud"
        );
        assert!(
            sm.contains('9') && sm.contains('4'),
            "it names the required + served revisions"
        );
    }
}

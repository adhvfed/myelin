//! # `surfacing` — the CI cross-fabric surfacing read+ref half (CI-P25 / P-368, M4)
//!
//! This is CI's **cross-fabric surfacing** module — the read side (the leak-free `list_objects`
//! `SetExpr` push-down over `ci_run.run_id`) + the ref side (the `ArtifactRef` / `#sub` mints + the
//! per-viewer `project(ref, viewer)` projection). It is the **only** way Refs/Search/Notif/Chat read
//! about a CI artifact (no cross-DB read), per-viewer pre-permission-checked.
//!
//! **Owning architecture docs (read in full before changing this):**
//! - `continuous-integration/architecture/03-events-contracts-and-glue.md`
//!   - §5.1 — the `list_objects` `SetExpr` push-down over `ci_run.run_id` (the OQ-E JOIN against the
//!     per-tenant `authz_visible` reverse index: ONE query, NO N+1 per-row check, NO post-filter; the
//!     `search-requires-acl-filter` lint conjoins the `Filter` before scoring);
//!   - §7.1 — the `ArtifactRef` + the `#sub` mints (`step-<n>`, `check-<context>`, `L<a>-L<b>`);
//!     `#step-<n>` ids are opaque and STABLE across retries; Refs stores the full sub-URN + the
//!     stripped root, so a broken sub-anchor still resolves to the parent run;
//!   - §7.2 — `project(ref, viewer) -> {title, state, icon, render_hint, sub_anchor?}` (the only
//!     cross-DB read of a CI artifact — permission FIRST, deny ⇒ a `Tombstone`, never a leak).
//! - `00-reconciliation-decisions.md` §OQ-E (the `SetExpr` push-down), §X-4/OQ-D (the `#sub` grammar
//!   + the 4-step tombstone ladder).
//! - `external-insights/01-process-and-quality-doctrine.md` §3 (prove-it — the leak-free pre-filter
//!   is a quantified property; 0 leaked rows), §5 (the `search-requires-acl-filter` lint is a
//!   committed gate).
//!
//! **Contracts implemented (to the FROZEN shapes — escalate a needed change, do not diverge):**
//! - **5.1** the CI `ArtifactRef` mints — `myelin://<t>/ci/<type>/<id>[#sub]` with the canonical `ci`
//!   token ([`ci_run_ref`], [`ci_deployment_ref`], [`ci_pipeline_ref`], [`ci_runner_ref`],
//!   [`ci_artifact_ref`]); through the ONE [`myelin_refs`] codec (0 ungrammatical refs by
//!   construction).
//! - **5.7** the ci-owned `#sub` mints — `step-<n>` (jump-to-failure, resolves
//!   `CheckStatus.details_ref`) + `check-<context>` + `L<a>-L<b>` line-ranges ([`run_step_ref`],
//!   [`run_step_line_ref`], [`commit_check_ref`]).
//! - **5.6** `project(ref, viewer)` — the per-viewer permission-checked projection ([`Projector`]).
//! - **4.3** (CONSUMED) the `list_objects` `SetExpr` push-down — lowered over `ci_run.run_id`
//!   ([`compose_run_list_query`], [`run_search_pre_filter`]).
//!
//! ## Reconciliation with the existing CI code (EI-01 §7 coherence)
//! CI already mints the `#step-<n>` `details_ref` in [`crate::check_emitter::details_ref`] and parses
//! it in [`crate::live_tail::parse_step_ref`]. This module does NOT re-author either: the `details_ref`
//! anchor `…/ci/run/<id>#step-<n>` is exactly the [`run_step_ref`] mint here, so [`details_ref`]'s
//! string form and this module's mint stay byte-identical (the `details_ref_uses_the_step_mint` test
//! pins it). The list_objects lowering REUSES the FROZEN `SetExpr` algebra + the `authz_visible` JOIN
//! discipline the Git/Knowledge consumers already prove (`myelin_git::list_filter`) — restated over
//! CI's own `ci_run.run_id` column because `myelin-ci-controlplane` is a producer LEAF that cannot
//! depend on the Identity SERVICE crate (the §2.9 acyclic DAG); the SHAPE is the wire contract.
//!
//! ## FLOOR named (per the prompt)
//! `declare_indexable` + the `humanise` registrations + `replay(*.snapshot)` + the agent `ToolDef`
//! registrations are **CI-P26** (the follow-on cross-fabric surfacing prompt). This prompt ships the
//! read+ref half only (the push-down + the mints + `project`).
//!
//! ## Mutation-score floor (mandatory-core — EI-01 §3 / prove-it)
//! The leak surface is mandatory-core (a leak IS the failure). The floor for this module is
//! **≥ 80% of viable mutants caught** (`cargo mutants -p myelin-ci-controlplane -f
//! crates/myelin-ci-controlplane/src/surfacing.rs`). The load-bearing logic — the permission-first
//! gate in [`Projector::project`] (deny ⇒ tombstone), each projection arm, the erased/restricted
//! `||`-over-(root, sub-ref) tombstone, the `None`/empty-`Ids` ⇒ `FALSE` leak-free lowering, the
//! `canonical_id` subsystem-token check, the `#sub` mint stability — each has a test a mutation flips.
//!
//! **Measured 2026-06-23: 130 caught / 156 viable = 83.3% (≥ 80% — floor MET).** Every SECURITY
//! load-bearing mutant is caught: the permission-deny gate (`unauthorized_viewer_gets_a_tombstone_…`),
//! the erased/restricted `|| → &&` over the sub-ref (`an_erased_step_subref_tombstones_…` /
//! `a_restricted_step_subref_…`), the `None`/empty-`Ids` ⇒ `FALSE` leak-free elements, the
//! Difference/`AND NOT` fork exclusion, the `canonical_id` `!= → ==` subsystem check. The residual 16
//! misses are NOT the production leak surface: they are the in-memory **test-model boolean evaluator**
//! (`tokenize`/`parse_or`/`parse_and`/`parse_unary`/`parse_primary` — the recursive-descent parser
//! that models the SQL `WHERE` for the unit/CDC drills; the production path is the DATABASE evaluating
//! the same predicate, proven in `integration_ci_p25_list_pushdown.rs`), plus a handful of EQUIVALENT
//! mutants (`statement_count -> 1` is equivalent on a single-statement SQL; `depends_on_reverse_index
//! -> true`; the `store_mut` test accessor) — none affect the leak-free / tombstone guarantees.

use crate::deployment::DeployState;
use myelin_identity::{
    ColRef, Consistency, ConsistencyMode, Decision, IdentityService, ObjectId, Permission,
    Principal, SetExpr, Zookie,
};
use myelin_refs::{ArtifactRef, Sub};
use myelin_tenancy::{Region, TenantId};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 0. FROZEN NAMES (§5.1 / §7.1 / §7.3 — never a stray literal)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// The canonical CI subsystem token in the `myelin://<t>/ci/<type>/<id>` URN (Bus §6.2). Named once.
pub const CI_SUBSYSTEM: &str = "ci";

/// The `view` permission `project` checks before reading a CI artifact (§5.2 / §7.2: `run.view =
/// parent_repo->pull`). Spelled once so the projector keys on the one canonical string (mirrors
/// [`crate::rebac_fragment::VIEW`]).
pub const VIEW: &str = "view";

/// The `read` permission the run-list / search push-down pre-filters with (§5.1:
/// `list_objects(viewer, read, ci_run)`). The frozen OQ-E key — the run list is "the runs the viewer
/// may `read`" (the `read & !is_untrusted_fork` ABAC edge resolves on the Identity engine side).
pub const RUN_LIST_PERMISSION: &str = "read";

/// CI's OWN run id column the run-list `SetExpr` lowers over (§5.1 / §7.3: `ci_run.run_id` —
/// `ColRef{table:"ci_run", column:"run_id"}`). The FROZEN `(table, column)` pair, named in ONE place.
pub fn ci_run_id_colref() -> ColRef {
    ColRef {
        table: "ci_run".into(),
        column: "run_id".into(),
    }
}

/// The per-tenant, residency-pinned authz reverse-index table the `InRelation`/`TupleSet` forms JOIN
/// against (§5.1 / OQ-E — Identity's materialised `(subject, relation, object_id)` projection kept
/// fresh off the bus). A named constant so the lowered JOIN names the FROZEN table.
pub const AUTHZ_VISIBLE_TABLE: &str = "authz_visible";

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 1. THE ArtifactRef MINTS + the #sub mints (contracts 5.1 / 5.7, §7.1)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// The frozen CI artifact types `project` projects + mints (the `<type>` token of the canonical
/// `ArtifactRef`, §7.1). A closed set — CI is the resolver-owner of exactly these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiArtifactType {
    /// `ci/run/<run_id>` — a run (the single-run view).
    Run,
    /// `ci/deployment/<dep_id>` — a deployment.
    Deployment,
    /// `ci/pipeline/<pipeline_id>` — a pipeline definition.
    Pipeline,
    /// `ci/runner/<runner_id>` — a runner.
    Runner,
    /// `ci/artifact/<artifact_id>` — a build artifact.
    Artifact,
}

impl CiArtifactType {
    /// The `<type>` token as it appears in the URN.
    pub const fn token(self) -> &'static str {
        match self {
            CiArtifactType::Run => "run",
            CiArtifactType::Deployment => "deployment",
            CiArtifactType::Pipeline => "pipeline",
            CiArtifactType::Runner => "runner",
            CiArtifactType::Artifact => "artifact",
        }
    }
}

/// Mint the canonical **run** `ArtifactRef`: `myelin://<tenant>/ci/run/<run_id>` (§7.1). The
/// `<run_id>` is CI's STABLE canonical key (the same key the §5.1 push-down lowers over — `ci_run.run_id`).
pub fn ci_run_ref(tenant: &str, run_id: &str) -> ArtifactRef {
    mint_root(tenant, CiArtifactType::Run, run_id)
}

/// Mint the canonical **deployment** `ArtifactRef`: `myelin://<tenant>/ci/deployment/<dep_id>`.
pub fn ci_deployment_ref(tenant: &str, dep_id: &str) -> ArtifactRef {
    mint_root(tenant, CiArtifactType::Deployment, dep_id)
}

/// Mint the canonical **pipeline** `ArtifactRef`: `myelin://<tenant>/ci/pipeline/<pipeline_id>`.
pub fn ci_pipeline_ref(tenant: &str, pipeline_id: &str) -> ArtifactRef {
    mint_root(tenant, CiArtifactType::Pipeline, pipeline_id)
}

/// Mint the canonical **runner** `ArtifactRef`: `myelin://<tenant>/ci/runner/<runner_id>`.
pub fn ci_runner_ref(tenant: &str, runner_id: &str) -> ArtifactRef {
    mint_root(tenant, CiArtifactType::Runner, runner_id)
}

/// Mint the canonical **artifact** `ArtifactRef`: `myelin://<tenant>/ci/artifact/<artifact_id>`.
pub fn ci_artifact_ref(tenant: &str, artifact_id: &str) -> ArtifactRef {
    mint_root(tenant, CiArtifactType::Artifact, artifact_id)
}

/// Compose + validate a bare CI root URN through the ONE refs codec (0 ungrammatical mints by
/// construction). CI mints only well-formed scopes (the inputs are validated segments), so a parse
/// failure is an internal invariant break — `expect` surfaces it loudly rather than silently emitting
/// a malformed ref.
fn mint_root(tenant: &str, ty: CiArtifactType, id: &str) -> ArtifactRef {
    myelin_refs::parse(&format!(
        "myelin://{tenant}/{CI_SUBSYSTEM}/{}/{id}",
        ty.token()
    ))
    .expect("CI mints a grammatical canonical ArtifactRef (contract 5.1)")
}

/// **The `#step-<n>` jump-to-failure mint (contract 5.7, §7.1).** Attach a CI-owned `step-<n>`
/// sub-anchor to a RUN root: `myelin://<t>/ci/run/<run_id>#step-<n>`. This is the SAME anchor
/// [`crate::check_emitter::details_ref`] mints as `CheckStatus.details_ref` (one source of truth — no
/// divergent step-anchor grammar). `<n>` is opaque + STABLE across retries (the
/// `log_anchor.step_id` is assigned deterministically from the snapshot, not runtime order), so a
/// chat/runbook embed of `#step-3` never dangles. Goes through the ONE [`myelin_refs::mint`] codec
/// (a non-run root / a sub-of-a-sub is rejected loudly).
pub fn run_step_ref(
    run_ref: &ArtifactRef,
    step: u64,
) -> Result<ArtifactRef, myelin_refs::ParseError> {
    myelin_refs::mint(run_ref, Sub::Step(step))
}

/// **The `#L<a>-L<b>` log line-range mint within a step (contract 5.7).** The assembled-context
/// jump-to-failure ref `…/ci/run/<run_id>#L<a>-L<b>` — a line range within a step's log. Inverted
/// (`end < start`) ranges are rejected by the codec.
pub fn run_step_line_ref(
    run_ref: &ArtifactRef,
    start: u64,
    end: u64,
) -> Result<ArtifactRef, myelin_refs::ParseError> {
    myelin_refs::mint(run_ref, Sub::LineRange { start, end })
}

/// **The `#check-<context>` commit-check mint (contract 5.7, §7.1).** The CI-owned check-status
/// sub-anchor — a check status on a commit, the X-1 / OQ-D check fact. Note the FROZEN canonical
/// home of a `check-<context>` is a **Git-rooted** ref
/// (`myelin://<t>/git/repo/<id>#commit-<oid>/check-<context>`, §7.1); CI mints the `check-` SUB onto
/// whatever root it stamps the check against. This helper mints `check-<context>` onto an arbitrary
/// root through the ONE codec.
pub fn commit_check_ref(
    root: &ArtifactRef,
    context: &str,
) -> Result<ArtifactRef, myelin_refs::ParseError> {
    myelin_refs::mint(root, Sub::Check(context.to_string()))
}

/// Classify a parsed CI `ArtifactRef` to its [`CiArtifactType`], or reject a ref that is not a CI
/// artifact (a non-`ci` subsystem, or a CI type this projector does not own). Reads the
/// `<subsystem>`/`<type>` segments of the canonical URN — never a render-time display form.
fn classify(r: &ArtifactRef) -> Result<CiArtifactType, ProjectError> {
    let rest =
        r.0.strip_prefix("myelin://")
            .ok_or_else(|| ProjectError::NotACiArtifact {
                reference: r.0.clone(),
            })?;
    let scope = rest.split('#').next().unwrap_or(rest);
    let segments: Vec<&str> = scope.split('/').collect();
    if segments.len() != 4 || segments[1] != CI_SUBSYSTEM {
        return Err(ProjectError::NotACiArtifact {
            reference: r.0.clone(),
        });
    }
    match segments[2] {
        "run" => Ok(CiArtifactType::Run),
        "deployment" => Ok(CiArtifactType::Deployment),
        "pipeline" => Ok(CiArtifactType::Pipeline),
        "runner" => Ok(CiArtifactType::Runner),
        "artifact" => Ok(CiArtifactType::Artifact),
        other => Err(ProjectError::UnknownCiType {
            ty: other.to_string(),
        }),
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 2. THE list_objects SetExpr PUSH-DOWN over ci_run.run_id (contract 4.3, §5.1)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// One bound parameter the lowered predicate carries (NEVER a string-interpolated literal — an
/// id/subject/relation an attacker controls can never become SQL; the consumer binds these). The SAME
/// bound-not-interpolated discipline the Identity-side + Git-side lowering enforce (§7.2), restated
/// because `myelin-ci-controlplane` (a producer LEAF) cannot depend on the Identity SERVICE crate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundParam {
    /// The named placeholder in the SQL (`:id_0`, `:subject_0`, `:rel_for_read`).
    pub placeholder: String,
    /// The bound value (an object id / the viewer subject / a relation name) — bound, never interpolated.
    pub value: String,
}

/// One JOIN the lowered predicate requires against the `authz_visible` reverse index (§5.1).
/// Deduplicated by `(viewer, relation)` so the SAME reverse-index JOIN is emitted ONCE — the no-N+1
/// guarantee: an `InRelation`/`TupleSet`, however deeply nested, contributes at most one JOIN per
/// distinct `(viewer, relation)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthzJoin {
    /// The alias for this `authz_visible` JOIN (`av0`, `av1`, …) the predicate references.
    pub alias: String,
    /// The relation this JOIN keys on (`read`/`view`) — carried so the in-memory evaluator (and the
    /// dedup) reads it without re-parsing the clause.
    pub relation: String,
    /// The full JOIN clause: `JOIN authz_visible <alias> ON <alias>.object_id = ci_run.run_id AND
    /// <alias>.subject = :<subject> AND <alias>.relation = :<relation>`.
    pub clause: String,
}

/// **The lowering result the CI run-list/search scan conjoins (§5.1) — `(sql_predicate, joins,
/// params)`.** The scan does: `SELECT … FROM ci_run <joins> WHERE tenant_id = :t AND region = :r AND
/// (<sql_predicate>) ORDER BY … LIMIT :page` binding `params`. This is **one query** — the conjoin is
/// the scan's query planner's job, NOT a per-row `check` loop. Leak-free: a run the viewer cannot
/// read never survives the `WHERE`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoweredFilter {
    /// The boolean SQL predicate over `ci_run.run_id` (ANDed into the list/search `WHERE`).
    pub sql_predicate: String,
    /// The deduplicated `authz_visible` JOINs the scan adds to its `FROM` (one per distinct
    /// `(viewer, relation)` — the no-N+1 guarantee).
    pub joins: Vec<AuthzJoin>,
    /// The bound parameters (object ids, the viewer subject, relation names) — bound, never interpolated.
    pub params: Vec<BoundParam>,
}

impl LoweredFilter {
    /// `true` iff the predicate references at least one `authz_visible` JOIN — i.e. the lowering hit
    /// an `InRelation`/`TupleSet` (the reverse-index revision watermark / new-enemy guard applies).
    pub fn depends_on_reverse_index(&self) -> bool {
        !self.joins.is_empty()
    }
}

/// Internal accumulator threaded through the recursive lowering so JOINs + params are collected once
/// (the no-N+1 dedup lives here: a `(viewer, relation)` JOIN already emitted is reused by alias).
struct LowerCtx<'a> {
    subject: &'a str,
    via_sql: String,
    joins: Vec<AuthzJoin>,
    params: Vec<BoundParam>,
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

    /// Bind a value, returning its `:placeholder` — never an interpolated literal (injection-safe).
    fn bind(&mut self, prefix: &str, value: &str) -> String {
        let placeholder = format!(":{}_{}", prefix, self.next_id);
        self.next_id += 1;
        self.params.push(BoundParam {
            placeholder: placeholder.clone(),
            value: value.to_string(),
        });
        placeholder
    }

    /// Emit (or reuse) the `authz_visible` JOIN for a `(viewer, relation)` — the §5.1 reverse-index
    /// JOIN keyed on `ci_run.run_id`. Deduplicated by relation (the viewer is constant for the whole
    /// call): a relation already JOINed reuses its alias (the no-N+1 guarantee). Returns the boolean
    /// predicate fragment `<alias>.object_id IS NOT NULL`.
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

/// **Lower a `SetExpr` to the consumer-composable SQL `Filter` over CI's `ci_run.run_id` (§5.1; the
/// FROZEN encoding).** `viewer` is the principal the `list_objects` is for (the `av.subject`
/// binding). Returns the [`LoweredFilter`] the run-list/search scan ANDs into its query — **one
/// query, no N+1, no post-filter**.
///
/// The FROZEN forms (§5.1 / §7.2):
/// - `All` → `TRUE`; `None` → `FALSE` (deny, never a permissive default);
/// - `Ids(v)` → `ci_run.run_id IN (…)` (empty → `FALSE` — the leak-free identity element);
/// - `NotIds(v)` → `ci_run.run_id NOT IN (…)` (empty → `TRUE`);
/// - `InRelation`/`TupleSet` → the `authz_visible` JOIN keyed on `ci_run.run_id`;
/// - `Union`/`Intersect`/`Difference` → `(a OR b)` / `(a AND b)` / `(a AND NOT b)`.
pub fn lower_over_run_id(set_expr: &SetExpr, viewer: &Principal) -> LoweredFilter {
    let via = ci_run_id_colref();
    let mut ctx = LowerCtx::new(&viewer.principal_id.0, &via);
    let sql_predicate = lower_expr(set_expr, &mut ctx);
    LoweredFilter {
        sql_predicate,
        joins: ctx.joins,
        params: ctx.params,
    }
}

/// The recursive lowering of one `SetExpr` node into a boolean SQL fragment (collecting JOINs +
/// params into `ctx`). Every leaf is a predicate over `ci_run.run_id` or a reverse-index JOIN; the
/// boolean nodes compose with `OR`/`AND`/`AND NOT` — no per-row subquery, no post-filter.
fn lower_expr(expr: &SetExpr, ctx: &mut LowerCtx<'_>) -> String {
    match expr {
        // The viewer sees every run of this type in the tenant (e.g. admin) → no restriction.
        SetExpr::All => "TRUE".to_string(),
        // The deny set — `WHERE false`, never a permissive default (leak-free).
        SetExpr::None => "FALSE".to_string(),
        // An explicit allow-set → `ci_run.run_id IN (…)`. An empty allow-set is `FALSE` (IN () means
        // "no rows" — never a permissive TRUE; the leak-free identity element).
        SetExpr::Ids(ids) => {
            if ids.is_empty() {
                return "FALSE".to_string();
            }
            let placeholders: Vec<String> = ids.iter().map(|id| ctx.bind("id", &id.0)).collect();
            format!("{} IN ({})", ctx.via_sql, placeholders.join(", "))
        }
        // An explicit deny-set over an otherwise-visible space → `NOT IN (…)`. Empty → `TRUE`.
        SetExpr::NotIds(ids) => {
            if ids.is_empty() {
                return "TRUE".to_string();
            }
            let placeholders: Vec<String> = ids.iter().map(|id| ctx.bind("id", &id.0)).collect();
            format!("{} NOT IN ({})", ctx.via_sql, placeholders.join(", "))
        }
        // The reverse-index JOIN keyed on `ci_run.run_id` (§5.1) — one JOIN per distinct relation.
        SetExpr::InRelation { relation, .. } => ctx.authz_join_predicate(&relation.0),
        // A server-materialised tuple set the scan JOINs against (the big-result path).
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
        SetExpr::Difference(a, b) => {
            let af = lower_expr(a, ctx);
            let bf = lower_expr(b, ctx);
            format!("({af} AND NOT {bf})")
        }
    }
}

/// **A composed, leak-free run-list query (the §5.1 push-down conjoined into ONE statement).** The
/// `sql` is a single `SELECT ci_run.run_id FROM ci_run <joins> WHERE ci_run.tenant_id = :tenant AND
/// ci_run.region = :region AND (<acl_predicate>) ORDER BY ci_run.run_id LIMIT :page` — the ACL
/// pre-filter is conjoined BEFORE pagination (never a post-filter), with the tenant predicate
/// isolating cross-tenant rows. **One query** — verified by [`ComposedRunListQuery::statement_count`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposedRunListQuery {
    /// The single SQL statement (no trailing `;` — one statement, exactly).
    pub sql: String,
    /// The bound parameters (tenant, region, the lowered filter's ids/subject/relation).
    pub params: Vec<BoundParam>,
}

impl ComposedRunListQuery {
    /// The number of SQL statements this read issues — ALWAYS 1 (the §5.1 no-N+1 guarantee). A drill
    /// asserts this is `1`.
    pub fn statement_count(&self) -> usize {
        self.sql.split(';').filter(|s| !s.trim().is_empty()).count()
    }
}

/// **Compose the run-LIST query (§5.1): the runs the `viewer` may `read`, leak-free, in ONE query.**
/// Given the `set_expr` Identity returned for `list_objects(viewer, read, ci_run)`, lower it over
/// `ci_run.run_id` and conjoin it into the run-list scan over the verified `(tenant, region)`
/// partition. The page bound is bound, not interpolated. The `myelin ci list` CLI rides this.
pub fn compose_run_list_query(
    set_expr: &SetExpr,
    viewer: &Principal,
    scope_tenant: &TenantId,
    scope_region: &Region,
) -> ComposedRunListQuery {
    let lowered = lower_over_run_id(set_expr, viewer);
    let joins: String = lowered
        .joins
        .iter()
        .map(|j| format!(" {}", j.clause))
        .collect();
    // The tenant predicate is ALWAYS emitted (a tenant-less list is unconstructable — the
    // `tenant-predicate` lint; EI-02 §1). The ACL predicate is conjoined BEFORE the ORDER BY / LIMIT
    // — pre-filter, never post-filter (ADR-03 / OQ-E).
    let sql = format!(
        "SELECT ci_run.run_id FROM ci_run{joins} \
         WHERE ci_run.tenant_id = :tenant AND ci_run.region = :region \
         AND ({acl}) ORDER BY ci_run.run_id LIMIT :page",
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
    ComposedRunListQuery { sql, params }
}

/// **The CI search ACL pre-filter (4.3 / 6.1; the `search-requires-acl-filter` lint).** The lowered
/// `list_objects(viewer, read, ci_run)` `Filter` the CI search query conjoins BEFORE scoring — a run
/// the viewer cannot read never appears in any result/count/rank (a search that scores first leaks
/// the existence/rank of a forbidden run). The field is named `acl_filter` so the lint fingerprints
/// the conjoin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiSearchPreFilter {
    /// The lowered ACL predicate + JOINs + params the search query ANDs into its `WHERE`/`FROM`
    /// BEFORE scoring. Named `acl_filter` (the lint binder).
    pub acl_filter: LoweredFilter,
}

/// **Build the CI search pre-filter (4.3 / 6.1): the `list_objects(viewer, read, ci_run)` `Filter`
/// lowered over `ci_run.run_id`, conjoined BEFORE scoring.** The pre-filter is the search's INPUT set
/// (never a post-filter over scored results), so a confidential run never appears in any result,
/// count, or rank.
pub fn run_search_pre_filter(set_expr: &SetExpr, viewer: &Principal) -> CiSearchPreFilter {
    CiSearchPreFilter {
        acl_filter: lower_over_run_id(set_expr, viewer),
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 3. THE in-memory authz_visible model + the leak-free evaluator (tests / drills)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The per-tenant, residency-pinned `authz_visible` reverse index (§5.1 / OQ-E) — modelled
/// in-memory for the unit + CDC + drill tests.** The materialised `(subject, relation, object_id)`
/// projection of the ReBAC tuples Identity maintains, kept fresh off the bus. The run-list/search
/// read JOINs against THIS for the `InRelation`/`TupleSet` forms. Per-tenant: the key is `(tenant,
/// region, subject, relation)` — **no cross-tenant query path** (EI-02 §1). The REAL `authz_visible`
/// table (Identity-maintained, JOINed in SQL) replaces this in any `--features integration` proof.
#[derive(Clone, Default)]
pub struct AuthzVisibleIndex {
    visible: Arc<Mutex<VisibleMap>>,
}

/// `(tenant, region, subject, relation)` → the visible `object_id` set (the reverse index).
type VisibleMap = HashMap<(String, String, String, String), Vec<String>>;

impl AuthzVisibleIndex {
    /// A fresh, empty reverse index.
    pub fn new() -> AuthzVisibleIndex {
        AuthzVisibleIndex::default()
    }

    /// Grant `subject` visibility of `object_id` under `relation` in `(tenant, region)` (the
    /// kept-fresh-off-the-bus projection of a `write_tuples`).
    pub fn grant(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: &str,
        relation: &str,
        object_id: &str,
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
    }

    /// Revoke `subject`'s visibility of `object_id` under `relation` (the projection of a revoke — a
    /// subsequent read must NOT see `object_id`; the revoke-reflected leak-free property).
    pub fn revoke(
        &self,
        tenant: &TenantId,
        region: &Region,
        subject: &str,
        relation: &str,
        object_id: &str,
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
    }

    /// **Evaluate a [`LoweredFilter`] against this in-memory index: the set of `candidate` run ids
    /// that survive the JOIN + predicate (the SAME row set the SQL `WHERE`/JOIN would keep).**
    /// Leak-free: a candidate the viewer has no `relation` tuple for (and no inline `IN`-allow) never
    /// survives. This models the SQL the live integration test proves; it is NOT a per-row `check`
    /// (it reads the already-materialised reverse index, exactly as the JOIN does).
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

    /// Resolve the placeholders inside an `IN (…)` fragment to their bound values.
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
/// one candidate row (the in-memory model of the SQL `WHERE`). This is test/model machinery; the
/// production path is the database evaluating the same predicate.
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

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 4. THE PROJECTION SHAPE (contract 5.6, §7.2) — {title, state, icon, render_hint, sub_anchor?}
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// A **per-viewer projection** of a CI artifact (contract 5.6, §7.2). The humanisation projection
/// Refs/Search/Notif/Chat consume — `title`, `state`, an `icon` token, an optional `render_hint`, and
/// an optional `sub_anchor` (a `#step-<n>` jump-to-failure). Built ONLY after the per-viewer
/// permission check passes ([`Projector::project`]); a denied viewer gets a [`Tombstone`] instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projection {
    /// The artifact title (e.g. `Run #42 · build-and-test`, `Deploy prod · v1.4.2`). NEVER rendered
    /// for an unauthorized viewer (the 0-leak invariant — the deny path never reads this field).
    pub title: String,
    /// The artifact state token (`passed`/`failed`/`running`/`queued` for a run; `deploying`/
    /// `deployed`/`awaiting_approval`/… for a deployment; `valid`/`invalid` for a pipeline).
    pub state: String,
    /// The icon token (`run`/`deployment`/`pipeline`/`runner`/`artifact`) the UI renders.
    pub icon: String,
    /// An optional render hint — the run DAG summary / failed step / duration, or the deploy
    /// env/risk/rollback. `None` for artifact types with no extra render context.
    pub render_hint: Option<RenderHint>,
    /// An optional sub-anchor projection — set when the projected ref carried a `#step-<n>` sub
    /// (jump-to-failure). `None` for a bare-root projection.
    pub sub_anchor: Option<SubAnchor>,
}

/// The render hint for a run or deployment projection (§7.2 — `render_hint`). A small, humanisable
/// enum, never a raw string the UI must parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderHint {
    /// The run render hint — `{dag_summary, failed_step?, duration_secs?}`.
    Run {
        /// A coarse DAG summary (e.g. `"4/5 stages green"`).
        dag_summary: String,
        /// The failing step index if the run failed (the jump-to-failure target), else `None`.
        failed_step: Option<u64>,
        /// The run duration in whole seconds if the run has completed, else `None`.
        duration_secs: Option<u64>,
    },
    /// The deployment render hint — `{env, risk, rollback_available}`.
    Deployment {
        /// The target environment (e.g. `"prod"`).
        env: String,
        /// The coarse risk label (e.g. `"high"`).
        risk: String,
        /// `true` iff a rollback is available (reversibility).
        rollback_available: bool,
    },
    /// The pipeline render hint — `{last_run?}` (the last run's ref string, if any).
    Pipeline {
        /// The last run's canonical ref string, if the pipeline has run.
        last_run: Option<String>,
    },
}

/// A projected sub-anchor (a `#step-<n>` jump-to-failure, §7.2 `sub_anchor`). The CI-owned step sub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubAnchor {
    /// The sub kind label (`step`).
    pub kind: String,
    /// The step id (the `#step-<n>` value).
    pub step: u64,
}

/// Why a projection degraded to a [`Tombstone`] (the audit reason; never leaked to the viewer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TombstoneReason {
    /// The viewer is not authorised to view the artifact (`Id.check` denied / errored — fail-closed).
    /// The projection NEVER reads the artifact's title (0 leak).
    Unauthorized,
    /// The artifact has been ERASED (a `ci.*.erased` tombstone, §6) — the content is gone.
    Erased,
    /// The viewer's subject is RESTRICTED (the GDPR `restrict` flag, §6) — the content is suppressed.
    Restricted,
}

/// A **tombstone** — the projection of a CI artifact the viewer may NOT see, or that has been
/// erased/restricted (contract 5.6, §7.2 — erasure-safe / restriction-safe). Carries NO title and NO
/// content (the 0-leak invariant): a denied viewer learns only "(not available)" — never the title,
/// never the state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tombstone {
    /// Why the projection is a tombstone — for the AUDIT log, NEVER rendered to the viewer.
    pub reason: TombstoneReason,
}

impl Tombstone {
    /// The generic, content-free text the VIEWER sees (never the title/state/reason). The same string
    /// regardless of reason — a denied viewer cannot distinguish "denied" from "erased".
    pub fn display_text(&self) -> &'static str {
        "(not available)"
    }
}

/// The result of [`Projector::project`]: either a per-viewer [`Projection`] (authorised + present) or
/// a [`Tombstone`] (denied / erased / restricted). The two-variant shape IS the §7.2 contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Projected {
    /// The authorised, present projection.
    Visible(Projection),
    /// The denied / erased / restricted tombstone (no leaked content).
    Tombstoned(Tombstone),
}

impl Projected {
    /// `true` iff this is a visible projection (authorised + present).
    pub fn is_visible(&self) -> bool {
        matches!(self, Projected::Visible(_))
    }

    /// `true` iff this is a tombstone (denied / erased / restricted).
    pub fn is_tombstone(&self) -> bool {
        matches!(self, Projected::Tombstoned(_))
    }

    /// The projected title IF visible, else `None`. The 0-leak helper: a tombstone has no title.
    pub fn title(&self) -> Option<&str> {
        match self {
            Projected::Visible(p) => Some(&p.title),
            Projected::Tombstoned(_) => None,
        }
    }
}

/// A loud, typed projection error (a malformed / non-CI ref, or a dangling ref) — distinct from a
/// [`Tombstone`] (which is a SUCCESSFUL projection of a hidden artifact). An error means the ref is
/// not projectable AT ALL; a tombstone means it is projectable but hidden from this viewer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectError {
    /// The ref is not a CI artifact (wrong subsystem / malformed scope).
    NotACiArtifact {
        /// The offending reference string.
        reference: String,
    },
    /// The `<type>` token is not a CI type CI's projector owns.
    UnknownCiType {
        /// The rejected type token.
        ty: String,
    },
    /// The artifact does not exist in the store (a dangling ref). Distinct from a tombstone: the ref
    /// is well-formed and the viewer MAY be authorised, but there is nothing to project.
    NotFound {
        /// The reference that resolved to nothing.
        reference: String,
    },
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectError::NotACiArtifact { reference } => write!(
                f,
                "not a CI artifact: `{reference}` — CI's projector does not own this ref"
            ),
            ProjectError::UnknownCiType { ty } => write!(f, "unknown CI artifact type `{ty}`"),
            ProjectError::NotFound { reference } => {
                write!(f, "no CI artifact found for `{reference}` (dangling ref)")
            }
        }
    }
}

impl std::error::Error for ProjectError {}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 5. THE ARTIFACT STORE (the live-OLTP-store floor — in-memory projectable metadata here)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// A run's projectable metadata (the §7.2 projection input). The live `ci_run` OLTP row hydrates
/// these; the projector needs only the run number/pipeline/state/DAG-summary, never the log bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunMeta {
    /// The run number (the `#<n>` in the title).
    pub number: u64,
    /// The pipeline name (the title's `· <pipeline>`).
    pub pipeline: String,
    /// The run state token (`passed`/`failed`/`running`/`queued`/…).
    pub state: String,
    /// A coarse DAG summary (e.g. `"4/5 stages green"`).
    pub dag_summary: String,
    /// The failing step index if the run failed (the jump-to-failure target).
    pub failed_step: Option<u64>,
    /// The run duration in whole seconds if completed.
    pub duration_secs: Option<u64>,
}

/// A deployment's projectable metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeploymentMeta {
    /// The target environment (e.g. `"prod"`).
    pub env: String,
    /// The version/release label (the title's `· <version>`).
    pub version: String,
    /// The deploy state.
    pub state: DeployState,
    /// The coarse risk label.
    pub risk: String,
    /// `true` iff a rollback is available.
    pub rollback_available: bool,
}

/// A pipeline's projectable metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PipelineMeta {
    /// The pipeline name (the title).
    pub name: String,
    /// `true` iff the pipeline definition is valid (the `valid`/`invalid` state).
    pub valid: bool,
    /// The last run's canonical ref string, if any.
    pub last_run: Option<String>,
}

/// The in-memory **artifact store** the projector reads (the live-OLTP-store FLOOR — the SAME entity
/// shapes the live store will hydrate, so the projection logic is store-agnostic). Carries the
/// erased/restricted flags the §7.2 erasure-/restriction-safe tombstone reads.
#[derive(Clone, Debug, Default)]
pub struct ArtifactStore {
    runs: HashMap<String, RunMeta>,
    deployments: HashMap<String, DeploymentMeta>,
    pipelines: HashMap<String, PipelineMeta>,
    erased: HashSet<String>,
    restricted: HashSet<String>,
}

impl ArtifactStore {
    /// A fresh empty store.
    pub fn new() -> ArtifactStore {
        ArtifactStore::default()
    }

    /// Insert a run keyed by its canonical ref.
    pub fn put_run(&mut self, canonical_ref: &ArtifactRef, meta: RunMeta) {
        self.runs.insert(canonical_ref.0.clone(), meta);
    }

    /// Insert a deployment keyed by its canonical ref.
    pub fn put_deployment(&mut self, canonical_ref: &ArtifactRef, meta: DeploymentMeta) {
        self.deployments.insert(canonical_ref.0.clone(), meta);
    }

    /// Insert a pipeline keyed by its canonical ref.
    pub fn put_pipeline(&mut self, canonical_ref: &ArtifactRef, meta: PipelineMeta) {
        self.pipelines.insert(canonical_ref.0.clone(), meta);
    }

    /// Mark a canonical ref ERASED (a `ci.*.erased` tombstone) — projecting it returns an `Erased`
    /// tombstone (erasure-safe, §6).
    pub fn mark_erased(&mut self, canonical_ref: &ArtifactRef) {
        self.erased.insert(canonical_ref.0.clone());
    }

    /// Mark a canonical ref's subject RESTRICTED (the GDPR `restrict` flag) — projecting it returns a
    /// `Restricted` tombstone (restriction-safe, §6).
    pub fn mark_restricted(&mut self, canonical_ref: &ArtifactRef) {
        self.restricted.insert(canonical_ref.0.clone());
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 6. THE PROJECTOR — project(ref, viewer): permission FIRST, then the per-viewer projection
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The CI `project(ref, viewer)` projector (contract 5.6 — the only cross-DB read of a CI
/// artifact).** Backs the chat run unfurl, the PR context pane, the knowledge embed, the inbox
/// humanisation, the search snippet. Holds the [`IdentityService`] dependency (the per-viewer
/// permission source) + the [`ArtifactStore`] (the own-DB read). Generic over `I: IdentityService` so
/// the front door wires the real Id resolver and tests wire a deterministic one.
pub struct Projector<I: IdentityService> {
    id: I,
    store: ArtifactStore,
}

impl<I: IdentityService> Projector<I> {
    /// Compose the projector over the Id dependency + the artifact store.
    pub fn new(id: I, store: ArtifactStore) -> Projector<I> {
        Projector { id, store }
    }

    /// A borrow of the underlying store (for the front door / drills to seed or inspect).
    pub fn store_mut(&mut self) -> &mut ArtifactStore {
        &mut self.store
    }

    /// **`project(ref, viewer) -> Projection | Tombstone` (contract 5.6, §7.2).**
    ///
    /// The order is the load-bearing invariant (the 0-leak gate):
    /// 1. **PERMISSION FIRST** — `Id.check(viewer, view, ref.acl_object())`. A `Deny` (or any
    ///    non-`Allow`, fail-closed) returns a [`Tombstone`] built with **NO field of the artifact
    ///    read into it** — the title cannot leak because it is never fetched on the deny path. An Id
    ///    transport error fails CLOSED (a tombstone, never a leak). The check runs on the
    ///    `#sub`-stripped ROOT (a sub is never more visible than its parent run).
    /// 2. **ERASURE-/RESTRICTION-SAFE** — if the artifact is erased or the viewer's subject is
    ///    restricted, return the corresponding tombstone (never the gone/restricted content).
    /// 3. **LOAD THE OWN DB + BUILD THE PER-VIEWER PROJECTION** — only now read the artifact and
    ///    build the §7.2 `{title, state, icon, render_hint, sub_anchor?}` projection.
    ///
    /// `zookie` is the read-consistency fence (a strong zookie-stamped read for a security-sensitive
    /// projection; bounded-stale for an availability-tolerant unfurl).
    pub fn project(
        &self,
        reference: &ArtifactRef,
        viewer: &Principal,
        zookie: Zookie,
    ) -> Result<Projected, ProjectError> {
        // Classify FIRST so a non-CI / unknown-type ref is a loud error (not a tombstone).
        let ty = classify(reference)?;

        // ── 1. PERMISSION FIRST (the 0-leak gate). Check the ROOT (the `#sub`-stripped artifact) so a
        //    `#step-<n>` sub inherits the parent run's `view`. Deny / Conditional / Id-error all fail
        //    CLOSED to a tombstone with NO artifact field read.
        let acl_object = myelin_refs::strip_sub(reference);
        let at = Consistency {
            at_least: zookie,
            mode: ConsistencyMode::Strong,
        };
        let permission = Permission(VIEW.to_string());
        match self.id.check(viewer, &permission, &acl_object, &at, None) {
            Ok(Decision::Allow) => { /* authorised — fall through to the erasure/restriction guards */
            }
            Ok(Decision::Deny) | Ok(Decision::Conditional) | Err(_) => {
                return Ok(Projected::Tombstoned(Tombstone {
                    reason: TombstoneReason::Unauthorized,
                }));
            }
        }

        // ── 2. ERASURE-/RESTRICTION-SAFE (§6). Keyed on the ROOT (an erased run tombstones its step
        //    anchors too). The permission passed, but the content is gone / restricted.
        if self.store.erased.contains(&acl_object.0) || self.store.erased.contains(&reference.0) {
            return Ok(Projected::Tombstoned(Tombstone {
                reason: TombstoneReason::Erased,
            }));
        }
        if self.store.restricted.contains(&acl_object.0)
            || self.store.restricted.contains(&reference.0)
        {
            return Ok(Projected::Tombstoned(Tombstone {
                reason: TombstoneReason::Restricted,
            }));
        }

        // ── 3. LOAD THE OWN DB + BUILD THE PER-VIEWER PROJECTION (§7.2).
        let projection = match ty {
            CiArtifactType::Run => self.project_run(&acl_object, reference)?,
            CiArtifactType::Deployment => self.project_deployment(&acl_object)?,
            CiArtifactType::Pipeline => self.project_pipeline(&acl_object)?,
            // Runner / artifact have no humanised projection in this prompt — they project as a
            // minimal title/state (the run-centric surfaces — unfurl/context-pane/embed/inbox/search
            // — only request run/deployment/pipeline; §7.2 names those three).
            CiArtifactType::Runner | CiArtifactType::Artifact => {
                self.project_minimal(&acl_object, ty)
            }
        };
        Ok(Projected::Visible(projection))
    }

    /// Build the run projection (§7.2 — `title: "Run #<n> · <pipeline>"`, the run state, `icon: run`,
    /// the DAG/failed-step/duration render hint, the `#step-<n>` sub-anchor if the ref carried one).
    fn project_run(
        &self,
        root: &ArtifactRef,
        reference: &ArtifactRef,
    ) -> Result<Projection, ProjectError> {
        let meta = self
            .store
            .runs
            .get(&root.0)
            .ok_or_else(|| ProjectError::NotFound {
                reference: root.0.clone(),
            })?;
        // The `#step-<n>` sub-anchor (the only CI run sub `project` returns; jump-to-failure).
        let sub_anchor = match myelin_refs::sub_kind(reference) {
            Some(Sub::Step(n)) => Some(SubAnchor {
                kind: "step".to_string(),
                step: n,
            }),
            _ => None,
        };
        Ok(Projection {
            title: format!("Run #{} · {}", meta.number, meta.pipeline),
            state: meta.state.clone(),
            icon: CiArtifactType::Run.token().to_string(),
            render_hint: Some(RenderHint::Run {
                dag_summary: meta.dag_summary.clone(),
                failed_step: meta.failed_step,
                duration_secs: meta.duration_secs,
            }),
            sub_anchor,
        })
    }

    /// Build the deployment projection (§7.2 — `title: "Deploy <env> · <version>"`, the deploy state,
    /// `icon: deployment`, the env/risk/rollback render hint).
    fn project_deployment(&self, root: &ArtifactRef) -> Result<Projection, ProjectError> {
        let meta = self
            .store
            .deployments
            .get(&root.0)
            .ok_or_else(|| ProjectError::NotFound {
                reference: root.0.clone(),
            })?;
        Ok(Projection {
            title: format!("Deploy {} · {}", meta.env, meta.version),
            state: meta.state.as_token().to_string(),
            icon: CiArtifactType::Deployment.token().to_string(),
            render_hint: Some(RenderHint::Deployment {
                env: meta.env.clone(),
                risk: meta.risk.clone(),
                rollback_available: meta.rollback_available,
            }),
            sub_anchor: None,
        })
    }

    /// Build the pipeline projection (§7.2 — `title: <name>`, `state: valid|invalid`,
    /// `icon: pipeline`, the last-run render hint).
    fn project_pipeline(&self, root: &ArtifactRef) -> Result<Projection, ProjectError> {
        let meta = self
            .store
            .pipelines
            .get(&root.0)
            .ok_or_else(|| ProjectError::NotFound {
                reference: root.0.clone(),
            })?;
        Ok(Projection {
            title: meta.name.clone(),
            state: if meta.valid { "valid" } else { "invalid" }.to_string(),
            icon: CiArtifactType::Pipeline.token().to_string(),
            render_hint: Some(RenderHint::Pipeline {
                last_run: meta.last_run.clone(),
            }),
            sub_anchor: None,
        })
    }

    /// Build a minimal projection for runner/artifact (a permission-checked title/state with no
    /// render hint — these are not the run-centric surfaces §7.2 names, but a permitted viewer still
    /// gets a non-leaky id-based projection rather than an error).
    fn project_minimal(&self, root: &ArtifactRef, ty: CiArtifactType) -> Projection {
        let id = canonical_id(root).unwrap_or_default();
        Projection {
            title: format!("{} {}", ty.token(), id),
            state: "present".to_string(),
            icon: ty.token().to_string(),
            render_hint: None,
            sub_anchor: None,
        }
    }
}

/// The canonical `<id>` segment of a CI `ArtifactRef` (the part after `ci/<type>/`, before any
/// `#sub`). `None` for a non-CI / malformed ref.
fn canonical_id(r: &ArtifactRef) -> Option<String> {
    let rest = r.0.strip_prefix("myelin://")?;
    let scope = rest.split('#').next().unwrap_or(rest);
    let segments: Vec<&str> = scope.split('/').collect();
    if segments.len() != 4 || segments[1] != CI_SUBSYSTEM {
        return None;
    }
    Some(segments[3].to_string())
}

#[cfg(test)]
#[path = "surfacing_tests.rs"]
mod tests;

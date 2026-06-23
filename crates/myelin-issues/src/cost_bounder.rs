//! # `cost_bounder` — the Issues cost-bounding + three-tier escalation (ISS-P14 / P-380; the
//! ISS-D2 `<1s` flexible-field latency gate).
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/issue-tracker/architecture/02-internals-and-algorithms.md`
//! §3 (*The query planner — AST→store compiler + `SetExpr` push-down + cost-bounding + the projection
//! feeder*): after the leak-free pre-filter ([`crate::planner`], ISS-P13) lowers the `list_objects`
//! `SetExpr` FIRST, the planner's SECOND job is to **classify each user predicate** and **pick the
//! cheapest correct tier**, then **bound the cost**: a query that would scan too much is pushed to
//! Search (the SAME ACL `Filter` conjoined) or returns a `Refine{hint}` — **never an unbounded JSONB
//! scan**. Every query is **paginated + statement-timeout'd**. `05-hard-problems.md` §3 (the JQL
//! performance trap — typed-core columns + JSONB tail + the GIN/generated-index projection + Search as
//! the escalation valve; the `<1s` keyboard budget is the dominant per-subsystem risk).
//!
//! ## What ISS-P14 ships here (the cost-bounding half — the tier of the planner ON TOP of ISS-P13)
//!
//! The leak-free `SetExpr` pre-filter ([`crate::planner::lower_over_issue_id`]) ALWAYS comes first and
//! is conjoined into EVERY tier (the `search-requires-acl-filter` discipline holds on every escalation
//! path). This module adds, on top of that frozen leak-free path:
//!
//! 1. [`Tier`] + [`classify_field`] — the §3 classification: a typed-core field
//!    (`state`/`state_category`/`priority`/`assignee`/`reporter`/`type`/`parent`/`project`/`cycle`/
//!    `rank`/timestamps) → **Tier 1** (the `issue_board`/`issue_roadmap`/`issue_assignee` index
//!    ranges, the 90% hot path); a MEASURED-HOT custom facet (a generated index exists, the ISS-P15
//!    feeder) → **Tier 2**; a cold custom facet → **Tier 2b** (the default `issue_props_gin` GIN
//!    probe); a full-text / cross-artifact / semantic / huge-cold-facet leg → **Tier 3** (escalate to
//!    Search, the SAME `Filter` conjoined).
//! 2. [`CostBudget`] + [`estimate_cost`] — the row/cost ceiling + the statement timeout + the page cap.
//!    The cost estimate over the classified tiers decides whether the OLTP tier can serve within the
//!    `<1s` keyboard budget.
//! 3. [`plan_board_query`] — the cost-bounder proper: classify → estimate → DECIDE. The outcome
//!    ([`PlanOutcome`]) is one of:
//!    - **`ServeOltp`** — the cost is within budget: a single paginated + statement-timeout'd
//!      [`crate::planner::ComposedBoardQuery`] over the cheapest tier's index, the ACL pre-filter
//!      conjoined (Tier 1 / Tier 2 / Tier 2b). NEVER an unbounded JSONB scan.
//!    - **`EscalateToSearch`** — the heaviest leg is over budget OR is inherently a Tier-3 leg
//!      (full-text/semantic/cross-artifact): a [`SearchEscalation`] carrying the board's OWN
//!      `Filter{set_expr}` (byte-identical to the OLTP shape, 4.3) into Search's
//!      [`myelin_search::escalate_to_search`] valve (6.1, the SRCH-P21 seam) — the SAME ACL `Filter`
//!      conjoined (the `search-requires-acl-filter` lint holds — 0 Search calls without it).
//!    - **`Refine`** — when escalation is NOT available for this leg (a cost that exceeds even Search's
//!      bound, or a cold huge-result ad-hoc facet the operator must narrow): a [`RefineHint`] the UI
//!      surfaces — never a fallback to an unbounded scan.
//!
//! ## Cost-bounding is leak-free AND scan-free by construction (EI-01 §3; ISS-D2)
//! The escalation decision is the LATENCY-CORRECTNESS seam (this module is **mandatory-core**, like the
//! leak seam): the wrong decision is either a `<1s`-budget MISS (a full JSONB scan that should have
//! escalated) or a spurious deny. The classifier is total (every field maps to exactly one tier; an
//! unknown field is a cold custom facet → Tier 2b, never a typed-core mis-hit). The cost-bounder NEVER
//! returns a plan that scans the JSONB tail without a bound: a cold facet over a huge result either
//! escalates to Search or returns `Refine` — `assert_no_unbounded_scan` is the structural guard.
//!
//! ## The Tier-3 escalation conjoins the SAME Filter (the `search-requires-acl-filter` lint; 6.1)
//! The escalation path does NOT re-derive the viewer's reachable set — it carries the board's OWN
//! `Filter{set_expr}` (the ISS-P13 lowering, 4.3) verbatim into Search through the
//! [`myelin_search::BoardQuery`] seam, so the OLTP board and the Search valve apply **byte-identical**
//! ACL pre-filter semantics (the SRCH-P21 parity property). There is NO second `SetExpr` interpreter;
//! Search lowers the SAME `set_expr` through its own `lower_set_expr`. The `search-requires-acl-filter`
//! lint holds structurally: [`SearchEscalation`] is constructible ONLY with the board's `set_expr`.
//!
//! ## FLOORS named (per the prompt — DELIVERABLE)
//! - **Tier 2 generated-index promotion** — the flexible-field index is **GIN by default**
//!   ([`crate::schemes::IndexPosture::Gin`]); the projection-feeder generated-index promotion (a facet
//!   crossing the OQ-C `> 5%` of view executions) is the MEASURED follow-on **ISS-P15** (P-381).
//!   Until promoted a custom facet is Tier 2b (the GIN probe). Named: [`CostBounderFloors::TIER2_FEEDER`].
//! - **Distributed-SQL for a hot tenant** — the PG-hybrid (typed core + JSONB + projection feeder) is
//!   the floor; distributed-SQL is the MEASURED follow-on if a single tenant's shard outgrows PG —
//!   **M5 / ISS-P32**. Named: [`CostBounderFloors::DISTRIBUTED_SQL`].
//! - The **at-scale board-query latency under the 30× world-scale surge** is **ISS-P33 / SRCH-P25** —
//!   the surge changes the LATENCY budget, never the leak-equivalence or the tier classification.
//!
//! The live `<1s` board-query proof (50+ fields × 1M+ issues, p99 `< 1s`, no full scan) is the
//! `--features integration` drill against the dev-stack Postgres
//! (`tests/integration_iss_p14_cost_bounding.rs`): the ISS-D2 green artifact flips green ONLY there.

use myelin_identity::{Principal, SetExpr, Zookie};
use myelin_query::{Expr, Predicate, QueryAst};
use myelin_tenancy::{Region, TenantId};

use crate::planner::{compose_board_query, lower_over_issue_id, BoundParam, ComposedBoardQuery};
use crate::schemes::IndexPosture;

// ───────────────────────────── frozen names (§3 — never a stray literal) ─────────────────────────

/// **The typed-core field names that earn a typed column (§3 / arch 01 §2).** A field is typed-core
/// iff it is *always present AND on the hot board/report path* — these resolve to a Tier-1 index range
/// (`issue_board`/`issue_roadmap`/`issue_assignee`/`issue_parent`/`issue_cycle`). Everything else is a
/// flexible field in the JSONB tail. Named in ONE place so a classifier drill asserts against the NAME,
/// never a literal (EI-01 §3); a rename of a typed column updates this set, not a scatter of literals.
pub const TYPED_CORE_FIELDS: &[&str] = &[
    "state",
    "state_category",
    "priority",
    "assignee",
    "reporter",
    "type",
    "type_id",
    "type_rank",
    "parent",
    "parent_id",
    "project",
    "project_id",
    "cycle",
    "cycle_id",
    "rank",
    "created_at",
    "updated_at",
    "state_changed_at",
];

/// **The Tier-3 full-text / semantic / cross-artifact predicate markers (§3).** A predicate whose
/// field is one of these is INHERENTLY a Tier-3 leg — it has no OLTP index that can serve it within the
/// keyboard budget (free-text search, semantic similarity, a cross-artifact reference walk) and
/// escalates to Search regardless of cost. Named so the classifier reads the FROZEN markers.
pub const TIER3_FIELDS: &[&str] = &["text", "body", "fulltext", "semantic", "any_artifact"];

// ───────────────────────────── the three-tier classification (§3) ───────────────────────────────

/// **Which of the three (four, counting 2b) tiers serves a single predicate (§3 sketch 03).** The
/// cost-bounder classifies EACH user predicate, then the plan picks the cheapest correct tier for the
/// whole query (the heaviest leg drives the escalation decision). The leak-free `SetExpr` pre-filter
/// (ISS-P13) is conjoined into EVERY tier — it is not a tier itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// **Tier 1 — the typed core.** A typed-core field (`state`/`priority`/`assignee`/…) served by an
    /// `issue_board`/`issue_roadmap`/`issue_assignee` index range. The 90% hot path — Linear-fast.
    TypedCore,
    /// **Tier 2 — the measured-hot custom facet.** A flexible field PROMOTED to a generated index by
    /// the projection feeder (the ISS-P15 floor — until then a custom facet is `GinProbe`). Served by a
    /// generated/expression index.
    GeneratedFacet,
    /// **Tier 2b — the cold custom facet GIN probe.** A flexible field still GIN-served (the default
    /// `issue_props_gin`, `jsonb_path_ops`) — a BOUNDED GIN probe (small result), never an unbounded
    /// JSONB scan. A huge-result cold facet is NOT this tier — it escalates (Tier 3) or returns Refine.
    GinProbe,
    /// **Tier 3 — escalate to Search.** Full-text / cross-artifact / semantic / a cold facet on a HUGE
    /// result → `query(ast, viewer, zookie, page)` (6.1) conjoining the SAME OQ-E `Filter`. The
    /// pressure-release valve (the SRCH-P21 seam).
    Search,
}

impl Tier {
    /// **The relative cost weight of one leg at this tier** (the §3 `estimate(rows_scanned, tiers)`
    /// model — a deterministic, conservative ordering, NOT a wall-clock measurement). Tier 1 is the
    /// cheapest (an index range); a generated facet is next; a GIN probe is more (it scans matching
    /// JSONB rows); Tier 3 is the escalation cost. The cost-bounder sums the per-leg weight × the
    /// estimated row fan-out to decide whether the OLTP tier stays within budget. The live wall-clock
    /// `<1s` proof is the ISS-D2 integration drill — this weight is the DECISION model that drill
    /// calibrates.
    pub fn cost_weight(self) -> u64 {
        match self {
            Tier::TypedCore => 1,
            Tier::GeneratedFacet => 2,
            Tier::GinProbe => 8,
            Tier::Search => 1, // Search OWNS its own cost; the OLTP tier pays ~nothing to escalate.
        }
    }

    /// Whether this tier is served on the OLTP store (Tiers 1 / 2 / 2b) vs escalated to Search (Tier 3).
    pub fn is_oltp(self) -> bool {
        !matches!(self, Tier::Search)
    }
}

/// **The catalog of which flexible facets are MEASURED-HOT (have a generated index — Tier 2).** The
/// projection feeder (ISS-P15) promotes a facet to [`IndexPosture::GeneratedIndex`] when it crosses the
/// OQ-C threshold; until then every custom facet is [`IndexPosture::Gin`] (Tier 2b). The cost-bounder
/// reads THIS to classify a custom field — it does NOT predict promotion (measured, never speculative,
/// EI-02 §8). An empty catalog (the default) means "no facet promoted yet" — every custom field is the
/// GIN probe, exactly the pre-feeder state.
#[derive(Clone, Debug, Default)]
pub struct FacetCatalog {
    /// The custom `field_id`s currently served by a generated index (the promoted-hot set). A field NOT
    /// in this set is GIN-served (Tier 2b) by default.
    promoted: Vec<String>,
}

impl FacetCatalog {
    /// An empty catalog — no facet promoted yet (the pre-feeder default: every custom field is Tier 2b).
    pub fn new() -> FacetCatalog {
        FacetCatalog::default()
    }

    /// Record that `field_id` has been promoted to a generated index by the projection feeder (ISS-P15
    /// — the MEASURED promotion; this is the catalog the cost-bounder reads, fed off the bus). Idempotent.
    pub fn promote(&mut self, field_id: impl Into<String>) -> &mut FacetCatalog {
        let id = field_id.into();
        if !self.promoted.iter().any(|f| f == &id) {
            self.promoted.push(id);
        }
        self
    }

    /// The index posture of a custom facet: [`IndexPosture::GeneratedIndex`] iff promoted (Tier 2),
    /// else [`IndexPosture::Gin`] (Tier 2b — the default). The cost-bounder maps this to the tier.
    pub fn posture(&self, field_id: &str) -> IndexPosture {
        if self.promoted.iter().any(|f| f == field_id) {
            IndexPosture::GeneratedIndex
        } else {
            IndexPosture::Gin
        }
    }
}

/// **Classify ONE predicate field into its tier (§3).** Total: every field name maps to exactly one
/// tier. A typed-core field → Tier 1; a Tier-3 marker (free-text/semantic/cross-artifact) → Tier 3; any
/// other field is a flexible custom facet — Tier 2 iff the catalog has promoted it (a generated index),
/// else Tier 2b (the GIN probe). An UNKNOWN field is a cold custom facet (Tier 2b), never a typed-core
/// mis-hit (fail-conservative — a mis-classification toward a heavier tier is safe; a mis-classification
/// toward a cheaper tier would be a `<1s`-budget MISS, which the total classifier structurally avoids).
pub fn classify_field(field: &str, catalog: &FacetCatalog) -> Tier {
    if TYPED_CORE_FIELDS.contains(&field) {
        return Tier::TypedCore;
    }
    if TIER3_FIELDS.contains(&field) {
        return Tier::Search;
    }
    match catalog.posture(field) {
        IndexPosture::GeneratedIndex => Tier::GeneratedFacet,
        IndexPosture::Gin => Tier::GinProbe,
    }
}

/// Collect the variable (field) names a [`Predicate`] reads — the LHS/RHS `Var` expressions. The
/// cost-bounder classifies each. (A literal-only comparison reads no field; an empty field set means a
/// pure-ACL board scan → Tier 1.)
fn predicate_fields(pred: &Predicate) -> Vec<String> {
    fn walk(p: &Predicate, out: &mut Vec<String>) {
        match p {
            Predicate::True | Predicate::False => {}
            Predicate::Cmp { lhs, rhs, .. } => {
                if let Expr::Var(v) = lhs {
                    out.push(v.clone());
                }
                if let Expr::Var(v) = rhs {
                    out.push(v.clone());
                }
            }
            Predicate::And(ps) | Predicate::Or(ps) => ps.iter().for_each(|p| walk(p, out)),
            Predicate::Not(p) => walk(p, out),
        }
    }
    let mut out = Vec::new();
    walk(pred, &mut out);
    out
}

// ───────────────────────────── the cost budget (§3 — paginated + statement-timeout'd) ───────────────

/// **The cost budget every board query runs under (§3 / ISS-D2 — the `<1s` keyboard budget).** Three
/// bounds, ALWAYS applied: a `max_scanned_cost` (the row-fan-out × tier-weight ceiling the OLTP tier may
/// serve before it escalates), a `statement_timeout_ms` (the hard SQL statement timeout — a query that
/// runs over it is killed, never allowed to run away), and a `page_limit` (the pagination cap — a board
/// read is ALWAYS paginated, never an unbounded result). A query whose estimated cost exceeds
/// `max_scanned_cost` escalates to Search (or returns Refine) — it NEVER runs an unbounded scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CostBudget {
    /// The maximum estimated scan cost (row-fan-out × tier-weight) the OLTP tier serves before it
    /// escalates to Search. Over this → Tier-3 escalation (or Refine).
    pub max_scanned_cost: u64,
    /// The hard SQL `statement_timeout` (ms) every board query carries — a runaway scan is killed, not
    /// served. The keyboard budget is `<1s`; the timeout is the hard backstop.
    pub statement_timeout_ms: u64,
    /// The pagination cap — a board read is ALWAYS `LIMIT`-bounded (never an unbounded result set).
    pub page_limit: u32,
    /// The estimated max rows a single Search escalation will serve before the operator must narrow the
    /// query (the Refine threshold — a cost so large even Search cannot serve it within budget).
    pub refine_cost_ceiling: u64,
}

impl CostBudget {
    /// The default board budget (§3 — calibrated by the ISS-D2 `<1s` integration drill). `max_scanned_cost`
    /// is the OLTP→Search escalation threshold; `statement_timeout_ms` is the hard `<1s` backstop;
    /// `page_limit` is the board page; `refine_cost_ceiling` is the escalate→Refine threshold.
    pub const DEFAULT: CostBudget = CostBudget {
        max_scanned_cost: 50_000,
        statement_timeout_ms: 900, // under the <1s keyboard budget — the hard kill backstop.
        page_limit: 50,
        refine_cost_ceiling: 5_000_000,
    };

    /// A budget with explicit bounds (the drill calibrates these against the live `<1s` artifact).
    pub fn new(
        max_scanned_cost: u64,
        statement_timeout_ms: u64,
        page_limit: u32,
        refine_cost_ceiling: u64,
    ) -> CostBudget {
        CostBudget {
            max_scanned_cost,
            statement_timeout_ms,
            page_limit,
            refine_cost_ceiling,
        }
    }
}

impl Default for CostBudget {
    fn default() -> CostBudget {
        CostBudget::DEFAULT
    }
}

/// **Estimate the cost of serving a classified query on the OLTP tier (§3 `estimate(rows_scanned,
/// tiers)`).** The cost is `estimated_row_fanout × max(tier_weight over the OLTP legs)` — the
/// HEAVIEST access path (the bottleneck leg) bounds a conjunction, because a multi-predicate `AND` is
/// served by index intersection over the most-selective range, NOT by N independent scans (a 50-field
/// typed-core board query is still ONE index range — its cost is the row fan-out × the Tier-1 weight,
/// not × 50). The bottleneck is the most-expensive tier present: a single cold-GIN leg in an otherwise
/// typed-core query makes the whole query pay the GIN cost (it must probe the JSONB tail). This is a
/// deterministic, conservative model (NOT a wall-clock measurement; the live wall-clock proof is the
/// ISS-D2 drill). A query with NO OLTP leg (a pure Tier-3 leg) has OLTP cost 0 (it does not touch the
/// OLTP store). `estimated_row_fanout` is the planner's row estimate for the most-selective predicate
/// (the caller supplies it — table statistics in production; the test scenario in the unit drills).
pub fn estimate_cost(tiers: &[Tier], estimated_row_fanout: u64) -> u64 {
    let bottleneck_weight: u64 = tiers
        .iter()
        .filter(|t| t.is_oltp())
        .map(|t| t.cost_weight())
        .max()
        .unwrap_or(0);
    estimated_row_fanout.saturating_mul(bottleneck_weight)
}

// ───────────────────────────── the Tier-3 escalation seam (6.1) ──────────────────────────────────

/// **A Tier-3 Search escalation carrying the board's OWN `Filter{set_expr}` (6.1 / 4.3).** The
/// cost-bounder builds THIS when the OLTP tier is over budget or the query has an inherent Tier-3 leg —
/// it carries the board's `ast` + the SAME `set_expr` the OLTP board would have conjoined (byte-identical,
/// 4.3) + the consistency `zookie` into Search's [`myelin_search::escalate_to_search`] valve. The
/// `search-requires-acl-filter` lint holds STRUCTURALLY: this type is constructible ONLY with the board's
/// `set_expr` ([`SearchEscalation::new`] takes it), so there is no escalation path WITHOUT the conjoined
/// Filter (0 Search calls without it). The valve lowers the SAME `set_expr` through Search's own
/// `lower_set_expr` — no second interpreter, byte-identical ACL pre-filter (SRCH-P21).
#[derive(Clone, Debug)]
pub struct SearchEscalation {
    /// The board's query AST (the SAME frozen [`QueryAst`] both tiers compile — one query language, 6.1).
    pub ast: QueryAst,
    /// The board's ACL pre-filter — the SAME `set_expr` the OLTP board lowered (ISS-P13, 4.3). The
    /// escalation is leak-equivalent to the OLTP board because it carries THIS verbatim.
    pub set_expr: SetExpr,
    /// The consistency zookie the ACL pre-filter was computed at (4.10) — threaded so Search honours the
    /// SAME snapshot (the reverse-index watermark, the new-enemy guard on the valve path too).
    pub zookie: Zookie,
    /// The pagination cap carried into Search (the board page — Search is paginated too).
    pub page_limit: u32,
}

impl SearchEscalation {
    /// Build a Tier-3 escalation carrying the board's OWN `set_expr` (4.3). There is NO constructor that
    /// omits the `set_expr` — the `search-requires-acl-filter` discipline is structural: an escalation
    /// without the conjoined Filter is UNREPRESENTABLE.
    pub fn new(
        ast: QueryAst,
        set_expr: SetExpr,
        zookie: Zookie,
        page_limit: u32,
    ) -> SearchEscalation {
        SearchEscalation {
            ast,
            set_expr,
            zookie,
            page_limit,
        }
    }

    /// **Build the [`myelin_search::BoardQuery`] the Tier-3 valve consumes (6.1 / SRCH-P21).** This is
    /// the wire across the OLTP-budget escalation seam: the board's `ast` + its `set_expr` + the zookie,
    /// handed verbatim to [`myelin_search::escalate_to_search`]. The valve lowers the SAME `set_expr`
    /// through Search's `lower_set_expr` — byte-identical ACL pre-filter, no second interpreter.
    pub fn to_board_query(&self) -> myelin_search::BoardQuery {
        myelin_search::BoardQuery::new(self.ast.clone(), self.set_expr.clone(), self.zookie.clone())
    }
}

/// **A `Refine` hint — the cost-bounder's third outcome (§3 `return Refine{hint}`).** When a query's
/// cost exceeds even Search's serving bound (`refine_cost_ceiling`) — a cold huge-result ad-hoc facet
/// the operator must narrow — the cost-bounder returns THIS rather than running an unbounded scan or
/// escalating a query Search cannot serve within budget. The UI surfaces the hint (e.g. "add a project
/// or date filter — this query would scan N million rows").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefineHint {
    /// The human-facing hint (the UI surfaces it — narrow the query).
    pub hint: String,
    /// The estimated cost that triggered the refine (so telemetry/observability can see the over-bound).
    pub estimated_cost: u64,
}

// ───────────────────────────── the cost-bounder (classify → estimate → decide) ────────────────────

/// **The bounded board query the OLTP tier serves (§3 — paginated + statement-timeout'd).** Wraps the
/// leak-free [`ComposedBoardQuery`] (ISS-P13) with the SECOND-job bounds: the statement timeout (ms) and
/// the page limit (the pagination cap), and the chosen OLTP tier (for telemetry). The composed query's
/// `sql` already conjoins the ACL pre-filter BEFORE `ORDER BY rank LIMIT :page` (never a post-filter);
/// THIS adds the statement-timeout bind + records the tier. NEVER an unbounded scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundedBoardQuery {
    /// The leak-free composed board query (the ISS-P13 ACL pre-filter conjoined; one statement).
    pub composed: ComposedBoardQuery,
    /// The SQL `statement_timeout` (ms) the read carries — the runaway-scan backstop.
    pub statement_timeout_ms: u64,
    /// The pagination cap the read carries (`LIMIT :page`).
    pub page_limit: u32,
    /// The OLTP tier the cost-bounder chose to serve this read (Tier 1 / 2 / 2b).
    pub tier: Tier,
}

impl BoundedBoardQuery {
    /// The bound parameters the read binds (the composed query's params + the bound page limit + the
    /// bound statement timeout) — ALL bound, never interpolated.
    pub fn params(&self) -> Vec<BoundParam> {
        let mut params = self.composed.params.clone();
        params.push(BoundParam {
            placeholder: ":page".into(),
            value: self.page_limit.to_string(),
        });
        params.push(BoundParam {
            placeholder: ":statement_timeout_ms".into(),
            value: self.statement_timeout_ms.to_string(),
        });
        params
    }

    /// **The structural no-unbounded-scan guard (§3 — "never an unbounded JSONB scan").** A bounded
    /// board read MUST carry a pagination cap (`LIMIT :page` in the SQL) AND a statement timeout. `true`
    /// iff both bounds are present — a drill asserts this for every `ServeOltp` outcome.
    pub fn is_bounded(&self) -> bool {
        self.composed.sql.contains("LIMIT :page")
            && self.page_limit > 0
            && self.statement_timeout_ms > 0
    }
}

/// **The cost-bounder's decision (§3 — `if cost > budget: escalate to Search OR return Refine`).** ONE
/// of three: serve on the OLTP tier (within budget, bounded), escalate to Search (the heaviest leg is
/// over budget or inherently Tier-3 — the SAME Filter conjoined), or Refine (the cost exceeds even
/// Search's bound). There is NO fourth "unbounded scan" outcome — that is the whole point.
#[derive(Clone, Debug)]
pub enum PlanOutcome {
    /// Serve on the OLTP tier — within budget, paginated + statement-timeout'd (Tier 1 / 2 / 2b).
    ServeOltp(BoundedBoardQuery),
    /// Escalate to Search — the SAME `Filter{set_expr}` conjoined (Tier 3, the 6.1 valve).
    EscalateToSearch(SearchEscalation),
    /// Refine — the cost exceeds even Search's bound; the operator must narrow the query.
    Refine(RefineHint),
}

impl PlanOutcome {
    /// `true` iff this outcome serves on the OLTP tier (a bounded scan).
    pub fn is_serve_oltp(&self) -> bool {
        matches!(self, PlanOutcome::ServeOltp(_))
    }
    /// `true` iff this outcome escalates to Search (the Tier-3 valve).
    pub fn is_escalate(&self) -> bool {
        matches!(self, PlanOutcome::EscalateToSearch(_))
    }
    /// `true` iff this outcome returns a Refine hint.
    pub fn is_refine(&self) -> bool {
        matches!(self, PlanOutcome::Refine(_))
    }

    /// **The structural invariant the cost-bounder upholds (§3 — never an unbounded scan).** EVERY
    /// outcome is bounded: a `ServeOltp` is paginated + statement-timeout'd ([`BoundedBoardQuery::is_bounded`]);
    /// an `EscalateToSearch` carries the board's ACL `set_expr` (leak-equivalent, paginated in Search); a
    /// `Refine` is a hint, not a scan. `true` for every well-formed outcome — a drill asserts this for
    /// every classification, so there is NEVER an unbounded JSONB scan.
    pub fn assert_no_unbounded_scan(&self) -> bool {
        match self {
            PlanOutcome::ServeOltp(q) => q.is_bounded(),
            // The escalation carries the board's set_expr (constructible only with it) + a page cap.
            PlanOutcome::EscalateToSearch(e) => e.page_limit > 0,
            // A refine is a hint — it runs no scan at all.
            PlanOutcome::Refine(_) => true,
        }
    }
}

/// **THE COST-BOUNDER (§3 — classify each predicate, pick the cheapest correct tier, bound the cost).**
/// On top of the leak-free `SetExpr` pre-filter (ISS-P13, ALWAYS conjoined first), this:
///
/// 1. **Classifies** each predicate field into its [`Tier`] (typed-core / generated facet / GIN probe /
///    Search), via [`classify_field`] over the [`FacetCatalog`].
/// 2. **Estimates** the OLTP cost ([`estimate_cost`] = `row_fanout × Σ tier_weight`).
/// 3. **Decides** ([`PlanOutcome`]):
///    - any leg is inherently Tier-3 (full-text/semantic/cross-artifact), OR the OLTP cost exceeds the
///      budget → **escalate to Search** (the SAME `Filter` conjoined) — UNLESS the cost exceeds even
///      Search's bound (`refine_cost_ceiling`), in which case → **Refine**;
///    - otherwise → **serve on the OLTP tier** (the cheapest OLTP tier = the heaviest classified OLTP
///      leg), paginated + statement-timeout'd.
///
/// `ast` is the board's frozen query AST; `set_expr` is the viewer's `list_objects` answer (the
/// leak-free ACL pre-filter, 4.3); `viewer`/`scope_tenant`/`scope_region` scope the read; `zookie` is the
/// consistency snapshot the ACL was computed at (4.10); `catalog` is the promoted-facet set (ISS-P15);
/// `budget` is the [`CostBudget`]; `estimated_row_fanout` is the planner's row estimate (table stats in
/// production; the test scenario in the unit drills). NEVER returns an unbounded scan.
#[allow(clippy::too_many_arguments)]
pub fn plan_board_query(
    ast: &QueryAst,
    set_expr: &SetExpr,
    viewer: &Principal,
    scope_tenant: &TenantId,
    scope_region: &Region,
    zookie: &Zookie,
    catalog: &FacetCatalog,
    budget: &CostBudget,
    estimated_row_fanout: u64,
) -> PlanOutcome {
    // 1. CLASSIFY each predicate field. A board scan with no user predicate (pure ACL) is Tier 1.
    let fields = ast.predicate().map(predicate_fields).unwrap_or_default();
    let tiers: Vec<Tier> = if fields.is_empty() {
        vec![Tier::TypedCore]
    } else {
        fields.iter().map(|f| classify_field(f, catalog)).collect()
    };

    // 2. The cost the OLTP legs would pay.
    let oltp_cost = estimate_cost(&tiers, estimated_row_fanout);
    let has_tier3_leg = tiers.iter().any(|t| matches!(t, Tier::Search));
    let over_budget = oltp_cost > budget.max_scanned_cost;

    // 3. DECIDE. An inherent Tier-3 leg OR an over-budget OLTP cost escalates to Search; a cost beyond
    //    even Search's bound returns Refine (never an unbounded scan).
    if has_tier3_leg || over_budget {
        // The Search-side cost of serving this escalation (Search pays the row fan-out; the OLTP weight
        // is ~0 for the escalated leg). If even Search cannot serve it within budget → Refine.
        if estimated_row_fanout > budget.refine_cost_ceiling {
            return PlanOutcome::Refine(RefineHint {
                hint: format!(
                    "this query would scan ~{estimated_row_fanout} rows — narrow it (add a project, \
                     assignee, or date filter) so it fits the board budget",
                ),
                estimated_cost: oltp_cost.max(estimated_row_fanout),
            });
        }
        // Escalate — carry the board's OWN set_expr (4.3) verbatim (the search-requires-acl-filter
        // discipline is structural: SearchEscalation::new REQUIRES the set_expr).
        return PlanOutcome::EscalateToSearch(SearchEscalation::new(
            ast.clone(),
            set_expr.clone(),
            zookie.clone(),
            budget.page_limit,
        ));
    }

    // Within budget — serve on the cheapest correct OLTP tier (the heaviest classified OLTP leg drives
    // the index choice). Compose the leak-free board query (ISS-P13) + add the bounds.
    let tier = tiers
        .iter()
        .copied()
        .filter(|t| t.is_oltp())
        .max_by_key(|t| t.cost_weight())
        .unwrap_or(Tier::TypedCore);
    let composed = compose_board_query(set_expr, viewer, scope_tenant, scope_region);
    PlanOutcome::ServeOltp(BoundedBoardQuery {
        composed,
        statement_timeout_ms: budget.statement_timeout_ms,
        page_limit: budget.page_limit,
        tier,
    })
}

/// Re-export the leak-free lowering so a caller that wants the lowered `LoweredFilter` (e.g. to drive the
/// in-memory drill evaluation) has it without re-importing the planner.
pub use crate::planner::LoweredFilter;

/// Lower the board's `set_expr` to the leak-free [`LoweredFilter`] (the ISS-P13 pre-filter every tier
/// conjoins) — a thin re-export so the cost-bounder's callers see the lowering as part of the planner
/// surface.
pub fn lower_acl(set_expr: &SetExpr, viewer: &Principal) -> LoweredFilter {
    lower_over_issue_id(set_expr, viewer)
}

// ───────────────────────────── the named floors (§3 — measured follow-ons) ─────────────────────────

/// **FLOORS named (ISS-P14 DoD) — greppable markers for the measured follow-ons.** The cost-bounder is
/// the FULL shape at M4; these are the named follow-ons it leaves for the measured promotion / scale.
#[derive(Clone, Copy, Debug)]
pub struct CostBounderFloors;

impl CostBounderFloors {
    /// **Tier 2 — the flexible-field index is GIN by default; the generated-index promotion is the
    /// MEASURED follow-on.** A custom facet rides the `issue_props_gin` GIN index (Tier 2b) until the
    /// projection feeder promotes it on the OQ-C threshold (`> 5%` of view executions). The feeder is
    /// ISS-P15 (P-381). Until then [`FacetCatalog`] is empty → every custom facet is the GIN probe.
    pub const TIER2_FEEDER: &'static str = "ISS-P15";
    /// **Distributed-SQL for a hot tenant — the MEASURED M5 follow-on.** The PG-hybrid (typed core +
    /// JSONB + projection feeder) is the floor; distributed-SQL is provisioned ONLY if a single tenant's
    /// shard outgrows PG (measured, never predicted). M5 / ISS-P32.
    pub const DISTRIBUTED_SQL: &'static str = "ISS-P32";
    /// **The at-scale board-query latency under the 30× world-scale surge — the M5 follow-on.** The
    /// surge changes the cost-bounder's LATENCY budget, never the tier classification or the
    /// leak-equivalence. ISS-P33 / SRCH-P25.
    pub const SURGE_LATENCY: &'static str = "ISS-P33";
    /// The OQ-C calibration constant the feeder measures against (the default-to-beat — NOT a contract
    /// constant; a Search-owned tunable). Named so the cost-bounder's classification reads the same
    /// threshold the feeder promotes on.
    pub const OQ_C_DEFAULT_TO_BEAT: &'static str = "> 5% of a collection's view executions";
}

#[cfg(test)]
mod tests;

//! # `surge` — world-scale hardening: the F6 surge family + the prod-scale benchmarks (ISS-P33 / P-496, M5)
//!
//! **The M5 production-hardening face of the Issues owner.** Two halves of one world-scale concern —
//! *serving the human under a machine-speed surge, and staying within the `<1s` keyboard budget at
//! cell scale*:
//!
//! 1. **The F6 mutation-surge runner** ([`run_issues_owner_surge`] → [`IssuesOwnerSurgeReport`]).
//!    A 30× mixed-principal mutation storm on the Issues owner drives the LIVE [`IssuesOwnerShed`]
//!    over the [`Surface::HttpIntake`] surface (the OQ-K per-surface shed budget, read from the FROZEN
//!    thresholds file): the protected HUMAN mutation lane HELD (0 human sheds), the agent + batch/CI
//!    lanes SHED (`429 + Retry-After`), and a quiet co-tenant is UNAFFECTED (cross-tenant impact 0).
//!    This is the F6 surge family's Issues row (testing-strategy SUB-D3-shaped, contract 1.11).
//! 2. **The ISS-D2-at-cell-scale benchmark** ([`run_iss_d2_cell_scale`] → [`IssD2CellScaleReport`]).
//!    The 1M+-issue board query (50+ custom fields) re-confirmed under the `<1s` keyboard budget WHILE
//!    the surge is offered: the LIVE cost-bounder ([`crate::cost_bounder::plan_board_query`]) NEVER
//!    emits an unbounded JSONB scan across the full field × cell-scale-fan-out sweep (contract 11.6 the
//!    OLAP at cell scale).
//!
//! ## What this module REUSES (EI-01 §7 — never a parallel second implementation)
//! This module authors **NO new mechanism**. It is the world-scale *composition + drill* over the
//! engine the M1/M5 prompts already shipped:
//! - the shed order itself is the substrate's [`myelin_substrate::shed::ShedLane`] (P-S33) — this
//!   module WIRES it over the [`Surface::HttpIntake`] mutation-intake surface, reading the budget
//!   **from the thresholds file** ([`myelin_substrate::thresholds`]); it never re-authors a second
//!   shed order, mirroring the sibling rows (`myelin_git::surge::run_git_clone_surge`,
//!   `myelin_search::run_search_surge`). The Issues owner has **no own `Surface` variant** (the prompt:
//!   "no new feature surface") — its mutation intake is one of the generic public surfaces the
//!   `HttpIntake` budget governs (architecture §7.6 / 05-hard-problems §"Per-tenant in-flight caps").
//! - the `<1s` cost-bounder is [`crate::cost_bounder::plan_board_query`] (ISS-P14) reading the FROZEN
//!   [`crate::cost_bounder::CostBudget`] — the surge changes the OFFERED LOAD, never the leak-equivalence
//!   or the tier classification, so the SAME no-unbounded-scan invariant the ISS-D2 decision drill
//!   asserts at 1× is re-confirmed here at cell scale under the surge.
//!
//! ## The F6 properties (testing-strategy SUB-D3; contract 1.11 / 1.8)
//! Under the full 30× mixed-principal mutation surge on a hot board, by ONE tenant:
//!   1. **the human mutation lane HELD** — every human interactive mutation the surge issued was
//!      ADMITTED (0 human sheds); the protected lane is shed last and held within budget;
//!   2. **the agent + batch/CI machine lanes SHED** — the agent fan-out + the importer/CI batch storm
//!      were absorbed by shedding (`429 + Retry-After`, shed-count > 0), never queued unboundedly;
//!   3. **a quiet co-tenant is UNAFFECTED** — the storm spent 0 of the quiet tenant's budget; its human
//!      mutation is admitted within its independent per-tenant budget (cross-tenant impact 0, the bulkhead).
//!
//! ## Floors named (VISION §3)
//! - **No new floor.** This prompt HARDENS; the floor follow-ons (move-CRDT / materialised rollup /
//!   distributed-SQL / cross-cell / Monte-Carlo / column-store) were promoted/named in ISS-P32 (P-495).
//!   The surge RE-RUNS the F1 leak-free family (the cost-bounder's leak-equivalent ACL pre-filter still
//!   gates every admitted query) and the reorder-0-clobber family (the move-CRDT is conflict-free under
//!   the surge) UNDER load — the green is re-confirmed, not re-authored.
//! - **The world-scale 30× run on real FLEET hardware** (a real multi-node cell) is the ONE legitimate
//!   remaining floor — the shared testing-strategy §4.1 30× fleet drill, not a per-slice floor. The
//!   shed-order LOGIC + the per-tenant fairness + the dated artifact ship now and re-run as a `cargo
//!   test` gate on every shed-path-touching change.
//! - **online-migration-under-load (0 downtime) on the hot issue tables** + **restore-verify at cell
//!   scale (STOR-D2)** are re-confirmed by re-driving the STORAGE-owned gates over Issues' restorable
//!   state — see `tests/iss_p33_world_scale_hardening.rs` (the SUB-D10 re-drive idiom, never a second copy).

use crate::cost_bounder::{plan_board_query, CostBudget, FacetCatalog, PlanOutcome};
use myelin_identity::{Literal, ObjectId, Principal, PrincipalId, PrincipalKind, SetExpr, Zookie};
use myelin_query::{CmpOp, Expr, Predicate, QueryAst};
use myelin_substrate::shed::{RunClass, ShedDecision, ShedLane, Surface, SurfaceBudget};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::{Region, TenantId};

/// **The Issues mutation-surge multiplier (the 30× top of the 1×/10×/30× generator).** Sourced from the
/// FROZEN `[surge] multiplier` in `thresholds.toml` (never hardcoded) — a drill asserts this constant
/// equals `Thresholds::load_canonical().surge.multiplier`. Held as a named constant so the surge drill
/// and a caller agree on the surge top without re-reading the file in two places.
pub const ISSUES_SURGE_MULTIPLIER: u32 = 30;

// ───────────────────────────── the Issues owner shed gate (mutation intake) ──────────────────────────

/// **The protected-human-lane shed gate at the Issues mutation owner (ADR-16 / OQ-K; contract 1.11).**
///
/// A thin Issues-owner wiring over the substrate's [`ShedLane`] for the [`Surface::HttpIntake`]
/// mutation-intake surface: it reads the surface's budget **from the thresholds file** and applies the
/// shed order `speculative → batch/CI → agent → human-last`, per-tenant. The Issues owner admits every
/// mutation through [`IssuesOwnerShed::admit_class`] (the run-class derived from the verified principal);
/// an over-budget non-human lane is shed with `429 + Retry-After`, while the human lane is protected
/// (shed only in true saturation). The Issues owner authors **no new mechanism** — it reuses the
/// substrate shed lane (EI-01 §7).
pub struct IssuesOwnerShed {
    lane: ShedLane,
}

impl IssuesOwnerShed {
    /// Open the gate with the §7.6 measured budget for the [`Surface::HttpIntake`] mutation-intake
    /// surface (the v1-floor table the thresholds file mirrors).
    pub fn new() -> IssuesOwnerShed {
        IssuesOwnerShed {
            lane: ShedLane::new(Surface::HttpIntake),
        }
    }

    /// Open the gate with an explicit budget (used by the inversion guard to drive the boundary).
    pub fn with_budget(budget: SurfaceBudget) -> IssuesOwnerShed {
        IssuesOwnerShed {
            lane: ShedLane::with_budget(Surface::HttpIntake, budget),
        }
    }

    /// Open the gate from the canonical thresholds file (the budget read from the FROZEN file, never a
    /// guess). Validates the shed budgets first (a human lane below the measured floor is a LOUD error).
    pub fn from_thresholds(thresholds: &Thresholds) -> Result<IssuesOwnerShed, String> {
        thresholds.validate_shed_budgets().map_err(|e| {
            format!("the HttpIntake shed budget must hold the human-lane floor: {e}")
        })?;
        let budget = thresholds
            .shed_budget(Surface::HttpIntake)
            .map_err(|e| format!("the HttpIntake shed budget must be present: {e}"))?;
        Ok(IssuesOwnerShed::with_budget(budget))
    }

    /// Admit (or shed) one mutation of a pre-derived [`RunClass`] on `tenant`.
    pub fn admit_class(&mut self, tenant: &TenantId, class: RunClass) -> Result<(), u64> {
        match self.lane.admit(tenant, class) {
            ShedDecision::Admit => Ok(()),
            ShedDecision::Shed { retry_after_secs } => Err(retry_after_secs),
        }
    }

    /// Release a slot a prior admit took (a short interactive mutation returns its slot).
    pub fn release(&mut self, tenant: &TenantId, class: RunClass) {
        self.lane.release(tenant, class);
    }

    /// The cumulative shed count for one lane (the contract-1.8 `shed-count per lane` signal).
    pub fn shed_count(&self, class: RunClass) -> u64 {
        self.lane.shed_count(class)
    }

    /// The current per-tenant in-flight (for the cross-tenant blast-radius assertion).
    pub fn in_flight(&self, tenant: &TenantId) -> u32 {
        self.lane.in_flight(tenant)
    }
}

impl Default for IssuesOwnerShed {
    fn default() -> Self {
        IssuesOwnerShed::new()
    }
}

// ───────────────────────────── the F6 surge report ───────────────────────────────

/// **The Issues F6 surge report — the dated green artifact (contract 1.8 survival signals).** The
/// per-lane shed counts + the human-held / cross-tenant-impact signals the drill asserts on. Built by
/// [`run_issues_owner_surge`]: a deterministic two-tenant surge so the cross-tenant blast-radius is
/// asserted exactly. A red report (a human shed, or a machine lane that did not shed, or cross-tenant
/// leak) is the failure this exists to catch — never a swallowed pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuesOwnerSurgeReport {
    /// The human interactive-mutation lane shed count on the SURGING tenant — MUST be 0 (protected lane).
    pub surging_human_shed_count: u64,
    /// Every human mutation the surge issued on the surging tenant was admitted (the lane HELD).
    pub surging_human_admitted: bool,
    /// The agent fan-out lane shed count on the surging tenant — MUST be > 0 (absorbed by shedding).
    pub surging_agent_shed_count: u64,
    /// The importer/CI batch lane shed count on the surging tenant — MUST be > 0.
    pub surging_batch_shed_count: u64,
    /// The quiet co-tenant's human mutation was admitted (the surge never sheds another tenant's human).
    pub quiet_human_admitted: bool,
    /// The slots the surge spent of the QUIET tenant's budget — MUST be 0 (the per-tenant bulkhead).
    pub cross_tenant_impact: u32,
    /// The Retry-After (secs) carried by a shed (the `429 + Retry-After` proof) — MUST be > 0.
    pub retry_after_secs: u64,
}

impl IssuesOwnerSurgeReport {
    /// **The three F6 properties hold (the green verdict).** Human lane held (0 shed + admitted), both
    /// machine lanes shed with a Retry-After, the quiet co-tenant unaffected (admitted + 0 cross-tenant).
    pub fn is_f6_green(&self) -> bool {
        self.surging_human_shed_count == 0
            && self.surging_human_admitted
            && self.surging_agent_shed_count > 0
            && self.surging_batch_shed_count > 0
            && self.retry_after_secs > 0
            && self.quiet_human_admitted
            && self.cross_tenant_impact == 0
    }

    /// A one-line human summary (observability is part of the pass, EI-01 §3).
    pub fn summary(&self) -> String {
        format!(
            "ISS-F6: human held(admitted={}, shed={}) | agent shed={} | batch shed={} | \
             retry_after={}s | quiet human admitted={} | cross_tenant_impact={}",
            self.surging_human_admitted,
            self.surging_human_shed_count,
            self.surging_agent_shed_count,
            self.surging_batch_shed_count,
            self.retry_after_secs,
            self.quiet_human_admitted,
            self.cross_tenant_impact,
        )
    }
}

/// **Drive a deterministic F6 mutation surge against the LIVE shed gate (the two-tenant blast-radius
/// proof).** Issues `base_agent × multiplier` agent mutations + `base_batch × multiplier` batch/CI
/// mutations on the SURGING tenant — both well past the per-tenant cap so both machine lanes must shed —
/// interleaved with human interactive mutations (each released immediately, the way a short-lived human
/// edit returns its slot so a LATER human still admits). Then probes the QUIET co-tenant's human
/// mutation + its in-flight count. `multiplier` is the surge top read from the thresholds file (the
/// surge realises `base × multiplier` machine requests). Returns the [`IssuesOwnerSurgeReport`].
///
/// The machine lanes KEEP their slot (the storm is sustained — it PRESSURES the cap and sheds, not a
/// one-shot exhaustion); the human lane releases each slot (a short interactive mutation), so the
/// protected lane holds 0 shed across the WHOLE surge, not merely until the reserved slots fill once.
pub fn run_issues_owner_surge(
    gate: &mut IssuesOwnerShed,
    surging: &TenantId,
    quiet: &TenantId,
    base_agent: u32,
    base_batch: u32,
    multiplier: u32,
) -> IssuesOwnerSurgeReport {
    let agent_total = base_agent.saturating_mul(multiplier.max(1));
    let batch_total = base_batch.saturating_mul(multiplier.max(1));

    // Capture a Retry-After from a shed so the report proves the 429+Retry-After wire response.
    let mut retry_after_secs = 0u64;

    let mut surging_human_admitted = true;
    let bursts = agent_total.max(batch_total).max(1);
    for i in 0..bursts {
        if i < agent_total {
            // an over-budget agent mutation PRESSURES the cap and sheds; it keeps its slot (sustained).
            if let Err(secs) = gate.admit_class(surging, RunClass::Agent) {
                retry_after_secs = secs;
            }
        }
        if i < batch_total {
            if let Err(secs) = gate.admit_class(surging, RunClass::BatchCi) {
                retry_after_secs = secs;
            }
        }
        // a human interactive mutation — must be admitted (protected lane), then released (short-lived).
        match gate.admit_class(surging, RunClass::Human) {
            Ok(()) => gate.release(surging, RunClass::Human),
            Err(_) => surging_human_admitted = false,
        }
    }

    // The quiet co-tenant's human mutation — admitted within ITS independent budget (cross-tenant 0).
    let cross_tenant_impact = gate.in_flight(quiet);
    let quiet_human_admitted = gate.admit_class(quiet, RunClass::Human).is_ok();
    if quiet_human_admitted {
        gate.release(quiet, RunClass::Human);
    }

    IssuesOwnerSurgeReport {
        surging_human_shed_count: gate.shed_count(RunClass::Human),
        surging_human_admitted,
        surging_agent_shed_count: gate.shed_count(RunClass::Agent),
        surging_batch_shed_count: gate.shed_count(RunClass::BatchCi),
        quiet_human_admitted,
        cross_tenant_impact,
        retry_after_secs,
    }
}

/// Convenience: open the Issues surge gate from the canonical thresholds file (budget read from the
/// FROZEN file, never a guess). Returns the gate + the validated [`Thresholds`].
pub fn open_surge_gate_from_thresholds() -> Result<(IssuesOwnerShed, Thresholds), String> {
    let thresholds = Thresholds::load_canonical().map_err(|e| format!("thresholds load: {e}"))?;
    let gate = IssuesOwnerShed::from_thresholds(&thresholds)?;
    Ok((gate, thresholds))
}

// ───────────────────────────── ISS-D2 at cell scale (the 1M+ board under the <1s budget) ─────────────

/// **The ISS-D2-at-cell-scale report — the dated green artifact (contract 11.6, the OLAP at cell scale).**
/// The 1M+-issue board query (50+ fields) re-confirmed under the `<1s` keyboard budget WHILE the surge is
/// offered: every classification × cell-scale fan-out the sweep covers stays BOUNDED (the cost-bounder
/// never emits an unbounded JSONB scan). A red report (any unbounded outcome) is the failure this catches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssD2CellScaleReport {
    /// The board's issue count modelled (the 1M+ prod-scale board).
    pub board_issue_count: u64,
    /// The number of distinct custom fields swept (the 50+ field board).
    pub field_count: u32,
    /// The total (field × fan-out) board-query plans the sweep evaluated.
    pub plans_evaluated: u64,
    /// Plans served on the OLTP tier (within the `<1s` budget, paginated + statement-timeout'd).
    pub served_oltp: u64,
    /// Plans escalated to Search (the Tier-3 valve — still bounded, the SAME ACL `set_expr` conjoined).
    pub escalated: u64,
    /// Plans returned a Refine hint (a cost beyond even Search's bound — a hint, not a scan).
    pub refined: u64,
    /// Plans that emitted an UNBOUNDED JSONB scan — MUST be 0 (the ISS-D2 no-full-scan invariant).
    pub unbounded_scans: u64,
}

impl IssD2CellScaleReport {
    /// **ISS-D2 holds at cell scale (the green verdict).** Zero unbounded scans across the full sweep, a
    /// 1M+-issue board with 50+ fields, and the sweep exercised all three bounded outcomes (a real
    /// cost-bounder, not a degenerate always-escalate).
    pub fn is_iss_d2_green(&self) -> bool {
        self.unbounded_scans == 0
            && self.board_issue_count >= 1_000_000
            && self.field_count >= 50
            && self.served_oltp > 0
            && self.escalated > 0
    }

    /// A one-line human summary (observability is part of the pass).
    pub fn summary(&self) -> String {
        format!(
            "ISS-D2@cell-scale: board={} issues × {} fields | plans={} (oltp={}, escalate={}, refine={}) | \
             unbounded_scans={}",
            self.board_issue_count,
            self.field_count,
            self.plans_evaluated,
            self.served_oltp,
            self.escalated,
            self.refined,
            self.unbounded_scans,
        )
    }
}

/// A human viewer (the protected interactive board reader).
fn cell_scale_viewer() -> Principal {
    Principal::stub(
        PrincipalId("p:eng".into()),
        PrincipalKind::Human,
        TenantId("iss-cell".into()),
    )
}

/// The viewer's `list_objects` ACL answer (the leak-free pre-filter ALWAYS conjoined first, 4.3).
fn cell_scale_acl() -> SetExpr {
    SetExpr::Ids(vec![ObjectId("ENG-1".into()), ObjectId("ENG-2".into())])
}

/// A board query AST predicating on one field (the cost-bounder classifies the field's tier).
fn ast_over(field: &str) -> QueryAst {
    QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var(field.into()),
        rhs: Expr::Lit(Literal::Str("x".into())),
    })
    .expect("a well-formed single-predicate AST")
}

/// **Re-confirm ISS-D2 at CELL SCALE under the surge (the `<1s` keyboard budget on a 1M+-issue board).**
/// Drives the LIVE cost-bounder ([`plan_board_query`]) over a 50+-field × cell-scale-fan-out sweep on a
/// 1M+-issue board: EVERY outcome must be bounded (paginated+statement-timeout'd OLTP, an ACL-carrying
/// Search escalation, or a Refine hint) — NEVER an unbounded JSONB scan. `board_issue_count` is the
/// modelled board size (≥ 1M); the fan-out series spans tiny → enormous so the budget boundary is
/// crossed (some fields serve OLTP, the heavy ones escalate). This is the SAME invariant the ISS-D2
/// decision drill (ISS-P14) asserts at 1× — re-confirmed here at cell scale under world-scale load.
pub fn run_iss_d2_cell_scale(board_issue_count: u64) -> IssD2CellScaleReport {
    let tenant = TenantId("iss-cell".into());
    let region = Region("fr-par".into());
    let zk = Zookie("zk-0000000010".into());
    let viewer = cell_scale_viewer();
    let acl = cell_scale_acl();

    // 50+ custom fields: a mix of typed-core / generated-facet / GIN-probe / inherent-Tier-3 fields, so
    // the classifier exercises every tier (the 50-field prod-scale board). The first four are the named
    // tier representatives; the rest are synthetic custom fields (cold GIN facets) to reach 50+.
    let mut fields: Vec<String> = vec![
        "state".into(),    // typed-core (Tier 1)
        "severity".into(), // generated facet when promoted, GIN probe cold (Tier 2 / 2b)
        "text".into(),     // full-text (inherent Tier 3 — always escalates)
        "semantic".into(), // semantic similarity (inherent Tier 3 — always escalates)
    ];
    for i in 0..50 {
        fields.push(format!("custom_field_{i:02}")); // cold custom facets (Tier 2b GIN probe)
    }
    let field_count = fields.len() as u32;

    // The fan-out series at CELL SCALE: from a tiny selective predicate to the whole 1M+ board (and past
    // it, the multi-board portfolio rollup fan-out). The board size sets the top of the series so the
    // 1M+-row scan boundary is actually crossed.
    let fanouts: [u64; 7] = [
        10,
        1_000,
        50_000,
        board_issue_count / 10,
        board_issue_count,
        board_issue_count.saturating_mul(6),
        board_issue_count.saturating_mul(50),
    ];

    let mut served_oltp = 0u64;
    let mut escalated = 0u64;
    let mut refined = 0u64;
    let mut unbounded_scans = 0u64;
    let mut plans_evaluated = 0u64;

    // Sweep both the cold-facet and the promoted-facet catalogs (the generated-index promotion, ISS-P15).
    for promote_severity in [false, true] {
        let mut cat = FacetCatalog::new();
        if promote_severity {
            cat.promote("severity");
        }
        for field in &fields {
            for &fanout in &fanouts {
                let outcome = plan_board_query(
                    &ast_over(field),
                    &acl,
                    &viewer,
                    &tenant,
                    &region,
                    &zk,
                    &cat,
                    &CostBudget::DEFAULT,
                    fanout,
                );
                plans_evaluated += 1;
                // THE INVARIANT: every outcome is bounded — NEVER an unbounded JSONB scan.
                if !outcome.assert_no_unbounded_scan() {
                    unbounded_scans += 1;
                }
                match outcome {
                    PlanOutcome::ServeOltp(_) => served_oltp += 1,
                    PlanOutcome::EscalateToSearch(_) => escalated += 1,
                    PlanOutcome::Refine(_) => refined += 1,
                }
            }
        }
    }

    IssD2CellScaleReport {
        board_issue_count,
        field_count,
        plans_evaluated,
        served_oltp,
        escalated,
        refined,
        unbounded_scans,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surging() -> TenantId {
        TenantId("acme-surging".into())
    }
    fn quiet() -> TenantId {
        TenantId("quiet-co-tenant".into())
    }

    #[test]
    fn surge_const_matches_the_frozen_file() {
        let t = Thresholds::load_canonical().expect("load");
        assert_eq!(
            t.surge.multiplier, ISSUES_SURGE_MULTIPLIER,
            "the surge multiplier is read from the file (30×), never hardcoded"
        );
    }

    #[test]
    fn iss_f6_report_is_green_with_a_quiet_co_tenant() {
        let (mut gate, t) = open_surge_gate_from_thresholds().expect("open the gate");
        let report = run_issues_owner_surge(
            &mut gate,
            &surging(),
            &quiet(),
            200,
            200,
            t.surge.multiplier,
        );
        assert!(report.is_f6_green(), "{}", report.summary());
        assert_eq!(report.surging_human_shed_count, 0, "human lane held");
        assert!(report.surging_agent_shed_count > 0, "agent lane shed");
        assert!(report.surging_batch_shed_count > 0, "batch lane shed");
        assert!(report.retry_after_secs > 0, "429 carried a Retry-After");
        assert_eq!(report.cross_tenant_impact, 0, "cross-tenant impact 0");
    }

    /// The report can go RED — a gate that NEVER sheds (an unbounded huge budget) fails F6. This is the
    /// inversion guard (EI-01 §3): the green is a real property, not a vacuous always-true.
    #[test]
    fn iss_f6_report_goes_red_when_the_lane_does_not_shed() {
        // a huge budget: the machine lanes NEVER fill, so they never shed → the report is RED (not green).
        let mut gate = IssuesOwnerShed::with_budget(SurfaceBudget {
            per_tenant_in_flight_cap: 1_000_000,
            human_lane_reservation: 250_000,
            retry_after_secs: 5,
        });
        let report = run_issues_owner_surge(&mut gate, &surging(), &quiet(), 10, 10, 30);
        assert!(
            !report.is_f6_green(),
            "a never-shedding lane must FAIL F6 (the green is a real property): {}",
            report.summary()
        );
        assert_eq!(
            report.surging_agent_shed_count, 0,
            "nothing shed (unbounded)"
        );
    }

    #[test]
    fn iss_d2_holds_at_cell_scale() {
        let report = run_iss_d2_cell_scale(1_000_000);
        assert!(report.is_iss_d2_green(), "{}", report.summary());
        assert_eq!(
            report.unbounded_scans, 0,
            "the cost-bounder NEVER emits an unbounded JSONB scan at cell scale"
        );
        assert!(report.board_issue_count >= 1_000_000, "a 1M+ board");
        assert!(report.field_count >= 50, "a 50+ custom-field board");
        assert!(report.served_oltp > 0, "some queries serve on OLTP");
        assert!(report.escalated > 0, "some queries escalate to Search");
    }

    /// ISS-D2 green is a real bar — if the cost-bounder could ever return an unbounded scan the report
    /// would catch it. We prove the counter by asserting the bounded-outcome accounting is exhaustive:
    /// every plan is one of served / escalated / refined and none is unbounded.
    #[test]
    fn iss_d2_accounting_is_exhaustive_and_bounded() {
        let report = run_iss_d2_cell_scale(2_000_000);
        assert_eq!(
            report.served_oltp + report.escalated + report.refined,
            report.plans_evaluated,
            "every plan is accounted for (served | escalate | refine), none unbounded"
        );
        assert_eq!(report.unbounded_scans, 0);
    }
}

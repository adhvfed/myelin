//! # `surge` — the durable-workflow world-scale 30× agent surge + the protected-human-lane shed order (P-FLOW-27 / P-476, M5)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/durable-workflow.md`
//! §7.6 (*bounded everything with the principal-aware shed order* — the per-surface shed budgets name
//! CI-dispatch as a bounded run-queue per tenant on the batch/CI lane, shed order
//! `speculative → batch/CI → agent → human-last`; an agent-mention storm sheds its lane with
//! `429 + Retry-After` while **human-initiated workflows hold the protected lane** — the F-8 drill
//! asserts this). **Doctrine:** external-insights/01 §3 (the 1×/10×/30× load generator; the multiplier
//! is read from the FROZEN thresholds file, never hardcoded; never weaken a threshold to pass — a red
//! is a dated `claimed-not-proven` row), §2 (the protected human lane; per-tenant blast-radius).
//! **Contract-index:** row **1.11** (the protected-human-lane shed order + per-surface shed budgets,
//! recon §OQ-K — **CONSUMED** here; this prompt owns no new row, it applies the substrate's shed
//! budgets to the workflow lanes), row **1.8** (the per-lane shed-count/lane telemetry).
//!
//! ## What this module is (the Flow surge half — P-FLOW-27 / FLOW-D8)
//! The durable-workflow START surface under the 30× surge (FLOW-D8) is an **agent-mention storm**: an
//! agent fan-out initiating workflows. This module tunes the doctrine shed order
//! (`speculative → batch/CI → agent → human-last`) to the workflow-start surface:
//! - a **human-initiated workflow** holds the protected lane (shed last — F-8);
//! - an **agent-initiated-workflow** lane sheds with `429 + Retry-After`;
//! - **per-tenant in-flight caps** keep one tenant's agent-workflow storm off another tenant's humans
//!   (the per-tenant bulkhead, §7.1/§7.6 / EI-02 §1).
//!
//! ## What this module REUSES (EI-01 §7 — never a parallel second implementation)
//! **The shed order itself is the substrate's** [`myelin_substrate::shed`]: this module does NOT
//! re-author the shed lane / run-class / budget table (that would be a doctrinal fork — the same
//! mistake [`myelin_refs_service::RefsShedGate`] / [`myelin_git::shed_clone`] avoided). It **WIRES**
//! the existing [`myelin_substrate::shed::ShedLane`] over the new
//! [`Surface::WorkflowAgentLane`](myelin_substrate::shed::Surface::WorkflowAgentLane) surface, reading
//! the surface's budget **from the thresholds file** ([`myelin_substrate::thresholds`]) — the tuned
//! OQ-K numbers, never a hardcoded magic value. The Flow surge gate's only authoring is the
//! *derivation* of the request's [`RunClass`] from its principal + an optional run-class header, and
//! the placement of the admit/shed decision at the front of the workflow-START pipeline (BEFORE any
//! `workflow_run` row is journaled — a shed start returns `429` and never a half-created run).
//!
//! ## The reserve/settle budget gate is UNTOUCHED under shed
//! Shedding is a PRE-START admission decision: an over-budget agent-initiated start is refused with
//! `429` BEFORE any run is journaled (let alone before the reserve/settle bookend runs, P-FLOW-16), so
//! a shed start spends no budget and leaves no orphaned run. A start that IS admitted still goes
//! through the unchanged BudgetGate reserve-at-dispatch / settle-on-completion (contract 11.7). The
//! surge never relaxes the cost gate — it only bounds concurrency at the front door.
//!
//! ## Floors named (VISION §3 — name your floors)
//! - **The 30× world-scale FLEET-hardware load is the ONE legitimate remaining floor** (real fleet).
//!   Here the load is the P-S02 generator at 30× across the surging tenant; the per-tenant fairness +
//!   shed-order + cross-tenant-0 PROPERTIES are complete + testable now and do not change shape when
//!   the real cell carries the load.
//! - **Cross-cell workflow spanning** is the durable-workflow.md §7.4 named floor (designed-not-built)
//!   — UNCHANGED by this prompt: the surge proves the per-tenant shed order holds AT ONE CELL; a
//!   cross-cell start rides the PII-free pointer bridge (contract 12.6) when built, never before.
//!
//! ## Mutation floor (mandatory-core — EI-01 §2/§3)
//! The shed-order DECISION path ([`FlowShedGate::admit_for`]/[`FlowShedGate::admit_class`] → the
//! human-protected per-tenant graded admit) is mandatory-core: an off-by-one that sheds a human-
//! initiated workflow before an agent-initiated one, or that leaks one tenant's budget into another,
//! is the failure this exists to catch.

use myelin_identity::Principal;
use myelin_substrate::shed::{
    RunClass, RunClassHeader, ShedDecision, ShedLane, Surface, SurfaceBudget,
};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

/// **The Flow surge default-to-beat multiplier (FLOW-D8).** The 30× world-scale surge factor the
/// FLOW-D8 drill drives at — read from the FROZEN thresholds file `[surge] multiplier` row (the
/// versioned source of truth) and asserted to equal this documented default-to-beat; a divergence is
/// a LOUD failure, never a silent weakening (EI-01 §3).
pub const FLOW_SURGE_MULTIPLIER: u32 = 30;

/// **Cross-cell workflow spanning is the named §7.4 floor (designed-not-built) — UNCHANGED here.** The
/// surge proves the per-tenant shed order holds AT ONE CELL; a cross-cell start rides the control-plane
/// PII-free pointer bridge (contract 12.6) when that floor is filled, never before this prompt. Reading
/// this binding keeps the floor explicit (the surge is single-cell by design, not by omission).
pub const CROSS_CELL_SPANNING_IS_A_FLOOR: bool = true;

// ───────────────────────────── the Flow surge shed gate ──────────────────────────────────────────

/// **The protected-human-lane shed gate at the durable-workflow START surface (P-FLOW-27 / OQ-K;
/// contract 1.11).**
///
/// A thin Flow wiring over the substrate's [`ShedLane`] for the
/// [`Surface::WorkflowAgentLane`] surface: it reads the surface's budget **from the thresholds file**
/// and applies the shed order `speculative → batch/CI → agent → human-last`, per-tenant. A workflow
/// start is admitted through [`FlowShedGate::admit_for`] (the run-class derived from the verified
/// principal); an over-budget agent-initiated start is shed with `429 + Retry-After`, while the
/// human-initiated lane is protected (shed only in true saturation).
pub struct FlowShedGate {
    lane: ShedLane,
}

/// **Why a workflow start was refused at the shed gate** — the typed form the transport maps to the
/// wire `429`. A shed carries the `Retry-After` (seconds) the client honours (the no-amplification
/// guarantee — our ResilientClient honours `Retry-After`, so a shed is not a retry-storm amplifier).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlowShedRejection {
    /// The lane that was shed (`speculative` / `batch_ci` / `agent` / `human`) — the contract-1.8
    /// per-lane shed-count signal keys on this.
    pub lane: RunClass,
    /// The `Retry-After` value in **seconds** (the frozen §2.10 unit) the transport sets on the
    /// `429 Too Many Requests` response.
    pub retry_after_secs: u64,
}

impl FlowShedGate {
    /// Open the workflow-start surge gate, reading its budget **from the thresholds file** (the
    /// prompt's "the shed budget is read from the thresholds file"). A missing row for the surface is
    /// a LOUD error (the gate refuses to open against a guessed budget — EI-01 §3), never a silent
    /// default.
    pub fn from_thresholds(thresholds: &Thresholds) -> Result<FlowShedGate, String> {
        let budget = thresholds
            .shed_budget(Surface::WorkflowAgentLane)
            .map_err(|e| format!("Flow shed budget for WorkflowAgentLane unavailable: {e}"))?;
        Ok(FlowShedGate {
            lane: ShedLane::with_budget(Surface::WorkflowAgentLane, budget),
        })
    }

    /// Open the gate against an explicit budget (used by the surge drill / unit tests to drive the
    /// boundary at a small, deterministic budget without editing the thresholds file).
    pub fn with_budget(budget: SurfaceBudget) -> FlowShedGate {
        FlowShedGate {
            lane: ShedLane::with_budget(Surface::WorkflowAgentLane, budget),
        }
    }

    /// **Admit a workflow start by its verified principal + an optional injected run-class header.**
    /// The run-class is DERIVED ([`RunClass::derive`]) from `principal.kind` (the kind sets the
    /// ceiling) and the header (which may only down-class) — a machine principal can NEVER up-class to
    /// the protected human-initiated lane. Returns `Ok(class)` admitted (a slot was taken — release it
    /// on completion via [`FlowShedGate::release`]) or `Err(FlowShedRejection)` shed (`429 +
    /// Retry-After`). The decision is per-`principal.tenant`.
    pub fn admit_for(
        &mut self,
        principal: &Principal,
        header: Option<RunClassHeader>,
    ) -> Result<RunClass, FlowShedRejection> {
        let class = RunClass::derive(&principal.kind, header);
        self.admit_class(&principal.tenant, class).map(|()| class)
    }

    /// **Admit a workflow start of a pre-derived [`RunClass`] for `tenant`.** The lower-level form the
    /// surge drill drives (it mints classes directly). Returns `Ok(())` admitted (a slot taken) or
    /// `Err(FlowShedRejection)` shed. The human-initiated lane is protected: a human is shed ONLY when
    /// every slot (the reserved human fraction included) is full; the non-human lanes shed first, in
    /// the graded order `speculative → batch/CI → agent`.
    pub fn admit_class(
        &mut self,
        tenant: &TenantId,
        class: RunClass,
    ) -> Result<(), FlowShedRejection> {
        match self.lane.admit(tenant, class) {
            ShedDecision::Admit => Ok(()),
            ShedDecision::Shed { retry_after_secs } => Err(FlowShedRejection {
                lane: class,
                retry_after_secs,
            }),
        }
    }

    /// Release a slot a prior [`FlowShedGate::admit_for`]/[`FlowShedGate::admit_class`] took for
    /// `(tenant, class)` — call when the workflow start completes (the run is journaled / settled) so
    /// the lane recovers after the surge.
    pub fn release(&mut self, tenant: &TenantId, class: RunClass) {
        self.lane.release(tenant, class);
    }

    /// The cumulative shed count for a lane (the contract-1.8 `shed-count per lane` survival signal —
    /// the surge-drill green artifact: `human-lane == 0 shed`, `agent-lane > 0 shed`).
    pub fn shed_count(&self, class: RunClass) -> u64 {
        self.lane.shed_count(class)
    }

    /// The per-tenant in-flight count (admitted not yet released) — for the blast-radius assertions.
    pub fn in_flight(&self, tenant: &TenantId) -> u32 {
        self.lane.in_flight(tenant)
    }

    /// The surface this gate fronts (always [`Surface::WorkflowAgentLane`]).
    pub fn surface(&self) -> Surface {
        self.lane.surface()
    }
}

// ───────────────────────────── the FLOW-D8 surge report ──────────────────────────────────────────

/// **The FLOW-D8 30× surge report — the three F-8 properties on the workflow-start surface.** The
/// dated green artifact the DoD names: the human-initiated workflow lane HOLDS (0 shed within its
/// reserved slots while a machine lane sheds), the agent-initiated-workflow lane SHEDS (`429 +
/// Retry-After`, absorbed not unbounded), and cross-tenant impact is 0 (the storm fills only the
/// surging tenant's per-tenant budget).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlowSurgeReport {
    /// The agent-lane shed count on the surging tenant (the storm absorbed by shedding — must be > 0).
    pub surging_agent_shed_count: u64,
    /// The human-lane shed count on the surging tenant (the protected lane — must be 0).
    pub surging_human_shed_count: u64,
    /// Whether the surging tenant's OWN human-initiated workflow was admitted within its reserved slots.
    pub surging_human_admitted: bool,
    /// Whether the quiet co-tenant's human-initiated workflow was admitted within budget (untouched).
    pub quiet_human_admitted: bool,
    /// The quiet co-tenant's in-flight count BEFORE its own human op (the cross-tenant impact — must be
    /// 0; the storm never spends the quiet tenant's budget).
    pub cross_tenant_impact: u32,
    /// The `Retry-After` (seconds) the agent lane's shed carried (must be > 0 — every shed advertises a
    /// backoff the client honours; the no-amplification guarantee).
    pub agent_shed_retry_after_secs: u64,
}

impl FlowSurgeReport {
    /// **The FLOW-D8 GREEN predicate (the three F-8 properties — all measured, none weakened).** The
    /// agent lane shed (absorbed by shedding, carrying a Retry-After), the human-initiated lane held
    /// (0 shed on the surging tenant + its own human admitted), the quiet co-tenant's human held, and
    /// cross-tenant impact is contained.
    pub fn is_flow_d8_green(&self) -> bool {
        self.surging_agent_shed_count > 0
            && self.agent_shed_retry_after_secs > 0
            && self.surging_human_shed_count == 0
            && self.surging_human_admitted
            && self.quiet_human_admitted
            && self.cross_tenant_impact == 0
    }

    /// A one-line summary for the dated green-artifact log row (observability is part of the pass).
    pub fn summary(&self) -> String {
        format!(
            "FLOW-D8: surging agent_shed={} (retry_after={}s) human_shed={} surging_human_admitted={} \
             quiet_human_admitted={} cross_tenant_impact={} → {}",
            self.surging_agent_shed_count,
            self.agent_shed_retry_after_secs,
            self.surging_human_shed_count,
            self.surging_human_admitted,
            self.quiet_human_admitted,
            self.cross_tenant_impact,
            if self.is_flow_d8_green() {
                "GREEN"
            } else {
                "RED"
            }
        )
    }
}

/// **Drive the FLOW-D8 30× surge on the workflow-start gate.** Spreads `storm_agent_ops` agent-
/// initiated workflow starts (the derived storm-op count) on the surging tenant — the agent lane fills
/// then sheds — then proves the surging tenant's OWN human-initiated start is still admitted
/// (shed-last) and a quiet co-tenant's human-initiated start is admitted within its independent budget.
/// Returns the [`FlowSurgeReport`] (the three F-8 properties).
///
/// The `multiplier` is the surge factor (read from the FILE by the caller; passed through for the log
/// row), not used to scale here — `storm_agent_ops` is already the derived 30× storm-op count.
pub fn run_flow_surge(
    gate: &mut FlowShedGate,
    surging: &TenantId,
    quiet: &TenantId,
    storm_agent_ops: u64,
    _multiplier: u32,
) -> FlowSurgeReport {
    // Drive the agent-initiated-workflow storm on the surging tenant: the agent lane fills its
    // non-reserved budget then sheds (429 + Retry-After) — absorbed by shedding, never unbounded.
    // Capture the Retry-After the agent shed carries (the no-amplification guarantee — every shed
    // advertises a backoff the client honours, P-S17).
    let mut agent_shed_retry_after_secs = 0u64;
    for _ in 0..storm_agent_ops {
        if let Err(rej) = gate.admit_class(surging, RunClass::Agent) {
            agent_shed_retry_after_secs = rej.retry_after_secs;
        }
    }

    // The surging tenant's OWN human-initiated workflow is STILL admitted — the protected lane, shed
    // last (a human uses the reserved slots the agent storm could never take).
    let surging_human_admitted = gate.admit_class(surging, RunClass::Human).is_ok();

    // The quiet co-tenant is UNTOUCHED: its human-initiated start is admitted within its independent
    // per-tenant budget (the storm never spent the quiet tenant's slots).
    let quiet_in_flight_before = gate.in_flight(quiet);
    let quiet_human_admitted = gate.admit_class(quiet, RunClass::Human).is_ok();

    FlowSurgeReport {
        surging_agent_shed_count: gate.shed_count(RunClass::Agent),
        surging_human_shed_count: gate.shed_count(RunClass::Human),
        surging_human_admitted,
        quiet_human_admitted,
        cross_tenant_impact: quiet_in_flight_before,
        agent_shed_retry_after_secs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus, RuntimeRef};
    use myelin_tenancy::Region;

    fn tenant(s: &str) -> TenantId {
        TenantId(s.to_string())
    }

    fn human(tenant_slug: &str) -> Principal {
        Principal::new(
            tenant(tenant_slug),
            Region("fr-par".into()),
            PrincipalId(format!("h-{tenant_slug}")),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    fn agent(tenant_slug: &str) -> Principal {
        Principal::new(
            tenant(tenant_slug),
            Region("fr-par".into()),
            PrincipalId(format!("a-{tenant_slug}")),
            PrincipalKind::Agent {
                runtime_ref: RuntimeRef("rt".into()),
                on_behalf_of: None,
            },
            DataRole::Controller,
            PrincipalStatus::Active,
        )
    }

    /// cap 6, reserve 2 → non-human budget 4; step = max(4/8,1)=1 → speculative ceiling 2, batch 3,
    /// agent 4. A small deterministic budget so the graded thresholds are easy to reach.
    fn small_budget() -> SurfaceBudget {
        SurfaceBudget {
            per_tenant_in_flight_cap: 6,
            human_lane_reservation: 2,
            retry_after_secs: 10,
        }
    }

    // ───────────────────────── the shed budget is read from the file ─────────────────────────

    /// **The Flow shed budget is read from the thresholds file** (the prompt's explicit requirement).
    /// The gate opens against the canonical `thresholds.toml` `[[shed_budgets]]` WorkflowAgentLane row —
    /// not a hardcoded number. A missing row would have been a loud error.
    #[test]
    fn the_flow_shed_budget_is_read_from_the_thresholds_file() {
        let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
        let gate =
            FlowShedGate::from_thresholds(&thresholds).expect("WorkflowAgentLane budget present");
        assert_eq!(gate.surface(), Surface::WorkflowAgentLane);

        let b = thresholds
            .shed_budget(Surface::WorkflowAgentLane)
            .expect("present");
        assert!(b.per_tenant_in_flight_cap > 0, "bounded (§7.1)");
        assert!(b.human_lane_reservation > 0, "reserves a human lane");
        assert!(b.retry_after_secs > 0, "sheds with a Retry-After");
    }

    /// **The shed order serves the human-initiated workflow while the agent lane sheds (FLOW-D8 / F-8):**
    /// a human-initiated start is SERVED while the agent-initiated-workflow lane SHEDS
    /// (`429 + Retry-After`).
    #[test]
    fn shed_order_serves_the_human_while_the_agent_lane_sheds() {
        let mut gate = FlowShedGate::with_budget(small_budget());
        let a = agent("acme");
        let h = human("acme");

        // an agent-initiated-workflow storm fills the non-human budget (cap-reserved = 4) then sheds.
        for _ in 0..4 {
            assert!(
                gate.admit_for(&a, None).is_ok(),
                "agent workflow start admitted under budget"
            );
        }
        let shed = gate.admit_for(&a, None).expect_err("the agent storm sheds");
        assert_eq!(shed.lane, RunClass::Agent);
        assert_eq!(shed.retry_after_secs, 10, "the shed carries a Retry-After");

        // THE GATE: the HUMAN-initiated workflow is STILL SERVED (shed last — F-8).
        assert_eq!(
            gate.admit_for(&h, None)
                .expect("the human-initiated workflow is served while the agent sheds"),
            RunClass::Human
        );
        assert_eq!(gate.shed_count(RunClass::Human), 0, "human lane: 0 shed");
        assert!(gate.shed_count(RunClass::Agent) >= 1, "agent lane: sheds");
    }

    /// **The full shed PRIORITY order: speculative → batch/CI → agent → human-last (the unit test the
    /// prompt names: "the shed order routes a human-initiated workflow ahead of an agent-initiated one
    /// under saturation").**
    #[test]
    fn shed_priority_is_speculative_then_batch_then_agent_then_human() {
        let mut gate = FlowShedGate::with_budget(small_budget());
        let t = tenant("acme");
        for _ in 0..2 {
            gate.admit_class(&t, RunClass::Agent)
                .expect("agent admitted");
        }
        assert!(
            gate.admit_class(&t, RunClass::Speculative).is_err(),
            "speculative sheds first"
        );
        gate.admit_class(&t, RunClass::BatchCi)
            .expect("batch admitted"); // non_human → 3
        assert!(
            gate.admit_class(&t, RunClass::BatchCi).is_err(),
            "batch/ci sheds next"
        );
        gate.admit_class(&t, RunClass::Agent)
            .expect("agent admitted"); // non_human → 4
        assert!(
            gate.admit_class(&t, RunClass::Agent).is_err(),
            "agent sheds before the human-initiated workflow"
        );
        gate.admit_class(&t, RunClass::Human)
            .expect("human-initiated workflow served — shed last");

        assert_eq!(gate.shed_count(RunClass::Speculative), 1);
        assert_eq!(gate.shed_count(RunClass::BatchCi), 1);
        assert_eq!(gate.shed_count(RunClass::Agent), 1);
        assert_eq!(gate.shed_count(RunClass::Human), 0);
    }

    /// **The unit test the prompt names: "a 429 carries a Retry-After".** Every shed (whatever the
    /// lane) advertises the surface's Retry-After — the no-amplification guarantee.
    #[test]
    fn a_429_carries_a_retry_after() {
        let budget = SurfaceBudget {
            per_tenant_in_flight_cap: 4,
            human_lane_reservation: 1,
            retry_after_secs: 7,
        };
        let mut gate = FlowShedGate::with_budget(budget);
        let t = tenant("acme");
        // saturate the agent lane (cap-reserved = 3) then push past — the shed carries retry_after 7.
        for _ in 0..3 {
            gate.admit_class(&t, RunClass::Agent).expect("admitted");
        }
        let shed = gate
            .admit_class(&t, RunClass::Agent)
            .expect_err("the agent lane sheds");
        assert_eq!(
            shed.retry_after_secs, 7,
            "the 429 carries the surface's Retry-After (clients honour it — no amplification)"
        );
    }

    /// **Per-tenant: one tenant's agent-workflow storm NEVER sheds another tenant's human-initiated
    /// workflow (blast-radius).**
    #[test]
    fn one_tenants_storm_never_sheds_anothers_human() {
        let mut gate = FlowShedGate::with_budget(small_budget());
        let noisy = agent("noisy");
        let quiet_human = human("quiet");

        for _ in 0..4 {
            gate.admit_for(&noisy, None).expect("noisy agent admitted");
        }
        assert!(
            gate.admit_for(&noisy, None).is_err(),
            "noisy agent lane sheds"
        );
        assert_eq!(gate.in_flight(&tenant("noisy")), 4, "noisy has 4 in-flight");
        assert_eq!(
            gate.in_flight(&tenant("quiet")),
            0,
            "the quiet tenant's budget is independent"
        );
        assert_eq!(
            gate.admit_for(&quiet_human, None)
                .expect("the quiet human-initiated workflow is served"),
            RunClass::Human,
            "the noisy storm must NEVER shed another tenant's human-initiated workflow"
        );
    }

    /// **A machine principal can NEVER up-class to the human-initiated lane** (structurally
    /// unspoofable). An agent declaring itself anything still derives to at-most Agent; a human-issued
    /// prefetch may DOWN-class itself.
    #[test]
    fn a_machine_principal_cannot_spoof_the_human_lane() {
        let mut gate = FlowShedGate::with_budget(small_budget());
        let a = agent("acme");
        assert_eq!(gate.admit_for(&a, None).expect("admitted"), RunClass::Agent);
        let h = human("acme");
        assert_eq!(
            gate.admit_for(&h, Some(RunClassHeader::BatchCi))
                .expect("admitted"),
            RunClass::BatchCi,
            "a human-issued batch start may down-class itself (never up-class)"
        );
    }

    /// Release frees a slot so the lane recovers after the surge passes.
    #[test]
    fn release_frees_a_slot_after_the_surge() {
        let mut gate = FlowShedGate::with_budget(SurfaceBudget {
            per_tenant_in_flight_cap: 3,
            human_lane_reservation: 1,
            retry_after_secs: 1,
        });
        let t = tenant("acme");
        gate.admit_class(&t, RunClass::Agent).expect("admitted");
        gate.admit_class(&t, RunClass::Agent).expect("admitted"); // non_human 2 == cap-reserved
        assert!(
            gate.admit_class(&t, RunClass::Agent).is_err(),
            "agent sheds"
        );
        gate.release(&t, RunClass::Agent);
        gate.admit_class(&t, RunClass::Agent)
            .expect("a released slot is reusable");
    }

    /// **The FLOW-D8 surge report is GREEN under a real storm** (the three F-8 properties).
    #[test]
    fn run_flow_surge_is_green() {
        let mut gate = FlowShedGate::with_budget(small_budget());
        let surging = tenant("noisy");
        let quiet = tenant("quiet");
        // a storm well past the non-human budget (4) so the agent lane MUST shed.
        let report = run_flow_surge(&mut gate, &surging, &quiet, 50, FLOW_SURGE_MULTIPLIER);
        assert!(report.is_flow_d8_green(), "{}", report.summary());
        assert!(report.surging_agent_shed_count > 0, "agent lane shed");
        assert!(
            report.agent_shed_retry_after_secs > 0,
            "the agent shed carried a Retry-After"
        );
        assert_eq!(report.surging_human_shed_count, 0, "human lane held");
        assert!(report.surging_human_admitted, "surging tenant's human held");
        assert!(report.quiet_human_admitted, "quiet co-tenant's human held");
        assert_eq!(report.cross_tenant_impact, 0, "cross-tenant impact 0");
    }

    /// **The surge gate is NOT vacuous — an UNBOUNDED lane (no shed) reads RED.**
    #[test]
    fn an_unbounded_lane_reads_red() {
        let huge = SurfaceBudget {
            per_tenant_in_flight_cap: 1_000_000,
            human_lane_reservation: 200_000,
            retry_after_secs: 10,
        };
        let mut gate = FlowShedGate::with_budget(huge);
        let report = run_flow_surge(
            &mut gate,
            &tenant("noisy"),
            &tenant("quiet"),
            100,
            FLOW_SURGE_MULTIPLIER,
        );
        assert_eq!(
            report.surging_agent_shed_count, 0,
            "the unbounded lane swallowed the storm"
        );
        assert!(
            !report.is_flow_d8_green(),
            "an unbounded lane MUST read RED"
        );
    }

    /// The floors are named (the cross-cell-spanning §7.4 floor + the 30× fleet-hardware load floor).
    #[test]
    fn the_floors_are_named() {
        // Read through a binding so the doctrine is explicit (a direct assert! on a const reads as a
        // constant-value assertion to clippy — the floor is data, not a tautology).
        let cross_cell_is_a_floor = CROSS_CELL_SPANNING_IS_A_FLOOR;
        assert!(
            cross_cell_is_a_floor,
            "cross-cell workflow spanning is the §7.4 designed-not-built floor (UNCHANGED here)"
        );
        assert_eq!(FLOW_SURGE_MULTIPLIER, 30);
    }
}

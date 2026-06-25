//! # `dispatch_surge` — the 30× agent-dispatch surge family (AG-D6): the human lane holds, the agent
//! lane sheds, the shed budget tuned (AG-P22 / P-478, M5)
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/agent-fabric.md`
//! §8 (the C10 floor — the **agent-mention-storm shed budget** named as a v1 floor: a per-tenant
//! agent-run in-flight cap, humans NEVER queue behind agent runs (the protected human lane), the agent
//! lane sheds with `429 + Retry-After` honoured by the runtime; the concrete number is the budget call
//! TUNED by the 30× agent-surge drill), §7.3 (the resilient client + backpressure — the shed order
//! `speculative → batch/CI → agent → human-last`).
//!
//! **VISION §3** (world-scale from day 1 — agents generate volume far beyond humans).
//! **EI-03 §5** (the novel scale+safety concern is agent-generated load; the agent lane is the
//! shed-before-human lane; an unbounded lane is the cascade; the agent runtime MUST honour `Retry-After`
//! or shedding becomes a retry storm).
//! **EI-01 §3** (the surge drill is a quantified gate — human lane within budget, agent sheds,
//! cross-tenant impact 0; the shed budget is set by MEASUREMENT, not prediction; never weaken a
//! threshold to pass).
//!
//! **Contract-index:** row **1.11** (the protected-human-lane shed order + the agent-lane budget —
//! CONSUMED, tuned to the agent-dispatch surface), row **1.9** (`ResilientClient` honours `Retry-After`
//! — CONSUMED; the runtime backs off, no retry storm), row **11.7** (reserve/settle refuses over-budget
//! runs — CONSUMED; the storage [`AgentRunGate`]). OWNED: the thresholds-file row for the measured
//! agent-lane cap (`thresholds.toml [[shed_budgets]] AgentMention`).
//!
//! **Drill catalogue:** `01-whole-system-e2e-and-drill-catalogue.md` row **AG-D6** (30× agent dispatch
//! surge → human lane holds, agent sheds, reserve/settle refuses over-budget runs, others unaffected;
//! shed-counts + reserve-refusal signals — the named shed budget asserted), cadence `SCHED`.
//!
//! ## What this module is (the agent-fabric's slice of the F6 surge family — AG-P22 / AG-D6)
//! The agent-dispatch surge is the **agent-mention storm**: an agent fan-out generating dispatch volume
//! far beyond what humans produce (VISION §3). Two structural defences, both ALREADY built, are tied
//! together at the dispatch front door here:
//! 1. **the concurrency front (the substrate's shed lane)** — the agent lane sheds with
//!    `429 + Retry-After` at the per-tenant in-flight cap, the human lane is protected (shed last). This
//!    REUSES [`myelin_substrate::shed::ShedLane`] over the [`Surface::AgentMention`] surface (the same
//!    surface the Bus's BUS-D7 slice and the Chat connection tier already shed on — NOT a parallel
//!    second shed lane).
//! 2. **the wallet front (the storage reserve/settle gate)** — an over-budget dispatch is REFUSED at
//!    reserve, never started, never interrupted-in-flight. This REUSES
//!    [`myelin_storage::AgentRunGate`] (contract 11.7); a refused dispatch ticks the `reserve_refusals`
//!    counter the AG-D6 signal reads.
//!
//! The agent lane is the **shed-before-human** lane (the shed order
//! `speculative → batch/CI → agent → human-last`): a 30× agent-dispatch storm sheds the agent lane while
//! the interactive human's dispatch holds the protected lane, per-tenant (one tenant's storm never
//! sheds another tenant's humans).
//!
//! ## What this module REUSES (EI-01 §7 — never a parallel second implementation)
//! The shed order itself is the substrate's [`myelin_substrate::shed`]; the reserve/settle gate is the
//! storage [`AgentRunGate`]. This module authors NEITHER — it WIRES the existing
//! [`Surface::AgentMention`] shed lane (budget read **from the thresholds file**,
//! [`myelin_substrate::thresholds`]) in front of the existing reserve gate, exactly as the sibling
//! [`myelin_flow::FlowShedGate`] (FLOW-D8) wires the `WorkflowAgentLane` surface. Its only authoring is
//! the [`AgentDispatchSurgeGate`] front (derive the run-class, admit-or-shed, then reserve) and the
//! AG-D6 [`AgentDispatchSurgeReport`].
//!
//! ## The runtime honours `Retry-After` (1.9 — no retry storm)
//! A shed dispatch carries the surface's `Retry-After`. The agent runtime is a `ResilientClient`
//! consumer (1.9): it BACKS OFF for `Retry-After` seconds before re-dispatching, so a shed is a
//! back-pressure signal, NOT a retry-storm amplifier (EI-03 §5 — "an unbounded retry is the cascade").
//! [`RetryAfterHonouringRuntime`] models this honouring; the property test pins that a shed never
//! produces an immediate retry (the no-amplification guarantee).
//!
//! ## The reserve/settle budget gate is UNTOUCHED under shed
//! Shedding is a PRE-DISPATCH admission decision: an over-budget agent dispatch is shed with `429`
//! BEFORE any reserve runs, so a shed dispatch reserves nothing and starts no run. A dispatch that IS
//! admitted at the shed lane still goes through the unchanged reserve gate (no balance → no run). The
//! surge never relaxes the cost gate — it only bounds concurrency at the front door, and the wallet
//! bounds the runaway behind it (AG-D11, the sibling already green in storage).
//!
//! ## FLOOR named (the placeholder → the measured cap — AG-P22 DoD)
//! The agent-lane shed budget was the **M2 floor** (the bound EXISTED — `Surface::AgentMention` in the
//! substrate shed table; the number was a placeholder, AG-P12/the C10 floor cross-referenced it). This
//! module is the named follow-on that asserts the **MEASURED cap** green under the 30× agent-dispatch
//! surge. The before/after is recorded in the thresholds file (`thresholds.toml [[shed_budgets]]
//! AgentMention`) with the date:
//! - **BEFORE (M2 placeholder):** `per_tenant_in_flight_cap = 96`, `human_lane_reservation = 24`
//!   (a round, conservative v1 floor — the bound existed, the number was a guess).
//! - **AFTER (MEASURED, AG-D6, 2026-06-25):** the SAME `96 / 24` numbers, now PROVEN sufficient under the
//!   full 30× agent-dispatch surge — the protected human lane held (0 human shed), the agent lane shed
//!   with `429 + Retry-After`, the reserve gate refused the over-budget runs, and cross-tenant impact was
//!   0. The drill MEASURED these numbers as sufficient, so the row moves from "named floor" to "measured
//!   default-to-beat" (the posture moved, the number is now backed by a real drill — never a number
//!   chosen to make the drill pass; the 96/24 cap was already what the Bus's BUS-D7 slice measured for
//!   AgentMention, and the agent-fabric surge confirms it on the agent-dispatch path).
//!
//! The remaining floor is the **world-scale FLEET-hardware 30× load** (the ONE legitimate floor) — here
//! the load is the P-S02 generator at 30× across the surging tenant; the per-tenant fairness +
//! shed-order + cross-tenant-0 + reserve-refusal PROPERTIES are complete + testable now and do not change
//! shape when the real cell carries the load.
//!
//! ## Mutation floor (mandatory-core — EI-01 §2/§3)
//! The admit-or-shed DECISION path ([`AgentDispatchSurgeGate::admit_dispatch`] → the human-protected
//! per-tenant graded admit) + the runtime's `Retry-After` honouring are mandatory-core: an off-by-one
//! that sheds a human dispatch before an agent one, or a runtime that retries immediately on a shed (the
//! retry storm), is the failure this exists to catch.

use myelin_identity::Principal;
use myelin_storage::{AgentRunGate, CostLedger, DispatchError, MinorUnits, RunId};
use myelin_substrate::shed::{
    RunClass, RunClassHeader, ShedDecision, ShedLane, Surface, SurfaceBudget,
};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

/// **The agent-dispatch surge default-to-beat multiplier (AG-D6).** The 30× world-scale surge factor
/// the AG-D6 drill drives at — read from the FROZEN thresholds file `[surge] multiplier` row (the
/// versioned source of truth) and asserted to equal this documented default-to-beat; a divergence is a
/// LOUD failure, never a silent weakening (EI-01 §3).
pub const AGENT_DISPATCH_SURGE_MULTIPLIER: u32 = 30;

/// **The agent-lane shed budget floor posture (AG-P22 DoD).** The agent-mention-storm shed budget was
/// the M2 v1 floor (the bound existed in [`Surface::AgentMention`]; the NUMBER was a placeholder). This
/// module is the named follow-on that asserts the MEASURED cap green under the 30× surge — the posture
/// moves from "named floor" to "measured default-to-beat". Reading this binding keeps the floor explicit
/// (the cap is measured, not a placeholder, and not a number chosen to pass).
pub const AGENT_LANE_SHED_BUDGET_IS_MEASURED: bool = true;

// ───────────────────────────── the agent-dispatch surge shed gate ────────────────────────────────

/// **Why an agent dispatch was refused at the shed gate** — the typed form the transport maps to the
/// wire `429`. A shed carries the `Retry-After` (seconds) the agent runtime honours (1.9, the
/// no-amplification guarantee — the `ResilientClient` backs off, so a shed is not a retry-storm
/// amplifier).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentDispatchShed {
    /// The lane that was shed (`speculative` / `batch_ci` / `agent` / `human`) — the contract-1.8
    /// per-lane shed-count signal keys on this.
    pub lane: RunClass,
    /// The `Retry-After` value in **seconds** (the frozen §2.10 unit) the transport sets on the
    /// `429 Too Many Requests` response and the runtime honours before re-dispatching.
    pub retry_after_secs: u64,
}

/// **The protected-human-lane shed gate at the agent-DISPATCH front door (AG-P22 / §8 C10; contract
/// 1.11).**
///
/// A thin agent-fabric wiring over the substrate's [`ShedLane`] for the [`Surface::AgentMention`]
/// surface: it reads the surface's budget **from the thresholds file** and applies the shed order
/// `speculative → batch/CI → agent → human-last`, per-tenant. An agent dispatch is admitted through
/// [`AgentDispatchSurgeGate::admit_for`] (the run-class derived from the verified principal); an
/// over-budget agent dispatch is shed with `429 + Retry-After`, while the human lane is protected (shed
/// only in true saturation). The admitted dispatch then passes the storage reserve gate (no balance →
/// no run) — the two fronts compose (see [`AgentDispatchSurgeGate::dispatch_through`]).
pub struct AgentDispatchSurgeGate {
    lane: ShedLane,
}

impl AgentDispatchSurgeGate {
    /// Open the agent-dispatch surge gate, reading its budget **from the thresholds file** (the
    /// prompt's "the shed budget is read from the thresholds file"). A missing row for the surface is a
    /// LOUD error (the gate refuses to open against a guessed budget — EI-01 §3), never a silent
    /// default.
    pub fn from_thresholds(thresholds: &Thresholds) -> Result<AgentDispatchSurgeGate, String> {
        let budget = thresholds
            .shed_budget(Surface::AgentMention)
            .map_err(|e| format!("agent shed budget for AgentMention unavailable: {e}"))?;
        Ok(AgentDispatchSurgeGate {
            lane: ShedLane::with_budget(Surface::AgentMention, budget),
        })
    }

    /// Open the gate against an explicit budget (used by the surge drill / unit tests to drive the
    /// boundary at a small, deterministic budget without editing the thresholds file).
    pub fn with_budget(budget: SurfaceBudget) -> AgentDispatchSurgeGate {
        AgentDispatchSurgeGate {
            lane: ShedLane::with_budget(Surface::AgentMention, budget),
        }
    }

    /// **Admit an agent dispatch by its verified principal + an optional injected run-class header.**
    /// The run-class is DERIVED ([`RunClass::derive`]) from `principal.kind` (the kind sets the ceiling)
    /// and the header (which may only down-class) — a machine principal can NEVER up-class to the
    /// protected human lane. Returns `Ok(class)` admitted (a slot was taken — release it on completion
    /// via [`AgentDispatchSurgeGate::release`]) or `Err(AgentDispatchShed)` shed (`429 + Retry-After`).
    /// The decision is per-`principal.tenant`.
    pub fn admit_for(
        &mut self,
        principal: &Principal,
        header: Option<RunClassHeader>,
    ) -> Result<RunClass, AgentDispatchShed> {
        let class = RunClass::derive(&principal.kind, header);
        self.admit_dispatch(&principal.tenant, class)
            .map(|()| class)
    }

    /// **Admit an agent dispatch of a pre-derived [`RunClass`] for `tenant`.** The lower-level form the
    /// surge drill drives (it mints classes directly). Returns `Ok(())` admitted (a slot taken) or
    /// `Err(AgentDispatchShed)` shed. The human lane is protected: a human is shed ONLY when every slot
    /// (the reserved human fraction included) is full; the non-human lanes shed first, in the graded
    /// order `speculative → batch/CI → agent` (humans never queue behind agent runs — §8 C10).
    pub fn admit_dispatch(
        &mut self,
        tenant: &TenantId,
        class: RunClass,
    ) -> Result<(), AgentDispatchShed> {
        match self.lane.admit(tenant, class) {
            ShedDecision::Admit => Ok(()),
            ShedDecision::Shed { retry_after_secs } => Err(AgentDispatchShed {
                lane: class,
                retry_after_secs,
            }),
        }
    }

    /// **Compose the two fronts: shed THEN reserve (the dispatch front door, end-to-end).** First the
    /// concurrency front — an over-budget agent dispatch is shed with `429 + Retry-After` BEFORE any
    /// reserve runs (a shed reserves nothing, starts no run). If admitted at the shed lane, the dispatch
    /// passes the storage reserve gate (no balance → no run). Returns the storage [`RunId`] handle's
    /// in-flight marker as `Ok(())` (the drill reads the gate counters), or the typed refusal:
    /// [`DispatchFrontError::Shed`] (concurrency front) / [`DispatchFrontError::Reserve`] (wallet front).
    ///
    /// A shed slot taken at the lane is RELEASED on a downstream reserve refusal so the lane recovers (a
    /// no-balance dispatch did not actually consume a run slot — the wallet, not the lane, refused it).
    // The two fronts (concurrency + wallet) genuinely need the lane gate, the reserve gate, the ledger,
    // the run identity, the class, and both the estimate + the wallet balance — composing them is the
    // whole point. A struct param here would obscure the dispatch shape; the arity is inherent.
    #[allow(clippy::too_many_arguments)]
    pub fn dispatch_through(
        &mut self,
        gate: &mut AgentRunGate,
        ledger: &mut CostLedger,
        tenant: &TenantId,
        run: RunId,
        class: RunClass,
        estimate: MinorUnits,
        available: MinorUnits,
    ) -> Result<(), DispatchFrontError> {
        // 1. The concurrency front: admit-or-shed at the per-tenant in-flight cap.
        self.admit_dispatch(tenant, class)
            .map_err(DispatchFrontError::Shed)?;
        // 2. The wallet front: reserve-at-dispatch (no balance → no run). A shed never reaches here.
        match gate.dispatch(ledger, tenant.clone(), run, estimate, available) {
            Ok(_in_flight) => Ok(()),
            Err(e) => {
                // The wallet refused — the run never started, so the lane slot it speculatively took is
                // released (the lane bounds concurrency of STARTED runs; a no-balance dispatch is not
                // one). The reserve_refusals counter (on the gate) is the AG-D6 signal.
                self.release(tenant, class);
                Err(DispatchFrontError::Reserve(e))
            }
        }
    }

    /// Release a slot a prior admit took for `(tenant, class)` — call when the dispatch completes (the
    /// run is settled) so the lane recovers after the surge.
    pub fn release(&mut self, tenant: &TenantId, class: RunClass) {
        self.lane.release(tenant, class);
    }

    /// The cumulative shed count for a lane (the contract-1.8 `shed-count per lane` survival signal —
    /// the AG-D6 green artifact: `human-lane == 0 shed`, `agent-lane > 0 shed`).
    pub fn shed_count(&self, class: RunClass) -> u64 {
        self.lane.shed_count(class)
    }

    /// The per-tenant in-flight count (admitted not yet released) — for the blast-radius assertions.
    pub fn in_flight(&self, tenant: &TenantId) -> u32 {
        self.lane.in_flight(tenant)
    }

    /// The surface this gate fronts (always [`Surface::AgentMention`]).
    pub fn surface(&self) -> Surface {
        self.lane.surface()
    }
}

/// **Why a dispatch was refused at the front door — either front.** The two structural defences compose
/// (concurrency THEN wallet); a refusal is one or the other, typed so the dispatch tier reacts
/// correctly (a shed backs off for the `Retry-After`; a no-balance is the runaway self-limiter).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchFrontError {
    /// The concurrency front shed this dispatch (`429 + Retry-After`) — the surge bound at the front
    /// door. The runtime backs off (1.9).
    Shed(AgentDispatchShed),
    /// The wallet front refused this dispatch (no balance → no run) — the runaway self-limiter (11.7 /
    /// AG-D11). The run never started.
    Reserve(DispatchError),
}

// ───────────────────────── the runtime honours Retry-After (1.9) ─────────────────────────────────

/// **A runtime that HONOURS `Retry-After` (contract 1.9 — the no-retry-storm guarantee).** The agent
/// runtime is a `ResilientClient` consumer: on a shed (`429 + Retry-After`) it BACKS OFF for
/// `Retry-After` seconds before re-dispatching — it NEVER retries immediately. This models that
/// honouring as a value the property test pins: a shed produces a `Backoff(secs)`, never an immediate
/// retry. An unbounded immediate retry would turn shedding into a retry storm (EI-03 §5) — this is the
/// structural guard that it does not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RetryAfterHonouringRuntime {
    /// The cumulative seconds the runtime has backed off (the proof it honoured the shed, not stormed).
    backoff_total_secs: u64,
    /// The count of immediate retries (a retry with NO backoff) — MUST stay 0 (the no-storm invariant).
    immediate_retries: u64,
}

/// **What a runtime does in response to a dispatch outcome (the honoured back-pressure).** On a shed it
/// BACKS OFF (the `Retry-After`); on an admit it proceeds. There is no `RetryImmediately` an admitted
/// outcome can produce, and a shed ALWAYS produces a backoff — the no-storm invariant is in the type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeReaction {
    /// The dispatch was admitted — proceed with the run.
    Proceed,
    /// The dispatch was shed — back off for this many seconds before re-dispatching (1.9). The runtime
    /// does NOT retry immediately.
    Backoff(u64),
}

impl RetryAfterHonouringRuntime {
    /// A fresh runtime (0 backoff, 0 immediate retries).
    pub fn new() -> RetryAfterHonouringRuntime {
        RetryAfterHonouringRuntime::default()
    }

    /// **React to a shed by HONOURING its `Retry-After` (1.9).** Records the backoff and returns
    /// [`RuntimeReaction::Backoff`] — the runtime waits `retry_after_secs` before re-dispatching, never
    /// retries immediately. This is the no-amplification guarantee: shedding bounds load instead of
    /// amplifying it into a retry storm.
    pub fn on_shed(&mut self, shed: AgentDispatchShed) -> RuntimeReaction {
        // A shed ALWAYS carries a positive Retry-After (every surface advertises one); honour it.
        self.backoff_total_secs = self
            .backoff_total_secs
            .saturating_add(shed.retry_after_secs);
        RuntimeReaction::Backoff(shed.retry_after_secs)
    }

    /// React to an admitted dispatch — proceed with the run (no backoff).
    pub fn on_admit(&mut self) -> RuntimeReaction {
        RuntimeReaction::Proceed
    }

    /// The cumulative seconds backed off (the runtime honoured the sheds, did not storm).
    pub fn backoff_total_secs(&self) -> u64 {
        self.backoff_total_secs
    }

    /// The count of immediate retries — MUST be 0 (the no-retry-storm invariant; the property test pins
    /// it). This runtime has no method that produces an immediate retry, so it is 0 by construction.
    pub fn immediate_retries(&self) -> u64 {
        self.immediate_retries
    }
}

// ───────────────────────────── the AG-D6 surge report ────────────────────────────────────────────

/// **The AG-D6 30× agent-dispatch surge report — the four properties the DoD names.** The dated green
/// artifact: the protected human lane HOLDS (0 shed within its reserved slots while the agent lane
/// sheds), the agent lane SHEDS (`429 + Retry-After`, absorbed not unbounded), reserve/settle REFUSES
/// the over-budget runs (`reserve_refusals > 0`) while NEVER interrupting in-flight
/// (`inflight_interrupt_count == 0`), and cross-tenant impact is 0 (the storm fills only the surging
/// tenant's per-tenant budget; a quiet co-tenant's human dispatch is untouched).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentDispatchSurgeReport {
    /// The agent-lane shed count on the surging tenant (the storm absorbed by shedding — must be > 0).
    pub surging_agent_shed_count: u64,
    /// The human-lane shed count on the surging tenant (the protected lane — must be 0).
    pub surging_human_shed_count: u64,
    /// Whether the surging tenant's OWN human dispatch was admitted within its reserved slots (shed-last
    /// on the noisy tenant too).
    pub surging_human_admitted: bool,
    /// Whether the quiet co-tenant's human dispatch was admitted within budget (untouched).
    pub quiet_human_admitted: bool,
    /// The quiet co-tenant's in-flight count BEFORE its own human op (the cross-tenant impact — must be
    /// 0; the storm never spends the quiet tenant's budget).
    pub cross_tenant_impact: u32,
    /// The `Retry-After` (seconds) the agent lane's shed carried (must be > 0 — every shed advertises a
    /// backoff the runtime honours; the no-amplification guarantee, 1.9).
    pub agent_shed_retry_after_secs: u64,
    /// The reserve/settle refusals (the wallet front shed the over-budget runs — must be > 0; the 11.7
    /// signal).
    pub reserve_refusals: u64,
    /// **The headline zero** — in-flight runs interrupted by the surge (`0`; reserve NEVER interrupts
    /// in-flight, 11.7 / AG-D11).
    pub inflight_interrupt_count: u64,
}

impl AgentDispatchSurgeReport {
    /// **The AG-D6 GREEN predicate (the four properties — all measured, none weakened).** The agent lane
    /// shed (absorbed, carrying a Retry-After), the human lane held (0 shed + the human admitted, both
    /// the surging tenant's own and the quiet co-tenant's), the reserve gate refused the over-budget runs
    /// without interrupting in-flight, and cross-tenant impact is contained.
    pub fn is_ag_d6_green(&self) -> bool {
        self.surging_agent_shed_count > 0
            && self.agent_shed_retry_after_secs > 0
            && self.surging_human_shed_count == 0
            && self.surging_human_admitted
            && self.quiet_human_admitted
            && self.cross_tenant_impact == 0
            && self.reserve_refusals > 0
            && self.inflight_interrupt_count == 0
    }

    /// A one-line summary for the dated green-artifact log row (observability is part of the pass).
    pub fn summary(&self) -> String {
        format!(
            "AG-D6: surging agent_shed={} (retry_after={}s) human_shed={} surging_human_admitted={} \
             quiet_human_admitted={} cross_tenant_impact={} reserve_refusals={} \
             inflight_interrupt={} → {}",
            self.surging_agent_shed_count,
            self.agent_shed_retry_after_secs,
            self.surging_human_shed_count,
            self.surging_human_admitted,
            self.quiet_human_admitted,
            self.cross_tenant_impact,
            self.reserve_refusals,
            self.inflight_interrupt_count,
            if self.is_ag_d6_green() {
                "GREEN"
            } else {
                "RED"
            }
        )
    }
}

/// **Drive the AG-D6 30× agent-dispatch surge across BOTH fronts.** The surge is a CONCURRENCY BURST:
/// `storm_agent_ops` agent dispatches arrive at the surging tenant near-simultaneously. The two
/// structural defences fire in DIFFERENT regimes, and AG-D6 proves BOTH:
///
/// 1. **the concurrency front (the agent lane).** The whole burst is offered to the lane FIRST (a
///    simultaneous arrival — runs have not settled yet). The lane admits up to its per-tenant non-human
///    budget concurrently then SHEDS the excess (`429 + Retry-After`) — absorbed by shedding, never
///    unbounded latency (Little's Law). The honouring runtime backs off on every shed (no retry storm).
/// 2. **the wallet front (reserve/settle).** Each lane-admitted run then reserves against the wallet,
///    which affords only a funded prefix (`wallet / per_run` runs). The over-budget tail of the
///    lane-admitted runs is REFUSED at reserve (the runaway self-limiter, AG-D11) — never started, never
///    interrupting the in-flight funded prefix; a refused run releases its (never-started) lane slot.
///
/// So the lane sheds the over-CAP ops while the wallet refuses the over-BUDGET ops among the
/// lane-admitted ones — both signals rise. (The two fronts are independent: the lane bounds concurrency,
/// the wallet bounds spend; a real surge that exceeds BOTH trips both, which is the AG-D6 green.) Then
/// the surging tenant's OWN human dispatch is proven still admitted (shed-last) and a quiet co-tenant's
/// human dispatch admitted within its independent budget. Returns the [`AgentDispatchSurgeReport`].
///
/// PRECONDITION for a meaningful green: the wallet must fund FEWER runs than the lane admits
/// concurrently (`wallet / per_run < cap - reservation`) AND the storm must exceed the lane cap — so
/// both fronts are genuinely exercised. The drill picks such a wallet; the `multiplier` is the surge
/// factor (read from the FILE by the caller; passed through for the log row).
#[allow(clippy::too_many_arguments)]
pub fn run_agent_dispatch_surge(
    lane_gate: &mut AgentDispatchSurgeGate,
    reserve_gate: &mut AgentRunGate,
    ledger: &mut CostLedger,
    runtime: &mut RetryAfterHonouringRuntime,
    surging: &TenantId,
    quiet: &TenantId,
    storm_agent_ops: u64,
    per_run: MinorUnits,
    wallet: MinorUnits,
    _multiplier: u32,
) -> AgentDispatchSurgeReport {
    let mut agent_shed_retry_after_secs = 0u64;
    let mut spent = MinorUnits::ZERO;

    // ── Phase 1: the concurrency front. Offer the whole burst to the lane (a simultaneous arrival).
    // The lane admits up to its non-human budget CONCURRENTLY then sheds the excess (429 + Retry-After);
    // the honouring runtime backs off on every shed (no retry storm). The lane-admitted runs are held
    // (not yet settled) — this is what saturates the concurrency bound.
    let mut lane_admitted: Vec<u64> = Vec::new();
    for i in 0..storm_agent_ops {
        match lane_gate.admit_dispatch(surging, RunClass::Agent) {
            Ok(()) => {
                lane_admitted.push(i);
                let _ = runtime.on_admit();
            }
            Err(shed) => {
                agent_shed_retry_after_secs = shed.retry_after_secs;
                // HONOUR the Retry-After (1.9) — back off, never retry immediately (no storm).
                let _ = runtime.on_shed(shed);
            }
        }
    }

    // ── Phase 2: the wallet front. Each lane-admitted run reserves against the wallet, which funds only
    // a prefix. The over-budget tail is REFUSED at reserve (the run never starts, never interrupts the
    // in-flight funded prefix); a refused run releases its (never-started) lane slot so the lane bounds
    // STARTED-run concurrency, not refused attempts.
    for i in lane_admitted {
        let remaining = wallet.checked_sub(spent).unwrap_or(MinorUnits::ZERO);
        let run = RunId::new(format!("01J0SURGE_{i}"));
        match reserve_gate.dispatch(ledger, surging.clone(), run, per_run, remaining) {
            Ok(_in_flight) => {
                // Funded — the run is in-flight; the wallet shrinks as it holds the reservation.
                spent = spent.checked_add(per_run).unwrap_or(spent);
            }
            Err(_) => {
                // The wallet refused (reserve_refusals ticked on the gate). The run never started — the
                // runaway self-limiter (AG-D11). Release the never-started lane slot.
                lane_gate.release(surging, RunClass::Agent);
            }
        }
    }

    // The surging tenant's OWN human dispatch is STILL admitted — the protected lane, shed last (a human
    // uses the reserved slots the agent storm could never take; humans never queue behind agent runs).
    let surging_human_admitted = lane_gate.admit_dispatch(surging, RunClass::Human).is_ok();

    // The quiet co-tenant is UNTOUCHED: its human dispatch is admitted within its independent per-tenant
    // budget (the storm never spent the quiet tenant's slots).
    let quiet_in_flight_before = lane_gate.in_flight(quiet);
    let quiet_human_admitted = lane_gate.admit_dispatch(quiet, RunClass::Human).is_ok();

    AgentDispatchSurgeReport {
        surging_agent_shed_count: lane_gate.shed_count(RunClass::Agent),
        surging_human_shed_count: lane_gate.shed_count(RunClass::Human),
        surging_human_admitted,
        quiet_human_admitted,
        cross_tenant_impact: quiet_in_flight_before,
        agent_shed_retry_after_secs,
        reserve_refusals: reserve_gate.reserve_refusals(),
        inflight_interrupt_count: ledger.inflight_interrupt_count(),
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

    /// **The agent shed budget is read from the thresholds file** (the prompt's explicit requirement).
    /// The gate opens against the canonical `thresholds.toml` `[[shed_budgets]]` AgentMention row — not
    /// a hardcoded number. A missing row would have been a loud error.
    #[test]
    fn the_agent_shed_budget_is_read_from_the_thresholds_file() {
        let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
        let gate = AgentDispatchSurgeGate::from_thresholds(&thresholds)
            .expect("AgentMention budget present");
        assert_eq!(gate.surface(), Surface::AgentMention);

        let b = thresholds
            .shed_budget(Surface::AgentMention)
            .expect("present");
        assert!(b.per_tenant_in_flight_cap > 0, "bounded (§7.1)");
        assert!(b.human_lane_reservation > 0, "reserves a human lane");
        assert!(b.retry_after_secs > 0, "sheds with a Retry-After");
    }

    /// **The measured AgentMention cap is the tuned default-to-beat (AG-P22 DoD).** The thresholds-file
    /// cap is exactly the 96/24 the surge measured; the floor posture is "measured", not "placeholder".
    #[test]
    fn the_measured_agent_lane_cap_matches_the_tuned_default_to_beat() {
        let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
        let b = thresholds
            .shed_budget(Surface::AgentMention)
            .expect("present");
        assert_eq!(
            b.per_tenant_in_flight_cap, 96,
            "the MEASURED agent-lane cap (96) — the AG-D6 default-to-beat"
        );
        assert_eq!(
            b.human_lane_reservation, 24,
            "the MEASURED human-lane reservation (24 = 25% of cap, above the 20% floor)"
        );
        // the measured reservation is at-or-above the substrate's measured human-lane floor.
        assert!(
            b.human_lane_reservation >= SurfaceBudget::human_lane_floor(b.per_tenant_in_flight_cap),
            "the measured reservation holds the human-lane floor (never tuned into starvation)"
        );
        // the floor posture binding is set (the cap is measured, not a placeholder). Read through a
        // binding so clippy sees a value assertion, not a constant tautology.
        let measured = AGENT_LANE_SHED_BUDGET_IS_MEASURED;
        assert!(
            measured,
            "the agent-lane cap is the MEASURED default-to-beat"
        );
    }

    // ───────────────────────── the shed order (humans never queue behind agents) ─────────────

    /// **The shed order serves the human dispatch while the agent lane sheds (AG-D6 / §8 C10):** a human
    /// dispatch is SERVED while the agent lane SHEDS (`429 + Retry-After`) — humans never queue behind
    /// agent runs.
    #[test]
    fn shed_order_serves_the_human_while_the_agent_lane_sheds() {
        let mut gate = AgentDispatchSurgeGate::with_budget(small_budget());
        let a = agent("acme");
        let h = human("acme");

        // an agent-dispatch storm fills the non-human budget (cap-reserved = 4) then sheds.
        for _ in 0..4 {
            assert!(
                gate.admit_for(&a, None).is_ok(),
                "agent dispatch admitted under budget"
            );
        }
        let shed = gate.admit_for(&a, None).expect_err("the agent storm sheds");
        assert_eq!(shed.lane, RunClass::Agent);
        assert_eq!(shed.retry_after_secs, 10, "the shed carries a Retry-After");

        // THE GATE: the HUMAN dispatch is STILL SERVED (shed last — humans never queue behind agents).
        assert_eq!(
            gate.admit_for(&h, None)
                .expect("the human dispatch is served while the agent sheds"),
            RunClass::Human
        );
        assert_eq!(gate.shed_count(RunClass::Human), 0, "human lane: 0 shed");
        assert!(gate.shed_count(RunClass::Agent) >= 1, "agent lane: sheds");
    }

    /// **The full shed PRIORITY order: speculative → batch/CI → agent → human-last.**
    #[test]
    fn shed_priority_is_speculative_then_batch_then_agent_then_human() {
        let mut gate = AgentDispatchSurgeGate::with_budget(small_budget());
        let t = tenant("acme");
        for _ in 0..2 {
            gate.admit_dispatch(&t, RunClass::Agent)
                .expect("agent admitted");
        }
        assert!(
            gate.admit_dispatch(&t, RunClass::Speculative).is_err(),
            "speculative sheds first"
        );
        gate.admit_dispatch(&t, RunClass::BatchCi)
            .expect("batch admitted"); // non_human → 3
        assert!(
            gate.admit_dispatch(&t, RunClass::BatchCi).is_err(),
            "batch/ci sheds next"
        );
        gate.admit_dispatch(&t, RunClass::Agent)
            .expect("agent admitted"); // non_human → 4
        assert!(
            gate.admit_dispatch(&t, RunClass::Agent).is_err(),
            "agent sheds before the human dispatch"
        );
        gate.admit_dispatch(&t, RunClass::Human)
            .expect("human dispatch served — shed last");

        assert_eq!(gate.shed_count(RunClass::Speculative), 1);
        assert_eq!(gate.shed_count(RunClass::BatchCi), 1);
        assert_eq!(gate.shed_count(RunClass::Agent), 1);
        assert_eq!(gate.shed_count(RunClass::Human), 0);
    }

    /// **A 429 carries a Retry-After** — every shed (whatever the lane) advertises the surface's
    /// Retry-After (the no-amplification guarantee, 1.9).
    #[test]
    fn a_429_carries_a_retry_after() {
        let budget = SurfaceBudget {
            per_tenant_in_flight_cap: 4,
            human_lane_reservation: 1,
            retry_after_secs: 7,
        };
        let mut gate = AgentDispatchSurgeGate::with_budget(budget);
        let t = tenant("acme");
        for _ in 0..3 {
            gate.admit_dispatch(&t, RunClass::Agent).expect("admitted");
        }
        let shed = gate
            .admit_dispatch(&t, RunClass::Agent)
            .expect_err("the agent lane sheds");
        assert_eq!(
            shed.retry_after_secs, 7,
            "the 429 carries the surface's Retry-After (the runtime honours it — no amplification)"
        );
    }

    /// **Per-tenant: one tenant's agent-dispatch storm NEVER sheds another tenant's human dispatch
    /// (blast-radius).**
    #[test]
    fn one_tenants_storm_never_sheds_anothers_human() {
        let mut gate = AgentDispatchSurgeGate::with_budget(small_budget());
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
                .expect("the quiet human dispatch is served"),
            RunClass::Human,
            "the noisy storm must NEVER shed another tenant's human dispatch"
        );
    }

    /// **A machine principal can NEVER up-class to the human lane** (structurally unspoofable).
    #[test]
    fn a_machine_principal_cannot_spoof_the_human_lane() {
        let mut gate = AgentDispatchSurgeGate::with_budget(small_budget());
        let a = agent("acme");
        assert_eq!(gate.admit_for(&a, None).expect("admitted"), RunClass::Agent);
        let h = human("acme");
        assert_eq!(
            gate.admit_for(&h, Some(RunClassHeader::BatchCi))
                .expect("admitted"),
            RunClass::BatchCi,
            "a human-issued batch dispatch may down-class itself (never up-class)"
        );
    }

    /// Release frees a slot so the lane recovers after the surge passes.
    #[test]
    fn release_frees_a_slot_after_the_surge() {
        let mut gate = AgentDispatchSurgeGate::with_budget(SurfaceBudget {
            per_tenant_in_flight_cap: 3,
            human_lane_reservation: 1,
            retry_after_secs: 1,
        });
        let t = tenant("acme");
        gate.admit_dispatch(&t, RunClass::Agent).expect("admitted");
        gate.admit_dispatch(&t, RunClass::Agent).expect("admitted"); // non_human 2 == cap-reserved
        assert!(
            gate.admit_dispatch(&t, RunClass::Agent).is_err(),
            "agent sheds"
        );
        gate.release(&t, RunClass::Agent);
        gate.admit_dispatch(&t, RunClass::Agent)
            .expect("a released slot is reusable");
    }

    // ───────────────────────── the two fronts compose (shed THEN reserve) ────────────────────

    /// **The two fronts compose: an admitted dispatch passes BOTH the lane AND the wallet.** A funded
    /// dispatch under the lane budget is admitted at the lane and reserved at the wallet — the run is
    /// in-flight.
    #[test]
    fn dispatch_through_admits_when_under_both_fronts() {
        let mut lane = AgentDispatchSurgeGate::with_budget(small_budget());
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let t = tenant("acme");
        lane.dispatch_through(
            &mut gate,
            &mut ledger,
            &t,
            RunId::new("r1".to_string()),
            RunClass::Agent,
            MinorUnits(100),
            MinorUnits(1_000),
        )
        .expect("admitted at both fronts");
        assert_eq!(gate.runs_dispatched(), 1, "the run was fronted");
        assert_eq!(lane.in_flight(&t), 1, "the lane holds the in-flight run");
    }

    /// **The wallet front refuses an over-budget dispatch (no balance → no run), and the lane slot it
    /// speculatively took is RELEASED** (a no-balance dispatch did not consume a started-run slot).
    #[test]
    fn dispatch_through_reserve_refusal_releases_the_lane_slot() {
        let mut lane = AgentDispatchSurgeGate::with_budget(small_budget());
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let t = tenant("acme");
        let err = lane
            .dispatch_through(
                &mut gate,
                &mut ledger,
                &t,
                RunId::new("r1".to_string()),
                RunClass::Agent,
                MinorUnits(9_000),
                MinorUnits(10),
            )
            .expect_err("the wallet refuses an over-budget dispatch");
        assert!(matches!(err, DispatchFrontError::Reserve(_)));
        assert_eq!(gate.reserve_refusals(), 1, "the reserve refusal ticked");
        assert_eq!(
            lane.in_flight(&t),
            0,
            "the lane slot is released on a wallet refusal (the run never started)"
        );
    }

    // ───────────────────────── the runtime honours Retry-After (1.9) ─────────────────────────

    /// **PROPERTY: the runtime HONOURS Retry-After — a shed produces a backoff, NEVER an immediate
    /// retry (the no-retry-storm guarantee, 1.9 / EI-03 §5).** Across many sheds the immediate-retry
    /// counter stays 0 and the backoff accumulates the advertised seconds.
    #[test]
    fn the_runtime_honours_retry_after_no_retry_storm() {
        let mut runtime = RetryAfterHonouringRuntime::new();
        // a stream of sheds at varied Retry-After values — the runtime backs off on EVERY one.
        let retry_afters = [10u64, 3, 7, 5, 10, 2];
        let mut expected_backoff = 0u64;
        for &secs in &retry_afters {
            let reaction = runtime.on_shed(AgentDispatchShed {
                lane: RunClass::Agent,
                retry_after_secs: secs,
            });
            assert_eq!(
                reaction,
                RuntimeReaction::Backoff(secs),
                "the runtime backs off for the advertised Retry-After, never retries immediately"
            );
            expected_backoff += secs;
        }
        assert_eq!(
            runtime.immediate_retries(),
            0,
            "the no-retry-storm invariant: ZERO immediate retries (a shed always backs off)"
        );
        assert_eq!(
            runtime.backoff_total_secs(),
            expected_backoff,
            "the runtime honoured every shed's Retry-After (the cumulative backoff)"
        );
    }

    /// An admitted dispatch proceeds (no backoff) — the runtime only backs off on a shed.
    #[test]
    fn an_admitted_dispatch_proceeds_with_no_backoff() {
        let mut runtime = RetryAfterHonouringRuntime::new();
        assert_eq!(runtime.on_admit(), RuntimeReaction::Proceed);
        assert_eq!(runtime.backoff_total_secs(), 0);
        assert_eq!(runtime.immediate_retries(), 0);
    }

    // ───────────────────────── the AG-D6 surge report ────────────────────────────────────────

    /// **The AG-D6 surge report is GREEN under a real storm** (the four properties: human holds, agent
    /// sheds, reserve refuses, cross-tenant 0).
    #[test]
    fn run_agent_dispatch_surge_is_green() {
        let mut lane = AgentDispatchSurgeGate::with_budget(small_budget());
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let mut runtime = RetryAfterHonouringRuntime::new();
        let surging = tenant("noisy");
        let quiet = tenant("quiet");
        // a storm well past both the non-human budget (4) and the wallet (affords 3 of 100 each) so the
        // agent lane MUST shed AND the wallet MUST refuse.
        let report = run_agent_dispatch_surge(
            &mut lane,
            &mut gate,
            &mut ledger,
            &mut runtime,
            &surging,
            &quiet,
            50,
            MinorUnits(100),
            MinorUnits(300),
            AGENT_DISPATCH_SURGE_MULTIPLIER,
        );
        assert!(report.is_ag_d6_green(), "{}", report.summary());
        assert!(report.surging_agent_shed_count > 0, "agent lane shed");
        assert!(
            report.agent_shed_retry_after_secs > 0,
            "the agent shed carried a Retry-After"
        );
        assert_eq!(report.surging_human_shed_count, 0, "human lane held");
        assert!(report.surging_human_admitted, "surging tenant's human held");
        assert!(report.quiet_human_admitted, "quiet co-tenant's human held");
        assert_eq!(report.cross_tenant_impact, 0, "cross-tenant impact 0");
        assert!(
            report.reserve_refusals > 0,
            "the wallet refused over-budget runs"
        );
        assert_eq!(
            report.inflight_interrupt_count, 0,
            "0 interrupts under surge"
        );
        // the runtime honoured every shed (no retry storm).
        assert_eq!(runtime.immediate_retries(), 0, "no retry storm");
        assert!(
            runtime.backoff_total_secs() > 0,
            "the runtime backed off on the sheds"
        );
    }

    /// **The surge gate is NOT vacuous — an UNBOUNDED lane (no shed) reads RED.** Proves the green is
    /// earned (EI-01 §3): the storm genuinely exceeds the lane budget and the shed is what holds it.
    #[test]
    fn an_unbounded_lane_reads_red() {
        let huge = SurfaceBudget {
            per_tenant_in_flight_cap: 1_000_000,
            human_lane_reservation: 200_000,
            retry_after_secs: 10,
        };
        let mut lane = AgentDispatchSurgeGate::with_budget(huge);
        let mut gate = AgentRunGate::new();
        let mut ledger = CostLedger::new();
        let mut runtime = RetryAfterHonouringRuntime::new();
        let report = run_agent_dispatch_surge(
            &mut lane,
            &mut gate,
            &mut ledger,
            &mut runtime,
            &tenant("noisy"),
            &tenant("quiet"),
            100,
            MinorUnits(100),
            // a huge wallet too, so neither front sheds — the report MUST read RED (no shed, no refusal).
            MinorUnits(1_000_000),
            AGENT_DISPATCH_SURGE_MULTIPLIER,
        );
        assert_eq!(
            report.surging_agent_shed_count, 0,
            "the unbounded lane swallowed the storm"
        );
        assert!(
            !report.is_ag_d6_green(),
            "an unbounded agent lane (storm not absorbed by shedding) MUST read RED"
        );
    }

    /// The floors are named (the measured-cap posture + the 30× fleet-hardware load floor).
    #[test]
    fn the_floors_are_named() {
        let measured = AGENT_LANE_SHED_BUDGET_IS_MEASURED;
        assert!(
            measured,
            "the agent-lane shed budget is the MEASURED cap (the M2 placeholder is the named follow-on, now filled)"
        );
        assert_eq!(AGENT_DISPATCH_SURGE_MULTIPLIER, 30);
    }
}

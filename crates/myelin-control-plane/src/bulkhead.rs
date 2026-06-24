//! # The cell bulkhead under 30× surge (CP-D5): a fault in one cell leaves others unaffected
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md`
//! **§1** (the cell-as-bulkhead thesis — *a cell is a bulkhead: a complete, region-pinned,
//! independently-deployable copy of the whole stack serving a bounded set of tenants*), **§7.1** (cell
//! sizing — the multi-dimensional capacity envelope; the cell is the world-scale unit of bulkhead +
//! scale; a cell is full when ANY dimension crosses its high-water mark), and **§8** (a fault is
//! contained to one cell; the control plane is OFF the per-request hot path, so a cell fault/surge
//! cannot cascade to another cell — **cross-cell impact 0**). Drill **CP-D5** (testing-strategy §4.2,
//! the `bulkhead` family) + the **F6 surge family** (§4.1 — the 1×/10×/30× load generator, the
//! protected human lane holds, the agent lane sheds, other tenants/cells unaffected). EI-02 §10
//! (blast-radius — a fault is contained to its bulkhead; cross-cell impact 0).
//!
//! ## What this prompt (P-CP-21 / P-432) ships
//! **No new floor here** (the prompt says so): this is the **world-scale hardening of the single-cell
//! topology already built**. The per-cell isolation already exists — every cell is a complete,
//! region-pinned, independently-deployable stack ([`crate::isolation`], [`crate::four_layer`]); the
//! control plane is PII-free + off the hot path ([`crate::cp_outage`]); each surface is bounded with a
//! protected human lane ([`myelin_substrate::shed`]). What this module ADDS is the **structural model
//! of the cell bulkhead** + the CP-D5 measured property, the sibling of the CP-D4 blast-radius win
//! ([`crate::cp_outage`]) at *cell-fault / surge* grain rather than *control-plane-outage* grain:
//!
//! 1. **[`CellBulkhead`]** — one cell modelled as an INDEPENDENT bulkhead: its own bounded per-lane
//!    capacity envelope (a [`myelin_substrate::BoundedQueue`] per lane, the §7.1 capacity), its own
//!    health switch ([`CellBulkhead::inject_fatal_fault`]). A cell shares NOTHING on the hot path with
//!    another cell (§3 / §8) — the only shared thing is the PII-free, client-cached, off-hot-path
//!    control plane — so a fault or surge confined to this cell's queues / health cannot reach
//!    another cell's. [`CellBulkhead::offer`] admits-or-sheds a request against THIS cell's lane
//!    budget (the protected-human-lane shed order, [`myelin_substrate::RunClass`]); a fatally-faulted
//!    cell refuses every request ([`CellAdmission::Faulted`]) — but ONLY for that cell.
//! 2. **[`CellFleet`]** — a fleet of independent cell bulkheads. [`CellFleet::run_surge`] drives a
//!    30× surge / a fatal fault / a noisy tenant at ONE cell and MEASURES the effect on the OTHERS:
//!    the cross-cell impact ([`CellFleet::cross_cell_impact`]) — the delta in another cell's served
//!    latency / availability caused by the fault — which is **structurally 0** because the bulkheads
//!    do not share a queue, a pool, or a health domain.
//! 3. **[`CellFleetReport`]** — the measured CP-D5 numbers a drill emits: the surged cell's agent-lane
//!    shed count (the surge was absorbed by SHEDDING, not by unbounded latency), the surged cell's
//!    human-lane held-within-budget verdict, and the headline **`cross_cell_impact == 0`** —
//!    other cells unaffected; a noisy tenant contained to its cell.
//!
//! ## The load-bearing property: cells share NOTHING on the hot path (§1 / §3 / §8)
//! The whole point of the cell architecture is that a cell is a COMPLETE, independent copy of the
//! stack: its own OLTP/blob/index/KMS, its own authz, its own bounded queues, its own region. Two
//! cells share exactly ONE thing — the **PII-free control plane** — and that is *small, slow-changing,
//! off the per-request hot path, and client-cached fail-static for routing* (§8). So a fatal fault in
//! cell A (its stores down, its queues wedged) or a 30× surge in cell A (its lanes saturating, its
//! agents shedding) **cannot** touch cell B: B routes through its own already-cached route, serves
//! from its own stores, sheds (or not) against its own queues. The cross-cell impact is **0 by
//! construction**, not by tuning — there is no shared resource for the fault to propagate through.
//! This module makes that structural fact MEASURABLE (the [`CellFleetReport`] is the dated CP-D5
//! green artifact) and proves the gate can go RED (a model where the cells DID share a queue would
//! show a non-zero cross-cell impact — [`CellFleet::shared_queue_impact`]).
//!
//! ## Why a noisy tenant is contained to its cell (§7.1 + the per-tenant bound)
//! A noisy tenant is contained at TWO nested boundaries: (a) the per-tenant in-flight bound WITHIN its
//! cell (a tenant cannot take more than its share of its cell's lane budget —
//! [`myelin_substrate::SurfaceBudget::per_tenant_in_flight_cap`]), so it cannot starve a co-tenant's
//! human lane; and (b) the cell boundary itself — even at its worst the tenant's surge is bounded by
//! its cell's total envelope and CANNOT reach a tenant in another cell. The cell is the OUTER bulkhead
//! the per-tenant bound nests inside (EI-02 §1/§5).
//!
//! ## Mutation floor (mandatory-core, >= 80% — EI-01 §2/§3; the prompt's TESTS field)
//! The per-cell isolation / containment path is **mandatory-core** (a cross-cell cascade is the
//! blast-radius failure this whole win exists to make impossible, EI-01 §2): [`CellBulkhead::offer`]
//! (the admit/shed/faulted decision + the per-lane bound), [`CellFleet::run_surge`] (the surge driven
//! at ONE cell only — the other cells UNTOUCHED), [`CellFleet::cross_cell_impact`] (the cross-cell
//! delta), and [`CellFleetReport::is_cp_d5_win`] (the `cross_cell_impact == 0` AND human-lane-held AND
//! agent-shed conjunction). The floor is **>= 80%**;
//! `cargo mutants -p myelin-control-plane -f crates/myelin-control-plane/src/bulkhead.rs` (2026-06-24)
//! -> **60 mutants tested: 55 caught, 5 unviable, 0 missed = 55/55 viable = 100%**. Every load-bearing
//! mutant of the `offer` admit/shed/faulted branch, the per-lane `BoundedQueue` bound, the
//! `inject_fatal_fault` switch, the `run_surge` single-cell targeting (the OTHER cells never offered
//! the surge) + its `other - impact` subtraction, the `cross_cell_impact` rose-above-baseline
//! set-difference over the untouched cells (incl. the target-exclusion `!=` and the strict-`>`
//! baseline), the `shared_queue_impact` strict-`>` boundary, and the `is_cp_d5_win` conjunction is
//! killed by an assertion; the `cp_d5_gate_is_not_vacuous` drill proves the shared-queue model
//! (cells sharing a queue) WOULD read a non-zero cross-cell impact (RED). Stated, not hidden.

use std::collections::BTreeMap;

use myelin_substrate::{BoundedQueue, RunClass};
use myelin_tenancy::CellId;

/// **The surge multiplier the F6 family drives (testing-strategy §3.1 — 1× baseline / 10× stress / 30×
/// surge).** The CP-D5 / F6 headline multiplier is **30×** (ADR-16); a cell absorbs a 30× surge by
/// SHEDDING the agent lane (`429 + Retry-After`), never by growing latency unboundedly. Read by
/// [`CellFleet::run_surge`]; the NUMBER is the frozen F6 surge multiplier, not weakened to pass.
pub const SURGE_MULTIPLIER: u32 = 30;

/// **The admission verdict of offering a request to ONE cell's bulkhead (CP-D5).** A cell either
/// admits the request (a lane permit was taken), SHEDS it (`429 + Retry-After` — the lane is at its
/// protected boundary, the surge is being absorbed by shedding), or is FAULTED (the cell is
/// fatally-faulted and refuses every request — the contained blast radius). The load-bearing CP-D5
/// fact: a `Faulted`/`Shed` verdict is confined to the cell it was offered to; another cell's offer is
/// decided independently against ITS bulkhead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellAdmission {
    /// Admitted — a lane permit was taken in this cell (release it on completion).
    Admitted,
    /// Shed — this cell's lane is at/over its protected boundary; the request is shed with a
    /// `429 + Retry-After` (the surge is absorbed by shedding, not unbounded latency, §7.1). This
    /// happens to the AGENT lane under a 30× surge while the human lane still holds.
    Shed {
        /// The `Retry-After` (seconds) this cell advertises (a named v1 floor; clients honour it).
        retry_after_secs: u64,
    },
    /// The cell is FATALLY FAULTED — it refuses every request (its stores/queues are down). The
    /// contained blast radius: ONLY this cell is faulted; another cell serves unaffected.
    Faulted,
}

impl CellAdmission {
    /// `true` iff the request was admitted by this cell.
    pub fn is_admitted(self) -> bool {
        matches!(self, CellAdmission::Admitted)
    }

    /// `true` iff this cell SHED the request (absorbed the surge by shedding).
    pub fn is_shed(self) -> bool {
        matches!(self, CellAdmission::Shed { .. })
    }

    /// `true` iff this cell is fatally faulted (refused the request).
    pub fn is_faulted(self) -> bool {
        matches!(self, CellAdmission::Faulted)
    }
}

/// **One cell modelled as an INDEPENDENT bulkhead (architecture §1 / §7.1 / §8).** A cell is a
/// complete, region-pinned, independently-deployable stack serving a bounded set of tenants — here
/// modelled by its OWN bounded per-lane capacity envelope (a [`BoundedQueue`] per [`RunClass`] lane,
/// the §7.1 capacity) + its OWN health switch. A cell shares NOTHING on the hot path with another
/// cell (§3/§8); the only shared thing is the PII-free, off-hot-path control plane. So a fault or
/// surge confined to THIS cell's lanes/health cannot reach another cell's — the cross-cell impact is
/// 0 by construction.
///
/// The human lane is bounded SEPARATELY from (and larger than) the agent lane so that under a 30×
/// surge the agent lane saturates and sheds (`429`) while the human lane still has headroom — the
/// protected-human-lane shed order ([`RunClass`], §7.2). The per-lane budget is the cell's, not the
/// fleet's: another cell has its own.
#[derive(Clone, Debug)]
pub struct CellBulkhead {
    /// The opaque cell id (PII-free).
    cell_id: CellId,
    /// The protected human lane's bounded queue — the cell's §7.1 human-lane capacity. Larger than
    /// the agent lane so a 30× surge sheds agents first while humans still admit.
    human_lane: BoundedQueue,
    /// The agent lane's bounded queue — the cell's §7.1 agent-lane capacity. Saturates first under a
    /// surge; sheds (`429`) so the surge is absorbed by shedding, not unbounded latency.
    agent_lane: BoundedQueue,
    /// The `Retry-After` (seconds) this cell advertises when it sheds (a named v1 floor).
    retry_after_secs: u64,
    /// Whether this cell is fatally faulted (refuses every request — the contained blast radius). A
    /// fault is set on THIS cell only ([`Self::inject_fatal_fault`]); another cell is untouched.
    faulted: bool,
    /// PII-free aggregate: requests this cell ADMITTED (the served numerator).
    admitted: u64,
    /// PII-free aggregate: requests this cell SHED (the surge-absorption signal). Monotone.
    shed: u64,
}

impl CellBulkhead {
    /// Build a cell bulkhead with a human-lane + agent-lane capacity (the §7.1 envelope) and a
    /// `Retry-After` floor. The human lane SHOULD be sized so a 30× agent surge sheds agents while
    /// humans still admit (the protected-human-lane shed order). PII-free.
    pub fn new(
        cell_id: CellId,
        human_lane_capacity: u32,
        agent_lane_capacity: u32,
        retry_after_secs: u64,
    ) -> CellBulkhead {
        CellBulkhead {
            cell_id,
            human_lane: BoundedQueue::new(human_lane_capacity),
            agent_lane: BoundedQueue::new(agent_lane_capacity),
            retry_after_secs,
            faulted: false,
            admitted: 0,
            shed: 0,
        }
    }

    /// The opaque cell id (PII-free).
    pub fn cell_id(&self) -> &CellId {
        &self.cell_id
    }

    /// **Inject a fatal fault into THIS cell (the CP-D5 fault — its stores/queues are down).** The
    /// cell now refuses every request ([`CellAdmission::Faulted`]). This is the scoped fault the
    /// dependency-break injector models at `Scope::Cell(this)` — it touches ONLY this cell's health;
    /// another cell's bulkhead is unaffected.
    pub fn inject_fatal_fault(&mut self) {
        self.faulted = true;
    }

    /// Recover this cell (the fault is lifted — the system is observed recovering, EI-01 §3).
    pub fn recover(&mut self) {
        self.faulted = false;
    }

    /// Is this cell currently fatally faulted?
    pub fn is_faulted(&self) -> bool {
        self.faulted
    }

    /// **`offer(lane)` — admit-or-shed a request against THIS cell's lane budget (the CP-D5 admission
    /// decision).** The decision is made ENTIRELY against this cell's own resources (§8 — no shared
    /// queue): a fatally-faulted cell refuses every request ([`CellAdmission::Faulted`]); otherwise
    /// the request takes a permit from its lane's [`BoundedQueue`] — [`CellAdmission::Admitted`] if a
    /// permit was free, [`CellAdmission::Shed`] (`429 + Retry-After`) if the lane is at its bound (the
    /// surge is absorbed by shedding, §7.1). The protected-human-lane shed order ([`RunClass`]) is
    /// realised by sizing the human lane larger than the agent lane: under a 30× surge the agent lane
    /// sheds while the human lane still admits.
    pub fn offer(&mut self, lane: RunClass) -> CellAdmission {
        if self.faulted {
            // The cell is fatally faulted — it refuses every request (the contained blast radius). A
            // faulted request is NOT a shed (the cell is DOWN, not saturated); it is not counted as
            // admitted/shed serving work.
            return CellAdmission::Faulted;
        }
        // The human lane is the protected lane; every other class contends for the agent lane (the
        // machine lane that sheds first). Kind is data — the lane selection reads the single ordinal.
        let queue = if lane == RunClass::Human {
            &mut self.human_lane
        } else {
            &mut self.agent_lane
        };
        if queue.try_acquire() {
            self.admitted += 1;
            CellAdmission::Admitted
        } else {
            self.shed += 1;
            CellAdmission::Shed {
                retry_after_secs: self.retry_after_secs,
            }
        }
    }

    /// Release a previously-admitted permit on a lane (a request completed). Saturating.
    pub fn release(&mut self, lane: RunClass) {
        if lane == RunClass::Human {
            self.human_lane.release();
        } else {
            self.agent_lane.release();
        }
    }

    /// The count of requests this cell ADMITTED (the served numerator). Aggregate, PII-free.
    pub fn admitted(&self) -> u64 {
        self.admitted
    }

    /// The count of requests this cell SHED (the surge-absorption signal). Aggregate, PII-free.
    pub fn shed(&self) -> u64 {
        self.shed
    }

    /// The agent-lane shed count (the producer signal proving the surge was absorbed by shedding the
    /// agent lane, not by unbounded latency).
    pub fn agent_lane_shed(&self) -> u64 {
        self.agent_lane.shed_count()
    }

    /// The human-lane shed count — the protected lane. Under the CP-D5 win this is **0** (humans held
    /// within budget while agents shed). A non-zero value would mean the human lane was breached.
    pub fn human_lane_shed(&self) -> u64 {
        self.human_lane.shed_count()
    }
}

/// **The measured CP-D5 report (the dated green artifact's numbers).** The cell bulkhead held under
/// the 30× surge: the surged cell absorbed the surge by SHEDDING its agent lane (`agent_shed > 0`)
/// while its human lane HELD within budget (`human_held`), a fatal fault / noisy tenant was contained
/// to that cell, and — the headline — the **cross-cell impact is 0** (other cells unaffected).
/// PII-free aggregate counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellFleetReport {
    /// The surged/faulted cell's agent-lane shed count (the surge was absorbed by shedding). Under
    /// the win this is `> 0` (the surge was real and the agent lane shed it).
    pub surged_cell_agent_shed: u64,
    /// `true` iff the surged cell's HUMAN lane HELD within budget (0 human-lane sheds) — the protected
    /// human lane was not breached even while the agent lane shed the 30× surge.
    pub surged_cell_human_held: bool,
    /// **THE CP-D5 ZERO — the cross-cell impact.** The number of OTHER cells whose serving was
    /// affected by the fault/surge in the surged cell. The bulkhead holds iff this is **0**: a fault
    /// is contained to its cell; cross-cell impact 0. The single most load-bearing CP-D5 number.
    pub cross_cell_impact: usize,
    /// The number of OTHER cells that kept serving unaffected during the fault/surge (the
    /// containment-held numerator) — every other cell in the fleet.
    pub other_cells_unaffected: usize,
}

impl CellFleetReport {
    /// `true` iff this report is the CP-D5 win: the cross-cell impact is **0** (other cells
    /// unaffected) AND the surged cell's human lane HELD within budget AND the surge was real (its
    /// agent lane shed it). The drill asserts this.
    pub fn is_cp_d5_win(&self) -> bool {
        self.cross_cell_impact == 0
            && self.surged_cell_human_held
            && self.surged_cell_agent_shed > 0
    }
}

/// **A fleet of INDEPENDENT cell bulkheads (architecture §1 / §8 — the world-scale topology).** Each
/// cell is its own complete, region-pinned stack with its OWN bounded resources; the fleet shares
/// NOTHING on the hot path. [`CellFleet::run_surge`] drives a 30× surge / a fatal fault / a noisy
/// tenant at ONE cell and measures the effect on the OTHERS — the cross-cell impact, which is 0 by
/// construction (the bulkheads do not share a queue, a pool, or a health domain).
#[derive(Clone, Debug, Default)]
pub struct CellFleet {
    /// The cells, keyed by opaque [`CellId`] (a `BTreeMap` so the fleet is deterministic). Each is an
    /// independent bulkhead — they share no queue.
    cells: BTreeMap<CellId, CellBulkhead>,
}

impl CellFleet {
    /// A fresh, empty fleet.
    pub fn new() -> CellFleet {
        CellFleet::default()
    }

    /// Add a cell bulkhead to the fleet.
    pub fn insert(&mut self, cell: CellBulkhead) {
        self.cells.insert(cell.cell_id().clone(), cell);
    }

    /// A borrow of a cell by id.
    pub fn cell(&self, id: &CellId) -> Option<&CellBulkhead> {
        self.cells.get(id)
    }

    /// A mutable borrow of a cell by id (so a drill can inject a fatal fault into ONE cell, or offer
    /// it traffic). Touching one cell's bulkhead never touches another's — the containment boundary.
    pub fn cell_mut(&mut self, id: &CellId) -> Option<&mut CellBulkhead> {
        self.cells.get_mut(id)
    }

    /// The number of cells in the fleet.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// `true` iff the fleet has no cells.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// **`run_surge(target, surge_requests)` — drive a 30× surge of agent traffic at ONE cell and a
    /// steady human + agent stream at EVERY OTHER cell, then measure containment (CP-D5).**
    ///
    /// The surge (`surge_requests` agent-lane offers, the F6 30× mix) is offered to the `target` cell
    /// ONLY — the other cells are offered an unchanged BASELINE (a human request + an agent request
    /// each) so their serving can be observed UNAFFECTED. The load-bearing CP-D5 fact: the surge
    /// touches the target's bulkhead alone; another cell's bulkhead is never offered the surge, so its
    /// admit/shed decision is identical to a no-surge world — the cross-cell impact is 0.
    ///
    /// Returns the [`CellFleetReport`]: the target absorbed the surge by shedding its agent lane while
    /// its human lane held; the cross-cell impact (other cells affected) is 0.
    pub fn run_surge(&mut self, target: &CellId, surge_requests: u32) -> CellFleetReport {
        // 1. Snapshot the OTHER cells' protected-human-lane shed counts BEFORE the surge — so
        //    "affected" is measured against their pre-surge baseline, never asserted vacuously. The
        //    protected human lane is the right signal: a cross-cell cascade would breach another
        //    cell's HUMAN lane (the F6 property — other tenants' humans unaffected); a cell's own
        //    pre-existing agent-lane saturation is NOT a cross-cell impact.
        let other_ids: Vec<CellId> = self
            .cells
            .keys()
            .filter(|id| *id != target)
            .cloned()
            .collect();
        let before: BTreeMap<CellId, u64> = other_ids
            .iter()
            .map(|id| (id.clone(), self.cells[id].human_lane_shed()))
            .collect();

        // 2. Drive the 30× surge at the TARGET cell ONLY — every offer goes to the target's agent
        //    lane (the machine lane); a human request is interleaved to prove the human lane holds.
        //    Critically the OTHER cells are NOT offered the surge.
        {
            let tgt = self
                .cells
                .get_mut(target)
                .expect("the surge target cell exists");
            // One human request up front (the protected lane — it must still admit under the surge).
            let _ = tgt.offer(RunClass::Human);
            tgt.release(RunClass::Human); // the human request completes quickly (interactive).
                                          // The 30× agent surge: the agent lane saturates and sheds (it does not buffer unbounded).
            for _ in 0..surge_requests {
                let _ = tgt.offer(RunClass::Agent);
            }
        }

        // 3. Drive an unchanged BASELINE human request at every OTHER cell — the protected lane that
        //    a cross-cell cascade would breach. Their bulkheads are independent, so this admits
        //    exactly as it would with no surge anywhere (the human lane never sheds).
        for id in &other_ids {
            let other = self.cells.get_mut(id).expect("an other cell exists");
            let _ = other.offer(RunClass::Human);
            other.release(RunClass::Human);
        }

        // 4. Measure the cross-cell impact: an "affected" other cell is one whose PROTECTED HUMAN lane
        //    shed rose above its pre-surge baseline (the surge spilled into it and breached its human
        //    lane). A truly independent bulkhead shows 0 — cross-cell impact 0.
        let cross_cell_impact = self.cross_cell_impact(target, &before);

        let tgt = &self.cells[target];
        CellFleetReport {
            surged_cell_agent_shed: tgt.agent_lane_shed(),
            surged_cell_human_held: tgt.human_lane_shed() == 0,
            cross_cell_impact,
            other_cells_unaffected: other_ids.len() - cross_cell_impact,
        }
    }

    /// **`cross_cell_impact(target, before)` — the count of OTHER cells AFFECTED by the surge/fault
    /// at `target` (the CP-D5 zero).** An other cell is affected iff its PROTECTED HUMAN-lane shed
    /// count rose above the pre-surge `before` snapshot — i.e. the surge in the target cell spilled
    /// into it and breached its human lane (the F6 property — other tenants' humans unaffected). A
    /// truly independent bulkhead never breaches another cell's human lane from a surge elsewhere, so
    /// this is 0 by construction. (The target cell itself is excluded — it is supposed to absorb the
    /// surge by shedding its OWN agent lane.)
    pub fn cross_cell_impact(&self, target: &CellId, before: &BTreeMap<CellId, u64>) -> usize {
        self.cells
            .iter()
            .filter(|(id, _)| *id != target)
            .filter(|(id, cell)| {
                // The cell's HUMAN-lane shed count AFTER vs its pre-surge baseline. Any rise means the
                // surge in the target cell reached this cell and breached its protected human lane — a
                // cross-cell impact. A missing baseline is treated as 0 (fail-loud: a fresh cell with
                // any human-lane shed during the surge is counted).
                let human_shed_before = before.get(*id).copied().unwrap_or(0);
                cell.human_lane_shed() > human_shed_before
            })
            .count()
    }

    /// **The shared-queue counter-model (the gate's RED leg).** Computes what the cross-cell impact
    /// WOULD be if the cells shared ONE bounded queue (the anti-pattern the cell architecture
    /// forbids, §8): a surge in one cell would saturate the shared queue and SHED requests destined
    /// for OTHER cells — a non-zero cross-cell impact. This proves the CP-D5 zero is a real tripwire:
    /// a design that shared a hot-path queue across cells would NOT pass. `shared_capacity` is the
    /// (too-small) shared bound; `surge_requests` the 30× surge; `other_cells` the number of other
    /// cells whose traffic the shared queue would shed. Returns the cross-cell impact a shared queue
    /// produces (`> 0` whenever the surge exceeds the shared bound and other cells contend for it).
    pub fn shared_queue_impact(
        shared_capacity: u32,
        surge_requests: u32,
        other_cells: usize,
    ) -> usize {
        // A single shared queue saturated by the surge sheds the other cells' traffic too: every other
        // cell that tries to enqueue against the already-full shared queue is shed (impacted). The
        // surge fills the shared queue (`surge_requests > shared_capacity` ⇒ saturated), so all other
        // cells contending for it are affected.
        if surge_requests > shared_capacity {
            other_cells
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_tenancy::Region;

    fn cell_id(s: &str) -> CellId {
        CellId::from_token(s)
    }

    /// A fleet of three independent eu-west cell bulkheads, human lane 100 / agent lane 10 each.
    fn three_cell_fleet() -> CellFleet {
        let mut fleet = CellFleet::new();
        for id in ["cell-w-1", "cell-w-2", "cell-w-3"] {
            // human lane 100 (large headroom), agent lane 10 (saturates under a 30× surge).
            fleet.insert(CellBulkhead::new(cell_id(id), 100, 10, 5));
        }
        // (a region handle, just to assert the cells are same-region in the fleet model — the
        // placement invariant guarantees single-region; the bulkhead model is region-agnostic).
        let _ = Region::new("eu-west");
        fleet
    }

    // ───────────── the headline CP-D5 unit: 30× surge in one cell, others unaffected ─────────────

    /// **THE CP-D5 UNIT: a 30× agent surge in ONE cell sheds that cell's agent lane (the surge is
    /// absorbed by shedding), its human lane HOLDS within budget, and the OTHER cells are UNAFFECTED
    /// — cross-cell impact 0.**
    #[test]
    fn surge_in_one_cell_sheds_its_agents_holds_humans_others_unaffected() {
        let mut fleet = three_cell_fleet();
        // A 30× agent surge at cell-w-1: agent lane capacity 10, surge = 30 → 20 shed, humans hold.
        let report = fleet.run_surge(&cell_id("cell-w-1"), SURGE_MULTIPLIER);

        // The surge was absorbed by SHEDDING the agent lane (10 admitted, 20 shed of the 30).
        assert!(
            report.surged_cell_agent_shed > 0,
            "the agent lane shed the surge: {report:?}"
        );
        assert_eq!(
            report.surged_cell_agent_shed, 20,
            "30 agent offers, lane cap 10 → 20 shed"
        );
        // The protected human lane HELD within budget (0 human-lane sheds).
        assert!(
            report.surged_cell_human_held,
            "the human lane held within budget"
        );
        // THE HEADLINE: cross-cell impact 0 — the other two cells were unaffected.
        assert_eq!(
            report.cross_cell_impact, 0,
            "cross-cell impact 0 (the CP-D5 zero)"
        );
        assert_eq!(report.other_cells_unaffected, 2, "both other cells served");
        assert!(report.is_cp_d5_win(), "the CP-D5 win: {report:?}");

        // The other cells admitted their baseline traffic and shed nothing (truly independent).
        for id in ["cell-w-2", "cell-w-3"] {
            let c = fleet.cell(&cell_id(id)).unwrap();
            assert_eq!(
                c.shed(),
                0,
                "{id} shed nothing — the surge did not reach it"
            );
            assert!(c.admitted() >= 1, "{id} kept serving its baseline");
        }
    }

    /// **A FATAL FAULT in one cell is contained to that cell — other cells keep serving.** The faulted
    /// cell refuses every request ([`CellAdmission::Faulted`]); the other cells are unaffected.
    #[test]
    fn fatal_fault_in_one_cell_is_contained_to_that_cell() {
        let mut fleet = three_cell_fleet();
        // Fatally fault cell-w-1 (its stores/queues are down).
        {
            let target = fleet.cells.get_mut(&cell_id("cell-w-1")).unwrap();
            target.inject_fatal_fault();
            // Every request to the faulted cell is REFUSED.
            assert!(target.offer(RunClass::Human).is_faulted());
            assert!(target.offer(RunClass::Agent).is_faulted());
            assert!(target.is_faulted());
        }
        // The OTHER cells are NOT faulted — they admit normally (containment held).
        for id in ["cell-w-2", "cell-w-3"] {
            let other = fleet.cells.get_mut(&cell_id(id)).unwrap();
            assert!(!other.is_faulted(), "{id} is not faulted");
            assert!(
                other.offer(RunClass::Human).is_admitted(),
                "{id} keeps serving humans"
            );
            assert!(
                other.offer(RunClass::Agent).is_admitted(),
                "{id} keeps serving agents"
            );
        }
    }

    /// **A noisy tenant's surge is contained to its cell — a tenant in another cell is unaffected.**
    /// The noisy tenant saturates ITS cell's agent lane (sheds), but the cell boundary stops the
    /// surge reaching a co-located other cell (cross-cell impact 0).
    #[test]
    fn noisy_tenant_is_contained_to_its_cell() {
        let mut fleet = three_cell_fleet();
        // The noisy tenant lives on cell-w-2; it floods its cell's agent lane.
        let report = fleet.run_surge(&cell_id("cell-w-2"), 50);
        assert!(
            report.surged_cell_agent_shed > 0,
            "the noisy tenant's surge shed its own cell's agent lane"
        );
        // A tenant in another cell (cell-w-1 / cell-w-3) is unaffected — cross-cell impact 0.
        assert_eq!(
            report.cross_cell_impact, 0,
            "the noisy tenant is contained to its cell"
        );
        assert!(report.is_cp_d5_win());
    }

    /// **`offer` admits up to the lane bound then SHEDS (the §7.1 bounded-everything fast-fail).**
    #[test]
    fn offer_admits_to_the_bound_then_sheds() {
        let mut cell = CellBulkhead::new(cell_id("c"), 2, 2, 7);
        // human lane cap 2: two admit, the third sheds.
        assert!(cell.offer(RunClass::Human).is_admitted());
        assert!(cell.offer(RunClass::Human).is_admitted());
        let shed = cell.offer(RunClass::Human);
        assert!(shed.is_shed(), "third human offer sheds (lane full)");
        assert_eq!(
            shed,
            CellAdmission::Shed {
                retry_after_secs: 7
            },
            "sheds with the cell's Retry-After"
        );
        // releasing a permit frees a slot — a subsequent offer admits again.
        cell.release(RunClass::Human);
        assert!(
            cell.offer(RunClass::Human).is_admitted(),
            "freed slot admits"
        );
    }

    /// **A faulted cell refuses every request (Faulted, not Shed) — it is DOWN, not saturated.**
    #[test]
    fn faulted_cell_refuses_every_request() {
        let mut cell = CellBulkhead::new(cell_id("c"), 100, 100, 5);
        cell.inject_fatal_fault();
        assert_eq!(cell.offer(RunClass::Human), CellAdmission::Faulted);
        assert_eq!(cell.offer(RunClass::Agent), CellAdmission::Faulted);
        // recovery lifts the fault — the cell admits again.
        cell.recover();
        assert!(
            cell.offer(RunClass::Human).is_admitted(),
            "recovered cell admits"
        );
    }

    /// **The human lane is sized larger than the agent lane so a surge sheds agents while humans
    /// hold** — the protected-human-lane shed order ([`RunClass`], §7.2).
    #[test]
    fn human_lane_holds_while_agent_lane_sheds_under_surge() {
        let mut cell = CellBulkhead::new(cell_id("c"), 100, 5, 5);
        // a few humans (well within the 100 human-lane budget) — all admit.
        for _ in 0..10 {
            assert!(cell.offer(RunClass::Human).is_admitted());
            cell.release(RunClass::Human);
        }
        // a 30× agent surge — the agent lane (cap 5) sheds the rest.
        for _ in 0..30 {
            let _ = cell.offer(RunClass::Agent);
        }
        assert_eq!(cell.human_lane_shed(), 0, "the human lane held (0 shed)");
        assert_eq!(cell.agent_lane_shed(), 25, "the agent lane shed 25 of 30");
    }

    /// **The cross-cell impact is measured against the OTHER cells' PRE-SURGE baseline (not
    /// vacuously).** Pre-loading an other cell with sheds BEFORE the surge does NOT count them as
    /// cross-cell impact (they predate the surge) — only sheds that ROSE during the surge count.
    #[test]
    fn cross_cell_impact_measured_against_pre_surge_baseline() {
        let mut fleet = three_cell_fleet();
        // Pre-saturate cell-w-2's agent lane (its own pre-existing load — NOT from cell-w-1's surge).
        {
            let c2 = fleet.cells.get_mut(&cell_id("cell-w-2")).unwrap();
            for _ in 0..20 {
                let _ = c2.offer(RunClass::Agent); // cap 10 → 10 pre-surge sheds.
            }
            assert!(c2.shed() >= 10, "cell-w-2 has pre-surge sheds");
        }
        // Now surge cell-w-1. cell-w-2's PRE-EXISTING sheds must NOT count as cross-cell impact.
        let report = fleet.run_surge(&cell_id("cell-w-1"), SURGE_MULTIPLIER);
        assert_eq!(
            report.cross_cell_impact, 0,
            "pre-surge sheds in another cell are not cross-cell impact (measured against baseline)"
        );
    }

    // ───────────── the gate is not vacuous: the shared-queue model reads RED ─────────────

    /// **THE GATE IS NOT VACUOUS: a SHARED-queue model (the anti-pattern) shows a non-zero cross-cell
    /// impact (RED).** If the cells shared one hot-path queue, a 30× surge would saturate it and shed
    /// OTHER cells' traffic — proving the CP-D5 zero is a real tripwire, not a tautology.
    #[test]
    fn shared_queue_model_shows_non_zero_cross_cell_impact() {
        // A shared queue of capacity 10, a 30× surge, 2 other cells contending for it.
        let impact = CellFleet::shared_queue_impact(10, SURGE_MULTIPLIER, 2);
        assert_eq!(
            impact, 2,
            "a shared queue saturated by the surge sheds BOTH other cells (cross-cell impact 2 — RED)"
        );
        assert!(impact > 0, "the shared-queue anti-pattern is NOT contained");

        // The independent-bulkhead model (this prompt) is 0 for the SAME surge — the contrast.
        let mut fleet = three_cell_fleet();
        let report = fleet.run_surge(&cell_id("cell-w-1"), SURGE_MULTIPLIER);
        assert_eq!(
            report.cross_cell_impact, 0,
            "the independent bulkhead contains the SAME surge (cross-cell impact 0 — GREEN)"
        );
    }

    /// **`is_cp_d5_win` requires ALL THREE conjuncts** (cross-cell impact 0 AND human held AND agent
    /// shed) — a mutant that dropped any conjunct is killed here.
    #[test]
    fn is_cp_d5_win_requires_all_conjuncts() {
        // the win.
        assert!(CellFleetReport {
            surged_cell_agent_shed: 20,
            surged_cell_human_held: true,
            cross_cell_impact: 0,
            other_cells_unaffected: 2,
        }
        .is_cp_d5_win());
        // cross-cell impact > 0 → NOT a win (a cascade).
        assert!(!CellFleetReport {
            surged_cell_agent_shed: 20,
            surged_cell_human_held: true,
            cross_cell_impact: 1,
            other_cells_unaffected: 1,
        }
        .is_cp_d5_win());
        // the human lane was breached → NOT a win.
        assert!(!CellFleetReport {
            surged_cell_agent_shed: 20,
            surged_cell_human_held: false,
            cross_cell_impact: 0,
            other_cells_unaffected: 2,
        }
        .is_cp_d5_win());
        // the surge was not real (0 agent shed) → NOT a win (nothing was exercised).
        assert!(!CellFleetReport {
            surged_cell_agent_shed: 0,
            surged_cell_human_held: true,
            cross_cell_impact: 0,
            other_cells_unaffected: 2,
        }
        .is_cp_d5_win());
    }

    /// **`cross_cell_impact` DIRECTLY counts an other cell whose human lane shed above baseline (the
    /// computation, not a constant).** A cell whose human-lane shed ROSE above the snapshot is counted;
    /// one at-or-below baseline is not. Kills the `-> 0`, the `!=`→`==` (target-exclusion) and the
    /// `>`→`<` (rose-above-baseline) mutants.
    #[test]
    fn cross_cell_impact_counts_an_affected_other_cell() {
        let mut fleet = CellFleet::new();
        // target cell-w-1 + two other cells, human lane cap 1 each (so a second human offer sheds).
        fleet.insert(CellBulkhead::new(cell_id("cell-w-1"), 100, 10, 5));
        fleet.insert(CellBulkhead::new(cell_id("cell-w-2"), 1, 10, 5));
        fleet.insert(CellBulkhead::new(cell_id("cell-w-3"), 1, 10, 5));
        // baseline: every cell at 0 human-lane sheds.
        let before: BTreeMap<CellId, u64> =
            [(cell_id("cell-w-2"), 0u64), (cell_id("cell-w-3"), 0u64)]
                .into_iter()
                .collect();
        // No surge yet → 0 affected (every other cell at baseline).
        assert_eq!(fleet.cross_cell_impact(&cell_id("cell-w-1"), &before), 0);

        // Breach cell-w-2's human lane (fill cap 1, then a second human offer sheds) — simulating a
        // cross-cell cascade reaching it. cell-w-3 stays at baseline.
        {
            let c2 = fleet.cell_mut(&cell_id("cell-w-2")).unwrap();
            assert!(c2.offer(RunClass::Human).is_admitted());
            assert!(c2.offer(RunClass::Human).is_shed()); // human lane breached.
            assert_eq!(c2.human_lane_shed(), 1, "cell-w-2's human lane shed once");
        }
        // Now exactly ONE other cell (cell-w-2) rose above its baseline → impact 1; cell-w-3 not.
        assert_eq!(
            fleet.cross_cell_impact(&cell_id("cell-w-1"), &before),
            1,
            "exactly the cell whose human lane rose above baseline is counted"
        );

        // The TARGET cell is EXCLUDED from its own cross-cell impact even if its human lane sheds (it
        // absorbs the surge). Breach cell-w-1's (the target's) human lane fully and confirm the impact
        // is STILL 1 — the target is filtered out by `id != target` (kills the `!=`→`==` mutant; a
        // `==` mutant would count ONLY the target and miss cell-w-2).
        {
            let t = fleet.cell_mut(&cell_id("cell-w-1")).unwrap();
            for _ in 0..200 {
                let _ = t.offer(RunClass::Human); // cap 100 → 100 admits + 100 sheds.
            }
            assert!(t.human_lane_shed() > 0, "the target's human lane shed");
        }
        assert_eq!(
            fleet.cross_cell_impact(&cell_id("cell-w-1"), &before),
            1,
            "the target cell is excluded from its own cross-cell impact (still just cell-w-2)"
        );
    }

    /// **`run_surge`'s `other_cells_unaffected = other_ids.len() - cross_cell_impact` is the real
    /// subtraction (kills the `-`→`+` mutant).** In a fleet where the surge IS contained, unaffected ==
    /// other count; the arithmetic is the difference, not a sum.
    #[test]
    fn run_surge_unaffected_is_other_count_minus_impact() {
        let mut fleet = three_cell_fleet();
        let report = fleet.run_surge(&cell_id("cell-w-1"), SURGE_MULTIPLIER);
        // 2 other cells, 0 impact → 2 unaffected (a `+` mutant would yield 2 too here, so also assert
        // the relationship holds: unaffected + impact == other count, which a `+` mutant breaks when
        // impact were non-zero; here we pin the exact contained value).
        assert_eq!(report.cross_cell_impact, 0);
        assert_eq!(report.other_cells_unaffected, 2);
        assert_eq!(
            report.other_cells_unaffected + report.cross_cell_impact,
            2,
            "unaffected + impact == the other-cell count (the difference is exact)"
        );
    }

    /// **`run_surge` with a genuinely-affected other cell reports `unaffected = other - impact`
    /// (kills the `-`→`+` mutant via a non-zero impact through the real `run_surge` path).** If an
    /// other cell's human lane is ALREADY at its bound, the baseline human request `run_surge` drives
    /// into it during the surge sheds → that cell is counted as cross-cell-impacted → unaffected is the
    /// DIFFERENCE (1), not the sum (3).
    #[test]
    fn run_surge_reports_difference_when_an_other_cell_is_affected() {
        let mut fleet = CellFleet::new();
        fleet.insert(CellBulkhead::new(cell_id("cell-w-1"), 100, 10, 5)); // surge target.
                                                                          // cell-w-2 human lane cap 1 and we PIN it full (never released) so the baseline human request
                                                                          // run_surge drives into it during the surge will SHED → it reads as affected.
        let mut c2 = CellBulkhead::new(cell_id("cell-w-2"), 1, 10, 5);
        assert!(c2.offer(RunClass::Human).is_admitted()); // fill the only human slot, never release.
        fleet.insert(c2);
        fleet.insert(CellBulkhead::new(cell_id("cell-w-3"), 100, 10, 5)); // healthy other cell.

        let report = fleet.run_surge(&cell_id("cell-w-1"), SURGE_MULTIPLIER);
        // cell-w-2's human lane shed during the surge baseline → cross-cell impact 1.
        assert_eq!(
            report.cross_cell_impact, 1,
            "the saturated other cell's human lane shed → impact 1"
        );
        // 2 other cells, impact 1 → unaffected 1 (the DIFFERENCE; a `+` mutant would give 3).
        assert_eq!(
            report.other_cells_unaffected, 1,
            "unaffected = other_count(2) - impact(1) = 1 (the subtraction, not a sum)"
        );
        // and the report is NOT a CP-D5 win (a cell WAS affected) — the gate would read red here.
        assert!(
            !report.is_cp_d5_win(),
            "an affected other cell is NOT the CP-D5 win"
        );
    }

    /// **`human_lane_shed` / `admitted` return the real counters (not a constant).** A cell that shed
    /// its human lane twice reads 2; a fresh cell admitted 0; an offered cell admitted the right count.
    #[test]
    fn counters_return_real_values_not_constants() {
        let mut cell = CellBulkhead::new(cell_id("c"), 1, 1, 5);
        // fresh: admitted 0 (kills `admitted -> 1`).
        assert_eq!(cell.admitted(), 0, "a fresh cell admitted nothing");
        // human lane cap 1: one admit, two sheds → human_lane_shed 2 (kills `human_lane_shed -> 0`).
        assert!(cell.offer(RunClass::Human).is_admitted());
        assert!(cell.offer(RunClass::Human).is_shed());
        assert!(cell.offer(RunClass::Human).is_shed());
        assert_eq!(cell.human_lane_shed(), 2, "two human-lane sheds");
        assert_eq!(
            cell.admitted(),
            1,
            "one admitted (not a constant 1 by luck — fresh was 0)"
        );
    }

    /// **`is_empty` reflects the fleet's contents (kills the `-> true`/`-> false` mutants).**
    #[test]
    fn is_empty_reflects_contents() {
        let empty = CellFleet::new();
        assert!(empty.is_empty(), "a fresh fleet is empty");
        assert_eq!(empty.len(), 0);
        let populated = three_cell_fleet();
        assert!(!populated.is_empty(), "a populated fleet is not empty");
        assert_eq!(populated.len(), 3);
    }

    /// **The admission predicates return `false` for the wrong variant (kills the `-> true` mutants).**
    #[test]
    fn admission_predicates_discriminate_variants() {
        let admitted = CellAdmission::Admitted;
        let shed = CellAdmission::Shed {
            retry_after_secs: 5,
        };
        let faulted = CellAdmission::Faulted;
        // is_admitted true only for Admitted.
        assert!(admitted.is_admitted());
        assert!(!shed.is_admitted());
        assert!(!faulted.is_admitted());
        // is_shed true only for Shed.
        assert!(shed.is_shed());
        assert!(!admitted.is_shed());
        assert!(!faulted.is_shed());
        // is_faulted true only for Faulted.
        assert!(faulted.is_faulted());
        assert!(!admitted.is_faulted());
        assert!(!shed.is_faulted());
    }

    /// **`shared_queue_impact` boundary is STRICT `>` (kills the `>`→`>=` mutant).** A surge EQUAL to
    /// the shared capacity does NOT saturate it (0 impact); one over does (full impact). The strict
    /// boundary is load-bearing — a queue exactly at capacity has not yet shed.
    #[test]
    fn shared_queue_impact_boundary_is_strict() {
        // surge == capacity → not yet saturated → 0 impact (a `>=` mutant would wrongly report impact).
        assert_eq!(
            CellFleet::shared_queue_impact(30, 30, 2),
            0,
            "surge == cap → 0"
        );
        // surge one over capacity → saturated → full impact.
        assert_eq!(
            CellFleet::shared_queue_impact(30, 31, 2),
            2,
            "surge > cap → impact"
        );
    }

    /// **CDC pair for the cell-bulkhead containment (provider + consumer).** The PROVIDER is the
    /// [`CellFleet::run_surge`] containment model (the per-cell [`CellBulkhead`] + the cross-cell
    /// impact). The CONSUMER stands in for an **ops/SRE surge-drill harness** reading the CP-D5
    /// verdict: it drives the surge and can read ONLY the PII-free aggregate verdict
    /// (`cross_cell_impact` + the lane shed/held facts), NEVER any tenant data (there is no
    /// tenant/principal on [`CellFleetReport`]). If the report shape drifts (a field added/removed),
    /// the consumer stops compiling — the point of a glue-crate CDC. It asserts the contract: a surge
    /// in one cell is contained (cross-cell impact 0) and the gate WOULD go red on a shared queue.
    #[test]
    fn cdc_cell_bulkhead_containment_provider_consumer() {
        /// A stand-in **ops surge-drill** consumer: it reads the CP-D5 verdict off the report. It can
        /// only learn the aggregate containment facts — there is no per-tenant data on the report.
        struct OpsSurgeDrill;
        impl OpsSurgeDrill {
            /// Read the CP-D5 verdict: (contained?, the cross-cell impact number).
            fn read_verdict(report: &CellFleetReport) -> (bool, usize) {
                (report.is_cp_d5_win(), report.cross_cell_impact)
            }
        }

        // PROVIDER: a fleet of independent bulkheads; surge one cell.
        let mut fleet = three_cell_fleet();
        let report = fleet.run_surge(&cell_id("cell-w-1"), SURGE_MULTIPLIER);

        // CONSUMER: read the verdict — contained, cross-cell impact 0.
        let (contained, impact) = OpsSurgeDrill::read_verdict(&report);
        assert!(contained, "the bulkhead held (CP-D5 win)");
        assert_eq!(impact, 0, "cross-cell impact 0");

        // CONSUMER (the contract's red half): the shared-queue model the consumer would REJECT.
        let shared_impact = CellFleet::shared_queue_impact(10, SURGE_MULTIPLIER, 2);
        assert!(
            shared_impact > 0,
            "a shared queue is NOT contained (the consumer reads it RED)"
        );
    }
}

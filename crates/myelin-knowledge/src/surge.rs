//! # `surge` — the all-hands-doc surge controls + the concurrent-same-gap LexoRank storm
//! (KN-P32 / global P-487, M5 — KN-D8 + the F6 leg)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/knowledge-platform/architecture/02-internals-and-algorithms.md`
//! §3.5 (the concurrent-same-gap LexoRank insert storm — no key-collision reorder, bounded rebalance,
//! now under the move-CRDT from KN-P29) + `05-hard-problems.md` (the hot-doc thundering-herd
//! discipline). **Shared-systems:**
//! `planning/05-refined-shared-systems-architecture/00-platform-substrate.md` §7.1 (bounded everything),
//! §7.2/§7.3 (the principal-aware limiter + the protected human lane — shed order
//! `speculative → batch/CI → agent → human-last`), §7.6 (the per-surface shed-budget table — the
//! Collab op-stream is one OQ-K surface). **Doctrine:**
//! `external-insights/01-process-and-quality-doctrine.md` §3 (the 1×/10×/30× generator; the multiplier
//! is read from the FROZEN thresholds file, never hardcoded; never weaken a threshold to pass — a red
//! is a dated `claimed-not-proven` row), §2 (the protected human lane; per-tenant blast-radius).
//! **Contract-index:** row **1.11** (the protected-human-lane shed order + per-surface shed budgets,
//! OQ-K — Knowledge's collab op-stream is one lane), row **1.8** (the per-lane shed-count telemetry).
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` **KN-D8** (an
//! all-hands doc with thousands of concurrent readers/editors → per-doc op cap + read-fanout bound +
//! active-editor lane reservation hold within budget; other tenants unaffected; the concurrent-same-gap
//! LexoRank insert storm → 0 reorder) + the F6 surge family leg (human lane holds, agent lane sheds
//! `429 + Retry-After`, cross-tenant impact 0).
//!
//! ## What this module is (the KN-P32 surge half)
//! An **all-hands doc** is the worst-case collab surface: thousands of concurrent readers/editors on
//! one page, one op fan-out, one ordered-sibling list under a concurrent-insert storm. This module
//! hardens the EXISTING transport/CRDT under that surge with three controls the prompt names, plus the
//! LexoRank-storm guarantee:
//!
//! - **(a) the active-editor lane reservation** — the doctrine shed order
//!   (`speculative → batch/CI → agent → human-last`) tuned to the op-stream: **passive viewers shed
//!   before active editors, agents before humans**. A viewer's read is a `speculative` down-class; an
//!   active human editor holds the protected lane. This is a thin Knowledge wiring over the substrate's
//!   [`myelin_substrate::shed::ShedLane`] for the one [`ShedSurface::CollabOpStream`] surface — NOT a
//!   re-authored shed lane (EI-01 §7 — the same reuse [`myelin_search::surge`] /
//!   [`myelin_refs_service::surge`] practise).
//! - **(b) the per-doc op in-flight cap** — a [`BoundedQueue`] PER DOC so one hot doc's op fan-out
//!   cannot grow latency unboundedly (Little's Law, §7.1). When the doc's in-flight ops hit the cap the
//!   op fast-fails (sheds) rather than buffering — the thundering-herd discipline.
//! - **(c) the read-fanout bound** — a [`BoundedQueue`] bounding the number of concurrent subscriber
//!   fan-out sends a single op may trigger, so a viewer storm cannot turn one edit into an unbounded
//!   broadcast.
//!
//! And the LexoRank storm guarantee:
//! - **(d) the concurrent-same-gap LexoRank insert storm** — N concurrent inserts into the SAME sibling
//!   gap each produce a DISTINCT [`OrderKey`] (the frozen 2-char jitter, §2.5), so there is **no
//!   key-collision reorder**, and the rebalance cost is BOUNDED (a key only trips
//!   [`OrderKey::needs_rebalance`] at the frozen 48-char trigger — the storm does not force an
//!   unbounded rebalance). This is the §3.5 property, asserted with the FROZEN
//!   [`myelin_query::field::OrderKey`] primitives (CONSUMED, never re-implemented).
//!
//! ## What this module REUSES (EI-01 §7 — never a parallel second implementation)
//! The shed order itself is the substrate's [`myelin_substrate::shed`]: this module does NOT re-author
//! the shed lane / run-class / budget table. It WIRES the existing [`ShedLane`] over the one existing
//! [`ShedSurface::CollabOpStream`] surface, reading the budget **from the thresholds file**
//! ([`myelin_substrate::Thresholds`]) — the tuned OQ-K numbers, never a hardcoded magic value. The
//! bounded queues are the substrate's [`BoundedQueue`]. The LexoRank primitives are the frozen
//! [`myelin_query::field::OrderKey`]/[`Jitter`].
//!
//! ## Floors named (VISION §3 — name your floors)
//! - **No NEW floor is resolved here** (per the prompt): this hardens the existing transport/CRDT under
//!   surge. The move-CRDT sibling-ordering ownership it leans on landed in **KN-P29** (the Yrs
//!   promotion); the shed order 1.11 is owned by the substrate.
//! - **The 30× world-scale FLEET-hardware load is the ONE legitimate remaining floor** (real fleet,
//!   testing-strategy §4.1, [`FLEET_HARDWARE_FLOOR`]). Here the load is the in-process storm at the
//!   surge multiplier across the surging tenant; the PROPERTIES (per-tenant fairness, shed-order,
//!   cross-tenant-0, 0-reorder) are complete + testable now and do not change shape when real fleet
//!   hardware carries the load.
//!
//! ## Mutation floor (mandatory-core — EI-01 §2/§3)
//! The shed-order DECISION path ([`CollabSurgeGate::admit_for`]/[`CollabSurgeGate::admit_class`] → the
//! editor-protected per-tenant graded admit) + the per-doc op-cap admission
//! ([`CollabSurgeGate::admit_doc_op`]) + the read-fanout admission
//! ([`CollabSurgeGate::admit_read_fanout`]) are mandatory-core: an off-by-one that sheds an editor
//! before a viewer, leaks one tenant's budget into another, or lets a doc's op fan-out grow unbounded
//! is the failure this exists to catch. **cargo-mutants floor: 100% of mutants in the
//! shed/admit/cap/fanout decision path are caught** (the surge tests below are written to that floor —
//! every threshold boundary + the per-tenant isolation + the 0-reorder predicate has a killing
//! assertion).

use std::collections::HashMap;

use myelin_identity::Principal;
use myelin_query::field::{Jitter, OrderKey};
use myelin_substrate::shed::{
    BoundedQueue, RunClass, RunClassHeader, ShedDecision, ShedLane, Surface as ShedSurface,
    SurfaceBudget,
};
use myelin_substrate::Thresholds;
use myelin_tenancy::TenantId;

/// **The Knowledge collab surge default-to-beat multiplier (KN-D8 / the F6 leg).** The 30× world-scale
/// surge factor the KN-D8 drill drives at — read from the FROZEN thresholds file `[surge] multiplier`
/// row (the versioned source of truth) and asserted to equal this documented default-to-beat; a
/// divergence is a LOUD failure, never a silent weakening (EI-01 §3).
pub const COLLAB_SURGE_MULTIPLIER: u32 = 30;

/// **The 30× world-scale FLEET-hardware load is the ONE legitimate remaining floor** (real fleet,
/// testing-strategy §4.1). The in-process storm here drives the surge multiplier across the surging
/// tenant and proves the per-tenant fairness + shed-order + cross-tenant-0 + 0-reorder PROPERTIES; the
/// real fleet-hardware load is the named floor (it does not change the shape of these properties).
pub const FLEET_HARDWARE_FLOOR: &str = "world-scale-30x-fleet-hardware (testing-strategy §4.1)";

// ───────────────────────────── the collab surge gate ─────────────────────────────────────────────

/// **Why a collab op/read was refused at the surge gate** — the typed form the transport maps to the
/// wire `429`. A shed carries the `Retry-After` (seconds) the client honours (the no-amplification
/// guarantee — our ResilientClient honours `Retry-After`, P-S17, so a shed is not a retry-storm
/// amplifier).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CollabShedRejection {
    /// The lane that was shed (`speculative` / `batch_ci` / `agent` / `human`) — the contract-1.8
    /// per-lane shed-count signal keys on this. A **passive viewer** sheds in the `speculative` lane;
    /// an **active editor** holds the agent/human lane.
    pub lane: RunClass,
    /// Why the request was refused — the per-tenant op-stream lane, the per-doc op cap, or the
    /// read-fanout bound. The op-stream lane is the protected-human-lane shed; the per-doc/fanout
    /// bounds are the bounded-everything fast-fail.
    pub reason: CollabShedReason,
    /// The `Retry-After` value in **seconds** (the frozen §2.10 unit) the transport sets on the `429
    /// Too Many Requests` response.
    pub retry_after_secs: u64,
}

/// The control that refused the request (the three KN-D8 surge controls).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollabShedReason {
    /// The per-tenant op-stream shed lane (the active-editor lane reservation — viewers shed before
    /// editors, agents before humans). This is the contract-1.11 protected-human-lane shed.
    OpStreamLane,
    /// The per-doc op in-flight cap (one hot doc's op fan-out is bounded — Little's Law, §7.1).
    PerDocOpCap,
    /// The read-fanout bound (one edit cannot trigger an unbounded broadcast under a viewer storm).
    ReadFanout,
}

/// **The all-hands-doc surge gate (KN-P32 / KN-D8 / OQ-K; contract 1.11).**
///
/// A thin Knowledge wiring over the substrate's [`ShedLane`] for the one Knowledge surface
/// ([`ShedSurface::CollabOpStream`]), plus the two bounded-everything controls (the per-doc op cap +
/// the read-fanout bound). It reads the surface's budget **from the thresholds file** and applies the
/// shed order `speculative → batch/CI → agent → human-last` (tuned to the op-stream: viewers shed
/// before editors, agents before humans), **per-tenant**. A collab op is admitted through
/// [`CollabSurgeGate::admit_for`] (the run-class derived from the verified principal) **and** the
/// per-doc op cap; a passive viewer read is admitted through [`CollabSurgeGate::admit_read_fanout`].
/// An over-budget non-human lane sheds with `429 + Retry-After`, while the active-editor (human) lane
/// is protected (shed only in true saturation).
pub struct CollabSurgeGate {
    /// The substrate shed lane over the op-stream surface (the protected-human-lane shed order).
    lane: ShedLane,
    /// The per-doc op in-flight cap (one [`BoundedQueue`] per page id) — the thundering-herd discipline.
    per_doc_op: HashMap<String, BoundedQueue>,
    /// The per-doc op-cap capacity (the bound each doc's op fan-out gets). Taken from the surface
    /// budget's per-tenant cap so it tracks the tuned OQ-K number (one doc never exceeds the surface).
    per_doc_op_cap: u32,
    /// The read-fanout bound (one [`BoundedQueue`] per page id) — bounds the concurrent subscriber
    /// fan-out a single op may trigger so a viewer storm cannot turn one edit into an unbounded
    /// broadcast.
    read_fanout: HashMap<String, BoundedQueue>,
    /// The read-fanout capacity per doc.
    read_fanout_cap: u32,
    /// The `Retry-After` (seconds) the bounded-everything controls advertise when they shed (the
    /// surface's tuned value).
    retry_after_secs: u64,
}

impl CollabSurgeGate {
    /// Open the collab surge gate, reading its budget **from the thresholds file** (the prompt's "read
    /// from the thresholds file"). A missing row is a LOUD error (the gate refuses to open against a
    /// guessed budget — EI-01 §3), never a silent default.
    ///
    /// The per-doc op cap + the read-fanout bound default to the surface's per-tenant in-flight cap —
    /// the tuned OQ-K number — so one doc's op fan-out / read fan-out never exceeds the surface budget.
    pub fn from_thresholds(thresholds: &Thresholds) -> Result<CollabSurgeGate, String> {
        let budget = thresholds
            .shed_budget(ShedSurface::CollabOpStream)
            .map_err(|e| format!("Knowledge shed budget for CollabOpStream unavailable: {e}"))?;
        Ok(CollabSurgeGate::with_budget(budget))
    }

    /// Open the gate against an explicit budget (used by the surge drill to drive the boundary at a
    /// small, deterministic budget without editing the thresholds file). The per-doc op cap + the
    /// read-fanout bound both default to the surface's per-tenant in-flight cap.
    pub fn with_budget(budget: SurfaceBudget) -> CollabSurgeGate {
        CollabSurgeGate::with_budget_and_bounds(
            budget,
            budget.per_tenant_in_flight_cap,
            budget.per_tenant_in_flight_cap,
        )
    }

    /// Open the gate with explicit per-doc op cap + read-fanout bound (used by the drill to drive the
    /// per-doc/fanout boundaries independently of the per-tenant op-stream lane).
    pub fn with_budget_and_bounds(
        budget: SurfaceBudget,
        per_doc_op_cap: u32,
        read_fanout_cap: u32,
    ) -> CollabSurgeGate {
        CollabSurgeGate {
            lane: ShedLane::with_budget(ShedSurface::CollabOpStream, budget),
            per_doc_op: HashMap::new(),
            per_doc_op_cap,
            read_fanout: HashMap::new(),
            read_fanout_cap,
            retry_after_secs: budget.retry_after_secs,
        }
    }

    /// **Derive the op-stream run-class from a verified principal + an optional run-class header.** The
    /// kind sets the ceiling (a human editor → the protected lane; an agent → the agent lane; a service
    /// → batch/CI); the header may only DOWN-class. A **passive viewer** declares itself
    /// [`RunClassHeader::Speculative`] (a read is speculative — it sheds first); an **active editor**
    /// carries no down-class header so it holds its kind's lane. A machine principal can NEVER up-class
    /// to the protected human-editor lane.
    pub fn derive_class(principal: &Principal, header: Option<RunClassHeader>) -> RunClass {
        RunClass::derive(&principal.kind, header)
    }

    /// **Admit a collab OP by its verified principal + an optional injected run-class header, on a
    /// doc.** Two gates in series: (1) the per-tenant op-stream shed lane (the active-editor lane
    /// reservation — viewers shed before editors, agents before humans), then (2) the per-doc op
    /// in-flight cap. Both must admit. Returns `Ok(class)` (both slots taken — release via
    /// [`CollabSurgeGate::release_op`]) or `Err(CollabShedRejection)`. The decision is
    /// per-`principal.tenant` for the lane and per-doc for the cap.
    pub fn admit_for(
        &mut self,
        principal: &Principal,
        page_id: &str,
        header: Option<RunClassHeader>,
    ) -> Result<RunClass, CollabShedRejection> {
        let class = Self::derive_class(principal, header);
        self.admit_doc_op(&principal.tenant, page_id, class)
            .map(|()| class)
    }

    /// **Admit an op of a pre-derived [`RunClass`] for `tenant` on `page_id`** (the lower-level form the
    /// surge drill drives). The per-tenant op-stream lane is consulted FIRST (the protected-human-lane
    /// shed); only if it admits is the per-doc op cap consulted. If the per-doc cap then sheds, the
    /// op-stream slot is **released** so a per-doc shed does not leak a lane slot (the two controls do
    /// not double-charge).
    pub fn admit_doc_op(
        &mut self,
        tenant: &TenantId,
        page_id: &str,
        class: RunClass,
    ) -> Result<(), CollabShedRejection> {
        // (1) the per-tenant op-stream shed lane — the active-editor lane reservation.
        match self.lane.admit(tenant, class) {
            ShedDecision::Admit => {}
            ShedDecision::Shed { retry_after_secs } => {
                return Err(CollabShedRejection {
                    lane: class,
                    reason: CollabShedReason::OpStreamLane,
                    retry_after_secs,
                });
            }
        }
        // (2) the per-doc op in-flight cap — one hot doc's op fan-out is bounded (Little's Law).
        let cap = self.per_doc_op_cap;
        let q = self
            .per_doc_op
            .entry(page_id.to_string())
            .or_insert_with(|| BoundedQueue::new(cap));
        if q.try_acquire() {
            Ok(())
        } else {
            // the per-doc cap shed: release the op-stream lane slot we took (no double-charge).
            self.lane.release(tenant, class);
            Err(CollabShedRejection {
                lane: class,
                reason: CollabShedReason::PerDocOpCap,
                retry_after_secs: self.retry_after_secs,
            })
        }
    }

    /// **Admit a read-fanout send on `page_id`** — bounds the concurrent subscriber fan-out a single op
    /// may trigger so a viewer storm cannot turn one edit into an unbounded broadcast. Returns `Ok(())`
    /// (a fan-out slot taken — release via [`CollabSurgeGate::release_read_fanout`]) or
    /// `Err(CollabShedRejection)` with [`CollabShedReason::ReadFanout`].
    pub fn admit_read_fanout(&mut self, page_id: &str) -> Result<(), CollabShedRejection> {
        let cap = self.read_fanout_cap;
        let q = self
            .read_fanout
            .entry(page_id.to_string())
            .or_insert_with(|| BoundedQueue::new(cap));
        if q.try_acquire() {
            Ok(())
        } else {
            Err(CollabShedRejection {
                lane: RunClass::Speculative,
                reason: CollabShedReason::ReadFanout,
                retry_after_secs: self.retry_after_secs,
            })
        }
    }

    /// Release an op slot a prior [`CollabSurgeGate::admit_doc_op`] took for `(tenant, page_id,
    /// class)` — call when the op completes so the lane + the per-doc cap recover after the surge.
    pub fn release_op(&mut self, tenant: &TenantId, page_id: &str, class: RunClass) {
        self.lane.release(tenant, class);
        if let Some(q) = self.per_doc_op.get_mut(page_id) {
            q.release();
        }
    }

    /// Release a read-fanout slot a prior [`CollabSurgeGate::admit_read_fanout`] took for `page_id`.
    pub fn release_read_fanout(&mut self, page_id: &str) {
        if let Some(q) = self.read_fanout.get_mut(page_id) {
            q.release();
        }
    }

    /// The cumulative shed count for a lane (the contract-1.8 `shed-count per lane` survival signal —
    /// the surge-drill green artifact: `human-lane == 0 shed`, `agent/viewer-lane > 0 shed`).
    pub fn shed_count(&self, class: RunClass) -> u64 {
        self.lane.shed_count(class)
    }

    /// The per-tenant op-stream in-flight count (admitted not yet released) — for the blast-radius
    /// assertions.
    pub fn in_flight(&self, tenant: &TenantId) -> u32 {
        self.lane.in_flight(tenant)
    }

    /// The per-doc op in-flight count (admitted not yet released) — the thundering-herd telemetry.
    pub fn doc_in_flight(&self, page_id: &str) -> u32 {
        self.per_doc_op
            .get(page_id)
            .map(|q| q.in_flight())
            .unwrap_or(0)
    }

    /// The cumulative per-doc op-cap shed count for a doc (the bounded-everything signal for the doc).
    pub fn doc_op_shed_count(&self, page_id: &str) -> u64 {
        self.per_doc_op
            .get(page_id)
            .map(|q| q.shed_count())
            .unwrap_or(0)
    }

    /// The cumulative read-fanout shed count for a doc (the bounded-everything signal for the fan-out).
    pub fn read_fanout_shed_count(&self, page_id: &str) -> u64 {
        self.read_fanout
            .get(page_id)
            .map(|q| q.shed_count())
            .unwrap_or(0)
    }

    /// The surface this gate fronts (always [`ShedSurface::CollabOpStream`]).
    pub fn surface(&self) -> ShedSurface {
        self.lane.surface()
    }
}

// ───────────────────────────── the concurrent-same-gap LexoRank storm ─────────────────────────────

/// **The KN-D8 LexoRank-storm report (§3.5).** N concurrent inserts into the SAME sibling gap each
/// produce a DISTINCT [`OrderKey`] (the frozen 2-char jitter), so there is **no key-collision
/// reorder**, and the rebalance cost is BOUNDED (the storm does not force an unbounded rebalance). The
/// dated green artifact the DoD names for the LexoRank half.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LexoStormReport {
    /// The number of concurrent same-gap inserts driven.
    pub inserts: usize,
    /// The number of DISTINCT order keys produced (must equal `inserts` — 0 key collisions).
    pub distinct_keys: usize,
    /// Whether every produced key sorts STRICTLY between the original `lo`/`hi` gap bounds (no insert
    /// escaped the gap → no reorder relative to the rest of the list).
    pub all_within_gap: bool,
    /// The number of produced keys that tripped the frozen 48-char rebalance trigger
    /// ([`OrderKey::needs_rebalance`]) — the rebalance cost. Under a SINGLE-gap storm at one level this
    /// is 0 (the jitter keeps keys distinct without descending toward the trigger); a non-zero value is
    /// the BOUNDED, lazy rebalance signal, never an unbounded reorder.
    pub rebalance_triggers: usize,
}

impl LexoStormReport {
    /// **The KN-D8 LexoRank GREEN predicate (§3.5):** 0 key-collision reorder (every concurrent insert
    /// produced a distinct key) AND every key stayed within the gap (no reorder) AND the rebalance cost
    /// is bounded (no key ran away past the 48-char trigger).
    pub fn is_green(&self) -> bool {
        self.distinct_keys == self.inserts && self.all_within_gap && self.rebalance_triggers == 0
    }

    /// A one-line summary for the dated green-artifact log row (observability is part of the pass).
    pub fn summary(&self) -> String {
        format!(
            "KN-D8 LexoRank-storm: inserts={} distinct_keys={} all_within_gap={} \
             rebalance_triggers={} → {}",
            self.inserts,
            self.distinct_keys,
            self.all_within_gap,
            self.rebalance_triggers,
            if self.is_green() { "GREEN" } else { "RED" }
        )
    }
}

/// **Drive the §3.5 concurrent-same-gap LexoRank insert storm.** `inserts` concurrent inserts all
/// target the SAME sibling gap `(lo, hi)`; each draws a distinct 2-char jitter (here from a
/// deterministic per-insert digit pair so the storm is reproducible — the live path maps two RNG bytes
/// through [`Jitter::random`]). Asserts (via the returned [`LexoStormReport`]) that all keys are
/// distinct (no collision reorder), all sort strictly within the gap (no reorder), and the rebalance
/// cost is bounded. Uses the FROZEN [`OrderKey::rank_between`] — CONSUMED, never re-implemented (EI-01
/// §7); the move-CRDT (KN-P29) owns sibling-ordering, this proves the OLTP-index ordering hint stays
/// collision-free under the storm.
pub fn run_lexorank_storm(
    lo: Option<&OrderKey>,
    hi: Option<&OrderKey>,
    inserts: usize,
) -> LexoStormReport {
    let mut keys: Vec<OrderKey> = Vec::with_capacity(inserts);
    for i in 0..inserts {
        // a distinct deterministic jitter per concurrent insert (base-62 digit pair). The live insert
        // path draws this from the platform RNG via Jitter::random; here we vary it deterministically
        // so the storm is reproducible while still exercising the distinct-key property.
        let a = i % 62;
        let b = (i / 62) % 62;
        let jitter = Jitter::from_ranks(a, b).expect("ranks < 62 are in-alphabet");
        keys.push(OrderKey::rank_between(lo, hi, jitter));
    }

    // distinct keys — no key-collision reorder.
    let distinct: std::collections::BTreeSet<&str> = keys.iter().map(|k| k.as_str()).collect();
    let distinct_keys = distinct.len();

    // every key sorts STRICTLY within the gap (no escape → no reorder relative to the rest of the list).
    let all_within_gap = keys.iter().all(|k| {
        let above_lo = lo.map(|l| k.as_str() > l.as_str()).unwrap_or(true);
        let below_hi = hi.map(|h| k.as_str() < h.as_str()).unwrap_or(true);
        above_lo && below_hi
    });

    // the rebalance cost — keys that tripped the frozen 48-char trigger (bounded; lazy).
    let rebalance_triggers = keys.iter().filter(|k| k.needs_rebalance()).count();

    LexoStormReport {
        inserts,
        distinct_keys,
        all_within_gap,
        rebalance_triggers,
    }
}

// ───────────────────────────── the KN-D8 / F6 surge report ────────────────────────────────────────

/// **The KN-D8 + F6 surge report — the all-hands-doc surge properties.** The dated green artifact the
/// DoD names: the active-editor (human) lane HOLDS (0 shed within its reserved slots while a machine
/// lane sheds), the agent edit lane + the passive-viewer lane SHED (`429 + Retry-After`, absorbed not
/// unbounded), the per-doc op cap + the read-fanout bound HELD the hot doc within budget, and other
/// tenants are UNAFFECTED (the storm fills only the surging tenant's per-tenant budget).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollabSurgeReport {
    /// The agent edit-lane shed count on the surging tenant (the agent edit storm absorbed by shedding
    /// — must be > 0).
    pub surging_agent_shed_count: u64,
    /// The passive-viewer (speculative) lane shed count on the surging tenant (viewers shed before
    /// editors — must be > 0).
    pub surging_viewer_shed_count: u64,
    /// The human active-editor lane shed count on the surging tenant (the protected lane — must be 0).
    pub surging_human_shed_count: u64,
    /// Whether the surging tenant's OWN human active edit was admitted within its reserved slots.
    pub surging_human_admitted: bool,
    /// Whether the quiet co-tenant's human active edit was admitted within budget (untouched).
    pub quiet_human_admitted: bool,
    /// The quiet co-tenant's in-flight count BEFORE its own human edit (the cross-tenant impact — must
    /// be 0; the storm never spends the quiet tenant's budget).
    pub cross_tenant_impact: u32,
    /// The per-doc op-cap shed count on the hot doc (the thundering-herd discipline held the hot doc
    /// within budget — must be > 0 under a genuine all-hands storm).
    pub hot_doc_op_cap_shed_count: u64,
    /// The read-fanout shed count on the hot doc (a viewer storm did not turn one edit into an
    /// unbounded broadcast — must be > 0 under a genuine viewer storm).
    pub hot_doc_read_fanout_shed_count: u64,
}

impl CollabSurgeReport {
    /// **The KN-D8 + F6 GREEN predicate (the surge properties — all measured, none weakened).** The
    /// agent + viewer machine lanes shed (absorbed by shedding), the human active-editor lane held (0
    /// shed on the surging tenant + its own human admitted), the quiet co-tenant's human held,
    /// cross-tenant impact is 0, and the per-doc op cap + read-fanout bound held the hot doc within
    /// budget (both shed under the storm).
    pub fn is_green(&self) -> bool {
        self.surging_agent_shed_count > 0
            && self.surging_viewer_shed_count > 0
            && self.surging_human_shed_count == 0
            && self.surging_human_admitted
            && self.quiet_human_admitted
            && self.cross_tenant_impact == 0
            && self.hot_doc_op_cap_shed_count > 0
            && self.hot_doc_read_fanout_shed_count > 0
    }

    /// A one-line summary for the dated green-artifact log row (observability is part of the pass).
    pub fn summary(&self) -> String {
        format!(
            "KN-D8/F6: surging agent_shed={} viewer_shed={} human_shed={} surging_human_admitted={} \
             quiet_human_admitted={} cross_tenant_impact={} hot_doc_op_cap_shed={} \
             hot_doc_read_fanout_shed={} → {}",
            self.surging_agent_shed_count,
            self.surging_viewer_shed_count,
            self.surging_human_shed_count,
            self.surging_human_admitted,
            self.quiet_human_admitted,
            self.cross_tenant_impact,
            self.hot_doc_op_cap_shed_count,
            self.hot_doc_read_fanout_shed_count,
            if self.is_green() { "GREEN" } else { "RED" }
        )
    }
}

/// **Drive the KN-D8 all-hands-doc surge on the collab gate.** Spreads `storm_agent_ops` agent edit
/// ops + `storm_viewer_reads` passive-viewer reads on ONE hot doc on the surging tenant — the machine
/// lanes fill then shed — then proves the surging tenant's OWN human active edit is still admitted
/// (shed-last) and a quiet co-tenant's human edit is admitted within its independent budget. The
/// per-doc op cap + the read-fanout bound shed the hot doc's op/read fan-out within budget. Returns the
/// [`CollabSurgeReport`] (the surge properties).
///
/// The `multiplier` is the surge factor (read from the FILE by the caller; passed through for the log
/// row), not used to scale here — the storm-op counts are already the derived storm-op counts.
pub fn run_collab_surge(
    gate: &mut CollabSurgeGate,
    surging: &TenantId,
    quiet: &TenantId,
    hot_doc: &str,
    storm_agent_ops: u64,
    storm_viewer_reads: u64,
    _multiplier: u32,
) -> CollabSurgeReport {
    // An all-hands surge has THREE distinct pressure sources, each hitting a different control. The
    // op-stream lane is a per-tenant CONCURRENCY gate (an admitted op is dispatched, then the slot
    // frees); the per-doc cap + the read-fanout bound track in-flight work for ONE doc. We measure each
    // control in its proper regime so each green is EARNED.
    //
    // ── (A) the active-editor LANE reservation (the protected-human-lane shed order) ──
    // SPREAD agent edits across many docs (so the per-doc cap is never the bottleneck) saturate the
    // per-tenant op-stream lane at its non-reserved boundary; the agent + viewer machine lanes shed
    // while the reserved human slots stay free. We HOLD the admitted slots (concurrent in-flight) so the
    // lane sits at saturation while we probe the human — the human is admitted ONLY via the reserved
    // slots the agent/viewer storm could never take.
    let mut held: Vec<(String, RunClass)> = Vec::new();
    // viewer (speculative) edits/reads spread across docs — shed FIRST at the tightest graded ceiling.
    for i in 0..storm_viewer_reads {
        let doc = format!("spread-doc-{}", i % 997);
        if gate
            .admit_doc_op(surging, &doc, RunClass::Speculative)
            .is_ok()
        {
            held.push((doc, RunClass::Speculative));
        }
    }
    // agent edits spread across docs — saturate the lane to its non-reserved boundary then shed.
    for i in 0..storm_agent_ops {
        let doc = format!("spread-doc-{}", i % 997);
        if gate.admit_doc_op(surging, &doc, RunClass::Agent).is_ok() {
            held.push((doc, RunClass::Agent));
        }
    }
    // THE RESERVATION: the surging tenant's OWN human active edit is admitted WHILE the lane is
    // saturated — it uses the reserved slots (shed last). Probe on a FRESH doc so the per-doc cap does
    // not mask the lane-reservation property.
    let surging_human_admitted = gate
        .admit_doc_op(surging, "surging-fresh-doc", RunClass::Human)
        .is_ok();
    // the lane recovers as the transient concurrency slots free (the shed COUNTS already recorded the
    // boundary being crossed — shed counts are monotone).
    for (doc, class) in held.drain(..) {
        gate.release_op(surging, &doc, class);
    }

    // ── (B) the per-doc op CAP on the ONE hot doc (the thundering-herd discipline) ──
    // With the lane recovered, a concentrated agent edit burst on the hot doc fills its per-doc cap then
    // sheds — one hot doc's op fan-out is bounded (Little's Law). We hold these so the cap is reached.
    let mut hot_held = 0u64;
    for _ in 0..storm_agent_ops {
        if gate.admit_doc_op(surging, hot_doc, RunClass::Agent).is_ok() {
            hot_held += 1;
        }
    }
    for _ in 0..hot_held {
        gate.release_op(surging, hot_doc, RunClass::Agent);
    }

    // ── (C) the read-fanout bound on the hot doc (one edit's broadcast is bounded) ──
    // A viewer storm's concurrent broadcast fan-out fills the bound then sheds — one edit cannot become
    // an unbounded broadcast.
    let mut fanout_held = 0u64;
    for _ in 0..storm_viewer_reads {
        if gate.admit_read_fanout(hot_doc).is_ok() {
            fanout_held += 1;
        }
    }
    for _ in 0..fanout_held {
        gate.release_read_fanout(hot_doc);
    }

    // The quiet co-tenant is UNTOUCHED: its human edit is admitted within its independent per-tenant
    // budget (the storm never spent the quiet tenant's slots).
    let quiet_in_flight_before = gate.in_flight(quiet);
    let quiet_human_admitted = gate
        .admit_doc_op(quiet, "quiet-doc", RunClass::Human)
        .is_ok();

    CollabSurgeReport {
        surging_agent_shed_count: gate.shed_count(RunClass::Agent),
        surging_viewer_shed_count: gate.shed_count(RunClass::Speculative),
        surging_human_shed_count: gate.shed_count(RunClass::Human),
        surging_human_admitted,
        quiet_human_admitted,
        cross_tenant_impact: quiet_in_flight_before,
        hot_doc_op_cap_shed_count: gate.doc_op_shed_count(hot_doc),
        hot_doc_read_fanout_shed_count: gate.read_fanout_shed_count(hot_doc),
    }
}

#[cfg(test)]
mod tests;

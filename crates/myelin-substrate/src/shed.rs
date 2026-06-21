//! # The protected-human-lane shed order + bounded-everything (P-S19 → global P-035)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §7.1 (bounded everything — every queue/pool bounded; an unbounded queue = unbounded latency =
//! indistinguishable from down, Little's Law), §7.2 (the principal-aware limiter + protected human
//! lane — shed order `speculative → batch/CI → agent → human-last`; `429 + Retry-After`; per-tenant
//! so one tenant's surge does not shed another's humans), §7.3 (why this order — promise strength),
//! §7.6 (the per-surface shed-budget v1 floor table).
//!
//! **Contract-index:** row 1.11 (protected-human-lane shed order + per-surface shed budgets) — OWNED
//! here. Row 1.8 (the telemetry signal set) — the `shed-counts per lane` + per-surface budget
//! producer slice is exported here.
//!
//! ## What this module is (the substrate's stake)
//! Under saturation a public surface must shed **the right work first** and shed the interactive
//! human **last** — and it must do so **per-tenant** (EI-02 §1 / EI-01 §2 blast-radius: one tenant's
//! surge may never shed another tenant's humans). Raw FIFO would starve a human behind an agent burst
//! (§7.3); we adopt the doctrine order verbatim. This module ships three things the prompt names:
//!
//! - **(a) the principal-aware shed lane** — [`ShedLane`] reads the run-class (derived from the
//!   injected `Principal.kind` + an optional run-class header, [`RunClass::derive`]) and applies the
//!   [shed order](RunClass) with `429 + Retry-After` ([`ShedDecision::Shed`]), per-tenant.
//! - **(b) bounded everything** — [`BoundedQueue`] is the one bounded-queue/pool primitive every
//!   queue/pool the substrate names is built from (consumer prefetch, the DB pool, the bulkhead per
//!   target, per-tenant in-flight work, the HTTP intake queue): it **fast-fails (sheds)** when full
//!   rather than growing latency unboundedly (§7.1, Little's Law).
//! - **(c) the §7.6 per-surface shed-budget v1 floor table** — [`ShedBudgetTable`] (named floors).
//!
//! ## Floors named (deferred bodies → filling prompt)
//! - **The shed-budget NUMBERS** in [`ShedBudgetTable::v1_floor`] are the **M0 v1 floor** — the
//!   discipline (every surface bounded, a reserved human lane, the shed order applied) is the
//!   contract; the numbers are **tuned by the surge/latency drills (SUB-D3 / connection-storm) in
//!   M5, P-S33**. They are named floors, not claimed-final.
//! - **The agent-load caps + the SUB-D8 causal-loop guard** (the bounded dispatch pool / causal-depth
//!   ceiling / shared-root tripwire / bounded predicate guard) land in **P-S20 (P-036)**. This
//!   module ships the agent *lane* of the shed order (agents shed before humans); the agent-fan-out
//!   structural caps ride P-S20.
//! - **The real gateway/listener wiring** that calls [`ShedLane::admit`] at the public surface (and
//!   issues the literal HTTP `429` + `Retry-After` header) lands with the real transport
//!   (P-S13/P-S14+). Here the decision is a typed value the gateway maps to a `429`.

use myelin_identity::PrincipalKind;
use myelin_tenancy::TenantId;
use std::collections::HashMap;

/// **The shed run-class — the shed priority order (architecture §7.2/§7.3, contract 1.11).**
///
/// The order encodes *promise strength* (§7.3): speculative made no promise (shed first); batch/CI
/// and agents are machine clients that can and should back off; the **human** is the interactive
/// principal the product exists for (shed **last**, protected). The variants are declared in shed
/// order; the derived `Ord`/discriminant is therefore the shed priority — a LOWER class sheds FIRST.
///
/// **Kind is data, not a code branch** (the §3 `Principal` doctrine): the run-class is *derived*
/// from `Principal.kind` + the injected run-class header ([`RunClass::derive`]); the shed lane never
/// special-cases a kind in its control flow beyond reading this single ordinal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RunClass {
    /// Made no promise — dropped FIRST under pressure (prefetch/warm/speculative reads).
    Speculative,
    /// Batch + CI: machine clients that can and should back off (`429 + Retry-After`).
    BatchCi,
    /// Agent runs: machine clients; shed AFTER batch/CI but BEFORE humans (§7.2).
    Agent,
    /// The interactive human — shed **LAST**, and only in true saturation (the protected lane).
    Human,
}

/// The run-class header a request may carry (the §7.2 "the run's class from the injected headers").
/// `None` ⇒ derive purely from `Principal.kind`. A request can declare itself **speculative** or
/// **batch/CI** (down-classing itself so it sheds earlier — e.g. a warm/prefetch read), but it can
/// NEVER up-class to `Human`: the human lane is reserved for verified `PrincipalKind::Human`
/// traffic, so a machine principal cannot name itself a human to dodge the shed order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunClassHeader {
    /// The caller declares this request speculative (prefetch/warm) — shed first.
    Speculative,
    /// The caller declares this request batch/CI — a machine client that backs off.
    BatchCi,
}

impl RunClass {
    /// **Derive the run-class from the verified `Principal.kind` + the optional injected header**
    /// (architecture §7.2 — "reads `Principal.kind` + the run's class from the injected headers").
    ///
    /// The kind sets the *ceiling*; the header may only down-class (shed earlier), never up-class:
    /// - `Human`   → [`RunClass::Human`] (the protected lane) — a header may down-class it to
    ///   speculative/batch (a human-issued prefetch), but a `Human` is never *forced* lower without
    ///   its own say.
    /// - `Agent`   → [`RunClass::Agent`] (a header may down-class to speculative/batch).
    /// - `Service` → [`RunClass::BatchCi`] by default (a machine client; a header may down-class to
    ///   speculative).
    ///
    /// The up-class guard: a header is honoured only when it names a class **lower-or-equal** to the
    /// kind's ceiling. A `Service`/`Agent` cannot become `Human`; there is no `Human` header at all
    /// ([`RunClassHeader`] has no human variant), so the human lane is structurally unspoofable.
    pub fn derive(kind: &PrincipalKind, header: Option<RunClassHeader>) -> RunClass {
        let ceiling = match kind {
            PrincipalKind::Human => RunClass::Human,
            PrincipalKind::Agent { .. } => RunClass::Agent,
            PrincipalKind::Service => RunClass::BatchCi,
        };
        let requested = match header {
            None => ceiling,
            Some(RunClassHeader::Speculative) => RunClass::Speculative,
            Some(RunClassHeader::BatchCi) => RunClass::BatchCi,
        };
        // honour the header only if it down-classes (or equals) — never let it raise priority.
        requested.min(ceiling)
    }

    /// Stable lowercase label for the contract-1.8 `shed-count per lane` signal.
    pub fn lane(self) -> &'static str {
        match self {
            RunClass::Speculative => "speculative",
            RunClass::BatchCi => "batch_ci",
            RunClass::Agent => "agent",
            RunClass::Human => "human",
        }
    }
}

/// **The shed decision (contract 1.11).** Either the request is admitted, or it is shed with a
/// `429 + Retry-After` — the substrate's typed form of the wire response the gateway issues. Our own
/// clients honour the `Retry-After` (P-S17), so this is not a retry-storm amplifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShedDecision {
    /// Admitted — the request proceeds (a permit was taken; release it on completion).
    Admit,
    /// Shed — the surface is saturated and this lane is at/over its protected boundary. The gateway
    /// maps this to HTTP **`429 Too Many Requests` + `Retry-After: {retry_after_secs}`** (§7.2).
    Shed {
        /// The `Retry-After` value in **seconds** (the frozen unit, §2.10) — clients honour it
        /// (P-S17, the no-amplification guarantee).
        retry_after_secs: u64,
    },
}

impl ShedDecision {
    /// `true` iff the request was admitted.
    pub fn is_admitted(self) -> bool {
        matches!(self, ShedDecision::Admit)
    }
}

/// **A bounded queue / pool that fast-fails (architecture §7.1, contract 1.11).**
///
/// The ONE bounded-everything primitive: every queue/pool the substrate names — consumer prefetch,
/// the DB pool, the bulkhead per target, per-tenant in-flight work, the HTTP intake queue — is a
/// `BoundedQueue`. When full it **sheds** ([`BoundedQueue::try_acquire`] returns `false`) rather than
/// growing latency unboundedly: an unbounded queue is unbounded latency is indistinguishable from
/// down (Little's Law, §7.1). A permit is released by [`BoundedQueue::release`].
#[derive(Clone, Debug)]
pub struct BoundedQueue {
    in_flight: u32,
    capacity: u32,
    /// The cumulative shed count (the producer side of the contract-1.8 `shed-count` signal for this
    /// bounded surface). Monotone — proves the queue fast-failed rather than buffered.
    shed_count: u64,
}

impl BoundedQueue {
    /// A bounded queue of `capacity` permits. A real surface uses a positive capacity (its §7.6
    /// floor); `0` is accepted but is the degenerate "always-shed" (== down) queue, not a "bounded"
    /// one — callers take their capacity from [`ShedBudgetTable`], which is always positive.
    pub fn new(capacity: u32) -> BoundedQueue {
        BoundedQueue { in_flight: 0, capacity, shed_count: 0 }
    }

    /// Try to take a permit. Returns `true` if admitted (a permit was taken); `false` if the queue
    /// is full — in which case the call has **fast-failed (shed)**, NOT queued. The shed count is
    /// incremented on a `false` so the bounded-everything signal is observable.
    pub fn try_acquire(&mut self) -> bool {
        if self.in_flight < self.capacity {
            self.in_flight += 1;
            true
        } else {
            self.shed_count += 1;
            false
        }
    }

    /// Release a previously-acquired permit. Saturating at 0 — a double-release never wraps.
    pub fn release(&mut self) {
        self.in_flight = self.in_flight.saturating_sub(1);
    }

    /// The current in-flight count (permits taken and not yet released).
    pub fn in_flight(&self) -> u32 {
        self.in_flight
    }

    /// The bound (the §7.1 capacity).
    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// The cumulative shed count (the contract-1.8 producer signal for this bounded surface).
    pub fn shed_count(&self) -> u64 {
        self.shed_count
    }
}

/// **The §7.6 per-surface shed-budget v1 floor table (SHARPEN C-4 / OQ-K; contract 1.11).**
///
/// The discipline is the contract: every surface is bounded, has a reserved human-lane fraction, and
/// applies the shed order. The **numbers are named v1 floors** tuned by the drills (M5, P-S33) — they
/// are NOT claimed-final. Each row maps a [`Surface`] to its per-tenant in-flight cap and the
/// protected-human-lane reservation (the absolute number of slots, within the cap, reserved so a
/// human is never shed while a machine lane still occupies the surface).
#[derive(Clone, Debug)]
pub struct ShedBudgetTable {
    rows: HashMap<Surface, SurfaceBudget>,
}

/// One §7.6 surface row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceBudget {
    /// The per-tenant in-flight cap (the bound on concurrent admitted requests *for one tenant* on
    /// this surface) — the §7.1 bound, per-tenant so one tenant cannot starve another.
    pub per_tenant_in_flight_cap: u32,
    /// The protected-human-lane reservation: the number of in-flight slots (within the cap) reserved
    /// so non-human lanes are shed *before* a human is ever shed. A non-human request is shed once
    /// the tenant's non-reserved slots are full; a human request may use the reserved slots too.
    pub human_lane_reservation: u32,
    /// The `Retry-After` (seconds) the surface advertises when it sheds (§7.2). A named v1 floor.
    pub retry_after_secs: u64,
}

/// The §7.6 public surfaces that carry a per-surface shed budget. These are the four storm profiles
/// the table names (CI dispatch / collab op-stream / connection tier / agent-mention) plus the
/// generic HTTP intake every public surface has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Surface {
    /// CI dispatch — the CI-surge (30× agent) profile. CI is the batch lane (no human reservation).
    CiDispatch,
    /// Collab op-stream (Knowledge) — the hot-doc edit/read storm; a fraction reserved for active
    /// editors vs passive viewers.
    CollabOpStream,
    /// Connection tier (Chat) — the connection-storm; reserved connection slots for interactive
    /// humans, presence/speculative shed first.
    ConnectionTier,
    /// Agent-mention (Chat/all) — the agent-mention-storm; humans never queue behind agent runs.
    AgentMention,
    /// **The Git front door (clone/push)** — the clone-storm / hot-repo read profile (Git
    /// architecture `02 §6`, the OQ-K per-surface budget). A CI/agent clone storm sheds (`429 +
    /// Retry-After`) BEFORE a human's interactive fetch; the human lane is protected. This is the
    /// surface the Git front door's shed gate (GIT-P15) admits against, per-tenant. The CDN
    /// bundle-URI accelerated-clone (Git `02 §1.4`, Storage 11.2 C3) is the *complement* — it moves
    /// clone-storm read fan-out off serving compute so the budget is reached later.
    GitFrontDoor,
    /// The generic HTTP intake queue (§7.1) — every public surface's request intake.
    HttpIntake,
}

impl ShedBudgetTable {
    /// **The §7.6 v1 FLOOR table (named floors, tuned by drills in M5/P-S33).** These numbers are
    /// the M0 floor — small, conservative, and *deliberately* round; the surge/latency drills
    /// (SUB-D3, the connection-storm drill) measure the tuned values. Changing a number here is a
    /// floor-tuning change, not a contract change (the contract is "bounded + reserved lane + shed
    /// order", which is structural and tested).
    pub fn v1_floor() -> ShedBudgetTable {
        let mut rows = HashMap::new();
        // CI dispatch: CI is the batch lane (the §7.6 row says n/a human reservation) — no protected
        // human slots, CI + agent share the wallet.
        rows.insert(
            Surface::CiDispatch,
            SurfaceBudget { per_tenant_in_flight_cap: 64, human_lane_reservation: 0, retry_after_secs: 5 },
        );
        // Collab op-stream: a fraction reserved for active editors (the human lane).
        rows.insert(
            Surface::CollabOpStream,
            SurfaceBudget { per_tenant_in_flight_cap: 128, human_lane_reservation: 32, retry_after_secs: 2 },
        );
        // Connection tier: reserved connection slots for interactive humans.
        rows.insert(
            Surface::ConnectionTier,
            SurfaceBudget { per_tenant_in_flight_cap: 256, human_lane_reservation: 64, retry_after_secs: 3 },
        );
        // Agent-mention: humans never queue behind agent runs (a reserved human fraction).
        rows.insert(
            Surface::AgentMention,
            SurfaceBudget { per_tenant_in_flight_cap: 96, human_lane_reservation: 24, retry_after_secs: 10 },
        );
        // Git front door (clone/push): the clone-storm read profile. A CI/agent clone storm sheds
        // before a human's interactive fetch — a reserved human fraction protects the human lane.
        rows.insert(
            Surface::GitFrontDoor,
            SurfaceBudget { per_tenant_in_flight_cap: 128, human_lane_reservation: 32, retry_after_secs: 5 },
        );
        // The generic HTTP intake (§7.1): every public surface reserves a human fraction.
        rows.insert(
            Surface::HttpIntake,
            SurfaceBudget { per_tenant_in_flight_cap: 200, human_lane_reservation: 50, retry_after_secs: 5 },
        );
        ShedBudgetTable { rows }
    }

    /// The budget row for a surface (the §7.6 floor).
    pub fn budget(&self, surface: Surface) -> SurfaceBudget {
        self.rows[&surface]
    }

    /// The surfaces the table covers (all §7.6 rows + HTTP intake).
    pub fn surfaces(&self) -> impl Iterator<Item = Surface> + '_ {
        self.rows.keys().copied()
    }
}

/// **The principal-aware shed lane at one public surface (architecture §7.2; contract 1.11).**
///
/// Reserves a protected lane for interactive humans and applies the shed order
/// `speculative → batch/CI → agent → human-last`, **per-tenant**: in-flight work is counted per
/// `(tenant)`, so one tenant's surge fills only that tenant's budget and never sheds another
/// tenant's humans (EI-02 §1 / EI-01 §2 blast-radius).
///
/// The admission rule, given a surface budget `{ cap, reserved }` and the tenant's current
/// non-human in-flight `nh` (+ human in-flight `h`):
/// - A **human** ([`RunClass::Human`]) is admitted while `h + nh < cap` — it may use the reserved
///   slots, so it is shed ONLY in true saturation (every slot, reserved included, full). This is
///   "shed last".
/// - A **non-human** lane (speculative/batch/agent) is admitted only while `nh < cap - reserved`
///   AND `h + nh < cap` — i.e. it may never consume the reserved-for-humans slots. So under pressure
///   the non-human lanes are shed *first* (at `cap - reserved`), leaving the reserved slots for
///   humans. Among non-human lanes the shed order is enforced by [`ShedLane::admit`] consulting the
///   class ordinal: a lower-promise lane is shed at a *lower* fill than a higher one (a graded
///   threshold, so speculative sheds before batch/CI sheds before agent).
#[derive(Clone, Debug)]
pub struct ShedLane {
    surface: Surface,
    budget: SurfaceBudget,
    /// Per-tenant in-flight accounting: `(human_in_flight, non_human_in_flight)`. Per-tenant is the
    /// blast-radius guarantee — keyed by `TenantId`, so a surge on one tenant cannot evict another.
    tenants: HashMap<TenantId, TenantInFlight>,
    /// Per-lane cumulative shed counts (the contract-1.8 `shed-count per lane` producer signal).
    shed_counts: HashMap<RunClass, u64>,
}

#[derive(Clone, Copy, Debug, Default)]
struct TenantInFlight {
    human: u32,
    non_human: u32,
}

impl TenantInFlight {
    fn total(self) -> u32 {
        self.human + self.non_human
    }
}

impl ShedLane {
    /// Open a shed lane for a surface using the §7.6 v1 floor budget for that surface.
    pub fn new(surface: Surface) -> ShedLane {
        ShedLane::with_budget(surface, ShedBudgetTable::v1_floor().budget(surface))
    }

    /// Open a shed lane for a surface with an explicit budget (used by tests to drive the boundary).
    pub fn with_budget(surface: Surface, budget: SurfaceBudget) -> ShedLane {
        ShedLane { surface, budget, tenants: HashMap::new(), shed_counts: HashMap::new() }
    }

    /// **The admission decision (contract 1.11).** Reads the run-class (already derived from
    /// `Principal.kind` + header via [`RunClass::derive`]) and the tenant's current per-tenant
    /// in-flight, and returns [`ShedDecision::Admit`] (taking a slot) or [`ShedDecision::Shed`]
    /// (`429 + Retry-After`), applying the shed order with the human lane protected and per-tenant.
    pub fn admit(&mut self, tenant: &TenantId, class: RunClass) -> ShedDecision {
        let cap = self.budget.per_tenant_in_flight_cap;
        let reserved = self.budget.human_lane_reservation.min(cap);
        let cur = self.tenants.get(tenant).copied().unwrap_or_default();

        let admit = match class {
            // human: shed LAST — admitted while ANY slot (reserved included) is free.
            RunClass::Human => cur.total() < cap,
            // non-human lanes: may never take the reserved-for-human slots → shed at `cap - reserved`.
            // The shed order among non-human lanes is a GRADED threshold by promise strength: a
            // lower-promise lane is held to a tighter ceiling so it sheds FIRST.
            //   speculative : non_human < (cap - reserved) - 2*step
            //   batch/ci    : non_human < (cap - reserved) - 1*step
            //   agent       : non_human < (cap - reserved)
            // (step is a small fraction of the non-reserved budget; clamped so it never underflows).
            other => {
                let non_human_budget = cap.saturating_sub(reserved);
                let step = (non_human_budget / 8).max(1);
                let ceiling = match other {
                    RunClass::Speculative => non_human_budget.saturating_sub(2 * step),
                    RunClass::BatchCi => non_human_budget.saturating_sub(step),
                    RunClass::Agent => non_human_budget,
                    RunClass::Human => unreachable!("human handled above"),
                };
                cur.non_human < ceiling && cur.total() < cap
            }
        };

        if admit {
            let entry = self.tenants.entry(tenant.clone()).or_default();
            if class == RunClass::Human {
                entry.human += 1;
            } else {
                entry.non_human += 1;
            }
            ShedDecision::Admit
        } else {
            *self.shed_counts.entry(class).or_insert(0) += 1;
            ShedDecision::Shed { retry_after_secs: self.budget.retry_after_secs }
        }
    }

    /// Release a slot a prior [`ShedLane::admit`] took for `(tenant, class)`. Saturating — a stray
    /// release never wraps.
    pub fn release(&mut self, tenant: &TenantId, class: RunClass) {
        if let Some(entry) = self.tenants.get_mut(tenant) {
            if class == RunClass::Human {
                entry.human = entry.human.saturating_sub(1);
            } else {
                entry.non_human = entry.non_human.saturating_sub(1);
            }
        }
    }

    /// The cumulative shed count for a lane (the contract-1.8 `shed-count per lane` producer signal).
    pub fn shed_count(&self, class: RunClass) -> u64 {
        self.shed_counts.get(&class).copied().unwrap_or(0)
    }

    /// The total shed count across all lanes (the per-surface budget signal).
    pub fn total_shed_count(&self) -> u64 {
        self.shed_counts.values().sum()
    }

    /// The current per-tenant total in-flight (admitted not yet released) — for the per-tenant
    /// blast-radius assertions.
    pub fn in_flight(&self, tenant: &TenantId) -> u32 {
        self.tenants.get(tenant).copied().unwrap_or_default().total()
    }

    /// The surface this lane fronts.
    pub fn surface(&self) -> Surface {
        self.surface
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalKind, RuntimeRef};

    fn tenant(s: &str) -> TenantId {
        TenantId(s.to_string())
    }

    fn agent_kind() -> PrincipalKind {
        PrincipalKind::Agent {
            runtime_ref: RuntimeRef("rt".into()),
            on_behalf_of: None,
        }
    }

    // ---- RunClass::derive: kind sets the ceiling; a header may only down-class -------------------

    #[test]
    fn derive_maps_kind_to_lane_and_never_up_classes() {
        // human with no header → the protected human lane.
        assert_eq!(RunClass::derive(&PrincipalKind::Human, None), RunClass::Human);
        // agent → agent lane.
        assert_eq!(RunClass::derive(&agent_kind(), None), RunClass::Agent);
        // service → batch/ci by default.
        assert_eq!(RunClass::derive(&PrincipalKind::Service, None), RunClass::BatchCi);

        // a header may DOWN-class (a human-issued prefetch sheds early).
        assert_eq!(
            RunClass::derive(&PrincipalKind::Human, Some(RunClassHeader::Speculative)),
            RunClass::Speculative
        );
        assert_eq!(
            RunClass::derive(&PrincipalKind::Human, Some(RunClassHeader::BatchCi)),
            RunClass::BatchCi
        );

        // a header can NEVER up-class: a Service naming itself batch is still batch (no human header
        // exists at all → the human lane is structurally unspoofable).
        assert_eq!(
            RunClass::derive(&PrincipalKind::Service, Some(RunClassHeader::Speculative)),
            RunClass::Speculative
        );
        // an agent declaring batch/ci down-classes to batch (lower of agent vs batch is batch).
        assert_eq!(
            RunClass::derive(&agent_kind(), Some(RunClassHeader::BatchCi)),
            RunClass::BatchCi
        );
    }

    #[test]
    fn shed_priority_order_is_speculative_then_batch_then_agent_then_human() {
        // the declared variant order IS the shed order: a lower class sheds first.
        assert!(RunClass::Speculative < RunClass::BatchCi);
        assert!(RunClass::BatchCi < RunClass::Agent);
        assert!(RunClass::Agent < RunClass::Human);
    }

    // ---- the shed order: speculative → batch/CI → agent → human-last -----------------------------

    /// **Drives the surface to saturation and asserts the shed order fires in the right priority:**
    /// speculative sheds first, then batch/CI, then agent, and the human is admitted while a machine
    /// lane is already being shed (human shed last).
    #[test]
    fn shed_order_sheds_speculative_then_batch_ci_then_agent_then_human_last() {
        // a small budget so the graded thresholds are easy to reach. cap 10, reserve 4 for humans.
        let budget = SurfaceBudget {
            per_tenant_in_flight_cap: 10,
            human_lane_reservation: 4,
            retry_after_secs: 5,
        };
        let mut lane = ShedLane::with_budget(Surface::HttpIntake, budget);
        let t = tenant("acme");
        // non_human_budget = 6, step = max(6/8,1)=1 → speculative ceiling 4, batch 5, agent 6.

        // fill the non-human in-flight up to 4 with agent traffic (all admitted; under every ceiling).
        for _ in 0..4 {
            assert_eq!(lane.admit(&t, RunClass::Agent), ShedDecision::Admit);
        }
        // now non_human == 4: SPECULATIVE sheds first (ceiling 4, not < 4).
        assert!(matches!(lane.admit(&t, RunClass::Speculative), ShedDecision::Shed { .. }));
        // batch/ci still admitted (ceiling 5, 4 < 5).
        assert_eq!(lane.admit(&t, RunClass::BatchCi), ShedDecision::Admit); // non_human → 5
        // now non_human == 5: batch/ci sheds (ceiling 5, not < 5), agent still admitted (ceiling 6).
        assert!(matches!(lane.admit(&t, RunClass::BatchCi), ShedDecision::Shed { .. }));
        assert_eq!(lane.admit(&t, RunClass::Agent), ShedDecision::Admit); // non_human → 6
        // now non_human == 6 == cap-reserved: AGENT sheds (ceiling 6), but the HUMAN is still admitted
        // — the human lane is protected, shed LAST.
        assert!(matches!(lane.admit(&t, RunClass::Agent), ShedDecision::Shed { .. }));
        assert_eq!(lane.admit(&t, RunClass::Human), ShedDecision::Admit); // total 7, humans use reserved

        // the shed counts are exported per lane (contract-1.8) and follow the order.
        assert_eq!(lane.shed_count(RunClass::Speculative), 1);
        assert_eq!(lane.shed_count(RunClass::BatchCi), 1);
        assert_eq!(lane.shed_count(RunClass::Agent), 1);
        assert_eq!(lane.shed_count(RunClass::Human), 0, "the human lane has NOT been shed");
    }

    /// **Human shed last: only in true saturation (every slot, reserved included, full).**
    #[test]
    fn human_lane_is_shed_last_only_in_true_saturation() {
        let budget = SurfaceBudget {
            per_tenant_in_flight_cap: 5,
            human_lane_reservation: 2,
            retry_after_secs: 7,
        };
        let mut lane = ShedLane::with_budget(Surface::ConnectionTier, budget);
        let t = tenant("acme");
        // fill the non-reserved budget (3) with agents; agents then shed (cap-reserved = 3).
        for _ in 0..3 {
            assert_eq!(lane.admit(&t, RunClass::Agent), ShedDecision::Admit);
        }
        assert!(matches!(lane.admit(&t, RunClass::Agent), ShedDecision::Shed { .. }), "agent shed at cap-reserved");
        // humans keep being admitted into the reserved slots (total 3 → 4 → 5).
        assert_eq!(lane.admit(&t, RunClass::Human), ShedDecision::Admit);
        assert_eq!(lane.admit(&t, RunClass::Human), ShedDecision::Admit);
        // now total == cap == 5: TRUE saturation — even the human is shed (shed last, but it IS shed
        // when there is genuinely no slot left).
        match lane.admit(&t, RunClass::Human) {
            ShedDecision::Shed { retry_after_secs } => assert_eq!(retry_after_secs, 7),
            ShedDecision::Admit => panic!("a fully-saturated surface must shed even the human"),
        }
        assert_eq!(lane.shed_count(RunClass::Human), 1);
    }

    /// **Per-tenant: one tenant's surge does NOT shed another tenant's humans (the blast-radius
    /// guarantee, EI-02 §1 / EI-01 §2).**
    #[test]
    fn shedding_is_per_tenant_one_tenants_surge_never_sheds_anothers_human() {
        let budget = SurfaceBudget {
            per_tenant_in_flight_cap: 4,
            human_lane_reservation: 1,
            retry_after_secs: 3,
        };
        let mut lane = ShedLane::with_budget(Surface::HttpIntake, budget);
        let noisy = tenant("noisy");
        let quiet = tenant("quiet");

        // SATURATE the noisy tenant completely (cap 4): 3 non-human + push past, then a human fills
        // the reserved slot, then even noisy's human sheds.
        for _ in 0..3 {
            assert_eq!(lane.admit(&noisy, RunClass::Agent), ShedDecision::Admit);
        }
        assert!(matches!(lane.admit(&noisy, RunClass::Agent), ShedDecision::Shed { .. }));
        assert_eq!(lane.admit(&noisy, RunClass::Human), ShedDecision::Admit); // reserved slot, total 4
        assert!(matches!(lane.admit(&noisy, RunClass::Human), ShedDecision::Shed { .. }), "noisy saturated");

        // the QUIET tenant is COMPLETELY UNAFFECTED — its human is admitted, its budget untouched.
        assert_eq!(lane.in_flight(&quiet), 0, "the quiet tenant's budget is independent");
        assert_eq!(
            lane.admit(&quiet, RunClass::Human),
            ShedDecision::Admit,
            "the noisy tenant's surge must NEVER shed another tenant's human"
        );
        assert_eq!(lane.admit(&quiet, RunClass::Agent), ShedDecision::Admit);
    }

    /// Release frees a slot so the lane recovers after the surge passes.
    #[test]
    fn release_frees_a_slot() {
        let budget = SurfaceBudget {
            per_tenant_in_flight_cap: 3,
            human_lane_reservation: 1,
            retry_after_secs: 1,
        };
        let mut lane = ShedLane::with_budget(Surface::HttpIntake, budget);
        let t = tenant("acme");
        assert_eq!(lane.admit(&t, RunClass::Agent), ShedDecision::Admit);
        assert_eq!(lane.admit(&t, RunClass::Agent), ShedDecision::Admit); // non_human 2 == cap-reserved
        assert!(matches!(lane.admit(&t, RunClass::Agent), ShedDecision::Shed { .. }));
        lane.release(&t, RunClass::Agent);
        assert_eq!(lane.admit(&t, RunClass::Agent), ShedDecision::Admit, "a released slot is reusable");
    }

    // ---- bounded everything: every queue/pool fast-fails (never grows latency unboundedly) -------

    #[test]
    fn bounded_queue_fast_fails_rather_than_growing_unboundedly() {
        let mut q = BoundedQueue::new(2);
        assert!(q.try_acquire(), "first permit");
        assert!(q.try_acquire(), "second permit");
        // full → fast-fail (shed), NOT queue: in_flight does NOT grow past the bound.
        assert!(!q.try_acquire(), "a full bounded queue fast-fails (sheds), never grows latency");
        assert_eq!(q.in_flight(), 2, "in-flight never exceeds the bound (Little's Law)");
        assert_eq!(q.shed_count(), 1, "the shed is counted (the bounded-everything signal)");
        // releasing makes a slot reusable.
        q.release();
        assert!(q.try_acquire(), "a released slot is reusable");
        assert_eq!(q.in_flight(), 2);
    }

    #[test]
    fn bounded_queue_release_saturates_at_zero() {
        let mut q = BoundedQueue::new(1);
        q.release(); // stray release
        assert_eq!(q.in_flight(), 0, "a double/stray release never wraps");
        assert!(q.try_acquire());
    }

    // ---- the §7.6 per-surface shed-budget v1 floor table -----------------------------------------

    #[test]
    fn v1_floor_table_covers_every_surface_with_a_bounded_reserved_lane() {
        let table = ShedBudgetTable::v1_floor();
        for surface in [
            Surface::CiDispatch,
            Surface::CollabOpStream,
            Surface::ConnectionTier,
            Surface::AgentMention,
            Surface::GitFrontDoor,
            Surface::HttpIntake,
        ] {
            let b = table.budget(surface);
            // every surface is BOUNDED.
            assert!(b.per_tenant_in_flight_cap > 0, "{surface:?} must be bounded (§7.1)");
            // the reservation never exceeds the cap.
            assert!(
                b.human_lane_reservation <= b.per_tenant_in_flight_cap,
                "{surface:?} reservation within the cap"
            );
            // every surface advertises a Retry-After (clients honour it, P-S17).
            assert!(b.retry_after_secs > 0, "{surface:?} sheds with a Retry-After");
        }
        // CI is the batch lane: no human reservation (the §7.6 row says n/a).
        assert_eq!(table.budget(Surface::CiDispatch).human_lane_reservation, 0);
        // the human-facing surfaces DO reserve a human lane.
        assert!(table.budget(Surface::CollabOpStream).human_lane_reservation > 0);
        assert!(table.budget(Surface::ConnectionTier).human_lane_reservation > 0);
        assert!(table.budget(Surface::AgentMention).human_lane_reservation > 0);
        // The Git front door protects a human lane (a human's interactive fetch is shed last).
        assert!(table.budget(Surface::GitFrontDoor).human_lane_reservation > 0);
    }

    #[test]
    fn derive_then_admit_end_to_end_protects_the_human_over_an_agent_surge() {
        // the realistic path: derive the class from kind+header, then admit. An agent surge sheds;
        // the human (derived from PrincipalKind::Human) is admitted.
        let budget = SurfaceBudget {
            per_tenant_in_flight_cap: 4,
            human_lane_reservation: 2,
            retry_after_secs: 5,
        };
        let mut lane = ShedLane::with_budget(Surface::AgentMention, budget);
        let t = tenant("acme");
        let agent = agent_kind();
        // saturate the non-reserved budget (cap-reserved = 2) with agent runs.
        let c = RunClass::derive(&agent, None);
        assert_eq!(c, RunClass::Agent);
        assert_eq!(lane.admit(&t, c), ShedDecision::Admit);
        assert_eq!(lane.admit(&t, c), ShedDecision::Admit);
        assert!(matches!(lane.admit(&t, c), ShedDecision::Shed { .. }), "the agent lane sheds");
        // a human (derived) is still admitted — humans never queue behind agent runs (§7.6).
        let h = RunClass::derive(&PrincipalKind::Human, None);
        assert_eq!(lane.admit(&t, h), ShedDecision::Admit);
    }
}

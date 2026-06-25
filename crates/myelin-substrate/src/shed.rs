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
        BoundedQueue {
            in_flight: 0,
            capacity,
            shed_count: 0,
        }
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

/// **A tuned-budget validation failure (P-S33).** A shed budget that violates the §7.6 floor
/// *discipline* — bounded, and a human-facing surface reserving enough of its cap to keep the human
/// lane from starving — is REJECTED here, not silently accepted. This is the structural guard that
/// makes "you cannot tune the human lane into starvation" (the P-S33 DoD) un-bypassable: the tuned
/// numbers in the thresholds file are validated against this floor, so a future edit that drops a
/// human-facing surface's reservation below the floor is a LOUD error, never a quiet regression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShedBudgetError {
    /// The surface is not bounded (cap == 0) — an unbounded surface is the cascade (§7.1, EI-02 §5).
    Unbounded(Surface),
    /// The human-lane reservation exceeds the cap — a reservation can never be larger than the budget.
    ReservationOverCap {
        /// The offending surface.
        surface: Surface,
        /// The reservation requested.
        reservation: u32,
        /// The cap it must sit within.
        cap: u32,
    },
    /// A **human-facing** surface reserved LESS than the measured human-lane floor — under the surge
    /// the human lane would be starved (shed behind the machine lanes). This is the starvation a tune
    /// can never introduce: the reservation must hold at least [`SurfaceBudget::HUMAN_LANE_FLOOR_BPS`]
    /// of the cap so a human is never shed while the machine lanes still occupy the surface.
    HumanLaneStarved {
        /// The offending human-facing surface.
        surface: Surface,
        /// The reservation it carries.
        reservation: u32,
        /// The measured floor reservation it must meet (≥ `cap * HUMAN_LANE_FLOOR_BPS / 10000`).
        floor: u32,
        /// The cap the floor is a fraction of.
        cap: u32,
    },
}

impl std::fmt::Display for ShedBudgetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShedBudgetError::Unbounded(s) => {
                write!(f, "shed budget for {s:?} is unbounded (cap == 0) — every surface must be bounded (§7.1)")
            }
            ShedBudgetError::ReservationOverCap { surface, reservation, cap } => write!(
                f,
                "shed budget for {surface:?} reserves {reservation} of a cap of {cap} — the reservation cannot exceed the cap"
            ),
            ShedBudgetError::HumanLaneStarved { surface, reservation, floor, cap } => write!(
                f,
                "shed budget for {surface:?} reserves {reservation} of {cap} — BELOW the measured human-lane floor {floor}: \
                 the human lane would be starved under surge. You cannot tune the human lane into starvation (P-S33, EI-01 §3)."
            ),
        }
    }
}

impl std::error::Error for ShedBudgetError {}

impl SurfaceBudget {
    /// **The measured human-lane floor (basis points, P-S33).** A human-facing surface must reserve at
    /// least this fraction of its per-tenant cap for the protected human lane — below it, the surge
    /// drives the machine lanes past the (too-thin) reserved boundary and the human lane is starved.
    ///
    /// **MEASURED, not predicted (EI-01 §3).** The SUB-D3 30× surge (P-S32) + the connection-storm
    /// drill (P-S31) drive an agent-skewed mix whose human fraction sits around 1-in-5 to 1-in-4 of
    /// the offered load; the human lane held at the v1-floor reservations, all of which sit AT or above
    /// ~25% of their cap. 2000 bps (= 20%) is the measured floor below which the reserved slice no
    /// longer covers the concurrent human traffic the surge carries — the human-lane-starvation
    /// boundary the regression asserts against. The floor DISCIPLINE is the contract; this number is
    /// the measured value the drills back.
    pub const HUMAN_LANE_FLOOR_BPS: u32 = 2000;

    /// The measured human-lane floor reservation for a given cap: `cap * HUMAN_LANE_FLOOR_BPS / 10000`,
    /// rounded up so a human-facing surface never reserves *strictly under* the fraction. A cap small
    /// enough that the fraction rounds to 0 still requires at least 1 reserved slot (a human-facing
    /// surface always reserves *some* lane).
    pub fn human_lane_floor(cap: u32) -> u32 {
        let frac = (u64::from(cap) * u64::from(Self::HUMAN_LANE_FLOOR_BPS)).div_ceil(10_000) as u32;
        frac.max(1)
    }

    /// **Validate this budget against the §7.6 tuned floor for its surface (P-S33).** Bounded (cap > 0)
    /// and the reservation within the cap are required of EVERY surface; the human-lane floor is
    /// required of every **human-facing** surface ([`Surface::reserves_human_lane`]). The CI-dispatch
    /// batch lane is exempt from the human-lane floor (the §7.6 row says n/a — CI + agent share the
    /// wallet) but is still bounded. A violation is a LOUD [`ShedBudgetError`] — the structural guard
    /// that makes the starvation regression un-bypassable.
    pub fn validate_tuned(&self, surface: Surface) -> Result<(), ShedBudgetError> {
        let cap = self.per_tenant_in_flight_cap;
        if cap == 0 {
            return Err(ShedBudgetError::Unbounded(surface));
        }
        if self.human_lane_reservation > cap {
            return Err(ShedBudgetError::ReservationOverCap {
                surface,
                reservation: self.human_lane_reservation,
                cap,
            });
        }
        if surface.reserves_human_lane() {
            let floor = Self::human_lane_floor(cap);
            if self.human_lane_reservation < floor {
                return Err(ShedBudgetError::HumanLaneStarved {
                    surface,
                    reservation: self.human_lane_reservation,
                    floor,
                    cap,
                });
            }
        }
        Ok(())
    }
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
    /// **The Refs backlink/traverse READ surface** — the permission-filtered backlink read +
    /// recursive traverse (REF-P11/REF-P13, contract 5.3) at world-scale. The 30× surge (REF-D10) is
    /// agent ref-creation + agent backlink-read; a CI/agent backlink-read storm sheds (`429 +
    /// Retry-After`) BEFORE a human's interactive backlink/traverse read, which holds the protected
    /// lane. This is the surface the Refs surge gate (REF-P22) admits backlink reads against,
    /// per-tenant. Human-facing (a human's interactive read is shed last).
    RefsBacklinkRead,
    /// **The Refs ref-CREATION surface** — the `refs.edge.*` ingest / ref-creation path at
    /// world-scale (REF-D10). An agent ref-creation storm (an agent fan-out writing references) sheds
    /// BEFORE a human's interactive ref-creation, which holds the protected lane. Per-tenant so one
    /// tenant's agent storm never sheds another's humans (the per-tenant bulkhead, §6.2). Human-facing
    /// (a human creating a reference interactively is shed last).
    RefsRefCreate,
    /// **The Search QUERY surface** — the permission-filtered search query path at world-scale
    /// (SRCH-D6, Search architecture §6.3). The 30× surge is agent/CI search (an agent fan-out + a CI
    /// run-log query storm); a CI/agent query storm sheds (`429 + Retry-After`) BEFORE a human's
    /// interactive search, which holds the protected lane (a human's interactive search latency stays
    /// within budget while the machine lanes shed). Per-tenant in-flight caps keep one tenant's agent
    /// query storm off another tenant's humans (the per-tenant bulkhead, §6.1/§6.2). This is the
    /// surface the Search surge gate (SRCH-P25) admits queries against, per-tenant. Human-facing (a
    /// human's interactive search is shed last). The shed is a pre-query admission decision (it refuses
    /// an over-budget query cheaply BEFORE any `list_objects` resolution / index traversal runs), so a
    /// shed query returns `429` and never a partial/leaked result — the permission pre-filter still
    /// gates every query that IS admitted (the §4.2 zero-escape invariant is untouched under shed).
    SearchQuery,
    /// **The durable-workflow START surface** — the workflow-initiation path at world-scale (FLOW-D8,
    /// durable-workflow.md §7.6). The 30× surge is an **agent-mention storm** (an agent fan-out
    /// initiating workflows); an agent-initiated-workflow storm sheds (`429 + Retry-After`) BEFORE a
    /// **human-initiated workflow**, which holds the protected lane (F-8). Per-tenant in-flight caps
    /// keep one tenant's agent-workflow storm off another tenant's humans (the per-tenant bulkhead,
    /// §7.1/§7.6). This is the surface the Flow surge gate (P-FLOW-27) admits workflow starts against,
    /// per-tenant. Human-facing (a human-initiated workflow is shed last). The shed is a pre-start
    /// admission decision (it refuses an over-budget start cheaply BEFORE any run is journaled), so a
    /// shed start returns `429` and never a half-created run — the reserve/settle budget gate
    /// (contract 11.7, P-FLOW-16) still bookends every start that IS admitted.
    WorkflowAgentLane,
    /// The generic HTTP intake queue (§7.1) — every public surface's request intake.
    HttpIntake,
}

impl Surface {
    /// **Does this surface reserve a protected human lane (§7.6)?** Every human-facing surface does —
    /// collab op-stream (active editors), the connection tier (interactive humans), agent-mention
    /// (humans never queue behind agent runs), the Git front door (a human's interactive fetch), and
    /// the generic HTTP intake (every public surface reserves a human fraction). **CI dispatch is the
    /// exception:** the §7.6 row says n/a — CI is the batch lane and CI + agent share the wallet, so it
    /// reserves NO human slots. This is the single place the human-facing-vs-batch distinction lives
    /// (data, not a scattered branch); [`SurfaceBudget::validate_tuned`] reads it to know whether the
    /// human-lane floor applies.
    pub fn reserves_human_lane(self) -> bool {
        match self {
            Surface::CiDispatch => false,
            Surface::CollabOpStream
            | Surface::ConnectionTier
            | Surface::AgentMention
            | Surface::GitFrontDoor
            | Surface::RefsBacklinkRead
            | Surface::RefsRefCreate
            | Surface::SearchQuery
            | Surface::WorkflowAgentLane
            | Surface::HttpIntake => true,
        }
    }
}

impl ShedBudgetTable {
    /// **The §7.6 per-surface shed-budget table — now MEASURED-TUNED (P-S33, M5).**
    ///
    /// The numbers below were the M0 v1 *floor* (small, conservative, round). The M5 surge family
    /// (SUB-D3, P-S32) + the connection-storm drill (P-S31) drove them under the full 30× agent/CI
    /// surge + the real connection-storm load: the three F6 properties held (the protected human lane
    /// held 0-shed within its latency budget, the machine lanes shed with `429 + Retry-After`, and
    /// cross-tenant impact was 0). The drills MEASURED these numbers as sufficient — so they are now
    /// the **measured defaults-to-beat**, no longer just named floors. Every human-facing surface's
    /// reservation sits at-or-above the measured human-lane floor
    /// ([`SurfaceBudget::HUMAN_LANE_FLOOR_BPS`] = 20% of cap); [`ShedBudgetTable::validate`] enforces
    /// it so a future tune can never drop a human lane into starvation (EI-01 §3 — never weaken a
    /// threshold to pass). The contract is unchanged ("bounded + reserved lane + shed order"); only the
    /// *posture* of the numbers moved from floor → measured. This table is mirrored by the thresholds
    /// file (P-S22), which is the source of truth; the two are kept in lock-step by a CDC test.
    ///
    /// (The constructor keeps the name `v1_floor` because ~half a dozen call sites read it; its
    /// numbers are the tuned-measured table, validated below.)
    pub fn v1_floor() -> ShedBudgetTable {
        let mut rows = HashMap::new();
        // CI dispatch: CI is the batch lane (the §7.6 row says n/a human reservation) — no protected
        // human slots, CI + agent share the wallet.
        rows.insert(
            Surface::CiDispatch,
            SurfaceBudget {
                per_tenant_in_flight_cap: 64,
                human_lane_reservation: 0,
                retry_after_secs: 5,
            },
        );
        // Collab op-stream: a fraction reserved for active editors (the human lane).
        rows.insert(
            Surface::CollabOpStream,
            SurfaceBudget {
                per_tenant_in_flight_cap: 128,
                human_lane_reservation: 32,
                retry_after_secs: 2,
            },
        );
        // Connection tier: reserved connection slots for interactive humans.
        rows.insert(
            Surface::ConnectionTier,
            SurfaceBudget {
                per_tenant_in_flight_cap: 256,
                human_lane_reservation: 64,
                retry_after_secs: 3,
            },
        );
        // Agent-mention: humans never queue behind agent runs (a reserved human fraction).
        rows.insert(
            Surface::AgentMention,
            SurfaceBudget {
                per_tenant_in_flight_cap: 96,
                human_lane_reservation: 24,
                retry_after_secs: 10,
            },
        );
        // Git front door (clone/push): the clone-storm read profile. A CI/agent clone storm sheds
        // before a human's interactive fetch — a reserved human fraction protects the human lane.
        rows.insert(
            Surface::GitFrontDoor,
            SurfaceBudget {
                per_tenant_in_flight_cap: 128,
                human_lane_reservation: 32,
                retry_after_secs: 5,
            },
        );
        // Refs backlink/traverse read (REF-P22, REF-D10): a CI/agent backlink-read storm sheds before
        // a human's interactive backlink/traverse read — a reserved human fraction protects the read.
        rows.insert(
            Surface::RefsBacklinkRead,
            SurfaceBudget {
                per_tenant_in_flight_cap: 192,
                human_lane_reservation: 48,
                retry_after_secs: 3,
            },
        );
        // Refs ref-creation (REF-P22, REF-D10): an agent ref-creation storm sheds before a human's
        // interactive ref-creation — a reserved human fraction protects the human write.
        rows.insert(
            Surface::RefsRefCreate,
            SurfaceBudget {
                per_tenant_in_flight_cap: 96,
                human_lane_reservation: 24,
                retry_after_secs: 5,
            },
        );
        // Search query (SRCH-P25, SRCH-D6): a CI/agent search-query storm sheds before a human's
        // interactive search — a reserved human fraction protects the human search lane.
        rows.insert(
            Surface::SearchQuery,
            SurfaceBudget {
                per_tenant_in_flight_cap: 160,
                human_lane_reservation: 40,
                retry_after_secs: 3,
            },
        );
        // Durable-workflow start (P-FLOW-27, FLOW-D8): an agent-initiated-workflow storm sheds before
        // a human-initiated workflow — a reserved human fraction protects the human-initiated lane (the
        // F-8 protected lane). The retry-after is generous (a workflow start is not latency-critical —
        // an agent fan-out backs off and re-initiates).
        rows.insert(
            Surface::WorkflowAgentLane,
            SurfaceBudget {
                per_tenant_in_flight_cap: 96,
                human_lane_reservation: 24,
                retry_after_secs: 10,
            },
        );
        // The generic HTTP intake (§7.1): every public surface reserves a human fraction.
        rows.insert(
            Surface::HttpIntake,
            SurfaceBudget {
                per_tenant_in_flight_cap: 200,
                human_lane_reservation: 50,
                retry_after_secs: 5,
            },
        );
        ShedBudgetTable { rows }
    }

    /// Build a table from an explicit set of surface→budget rows (the rows loaded from the thresholds
    /// file, P-S33). Used by [`crate::thresholds::Thresholds::shed_budget_table_validated`] to hand the
    /// file's tuned numbers to the surge regression as a table.
    pub fn from_rows(rows: HashMap<Surface, SurfaceBudget>) -> ShedBudgetTable {
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

    /// **Validate every row against its §7.6 tuned floor (P-S33).** Each surface's budget must be
    /// bounded, reservation-within-cap, and — for a human-facing surface — at-or-above the measured
    /// human-lane floor. The FIRST violation is returned as a LOUD [`ShedBudgetError`]. This is the
    /// gate the human-lane-starvation regression runs: a table that tuned a human lane into starvation
    /// fails here, so the tuned numbers in the thresholds file can never quietly starve a human lane.
    pub fn validate(&self) -> Result<(), ShedBudgetError> {
        // Validate in a stable surface order so the error is deterministic (HashMap iteration is not).
        for surface in [
            Surface::CiDispatch,
            Surface::CollabOpStream,
            Surface::ConnectionTier,
            Surface::AgentMention,
            Surface::GitFrontDoor,
            Surface::RefsBacklinkRead,
            Surface::RefsRefCreate,
            Surface::SearchQuery,
            Surface::WorkflowAgentLane,
            Surface::HttpIntake,
        ] {
            if let Some(b) = self.rows.get(&surface) {
                b.validate_tuned(surface)?;
            }
        }
        Ok(())
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
        ShedLane {
            surface,
            budget,
            tenants: HashMap::new(),
            shed_counts: HashMap::new(),
        }
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
            ShedDecision::Shed {
                retry_after_secs: self.budget.retry_after_secs,
            }
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
        self.tenants
            .get(tenant)
            .copied()
            .unwrap_or_default()
            .total()
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
        assert_eq!(
            RunClass::derive(&PrincipalKind::Human, None),
            RunClass::Human
        );
        // agent → agent lane.
        assert_eq!(RunClass::derive(&agent_kind(), None), RunClass::Agent);
        // service → batch/ci by default.
        assert_eq!(
            RunClass::derive(&PrincipalKind::Service, None),
            RunClass::BatchCi
        );

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
        assert!(matches!(
            lane.admit(&t, RunClass::Speculative),
            ShedDecision::Shed { .. }
        ));
        // batch/ci still admitted (ceiling 5, 4 < 5).
        assert_eq!(lane.admit(&t, RunClass::BatchCi), ShedDecision::Admit); // non_human → 5
                                                                            // now non_human == 5: batch/ci sheds (ceiling 5, not < 5), agent still admitted (ceiling 6).
        assert!(matches!(
            lane.admit(&t, RunClass::BatchCi),
            ShedDecision::Shed { .. }
        ));
        assert_eq!(lane.admit(&t, RunClass::Agent), ShedDecision::Admit); // non_human → 6
                                                                          // now non_human == 6 == cap-reserved: AGENT sheds (ceiling 6), but the HUMAN is still admitted
                                                                          // — the human lane is protected, shed LAST.
        assert!(matches!(
            lane.admit(&t, RunClass::Agent),
            ShedDecision::Shed { .. }
        ));
        assert_eq!(lane.admit(&t, RunClass::Human), ShedDecision::Admit); // total 7, humans use reserved

        // the shed counts are exported per lane (contract-1.8) and follow the order.
        assert_eq!(lane.shed_count(RunClass::Speculative), 1);
        assert_eq!(lane.shed_count(RunClass::BatchCi), 1);
        assert_eq!(lane.shed_count(RunClass::Agent), 1);
        assert_eq!(
            lane.shed_count(RunClass::Human),
            0,
            "the human lane has NOT been shed"
        );
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
        assert!(
            matches!(lane.admit(&t, RunClass::Agent), ShedDecision::Shed { .. }),
            "agent shed at cap-reserved"
        );
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
        assert!(matches!(
            lane.admit(&noisy, RunClass::Agent),
            ShedDecision::Shed { .. }
        ));
        assert_eq!(lane.admit(&noisy, RunClass::Human), ShedDecision::Admit); // reserved slot, total 4
        assert!(
            matches!(
                lane.admit(&noisy, RunClass::Human),
                ShedDecision::Shed { .. }
            ),
            "noisy saturated"
        );

        // the QUIET tenant is COMPLETELY UNAFFECTED — its human is admitted, its budget untouched.
        assert_eq!(
            lane.in_flight(&quiet),
            0,
            "the quiet tenant's budget is independent"
        );
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
        assert!(matches!(
            lane.admit(&t, RunClass::Agent),
            ShedDecision::Shed { .. }
        ));
        lane.release(&t, RunClass::Agent);
        assert_eq!(
            lane.admit(&t, RunClass::Agent),
            ShedDecision::Admit,
            "a released slot is reusable"
        );
    }

    // ---- bounded everything: every queue/pool fast-fails (never grows latency unboundedly) -------

    #[test]
    fn bounded_queue_fast_fails_rather_than_growing_unboundedly() {
        let mut q = BoundedQueue::new(2);
        assert!(q.try_acquire(), "first permit");
        assert!(q.try_acquire(), "second permit");
        // full → fast-fail (shed), NOT queue: in_flight does NOT grow past the bound.
        assert!(
            !q.try_acquire(),
            "a full bounded queue fast-fails (sheds), never grows latency"
        );
        assert_eq!(
            q.in_flight(),
            2,
            "in-flight never exceeds the bound (Little's Law)"
        );
        assert_eq!(
            q.shed_count(),
            1,
            "the shed is counted (the bounded-everything signal)"
        );
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
            Surface::RefsBacklinkRead,
            Surface::RefsRefCreate,
            Surface::SearchQuery,
            Surface::WorkflowAgentLane,
            Surface::HttpIntake,
        ] {
            let b = table.budget(surface);
            // every surface is BOUNDED.
            assert!(
                b.per_tenant_in_flight_cap > 0,
                "{surface:?} must be bounded (§7.1)"
            );
            // the reservation never exceeds the cap.
            assert!(
                b.human_lane_reservation <= b.per_tenant_in_flight_cap,
                "{surface:?} reservation within the cap"
            );
            // every surface advertises a Retry-After (clients honour it, P-S17).
            assert!(
                b.retry_after_secs > 0,
                "{surface:?} sheds with a Retry-After"
            );
        }
        // CI is the batch lane: no human reservation (the §7.6 row says n/a).
        assert_eq!(table.budget(Surface::CiDispatch).human_lane_reservation, 0);
        // the human-facing surfaces DO reserve a human lane.
        assert!(table.budget(Surface::CollabOpStream).human_lane_reservation > 0);
        assert!(table.budget(Surface::ConnectionTier).human_lane_reservation > 0);
        assert!(table.budget(Surface::AgentMention).human_lane_reservation > 0);
        // The Git front door protects a human lane (a human's interactive fetch is shed last).
        assert!(table.budget(Surface::GitFrontDoor).human_lane_reservation > 0);
        // The Refs read + ref-create surfaces protect a human lane (REF-P22).
        assert!(
            table
                .budget(Surface::RefsBacklinkRead)
                .human_lane_reservation
                > 0
        );
        assert!(table.budget(Surface::RefsRefCreate).human_lane_reservation > 0);
        // The Search query surface protects a human lane (a human's interactive search is shed last).
        assert!(table.budget(Surface::SearchQuery).human_lane_reservation > 0);
        // The durable-workflow start surface protects a human lane (a human-initiated workflow is shed
        // last; an agent-initiated-workflow storm sheds first — F-8).
        assert!(
            table
                .budget(Surface::WorkflowAgentLane)
                .human_lane_reservation
                > 0
        );
    }

    // ---- P-S33: the tuned-budget human-lane floor (you cannot tune a human lane into starvation) ---

    /// **The measured table validates (P-S33): every human-facing surface holds the human-lane
    /// floor.** The tuned numbers in [`ShedBudgetTable::v1_floor`] each sit at-or-above the measured
    /// 20%-of-cap human-lane floor; the whole table passes [`ShedBudgetTable::validate`]. This is the
    /// green half of the starvation regression — the tuned numbers are NOT starved.
    #[test]
    fn the_tuned_table_validates_against_the_human_lane_floor() {
        let table = ShedBudgetTable::v1_floor();
        table
            .validate()
            .expect("the tuned shed-budget table must hold the human-lane floor on every surface");
        // and each human-facing surface's reservation is at-or-above its measured floor (earned, not vacuous).
        for surface in [
            Surface::CollabOpStream,
            Surface::ConnectionTier,
            Surface::AgentMention,
            Surface::GitFrontDoor,
            Surface::RefsBacklinkRead,
            Surface::RefsRefCreate,
            Surface::SearchQuery,
            Surface::WorkflowAgentLane,
            Surface::HttpIntake,
        ] {
            let b = table.budget(surface);
            assert!(
                b.human_lane_reservation
                    >= SurfaceBudget::human_lane_floor(b.per_tenant_in_flight_cap),
                "{surface:?} reserves {} of {} — at-or-above the measured human-lane floor {}",
                b.human_lane_reservation,
                b.per_tenant_in_flight_cap,
                SurfaceBudget::human_lane_floor(b.per_tenant_in_flight_cap),
            );
        }
    }

    /// **The starvation regression (P-S33 DoD): a budget tuned BELOW the human-lane floor FAILS the
    /// gate.** You cannot tune the human lane into starvation — a human-facing surface whose
    /// reservation drops under the measured floor is a LOUD [`ShedBudgetError::HumanLaneStarved`],
    /// never a silently-accepted regression (EI-01 §3).
    #[test]
    fn a_budget_tuned_below_the_human_lane_floor_fails_the_gate() {
        // ConnectionTier is human-facing: cap 256, floor = 20% = 52 (rounded up). A reservation of 4
        // (well under the floor) starves the human lane under surge → must be rejected.
        let starved = SurfaceBudget {
            per_tenant_in_flight_cap: 256,
            human_lane_reservation: 4,
            retry_after_secs: 3,
        };
        let err = starved
            .validate_tuned(Surface::ConnectionTier)
            .expect_err("a starved human lane must be rejected");
        match err {
            ShedBudgetError::HumanLaneStarved {
                surface,
                reservation,
                floor,
                cap,
            } => {
                assert_eq!(surface, Surface::ConnectionTier);
                assert_eq!(reservation, 4);
                assert_eq!(cap, 256);
                assert_eq!(floor, SurfaceBudget::human_lane_floor(256));
                assert!(reservation < floor, "the regression caught the starvation");
            }
            other => panic!("expected HumanLaneStarved, got {other:?}"),
        }

        // a table carrying that starved row fails table-level validation too.
        let mut rows = HashMap::new();
        rows.insert(Surface::ConnectionTier, starved);
        let bad = ShedBudgetTable { rows };
        assert!(
            matches!(
                bad.validate(),
                Err(ShedBudgetError::HumanLaneStarved { .. })
            ),
            "the table validation gate catches a starved human lane"
        );
    }

    /// **CI dispatch is exempt from the human-lane floor (the batch lane, §7.6 n/a).** A
    /// zero-reservation CI-dispatch budget is VALID (CI + agent share the wallet); the same
    /// zero-reservation on a human-facing surface is starvation. The exemption lives in one place
    /// ([`Surface::reserves_human_lane`]) — data, not a scattered branch.
    #[test]
    fn ci_dispatch_is_exempt_from_the_human_lane_floor_but_still_bounded() {
        let ci = SurfaceBudget {
            per_tenant_in_flight_cap: 64,
            human_lane_reservation: 0,
            retry_after_secs: 5,
        };
        ci.validate_tuned(Surface::CiDispatch)
            .expect("CI dispatch reserves no human lane (the batch lane) — valid");
        assert!(!Surface::CiDispatch.reserves_human_lane());

        // the SAME zero reservation on a human-facing surface is starvation.
        assert!(matches!(
            SurfaceBudget {
                per_tenant_in_flight_cap: 64,
                human_lane_reservation: 0,
                retry_after_secs: 5,
            }
            .validate_tuned(Surface::HttpIntake),
            Err(ShedBudgetError::HumanLaneStarved { .. })
        ));

        // an UNBOUNDED CI dispatch (cap 0) is still rejected — every surface is bounded (§7.1).
        assert!(matches!(
            SurfaceBudget {
                per_tenant_in_flight_cap: 0,
                human_lane_reservation: 0,
                retry_after_secs: 5,
            }
            .validate_tuned(Surface::CiDispatch),
            Err(ShedBudgetError::Unbounded(Surface::CiDispatch))
        ));
    }

    /// A reservation larger than the cap is rejected (a reservation can never exceed the budget).
    #[test]
    fn a_reservation_over_the_cap_is_rejected() {
        let over = SurfaceBudget {
            per_tenant_in_flight_cap: 10,
            human_lane_reservation: 20,
            retry_after_secs: 5,
        };
        assert!(matches!(
            over.validate_tuned(Surface::HttpIntake),
            Err(ShedBudgetError::ReservationOverCap {
                cap: 10,
                reservation: 20,
                ..
            })
        ));
    }

    /// The measured human-lane floor is 20% of the cap, rounded up, and never 0 for a tiny cap.
    #[test]
    fn human_lane_floor_is_twenty_percent_rounded_up_min_one() {
        assert_eq!(SurfaceBudget::HUMAN_LANE_FLOOR_BPS, 2000); // 20%.
        assert_eq!(SurfaceBudget::human_lane_floor(200), 40); // 20% of 200.
        assert_eq!(SurfaceBudget::human_lane_floor(256), 52); // ceil(51.2).
        assert_eq!(SurfaceBudget::human_lane_floor(1), 1); // rounds up to at least 1 slot.
        assert_eq!(SurfaceBudget::human_lane_floor(3), 1); // ceil(0.6) → 1.
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
        assert!(
            matches!(lane.admit(&t, c), ShedDecision::Shed { .. }),
            "the agent lane sheds"
        );
        // a human (derived) is still admitted — humans never queue behind agent runs (§7.6).
        let h = RunClass::derive(&PrincipalKind::Human, None);
        assert_eq!(lane.admit(&t, h), ShedDecision::Admit);
    }
}

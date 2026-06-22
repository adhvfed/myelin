//! # `dispatch` — the reactive/dispatch tier: nested causality + structural loop guards +
//! bounded dispatch + explicit-first + reserve/settle (contract 3.6; Bus §4.7; P-143 / EB-23)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/event-bus.md` §4.7 (the reactive/dispatch
//! tier — the stateful exception that consumes Signals and **dispatches agents/automations**,
//! the separately-reviewed D7 design EI-03 §6 warns about). Its disciplines, on top of the §4.2
//! consumer template:
//! - **Thread causality NESTED, not flat** (BUS-5): a dispatched action derives
//!   `causation_id = dispatch_event.event_id`, carries `correlation_id`, `depth = +1`. Flat
//!   threading is forbidden — it breaks depth-capping.
//! - **Structural loop guards** (AG-6 / EI-03 §5.3): **self-guard** (drop an event whose
//!   `actor.principal == the consumer's agent`), **reference gate** (only a structured
//!   `artifact_ref` content node re-triggers, never raw typed text), **causal-depth ceiling**
//!   (drop/park when `depth > ceiling`, default 12), **shared-causal-root tripwire** (>K events
//!   on one `correlation_id` in a short window → a per-tenant circuit breaker).
//! - **Bounded dispatch worker pool that drops over-cap** (AG-6): a mention/event storm is
//!   bounded; over-cap dispatches are dropped (with a Signal), never forked unboundedly.
//! - **Explicit-first dispatch (CHAT-1):** a mention NOTIFIES, it does NOT auto-spawn a costed
//!   run; implicit auto-dispatch is L-3 (counsel-gated), not done here.
//! - **Reserve/settle cost gate** (contract 11.7): every dispatched run passes the universal
//!   reserve/settle gate before the Agent Fabric/CI runner starts it — no balance → no execution.
//! - **Per-surface shed budgets the tier inherits (OQ-K — FLOOR named):** the agent-mention
//!   storm sheds the agent lane with `429 + Retry-After`; humans never queue behind agent runs.
//!   The Bus owns the *discipline* (the dispatch worker pool + per-tenant in-flight caps); the
//!   concrete *numbers* are each subsystem's M5 call (**EB-29**), asserted by the drills.
//!
//! Contract-index rows **3.6** (reactive/dispatch tier — OWNED) + **8.6** (`EventInbox::deliver`
//! explicit-first — CONSUMED, the dispatch target) + **11.7** (reserve/settle — CONSUMED, the
//! dispatch cost gate).
//!
//! ## Why the dispatch tier lives in `myelin-query`, not `myelin-events` (DOCUMENTED DEVIATION)
//! The EB-23 prompt's DELIVERABLE field says "In `myelin-events`: `dispatch.rs`". That is
//! **genuinely unworkable against the frozen crate DAG** for the SAME reason the
//! [`EventMatcher`](crate::EventMatcher) (P-137 / EB-17), the [`SignalEngine`](crate::SignalEngine)
//! (P-138 / EB-18), and the [`AutomationEngine`](crate::AutomationEngine) (P-139 / EB-19) had to
//! be built here and not in `myelin-events`: the dispatch tier **consumes curated Signals** — it
//! takes a [`Signal`](crate::Signal) / a matcher result as its input — and `Signal` + `EventMatcher` live in
//! `myelin-query`, which **depends on `myelin-events`** (architecture §2.9). Siting the dispatch
//! tier in `myelin-events` would require `…-events → …-query`, the cycle the `no-cross-sync-cycle`
//! lint (E-5) and the events `Cargo.toml` forbid. So the dispatch tier is built HERE, ON TOP of
//! the one [`EventMatcher`] + the one Signal/Automation/Trigger engine, over the upstream
//! [`EventEnvelope`] + the upstream pure [`derive_envelope`] (the causality derivation is reused
//! verbatim — there is NO second causality function). This is the SAME pattern the matcher +
//! signals + automations + triggers already follow, recorded here and in the P-143 report per
//! external-insights/01 §1.
//!
//! ## The two CONSUMED seams (8.6, 11.7) — abstract traits owned here, real impls higher up
//! `myelin-query` MUST NOT depend on `-agent` (the real `EventInbox`, contract 8.6) or `-storage`
//! (the real reserve/settle `CostLedger`, contract 11.7) — both sit ABOVE query in the §2.9 DAG.
//! So the dispatch tier consumes them through DAG-respecting trait seams defined here:
//! - [`DispatchTarget`] — the 8.6 `EventInbox::deliver` seam (deliver a dispatched run into the
//!   Agent Fabric's inbox). The REAL target is `myelin-agent`'s `EventInbox` (the named floor,
//!   **AG-P4 / P-216**); [`RecordingTarget`] is the deterministic floor for the unit/CDC tests.
//! - [`CostGate`] — the 11.7 reserve/settle seam (reserve before a run, settle after). The REAL
//!   gate is `myelin-storage`'s `CostLedger::reserve`/`settle` (the named floor, **P-ST-16 /
//!   P-103 + P-ST-19 / P-146**); [`InMemoryCostGate`] is the deterministic floor here.
//!
//! This mirrors exactly how [`crate::automations`] consumes the `myelin-flow`
//! [`DurableExecutor`](crate::DurableExecutor) seam (9.1) and how `myelin_events::holder` consumes
//! the KMS crypto-shred seam — the upstream crate owns the trait, the downstream crate owns the
//! impl. There is no second inbox type and no second cost-ledger type platform-wide.
//!
//! ## Determinism (BUS-D3 replay): the tier is a pure reflex over its inputs
//! [`DispatchTier::dispatch`] is a pure function of `(event, decision-inputs, breaker-state)` →
//! [`Disposition`] + the dispatched-action envelope. The same event sequence replays to the same
//! dispositions, the same dispatched envelopes (byte-identical, because [`derive_envelope`] is
//! pure), and the same breaker trip — exactly what the BUS-D3 replay-determinism drill asserts.
//! All counters ([`DispatchTelemetry`]) are deterministic in the input sequence.

use crate::PublishDraft;
use myelin_events::{
    derive_envelope, Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EmitContext,
    EventDraft, EventEnvelope, EventId, EventType, PiiKeyRef, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId};
use myelin_tenancy::TenantId;
use std::collections::HashMap;

/// The **default causal-depth ceiling** (Bus §4.7; AG-6; the §00 two-ceilings gate). A dispatch
/// whose triggering event is at `depth >= CAUSAL_DEPTH_CEILING` is **parked** (not delivered) —
/// the structural guard that a self-perpetuating reactive chain halts at a bounded depth rather
/// than recursing unboundedly. **12** is the frozen agent/causal ceiling (the Refs *traversal*
/// ceiling 16 is a different number, REF-P13). A dispatched action is `depth = parent + 1`, so a
/// chain that has reached the ceiling produces no further dispatch.
pub const CAUSAL_DEPTH_CEILING: u32 = 12;

/// The **default shared-causal-root tripwire threshold** `K` (Bus §4.7). More than `K` dispatches
/// charged to ONE `correlation_id` (one causal root) within the engine's accounting window trips
/// the **per-tenant circuit breaker** — the guard against a storm that stays under the depth
/// ceiling but fans out wide on one root (e.g. an agent that re-triggers a sibling on every event
/// of the same conversation). The concrete production number + window are an M5 tuning call
/// (**EB-29**); this is the structural default the BUS-D6 drill asserts against.
pub const SHARED_ROOT_TRIPWIRE_K: u32 = 64;

/// The **default per-tenant in-flight dispatch cap** (the bounded dispatch worker pool, Bus §4.7 /
/// AG-6). At most this many dispatched runs may be in flight for one tenant at once; an over-cap
/// dispatch is **shed** (dropped with a Signal + `429 + Retry-After`), never forked unboundedly —
/// the agent-mention-storm bound (OQ-K). The concrete number is an M5 tuning call (**EB-29**).
pub const DISPATCH_INFLIGHT_CAP: u32 = 32;

/// The frozen **`Retry-After` seconds** the agent lane sheds with on an over-cap / breaker-tripped
/// dispatch (ADR-16.3; OQ-K). A floor value (the M5-tuned number is **EB-29**); the structural
/// property the drill asserts is that a shed carries a `429 + Retry-After`, never a silent drop.
pub const SHED_RETRY_AFTER_SECONDS: u32 = 30;

// ===========================================================================
// The dispatch action — what the tier produces on a deliver decision
// ===========================================================================

/// A **dispatch request** the tier consumes: the triggering event + the agent this consumer
/// dispatches to + how this consumer was triggered (explicit mention vs an automation reflex).
///
/// The dispatch tier is the matching/guarding/rate-limiting layer between a Signal/event and the
/// Agent Fabric inbox (Bus §4.7 closing). A request is the unit it decides on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchRequest {
    /// The triggering event (the dispatch derives its action's causality FROM this — nested).
    pub event: EventEnvelope,
    /// The agent principal this consumer would dispatch the action to (the self-guard compares
    /// `event.actor.principal` against this — an agent must not react to its OWN event).
    pub agent: PrincipalId,
    /// The workflow/run reference the dispatched action would start (opaque — the dispatch target
    /// resolves it; the tier never interprets it).
    pub run_ref: String,
    /// How this consumer was triggered. [`TriggerKind::Mention`] is EXPLICIT-FIRST: it notifies,
    /// it does NOT auto-spawn a costed run (CHAT-1). [`TriggerKind::Automation`] is a project-owned
    /// reflex that DOES dispatch a costed run (a costed run still passes reserve/settle + the
    /// guards).
    pub trigger: TriggerKind,
}

/// **Which agent reflex a curated Signal binds to** — the `(agent, run_ref, trigger)` triple that
/// turns a [`PublishDraft`] + its originating event into a [`DispatchRequest`] in
/// [`DispatchTier::dispatch_for_signal`]. Grouped into one value so the Signal-consumption entry
/// point stays a small, named seam (the Signal says WHAT happened; the binding says WHICH agent
/// reacts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalBinding {
    /// The agent principal this Signal dispatches to (the self-guard compares the origin event's
    /// actor against it).
    pub agent: PrincipalId,
    /// The workflow/run reference the dispatched action would start.
    pub run_ref: String,
    /// How the agent was triggered (explicit mention vs automation reflex).
    pub trigger: TriggerKind,
}

/// How a dispatch consumer was triggered — the **explicit-first** discriminator (CHAT-1, Bus
/// §4.7). A `Mention` is explicit-first (notify, never auto-spawn a costed run); an `Automation`
/// reflex is the project-owned "when X, do Y" that does dispatch a costed run (still guarded).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerKind {
    /// A mention of the agent (e.g. `@agent` in a comment). EXPLICIT-FIRST: notifies, does NOT
    /// auto-spawn a costed run (CHAT-1). Implicit auto-dispatch on a mention is L-3 (counsel-gated).
    Mention,
    /// A project-owned automation reflex. Dispatches a costed run (after the guards + reserve).
    Automation,
}

/// The **disposition** of one dispatch decision — what the tier did with a [`DispatchRequest`].
/// Every branch is a *named, audited* outcome: there is NO silent drop (EI-02 §4). A dropped /
/// shed / parked request still records WHY + (for a shed) emits a `signal.dispatch.*` so the
/// over-cap / loop / break is observable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Disposition {
    /// **Delivered**: the guards passed, the cost was reserved, and the dispatched-action envelope
    /// was delivered to the Agent Fabric inbox (8.6). Carries the derived (nested-causality)
    /// envelope for the action (`causation_id = event.event_id`, `correlation_id` carried,
    /// `depth = +1`) and the reservation handle the run settles against.
    Delivered {
        /// The dispatched action's envelope — derived from the triggering event (nested causality).
        /// Boxed so the `Delivered` variant does not dominate the enum size (the `EventEnvelope` is
        /// the one large variant; every other disposition is a handful of words).
        action: Box<EventEnvelope>,
    },
    /// **Notified only** (EXPLICIT-FIRST, CHAT-1): a mention notified the agent, it did NOT
    /// auto-spawn a costed run. No reservation, no inbox dispatch of a run — just the notice.
    NotifiedOnly,
    /// **Self-guard drop**: the triggering event's actor IS the consumer's agent — an agent must
    /// not react to its own event (the most basic structural loop guard). Dropped, audited.
    SelfGuardDropped,
    /// **Reference-gate drop**: the trigger was raw typed text, not a structured `artifact_ref`
    /// content node — only an `artifact_ref` node may re-trigger (never raw text, AG-6 / ADR-05).
    ReferenceGateDropped,
    /// **Depth-ceiling parked**: the triggering event is at/over the causal-depth ceiling
    /// (default 12). The dispatch is parked (no further dispatch) so the chain halts ≤ ceiling.
    DepthCeilingParked {
        /// The triggering event's depth (>= the ceiling — the dispatched action would exceed it).
        depth: u32,
    },
    /// **Breaker-tripped shed**: the per-tenant breaker is OPEN (the shared-root tripwire tripped,
    /// or in-flight is at cap). The dispatch is shed with `429 + Retry-After` (OQ-K) and a
    /// `signal.dispatch.breaker_open` Signal — never a silent drop, never an unbounded fork.
    BreakerShed {
        /// The `429`-equivalent shed signal the agent lane returns (carries `Retry-After`).
        shed: ShedSignal,
    },
    /// **Over-cap shed**: the per-tenant in-flight dispatch pool is at cap. Shed with
    /// `429 + Retry-After` (OQ-K) + a `signal.dispatch.over_cap` Signal — the agent-mention storm
    /// is bounded, never forked unboundedly.
    OverCapShed {
        /// The `429`-equivalent shed signal (carries `Retry-After`).
        shed: ShedSignal,
    },
    /// **No-balance refused** (reserve/settle, 11.7): the reservation failed (no balance) — no
    /// balance, no execution. The run is NOT dispatched; the refusal is audited.
    NoBalanceRefused,
}

/// The **`429 + Retry-After` shed signal** the agent lane returns when the dispatch tier sheds an
/// over-cap / breaker-tripped dispatch (ADR-16.3; OQ-K; Bus §4.7). The Bus owns the discipline
/// (there IS a shed, it carries a `Retry-After`); the M5-tuned `retry_after_seconds` number is
/// **EB-29**. This is the structural artifact the drills assert (a shed is never silent).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShedSignal {
    /// The HTTP-status-equivalent of the shed (always `429 Too Many Requests` for a lane shed).
    pub status: u16,
    /// The `Retry-After` the runtime must honour (seconds; ADR-16.3 — the resilient client honours
    /// it, no retry-storm amplification, SUB-D5).
    pub retry_after_seconds: u32,
    /// Why the lane was shed (over-cap vs breaker-open) — the curated Signal subject token.
    pub reason: ShedReason,
}

impl ShedSignal {
    /// A `429` shed with the frozen-floor `Retry-After` (EB-29 tunes the number).
    fn lane_shed(reason: ShedReason) -> ShedSignal {
        ShedSignal {
            status: 429,
            retry_after_seconds: SHED_RETRY_AFTER_SECONDS,
            reason,
        }
    }

    /// The curated-Signal subject this shed publishes as (`signal.dispatch.<reason>`), so the
    /// over-cap / breaker is observable (Notif/observability consume it).
    pub fn signal_subject(&self) -> &'static str {
        match self.reason {
            ShedReason::OverCap => "signal.dispatch.over_cap",
            ShedReason::BreakerOpen => "signal.dispatch.breaker_open",
        }
    }
}

/// Why the agent lane was shed (the [`ShedSignal::reason`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShedReason {
    /// The per-tenant in-flight dispatch pool was at cap (the bounded worker pool, OQ-K).
    OverCap,
    /// The per-tenant breaker is open (the shared-root tripwire tripped).
    BreakerOpen,
}

// ===========================================================================
// 8.6 — the EventInbox::deliver seam (CONSUMED). DAG-respecting trait owned here.
// ===========================================================================

/// **The 8.6 `EventInbox::deliver` seam (CONSUMED)** — the dispatch target the tier delivers a
/// dispatched action to. The REAL target is `myelin-agent`'s `EventInbox` (contract 8.6, the
/// **explicit-first** delivery surface; the named floor is **AG-P4 / P-216**). `myelin-query`
/// sits UPSTREAM of `-agent` in the §2.9 DAG, so it consumes this trait, never `-agent` directly —
/// the SAME DAG-respecting seam pattern [`crate::DurableExecutor`] (9.1) uses. There is no second
/// inbox type platform-wide; the real `EventInbox::deliver` is wired behind this in AG-P4.
pub trait DispatchTarget {
    /// Deliver a dispatched action's envelope into the agent inbox. A genuine delivery failure is
    /// surfaced as `Err` (never a silent drop — EI-02 §4). `Ok(())` means the inbox accepted it.
    fn deliver(&self, action: &EventEnvelope) -> Result<(), DispatchError>;
}

/// A dispatch-target delivery failure (the inbox is unreachable / refused). Surfaced, never
/// swallowed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchError(pub String);

/// The deterministic in-memory [`DispatchTarget`] floor (for the unit/CDC tests + the BUS-D3
/// replay-determinism substrate). It records every delivered action so a test can assert WHAT was
/// delivered (the nested-causality envelope) and that a redelivery (same `event_id`) is observable.
/// The real target is `myelin-agent`'s `EventInbox` (the named floor); this models exactly its
/// `deliver` semantics until then.
#[derive(Debug, Default)]
pub struct RecordingTarget {
    delivered: std::cell::RefCell<Vec<EventEnvelope>>,
}

impl RecordingTarget {
    /// A fresh target with no deliveries.
    pub fn new() -> RecordingTarget {
        RecordingTarget::default()
    }

    /// The actions delivered so far (in delivery order) — the test-observable proof of dispatch.
    pub fn delivered(&self) -> Vec<EventEnvelope> {
        self.delivered.borrow().clone()
    }

    /// How many actions were delivered.
    pub fn delivered_count(&self) -> usize {
        self.delivered.borrow().len()
    }
}

impl DispatchTarget for RecordingTarget {
    fn deliver(&self, action: &EventEnvelope) -> Result<(), DispatchError> {
        self.delivered.borrow_mut().push(action.clone());
        Ok(())
    }
}

// ===========================================================================
// 11.7 — the reserve/settle cost gate seam (CONSUMED). DAG-respecting trait owned here.
// ===========================================================================

/// **The 11.7 reserve/settle seam (CONSUMED)** — the universal cost gate every dispatched run
/// passes BEFORE the Agent Fabric/CI runner starts it (Bus §4.7: "no balance → no execution"). The
/// REAL gate is `myelin-storage`'s `CostLedger::reserve`/`settle` (contract 11.7, the durable
/// per-tenant ledger; the named floors are **P-ST-16 / P-103** (reserve/settle mechanism) +
/// **P-ST-19 / P-146** (reserve/settle fronts agent runs)). `myelin-query` sits UPSTREAM of
/// `-storage` here, so it consumes this trait, never `-storage` directly — the DAG-respecting seam.
/// The dispatch tier reserves before delivering and the run settles after; this trait is the
/// reserve half the tier gates on (settle is the run's concern, surfaced by the durable ledger).
pub trait CostGate {
    /// Reserve budget for a dispatched run for `(tenant, run_ref)`. Returns a [`Reservation`] iff
    /// there IS balance; `None` means **no balance → no execution** (the run is refused, audited).
    /// Idempotent on `run_ref` (a redelivered dispatch re-reserves the SAME reservation, never
    /// double-charges — the effectively-once posture, 2.5 / 11.7).
    fn reserve(&self, tenant: &TenantId, run_ref: &str) -> Option<Reservation>;

    /// The current in-flight dispatch count for `tenant` (the bounded worker pool reads this for
    /// the over-cap shed). The durable ledger tracks reserved-but-unsettled runs per tenant.
    fn in_flight(&self, tenant: &TenantId) -> u32;
}

/// A reservation handle (the 11.7 reserve receipt the run settles against). Opaque here; the
/// durable ledger owns the cost arithmetic. References-not-payloads: it carries the run ref + the
/// reserved cost, never a body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reservation {
    /// The run this reservation fronts (the settle key).
    pub run_ref: String,
    /// The reserved cost (integer minor-units — the frozen unit posture, §2.10; never a float).
    pub reserved_units: u64,
}

/// The deterministic in-memory [`CostGate`] floor (for the unit/CDC tests + the BUS-D3
/// replay-determinism substrate). A per-tenant balance + an in-flight set; `reserve` admits iff
/// the balance covers the per-run cost and is **idempotent on `run_ref`** (a redelivery re-reserves
/// the same reservation). The real gate is `myelin-storage`'s `CostLedger` (the named floor); this
/// models exactly its reserve semantics until then.
#[derive(Debug)]
pub struct InMemoryCostGate {
    /// Per-tenant remaining balance (integer minor-units). A tenant absent ⇒ zero balance.
    balance: std::cell::RefCell<HashMap<TenantId, u64>>,
    /// Per-tenant reserved-but-unsettled runs (`run_ref`s) — the in-flight set.
    in_flight: std::cell::RefCell<HashMap<TenantId, std::collections::BTreeSet<String>>>,
    /// The per-run reserved cost (the floor's flat charge; the real ledger meters actual usage).
    cost_per_run: u64,
}

impl InMemoryCostGate {
    /// A fresh gate with a flat per-run cost and no balances (every tenant starts at 0 — set a
    /// balance with [`InMemoryCostGate::credit`]).
    pub fn new(cost_per_run: u64) -> InMemoryCostGate {
        InMemoryCostGate {
            balance: std::cell::RefCell::new(HashMap::new()),
            in_flight: std::cell::RefCell::new(HashMap::new()),
            cost_per_run,
        }
    }

    /// Credit a tenant's balance (a test/dev lever; the real ledger is fed by Storage M1).
    pub fn credit(&self, tenant: &TenantId, units: u64) {
        let mut b = self.balance.borrow_mut();
        *b.entry(tenant.clone()).or_insert(0) += units;
    }
}

impl CostGate for InMemoryCostGate {
    fn reserve(&self, tenant: &TenantId, run_ref: &str) -> Option<Reservation> {
        // Idempotent on run_ref: a redelivery returns the SAME reservation, never double-charges.
        {
            let inflight = self.in_flight.borrow();
            if inflight.get(tenant).is_some_and(|s| s.contains(run_ref)) {
                return Some(Reservation {
                    run_ref: run_ref.to_string(),
                    reserved_units: self.cost_per_run,
                });
            }
        }
        let mut bal = self.balance.borrow_mut();
        let remaining = bal.entry(tenant.clone()).or_insert(0);
        if *remaining < self.cost_per_run {
            return None; // no balance → no execution (11.7).
        }
        *remaining -= self.cost_per_run;
        self.in_flight
            .borrow_mut()
            .entry(tenant.clone())
            .or_default()
            .insert(run_ref.to_string());
        Some(Reservation {
            run_ref: run_ref.to_string(),
            reserved_units: self.cost_per_run,
        })
    }

    fn in_flight(&self, tenant: &TenantId) -> u32 {
        self.in_flight
            .borrow()
            .get(tenant)
            .map(|s| s.len() as u32)
            .unwrap_or(0)
    }
}

// ===========================================================================
// The per-tenant circuit breaker + the shared-causal-root tripwire
// ===========================================================================

/// The **per-tenant dispatch breaker + shared-causal-root tripwire** (Bus §4.7; the BUS-D6 gate).
/// It counts dispatches per `(tenant, correlation_id)` within the accounting window; when one root
/// exceeds [`SHARED_ROOT_TRIPWIRE_K`] the tenant's breaker **trips OPEN** and stays open — every
/// further dispatch for that tenant is shed (`BreakerShed`) until the breaker is reset. This is the
/// guard against a storm that stays UNDER the depth ceiling but fans wide on one causal root.
///
/// Deterministic in the input sequence (a `BTreeMap`/`HashMap` of counters) — the BUS-D3 replay
/// drill relies on the same sequence tripping the breaker at the same dispatch.
#[derive(Debug, Default)]
pub struct DispatchBreaker {
    /// Per-`(tenant, correlation_id)` dispatch count within the window (the shared-root counter).
    root_counts: HashMap<(TenantId, CorrelationId), u32>,
    /// Per-tenant breaker state — once OPEN it stays open until [`DispatchBreaker::reset`].
    open: std::collections::HashSet<TenantId>,
    /// The tripwire threshold `K` (default [`SHARED_ROOT_TRIPWIRE_K`]).
    tripwire_k: u32,
}

impl DispatchBreaker {
    /// A fresh breaker with the default tripwire threshold.
    pub fn new() -> DispatchBreaker {
        DispatchBreaker {
            tripwire_k: SHARED_ROOT_TRIPWIRE_K,
            ..Default::default()
        }
    }

    /// A breaker with a custom tripwire threshold (the drill sets a small `K` to force the trip
    /// without 64 events).
    pub fn with_tripwire(k: u32) -> DispatchBreaker {
        DispatchBreaker {
            tripwire_k: k,
            ..Default::default()
        }
    }

    /// Whether the tenant's breaker is currently OPEN (every dispatch is shed while open).
    pub fn is_open(&self, tenant: &TenantId) -> bool {
        self.open.contains(tenant)
    }

    /// Record one dispatch charged to `(tenant, correlation_id)` and return whether the tenant's
    /// breaker is OPEN AFTER this dispatch. Crossing [`Self::tripwire_k`] on ONE root trips the
    /// per-tenant breaker (and it stays open). Called only on an otherwise-admissible dispatch.
    fn record_and_check(&mut self, tenant: &TenantId, root: &CorrelationId) -> bool {
        if self.open.contains(tenant) {
            return true;
        }
        let count = self
            .root_counts
            .entry((tenant.clone(), root.clone()))
            .or_insert(0);
        *count += 1;
        if *count > self.tripwire_k {
            self.open.insert(tenant.clone());
            return true;
        }
        false
    }

    /// The current dispatch count for `(tenant, correlation_id)` (the shared-root counter — the
    /// drill asserts it crossed the tripwire).
    pub fn root_count(&self, tenant: &TenantId, root: &CorrelationId) -> u32 {
        self.root_counts
            .get(&(tenant.clone(), root.clone()))
            .copied()
            .unwrap_or(0)
    }

    /// Reset a tenant's breaker (the half-open recovery; the operator/auto-reset lever). Clears the
    /// open state AND the per-root counters for that tenant so a fresh window starts clean.
    pub fn reset(&mut self, tenant: &TenantId) {
        self.open.remove(tenant);
        self.root_counts.retain(|(t, _), _| t != tenant);
    }
}

// ===========================================================================
// Telemetry — the §4.11 dispatch-tier signals the drills assert against
// ===========================================================================

/// The **dispatch-tier telemetry** (Bus §4.11 / §4.7): the per-tenant in-flight count, the
/// shed-counts (over-cap + breaker), the causal-depth histogram input (max depth dispatched), and
/// the shared-root-tripwire firing count. These ARE the assertions the BUS-D1/BUS-D3/BUS-D6 drills
/// read (telemetry IS part of the pass — observability watches the breaker trip, EI-01 §3).
/// Deterministic in the input sequence (the BUS-D3 replay gives identical telemetry).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DispatchTelemetry {
    /// Total dispatches delivered (the `Delivered` count).
    pub delivered: u64,
    /// Total mentions notified-only (explicit-first; CHAT-1 — these did NOT auto-spawn a run).
    pub notified_only: u64,
    /// Self-guard drops (an agent's own event).
    pub self_guard_dropped: u64,
    /// Reference-gate drops (raw text, not an `artifact_ref` node).
    pub reference_gate_dropped: u64,
    /// Depth-ceiling parks (a chain that reached the ceiling).
    pub depth_ceiling_parked: u64,
    /// Over-cap sheds (the bounded worker pool was full).
    pub over_cap_shed: u64,
    /// Breaker sheds (the per-tenant breaker was open).
    pub breaker_shed: u64,
    /// No-balance refusals (reserve/settle, 11.7).
    pub no_balance_refused: u64,
    /// The maximum causal depth of any dispatched action (the causal-depth-histogram max input).
    pub max_dispatched_depth: u32,
    /// How many times the shared-root tripwire tripped a per-tenant breaker (the BUS-D6 assertion).
    pub tripwire_firings: u64,
}

impl DispatchTelemetry {
    /// Fold one disposition into the running telemetry (called by [`DispatchTier::dispatch`]).
    fn record(&mut self, disp: &Disposition, tripwire_fired: bool) {
        match disp {
            Disposition::Delivered { action } => {
                self.delivered += 1;
                self.max_dispatched_depth = self.max_dispatched_depth.max(action.depth);
            }
            Disposition::NotifiedOnly => self.notified_only += 1,
            Disposition::SelfGuardDropped => self.self_guard_dropped += 1,
            Disposition::ReferenceGateDropped => self.reference_gate_dropped += 1,
            Disposition::DepthCeilingParked { .. } => self.depth_ceiling_parked += 1,
            Disposition::OverCapShed { .. } => self.over_cap_shed += 1,
            Disposition::BreakerShed { .. } => self.breaker_shed += 1,
            Disposition::NoBalanceRefused => self.no_balance_refused += 1,
        }
        if tripwire_fired {
            self.tripwire_firings += 1;
        }
    }
}

// ===========================================================================
// The dispatch tier
// ===========================================================================

/// The **reactive/dispatch tier** (contract 3.6; Bus §4.7) — the matching/guarding/rate-limiting
/// layer between a Signal/event and the Agent Fabric inbox. It is a stateful exception over the
/// §4.2 consumer template: it holds the per-tenant breaker + the telemetry, and on each
/// [`DispatchRequest`] it runs the discipline gauntlet:
///
/// 1. **Self-guard** — drop an event whose `actor.principal == request.agent` (an agent's own
///    event never re-triggers it).
/// 2. **Reference gate** — only a structured `artifact_ref` content node re-triggers; raw typed
///    text is dropped.
/// 3. **Explicit-first** — a `Mention` notifies (`NotifiedOnly`), it does NOT auto-spawn a costed
///    run (CHAT-1). An `Automation` reflex proceeds to dispatch a costed run.
/// 4. **Depth ceiling** — if the triggering event is at/over the causal-depth ceiling (default
///    12), park (no dispatch); the chain halts ≤ ceiling.
/// 5. **Breaker / over-cap** — if the per-tenant breaker is open, or in-flight is at cap, shed with
///    `429 + Retry-After` + a `signal.dispatch.*` (the bounded pool; the shared-root tripwire).
/// 6. **Reserve/settle** — reserve the cost (11.7); no balance → no execution (`NoBalanceRefused`).
/// 7. **Nested-causality dispatch** — derive the action envelope from the triggering event
///    (`causation_id = event.event_id`, `correlation_id` carried, `depth = +1`) via the upstream
///    pure [`derive_envelope`] (flat threading is structurally impossible — the draft has no
///    causal fields), record the dispatch against the breaker (which may trip the tripwire), and
///    deliver it to the inbox (8.6).
///
/// `T` is the consumed 8.6 dispatch target; `G` is the consumed 11.7 cost gate. Both are seams
/// (real impls higher up the DAG) — the tier never depends on `-agent` / `-storage`.
#[derive(Debug)]
pub struct DispatchTier<T: DispatchTarget, G: CostGate> {
    target: T,
    cost_gate: G,
    breaker: DispatchBreaker,
    telemetry: DispatchTelemetry,
    /// The causal-depth ceiling (default [`CAUSAL_DEPTH_CEILING`]).
    depth_ceiling: u32,
    /// The per-tenant in-flight dispatch cap (default [`DISPATCH_INFLIGHT_CAP`]).
    inflight_cap: u32,
}

/// The interior carry from [`DispatchTier::decide`] to [`DispatchTier::dispatch`]'s telemetry
/// fold: the disposition + whether THIS dispatch was the one that crossed the shared-root tripwire
/// (so the `tripwire_firings` counter increments exactly once per trip, not on every shed-after).
struct Decision {
    disposition: Disposition,
    tripwire_fired: bool,
}

impl<T: DispatchTarget, G: CostGate> DispatchTier<T, G> {
    /// A dispatch tier with the frozen-default ceiling + cap + tripwire `K`.
    pub fn new(target: T, cost_gate: G) -> DispatchTier<T, G> {
        DispatchTier {
            target,
            cost_gate,
            breaker: DispatchBreaker::new(),
            telemetry: DispatchTelemetry::default(),
            depth_ceiling: CAUSAL_DEPTH_CEILING,
            inflight_cap: DISPATCH_INFLIGHT_CAP,
        }
    }

    /// A dispatch tier with custom guard limits (the drills set a small ceiling / tripwire / cap to
    /// force the structural property without thousands of events). The thresholds are the FLOOR
    /// defaults (EB-29 tunes the production numbers); the drill exercises the SAME structural code.
    pub fn with_limits(
        target: T,
        cost_gate: G,
        depth_ceiling: u32,
        tripwire_k: u32,
        inflight_cap: u32,
    ) -> DispatchTier<T, G> {
        DispatchTier {
            target,
            cost_gate,
            breaker: DispatchBreaker::with_tripwire(tripwire_k),
            telemetry: DispatchTelemetry::default(),
            depth_ceiling,
            inflight_cap,
        }
    }

    /// The running dispatch telemetry (the BUS-D1/BUS-D3/BUS-D6 assertion surface).
    pub fn telemetry(&self) -> &DispatchTelemetry {
        &self.telemetry
    }

    /// Whether the tenant's breaker is open (the BUS-D6 assertion).
    pub fn breaker_open(&self, tenant: &TenantId) -> bool {
        self.breaker.is_open(tenant)
    }

    /// The shared-root dispatch count for `(tenant, correlation_id)` (the BUS-D6 assertion that the
    /// tripwire counter crossed `K`).
    pub fn root_count(&self, tenant: &TenantId, root: &CorrelationId) -> u32 {
        self.breaker.root_count(tenant, root)
    }

    /// The consumed dispatch target (for a test to inspect what was delivered).
    pub fn target(&self) -> &T {
        &self.target
    }

    /// Reset a tenant's breaker (the half-open recovery lever).
    pub fn reset_breaker(&mut self, tenant: &TenantId) {
        self.breaker.reset(tenant);
    }

    /// **The dispatch reflex** — run the §4.7 discipline gauntlet on one [`DispatchRequest`] and
    /// return its [`Disposition`]. Pure in `(request, breaker-state, gate-state)` (the BUS-D3
    /// replay determinism property): the same sequence replays to the same dispositions + the same
    /// derived envelopes + the same breaker trip.
    ///
    /// `mint_event_id` supplies the new event id for the dispatched action's envelope (the outbox
    /// mints a ULID in production; the tests pass a deterministic minter so replay is byte-exact).
    /// `now` is the dispatched action's `recorded_at` (RFC-3339 UTC, §2.10).
    pub fn dispatch(
        &mut self,
        req: &DispatchRequest,
        mint_event_id: impl FnOnce() -> EventId,
        now: &Timestamp,
    ) -> Disposition {
        let Decision {
            disposition,
            tripwire_fired,
        } = self.decide(req, mint_event_id, now);
        self.telemetry.record(&disposition, tripwire_fired);
        disposition
    }

    /// **Consume a curated [`Signal`](crate::Signal)** (the §4.4 dispatch input, contract 3.6): the dispatch tier
    /// subscribes to curated Signals (the upstream defence BUS-4 — never the raw `evt.*` firehose),
    /// and a Signal that selects an agent reflex turns into a [`DispatchRequest`] over the
    /// originating event. This is the wiring from [`SignalEngine`](crate::SignalEngine)'s
    /// [`PublishDraft`] into the dispatch gauntlet: the `draft` names which Signal fired, the
    /// `origin` is the event that produced it (the causal parent the action nests under), and
    /// `(agent, run_ref, trigger)` say which agent reflex this Signal binds to. A `Resolved` Signal
    /// (an incident closing) never dispatches a run — it is informational; only an `Opened` /
    /// `Collapsed` Signal carrying an open incident dispatches.
    pub fn dispatch_for_signal(
        &mut self,
        draft: &PublishDraft,
        origin: &EventEnvelope,
        binding: SignalBinding,
        mint_event_id: impl FnOnce() -> EventId,
        now: &Timestamp,
    ) -> Disposition {
        // A resolving Signal closes an incident; it does not dispatch a costed run (informational).
        if draft.signal.state == crate::SignalState::Resolved {
            let disp = Disposition::NotifiedOnly;
            self.telemetry.record(&disp, false);
            return disp;
        }
        let req = DispatchRequest {
            event: origin.clone(),
            agent: binding.agent,
            run_ref: binding.run_ref,
            trigger: binding.trigger,
        };
        self.dispatch(&req, mint_event_id, now)
    }

    /// The pure decision (split out so `dispatch` can fold telemetry uniformly). Returns the
    /// [`Disposition`] + whether THIS dispatch crossed the shared-root tripwire.
    fn decide(
        &mut self,
        req: &DispatchRequest,
        mint_event_id: impl FnOnce() -> EventId,
        now: &Timestamp,
    ) -> Decision {
        let ev = &req.event;
        let no_trip = |disposition| Decision {
            disposition,
            tripwire_fired: false,
        };

        // (1) SELF-GUARD: an agent never reacts to its own event.
        if ev.actor.0.principal_id == req.agent {
            return no_trip(Disposition::SelfGuardDropped);
        }

        // (2) REFERENCE GATE: only a structured `artifact_ref` content node re-triggers; raw text
        // is dropped. The triggering event's `subject` is the `ArtifactRef` — a well-formed
        // `myelin://…` ref is the structured node; a raw-text trigger is signalled by a non-ref
        // subject (see `is_artifact_ref`).
        if !is_artifact_ref(&ev.subject) {
            return no_trip(Disposition::ReferenceGateDropped);
        }

        // (3) EXPLICIT-FIRST (CHAT-1): a mention notifies, it does NOT auto-spawn a costed run.
        if req.trigger == TriggerKind::Mention {
            return no_trip(Disposition::NotifiedOnly);
        }

        // (4) DEPTH CEILING: a dispatched action would be `depth + 1`; if the trigger is already
        // at/over the ceiling, park (the chain halts ≤ ceiling).
        if ev.depth >= self.depth_ceiling {
            return no_trip(Disposition::DepthCeilingParked { depth: ev.depth });
        }

        // (5) BREAKER (shared-root tripwire) + OVER-CAP (bounded worker pool). If the breaker is
        // already open, shed. If in-flight is at cap, shed over-cap.
        if self.breaker.is_open(&ev.tenant) {
            return no_trip(Disposition::BreakerShed {
                shed: ShedSignal::lane_shed(ShedReason::BreakerOpen),
            });
        }
        if self.cost_gate.in_flight(&ev.tenant) >= self.inflight_cap {
            return no_trip(Disposition::OverCapShed {
                shed: ShedSignal::lane_shed(ShedReason::OverCap),
            });
        }

        // (6) RESERVE/SETTLE (11.7): no balance → no execution.
        if self.cost_gate.reserve(&ev.tenant, &req.run_ref).is_none() {
            return no_trip(Disposition::NoBalanceRefused);
        }

        // (7) NESTED-CAUSALITY DISPATCH. Derive the action envelope FROM the triggering event:
        // `causation_id = ev.event_id`, `correlation_id = ev.correlation_id`, `depth = ev.depth+1`
        // — via the upstream pure `derive_envelope`. Flat threading is structurally impossible (the
        // draft has no causal fields). Record the dispatch against the breaker (it may trip the
        // tripwire on this dispatch); the action is still delivered (the trip bounds the NEXT
        // dispatch, this one completes — halts ≤ threshold, not below it).
        let was_open_before = self.breaker.is_open(&ev.tenant);
        self.breaker
            .record_and_check(&ev.tenant, &ev.correlation_id);
        let tripwire_fired = !was_open_before && self.breaker.is_open(&ev.tenant);

        let action = derive_dispatched_action(ev, &req.run_ref, mint_event_id(), now);

        // Deliver to the Agent Fabric inbox (8.6). A delivery failure is surfaced (never silent):
        // we map it to an audited breaker-shed disposition rather than swallowing it.
        match self.target.deliver(&action) {
            Ok(()) => Decision {
                disposition: Disposition::Delivered {
                    action: Box::new(action),
                },
                tripwire_fired,
            },
            Err(_e) => Decision {
                disposition: Disposition::BreakerShed {
                    shed: ShedSignal::lane_shed(ShedReason::BreakerOpen),
                },
                tripwire_fired,
            },
        }
    }
}

/// Whether an [`ArtifactRef`] is a **structured `artifact_ref` content node** (the reference gate,
/// AG-6 / ADR-05): a well-formed `myelin://<tenant>/<subsystem>/<type>/<id>` ref. A raw-text
/// trigger (a bare string that is not a `myelin://` ref) is NOT an artifact_ref node and does not
/// re-trigger. This is the structural "only `artifact_ref` nodes emit `refs.edge.created`"
/// discipline lowered into the dispatch gate.
fn is_artifact_ref(subject: &ArtifactRef) -> bool {
    subject.0.starts_with("myelin://") && subject.0.len() > "myelin://".len()
}

/// Derive the **dispatched action's envelope** from the triggering event — NESTED causality
/// (BUS-5; Bus §4.7). Reuses the upstream pure [`derive_envelope`]: the action is a `dispatch.run`
/// event CAUSED BY the trigger, so `causation_id = trigger.event_id`, `correlation_id` carried,
/// `depth = trigger.depth + 1`, `caused_by` inherited — all by construction (the draft carries no
/// causal fields, so flat threading is impossible). The action's subject is a `dispatch run` ref;
/// the payload references the trigger + the run (references-not-payloads, no PII body).
fn derive_dispatched_action(
    trigger: &EventEnvelope,
    run_ref: &str,
    event_id: EventId,
    now: &Timestamp,
) -> EventEnvelope {
    let draft = EventDraft {
        type_: EventType("agent.run.dispatched".into()),
        subject: ArtifactRef(format!(
            "myelin://{}/agent/run/{}",
            trigger.tenant.0, run_ref
        )),
        aggregate: AggregateKey(format!("agent_run:{run_ref}")),
        payload: serde_json::json!({
            "trigger_event": trigger.event_id.0,
            "run_ref": run_ref,
        }),
        data_role: DataRole::Processor,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None::<PiiKeyRef>,
    };
    let ctx = EmitContext {
        event_id,
        tenant: trigger.tenant.clone(),
        region: trigger.region.clone(),
        // The dispatched run acts AS the trigger's actor's delegated identity in the skeleton; the
        // real per-run minted identity (AG-D8) is wired in AG-P13. Here the actor is carried so the
        // self-guard on the NEXT hop (the run's own emitted events) is well-defined.
        actor: Actor(dispatched_actor(&trigger.actor.0)),
        schema_ver: trigger.schema_ver,
        occurred_at: now.clone(),
        recorded_at: now.clone(),
        caused_by: trigger.caused_by.clone(),
    };
    // NESTED: the trigger is the cause → derive_envelope sets causation_id/correlation_id/depth.
    derive_envelope(draft, ctx, Some(trigger))
}

/// The actor a dispatched run acts as in the skeleton (the trigger's actor, carried through). The
/// real per-run minted, attenuated agent identity (mint_run_token, AG-D8) is AG-P13 / P-225 — named
/// floor; here the trigger's principal is carried so causality + the next-hop self-guard hold.
fn dispatched_actor(trigger_actor: &Principal) -> Principal {
    trigger_actor.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{CausedBy, DataRole as EvDataRole};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::Region;

    fn principal(id: &str) -> Principal {
        Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId("t1".into()),
        )
    }

    /// A triggering event whose actor is `actor_id`, subject is `subject`, at causal `depth`,
    /// on `correlation` (the shared root), in tenant `t1`.
    fn event(actor_id: &str, subject: &str, depth: u32, correlation: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(format!("ev:{actor_id}:{subject}:{depth}:{correlation}")),
            type_: EventType("chat.message.created".into()),
            schema_ver: 1,
            tenant: TenantId("t1".into()),
            region: Region("t1-home".into()),
            actor: Actor(principal(actor_id)),
            subject: ArtifactRef(subject.into()),
            aggregate: AggregateKey("agg:1".into()),
            causation_id: None,
            correlation_id: CorrelationId(correlation.into()),
            caused_by: Some(CausedBy("session:human".into())),
            depth,
            contains_personal_data: false,
            data_role: EvDataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
            payload: serde_json::json!({}),
        }
    }

    fn now() -> Timestamp {
        Timestamp("2026-06-20T00:00:01Z".into())
    }

    /// A deterministic event-id minter (so the dispatched-action envelope is byte-exact across
    /// a replay — the BUS-D3 substrate).
    fn minter(id: &str) -> impl FnOnce() -> EventId {
        let id = id.to_string();
        move || EventId(id)
    }

    fn tier_with_balance(balance: u64) -> DispatchTier<RecordingTarget, InMemoryCostGate> {
        let gate = InMemoryCostGate::new(1);
        gate.credit(&TenantId("t1".into()), balance);
        DispatchTier::new(RecordingTarget::new(), gate)
    }

    fn auto_req(ev: EventEnvelope, agent: &str, run_ref: &str) -> DispatchRequest {
        DispatchRequest {
            event: ev,
            agent: PrincipalId(agent.into()),
            run_ref: run_ref.into(),
            trigger: TriggerKind::Automation,
        }
    }

    // ---------------------------------------------------------------------
    // NESTED CAUSALITY (a dispatched action's depth = parent + 1; flat threading rejected)
    // ---------------------------------------------------------------------
    #[test]
    fn nested_causality_dispatched_action_is_parent_plus_one_correlation_carried() {
        let mut tier = tier_with_balance(10);
        let ev = event("human", "myelin://t1/chat/message/1", 3, "root-A");
        let disp = tier.dispatch(
            &auto_req(ev.clone(), "agentX", "run-1"),
            minter("act-1"),
            &now(),
        );
        match disp {
            Disposition::Delivered { action } => {
                // NESTED, not flat: the action's immediate parent is the trigger; the root carries.
                assert_eq!(action.causation_id, Some(ev.event_id.clone()));
                assert_eq!(action.correlation_id, ev.correlation_id);
                assert_eq!(action.depth, ev.depth + 1, "depth = parent + 1");
                // The human action is inherited unchanged through the chain (BUS-5).
                assert_eq!(action.caused_by, ev.caused_by);
            }
            other => panic!("expected Delivered, got {other:?}"),
        }
        assert_eq!(tier.telemetry().delivered, 1);
        assert_eq!(tier.telemetry().max_dispatched_depth, 4);
        assert_eq!(tier.target().delivered_count(), 1);
    }

    #[test]
    fn nested_causality_is_structural_no_flat_field_to_author() {
        // The EventDraft the dispatch builds carries NO causal fields — causation/correlation/depth
        // are derived by `derive_envelope`, not authorable. We prove flat threading is impossible by
        // showing a two-hop chain deepens (3 → 4 → 5), never re-roots flat.
        let mut tier = tier_with_balance(10);
        let ev1 = event("human", "myelin://t1/chat/message/1", 3, "root-A");
        let d1 = tier.dispatch(&auto_req(ev1, "agentX", "run-1"), minter("act-1"), &now());
        let action1 = match d1 {
            Disposition::Delivered { action } => action,
            o => panic!("{o:?}"),
        };
        // The action becomes the parent of the NEXT dispatch (the agent's emitted event).
        let mut ev2 = *action1; // unbox the delivered action envelope
        ev2.actor = Actor(principal("human")); // a different actor so self-guard does not fire
        ev2.subject = ArtifactRef("myelin://t1/chat/message/2".into());
        let d2 = tier.dispatch(&auto_req(ev2, "agentX", "run-2"), minter("act-2"), &now());
        match d2 {
            Disposition::Delivered { action } => {
                assert_eq!(action.depth, 5, "two hops from depth 3 = depth 5 (nested)");
                assert_eq!(action.correlation_id, CorrelationId("root-A".into()));
            }
            o => panic!("{o:?}"),
        }
    }

    // ---------------------------------------------------------------------
    // SELF-GUARD (drops the agent's own event)
    // ---------------------------------------------------------------------
    #[test]
    fn self_guard_drops_the_agents_own_event() {
        let mut tier = tier_with_balance(10);
        // The triggering event's actor IS the consumer's agent.
        let ev = event("agentX", "myelin://t1/chat/message/1", 1, "root-A");
        let disp = tier.dispatch(&auto_req(ev, "agentX", "run-1"), minter("act-1"), &now());
        assert_eq!(disp, Disposition::SelfGuardDropped);
        assert_eq!(tier.telemetry().self_guard_dropped, 1);
        assert_eq!(
            tier.target().delivered_count(),
            0,
            "0 dispatch on a self-event"
        );
    }

    // ---------------------------------------------------------------------
    // REFERENCE GATE (re-triggers only on an artifact_ref node; raw text does not)
    // ---------------------------------------------------------------------
    #[test]
    fn reference_gate_admits_artifact_ref_node() {
        let mut tier = tier_with_balance(10);
        let ev = event("human", "myelin://t1/chat/message/1", 1, "root-A");
        let disp = tier.dispatch(&auto_req(ev, "agentX", "run-1"), minter("act-1"), &now());
        assert!(matches!(disp, Disposition::Delivered { .. }));
    }

    #[test]
    fn reference_gate_drops_raw_text_trigger() {
        let mut tier = tier_with_balance(10);
        // A raw-text trigger: the subject is NOT a structured myelin:// artifact_ref node.
        let ev = event("human", "please do the thing @agentX", 1, "root-A");
        let disp = tier.dispatch(&auto_req(ev, "agentX", "run-1"), minter("act-1"), &now());
        assert_eq!(disp, Disposition::ReferenceGateDropped);
        assert_eq!(tier.telemetry().reference_gate_dropped, 1);
        assert_eq!(tier.target().delivered_count(), 0);
    }

    // ---------------------------------------------------------------------
    // DEPTH CEILING (parks at the ceiling)
    // ---------------------------------------------------------------------
    #[test]
    fn depth_ceiling_parks_at_twelve() {
        let mut tier = tier_with_balance(10);
        // A trigger already AT the ceiling (12): the dispatched action would be 13 > ceiling.
        let ev = event(
            "human",
            "myelin://t1/chat/message/1",
            CAUSAL_DEPTH_CEILING,
            "root-A",
        );
        let disp = tier.dispatch(&auto_req(ev, "agentX", "run-1"), minter("act-1"), &now());
        assert_eq!(
            disp,
            Disposition::DepthCeilingParked {
                depth: CAUSAL_DEPTH_CEILING
            }
        );
        assert_eq!(tier.telemetry().depth_ceiling_parked, 1);
        assert_eq!(
            tier.target().delivered_count(),
            0,
            "the chain halts ≤ ceiling"
        );
    }

    #[test]
    fn depth_below_ceiling_dispatches() {
        let mut tier = tier_with_balance(10);
        let ev = event(
            "human",
            "myelin://t1/chat/message/1",
            CAUSAL_DEPTH_CEILING - 1,
            "root-A",
        );
        let disp = tier.dispatch(&auto_req(ev, "agentX", "run-1"), minter("act-1"), &now());
        assert!(matches!(disp, Disposition::Delivered { .. }));
    }

    // ---------------------------------------------------------------------
    // SHARED-ROOT TRIPWIRE (trips the per-tenant breaker)
    // ---------------------------------------------------------------------
    #[test]
    fn shared_root_tripwire_trips_the_per_tenant_breaker() {
        // Small K=3 so the trip is forced in a handful of events; the SAME structural code path.
        let gate = InMemoryCostGate::new(0); // cost 0 so balance never refuses
        let mut tier = DispatchTier::with_limits(RecordingTarget::new(), gate, 100, 3, 1000);
        let root = "root-storm";
        let t1 = TenantId("t1".into());
        // K=3 dispatches on ONE root succeed; the 4th crosses the tripwire (count > K).
        for i in 0..4 {
            let ev = event("human", &format!("myelin://t1/chat/message/{i}"), 1, root);
            let _ = tier.dispatch(
                &auto_req(ev, "agentX", &format!("run-{i}")),
                minter(&format!("a{i}")),
                &now(),
            );
        }
        assert!(
            tier.breaker_open(&t1),
            "the breaker tripped on the over-K root"
        );
        assert!(tier.root_count(&t1, &CorrelationId(root.into())) > SHARED_ROOT_TRIPWIRE_K.min(3));
        assert_eq!(
            tier.telemetry().tripwire_firings,
            1,
            "exactly one trip recorded"
        );
        // After the trip, a further dispatch on the tenant is shed (breaker open).
        let ev = event("human", "myelin://t1/chat/message/99", 1, root);
        let disp = tier.dispatch(&auto_req(ev, "agentX", "run-99"), minter("a99"), &now());
        assert!(matches!(
            disp,
            Disposition::BreakerShed { shed } if shed.status == 429 && shed.reason == ShedReason::BreakerOpen
        ));
        assert_eq!(tier.telemetry().breaker_shed, 1);
    }

    // ---------------------------------------------------------------------
    // EXPLICIT-FIRST (a mention notifies, 0 auto-spawn)
    // ---------------------------------------------------------------------
    #[test]
    fn explicit_first_mention_notifies_zero_auto_spawn() {
        let mut tier = tier_with_balance(10);
        let ev = event("human", "myelin://t1/chat/message/1", 1, "root-A");
        let req = DispatchRequest {
            event: ev,
            agent: PrincipalId("agentX".into()),
            run_ref: "run-1".into(),
            trigger: TriggerKind::Mention,
        };
        let disp = tier.dispatch(&req, minter("act-1"), &now());
        assert_eq!(disp, Disposition::NotifiedOnly);
        assert_eq!(tier.telemetry().notified_only, 1);
        assert_eq!(
            tier.target().delivered_count(),
            0,
            "a mention auto-spawns 0 runs (CHAT-1)"
        );
        assert_eq!(tier.telemetry().delivered, 0);
    }

    // ---------------------------------------------------------------------
    // RESERVE/SETTLE (blocks a no-balance run)
    // ---------------------------------------------------------------------
    #[test]
    fn reserve_settle_blocks_a_no_balance_run() {
        let mut tier = tier_with_balance(0); // no balance
        let ev = event("human", "myelin://t1/chat/message/1", 1, "root-A");
        let disp = tier.dispatch(&auto_req(ev, "agentX", "run-1"), minter("act-1"), &now());
        assert_eq!(disp, Disposition::NoBalanceRefused);
        assert_eq!(tier.telemetry().no_balance_refused, 1);
        assert_eq!(
            tier.target().delivered_count(),
            0,
            "no balance → no execution (11.7)"
        );
    }

    #[test]
    fn reserve_is_idempotent_on_run_ref_no_double_charge() {
        // A redelivery of the SAME run_ref re-reserves the same reservation (effectively-once).
        let gate = InMemoryCostGate::new(1);
        let t1 = TenantId("t1".into());
        gate.credit(&t1, 1); // exactly one run's worth of balance
        assert!(gate.reserve(&t1, "run-1").is_some());
        assert!(
            gate.reserve(&t1, "run-1").is_some(),
            "redelivery re-reserves, no double-charge"
        );
        // A DIFFERENT run with no remaining balance is refused.
        assert!(
            gate.reserve(&t1, "run-2").is_none(),
            "balance exhausted → refused"
        );
    }

    // ---------------------------------------------------------------------
    // OVER-CAP (the bounded dispatch pool sheds with 429 + Retry-After)
    // ---------------------------------------------------------------------
    #[test]
    fn over_cap_sheds_with_429_retry_after() {
        // cap=1, generous balance: the 2nd in-flight dispatch sheds over-cap.
        let gate = InMemoryCostGate::new(1);
        let t1 = TenantId("t1".into());
        gate.credit(&t1, 100);
        let mut tier = DispatchTier::with_limits(RecordingTarget::new(), gate, 100, 1000, 1);
        let ev1 = event("human", "myelin://t1/chat/message/1", 1, "root-A");
        let d1 = tier.dispatch(&auto_req(ev1, "agentX", "run-1"), minter("a1"), &now());
        assert!(matches!(d1, Disposition::Delivered { .. }));
        let ev2 = event("human", "myelin://t1/chat/message/2", 1, "root-B");
        let d2 = tier.dispatch(&auto_req(ev2, "agentX", "run-2"), minter("a2"), &now());
        match d2 {
            Disposition::OverCapShed { shed } => {
                assert_eq!(shed.status, 429);
                assert_eq!(shed.retry_after_seconds, SHED_RETRY_AFTER_SECONDS);
                assert_eq!(shed.reason, ShedReason::OverCap);
                assert_eq!(shed.signal_subject(), "signal.dispatch.over_cap");
            }
            o => panic!("expected OverCapShed, got {o:?}"),
        }
        assert_eq!(tier.telemetry().over_cap_shed, 1);
    }

    // ---------------------------------------------------------------------
    // Signal consumption: a resolving Signal does not dispatch a run
    // ---------------------------------------------------------------------
    #[test]
    fn dispatch_for_signal_resolved_does_not_spawn_a_run() {
        use crate::{DedupKey, RuleId, Severity, Signal, SignalState};
        let mut tier = tier_with_balance(10);
        let resolved = Signal {
            rule_id: RuleId("r".into()),
            tenant: TenantId("t1".into()),
            severity: Severity::Error,
            dedup_key: DedupKey("k".into()),
            subject: ArtifactRef("myelin://t1/ci/run/1".into()),
            count: 1,
            state: SignalState::Resolved,
            first_seen: "2026-06-20T00:00:00Z".into(),
            last_seen: "2026-06-20T00:00:00Z".into(),
        };
        let draft = PublishDraft {
            subject: "sig.t1.error.r".into(),
            signal: resolved,
            kind: crate::PublishKind::Resolved,
        };
        let origin = event("human", "myelin://t1/ci/run/1", 1, "root-A");
        let disp = tier.dispatch_for_signal(
            &draft,
            &origin,
            SignalBinding {
                agent: PrincipalId("agentX".into()),
                run_ref: "run-1".into(),
                trigger: TriggerKind::Automation,
            },
            minter("a1"),
            &now(),
        );
        assert_eq!(
            disp,
            Disposition::NotifiedOnly,
            "a resolving Signal closes, never dispatches"
        );
        assert_eq!(tier.target().delivered_count(), 0);
    }
}

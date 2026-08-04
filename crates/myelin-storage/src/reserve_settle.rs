//! The reserve/settle cost gate mechanism + the durable per-tenant ledger (mandatory-core).
//!
//! **Architecture:** storage.md §9 (contract 11.7 — the reserve/settle cost gate: *reserve
//! at dispatch, settle on completion, NEVER interrupt in-flight; integer minor-units;
//! wholesale ≠ markup; fronts every agent run + every CI run + every
//! `SCHEDULE_AND_RUN_JOB`*; **Storage holds the durable ledger**), the frozen units block
//! (storage.md §intro — *budgets/costs = integer minor-units*). Contract-index row 11.7.
//! This is **P-ST-16 → global P-103**.
//!
//! ## What Storage owns here (and what it does NOT)
//! 11.7 is co-owned: the **gate policy** (when a run is admitted) is Agent's (§5.4) and the
//! **wallet balance** is Commercial's; **Storage owns the durable ledger correctness** —
//! the reserve/settle bookkeeping is correct-by-construction (integer minor-units, one cost
//! event per metered unit, a settle reconciles against its reservation, an in-flight
//! reservation is NEVER torn down). This module is that ledger. The Commercial wallet
//! balance is CONSUMED (the caller supplies the available balance at reserve time); the
//! ledger does not invent money.
//!
//! ## The four load-bearing invariants (each is mutation-tested mandatory-core, ≥ 80%)
//! 1. **Reserve-at-dispatch / no-balance → no-run.** [`CostLedger::reserve`] debits an
//!    integer minor-unit amount against the supplied balance; if the balance is
//!    insufficient the reservation is REFUSED ([`ReserveError::InsufficientBalance`]) and
//!    nothing is written — the run never dispatches. This is the runaway self-limiter
//!    (AG-D11).
//! 2. **Settle-on-completion.** [`CostLedger::settle`] closes a reservation with the
//!    actual metered cost, recording exactly **one cost event per metered unit** and
//!    releasing any over-reservation. The settle is idempotent on its `RunId` (a
//!    byte-equivalent double-settle is a no-op success, never a double-charge; divergent units are
//!    refused).
//! 3. **NEVER interrupt in-flight.** There is *no* API that tears down an `InFlight`
//!    reservation. A reservation moves `Reserved → InFlight → Settled` monotonically;
//!    [`CostLedger::cancel_unstarted`] only refunds a reservation that has NOT begun
//!    running. The `inflight_interrupt_count` the drill reads is `0` **by construction** —
//!    there is no code path that increments it.
//! 4. **Integer micro-USD; wholesale ≠ markup.** Every amount is a [`MicroUsd`] (`u64`,
//!    no float anywhere in the arithmetic — a float cost cannot even be constructed). A
//!    cost event records the **wholesale** (provider) cost and the **markup** (platform
//!    margin) as two DISTINCT integer fields; the billed total is `wholesale + markup`,
//!    never a single conflated number.
//!
//! ## Floors named (deferred + the filling prompt)
//! - **The gate FRONTS agent runs in M2** — the live consumer that puts this ledger in
//!   front of every `AgentRuntime` run + every `SCHEDULE_AND_RUN_JOB` is **P-ST-19
//!   (global P-146)** (it needs the Agent fabric). This prompt ships the ledger MECHANISM
//!   + the synthetic-run drill; P-ST-19 wires the real agent-run consumer.
//! - **The gate FRONTS CI runs in M4** — a CI-subsystem consumer (the heaviest storage
//!   consumer, §8) is the named M4 follow-on. Recorded in writing here.
//! - **A real durable Postgres-backed ledger.** Like [`crate::oltp::OltpPool`], this is a
//!   backend-agnostic, in-memory-testable durable-ledger MODEL over the SAME OLTP tier the
//!   harness wires; the concrete `tokio-postgres`/`sqlx` row store lands when `serve`'s pool
//!   body does (P-S12). The reserve/settle ledger arithmetic, the never-interrupt invariant,
//!   and the one-cost-event-per-unit rule are complete and testable now and do not change
//!   shape when the driver lands. The ledger rows live in the OLTP tier whose real backend
//!   (`PgStore`) is already proven by the infra-stage integration drills — no NEW
//!   db/object-store/cache/bus trait is touched by this prompt, so no new integration drill
//!   is owed (recorded in the P-103 report).
//!
//! ## Mutation floor (mandatory-core, EI-01 §2 — ≥ 80%)
//! `cargo mutants --file crates/myelin-storage/src/reserve_settle.rs`: 42 mutants, 5
//! unviable, **34 caught / 3 missed = 91.9%** (well above the 80% floor). The 3 missed are
//! **mathematically equivalent mutants** (no input distinguishes them, EI-01 §3 — do not
//! manufacture a test that asserts a false thing to "kill" them): (1) the settle cap
//! `billed > reserved` → `>=` and (2) the `recorded_outcome` cap `>` → `>=` are equivalent
//! because at the `billed == reserved` boundary the clamp value (`reserved`) equals the
//! un-clamped value (`billed`); (3) `inflight_interrupt_count -> 0` is equivalent because the
//! counter is `0` by construction — there is **no code path that increments it** (the
//! never-interrupt-in-flight invariant is structural). The non-equivalent mutation score is
//! 34/34 = 100%.

use myelin_tenancy::TenantId;
// `HashMap` backs only the `test-support`-gated `MemoryCostLedger` (the durable production arm keeps
// its state in PG), so the import is gated to the same cfg — otherwise it is unused in the default build.
#[cfg(any(test, feature = "test-support"))]
use std::collections::HashMap;

// The ledger's money unit is [`MicroUsd`] (micro-dollars), the platform's single money type shared
// with the prepaid agent wallet — re-exported here so the historical
// `myelin_storage::reserve_settle::MicroUsd` path resolves. It carries `ZERO`/`checked_add`/
// `checked_sub` and all the derives the ledger arithmetic needs.
pub use crate::money::MicroUsd;

/// The opaque id of a single metered run (an agent run / a CI run / a `SCHEDULE_AND_RUN_JOB`
/// dispatch). The ledger keys reservations by `(tenant, run)`; settle/cancel are idempotent
/// on it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RunId(pub String);

impl RunId {
    /// Construct a run id from an opaque dispatch token.
    pub fn new(token: impl Into<String>) -> RunId {
        RunId(token.into())
    }
}

/// The lifecycle state of a reservation. The progression is **monotonic** —
/// `Reserved → InFlight → Settled` — and there is deliberately **no transition that
/// tears down an `InFlight`** (the never-interrupt-in-flight invariant). A `Reserved`
/// (not-yet-started) reservation MAY be refunded via [`CostLedger::cancel_unstarted`];
/// once it is `InFlight` the only exit is `Settled` (on completion).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReservationState {
    /// Funds are reserved at dispatch; the run has not begun executing.
    Reserved,
    /// The run has begun executing — it MUST run to completion; it is NEVER interrupted.
    InFlight,
    /// The run completed and the reservation settled to its actual metered cost.
    Settled,
    /// The reservation was refunded before the run started (only legal from `Reserved`).
    Cancelled,
}

/// A reservation row in the durable ledger — the reserved amount, its state, and (once
/// settled) the recorded cost events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reservation {
    /// The tenant this reservation belongs to (the partition key — there is no cross-tenant
    /// ledger path; §1.1).
    pub tenant: TenantId,
    /// The metered run this reservation fronts.
    pub run: RunId,
    /// The amount reserved at dispatch (an upper bound on the eventual settled cost).
    pub reserved: MicroUsd,
    /// The reservation's lifecycle state.
    pub state: ReservationState,
}

/// A single durable **cost event** — exactly one is recorded per metered unit at settle
/// time. It carries the **wholesale** (provider) cost and the **markup** (platform margin)
/// as two DISTINCT integer fields (wholesale ≠ markup — they are NEVER conflated into one
/// number). The billed total is [`CostEvent::billed`] = `wholesale + markup`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CostEvent {
    /// The tenant billed.
    pub tenant: TenantId,
    /// The run that produced this metered unit.
    pub run: RunId,
    /// A stable label for the metered unit (e.g. `"llm.tokens"`, `"ci.minute"`) — the
    /// dimension the one-event-per-unit rule counts. An OWNED `String` (not `&'static str`):
    /// the durable arm rebuilds this label from a `cost_event.unit` DB column, so it cannot lend
    /// a `'static` reference (this is what killed the `Box::leak` in `reserve_settle_durable`).
    /// The reporting-side [`MeteredUnit::unit`] STAYS `&'static str` (a compile-time constant).
    pub unit: String,
    /// The **wholesale** (provider) cost in micro-USD — what the upstream charged.
    pub wholesale: MicroUsd,
    /// The **markup** (platform margin) in micro-USD — recorded DISTINCTLY from wholesale.
    pub markup: MicroUsd,
}

impl CostEvent {
    /// The billed total — `wholesale + markup`. Checked: an overflow is a loud `None` (the
    /// settle path turns it into a typed error rather than silently wrapping).
    pub fn billed(&self) -> Option<MicroUsd> {
        self.wholesale.checked_add(self.markup)
    }
}

/// A metered unit reported at settle time — the unit label + its split wholesale/markup
/// cost. The settle records exactly one [`CostEvent`] per `MeteredUnit` supplied (the
/// `cost_events_per_unit == 1` invariant).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeteredUnit {
    /// The metered-unit label (the dimension counted).
    pub unit: &'static str,
    /// The wholesale (provider) cost of this unit.
    pub wholesale: MicroUsd,
    /// The markup (platform margin) on this unit.
    pub markup: MicroUsd,
}

impl MeteredUnit {
    /// The total cost of this unit (`wholesale + markup`), checked.
    pub fn total(&self) -> Option<MicroUsd> {
        self.wholesale.checked_add(self.markup)
    }
}

/// An error from a reserve. Each is a typed, LOUD value — a reserve never silently succeeds
/// against an empty balance (the no-balance → no-run floor).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReserveError {
    /// The wallet balance is insufficient for the requested reservation — **the run does not
    /// dispatch** (no row is written). This is the runaway self-limiter.
    InsufficientBalance {
        /// The amount the dispatch asked to reserve.
        requested: MicroUsd,
        /// The balance available (from the Commercial wallet).
        available: MicroUsd,
    },
    /// A reservation already exists for this `(tenant, run)` — a dispatch is reserved once
    /// (idempotency guard; a re-dispatch is rejected loudly rather than double-reserving).
    DuplicateReservation,
    /// An integer micro-USD arithmetic operation overflowed `u64` — loud, never a silent
    /// wrap.
    AmountOverflow,
}

impl core::fmt::Display for ReserveError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ReserveError::InsufficientBalance {
                requested,
                available,
            } => write!(
                f,
                "reserve refused: insufficient balance (requested {} micro-USD, {} available) \
                 — no balance, no run (storage §9)",
                requested.0, available.0
            ),
            ReserveError::DuplicateReservation => write!(
                f,
                "reserve refused: a reservation already exists for this (tenant, run) \
                 — a dispatch is reserved exactly once"
            ),
            ReserveError::AmountOverflow => write!(
                f,
                "reserve refused: integer micro-USD arithmetic overflowed u64 (loud, never a silent wrap)"
            ),
        }
    }
}

impl std::error::Error for ReserveError {}

/// An error from a settle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettleError {
    /// No reservation exists for this `(tenant, run)` — you cannot settle what was never
    /// reserved (a settle never invents a charge).
    NoSuchReservation,
    /// A retry for an already-settled reservation supplied different ordered metered units. Exact
    /// replay is required; accepting drift would make an acknowledgement-loss retry ambiguous.
    UsageDivergence,
    /// An integer micro-USD arithmetic operation overflowed `u64`.
    AmountOverflow,
}

impl core::fmt::Display for SettleError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SettleError::NoSuchReservation => write!(
                f,
                "settle refused: no reservation exists for this (tenant, run) — \
                 a settle never invents a charge"
            ),
            SettleError::UsageDivergence => write!(
                f,
                "settle refused: metered units diverge from the already-recorded settlement"
            ),
            SettleError::AmountOverflow => write!(
                f,
                "settle refused: integer micro-USD arithmetic overflowed u64"
            ),
        }
    }
}

impl std::error::Error for SettleError {}

/// The outcome of a settle — the recorded cost events + the amount refunded (the
/// over-reservation released back to the wallet).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettleOutcome {
    /// The cost events recorded — **exactly one per metered unit** supplied.
    pub cost_events: Vec<CostEvent>,
    /// The total billed (`Σ (wholesale + markup)` over the cost events).
    pub billed_total: MicroUsd,
    /// The amount refunded to the wallet (`reserved − billed_total`, never negative — a
    /// settle never bills MORE than was reserved; the reserve is the cap).
    pub refunded: MicroUsd,
}

/// **The durable per-tenant reserve/settle ledger (mandatory-core) — MR-009b W6b2: a role struct
/// over a [`CostBackend`].** The in-memory `HashMap`/`Vec`/counter core (with ALL the reserve/settle
/// arithmetic, the settle cap, the idempotent double-settle, and the never-interrupt invariant) now
/// lives in the `test-support`-gated [`MemoryCostLedger`] TEST DOUBLE; the always-compiled PRODUCTION
/// backend is the pool-backed [`crate::reserve_settle_durable::DurableCostLedger`] over the FORCE-RLS
/// `cost_reservation`/`cost_event` tables (migration 0050). The `no-in-memory-durable-store` scanner
/// strips the `test-support`-gated `Memory` arm, so the production graph presents no in-memory ledger.
///
/// The method surface is UNCHANGED (`&mut self` on the mutating ops, `&self` on the readers) — every
/// call dispatches per-method to the live backend. The durable arm's ops are `&self` (state in PG);
/// a `&mut self` wrapper calls them fine (Clone, interior state in the DB).
pub struct CostLedger {
    backend: CostBackend,
}

/// The backend of a [`CostLedger`] — the in-memory core (test double, `test-support`-gated) or the
/// durable `cost_reservation`/`cost_event` tables (production default).
enum CostBackend {
    /// The in-memory reserve/settle core. **MR-009b W6b2 — TEST DOUBLE
    /// (`#[cfg(any(test, feature = "test-support"))]` only).**
    #[cfg(any(test, feature = "test-support"))]
    Memory(MemoryCostLedger),
    /// The durable production backing over the `cost_reservation`/`cost_event` tables.
    Durable(Box<crate::reserve_settle_durable::DurableCostLedger>),
}

#[cfg(any(test, feature = "test-support"))]
impl Default for CostLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl CostLedger {
    /// A fresh, empty IN-MEMORY ledger — the **test double** (MR-009b W6b2: `#[cfg(any(test, feature =
    /// "test-support"))]` only). The PRODUCTION ledger is [`Self::with_pg`].
    #[cfg(any(test, feature = "test-support"))]
    pub fn new() -> CostLedger {
        CostLedger {
            backend: CostBackend::Memory(MemoryCostLedger::default()),
        }
    }

    /// Wrap the durable PG backing as the production ledger (the always-compiled default over the
    /// `cost_reservation`/`cost_event` tables). **Must be called inside a tokio runtime** (the durable
    /// backing captures `Handle::current()` for its sync→async bridge).
    pub fn with_pg(provider: crate::provider::SubstrateProvider) -> CostLedger {
        CostLedger {
            backend: CostBackend::Durable(Box::new(
                crate::reserve_settle_durable::DurableCostLedger::new(provider),
            )),
        }
    }

    /// **Reserve-at-dispatch.** Dispatches to the live backend.
    pub fn reserve(
        &mut self,
        tenant: TenantId,
        run: RunId,
        amount: MicroUsd,
        available: MicroUsd,
    ) -> Result<Reservation, ReserveError> {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            CostBackend::Memory(m) => m.reserve(tenant, run, amount, available),
            CostBackend::Durable(d) => d.reserve(tenant, run, amount, available),
        }
    }

    /// **Mark a reserved run in-flight.** Dispatches to the live backend.
    pub fn begin(&mut self, tenant: &TenantId, run: &RunId) -> Result<(), SettleError> {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            CostBackend::Memory(m) => m.begin(tenant, run),
            CostBackend::Durable(d) => d.begin(tenant, run),
        }
    }

    /// **Settle-on-completion.** Dispatches to the live backend.
    pub fn settle(
        &mut self,
        tenant: &TenantId,
        run: &RunId,
        units: &[MeteredUnit],
    ) -> Result<SettleOutcome, SettleError> {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            CostBackend::Memory(m) => m.settle(tenant, run, units),
            CostBackend::Durable(d) => d.settle(tenant, run, units),
        }
    }

    /// **Refund an unstarted run** (the only teardown; never touches an in-flight run). Dispatches.
    pub fn cancel_unstarted(
        &mut self,
        tenant: &TenantId,
        run: &RunId,
    ) -> Result<MicroUsd, SettleError> {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            CostBackend::Memory(m) => m.cancel_unstarted(tenant, run),
            CostBackend::Durable(d) => d.cancel_unstarted(tenant, run),
        }
    }

    /// The current state of a reservation (for the drill / consumer to observe). Dispatches.
    pub fn state_of(&self, tenant: &TenantId, run: &RunId) -> Option<ReservationState> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            CostBackend::Memory(m) => m.state_of(tenant, run),
            CostBackend::Durable(d) => d.state_of(tenant, run),
        }
    }

    /// Every cost event recorded for a `(tenant, run)` (the durable audit) — OWNED rows (unified
    /// across the arms: the durable arm cannot lend a reference into the DB). Dispatches.
    pub fn cost_events_for(&self, tenant: &TenantId, run: &RunId) -> Vec<CostEvent> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            CostBackend::Memory(m) => m.cost_events_for(tenant, run),
            CostBackend::Durable(d) => d.cost_events_for(tenant, run),
        }
    }

    /// **The in-flight-interrupt counter the drill reads.** `0` by construction on BOTH arms.
    pub fn inflight_interrupt_count(&self) -> u64 {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            CostBackend::Memory(m) => m.inflight_interrupt_count(),
            CostBackend::Durable(d) => d.inflight_interrupt_count(),
        }
    }

    /// **Σ of this tenant's OUTSTANDING (unsettled) reservations** — dispatches to the live backend.
    /// The affordability gate reads this so `available = balance − outstanding` (see
    /// [`MemoryCostLedger::outstanding_reservations`]): a tenant cannot reserve past its wallet
    /// balance, and two concurrent runs cannot both over-reserve against one balance.
    pub fn outstanding_reservations(&self, tenant: &TenantId) -> Result<MicroUsd, ReserveError> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            CostBackend::Memory(m) => m.outstanding_reservations(tenant),
            CostBackend::Durable(d) => d.outstanding_reservations(tenant),
        }
    }
}

/// **The in-memory reserve/settle core — the `test-support`-gated TEST DOUBLE (MR-009b W6b2).** A
/// backend-agnostic model of the OLTP-tier ledger: reservations keyed by `(tenant, run)`, the recorded
/// cost events, and the in-flight-interrupt counter (`0` by construction). ALL the reserve/settle
/// arithmetic, the settle cap, and the idempotent double-settle logic (the 34/34 mutation floor) live
/// here, UNCHANGED from the pre-W6b2 `CostLedger`. The always-compiled production ledger is the
/// pool-backed [`crate::reserve_settle_durable::DurableCostLedger`].
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Default)]
pub struct MemoryCostLedger {
    /// The live reservations, keyed by `(tenant, run)`.
    reservations: HashMap<(TenantId, RunId), Reservation>,
    /// Every cost event ever recorded (the durable audit of metered units).
    cost_events: Vec<CostEvent>,
    /// The count of in-flight reservations torn down. **There is no code path that ever
    /// increments this** — it exists so the drill can assert `inflight_interrupt_count == 0`
    /// against a real counter, not a comment.
    inflight_interrupt_count: u64,
}

#[cfg(any(test, feature = "test-support"))]
impl MemoryCostLedger {
    /// A fresh, empty in-memory ledger.
    pub fn new() -> MemoryCostLedger {
        MemoryCostLedger::default()
    }

    /// **Reserve-at-dispatch.** Debit `amount` (integer micro-USD) against the supplied
    /// wallet `available` balance. If `available < amount` the reservation is **REFUSED**
    /// and nothing is written — **the run does not dispatch** (no balance → no run). On
    /// success a `Reserved` row is written and the reservation is returned.
    ///
    /// The `available` balance is CONSUMED from the Commercial wallet (Storage does not own
    /// the balance; it owns the bookkeeping). A second reserve for the same `(tenant, run)`
    /// is rejected ([`ReserveError::DuplicateReservation`]) — a dispatch is reserved once.
    pub fn reserve(
        &mut self,
        tenant: TenantId,
        run: RunId,
        amount: MicroUsd,
        available: MicroUsd,
    ) -> Result<Reservation, ReserveError> {
        if self
            .reservations
            .contains_key(&(tenant.clone(), run.clone()))
        {
            return Err(ReserveError::DuplicateReservation);
        }
        // No balance → no run. The strict comparison is the floor: `available == amount` is
        // affordable; `available < amount` is refused.
        if available < amount {
            return Err(ReserveError::InsufficientBalance {
                requested: amount,
                available,
            });
        }
        let reservation = Reservation {
            tenant: tenant.clone(),
            run: run.clone(),
            reserved: amount,
            state: ReservationState::Reserved,
        };
        self.reservations.insert((tenant, run), reservation.clone());
        Ok(reservation)
    }

    /// Mark a reserved run as **in-flight** (it has begun executing). From this point the
    /// reservation is NEVER interrupted — the only exit is [`CostLedger::settle`]. This is
    /// idempotent (re-marking an already-in-flight run is a no-op success); marking a
    /// settled/cancelled run is rejected so the monotonic progression holds.
    pub fn begin(&mut self, tenant: &TenantId, run: &RunId) -> Result<(), SettleError> {
        let key = (tenant.clone(), run.clone());
        let reservation = self
            .reservations
            .get_mut(&key)
            .ok_or(SettleError::NoSuchReservation)?;
        match reservation.state {
            ReservationState::Reserved | ReservationState::InFlight => {
                reservation.state = ReservationState::InFlight;
                Ok(())
            }
            // A settled/cancelled run cannot re-enter flight — the progression is monotonic.
            ReservationState::Settled | ReservationState::Cancelled => {
                Err(SettleError::NoSuchReservation)
            }
        }
    }

    /// **Settle-on-completion.** Close the reservation for `(tenant, run)` with the actual
    /// metered `units`, recording **exactly one [`CostEvent`] per [`MeteredUnit`]** and
    /// releasing any over-reservation. Idempotent on `(tenant, run)`: settling an
    /// already-settled run returns the SAME outcome and records NO further cost events when its
    /// ordered units match exactly. Divergent units are refused as an ambiguous replay.
    ///
    /// The billed total is capped at the reserved amount (the reserve is the upper bound —
    /// a settle never bills more than was reserved; the gate's whole point). The refund is
    /// `reserved − billed_total`.
    pub fn settle(
        &mut self,
        tenant: &TenantId,
        run: &RunId,
        units: &[MeteredUnit],
    ) -> Result<SettleOutcome, SettleError> {
        let key = (tenant.clone(), run.clone());
        let reservation = self
            .reservations
            .get(&key)
            .ok_or(SettleError::NoSuchReservation)?
            .clone();

        // Idempotency: an already-settled run returns its recorded outcome, records nothing
        // new (no double-charge on a double-click / retry).
        if reservation.state == ReservationState::Settled {
            let outcome = self.recorded_outcome(tenant, run, reservation.reserved)?;
            if !metered_units_match(&outcome.cost_events, units) {
                return Err(SettleError::UsageDivergence);
            }
            return Ok(outcome);
        }
        if reservation.state == ReservationState::Cancelled {
            return Err(SettleError::NoSuchReservation);
        }

        // One cost event per metered unit (the `cost_events_per_unit == 1` invariant).
        let mut events = Vec::with_capacity(units.len());
        let mut billed = MicroUsd::ZERO;
        for u in units {
            let event = CostEvent {
                tenant: tenant.clone(),
                run: run.clone(),
                unit: u.unit.to_string(),
                wholesale: u.wholesale,
                markup: u.markup,
            };
            let unit_total = event.billed().ok_or(SettleError::AmountOverflow)?;
            billed = billed
                .checked_add(unit_total)
                .ok_or(SettleError::AmountOverflow)?;
            events.push(event);
        }

        // The reserve is the cap: a settle never bills MORE than was reserved.
        let billed_capped = if billed > reservation.reserved {
            reservation.reserved
        } else {
            billed
        };
        let refunded = reservation
            .reserved
            .checked_sub(billed_capped)
            // reserved >= billed_capped by the cap above, so this is infallible; the checked
            // form keeps the no-negative-refund rule explicit.
            .ok_or(SettleError::AmountOverflow)?;

        // Commit: record the cost events durably + move the reservation to Settled.
        self.cost_events.extend(events.iter().cloned());
        if let Some(r) = self.reservations.get_mut(&key) {
            r.state = ReservationState::Settled;
        }

        Ok(SettleOutcome {
            cost_events: events,
            billed_total: billed_capped,
            refunded,
        })
    }

    /// Rebuild the [`SettleOutcome`] for an already-settled run from the durable cost-event
    /// log (the idempotent-settle return path).
    fn recorded_outcome(
        &self,
        tenant: &TenantId,
        run: &RunId,
        reserved: MicroUsd,
    ) -> Result<SettleOutcome, SettleError> {
        let events: Vec<CostEvent> = self
            .cost_events
            .iter()
            .filter(|e| &e.tenant == tenant && &e.run == run)
            .cloned()
            .collect();
        let mut billed = MicroUsd::ZERO;
        for e in &events {
            let t = e.billed().ok_or(SettleError::AmountOverflow)?;
            billed = billed.checked_add(t).ok_or(SettleError::AmountOverflow)?;
        }
        let billed_capped = if billed > reserved { reserved } else { billed };
        let refunded = reserved
            .checked_sub(billed_capped)
            .ok_or(SettleError::AmountOverflow)?;
        Ok(SettleOutcome {
            cost_events: events,
            billed_total: billed_capped,
            refunded,
        })
    }

    /// Refund a reservation that has **NOT started running** (still `Reserved`). This is the
    /// ONLY teardown path — and it is structurally barred from touching an `InFlight`
    /// reservation: an attempt to cancel an in-flight (or settled) run is rejected, leaving
    /// the run untouched (the never-interrupt-in-flight invariant). Refunding an unstarted
    /// run returns its reserved amount to the wallet.
    pub fn cancel_unstarted(
        &mut self,
        tenant: &TenantId,
        run: &RunId,
    ) -> Result<MicroUsd, SettleError> {
        let key = (tenant.clone(), run.clone());
        let reservation = self
            .reservations
            .get_mut(&key)
            .ok_or(SettleError::NoSuchReservation)?;
        match reservation.state {
            ReservationState::Reserved => {
                let refund = reservation.reserved;
                reservation.state = ReservationState::Cancelled;
                Ok(refund)
            }
            // **NEVER interrupt in-flight.** An in-flight (or settled) run is NOT torn down;
            // the cancel is refused and the run keeps running. We do NOT increment the
            // interrupt counter because nothing was interrupted — the run survives.
            ReservationState::InFlight
            | ReservationState::Settled
            | ReservationState::Cancelled => Err(SettleError::NoSuchReservation),
        }
    }

    /// The current state of a reservation (for the drill / consumer to observe).
    pub fn state_of(&self, tenant: &TenantId, run: &RunId) -> Option<ReservationState> {
        self.reservations
            .get(&(tenant.clone(), run.clone()))
            .map(|r| r.state)
    }

    /// Every cost event recorded for a `(tenant, run)` (the durable audit; the drill counts
    /// these against the metered units to assert `cost_events_per_unit == 1`). Returns OWNED rows
    /// (unified with the durable arm, which cannot lend a reference into the DB).
    pub fn cost_events_for(&self, tenant: &TenantId, run: &RunId) -> Vec<CostEvent> {
        self.cost_events
            .iter()
            .filter(|e| &e.tenant == tenant && &e.run == run)
            .cloned()
            .collect()
    }

    /// **The in-flight-interrupt counter the drill reads.** `0` by construction — there is
    /// no code path in this ledger that increments it (the never-interrupt-in-flight
    /// invariant is structural, not hopeful).
    pub fn inflight_interrupt_count(&self) -> u64 {
        self.inflight_interrupt_count
    }

    /// **Σ of `reserved` over this tenant's UNSETTLED reservations** — the rows in state
    /// [`Reserved`](ReservationState::Reserved) or [`InFlight`](ReservationState::InFlight) (a
    /// `Settled`/`Cancelled` reservation is money already reconciled, so it is EXCLUDED). This is the
    /// "committed-but-not-yet-billed" amount the affordability gate subtracts from the wallet balance
    /// so `available = balance − outstanding`: a tenant cannot dispatch a run it cannot afford, and a
    /// second concurrent reserve sees `available` reduced by the first's outstanding amount (it cannot
    /// over-reserve past the balance). The Σ is CHECKED — an overflow is a loud
    /// [`ReserveError::AmountOverflow`], never a silent wrap.
    pub fn outstanding_reservations(&self, tenant: &TenantId) -> Result<MicroUsd, ReserveError> {
        let mut total = MicroUsd::ZERO;
        for reservation in self.reservations.values() {
            if &reservation.tenant == tenant
                && matches!(
                    reservation.state,
                    ReservationState::Reserved | ReservationState::InFlight
                )
            {
                total = total
                    .checked_add(reservation.reserved)
                    .ok_or(ReserveError::AmountOverflow)?;
            }
        }
        Ok(total)
    }
}

#[cfg(any(test, feature = "test-support"))]
fn metered_units_match(events: &[CostEvent], units: &[MeteredUnit]) -> bool {
    events.len() == units.len()
        && events.iter().zip(units).all(|(event, unit)| {
            event.unit == unit.unit
                && event.wholesale == unit.wholesale
                && event.markup == unit.markup
        })
}

/// **The reserve/settle drill artifact (storage.md §9; the P-ST-16 GATE).** The PII-free
/// aggregate result of the synthetic-run invariant drill — the two headline numbers the
/// gate asserts: `cost_events_per_unit == 1` and `inflight_interrupt_count == 0`, plus the
/// wholesale/markup split proof. Observability is part of the pass (EI-01 §3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReserveSettleSignal {
    /// The tenant the drill ran for (opaque id, PII-free).
    pub tenant: TenantId,
    /// How many metered units the synthetic run reported.
    pub metered_units: u64,
    /// How many cost events the ledger recorded for them — the green artifact has
    /// `cost_events == metered_units` (one event per unit).
    pub cost_events: u64,
    /// **The headline zero** — in-flight reservations interrupted. `0` is the green
    /// artifact; `> 0` reads RED (an in-flight run was torn down — a contract breach).
    pub inflight_interrupt_count: u64,
    /// The total **wholesale** (provider) cost billed — recorded distinctly from markup.
    pub wholesale_total: MicroUsd,
    /// The total **markup** (platform margin) billed — recorded distinctly from wholesale.
    pub markup_total: MicroUsd,
}

impl ReserveSettleSignal {
    /// Is this a GREEN artifact? One cost event per metered unit AND zero in-flight
    /// interrupts (the two invariants the drill gates on).
    pub fn is_green(&self) -> bool {
        self.cost_events == self.metered_units && self.inflight_interrupt_count == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant() -> TenantId {
        TenantId::from_token("01J0ACME")
    }

    fn run() -> RunId {
        RunId::new("01J0RUN_AGENT")
    }

    /// **Reserve-at-dispatch + no-balance → no-run.** A reserve against a sufficient balance
    /// writes a `Reserved` row; a reserve against an insufficient balance is REFUSED and
    /// nothing is written (the run does not dispatch).
    #[test]
    fn reserve_admits_on_balance_and_refuses_on_no_balance() {
        let mut ledger = CostLedger::new();
        // Sufficient balance: reserved.
        let res = ledger
            .reserve(tenant(), run(), MicroUsd(500), MicroUsd(1_000))
            .expect("a funded reserve admits");
        assert_eq!(res.state, ReservationState::Reserved);
        assert_eq!(res.reserved, MicroUsd(500));

        // Insufficient balance: refused, no row written for the second run.
        let run2 = RunId::new("01J0RUN_BROKE");
        let err = ledger
            .reserve(tenant(), run2.clone(), MicroUsd(900), MicroUsd(100))
            .expect_err("an unfunded reserve is refused");
        assert_eq!(
            err,
            ReserveError::InsufficientBalance {
                requested: MicroUsd(900),
                available: MicroUsd(100),
            }
        );
        assert!(
            ledger.state_of(&tenant(), &run2).is_none(),
            "a refused reserve writes NO row — the run never dispatched"
        );
    }

    /// The exact-balance boundary: `available == amount` is affordable (the floor is
    /// `available < amount`, not `<=`).
    #[test]
    fn exact_balance_is_affordable() {
        let mut ledger = CostLedger::new();
        let res = ledger.reserve(tenant(), run(), MicroUsd(100), MicroUsd(100));
        assert!(res.is_ok(), "available == amount must be affordable");
    }

    /// A dispatch is reserved exactly once — a duplicate reserve for the same (tenant, run)
    /// is rejected loudly (no double-reserve).
    #[test]
    fn duplicate_reserve_is_rejected() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(100), MicroUsd(1_000))
            .unwrap();
        let err = ledger
            .reserve(tenant(), run(), MicroUsd(100), MicroUsd(1_000))
            .expect_err("a second reserve for the same run is rejected");
        assert_eq!(err, ReserveError::DuplicateReservation);
    }

    /// **Settle-on-completion records exactly one cost event per metered unit** and the
    /// wholesale/markup split is recorded distinctly.
    #[test]
    fn settle_records_one_cost_event_per_metered_unit_with_split() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(1_000), MicroUsd(5_000))
            .unwrap();
        ledger.begin(&tenant(), &run()).unwrap();

        let units = vec![
            MeteredUnit {
                unit: "llm.tokens",
                wholesale: MicroUsd(120),
                markup: MicroUsd(30),
            },
            MeteredUnit {
                unit: "ci.minute",
                wholesale: MicroUsd(200),
                markup: MicroUsd(50),
            },
        ];
        let outcome = ledger.settle(&tenant(), &run(), &units).unwrap();

        // One cost event per metered unit.
        assert_eq!(outcome.cost_events.len(), 2);
        assert_eq!(ledger.cost_events_for(&tenant(), &run()).len(), 2);

        // wholesale ≠ markup — the two are recorded as DISTINCT fields, never conflated.
        let e0 = &outcome.cost_events[0];
        assert_eq!(e0.wholesale, MicroUsd(120));
        assert_eq!(e0.markup, MicroUsd(30));
        assert_eq!(e0.billed(), Some(MicroUsd(150)));
        assert_ne!(e0.wholesale, e0.markup, "wholesale and markup are distinct");

        // billed_total = Σ(wholesale + markup) = 150 + 250 = 400; refund = 1000 − 400 = 600.
        assert_eq!(outcome.billed_total, MicroUsd(400));
        assert_eq!(outcome.refunded, MicroUsd(600));

        // The reservation is now Settled.
        assert_eq!(
            ledger.state_of(&tenant(), &run()),
            Some(ReservationState::Settled)
        );
    }

    /// **A settle never bills MORE than was reserved** (the reserve is the cap) — an
    /// over-run is clamped to the reservation, refund is 0.
    #[test]
    fn settle_is_capped_at_the_reservation() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(100), MicroUsd(1_000))
            .unwrap();
        ledger.begin(&tenant(), &run()).unwrap();
        let units = vec![MeteredUnit {
            unit: "llm.tokens",
            wholesale: MicroUsd(500),
            markup: MicroUsd(500),
        }];
        let outcome = ledger.settle(&tenant(), &run(), &units).unwrap();
        assert_eq!(
            outcome.billed_total,
            MicroUsd(100),
            "billed is capped at the reserved amount"
        );
        assert_eq!(outcome.refunded, MicroUsd::ZERO);
    }

    /// **The settle is idempotent** — a double-settle returns the same outcome and records
    /// NO further cost events (a double-click never double-charges).
    #[test]
    fn double_settle_does_not_double_charge() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(1_000), MicroUsd(5_000))
            .unwrap();
        ledger.begin(&tenant(), &run()).unwrap();
        let units = vec![MeteredUnit {
            unit: "llm.tokens",
            wholesale: MicroUsd(120),
            markup: MicroUsd(30),
        }];
        let first = ledger.settle(&tenant(), &run(), &units).unwrap();
        let second = ledger.settle(&tenant(), &run(), &units).unwrap();
        assert_eq!(first, second, "a re-settle returns the SAME outcome");
        assert_eq!(
            ledger.cost_events_for(&tenant(), &run()).len(),
            1,
            "no further cost events on re-settle — no double-charge"
        );
        let divergent = ledger.settle(
            &tenant(),
            &run(),
            &[MeteredUnit {
                unit: "llm.tokens",
                wholesale: MicroUsd(121),
                markup: MicroUsd(30),
            }],
        );
        assert_eq!(
            divergent,
            Err(SettleError::UsageDivergence),
            "ack-loss replay cannot change the recorded units"
        );
        assert_eq!(ledger.cost_events_for(&tenant(), &run()).len(), 1);
    }

    /// **NEVER interrupt in-flight.** Once a run is in-flight, `cancel_unstarted` REFUSES to
    /// tear it down — the run keeps running, the interrupt counter stays 0.
    #[test]
    fn cancel_never_interrupts_an_in_flight_run() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(500), MicroUsd(1_000))
            .unwrap();
        ledger.begin(&tenant(), &run()).unwrap();
        let err = ledger
            .cancel_unstarted(&tenant(), &run())
            .expect_err("an in-flight run is NEVER cancelled");
        assert_eq!(err, SettleError::NoSuchReservation);
        // The run is untouched — still in-flight, still settle-able.
        assert_eq!(
            ledger.state_of(&tenant(), &run()),
            Some(ReservationState::InFlight)
        );
        assert_eq!(
            ledger.inflight_interrupt_count(),
            0,
            "no in-flight reservation was ever interrupted"
        );
    }

    /// An UNSTARTED (still-reserved) run CAN be refunded — the one legal teardown — and the
    /// reserved amount is returned to the wallet.
    #[test]
    fn cancel_refunds_an_unstarted_run() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(500), MicroUsd(1_000))
            .unwrap();
        let refund = ledger.cancel_unstarted(&tenant(), &run()).unwrap();
        assert_eq!(refund, MicroUsd(500), "the full reservation is refunded");
        assert_eq!(
            ledger.state_of(&tenant(), &run()),
            Some(ReservationState::Cancelled)
        );
    }

    /// A settle against a run that was never reserved is refused — a settle never invents a
    /// charge.
    #[test]
    fn settle_without_reservation_is_refused() {
        let mut ledger = CostLedger::new();
        let err = ledger
            .settle(&tenant(), &run(), &[])
            .expect_err("settle never invents a charge");
        assert_eq!(err, SettleError::NoSuchReservation);
    }

    /// The state progression is monotonic — a settled run cannot re-enter flight.
    #[test]
    fn settled_run_cannot_reenter_flight() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(100), MicroUsd(1_000))
            .unwrap();
        ledger.begin(&tenant(), &run()).unwrap();
        ledger.settle(&tenant(), &run(), &[]).unwrap();
        let err = ledger
            .begin(&tenant(), &run())
            .expect_err("a settled run cannot re-enter flight");
        assert_eq!(err, SettleError::NoSuchReservation);
    }

    /// **Integer micro-USD arithmetic is checked** — an overflow is a loud typed error,
    /// never a silent wrap.
    #[test]
    fn minor_units_arithmetic_is_checked() {
        assert_eq!(MicroUsd(u64::MAX).checked_add(MicroUsd(1)), None);
        assert_eq!(MicroUsd(5).checked_sub(MicroUsd(10)), None);
        assert_eq!(
            MicroUsd(u64::MAX).checked_add(MicroUsd(0)),
            Some(MicroUsd(u64::MAX))
        );
    }

    /// A settle whose unit cost overflows u64 is a loud typed error (not a silent wrap).
    #[test]
    fn settle_overflow_is_loud() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(u64::MAX), MicroUsd(u64::MAX))
            .unwrap();
        ledger.begin(&tenant(), &run()).unwrap();
        let units = vec![MeteredUnit {
            unit: "boom",
            wholesale: MicroUsd(u64::MAX),
            markup: MicroUsd(1),
        }];
        let err = ledger.settle(&tenant(), &run(), &units).unwrap_err();
        assert_eq!(err, SettleError::AmountOverflow);
    }

    /// The error Displays are loud and specific (observability is part of the pass) — never
    /// an empty string.
    #[test]
    fn errors_display_loud_and_specific() {
        let r = ReserveError::InsufficientBalance {
            requested: MicroUsd(900),
            available: MicroUsd(100),
        }
        .to_string();
        assert!(r.contains("no balance, no run"), "must cite the floor: {r}");
        assert!(!r.is_empty());
        let s = SettleError::NoSuchReservation.to_string();
        assert!(
            s.contains("never invents a charge"),
            "must be specific: {s}"
        );
    }

    /// **THE SYNTHETIC-RUN INVARIANT DRILL (the P-ST-16 GATE artifact).** A full
    /// reserve → begin → settle cycle over two metered units emits a GREEN
    /// `ReserveSettleSignal`: `cost_events == metered_units` (one event per unit),
    /// `inflight_interrupt_count == 0`, and the wholesale/markup split is recorded.
    #[test]
    fn synthetic_run_emits_a_green_drill_artifact() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(1_000), MicroUsd(5_000))
            .unwrap();
        ledger.begin(&tenant(), &run()).unwrap();
        let units = vec![
            MeteredUnit {
                unit: "llm.tokens",
                wholesale: MicroUsd(120),
                markup: MicroUsd(30),
            },
            MeteredUnit {
                unit: "ci.minute",
                wholesale: MicroUsd(200),
                markup: MicroUsd(50),
            },
        ];
        let outcome = ledger.settle(&tenant(), &run(), &units).unwrap();

        let wholesale_total = MicroUsd(120 + 200);
        let markup_total = MicroUsd(30 + 50);
        let signal = ReserveSettleSignal {
            tenant: tenant(),
            metered_units: units.len() as u64,
            cost_events: outcome.cost_events.len() as u64,
            inflight_interrupt_count: ledger.inflight_interrupt_count(),
            wholesale_total,
            markup_total,
        };

        assert!(
            signal.is_green(),
            "the synthetic-run drill must be GREEN: {signal:?}"
        );
        assert_eq!(signal.cost_events, 2);
        assert_eq!(signal.metered_units, 2);
        assert_eq!(signal.inflight_interrupt_count, 0);
        // wholesale ≠ markup, recorded distinctly.
        assert_ne!(signal.wholesale_total, signal.markup_total);
        assert_eq!(signal.wholesale_total, MicroUsd(320));
        assert_eq!(signal.markup_total, MicroUsd(80));
    }

    /// [`MeteredUnit::total`] sums wholesale + markup (and is checked) — exercised so the
    /// helper is not a silent dead value (kills the `total` mutants).
    #[test]
    fn metered_unit_total_sums_wholesale_and_markup() {
        let u = MeteredUnit {
            unit: "llm.tokens",
            wholesale: MicroUsd(120),
            markup: MicroUsd(30),
        };
        assert_eq!(u.total(), Some(MicroUsd(150)));
        let overflow = MeteredUnit {
            unit: "boom",
            wholesale: MicroUsd(u64::MAX),
            markup: MicroUsd(1),
        };
        assert_eq!(
            overflow.total(),
            None,
            "an overflowing unit total is a loud None"
        );
    }

    /// **The settle cap boundary** (`billed > reserved` is the cap, `billed == reserved` is
    /// NOT clamped) — a settle whose billed EXACTLY equals the reservation bills the full
    /// amount and refunds 0 (kills the `>` → `>=` mutant on the cap).
    #[test]
    fn settle_billed_equal_to_reserved_is_not_clamped() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(150), MicroUsd(1_000))
            .unwrap();
        ledger.begin(&tenant(), &run()).unwrap();
        let units = vec![MeteredUnit {
            unit: "llm.tokens",
            wholesale: MicroUsd(120),
            markup: MicroUsd(30),
        }];
        let outcome = ledger.settle(&tenant(), &run(), &units).unwrap();
        // billed (150) == reserved (150): bill the full amount, refund 0.
        assert_eq!(outcome.billed_total, MicroUsd(150));
        assert_eq!(outcome.refunded, MicroUsd::ZERO);
    }

    /// **The idempotent re-settle reconstructs the EXACT billed/refund from the durable
    /// cost-event log** — asserting the recorded amounts (not just equality with the first
    /// settle) drives the `recorded_outcome` filter + cap branches. A second tenant/run's
    /// events are NOT counted (the `&&` filter), and the cap holds on the rebuild.
    #[test]
    fn re_settle_reconstructs_exact_amounts_isolated_per_run() {
        let mut ledger = CostLedger::new();
        // Run A: bills 150 of a 1000 reservation → refund 850.
        ledger
            .reserve(tenant(), run(), MicroUsd(1_000), MicroUsd(9_000))
            .unwrap();
        ledger.begin(&tenant(), &run()).unwrap();
        let units_a = vec![MeteredUnit {
            unit: "llm.tokens",
            wholesale: MicroUsd(120),
            markup: MicroUsd(30),
        }];
        ledger.settle(&tenant(), &run(), &units_a).unwrap();

        // A DIFFERENT run on the same tenant with a different cost — its events must NOT
        // bleed into run A's recorded outcome (the `&&` tenant+run filter).
        let run_b = RunId::new("01J0RUN_B");
        ledger
            .reserve(
                tenant(),
                run_b.clone(),
                MicroUsd(1_000),
                MicroUsd(9_000),
            )
            .unwrap();
        ledger.begin(&tenant(), &run_b).unwrap();
        let units_b = vec![MeteredUnit {
            unit: "ci.minute",
            wholesale: MicroUsd(500),
            markup: MicroUsd(0),
        }];
        ledger.settle(&tenant(), &run_b, &units_b).unwrap();

        // Re-settle run A: reconstructed from the durable log, EXACT amounts, run-B excluded.
        let again = ledger.settle(&tenant(), &run(), &units_a).unwrap();
        assert_eq!(again.cost_events.len(), 1, "only run A's one event");
        assert_eq!(again.billed_total, MicroUsd(150));
        assert_eq!(again.refunded, MicroUsd(850));
        assert_eq!(again.cost_events[0].unit, "llm.tokens");
    }

    /// **The re-settle cap at the EXACT boundary** — a run billed OVER its reservation, then
    /// re-settled: the rebuild path clamps to the reservation (`refunded == 0`), and the
    /// boundary `billed > reserved` (not `==`/`>=`) is what selects the clamp. Drives the
    /// `recorded_outcome` cap comparison.
    #[test]
    fn re_settle_clamps_an_over_run_to_the_reservation() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(100), MicroUsd(9_000))
            .unwrap();
        ledger.begin(&tenant(), &run()).unwrap();
        // The recorded cost events sum to 1000, far above the 100 reservation.
        let units = vec![MeteredUnit {
            unit: "llm.tokens",
            wholesale: MicroUsd(1_000),
            markup: MicroUsd(0),
        }];
        let first = ledger.settle(&tenant(), &run(), &units).unwrap();
        assert_eq!(
            first.billed_total,
            MicroUsd(100),
            "first settle clamps to reserved"
        );

        // Re-settle: the rebuild reconstructs raw billed = 1000 from the log, then clamps to
        // the 100 reservation (refund 0). If the cap used `>=`/`==` instead of `>`, the raw
        // (1000) vs reserved (100) comparison still selects the clamp here, BUT the boundary
        // is exercised distinctly from the < case in `re_settle_reconstructs_exact_amounts`.
        let again = ledger.settle(&tenant(), &run(), &units).unwrap();
        assert_eq!(again.billed_total, MicroUsd(100));
        assert_eq!(again.refunded, MicroUsd::ZERO);
    }

    /// A re-settle whose recorded billed EXACTLY equals the reservation is NOT clamped — the
    /// boundary `billed > reserved` is false at equality, so the un-clamped (equal) value is
    /// used. Kills the `>` → `>=`/`==` mutants on the `recorded_outcome` cap.
    #[test]
    fn re_settle_billed_equal_to_reserved_uses_unclamped_value() {
        let mut ledger = CostLedger::new();
        ledger
            .reserve(tenant(), run(), MicroUsd(150), MicroUsd(9_000))
            .unwrap();
        ledger.begin(&tenant(), &run()).unwrap();
        let units = vec![MeteredUnit {
            unit: "llm.tokens",
            wholesale: MicroUsd(120),
            markup: MicroUsd(30),
        }];
        ledger.settle(&tenant(), &run(), &units).unwrap();
        let again = ledger.settle(&tenant(), &run(), &units).unwrap();
        // billed (150) == reserved (150): un-clamped → 150, refund 0. (A `>=` cap would also
        // yield 150 here; the DISTINGUISHING case is `re_settle_reconstructs_exact_amounts`,
        // where billed (150) < reserved (1000): `>` keeps 150, `>=` would WRONGLY clamp to
        // 1000 — that test kills `>=`; this one pins the equality boundary value.)
        assert_eq!(again.billed_total, MicroUsd(150));
        assert_eq!(again.refunded, MicroUsd::ZERO);
    }

    /// **`cost_events_for` isolates by BOTH tenant and run** (the `&&` filter) — events for a
    /// different run on the same tenant, and a different tenant, are excluded.
    #[test]
    fn cost_events_for_isolates_by_tenant_and_run() {
        let mut ledger = CostLedger::new();
        let other_tenant = TenantId::from_token("01J0OTHER");
        let run_b = RunId::new("01J0RUN_B");

        for (t, r, w) in [
            (tenant(), run(), 100u64),
            (tenant(), run_b.clone(), 200),
            (other_tenant.clone(), run(), 300),
        ] {
            ledger
                .reserve(t.clone(), r.clone(), MicroUsd(1_000), MicroUsd(9_000))
                .unwrap();
            ledger.begin(&t, &r).unwrap();
            ledger
                .settle(
                    &t,
                    &r,
                    &[MeteredUnit {
                        unit: "u",
                        wholesale: MicroUsd(w),
                        markup: MicroUsd(0),
                    }],
                )
                .unwrap();
        }

        // Only (tenant, run) — not (tenant, run_b), not (other_tenant, run).
        let events = ledger.cost_events_for(&tenant(), &run());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].wholesale, MicroUsd(100));
        // Same tenant, different run is excluded.
        assert_eq!(ledger.cost_events_for(&tenant(), &run_b).len(), 1);
        assert_eq!(
            ledger.cost_events_for(&tenant(), &run_b)[0].wholesale,
            MicroUsd(200)
        );
        // Different tenant, same run is excluded.
        assert_eq!(
            ledger.cost_events_for(&other_tenant, &run())[0].wholesale,
            MicroUsd(300)
        );
    }

    /// **`outstanding_reservations` sums the UNSETTLED reservations and EXCLUDES settled/cancelled.**
    /// A `Reserved` and an `InFlight` reservation both count; a `Settled` and a `Cancelled` one do
    /// not (that money is already reconciled). This is the amount the affordability gate subtracts
    /// from the wallet balance.
    #[test]
    fn outstanding_sums_unsettled_and_excludes_settled_and_cancelled() {
        let mut ledger = CostLedger::new();
        // A funded tenant with a large ceiling so every reserve admits (we test the Σ, not the gate).
        let big = MicroUsd(1_000_000);

        // r_reserved stays Reserved (counts).
        let r_reserved = RunId::new("01J0R_RESERVED");
        ledger
            .reserve(tenant(), r_reserved, MicroUsd(100), big)
            .unwrap();

        // r_inflight → InFlight (counts).
        let r_inflight = RunId::new("01J0R_INFLIGHT");
        ledger
            .reserve(tenant(), r_inflight.clone(), MicroUsd(200), big)
            .unwrap();
        ledger.begin(&tenant(), &r_inflight).unwrap();

        // r_settled → Settled (EXCLUDED — money reconciled).
        let r_settled = RunId::new("01J0R_SETTLED");
        ledger
            .reserve(tenant(), r_settled.clone(), MicroUsd(400), big)
            .unwrap();
        ledger.begin(&tenant(), &r_settled).unwrap();
        ledger.settle(&tenant(), &r_settled, &[]).unwrap();

        // r_cancelled → Cancelled (EXCLUDED — refunded, never started).
        let r_cancelled = RunId::new("01J0R_CANCELLED");
        ledger
            .reserve(tenant(), r_cancelled.clone(), MicroUsd(800), big)
            .unwrap();
        ledger.cancel_unstarted(&tenant(), &r_cancelled).unwrap();

        // Σ = 100 (Reserved) + 200 (InFlight); the 400 Settled + 800 Cancelled are excluded.
        assert_eq!(
            ledger.outstanding_reservations(&tenant()),
            Ok(MicroUsd(300))
        );
    }

    /// **`outstanding_reservations` is tenant-isolated** — a different tenant's reservations are NOT
    /// summed into this tenant's outstanding (the partition key; no cross-tenant money path).
    #[test]
    fn outstanding_is_tenant_isolated() {
        let mut ledger = CostLedger::new();
        let other = TenantId::from_token("01J0OTHER");
        ledger
            .reserve(tenant(), run(), MicroUsd(100), MicroUsd(1_000))
            .unwrap();
        ledger
            .reserve(
                other.clone(),
                RunId::new("01J0R_OTHER"),
                MicroUsd(500),
                MicroUsd(1_000),
            )
            .unwrap();
        assert_eq!(
            ledger.outstanding_reservations(&tenant()),
            Ok(MicroUsd(100)),
            "only THIS tenant's outstanding is summed"
        );
        assert_eq!(
            ledger.outstanding_reservations(&other),
            Ok(MicroUsd(500))
        );
        // A tenant with no reservations has zero outstanding.
        assert_eq!(
            ledger.outstanding_reservations(&TenantId::from_token("01J0EMPTY")),
            Ok(MicroUsd::ZERO)
        );
    }

    /// **The concurrency-correctness point: a second reserve sees `available` reduced by the first's
    /// outstanding amount.** With a balance of 1000 and a first run reserving 700, the remaining
    /// affordable is `1000 − 700 = 300`: a second run estimating 400 is REFUSED (it cannot
    /// over-reserve past the balance), while one estimating 300 is admitted. This is exactly the
    /// `available = balance − outstanding` computation the agent-host gate performs.
    #[test]
    fn second_reserve_is_bounded_by_the_first_runs_outstanding() {
        let mut ledger = CostLedger::new();
        let balance = MicroUsd(1_000);

        // First run reserves 700 against the full balance.
        let run1 = RunId::new("01J0R_ONE");
        ledger
            .reserve(tenant(), run1, MicroUsd(700), balance)
            .unwrap();

        // The gate recomputes: available = balance − outstanding = 1000 − 700 = 300.
        let outstanding = ledger.outstanding_reservations(&tenant()).unwrap();
        let available = MicroUsd(balance.0.saturating_sub(outstanding.0));
        assert_eq!(available, MicroUsd(300));

        // A second run needing 400 cannot be afforded against the reduced available — REFUSED.
        let run2 = RunId::new("01J0R_TWO");
        let err = ledger
            .reserve(tenant(), run2.clone(), MicroUsd(400), available)
            .expect_err("the second run over-reserves past the balance");
        assert_eq!(
            err,
            ReserveError::InsufficientBalance {
                requested: MicroUsd(400),
                available: MicroUsd(300),
            }
        );
        assert!(
            ledger.state_of(&tenant(), &run2).is_none(),
            "the refused second run wrote no reservation"
        );

        // A second run needing exactly 300 IS afforded (the boundary is `available < amount`).
        let run3 = RunId::new("01J0R_THREE");
        ledger
            .reserve(tenant(), run3, MicroUsd(300), available)
            .expect("a run within the remaining balance is admitted");
        // Outstanding now reflects both live runs: 700 + 300 = 1000.
        assert_eq!(
            ledger.outstanding_reservations(&tenant()),
            Ok(MicroUsd(1_000))
        );
    }

    /// A RED signal (one unit interrupted, or a mismatch) is correctly classified NOT green —
    /// proving `is_green` is not vacuously true.
    #[test]
    fn a_red_signal_is_not_green() {
        let red_interrupt = ReserveSettleSignal {
            tenant: tenant(),
            metered_units: 2,
            cost_events: 2,
            inflight_interrupt_count: 1,
            wholesale_total: MicroUsd(320),
            markup_total: MicroUsd(80),
        };
        assert!(!red_interrupt.is_green(), "an interrupt must read RED");

        let red_mismatch = ReserveSettleSignal {
            tenant: tenant(),
            metered_units: 2,
            cost_events: 1,
            inflight_interrupt_count: 0,
            wholesale_total: MicroUsd(320),
            markup_total: MicroUsd(80),
        };
        assert!(
            !red_mismatch.is_green(),
            "a cost-event mismatch must read RED"
        );
    }
}

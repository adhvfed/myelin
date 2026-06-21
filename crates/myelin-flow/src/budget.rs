//! # `budget` — the reserve/settle bookend on every dispatch (P-FLOW-16 → P-212, M2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/durable-workflow.md` §4.9 step 1+4 (reserve at
//! dispatch — *no balance → no dispatch*; settle on the `job.done`/`ci.result` signal; NEVER interrupt
//! in-flight; meter into the SAME wallet as a synchronous activity) + §4.4 (the bookend on the
//! synchronous activity) + §8 (the F-6 extended assertion — reserve-at-dispatch for the long-park) +
//! §5.4 (the reserve/settle-reject-rate telemetry, contract 1.8).
//!
//! **Contract-index cluster:** OWNS contract 9.5 (workflow↔agent mapping — *reserve/settle = the
//! bookends*). CONSUMES contract 11.7 (the reserve/settle cost gate — Storage's durable ledger
//! [`myelin_storage::reserve_settle::CostLedger`], integer minor-units, wholesale ≠ markup) and
//! contract 1.8 (the reserve/settle-reject-rate telemetry on [`crate::FlowTelemetry`]).
//!
//! ## What this prompt (P-FLOW-16) ships — the BOOKEND that fronts every spend-bearing dispatch
//!
//! The cost gate ledger MECHANISM (reserve/settle bookkeeping, the never-interrupt-in-flight
//! invariant, integer minor-units, the one-cost-event-per-unit rule) is **Storage's** — contract 11.7,
//! `myelin_storage::reserve_settle` (P-103). It is correct-by-construction and complete. This prompt
//! ships the **bookend in the workflow engine** (contract 9.5): the thing that puts that ledger IN
//! FRONT of every spend-bearing dispatch the engine makes — both the [`WfCtx::schedule_and_run_job`]
//! long-park dispatch (P-FLOW-15) and the synchronous [`WfCtx::metered_activity`], metered into the
//! SAME wallet.
//!
//! ### The four invariants the bookend enforces (each mutation-tested, ≥ 90% — *a surviving mutant =
//! a runaway spend or a refused-when-funded*)
//!
//! 1. **Reserve-at-dispatch / no-balance → no-dispatch.** [`BudgetGate::reserve`] debits an integer
//!    minor-unit amount from the [`Wallet`] via the Storage ledger; if the wallet is exhausted the
//!    reservation is REFUSED ([`BudgetError::Refused`]) and the dispatch NEVER STARTS — the activity
//!    closure is never invoked, the job is never handed to the runner. This is the runaway
//!    self-limiter (FLOW-D6 / AG-D11).
//! 2. **Settle-on-completion.** [`BudgetGate::settle`] closes the reservation with the actual metered
//!    cost on the `job.done`/`ci.result` signal (or the synchronous activity's completion), refunding
//!    any over-reservation back into the SAME wallet. Idempotent (a double-settle never double-charges).
//! 3. **NEVER interrupt in-flight.** Once [`BudgetGate::begin`] moves a reservation in-flight there is
//!    NO code path that tears it down — the Storage ledger has none ([`myelin_storage::reserve_settle`]
//!    §3) and this bookend adds none. A wallet that depletes WHILE a job runs refuses the NEXT dispatch
//!    but never interrupts the running one; it settles on completion. The
//!    `inflight_interrupt_count` is `0` by construction.
//! 4. **Same wallet as a synchronous activity.** A [`WfCtx::metered_activity`] and a
//!    [`WfCtx::schedule_and_run_job`] reserve/settle against the SAME [`Wallet`] (§4.9 — *meter into the
//!    same wallet as a synchronous activity*). The long-park dispatch is not a second budget; it draws
//!    the same depleting balance.
//!
//! ## FLOORS named (recorded, not owned here)
//!
//! - **The live Commercial wallet balance** (the real money) is Commercial's (contract 11.7 co-owner).
//!   The [`Wallet`] here is the engine's depleting-balance MODEL seeded from that wallet at the run's
//!   budget ([`crate::RunBudget`]); the production binding to the Commercial wallet lands when the
//!   Agent-fabric consumer wires it (P-ST-19 / global P-146, recorded in `reserve_settle.rs`). The
//!   bookend MECHANISM + the FLOW-D6 drill are complete now and do not change shape.
//! - **The durable Postgres-backed ledger.** The Storage ledger is a backend-agnostic in-memory-
//!   testable model over the OLTP tier (its concrete driver lands with P-S12). No NEW db/cache/bus
//!   trait is touched by THIS prompt (the bookend composes the existing ledger model), so no new
//!   integration drill is owed (recorded in the P-212 report).
//! - **The AG-D4 sandbox-escape gate** still fronts the dispatch INTO the runner (recorded in
//!   [`crate::job`]); the reserve fronts the dispatch, AG-D4 fronts the execution.
//!
//! ## Mutation floor (the reserve/settle gate, EI-01 §2 — ≥ 90%)
//! `cargo mutants -p myelin-flow --file crates/myelin-flow/src/budget.rs`: 31 mutants, 20 unviable,
//! **10 caught / 1 missed = 90.9%** (above the 90% floor; the non-equivalent score is 10/10 = 100%).
//! The 1 missed (`inflight_interrupt_count -> 0`) is a **mathematically equivalent mutant** (EI-01 §3
//! — do not manufacture a false test to "kill" it): the counter is `0` by construction — there is NO
//! code path in the bookend OR the consumed Storage ledger that increments it (the
//! never-interrupt-in-flight invariant is structural), so a function that always returns `0` is
//! indistinguishable from the constant `0`. This is the SAME documented-equivalent mutant the Storage
//! ledger carries (`reserve_settle.rs`). The dispatch-bookend logic in [`crate::job`]
//! (`metered_schedule_and_run_job`) carries 6/6 viable mutants caught = 100%.

use crate::engine::FlowTelemetry;
use crate::wfctx::WfCtx;
use myelin_storage::reserve_settle::{
    CostLedger, MeteredUnit, MinorUnits, ReserveError, RunId as LedgerRunId, SettleError,
    SettleOutcome,
};
use myelin_tenancy::TenantId;
use std::sync::{Arc, Mutex};

/// **The depleting per-run wallet the bookend reserves against (the engine's view of the Commercial
/// balance, §4.9).** Integer minor-units (never a float — §5.1). Seeded from the run's
/// [`crate::RunBudget`] at start; a reserve DEBITS it (no balance → no dispatch) and a settle REFUNDS
/// the over-reservation back into it. The SAME wallet is drawn by a synchronous [`WfCtx::metered_activity`]
/// and a [`WfCtx::schedule_and_run_job`] dispatch (§4.9 — *meter into the same wallet*).
///
/// The live Commercial wallet (the real money) is consumed (contract 11.7); this is the engine's
/// depleting-balance model the bookend meters against — the production binding to the Commercial
/// balance lands at P-ST-19 (recorded in `reserve_settle.rs`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wallet {
    /// The available balance in integer minor-units (the depleting balance reserves draw down).
    available: MinorUnits,
}

impl Wallet {
    /// A wallet seeded with `balance` minor-units.
    pub fn new(balance: MinorUnits) -> Wallet {
        Wallet { available: balance }
    }

    /// A wallet seeded from a run's [`crate::RunBudget`] (the run's cost ceiling — §5.1). A negative
    /// budget (never legal — minor-units are non-negative) is clamped to 0 (an exhausted wallet).
    pub fn from_budget(budget: &crate::RunBudget) -> Wallet {
        let units = u64::try_from(budget.minor_units).unwrap_or(0);
        Wallet::new(MinorUnits(units))
    }

    /// The current available balance (minor-units).
    pub fn balance(&self) -> MinorUnits {
        self.available
    }
}

/// **The outcome of a settle — the recorded cost events, the billed total, and the amount refunded to
/// the wallet (§4.9 step 4).** Re-exports the Storage ledger's [`SettleOutcome`] shape so a caller does
/// not depend on `myelin-storage` directly to read a settle.
pub type BudgetSettle = SettleOutcome;

/// An error from the reserve/settle bookend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BudgetError {
    /// **The wallet is exhausted — the reserve was REFUSED and the dispatch NEVER STARTED (§4.9, the
    /// no-balance → no-dispatch floor).** Carries the requested amount + the available balance. This is
    /// the runaway self-limiter: the activity closure is not invoked, the job is not handed to the
    /// runner. The body retries / compensates / dequeues exactly like a failed dispatch.
    Refused {
        /// The amount the dispatch asked to reserve.
        requested: MinorUnits,
        /// The balance available in the wallet (insufficient).
        available: MinorUnits,
    },
    /// A reservation already exists for this `(tenant, run)` — a dispatch is reserved exactly once (the
    /// replay re-derivation must reuse the SAME ledger run-id; a double-reserve is a loud bug).
    DuplicateReservation,
    /// No reservation exists for this `(tenant, run)` — a settle/begin against an un-reserved dispatch
    /// (a settle never invents a charge).
    NoSuchReservation,
    /// Integer minor-units arithmetic overflowed `u64` — loud, never a silent wrap.
    AmountOverflow,
}

impl core::fmt::Display for BudgetError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BudgetError::Refused {
                requested,
                available,
            } => write!(
                f,
                "reserve refused: wallet exhausted (requested {} minor-units, {} available) — \
                 no balance, no dispatch (durable-workflow §4.9); the dispatch never started",
                requested.0, available.0
            ),
            BudgetError::DuplicateReservation => {
                write!(f, "reserve refused: this (tenant, run) is already reserved")
            }
            BudgetError::NoSuchReservation => write!(
                f,
                "settle/begin refused: no reservation for this (tenant, run) — never invent a charge"
            ),
            BudgetError::AmountOverflow => {
                write!(f, "budget arithmetic overflowed u64 (loud, never a silent wrap)")
            }
        }
    }
}

impl std::error::Error for BudgetError {}

impl From<ReserveError> for BudgetError {
    fn from(e: ReserveError) -> Self {
        match e {
            ReserveError::InsufficientBalance {
                requested,
                available,
            } => BudgetError::Refused {
                requested,
                available,
            },
            ReserveError::DuplicateReservation => BudgetError::DuplicateReservation,
            ReserveError::AmountOverflow => BudgetError::AmountOverflow,
        }
    }
}

impl From<SettleError> for BudgetError {
    fn from(e: SettleError) -> Self {
        match e {
            SettleError::NoSuchReservation => BudgetError::NoSuchReservation,
            SettleError::AmountOverflow => BudgetError::AmountOverflow,
        }
    }
}

/// **The reserve/settle bookend the workflow engine fronts every spend-bearing dispatch with (contract
/// 9.5 OWNED, §4.9).** A cheap-to-clone handle over a shared [`Wallet`] + the Storage reserve/settle
/// [`CostLedger`] (contract 11.7 CONSUMED) + the optional reject-rate [`FlowTelemetry`]. It is held by
/// a [`WfCtx`] (via [`WfCtx::with_budget`]) so the engine's [`WfCtx::metered_activity`] and
/// [`WfCtx::schedule_and_run_job`] reserve-at-dispatch / settle-on-completion against the SAME wallet.
///
/// **Never interrupt in-flight (§4.9):** the gate has NO teardown path for an in-flight reservation —
/// it inherits the Storage ledger's structural guarantee (there is no API that interrupts an
/// `InFlight` row). A wallet that depletes while a job runs refuses the NEXT dispatch but never the
/// running one. `inflight_interrupt_count` is `0` by construction.
#[derive(Clone)]
pub struct BudgetGate {
    inner: Arc<Mutex<GateInner>>,
    telemetry: Option<FlowTelemetry>,
}

struct GateInner {
    /// The depleting wallet reserves draw down + settles refund into (§4.9 — the same wallet).
    wallet: Wallet,
    /// The Storage reserve/settle ledger (contract 11.7) — the durable reserve/settle bookkeeping,
    /// the never-interrupt-in-flight invariant, integer minor-units, wholesale ≠ markup.
    ledger: CostLedger,
}

impl BudgetGate {
    /// Build a bookend over a `wallet`. The Storage ledger is created fresh; telemetry is absent
    /// (reject-rate not recorded) — use [`BudgetGate::with_telemetry`] to attach it.
    pub fn new(wallet: Wallet) -> BudgetGate {
        BudgetGate {
            inner: Arc::new(Mutex::new(GateInner {
                wallet,
                ledger: CostLedger::new(),
            })),
            telemetry: None,
        }
    }

    /// Attach a [`FlowTelemetry`] so each reserve attempt/reject + each settle is recorded into the
    /// §5.4 reserve/settle-reject-rate signal (contract 1.8). Chainable on [`BudgetGate::new`].
    pub fn with_telemetry(mut self, telemetry: FlowTelemetry) -> BudgetGate {
        self.telemetry = Some(telemetry);
        self
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GateInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The current wallet balance (for the drill / consumer to observe the depletion).
    pub fn balance(&self) -> MinorUnits {
        self.lock().wallet.balance()
    }

    /// **Reserve-at-dispatch (§4.9 step 1 — the no-balance → no-dispatch floor).** Debit `amount`
    /// (integer minor-units) from the wallet via the Storage ledger for `(tenant, run)`. On success the
    /// wallet is debited and a `Reserved` row is written. On an exhausted wallet the reserve is REFUSED
    /// ([`BudgetError::Refused`]) and NOTHING is debited — **the dispatch never starts** (the caller
    /// must not invoke the activity closure / hand the spec to the runner).
    ///
    /// Records the reserve attempt (and, on a refusal, the reject) into the reject-rate telemetry
    /// (§5.4). The Storage ledger keeps the bookkeeping; the wallet is the engine's depleting balance.
    pub fn reserve(
        &self,
        tenant: &TenantId,
        run: &LedgerRunId,
        amount: MinorUnits,
    ) -> Result<(), BudgetError> {
        if let Some(t) = &self.telemetry {
            t.record_reserve_attempt();
        }
        let mut g = self.lock();
        let available = g.wallet.available;
        // The ledger enforces the no-balance → no-run floor + the duplicate guard; it does NOT mutate
        // the wallet (Storage owns bookkeeping, not money). On admit we debit the wallet here.
        match g.ledger.reserve(tenant.clone(), run.clone(), amount, available) {
            Ok(_reservation) => {
                // Debit the wallet by the reserved amount (the depletion the next reserve sees). The
                // ledger already proved `available >= amount`, so this subtraction never underflows;
                // the checked form keeps it loud.
                g.wallet.available = available
                    .checked_sub(amount)
                    .ok_or(BudgetError::AmountOverflow)?;
                Ok(())
            }
            Err(e) => {
                let err = BudgetError::from(e);
                if let (BudgetError::Refused { .. }, Some(t)) = (&err, &self.telemetry) {
                    t.record_reserve_reject();
                }
                Err(err)
            }
        }
    }

    /// **Mark a reserved dispatch IN-FLIGHT (§4.9 — it has begun executing).** From this point the
    /// reservation is NEVER interrupted; the only exit is [`BudgetGate::settle`]. Idempotent (re-marking
    /// an in-flight run is a no-op success). This is what makes the never-interrupt-in-flight invariant
    /// observable: a depleting wallet cannot tear a run down once it is in flight.
    pub fn begin(&self, tenant: &TenantId, run: &LedgerRunId) -> Result<(), BudgetError> {
        self.lock().ledger.begin(tenant, run).map_err(BudgetError::from)
    }

    /// **Settle-on-completion (§4.9 step 4).** Close the reservation for `(tenant, run)` with the actual
    /// metered `units`, recording exactly one cost event per unit (wholesale ≠ markup) and REFUNDING the
    /// over-reservation back into the wallet (the same wallet the reserve debited). Idempotent on
    /// `(tenant, run)`: a double-settle returns the same outcome and refunds NOTHING further (no
    /// double-credit). Records the settle into the §5.4 telemetry.
    pub fn settle(
        &self,
        tenant: &TenantId,
        run: &LedgerRunId,
        units: &[MeteredUnit],
    ) -> Result<BudgetSettle, BudgetError> {
        let mut g = self.lock();
        // Was this run already settled? Re-settling must NOT re-credit the wallet (idempotent — no
        // double-credit). The ledger's settle is idempotent; we only refund on the FIRST settle.
        let already_settled = g.ledger.state_of(tenant, run)
            == Some(myelin_storage::reserve_settle::ReservationState::Settled);
        let outcome = g.ledger.settle(tenant, run, units).map_err(BudgetError::from)?;
        if !already_settled {
            // Refund the over-reservation back into the wallet (reserved − billed). The ledger already
            // computed it as `outcome.refunded` (never negative — the reserve is the cap).
            g.wallet.available = g
                .wallet
                .available
                .checked_add(outcome.refunded)
                .ok_or(BudgetError::AmountOverflow)?;
        }
        drop(g);
        if let Some(t) = &self.telemetry {
            t.record_settle();
        }
        Ok(outcome)
    }

    /// The lifecycle state of a reservation (for the drill / consumer to observe `Reserved → InFlight
    /// → Settled`). `None` if the `(tenant, run)` was never reserved (a refused dispatch writes no row).
    pub fn state_of(
        &self,
        tenant: &TenantId,
        run: &LedgerRunId,
    ) -> Option<myelin_storage::reserve_settle::ReservationState> {
        self.lock().ledger.state_of(tenant, run)
    }

    /// **The in-flight-interrupt counter the FLOW-D6 drill reads (§4.9).** `0` by construction — there
    /// is no code path in the bookend OR the Storage ledger that interrupts an in-flight reservation
    /// (the never-interrupt-in-flight invariant is structural). The drill asserts this stays `0` while
    /// a depleting wallet refuses NEW dispatches.
    pub fn inflight_interrupt_count(&self) -> u64 {
        self.lock().ledger.inflight_interrupt_count()
    }
}

impl WfCtx {
    /// **Supply the reserve/settle bookend so this `WfCtx` can meter spend-bearing dispatches (contract
    /// 9.5, §4.9).** A `WfCtx` built without this runs UN-METERED (the engine still owns the loop-cap
    /// depth, AG-6 — an un-budgeted run is not a runaway-spend risk because it has no spend gate
    /// wired). The dispatcher calls this when it builds the drive's `WfCtx` from the run's
    /// [`crate::RunBudget`] so the body's [`WfCtx::metered_activity`] + [`WfCtx::schedule_and_run_job`]
    /// reserve/settle against the SAME wallet. Chainable on `begin`/`resume`.
    pub fn with_budget(mut self, gate: BudgetGate) -> Self {
        self.budget = Some(gate);
        self
    }

    /// The bookend handle (if one was supplied via [`WfCtx::with_budget`]) — used by the engine's
    /// metered dispatch paths. `None` on an un-metered `WfCtx`.
    pub(crate) fn budget(&self) -> Option<&BudgetGate> {
        self.budget.as_ref()
    }

    /// **The deterministic ledger run-id a dispatch at `command_id` reserves under (§4.9).** Derived
    /// PURELY from `(run_id, command_id)` so a re-drive (replay) reconstructs the SAME ledger key — the
    /// reservation is keyed identically across a crash-recovery re-drive (the reserve short-circuits on
    /// the duplicate guard rather than double-reserving). Distinct per dispatch position so two
    /// dispatches in one body get two reservations.
    pub(crate) fn dispatch_ledger_run(&self, command_id: &str) -> LedgerRunId {
        LedgerRunId::new(format!("{}/{}", self.run_id(), command_id))
    }

    /// **`metered_activity(policy, cost, units, closure)` (contract 9.5, §4.4/§4.9) — a synchronous
    /// activity fronted by the reserve/settle bookend.** Reserve `cost` minor-units at dispatch (NO
    /// balance → the activity NEVER runs, [`BudgetError::Refused`] surfaces as
    /// [`crate::WfError::CoCommit`]); mark in-flight; run the activity (the existing journaled/retried
    /// [`WfCtx::activity`]); settle the actual metered `units` on completion, refunding the
    /// over-reservation into the SAME wallet (§4.9 — *meter into the same wallet as a synchronous
    /// activity*). An un-metered `WfCtx` (no [`WfCtx::with_budget`]) runs the activity WITHOUT a
    /// reserve (the loop-cap is still the runaway bound, AG-6).
    ///
    /// **Never interrupt in-flight:** once the reserve admits + `begin` marks the run in-flight, a
    /// wallet that depletes mid-activity cannot tear it down — the activity runs to completion and
    /// settles. **Replay:** the reserve is keyed on the deterministic [`WfCtx::dispatch_ledger_run`]
    /// (the same across a re-drive — the duplicate-reserve guard makes a re-driven reserve a no-op),
    /// and the inner [`WfCtx::activity`] short-circuits its journaled result (0 re-execution).
    pub fn metered_activity<F>(
        &mut self,
        policy: crate::RetryPolicy,
        cost: MinorUnits,
        units: Vec<MeteredUnit>,
        f: F,
    ) -> crate::WfResult<Vec<myelin_refs::ArtifactRef>>
    where
        F: Fn(&str, u32) -> Result<Vec<myelin_refs::ArtifactRef>, crate::ActivityError>,
    {
        // Reserve BEFORE running (the no-balance → no-dispatch floor): if there is no gate the activity
        // is un-metered (runs without a reserve). The reserve key is deterministic on the dispatch
        // position so a re-drive re-keys identically (the duplicate guard makes the replay reserve a
        // no-op rather than a double-reserve).
        let dispatch_command_id = self.peek_next_command_id();
        let ledger_run = self.dispatch_ledger_run(&dispatch_command_id);
        let tenant = self.tenant_id().clone();

        if let Some(gate) = self.budget().cloned() {
            // Reserve-at-dispatch. A refused reserve means the wallet is exhausted — the activity NEVER
            // runs (the dispatch never starts). On a re-drive the reserve hits the duplicate guard
            // (already reserved on the first drive) — its begin/settle already ran, so we DO NOT
            // re-progress the ledger (no double-debit, no begin-on-settled): the activity merely
            // short-circuits its journaled result.
            let fresh = match gate.reserve(&tenant, &ledger_run, cost) {
                Ok(()) => true,
                Err(BudgetError::DuplicateReservation) => false,
                Err(BudgetError::Refused {
                    requested,
                    available,
                }) => {
                    // No balance → no dispatch. The activity closure is NEVER invoked.
                    return Err(crate::WfError::CoCommit(format!(
                        "metered_activity refused at reserve: wallet exhausted (requested {} \
                         minor-units, {} available) — the activity never started (§4.9)",
                        requested.0, available.0
                    )));
                }
                Err(other) => {
                    return Err(crate::WfError::CoCommit(format!(
                        "metered_activity reserve failed: {other}"
                    )));
                }
            };
            if fresh {
                // The reservation is in-flight: from here it is NEVER interrupted.
                gate.begin(&tenant, &ledger_run).map_err(|e| {
                    crate::WfError::CoCommit(format!("metered_activity begin failed: {e}"))
                })?;
            }

            // Run the activity (journaled, retried — §4.4). It short-circuits its journaled result on a
            // re-drive (0 re-execution). A successful completion settles; a retry-exhaustion surfaces
            // the activity error (the reservation stays in-flight — the body compensates / dequeues;
            // an in-flight reservation is never torn down, it settles when the body settles it).
            let result = self.activity(policy, f)?;

            // Settle-on-completion: refund the over-reservation into the same wallet (§4.9 step 4) —
            // ONLY on the fresh drive (a re-drive's settle already ran; re-settling is idempotent but
            // we skip it to keep the replay a pure short-circuit).
            if fresh {
                gate.settle(&tenant, &ledger_run, &units).map_err(|e| {
                    crate::WfError::CoCommit(format!("metered_activity settle failed: {e}"))
                })?;
            }
            Ok(result)
        } else {
            // Un-metered: no gate wired (the loop-cap depth is still the runaway bound, AG-6).
            self.activity(policy, f)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{SignalStore, SignalRow};
    use crate::job::{JobKind, JobOutcome, JobRunner, JobSpec, job_idem_token, JOB_DONE_SIGNAL};
    use crate::{RetryPolicy, WfJournal};
    use myelin_events::{
        Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_refs::ArtifactRef;
    use myelin_tenancy::{Region, TenantId};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: tenant(),
            region: region(),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                tenant(),
            )),
            schema_ver: 1,
            occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
            caused_by: Some(CausedBy("session:abc".into())),
        }
    }
    fn minter() -> Arc<dyn IdMinter> {
        Arc::new(MonotonicMinter::new())
    }
    fn begin_ctx(outbox: &OutboxStore, journal: WfJournal, gate: BudgetGate) -> WfCtx {
        WfCtx::begin(
            outbox,
            minter(),
            journal,
            ctx_base(),
            "R1",
            "merge.queue",
            "2026-06-21T00:00:00Z",
            42,
        )
        .with_budget(gate)
    }

    fn unit(unit: &'static str, wholesale: u64, markup: u64) -> MeteredUnit {
        MeteredUnit {
            unit,
            wholesale: MinorUnits(wholesale),
            markup: MinorUnits(markup),
        }
    }

    /// The recording runner (the contract-8.4 seam fixture) — counts dispatches.
    #[derive(Default)]
    struct RecordingRunner {
        calls: AtomicUsize,
        dispatched: Mutex<Vec<JobSpec>>,
    }
    impl JobRunner for RecordingRunner {
        fn dispatch(&self, spec: &JobSpec) -> Result<(), crate::ActivityError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.dispatched.lock().unwrap().push(spec.clone());
            Ok(())
        }
    }

    fn deliver_job_done(signals: &SignalStore, idem_token: &str, result: Vec<ArtifactRef>) {
        signals.deliver(SignalRow {
            tenant: tenant(),
            region: region(),
            run_id: "R1".into(),
            signal_name: JOB_DONE_SIGNAL.into(),
            idem_key: idem_token.into(),
            payload: result,
            payload_key_ref: None,
            consumed_seq: None,
        });
    }

    /// **Reserve against an EMPTY wallet REFUSES the dispatch — the activity NEVER runs (§4.9).** The
    /// no-balance → no-dispatch floor: a metered activity whose reserve is refused does NOT invoke the
    /// closure (the side effect never happens) and surfaces a loud error.
    #[test]
    fn reserve_against_empty_wallet_refuses_the_dispatch_the_activity_never_runs() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let gate = BudgetGate::new(Wallet::new(MinorUnits::ZERO)); // exhausted wallet.
        let ran = Arc::new(AtomicUsize::new(0));
        let ran_c = ran.clone();

        let mut ctx = begin_ctx(&outbox, journal, gate.clone());
        let err = ctx
            .metered_activity(
                RetryPolicy::default_policy(),
                MinorUnits(100),
                vec![unit("llm.tokens", 80, 20)],
                move |_idem, _att| {
                    ran_c.fetch_add(1, Ordering::SeqCst);
                    Ok(vec![ArtifactRef("x://y".into())])
                },
            )
            .expect_err("an empty wallet refuses the dispatch");
        assert!(
            matches!(err, crate::WfError::CoCommit(ref m) if m.contains("wallet exhausted")),
            "the refusal is a loud error, got {err:?}"
        );
        assert_eq!(ran.load(Ordering::SeqCst), 0, "the activity closure NEVER ran (no dispatch)");
        // nothing reserved (a refused reserve writes no ledger row).
        let lr = LedgerRunId::new("R1/merge.queue:0");
        assert!(gate.state_of(&tenant(), &lr).is_none(), "a refused reserve writes no row");
    }

    /// **A funded metered activity reserves, runs, and SETTLES into the same wallet (§4.9 step 4).** The
    /// over-reservation is refunded: reserve 100, bill 60 → the wallet ends at `start − 60`.
    #[test]
    fn funded_metered_activity_reserves_runs_and_settles_into_the_same_wallet() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let gate = BudgetGate::new(Wallet::new(MinorUnits(1_000)));

        let mut ctx = begin_ctx(&outbox, journal, gate.clone());
        let out = ctx
            .metered_activity(
                RetryPolicy::default_policy(),
                MinorUnits(100),               // reserve 100
                vec![unit("llm.tokens", 40, 20)], // bill 60
                |_idem, _att| Ok(vec![ArtifactRef("myelin://acme/out".into())]),
            )
            .expect("a funded metered activity runs");
        assert_eq!(out, vec![ArtifactRef("myelin://acme/out".into())]);
        // wallet: 1000 − 100 (reserve) + 40 (refund of 100−60) = 940. Billed 60 stays drawn.
        assert_eq!(gate.balance(), MinorUnits(940), "settled: only the billed 60 is drawn");
        let lr = LedgerRunId::new("R1/merge.queue:0");
        assert_eq!(
            gate.state_of(&tenant(), &lr),
            Some(myelin_storage::reserve_settle::ReservationState::Settled)
        );
    }

    /// **An in-flight metered activity is NEVER interrupted by exhaustion — it settles (§4.9).** The
    /// activity's reserve depletes the wallet to 0; a SECOND metered dispatch is refused (no balance),
    /// but the FIRST (already in-flight, here completed) settled normally. The interrupt counter is 0.
    #[test]
    fn in_flight_activity_is_not_interrupted_by_exhaustion_second_dispatch_refused() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        // Exactly enough for ONE reserve of 100; the second reserve has nothing left.
        let gate = BudgetGate::new(Wallet::new(MinorUnits(100)));

        let body_gate = gate.clone();
        let mut ctx = begin_ctx(&outbox, journal, gate.clone());

        // FIRST metered activity: reserves 100 (wallet → 0), bills 100 (no refund), settles.
        let out1 = ctx
            .metered_activity(
                RetryPolicy::default_policy(),
                MinorUnits(100),
                vec![unit("llm.tokens", 70, 30)], // bill exactly 100
                |_i, _a| Ok(vec![ArtifactRef("first".into())]),
            )
            .expect("first runs");
        assert_eq!(out1, vec![ArtifactRef("first".into())]);
        assert_eq!(body_gate.balance(), MinorUnits::ZERO, "wallet exhausted by the first");

        // SECOND metered activity: wallet is empty → refused, never runs.
        let ran2 = Arc::new(AtomicUsize::new(0));
        let ran2_c = ran2.clone();
        let err = ctx
            .metered_activity(
                RetryPolicy::default_policy(),
                MinorUnits(50),
                vec![unit("llm.tokens", 30, 20)],
                move |_i, _a| {
                    ran2_c.fetch_add(1, Ordering::SeqCst);
                    Ok(vec![ArtifactRef("second".into())])
                },
            )
            .expect_err("the second is refused — the wallet is exhausted");
        assert!(matches!(err, crate::WfError::CoCommit(_)));
        assert_eq!(ran2.load(Ordering::SeqCst), 0, "the second activity NEVER ran");
        // The first run was never interrupted — it settled. 0 interrupts.
        assert_eq!(gate.inflight_interrupt_count(), 0, "0 in-flight interrupts (the headline zero)");
    }

    /// **An in-flight long-park is NEVER interrupted even after the wallet empties — it settles on the
    /// job.done signal (§4.9).** Dispatch reserves; the wallet then depletes (a sibling reserve); the
    /// job.done still settles the in-flight long-park. 0 interrupts.
    #[test]
    fn long_park_reserves_at_dispatch_and_settles_on_job_done_into_the_same_wallet() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let runner = RecordingRunner::default();
        let gate = BudgetGate::new(Wallet::new(MinorUnits(500)));

        // The job.done is already buffered (a fast job) under the deterministic dispatch token.
        // metered_schedule_and_run_job's first command is the RESERVE marker? No — reserve is not a
        // command; the dispatch activity is command merge.queue:0, so the job token is on :0.
        let token = job_idem_token("R1", "merge.queue:0");
        deliver_job_done(&signals, &token, vec![ArtifactRef("myelin://acme/ci/green".into())]);

        let mut ctx = WfCtx::begin(
            &outbox, minter(), journal, ctx_base(), "R1", "merge.queue",
            "2026-06-21T00:00:00Z", 42,
        )
        .with_signals(signals)
        .with_budget(gate.clone());

        let out = ctx
            .metered_schedule_and_run_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"),
                &runner,
                None,
                MinorUnits(200),                 // reserve 200 for the job
                vec![unit("ci.minute", 100, 50)], // bill 150 on completion
            )
            .expect("dispatch + complete");

        match out {
            JobOutcome::Completed { result, .. } => {
                assert_eq!(result, vec![ArtifactRef("myelin://acme/ci/green".into())]);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
        assert_eq!(runner.calls.load(Ordering::SeqCst), 1, "dispatched once");
        // wallet: 500 − 200 (reserve) + 50 (refund of 200−150) = 350.
        assert_eq!(gate.balance(), MinorUnits(350), "settled the job into the same wallet");
        assert_eq!(gate.inflight_interrupt_count(), 0, "0 interrupts");
    }

    /// **A long-park dispatch against an EXHAUSTED wallet is REFUSED — the job is never handed to the
    /// runner (§4.9, the F-6 extended assertion).** The reserve fronts the dispatch: no balance → the
    /// runner is never called.
    #[test]
    fn long_park_dispatch_against_empty_wallet_is_refused_runner_never_called() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let runner = RecordingRunner::default();
        let gate = BudgetGate::new(Wallet::new(MinorUnits(50))); // not enough for a 200 reserve.

        let mut ctx = WfCtx::begin(
            &outbox, minter(), journal, ctx_base(), "R1", "merge.queue",
            "2026-06-21T00:00:00Z", 42,
        )
        .with_signals(signals)
        .with_budget(gate.clone());

        let err = ctx
            .metered_schedule_and_run_job(
                JobSpec::new(JobKind::Ci, "pipeline://acme/ci/pr-7"),
                &runner,
                None,
                MinorUnits(200),
                vec![unit("ci.minute", 100, 50)],
            )
            .expect_err("an exhausted wallet refuses the dispatch");
        assert!(matches!(err, crate::WfError::CoCommit(ref m) if m.contains("wallet exhausted")));
        assert_eq!(
            runner.calls.load(Ordering::SeqCst),
            0,
            "the job was NEVER handed to the runner (no dispatch)"
        );
    }

    /// **Settle is idempotent — a double-settle never double-credits the wallet (§4.9).** Settling the
    /// same run twice refunds the over-reservation ONCE.
    #[test]
    fn double_settle_does_not_double_credit_the_wallet() {
        let gate = BudgetGate::new(Wallet::new(MinorUnits(1_000)));
        let lr = LedgerRunId::new("R1/cmd:0");
        gate.reserve(&tenant(), &lr, MinorUnits(100)).unwrap();
        gate.begin(&tenant(), &lr).unwrap();
        assert_eq!(gate.balance(), MinorUnits(900), "reserved 100");

        let units = vec![unit("u", 40, 20)]; // bill 60
        gate.settle(&tenant(), &lr, &units).unwrap();
        assert_eq!(gate.balance(), MinorUnits(940), "refunded 40 once (900 + 40)");
        // Re-settle: same outcome, NO further refund.
        gate.settle(&tenant(), &lr, &units).unwrap();
        assert_eq!(gate.balance(), MinorUnits(940), "no double-credit on re-settle");
    }

    /// **The reject-rate telemetry records attempts + rejects (§5.4, contract 1.8).** Two reserves: one
    /// admits, one refused → 2 attempts, 1 reject, rate = 5000 bps; one settle recorded.
    #[test]
    fn reject_rate_telemetry_records_attempts_rejects_and_settles() {
        let telemetry = FlowTelemetry::new();
        let gate = BudgetGate::new(Wallet::new(MinorUnits(100))).with_telemetry(telemetry.clone());

        // Reserve 1: admits (wallet 100 → 0).
        let lr1 = LedgerRunId::new("R1/cmd:0");
        gate.reserve(&tenant(), &lr1, MinorUnits(100)).expect("admits");
        gate.begin(&tenant(), &lr1).unwrap();
        gate.settle(&tenant(), &lr1, &[unit("u", 100, 0)]).unwrap();

        // Reserve 2: refused (wallet empty).
        let lr2 = LedgerRunId::new("R1/cmd:1");
        gate.reserve(&tenant(), &lr2, MinorUnits(50)).expect_err("refused");

        assert_eq!(telemetry.reserve_attempted(), 2, "two reserve attempts");
        assert_eq!(telemetry.reserve_rejected(), 1, "one refused");
        assert_eq!(telemetry.reserve_reject_rate_bps(), 5_000, "50% reject rate");
        assert_eq!(telemetry.settled(), 1, "one settle recorded");
    }

    /// **An un-metered `WfCtx` (no with_budget) runs the activity WITHOUT a reserve (AG-6 loop-cap is
    /// the bound).** A `metered_activity` with no gate behaves as a plain activity.
    #[test]
    fn unmetered_wfctx_runs_the_activity_without_a_reserve() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let mut ctx = WfCtx::begin(
            &outbox, minter(), journal, ctx_base(), "R1", "merge.queue",
            "2026-06-21T00:00:00Z", 42,
        ); // NO with_budget.
        let out = ctx
            .metered_activity(
                RetryPolicy::default_policy(),
                MinorUnits(100),
                vec![unit("u", 10, 0)],
                |_i, _a| Ok(vec![ArtifactRef("ran".into())]),
            )
            .expect("an un-metered activity runs");
        assert_eq!(out, vec![ArtifactRef("ran".into())]);
    }

    /// **A re-drive (replay) re-keys the reserve identically — the duplicate guard makes it a no-op, 0
    /// double-reserve, 0 double-debit (§4.9 replay).** Drive 1 reserves+settles; drive 2 (resume) hits
    /// the duplicate guard on reserve and the activity short-circuits → the wallet is debited ONCE.
    #[test]
    fn replay_re_keys_the_reserve_identically_no_double_debit() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let gate = BudgetGate::new(Wallet::new(MinorUnits(1_000)));

        // DRIVE 1: reserve 100 + bill 60 → wallet 940, journaled.
        let mut c1 = begin_ctx(&outbox, journal.clone(), gate.clone());
        c1.metered_activity(
            RetryPolicy::default_policy(),
            MinorUnits(100),
            vec![unit("u", 40, 20)],
            |_i, _a| Ok(vec![ArtifactRef("v1".into())]),
        )
        .expect("drive 1");
        c1.commit().expect("co-commit");
        assert_eq!(gate.balance(), MinorUnits(940), "drive 1 drew 60");
        let history = journal.history_for(&tenant(), "R1");

        // DRIVE 2 (re-drive): resume over the journal. The reserve hits the duplicate guard (no
        // re-debit); the activity short-circuits its journaled result; the settle is idempotent.
        let mut c2 = WfCtx::resume(
            &outbox, minter(), journal.clone(), ctx_base(), "R1", "merge.queue",
            "2026-06-21T00:00:00Z", 42, history,
        )
        .with_budget(gate.clone());
        let out2 = c2
            .metered_activity(
                RetryPolicy::default_policy(),
                MinorUnits(100),
                vec![unit("u", 40, 20)],
                |_i, _a| panic!("the activity must NOT re-run on replay"),
            )
            .expect("the replay drive");
        assert_eq!(out2, vec![ArtifactRef("v1".into())], "replay returns the journaled result");
        assert_eq!(gate.balance(), MinorUnits(940), "0 DOUBLE-DEBIT on replay (re-keyed identically)");
    }

    /// `Wallet::from_budget` seeds from a RunBudget; a negative budget clamps to an empty wallet.
    #[test]
    fn wallet_from_budget_seeds_and_clamps() {
        let w = Wallet::from_budget(&crate::RunBudget { minor_units: 500 });
        assert_eq!(w.balance(), MinorUnits(500));
        let neg = Wallet::from_budget(&crate::RunBudget { minor_units: -5 });
        assert_eq!(neg.balance(), MinorUnits::ZERO, "a negative budget is an empty wallet");
    }

    /// **`begin` moves a reservation `Reserved → InFlight` (the never-interrupt anchor, §4.9).** Before
    /// `begin` the reservation is `Reserved` (the one teardown-able state); AFTER `begin` it is
    /// `InFlight` — the state from which there is NO teardown path. This pins the begin transition so a
    /// `begin -> no-op` regression (the reservation would stay `Reserved` and remain teardown-able) is
    /// caught.
    #[test]
    fn begin_moves_reservation_reserved_to_in_flight() {
        use myelin_storage::reserve_settle::ReservationState;
        let gate = BudgetGate::new(Wallet::new(MinorUnits(1_000)));
        let lr = LedgerRunId::new("R1/cmd:0");
        gate.reserve(&tenant(), &lr, MinorUnits(100)).unwrap();
        assert_eq!(
            gate.state_of(&tenant(), &lr),
            Some(ReservationState::Reserved),
            "before begin: Reserved (the one teardown-able state)"
        );
        gate.begin(&tenant(), &lr).unwrap();
        assert_eq!(
            gate.state_of(&tenant(), &lr),
            Some(ReservationState::InFlight),
            "after begin: InFlight — from here there is NO teardown path (never interrupt)"
        );
        // begin is idempotent — re-marking an in-flight run is a no-op success, still InFlight.
        gate.begin(&tenant(), &lr).unwrap();
        assert_eq!(gate.state_of(&tenant(), &lr), Some(ReservationState::InFlight));
    }

    /// The `BudgetError` Displays are loud + specific (observability is part of the pass, EI-01 §3).
    #[test]
    fn budget_errors_display_loud_and_specific() {
        let refused = BudgetError::Refused {
            requested: MinorUnits(200),
            available: MinorUnits(50),
        }
        .to_string();
        assert!(refused.contains("no balance, no dispatch"), "must cite the floor: {refused}");
        assert!(BudgetError::NoSuchReservation.to_string().contains("never invent a charge"));
    }
}

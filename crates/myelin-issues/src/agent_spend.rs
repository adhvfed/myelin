//! # `agent_spend` — reserve/settle on every spend-bearing Issues agent run (ISS-P24 / P-391, M4-I6)
//!
//! **Owning architecture doc:**
//! `planning/04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md`
//! §9 (*Reserve/settle — spend-bearing agent work, contract 11.7*: where Issues runs spend-bearing
//! work — the **triage agent**, the **forecast agent**, the **SLA-draft agent**, any agent invoked via
//! an automation/trigger — the run is a **durable workflow** (contract 9.5) with the reserve/settle
//! gate as its **bookends**: `reserve` at dispatch (**no balance → no start**), `settle` on completion
//! (**never interrupt in-flight**). Metering is integer minor-units; CI runs and agent runs meter into
//! the **same wallet** (Commercial C-1). The HITL approval card surfaces a **live cost estimate**
//! before a human approves a gated effect. **Issues does NOT own the wallet — it consumes the gate.**),
//! 05-hard-problems.md (the reserve/settle posture).
//!
//! **Contract-index:** CONSUMES **11.7** (reserve/settle cost gate — reserve at dispatch, no balance →
//! no start; settle on completion, never interrupt in-flight; integer minor-units; wholesale ≠ markup;
//! **the same wallet as CI runs**) and **9.5** (the workflow↔agent mapping — *reserve/settle = the
//! bookends*; the workflow owns the `RunBudget`/gates/state, `step`/`exec` are activities, and the
//! reserve/settle pair brackets the run). Implemented to the FROZEN shapes — this module does NOT
//! re-implement the ledger; it DRIVES the Storage-owned gate through the Issues run lifecycle.
//!
//! ## What ISS-P24 ships — the Issues-side consumer of the SHARED reserve/settle gate
//!
//! The reserve/settle MECHANISM is the Storage-owned [`AgentRunGate`] over the durable
//! [`CostLedger`](myelin_storage::reserve_settle::CostLedger) (P-ST-16/P-ST-19 → P-103/P-146): a
//! funded dispatch mints a move-only [`InFlightRun`] handle; an unfunded dispatch mints NONE (no
//! balance → no run); an in-flight run is NEVER interrupted (no gate API tears it down); a settle
//! records exactly one cost event per metered unit and refunds the over-reservation. The agent-fabric
//! `EffectApi` BUDGET/METER steps (AG-P6 → P-218) front each *effect*; THIS module fronts each *run* —
//! the **9.5 bookends** that bracket a whole spend-bearing Issues agent run (the durable workflow).
//!
//! [`spend_bearing_run`] is the ONE entry every Issues spend-bearing agent run goes through:
//!
//! 1. **Reserve-at-dispatch (the OPENING bookend, 9.5).** Reserve the run's estimated upper-bound cost
//!    against the wallet balance BEFORE the run starts. **No balance → no start** ([`SpendError::NoBalance`]
//!    — the run is never dispatched, no [`InFlightRun`] handle is minted, the runaway self-limiter
//!    AG-D11). The wallet balance is the SAME Commercial wallet CI runs draw from (11.7) — Issues does
//!    not own it; it passes the balance the control-plane wallet reports.
//! 2. **Run behind the move-only in-flight handle.** The run executes; it is `InFlight` in the ledger.
//!    The only exit is a settle through the handle — there is **no API that interrupts an in-flight
//!    run** (the never-interrupt-in-flight invariant is structural, AG-D11).
//! 3. **Settle-on-completion (the CLOSING bookend, 9.5).** On completion the run settles with its
//!    actual metered units (one cost event per unit, wholesale ≠ markup), releasing the
//!    over-reservation. **A settle never interrupts an in-flight run** — it closes a COMPLETED one.
//!
//! The headline green artifact is the **balanced wallet** ([`BalancedRunSignal`]): for a completed run
//! the reserve EQUALS the settle (`reserved == billed + refunded`) and zero in-flight runs were
//! interrupted. An unbalanced wallet is a cost-correctness failure (mandatory-core).
//!
//! ## The kind of spend-bearing Issues run (the §9 enumeration)
//! [`IssueRunKind`] enumerates the Issues spend-bearing agent runs §9 names: the **triage** agent, the
//! **forecast** agent, the **SLA-draft** agent, and a generic **automation** run (a trigger/automation-
//! invoked agent). The cost gate behaviour is IDENTICAL across kinds — every spend-bearing run reserves
//! at dispatch and settles on completion (the whole point: ONE gate fronts every run). The kind is an
//! observability label the run record carries (it also picks the metered-unit dimension).
//!
//! ## The per-effect `idem_key` rule (OQ-F — stated here, owned by the HITL card machinery)
//! When a spend-bearing Issues run gates an effect for HITL approval, the durable approval signal is
//! keyed **per-effect** (reconciliation OQ-F; the `card_id:<effect_idx>` rule):
//!
//! - a **single-effect** card → `idem_key = card_id` (the degenerate case — a double-click is one
//!   approval, one apply);
//! - a **multi/partial-approval** card → `idem_key = card_id ":" effect_idx` (each effect approved /
//!   declined independently and idempotently; a partial approval is well-defined; a declined effect is
//!   withheld → 0 mutation, AG-8).
//!
//! Issues does NOT re-implement this key — the rule + its construction live in the agent-fabric HITL
//! card machinery (`myelin-agent-service::hitl_batch::per_effect_idem_key`, consumed by
//! `myelin-flow::approval`, 9.1/9.4). [`per_effect_idem_key`] HERE is the Issues-side statement of the
//! SAME frozen rule (so the Issues run lifecycle names the key its HITL cards ride on) — the two sides
//! agree by construction (a CDC pins parity).
//!
//! ## FLOORS named (deferred + the filling prompt; VISION §3, EI-01 §1)
//! - **No NEW floor.** Reserve/settle IS the floor (§9): a spend-bearing run that did not reserve, or
//!   did not settle, is the failure this module forecloses. The wallet is shared with CI (11.7) —
//!   there is no Issues-private wallet to deprecate.
//! - **The wallet BALANCE is the Commercial control-plane wallet (C-1).** This module CONSUMES the
//!   balance (the caller passes the control-plane wallet's available balance) and the Storage gate;
//!   it does NOT own the balance source. The real durable Postgres-backed ledger lands with the OLTP
//!   driver (P-S12) behind the SAME backend-agnostic `CostLedger` — no NEW db/object-store/cache/bus
//!   trait is touched here, so **no new integration drill is owed** (the gate's durability drill is
//!   Storage's, P-146).
//! - **The agent brain is the MOCK runtime** ([`crate`]-external `MockAgentRuntime`); the real-LLM
//!   runtime is post-M5 (ISS-P32 / AG-P25, R-10). The cost gate is brain-agnostic — it fronts BOTH,
//!   which is exactly the point (the runaway self-limiter holds regardless of which brain runs).
//!
//! ## Mutation floor (mandatory-core, EI-01 §2 — the reserve == settle path is ≥ 80%)
//! The reserve-at-dispatch / never-interrupt-in-flight / settle-on-completion / balanced-wallet path is
//! mandatory-core: an unbalanced wallet is a cost-correctness failure. The unit + e2e tests below
//! exercise: a funded run reserves → runs → settles balanced (reserve == settle); an unfunded run never
//! starts (no balance → no start, no handle); an in-flight run is never interrupted; the wallet nets to
//! 0 over a completed run (reserve == billed + refunded). The mutation score is reported in the P-391
//! report (`cargo mutants --file crates/myelin-issues/src/agent_spend.rs`).

use myelin_storage::agent_run_gate::{AgentRunGate, DispatchError, InFlightRun, RunKind};
use myelin_storage::reserve_settle::{
    CostLedger, MeteredUnit, MicroUsd, RunId, SettleError, SettleOutcome,
};
use myelin_tenancy::TenantId;

// ───────────────────────── the spend-bearing Issues run kinds (arch §9) ──────────────────────────

/// **The kind of spend-bearing Issues agent run the gate fronts (arch §9).** §9 enumerates exactly the
/// Issues runs that spend: the triage agent, the forecast agent, the SLA-draft agent, and a generic
/// automation/trigger-invoked agent run. The reserve/settle behaviour is IDENTICAL across kinds (one
/// gate fronts every run); the kind picks the run's metered-unit dimension + is an observability label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueRunKind {
    /// The triage agent (the S9 suggestion strip — proposes effects via `run --dry-run`, ISS-P23).
    Triage,
    /// The forecast agent (compute-only — reads OLAP, emits the linear forecast, ISS-P23).
    Forecast,
    /// The SLA-draft agent (drafts an SLA policy — advisory, suggest-by-default, ISS-P23).
    SlaDraft,
    /// A generic automation/trigger-invoked agent run (an Issues automation that spends).
    Automation,
}

impl IssueRunKind {
    /// The frozen metered-unit dimension this run kind bills (a `&'static str` label — the cost
    /// dimension is frozen, never invented at runtime). Every Issues agent run bills the
    /// `agent.effect` dimension (the run's compute), the SAME dimension the agent-fabric effect meter
    /// uses; a future per-kind split adds a dimension here, never a silent re-label.
    pub fn metered_unit(self) -> &'static str {
        // All Issues spend-bearing runs bill the shared `agent.effect` compute dimension (the wallet
        // is shared with CI; the dimension distinguishes agent compute from CI minutes).
        match self {
            IssueRunKind::Triage
            | IssueRunKind::Forecast
            | IssueRunKind::SlaDraft
            | IssueRunKind::Automation => "agent.effect",
        }
    }

    /// The stable observability token for this run kind (the run record / trace label — a taxonomy
    /// token, not PII).
    pub fn as_str(self) -> &'static str {
        match self {
            IssueRunKind::Triage => "triage",
            IssueRunKind::Forecast => "forecast",
            IssueRunKind::SlaDraft => "sla_draft",
            IssueRunKind::Automation => "automation",
        }
    }
}

// ───────────────────────── the spend-bearing run errors ──────────────────────────────────────────

/// **An error fronting a spend-bearing Issues run.** A run is never started against an empty wallet
/// (the §9 *no balance → no start* floor); a settle of a non-dispatched run is a loud bug, never a
/// silent success.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpendError {
    /// **No balance → no start.** The wallet (the SAME Commercial wallet as CI) cannot afford the
    /// run's reservation — the run is NOT dispatched (no in-flight handle is minted, no work runs).
    /// The runaway self-limiter (AG-D11): a loop against an exhausted wallet stops at the wallet.
    NoBalance {
        /// The amount the dispatch asked to reserve (the run's estimated upper bound, minor-units).
        requested: MicroUsd,
        /// The wallet balance available (from the Commercial control-plane wallet).
        available: MicroUsd,
    },
    /// This run is already dispatched — a spend-bearing run is fronted exactly once (the idempotency
    /// guard; a re-dispatch of a live run is rejected loudly, never double-reserved).
    AlreadyDispatched,
    /// Integer minor-units arithmetic overflowed (loud, never a silent wrap — a cost is never wrapped).
    AmountOverflow,
    /// A settle failed against the ledger (e.g. the run was never reserved) — surfaced LOUD; a
    /// non-dispatched run is never silently "settled".
    Settle(SettleError),
}

impl From<DispatchError> for SpendError {
    fn from(e: DispatchError) -> SpendError {
        match e {
            DispatchError::NoBalance {
                requested,
                available,
            } => SpendError::NoBalance {
                requested,
                available,
            },
            DispatchError::AlreadyDispatched => SpendError::AlreadyDispatched,
            DispatchError::AmountOverflow => SpendError::AmountOverflow,
        }
    }
}

impl From<SettleError> for SpendError {
    fn from(e: SettleError) -> SpendError {
        SpendError::Settle(e)
    }
}

impl core::fmt::Display for SpendError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SpendError::NoBalance {
                requested,
                available,
            } => write!(
                f,
                "spend-bearing Issues run refused: no balance, no start (requested {} minor-units, \
                 {} available) — the run was NEVER started (arch §9 / 11.7, AG-D11)",
                requested.0, available.0
            ),
            SpendError::AlreadyDispatched => write!(
                f,
                "spend-bearing Issues run refused: this run is already in flight — a run is fronted \
                 exactly once (never double-reserved)"
            ),
            SpendError::AmountOverflow => write!(
                f,
                "spend-bearing Issues run refused: integer minor-units arithmetic overflowed (loud, \
                 never a silent wrap)"
            ),
            SpendError::Settle(e) => write!(f, "spend-bearing Issues run settle failed: {e}"),
        }
    }
}

impl std::error::Error for SpendError {}

// ───────────────────────── the dispatched run handle (the 9.5 bookends) ──────────────────────────

/// **A dispatched spend-bearing Issues run — the run between the two 9.5 bookends.** Minted ONLY by a
/// successful [`reserve_run`] (reserve-at-dispatch); the run is `InFlight` in the ledger the moment
/// this exists. The ONLY exit is [`settle`](DispatchedRun::settle) (settle-on-completion) — there is
/// **no method that interrupts an in-flight run** (the never-interrupt-in-flight invariant is
/// structural: the inner [`InFlightRun`] exposes no teardown, and neither does this).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DispatchedRun {
    /// The kind of Issues run (the observability label + the metered-unit dimension).
    kind: IssueRunKind,
    /// The Storage in-flight handle (the move-only settle key — the only way to close the run).
    handle: InFlightRun,
}

impl DispatchedRun {
    /// The kind of Issues run this is.
    pub fn kind(&self) -> IssueRunKind {
        self.kind
    }
    /// The tenant this run belongs to (the partition key — no cross-tenant path).
    pub fn tenant(&self) -> &TenantId {
        self.handle.tenant()
    }
    /// The run id.
    pub fn run(&self) -> &RunId {
        self.handle.run()
    }
    /// The amount reserved at dispatch (the billing cap — a settle never bills more than this).
    pub fn reserved(&self) -> MicroUsd {
        self.handle.reserved()
    }

    /// **Settle-on-completion (the CLOSING 9.5 bookend).** Close this run with its actual metered
    /// `units` against the supplied `ledger`, recording exactly one cost event per unit (wholesale ≠
    /// markup) and refunding the over-reservation. A settle NEVER interrupts an in-flight run — it
    /// closes a COMPLETED one. Idempotent: settling an already-settled run returns the same outcome
    /// and records no further cost events (a double-completion never double-charges). Returns the
    /// [`SettleOutcome`] (the bill the run reports).
    pub fn settle(
        &self,
        ledger: &mut CostLedger,
        units: &[MeteredUnit],
    ) -> Result<SettleOutcome, SettleError> {
        self.handle.settle(ledger, units)
    }
}

// ───────────────────────── the reserve/settle bookends (the OWNED ISS-P24 deliverable) ───────────

/// **The Issues reserve/settle gate fronting every spend-bearing agent run (the OWNED ISS-P24
/// deliverable; 11.7 + 9.5).** A thin Issues-side consumer of the Storage [`AgentRunGate`]: Issues
/// does NOT own the wallet or the ledger (§9 — *Issues does not own the wallet; it consumes the gate*);
/// it owns the run-lifecycle policy that brackets a spend-bearing Issues agent run with the 9.5
/// reserve/settle bookends. Construct one per cell; every Issues spend-bearing run goes through it, so
/// *fronting* is correct-by-construction, not a convention the caller must remember.
#[derive(Debug, Default)]
pub struct IssueSpendGate {
    /// The Storage gate the Issues runs reserve/settle through (the shared mechanism).
    gate: AgentRunGate,
}

impl IssueSpendGate {
    /// A fresh gate.
    pub fn new() -> IssueSpendGate {
        IssueSpendGate::default()
    }

    /// **Reserve-at-dispatch for a spend-bearing Issues run (the OPENING 9.5 bookend, 11.7).** Reserve
    /// `estimate` (the run's estimated upper-bound cost, integer minor-units) against the wallet
    /// `available` balance (the SAME Commercial wallet as CI), then start the run. On success a
    /// [`DispatchedRun`] handle is returned (the run is now `InFlight` and NEVER interrupted). On no
    /// balance the dispatch is REFUSED ([`SpendError::NoBalance`]) and **no handle is minted** — the
    /// run never starts (the §9 no-balance-no-start floor; the AG-D11 self-limiter).
    pub fn reserve_run(
        &mut self,
        ledger: &mut CostLedger,
        tenant: TenantId,
        run: RunId,
        kind: IssueRunKind,
        estimate: MicroUsd,
        available: MicroUsd,
    ) -> Result<DispatchedRun, SpendError> {
        // An Issues agent run is an `AgentRun` to the Storage gate (the brain loop — mock now,
        // LLM post-M5); the RunKind label is the gate's observability, the IssueRunKind is ours.
        let handle = self
            .gate
            .dispatch(ledger, tenant, run, estimate, available)
            .map_err(SpendError::from)?;
        debug_assert_eq!(handle.kind(), RunKind::AgentRun);
        Ok(DispatchedRun { kind, handle })
    }

    /// The number of spend-bearing Issues runs REFUSED for no balance (the AG-D11 `reserve refusals`
    /// telemetry — a loop against an exhausted wallet stops here).
    pub fn reserve_refusals(&self) -> u64 {
        self.gate.reserve_refusals()
    }

    /// The number of spend-bearing Issues runs ADMITTED (reserved + started).
    pub fn runs_dispatched(&self) -> u64 {
        self.gate.runs_dispatched()
    }
}

/// **Run a spend-bearing Issues agent run end-to-end through the reserve/settle bookends (the §9 / 9.5
/// run lifecycle — the convenience entry).** The ONE entry every Issues spend-bearing run goes
/// through: reserve at dispatch (no balance → no start) → run the supplied `work` closure (the agent
/// brain — mock now, LLM post-M5) → settle on completion with the metered `units` the work reports.
///
/// `work` is the brain loop; it returns the run's metered units (the actual cost). The reserve fronts
/// it (the run does not even START on an empty wallet), and the settle closes it (never interrupting an
/// in-flight run — the work always runs to completion once started). Returns the [`BalancedRunSignal`]
/// — the green artifact proving the wallet balanced (reserve == settle for the completed run).
///
/// **No balance → no start:** if the reserve is refused, `work` is **never called** (the run never
/// starts) and the [`SpendError::NoBalance`] is returned — the runaway self-limiter (AG-D11).
// The full run-lifecycle entry takes the ledger + wallet + the run identity + the brain closure; each
// arg is load-bearing (the SAME shape `AgentRunGate::dispatch` takes — there is no struct to fold them
// into without obscuring the reserve/settle bookends). The SAME allow `effect_api`/`hitl` use.
#[allow(clippy::too_many_arguments)]
pub fn spend_bearing_run<F>(
    gate: &mut IssueSpendGate,
    ledger: &mut CostLedger,
    tenant: TenantId,
    run: RunId,
    kind: IssueRunKind,
    estimate: MicroUsd,
    available: MicroUsd,
    work: F,
) -> Result<BalancedRunSignal, SpendError>
where
    F: FnOnce() -> Vec<MeteredUnit>,
{
    // OPENING bookend — reserve at dispatch. No balance → no run (the work is never called).
    let dispatched = gate.reserve_run(
        ledger,
        tenant.clone(),
        run.clone(),
        kind,
        estimate,
        available,
    )?;
    let reserved = dispatched.reserved();

    // The run executes (the brain loop — it is in-flight and will run to completion; NEVER
    // interrupted). The work reports its actual metered units.
    let units = work();

    // CLOSING bookend — settle on completion with the actual metered units (one cost event per unit,
    // wholesale ≠ markup). A settle closes a COMPLETED run; it never interrupts an in-flight one.
    let outcome = dispatched.settle(ledger, &units)?;

    Ok(BalancedRunSignal {
        tenant,
        run,
        kind,
        reserved,
        billed: outcome.billed_total,
        refunded: outcome.refunded,
        cost_events: outcome.cost_events.len() as u64,
        metered_units: units.len() as u64,
        inflight_interrupt_count: ledger.inflight_interrupt_count(),
    })
}

// ───────────────────────── the balanced-wallet green artifact (the GATE signal) ──────────────────

/// **The balanced-wallet signal a completed spend-bearing Issues run emits (the GATE green artifact;
/// arch §9, 11.7).** The PII-free aggregate proving the run's wallet balanced: the reserve EQUALS the
/// settle (`reserved == billed + refunded` — the reserve is fully accounted, none leaked), exactly one
/// cost event per metered unit (wholesale ≠ markup), and **zero in-flight runs interrupted** (the
/// headline zero). An unbalanced wallet reads RED — a cost-correctness failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BalancedRunSignal {
    /// The tenant the run billed (opaque id, PII-free).
    pub tenant: TenantId,
    /// The run id.
    pub run: RunId,
    /// The kind of spend-bearing Issues run.
    pub kind: IssueRunKind,
    /// The amount reserved at dispatch (the billing cap).
    pub reserved: MicroUsd,
    /// The amount actually billed on settle (`Σ wholesale + markup`, capped at `reserved`).
    pub billed: MicroUsd,
    /// The amount refunded to the wallet (`reserved − billed`, the released over-reservation).
    pub refunded: MicroUsd,
    /// The cost events recorded — the green artifact has `cost_events == metered_units`.
    pub cost_events: u64,
    /// The metered units the run reported.
    pub metered_units: u64,
    /// **The headline zero** — in-flight runs interrupted. `0` is GREEN; `> 0` reads RED.
    pub inflight_interrupt_count: u64,
}

impl BalancedRunSignal {
    /// **Is this a GREEN artifact? The wallet BALANCED.** For a completed run the reserve is fully
    /// accounted — `reserved == billed + refunded` (none leaked) — AND exactly one cost event per
    /// metered unit was recorded AND zero in-flight runs were interrupted. This is the "the wallet
    /// nets to 0 over a run (reserve == settle for a completed run)" gate (arch §9).
    pub fn is_green(&self) -> bool {
        // reserve == settle: the reserved amount is fully split into billed + refunded (no leak).
        let accounted = self
            .billed
            .checked_add(self.refunded)
            .map(|a| a == self.reserved)
            .unwrap_or(false);
        accounted && self.cost_events == self.metered_units && self.inflight_interrupt_count == 0
    }
}

// ───────────────────────── the per-effect idem_key rule (OQ-F — stated Issues-side) ──────────────

/// **The per-effect resume `idem_key` rule for an Issues spend-bearing run's HITL cards (OQ-F —
/// stated Issues-side; the SAME frozen rule the agent-fabric owns).** When a spend-bearing Issues run
/// gates an effect for HITL approval, the durable approval signal is keyed PER-EFFECT (reconciliation
/// OQ-F, the `card_id:<effect_idx>` rule):
///
/// - a **single-effect** card (`total_effects == 1`) → `idem_key = card_id` (a double-click is one
///   approval, one apply);
/// - a **multi/partial-approval** card (`total_effects > 1`) → `idem_key = card_id ":" effect_idx`
///   (each effect approved/declined independently and idempotently; a partial approval is
///   well-defined; a declined effect is withheld → 0 mutation, AG-8).
///
/// Issues does NOT own this key — its construction lives in the agent-fabric HITL card machinery
/// (`myelin-agent-service::hitl_batch::per_effect_idem_key`, consumed by `myelin-flow::approval`,
/// 9.1/9.4). This is the Issues-side STATEMENT of the SAME rule (so the Issues run lifecycle names the
/// key its cards ride on) — byte-identical to the agent-fabric derivation (a CDC pins parity).
///
/// # Panics
/// Panics in debug if `effect_idx >= total_effects` (a caller bug — the index must address an effect
/// in the card). The same precondition the agent-fabric derivation enforces.
pub fn per_effect_idem_key(card_id: &str, effect_idx: usize, total_effects: usize) -> String {
    debug_assert!(total_effects >= 1, "a card has at least one effect");
    debug_assert!(
        effect_idx < total_effects,
        "effect_idx ({effect_idx}) must index into the card's {total_effects} effect(s)"
    );
    if total_effects == 1 {
        // Single-effect card: the bare card_id (the degenerate per-effect case — `:0` is implicit).
        card_id.to_string()
    } else {
        // Multi/partial-approval card: each effect keys on `card_id ":" effect_idx`.
        format!("{card_id}:{effect_idx}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_storage::reserve_settle::ReservationState;

    fn tenant() -> TenantId {
        TenantId::from_token("01J0ACME")
    }

    fn run(n: u32) -> RunId {
        RunId::new(format!("01J0ISSUE_RUN_{n}"))
    }

    fn units(wholesale: u64, markup: u64) -> Vec<MeteredUnit> {
        vec![MeteredUnit {
            unit: "agent.effect",
            wholesale: MicroUsd(wholesale),
            markup: MicroUsd(markup),
        }]
    }

    // ───────────────────── reserve == settle: the balanced-wallet green artifact ─────────────────

    /// **A funded spend-bearing run balances the wallet: reserve == settle (the headline gate).** The
    /// run reserves its estimate, runs, settles its actual cost; the reserve is fully accounted
    /// (`reserved == billed + refunded`), one cost event per unit, 0 interrupts → GREEN.
    #[test]
    fn funded_run_balances_the_wallet_reserve_equals_settle() {
        let mut gate = IssueSpendGate::new();
        let mut ledger = CostLedger::new();
        let signal = spend_bearing_run(
            &mut gate,
            &mut ledger,
            tenant(),
            run(1),
            IssueRunKind::Triage,
            MicroUsd(1_000),  // reserve an upper bound of 1000
            MicroUsd(5_000),  // the wallet (shared with CI) affords it
            || units(300, 100), // the run actually costs 400 (wholesale 300 + markup 100)
        )
        .expect("a funded run completes");

        // reserve == settle: 1000 reserved = 400 billed + 600 refunded (the reserve is fully accounted).
        assert_eq!(signal.reserved, MicroUsd(1_000));
        assert_eq!(
            signal.billed,
            MicroUsd(400),
            "billed wholesale 300 + markup 100"
        );
        assert_eq!(
            signal.refunded,
            MicroUsd(600),
            "the over-reservation refunds"
        );
        assert_eq!(signal.cost_events, 1, "one cost event per metered unit");
        assert_eq!(signal.metered_units, 1);
        assert_eq!(signal.inflight_interrupt_count, 0, "the headline zero");
        assert!(signal.is_green(), "the wallet BALANCED: {signal:?}");

        // the run is Settled in the ledger (it ran to completion).
        assert_eq!(
            ledger.state_of(&tenant(), &run(1)),
            Some(ReservationState::Settled),
            "a completed run settles"
        );
        assert_eq!(gate.runs_dispatched(), 1);
        assert_eq!(gate.reserve_refusals(), 0);
    }

    /// **No balance → no start: the work is NEVER called, no run starts, the refusal counts.** An
    /// over-budget run is refused at the reserve bookend; the brain closure never fires; no
    /// reservation is left behind; the refusal counter ticks (the AG-D11 self-limiter).
    #[test]
    fn no_balance_means_no_start_and_the_work_never_runs() {
        let mut gate = IssueSpendGate::new();
        let mut ledger = CostLedger::new();
        let mut work_ran = false;
        let err = spend_bearing_run(
            &mut gate,
            &mut ledger,
            tenant(),
            run(1),
            IssueRunKind::Forecast,
            MicroUsd(9_000),
            MicroUsd(100), // the wallet cannot afford the run
            || {
                work_ran = true; // this MUST NOT execute (no balance → no start)
                units(10, 0)
            },
        )
        .expect_err("an over-budget run is refused");

        assert_eq!(
            err,
            SpendError::NoBalance {
                requested: MicroUsd(9_000),
                available: MicroUsd(100),
            }
        );
        assert!(
            !work_ran,
            "no balance → no start: the agent brain NEVER ran"
        );
        assert!(
            ledger.state_of(&tenant(), &run(1)).is_none(),
            "a refused run leaves NO reservation — it never started"
        );
        assert_eq!(gate.reserve_refusals(), 1);
        assert_eq!(gate.runs_dispatched(), 0);
    }

    /// **Settle never interrupts an in-flight run.** A run reserved + begun is `InFlight`; the only
    /// exit is its OWN settle. The ledger's only teardown (`cancel_unstarted`) is structurally barred
    /// from an in-flight row, and neither the gate nor the handle exposes an interrupt — so the run
    /// runs to completion and the interrupt counter stays 0.
    #[test]
    fn settle_never_interrupts_an_in_flight_run() {
        let mut gate = IssueSpendGate::new();
        let mut ledger = CostLedger::new();
        let dispatched = gate
            .reserve_run(
                &mut ledger,
                tenant(),
                run(1),
                IssueRunKind::SlaDraft,
                MicroUsd(500),
                MicroUsd(1_000),
            )
            .expect("a funded run is dispatched");
        assert_eq!(
            ledger.state_of(&tenant(), &run(1)),
            Some(ReservationState::InFlight),
            "the dispatched run is in-flight"
        );
        // the in-flight run is NEVER torn down (the ledger bars its only teardown on an in-flight row).
        assert!(
            ledger.cancel_unstarted(&tenant(), &run(1)).is_err(),
            "an in-flight run is NEVER interrupted"
        );
        assert_eq!(ledger.inflight_interrupt_count(), 0, "0 interrupts");
        // the run STILL settles normally — it kept running.
        let outcome = dispatched.settle(&mut ledger, &units(200, 50)).unwrap();
        assert_eq!(outcome.billed_total, MicroUsd(250));
        assert_eq!(
            ledger.state_of(&tenant(), &run(1)),
            Some(ReservationState::Settled)
        );
    }

    /// **A run that settles its full reservation refunds nothing (reserve == billed, refund 0) — still
    /// balanced.** The boundary case: the actual cost exactly equals the reserve.
    #[test]
    fn a_run_billed_at_its_full_reserve_is_balanced_with_zero_refund() {
        let mut gate = IssueSpendGate::new();
        let mut ledger = CostLedger::new();
        let signal = spend_bearing_run(
            &mut gate,
            &mut ledger,
            tenant(),
            run(1),
            IssueRunKind::Automation,
            MicroUsd(400),
            MicroUsd(400), // available == estimate (affordable; the floor is `available < amount`)
            || units(300, 100), // bills exactly 400 = the reserve
        )
        .expect("a run billed at its full reserve");
        assert_eq!(signal.billed, MicroUsd(400));
        assert_eq!(signal.refunded, MicroUsd(0), "nothing to refund");
        assert!(
            signal.is_green(),
            "reserve == billed (refund 0) is balanced"
        );
    }

    /// **A re-dispatch of a live run is rejected loudly (a run is fronted exactly once).** The
    /// idempotency guard — never a double-reserve of a live run.
    #[test]
    fn redispatch_of_a_live_run_is_rejected() {
        let mut gate = IssueSpendGate::new();
        let mut ledger = CostLedger::new();
        gate.reserve_run(
            &mut ledger,
            tenant(),
            run(1),
            IssueRunKind::Triage,
            MicroUsd(100),
            MicroUsd(1_000),
        )
        .unwrap();
        let err = gate
            .reserve_run(
                &mut ledger,
                tenant(),
                run(1),
                IssueRunKind::Triage,
                MicroUsd(100),
                MicroUsd(1_000),
            )
            .expect_err("a second dispatch of a live run is rejected");
        assert_eq!(err, SpendError::AlreadyDispatched);
    }

    /// **A double-settle never double-charges (the settle is idempotent).** Settling a completed run
    /// twice returns the same outcome and records no further cost events — a double-completion signal
    /// is one bill.
    #[test]
    fn double_settle_never_double_charges() {
        let mut gate = IssueSpendGate::new();
        let mut ledger = CostLedger::new();
        let dispatched = gate
            .reserve_run(
                &mut ledger,
                tenant(),
                run(1),
                IssueRunKind::Triage,
                MicroUsd(1_000),
                MicroUsd(1_000),
            )
            .unwrap();
        let first = dispatched.settle(&mut ledger, &units(300, 100)).unwrap();
        let second = dispatched.settle(&mut ledger, &units(300, 100)).unwrap();
        assert_eq!(first, second, "a double-settle returns the same outcome");
        assert_eq!(
            ledger.cost_events_for(&tenant(), &run(1)).len(),
            1,
            "no further cost events on the second settle (no double-charge)"
        );
    }

    /// **The metered-unit dimension is the frozen `agent.effect` for every Issues run kind.** The cost
    /// dimension is frozen (a `&'static str`) — a future per-kind split adds a dimension, never a
    /// silent re-label.
    #[test]
    fn every_run_kind_bills_the_frozen_agent_effect_dimension() {
        for kind in [
            IssueRunKind::Triage,
            IssueRunKind::Forecast,
            IssueRunKind::SlaDraft,
            IssueRunKind::Automation,
        ] {
            assert_eq!(kind.metered_unit(), "agent.effect");
        }
        // the observability tokens are the frozen taxonomy (a rename fails the audit/trace).
        assert_eq!(IssueRunKind::Triage.as_str(), "triage");
        assert_eq!(IssueRunKind::Forecast.as_str(), "forecast");
        assert_eq!(IssueRunKind::SlaDraft.as_str(), "sla_draft");
        assert_eq!(IssueRunKind::Automation.as_str(), "automation");
    }

    /// **An UNBALANCED signal reads RED (proving `is_green` is not vacuously true).** A reserve that
    /// is not fully accounted (`billed + refunded != reserved`), a cost-event/unit mismatch, or an
    /// in-flight interrupt each classifies NOT green.
    #[test]
    fn an_unbalanced_signal_is_not_green() {
        // a leaked reserve: 1000 reserved but only 400 + 100 = 500 accounted (500 vanished).
        let leaked = BalancedRunSignal {
            tenant: tenant(),
            run: run(1),
            kind: IssueRunKind::Triage,
            reserved: MicroUsd(1_000),
            billed: MicroUsd(400),
            refunded: MicroUsd(100),
            cost_events: 1,
            metered_units: 1,
            inflight_interrupt_count: 0,
        };
        assert!(!leaked.is_green(), "a leaked reserve must read RED");

        // a cost-event/unit mismatch (2 units reported, 1 event recorded).
        let mismatch = BalancedRunSignal {
            tenant: tenant(),
            run: run(1),
            kind: IssueRunKind::Triage,
            reserved: MicroUsd(1_000),
            billed: MicroUsd(400),
            refunded: MicroUsd(600),
            cost_events: 1,
            metered_units: 2,
            inflight_interrupt_count: 0,
        };
        assert!(!mismatch.is_green(), "a cost-event/unit mismatch reads RED");

        // an in-flight interrupt (the headline zero violated).
        let interrupted = BalancedRunSignal {
            tenant: tenant(),
            run: run(1),
            kind: IssueRunKind::Triage,
            reserved: MicroUsd(1_000),
            billed: MicroUsd(400),
            refunded: MicroUsd(600),
            cost_events: 1,
            metered_units: 1,
            inflight_interrupt_count: 1,
        };
        assert!(!interrupted.is_green(), "an in-flight interrupt reads RED");
    }

    // ───────────────────── the per-effect idem_key rule (OQ-F) ─────────────────

    /// **The per-effect `idem_key` rule (OQ-F): single-effect → `card_id`; multi → `card_id:idx`.**
    /// The Issues-side statement of the frozen agent-fabric rule (a CDC pins parity).
    #[test]
    fn per_effect_idem_key_follows_the_frozen_oq_f_rule() {
        // single-effect card: the bare card_id (a double-click is one approval).
        assert_eq!(
            per_effect_idem_key("card:R1:triage", 0, 1),
            "card:R1:triage"
        );
        // multi/partial-approval card: each effect keys on `card_id:effect_idx`.
        assert_eq!(
            per_effect_idem_key("card:R1:batch", 0, 3),
            "card:R1:batch:0"
        );
        assert_eq!(
            per_effect_idem_key("card:R1:batch", 1, 3),
            "card:R1:batch:1"
        );
        assert_eq!(
            per_effect_idem_key("card:R1:batch", 2, 3),
            "card:R1:batch:2"
        );
        // a partial approval (approve 0 + 2, decline 1) has three DISTINCT keys → exactly-once each.
        let k0 = per_effect_idem_key("card:R1:batch", 0, 3);
        let k1 = per_effect_idem_key("card:R1:batch", 1, 3);
        let k2 = per_effect_idem_key("card:R1:batch", 2, 3);
        assert_ne!(k0, k1);
        assert_ne!(k1, k2);
        assert_ne!(k0, k2);
    }

    /// **An out-of-range effect_idx panics in debug (a caller bug — the index must address an
    /// effect).** The same precondition the agent-fabric derivation enforces.
    #[test]
    #[should_panic(expected = "must index into")]
    fn per_effect_idem_key_panics_on_out_of_range_idx() {
        let _ = per_effect_idem_key("card", 3, 3);
    }

    /// **The error Displays are loud + specific (observability is part of the pass).**
    #[test]
    fn spend_error_displays_are_loud() {
        let e = SpendError::NoBalance {
            requested: MicroUsd(9_000),
            available: MicroUsd(100),
        }
        .to_string();
        assert!(
            e.contains("no balance, no start"),
            "must cite the floor: {e}"
        );
        assert!(
            e.contains("NEVER started"),
            "must say it never started: {e}"
        );
        assert!(!SpendError::AlreadyDispatched.to_string().is_empty());
        assert!(!SpendError::AmountOverflow.to_string().is_empty());
        assert!(SpendError::Settle(SettleError::NoSuchReservation)
            .to_string()
            .contains("settle failed"));
    }

    /// **The `DispatchError` → `SpendError` conversion maps every variant (one error type at the
    /// Issues run boundary).**
    #[test]
    fn dispatch_error_maps_to_spend_error() {
        assert_eq!(
            SpendError::from(DispatchError::AlreadyDispatched),
            SpendError::AlreadyDispatched
        );
        assert_eq!(
            SpendError::from(DispatchError::AmountOverflow),
            SpendError::AmountOverflow
        );
        assert_eq!(
            SpendError::from(DispatchError::NoBalance {
                requested: MicroUsd(5),
                available: MicroUsd(1),
            }),
            SpendError::NoBalance {
                requested: MicroUsd(5),
                available: MicroUsd(1),
            }
        );
    }
}

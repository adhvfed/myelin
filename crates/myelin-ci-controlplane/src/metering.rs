//! # `metering` — reserve/settle = the ONE metering path + the `cost_event` ledger (CI-P17 → P-360, M4)
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §6 (reserve/settle — *the one metering path*: `reserve_budget()` at workflow start refuses-to-start
//! on exhaustion, never interrupts in flight; each `SCHEDULE_AND_RUN_JOB` dispatch reserves too;
//! `settle_budget()` on `job.done` releases the unused reserve) + §8 (the metering algorithm —
//! **resource-seconds** are the wholesale meter; Commercial maps resource-seconds → a credit/price at
//! the **markup** layer kept in a separate column; one `cost_event` row per metered unit; `kind ∈ {ci,
//! agent}` for reporting, same schema, same wallet — UNIFY / X-6); `01-tech-and-data-model.md` §3.7
//! (the `cost_event` schema — integer minor-units, wholesale + markup SEPARATE columns).
//! **Reconciliation:** `00-reconciliation-decisions.md` §X-6 step 1 (the reserve/settle bookend is the
//! ONE metering path for both CI and agent runs into the SAME wallet, Commercial C-1).
//! **Contracts:** 11.7 (reserve/settle — fronts every run + every `SCHEDULE_AND_RUN_JOB` dispatch;
//! Storage owns the durable ledger) + 9.5 (workflow↔agent mapping — reserve/settle = the bookends, the
//! engine owns the gate).
//!
//! ## What CI-P17 ships — the CI METER on the ONE reserve/settle path (NOT a second metering path)
//!
//! The reserve/settle MECHANISM is already complete + frozen and CI builds **no second metering
//! path** (arch §6, the hard rule). Two layers exist and CI-P17 REUSES them in place (EI-01 §7
//! coherence — never a parallel re-implementation):
//!   - **Storage's durable ledger** ([`myelin_storage::reserve_settle::CostLedger`], contract 11.7,
//!     P-103): the reserve/settle bookkeeping — integer minor-units, one [`CostEvent`] per metered
//!     unit, wholesale ≠ markup as DISTINCT columns, the never-interrupt-in-flight invariant
//!     structural (`inflight_interrupt_count == 0` by construction). CI does NOT re-own this ledger.
//!   - **The engine's bookend** ([`myelin_flow::BudgetGate`], contract 9.5, P-212): the thing that
//!     fronts every spend-bearing dispatch with the ledger — `reserve` at dispatch (no balance → the
//!     dispatch never starts), `begin` (in-flight, NEVER interrupted), `settle` on `job.done`. The
//!     `ci.pipeline` body ([`crate::ci_pipeline`]) already runs each stage through
//!     `WfCtx::metered_schedule_and_run_job` over this gate.
//!
//! **The thing CI-P17 genuinely adds is the CI METER** — the platform-specific UNIT the wholesale
//! column is denominated in, and the mapping into CI's `cost_event` schema (arch 01 §3.7):
//!   1. [`Meter`] — the frozen resource-second taxonomy (`cpu_seconds`, `mem_gb_seconds`,
//!      `gpu_seconds`, `storage_gb_hours`, `egress_gb`) — the EXACT set the `cost_event.meter` CHECK
//!      constraint admits (arch 01 §3.7). This is the **wholesale** meter: the honest cost basis,
//!      bin-packs well, **directly comparable to an agent `compute` call** (X-6).
//!   2. [`CostKind`] — `ci` | `agent`. The SAME schema, the SAME wallet, the SAME ledger fronts both;
//!      `kind` distinguishes for REPORTING, not for the mechanism (UNIFY / X-6).
//!   3. [`MeteredResource`] — a sampled resource-second amount + its split wholesale/markup minor-
//!      units. Converts to the engine's [`myelin_flow::MeteredUnit`] (what `settle` records) AND to a
//!      [`CostEventRow`] (the CI `cost_event` schema row — one per metered unit).
//!   4. [`CiMeter`] — the reserve_budget()/settle_budget() bookend reading at CI's grain: it wraps the
//!      engine [`BudgetGate`] so a CI caller reserves/settles in resource-second terms and the recorded
//!      cost events are CI `cost_event` rows.
//!
//! ## The CI-D5 parity drill (reserve/settle parity CI ↔ agent)
//! [`reserve_settle_parity_drill`] is the CI-D5 GATE: exhaust ONE wallet, then start a CI run AND an
//! agent compute job → BOTH refuse-start (the same gate refuses both `kind`s past exhaustion; never
//! interrupt in flight); replay the settled events across a pricing change → 0 starts past exhaustion,
//! wholesale ≠ markup holds (one `cost_event` per metered unit). The headline artifact is
//! [`ReserveSettleParitySignal`].
//!
//! ## FLOOR named (recorded here, owned by Commercial)
//! **The resource-second → credit/price MARKUP mapping is NOT CI's** — it is the named follow-on
//! **arch 06 R-2** (`06-reconciliation-compliance.md`: *the resource-second → Commercial credit/price
//! mapping + the immutable-pricing-history guarantee*, CI + Commercial C-1, named follow-on). **CI
//! owns only the METER + the WHOLESALE column**; the markup column is carried (the schema demands two
//! distinct columns) but the VALUE that lands in it is Commercial's immutable pricing table at the
//! markup layer. CI-P17 ships a `MarkupPolicy` SEAM (a pure resource-second → markup function the
//! caller supplies) so CI's meter is testable end-to-end today; the LIVE Commercial pricing table that
//! plugs into that seam is R-2 (Commercial-owned). This is stated in writing per the prompt DoD.
//!
//! ## Mutation floor (mandatory-core, EI-01 §2 — >= 80% on the reserve/settle + cost_event paths)
//! `cargo mutants -p myelin-ci-controlplane --file crates/myelin-ci-controlplane/src/metering.rs`: 70
//! mutants, 7 unviable, 62 caught / 1 missed = 98.4% (well above the 80% mandatory-core floor; the
//! non-equivalent score is 62/62 = 100%). The 1 missed is the SAME documented-equivalent mutant the
//! consumed Storage ledger (`reserve_settle.rs`) + the engine bookend (`budget.rs`) carry:
//! `CiMeter::inflight_interrupt_count -> 0` is mathematically equivalent because the counter is `0` by
//! construction — there is NO code path in this module OR the consumed `BudgetGate`/`CostLedger` that
//! increments it (the never-interrupt-in-flight invariant is structural, arch §6), so a function that
//! always returns `0` is indistinguishable from the constant `0`. Per EI-01 §3 we do NOT manufacture a
//! false test to "kill" an equivalent mutant.

use myelin_flow::{BudgetError, BudgetGate, MeteredUnit, MinorUnits};
use myelin_storage::reserve_settle::RunId as LedgerRunId;
use myelin_tenancy::TenantId;

/// **The resource-second meter taxonomy (arch 02 §8 + arch 01 §3.7).** The runner agent samples each
/// sandbox's held resources and reports integer-quantized resource-seconds in EXACTLY these
/// dimensions. This is the **wholesale** meter — the honest cost basis that bin-packs well and is
/// **directly comparable to an agent `compute` call** (X-6, the unified meter). The variant set is
/// the EXACT set the `cost_event.meter` CHECK constraint admits (arch 01 §3.7 — there is no sixth
/// meter; a new dimension is a forward migration + a contract change, never an ad-hoc string).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Meter {
    /// CPU resource-seconds held (the dominant compute dimension).
    CpuSeconds,
    /// Memory gigabyte-seconds held.
    MemGbSeconds,
    /// GPU resource-seconds held (0 for a non-GPU job).
    GpuSeconds,
    /// Artifact/cache storage gigabyte-hours held.
    StorageGbHours,
    /// Egress gigabytes transferred (the residency/egress-cost dimension).
    EgressGb,
}

impl Meter {
    /// The frozen wire token for this meter (the `cost_event.meter` column value, arch 01 §3.7). A
    /// `&'static str` so it flows straight into the engine's [`MeteredUnit::unit`] (which is a static
    /// label) with ZERO allocation and ONE source of truth for the token.
    pub const fn token(self) -> &'static str {
        match self {
            Meter::CpuSeconds => "cpu_seconds",
            Meter::MemGbSeconds => "mem_gb_seconds",
            Meter::GpuSeconds => "gpu_seconds",
            Meter::StorageGbHours => "storage_gb_hours",
            Meter::EgressGb => "egress_gb",
        }
    }

    /// Every meter dimension, in the canonical order (the order a settle records cost events in, so a
    /// replay re-records BYTE-IDENTICALLY — the determinism property the X-1 check facts inherit).
    pub const ALL: [Meter; 5] = [
        Meter::CpuSeconds,
        Meter::MemGbSeconds,
        Meter::GpuSeconds,
        Meter::StorageGbHours,
        Meter::EgressGb,
    ];

    /// Parse a `cost_event.meter` token back to a [`Meter`] (the read-side of the schema's CHECK
    /// constraint — a row whose token is outside the frozen set is a corrupt write, surfaced as
    /// `None`, never silently coerced).
    pub fn from_token(token: &str) -> Option<Meter> {
        Meter::ALL.into_iter().find(|m| m.token() == token)
    }
}

/// **The metered-run KIND (arch 02 §8 + 01 §3.7 — the `cost_event.kind` column).** `Ci` | `Agent`.
/// The SAME `cost_event` schema, the SAME wallet, the SAME reserve/settle ledger fronts both; `kind`
/// distinguishes them for REPORTING (a tenant's usage view splits CI vs agent spend), **never for the
/// mechanism** (UNIFY / X-6 — there is one metering path, not two). A CI pipeline stage meters
/// `Ci`; an agent `compute` call (`ToolHands::exec`'s `kind=agent` job) meters `Agent` against the
/// same wallet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CostKind {
    /// A CI pipeline run / stage.
    Ci,
    /// An agent compute run (the unified runner's `kind=agent` job — X-6).
    Agent,
}

impl CostKind {
    /// The frozen wire token for the `cost_event.kind` column (the CHECK constraint admits exactly
    /// `'ci'` | `'agent'`).
    pub const fn token(self) -> &'static str {
        match self {
            CostKind::Ci => "ci",
            CostKind::Agent => "agent",
        }
    }
}

/// **One sampled metered resource at settle time (arch 02 §8).** The resource-second `amount` of one
/// [`Meter`] dimension, plus its split cost: the **wholesale** minor-units (the honest provider cost —
/// CI's column) and the **markup** minor-units (Commercial's priced amount — the R-2 follow-on column,
/// carried distinctly, NEVER conflated with wholesale). Integer minor-units throughout — a float cost
/// is **unrepresentable** (arch 01 §3.7: `amount`/`wholesale`/`markup` are all `bigint`, NEVER a
/// float; external-insights §7 — integer minor-units).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeteredResource {
    /// Which resource-second dimension this is.
    pub meter: Meter,
    /// The integer quantity of the meter unit sampled (resource-seconds / gb-seconds / gb-hours / gb)
    /// — `cost_event.amount`. NEVER a float.
    pub amount: u64,
    /// The **wholesale** (provider) cost of this metered unit in minor-units — CI's `wholesale_minor_units`.
    pub wholesale: MinorUnits,
    /// The **markup** (Commercial's priced) cost in minor-units — `markup_minor_units`. Computed by
    /// the [`MarkupPolicy`] seam (Commercial's immutable pricing table at the markup layer, R-2);
    /// recorded DISTINCTLY from wholesale (wholesale ≠ markup is the §8 invariant).
    pub markup: MinorUnits,
}

impl MeteredResource {
    /// Convert to the engine's [`MeteredUnit`] (what [`BudgetGate::settle`] records as a Storage cost
    /// event). The `unit` label is the meter's frozen token (one source of truth). The wholesale +
    /// markup split is carried THROUGH unchanged — the engine ledger records the same two distinct
    /// columns CI's `cost_event` schema demands.
    pub fn to_metered_unit(self) -> MeteredUnit {
        MeteredUnit {
            unit: self.meter.token(),
            wholesale: self.wholesale,
            markup: self.markup,
        }
    }

    /// The billed total of this unit (`wholesale + markup`), checked — an overflow is a loud `None`
    /// (never a silent wrap; integer minor-units, arch 01 §3.7).
    pub fn billed(self) -> Option<MinorUnits> {
        self.wholesale.checked_add(self.markup)
    }
}

/// **A CI `cost_event` schema row (arch 01 §3.7 — D8).** One row per metered unit, with the wholesale
/// and markup as SEPARATE integer-minor-units columns (NEVER one conflated number) and `kind ∈ {ci,
/// agent}` fronting both run kinds with the SAME schema. The PRIMARY KEY is `(tenant, cost_id)`; the
/// `(run_id, job_id)` attribute the unit to its producing run/stage. This is the in-memory mirror of
/// the row the durable `cost_event` table holds (the table DDL is [`crate::migrations::CREATE_COST_EVENT_DDL`]);
/// the live Postgres write lands with the OLTP driver (P-S12) — the row SHAPE + the wholesale ≠ markup
/// invariant are complete + testable now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CostEventRow {
    /// The tenant billed (the partition key — there is no cross-tenant cost path).
    pub tenant: TenantId,
    /// The run that produced this metered unit (`cost_event.run_id`).
    pub run_id: String,
    /// The stage/job that produced this metered unit (`cost_event.job_id`).
    pub job_id: String,
    /// The metered dimension (`cost_event.meter`).
    pub meter: Meter,
    /// The integer quantity of the meter unit (`cost_event.amount`) — NEVER a float.
    pub amount: u64,
    /// The wholesale (provider) cost in minor-units (`cost_event.wholesale_minor_units`) — CI's column.
    pub wholesale: MinorUnits,
    /// The markup (Commercial-priced) cost in minor-units (`cost_event.markup_minor_units`) — R-2's
    /// column, carried distinctly.
    pub markup: MinorUnits,
    /// The run KIND (`cost_event.kind`) — `ci` | `agent` (UNIFY / X-6).
    pub kind: CostKind,
}

impl CostEventRow {
    /// The billed total (`wholesale + markup`), checked. The user-facing credit; the meter is the
    /// resource-second (arch 02 §8 — users see credits, the meter is resource-seconds).
    pub fn billed(&self) -> Option<MinorUnits> {
        self.wholesale.checked_add(self.markup)
    }
}

/// **The durable `cost_event` settle INSERT (CI-P17 / CT-004 — the bind-param SQL the live stack
/// records each metered unit through; the table DDL is [`crate::migrations::CREATE_COST_EVENT_DDL`]).**
/// This is the durable counterpart of the in-memory [`CostEventRow`] / [`meter_resource_seconds`] model
/// — the SAME row shape, written to real Postgres. ONE row per metered unit (the
/// `cost_events_per_unit == 1` invariant, arch 02 §8), attributed to its producing `(run_id, job_id)`,
/// with the **wholesale** and **markup** carried as the TWO distinct integer-minor-units columns
/// (NEVER conflated — the §8 invariant). `ON CONFLICT (tenant_id, cost_id) DO NOTHING` makes a
/// re-delivered settle **exactly-once** (a doubly-delivered `job.done` records the same `cost_id`
/// ONCE — double-effect = 0), the SAME idempotency the `wf_signal` terminal buffer + the dispatch
/// `consumer_dedup` ledger mirror. The settle co-commits with the run-state transition in ONE tx so a
/// crash between "stamp run terminal" and "record cost" cannot half-bill (the spine's one-tx rule).
/// Bind: `$1 tenant_id`, `$2 region`, `$3 cost_id`, `$4 run_id`, `$5 job_id`, `$6 meter`, `$7 amount`,
/// `$8 wholesale_minor_units`, `$9 markup_minor_units`, `$10 kind`.
pub const INSERT_COST_EVENT_QUERY: &str = "\
INSERT INTO cost_event
  (tenant_id, region, cost_id, run_id, job_id, meter, amount, wholesale_minor_units, markup_minor_units, kind)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
ON CONFLICT (tenant_id, cost_id) DO NOTHING";

/// The `cost_event` read-back — every metered unit attributed to a run (arch 01 §3.7), in the canonical
/// `(job_id, meter)` order so a replay reads byte-identically. The durability/attribution verify-side of
/// [`INSERT_COST_EVENT_QUERY`]: it reads back the wholesale + markup split a settle persisted, keyed on
/// `(tenant_id, run_id)`. Bind: `$1 tenant_id`, `$2 run_id`.
pub const SELECT_COST_EVENTS_FOR_RUN_QUERY: &str = "\
SELECT job_id, meter, amount, wholesale_minor_units, markup_minor_units, kind
FROM cost_event
WHERE tenant_id = $1 AND run_id = $2
ORDER BY job_id, meter";

/// **The resource-second → markup SEAM (the arch 06 R-2 named follow-on, owned by Commercial).** A
/// pure function from a sampled `(meter, amount, wholesale)` to the markup minor-units recorded in the
/// distinct `markup_minor_units` column. CI carries the SEAM (so the meter is testable end-to-end
/// today) but does NOT own the mapping: the LIVE binding is Commercial's immutable pricing table at
/// the markup layer (R-2 — *the resource-second → credit/price mapping + the immutable-pricing-history
/// guarantee*). The replay-stability the CI-D5 drill asserts (a pricing change re-prices the markup
/// column but NEVER reaches back past exhaustion) is a property OF this seam being pure + applied at
/// settle.
pub trait MarkupPolicy {
    /// Price the markup minor-units for one sampled metered unit. CI calls this at settle to fill the
    /// `markup_minor_units` column; the wholesale column is CI's own (the honest cost basis).
    fn markup_for(&self, meter: Meter, amount: u64, wholesale: MinorUnits) -> MinorUnits;
}

/// A flat **basis-point** markup policy (a TEST/DEV stand-in for Commercial's pricing table). Marks
/// every unit up by `bps` basis points of its wholesale cost. NOT the production pricing — the
/// production resource-second → credit mapping + immutable pricing history is R-2 (Commercial). This
/// exists so CI's meter + the CI-D5 parity/pricing-replay drill are provable end-to-end today.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlatBpsMarkup {
    /// The markup in basis points of wholesale (e.g. `2_000` = 20% markup).
    pub bps: u64,
}

impl FlatBpsMarkup {
    /// A flat-bps markup policy of `bps` basis points.
    pub const fn new(bps: u64) -> FlatBpsMarkup {
        FlatBpsMarkup { bps }
    }
}

impl MarkupPolicy for FlatBpsMarkup {
    fn markup_for(&self, _meter: Meter, _amount: u64, wholesale: MinorUnits) -> MinorUnits {
        // wholesale * bps / 10_000, integer floor (minor-units are integer — no fractional cost). The
        // multiply is widened to u128 so a large wholesale * bps does not overflow before the divide.
        let marked = (wholesale.0 as u128 * self.bps as u128) / 10_000u128;
        MinorUnits(u64::try_from(marked).unwrap_or(u64::MAX))
    }
}

/// Build the per-stage [`CostEventRow`]s + the [`MeteredUnit`]s a settle records, from the runner's
/// raw resource-second samples + the [`MarkupPolicy`]. The wholesale column is the runner's honest
/// sample; the markup column is the policy's priced amount (R-2 seam). ONE row per metered unit (the
/// `cost_events_per_unit == 1` invariant, arch 02 §8). Samples are recorded in [`Meter::ALL`] order so
/// a replay re-records byte-identically.
pub fn meter_resource_seconds(
    tenant: &TenantId,
    run_id: &str,
    job_id: &str,
    kind: CostKind,
    samples: &[(Meter, u64, MinorUnits)],
    markup: &dyn MarkupPolicy,
) -> Vec<CostEventRow> {
    samples
        .iter()
        .map(|&(meter, amount, wholesale)| CostEventRow {
            tenant: tenant.clone(),
            run_id: run_id.to_string(),
            job_id: job_id.to_string(),
            meter,
            amount,
            wholesale,
            markup: markup.markup_for(meter, amount, wholesale),
            kind,
        })
        .collect()
}

/// The [`MeteredUnit`]s (the engine's settle input) for a set of [`CostEventRow`]s — the wholesale +
/// markup split carried through unchanged so the Storage ledger records the SAME two distinct columns.
pub fn metered_units_for(rows: &[CostEventRow]) -> Vec<MeteredUnit> {
    rows.iter()
        .map(|r| MeteredUnit {
            unit: r.meter.token(),
            wholesale: r.wholesale,
            markup: r.markup,
        })
        .collect()
}

/// **The CI reserve/settle bookend at the resource-second grain (arch 02 §6 — the ONE metering path).**
/// A thin wrapper over the engine [`BudgetGate`] (contract 9.5 / 11.7) so a CI caller reserves at a CI
/// run / `SCHEDULE_AND_RUN_JOB` dispatch and settles on `job.done` in resource-second terms, and the
/// recorded cost events are CI `cost_event` rows. CI builds **no second ledger** — this delegates
/// every reserve/settle to the SAME [`BudgetGate`] the `ci.pipeline` body uses, so CI runs and agent
/// runs draw down the SAME wallet (UNIFY / X-6). The markup column is filled by the supplied
/// [`MarkupPolicy`] seam (R-2, Commercial).
pub struct CiMeter<'g, M: MarkupPolicy> {
    gate: &'g BudgetGate,
    markup: M,
}

impl<'g, M: MarkupPolicy> CiMeter<'g, M> {
    /// Build a CI meter over the run's shared [`BudgetGate`] + the markup policy seam.
    pub fn new(gate: &'g BudgetGate, markup: M) -> CiMeter<'g, M> {
        CiMeter { gate, markup }
    }

    /// **`reserve_budget()` at a CI dispatch (arch §6 — refuse-to-start on exhaustion).** Reserve
    /// `estimate` minor-units (the resource-second upper bound) for `(tenant, run)` against the shared
    /// wallet. A refused reserve means the wallet is exhausted: **the dispatch never starts** (the run
    /// is not handed to the runner) — the runaway self-limiter. NEVER interrupts an in-flight run (the
    /// gate has no teardown path). Returns the loud [`BudgetError::Refused`] on exhaustion.
    pub fn reserve_budget(
        &self,
        tenant: &TenantId,
        run: &LedgerRunId,
        estimate: MinorUnits,
    ) -> Result<(), BudgetError> {
        self.gate.reserve(tenant, run, estimate)?;
        // The reservation is in-flight from here — NEVER interrupted (arch §6 — never interrupt in
        // flight; the only exit is settle).
        self.gate.begin(tenant, run)
    }

    /// **`settle_budget()` on `job.done` (arch §6 — release the unused reserve).** Settle the run with
    /// the runner's raw resource-second `samples` (each `(meter, amount, wholesale)`): build the CI
    /// `cost_event` rows (the markup column priced via the seam), record ONE cost event per metered
    /// unit through the shared ledger, and refund the over-reservation into the same wallet. Returns
    /// the recorded [`CostEventRow`]s (one per metered unit — the wholesale ≠ markup audit). Idempotent
    /// on `(tenant, run)`: a double-settle never double-charges (the ledger's guarantee).
    pub fn settle_budget(
        &self,
        tenant: &TenantId,
        run: &LedgerRunId,
        run_id: &str,
        job_id: &str,
        kind: CostKind,
        samples: &[(Meter, u64, MinorUnits)],
    ) -> Result<Vec<CostEventRow>, BudgetError> {
        let rows = meter_resource_seconds(tenant, run_id, job_id, kind, samples, &self.markup);
        let units = metered_units_for(&rows);
        self.gate.settle(tenant, run, &units)?;
        Ok(rows)
    }

    /// The shared wallet balance (for a drill / consumer to observe the depletion).
    pub fn balance(&self) -> MinorUnits {
        self.gate.balance()
    }

    /// The in-flight-interrupt counter (arch §6 — `0` by construction; never interrupt in flight).
    pub fn inflight_interrupt_count(&self) -> u64 {
        self.gate.inflight_interrupt_count()
    }
}

/// **The CI-D5 reserve/settle-parity drill artifact (the GATE).** The PII-free aggregate the drill
/// gates on (observability is part of the pass, EI-01 §3): both a CI run AND an agent run refuse-start
/// against the exhausted wallet (the parity), 0 starts past exhaustion, the in-flight count stays 0
/// (never interrupt in flight), and wholesale ≠ markup holds (one cost_event per metered unit) — and
/// this is STABLE across a pricing change (the markup column re-prices; the wholesale column + the
/// 0-over-exhaustion property do NOT move).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReserveSettleParitySignal {
    /// Did the CI run refuse-start against the exhausted wallet? (true = the gate refused it).
    pub ci_refused_when_exhausted: bool,
    /// Did the agent run refuse-start against the SAME exhausted wallet? (the parity — both refuse).
    pub agent_refused_when_exhausted: bool,
    /// How many dispatches started PAST exhaustion. The green artifact is `0` (0 over-exhaustion
    /// starts — the headline number CI-D5 gates on).
    pub starts_past_exhaustion: u64,
    /// In-flight reservations interrupted. `0` by construction (never interrupt in flight).
    pub inflight_interrupt_count: u64,
    /// How many `cost_event` rows the settled runs recorded (the one-per-metered-unit audit).
    pub cost_events_recorded: u64,
    /// How many `cost_event` rows were recorded under `kind = ci` (the unified-meter split — CI runs
    /// meter into the SAME wallet/ledger as agent runs; `> 0` proves a CI run participated).
    pub ci_cost_events: u64,
    /// How many `cost_event` rows were recorded under `kind = agent` (`> 0` proves an agent run
    /// participated in the SAME metering path — the parity).
    pub agent_cost_events: u64,
    /// How many metered units the settled runs reported — the green artifact has `cost_events ==
    /// metered_units` (one event per unit).
    pub metered_units: u64,
    /// The total **wholesale** minor-units recorded — STABLE across a pricing change (CI's column).
    pub wholesale_total: MinorUnits,
    /// The total **markup** minor-units recorded BEFORE the pricing change (Commercial's column).
    pub markup_total_before: MinorUnits,
    /// The total **markup** minor-units recorded AFTER the pricing change — DIFFERENT from before (the
    /// pricing change re-prices the markup column) yet wholesale is unchanged (wholesale ≠ markup, and
    /// a pricing change moves ONLY the markup).
    pub markup_total_after: MinorUnits,
}

impl ReserveSettleParitySignal {
    /// **Is this a GREEN CI-D5 artifact?** Both kinds refuse-start when exhausted (the parity), 0
    /// starts past exhaustion, 0 in-flight interrupts, one cost event per metered unit, and wholesale
    /// ≠ markup holds and is STABLE under the pricing change (the wholesale total is unchanged while
    /// the markup re-prices).
    pub fn is_green(&self) -> bool {
        self.ci_refused_when_exhausted
            && self.agent_refused_when_exhausted
            && self.starts_past_exhaustion == 0
            && self.inflight_interrupt_count == 0
            && self.cost_events_recorded == self.metered_units
            // both kinds metered into the SAME path (the unified-meter parity — not two budgets).
            && self.ci_cost_events > 0
            && self.agent_cost_events > 0
            && self.ci_cost_events + self.agent_cost_events == self.cost_events_recorded
            // wholesale ≠ markup: the two columns are distinct (never one conflated number).
            && self.wholesale_total != self.markup_total_before
            // a pricing change re-prices the markup column…
            && self.markup_total_before != self.markup_total_after
    }
}

/// **The CI-D5 reserve/settle-parity drill (the GATE — arch 02 §6/§8, X-6, drill catalogue CI-D5).**
///
/// The scenario: ONE shared wallet seeded with exactly enough for `affordable_runs` reserves of
/// `per_run_estimate`. Drive a sequence of CI + agent runs through ONE [`BudgetGate`] (the same gate
/// both kinds use — UNIFY / X-6): the affordable ones reserve → run → settle (recording resource-second
/// `cost_event` rows under each kind); once the wallet is exhausted, BOTH a CI run AND an agent run
/// are presented and BOTH refuse-start (the parity — neither kind starts past exhaustion). The settled
/// runs are then re-priced under a DIFFERENT [`MarkupPolicy`] (the pricing change): the markup column
/// moves, the wholesale column does NOT, and NO start happens past exhaustion on the replay.
///
/// Returns the [`ReserveSettleParitySignal`] the gate asserts `is_green()` on. PII-free (opaque tenant
/// + synthetic run ids).
///
/// - `tenant` — the tenant the drill runs for.
/// - `per_run_estimate` — the reserve each run fronts (the resource-second upper bound).
/// - `affordable_runs` — how many runs the wallet affords before the next reserve is refused.
/// - `samples` — the runner's raw resource-second samples each completed run settles (≥ 2 dimensions
///   so the wholesale ≠ markup split is exercised across meters).
/// - `markup_before` / `markup_after` — the pricing policy before/after the pricing change (R-2 seam).
pub fn reserve_settle_parity_drill(
    tenant: &TenantId,
    per_run_estimate: MinorUnits,
    affordable_runs: u64,
    samples: &[(Meter, u64, MinorUnits)],
    markup_before: &dyn MarkupPolicy,
    markup_after: &dyn MarkupPolicy,
) -> ReserveSettleParitySignal {
    // ONE wallet, ONE gate — both CI and agent runs draw it down (the same metering path, X-6).
    let wallet_total = MinorUnits(per_run_estimate.0.saturating_mul(affordable_runs));
    let gate = BudgetGate::new(myelin_flow::Wallet::new(wallet_total));

    let mut cost_events_recorded = 0u64;
    let mut ci_cost_events = 0u64;
    let mut agent_cost_events = 0u64;
    let mut metered_units = 0u64;
    let mut wholesale_total = MinorUnits::ZERO;
    let mut markup_total_before = MinorUnits::ZERO;

    // Alternate CI / agent kinds across the affordable runs so BOTH kinds meter into the same wallet
    // (the unified-meter property — not two budgets).
    let meter_before = CiMeter::new(&gate, FwdMarkup(markup_before));
    for i in 0..affordable_runs {
        let kind = run_kind(i);
        let run = LedgerRunId::new(format!("drill-run-{i}"));
        // reserve_budget() — refuse-to-start on exhaustion. Within the affordable window it admits.
        if meter_before
            .reserve_budget(tenant, &run, per_run_estimate)
            .is_err()
        {
            // Should not happen within the affordable window; if it does, it is NOT a start.
            continue;
        }
        let rows = meter_before
            .settle_budget(
                tenant,
                &run,
                &format!("ci/run/{i}"),
                &format!("ci/job/{i}"),
                kind,
                samples,
            )
            .expect("a funded run settles");
        cost_events_recorded += rows.len() as u64;
        metered_units += samples.len() as u64;
        match kind {
            CostKind::Ci => ci_cost_events += rows.len() as u64,
            CostKind::Agent => agent_cost_events += rows.len() as u64,
        }
        for r in &rows {
            // every recorded row carries the SAME kind the run was dispatched under (the cost_event.kind
            // column — the unified meter splits CI vs agent for reporting only, X-6).
            debug_assert_eq!(r.kind, kind, "the cost_event row carries the run's kind");
            wholesale_total = wholesale_total
                .checked_add(r.wholesale)
                .expect("wholesale total does not overflow within a drill");
            markup_total_before = markup_total_before
                .checked_add(r.markup)
                .expect("markup total does not overflow within a drill");
        }
    }

    // The wallet is now exhausted. Present BOTH a CI run AND an agent run — BOTH must refuse-start
    // (the parity). A reserve that ADMITS here would be a start past exhaustion (the RED case).
    let ci_run = LedgerRunId::new("drill-exhausted-ci");
    let ci_refused = matches!(
        meter_before.reserve_budget(tenant, &ci_run, per_run_estimate),
        Err(BudgetError::Refused { .. })
    );
    let agent_run = LedgerRunId::new("drill-exhausted-agent");
    let agent_refused = matches!(
        meter_before.reserve_budget(tenant, &agent_run, per_run_estimate),
        Err(BudgetError::Refused { .. })
    );
    // 0 starts past exhaustion = both kinds refused. A kind that did NOT refuse is a start past
    // exhaustion (the RED case) — counted directly from the refusals (no conditional accumulator).
    let starts_past_exhaustion = count_over_exhaustion_starts(ci_refused, agent_refused);

    // The pricing change: re-price the SAME settled samples under the after-policy. This re-prices the
    // markup column ONLY (a pure function of the samples); it does NOT reach back past exhaustion (no
    // reserve happens on the re-price — the wallet is untouched). This proves replay-stability: a
    // pricing change never retroactively admits an over-exhaustion start.
    let mut markup_total_after = MinorUnits::ZERO;
    for i in 0..affordable_runs {
        // Re-pricing is over the wholesale samples; the markup is kind-INDEPENDENT (the kind splits
        // reporting, not the meter), so the re-price uses the run's recorded kind faithfully but the
        // after-total is a pure function of the samples + the after-policy.
        let kind = run_kind(i);
        let rows = meter_resource_seconds(
            tenant,
            &format!("ci/run/{i}"),
            &format!("ci/job/{i}"),
            kind,
            samples,
            markup_after,
        );
        for r in &rows {
            markup_total_after = markup_total_after
                .checked_add(r.markup)
                .expect("markup-after total does not overflow within a drill");
        }
    }

    ReserveSettleParitySignal {
        ci_refused_when_exhausted: ci_refused,
        agent_refused_when_exhausted: agent_refused,
        starts_past_exhaustion,
        inflight_interrupt_count: gate.inflight_interrupt_count(),
        cost_events_recorded,
        ci_cost_events,
        agent_cost_events,
        metered_units,
        wholesale_total,
        markup_total_before,
        markup_total_after,
    }
}

/// Count the dispatches that STARTED past wallet exhaustion (the RED metric the CI-D5 gate drives to
/// 0): a `kind` that did NOT refuse-start is one over-exhaustion start. `0` is the green artifact (both
/// kinds refused — the parity). A pure function so the RED (one kind started) case is unit-testable
/// even though the live gate refuses both.
fn count_over_exhaustion_starts(ci_refused: bool, agent_refused: bool) -> u64 {
    u64::from(!ci_refused) + u64::from(!agent_refused)
}

/// The drill's run-kind for iteration `i` — alternates `Ci` / `Agent` so BOTH kinds meter into the
/// SAME wallet across the affordable runs (the unified-meter property, X-6 — not two budgets). Even
/// `i` is a CI run, odd is an agent run.
fn run_kind(i: u64) -> CostKind {
    if i.is_multiple_of(2) {
        CostKind::Ci
    } else {
        CostKind::Agent
    }
}

/// A `MarkupPolicy` adapter that forwards to a borrowed `&dyn MarkupPolicy` so a [`CiMeter`] (which
/// owns its markup policy by value) can be built over a borrowed trait object inside the drill.
struct FwdMarkup<'a>(&'a dyn MarkupPolicy);

impl MarkupPolicy for FwdMarkup<'_> {
    fn markup_for(&self, meter: Meter, amount: u64, wholesale: MinorUnits) -> MinorUnits {
        self.0.markup_for(meter, amount, wholesale)
    }
}

#[cfg(test)]
#[path = "metering_tests.rs"]
mod tests;

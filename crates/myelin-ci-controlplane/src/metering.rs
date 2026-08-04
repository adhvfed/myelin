use myelin_flow::{BudgetError, BudgetGate, MeteredUnit, MicroUsd};
use myelin_storage::reserve_settle::RunId as LedgerRunId;
use myelin_tenancy::TenantId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Meter {
    CpuSeconds,
    MemGbSeconds,
    GpuSeconds,
    StorageGbHours,
    EgressGb,
}

impl Meter {
    pub const fn token(self) -> &'static str {
        match self {
            Meter::CpuSeconds => "cpu_seconds",
            Meter::MemGbSeconds => "mem_gb_seconds",
            Meter::GpuSeconds => "gpu_seconds",
            Meter::StorageGbHours => "storage_gb_hours",
            Meter::EgressGb => "egress_gb",
        }
    }

    pub const ALL: [Meter; 5] = [
        Meter::CpuSeconds,
        Meter::MemGbSeconds,
        Meter::GpuSeconds,
        Meter::StorageGbHours,
        Meter::EgressGb,
    ];

    pub fn from_token(token: &str) -> Option<Meter> {
        Meter::ALL.into_iter().find(|m| m.token() == token)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CostKind {
    Ci,
    Agent,
}

impl CostKind {
    pub const fn token(self) -> &'static str {
        match self {
            CostKind::Ci => "ci",
            CostKind::Agent => "agent",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeteredResource {
    pub meter: Meter,
    pub amount: u64,
    pub wholesale: MicroUsd,
    pub markup: MicroUsd,
}

impl MeteredResource {
    pub fn to_metered_unit(self) -> MeteredUnit {
        MeteredUnit {
            unit: self.meter.token(),
            wholesale: self.wholesale,
            markup: self.markup,
        }
    }

    pub fn billed(self) -> Option<MicroUsd> {
        self.wholesale.checked_add(self.markup)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CostEventRow {
    pub tenant: TenantId,
    pub run_id: String,
    pub job_id: String,
    pub meter: Meter,
    pub amount: u64,
    pub wholesale: MicroUsd,
    pub markup: MicroUsd,
    pub kind: CostKind,
}

impl CostEventRow {
    pub fn billed(&self) -> Option<MicroUsd> {
        self.wholesale.checked_add(self.markup)
    }
}

pub const INSERT_COST_EVENT_QUERY: &str = "\
INSERT INTO ci_cost_event
  (tenant_id, region, cost_id, run_id, job_id, meter, amount, wholesale_minor_units, markup_minor_units, kind)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
ON CONFLICT (tenant_id, cost_id) DO NOTHING";

pub const SELECT_COST_EVENTS_FOR_RUN_QUERY: &str = "\
SELECT job_id, meter, amount, wholesale_minor_units, markup_minor_units, kind
FROM ci_cost_event
WHERE tenant_id = $1 AND run_id = $2
ORDER BY job_id, meter";

pub const SELECT_COST_EVENT_BY_ID_QUERY: &str = "\
SELECT amount, wholesale_minor_units, markup_minor_units
FROM ci_cost_event
WHERE tenant_id = $1 AND cost_id = $2";

pub trait MarkupPolicy {
    fn markup_for(&self, meter: Meter, amount: u64, wholesale: MicroUsd) -> MicroUsd;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlatBpsMarkup {
    pub bps: u64,
}

impl FlatBpsMarkup {
    pub const fn new(bps: u64) -> FlatBpsMarkup {
        FlatBpsMarkup { bps }
    }
}

impl MarkupPolicy for FlatBpsMarkup {
    fn markup_for(&self, _meter: Meter, _amount: u64, wholesale: MicroUsd) -> MicroUsd {
        let marked = (wholesale.0 as u128 * self.bps as u128) / 10_000u128;
        MicroUsd(u64::try_from(marked).unwrap_or(u64::MAX))
    }
}

pub fn meter_resource_seconds(
    tenant: &TenantId,
    run_id: &str,
    job_id: &str,
    kind: CostKind,
    samples: &[(Meter, u64, MicroUsd)],
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

pub fn metered_units_for(rows: &[CostEventRow]) -> Vec<MeteredUnit> {
    rows.iter()
        .map(|r| MeteredUnit {
            unit: r.meter.token(),
            wholesale: r.wholesale,
            markup: r.markup,
        })
        .collect()
}

pub struct CiMeter<'g, M: MarkupPolicy> {
    gate: &'g BudgetGate,
    markup: M,
}

impl<'g, M: MarkupPolicy> CiMeter<'g, M> {
    pub fn new(gate: &'g BudgetGate, markup: M) -> CiMeter<'g, M> {
        CiMeter { gate, markup }
    }

    pub fn reserve_budget(
        &self,
        tenant: &TenantId,
        run: &LedgerRunId,
        estimate: MicroUsd,
    ) -> Result<(), BudgetError> {
        self.gate.reserve(tenant, run, estimate)?;
        self.gate.begin(tenant, run)
    }

    pub fn settle_budget(
        &self,
        tenant: &TenantId,
        run: &LedgerRunId,
        run_id: &str,
        job_id: &str,
        kind: CostKind,
        samples: &[(Meter, u64, MicroUsd)],
    ) -> Result<Vec<CostEventRow>, BudgetError> {
        let rows = meter_resource_seconds(tenant, run_id, job_id, kind, samples, &self.markup);
        let units = metered_units_for(&rows);
        self.gate.settle(tenant, run, &units)?;
        Ok(rows)
    }

    pub fn balance(&self) -> MicroUsd {
        self.gate.balance()
    }

    pub fn inflight_interrupt_count(&self) -> u64 {
        self.gate.inflight_interrupt_count()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReserveSettleParitySignal {
    pub ci_refused_when_exhausted: bool,
    pub agent_refused_when_exhausted: bool,
    pub starts_past_exhaustion: u64,
    pub inflight_interrupt_count: u64,
    pub cost_events_recorded: u64,
    pub ci_cost_events: u64,
    pub agent_cost_events: u64,
    pub metered_units: u64,
    pub wholesale_total: MicroUsd,
    pub markup_total_before: MicroUsd,
    pub markup_total_after: MicroUsd,
}

impl ReserveSettleParitySignal {
    pub fn is_green(&self) -> bool {
        self.ci_refused_when_exhausted
            && self.agent_refused_when_exhausted
            && self.starts_past_exhaustion == 0
            && self.inflight_interrupt_count == 0
            && self.cost_events_recorded == self.metered_units
            && self.ci_cost_events > 0
            && self.agent_cost_events > 0
            && self.ci_cost_events + self.agent_cost_events == self.cost_events_recorded
            && self.wholesale_total != self.markup_total_before
            && self.markup_total_before != self.markup_total_after
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn reserve_settle_parity_drill(
    tenant: &TenantId,
    per_run_estimate: MicroUsd,
    affordable_runs: u64,
    samples: &[(Meter, u64, MicroUsd)],
    markup_before: &dyn MarkupPolicy,
    markup_after: &dyn MarkupPolicy,
) -> ReserveSettleParitySignal {
    let wallet_total = MicroUsd(per_run_estimate.0.saturating_mul(affordable_runs));
    let gate = BudgetGate::new(myelin_flow::Wallet::new(wallet_total));

    let mut cost_events_recorded = 0u64;
    let mut ci_cost_events = 0u64;
    let mut agent_cost_events = 0u64;
    let mut metered_units = 0u64;
    let mut wholesale_total = MicroUsd::ZERO;
    let mut markup_total_before = MicroUsd::ZERO;

    let meter_before = CiMeter::new(&gate, FwdMarkup(markup_before));
    for i in 0..affordable_runs {
        let kind = run_kind(i);
        let run = LedgerRunId::new(format!("drill-run-{i}"));
        if meter_before
            .reserve_budget(tenant, &run, per_run_estimate)
            .is_err()
        {
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
            debug_assert_eq!(r.kind, kind, "the cost_event row carries the run's kind");
            wholesale_total = wholesale_total
                .checked_add(r.wholesale)
                .expect("wholesale total does not overflow within a drill");
            markup_total_before = markup_total_before
                .checked_add(r.markup)
                .expect("markup total does not overflow within a drill");
        }
    }

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
    let starts_past_exhaustion = count_over_exhaustion_starts(ci_refused, agent_refused);

    let mut markup_total_after = MicroUsd::ZERO;
    for i in 0..affordable_runs {
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

#[cfg(any(test, feature = "test-support"))]
fn count_over_exhaustion_starts(ci_refused: bool, agent_refused: bool) -> u64 {
    u64::from(!ci_refused) + u64::from(!agent_refused)
}

#[cfg(any(test, feature = "test-support"))]
fn run_kind(i: u64) -> CostKind {
    if i.is_multiple_of(2) {
        CostKind::Ci
    } else {
        CostKind::Agent
    }
}

#[cfg(any(test, feature = "test-support"))]
struct FwdMarkup<'a>(&'a dyn MarkupPolicy);

#[cfg(any(test, feature = "test-support"))]
impl MarkupPolicy for FwdMarkup<'_> {
    fn markup_for(&self, meter: Meter, amount: u64, wholesale: MicroUsd) -> MicroUsd {
        self.0.markup_for(meter, amount, wholesale)
    }
}

#[cfg(test)]
#[path = "metering_tests.rs"]
mod tests;

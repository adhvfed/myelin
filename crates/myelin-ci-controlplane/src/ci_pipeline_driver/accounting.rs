use std::sync::Arc;

use myelin_ci_sandbox::ResourceUsage;
use myelin_storage::{DurableCostLedger, MicroUsd, TenantScope};
use myelin_tenancy::TenantId;

use crate::ci_drive_manifest::CiDriveManifestStore;
use crate::cost_store::CiCostEventStore;
use crate::job_accounting_store::CiJobAccountingStore;
use crate::metering::{CostEventRow, CostKind, Meter};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiUsageAggregationError {
    Overflow,
    DurableRange,
}

impl core::fmt::Display for CiUsageAggregationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Overflow => f.write_str("CI usage aggregation overflowed"),
            Self::DurableRange => {
                f.write_str("CI usage aggregation exceeds the durable bigint range")
            }
        }
    }
}

impl std::error::Error for CiUsageAggregationError {}

pub(crate) fn checked_accounting_usage(
    usage: ResourceUsage,
) -> Result<ResourceUsage, CiUsageAggregationError> {
    if i64::try_from(usage.cpu_seconds).is_err() || i64::try_from(usage.mem_byte_seconds).is_err() {
        return Err(CiUsageAggregationError::DurableRange);
    }
    Ok(usage)
}

pub(crate) fn checked_add_accounting_usage(
    left: ResourceUsage,
    right: ResourceUsage,
) -> Result<ResourceUsage, CiUsageAggregationError> {
    checked_accounting_usage(ResourceUsage {
        cpu_seconds: left
            .cpu_seconds
            .checked_add(right.cpu_seconds)
            .ok_or(CiUsageAggregationError::Overflow)?,
        mem_byte_seconds: left
            .mem_byte_seconds
            .checked_add(right.mem_byte_seconds)
            .ok_or(CiUsageAggregationError::Overflow)?,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PricedCiJobUsage {
    pub pricing_revision: String,
    pub memory_gb_seconds: u64,
    pub cpu_wholesale: MicroUsd,
    pub cpu_markup: MicroUsd,
    pub memory_wholesale: MicroUsd,
    pub memory_markup: MicroUsd,
}

pub const TIER_P_OPERATIONAL_PRICING_REVISION: &str = "tier-p-operational:v1";
pub const MICRO_USD_PER_CPU_SECOND: u64 = 10_000;
pub const MICRO_USD_PER_GB_SECOND: u64 = 10_000;
pub(crate) const TIER_P_OPERATIONAL_RESERVATION_PREFIX: &str = "ci-reserve:v1:";
pub(crate) const TIER_P_OPERATIONAL_RESERVATION_V2_PREFIX: &str = "ci-reserve:v2:";
const PRICING_GIB_BYTES: u64 = 1_073_741_824;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiJobPricingError {
    Unavailable,
    InvalidOutput,
}

impl core::fmt::Display for CiJobPricingError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Unavailable => f.write_str("CI job pricing authority is unavailable"),
            Self::InvalidOutput => f.write_str("CI job pricing authority returned invalid output"),
        }
    }
}

impl std::error::Error for CiJobPricingError {}

pub trait CiJobAccountingPricer: Send + Sync {
    fn price(&self, usage: ResourceUsage) -> Result<PricedCiJobUsage, CiJobPricingError>;
}

pub(crate) fn validate_reservation_pricing_policy(
    reserve_handle: &str,
    usage: ResourceUsage,
    priced: &PricedCiJobUsage,
) -> Result<(), CiJobPricingError> {
    if !reserve_handle.starts_with(TIER_P_OPERATIONAL_RESERVATION_PREFIX)
        && !reserve_handle.starts_with(TIER_P_OPERATIONAL_RESERVATION_V2_PREFIX)
    {
        return Ok(());
    }
    let memory_gb_seconds = usage.mem_byte_seconds.div_ceil(PRICING_GIB_BYTES);
    let cpu_wholesale = usage
        .cpu_seconds
        .checked_mul(MICRO_USD_PER_CPU_SECOND)
        .ok_or(CiJobPricingError::InvalidOutput)?;
    let memory_wholesale = memory_gb_seconds
        .checked_mul(MICRO_USD_PER_GB_SECOND)
        .ok_or(CiJobPricingError::InvalidOutput)?;
    let exact_operational_policy = priced.pricing_revision == TIER_P_OPERATIONAL_PRICING_REVISION
        && priced.memory_gb_seconds == memory_gb_seconds
        && priced.cpu_wholesale == MicroUsd(cpu_wholesale)
        && priced.cpu_markup == MicroUsd::ZERO
        && priced.memory_wholesale == MicroUsd(memory_wholesale)
        && priced.memory_markup == MicroUsd::ZERO;
    if exact_operational_policy {
        Ok(())
    } else {
        Err(CiJobPricingError::InvalidOutput)
    }
}

#[derive(Clone)]
pub struct DurableCiJobAccounting {
    pub(super) scope: TenantScope,
    pub(super) manifest_store: CiDriveManifestStore,
    pub(super) money_ledger: DurableCostLedger,
    pub(super) cost_store: CiCostEventStore,
    pub(super) receipt_store: CiJobAccountingStore,
    pub(super) pricer: Arc<dyn CiJobAccountingPricer>,
}

impl DurableCiJobAccounting {
    pub fn new(
        scope: TenantScope,
        manifest_store: CiDriveManifestStore,
        money_ledger: DurableCostLedger,
        cost_store: CiCostEventStore,
        receipt_store: CiJobAccountingStore,
        pricer: Arc<dyn CiJobAccountingPricer>,
    ) -> Self {
        Self {
            scope,
            manifest_store,
            money_ledger,
            cost_store,
            receipt_store,
            pricer,
        }
    }
}

pub(crate) fn priced_cost_rows(
    tenant: &TenantId,
    ci_run_id: &str,
    job_id: &str,
    usage: ResourceUsage,
    priced: &PricedCiJobUsage,
) -> Result<Vec<CostEventRow>, CiJobPricingError> {
    if priced.pricing_revision.is_empty() || priced.pricing_revision.len() > 512 {
        return Err(CiJobPricingError::InvalidOutput);
    }
    Ok(vec![
        CostEventRow {
            tenant: tenant.clone(),
            run_id: ci_run_id.to_owned(),
            job_id: job_id.to_owned(),
            meter: Meter::CpuSeconds,
            amount: usage.cpu_seconds,
            wholesale: priced.cpu_wholesale,
            markup: priced.cpu_markup,
            kind: CostKind::Ci,
        },
        CostEventRow {
            tenant: tenant.clone(),
            run_id: ci_run_id.to_owned(),
            job_id: job_id.to_owned(),
            meter: Meter::MemGbSeconds,
            amount: priced.memory_gb_seconds,
            wholesale: priced.memory_wholesale,
            markup: priced.memory_markup,
            kind: CostKind::Ci,
        },
    ])
}

#[derive(Clone)]
pub(super) enum ReporterAccounting {
    Durable(Arc<DurableCiJobAccounting>),
    #[cfg(any(test, feature = "test-support"))]
    TestBypass,
}

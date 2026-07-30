//! Immutable per-job accounting receipts.
//!
//! The terminal reporter writes this row in the same PostgreSQL transaction as Storage's money
//! settlement, CI's metering projection, claim consumption, and the `job.done` signal. A repeated
//! completion may observe the existing row, but it is accepted only when every authoritative field
//! is identical; a conflicting replay fails closed.

use myelin_ci_sandbox::{PreparationPhase, PreparationTerminalDisposition, ResourceUsage};
use myelin_flow::MinorUnits;
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};
use sqlx::postgres::PgPool;
use sqlx::types::Uuid;
use sqlx::Row;

/// Insert one immutable accounting receipt. Bind order is documented by [`CiJobAccountingStore`].
pub const INSERT_CI_JOB_ACCOUNTING_QUERY: &str = "\
INSERT INTO ci_job_accounting
  (tenant_id, region, job_id, wf_run_id, ci_run_id, reserve_handle, passed, timed_out, skipped,
   cpu_seconds, mem_byte_seconds, pricing_revision, billed_minor_units, refunded_minor_units,
   completion_receipt, terminal_disposition, completion_receipt_v4)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)
ON CONFLICT (tenant_id, job_id) DO NOTHING
RETURNING job_id";

/// Read all immutable fields after a conflict so an idempotent replay cannot hide divergence.
pub const SELECT_CI_JOB_ACCOUNTING_QUERY: &str = "\
SELECT region, wf_run_id, ci_run_id, reserve_handle, passed, timed_out, skipped, cpu_seconds,
       mem_byte_seconds, pricing_revision, billed_minor_units, refunded_minor_units,
       completion_receipt, terminal_disposition, completion_receipt_v4
FROM ci_job_accounting
WHERE tenant_id = $1 AND job_id = $2";

/// The complete terminal-accounting fact for one claimed CI job.
#[derive(Clone, PartialEq, Eq)]
pub struct CiJobAccountingRecord {
    pub tenant: TenantId,
    pub job_id: String,
    pub wf_run_id: String,
    pub ci_run_id: String,
    pub reserve_handle: String,
    pub passed: bool,
    pub timed_out: bool,
    /// True for a manifest job settled without execution because a dependency failed or the whole
    /// run was cancelled before this job crossed the final launch fence.
    pub skipped: bool,
    pub usage: ResourceUsage,
    pub pricing_revision: String,
    pub billed: MinorUnits,
    pub refunded: MinorUnits,
    /// Closed, machine-readable terminal meaning for v4 receipts. `None` identifies a v3-compatible
    /// row, including fresh writes while production remains activation-gated to v3.
    pub disposition: Option<CiJobTerminalDisposition>,
    /// The authoritative receipt for this row's selected write generation.
    pub completion_receipt: String,
    /// V4 rows retain the exact v3 receipt in the byte-frozen legacy column so activation never
    /// requires weakening or replacing its shipped v3 CHECK/UNIQUE constraints.
    pub legacy_completion_receipt_v3: Option<String>,
}

/// Closed terminal-accounting vocabulary. Preparation dispositions deliberately carry no usage,
/// pass bit, or arbitrary text; those remain separate, independently validated accounting facts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiJobTerminalDisposition {
    WorkloadPassed,
    WorkloadFailed,
    WorkloadTimedOut,
    Preparation(PreparationTerminalDisposition),
    SkippedBeforeStart,
    CancelledDuringPreparation,
    CancelledAfterWorkloadLaunch,
}

impl CiJobTerminalDisposition {
    pub fn as_storage_token(self) -> &'static str {
        match self {
            Self::WorkloadPassed => "workload_passed",
            Self::WorkloadFailed => "workload_failed",
            Self::WorkloadTimedOut => "workload_timed_out",
            Self::Preparation(PreparationTerminalDisposition::Failed {
                phase: PreparationPhase::CheckoutTransport,
            }) => "checkout_transport_failed",
            Self::Preparation(PreparationTerminalDisposition::TimedOut {
                phase: PreparationPhase::CheckoutTransport,
            }) => "checkout_transport_timed_out",
            Self::Preparation(PreparationTerminalDisposition::Failed {
                phase: PreparationPhase::CheckoutMaterialization,
            }) => "checkout_materialization_failed",
            Self::Preparation(PreparationTerminalDisposition::TimedOut {
                phase: PreparationPhase::CheckoutMaterialization,
            }) => "checkout_materialization_timed_out",
            Self::Preparation(PreparationTerminalDisposition::AttemptsExhausted) => {
                "preparation_attempts_exhausted"
            }
            Self::SkippedBeforeStart => "skipped_before_start",
            Self::CancelledDuringPreparation => "cancelled_during_preparation",
            Self::CancelledAfterWorkloadLaunch => "cancelled_after_workload_launch",
        }
    }

    fn from_storage_token(value: &str) -> Option<Self> {
        Some(match value {
            "workload_passed" => Self::WorkloadPassed,
            "workload_failed" => Self::WorkloadFailed,
            "workload_timed_out" => Self::WorkloadTimedOut,
            "checkout_transport_failed" => {
                Self::Preparation(PreparationTerminalDisposition::Failed {
                    phase: PreparationPhase::CheckoutTransport,
                })
            }
            "checkout_transport_timed_out" => {
                Self::Preparation(PreparationTerminalDisposition::TimedOut {
                    phase: PreparationPhase::CheckoutTransport,
                })
            }
            "checkout_materialization_failed" => {
                Self::Preparation(PreparationTerminalDisposition::Failed {
                    phase: PreparationPhase::CheckoutMaterialization,
                })
            }
            "checkout_materialization_timed_out" => {
                Self::Preparation(PreparationTerminalDisposition::TimedOut {
                    phase: PreparationPhase::CheckoutMaterialization,
                })
            }
            "preparation_attempts_exhausted" => {
                Self::Preparation(PreparationTerminalDisposition::AttemptsExhausted)
            }
            "skipped_before_start" => Self::SkippedBeforeStart,
            "cancelled_during_preparation" => Self::CancelledDuringPreparation,
            "cancelled_after_workload_launch" => Self::CancelledAfterWorkloadLaunch,
            _ => return None,
        })
    }

    pub fn workload_started(self) -> bool {
        matches!(
            self,
            Self::WorkloadPassed
                | Self::WorkloadFailed
                | Self::WorkloadTimedOut
                | Self::CancelledAfterWorkloadLaunch
        )
    }
}

/// Bind a closed disposition to an existing deterministic v3 accounting receipt. Owners with
/// distinct historical v3 domains (normal completion, supersession, skipped-before-start) can
/// preserve those encoders byte-for-byte while converging on one v4 compatibility shape.
pub(crate) fn disposition_receipt_v4(
    legacy_v3: &str,
    disposition: CiJobTerminalDisposition,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"myelin.ci.accounting-disposition-receipt.v4\0");
    for field in [legacy_v3, disposition.as_storage_token()] {
        hasher.update(&(field.len() as u64).to_be_bytes());
        hasher.update(field.as_bytes());
    }
    format!("v4:{}", hasher.finalize().to_hex())
}

pub(crate) struct VersionedCiJobAccountingReceipt {
    pub disposition: Option<CiJobTerminalDisposition>,
    pub completion_receipt: String,
    pub legacy_completion_receipt_v3: Option<String>,
}

/// Select only the fresh-write representation. Replay readers remain dual-version regardless of
/// this choice.
pub(crate) fn versioned_accounting_receipt(
    write_version: CiJobAccountingWriteVersion,
    legacy_completion_receipt_v3: String,
    disposition: CiJobTerminalDisposition,
) -> VersionedCiJobAccountingReceipt {
    match write_version {
        CiJobAccountingWriteVersion::V3 => VersionedCiJobAccountingReceipt {
            disposition: None,
            completion_receipt: legacy_completion_receipt_v3,
            legacy_completion_receipt_v3: None,
        },
        CiJobAccountingWriteVersion::V4 => VersionedCiJobAccountingReceipt {
            disposition: Some(disposition),
            completion_receipt: disposition_receipt_v4(&legacy_completion_receipt_v3, disposition),
            legacy_completion_receipt_v3: Some(legacy_completion_receipt_v3),
        },
    }
}

impl core::fmt::Debug for CiJobAccountingRecord {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CiJobAccountingRecord")
            .field("tenant", &"<redacted>")
            .field("job_id", &"<redacted>")
            .field("wf_run_id", &"<redacted>")
            .field("ci_run_id", &"<redacted>")
            .field("reserve_handle", &"<redacted>")
            .field("passed", &self.passed)
            .field("timed_out", &self.timed_out)
            .field("skipped", &self.skipped)
            .field("usage", &"<redacted>")
            .field("pricing_revision", &"<redacted>")
            .field("billed", &"<redacted>")
            .field("refunded", &"<redacted>")
            .field("disposition", &self.disposition)
            .field("completion_receipt", &"<redacted>")
            .field("legacy_completion_receipt_v3", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiJobAccountingWrite {
    Inserted,
    ExactReplay,
}

/// Fresh-write format for immutable CI job accounting. Reads always understand both generations;
/// production remains pinned to v3 until the fleet can safely replay v4 queue receipts and result
/// summaries during a rolling deployment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiJobAccountingWriteVersion {
    V3,
    V4,
}

/// A safe-to-log refusal from the immutable accounting store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiJobAccountingError {
    ScopeMismatch,
    InvalidField(&'static str),
    ValueOverflow(&'static str),
    Db(&'static str),
    CorruptRow,
    ReplayDivergence,
}

impl core::fmt::Display for CiJobAccountingError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ScopeMismatch => f.write_str("CI job accounting scope rejected"),
            Self::InvalidField(field) => {
                write!(f, "CI job accounting refused an invalid {field}")
            }
            Self::ValueOverflow(field) => {
                write!(
                    f,
                    "CI job accounting {field} does not fit its durable column"
                )
            }
            Self::Db(operation) => write!(f, "durable CI job accounting failed during {operation}"),
            Self::CorruptRow => f.write_str("durable CI job accounting row is corrupt"),
            Self::ReplayDivergence => {
                f.write_str("CI job accounting replay diverged from the immutable receipt")
            }
        }
    }
}

impl std::error::Error for CiJobAccountingError {}

impl From<myelin_storage::PgError> for CiJobAccountingError {
    fn from(_: myelin_storage::PgError) -> Self {
        Self::Db("tenant-scoped transaction")
    }
}

#[derive(Clone)]
pub struct CiJobAccountingStore {
    pool: PgPool,
    region: Region,
    write_version: CiJobAccountingWriteVersion,
}

impl core::fmt::Debug for CiJobAccountingStore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CiJobAccountingStore")
            .field("pool", &"<redacted>")
            .field("region", &"<redacted>")
            .field("write_version", &self.write_version)
            .finish()
    }
}

impl CiJobAccountingStore {
    /// Production-safe constructor. Fresh writes remain v3 until every possible replay owner
    /// understands v4; additive v4 columns are still readable.
    pub fn with_pg(pool: PgPool, region: Region) -> Self {
        Self::with_pg_and_write_version(pool, region, CiJobAccountingWriteVersion::V3)
    }

    /// Explicit activation seam for tests and a future fleet-convergence switch.
    pub fn with_pg_and_write_version(
        pool: PgPool,
        region: Region,
        write_version: CiJobAccountingWriteVersion,
    ) -> Self {
        Self {
            pool,
            region,
            write_version,
        }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub(crate) fn write_version(&self) -> CiJobAccountingWriteVersion {
        self.write_version
    }

    /// Record or exactly verify a receipt on the caller's tenant-scoped transaction.
    ///
    /// Bind order: tenant, region, job, workflow run, CI run, reserve handle, verdict, timeout,
    /// CPU-seconds, memory-byte-seconds, pricing revision, billed, refunded, completion receipt.
    pub async fn record_in_tx(
        &self,
        conn: &mut sqlx::PgConnection,
        scope: &TenantScope,
        record: &CiJobAccountingRecord,
    ) -> Result<CiJobAccountingWrite, CiJobAccountingError> {
        if self.write_version == CiJobAccountingWriteVersion::V3 && record.disposition.is_some() {
            return Err(CiJobAccountingError::InvalidField(
                "v4 write while the accounting writer is pinned to v3",
            ));
        }
        if scope.tenant() != &record.tenant
            || scope.tenant().as_str().is_empty()
            || scope.region() != &self.region
            || scope.region().as_str().is_empty()
        {
            return Err(CiJobAccountingError::ScopeMismatch);
        }
        validate_record(record)?;

        let job_id = parse_uuid("job id", &record.job_id)?;
        let wf_run_id = parse_uuid("workflow run id", &record.wf_run_id)?;
        let ci_run_id = parse_uuid("CI run id", &record.ci_run_id)?;
        let cpu_seconds = fit_bigint("CPU usage", record.usage.cpu_seconds)?;
        let mem_byte_seconds = fit_bigint("memory usage", record.usage.mem_byte_seconds)?;
        let billed = fit_bigint("billed amount", record.billed.0)?;
        let refunded = fit_bigint("refunded amount", record.refunded.0)?;
        let tenant_id = record.tenant.as_str();
        let legacy_receipt = record
            .legacy_completion_receipt_v3
            .as_deref()
            .unwrap_or(record.completion_receipt.as_str());
        let disposition = record
            .disposition
            .map(CiJobTerminalDisposition::as_storage_token);
        let receipt_v4 = record
            .disposition
            .map(|_| record.completion_receipt.as_str());

        let inserted = sqlx::query(INSERT_CI_JOB_ACCOUNTING_QUERY)
            .bind(tenant_id)
            .bind(self.region.as_str())
            .bind(job_id)
            .bind(wf_run_id)
            .bind(ci_run_id)
            .bind(&record.reserve_handle)
            .bind(record.passed)
            .bind(record.timed_out)
            .bind(record.skipped)
            .bind(cpu_seconds)
            .bind(mem_byte_seconds)
            .bind(&record.pricing_revision)
            .bind(billed)
            .bind(refunded)
            .bind(legacy_receipt)
            .bind(disposition)
            .bind(receipt_v4)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|_| CiJobAccountingError::Db("receipt insert"))?;
        if inserted.is_some() {
            return Ok(CiJobAccountingWrite::Inserted);
        }

        let existing = sqlx::query(SELECT_CI_JOB_ACCOUNTING_QUERY)
            .bind(tenant_id)
            .bind(job_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(|_| CiJobAccountingError::Db("receipt replay verification"))?;

        let exact = existing.get::<String, _>("region") == self.region.as_str()
            && existing.get::<Uuid, _>("wf_run_id") == wf_run_id
            && existing.get::<Uuid, _>("ci_run_id") == ci_run_id
            && existing.get::<String, _>("reserve_handle") == record.reserve_handle
            && existing.get::<bool, _>("passed") == record.passed
            && existing.get::<bool, _>("timed_out") == record.timed_out
            && existing.get::<bool, _>("skipped") == record.skipped
            && existing.get::<i64, _>("cpu_seconds") == cpu_seconds
            && existing.get::<i64, _>("mem_byte_seconds") == mem_byte_seconds
            && existing.get::<String, _>("pricing_revision") == record.pricing_revision
            && existing.get::<i64, _>("billed_minor_units") == billed
            && existing.get::<i64, _>("refunded_minor_units") == refunded
            && existing.get::<String, _>("completion_receipt") == legacy_receipt
            && existing
                .get::<Option<String>, _>("terminal_disposition")
                .as_deref()
                == disposition
            && existing
                .get::<Option<String>, _>("completion_receipt_v4")
                .as_deref()
                == receipt_v4;
        if exact {
            Ok(CiJobAccountingWrite::ExactReplay)
        } else {
            Err(CiJobAccountingError::ReplayDivergence)
        }
    }

    /// Read the immutable receipt on the caller's scoped transaction. This is the terminal
    /// redelivery path: an already-consumed claim reuses historical pricing instead of consulting
    /// the current price table and accidentally repricing old work.
    pub async fn load_in_tx(
        &self,
        conn: &mut sqlx::PgConnection,
        scope: &TenantScope,
        job_id: &str,
    ) -> Result<Option<CiJobAccountingRecord>, CiJobAccountingError> {
        if scope.tenant().as_str().is_empty()
            || scope.region() != &self.region
            || scope.region().as_str().is_empty()
        {
            return Err(CiJobAccountingError::ScopeMismatch);
        }
        let job_uuid = parse_uuid("job id", job_id)?;
        let row = sqlx::query(SELECT_CI_JOB_ACCOUNTING_QUERY)
            .bind(scope.tenant().as_str())
            .bind(job_uuid)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|_| CiJobAccountingError::Db("receipt load"))?;
        let Some(row) = row else {
            return Ok(None);
        };
        if row.get::<String, _>("region") != self.region.as_str() {
            return Err(CiJobAccountingError::ScopeMismatch);
        }
        let nonnegative = |column: &'static str| -> Result<u64, CiJobAccountingError> {
            u64::try_from(row.get::<i64, _>(column)).map_err(|_| CiJobAccountingError::CorruptRow)
        };
        let disposition = row
            .get::<Option<String>, _>("terminal_disposition")
            .map(|value| {
                CiJobTerminalDisposition::from_storage_token(&value)
                    .ok_or(CiJobAccountingError::CorruptRow)
            })
            .transpose()?;
        let completion_receipt_v3: String = row.get("completion_receipt");
        let completion_receipt_v4: Option<String> = row.get("completion_receipt_v4");
        if disposition.is_some() != completion_receipt_v4.is_some() {
            return Err(CiJobAccountingError::CorruptRow);
        }
        Ok(Some(CiJobAccountingRecord {
            tenant: scope.tenant().clone(),
            job_id: job_id.to_owned(),
            wf_run_id: row.get::<Uuid, _>("wf_run_id").to_string(),
            ci_run_id: row.get::<Uuid, _>("ci_run_id").to_string(),
            reserve_handle: row.get("reserve_handle"),
            passed: row.get("passed"),
            timed_out: row.get("timed_out"),
            skipped: row.get("skipped"),
            usage: ResourceUsage {
                cpu_seconds: nonnegative("cpu_seconds")?,
                mem_byte_seconds: nonnegative("mem_byte_seconds")?,
            },
            pricing_revision: row.get("pricing_revision"),
            billed: MinorUnits(nonnegative("billed_minor_units")?),
            refunded: MinorUnits(nonnegative("refunded_minor_units")?),
            disposition,
            completion_receipt: completion_receipt_v4
                .unwrap_or_else(|| completion_receipt_v3.clone()),
            legacy_completion_receipt_v3: disposition.map(|_| completion_receipt_v3),
        }))
    }

    /// Convenience path for callers that need only the accounting receipt, not a larger co-commit.
    pub async fn record(
        &self,
        scope: &TenantScope,
        record: &CiJobAccountingRecord,
    ) -> Result<CiJobAccountingWrite, CiJobAccountingError> {
        let store = self.clone();
        let scope = scope.clone();
        let record = record.clone();
        let tenant = scope.tenant().as_str().to_owned();
        let region = scope.region().as_str().to_owned();
        myelin_storage::with_tenant_tx_error(&self.pool, &tenant, &region, move |conn| {
            Box::pin(async move { store.record_in_tx(conn, &scope, &record).await })
        })
        .await
    }
}

fn validate_record(record: &CiJobAccountingRecord) -> Result<(), CiJobAccountingError> {
    if (record.passed && (record.timed_out || record.skipped))
        || (record.timed_out && record.skipped)
    {
        return Err(CiJobAccountingError::InvalidField("terminal verdict"));
    }
    for (name, value) in [
        ("reserve handle", record.reserve_handle.as_str()),
        ("pricing revision", record.pricing_revision.as_str()),
    ] {
        if value.is_empty() || value.len() > 512 {
            return Err(CiJobAccountingError::InvalidField(name));
        }
    }
    match (
        record.disposition,
        record.legacy_completion_receipt_v3.as_deref(),
    ) {
        (None, None) if is_completion_receipt_v3(&record.completion_receipt) => {}
        (Some(disposition), Some(legacy))
            if is_completion_receipt_v4(&record.completion_receipt)
                && is_completion_receipt_v3(legacy)
                && disposition_matches_verdict(
                    disposition,
                    record.passed,
                    record.timed_out,
                    record.skipped,
                ) => {}
        _ => return Err(CiJobAccountingError::InvalidField("completion receipt")),
    }
    Ok(())
}

fn disposition_matches_verdict(
    disposition: CiJobTerminalDisposition,
    passed: bool,
    timed_out: bool,
    skipped: bool,
) -> bool {
    match disposition {
        CiJobTerminalDisposition::WorkloadPassed => passed && !timed_out && !skipped,
        CiJobTerminalDisposition::WorkloadTimedOut => !passed && timed_out && !skipped,
        CiJobTerminalDisposition::Preparation(PreparationTerminalDisposition::TimedOut {
            ..
        }) => !passed && timed_out && !skipped,
        CiJobTerminalDisposition::SkippedBeforeStart
        | CiJobTerminalDisposition::CancelledDuringPreparation => !passed && !timed_out && skipped,
        CiJobTerminalDisposition::WorkloadFailed
        | CiJobTerminalDisposition::Preparation(PreparationTerminalDisposition::Failed {
            ..
        })
        | CiJobTerminalDisposition::Preparation(
            PreparationTerminalDisposition::AttemptsExhausted,
        )
        | CiJobTerminalDisposition::CancelledAfterWorkloadLaunch => {
            !passed && !timed_out && !skipped
        }
    }
}

fn parse_uuid(field: &'static str, value: &str) -> Result<Uuid, CiJobAccountingError> {
    Uuid::parse_str(value).map_err(|_| CiJobAccountingError::InvalidField(field))
}

fn fit_bigint(field: &'static str, value: u64) -> Result<i64, CiJobAccountingError> {
    i64::try_from(value).map_err(|_| CiJobAccountingError::ValueOverflow(field))
}

fn is_completion_receipt_v3(value: &str) -> bool {
    value.len() == 67
        && value.starts_with("v3:")
        && value.as_bytes()[3..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

fn is_completion_receipt_v4(value: &str) -> bool {
    value.len() == 67
        && value.starts_with("v4:")
        && value.as_bytes()[3..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> CiJobAccountingRecord {
        CiJobAccountingRecord {
            tenant: TenantId::from_token("tenant-1"),
            job_id: "018f36a5-f6e9-7e41-8985-75f6aa57db1d".into(),
            wf_run_id: "018f36a5-f6e9-7e41-8985-75f6aa57db1e".into(),
            ci_run_id: "018f36a5-f6e9-7e41-8985-75f6aa57db1f".into(),
            reserve_handle: "reserve:1".into(),
            passed: true,
            timed_out: false,
            skipped: false,
            usage: ResourceUsage {
                cpu_seconds: 7,
                mem_byte_seconds: 11,
            },
            pricing_revision: "pricing:v1".into(),
            billed: MinorUnits(19),
            refunded: MinorUnits(4),
            disposition: None,
            completion_receipt: format!("v3:{}", "a".repeat(64)),
            legacy_completion_receipt_v3: None,
        }
    }

    #[test]
    fn receipt_validation_pins_version_lowercase_and_length() {
        let valid = record();
        assert_eq!(validate_record(&valid), Ok(()));

        for invalid in [
            format!("v2:{}", "a".repeat(64)),
            format!("v3:{}", "A".repeat(64)),
            format!("v3:{}", "a".repeat(63)),
            format!("v3:{}g", "a".repeat(63)),
        ] {
            let mut candidate = valid.clone();
            candidate.completion_receipt = invalid;
            assert_eq!(
                validate_record(&candidate),
                Err(CiJobAccountingError::InvalidField("completion receipt"))
            );
        }
    }

    #[test]
    fn v4_receipt_requires_a_closed_disposition_and_legacy_v3_twin() {
        let mut candidate = record();
        candidate.passed = false;
        candidate.disposition = Some(CiJobTerminalDisposition::WorkloadFailed);
        candidate.completion_receipt = format!("v4:{}", "b".repeat(64));
        candidate.legacy_completion_receipt_v3 = Some(format!("v3:{}", "a".repeat(64)));
        assert_eq!(validate_record(&candidate), Ok(()));

        candidate.legacy_completion_receipt_v3 = None;
        assert_eq!(
            validate_record(&candidate),
            Err(CiJobAccountingError::InvalidField("completion receipt"))
        );
    }

    #[tokio::test]
    async fn fresh_write_version_is_explicit_and_production_defaults_to_v3() {
        // @residency-cell-pinned: lazy unit-test pool that never connects; the store built from it
        // on the next line pins its region explicitly (`with_pg(pool, Region("fr-par"))`).
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1/unused")
            .unwrap();
        let production = CiJobAccountingStore::with_pg(pool, Region("fr-par".into()));
        assert_eq!(production.write_version(), CiJobAccountingWriteVersion::V3);

        let legacy = format!("v3:{}", "a".repeat(64));
        let v3 = versioned_accounting_receipt(
            CiJobAccountingWriteVersion::V3,
            legacy.clone(),
            CiJobTerminalDisposition::WorkloadFailed,
        );
        assert_eq!(v3.disposition, None);
        assert_eq!(v3.completion_receipt, legacy);
        assert_eq!(v3.legacy_completion_receipt_v3, None);

        let v4 = versioned_accounting_receipt(
            CiJobAccountingWriteVersion::V4,
            legacy.clone(),
            CiJobTerminalDisposition::WorkloadFailed,
        );
        assert_eq!(
            v4.disposition,
            Some(CiJobTerminalDisposition::WorkloadFailed)
        );
        assert!(v4.completion_receipt.starts_with("v4:"));
        assert_eq!(
            v4.legacy_completion_receipt_v3.as_deref(),
            Some(legacy.as_str())
        );
    }

    #[test]
    fn disposition_tokens_round_trip_and_bind_verdict_shape() {
        let all = [
            CiJobTerminalDisposition::WorkloadPassed,
            CiJobTerminalDisposition::WorkloadFailed,
            CiJobTerminalDisposition::WorkloadTimedOut,
            CiJobTerminalDisposition::Preparation(PreparationTerminalDisposition::Failed {
                phase: PreparationPhase::CheckoutTransport,
            }),
            CiJobTerminalDisposition::Preparation(PreparationTerminalDisposition::TimedOut {
                phase: PreparationPhase::CheckoutMaterialization,
            }),
            CiJobTerminalDisposition::Preparation(
                PreparationTerminalDisposition::AttemptsExhausted,
            ),
            CiJobTerminalDisposition::SkippedBeforeStart,
            CiJobTerminalDisposition::CancelledDuringPreparation,
            CiJobTerminalDisposition::CancelledAfterWorkloadLaunch,
        ];
        for disposition in all {
            assert_eq!(
                CiJobTerminalDisposition::from_storage_token(disposition.as_storage_token()),
                Some(disposition)
            );
        }
        assert!(disposition_matches_verdict(
            CiJobTerminalDisposition::WorkloadPassed,
            true,
            false,
            false
        ));
        assert!(!disposition_matches_verdict(
            CiJobTerminalDisposition::WorkloadPassed,
            false,
            false,
            false
        ));
    }

    #[test]
    fn contradictory_terminal_verdict_is_refused_before_sql() {
        let mut candidate = record();
        candidate.timed_out = true;
        assert_eq!(
            validate_record(&candidate),
            Err(CiJobAccountingError::InvalidField("terminal verdict"))
        );

        candidate.passed = false;
        candidate.skipped = true;
        assert_eq!(
            validate_record(&candidate),
            Err(CiJobAccountingError::InvalidField("terminal verdict"))
        );
    }

    #[test]
    fn query_is_insert_only_and_conflict_verified() {
        assert!(INSERT_CI_JOB_ACCOUNTING_QUERY.contains("ON CONFLICT"));
        assert!(INSERT_CI_JOB_ACCOUNTING_QUERY.contains("DO NOTHING"));
        assert!(!INSERT_CI_JOB_ACCOUNTING_QUERY.contains("DO UPDATE"));
        for field in [
            "region",
            "wf_run_id",
            "ci_run_id",
            "reserve_handle",
            "passed",
            "timed_out",
            "skipped",
            "cpu_seconds",
            "mem_byte_seconds",
            "pricing_revision",
            "billed_minor_units",
            "refunded_minor_units",
            "completion_receipt",
            "terminal_disposition",
            "completion_receipt_v4",
        ] {
            assert!(
                SELECT_CI_JOB_ACCOUNTING_QUERY.contains(field),
                "missing {field}"
            );
        }
    }
}

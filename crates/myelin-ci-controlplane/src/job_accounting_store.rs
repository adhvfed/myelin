//! Immutable per-job accounting receipts.
//!
//! The terminal reporter writes this row in the same PostgreSQL transaction as Storage's money
//! settlement, CI's metering projection, claim consumption, and the `job.done` signal. A repeated
//! completion may observe the existing row, but it is accepted only when every authoritative field
//! is identical; a conflicting replay fails closed.

use myelin_ci_sandbox::ResourceUsage;
use myelin_flow::MinorUnits;
use myelin_storage::TenantScope;
use myelin_tenancy::{Region, TenantId};
use sqlx::postgres::PgPool;
use sqlx::types::Uuid;
use sqlx::Row;

/// Insert one immutable accounting receipt. Bind order is documented by [`CiJobAccountingStore`].
pub const INSERT_CI_JOB_ACCOUNTING_QUERY: &str = "\
INSERT INTO ci_job_accounting
  (tenant_id, region, job_id, wf_run_id, ci_run_id, reserve_handle, passed, timed_out,
   cpu_seconds, mem_byte_seconds, pricing_revision, billed_minor_units, refunded_minor_units,
   completion_receipt)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
ON CONFLICT (tenant_id, job_id) DO NOTHING
RETURNING job_id";

/// Read all immutable fields after a conflict so an idempotent replay cannot hide divergence.
pub const SELECT_CI_JOB_ACCOUNTING_QUERY: &str = "\
SELECT region, wf_run_id, ci_run_id, reserve_handle, passed, timed_out, cpu_seconds,
       mem_byte_seconds, pricing_revision, billed_minor_units, refunded_minor_units,
       completion_receipt
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
    pub usage: ResourceUsage,
    pub pricing_revision: String,
    pub billed: MinorUnits,
    pub refunded: MinorUnits,
    pub completion_receipt: String,
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
            .field("usage", &"<redacted>")
            .field("pricing_revision", &"<redacted>")
            .field("billed", &"<redacted>")
            .field("refunded", &"<redacted>")
            .field("completion_receipt", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CiJobAccountingWrite {
    Inserted,
    ExactReplay,
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
}

impl core::fmt::Debug for CiJobAccountingStore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CiJobAccountingStore")
            .field("pool", &"<redacted>")
            .field("region", &"<redacted>")
            .finish()
    }
}

impl CiJobAccountingStore {
    pub fn with_pg(pool: PgPool, region: Region) -> Self {
        Self { pool, region }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
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

        let inserted = sqlx::query(INSERT_CI_JOB_ACCOUNTING_QUERY)
            .bind(record.tenant.as_str())
            .bind(self.region.as_str())
            .bind(job_id)
            .bind(wf_run_id)
            .bind(ci_run_id)
            .bind(&record.reserve_handle)
            .bind(record.passed)
            .bind(record.timed_out)
            .bind(cpu_seconds)
            .bind(mem_byte_seconds)
            .bind(&record.pricing_revision)
            .bind(billed)
            .bind(refunded)
            .bind(&record.completion_receipt)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|_| CiJobAccountingError::Db("receipt insert"))?;
        if inserted.is_some() {
            return Ok(CiJobAccountingWrite::Inserted);
        }

        let existing = sqlx::query(SELECT_CI_JOB_ACCOUNTING_QUERY)
            .bind(record.tenant.as_str())
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
            && existing.get::<i64, _>("cpu_seconds") == cpu_seconds
            && existing.get::<i64, _>("mem_byte_seconds") == mem_byte_seconds
            && existing.get::<String, _>("pricing_revision") == record.pricing_revision
            && existing.get::<i64, _>("billed_minor_units") == billed
            && existing.get::<i64, _>("refunded_minor_units") == refunded
            && existing.get::<String, _>("completion_receipt") == record.completion_receipt;
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
        Ok(Some(CiJobAccountingRecord {
            tenant: scope.tenant().clone(),
            job_id: job_id.to_owned(),
            wf_run_id: row.get::<Uuid, _>("wf_run_id").to_string(),
            ci_run_id: row.get::<Uuid, _>("ci_run_id").to_string(),
            reserve_handle: row.get("reserve_handle"),
            passed: row.get("passed"),
            timed_out: row.get("timed_out"),
            usage: ResourceUsage {
                cpu_seconds: nonnegative("cpu_seconds")?,
                mem_byte_seconds: nonnegative("mem_byte_seconds")?,
            },
            pricing_revision: row.get("pricing_revision"),
            billed: MinorUnits(nonnegative("billed_minor_units")?),
            refunded: MinorUnits(nonnegative("refunded_minor_units")?),
            completion_receipt: row.get("completion_receipt"),
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
    if record.passed && record.timed_out {
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
    if !is_completion_receipt_v3(&record.completion_receipt) {
        return Err(CiJobAccountingError::InvalidField("completion receipt"));
    }
    Ok(())
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
            usage: ResourceUsage {
                cpu_seconds: 7,
                mem_byte_seconds: 11,
            },
            pricing_revision: "pricing:v1".into(),
            billed: MinorUnits(19),
            refunded: MinorUnits(4),
            completion_receipt: format!("v3:{}", "a".repeat(64)),
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
    fn contradictory_terminal_verdict_is_refused_before_sql() {
        let mut candidate = record();
        candidate.timed_out = true;
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
            "cpu_seconds",
            "mem_byte_seconds",
            "pricing_revision",
            "billed_minor_units",
            "refunded_minor_units",
            "completion_receipt",
        ] {
            assert!(
                SELECT_CI_JOB_ACCOUNTING_QUERY.contains(field),
                "missing {field}"
            );
        }
    }
}

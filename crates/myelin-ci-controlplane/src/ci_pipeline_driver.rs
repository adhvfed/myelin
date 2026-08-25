use std::collections::BTreeMap;

#[cfg(test)]
use myelin_ci_sandbox::{
    CompletionClaim, IdemToken, RetryableAttemptCause, RetryableAttemptFailure, TrustTier,
};
use myelin_ci_sandbox::{
    PreparationPhase, PreparationTerminalDisposition, ResourceUsage, TerminalReport,
};
#[cfg(test)]
use myelin_refs::ArtifactRef;
#[cfg(test)]
use myelin_storage::MicroUsd;
use myelin_storage::{DurableSettleError, MeteredUnit, PgError, RunId as CostRunId};
use myelin_tenancy::Region;
use myelin_tenancy::TenantId;
use sqlx::types::Uuid;
use sqlx::Row;

use myelin_flow::{ExecutorError, RunId};

use crate::ci_job_result::{CiJobDisposition, CiJobResultSummary};
use crate::ci_manifest_job_runner::CiJobTokenRequest;
use crate::ci_prelaunch_usage_journal::{
    resolve_prelaunch_usage_on_conn, CiPrelaunchParentExpectation, CiPrelaunchSettlementIdentity,
    CiPrelaunchUnresolvedPolicy, CiPrelaunchUsageJournalError,
};
#[cfg(test)]
use crate::ci_run_store::CiRunRecord;
use crate::cost_store::CiCostStoreError;
use crate::job_accounting_store::{
    versioned_accounting_receipt, CiJobAccountingError, CiJobAccountingRecord,
    CiJobAccountingWriteVersion, CiJobTerminalDisposition,
};
#[cfg(test)]
use crate::job_schedule::JobScheduleTerms;
use crate::job_spec_store::CiJobSpecStoreError;
#[cfg(test)]
use crate::job_spec_store::ClaimedDispatchIdentity;
#[cfg(test)]
use crate::job_spec_store::MAX_JOB_TIMEOUT_SECS;
#[cfg(test)]
use crate::metering::Meter;
#[cfg(test)]
use crate::scheduler::Lane;

mod accounting;
mod completion;
mod reporter;
mod retry;
mod runner;

#[cfg(any(test, feature = "test-support"))]
mod driver;

pub(crate) use accounting::{
    checked_accounting_usage, checked_add_accounting_usage, priced_cost_rows,
    validate_reservation_pricing_policy, CiUsageAggregationError,
    TIER_P_OPERATIONAL_RESERVATION_PREFIX, TIER_P_OPERATIONAL_RESERVATION_V2_PREFIX,
};
pub use accounting::{
    CiJobAccountingPricer, CiJobPricingError, DurableCiJobAccounting, PricedCiJobUsage,
    MICRO_USD_PER_CPU_SECOND, MICRO_USD_PER_GB_SECOND, TIER_P_OPERATIONAL_PRICING_REVISION,
};
#[cfg(test)]
use completion::completion_receipt;
pub use completion::ClaimRefusal;
use completion::CompletionReceipts;
#[cfg(test)]
use completion::{
    completion_receipts_v4, preparation_completion_receipts, verify_claimed_identity,
    CompletionReceiptInput, PreparationCompletionReceiptInput,
};
#[cfg(test)]
use driver::validate_driver_tenant;
#[cfg(any(test, feature = "test-support"))]
pub use driver::{fixed_command_spec_builder, CiPipelineDriver, StartRunError};
pub(crate) use reporter::token_request_from_preparation_report_claim;
pub use reporter::{CiPipelineReporter, PreparationRetryOutcome};
pub(crate) use retry::decode_retry_attempt_usage;
#[cfg(test)]
use retry::{decode_retry_attempts, expected_retry_attempt_record};
#[cfg(test)]
use runner::build_dispatch_parts;
pub use runner::{unresolved_stage_spec_builder, DurableJobRunner, StageSpecBuilder};

fn bridge<F: std::future::Future>(rt: &tokio::runtime::Handle, fut: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(_) => tokio::task::block_in_place(|| rt.block_on(fut)),
        Err(_) => rt.block_on(fut),
    }
}

struct TerminalAccountingInput<'a> {
    tenant: &'a TenantId,
    wf_run: &'a RunId,
    ci_run_id: &'a str,
    job_id: &'a str,
    reserve_handle: &'a str,
    report: &'a TerminalReport,
    receipts: &'a CompletionReceipts,
    disposition: CiJobTerminalDisposition,
    diagnostic: Option<&'a str>,
    replay: bool,
}

struct TerminalSurfaceInput<'a> {
    report: &'a TerminalReport,
    disposition: Option<CiJobTerminalDisposition>,
    diagnostic: Option<&'a str>,
}

#[derive(Debug)]
pub(crate) enum CompletionTxError {
    Scope(PgError),
    Spec(CiJobSpecStoreError),
    Manifest,
    Pricing(CiJobPricingError),
    Money(DurableSettleError),
    Projection(CiCostStoreError),
    Accounting(CiJobAccountingError),
    CancelledClosure,
    Signal(ExecutorError),
    RetryStore,
    RetryCorrupt,
    Prelaunch(CiPrelaunchUsageJournalError),
    Usage(CiUsageAggregationError),
    Refused,
}

impl From<PgError> for CompletionTxError {
    fn from(error: PgError) -> Self {
        Self::Scope(error)
    }
}

async fn co_commit_terminal_accounting(
    conn: &mut sqlx::PgConnection,
    accounting: &DurableCiJobAccounting,
    input: TerminalAccountingInput<'_>,
) -> Result<(), CompletionTxError> {
    let surface_disposition = if input.replay {
        let existing = accounting
            .receipt_store
            .load_in_tx(conn, &accounting.scope, input.job_id)
            .await
            .map_err(CompletionTxError::Accounting)?
            .ok_or(CompletionTxError::Refused)?;
        let common_exact = existing.tenant == *input.tenant
            && existing.job_id == input.job_id
            && existing.wf_run_id == input.wf_run.0
            && existing.ci_run_id == input.ci_run_id
            && existing.reserve_handle == input.reserve_handle
            && existing.passed == input.report.passed
            && existing.timed_out == input.report.timed_out
            && existing.usage == input.report.usage;
        let receipt_exact = match existing.disposition {
            None => existing.completion_receipt == input.receipts.legacy_v3,
            Some(disposition) => {
                disposition == input.disposition
                    && existing.completion_receipt == input.receipts.current_v4
            }
        };
        if !common_exact || !receipt_exact {
            return Err(CompletionTxError::Refused);
        }
        existing.disposition
    } else {
        let priced = accounting
            .pricer
            .price(input.report.usage)
            .map_err(CompletionTxError::Pricing)?;
        validate_reservation_pricing_policy(input.reserve_handle, input.report.usage, &priced)
            .map_err(CompletionTxError::Pricing)?;
        let rows = priced_cost_rows(
            input.tenant,
            input.ci_run_id,
            input.job_id,
            input.report.usage,
            &priced,
        )
        .map_err(CompletionTxError::Pricing)?;
        let units: Vec<MeteredUnit> = rows
            .iter()
            .map(|row| MeteredUnit {
                unit: row.meter.token(),
                wholesale: row.wholesale,
                markup: row.markup,
            })
            .collect();
        let settled = accounting
            .money_ledger
            .settle_in_tx(
                conn,
                input.tenant,
                &CostRunId(input.reserve_handle.to_owned()),
                &units,
            )
            .await
            .map_err(CompletionTxError::Money)?;
        accounting
            .cost_store
            .settle_in_tx(conn, &accounting.scope, &rows)
            .await
            .map_err(CompletionTxError::Projection)?;
        let receipt = versioned_accounting_receipt(
            accounting.receipt_store.write_version(),
            input.receipts.legacy_v3.clone(),
            input.disposition,
        );
        accounting
            .receipt_store
            .record_in_tx(
                conn,
                &accounting.scope,
                &CiJobAccountingRecord {
                    tenant: input.tenant.clone(),
                    job_id: input.job_id.to_owned(),
                    wf_run_id: input.wf_run.0.clone(),
                    ci_run_id: input.ci_run_id.to_owned(),
                    reserve_handle: input.reserve_handle.to_owned(),
                    passed: input.report.passed,
                    timed_out: input.report.timed_out,
                    skipped: false,
                    usage: input.report.usage,
                    pricing_revision: priced.pricing_revision,
                    billed: settled.billed_total,
                    refunded: settled.refunded,
                    disposition: receipt.disposition,
                    completion_receipt: receipt.completion_receipt,
                    legacy_completion_receipt_v3: receipt.legacy_completion_receipt_v3,
                },
            )
            .await
            .map_err(CompletionTxError::Accounting)?;
        (accounting.receipt_store.write_version() == CiJobAccountingWriteVersion::V4)
            .then_some(input.disposition)
    };
    settle_ci_job_surface_on_conn(
        conn,
        input.tenant,
        accounting.scope.region(),
        input.ci_run_id,
        input.job_id,
        TerminalSurfaceInput {
            report: input.report,
            disposition: surface_disposition,
            diagnostic: input.diagnostic,
        },
    )
    .await?;
    Ok(())
}

struct TerminalUsageResolutionInput<'a> {
    tenant: &'a TenantId,
    wf_run_id: &'a str,
    job_id: &'a str,
    reserve_handle: &'a str,
    base_usage: ResourceUsage,
    parent_expectation: CiPrelaunchParentExpectation,
    unresolved_policy: CiPrelaunchUnresolvedPolicy,
}

async fn resolve_terminal_usage_on_conn(
    conn: &mut sqlx::PgConnection,
    accounting: &DurableCiJobAccounting,
    input: TerminalUsageResolutionInput<'_>,
) -> Result<(String, ResourceUsage), CompletionTxError> {
    let (manifest, _) = accounting
        .manifest_store
        .load_by_wf_run_on_conn(conn, input.wf_run_id)
        .await
        .map_err(|_| CompletionTxError::Manifest)?
        .ok_or(CompletionTxError::Refused)?;
    let granted_job = manifest
        .jobs
        .iter()
        .find(|job| job.job_id == input.job_id)
        .ok_or(CompletionTxError::Refused)?;
    if manifest.tenant_id != input.tenant.0 || granted_job.reserve_handle != input.reserve_handle {
        return Err(CompletionTxError::Refused);
    }
    let prelaunch = resolve_prelaunch_usage_on_conn(
        conn,
        CiPrelaunchSettlementIdentity {
            tenant_id: input.tenant.as_str(),
            region: accounting.scope.region().as_str(),
            job_id: input.job_id,
            wf_run_id: input.wf_run_id,
            ci_run_id: &manifest.ci_run_id,
            reserve_handle: input.reserve_handle,
        },
        input.parent_expectation,
        input.unresolved_policy,
    )
    .await
    .map_err(CompletionTxError::Prelaunch)?;
    let usage = checked_add_accounting_usage(input.base_usage, prelaunch.usage)
        .map_err(CompletionTxError::Usage)?;
    Ok((manifest.ci_run_id, usage))
}

async fn verify_preparation_disposition_on_conn(
    conn: &mut sqlx::PgConnection,
    claim: &CiJobTokenRequest,
    reserve_handle: &str,
    disposition: PreparationTerminalDisposition,
) -> Result<(), CompletionTxError> {
    let current = sqlx::query(
        "SELECT budget_revision, max_parent_attempts
         FROM ci_job_parent_attempt
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
           AND wf_run_id = $4::uuid AND ci_run_id = $5::uuid
           AND reserve_handle = $6 AND lease_owner = $7
           AND lease_epoch = $8 AND claim_nonce = $9::uuid
           AND claim_started_at_epoch_secs = $10
           AND claim_expires_at_epoch_secs = $11",
    )
    .bind(&claim.tenant_id)
    .bind(&claim.region)
    .bind(&claim.job_id)
    .bind(&claim.wf_run_id)
    .bind(&claim.ci_run_id)
    .bind(reserve_handle)
    .bind(&claim.lease_owner)
    .bind(claim.lease_epoch)
    .bind(&claim.claim_nonce)
    .bind(claim.claim_started_at_epoch_secs)
    .bind(claim.claim_expires_at_epoch_secs)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| CompletionTxError::Prelaunch(CiPrelaunchUsageJournalError::Database))?;

    match disposition {
        PreparationTerminalDisposition::Failed {
            phase: PreparationPhase::SecretResolution,
        } => {
            if current.is_some() {
                return Err(CompletionTxError::Refused);
            }
        }
        PreparationTerminalDisposition::TimedOut {
            phase: PreparationPhase::SecretResolution,
        } => return Err(CompletionTxError::Refused),
        PreparationTerminalDisposition::Failed { phase }
        | PreparationTerminalDisposition::TimedOut { phase } => {
            current.ok_or(CompletionTxError::Refused)?;
            let terminal = sqlx::query_scalar::<_, i32>(
                "SELECT 1
                 FROM ci_job_prelaunch_usage
                 WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
                   AND lease_epoch = $4 AND claim_nonce = $5::uuid AND phase = $6
                   AND status IN ('measured', 'sealed_ceiling')",
            )
            .bind(&claim.tenant_id)
            .bind(&claim.region)
            .bind(&claim.job_id)
            .bind(claim.lease_epoch)
            .bind(&claim.claim_nonce)
            .bind(phase.as_storage_token())
            .fetch_optional(&mut *conn)
            .await
            .map_err(|_| CompletionTxError::Prelaunch(CiPrelaunchUsageJournalError::Database))?;
            if terminal.is_none() {
                return Err(CompletionTxError::Refused);
            }
        }
        PreparationTerminalDisposition::AttemptsExhausted => {
            let (revision, maximum) = match &current {
                Some(row) => (
                    row.try_get::<i16, _>("budget_revision")
                        .map_err(|_| CompletionTxError::Refused)?,
                    row.try_get::<i64, _>("max_parent_attempts")
                        .map_err(|_| CompletionTxError::Refused)?,
                ),
                None => {
                    let policies = sqlx::query(
                        "SELECT budget_revision, max_parent_attempts
                         FROM ci_job_parent_attempt
                         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
                           AND wf_run_id = $4::uuid AND ci_run_id = $5::uuid
                           AND reserve_handle = $6
                         GROUP BY budget_revision, max_parent_attempts",
                    )
                    .bind(&claim.tenant_id)
                    .bind(&claim.region)
                    .bind(&claim.job_id)
                    .bind(&claim.wf_run_id)
                    .bind(&claim.ci_run_id)
                    .bind(reserve_handle)
                    .fetch_all(&mut *conn)
                    .await
                    .map_err(|_| {
                        CompletionTxError::Prelaunch(CiPrelaunchUsageJournalError::Database)
                    })?;
                    if policies.len() != 1 {
                        return Err(CompletionTxError::Refused);
                    }
                    (
                        policies[0]
                            .try_get::<i16, _>("budget_revision")
                            .map_err(|_| CompletionTxError::Refused)?,
                        policies[0]
                            .try_get::<i64, _>("max_parent_attempts")
                            .map_err(|_| CompletionTxError::Refused)?,
                    )
                }
            };
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*)
                 FROM ci_job_parent_attempt
                 WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
                   AND wf_run_id = $4::uuid AND ci_run_id = $5::uuid
                   AND reserve_handle = $6 AND budget_revision = $7
                   AND max_parent_attempts = $8",
            )
            .bind(&claim.tenant_id)
            .bind(&claim.region)
            .bind(&claim.job_id)
            .bind(&claim.wf_run_id)
            .bind(&claim.ci_run_id)
            .bind(reserve_handle)
            .bind(revision)
            .bind(maximum)
            .fetch_one(&mut *conn)
            .await
            .map_err(|_| CompletionTxError::Prelaunch(CiPrelaunchUsageJournalError::Database))?;
            if count != maximum {
                return Err(CompletionTxError::Refused);
            }
        }
    }
    Ok(())
}

enum PreparationRetryGate {
    Proceed,
    NotLive,
}

async fn verify_preparation_retry_permitted_on_conn(
    conn: &mut sqlx::PgConnection,
    claim: &CiJobTokenRequest,
    reserve_handle: &str,
) -> Result<PreparationRetryGate, CompletionTxError> {
    let live = sqlx::query_scalar::<_, String>(
        "SELECT q.job_id::text
         FROM job_queue AS q
         WHERE q.tenant_id = $1 AND q.region = $2 AND q.job_id = $3::uuid AND q.run_id = $4::uuid
           AND q.state = 'leased' AND q.lease_owner = $5 AND q.lease_epoch = $6
           AND q.claim_nonce = $7::uuid
           AND FLOOR(EXTRACT(EPOCH FROM q.claim_started_at))::bigint = $8
           AND FLOOR(EXTRACT(EPOCH FROM q.claim_expires_at))::bigint = $9
           AND q.claim_expires_at > statement_timestamp()
           AND q.completion_receipt IS NULL
         FOR UPDATE",
    )
    .bind(&claim.tenant_id)
    .bind(&claim.region)
    .bind(&claim.job_id)
    .bind(&claim.wf_run_id)
    .bind(&claim.lease_owner)
    .bind(claim.lease_epoch)
    .bind(&claim.claim_nonce)
    .bind(claim.claim_started_at_epoch_secs)
    .bind(claim.claim_expires_at_epoch_secs)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| CompletionTxError::Prelaunch(CiPrelaunchUsageJournalError::Database))?;
    if live.is_none() {
        return Ok(PreparationRetryGate::NotLive);
    }
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "myelin.ci.parent-attempt.v1:{}:{}:{}",
            claim.tenant_id, claim.region, claim.job_id
        ))
        .execute(&mut *conn)
        .await
        .map_err(|_| CompletionTxError::Prelaunch(CiPrelaunchUsageJournalError::Database))?;

    let current = sqlx::query(
        "SELECT budget_revision, max_parent_attempts
         FROM ci_job_parent_attempt
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
           AND wf_run_id = $4::uuid AND ci_run_id = $5::uuid
           AND reserve_handle = $6 AND lease_owner = $7
           AND lease_epoch = $8 AND claim_nonce = $9::uuid
           AND claim_started_at_epoch_secs = $10
           AND claim_expires_at_epoch_secs = $11",
    )
    .bind(&claim.tenant_id)
    .bind(&claim.region)
    .bind(&claim.job_id)
    .bind(&claim.wf_run_id)
    .bind(&claim.ci_run_id)
    .bind(reserve_handle)
    .bind(&claim.lease_owner)
    .bind(claim.lease_epoch)
    .bind(&claim.claim_nonce)
    .bind(claim.claim_started_at_epoch_secs)
    .bind(claim.claim_expires_at_epoch_secs)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| CompletionTxError::Prelaunch(CiPrelaunchUsageJournalError::Database))?
    .ok_or(CompletionTxError::Refused)?;
    let revision: i16 = current
        .try_get("budget_revision")
        .map_err(|_| CompletionTxError::Refused)?;
    let maximum: i64 = current
        .try_get("max_parent_attempts")
        .map_err(|_| CompletionTxError::Refused)?;

    let unresolved = sqlx::query_scalar::<_, i32>(
        "SELECT 1 FROM ci_job_prelaunch_usage
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
           AND lease_epoch = $4 AND claim_nonce = $5::uuid AND status = 'started'
         LIMIT 1",
    )
    .bind(&claim.tenant_id)
    .bind(&claim.region)
    .bind(&claim.job_id)
    .bind(claim.lease_epoch)
    .bind(&claim.claim_nonce)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| CompletionTxError::Prelaunch(CiPrelaunchUsageJournalError::Database))?;
    if unresolved.is_some() {
        return Err(CompletionTxError::Refused);
    }

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*)
         FROM ci_job_parent_attempt
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3::uuid
           AND wf_run_id = $4::uuid AND ci_run_id = $5::uuid
           AND reserve_handle = $6 AND budget_revision = $7 AND max_parent_attempts = $8",
    )
    .bind(&claim.tenant_id)
    .bind(&claim.region)
    .bind(&claim.job_id)
    .bind(&claim.wf_run_id)
    .bind(&claim.ci_run_id)
    .bind(reserve_handle)
    .bind(revision)
    .bind(maximum)
    .fetch_one(&mut *conn)
    .await
    .map_err(|_| CompletionTxError::Prelaunch(CiPrelaunchUsageJournalError::Database))?;
    if count >= maximum {
        return Err(CompletionTxError::Refused);
    }
    Ok(PreparationRetryGate::Proceed)
}

async fn settle_ci_job_surface_on_conn(
    conn: &mut sqlx::PgConnection,
    tenant: &TenantId,
    region: &Region,
    ci_run_id: &str,
    job_id: &str,
    surface: TerminalSurfaceInput<'_>,
) -> Result<(), CompletionTxError> {
    let preparation = matches!(
        surface.disposition,
        Some(CiJobTerminalDisposition::Preparation(_))
    );
    let state = match surface.disposition {
        Some(
            CiJobTerminalDisposition::SkippedBeforeStart
            | CiJobTerminalDisposition::CancelledDuringPreparation
            | CiJobTerminalDisposition::CancelledAfterWorkloadLaunch,
        ) => "cancelled",
        _ if !preparation && surface.report.passed && !surface.report.timed_out => "succeeded",
        _ => "failed",
    };
    let summary = terminal_result_summary(surface.report, surface.disposition, surface.diagnostic)?;
    let state_predicate = if preparation {
        "state IN ('queued','leased') OR (state=$5 AND result_summary=$6)"
    } else {
        "state IN ('queued','leased','running') OR (state=$5 AND result_summary=$6)"
    };
    let query = format!(
        "UPDATE ci_job
         SET state=$5, result_summary=$6
         WHERE tenant_id=$1 AND region=$2 AND run_id=$3::uuid AND job_id=$4::uuid
           AND ({state_predicate})
         RETURNING job_id"
    );
    let updated = sqlx::query_scalar::<_, Uuid>(&query)
        .bind(tenant.as_str())
        .bind(region.as_str())
        .bind(ci_run_id)
        .bind(job_id)
        .bind(state)
        .bind(summary)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|_| {
            CompletionTxError::Accounting(CiJobAccountingError::Db("job surface update"))
        })?;
    if updated.is_some() {
        Ok(())
    } else {
        Err(CompletionTxError::Refused)
    }
}

pub(crate) async fn settle_cancelled_ci_job_surface_on_conn(
    conn: &mut sqlx::PgConnection,
    tenant: &TenantId,
    region: &Region,
    ci_run_id: &str,
    job_id: &str,
    disposition: CiJobTerminalDisposition,
) -> Result<(), CompletionTxError> {
    if !matches!(
        disposition,
        CiJobTerminalDisposition::SkippedBeforeStart
            | CiJobTerminalDisposition::CancelledDuringPreparation
            | CiJobTerminalDisposition::CancelledAfterWorkloadLaunch
    ) {
        return Err(CompletionTxError::Refused);
    }
    settle_ci_job_surface_on_conn(
        conn,
        tenant,
        region,
        ci_run_id,
        job_id,
        TerminalSurfaceInput {
            report: &TerminalReport {
                passed: false,
                timed_out: false,
                usage: ResourceUsage {
                    cpu_seconds: 0,
                    mem_byte_seconds: 0,
                },
                result_refs: Vec::new(),
            },
            disposition: Some(disposition),
            diagnostic: None,
        },
    )
    .await
}

fn terminal_result_summary(
    report: &TerminalReport,
    disposition: Option<CiJobTerminalDisposition>,
    diagnostic: Option<&str>,
) -> Result<serde_json::Value, CompletionTxError> {
    match disposition {
        Some(disposition) => {
            let disposition = CiJobDisposition::from_token(disposition.as_storage_token())
                .ok_or(CompletionTxError::Refused)?;
            let summary = CiJobResultSummary::current(disposition, diagnostic);
            if summary.passed() != report.passed || summary.timed_out() != report.timed_out {
                return Err(CompletionTxError::Refused);
            }
            Ok(summary.to_value())
        }
        None => CiJobResultSummary::legacy(report.passed, report.timed_out)
            .map(|summary| summary.to_value())
            .map_err(|_| CompletionTxError::Refused),
    }
}

pub(crate) async fn close_cancelled_run_if_accounted(
    conn: &mut sqlx::PgConnection,
    accounting: &DurableCiJobAccounting,
    wf_run_id: &str,
) -> Result<(), CompletionTxError> {
    let (manifest, _) = accounting
        .manifest_store
        .load_by_wf_run_on_conn(conn, wf_run_id)
        .await
        .map_err(|_| CompletionTxError::CancelledClosure)?
        .ok_or(CompletionTxError::CancelledClosure)?;
    let run = sqlx::query(
        "SELECT state, cost_settled, finished_at IS NOT NULL AS finished \
         FROM ci_run \
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid \
           AND wf_run_id = $4::uuid FOR UPDATE",
    )
    .bind(accounting.scope.tenant().as_str())
    .bind(accounting.scope.region().as_str())
    .bind(&manifest.ci_run_id)
    .bind(&manifest.wf_run_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| CompletionTxError::CancelledClosure)?
    .ok_or(CompletionTxError::CancelledClosure)?;
    let state: String = run.get("state");
    let settled: bool = run.get("cost_settled");
    let finished: bool = run.get("finished");
    if state != "cancelled" {
        return Ok(());
    }
    if !finished {
        return Err(CompletionTxError::CancelledClosure);
    }

    let rows = sqlx::query(
        "SELECT job_id::text AS job_id, reserve_handle \
         FROM ci_job_accounting \
         WHERE tenant_id = $1 AND region = $2 AND ci_run_id = $3::uuid \
           AND wf_run_id = $4::uuid ORDER BY job_id FOR SHARE",
    )
    .bind(accounting.scope.tenant().as_str())
    .bind(accounting.scope.region().as_str())
    .bind(&manifest.ci_run_id)
    .bind(&manifest.wf_run_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(|_| CompletionTxError::CancelledClosure)?;
    let expected: BTreeMap<&str, &str> = manifest
        .jobs
        .iter()
        .map(|job| (job.job_id.as_str(), job.reserve_handle.as_str()))
        .collect();
    if rows.len() < expected.len() {
        return Ok(());
    }
    if rows.len() != expected.len()
        || rows.iter().any(|row| {
            let job_id: String = row.get("job_id");
            let reserve_handle: String = row.get("reserve_handle");
            expected.get(job_id.as_str()).copied() != Some(reserve_handle.as_str())
        })
    {
        return Err(CompletionTxError::CancelledClosure);
    }
    if settled {
        return Ok(());
    }
    let updated = sqlx::query(
        "UPDATE ci_run SET cost_settled = true \
         WHERE tenant_id = $1 AND region = $2 AND run_id = $3::uuid \
           AND wf_run_id = $4::uuid AND state = 'cancelled' \
           AND cost_settled = false AND finished_at IS NOT NULL",
    )
    .bind(accounting.scope.tenant().as_str())
    .bind(accounting.scope.region().as_str())
    .bind(&manifest.ci_run_id)
    .bind(&manifest.wf_run_id)
    .execute(&mut *conn)
    .await
    .map_err(|_| CompletionTxError::CancelledClosure)?;
    if updated.rows_affected() != 1 {
        return Err(CompletionTxError::CancelledClosure);
    }
    crate::ci_run_supersession::emit_settled_cancelled_checks_on_conn(
        conn,
        accounting.scope.tenant(),
        accounting.scope.region(),
        &manifest,
    )
    .await
    .map_err(|_| CompletionTxError::CancelledClosure)?;
    Ok(())
}

#[cfg(test)]
#[path = "ci_pipeline_driver_tests.rs"]
mod tests;

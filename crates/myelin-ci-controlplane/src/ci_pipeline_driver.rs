use std::collections::BTreeMap;
use std::sync::Arc;

use myelin_ci_sandbox::{
    derive_checkout_authorization_scope, CompletionClaim, CompletionSettlementOwner, JobKind,
    PreparationPhase, PreparationReportClaim, PreparationRetryReport,
    PreparationTerminalDisposition, ResourceUsage, RetryableAttemptFailure,
    RetryableAttemptOutcome, TerminalReport, TerminalReporter,
};
#[cfg(test)]
use myelin_ci_sandbox::{IdemToken, RetryableAttemptCause, TrustTier};
#[cfg(test)]
use myelin_refs::ArtifactRef;
#[cfg(test)]
use myelin_storage::MicroUsd;
use myelin_storage::{
    with_tenant_tx_error, DurableSettleError, MeteredUnit, PgError, RunId as CostRunId,
};
use myelin_tenancy::Region;
use myelin_tenancy::TenantId;
use sqlx::types::Uuid;
use sqlx::Row;

#[cfg(any(test, feature = "test-support"))]
use myelin_flow::{DurableExecutor, FlowExecutor};
use myelin_flow::{
    ExecutorError, PgFlowExecutor, RunId, SignalOutcome, SignalPayload, TypedSignalSpec,
    JOB_DONE_SIGNAL,
};

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
    disposition_receipt_v4, versioned_accounting_receipt, CiJobAccountingError,
    CiJobAccountingRecord, CiJobAccountingWriteVersion, CiJobTerminalDisposition,
};
use crate::job_queue_store::{
    CiJobQueueStore, ClaimConsumeOutcome, ClaimConsumeSpec, PreparationClaimConsumeSpec,
    PreparationRequeueOutcome, PreparationRequeueSpec,
};
#[cfg(test)]
use crate::job_schedule::JobScheduleTerms;
#[cfg(test)]
use crate::job_spec_store::ClaimedDispatchIdentity;
#[cfg(test)]
use crate::job_spec_store::MAX_JOB_TIMEOUT_SECS;
use crate::job_spec_store::{CiJobSpecStore, CiJobSpecStoreError};
#[cfg(test)]
use crate::metering::Meter;
#[cfg(test)]
use crate::scheduler::Lane;

mod accounting;
mod completion;
mod retry;
mod runner;

#[cfg(any(test, feature = "test-support"))]
mod driver;

use accounting::ReporterAccounting;
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
use completion::{
    completion_receipts_v4, preparation_completion_receipts, verify_claimed_identity,
    workload_disposition, CompletionReceiptInput, CompletionReceipts,
    PreparationCompletionReceiptInput,
};
#[cfg(test)]
use driver::validate_driver_tenant;
#[cfg(any(test, feature = "test-support"))]
pub use driver::{fixed_command_spec_builder, CiPipelineDriver, StartRunError};
pub(crate) use retry::decode_retry_attempt_usage;
use retry::{
    aggregate_usage, record_retryable_attempt_on_conn, retry_attempts_for_terminal_on_conn,
};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparationRetryOutcome {
    Requeued,
    NoOp,
}

#[derive(Clone)]
pub struct CiPipelineReporter {
    pg_executor: PgFlowExecutor,
    spec_store: CiJobSpecStore,
    queue_store: CiJobQueueStore,
    rt: tokio::runtime::Handle,
    tenant: TenantId,
    region: String,
    accounting: ReporterAccounting,
    #[cfg(any(test, feature = "test-support"))]
    test_executor: Option<FlowExecutor>,
}

impl CiPipelineReporter {
    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub fn new_accounted(
        pg_executor: PgFlowExecutor,
        spec_store: CiJobSpecStore,
        queue_store: CiJobQueueStore,
        rt: tokio::runtime::Handle,
        accounting: DurableCiJobAccounting,
    ) -> CiPipelineReporter {
        let tenant = accounting.scope.tenant().clone();
        let region = accounting.scope.region().as_str().to_owned();
        CiPipelineReporter {
            pg_executor,
            spec_store,
            queue_store,
            rt,
            tenant,
            region,
            accounting: ReporterAccounting::Durable(Arc::new(accounting)),
            #[cfg(any(test, feature = "test-support"))]
            test_executor: None,
        }
    }

    pub fn report_preparation_terminal(
        &self,
        claim: &CiJobTokenRequest,
        disposition: PreparationTerminalDisposition,
    ) -> Result<SignalOutcome, ExecutorError> {
        self.report_preparation_terminal_with_diagnostic(claim, disposition, None)
    }

    pub fn report_preparation_terminal_with_diagnostic(
        &self,
        claim: &CiJobTokenRequest,
        disposition: PreparationTerminalDisposition,
        diagnostic: Option<&str>,
    ) -> Result<SignalOutcome, ExecutorError> {
        claim.validate().map_err(|error| {
            ExecutorError::InvalidInput(format!(
                "ci.pipeline preparation completion refused: {}",
                error.0
            ))
        })?;
        if claim.tenant_id != self.tenant.0 || claim.region != self.region {
            return Err(ExecutorError::InvalidInput(
                "ci.pipeline preparation completion refused: reporter scope mismatch".into(),
            ));
        }
        let accounting = match &self.accounting {
            ReporterAccounting::Durable(accounting)
                if accounting.receipt_store.write_version() == CiJobAccountingWriteVersion::V4 =>
            {
                accounting.clone()
            }
            ReporterAccounting::Durable(_) => {
                return Err(ExecutorError::InvalidInput(
                    "ci.pipeline preparation completion refused: v4 accounting writer is not \
                     activated"
                        .into(),
                ))
            }
            #[cfg(any(test, feature = "test-support"))]
            ReporterAccounting::TestBypass => {
                return Err(ExecutorError::InvalidInput(
                    "ci.pipeline preparation completion requires durable v4 accounting".into(),
                ))
            }
        };

        let job_uuid = Uuid::parse_str(&claim.job_id)
            .map_err(|_| ExecutorError::InvalidInput("invalid job UUID".into()))?;
        let wf_run_uuid = Uuid::parse_str(&claim.wf_run_id)
            .map_err(|_| ExecutorError::InvalidInput("invalid workflow run UUID".into()))?;
        let ci_run_uuid = Uuid::parse_str(&claim.ci_run_id)
            .map_err(|_| ExecutorError::InvalidInput("invalid CI run UUID".into()))?;
        let nonce_uuid = Uuid::parse_str(&claim.claim_nonce)
            .map_err(|_| ExecutorError::InvalidInput("invalid claim nonce UUID".into()))?;
        let claim = claim.clone();
        let diagnostic = diagnostic.map(str::to_owned);
        let refusal_job = claim.job_id.clone();
        let refusal_owner = claim.lease_owner.clone();
        let refusal_epoch = claim.lease_epoch;
        let tenant = self.tenant.clone();
        let region = self.region.clone();
        let transaction_tenant = tenant.0.clone();
        let transaction_region = region.clone();
        let spec_store = self.spec_store.clone();
        let pg_executor = self.pg_executor.clone();

        let durable = bridge(
            &self.rt,
            with_tenant_tx_error(
                self.queue_store.pool(),
                &transaction_tenant,
                &transaction_region,
                move |conn| {
                    Box::pin(async move {
                        let identity = spec_store
                            .get_dispatch_identity_on_conn(
                                conn,
                                tenant.as_str(),
                                job_uuid,
                                &claim.job_id,
                            )
                            .await
                            .map_err(CompletionTxError::Spec)?;
                        let reserve_handle = identity
                            .as_ref()
                            .map(|identity| identity.reserve_handle.clone())
                            .ok_or(CompletionTxError::Refused)?;
                        let stage = verify_claimed_identity(
                            &tenant,
                            &tenant,
                            &claim.wf_run_id,
                            &claim.job_id,
                            &claim.idem_token,
                            identity,
                        )
                        .map_err(|_| CompletionTxError::Refused)?;
                        let launch = spec_store
                            .get_launch_template_on_conn(conn, tenant.as_str(), &claim.job_id)
                            .await
                            .map_err(CompletionTxError::Spec)?;
                        let checkout_scope = derive_checkout_authorization_scope(
                            JobKind::Ci,
                            &launch.spec.workspace,
                        )
                        .map_err(|_| CompletionTxError::Refused)?;
                        if launch.ci_run_id != claim.ci_run_id
                            || launch.token_authority_handle != claim.token_authority_handle
                            || launch.spec.idem_token.0 != claim.idem_token
                            || launch.spec.meter_to.reserve_id != reserve_handle
                            || (checkout_scope.is_none()
                                && !matches!(
                                    disposition,
                                    PreparationTerminalDisposition::Failed {
                                        phase: PreparationPhase::SecretResolution
                                    }
                                ))
                        {
                            return Err(CompletionTxError::Refused);
                        }

                        let signal = TypedSignalSpec {
                            run: RunId(claim.wf_run_id.clone()),
                            signal_name: JOB_DONE_SIGNAL.to_string(),
                            idem_key: claim.idem_token.clone(),
                            payload: SignalPayload::CiJobDone {
                                stage: stage.clone(),
                                passed: false,
                                result_refs: Vec::new(),
                            },
                            payload_key_ref: None,
                        };
                        let signal_outcome = pg_executor
                            .signal_typed_on_conn(conn, signal)
                            .await
                            .map_err(CompletionTxError::Signal)?;
                        let attempts =
                            retry_attempts_for_terminal_on_conn(conn, &tenant, &region, job_uuid)
                                .await?;
                        let base_usage = aggregate_usage(
                            attempts.as_ref(),
                            ResourceUsage {
                                cpu_seconds: 0,
                                mem_byte_seconds: 0,
                            },
                        )?;
                        let (ci_run_id, usage) = resolve_terminal_usage_on_conn(
                            conn,
                            &accounting,
                            TerminalUsageResolutionInput {
                                tenant: &tenant,
                                wf_run_id: &claim.wf_run_id,
                                job_id: &claim.job_id,
                                reserve_handle: &reserve_handle,
                                base_usage,
                                parent_expectation: if matches!(
                                    disposition,
                                    PreparationTerminalDisposition::Failed {
                                        phase: PreparationPhase::SecretResolution
                                    }
                                ) {
                                    CiPrelaunchParentExpectation::OptionalBeforeLaunch
                                } else {
                                    CiPrelaunchParentExpectation::Required
                                },
                                unresolved_policy: CiPrelaunchUnresolvedPolicy::Refuse,
                            },
                        )
                        .await?;
                        if ci_run_id != claim.ci_run_id {
                            return Err(CompletionTxError::Refused);
                        }
                        verify_preparation_disposition_on_conn(
                            conn,
                            &claim,
                            &reserve_handle,
                            disposition,
                        )
                        .await?;
                        let receipts = preparation_completion_receipts(
                            PreparationCompletionReceiptInput {
                                tenant: &tenant,
                                region: &region,
                                wf_run_id: &claim.wf_run_id,
                                ci_run_id: &claim.ci_run_id,
                                job_id: &claim.job_id,
                                idem_token: &claim.idem_token,
                                stage: &stage,
                                reserve_handle: &reserve_handle,
                                usage,
                                lease_owner: &claim.lease_owner,
                                lease_epoch: claim.lease_epoch,
                                claim_nonce: &claim.claim_nonce,
                                claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
                                claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
                            },
                            disposition,
                        );
                        let consume_spec = PreparationClaimConsumeSpec {
                            tenant_id: tenant.as_str(),
                            region: &region,
                            job_id: job_uuid,
                            wf_run_id: wf_run_uuid,
                            ci_run_id: ci_run_uuid,
                            idem_token: &claim.idem_token,
                            lease_owner: &claim.lease_owner,
                            lease_epoch: claim.lease_epoch,
                            claim_nonce: nonce_uuid,
                            stage: &stage,
                            claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
                            claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
                            reserve_handle: &reserve_handle,
                            completion_receipt: &receipts.current_v4,
                        };
                        let claim_outcome = match disposition {
                            PreparationTerminalDisposition::Failed {
                                phase: PreparationPhase::SecretResolution,
                            } => {
                                CiJobQueueStore::consume_secret_withheld_claim_on_conn(
                                    conn,
                                    consume_spec,
                                )
                                .await?
                            }
                            PreparationTerminalDisposition::AttemptsExhausted => {
                                CiJobQueueStore::consume_preparation_claim_exhausted_on_conn(
                                    conn,
                                    consume_spec,
                                )
                                .await?
                            }
                            PreparationTerminalDisposition::Failed { .. }
                            | PreparationTerminalDisposition::TimedOut { .. } => {
                                CiJobQueueStore::consume_preparation_claim_on_conn(
                                    conn,
                                    consume_spec,
                                )
                                .await?
                            }
                        };
                        if claim_outcome == ClaimConsumeOutcome::Refused {
                            return Err(CompletionTxError::Refused);
                        }
                        let report = TerminalReport {
                            passed: false,
                            timed_out: matches!(
                                disposition,
                                PreparationTerminalDisposition::TimedOut { .. }
                            ),
                            usage,
                            result_refs: Vec::new(),
                        };
                        co_commit_terminal_accounting(
                            conn,
                            &accounting,
                            TerminalAccountingInput {
                                tenant: &tenant,
                                wf_run: &RunId(claim.wf_run_id.clone()),
                                ci_run_id: &claim.ci_run_id,
                                job_id: &claim.job_id,
                                reserve_handle: &reserve_handle,
                                report: &report,
                                receipts: &receipts,
                                disposition: CiJobTerminalDisposition::Preparation(disposition),
                                diagnostic: diagnostic.as_deref(),
                                replay: claim_outcome == ClaimConsumeOutcome::AlreadyConsumed,
                            },
                        )
                        .await?;
                        close_cancelled_run_if_accounted(conn, &accounting, &claim.wf_run_id)
                            .await?;
                        Ok(signal_outcome)
                    })
                },
            ),
        );
        match durable {
            Ok(outcome) => Ok(outcome),
            Err(CompletionTxError::Refused) => Err(ExecutorError::InvalidInput(format!(
                "ci.pipeline preparation completion refused (stale or divergent generation): job \
                 `{}` owner `{}` epoch `{}`",
                refusal_job, refusal_owner, refusal_epoch
            ))),
            Err(CompletionTxError::Spec(error)) => Err(ExecutorError::Storage(format!(
                "durable preparation dispatch read refused: {error}"
            ))),
            Err(CompletionTxError::Signal(error)) => Err(error),
            Err(CompletionTxError::Prelaunch(error)) => Err(ExecutorError::Storage(format!(
                "preparation usage resolution refused: {error}"
            ))),
            Err(CompletionTxError::Usage(error)) => Err(ExecutorError::Storage(format!(
                "preparation usage aggregation refused: {error}"
            ))),
            Err(CompletionTxError::Pricing(error)) => Err(ExecutorError::Storage(format!(
                "preparation accounting pricing refused: {error}"
            ))),
            Err(CompletionTxError::Money(error)) => Err(ExecutorError::Storage(format!(
                "preparation money settlement refused: {error}"
            ))),
            Err(CompletionTxError::Projection(error)) => Err(ExecutorError::Storage(format!(
                "preparation cost projection refused: {error}"
            ))),
            Err(CompletionTxError::Accounting(error)) => Err(ExecutorError::Storage(format!(
                "preparation accounting receipt refused: {error}"
            ))),
            Err(error) => Err(ExecutorError::Storage(format!(
                "preparation completion transaction failed: {error:?}"
            ))),
        }
    }

    pub fn report_preparation_retry(
        &self,
        claim: &CiJobTokenRequest,
    ) -> Result<PreparationRetryOutcome, ExecutorError> {
        claim.validate().map_err(|error| {
            ExecutorError::InvalidInput(format!(
                "ci.pipeline preparation retry refused: {}",
                error.0
            ))
        })?;
        if claim.tenant_id != self.tenant.0 || claim.region != self.region {
            return Err(ExecutorError::InvalidInput(
                "ci.pipeline preparation retry refused: reporter scope mismatch".into(),
            ));
        }
        let job_uuid = Uuid::parse_str(&claim.job_id)
            .map_err(|_| ExecutorError::InvalidInput("invalid job UUID".into()))?;
        let claim = claim.clone();
        let refusal_job = claim.job_id.clone();
        let refusal_owner = claim.lease_owner.clone();
        let refusal_epoch = claim.lease_epoch;
        let tenant = self.tenant.clone();
        let region = self.region.clone();
        let transaction_tenant = tenant.0.clone();
        let transaction_region = region.clone();
        let spec_store = self.spec_store.clone();

        let durable = bridge(
            &self.rt,
            with_tenant_tx_error(
                self.queue_store.pool(),
                &transaction_tenant,
                &transaction_region,
                move |conn| {
                    Box::pin(async move {
                        let identity = spec_store
                            .get_dispatch_identity_on_conn(
                                conn,
                                tenant.as_str(),
                                job_uuid,
                                &claim.job_id,
                            )
                            .await
                            .map_err(CompletionTxError::Spec)?;
                        let reserve_handle = identity
                            .as_ref()
                            .map(|identity| identity.reserve_handle.clone())
                            .ok_or(CompletionTxError::Refused)?;
                        let stage = verify_claimed_identity(
                            &tenant,
                            &tenant,
                            &claim.wf_run_id,
                            &claim.job_id,
                            &claim.idem_token,
                            identity,
                        )
                        .map_err(|_| CompletionTxError::Refused)?;
                        let launch = spec_store
                            .get_launch_template_on_conn(conn, tenant.as_str(), &claim.job_id)
                            .await
                            .map_err(CompletionTxError::Spec)?;
                        if launch.ci_run_id != claim.ci_run_id
                            || launch.token_authority_handle != claim.token_authority_handle
                            || launch.spec.idem_token.0 != claim.idem_token
                            || launch.spec.meter_to.reserve_id != reserve_handle
                            || derive_checkout_authorization_scope(
                                JobKind::Ci,
                                &launch.spec.workspace,
                            )
                            .map_err(|_| CompletionTxError::Refused)?
                            .is_none()
                        {
                            return Err(CompletionTxError::Refused);
                        }
                        if let PreparationRetryGate::NotLive =
                            verify_preparation_retry_permitted_on_conn(
                                conn,
                                &claim,
                                &reserve_handle,
                            )
                            .await?
                        {
                            return Ok(PreparationRequeueOutcome::NoOp);
                        }
                        let outcome = CiJobQueueStore::requeue_preparation_claim_on_conn(
                            conn,
                            PreparationRequeueSpec {
                                tenant_id: tenant.as_str(),
                                region: &region,
                                job_id: &claim.job_id,
                                wf_run_id: &claim.wf_run_id,
                                ci_run_id: &claim.ci_run_id,
                                idem_token: &claim.idem_token,
                                lease_owner: &claim.lease_owner,
                                lease_epoch: claim.lease_epoch,
                                claim_nonce: &claim.claim_nonce,
                                stage: &stage,
                                claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
                                claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
                                reserve_handle: &reserve_handle,
                            },
                        )
                        .await?;
                        Ok(outcome)
                    })
                },
            ),
        );
        match durable {
            Ok(PreparationRequeueOutcome::Requeued) => Ok(PreparationRetryOutcome::Requeued),
            Ok(PreparationRequeueOutcome::NoOp) => Ok(PreparationRetryOutcome::NoOp),
            Err(CompletionTxError::Refused) => Err(ExecutorError::InvalidInput(format!(
                "ci.pipeline preparation retry refused (stale or divergent generation): job `{}` \
                 owner `{}` epoch `{}`",
                refusal_job, refusal_owner, refusal_epoch
            ))),
            Err(CompletionTxError::Spec(error)) => Err(ExecutorError::Storage(format!(
                "durable preparation dispatch read refused: {error}"
            ))),
            Err(CompletionTxError::Prelaunch(error)) => Err(ExecutorError::Storage(format!(
                "preparation retry verification refused: {error}"
            ))),
            Err(error) => Err(ExecutorError::Storage(format!(
                "preparation retry transaction failed: {error:?}"
            ))),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn new(
        pg_executor: PgFlowExecutor,
        spec_store: CiJobSpecStore,
        queue_store: CiJobQueueStore,
        rt: tokio::runtime::Handle,
        tenant: TenantId,
        region: impl Into<String>,
    ) -> CiPipelineReporter {
        CiPipelineReporter {
            pg_executor,
            spec_store,
            queue_store,
            rt,
            tenant,
            region: region.into(),
            accounting: ReporterAccounting::TestBypass,
            #[cfg(any(test, feature = "test-support"))]
            test_executor: None,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    fn with_test_executor(mut self, executor: FlowExecutor) -> Self {
        self.test_executor = Some(executor);
        self
    }
}

pub(crate) fn token_request_from_preparation_report_claim(
    claim: &PreparationReportClaim,
) -> CiJobTokenRequest {
    CiJobTokenRequest {
        tenant_id: claim.tenant_id.clone(),
        region: claim.region.clone(),
        project_id: claim.project_id.clone(),
        wf_run_id: claim.wf_run_id.clone(),
        ci_run_id: claim.ci_run_id.clone(),
        job_id: claim.job_id.clone(),
        token_authority_handle: claim.token_authority_handle.clone(),
        idem_token: claim.idem_token.clone(),
        lease_owner: claim.lease_owner.clone(),
        lease_epoch: claim.lease_epoch,
        claim_nonce: claim.claim_nonce.clone(),
        claim_started_at_epoch_secs: claim.claim_started_at_epoch_secs,
        claim_expires_at_epoch_secs: claim.claim_expires_at_epoch_secs,
    }
}

impl TerminalReporter for CiPipelineReporter {
    fn completion_settlement_owner(&self) -> CompletionSettlementOwner {
        match self.accounting {
            ReporterAccounting::Durable(_) => CompletionSettlementOwner::TerminalReporter,
            #[cfg(any(test, feature = "test-support"))]
            ReporterAccounting::TestBypass => CompletionSettlementOwner::Hook,
        }
    }

    fn report_done(
        &self,
        claim: &CompletionClaim,
        report: &TerminalReport,
    ) -> Result<SignalOutcome, ExecutorError> {
        let CompletionClaim {
            tenant,
            run,
            job_id,
            idem_token,
            lease_owner,
            lease_epoch,
            claim_nonce,
        } = claim;
        let lease_epoch = *lease_epoch;
        if tenant != &self.tenant {
            return Err(ExecutorError::InvalidInput(format!(
                "ci.pipeline job.done refused (unverified claim, fail-closed): {}",
                ClaimRefusal::TenantMismatch {
                    reporter: self.tenant.0.clone(),
                    claimed: tenant.0.clone(),
                }
            )));
        }
        if report.passed && report.timed_out {
            return Err(ExecutorError::InvalidInput(
                "ci.pipeline job.done refused: a timed-out job cannot pass".into(),
            ));
        }

        let job_uuid = Uuid::parse_str(job_id)
            .map_err(|_| ExecutorError::InvalidInput(format!("invalid job_id UUID `{job_id}`")))?;
        let nonce_uuid = Uuid::parse_str(claim_nonce).map_err(|_| {
            ExecutorError::InvalidInput("invalid claim_nonce UUID (completion refused)".into())
        })?;
        let tenant_owned = self.tenant.0.clone();
        let region_owned = self.region.clone();
        let run_owned = run.clone();
        let job_owned = job_id.to_string();
        let idem_owned = idem_token.to_string();
        let owner_owned = lease_owner.to_string();
        let nonce_owned = claim_nonce.to_string();
        let report_owned = report.clone();
        let spec_store = self.spec_store.clone();
        let pg_executor = self.pg_executor.clone();
        let accounting = self.accounting.clone();

        let durable = bridge(
            &self.rt,
            with_tenant_tx_error(
                self.queue_store.pool(),
                &self.tenant.0,
                &self.region,
                move |conn| {
                    Box::pin(async move {
                        let identity = spec_store
                            .get_dispatch_identity_on_conn(
                                conn,
                                &tenant_owned,
                                job_uuid,
                                &job_owned,
                            )
                            .await
                            .map_err(CompletionTxError::Spec)?;
                        let reserve_handle = identity
                            .as_ref()
                            .map(|identity| identity.reserve_handle.clone())
                            .ok_or(CompletionTxError::Refused)?;
                        let stage = verify_claimed_identity(
                            &TenantId(tenant_owned.clone()),
                            &TenantId(tenant_owned.clone()),
                            &run_owned.0,
                            &job_owned,
                            &idem_owned,
                            identity,
                        )
                        .map_err(|_| CompletionTxError::Refused)?;
                        let signal = TypedSignalSpec {
                            run: run_owned.clone(),
                            signal_name: JOB_DONE_SIGNAL.to_string(),
                            idem_key: idem_owned.clone(),
                            payload: SignalPayload::CiJobDone {
                                stage: stage.clone(),
                                passed: report_owned.passed,
                                result_refs: report_owned.result_refs.clone(),
                            },
                            payload_key_ref: None,
                        };
                        let outcome = pg_executor
                            .signal_typed_on_conn(conn, signal.clone())
                            .await
                            .map_err(CompletionTxError::Signal)?;
                        let attempts = retry_attempts_for_terminal_on_conn(
                            conn,
                            &TenantId(tenant_owned.clone()),
                            &region_owned,
                            job_uuid,
                        )
                        .await?;
                        let mut accounted_report = report_owned.clone();
                        accounted_report.usage =
                            aggregate_usage(attempts.as_ref(), report_owned.usage)?;
                        let (ci_run_id, usage) = match &accounting {
                            ReporterAccounting::Durable(accounting) => {
                                resolve_terminal_usage_on_conn(
                                    conn,
                                    accounting,
                                    TerminalUsageResolutionInput {
                                        tenant: &TenantId(tenant_owned.clone()),
                                        wf_run_id: &run_owned.0,
                                        job_id: &job_owned,
                                        reserve_handle: &reserve_handle,
                                        base_usage: accounted_report.usage,
                                        parent_expectation: CiPrelaunchParentExpectation::Required,
                                        unresolved_policy: CiPrelaunchUnresolvedPolicy::Refuse,
                                    },
                                )
                                .await?
                            }
                            #[cfg(any(test, feature = "test-support"))]
                            ReporterAccounting::TestBypass => {
                                (String::new(), accounted_report.usage)
                            }
                        };
                        accounted_report.usage = usage;
                        let disposition = workload_disposition(&accounted_report);
                        let receipts = completion_receipts_v4(
                            CompletionReceiptInput {
                                tenant: &TenantId(tenant_owned.clone()),
                                region: &region_owned,
                                run: &run_owned,
                                job_id: &job_owned,
                                idem_token: &idem_owned,
                                stage: &stage,
                                passed: accounted_report.passed,
                                timed_out: accounted_report.timed_out,
                                usage: accounted_report.usage,
                                result_refs: &accounted_report.result_refs,
                                lease_owner: &owner_owned,
                                lease_epoch,
                                claim_nonce: &nonce_owned,
                            },
                            disposition,
                        );
                        let write_version = match &accounting {
                            ReporterAccounting::Durable(accounting) => {
                                accounting.receipt_store.write_version()
                            }
                            #[cfg(any(test, feature = "test-support"))]
                            ReporterAccounting::TestBypass => CiJobAccountingWriteVersion::V3,
                        };
                        let (completion_receipt, alternate_replay_receipt) = match write_version {
                            CiJobAccountingWriteVersion::V3 => (
                                receipts.legacy_v3.as_str(),
                                Some(receipts.current_v4.as_str()),
                            ),
                            CiJobAccountingWriteVersion::V4 => (
                                receipts.current_v4.as_str(),
                                Some(receipts.legacy_v3.as_str()),
                            ),
                        };
                        let claim = CiJobQueueStore::consume_claim_on_conn(
                            conn,
                            ClaimConsumeSpec {
                                tenant_id: &tenant_owned,
                                job_id: job_uuid,
                                lease_owner: &owner_owned,
                                lease_epoch,
                                claim_nonce: nonce_uuid,
                                stage: &stage,
                                completion_receipt,
                                alternate_replay_receipt,
                            },
                        )
                        .await?;
                        if claim == ClaimConsumeOutcome::Refused {
                            return Err(CompletionTxError::Refused);
                        }
                        match &accounting {
                            ReporterAccounting::Durable(accounting) => {
                                co_commit_terminal_accounting(
                                    conn,
                                    accounting,
                                    TerminalAccountingInput {
                                        tenant: &TenantId(tenant_owned.clone()),
                                        wf_run: &run_owned,
                                        ci_run_id: &ci_run_id,
                                        job_id: &job_owned,
                                        reserve_handle: &reserve_handle,
                                        report: &accounted_report,
                                        receipts: &receipts,
                                        disposition,
                                        diagnostic: None,
                                        replay: claim == ClaimConsumeOutcome::AlreadyConsumed,
                                    },
                                )
                                .await?;
                                close_cancelled_run_if_accounted(conn, accounting, &run_owned.0)
                                    .await?;
                            }
                            #[cfg(any(test, feature = "test-support"))]
                            ReporterAccounting::TestBypass => {}
                        }
                        Ok((outcome, signal))
                    })
                },
            ),
        );
        let (outcome, signal) = match durable {
            Ok(value) => value,
            Err(CompletionTxError::Refused) => {
                return Err(ExecutorError::InvalidInput(format!(
                    "ci.pipeline job.done refused (unverified, stale, or divergent claim): job \
                     `{job_id}` owner `{lease_owner}` epoch `{lease_epoch}`"
                )))
            }
            Err(CompletionTxError::Spec(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "durable claimed-job read refused: {error}"
                )))
            }
            Err(CompletionTxError::Manifest) => {
                return Err(ExecutorError::Storage(
                    "durable CI launch authority could not be verified".into(),
                ))
            }
            Err(CompletionTxError::Pricing(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "terminal CI accounting refused: {error}"
                )))
            }
            Err(CompletionTxError::Money(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "terminal CI money settlement refused: {error}"
                )))
            }
            Err(CompletionTxError::Projection(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "terminal CI cost projection refused: {error}"
                )))
            }
            Err(CompletionTxError::Accounting(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "terminal CI accounting receipt refused: {error}"
                )))
            }
            Err(CompletionTxError::CancelledClosure) => {
                return Err(ExecutorError::Storage(
                    "cancelled CI run accounting closure was refused".into(),
                ))
            }
            Err(CompletionTxError::Signal(error)) => return Err(error),
            Err(CompletionTxError::RetryStore) => {
                return Err(ExecutorError::Storage(
                    "durable retry-attempt store failed".into(),
                ))
            }
            Err(CompletionTxError::RetryCorrupt) => {
                return Err(ExecutorError::Storage(
                    "durable retry-attempt state is corrupt".into(),
                ))
            }
            Err(CompletionTxError::Prelaunch(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "terminal CI prelaunch usage resolution refused: {error}"
                )))
            }
            Err(CompletionTxError::Usage(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "terminal CI usage aggregation refused: {error}"
                )))
            }
            Err(CompletionTxError::Scope(error)) => {
                return Err(ExecutorError::Storage(format!(
                    "atomic completion transaction failed: {error}"
                )))
            }
        };

        #[cfg(any(test, feature = "test-support"))]
        if let Some(executor) = &self.test_executor {
            executor.signal_typed(signal)?;
            executor.runs().wake(&self.tenant, &run.0);
        }
        #[cfg(not(any(test, feature = "test-support")))]
        let _ = signal;
        Ok(outcome)
    }

    fn report_retryable_attempt(
        &self,
        claim: &CompletionClaim,
        failure: &RetryableAttemptFailure,
    ) -> Result<RetryableAttemptOutcome, ExecutorError> {
        if claim.tenant != self.tenant {
            return Err(ExecutorError::InvalidInput(
                "ci.pipeline retryable attempt refused: reporter tenant mismatch".into(),
            ));
        }
        let job_uuid = Uuid::parse_str(&claim.job_id).map_err(|_| {
            ExecutorError::InvalidInput("invalid job_id UUID in retryable attempt".into())
        })?;
        Uuid::parse_str(&claim.claim_nonce).map_err(|_| {
            ExecutorError::InvalidInput("invalid claim_nonce UUID in retryable attempt".into())
        })?;
        let tenant_owned = self.tenant.0.clone();
        let region_owned = self.region.clone();
        let claim_owned = claim.clone();
        let failure_owned = *failure;
        let spec_store = self.spec_store.clone();
        let accounting = self.accounting.clone();
        let durable = bridge(
            &self.rt,
            with_tenant_tx_error(
                self.queue_store.pool(),
                &self.tenant.0,
                &self.region,
                move |conn| {
                    Box::pin(async move {
                        let flow_state: String = sqlx::query_scalar(
                            "SELECT state FROM workflow_run
                             WHERE tenant_id = $1 AND region = $2 AND run_id = $3
                             FOR UPDATE",
                        )
                        .bind(&tenant_owned)
                        .bind(&region_owned)
                        .bind(&claim_owned.run.0)
                        .fetch_optional(&mut *conn)
                        .await
                        .map_err(|_| CompletionTxError::RetryStore)?
                        .ok_or(CompletionTxError::Refused)?;
                        let requeue = matches!(flow_state.as_str(), "running" | "waiting");
                        let cancelled_ci_run = if !requeue {
                            if flow_state != "terminated" {
                                return Err(CompletionTxError::Refused);
                            }
                            let ci_run: Option<(String, String)> = sqlx::query_as(
                                "SELECT run_id::text, state FROM ci_run
                                 WHERE tenant_id = $1 AND region = $2 AND wf_run_id = $3::uuid",
                            )
                            .bind(&tenant_owned)
                            .bind(&region_owned)
                            .bind(&claim_owned.run.0)
                            .fetch_optional(&mut *conn)
                            .await
                            .map_err(|_| CompletionTxError::RetryStore)?;
                            let (ci_run_id, ci_state) = ci_run.ok_or(CompletionTxError::Refused)?;
                            if ci_state != "cancelled" {
                                return Err(CompletionTxError::Refused);
                            }
                            Some(ci_run_id)
                        } else {
                            None
                        };
                        let identity = spec_store
                            .get_dispatch_identity_on_conn(
                                conn,
                                &tenant_owned,
                                job_uuid,
                                &claim_owned.job_id,
                            )
                            .await
                            .map_err(CompletionTxError::Spec)?;
                        let reserve_handle = identity
                            .as_ref()
                            .map(|identity| identity.reserve_handle.clone())
                            .ok_or(CompletionTxError::Refused)?;
                        verify_claimed_identity(
                            &TenantId(tenant_owned.clone()),
                            &claim_owned.tenant,
                            &claim_owned.run.0,
                            &claim_owned.job_id,
                            &claim_owned.idem_token,
                            identity,
                        )
                        .map_err(|_| CompletionTxError::Refused)?;
                        let outcome = record_retryable_attempt_on_conn(
                            conn,
                            &region_owned,
                            &claim_owned,
                            &failure_owned,
                            requeue,
                        )
                        .await?;
                        if !requeue {
                            let attempts = retry_attempts_for_terminal_on_conn(
                                conn,
                                &TenantId(tenant_owned.clone()),
                                &region_owned,
                                job_uuid,
                            )
                            .await?
                            .ok_or(CompletionTxError::RetryCorrupt)?;
                            let report = TerminalReport {
                                passed: false,
                                timed_out: false,
                                usage: ResourceUsage {
                                    cpu_seconds: attempts.cpu_seconds,
                                    mem_byte_seconds: attempts.mem_byte_seconds,
                                },
                                result_refs: Vec::new(),
                            };
                            match &accounting {
                                ReporterAccounting::Durable(accounting) => {
                                    let (ci_run_id, usage) = resolve_terminal_usage_on_conn(
                                        conn,
                                        accounting,
                                        TerminalUsageResolutionInput {
                                            tenant: &TenantId(tenant_owned.clone()),
                                            wf_run_id: &claim_owned.run.0,
                                            job_id: &claim_owned.job_id,
                                            reserve_handle: &reserve_handle,
                                            base_usage: report.usage,
                                            parent_expectation:
                                                CiPrelaunchParentExpectation::Required,
                                            unresolved_policy: CiPrelaunchUnresolvedPolicy::Refuse,
                                        },
                                    )
                                    .await?;
                                    let report = TerminalReport { usage, ..report };
                                    let legacy_v3 = crate::ci_run_supersession::superseded_receipt(
                                        &accounting.scope,
                                        cancelled_ci_run
                                            .as_deref()
                                            .ok_or(CompletionTxError::Refused)?,
                                        &claim_owned.run.0,
                                        &claim_owned.job_id,
                                        &reserve_handle,
                                        report.usage,
                                        false,
                                    );
                                    let disposition =
                                        CiJobTerminalDisposition::CancelledAfterWorkloadLaunch;
                                    let receipts = CompletionReceipts {
                                        current_v4: disposition_receipt_v4(&legacy_v3, disposition),
                                        legacy_v3,
                                    };
                                    co_commit_terminal_accounting(
                                        conn,
                                        accounting,
                                        TerminalAccountingInput {
                                            tenant: &TenantId(tenant_owned.clone()),
                                            wf_run: &claim_owned.run,
                                            ci_run_id: &ci_run_id,
                                            job_id: &claim_owned.job_id,
                                            reserve_handle: &reserve_handle,
                                            report: &report,
                                            receipts: &receipts,
                                            disposition,
                                            diagnostic: None,
                                            replay: outcome == RetryableAttemptOutcome::ExactReplay,
                                        },
                                    )
                                    .await?;
                                    close_cancelled_run_if_accounted(
                                        conn,
                                        accounting,
                                        &claim_owned.run.0,
                                    )
                                    .await?;
                                }
                                #[cfg(any(test, feature = "test-support"))]
                                ReporterAccounting::TestBypass => {
                                    return Err(CompletionTxError::Refused);
                                }
                            }
                        }
                        Ok(outcome)
                    })
                },
            ),
        );
        match durable {
            Ok(outcome) => Ok(outcome),
            Err(CompletionTxError::Refused) => Err(ExecutorError::InvalidInput(format!(
                "ci.pipeline retryable attempt refused (unverified, stale, or divergent claim): \
                 job `{}` owner `{}` epoch `{}`",
                claim.job_id, claim.lease_owner, claim.lease_epoch
            ))),
            Err(CompletionTxError::Spec(error)) => Err(ExecutorError::Storage(format!(
                "durable retryable-attempt dispatch read refused: {error}"
            ))),
            Err(CompletionTxError::Scope(error)) => Err(ExecutorError::Storage(format!(
                "atomic retryable-attempt transaction failed: {error}"
            ))),
            Err(CompletionTxError::RetryStore) => Err(ExecutorError::Storage(
                "durable retry-attempt store failed".into(),
            )),
            Err(CompletionTxError::RetryCorrupt) => Err(ExecutorError::Storage(
                "durable retry-attempt state is corrupt".into(),
            )),
            Err(CompletionTxError::Prelaunch(error)) => Err(ExecutorError::Storage(format!(
                "terminal retry prelaunch usage resolution refused: {error}"
            ))),
            Err(CompletionTxError::Usage(error)) => Err(ExecutorError::Storage(format!(
                "terminal retry usage aggregation refused: {error}"
            ))),
            Err(_) => Err(ExecutorError::Storage(
                "retryable-attempt transaction reached an invalid accounting path".into(),
            )),
        }
    }

    fn report_preparation_terminal(
        &self,
        claim: &PreparationReportClaim,
        disposition: PreparationTerminalDisposition,
        diagnostic: Option<&str>,
    ) -> Result<SignalOutcome, ExecutorError> {
        CiPipelineReporter::report_preparation_terminal_with_diagnostic(
            self,
            &token_request_from_preparation_report_claim(claim),
            disposition,
            diagnostic,
        )
    }

    fn report_preparation_retry(
        &self,
        claim: &PreparationReportClaim,
    ) -> Result<PreparationRetryReport, ExecutorError> {
        match CiPipelineReporter::report_preparation_retry(
            self,
            &token_request_from_preparation_report_claim(claim),
        )? {
            PreparationRetryOutcome::Requeued => Ok(PreparationRetryReport::Requeued),
            PreparationRetryOutcome::NoOp => Ok(PreparationRetryReport::NoOp),
        }
    }
}

#[cfg(test)]
#[path = "ci_pipeline_driver_tests.rs"]
mod tests;

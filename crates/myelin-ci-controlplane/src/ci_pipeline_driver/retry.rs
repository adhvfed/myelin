use myelin_ci_sandbox::{
    CompletionClaim, ResourceUsage, RetryableAttemptCause, RetryableAttemptFailure,
    RetryableAttemptOutcome,
};
use myelin_tenancy::TenantId;
use serde::{Deserialize, Serialize};
use sqlx::types::Uuid;
use sqlx::Row;

use super::{checked_accounting_usage, checked_add_accounting_usage, CompletionTxError};

const RETRY_ATTEMPT_RECORD_VERSION: u8 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RetryAttemptRecord {
    lease_epoch: i64,
    claim_nonce: String,
    lease_owner: String,
    pub(super) cause: String,
    cpu_seconds: u64,
    mem_byte_seconds: u64,
    pub(super) receipt: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RetryAttemptAccrual {
    version: u8,
    attempts: u64,
    pub(super) cpu_seconds: u64,
    pub(super) mem_byte_seconds: u64,
    last: RetryAttemptRecord,
}

fn retry_attempt_receipt(
    claim: &CompletionClaim,
    region: &str,
    failure: &RetryableAttemptFailure,
) -> String {
    let cause = failure.cause.as_storage_token();
    let key = blake3::derive_key(
        "myelin.ci.retry-attempt-receipt.v1",
        claim.claim_nonce.as_bytes(),
    );
    let mut hasher = blake3::Hasher::new_keyed(&key);
    for frame in [
        claim.tenant.0.as_bytes(),
        region.as_bytes(),
        claim.run.0.as_bytes(),
        claim.job_id.as_bytes(),
        claim.idem_token.as_bytes(),
        claim.lease_owner.as_bytes(),
        &claim.lease_epoch.to_be_bytes(),
        claim.claim_nonce.as_bytes(),
        cause.as_bytes(),
        &failure.usage.cpu_seconds.to_be_bytes(),
        &failure.usage.mem_byte_seconds.to_be_bytes(),
    ] {
        hasher.update(&(frame.len() as u64).to_be_bytes());
        hasher.update(frame);
    }
    format!("retry-v1:{}", hasher.finalize().to_hex())
}

pub(super) fn expected_retry_attempt_record(
    claim: &CompletionClaim,
    region: &str,
    failure: &RetryableAttemptFailure,
) -> RetryAttemptRecord {
    RetryAttemptRecord {
        lease_epoch: claim.lease_epoch,
        claim_nonce: claim.claim_nonce.clone(),
        lease_owner: claim.lease_owner.clone(),
        cause: failure.cause.as_storage_token().to_string(),
        cpu_seconds: failure.usage.cpu_seconds,
        mem_byte_seconds: failure.usage.mem_byte_seconds,
        receipt: retry_attempt_receipt(claim, region, failure),
    }
}

pub(super) fn decode_retry_attempts(
    value: serde_json::Value,
) -> Result<Option<RetryAttemptAccrual>, CompletionTxError> {
    if value.as_object().is_some_and(serde_json::Map::is_empty)
        || value.as_array().is_some_and(Vec::is_empty)
    {
        return Ok(None);
    }
    let accrual: RetryAttemptAccrual =
        serde_json::from_value(value).map_err(|_| CompletionTxError::RetryCorrupt)?;
    let valid = accrual.version == RETRY_ATTEMPT_RECORD_VERSION
        && accrual.attempts > 0
        && accrual.last.lease_epoch > 0
        && accrual.attempts <= accrual.last.lease_epoch as u64
        && Uuid::parse_str(&accrual.last.claim_nonce).is_ok()
        && !accrual.last.lease_owner.is_empty()
        && RetryableAttemptCause::from_storage_token(&accrual.last.cause).is_some()
        && accrual.last.receipt.starts_with("retry-v1:")
        && accrual.last.receipt.len() == "retry-v1:".len() + 64;
    if valid {
        Ok(Some(accrual))
    } else {
        Err(CompletionTxError::RetryCorrupt)
    }
}

pub(crate) fn decode_retry_attempt_usage(
    value: serde_json::Value,
) -> Result<Option<ResourceUsage>, ()> {
    decode_retry_attempts(value)
        .map(|attempts| {
            attempts.map(|attempts| ResourceUsage {
                cpu_seconds: attempts.cpu_seconds,
                mem_byte_seconds: attempts.mem_byte_seconds,
            })
        })
        .map_err(|_| ())
}

pub(super) fn aggregate_usage(
    attempts: Option<&RetryAttemptAccrual>,
    current: ResourceUsage,
) -> Result<ResourceUsage, CompletionTxError> {
    let Some(attempts) = attempts else {
        return checked_accounting_usage(current).map_err(CompletionTxError::Usage);
    };
    checked_add_accounting_usage(
        current,
        ResourceUsage {
            cpu_seconds: attempts.cpu_seconds,
            mem_byte_seconds: attempts.mem_byte_seconds,
        },
    )
    .map_err(CompletionTxError::Usage)
}

pub(super) async fn record_retryable_attempt_on_conn(
    conn: &mut sqlx::PgConnection,
    region: &str,
    claim: &CompletionClaim,
    failure: &RetryableAttemptFailure,
    requeue: bool,
) -> Result<RetryableAttemptOutcome, CompletionTxError> {
    let job_id = Uuid::parse_str(&claim.job_id).map_err(|_| CompletionTxError::Refused)?;
    let row = sqlx::query(
        "SELECT run_id::text AS run_id, idem_token, state, lease_owner, lease_epoch,
                claim_nonce::text AS claim_nonce, completion_receipt, retry_attempts
         FROM job_queue
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3
         FOR UPDATE",
    )
    .bind(&claim.tenant.0)
    .bind(region)
    .bind(job_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| CompletionTxError::RetryStore)?
    .ok_or(CompletionTxError::Refused)?;
    let durable_run: String = row.get("run_id");
    let durable_idem: String = row.get("idem_token");
    let state: String = row.get("state");
    let lease_owner: Option<String> = row.get("lease_owner");
    let lease_epoch: i64 = row.get("lease_epoch");
    let claim_nonce: Option<String> = row.get("claim_nonce");
    let completion_receipt: Option<String> = row.get("completion_receipt");
    let retry_attempts: serde_json::Value = row.get("retry_attempts");
    let attempts = decode_retry_attempts(retry_attempts.clone())?;
    let expected = expected_retry_attempt_record(claim, region, failure);

    if let Some(recorded) = attempts
        .as_ref()
        .filter(|attempts| attempts.last.lease_epoch == claim.lease_epoch)
    {
        return if recorded.last == expected {
            Ok(RetryableAttemptOutcome::ExactReplay)
        } else {
            Err(CompletionTxError::Refused)
        };
    }
    let exact_live_generation = durable_run == claim.run.0
        && durable_idem == claim.idem_token
        && state == "running"
        && lease_owner.as_deref() == Some(claim.lease_owner.as_str())
        && lease_epoch == claim.lease_epoch
        && claim_nonce.as_deref() == Some(claim.claim_nonce.as_str())
        && completion_receipt.is_none();
    if !exact_live_generation
        || attempts
            .as_ref()
            .is_some_and(|prior| prior.last.lease_epoch >= claim.lease_epoch)
    {
        return Err(CompletionTxError::Refused);
    }
    let prior_attempts = attempts.as_ref().map_or(0, |prior| prior.attempts);
    let prior_cpu = attempts.as_ref().map_or(0, |prior| prior.cpu_seconds);
    let prior_memory = attempts.as_ref().map_or(0, |prior| prior.mem_byte_seconds);
    let encoded = serde_json::to_value(RetryAttemptAccrual {
        version: RETRY_ATTEMPT_RECORD_VERSION,
        attempts: prior_attempts
            .checked_add(1)
            .ok_or(CompletionTxError::Refused)?,
        cpu_seconds: prior_cpu
            .checked_add(failure.usage.cpu_seconds)
            .ok_or(CompletionTxError::Refused)?,
        mem_byte_seconds: prior_memory
            .checked_add(failure.usage.mem_byte_seconds)
            .ok_or(CompletionTxError::Refused)?,
        last: expected,
    })
    .map_err(|_| CompletionTxError::Refused)?;
    let next_state = if requeue { "queued" } else { "terminal" };
    let updated = sqlx::query(
        "UPDATE job_queue
         SET retry_attempts = $10, state = $11, lease_owner = NULL, lease_expires = NULL,
             claim_nonce = NULL
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3 AND run_id = $4::uuid
           AND idem_token = $5 AND state = 'running' AND lease_owner = $6
           AND lease_epoch = $7 AND claim_nonce = $8::uuid AND completion_receipt IS NULL
           AND retry_attempts = $9
         RETURNING job_id",
    )
    .bind(&claim.tenant.0)
    .bind(region)
    .bind(job_id)
    .bind(&claim.run.0)
    .bind(&claim.idem_token)
    .bind(&claim.lease_owner)
    .bind(claim.lease_epoch)
    .bind(&claim.claim_nonce)
    .bind(retry_attempts)
    .bind(encoded)
    .bind(next_state)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| CompletionTxError::RetryStore)?;
    if requeue && updated.is_some() {
        sqlx::query(
            "UPDATE ci_job SET state = 'queued' \
             WHERE tenant_id = $1 AND job_id = $2 AND state = 'running'",
        )
        .bind(&claim.tenant.0)
        .bind(job_id)
        .execute(&mut *conn)
        .await
        .map_err(|_| CompletionTxError::RetryStore)?;
    }
    if updated.is_some() {
        Ok(if requeue {
            RetryableAttemptOutcome::Requeued
        } else {
            RetryableAttemptOutcome::Cancelled
        })
    } else {
        Err(CompletionTxError::Refused)
    }
}

pub(super) async fn retry_attempts_for_terminal_on_conn(
    conn: &mut sqlx::PgConnection,
    tenant: &TenantId,
    region: &str,
    job_id: Uuid,
) -> Result<Option<RetryAttemptAccrual>, CompletionTxError> {
    let value: serde_json::Value = sqlx::query_scalar(
        "SELECT retry_attempts FROM job_queue
         WHERE tenant_id = $1 AND region = $2 AND job_id = $3
         FOR UPDATE",
    )
    .bind(&tenant.0)
    .bind(region)
    .bind(job_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|_| CompletionTxError::RetryStore)?
    .ok_or(CompletionTxError::Refused)?;
    decode_retry_attempts(value)
}

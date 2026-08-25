mod model;
mod schema;

pub use model::{
    ClaimPrivacyRequestOutcome, CompletePrivacyRequestOutcome, CreatePrivacyRequestOutcome,
    DurablePrivacyRequest, NewPrivacyRequest, PrivacyHolderReceipt, PrivacyRequestCertificate,
    PrivacyRequestKind, PrivacyRequestLease, PrivacyRequestScope, PrivacyRequestState,
    MAX_PRIVACY_HOLDER_RECEIPTS, PRIVACY_REQUEST_DEADLINE_DAYS,
};
pub use schema::{privacy_request_durable_migrations, PRIVACY_REQUEST_MIGRATION};

use chrono::{DateTime, Duration, Utc};
use sqlx::{types::Uuid, PgConnection, Row};

use crate::{
    PgError, ProviderError, SubstrateProvider, ACTIVE_PRINCIPAL_STATUS_JSON,
    HUMAN_PRINCIPAL_KIND_JSON,
};

const MAX_OWNER_BYTES: usize = 255;
const MAX_NONCE_BYTES: usize = 128;
const MAX_WORKER_BYTES: usize = 255;
const MAX_FAILURE_BYTES: usize = 1024;
const MIN_LEASE_SECONDS: i64 = 1;
const MAX_LEASE_SECONDS: i64 = 300;

const RETURNING_REQUEST: &str = "RETURNING request_id, owner_principal_id, kind, scope, state, \
    attempt_count, failure_reason, submitted_at, deadline_at, completed_at, certificate";

#[derive(Clone)]
pub struct DurablePrivacyRequestStore {
    provider: SubstrateProvider,
}

impl DurablePrivacyRequestStore {
    pub fn new(provider: SubstrateProvider) -> Self {
        Self { provider }
    }

    pub async fn create(
        &self,
        tenant: &str,
        proposal: NewPrivacyRequest,
    ) -> Result<CreatePrivacyRequestOutcome, ProviderError> {
        validate_new_request(&proposal)?;
        let deadline_at = proposal
            .submitted_at
            .checked_add_signed(Duration::days(PRIVACY_REQUEST_DEADLINE_DAYS))
            .ok_or_else(|| invalid("privacy request deadline is outside the supported range"))?;
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();

        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    if let Some(existing) = by_nonce(
                        connection,
                        &tenant,
                        &region,
                        &proposal.owner_principal_id,
                        &proposal.client_nonce,
                    )
                    .await?
                    {
                        return Ok(if same_intent(&existing, &proposal) {
                            CreatePrivacyRequestOutcome::Replayed(existing)
                        } else {
                            CreatePrivacyRequestOutcome::Conflict
                        });
                    }

                    if !active_human_owner(
                        connection,
                        &tenant,
                        &region,
                        &proposal.owner_principal_id,
                    )
                    .await?
                    {
                        return Ok(CreatePrivacyRequestOutcome::OwnerUnavailable);
                    }

                    let query = format!(
                        "INSERT INTO privacy_request (\
                           tenant_id, region, request_id, owner_principal_id, client_nonce, kind, \
                           scope, submitted_at, deadline_at\
                         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) \
                         ON CONFLICT DO NOTHING {RETURNING_REQUEST}"
                    );
                    let row = sqlx::query(&query)
                        .bind(&tenant)
                        .bind(&region)
                        .bind(proposal.request_id)
                        .bind(&proposal.owner_principal_id)
                        .bind(&proposal.client_nonce)
                        .bind(proposal.kind.token())
                        .bind(proposal.scope.token())
                        .bind(proposal.submitted_at)
                        .bind(deadline_at)
                        .fetch_optional(&mut *connection)
                        .await
                        .map_err(query_error("create privacy request"))?;
                    if let Some(row) = row {
                        return decode_request(&row).map(CreatePrivacyRequestOutcome::Created);
                    }

                    if let Some(existing) = by_nonce(
                        connection,
                        &tenant,
                        &region,
                        &proposal.owner_principal_id,
                        &proposal.client_nonce,
                    )
                    .await?
                    {
                        return Ok(if same_intent(&existing, &proposal) {
                            CreatePrivacyRequestOutcome::Replayed(existing)
                        } else {
                            CreatePrivacyRequestOutcome::Conflict
                        });
                    }
                    Ok(CreatePrivacyRequestOutcome::Conflict)
                })
            })
            .await
    }

    pub async fn get_owned(
        &self,
        tenant: &str,
        owner_principal_id: &str,
        request_id: Uuid,
    ) -> Result<Option<DurablePrivacyRequest>, ProviderError> {
        bounded("privacy request owner", owner_principal_id, MAX_OWNER_BYTES)?;
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let owner = owner_principal_id.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT request_id, owner_principal_id, kind, scope, state, attempt_count, \
                                failure_reason, submitted_at, deadline_at, completed_at, certificate \
                           FROM privacy_request \
                          WHERE tenant_id = $1 AND region = $2 AND request_id = $3 \
                            AND owner_principal_id = $4",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(request_id)
                    .bind(&owner)
                    .fetch_optional(&mut *connection)
                    .await
                    .map_err(query_error("read owned privacy request"))?;
                    row.as_ref().map(decode_request).transpose()
                })
            })
            .await
    }

    pub async fn claim_owned(
        &self,
        tenant: &str,
        owner_principal_id: &str,
        request_id: Uuid,
        worker: &str,
        now: DateTime<Utc>,
        lease_seconds: i64,
    ) -> Result<ClaimPrivacyRequestOutcome, ProviderError> {
        bounded("privacy request owner", owner_principal_id, MAX_OWNER_BYTES)?;
        bounded("privacy request worker", worker, MAX_WORKER_BYTES)?;
        if !(MIN_LEASE_SECONDS..=MAX_LEASE_SECONDS).contains(&lease_seconds) {
            return Err(invalid(
                "privacy request lease must be between 1 and 300 seconds",
            ));
        }
        let lease_expires = now
            .checked_add_signed(Duration::seconds(lease_seconds))
            .ok_or_else(|| invalid("privacy request lease is outside the supported range"))?;
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let owner = owner_principal_id.to_string();
        let worker = worker.to_string();

        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    let row = sqlx::query(
                        "SELECT request_id, owner_principal_id, kind, scope, state, attempt_count, \
                                failure_reason, submitted_at, deadline_at, completed_at, certificate, \
                                lease_expires \
                           FROM privacy_request \
                          WHERE tenant_id = $1 AND region = $2 AND request_id = $3 \
                            AND owner_principal_id = $4 \
                          FOR UPDATE",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(request_id)
                    .bind(&owner)
                    .fetch_optional(&mut *connection)
                    .await
                    .map_err(query_error("claim owned privacy request"))?;
                    let Some(row) = row else {
                        return Ok(ClaimPrivacyRequestOutcome::NotFound);
                    };
                    let request = decode_request(&row)?;
                    if request.state == PrivacyRequestState::Completed {
                        return Ok(ClaimPrivacyRequestOutcome::Completed(request));
                    }
                    let live_lease = row
                        .try_get::<Option<DateTime<Utc>>, _>("lease_expires")
                        .map_err(decode_error("privacy request lease expiry"))?
                        .is_some_and(|expires| expires > now);
                    if request.state == PrivacyRequestState::Processing && live_lease {
                        return Ok(ClaimPrivacyRequestOutcome::Busy(request));
                    }

                    let query = format!(
                        "UPDATE privacy_request \
                            SET state = 'processing', attempt_count = attempt_count + 1, \
                                lease_owner = $5, lease_epoch = lease_epoch + 1, lease_expires = $6 \
                          WHERE tenant_id = $1 AND region = $2 AND request_id = $3 \
                            AND owner_principal_id = $4 \
                          {RETURNING_REQUEST}, lease_epoch"
                    );
                    let claimed = sqlx::query(&query)
                        .bind(&tenant)
                        .bind(&region)
                        .bind(request_id)
                        .bind(&owner)
                        .bind(&worker)
                        .bind(lease_expires)
                        .fetch_one(&mut *connection)
                        .await
                        .map_err(query_error("acquire privacy request lease"))?;
                    let lease_epoch = claimed
                        .try_get("lease_epoch")
                        .map_err(decode_error("privacy request lease epoch"))?;
                    Ok(ClaimPrivacyRequestOutcome::Claimed(PrivacyRequestLease {
                        request: decode_request(&claimed)?,
                        lease_owner: worker,
                        lease_epoch,
                    }))
                })
            })
            .await
    }

    pub async fn complete(
        &self,
        tenant: &str,
        lease: &PrivacyRequestLease,
        certificate: &PrivacyRequestCertificate,
        completed_at: DateTime<Utc>,
    ) -> Result<CompletePrivacyRequestOutcome, ProviderError> {
        if certificate.request_id != lease.request.request_id.to_string()
            || certificate.kind != lease.request.kind
            || certificate.scope != lease.request.scope
        {
            return Err(invalid(
                "privacy certificate does not match its leased request",
            ));
        }
        let certificate = serde_json::to_value(certificate)
            .map_err(|_| invalid("privacy certificate could not be serialized"))?;
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let request_id = lease.request.request_id;
        let owner = lease.request.owner_principal_id.clone();
        let lease_owner = lease.lease_owner.clone();
        let lease_epoch = lease.lease_epoch;

        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    let query = format!(
                        "UPDATE privacy_request \
                            SET state = 'completed', lease_owner = NULL, lease_expires = NULL, \
                                failure_reason = NULL, certificate = $7, completed_at = $8 \
                          WHERE tenant_id = $1 AND region = $2 AND request_id = $3 \
                            AND owner_principal_id = $4 AND state = 'processing' \
                            AND lease_owner = $5 AND lease_epoch = $6 AND lease_expires > $8 \
                          {RETURNING_REQUEST}"
                    );
                    let row = sqlx::query(&query)
                        .bind(&tenant)
                        .bind(&region)
                        .bind(request_id)
                        .bind(&owner)
                        .bind(&lease_owner)
                        .bind(lease_epoch)
                        .bind(certificate)
                        .bind(completed_at)
                        .fetch_optional(&mut *connection)
                        .await
                        .map_err(query_error("complete privacy request"))?;
                    match row.as_ref() {
                        Some(row) => {
                            decode_request(row).map(CompletePrivacyRequestOutcome::Completed)
                        }
                        None => Ok(CompletePrivacyRequestOutcome::LeaseLost),
                    }
                })
            })
            .await
    }

    pub async fn release_after_failure(
        &self,
        tenant: &str,
        lease: &PrivacyRequestLease,
        reason: &str,
    ) -> Result<bool, ProviderError> {
        bounded("privacy request failure", reason, MAX_FAILURE_BYTES)?;
        let tenant = tenant.to_string();
        let region = self.provider.config().region.clone();
        let request_id = lease.request.request_id;
        let owner = lease.request.owner_principal_id.clone();
        let lease_owner = lease.lease_owner.clone();
        let lease_epoch = lease.lease_epoch;
        let reason = reason.to_string();
        self.provider
            .with_tenant_tx(&tenant.clone(), move |connection| {
                Box::pin(async move {
                    let changed = sqlx::query(
                        "UPDATE privacy_request \
                            SET state = 'pending', lease_owner = NULL, lease_expires = NULL, \
                                failure_reason = $7 \
                          WHERE tenant_id = $1 AND region = $2 AND request_id = $3 \
                            AND owner_principal_id = $4 AND state = 'processing' \
                            AND lease_owner = $5 AND lease_epoch = $6",
                    )
                    .bind(&tenant)
                    .bind(&region)
                    .bind(request_id)
                    .bind(&owner)
                    .bind(&lease_owner)
                    .bind(lease_epoch)
                    .bind(&reason)
                    .execute(&mut *connection)
                    .await
                    .map_err(query_error("release failed privacy request"))?
                    .rows_affected();
                    Ok(changed == 1)
                })
            })
            .await
    }
}

fn validate_new_request(proposal: &NewPrivacyRequest) -> Result<(), ProviderError> {
    bounded(
        "privacy request owner",
        &proposal.owner_principal_id,
        MAX_OWNER_BYTES,
    )?;
    bounded(
        "privacy request nonce",
        &proposal.client_nonce,
        MAX_NONCE_BYTES,
    )
}

fn bounded(label: &str, value: &str, maximum: usize) -> Result<(), ProviderError> {
    if value.is_empty() || value.len() > maximum || value.trim() != value {
        return Err(invalid(format!(
            "{label} must be non-empty, canonical, and at most {maximum} bytes"
        )));
    }
    Ok(())
}

fn invalid(reason: impl Into<String>) -> ProviderError {
    ProviderError::Pg(PgError::Query(reason.into()))
}

fn same_intent(existing: &DurablePrivacyRequest, proposal: &NewPrivacyRequest) -> bool {
    existing.owner_principal_id == proposal.owner_principal_id
        && existing.kind == proposal.kind
        && existing.scope == proposal.scope
}

async fn active_human_owner(
    connection: &mut PgConnection,
    tenant: &str,
    region: &str,
    owner: &str,
) -> Result<bool, PgError> {
    let row = sqlx::query_as::<_, (String, String)>(
        "SELECT kind, status FROM principal \
          WHERE tenant_id = $1 AND region = $2 AND principal_id = $3 \
          FOR UPDATE",
    )
    .bind(tenant)
    .bind(region)
    .bind(owner)
    .fetch_optional(&mut *connection)
    .await
    .map_err(query_error("verify privacy request owner"))?;
    Ok(row.is_some_and(|(kind, status)| {
        kind == HUMAN_PRINCIPAL_KIND_JSON && status == ACTIVE_PRINCIPAL_STATUS_JSON
    }))
}

async fn by_nonce(
    connection: &mut PgConnection,
    tenant: &str,
    region: &str,
    owner: &str,
    nonce: &str,
) -> Result<Option<DurablePrivacyRequest>, PgError> {
    let row = sqlx::query(
        "SELECT request_id, owner_principal_id, kind, scope, state, attempt_count, \
                failure_reason, submitted_at, deadline_at, completed_at, certificate \
           FROM privacy_request \
          WHERE tenant_id = $1 AND region = $2 AND owner_principal_id = $3 \
            AND client_nonce = $4 \
          FOR UPDATE",
    )
    .bind(tenant)
    .bind(region)
    .bind(owner)
    .bind(nonce)
    .fetch_optional(&mut *connection)
    .await
    .map_err(query_error("find privacy request by nonce"))?;
    row.as_ref().map(decode_request).transpose()
}

fn decode_request(row: &sqlx::postgres::PgRow) -> Result<DurablePrivacyRequest, PgError> {
    let kind_token: String = row
        .try_get("kind")
        .map_err(decode_error("privacy request kind"))?;
    let scope_token: String = row
        .try_get("scope")
        .map_err(decode_error("privacy request scope"))?;
    let state_token: String = row
        .try_get("state")
        .map_err(decode_error("privacy request state"))?;
    let attempts: i32 = row
        .try_get("attempt_count")
        .map_err(decode_error("privacy request attempt count"))?;
    let attempt_count = u32::try_from(attempts)
        .map_err(|_| PgError::Query("privacy request attempt count is negative".into()))?;
    let certificate = row
        .try_get::<Option<serde_json::Value>, _>("certificate")
        .map_err(decode_error("privacy request certificate"))?
        .map(|value| {
            serde_json::from_value(value)
                .map_err(|error| PgError::Query(format!("decode privacy certificate: {error}")))
        })
        .transpose()?;

    Ok(DurablePrivacyRequest {
        request_id: row
            .try_get("request_id")
            .map_err(decode_error("privacy request id"))?,
        owner_principal_id: row
            .try_get("owner_principal_id")
            .map_err(decode_error("privacy request owner"))?,
        kind: PrivacyRequestKind::parse(&kind_token).ok_or_else(|| {
            PgError::Query(format!("unknown privacy request kind `{kind_token}`"))
        })?,
        scope: PrivacyRequestScope::parse(&scope_token).ok_or_else(|| {
            PgError::Query(format!("unknown privacy request scope `{scope_token}`"))
        })?,
        state: PrivacyRequestState::parse(&state_token).ok_or_else(|| {
            PgError::Query(format!("unknown privacy request state `{state_token}`"))
        })?,
        attempt_count,
        last_failure: row
            .try_get("failure_reason")
            .map_err(decode_error("privacy request failure"))?,
        submitted_at: row
            .try_get("submitted_at")
            .map_err(decode_error("privacy request submission time"))?,
        deadline_at: row
            .try_get("deadline_at")
            .map_err(decode_error("privacy request deadline"))?,
        completed_at: row
            .try_get("completed_at")
            .map_err(decode_error("privacy request completion time"))?,
        certificate,
    })
}

fn query_error(operation: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| PgError::Query(format!("{operation}: {error}"))
}

fn decode_error(field: &'static str) -> impl FnOnce(sqlx::Error) -> PgError {
    move |error| PgError::Query(format!("decode {field}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn certificate_is_order_independent_and_rejects_incomplete_holders() {
        let request_id = Uuid::from_u128(7);
        let receipt = |holder: &str| PrivacyHolderReceipt {
            holder: holder.into(),
            operation: "erasure".into(),
            content_hash: format!("blake3:{}", "a".repeat(64)),
            records_erased: 3,
            already_erased: false,
            key_unrecoverable: true,
        };
        let ordered = PrivacyRequestCertificate::build(
            request_id,
            PrivacyRequestKind::Erasure,
            PrivacyRequestScope::AgentData,
            vec![receipt("agent_model_replay"), receipt("agent_traces")],
        )
        .unwrap();
        let reversed = PrivacyRequestCertificate::build(
            request_id,
            PrivacyRequestKind::Erasure,
            PrivacyRequestScope::AgentData,
            vec![receipt("agent_traces"), receipt("agent_model_replay")],
        )
        .unwrap();
        assert_eq!(ordered, reversed);

        let mut incomplete = receipt("agent_traces");
        incomplete.key_unrecoverable = false;
        assert!(PrivacyRequestCertificate::build(
            request_id,
            PrivacyRequestKind::Erasure,
            PrivacyRequestScope::AgentData,
            vec![incomplete],
        )
        .is_err());
    }
}

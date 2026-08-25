use std::sync::Arc;

use chrono::Utc;
use myelin_chat::chat_message_holder_receipts;
use myelin_storage::{
    agent_data_holder_receipts, ClaimPrivacyRequestOutcome, CompletePrivacyRequestOutcome,
    CreatePrivacyRequestOutcome, DurablePrivacyRequest, NewPrivacyRequest, PrivacyHolderReceipt,
    PrivacyRequestCertificate, PrivacyRequestKind, PrivacyRequestScope, PrivacyRequestState,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::types::Uuid;

use super::{no_store, PrivacyHttpApi, MAX_PRIVACY_JSON_BYTES};
use crate::catalogue::{Handler, HandlerCtx};
use crate::gateway::GatewayBuilder;
use crate::request::EdgeResponse;
use crate::{EdgeError, Method};

const PRIVACY_REQUEST_WORKER: &str = "edge:privacy-request";
const PRIVACY_REQUEST_LEASE_SECONDS: i64 = 120;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitPrivacyRequestBody {
    kind: PrivacyRequestKind,
    scope: PrivacyRequestScope,
}

struct SubmitPrivacyRequestHandler {
    api: PrivacyHttpApi,
}

impl Handler for SubmitPrivacyRequestHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        let body = parse_submit_body(&ctx.request.body)?;
        let client_nonce = ctx
            .request
            .stable_idempotency_nonce(&ctx.principal.principal_id.0)?;
        let submitted_at = Utc::now();
        let outcome = self.api.drive(self.api.requests.create(
            &ctx.principal.tenant.0,
            NewPrivacyRequest {
                request_id: Uuid::new_v4(),
                owner_principal_id: ctx.principal.principal_id.0.clone(),
                client_nonce,
                kind: body.kind,
                scope: body.scope,
                submitted_at,
            },
        ))?;
        let (request, created) = match outcome {
            CreatePrivacyRequestOutcome::Created(request) => (request, true),
            CreatePrivacyRequestOutcome::Replayed(request) => (request, false),
            CreatePrivacyRequestOutcome::OwnerUnavailable => {
                return Err(EdgeError::Conflict(
                    "the privacy request owner is not available".into(),
                ))
            }
            CreatePrivacyRequestOutcome::Conflict => {
                return Err(EdgeError::Conflict(
                    "that idempotency key is already bound to another privacy request".into(),
                ))
            }
        };
        let request = finish_erasure_request(&self.api, ctx, request)?;
        let status = match (created, request.state) {
            (_, PrivacyRequestState::Pending | PrivacyRequestState::Processing) => 202,
            (true, PrivacyRequestState::Completed) => 201,
            (false, PrivacyRequestState::Completed) => 200,
        };
        Ok(no_store(EdgeResponse::json(
            status,
            &json!({
                "request": privacy_request_json(&ctx.principal.tenant.0, &request),
                "created": created,
                "durable": true,
            }),
        )))
    }
}

struct PrivacyRequestStatusHandler {
    api: PrivacyHttpApi,
}

impl Handler for PrivacyRequestStatusHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        super::require_no_parameters_or_body(ctx, "privacy request status")?;
        let request = owned_request(&self.api, ctx, request_id(ctx)?)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({
                "request": privacy_request_json(&ctx.principal.tenant.0, &request),
                "durable": true,
            }),
        )))
    }
}

struct PrivacyRequestCertificateHandler {
    api: PrivacyHttpApi,
}

impl Handler for PrivacyRequestCertificateHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        super::require_no_parameters_or_body(ctx, "privacy request certificate")?;
        let request = owned_request(&self.api, ctx, request_id(ctx)?)?;
        let certificate = request
            .certificate
            .as_ref()
            .ok_or_else(|| EdgeError::Conflict("the privacy request is not complete yet".into()))?;
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({ "certificate": certificate_json(certificate) }),
        )))
    }
}

pub(super) fn register(builder: GatewayBuilder, api: PrivacyHttpApi) -> GatewayBuilder {
    builder
        .route(
            Method::Post,
            "/v1/privacy/me/requests",
            "privacy.requests.submit",
            Arc::new(SubmitPrivacyRequestHandler { api: api.clone() }),
        )
        .route(
            Method::Get,
            "/v1/privacy/me/requests/{request}",
            "privacy.request.read",
            Arc::new(PrivacyRequestStatusHandler { api: api.clone() }),
        )
        .route(
            Method::Get,
            "/v1/privacy/me/requests/{request}/certificate",
            "privacy.request.read",
            Arc::new(PrivacyRequestCertificateHandler { api }),
        )
}

fn finish_erasure_request(
    api: &PrivacyHttpApi,
    ctx: &HandlerCtx<'_>,
    request: DurablePrivacyRequest,
) -> Result<DurablePrivacyRequest, EdgeError> {
    let tenant = &ctx.principal.tenant.0;
    let owner = &ctx.principal.principal_id.0;
    let claimed = api.drive(api.requests.claim_owned(
        tenant,
        owner,
        request.request_id,
        PRIVACY_REQUEST_WORKER,
        Utc::now(),
        PRIVACY_REQUEST_LEASE_SECONDS,
    ))?;
    let lease = match claimed {
        ClaimPrivacyRequestOutcome::Claimed(lease) => lease,
        ClaimPrivacyRequestOutcome::Busy(request)
        | ClaimPrivacyRequestOutcome::Completed(request) => return Ok(request),
        ClaimPrivacyRequestOutcome::NotFound => {
            return Err(EdgeError::NotFound("privacy request not found".into()))
        }
    };

    let receipts = match erasure_receipts(api, ctx, lease.request()) {
        Ok(receipts) => receipts,
        Err(error) => {
            release_failed_request(api, tenant, &lease)?;
            return Err(error);
        }
    };
    let certificate = match PrivacyRequestCertificate::build(
        lease.request().request_id,
        lease.request().kind,
        lease.request().scope,
        receipts,
    ) {
        Ok(certificate) => certificate,
        Err(error) => {
            release_failed_request(api, tenant, &lease)?;
            return Err(EdgeError::Internal(error.into()));
        }
    };
    let completion =
        match api.drive(
            api.requests
                .complete(tenant, &lease, &certificate, Utc::now()),
        ) {
            Ok(completion) => completion,
            Err(_) => {
                release_failed_request(api, tenant, &lease)?;
                return Err(retryable_holder_failure());
            }
        };
    match completion {
        CompletePrivacyRequestOutcome::Completed(request) => Ok(request),
        CompletePrivacyRequestOutcome::LeaseLost => owned_request(api, ctx, request.request_id),
    }
}

fn erasure_receipts(
    api: &PrivacyHttpApi,
    ctx: &HandlerCtx<'_>,
    request: &DurablePrivacyRequest,
) -> Result<Vec<PrivacyHolderReceipt>, EdgeError> {
    match request.scope {
        PrivacyRequestScope::AgentData => agent_data_erasure_receipts(api, ctx),
        PrivacyRequestScope::ChatMessages => {
            let operation_id = format!("privacy-request:{}", request.request_id);
            let proof = api
                .erase_chat_messages(ctx.principal, &operation_id)
                .map_err(|_| retryable_holder_failure())?;
            chat_message_holder_receipts(&proof).map_err(|error| EdgeError::Internal(error.into()))
        }
    }
}

fn agent_data_erasure_receipts(
    api: &PrivacyHttpApi,
    ctx: &HandlerCtx<'_>,
) -> Result<Vec<PrivacyHolderReceipt>, EdgeError> {
    let tenant = &ctx.principal.tenant.0;
    let owner = &ctx.principal.principal_id.0;
    if api.erase(tenant, owner).is_err() {
        return Err(retryable_holder_failure());
    }
    let proof = match api.drive(api.traces.erasure_proof_for_subject(tenant, owner)) {
        Ok(Some(proof)) => proof,
        Ok(None) => {
            return Err(EdgeError::Internal(
                "agent-data holder did not preserve its erasure proof".into(),
            ));
        }
        Err(_) => return Err(retryable_holder_failure()),
    };
    agent_data_holder_receipts(&proof).map_err(|_| {
        EdgeError::Internal("agent-data holder returned an incomplete erasure proof".into())
    })
}

fn release_failed_request(
    api: &PrivacyHttpApi,
    tenant: &str,
    lease: &myelin_storage::PrivacyRequestLease,
) -> Result<(), EdgeError> {
    let released = api.drive(api.requests.release_after_failure(
        tenant,
        lease,
        "privacy holder temporarily unavailable",
    ))?;
    if !released {
        return Err(EdgeError::Internal(
            "privacy request lease was lost while recording a holder failure".into(),
        ));
    }
    Ok(())
}

fn retryable_holder_failure() -> EdgeError {
    EdgeError::Unavailable(
        "privacy request could not complete; retry with the same Idempotency-Key".into(),
    )
}

fn owned_request(
    api: &PrivacyHttpApi,
    ctx: &HandlerCtx<'_>,
    request_id: Uuid,
) -> Result<DurablePrivacyRequest, EdgeError> {
    api.drive(api.requests.get_owned(
        &ctx.principal.tenant.0,
        &ctx.principal.principal_id.0,
        request_id,
    ))?
    .ok_or_else(|| EdgeError::NotFound("privacy request not found".into()))
}

fn parse_submit_body(bytes: &[u8]) -> Result<SubmitPrivacyRequestBody, EdgeError> {
    if bytes.len() > MAX_PRIVACY_JSON_BYTES {
        return Err(EdgeError::PayloadTooLarge(format!(
            "privacy request body exceeds {MAX_PRIVACY_JSON_BYTES} bytes"
        )));
    }
    if bytes.is_empty() {
        return Err(EdgeError::BadRequest(
            "empty privacy request body (expected JSON)".into(),
        ));
    }
    serde_json::from_slice(bytes)
        .map_err(|error| EdgeError::BadRequest(format!("invalid privacy request body: {error}")))
}

fn require_empty_query(ctx: &HandlerCtx<'_>) -> Result<(), EdgeError> {
    if ctx.request.query.is_empty() {
        Ok(())
    } else {
        Err(EdgeError::BadRequest(
            "privacy request submission accepts no query parameters".into(),
        ))
    }
}

fn request_id(ctx: &HandlerCtx<'_>) -> Result<Uuid, EdgeError> {
    let raw = ctx
        .params
        .get("request")
        .ok_or_else(|| EdgeError::BadRequest("route did not bind a privacy request id".into()))?;
    let parsed = Uuid::parse_str(raw)
        .map_err(|_| EdgeError::BadRequest("privacy request id must be a UUID".into()))?;
    if parsed.to_string() != *raw {
        return Err(EdgeError::BadRequest(
            "privacy request id must be a canonical lowercase UUID".into(),
        ));
    }
    Ok(parsed)
}

fn privacy_request_json(tenant: &str, request: &DurablePrivacyRequest) -> Value {
    json!({
        "id": request.request_id,
        "ref": format!("myelin://{tenant}/privacy/request/{}", request.request_id),
        "kind": request.kind,
        "scope": request.scope,
        "state": request.state,
        "attempt_count": request.attempt_count,
        "submitted_at": request.submitted_at,
        "deadline_at": request.deadline_at,
        "completed_at": request.completed_at,
        "last_attempt_failed": request.last_failure.is_some(),
        "certificate_available": request.certificate.is_some(),
    })
}

fn certificate_json(certificate: &PrivacyRequestCertificate) -> Value {
    json!({
        "request_id": certificate.request_id,
        "kind": certificate.kind,
        "scope": certificate.scope,
        "holders": certificate.holder_receipts,
        "content_hash": certificate.content_hash,
    })
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;
    use myelin_storage::PrivacyHolderReceipt;

    #[test]
    fn the_public_status_omits_owner_and_internal_failure_detail() {
        let request = DurablePrivacyRequest {
            request_id: Uuid::from_u128(19),
            owner_principal_id: "person-private".into(),
            kind: PrivacyRequestKind::Erasure,
            scope: PrivacyRequestScope::AgentData,
            state: PrivacyRequestState::Pending,
            attempt_count: 2,
            last_failure: Some("database hostname and statement".into()),
            submitted_at: Utc.timestamp_opt(1_788_912_000, 0).single().unwrap(),
            deadline_at: Utc.timestamp_opt(1_791_504_000, 0).single().unwrap(),
            completed_at: None,
            certificate: None,
        };

        let body = privacy_request_json("acme", &request);
        assert_eq!(body["last_attempt_failed"], true);
        assert_eq!(body["certificate_available"], false);
        assert!(!body.to_string().contains("person-private"));
        assert!(!body.to_string().contains("hostname"));
    }

    #[test]
    fn the_public_certificate_calls_its_proofs_holders() {
        let certificate = PrivacyRequestCertificate::build(
            Uuid::from_u128(23),
            PrivacyRequestKind::Erasure,
            PrivacyRequestScope::AgentData,
            vec![PrivacyHolderReceipt {
                holder: "agent_traces".into(),
                operation: "erasure".into(),
                content_hash: format!("blake3:{}", "a".repeat(64)),
                records_erased: 3,
                key_unrecoverable: true,
            }],
        )
        .unwrap();

        let body = certificate_json(&certificate);
        assert_eq!(body["holders"][0]["holder"], "agent_traces");
        assert!(body.get("holder_receipts").is_none());
    }
}

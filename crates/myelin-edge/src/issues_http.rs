//! Durable Issues product routes mounted on the production Edge.
//!
//! The handler receives only the gateway's verified principal and delegates every operation to
//! [`PgIssueStore`]. Lists use the materialized effective `issue:view` projection in the store's
//! SQL query; object reads and writes are guarded by a live strong ReBAC decision and then checked
//! again at the durable store boundary. No handler accepts tenant, region, creator, or subject from
//! a path, query, or body.

use crate::catalogue::{page_envelope, Handler, HandlerCtx};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::request::EdgeResponse;
use crate::{Method, StoreBackedIssueAuthorizer};
use myelin_issues::{
    CreateIssue, IssueAuthorizer, IssuePageRequest, IssuePermission, IssueStoreError, PgIssueStore,
    StoredIssue,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::future::Future;
use std::sync::Arc;
use tokio::runtime::{Handle, RuntimeFlavor};

/// Tight per-route JSON bound. The transport's larger 100 MiB ceiling exists for Git packfiles;
/// issue mutations carry at most a 512-byte title plus three short identifiers.
pub const MAX_ISSUE_JSON_BYTES: usize = 4 * 1024;

type ProductionIssueStore = PgIssueStore<StoreBackedIssueAuthorizer>;

/// Sync adapter required by Edge's established object-safe Handler ABI. Production runs on Tokio's
/// multi-thread runtime, where `block_in_place` yields the worker before the async PostgreSQL call.
/// A current-thread runtime is refused as unavailable instead of panicking or applying a mutation.
#[derive(Clone)]
struct DurableIssueHttpApi {
    store: Arc<ProductionIssueStore>,
    runtime: Handle,
}

impl DurableIssueHttpApi {
    fn new(store: Arc<ProductionIssueStore>, runtime: Handle) -> Self {
        Self { store, runtime }
    }

    fn drive<F, T>(&self, future: F) -> Result<T, IssueStoreError>
    where
        F: Future<Output = Result<T, IssueStoreError>>,
    {
        match Handle::try_current() {
            Ok(current) if current.runtime_flavor() == RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| self.runtime.block_on(future))
            }
            Ok(_) => Err(IssueStoreError::AuthorizationUnavailable(
                "Issues HTTP requires the Edge multi-thread runtime".into(),
            )),
            Err(_) => self.runtime.block_on(future),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateIssueBody {
    project_id: String,
    type_id: String,
    prefix: String,
    title: String,
}

fn parse_create_body(ctx: &HandlerCtx<'_>) -> Result<CreateIssue, EdgeError> {
    parse_create_bytes(&ctx.request.body)
}

fn parse_create_bytes(bytes: &[u8]) -> Result<CreateIssue, EdgeError> {
    if bytes.len() > MAX_ISSUE_JSON_BYTES {
        return Err(EdgeError::PayloadTooLarge(format!(
            "Issues request body exceeds {MAX_ISSUE_JSON_BYTES} bytes"
        )));
    }
    if bytes.is_empty() {
        return Err(EdgeError::BadRequest(
            "empty request body (expected JSON)".into(),
        ));
    }
    let body: CreateIssueBody = serde_json::from_slice(bytes)
        .map_err(|error| EdgeError::BadRequest(format!("invalid issue create body: {error}")))?;
    Ok(CreateIssue {
        project_id: body.project_id,
        type_id: body.type_id,
        prefix: body.prefix,
        title: body.title,
    })
}

fn issue_param<'a>(ctx: &'a HandlerCtx<'_>) -> Result<&'a str, EdgeError> {
    let value = ctx
        .params
        .get("issue")
        .map(String::as_str)
        .ok_or_else(|| EdgeError::BadRequest("route did not bind an issue id".into()))?;
    if !is_canonical_uuid(value) {
        return Err(EdgeError::BadRequest(
            "issue id must be a canonical UUID".into(),
        ));
    }
    Ok(value)
}

fn is_canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}

fn map_store_error(error: IssueStoreError) -> EdgeError {
    match error {
        IssueStoreError::BadInput(reason) => EdgeError::BadRequest(reason),
        // An absent object and an object denied by ReBAC deliberately share one envelope.
        IssueStoreError::NotFound => EdgeError::NotFound("issue not found".into()),
        // Projection state, tuple faults, and policy internals are not client-visible.
        IssueStoreError::AuthorizationUnavailable(_) => {
            EdgeError::Unavailable("issue authorization is temporarily unavailable".into())
        }
        IssueStoreError::Storage(reason) | IssueStoreError::Crypto(reason) => {
            EdgeError::Internal(reason)
        }
    }
}

fn issue_json(issue: &StoredIssue) -> Value {
    json!({
        "id": issue.id,
        "key": issue.key,
        "project_id": issue.project_id,
        "state": issue.state,
        "state_category": issue.state_category,
        "title": issue.title,
        "version": issue.version,
        "created_at": issue.created_at,
        "updated_at": issue.updated_at,
    })
}

fn no_store(response: EdgeResponse) -> EdgeResponse {
    response.with_header("Cache-Control", "no-store")
}

struct IssueListHandler {
    api: DurableIssueHttpApi,
}

impl Handler for IssueListHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let request = IssuePageRequest::new(ctx.page.limit as u32, ctx.page.cursor.clone())
            .map_err(map_store_error)?;
        let page = self
            .api
            .drive(self.api.store.list(ctx.principal, request))
            .map_err(map_store_error)?;
        let items: Vec<Value> = page.items.iter().map(issue_json).collect();
        Ok(no_store(EdgeResponse::json(
            200,
            &page_envelope(json!(items), page.next_cursor, page.limit as usize),
        )))
    }
}

struct IssueCreateHandler {
    api: DurableIssueHttpApi,
}

impl Handler for IssueCreateHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let proposal = parse_create_body(ctx)?;
        let receipt = self
            .api
            .drive(self.api.store.create(ctx.principal, proposal))
            .map_err(map_store_error)?;
        // Creation is intentionally asynchronous: the row remains invisible until the durable
        // Identity tuple reconciler activates the staged binding.
        Ok(no_store(
            EdgeResponse::json(
                202,
                &json!({
                    "issue": {
                        "id": receipt.id,
                        "key": receipt.key,
                        "project_id": receipt.project_id,
                    },
                    "authorization": {
                        "status": "pending",
                        "request_event_id": receipt.authorization_request_event_id,
                    }
                }),
            )
            .with_header("Location", format!("/v1/issues/{}", receipt.id)),
        ))
    }
}

struct IssueViewHandler {
    api: DurableIssueHttpApi,
}

impl Handler for IssueViewHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let id = issue_param(ctx)?;
        let issue = self
            .api
            .drive(self.api.store.view(ctx.principal, id))
            .map_err(map_store_error)?;
        Ok(no_store(EdgeResponse::json(200, &issue_json(&issue))))
    }
}

struct IssueCloseHandler {
    api: DurableIssueHttpApi,
}

impl Handler for IssueCloseHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if ctx.request.body.len() > MAX_ISSUE_JSON_BYTES {
            return Err(EdgeError::PayloadTooLarge(format!(
                "Issues request body exceeds {MAX_ISSUE_JSON_BYTES} bytes"
            )));
        }
        // Close currently accepts no mutable client fields. Rejecting a non-empty body prevents a
        // caller from believing a supplied tenant, state, reason, or version was honored.
        if !ctx.request.body.is_empty() {
            let value = ctx.request.json_body()?;
            if value.as_object().is_none_or(|object| !object.is_empty()) {
                return Err(EdgeError::BadRequest(
                    "issue close body must be an empty JSON object".into(),
                ));
            }
        }
        let id = issue_param(ctx)?;
        let issue = self
            .api
            .drive(self.api.store.close(ctx.principal, id))
            .map_err(map_store_error)?;
        Ok(no_store(EdgeResponse::json(200, &issue_json(&issue))))
    }
}

/// Registration-time object guard for every route carrying `{issue}`. This is independent of the
/// identical check inside PgIssueStore, so future handler refactors cannot silently turn an
/// object-addressed route into action-only authorization.
struct IssueObjectGuard {
    authorizer: StoreBackedIssueAuthorizer,
    permission: IssuePermission,
    inner: Arc<dyn Handler>,
}

impl Handler for IssueObjectGuard {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let id = issue_param(ctx)?;
        if !self
            .authorizer
            .may_access(ctx.principal, id, self.permission)
        {
            return Err(EdgeError::NotFound("issue not found".into()));
        }
        self.inner.handle(ctx)
    }
}

fn guarded(
    authorizer: &StoreBackedIssueAuthorizer,
    permission: IssuePermission,
    inner: Arc<dyn Handler>,
) -> Arc<dyn Handler> {
    Arc::new(IssueObjectGuard {
        authorizer: authorizer.clone(),
        permission,
        inner,
    })
}

/// Mount the useful durable Issues floor: keyset list, create receipt, leak-free object view, and
/// idempotent close. Every action is separately capability-mapped in the Edge catalogue.
pub fn register_issues(
    builder: GatewayBuilder,
    store: Arc<ProductionIssueStore>,
    authorizer: StoreBackedIssueAuthorizer,
    runtime: Handle,
) -> GatewayBuilder {
    let api = DurableIssueHttpApi::new(store, runtime);
    builder
        .route(
            Method::Get,
            "/v1/issues",
            "issues.list",
            Arc::new(IssueListHandler { api: api.clone() }),
        )
        .route(
            Method::Post,
            "/v1/issues",
            "issues.create",
            Arc::new(IssueCreateHandler { api: api.clone() }),
        )
        .route(
            Method::Get,
            "/v1/issues/{issue}",
            "issues.view",
            guarded(
                &authorizer,
                IssuePermission::View,
                Arc::new(IssueViewHandler { api: api.clone() }),
            ),
        )
        .route(
            Method::Post,
            "/v1/issues/{issue}/close",
            "issues.close",
            guarded(
                &authorizer,
                IssuePermission::Close,
                Arc::new(IssueCloseHandler { api }),
            ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_ids_are_canonical_and_bounded() {
        assert!(is_canonical_uuid("33333333-3333-3333-3333-333333333333"));
        assert!(!is_canonical_uuid("33333333333333333333333333333333"));
        assert!(!is_canonical_uuid("33333333-3333-3333-3333-33333333333/"));
        assert!(!is_canonical_uuid("*"));
    }

    #[test]
    fn store_failures_map_without_existence_or_internal_leaks() {
        assert_eq!(
            map_store_error(IssueStoreError::NotFound),
            EdgeError::NotFound("issue not found".into())
        );
        let unavailable = map_store_error(IssueStoreError::AuthorizationUnavailable(
            "projection source revision=secret".into(),
        ));
        assert_eq!(
            unavailable.client_message(),
            "issue authorization is temporarily unavailable"
        );
        assert!(!unavailable.envelope().to_string().contains("revision"));
        let internal = map_store_error(IssueStoreError::Storage(
            "postgres secret relation customer@example.test".into(),
        ));
        assert_eq!(internal.client_message(), "internal error");
        assert!(!internal.envelope().to_string().contains("postgres"));
    }

    #[test]
    fn create_body_is_small_strict_and_has_no_scope_selectors() {
        let valid = br#"{
            "project_id":"11111111-1111-1111-1111-111111111111",
            "type_id":"22222222-2222-2222-2222-222222222222",
            "prefix":"ENG",
            "title":"bounded"
        }"#;
        assert!(parse_create_bytes(valid).is_ok());
        assert!(matches!(
            parse_create_bytes(&vec![b'x'; MAX_ISSUE_JSON_BYTES + 1]),
            Err(EdgeError::PayloadTooLarge(_))
        ));
        assert!(parse_create_bytes(b"{}").is_err());
        assert!(parse_create_bytes(
            br#"{"project_id":"11111111-1111-1111-1111-111111111111","type_id":"22222222-2222-2222-2222-222222222222","prefix":"ENG","title":"x","tenant":"other"}"#
        )
        .is_err());
        assert!(parse_create_bytes(
            br#"{"project_id":"11111111-1111-1111-1111-111111111111","project_id":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa","type_id":"22222222-2222-2222-2222-222222222222","prefix":"ENG","title":"x"}"#
        )
        .is_err());
    }
}

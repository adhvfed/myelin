mod import_http;

use crate::catalogue::{page_envelope, Handler, HandlerCtx};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::request::EdgeResponse;
use crate::{Method, StoreBackedIssueAuthorizer};
use myelin_identity_service::{PgProjectStore, Project, ProjectError};
use myelin_issues::{
    api::IssueListState, is_canonical_request_event_id, CreateIssue, IssueAuthorizationStatus,
    IssueAuthorizer, IssueCreationOutcome, IssuePageRequest, IssuePermission, IssueStoreError,
    PgIssueStore, StoredIssue,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::future::Future;
use std::sync::Arc;
use tokio::runtime::{Handle, RuntimeFlavor};

pub const MAX_ISSUE_JSON_BYTES: usize = 4 * 1024;
pub use import_http::{MAX_ISSUE_IMPORT_JSON_BYTES, MAX_ISSUE_IMPORT_RECORDS};
const AUTHORIZATION_STATUS_RETRY_AFTER_MS: u64 = 1_000;

type ProductionIssueStore = PgIssueStore<StoreBackedIssueAuthorizer>;

#[derive(Clone)]
pub struct DurableIssueReadApi {
    store: Arc<ProductionIssueStore>,
    runtime: Handle,
}

impl DurableIssueReadApi {
    pub fn new(store: Arc<ProductionIssueStore>, runtime: Handle) -> Self {
        Self { store, runtime }
    }

    fn drive<F, T>(&self, future: F) -> Result<T, IssueStoreError>
    where
        F: Future<Output = Result<T, IssueStoreError>>,
    {
        drive_on_runtime(
            &self.runtime,
            future,
            IssueStoreError::AuthorizationUnavailable(
                "Issues read API requires the Edge multi-thread runtime".into(),
            ),
        )
    }

    pub fn list(
        &self,
        principal: &myelin_identity::Principal,
        request: IssuePageRequest,
    ) -> Result<Value, EdgeError> {
        let page = self
            .drive(self.store.list(principal, request))
            .map_err(map_store_error)?;
        let items = page
            .items
            .iter()
            .map(|issue| issue_json(&principal.tenant.0, issue))
            .collect::<Vec<_>>();
        Ok(page_envelope(
            json!(items),
            page.next_cursor,
            page.limit as usize,
        ))
    }

    pub fn view(
        &self,
        principal: &myelin_identity::Principal,
        issue_id: &str,
    ) -> Result<Value, EdgeError> {
        let issue = self
            .drive(self.store.view(principal, issue_id))
            .map_err(map_store_error)?;
        Ok(issue_json(&principal.tenant.0, &issue))
    }
}

#[derive(Clone)]
pub struct DurableIssueMutationApi {
    store: Arc<ProductionIssueStore>,
    projects: PgProjectStore,
    authorizer: StoreBackedIssueAuthorizer,
    runtime: Handle,
}

impl DurableIssueMutationApi {
    pub fn new(
        store: Arc<ProductionIssueStore>,
        projects: PgProjectStore,
        authorizer: StoreBackedIssueAuthorizer,
        runtime: Handle,
    ) -> Self {
        Self {
            store,
            projects,
            authorizer,
            runtime,
        }
    }

    pub fn reads(&self) -> DurableIssueReadApi {
        DurableIssueReadApi::new(self.store.clone(), self.runtime.clone())
    }

    pub fn create_issue(
        &self,
        actor: &myelin_identity::Principal,
        authorized_viewer: &myelin_identity::Principal,
        request: IssueCreateRequest,
        caller_key: &str,
    ) -> Result<IssueCreationOutcome, EdgeError> {
        let proposal = resolve_create(self, authorized_viewer, request)?;
        self.drive(
            self.store
                .create_idempotent(actor, authorized_viewer, proposal, caller_key),
        )
        .map_err(map_store_error)
    }

    fn drive<F, T>(&self, future: F) -> Result<T, IssueStoreError>
    where
        F: Future<Output = Result<T, IssueStoreError>>,
    {
        drive_on_runtime(
            &self.runtime,
            future,
            IssueStoreError::AuthorizationUnavailable(
                "Issues HTTP requires the Edge multi-thread runtime".into(),
            ),
        )
    }

    fn drive_project<F, T>(&self, future: F) -> Result<T, ProjectError>
    where
        F: Future<Output = Result<T, ProjectError>>,
    {
        drive_on_runtime(
            &self.runtime,
            future,
            ProjectError::Storage("Issues HTTP requires the Edge multi-thread runtime".into()),
        )
    }
}

fn drive_on_runtime<F, T, E>(runtime: &Handle, future: F, runtime_error: E) -> Result<T, E>
where
    F: Future<Output = Result<T, E>>,
{
    match Handle::try_current() {
        Ok(current) if current.runtime_flavor() == RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(|| runtime.block_on(future))
        }
        Ok(_) => Err(runtime_error),
        Err(_) => runtime.block_on(future),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssueCreateRequest {
    pub project_id: String,
    #[serde(default)]
    pub type_id: Option<String>,
    #[serde(default)]
    pub prefix: Option<String>,
    pub title: String,
}

fn parse_create_body(ctx: &HandlerCtx<'_>) -> Result<IssueCreateRequest, EdgeError> {
    parse_create_bytes(&ctx.request.body)
}

fn parse_create_bytes(bytes: &[u8]) -> Result<IssueCreateRequest, EdgeError> {
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
    serde_json::from_slice(bytes)
        .map_err(|error| EdgeError::BadRequest(format!("invalid issue create body: {error}")))
}

fn resolve_create(
    api: &DurableIssueMutationApi,
    principal: &myelin_identity::Principal,
    request: IssueCreateRequest,
) -> Result<CreateIssue, EdgeError> {
    if !is_canonical_uuid(&request.project_id) {
        return Err(EdgeError::BadRequest(
            "project_id must be a canonical UUID".into(),
        ));
    }
    if !api.authorizer.may_create(principal, &request.project_id) {
        return Err(EdgeError::NotFound("issue not found".into()));
    }

    let metadata = match api.drive_project(api.projects.get(principal, &request.project_id)) {
        Ok(project) => Some(project),
        Err(ProjectError::NotFound) => None,
        Err(ProjectError::BadInput(reason)) => return Err(EdgeError::BadRequest(reason)),
        Err(ProjectError::Conflict(_) | ProjectError::Storage(_)) => {
            return Err(EdgeError::Internal("project metadata lookup failed".into()))
        }
    };

    finish_create_request(request, metadata)
}

fn finish_create_request(
    request: IssueCreateRequest,
    metadata: Option<Project>,
) -> Result<CreateIssue, EdgeError> {
    let (type_id, prefix) = match metadata {
        Some(project) => {
            if request
                .prefix
                .as_deref()
                .is_some_and(|prefix| prefix != project.issue_prefix)
            {
                return Err(EdgeError::BadRequest(
                    "prefix does not match the project's issue prefix".into(),
                ));
            }
            (
                request.type_id.unwrap_or(project.default_issue_type_id),
                project.issue_prefix,
            )
        }
        None => (
            request.type_id.ok_or_else(|| {
                EdgeError::BadRequest(
                    "type_id is required for a project without durable metadata".into(),
                )
            })?,
            request.prefix.ok_or_else(|| {
                EdgeError::BadRequest(
                    "prefix is required for a project without durable metadata".into(),
                )
            })?,
        ),
    };
    Ok(CreateIssue {
        project_id: request.project_id,
        type_id,
        prefix,
        title: request.title,
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

fn authorization_request_param<'a>(ctx: &'a HandlerCtx<'_>) -> Result<&'a str, EdgeError> {
    if !ctx.request.query.is_empty() || !ctx.request.body.is_empty() {
        return Err(EdgeError::BadRequest(
            "authorization status accepts no query parameters or request body".into(),
        ));
    }
    let value = ctx
        .params
        .get("request_event_id")
        .map(String::as_str)
        .ok_or_else(|| {
            EdgeError::BadRequest("route did not bind an authorization request id".into())
        })?;
    if !is_canonical_request_event_id(value) {
        return Err(EdgeError::BadRequest(
            "authorization request id must be a canonical ULID".into(),
        ));
    }
    Ok(value)
}

fn is_canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
        })
}

fn map_store_error(error: IssueStoreError) -> EdgeError {
    match error {
        IssueStoreError::BadInput(reason) => EdgeError::BadRequest(reason),
        IssueStoreError::Conflict(reason) => EdgeError::Conflict(reason),
        IssueStoreError::NotFound => EdgeError::NotFound("issue not found".into()),
        IssueStoreError::AuthorizationUnavailable(_) => {
            EdgeError::Unavailable("issue authorization is temporarily unavailable".into())
        }
        IssueStoreError::Storage(reason) | IssueStoreError::Crypto(reason) => {
            EdgeError::Internal(reason)
        }
    }
}

pub(super) fn canonical_issue_ref(tenant: &str, key: &str) -> String {
    myelin_issues::issue_root_ref(tenant, key).0
}

fn issue_json(tenant: &str, issue: &StoredIssue) -> Value {
    json!({
        "id": issue.id,
        "key": issue.key,
        "ref": canonical_issue_ref(tenant, &issue.key),
        "project_id": issue.project_id,
        "state": issue.state,
        "state_category": issue.state_category,
        "title": issue.title,
        "created_by": issue.created_by_principal,
        "creator_kind": if issue.created_by_principal.starts_with("agent:") {
            "agent"
        } else {
            "human"
        },
        "version": issue.version,
        "created_at": issue.created_at,
        "updated_at": issue.updated_at,
    })
}

fn no_store(response: EdgeResponse) -> EdgeResponse {
    response.with_header("Cache-Control", "no-store")
}

struct IssueListHandler {
    api: DurableIssueReadApi,
}

impl Handler for IssueListHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let request = parse_issue_list_query(&ctx.request.query)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &self.api.list(ctx.principal, request)?,
        )))
    }
}

fn parse_issue_list_query(query: &str) -> Result<IssuePageRequest, EdgeError> {
    let mut state = None;
    let mut key = None;
    let mut limit = None;
    let mut cursor = None;
    if !query.is_empty() {
        for pair in query.split('&') {
            let (name, value) = pair
                .split_once('=')
                .ok_or_else(|| EdgeError::BadRequest("malformed Issues query parameter".into()))?;
            let duplicate = |field: &str| {
                EdgeError::BadRequest(format!("duplicate Issues query parameter `{field}`"))
            };
            match name {
                "state" => {
                    if state.is_some() {
                        return Err(duplicate("state"));
                    }
                    state = Some(IssueListState::parse(value).ok_or_else(|| {
                        EdgeError::BadRequest("state must be open, closed, or all".into())
                    })?);
                }
                "key" => {
                    if key.is_some() {
                        return Err(duplicate("key"));
                    }
                    key = Some(value.to_string());
                }
                "limit" => {
                    if limit.is_some() {
                        return Err(duplicate("limit"));
                    }
                    limit = Some(value.parse::<u32>().map_err(|_| {
                        EdgeError::BadRequest("limit must be an integer between 1 and 100".into())
                    })?);
                }
                "cursor" => {
                    if cursor.is_some() {
                        return Err(duplicate("cursor"));
                    }
                    cursor = Some(value.to_string());
                }
                "" => return Err(EdgeError::BadRequest("empty Issues query parameter".into())),
                other => {
                    return Err(EdgeError::BadRequest(format!(
                        "unknown Issues query parameter `{other}`"
                    )))
                }
            }
        }
    }
    IssuePageRequest::filtered(
        state.unwrap_or(IssueListState::Open),
        key,
        limit.unwrap_or(50),
        cursor,
    )
    .map_err(map_store_error)
}

struct IssueCreateHandler {
    api: DurableIssueMutationApi,
}

impl Handler for IssueCreateHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let client_nonce = ctx
            .request
            .stable_idempotency_nonce(&ctx.principal.principal_id.0)?;
        let outcome = self.api.create_issue(
            ctx.principal,
            ctx.principal,
            parse_create_body(ctx)?,
            &client_nonce,
        )?;
        let receipt = outcome.receipt;
        Ok(no_store(EdgeResponse::json(
            if outcome.created { 202 } else { 200 },
            &json!({
                "issue": {
                    "id": receipt.id,
                    "key": receipt.key,
                    "ref": canonical_issue_ref(&ctx.principal.tenant.0, &receipt.key),
                    "project_id": receipt.project_id,
                },
                "authorization": {
                    "status": "pending",
                    "request_event_id": receipt.authorization_request_event_id,
                },
                "created": outcome.created,
                "durable": true,
            }),
        )))
    }
}

struct IssueAuthorizationStatusHandler {
    api: DurableIssueMutationApi,
}

impl Handler for IssueAuthorizationStatusHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let request_event_id = authorization_request_param(ctx)?;
        let status = self
            .api
            .drive(
                self.api
                    .store
                    .authorization_status(ctx.principal, request_event_id),
            )
            .map_err(map_store_error)?;
        let (status_code, body) = match status {
            IssueAuthorizationStatus::Pending(receipt) => (
                202,
                json!({
                    "status": "pending",
                    "issue": {
                        "id": receipt.id,
                        "key": receipt.key,
                        "ref": canonical_issue_ref(&ctx.principal.tenant.0, &receipt.key),
                        "project_id": receipt.project_id,
                    },
                    "retry_after_ms": AUTHORIZATION_STATUS_RETRY_AFTER_MS,
                }),
            ),
            IssueAuthorizationStatus::Active(issue) => (
                200,
                json!({
                    "status": "active",
                    "issue": issue_json(&ctx.principal.tenant.0, &issue),
                }),
            ),
        };
        Ok(no_store(EdgeResponse::json(status_code, &body)))
    }
}

struct IssueViewHandler {
    api: DurableIssueReadApi,
}

impl Handler for IssueViewHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let id = issue_param(ctx)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &self.api.view(ctx.principal, id)?,
        )))
    }
}

struct IssueCloseHandler {
    api: DurableIssueMutationApi,
}

impl Handler for IssueCloseHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if ctx.request.body.len() > MAX_ISSUE_JSON_BYTES {
            return Err(EdgeError::PayloadTooLarge(format!(
                "Issues request body exceeds {MAX_ISSUE_JSON_BYTES} bytes"
            )));
        }
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
        Ok(no_store(EdgeResponse::json(
            200,
            &issue_json(&ctx.principal.tenant.0, &issue),
        )))
    }
}

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

pub fn register_issues(builder: GatewayBuilder, api: DurableIssueMutationApi) -> GatewayBuilder {
    let authorizer = api.authorizer.clone();
    let reads = api.reads();
    let builder = builder
        .route(
            Method::Get,
            "/v1/issues",
            "issues.list",
            Arc::new(IssueListHandler { api: reads.clone() }),
        )
        .route(
            Method::Post,
            "/v1/issues",
            "issues.create",
            Arc::new(IssueCreateHandler { api: api.clone() }),
        );
    import_http::register(builder, api.clone())
        .route(
            Method::Get,
            "/v1/issues/authorization-requests/{request_event_id}",
            "issues.authorization_status",
            Arc::new(IssueAuthorizationStatusHandler { api: api.clone() }),
        )
        .route(
            Method::Get,
            "/v1/issues/{issue}",
            "issues.view",
            guarded(
                &authorizer,
                IssuePermission::View,
                Arc::new(IssueViewHandler { api: reads }),
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
    fn list_query_is_strict_normalized_and_defaults_to_open() {
        assert_eq!(
            parse_issue_list_query("").unwrap(),
            IssuePageRequest::filtered(IssueListState::Open, None, 50, None).unwrap()
        );
        assert_eq!(
            parse_issue_list_query("state=closed&key=eng-&limit=7").unwrap(),
            IssuePageRequest::filtered(IssueListState::Closed, Some("ENG-".into()), 7, None,)
                .unwrap()
        );
        for query in [
            "state=OPEN",
            "state=all&state=open",
            "key=title search",
            "limit=0",
            "limit=nope",
            "tenant=acme",
            "cursor=not-opaque",
            "state",
        ] {
            assert!(parse_issue_list_query(query).is_err(), "accepted `{query}`");
        }
    }

    #[test]
    fn issue_ids_are_canonical_and_bounded() {
        assert!(is_canonical_uuid("33333333-3333-3333-3333-333333333333"));
        assert!(!is_canonical_uuid("33333333333333333333333333333333"));
        assert!(!is_canonical_uuid("33333333-3333-3333-3333-33333333333/"));
        assert!(!is_canonical_uuid("*"));
        assert!(!is_canonical_uuid("AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA"));
    }

    #[test]
    fn issue_keys_are_re_addressable_without_reconstructing_scope() {
        assert_eq!(
            canonical_issue_ref("acme-eu", "ENG-41"),
            "myelin://acme-eu/issue/issue/ENG-41"
        );
    }

    #[test]
    fn authorization_request_ids_are_canonical_and_retry_is_bounded() {
        assert!(is_canonical_request_event_id("01ARZ3NDEKTSV4RRFFQ69G5FAV"));
        assert!(!is_canonical_request_event_id("01arz3ndektsv4rrffq69g5fav"));
        assert!((100..=10_000).contains(&AUTHORIZATION_STATUS_RETRY_AFTER_MS));
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
        assert!(parse_create_bytes(
            br#"{"project_id":"11111111-1111-1111-1111-111111111111","title":"uses project defaults"}"#
        )
        .is_ok());
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

    #[test]
    fn durable_project_metadata_removes_issue_creation_ceremony_without_losing_overrides() {
        let project = Project {
            id: "11111111-1111-1111-1111-111111111111".into(),
            name: "Developer experience".into(),
            issue_prefix: "DX".into(),
            default_issue_type_id: "22222222-2222-2222-2222-222222222222".into(),
            created_by: "human:founder".into(),
            created_at: "2026-08-09T18:00:00.000000Z".into(),
        };
        let request = parse_create_bytes(
            br#"{"project_id":"11111111-1111-1111-1111-111111111111","title":"Keep the CLI calm"}"#,
        )
        .unwrap();
        assert_eq!(
            finish_create_request(request, Some(project.clone())).unwrap(),
            CreateIssue {
                project_id: project.id.clone(),
                type_id: project.default_issue_type_id.clone(),
                prefix: project.issue_prefix.clone(),
                title: "Keep the CLI calm".into(),
            }
        );

        let explicit_type = parse_create_bytes(
            br#"{"project_id":"11111111-1111-1111-1111-111111111111","type_id":"33333333-3333-3333-3333-333333333333","title":"A specialized issue"}"#,
        )
        .unwrap();
        assert_eq!(
            finish_create_request(explicit_type, Some(project.clone()))
                .unwrap()
                .type_id,
            "33333333-3333-3333-3333-333333333333"
        );

        let wrong_prefix = parse_create_bytes(
            br#"{"project_id":"11111111-1111-1111-1111-111111111111","prefix":"NOPE","title":"Wrong namespace"}"#,
        )
        .unwrap();
        assert!(finish_create_request(wrong_prefix, Some(project)).is_err());

        let legacy = parse_create_bytes(
            br#"{"project_id":"11111111-1111-1111-1111-111111111111","type_id":"22222222-2222-2222-2222-222222222222","prefix":"OLD","title":"Legacy bootstrap"}"#,
        )
        .unwrap();
        assert!(finish_create_request(legacy, None).is_ok());
        let incomplete_legacy = parse_create_bytes(
            br#"{"project_id":"11111111-1111-1111-1111-111111111111","title":"Missing metadata"}"#,
        )
        .unwrap();
        assert!(finish_create_request(incomplete_legacy, None).is_err());
    }

    #[test]
    fn current_thread_runtime_refuses_without_polling_or_panicking() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let current = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let target = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let polled = Arc::new(AtomicBool::new(false));
        let polled_by_future = polled.clone();
        let result = current.block_on(async {
            drive_on_runtime(
                target.handle(),
                async move {
                    polled_by_future.store(true, Ordering::SeqCst);
                    Ok::<_, IssueStoreError>(())
                },
                IssueStoreError::AuthorizationUnavailable("wrong runtime".into()),
            )
        });

        assert!(matches!(
            result,
            Err(IssueStoreError::AuthorizationUnavailable(_))
        ));
        assert!(!polled.load(Ordering::SeqCst));
    }
}

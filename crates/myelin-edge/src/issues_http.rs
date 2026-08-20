mod import_http;

use crate::catalogue::{page_envelope, Handler, HandlerCtx};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::request::EdgeResponse;
use crate::runtime::drive_result_on_runtime;
use crate::{IssueReconciliationWakeup, Method, StoreBackedIssueAuthorizer};
use myelin_identity_service::{PgProjectStore, Project, ProjectError};
use myelin_issues::{
    api::{is_canonical_issue_key, is_canonical_uuid, IssueListState},
    is_canonical_request_event_id, public_issue_actor, CreateIssue, CreateIssueIntent,
    IssueAuthorizationStatus, IssueAuthorizer, IssueCreationOutcome, IssueLifecycleRel,
    IssuePageRequest, IssuePermission, IssueRelationCreationOutcome, IssueStoreError, PgIssueStore,
    StoredIssue, StoredIssueRelation, MAX_RELATIONS_PER_ISSUE,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::runtime::Handle;

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
        F: std::future::Future<Output = Result<T, IssueStoreError>>,
    {
        drive_result_on_runtime(
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

    pub fn view_ref(
        &self,
        principal: &myelin_identity::Principal,
        issue_ref: &str,
    ) -> Result<Value, EdgeError> {
        let key = issue_key_from_ref(principal, issue_ref)?;
        let issue_id = self
            .drive(self.store.resolve_id_by_key(principal, &key))
            .map_err(map_store_error)?;
        self.view(principal, &issue_id)
    }

    pub fn resolve_locator(
        &self,
        principal: &myelin_identity::Principal,
        locator: &str,
    ) -> Result<String, EdgeError> {
        if is_canonical_uuid(locator) {
            return Ok(locator.to_string());
        }
        if !is_canonical_issue_key(locator) {
            return Err(EdgeError::BadRequest(
                "issue locator must be a canonical UUID or PROJECT-123 key".into(),
            ));
        }
        self.drive(self.store.resolve_id_by_key(principal, locator))
            .map_err(map_store_error)
    }

    pub fn list_relations(
        &self,
        principal: &myelin_identity::Principal,
        issue_id: &str,
    ) -> Result<Value, EdgeError> {
        let relations = self
            .drive(self.store.list_relations(principal, issue_id))
            .map_err(map_store_error)?;
        Ok(page_envelope(
            json!(relations
                .iter()
                .map(|relation| relation_json(&principal.tenant.0, relation))
                .collect::<Vec<_>>()),
            None,
            MAX_RELATIONS_PER_ISSUE as usize,
        ))
    }
}

#[derive(Clone)]
pub struct DurableIssueMutationApi {
    store: Arc<ProductionIssueStore>,
    projects: PgProjectStore,
    authorizer: StoreBackedIssueAuthorizer,
    reconciliation: IssueReconciliationWakeup,
    runtime: Handle,
}

impl DurableIssueMutationApi {
    pub fn new(
        store: Arc<ProductionIssueStore>,
        projects: PgProjectStore,
        authorizer: StoreBackedIssueAuthorizer,
        reconciliation: IssueReconciliationWakeup,
        runtime: Handle,
    ) -> Self {
        Self {
            store,
            projects,
            authorizer,
            reconciliation,
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
        let intent = CreateIssueIntent {
            project_id: request.project_id.clone(),
            type_id: request.type_id.clone(),
            prefix: request.prefix.clone(),
            title: request.title.clone(),
        };
        let proposal = resolve_create(self, authorized_viewer, request)?;
        let outcome = self
            .drive(self.store.create_idempotent_from_intent(
                actor,
                authorized_viewer,
                proposal,
                intent,
                caller_key,
            ))
            .map_err(map_store_error)?;
        self.reconciliation.request_sweep();
        Ok(outcome)
    }

    pub(crate) fn request_reconciliation(&self) {
        self.reconciliation.request_sweep();
    }

    pub fn create_relation(
        &self,
        actor: &myelin_identity::Principal,
        issue_id: &str,
        target_ref: &str,
        relation: IssueLifecycleRel,
    ) -> Result<IssueRelationCreationOutcome, EdgeError> {
        self.drive(
            self.store
                .create_relation(actor, issue_id, target_ref, relation),
        )
        .map_err(map_store_error)
    }

    pub fn remove_relation(
        &self,
        actor: &myelin_identity::Principal,
        issue_id: &str,
        relation_id: &str,
    ) -> Result<Option<StoredIssueRelation>, EdgeError> {
        self.drive(self.store.remove_relation(actor, issue_id, relation_id))
            .map_err(map_store_error)
    }

    pub fn close_issue(
        &self,
        actor: &myelin_identity::Principal,
        authorized_viewer: &myelin_identity::Principal,
        issue_id: &str,
    ) -> Result<StoredIssue, EdgeError> {
        self.drive(self.store.close_as(actor, authorized_viewer, issue_id))
            .map_err(map_store_error)
    }

    pub fn close_issue_ref(
        &self,
        actor: &myelin_identity::Principal,
        authorized_viewer: &myelin_identity::Principal,
        issue_ref: &str,
    ) -> Result<StoredIssue, EdgeError> {
        let key = issue_key_from_ref(authorized_viewer, issue_ref)?;
        let issue_id = self
            .drive(self.store.resolve_id_by_key(authorized_viewer, &key))
            .map_err(map_store_error)?;
        self.close_issue(actor, authorized_viewer, &issue_id)
    }

    pub fn may_close_ref(
        &self,
        authorized_viewer: &myelin_identity::Principal,
        issue_ref: &str,
    ) -> Result<bool, EdgeError> {
        let key = issue_key_from_ref(authorized_viewer, issue_ref)?;
        let issue_id = match self.drive(self.store.resolve_id_by_key(authorized_viewer, &key)) {
            Ok(issue_id) => issue_id,
            Err(IssueStoreError::NotFound) => return Ok(false),
            Err(error) => return Err(map_store_error(error)),
        };
        Ok(self
            .authorizer
            .may_access(authorized_viewer, &issue_id, IssuePermission::Close))
    }

    fn drive<F, T>(&self, future: F) -> Result<T, IssueStoreError>
    where
        F: std::future::Future<Output = Result<T, IssueStoreError>>,
    {
        drive_result_on_runtime(
            &self.runtime,
            future,
            IssueStoreError::AuthorizationUnavailable(
                "Issues HTTP requires the Edge multi-thread runtime".into(),
            ),
        )
    }

    fn drive_project<F, T>(&self, future: F) -> Result<T, ProjectError>
    where
        F: std::future::Future<Output = Result<T, ProjectError>>,
    {
        drive_result_on_runtime(
            &self.runtime,
            future,
            ProjectError::Storage("Issues HTTP requires the Edge multi-thread runtime".into()),
        )
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IssueRelationCreateRequest {
    target_ref: String,
    relation: String,
}

impl IssueRelationCreateRequest {
    fn relation(&self) -> Result<IssueLifecycleRel, EdgeError> {
        IssueLifecycleRel::from_token(&self.relation).ok_or_else(|| {
            EdgeError::BadRequest(
                "relation must be parent, blocks, blocked_by, closes, depends_on, or relates"
                    .into(),
            )
        })
    }
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

fn parse_relation_create_body(
    ctx: &HandlerCtx<'_>,
) -> Result<IssueRelationCreateRequest, EdgeError> {
    if !ctx.request.query.is_empty() {
        return Err(EdgeError::BadRequest(
            "issue relation creation accepts no query parameters".into(),
        ));
    }
    parse_relation_create_bytes(&ctx.request.body)
}

fn parse_relation_create_bytes(bytes: &[u8]) -> Result<IssueRelationCreateRequest, EdgeError> {
    if bytes.len() > MAX_ISSUE_JSON_BYTES {
        return Err(EdgeError::PayloadTooLarge(format!(
            "Issues request body exceeds {MAX_ISSUE_JSON_BYTES} bytes"
        )));
    }
    if bytes.is_empty() {
        return Err(EdgeError::BadRequest(
            "empty request body (expected an issue relation)".into(),
        ));
    }
    serde_json::from_slice(bytes).map_err(|error| {
        EdgeError::BadRequest(format!("invalid issue relation create body: {error}"))
    })
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

fn resolved_issue_id<'a>(ctx: &'a HandlerCtx<'_>) -> Result<&'a str, EdgeError> {
    let value =
        ctx.params.get("issue").map(String::as_str).ok_or_else(|| {
            EdgeError::BadRequest("route did not bind a resolved issue id".into())
        })?;
    if !is_canonical_uuid(value) {
        return Err(EdgeError::BadRequest(
            "resolved issue id must be a canonical UUID".into(),
        ));
    }
    Ok(value)
}

fn issue_locator<'a>(ctx: &'a HandlerCtx<'_>) -> Result<&'a str, EdgeError> {
    ctx.params
        .get("issue")
        .map(String::as_str)
        .ok_or_else(|| EdgeError::BadRequest("route did not bind an issue locator".into()))
}

fn issue_key_from_ref(
    principal: &myelin_identity::Principal,
    issue_ref: &str,
) -> Result<String, EdgeError> {
    let parsed = myelin_refs::parse_scoped(issue_ref)
        .map_err(|error| EdgeError::BadRequest(format!("invalid issue_ref: {error}")))?;
    if parsed.tenant != principal.tenant
        || parsed.subsystem != "issue"
        || parsed.type_ != "issue"
        || parsed.sub.is_some()
    {
        return Err(EdgeError::BadRequest(
            "issue_ref must name an issue root in the current tenant".into(),
        ));
    }
    Ok(parsed.id)
}

fn relation_param<'a>(ctx: &'a HandlerCtx<'_>) -> Result<&'a str, EdgeError> {
    let value = ctx
        .params
        .get("relation")
        .map(String::as_str)
        .ok_or_else(|| EdgeError::BadRequest("route did not bind a relation id".into()))?;
    if !is_canonical_uuid(value) {
        return Err(EdgeError::BadRequest(
            "relation id must be a canonical UUID".into(),
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
        "created_by": public_issue_actor(tenant, &issue.created_by_principal),
        "creator_kind": issue.creator_kind.as_str(),
        "version": issue.version,
        "created_at": issue.created_at,
        "updated_at": issue.updated_at,
    })
}

fn relation_json(tenant: &str, relation: &StoredIssueRelation) -> Value {
    json!({
        "id": relation.id,
        "source_ref": relation.source_ref,
        "target_ref": relation.target_ref,
        "relation": relation.relation,
        "created_by": public_issue_actor(tenant, &relation.created_by),
        "creator_kind": relation.creator_kind.as_str(),
        "created_at": relation.created_at,
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
        let id = resolved_issue_id(ctx)?;
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
        let id = resolved_issue_id(ctx)?;
        let issue = self.api.close_issue(ctx.principal, ctx.principal, id)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &issue_json(&ctx.principal.tenant.0, &issue),
        )))
    }
}

struct IssueRelationListHandler {
    api: DurableIssueReadApi,
}

impl Handler for IssueRelationListHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if !ctx.request.query.is_empty() || !ctx.request.body.is_empty() {
            return Err(EdgeError::BadRequest(
                "issue relation listing accepts no query parameters or request body".into(),
            ));
        }
        let issue_id = resolved_issue_id(ctx)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &self.api.list_relations(ctx.principal, issue_id)?,
        )))
    }
}

struct IssueRelationCreateHandler {
    api: DurableIssueMutationApi,
}

impl Handler for IssueRelationCreateHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let issue_id = resolved_issue_id(ctx)?;
        let request = parse_relation_create_body(ctx)?;
        let outcome = self.api.create_relation(
            ctx.principal,
            issue_id,
            &request.target_ref,
            request.relation()?,
        )?;
        Ok(no_store(EdgeResponse::json(
            if outcome.created { 201 } else { 200 },
            &json!({
                "relation": relation_json(&ctx.principal.tenant.0, &outcome.relation),
                "created": outcome.created,
                "durable": true,
            }),
        )))
    }
}

struct IssueRelationRemoveHandler {
    api: DurableIssueMutationApi,
}

impl Handler for IssueRelationRemoveHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        if !ctx.request.query.is_empty() || !ctx.request.body.is_empty() {
            return Err(EdgeError::BadRequest(
                "issue relation removal accepts no query parameters or request body".into(),
            ));
        }
        let issue_id = resolved_issue_id(ctx)?;
        let relation_id = relation_param(ctx)?;
        let removed = self
            .api
            .remove_relation(ctx.principal, issue_id, relation_id)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &match removed {
                Some(relation) => json!({
                    "relation": relation_json(&ctx.principal.tenant.0, &relation),
                    "removed": true,
                    "durable": true,
                }),
                None => json!({
                    "relation_id": relation_id,
                    "removed": false,
                    "durable": true,
                }),
            },
        )))
    }
}

struct IssueObjectGuard {
    authorizer: StoreBackedIssueAuthorizer,
    reads: DurableIssueReadApi,
    permission: IssuePermission,
    inner: Arc<dyn Handler>,
}

impl Handler for IssueObjectGuard {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let id = self
            .reads
            .resolve_locator(ctx.principal, issue_locator(ctx)?)?;
        if !self
            .authorizer
            .may_access(ctx.principal, &id, self.permission)
        {
            return Err(EdgeError::NotFound("issue not found".into()));
        }

        let mut resolved_params = ctx.params.clone();
        resolved_params.insert("issue".into(), id);
        self.inner.handle(&HandlerCtx {
            identity: ctx.identity,
            principal: ctx.principal,
            scope: ctx.scope,
            params: &resolved_params,
            request: ctx.request,
        })
    }
}

fn guarded(
    authorizer: &StoreBackedIssueAuthorizer,
    reads: &DurableIssueReadApi,
    permission: IssuePermission,
    inner: Arc<dyn Handler>,
) -> Arc<dyn Handler> {
    Arc::new(IssueObjectGuard {
        authorizer: authorizer.clone(),
        reads: reads.clone(),
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
            "/v1/issues/{issue}/relations",
            "issues.relations.list",
            guarded(
                &authorizer,
                &reads,
                IssuePermission::View,
                Arc::new(IssueRelationListHandler { api: reads.clone() }),
            ),
        )
        .route(
            Method::Post,
            "/v1/issues/{issue}/relations",
            "issues.relations.create",
            guarded(
                &authorizer,
                &reads,
                IssuePermission::ManageRelations,
                Arc::new(IssueRelationCreateHandler { api: api.clone() }),
            ),
        )
        .route(
            Method::Delete,
            "/v1/issues/{issue}/relations/{relation}",
            "issues.relations.remove",
            guarded(
                &authorizer,
                &reads,
                IssuePermission::ManageRelations,
                Arc::new(IssueRelationRemoveHandler { api: api.clone() }),
            ),
        )
        .route(
            Method::Get,
            "/v1/issues/{issue}",
            "issues.view",
            guarded(
                &authorizer,
                &reads,
                IssuePermission::View,
                Arc::new(IssueViewHandler { api: reads.clone() }),
            ),
        )
        .route(
            Method::Post,
            "/v1/issues/{issue}/close",
            "issues.close",
            guarded(
                &authorizer,
                &reads,
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
    fn agent_issue_refs_are_rooted_in_the_authorizing_humans_tenant() {
        let principal = myelin_identity::Principal::new(
            myelin_tenancy::TenantId::from_token("acme-eu"),
            myelin_tenancy::Region::new("fr-par"),
            myelin_identity::PrincipalId("human:ada".into()),
            myelin_identity::PrincipalKind::Human,
            myelin_identity::DataRole::Controller,
            myelin_identity::PrincipalStatus::Active,
        );
        assert_eq!(
            issue_key_from_ref(&principal, "myelin://acme-eu/issue/issue/ENG-41").unwrap(),
            "ENG-41"
        );
        for refused in [
            "ENG-41",
            "myelin://other/issue/issue/ENG-41",
            "myelin://acme-eu/git/repo/ENG-41",
            "myelin://acme-eu/issue/issue/ENG-41#comment-7",
        ] {
            assert!(
                issue_key_from_ref(&principal, refused).is_err(),
                "accepted `{refused}`"
            );
        }
    }

    #[test]
    fn public_attribution_is_opaque_while_durable_actor_kind_stays_legible() {
        let raw_actor = "opaque-actor-without-a-kind-prefix";
        let issue = StoredIssue {
            id: "33333333-3333-3333-3333-333333333333".into(),
            key: "ENG-41".into(),
            project_id: "11111111-1111-1111-1111-111111111111".into(),
            state: "Todo".into(),
            state_category: "unstarted".into(),
            title: "Keep attribution private".into(),
            created_by_principal: raw_actor.into(),
            creator_kind: myelin_issues::IssueActorKind::Agent,
            version: 1,
            created_at: "2026-08-11T00:00:00Z".into(),
            updated_at: "2026-08-11T00:00:00Z".into(),
        };
        let relation = StoredIssueRelation {
            id: "44444444-4444-4444-4444-444444444444".into(),
            source_ref: "myelin://acme/issue/issue/ENG-41".into(),
            target_ref: "myelin://acme/issue/issue/ENG-42".into(),
            relation: "blocks".into(),
            created_by: raw_actor.into(),
            creator_kind: myelin_issues::IssueActorKind::Agent,
            created_at: "2026-08-11T00:00:00Z".into(),
        };

        let projected_issue = issue_json("acme", &issue);
        let projected_relation = relation_json("acme", &relation);
        let public_actor = projected_issue["created_by"].as_str().unwrap();
        assert_ne!(public_actor, raw_actor);
        assert!(myelin_issues::is_resolvable_pseudonym(public_actor));
        assert_eq!(projected_relation["created_by"], public_actor);
        assert_eq!(projected_issue["creator_kind"], "agent");
        assert_eq!(projected_relation["creator_kind"], "agent");
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
    fn relation_body_names_one_visible_target_and_one_typed_relation() {
        let request = parse_relation_create_bytes(
            br#"{"target_ref":"myelin://acme/issue/issue/ENG-2","relation":"blocks"}"#,
        )
        .unwrap();
        assert_eq!(request.target_ref, "myelin://acme/issue/issue/ENG-2");
        assert_eq!(request.relation().unwrap(), IssueLifecycleRel::Blocks);
        for body in [
            br#"{}"#.as_slice(),
            br#"{"target_ref":"myelin://acme/issue/issue/ENG-2","relation":"follows"}"#,
            br#"{"target_ref":"myelin://acme/issue/issue/ENG-2","relation":"blocks","tenant":"other"}"#,
        ] {
            let parsed = parse_relation_create_bytes(body);
            assert!(
                parsed
                    .and_then(|request| request.relation())
                    .is_err(),
                "an untyped or scope-smuggling relation body was accepted"
            );
        }
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
}

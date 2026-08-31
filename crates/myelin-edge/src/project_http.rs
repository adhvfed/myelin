use crate::catalogue::{page_envelope, Handler, HandlerCtx, Page};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::request::EdgeResponse;
use crate::runtime::drive_result_on_runtime;
use crate::Method;
use myelin_events::Timestamp;
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, ObjectId, Permission, Principal,
    PrincipalId, PrincipalKind, PrincipalStatus, RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{
    project_ref, NewProject, PgProjectStore, PrincipalStore, Project, ProjectError,
    StoreBackedCheck,
};
use myelin_storage::TenantScope;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::runtime::Handle;

const MAX_PROJECT_JSON_BYTES: usize = 4 * 1024;
const DEFAULT_PAGE_LIMIT: u32 = 50;
const MAX_PAGE_LIMIT: u32 = 100;
const MAX_COLLABORATOR_TEXT_BYTES: usize = 255;
const COLLABORATOR_SCAN_MULTIPLIER: usize = 2;
const PROJECT_READER_RELATION: &str = "reader";

#[derive(Clone)]
pub struct DurableProjectReadApi {
    store: PgProjectStore,
    runtime: Handle,
}

impl DurableProjectReadApi {
    pub fn new(store: PgProjectStore, runtime: Handle) -> Self {
        Self { store, runtime }
    }

    pub fn list(
        &self,
        principal: &myelin_identity::Principal,
        limit: u32,
        cursor: Option<String>,
    ) -> Result<Value, EdgeError> {
        if limit == 0 || limit > MAX_PAGE_LIMIT {
            return Err(EdgeError::BadRequest(
                "project limit must be an integer between 1 and 100".into(),
            ));
        }
        let mut visible = drive_result_on_runtime(
            &self.runtime,
            self.store
                .list_visible(principal, cursor.as_deref(), limit + 1),
            ProjectError::Storage("project read requires the Edge multi-thread runtime".into()),
        )
        .map_err(map_project_error)?;
        let has_more = visible.len() > limit as usize;
        visible.truncate(limit as usize);
        let next_cursor = has_more
            .then(|| visible.last().map(|project| project.id.clone()))
            .flatten();
        let items = visible
            .iter()
            .map(|project| project_json(&principal.tenant.0, project))
            .collect::<Vec<_>>();
        Ok(page_envelope(json!(items), next_cursor, limit as usize))
    }
}

#[derive(Clone)]
struct ProjectHttpApi {
    store: PgProjectStore,
    reads: DurableProjectReadApi,
    authz: StoreBackedCheck,
    principals: PrincipalStore,
    runtime: Handle,
}

impl ProjectHttpApi {
    fn drive<F, T>(&self, future: F) -> Result<T, ProjectError>
    where
        F: std::future::Future<Output = Result<T, ProjectError>>,
    {
        drive_result_on_runtime(
            &self.runtime,
            future,
            ProjectError::Storage("project HTTP requires the Edge multi-thread runtime".into()),
        )
    }

    fn allows(&self, principal: &Principal, project_id: &str, permission: &str) -> bool {
        let consistency = Consistency {
            at_least: Zookie(String::new()),
            mode: ConsistencyMode::Strong,
        };
        matches!(
            self.authz.check(
                principal,
                &Permission(permission.into()),
                &project_ref(&principal.tenant.0, project_id),
                &consistency,
                None,
            ),
            Ok(Decision::Allow)
        )
    }

    fn may_view(&self, ctx: &HandlerCtx<'_>, project_id: &str) -> bool {
        self.allows(ctx.principal, project_id, "view")
    }

    fn may_manage_access(&self, ctx: &HandlerCtx<'_>, project_id: &str) -> bool {
        self.allows(ctx.principal, project_id, "manage_access")
    }

    fn access_directory(
        &self,
        ctx: &HandlerCtx<'_>,
        project_id: &str,
        page: Page,
    ) -> Result<Value, EdgeError> {
        let project = self
            .drive(self.store.get(ctx.principal, project_id))
            .map_err(map_project_error)?;
        if !self.may_manage_access(ctx, &project.id) {
            return Err(EdgeError::NotFound("project not found".into()));
        }
        let scan_limit = page.limit.saturating_mul(COLLABORATOR_SCAN_MULTIPLIER);
        let mut rows = self
            .principals
            .principals_after(
                ctx.scope,
                page.cursor.as_deref(),
                u32::try_from(scan_limit + 1).expect("access directory scan is bounded"),
            )
            .map_err(|_| EdgeError::Unavailable("project access is unavailable".into()))?;
        let has_more_rows = rows.len() > scan_limit;
        rows.truncate(scan_limit);
        let mut rows = rows.into_iter().peekable();
        let mut items = Vec::with_capacity(page.limit);
        let mut last_scanned = None;
        for row in rows.by_ref() {
            last_scanned = Some(row.principal_id.0.clone());
            if row.kind != PrincipalKind::Human
                || row.status != PrincipalStatus::Active
                || row.principal_id == ctx.principal.principal_id
                || !valid_collaborator_text(&row.principal_id.0)
            {
                continue;
            }
            let display_name = self
                .principals
                .get_profile(ctx.scope, &row.principal_id)
                .map_err(|_| EdgeError::Unavailable("project access is unavailable".into()))?
                .map(|profile| profile.display_name)
                .filter(|name| valid_collaborator_text(name))
                .unwrap_or_else(|| row.principal_id.0.clone());
            let principal = Principal::new(
                row.tenant,
                row.region,
                row.principal_id.clone(),
                row.kind,
                row.data_role,
                row.status,
            );
            items.push(json!({
                "principal_id": row.principal_id.0,
                "display_name": display_name,
                "has_access": self.allows(&principal, &project.id, "view"),
                "removable": self.allows(&principal, &project.id, "assigned_reader"),
            }));
            if items.len() == page.limit {
                break;
            }
        }
        let next_cursor = (rows.peek().is_some() || has_more_rows)
            .then_some(last_scanned)
            .flatten();
        Ok(page_envelope(json!(items), next_cursor, page.limit))
    }

    fn set_access(
        &self,
        ctx: &HandlerCtx<'_>,
        project_id: &str,
        requested: ProjectAccessBody,
    ) -> Result<Value, EdgeError> {
        let project = self
            .drive(self.store.get(ctx.principal, project_id))
            .map_err(map_project_error)?;
        if !self.may_manage_access(ctx, &project.id) {
            return Err(EdgeError::NotFound("project not found".into()));
        }
        if requested.principal_id == ctx.principal.principal_id.0
            || !valid_collaborator_text(&requested.principal_id)
        {
            return Err(EdgeError::BadRequest(
                "project access requires another valid human principal".into(),
            ));
        }
        let principal_id = PrincipalId(requested.principal_id);
        let target = self
            .principals
            .get_principal(ctx.scope, &principal_id)
            .map_err(|_| EdgeError::Unavailable("project access is unavailable".into()))?
            .filter(|row| row.kind == PrincipalKind::Human && row.status == PrincipalStatus::Active)
            .ok_or_else(|| EdgeError::NotFound("project collaborator not found".into()))?;
        let tuple = RelationTuple {
            object: ObjectId(format!("project:{}", project.id)),
            relation: RelName(PROJECT_READER_RELATION.into()),
            subject: target.principal_id.clone(),
            caveat: None,
        };
        let delta = if requested.access {
            TupleDelta::Add(tuple)
        } else {
            TupleDelta::Remove(tuple)
        };
        self.authz
            .tuples()
            .write_tuples(
                ctx.scope,
                ctx.principal,
                &[delta],
                None,
                None,
                Timestamp(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true)),
            )
            .map_err(|_| EdgeError::Unavailable("project access is unavailable".into()))?;
        let principal = Principal::new(
            target.tenant,
            target.region,
            target.principal_id.clone(),
            target.kind,
            target.data_role,
            target.status,
        );
        Ok(json!({
            "principal_id": target.principal_id.0,
            "requested_access": requested.access,
            "has_access": self.allows(&principal, &project.id, "view"),
            "removable": self.allows(&principal, &project.id, "assigned_reader"),
            "durable": true,
        }))
    }

    fn collaborators(
        &self,
        ctx: &HandlerCtx<'_>,
        project_id: &str,
        page: Page,
    ) -> Result<Value, EdgeError> {
        let project = self
            .drive(self.store.get(ctx.principal, project_id))
            .map_err(map_project_error)?;
        if !self.may_view(ctx, &project.id) {
            return Err(EdgeError::NotFound("project not found".into()));
        }
        let scope = TenantScope::from_verified_token(ctx.principal, ctx.principal.region.clone());
        let object = ObjectId(project_ref(&ctx.principal.tenant.0, &project.id).0);
        let consistency = Consistency {
            at_least: Zookie(String::new()),
            mode: ConsistencyMode::Strong,
        };
        let subjects = self
            .authz
            .list_subjects_in(&scope, &object, &Permission("view".into()), &consistency)
            .map_err(|_| EdgeError::Unavailable("project collaborators are unavailable".into()))?;
        let mut candidates = subjects
            .members
            .into_iter()
            .filter(|principal_id| {
                page.cursor
                    .as_ref()
                    .is_none_or(|cursor| principal_id.0 > *cursor)
            })
            .peekable();
        let mut items = Vec::with_capacity(page.limit);
        let mut last_scanned = None;
        let mut scanned = 0;
        let scan_limit = page.limit.saturating_mul(COLLABORATOR_SCAN_MULTIPLIER);
        while items.len() < page.limit && scanned < scan_limit {
            let Some(principal_id) = candidates.next() else {
                break;
            };
            scanned += 1;
            last_scanned = Some(principal_id.0.clone());
            let Some(principal) = self
                .principals
                .get_principal(&scope, &principal_id)
                .map_err(|_| {
                    EdgeError::Unavailable("project collaborators are unavailable".into())
                })?
            else {
                continue;
            };
            if principal.kind != PrincipalKind::Human
                || principal.status != PrincipalStatus::Active
                || principal.principal_id == ctx.principal.principal_id
                || !valid_collaborator_text(&principal.principal_id.0)
            {
                continue;
            }
            let display_name = self
                .principals
                .get_profile(&scope, &principal.principal_id)
                .map_err(|_| {
                    EdgeError::Unavailable("project collaborators are unavailable".into())
                })?
                .map(|profile| profile.display_name)
                .filter(|name| valid_collaborator_text(name))
                .unwrap_or_else(|| principal.principal_id.0.clone());
            items.push(json!({
                "principal_id": principal.principal_id.0,
                "display_name": display_name,
            }));
        }
        let next_cursor = candidates
            .peek()
            .is_some()
            .then_some(last_scanned)
            .flatten();
        Ok(page_envelope(json!(items), next_cursor, page.limit))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateProjectBody {
    name: String,
    issue_prefix: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectAccessBody {
    principal_id: String,
    access: bool,
}

struct ProjectCreateHandler {
    api: ProjectHttpApi,
}

impl Handler for ProjectCreateHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        let body: CreateProjectBody = parse_body(&ctx.request.body)?;
        let client_nonce = ctx
            .request
            .stable_idempotency_nonce(&ctx.principal.principal_id.0)?;
        let creation = self
            .api
            .drive(self.api.store.create(
                ctx.principal,
                NewProject {
                    name: body.name,
                    issue_prefix: body.issue_prefix,
                    client_nonce,
                },
            ))
            .map_err(map_project_error)?;
        Ok(no_store(EdgeResponse::json(
            if creation.created { 201 } else { 200 },
            &json!({
                "project": project_json(&ctx.principal.tenant.0, &creation.project),
                "created": creation.created,
                "durable": true,
            }),
        )))
    }
}

struct ProjectGetHandler {
    api: ProjectHttpApi,
}

impl Handler for ProjectGetHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        require_empty_body(ctx, "project lookup")?;
        let project_id = project_param(ctx)?;
        let project = self
            .api
            .drive(self.api.store.get(ctx.principal, project_id))
            .map_err(map_project_error)?;
        if !self.api.may_view(ctx, &project.id) {
            return Err(EdgeError::NotFound("project not found".into()));
        }
        Ok(no_store(EdgeResponse::json(
            200,
            &json!({ "project": project_json(&ctx.principal.tenant.0, &project) }),
        )))
    }
}

struct ProjectListHandler {
    api: ProjectHttpApi,
}

struct ProjectCollaboratorListHandler {
    api: ProjectHttpApi,
}

struct ProjectAccessListHandler {
    api: ProjectHttpApi,
}

impl Handler for ProjectAccessListHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_body(ctx, "project access directory")?;
        let page = Page::parse(&ctx.request.query, "project access directory")?;
        let project_id = project_param(ctx)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &self.api.access_directory(ctx, project_id, page)?,
        )))
    }
}

struct ProjectAccessSetHandler {
    api: ProjectHttpApi,
}

impl Handler for ProjectAccessSetHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_query(ctx)?;
        let project_id = project_param(ctx)?;
        let body = parse_access_body(&ctx.request.body)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &self.api.set_access(ctx, project_id, body)?,
        )))
    }
}

impl Handler for ProjectCollaboratorListHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_body(ctx, "project collaborator list")?;
        let page = Page::parse(&ctx.request.query, "project collaborator list")?;
        let project_id = project_param(ctx)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &self.api.collaborators(ctx, project_id, page)?,
        )))
    }
}

impl Handler for ProjectListHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_body(ctx, "project list")?;
        let (limit, cursor) = parse_page_query(&ctx.request.query)?;
        Ok(no_store(EdgeResponse::json(
            200,
            &self.api.reads.list(ctx.principal, limit, cursor)?,
        )))
    }
}

pub fn register_projects(
    builder: GatewayBuilder,
    store: PgProjectStore,
    authz: StoreBackedCheck,
    principals: PrincipalStore,
    runtime: Handle,
) -> GatewayBuilder {
    let reads = DurableProjectReadApi::new(store.clone(), runtime.clone());
    let api = ProjectHttpApi {
        store,
        reads,
        authz,
        principals,
        runtime,
    };
    builder
        .route(
            Method::Get,
            "/v1/projects",
            "identity.projects.list",
            Arc::new(ProjectListHandler { api: api.clone() }),
        )
        .route(
            Method::Post,
            "/v1/projects",
            "identity.project.create",
            Arc::new(ProjectCreateHandler { api: api.clone() }),
        )
        .route(
            Method::Get,
            "/v1/projects/{project}",
            "identity.project.view",
            Arc::new(ProjectGetHandler { api: api.clone() }),
        )
        .route(
            Method::Get,
            "/v1/projects/{project}/collaborators",
            "identity.project.view",
            Arc::new(ProjectCollaboratorListHandler { api: api.clone() }),
        )
        .route(
            Method::Get,
            "/v1/projects/{project}/access",
            "identity.project.access.manage",
            Arc::new(ProjectAccessListHandler { api: api.clone() }),
        )
        .route(
            Method::Put,
            "/v1/projects/{project}/access",
            "identity.project.access.manage",
            Arc::new(ProjectAccessSetHandler { api }),
        )
}

fn project_json(tenant: &str, project: &Project) -> Value {
    json!({
        "id": project.id,
        "ref": project_ref(tenant, &project.id).0,
        "name": project.name,
        "issue_prefix": project.issue_prefix,
        "default_issue_type_id": project.default_issue_type_id,
        "created_at": project.created_at,
    })
}

fn parse_body(bytes: &[u8]) -> Result<CreateProjectBody, EdgeError> {
    if bytes.len() > MAX_PROJECT_JSON_BYTES {
        return Err(EdgeError::PayloadTooLarge(format!(
            "project request body exceeds {MAX_PROJECT_JSON_BYTES} bytes"
        )));
    }
    if bytes.is_empty() {
        return Err(EdgeError::BadRequest(
            "empty request body (expected JSON)".into(),
        ));
    }
    serde_json::from_slice(bytes)
        .map_err(|error| EdgeError::BadRequest(format!("invalid project create body: {error}")))
}

fn parse_access_body(bytes: &[u8]) -> Result<ProjectAccessBody, EdgeError> {
    if bytes.len() > MAX_PROJECT_JSON_BYTES {
        return Err(EdgeError::PayloadTooLarge(format!(
            "project access request body exceeds {MAX_PROJECT_JSON_BYTES} bytes"
        )));
    }
    if bytes.is_empty() {
        return Err(EdgeError::BadRequest(
            "empty request body (expected JSON)".into(),
        ));
    }
    serde_json::from_slice(bytes)
        .map_err(|error| EdgeError::BadRequest(format!("invalid project access body: {error}")))
}

fn parse_page_query(query: &str) -> Result<(u32, Option<String>), EdgeError> {
    let mut limit = None;
    let mut cursor = None;
    if !query.is_empty() {
        for pair in query.split('&') {
            let (name, value) = pair
                .split_once('=')
                .ok_or_else(|| EdgeError::BadRequest("malformed project query parameter".into()))?;
            match name {
                "limit" if limit.is_none() => {
                    let parsed = value.parse::<u32>().map_err(|_| {
                        EdgeError::BadRequest(
                            "project limit must be an integer between 1 and 100".into(),
                        )
                    })?;
                    if parsed == 0 || parsed > MAX_PAGE_LIMIT {
                        return Err(EdgeError::BadRequest(
                            "project limit must be an integer between 1 and 100".into(),
                        ));
                    }
                    limit = Some(parsed);
                }
                "cursor" if cursor.is_none() => cursor = Some(value.to_string()),
                "limit" | "cursor" => {
                    return Err(EdgeError::BadRequest(format!(
                        "duplicate project query parameter `{name}`"
                    )))
                }
                other => {
                    return Err(EdgeError::BadRequest(format!(
                        "unknown project query parameter `{other}`"
                    )))
                }
            }
        }
    }
    Ok((limit.unwrap_or(DEFAULT_PAGE_LIMIT), cursor))
}

fn project_param<'a>(ctx: &'a HandlerCtx<'_>) -> Result<&'a str, EdgeError> {
    let value = ctx
        .params
        .get("project")
        .map(String::as_str)
        .ok_or_else(|| EdgeError::BadRequest("route did not bind a project id".into()))?;
    let parsed = sqlx::types::Uuid::parse_str(value)
        .map_err(|_| EdgeError::BadRequest("project id must be a canonical UUID".into()))?;
    if parsed.to_string() != value {
        return Err(EdgeError::BadRequest(
            "project id must be a canonical UUID".into(),
        ));
    }
    Ok(value)
}

fn require_empty_query(ctx: &HandlerCtx<'_>) -> Result<(), EdgeError> {
    if ctx.request.query.is_empty() {
        Ok(())
    } else {
        Err(EdgeError::BadRequest(
            "project operation accepts no query parameters".into(),
        ))
    }
}

fn require_empty_body(ctx: &HandlerCtx<'_>, operation: &str) -> Result<(), EdgeError> {
    if ctx.request.body.is_empty() {
        Ok(())
    } else {
        Err(EdgeError::BadRequest(format!(
            "{operation} accepts no request body"
        )))
    }
}

fn map_project_error(error: ProjectError) -> EdgeError {
    match error {
        ProjectError::BadInput(reason) => EdgeError::BadRequest(reason),
        ProjectError::NotFound => EdgeError::NotFound("project not found".into()),
        ProjectError::Conflict(reason) => EdgeError::Conflict(reason),
        ProjectError::Storage(reason) => EdgeError::Internal(reason),
    }
}

fn no_store(response: EdgeResponse) -> EdgeResponse {
    response.with_header("Cache-Control", "no-store")
}

fn valid_collaborator_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_COLLABORATOR_TEXT_BYTES
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_create_body_and_list_query_are_strict() {
        assert!(parse_body(br#"{"name":"Developer experience","issue_prefix":"DX"}"#).is_ok());
        assert!(parse_body(br#"{"name":"DX","issue_prefix":"DX","tenant":"other"}"#).is_err());
        assert_eq!(parse_page_query("").unwrap(), (50, None));
        assert_eq!(
            parse_page_query("limit=7&cursor=11111111-1111-1111-1111-111111111111").unwrap(),
            (7, Some("11111111-1111-1111-1111-111111111111".into()))
        );
        for invalid in ["limit=0", "limit=101", "limit=1&limit=2", "tenant=other"] {
            assert!(parse_page_query(invalid).is_err(), "accepted `{invalid}`");
        }
        assert!(parse_access_body(br#"{"principal_id":"p:reviewer","access":true}"#).is_ok());
        assert!(parse_access_body(
            br#"{"principal_id":"p:reviewer","access":true,"tenant":"other"}"#
        )
        .is_err());
    }

    #[test]
    fn project_storage_failures_do_not_reach_the_client() {
        let error = map_project_error(ProjectError::Storage(
            "postgres relation customer@example.test".into(),
        ));
        assert_eq!(error.client_message(), "internal error");
        assert!(!error.envelope().to_string().contains("postgres"));
    }

    #[test]
    fn collaborator_projection_rejects_text_the_public_contract_cannot_represent() {
        assert!(valid_collaborator_text("Ada Lovelace"));
        assert!(!valid_collaborator_text(" Ada Lovelace"));
        assert!(!valid_collaborator_text("Ada\nLovelace"));
        assert!(!valid_collaborator_text(&"a".repeat(256)));
    }
}

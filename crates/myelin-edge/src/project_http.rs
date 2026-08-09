use crate::catalogue::{page_envelope, Handler, HandlerCtx};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::request::EdgeResponse;
use crate::Method;
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, Permission, Zookie,
};
use myelin_identity_service::{
    project_ref, NewProject, PgProjectStore, Project, ProjectError, StoreBackedCheck,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::future::Future;
use std::sync::Arc;
use tokio::runtime::{Handle, RuntimeFlavor};

const MAX_PROJECT_JSON_BYTES: usize = 4 * 1024;
const DEFAULT_PAGE_LIMIT: u32 = 50;
const MAX_PAGE_LIMIT: u32 = 100;

#[derive(Clone)]
struct ProjectHttpApi {
    store: PgProjectStore,
    authz: StoreBackedCheck,
    runtime: Handle,
}

impl ProjectHttpApi {
    fn drive<F, T>(&self, future: F) -> Result<T, ProjectError>
    where
        F: Future<Output = Result<T, ProjectError>>,
    {
        match Handle::try_current() {
            Ok(current) if current.runtime_flavor() == RuntimeFlavor::MultiThread => {
                tokio::task::block_in_place(|| self.runtime.block_on(future))
            }
            Ok(_) => Err(ProjectError::Storage(
                "project HTTP requires the Edge multi-thread runtime".into(),
            )),
            Err(_) => self.runtime.block_on(future),
        }
    }

    fn may_view(&self, ctx: &HandlerCtx<'_>, project_id: &str) -> bool {
        let consistency = Consistency {
            at_least: Zookie(String::new()),
            mode: ConsistencyMode::Strong,
        };
        matches!(
            self.authz.check(
                ctx.principal,
                &Permission("view".into()),
                &project_ref(&ctx.principal.tenant.0, project_id),
                &consistency,
                None,
            ),
            Ok(Decision::Allow)
        )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateProjectBody {
    name: String,
    issue_prefix: String,
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

impl Handler for ProjectListHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        require_empty_body(ctx, "project list")?;
        let (limit, cursor) = parse_page_query(&ctx.request.query)?;
        let mut visible = self
            .api
            .drive(
                self.api
                    .store
                    .list_visible(ctx.principal, cursor.as_deref(), limit + 1),
            )
            .map_err(map_project_error)?;
        let has_more = visible.len() > limit as usize;
        visible.truncate(limit as usize);
        let next_cursor = has_more
            .then(|| visible.last().map(|project| project.id.clone()))
            .flatten();
        let items = visible
            .iter()
            .map(|project| project_json(&ctx.principal.tenant.0, project))
            .collect::<Vec<_>>();
        Ok(no_store(EdgeResponse::json(
            200,
            &page_envelope(json!(items), next_cursor, limit as usize),
        )))
    }
}

pub fn register_projects(
    builder: GatewayBuilder,
    store: PgProjectStore,
    authz: StoreBackedCheck,
    runtime: Handle,
) -> GatewayBuilder {
    let api = ProjectHttpApi {
        store,
        authz,
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
            Arc::new(ProjectGetHandler { api }),
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
    }

    #[test]
    fn project_storage_failures_do_not_reach_the_client() {
        let error = map_project_error(ProjectError::Storage(
            "postgres relation customer@example.test".into(),
        ));
        assert_eq!(error.client_message(), "internal error");
        assert!(!error.envelope().to_string().contains("postgres"));
    }
}

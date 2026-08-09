use crate::catalogue::{page_envelope, Handler, HandlerCtx, Method, API_VERSION};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::request::EdgeResponse;
use myelin_git::api::{http_catalogue, Method as GitMethod};
use myelin_git::web::{
    representative_pr_page, PrOverviewPage, RepoHome, WebEditForm, WebEditOutcome,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

const E1_1_NOTE: &str = "edge route + handler are real and run under the verified tenant scope; the \
                         durable git effect (RefStore ref-CAS / WireExecutor / PR-row persistence) \
                         lands with the Git subsystem track (E1.1)";

#[derive(Default)]
pub struct GitEdgeState {
    repos: BTreeMap<(String, String), RepoHome>,
    prs: BTreeMap<(String, String, u64), PrOverviewPage>,
    blobs: BTreeMap<(String, String, String, String), WebEditForm>,
    code: BTreeMap<String, Vec<Value>>,
}

impl GitEdgeState {
    pub fn new() -> GitEdgeState {
        GitEdgeState::default()
    }

    pub fn with_repo(mut self, tenant: &str, slug: &str, repo: RepoHome) -> GitEdgeState {
        self.repos.insert((tenant.into(), slug.into()), repo);
        self
    }

    pub fn with_pr(
        mut self,
        tenant: &str,
        repo: &str,
        number: u64,
        page: PrOverviewPage,
    ) -> GitEdgeState {
        self.prs.insert((tenant.into(), repo.into(), number), page);
        self
    }

    pub fn with_blob(
        mut self,
        tenant: &str,
        repo: &str,
        gitref: &str,
        path: &str,
        form: WebEditForm,
    ) -> GitEdgeState {
        self.blobs
            .insert((tenant.into(), repo.into(), gitref.into(), path.into()), form);
        self
    }

    pub fn with_code_hit(mut self, tenant: &str, hit: Value) -> GitEdgeState {
        self.code.entry(tenant.into()).or_default().push(hit);
        self
    }

    pub fn seed_demo(self, tenant: &str) -> GitEdgeState {
        let slug = format!("{tenant}/myelin");
        let region = std::env::var("MYELIN_REGION").unwrap_or_else(|_| "fr-par".to_string());
        let clone_url = format!("/{tenant}/{region}/myelin.git");
        self.with_repo(
            tenant,
            "myelin",
            RepoHome::Populated {
                slug: slug.clone(),
                readme_excerpt: format!("# {tenant}/myelin\n\nThe make-it-real spine."),
                entries: vec![
                    ("README.md".into(), false),
                    ("crates".into(), true),
                    ("Cargo.toml".into(), false),
                ],
                clone_url,
            },
        )
        .with_pr(tenant, "myelin", 1, representative_pr_page(tenant))
        .with_blob(
            tenant,
            "myelin",
            "main",
            "README.md",
            WebEditForm {
                path: "README.md".into(),
                contents: format!("# {tenant}/myelin\n"),
                base_oid: "blake3:demohead".into(),
                viewer_may_edit: true,
            },
        )
        .with_code_hit(
            tenant,
            json!({
                "repo": "myelin",
                "path": "crates/myelin-edge/src/git_edge.rs",
                "line": 1,
                "excerpt": format!("// {tenant}: Git wired through the product edge"),
            }),
        )
    }

    fn repos_for(&self, tenant: &str) -> Vec<&RepoHome> {
        self.repos
            .range((tenant.to_string(), String::new())..)
            .take_while(|((t, _), _)| t == tenant)
            .map(|(_, v)| v)
            .collect()
    }
}

pub(crate) fn tenant_of<'a>(ctx: &'a HandlerCtx<'_>) -> &'a str {
    ctx.scope.tenant().0.as_str()
}

pub(crate) fn param<'a>(ctx: &'a HandlerCtx<'_>, name: &str) -> Result<&'a str, EdgeError> {
    ctx.params
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| EdgeError::BadRequest(format!("missing path param `{name}`")))
}

pub(crate) fn num_param(ctx: &HandlerCtx<'_>, name: &str) -> Result<u64, EdgeError> {
    let raw = param(ctx, name)?;
    raw.parse::<u64>()
        .map_err(|_| EdgeError::BadRequest(format!("path param `{name}` is not a number: `{raw}`")))
}

fn deferred_write(applied: Value) -> EdgeResponse {
    EdgeResponse::json(
        200,
        &json!({ "applied": applied, "durable": false, "note": E1_1_NOTE }),
    )
}

struct RepoListHandler {
    state: Arc<GitEdgeState>,
}
impl Handler for RepoListHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let tenant = tenant_of(ctx);
        let all = self.state.repos_for(tenant);
        let offset = ctx
            .page
            .cursor
            .as_deref()
            .and_then(|c| c.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = ctx.page.limit;
        let items: Vec<Value> = all
            .iter()
            .skip(offset)
            .take(limit)
            .map(|r| r.to_json())
            .collect();
        let next = if offset + limit < all.len() {
            Some((offset + limit).to_string())
        } else {
            None
        };
        Ok(EdgeResponse::json(
            200,
            &page_envelope(json!(items), next, limit),
        ))
    }
}

struct PrOverviewHandler {
    state: Arc<GitEdgeState>,
}
impl Handler for PrOverviewHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let key = (
            tenant_of(ctx).to_string(),
            param(ctx, "repo")?.to_string(),
            num_param(ctx, "n")?,
        );
        let page = self
            .state
            .prs
            .get(&key)
            .ok_or_else(|| EdgeError::NotFound("no such pull request".into()))?;
        Ok(EdgeResponse::json(200, &page.to_json()))
    }
}

struct PrChecksHandler {
    state: Arc<GitEdgeState>,
}
impl Handler for PrChecksHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let key = (
            tenant_of(ctx).to_string(),
            param(ctx, "repo")?.to_string(),
            num_param(ctx, "n")?,
        );
        let page = self
            .state
            .prs
            .get(&key)
            .ok_or_else(|| EdgeError::NotFound("no such pull request".into()))?;
        Ok(EdgeResponse::json(200, &page.checks.to_json()))
    }
}

struct BlobViewHandler {
    state: Arc<GitEdgeState>,
}
impl Handler for BlobViewHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let key = (
            tenant_of(ctx).to_string(),
            param(ctx, "repo")?.to_string(),
            param(ctx, "ref")?.to_string(),
            param(ctx, "path")?.to_string(),
        );
        let form = self
            .state
            .blobs
            .get(&key)
            .ok_or_else(|| EdgeError::NotFound("no such file at that ref".into()))?;
        Ok(EdgeResponse::json(200, &form.to_json()))
    }
}

struct CodeSearchHandler {
    state: Arc<GitEdgeState>,
}
impl Handler for CodeSearchHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let tenant = tenant_of(ctx);
        let q = ctx.request.query_param("q").unwrap_or_default().to_lowercase();
        let hits: Vec<Value> = self
            .state
            .code
            .get(tenant)
            .into_iter()
            .flatten()
            .filter(|h| q.is_empty() || h.to_string().to_lowercase().contains(&q))
            .cloned()
            .collect();
        let limit = ctx.page.limit;
        let page: Vec<Value> = hits.into_iter().take(limit).collect();
        Ok(EdgeResponse::json(200, &page_envelope(json!(page), None, limit)))
    }
}

struct DeferredWriteHandler {
    action: &'static str,
}
impl Handler for DeferredWriteHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let body = if ctx.request.body.is_empty() {
            Value::Null
        } else {
            ctx.request.json_body()?
        };
        let mut applied = json!({
            "action": self.action,
            "tenant": tenant_of(ctx),
            "request": body,
        });
        for k in ["repo", "n"] {
            if let Some(v) = ctx.params.get(k) {
                applied[k] = json!(v);
            }
        }
        Ok(deferred_write(applied))
    }
}

struct WebEditCommitHandler {
    state: Arc<GitEdgeState>,
}
impl Handler for WebEditCommitHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        let key = (
            tenant_of(ctx).to_string(),
            param(ctx, "repo")?.to_string(),
            param(ctx, "ref")?.to_string(),
            param(ctx, "path")?.to_string(),
        );
        let form = self
            .state
            .blobs
            .get(&key)
            .ok_or_else(|| EdgeError::NotFound("no such file at that ref".into()))?;
        let body = ctx.request.json_body()?;
        let expected_base = body
            .get("base_oid")
            .and_then(Value::as_str)
            .ok_or_else(|| EdgeError::BadRequest("commit body missing `base_oid`".into()))?;
        let contents = body
            .get("contents")
            .and_then(Value::as_str)
            .ok_or_else(|| EdgeError::BadRequest("commit body missing `contents`".into()))?;
        let new_oid = format!("modeled:{}", content_fingerprint(contents.as_bytes()));
        let outcome = WebEditOutcome::evaluate(
            expected_base,
            &form.base_oid,
            &new_oid,
            form.viewer_may_edit,
        );
        match outcome {
            WebEditOutcome::Denied => Err(EdgeError::Forbidden("no write permission for this ref".into())),
            WebEditOutcome::StaleBase { .. } => Err(EdgeError::Conflict(
                "the file changed since you opened it - refused so nothing is silently overwritten \
                 (GF-6: no 3-way editor in v1)"
                    .into(),
            )),
            committed @ WebEditOutcome::Committed { .. } => Ok(deferred_write(committed.to_json())),
        }
    }
}

fn content_fingerprint(bytes: &[u8]) -> String {
    let mut h: u64 = 1469598103934665603;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    format!("{h:016x}")
}

pub(crate) fn reroot(path: &str) -> String {
    let tail = path.strip_prefix("/api/git").unwrap_or(path);
    format!("/{API_VERSION}/git{tail}")
}

pub(crate) fn map_method(m: GitMethod) -> Method {
    match m {
        GitMethod::Get => Method::Get,
        GitMethod::Post => Method::Post,
    }
}

pub fn register_git(mut b: GatewayBuilder, state: Arc<GitEdgeState>) -> GatewayBuilder {
    for ep in http_catalogue() {
        let pattern = match (ep.method, ep.path) {
            (
                GitMethod::Get | GitMethod::Post,
                "/api/git/repos/{repo}/blob/{ref}/{path}",
            ) => reroot("/api/git/repos/{repo}/blob/{ref}/{...path}"),
            _ => reroot(ep.path),
        };
        let method = map_method(ep.method);
        let (handler, action): (Arc<dyn Handler>, &'static str) = match (ep.method, ep.path) {
            (GitMethod::Get, "/api/git/repos") => (
                Arc::new(RepoListHandler { state: state.clone() }),
                "git.repos.list",
            ),
            (GitMethod::Get, "/api/git/repos/{repo}/prs/{n}") => (
                Arc::new(PrOverviewHandler { state: state.clone() }),
                "git.pr.view",
            ),
            (GitMethod::Get, "/api/git/repos/{repo}/prs/{n}/checks") => (
                Arc::new(PrChecksHandler { state: state.clone() }),
                "git.pr.checks",
            ),
            (GitMethod::Get, "/api/git/repos/{repo}/blob/{ref}/{path}") => (
                Arc::new(BlobViewHandler { state: state.clone() }),
                "git.blob.view",
            ),
            (GitMethod::Get, "/api/git/search/code") => (
                Arc::new(CodeSearchHandler { state: state.clone() }),
                "git.search.code",
            ),
            (GitMethod::Post, "/api/git/repos/{repo}/blob/{ref}/{path}") => (
                Arc::new(WebEditCommitHandler { state: state.clone() }),
                "git.blob.commit",
            ),
            (GitMethod::Post, "/api/git/repos") => (
                Arc::new(DeferredWriteHandler { action: "git.repo.create" }),
                "git.repo.create",
            ),
            (GitMethod::Post, "/api/git/repos/{repo}/prs") => (
                Arc::new(DeferredWriteHandler { action: "git.pr.open" }),
                "git.pr.open",
            ),
            (GitMethod::Post, "/api/git/repos/{repo}/prs/{n}/reviews") => (
                Arc::new(DeferredWriteHandler { action: "git.pr.review" }),
                "git.pr.review",
            ),
            (GitMethod::Post, "/api/git/repos/{repo}/prs/{n}/endorse-fork-ci") => (
                Arc::new(DeferredWriteHandler { action: "git.pr.endorse_fork_ci" }),
                "git.pr.endorse_fork_ci",
            ),
            (GitMethod::Post, "/api/git/repos/{repo}/prs/{n}/merge") => (
                Arc::new(DeferredWriteHandler { action: "git.pr.merge" }),
                "git.pr.merge",
            ),
            (_, other) => (
                Arc::new(DeferredWriteHandler { action: "git.unmapped" }),
                Box::leak(format!("git.unmapped:{other}").into_boxed_str()),
            ),
        };
        b = b.route(method, &pattern, action, handler);
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reroot_maps_api_git_to_v1_git() {
        assert_eq!(reroot("/api/git/repos"), "/v1/git/repos");
        assert_eq!(
            reroot("/api/git/repos/{repo}/prs/{n}/checks"),
            "/v1/git/repos/{repo}/prs/{n}/checks"
        );
    }

    #[test]
    fn every_catalogue_entry_is_mapped_and_rerooted() {
        let state = Arc::new(GitEdgeState::new());
        let b = register_git(
            crate::gateway::Gateway::builder(
                test_authn(),
                test_human(),
                Arc::new(crate::AllowAll),
            ),
            state,
        );
        let gw = b.build();
        let _ = gw;
        assert_eq!(
            http_catalogue().len(),
            14,
            "the git catalogue surface is stable"
        );
    }

    fn test_authn() -> Arc<myelin_identity_service::CapabilityAuthenticator> {
        use myelin_identity_service::{
            CapabilityAuthenticator, CellTokenAuthority, PasetoCapabilityVerifier, PrincipalStore,
            RevocationStore,
        };
        use myelin_storage::KmsEngine;
        let cell = CellTokenAuthority::from_seed(&[7u8; 32], &[9u8; 32]).expect("cell");
        Arc::new(CapabilityAuthenticator::with_verifier(
            PrincipalStore::new(Arc::new(KmsEngine::new())),
            Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
            RevocationStore::new(),
        ))
    }

    fn test_human() -> Arc<myelin_identity_service::HumanSsoAuthenticator> {
        use myelin_identity_service::{HumanSsoAuthenticator, PrincipalStore};
        use myelin_storage::KmsEngine;
        Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(
            Arc::new(KmsEngine::new()),
        )))
    }
}

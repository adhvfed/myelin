//! # Git wired through the product edge (MR-015 / E0.6) — the read/projection surface
//!
//! This is the FIRST subsystem plugged into the MR-014 gateway, and the CONTRACT every subsequent
//! subsystem (issues/chat/knowledge/CI) follows: a subsystem adds ONLY its routes + handlers; the
//! gateway owns authentication, tenant-from-token, the IDOR reject/audit, authorization, the error
//! envelope, versioning, pagination, and SSE.
//!
//! ## Anti-duplication (the binding first step)
//! - **The endpoint SET is Git's own** — [`register_git`] iterates [`myelin_git::api::http_catalogue`]
//!   (Git's `(Method, path, Handler)` grammar) and re-roots each `/api/git/...` path under the edge's
//!   versioned `/v1/git/...` prefix. We do NOT re-invent the git endpoints or fork a parallel router.
//! - **The JSON body IS the Git ViewModel** — each read serves a `myelin_git::web` ViewModel's
//!   `to_json()` (the SAME vocabulary `render()` projects to HTML; design pass §0: never a parallel
//!   vocabulary). The UI renders; the edge provides the projection.
//! - **The MR-014 convention is reused verbatim** — handlers implement [`crate::Handler`], register via
//!   [`crate::GatewayBuilder::route`], read [`crate::HandlerCtx`] (post-auth: the VERIFIED tenant
//!   scope + path params + pagination), and never see a raw credential or a client-supplied tenant.
//!
//! ## Tenant scope — the cardinal IDOR rule
//! Every handler keys its lookup on `ctx.scope.tenant()` — the tenant of the VERIFIED token, set by
//! the gateway. Git's endpoint grammar carries NO `{tenant}` path segment (tenant is from the token,
//! never the URL — the GIT-D8 invariant). So a token for tenant A queries ONLY tenant A's git data;
//! it cannot even NAME tenant B's. (The path-tenant IDOR reject/audit the gateway applies to any
//! `{tenant}`-carrying route is proven generically in MR-014; git's routes carry none by design.)
//!
//! ## Read vs write vs deferred (HONEST scoping)
//! - **Reads (repos list, PR overview, PR checks, blob/file view, code search)** are FULLY REAL through
//!   the edge: the lifecycle (auth/scope/authz/IDOR/pagination/error) is real, AND the response is
//!   Git's real ViewModel projected to JSON. The DATA SOURCE behind the ViewModel ([`GitEdgeState`])
//!   is in-memory today — Git's durable repo/ref/pack storage is the **Git subsystem track (E1.1)**,
//!   NOT this prompt. The contract + projection are real; the durable backing is E1.1.
//! - **Writes (create repo, open PR, review, endorse-fork-ci, merge)**: the edge ROUTE + handler are
//!   real (parsed + validated under `ctx.scope`), but the DURABLE effect (the RefStore ref-CAS / the
//!   WireExecutor / the PR-row persistence) lands with the **Git track (E1.1)**. These return an
//!   explicit `{ "durable": false, ... }` write envelope — we do NOT fake a write that doesn't persist.
//! - **Web-edit commit** additionally runs the REAL pure [`myelin_git::web::WebEditOutcome::evaluate`]
//!   CAS (GF-6): a stale base is an honest `409` refuse (no silent overwrite, no 3-way editor); a clean
//!   base reports the modeled `committed` outcome with `durable: false` (the ref advance is E1.1).
//!
//! ## NOT MOUNTED IN PRODUCTION (R2.1 grounding — do not parallel-patch)
//! The production edge binary (`main.rs`) mounts [`crate::register_git_durable`] (GT-003, the durable
//! front door) + [`crate::register_git_wire`] — **NOT** [`register_git`]. This module's in-memory
//! predecessor handlers serve only the legacy integration proofs
//! (`tests/git_edge_integration.rs`, `myelin-cli/tests/cli_edge_integration.rs`) over seeded
//! [`GitEdgeState`] fixtures; no production route reaches them, so the R2.1 per-repo OBJECT
//! authorization (the [`crate::repo_authz::RepoAuthorizer`] guard every object-addressed durable
//! route now passes through) is deliberately NOT duplicated here. If these handlers are ever
//! re-mounted on a serving binary they MUST first be wrapped in the same object guard
//! (`git_durable.rs::RepoObjectGuard`) — grep for `RepoObjectGuard` and mirror the registration.

use crate::catalogue::{page_envelope, Handler, HandlerCtx, Method, API_VERSION};
use crate::error::EdgeError;
use crate::gateway::GatewayBuilder;
use crate::request::EdgeResponse;
use myelin_git::api::{http_catalogue, Method as GitMethod};
use myelin_git::web::{
    switch_test_representative_pr_page, PrOverviewPage, RepoHome, WebEditForm, WebEditOutcome,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The note attached to every write whose durable effect is the Git subsystem track (E1.1). The edge
/// contract is real; the git storage/execution hardening is honestly deferred (never faked).
const E1_1_NOTE: &str = "edge route + handler are real and run under the verified tenant scope; the \
                         durable git effect (RefStore ref-CAS / WireExecutor / PR-row persistence) \
                         lands with the Git subsystem track (E1.1)";

/// **The in-memory git data the edge projects** (per `(tenant, …)` key). This mirrors the reality that
/// Git's `ArtifactStore`/`RefStore`/pack-index are in-memory today — the durable backing is the Git
/// track (E1.1). The edge reads it ONLY under the verified `ctx.scope.tenant()`, so it is partitioned
/// by the token's tenant: a token for A can never index B's data.
#[derive(Default)]
pub struct GitEdgeState {
    /// `(tenant, slug)` → the repo-home ViewModel.
    repos: BTreeMap<(String, String), RepoHome>,
    /// `(tenant, repo, pr#)` → the PR overview ViewModel (the centrepiece).
    prs: BTreeMap<(String, String, u64), PrOverviewPage>,
    /// `(tenant, repo, ref, path)` → the single-file view/edit ViewModel.
    blobs: BTreeMap<(String, String, String, String), WebEditForm>,
    /// `tenant` → the code-search hits (the result projection; the ranked ACL-pre-filtered Search-index
    /// integration is the Search track — these are tenant-scoped seeded hits proving the edge contract
    /// + isolation).
    code: BTreeMap<String, Vec<Value>>,
}

impl GitEdgeState {
    /// An empty backend.
    pub fn new() -> GitEdgeState {
        GitEdgeState::default()
    }

    /// Seed a repo-home ViewModel for `(tenant, slug)`.
    pub fn with_repo(mut self, tenant: &str, slug: &str, repo: RepoHome) -> GitEdgeState {
        self.repos.insert((tenant.into(), slug.into()), repo);
        self
    }

    /// Seed a PR overview ViewModel for `(tenant, repo, number)`.
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

    /// Seed a single-file view ViewModel for `(tenant, repo, ref, path)`.
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

    /// Seed a code-search hit for a tenant (`{ repo, path, line, excerpt }`).
    pub fn with_code_hit(mut self, tenant: &str, hit: Value) -> GitEdgeState {
        self.code.entry(tenant.into()).or_default().push(hit);
        self
    }

    /// Seed a representative, fully-populated tenant using Git's OWN real ViewModels — the repo home,
    /// the GIT-P35 switch-test PR overview (`switch_test_representative_pr_page`), a single-file view,
    /// and a code hit. Used by the deployable binary's bootable demo + the integration proofs.
    pub fn seed_demo(self, tenant: &str) -> GitEdgeState {
        let slug = format!("{tenant}/myelin");
        // F3 (R4.1 dogfood): advertise the HONEST HTTP git-wire clone URL — the wire path grammar is
        // `/{tenant}/{region}/{repo}.git` over HTTP smart-transport (there is NO SSH server). The demo
        // fixture is keyed only by (tenant, slug), so it renders a relative wire path with the demo
        // residency region; never the old `ssh://git@myelin/…` (wrong scheme, missing region).
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
        .with_pr(tenant, "myelin", 1, switch_test_representative_pr_page(tenant))
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

/// The verified tenant of the request (the ONLY tenant a handler may touch — the IDOR floor).
pub(crate) fn tenant_of<'a>(ctx: &'a HandlerCtx<'_>) -> &'a str {
    ctx.scope.tenant().0.as_str()
}

/// A required path param, or a clean `400` (never a panic).
pub(crate) fn param<'a>(ctx: &'a HandlerCtx<'_>, name: &str) -> Result<&'a str, EdgeError> {
    ctx.params
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| EdgeError::BadRequest(format!("missing path param `{name}`")))
}

/// A required numeric path param (e.g. a PR number), or a clean `400`.
pub(crate) fn num_param(ctx: &HandlerCtx<'_>, name: &str) -> Result<u64, EdgeError> {
    let raw = param(ctx, name)?;
    raw.parse::<u64>()
        .map_err(|_| EdgeError::BadRequest(format!("path param `{name}` is not a number: `{raw}`")))
}

/// The honest write envelope for a route whose durable effect is the Git track (E1.1). Returns the
/// modeled/accepted outcome with `durable: false` + the explicit note — never a faked persisted write.
fn deferred_write(applied: Value) -> EdgeResponse {
    EdgeResponse::json(
        200,
        &json!({ "applied": applied, "durable": false, "note": E1_1_NOTE }),
    )
}

// ---------------------------------------------------------------------------
// Read handlers (fully real through the edge; ViewModel-backed)
// ---------------------------------------------------------------------------

/// `GET /v1/git/repos` — the leak-free repo list (Git `Handler::ListFilter`), paginated via the MR-014
/// uniform `{ items, page }` envelope. Serves ONLY the verified tenant's repos.
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

/// `GET /v1/git/repos/{repo}/prs/{n}` — the per-viewer PR overview projection (Git `Handler::Project`).
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

/// `GET /v1/git/repos/{repo}/prs/{n}/checks` — the X-1 checks projection (Git `Handler::CheckStatus`).
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

/// `GET /v1/git/repos/{repo}/blob/{ref}/{path}` — the single-file view projection (Git
/// `Handler::Project`). NOTE: the gateway router is segment-based, so `{path}` matches a single path
/// segment (a multi-segment file path is the URL-codec follow-on the gateway names).
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

/// `GET /v1/git/search/code` — the ACL-pre-filtered code search (Git `Handler::CodeSearch`). Serves the
/// verified tenant's hits, optionally filtered by `?q=`. The ranked, ACL-pre-filtered Search-INDEX
/// integration (`list_filter::code_search_pre_filter` conjoined before scoring) is the Search track;
/// these tenant-scoped hits prove the edge contract + tenant isolation.
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

// ---------------------------------------------------------------------------
// Write handlers (route + handler real; durable effect deferred to E1.1)
// ---------------------------------------------------------------------------

/// A write whose durable effect is the Git track (E1.1). Validates the body under `ctx.scope` and
/// returns the honest `durable: false` envelope echoing the accepted intent — never a faked write.
struct DeferredWriteHandler {
    action: &'static str,
}
impl Handler for DeferredWriteHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        // The body is parsed loudly (a malformed body is a clean 400) — the request shape is validated
        // even though the durable effect is deferred. An empty body is permitted (intent-only verbs).
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

/// `POST /v1/git/repos/{repo}/blob/{ref}/{path}` — the single-file web-edit commit (Git
/// `Handler::ReceivePack`, GF-6). Runs the REAL pure CAS ([`WebEditOutcome::evaluate`]): a stale base is
/// an honest `409` refuse (no silent overwrite, no 3-way editor); a clean base reports the modeled
/// `committed` outcome with `durable: false` (the durable ref-CAS lands with E1.1).
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
        // A modeled new-commit OID — a deterministic content fingerprint of the edited bytes. The REAL
        // blake3 content-address + the DURABLE ref advance are computed by the git receive-pack one-tx
        // ref-CAS (E1.1); this value is part of the `durable: false` modeled outcome, not a real commit.
        let new_oid = format!("modeled:{}", content_fingerprint(contents.as_bytes()));
        // The REAL pure GF-6 CAS: current head = the stored blob's base_oid.
        let outcome = WebEditOutcome::evaluate(
            expected_base,
            &form.base_oid,
            &new_oid,
            form.viewer_may_edit,
        );
        match outcome {
            WebEditOutcome::Denied => Err(EdgeError::Forbidden("no write permission for this ref".into())),
            WebEditOutcome::StaleBase { .. } => Err(EdgeError::Conflict(
                "the file changed since you opened it — refused so nothing is silently overwritten \
                 (GF-6: no 3-way editor in v1)"
                    .into(),
            )),
            committed @ WebEditOutcome::Committed { .. } => Ok(deferred_write(committed.to_json())),
        }
    }
}

/// A deterministic content fingerprint (FNV-1a) of the edited bytes — used ONLY to mint the modeled
/// new-commit OID in the `durable: false` web-edit outcome. This is NOT the real content-address: the
/// blake3 commit OID + the durable ref-CAS are computed by the git receive-pack path (E1.1).
fn content_fingerprint(bytes: &[u8]) -> String {
    let mut h: u64 = 1469598103934665603; // FNV-1a offset basis
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    format!("{h:016x}")
}

// ---------------------------------------------------------------------------
// Registration — the route table, sourced from Git's OWN catalogue (anti-duplication)
// ---------------------------------------------------------------------------

/// Re-root a Git catalogue path (`/api/git/...`) under the edge's versioned prefix (`/v1/git/...`).
pub(crate) fn reroot(path: &str) -> String {
    let tail = path.strip_prefix("/api/git").unwrap_or(path);
    format!("/{API_VERSION}/git{tail}")
}

/// Map a Git API method to the edge method (Git's surface is Get/Post; the edge supports the full set).
pub(crate) fn map_method(m: GitMethod) -> Method {
    match m {
        GitMethod::Get => Method::Get,
        GitMethod::Post => Method::Post,
    }
}

/// **Register Git through the product edge.** Iterates [`myelin_git::api::http_catalogue`] (Git's OWN
/// endpoint grammar — the set of routes is Git's, not a fork), re-roots each under `/v1/git/...`, and
/// binds the MR-014 [`Handler`] + the re-authorized action. The gateway owns auth/scope/IDOR/authz/
/// error/pagination/SSE; this adds ONLY Git's routes + handlers (the plug-in contract).
pub fn register_git(mut b: GatewayBuilder, state: Arc<GitEdgeState>) -> GatewayBuilder {
    for ep in http_catalogue() {
        let pattern = reroot(ep.path);
        let method = map_method(ep.method);
        let (handler, action): (Arc<dyn Handler>, &'static str) = match (ep.method, ep.path) {
            // Reads — ViewModel-backed, fully real through the edge.
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
            // Writes — route + handler real; durable effect deferred to E1.1.
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
            // Any future catalogue entry is registered as a deferred write (fail-honest) until a typed
            // handler is added — never silently dropped.
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
        // The edge route SET is exactly Git's catalogue, re-rooted (anti-duplication: no fork).
        let state = Arc::new(GitEdgeState::new());
        let b = register_git(
            crate::gateway::Gateway::builder(
                test_authn(),
                test_human(),
                Arc::new(crate::AllowAll),
            ),
            state,
        );
        // Build succeeds and registered one route per catalogue entry.
        let gw = b.build();
        let _ = gw; // routes are private; the build not panicking + reroot test cover the mapping.
        assert_eq!(http_catalogue().len(), 13, "the git catalogue surface is stable");
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

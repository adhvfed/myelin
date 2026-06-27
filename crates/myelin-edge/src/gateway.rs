//! # The edge gateway — the one HTTP edge's request lifecycle
//!
//! [`Gateway`] realises the `PublicSurface` request lifecycle the substrate contract describes
//! (`00-platform-substrate.md` §4.1) as a transport-agnostic, total pipeline:
//!
//! ```text
//! request → authenticate (Bearer capability token | session cookie)
//!         → resolve principal (tenant/region from the VERIFIED token)
//!         → set tenant scope (TenantScope::from_verified_token; reject+audit a cross-tenant IDOR)
//!         → authorize (re-authorize the action — "internal = safe" is never presumed)
//!         → dispatch to the subsystem handler
//!         → respond (JSON view-model | SSE | the {error:{message}} envelope)
//! ```
//!
//! It REUSES (never forks) the real components: the [`CapabilityAuthenticator`] (PASETO Bearer auth +
//! DPoP + durable revocation + S1 lookup, MR-011), [`HumanSsoAuthenticator::production`] (refuse-not-
//! mock human login, MR-012), [`PublicSurface`] (tenant-from-token + IDOR reject/audit, P-S13), and
//! [`TenantScope`] (the verified scope). It is TOTAL over a malformed request (every parse is checked;
//! a failure is a clean typed [`EdgeError`], never a panic), and FAIL-CLOSED (any authenticate failure
//! → a uniform 401; an authorize denial → 403).

use crate::catalogue::{Handler, HandlerCtx, Method, Page};
use crate::error::EdgeError;
use crate::request::{EdgeRequest, EdgeResponse};
use crate::session::{SessionStore, SESSION_COOKIE};
use crate::sse::SseHub;
use myelin_identity::{AuthzError, Credential, Principal, PrincipalKind};
use myelin_identity_service::{CapabilityAuthenticator, HumanSsoAuthenticator};
use myelin_storage::TenantScope;
use myelin_substrate::{Authorizer, InjectedIdentity, PublicSurface};
use myelin_tenancy::TenantId;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The default credential scheme assumed for a Bearer token when the client sends no
/// `X-Myelin-Token-Scheme` header. `pat` is the primary edge consumer (the UI/CLI cookie gateway
/// carries a PAT bearer server-side).
const DEFAULT_TOKEN_SCHEME: &str = "pat";

/// The bounded SSE scope for a verified tenant — `tenant:<t>` (never a client-supplied `*` selector;
/// the §7.7 "never `*`" stream-IDOR floor). A subsystem publishes to this same key.
pub fn sse_scope_for_tenant(tenant: &str) -> String {
    format!("tenant:{tenant}")
}

/// One route segment — a literal or a `{name}` path parameter.
enum Seg {
    Lit(String),
    Param(String),
}

/// What a matched route dispatches to.
enum RouteKind {
    /// A normal handler (a JSON view-model response).
    Normal(Arc<dyn Handler>),
    /// An SSE stream by name (the real-time convention).
    Sse(String),
}

/// A registered route: `(method, pattern)` → a handler/stream, gated by an authorize action.
struct Route {
    method: Method,
    segs: Vec<Seg>,
    action: String,
    kind: RouteKind,
}

fn parse_pattern(pattern: &str) -> Vec<Seg> {
    pattern
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| match s.strip_prefix('{').and_then(|x| x.strip_suffix('}')) {
            Some(name) => Seg::Param(name.to_string()),
            None => Seg::Lit(s.to_string()),
        })
        .collect()
}

/// The builder for a [`Gateway`] — wires the (injected, real) authenticators + authorizer, then
/// registers routes/SSE streams.
pub struct GatewayBuilder {
    authn: Arc<CapabilityAuthenticator>,
    human_login: Arc<HumanSsoAuthenticator>,
    authorizer: Arc<dyn Authorizer>,
    routes: Vec<Route>,
    default_scheme: String,
    sse: SseHub,
    sessions: SessionStore,
    public_surface: PublicSurface,
}

impl GatewayBuilder {
    /// Register a normal route (`method` + `pattern` like `/v1/git/repos/{repo}/prs/{n}`), gated by
    /// the re-authorized `action`, dispatching to `handler`.
    pub fn route(
        mut self,
        method: Method,
        pattern: &str,
        action: impl Into<String>,
        handler: Arc<dyn Handler>,
    ) -> GatewayBuilder {
        self.routes.push(Route {
            method,
            segs: parse_pattern(pattern),
            action: action.into(),
            kind: RouteKind::Normal(handler),
        });
        self
    }

    /// Register an SSE route (`GET pattern`) gated by `action`, streaming the `stream` channel scoped
    /// to the verified tenant.
    pub fn sse_route(
        mut self,
        pattern: &str,
        action: impl Into<String>,
        stream: impl Into<String>,
    ) -> GatewayBuilder {
        self.routes.push(Route {
            method: Method::Get,
            segs: parse_pattern(pattern),
            action: action.into(),
            kind: RouteKind::Sse(stream.into()),
        });
        self
    }

    /// Override the default Bearer-token scheme (when the client sends no `X-Myelin-Token-Scheme`).
    pub fn default_token_scheme(mut self, scheme: impl Into<String>) -> GatewayBuilder {
        self.default_scheme = scheme.into();
        self
    }

    /// Finish building the gateway.
    pub fn build(self) -> Gateway {
        Gateway {
            authn: self.authn,
            human_login: self.human_login,
            authorizer: self.authorizer,
            routes: self.routes,
            default_scheme: self.default_scheme,
            sse: self.sse,
            sessions: self.sessions,
            public_surface: self.public_surface,
        }
    }
}

/// **The edge gateway.** Owns the request lifecycle (authenticate → resolve → scope → authorize →
/// dispatch → respond), the cookie-session machinery, the IDOR reject/audit, and the SSE hub.
pub struct Gateway {
    authn: Arc<CapabilityAuthenticator>,
    human_login: Arc<HumanSsoAuthenticator>,
    authorizer: Arc<dyn Authorizer>,
    routes: Vec<Route>,
    default_scheme: String,
    sse: SseHub,
    sessions: SessionStore,
    public_surface: PublicSurface,
}

impl Gateway {
    /// Start building a gateway over the (real) injected auth components.
    pub fn builder(
        authn: Arc<CapabilityAuthenticator>,
        human_login: Arc<HumanSsoAuthenticator>,
        authorizer: Arc<dyn Authorizer>,
    ) -> GatewayBuilder {
        GatewayBuilder {
            authn,
            human_login,
            authorizer,
            routes: Vec::new(),
            default_scheme: DEFAULT_TOKEN_SCHEME.to_string(),
            sse: SseHub::new(),
            sessions: SessionStore::new(),
            public_surface: PublicSurface::default(),
        }
    }

    /// The SSE hub (so a subsystem / a test publishes frames to a `(stream, scope)`).
    pub fn sse_hub(&self) -> &SseHub {
        &self.sse
    }

    /// The session store (the cookie→bearer machinery; a test seeds a session here).
    pub fn sessions(&self) -> &SessionStore {
        &self.sessions
    }

    /// The public surface (so a test reads the IDOR audit sink — every rejected cross-tenant attempt
    /// is recorded).
    pub fn public_surface(&self) -> &PublicSurface {
        &self.public_surface
    }

    /// **Handle a request — TOTAL, never panics.** Runs the lifecycle; any typed error becomes the
    /// `{error:{message}}` envelope response.
    pub fn handle(&self, req: EdgeRequest) -> EdgeResponse {
        match self.handle_inner(&req) {
            Ok(resp) => resp,
            Err(e) => EdgeResponse::error(&e),
        }
    }

    fn handle_inner(&self, req: &EdgeRequest) -> Result<EdgeResponse, EdgeError> {
        let method = Method::parse(&req.method)
            .ok_or_else(|| EdgeError::BadRequest(format!("unsupported method `{}`", req.method)))?;

        // Built-in auth routes (the cookie-session machinery; login refuses-not-mocks).
        match (method, req.path.as_str()) {
            (Method::Post, "/v1/auth/login") => return self.login(req),
            (Method::Post, "/v1/auth/logout") => return Ok(self.logout(req)),
            (Method::Post, "/v1/auth/refresh") => return self.refresh(req),
            _ => {}
        }

        // Route match (404 if none) — total over an arbitrary path.
        let (idx, params) = self.match_route(method, &req.path).ok_or_else(|| {
            EdgeError::NotFound(format!("no route for {} {}", method.as_str(), req.path))
        })?;
        let route = &self.routes[idx];

        // The special `{tenant}` path param is used ONLY to detect/reject an IDOR — NEVER the source
        // of the operating tenant (the cardinal rule). Most routes carry no `{tenant}` at all.
        let path_tenant = params.get("tenant").map(|t| TenantId(t.clone()));

        // (1) authenticate → the verified Principal (uniform 401 on ANY failure).
        let principal = self.authenticate(req, path_tenant.as_ref())?;
        // (2) resolve + set the tenant scope (reject + audit a cross-tenant path here).
        let scope = self.resolve_scope(&principal, path_tenant.as_ref())?;
        // (3) re-authorize the action (fail-closed → 403); the seam is consulted on EVERY call.
        if !self.authorizer.authorize(&principal, &route.action) {
            return Err(EdgeError::Forbidden(format!(
                "authorization denied for action `{}`",
                route.action
            )));
        }
        // (4) dispatch.
        match &route.kind {
            RouteKind::Normal(handler) => {
                let page = Page::from_request(req);
                let ctx = HandlerCtx {
                    principal: &principal,
                    scope: &scope,
                    params: &params,
                    page: &page,
                    request: req,
                };
                handler.handle(&ctx)
            }
            RouteKind::Sse(stream) => {
                // The subscription scope is the VERIFIED tenant (bounded; never a client selector).
                let sub = self.sse.subscribe(stream, &sse_scope_for_tenant(&scope.tenant().0));
                Ok(EdgeResponse::sse(sub))
            }
        }
    }

    /// Authenticate a request to a [`Principal`]: Bearer capability token first, then session cookie.
    /// ANY failure (forged/expired/revoked/absent) collapses to a uniform [`EdgeError::Unauthorized`]
    /// (401) — the client cannot distinguish the cause (oracle-free). The verified principal's
    /// tenant/region is the request scope; a forged token never resolves a Principal (real PASETO).
    fn authenticate(
        &self,
        req: &EdgeRequest,
        path_tenant: Option<&TenantId>,
    ) -> Result<Principal, EdgeError> {
        if let Some(material) = req.bearer() {
            let scheme = req
                .header("x-myelin-token-scheme")
                .unwrap_or(&self.default_scheme)
                .to_string();
            let cred = Credential { scheme, material: material.to_string() };
            return self
                .authn
                .authenticate(&cred, path_tenant)
                .map_err(|e| EdgeError::Unauthorized(format!("bearer auth failed: {e:?}")));
        }
        if let Some(sid) = req.cookie(SESSION_COOKIE) {
            // The session carries the Bearer server-side (never exposed to client JS).
            if let Some(rec) = self.sessions.get(&sid) {
                let cred = Credential { scheme: rec.scheme, material: rec.material };
                return self
                    .authn
                    .authenticate(&cred, path_tenant)
                    .map_err(|e| EdgeError::Unauthorized(format!("session auth failed: {e:?}")));
            }
            return Err(EdgeError::Unauthorized(
                "session cookie does not resolve a live session".into(),
            ));
        }
        Err(EdgeError::Unauthorized(
            "no credential presented (Bearer token or session cookie required)".into(),
        ))
    }

    /// Build the operating [`TenantScope`] from the VERIFIED principal. If the route carried a
    /// `{tenant}` path param, run it through [`PublicSurface::resolve_tenant`] — a path-tenant ≠
    /// token-tenant is rejected + AUDITED as a cross-tenant IDOR (403), and is NEVER served. The
    /// scope is built from the token's tenant regardless; the path is never the source.
    fn resolve_scope(
        &self,
        principal: &Principal,
        path_tenant: Option<&TenantId>,
    ) -> Result<TenantScope, EdgeError> {
        if let Some(pt) = path_tenant {
            let id = InjectedIdentity::new(principal.clone());
            self.public_surface
                .resolve_tenant(&id, pt)
                .map_err(|reject| EdgeError::Forbidden(format!("cross-tenant IDOR rejected: {reject}")))?;
        }
        Ok(TenantScope::from_verified_token(principal, principal.region.clone()))
    }

    /// `POST /v1/auth/login` — runs the REAL production human verifier (refuse-not-mock). The human
    /// verifier config (JWKS/trust-anchors) is MR-012-deferred, so this REFUSES loudly (503) and
    /// never mints a mock session. On success (unreachable until the verifier is config-wired) it
    /// would issue a session carrying a server-side bearer — the issuance machinery is real
    /// ([`SessionStore::issue`]) but the token mint for a human principal is the named follow-on.
    fn login(&self, req: &EdgeRequest) -> Result<EdgeResponse, EdgeError> {
        let body = req.json_body()?;
        let scheme = body
            .get("scheme")
            .and_then(|v| v.as_str())
            .ok_or_else(|| EdgeError::BadRequest("login body missing `scheme`".into()))?;
        let material = body
            .get("material")
            .and_then(|v| v.as_str())
            .ok_or_else(|| EdgeError::BadRequest("login body missing `material`".into()))?;
        let cred = Credential { scheme: scheme.to_string(), material: material.to_string() };
        match self.human_login.authenticate(&cred, None) {
            Ok(_principal) => Err(EdgeError::Unavailable(
                "human login verified, but server-side token minting for a human principal is not \
                 yet wired (MR-012 deferred) — refused, not mocked"
                    .into(),
            )),
            // The production human verifier refuses every not-yet-configured scheme (refuse-not-mock).
            Err(AuthzError::NotYetImplemented(_)) | Err(AuthzError::Unavailable(_)) => {
                Err(EdgeError::Unavailable(
                    "human login is not configured (JWKS/trust-anchors pending — MR-012); refused, \
                     not mocked"
                        .into(),
                ))
            }
            Err(e) => Err(EdgeError::Unauthorized(format!("login failed: {e:?}"))),
        }
    }

    /// `POST /v1/auth/logout` — clear the session + the cookie. Idempotent.
    fn logout(&self, req: &EdgeRequest) -> EdgeResponse {
        if let Some(sid) = req.cookie(SESSION_COOKIE) {
            self.sessions.remove(&sid);
        }
        EdgeResponse::json(200, &json!({ "ok": true }))
            .with_header("set-cookie", SessionStore::clear_cookie_header())
    }

    /// `POST /v1/auth/refresh` — the backend half of the canon's 401→single-refresh→/login: re-validate
    /// the session's carried bearer. If the bearer is now revoked/expired the refresh fails 401 (the
    /// client then redirects to `/login`); if still valid it returns 200 (the session continues).
    fn refresh(&self, req: &EdgeRequest) -> Result<EdgeResponse, EdgeError> {
        let sid = req
            .cookie(SESSION_COOKIE)
            .ok_or_else(|| EdgeError::Unauthorized("no session cookie to refresh".into()))?;
        let rec = self
            .sessions
            .get(&sid)
            .ok_or_else(|| EdgeError::Unauthorized("session cookie does not resolve a live session".into()))?;
        let cred = Credential { scheme: rec.scheme, material: rec.material };
        self.authn
            .authenticate(&cred, None)
            .map_err(|e| EdgeError::Unauthorized(format!("stale session (re-auth failed): {e:?}")))?;
        Ok(EdgeResponse::json(200, &json!({ "refreshed": true })))
    }

    /// Match `(method, path)` against the registered routes, extracting path params. Total over an
    /// arbitrary path.
    fn match_route(&self, method: Method, path: &str) -> Option<(usize, BTreeMap<String, String>)> {
        let parts: Vec<&str> = path
            .trim_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();
        'routes: for (i, r) in self.routes.iter().enumerate() {
            if r.method != method || r.segs.len() != parts.len() {
                continue;
            }
            let mut params = BTreeMap::new();
            for (seg, part) in r.segs.iter().zip(parts.iter()) {
                match seg {
                    Seg::Lit(l) => {
                        if l != part {
                            continue 'routes;
                        }
                    }
                    Seg::Param(name) => {
                        params.insert(name.clone(), (*part).to_string());
                    }
                }
            }
            return Some((i, params));
        }
        None
    }
}

/// The map from a `PrincipalKind` to a PII-free label for the whoami view-model.
fn kind_label(kind: &PrincipalKind) -> &'static str {
    match kind {
        PrincipalKind::Human => "human",
        PrincipalKind::Agent { .. } => "agent",
        PrincipalKind::Service => "service",
    }
}

/// **The ONE trivial proof handler (MR-014): `whoami`.** Returns the VERIFIED principal + the SET
/// tenant scope as a JSON view-model — proving the lifecycle authenticated, resolved the principal,
/// and set the tenant scope before dispatch. The per-subsystem handlers are MR-015+; this is the
/// gateway's own coherence proof.
pub struct WhoamiHandler;

impl Handler for WhoamiHandler {
    fn handle(&self, ctx: &HandlerCtx<'_>) -> Result<EdgeResponse, EdgeError> {
        Ok(EdgeResponse::json(
            200,
            &json!({
                "principal_id": ctx.principal.principal_id.0,
                "tenant": ctx.scope.tenant().0,
                "region": ctx.scope.region().0,
                "kind": kind_label(&ctx.principal.kind),
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::AllowAll;
    use myelin_identity_service::{
        CapabilityAuthenticator, CellTokenAuthority, HumanSsoAuthenticator, PasetoCapabilityVerifier,
        PrincipalStore, RevocationStore,
    };
    use myelin_storage::KmsEngine;

    fn human_login() -> Arc<HumanSsoAuthenticator> {
        Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(Arc::new(
            KmsEngine::new(),
        ))))
    }

    fn authn_empty() -> Arc<CapabilityAuthenticator> {
        // The REAL PASETO verifier over a fresh cell (no token seeded — used for the no-token /
        // malformed-route / forged-token seam tests; a forged token cannot verify against the cell).
        let cell = CellTokenAuthority::from_seed(&[7u8; 32], &[9u8; 32]).expect("cell");
        Arc::new(CapabilityAuthenticator::with_verifier(
            PrincipalStore::new(Arc::new(KmsEngine::new())),
            Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
            RevocationStore::new(),
        ))
    }

    fn gw() -> Gateway {
        Gateway::builder(authn_empty(), human_login(), Arc::new(AllowAll))
            .route(Method::Get, "/v1/whoami", "edge.whoami", Arc::new(WhoamiHandler))
            .build()
    }

    #[test]
    fn unsupported_method_is_a_clean_400_no_panic() {
        let resp = gw().handle(EdgeRequest::new("TRACE", "/v1/whoami", "", vec![], vec![]));
        assert_eq!(resp.status(), 400);
        assert_eq!(resp.json_body().unwrap()["error"]["code"], "bad_request");
    }

    #[test]
    fn unknown_route_is_404() {
        let resp = gw().handle(EdgeRequest::new("GET", "/v1/nope", "", vec![], vec![]));
        assert_eq!(resp.status(), 404);
    }

    #[test]
    fn no_credential_is_401_with_the_envelope() {
        let resp = gw().handle(EdgeRequest::new("GET", "/v1/whoami", "", vec![], vec![]));
        assert_eq!(resp.status(), 401);
        let b = resp.json_body().unwrap();
        assert_eq!(b["error"]["message"], "authentication required");
        assert_eq!(b["error"]["code"], "unauthorized");
    }

    #[test]
    fn forged_bearer_is_401_never_resolves() {
        // A hand-rolled plaintext envelope the REAL PASETO verifier rejects (not a signed v4.public).
        let resp = gw().handle(EdgeRequest::new(
            "GET",
            "/v1/whoami",
            "",
            vec![
                ("authorization".into(), "Bearer acme|eu-west|subj|jti|0|".into()),
                ("x-myelin-token-scheme".into(), "agent".into()),
            ],
            vec![],
        ));
        assert_eq!(resp.status(), 401, "a forged token never resolves a principal");
    }

    #[test]
    fn login_refuses_not_mocks() {
        // The production human verifier is config-deferred (MR-012) → login REFUSES (503), never a
        // mock session (no Set-Cookie).
        let body = serde_json::to_vec(&json!({"scheme":"oidc","material":"acme|eu-west|subj-1"})).unwrap();
        let resp = gw().handle(EdgeRequest::new("POST", "/v1/auth/login", "", vec![], body));
        assert_eq!(resp.status(), 503, "human login refuses-not-mocks until configured");
    }

    #[test]
    fn malformed_login_body_is_400_no_panic() {
        let resp = gw().handle(EdgeRequest::new("POST", "/v1/auth/login", "", vec![], b"{garbage".to_vec()));
        assert_eq!(resp.status(), 400);
    }
}

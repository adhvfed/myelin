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

use crate::authz::authorize_edge_action;
use crate::catalogue::{Handler, HandlerCtx, Method, Page};
use crate::error::EdgeError;
use crate::request::{EdgeRequest, EdgeResponse};
use crate::session::{SessionStore, SESSION_COOKIE};
use crate::sse::{SseEvent, SseHub};
use myelin_identity::{AuthzError, Credential, Principal, PrincipalKind};
use myelin_identity_service::{CapabilityAuthenticator, HumanSsoAuthenticator, RequestIdentity};
use myelin_storage::TenantScope;
use myelin_substrate::{Authorizer, InjectedIdentity, PublicSurface};
use myelin_tenancy::{Region, TenantId};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;

/// The default credential scheme assumed for a Bearer token when the client sends no
/// `X-Myelin-Token-Scheme` header. `pat` is the primary edge consumer (the UI/CLI cookie gateway
/// carries a PAT bearer server-side).
const DEFAULT_TOKEN_SCHEME: &str = "pat";

/// The bounded SSE scope for a verified tenant — `tenant:<t>` (never a client-supplied `*` selector;
/// the §7.7 "never `*`" stream-IDOR floor). A subsystem publishes to this same key. This scope is
/// legal ONLY for a genuinely tenant-wide stream: an OBJECT-ADDRESSED stream must scope through
/// [`sse_scope_for_resource`] (R2.2 — [`GatewayBuilder::sse_route`] refuses, at registration time,
/// a tenant-coarse route whose pattern addresses an object).
pub fn sse_scope_for_tenant(tenant: &str) -> String {
    format!("tenant:{tenant}")
}

/// The bounded SSE scope for ONE object within a verified tenant —
/// `tenant:<t>/<param>:<id>` (R2.2). `param` is the route's path-parameter NAME (e.g. `repo`) and
/// `id` the matched, [`bounded`](is_bounded_resource_id) value — so a per-resource subscription can
/// never receive another object's frames, and a publisher addressing one object can never fan out
/// tenant-wide. The tenant ALWAYS prefixes the scope (a resource id can never cross tenants), and
/// the `/` separator cannot appear in either side (a path segment never contains `/`; the id is
/// bounded-validated), so the scope grammar is injective.
pub fn sse_scope_for_resource(tenant: &str, param: &str, id: &str) -> String {
    format!("tenant:{tenant}/{param}:{id}")
}

/// **The UNAUTHENTICATED public auth surface (R3.5).** What the logged-out login page needs to render
/// honestly BEFORE any session exists: whether SSO is wired (`sso_configured`), the provider label(s)
/// to name on the primary button, and whether the dev-login seam may render (`dev_login_enabled`).
/// Served by `GET /v1/auth/config` (in the pre-auth built-in route block, so it is reachable with no
/// credential). It carries NOTHING sensitive — the presence of an IdP + a display label + a dev flag,
/// nothing a logged-out attacker could not already infer from the login screen. The real OIDC
/// verification config (issuer/audience/JWKS) is NEVER projected here.
#[derive(Clone, Debug, Default)]
pub struct AuthPublicConfig {
    /// Whether a real OIDC IdP is wired at this edge (drives the primary button: enabled vs the
    /// honest "SSO unavailable" reason).
    pub sso_configured: bool,
    /// The provider(s) to name on the login button (empty when `sso_configured` is false).
    pub providers: Vec<AuthProvider>,
    /// Whether the dev-login seam may render at all (belt-and-braces with the frontend build-time
    /// PROD kill switch). Reflects the edge's `MYELIN_DEV_LOGIN` env at composition time.
    pub dev_login_enabled: bool,
    /// **Whether the operator-TOKEN login seam may render (R4.0 item D).** Gates the (separate, later)
    /// web "paste your `edge bootstrap` token" login the founder uses to sign in before SSO exists.
    /// Reflects the edge's `MYELIN_TOKEN_LOGIN == "1"` env at composition time. Additive + defaults
    /// `false` (a `serde`-additive field: an older client that does not read it is unaffected), so a
    /// prod edge with no operator-login configured projects the honest `false`.
    pub token_login_enabled: bool,
}

/// One login provider projected to the logged-out page — a stable `id` (the credential scheme, e.g.
/// `oidc`) + a human `label` for the button. No secret, no endpoint.
#[derive(Clone, Debug)]
pub struct AuthProvider {
    /// The credential scheme id (e.g. `oidc`).
    pub id: String,
    /// The human-facing button label (e.g. `Single sign-on`).
    pub label: String,
}

/// Current wall-clock as unix millis (the `at` stamp on a lifecycle SSE frame). Dependency-free;
/// a clock skew before the epoch collapses to 0 (the frame's meaning is carried by `type`+`slug`).
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// **The repo-lifecycle → firehose event mapping (R3.5, OQ-4 decision).** A successful (2xx) repo
/// create / wire push becomes a TYPED firehose frame (`repo.created` / `repo.pushed`) so the
/// first-run "waiting for your first push" affordance flips in place. Returns `None` for any other
/// action or a non-2xx response (a failed create must NOT announce a repo). These are TRANSPORT
/// events on the unified firehose, NOT inbox items (your own push is not a notification) — the
/// content policy stays separate from the transport (the gate's OQ-4 "no second channel").
fn repo_lifecycle_event(
    action: &str,
    params: &BTreeMap<String, String>,
    resp: &EdgeResponse,
) -> Option<(&'static str, String)> {
    if !(200..300).contains(&resp.status()) {
        return None;
    }
    match action {
        // The create handler's response carries `applied.slug` — the authoritative created slug.
        "git.repo.create" => resp
            .json_body()
            .and_then(|v| {
                v.get("applied")
                    .and_then(|a| a.get("slug"))
                    .and_then(|s| s.as_str())
                    .map(str::to_string)
            })
            .map(|slug| ("repo.created", slug)),
        // The wire receive-pack (push) route binds `{repo}` in its path — the pushed repo slug.
        "git.wire.receive_pack" => params
            .get("repo")
            .cloned()
            .map(|slug| ("repo.pushed", slug)),
        _ => None,
    }
}

/// Is a client-supplied resource id BOUNDED enough to become part of an SSE scope? (The §7.7
/// "verified tenant + optional bounded resource id" contract.) Non-empty, ≤ 128 bytes, and free of
/// wildcard/whitespace/control/`/` bytes — so a path value can neither smuggle a selector (`*`)
/// nor forge the scope grammar's separator. Fail-closed: an unbounded id is a 400, never a
/// subscription.
fn is_bounded_resource_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.contains('*')
        && !id.contains('/')
        && !id.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// One route segment — a literal, a single-segment `{name}` path parameter, or a trailing
/// **catch-all** `{...name}` that captures the remaining path segments joined by `/` (the nested
/// tree/blob path grammar, R3.4). A `Rest` segment is only meaningful as the LAST segment of a
/// pattern; [`match_route`] enforces that a `Rest` binds every remaining segment (zero or more, so
/// `tree/{ref}` root maps to the same route with an empty path).
enum Seg {
    Lit(String),
    Param(String),
    Rest(String),
}

/// What a matched route dispatches to.
enum RouteKind {
    /// A normal handler (a JSON view-model response).
    Normal(Arc<dyn Handler>),
    /// An SSE stream by name (the real-time convention). `resource_param` is `None` for a
    /// tenant-wide stream (scope = [`sse_scope_for_tenant`]) or `Some(param)` for an
    /// object-addressed stream (scope = [`sse_scope_for_resource`] over the named path param) —
    /// the R2.2 contract: which one a route is, is fixed at REGISTRATION time, so a subsystem
    /// cannot register an object-addressed stream behind a tenant-coarse scope.
    Sse {
        stream: String,
        resource_param: Option<String>,
    },
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
        .map(
            |s| match s.strip_prefix('{').and_then(|x| x.strip_suffix('}')) {
                // `{...name}` = a trailing catch-all that binds the remaining path segments (R3.4).
                Some(name) if name.starts_with("...") => Seg::Rest(name[3..].to_string()),
                Some(name) => Seg::Param(name.to_string()),
                None => Seg::Lit(s.to_string()),
            },
        )
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
    auth_config: AuthPublicConfig,
}

impl GatewayBuilder {
    /// The actions declared by routes registered so far. This is an audit/composition seam used to
    /// prove the final capability catalogue covers the actual router, not a parallel hand-written
    /// list.
    pub fn registered_actions(&self) -> impl Iterator<Item = &str> {
        self.routes.iter().map(|route| route.action.as_str())
    }

    /// Register a normal route (`method` + `pattern` like `/v1/git/repos/{repo}/prs/{n}`), gated by
    /// the re-authorized `action`, dispatching to `handler`.
    ///
    /// **The action gate is NOT an object gate (R2.1).** The authorizer here authorizes the ACTION
    /// (`git.pr.view`) only — it never sees WHICH object the route addresses. An object-addressed
    /// route (`{repo}`, `{channel}`, …) MUST additionally enforce the object-level check at its
    /// registration: wrap the handler in a subsystem object guard that consults the subsystem's
    /// injected object authorizer with the route's declared permission BEFORE the handler runs (the
    /// git precedent: `git_durable.rs::RepoObjectGuard`, declared per-route in
    /// `register_git_durable` — the analogue of the R2.2 `sse_route`/`sse_route_scoped`
    /// registration-time contract). A LIST route must instead prefilter through the Identity
    /// `list_objects` seam (never post-filter). Registering an object-addressed route bare repeats
    /// the R2.1 action-only bypass.
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

    /// Register a TENANT-WIDE SSE route (`GET pattern`) gated by `action`, streaming the `stream`
    /// channel scoped to the verified tenant ([`sse_scope_for_tenant`]).
    ///
    /// **R2.2 registration-time contract:** the pattern may carry NO path parameter other than the
    /// IDOR-check `{tenant}`. A pattern that addresses an object (`…/repos/{repo}/events`) names a
    /// per-object stream, and binding it to the tenant-coarse scope would let every in-tenant
    /// subscriber receive every object's frames regardless of the id in their URL — the stream
    /// analogue of the bare-trailing-id check defect. Such a route MUST register through
    /// [`GatewayBuilder::sse_route_scoped`]; this method PANICS at composition time (a
    /// mis-registered stream is a boot-time bug, never a live one).
    pub fn sse_route(
        mut self,
        pattern: &str,
        action: impl Into<String>,
        stream: impl Into<String>,
    ) -> GatewayBuilder {
        let segs = parse_pattern(pattern);
        let object_params: Vec<&str> = segs
            .iter()
            .filter_map(|s| match s {
                Seg::Param(name) if name != "tenant" => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            object_params.is_empty(),
            "sse_route(`{pattern}`): the pattern addresses object(s) {object_params:?} but binds \
             the TENANT-COARSE scope — every in-tenant subscriber would receive every object's \
             frames (stream IDOR, R2.2). Register an object-addressed stream with \
             sse_route_scoped(pattern, action, stream, resource_param) so the subscription scope \
             is bound to the verified tenant + the bounded resource id."
        );
        self.routes.push(Route {
            method: Method::Get,
            segs,
            action: action.into(),
            kind: RouteKind::Sse {
                stream: stream.into(),
                resource_param: None,
            },
        });
        self
    }

    /// Register an OBJECT-ADDRESSED SSE route (`GET pattern`) gated by `action`, streaming the
    /// `stream` channel scoped to the verified tenant + the bounded resource id taken from the
    /// `{resource_param}` path parameter ([`sse_scope_for_resource`]) — the R2.2 per-object scope
    /// contract. The pattern MUST contain `{resource_param}` (panics at composition time
    /// otherwise); at dispatch, an unbounded id (empty / >128 bytes / wildcard / separator bytes)
    /// is a 400, never a subscription.
    ///
    /// NOTE: the action-level `authorize(principal, action)` gate still runs on every subscribe;
    /// if the streamed frames are more sensitive than the action grant implies, the registering
    /// subsystem must ALSO thread the object-level `IdentityService::check` at its publish/route
    /// seam (the same check the JSON routes use) — the scope binding here guarantees isolation
    /// between objects, not object-level grant semantics.
    pub fn sse_route_scoped(
        mut self,
        pattern: &str,
        action: impl Into<String>,
        stream: impl Into<String>,
        resource_param: &str,
    ) -> GatewayBuilder {
        let segs = parse_pattern(pattern);
        assert!(
            resource_param != "tenant",
            "sse_route_scoped(`{pattern}`): `tenant` is the IDOR-check parameter, not a resource \
             id — the tenant already prefixes every SSE scope"
        );
        assert!(
            segs.iter()
                .any(|s| matches!(s, Seg::Param(name) if name == resource_param)),
            "sse_route_scoped(`{pattern}`): the pattern does not carry the `{{{resource_param}}}` \
             path parameter the subscription scope is bound to (R2.2 — an object-addressed stream \
             must derive its scope from the matched object id)"
        );
        self.routes.push(Route {
            method: Method::Get,
            segs,
            action: action.into(),
            kind: RouteKind::Sse {
                stream: stream.into(),
                resource_param: Some(resource_param.to_string()),
            },
        });
        self
    }

    /// Override the default Bearer-token scheme (when the client sends no `X-Myelin-Token-Scheme`).
    pub fn default_token_scheme(mut self, scheme: impl Into<String>) -> GatewayBuilder {
        self.default_scheme = scheme.into();
        self
    }

    /// Set the UNAUTHENTICATED public auth surface (`GET /v1/auth/config`) — the composition root
    /// derives it from the OIDC config + the dev-login env (R3.5). Default = SSO not configured,
    /// dev-login off (the fail-closed default: a login page over this default shows the honest
    /// "SSO unavailable" reason and no dev seam).
    pub fn with_auth_config(mut self, cfg: AuthPublicConfig) -> GatewayBuilder {
        self.auth_config = cfg;
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
            auth_config: self.auth_config,
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
    auth_config: AuthPublicConfig,
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
            auth_config: AuthPublicConfig::default(),
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
            Err(e) => {
                let mut resp = EdgeResponse::error(&e);
                // F1 (R4.1 dogfood) — the git smart-HTTP wire's HTTP-Basic challenge. A real `git
                // push`/`clone` over a credential helper/manager/interactive prompt only OFFERS its
                // Basic credential AFTER a 401 that carries `WWW-Authenticate: Basic …`; without the
                // header git never sends the credential and the push fails auth (proven: `curl -u`
                // and preemptive `http.extraHeader` both work, a helper-driven push did not). Emit the
                // challenge on a git-wire route 401 ONLY — NEVER on the JSON product API (a browser
                // Basic popup on `/v1/…` would wreck the cookie-session web login), NEVER on 200/403
                // (a successful or Bearer/Basic-authenticated wire call is untouched). The 401 body
                // stays byte-identical + oracle-free — only this header is added.
                if e.status() == 401 && self.is_git_wire_route(&req) {
                    resp = resp.with_header("WWW-Authenticate", r#"Basic realm="Myelin""#);
                }
                resp
            }
        }
    }

    /// **F1 — is this request's path a registered git smart-HTTP WIRE route?** Re-runs the SAME
    /// method-parse + route match the lifecycle uses and checks the matched route's declared action
    /// verb (`git.wire.*`) — the exact gate `handle_inner` computes `allow_basic` from (~L449), so the
    /// Basic-challenge scope can never drift onto a non-wire route. TOTAL over attacker bytes: an
    /// unparseable method or an unmatched path is simply `false` (no challenge). This is a pure
    /// path/action classification — it does NOT re-authenticate or weaken any check.
    fn is_git_wire_route(&self, req: &EdgeRequest) -> bool {
        let Some(method) = Method::parse(&req.method) else {
            return false;
        };
        match self.match_route(method, &req.path) {
            Some((idx, _)) => self.routes[idx].action.starts_with("git.wire."),
            None => false,
        }
    }

    fn handle_inner(&self, req: &EdgeRequest) -> Result<EdgeResponse, EdgeError> {
        let method = Method::parse(&req.method)
            .ok_or_else(|| EdgeError::BadRequest(format!("unsupported method `{}`", req.method)))?;

        // Built-in auth routes (the cookie-session machinery; login refuses-not-mocks).
        // `/v1/auth/config` is UNAUTHENTICATED by construction — it is matched here, BEFORE the
        // authenticate step, exactly like login/logout/refresh, because the logged-out login page
        // must read it with no session (R3.5 / OQ-3). It projects nothing sensitive.
        match (method, req.path.as_str()) {
            (Method::Get, "/v1/auth/config") => return Ok(self.auth_config_response()),
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
        let path_region = params.get("region").map(|r| Region(r.clone()));

        // R4.0 item C: the git smart-HTTP WIRE routes (and ONLY those) additionally accept HTTP Basic
        // (git's native `http://…` credential shape), where the password is the capability token. The
        // JSON product API stays Bearer-only. The gate is the route's declared action verb, so the
        // Basic seam can never leak onto a non-wire route.
        let allow_basic = route.action.starts_with("git.wire.");

        // (1) authenticate → the complete verified identity (uniform 401 on ANY failure).
        let identity = self.authenticate(req, path_tenant.as_ref(), allow_basic)?;
        // (2) resolve + set the tenant scope (reject + audit a cross-tenant path here).
        let scope = self.resolve_scope(
            &identity.principal,
            path_tenant.as_ref(),
            path_region.as_ref(),
        )?;
        debug_assert_eq!(scope, identity.scope);
        // A receive-pack advertisement shares Git's GET info/refs route with upload-pack. Select
        // the stronger signed capability before dispatch; the handler's object Write check remains
        // the independent second conjunct.
        let authorization_action = if route.action == "git.wire.upload_pack"
            && req.query_param("service").as_deref() == Some("git-receive-pack")
        {
            "git.wire.receive_pack"
        } else {
            route.action.as_str()
        };
        // (3) re-authorize the action (fail-closed → 403). Signed Edge audience, purpose, and
        // capability authority are conjunctive with the injected verb policy on EVERY call.
        if !authorize_edge_action(self.authorizer.as_ref(), &identity, authorization_action) {
            return Err(EdgeError::Forbidden(format!(
                "authorization denied for action `{}`",
                authorization_action
            )));
        }
        // (4) dispatch.
        match &route.kind {
            RouteKind::Normal(handler) => {
                let page = Page::from_request(req);
                let ctx = HandlerCtx {
                    identity: &identity,
                    principal: &identity.principal,
                    scope: &scope,
                    params: &params,
                    page: &page,
                    request: req,
                };
                // Dispatch first; an error short-circuits (no lifecycle announce on a failed write).
                let resp = handler.handle(&ctx)?;
                // R3.5 (OQ-4): on a successful repo create / wire push, publish a TYPED frame onto
                // the SAME tenant firehose (`edge` stream) the UI already subscribes to — no second
                // channel. This is what makes the first-run "waiting for your first push" affordance
                // flip in place.
                self.broadcast_repo_lifecycle(&route.action, &params, &scope, &resp);
                Ok(resp)
            }
            RouteKind::Sse {
                stream,
                resource_param,
            } => {
                // The subscription scope is ALWAYS derived, never a client selector: the VERIFIED
                // tenant, plus — for an object-addressed route (R2.2) — the bounded resource id
                // from the matched path. The registration-time contract (sse_route vs
                // sse_route_scoped) fixed WHICH of the two this route is.
                let sse_scope = match resource_param {
                    None => sse_scope_for_tenant(&scope.tenant().0),
                    Some(param) => {
                        // The param is present by construction (registration asserted it is in
                        // the pattern; the route only matched with every segment bound) — but
                        // fail CLOSED, never coarse, if that invariant is ever violated.
                        let id = params.get(param).ok_or_else(|| {
                            EdgeError::BadRequest(format!(
                                "SSE route is scoped by `{{{param}}}` but the match bound no \
                                 such parameter"
                            ))
                        })?;
                        if !is_bounded_resource_id(id) {
                            return Err(EdgeError::BadRequest(format!(
                                "SSE resource id for `{{{param}}}` is not a bounded id"
                            )));
                        }
                        sse_scope_for_resource(&scope.tenant().0, param, id)
                    }
                };
                let sub = self.sse.subscribe(stream, &sse_scope);
                Ok(EdgeResponse::sse(sub))
            }
        }
    }

    /// Authenticate a request to a complete [`RequestIdentity`]: Bearer capability token first,
    /// then a capability-backed session cookie.
    /// ANY failure (forged/expired/revoked/absent) collapses to a uniform [`EdgeError::Unauthorized`]
    /// (401) — the client cannot distinguish the cause (oracle-free). The verified principal's
    /// tenant/region is the request scope; a forged token never resolves a Principal (real PASETO).
    fn authenticate(
        &self,
        req: &EdgeRequest,
        path_tenant: Option<&TenantId>,
        allow_basic: bool,
    ) -> Result<RequestIdentity, EdgeError> {
        let scheme_of = || {
            req.header("x-myelin-token-scheme")
                .unwrap_or(&self.default_scheme)
                .to_string()
        };
        if let Some(material) = req.bearer() {
            let cred = Credential {
                scheme: scheme_of(),
                material: material.to_string(),
            };
            return self
                .authn
                .authenticate_identity(&cred, path_tenant)
                .map_err(|e| EdgeError::Unauthorized(format!("bearer auth failed: {e:?}")));
        }
        // R4.0 item C — git smart-HTTP WIRE ONLY: accept HTTP Basic where the PASSWORD is the
        // capability-token material (the username is ignored). SAME verify path as Bearer (same scheme
        // resolution, same `authn.authenticate`), so a valid token authorizes identically. A malformed/
        // empty Basic header is `None` and falls through to the uniform missing-credential 401 below —
        // no distinct oracle. The token is never logged (the error carries the AuthzError, not the
        // material). Bearer callers are entirely unaffected (this arm is reached only when NO Bearer).
        if allow_basic {
            if let Some(material) = req.basic_password() {
                let cred = Credential {
                    scheme: scheme_of(),
                    material,
                };
                return self
                    .authn
                    .authenticate_identity(&cred, path_tenant)
                    .map_err(|e| EdgeError::Unauthorized(format!("basic auth failed: {e:?}")));
            }
        }
        if let Some(sid) = req.cookie(SESSION_COOKIE) {
            // The session carries the Bearer server-side (never exposed to client JS).
            if let Some(rec) = self.sessions.get(&sid) {
                let cred = Credential {
                    scheme: rec.scheme,
                    material: rec.material,
                };
                return self
                    .authn
                    .authenticate_identity(&cred, path_tenant)
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
        path_region: Option<&Region>,
    ) -> Result<TenantScope, EdgeError> {
        if let Some(pt) = path_tenant {
            let id = InjectedIdentity::new(principal.clone());
            self.public_surface
                .resolve_tenant(&id, pt)
                .map_err(|reject| {
                    EdgeError::Forbidden(format!("cross-tenant IDOR rejected: {reject}"))
                })?;
        }
        if let Some(path_region) = path_region {
            if path_region != &principal.region {
                // PublicSurface currently exposes only a tenant-IDOR audit carrier. Refuse the
                // region mismatch here before authorization/dispatch; adding a region audit record
                // requires extending that substrate API rather than mislabelling it as a tenant IDOR.
                return Err(EdgeError::Forbidden(format!(
                    "cross-region scope rejected: path region `{}` does not match verified region",
                    path_region.0
                )));
            }
        }
        Ok(TenantScope::from_verified_token(
            principal,
            principal.region.clone(),
        ))
    }

    /// `GET /v1/auth/config` (UNAUTHENTICATED) — the logged-out login page's honest render source
    /// (R3.5): `{ sso_configured, providers:[{id,label}], dev_login_enabled }`. Projects nothing
    /// sensitive (see [`AuthPublicConfig`]).
    fn auth_config_response(&self) -> EdgeResponse {
        let providers: Vec<serde_json::Value> = self
            .auth_config
            .providers
            .iter()
            .map(|p| json!({ "id": p.id, "label": p.label }))
            .collect();
        EdgeResponse::json(
            200,
            &json!({
                "sso_configured": self.auth_config.sso_configured,
                "providers": providers,
                "dev_login_enabled": self.auth_config.dev_login_enabled,
                "token_login_enabled": self.auth_config.token_login_enabled,
            }),
        )
    }

    /// Publish a repo-lifecycle frame ([`repo_lifecycle_event`]) to the tenant firehose after a
    /// successful create/push — the R3.5 (OQ-4) unified-transport wiring. Scope is ALWAYS the
    /// gateway-derived verified tenant (never a client selector); a frame with no live subscriber is
    /// dropped (the ephemeral firehose posture — durability rides the resume-cursor seam).
    fn broadcast_repo_lifecycle(
        &self,
        action: &str,
        params: &BTreeMap<String, String>,
        scope: &TenantScope,
        resp: &EdgeResponse,
    ) {
        let Some((event_type, slug)) = repo_lifecycle_event(action, params, resp) else {
            return;
        };
        let tenant = &scope.tenant().0;
        let data = json!({ "type": event_type, "slug": slug, "at": now_millis() }).to_string();
        self.sse.broadcast(
            "edge",
            &sse_scope_for_tenant(tenant),
            SseEvent::typed(event_type, data),
        );
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
        let cred = Credential {
            scheme: scheme.to_string(),
            material: material.to_string(),
        };
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
        let rec = self.sessions.get(&sid).ok_or_else(|| {
            EdgeError::Unauthorized("session cookie does not resolve a live session".into())
        })?;
        let cred = Credential {
            scheme: rec.scheme,
            material: rec.material,
        };
        self.authn.authenticate_identity(&cred, None).map_err(|e| {
            EdgeError::Unauthorized(format!("stale session (re-auth failed): {e:?}"))
        })?;
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
            if r.method != method {
                continue;
            }
            // A trailing catch-all (`{...name}`, R3.4) binds zero-or-more remaining segments, so its
            // route matches any path with at least the fixed prefix length; a fixed route still needs
            // an exact segment-count match.
            let has_rest = matches!(r.segs.last(), Some(Seg::Rest(_)));
            if has_rest {
                if parts.len() + 1 < r.segs.len() {
                    continue; // not even the fixed prefix (before the catch-all) is present.
                }
            } else if r.segs.len() != parts.len() {
                continue;
            }
            let mut params = BTreeMap::new();
            for (idx, seg) in r.segs.iter().enumerate() {
                match seg {
                    Seg::Lit(l) => {
                        if parts.get(idx) != Some(&l.as_str()) {
                            continue 'routes;
                        }
                    }
                    Seg::Param(name) => match parts.get(idx) {
                        Some(part) => {
                            params.insert(name.clone(), (*part).to_string());
                        }
                        None => continue 'routes,
                    },
                    // The catch-all: everything from here on, joined by `/` (empty for the root path).
                    Seg::Rest(name) => {
                        let rest = parts[idx..].join("/");
                        params.insert(name.clone(), rest);
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
        CapabilityAuthenticator, CellTokenAuthority, HumanSsoAuthenticator,
        PasetoCapabilityVerifier, PrincipalStore, RevocationStore,
    };
    use myelin_storage::KmsEngine;

    fn human_login() -> Arc<HumanSsoAuthenticator> {
        Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(
            Arc::new(KmsEngine::new()),
        )))
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
            .route(
                Method::Get,
                "/v1/whoami",
                "edge.whoami",
                Arc::new(WhoamiHandler),
            )
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

    /// R3.4: `{...name}` parses to a trailing catch-all [`Seg::Rest`]; `{name}` stays a single-segment
    /// param and a literal stays a literal (the nested tree/blob path grammar).
    #[test]
    fn parse_pattern_recognizes_the_catch_all_rest_segment() {
        let segs = parse_pattern("/v1/git/repos/{repo}/tree/{ref}/{...path}");
        assert!(matches!(segs.last(), Some(Seg::Rest(n)) if n == "path"));
        assert!(matches!(&segs[3], Seg::Param(n) if n == "repo"));
        assert!(matches!(&segs[4], Seg::Lit(l) if l == "tree"));
        // A plain `{path}` is NOT a catch-all.
        let single = parse_pattern("/v1/git/repos/{repo}/blob/{ref}/{path}");
        assert!(matches!(single.last(), Some(Seg::Param(n)) if n == "path"));
    }

    #[test]
    fn no_credential_is_401_with_the_envelope() {
        let resp = gw().handle(EdgeRequest::new("GET", "/v1/whoami", "", vec![], vec![]));
        assert_eq!(resp.status(), 401);
        let b = resp.json_body().unwrap();
        assert_eq!(b["error"]["message"], "authentication required");
        assert_eq!(b["error"]["code"], "unauthorized");
    }

    /// The `WWW-Authenticate` header on an [`EdgeResponse`], if present (case-insensitive) — the
    /// seam-level accessor for the F1 challenge assertions.
    fn www_authenticate(resp: &EdgeResponse) -> Option<String> {
        match resp {
            EdgeResponse::Bytes { headers, .. } | EdgeResponse::Sse { headers, .. } => headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("www-authenticate"))
                .map(|(_, v)| v.clone()),
        }
    }

    /// A gateway carrying BOTH a JSON product route (`/v1/whoami`) and a git smart-HTTP WIRE route
    /// (a `git.wire.*` action) — the fixture for the F1 challenge-scoping assertions.
    fn gw_with_wire() -> Gateway {
        Gateway::builder(authn_empty(), human_login(), Arc::new(AllowAll))
            .route(
                Method::Get,
                "/v1/whoami",
                "edge.whoami",
                Arc::new(WhoamiHandler),
            )
            .route(
                Method::Get,
                "/{tenant}/{region}/{repo}/info/refs",
                "git.wire.upload_pack",
                Arc::new(WhoamiHandler),
            )
            .build()
    }

    /// **F1 (R4.1 dogfood) — a git-wire 401 carries the HTTP Basic challenge; the JSON API 401 does
    /// NOT.** A real `git push/clone` over a credential helper only OFFERS its Basic credential after a
    /// 401 that names `WWW-Authenticate: Basic …`; without it git never sends the credential and the
    /// operation fails auth. The challenge MUST be scoped to git-wire routes: a browser Basic popup on
    /// the JSON product API (the cookie-session web login surface) would wreck the UX.
    #[test]
    fn f1_git_wire_401_carries_basic_challenge_json_api_does_not() {
        let gw = gw_with_wire();

        // (i) a git-wire route with NO credential → 401 WITH the challenge.
        let wire = gw.handle(EdgeRequest::new(
            "GET",
            "/acme/eu-west/widgets.git/info/refs",
            "service=git-upload-pack",
            vec![],
            vec![],
        ));
        assert_eq!(
            wire.status(),
            401,
            "an unauthenticated wire request is a 401"
        );
        assert_eq!(
            www_authenticate(&wire).as_deref(),
            Some(r#"Basic realm="Myelin""#),
            "the git-wire 401 MUST carry the Basic challenge so git offers its credential (F1)"
        );
        // The body stays the oracle-free uniform envelope — only the header was added.
        assert_eq!(
            wire.json_body().unwrap()["error"]["message"],
            "authentication required"
        );

        // (ii) the JSON product API with NO credential → 401 WITHOUT the challenge (no Basic popup).
        let json = gw.handle(EdgeRequest::new("GET", "/v1/whoami", "", vec![], vec![]));
        assert_eq!(json.status(), 401);
        assert_eq!(
            www_authenticate(&json),
            None,
            "the JSON API 401 must NOT carry a Basic challenge (would break web login)"
        );

        // (iii) a forged Bearer on the wire is still a 401 — the challenge is keyed on 401-ness (an
        // authenticated 200/403 carries none; proven end-to-end in the real-git dogfood oracle).
        let forged = gw.handle(EdgeRequest::new(
            "GET",
            "/acme/eu-west/widgets.git/info/refs",
            "service=git-upload-pack",
            vec![("authorization".into(), "Bearer acme|eu-west|s|j|0|".into())],
            vec![],
        ));
        assert_eq!(forged.status(), 401);
        assert_eq!(
            www_authenticate(&forged).as_deref(),
            Some(r#"Basic realm="Myelin""#)
        );

        // A non-existent (non-wire) path 404 carries no challenge (is_git_wire_route is false).
        let miss = gw.handle(EdgeRequest::new("GET", "/v1/nope", "", vec![], vec![]));
        assert_eq!(miss.status(), 404);
        assert_eq!(www_authenticate(&miss), None);
    }

    #[test]
    fn forged_bearer_is_401_never_resolves() {
        // A hand-rolled plaintext envelope the REAL PASETO verifier rejects (not a signed v4.public).
        let resp = gw().handle(EdgeRequest::new(
            "GET",
            "/v1/whoami",
            "",
            vec![
                (
                    "authorization".into(),
                    "Bearer acme|eu-west|subj|jti|0|".into(),
                ),
                ("x-myelin-token-scheme".into(), "agent".into()),
            ],
            vec![],
        ));
        assert_eq!(
            resp.status(),
            401,
            "a forged token never resolves a principal"
        );
    }

    // ── R3.5 — the unauthenticated public auth surface + the repo-lifecycle firehose wiring ──

    /// `GET /v1/auth/config` is reachable with NO credential (it is matched in the pre-auth built-in
    /// block) and projects the `{ sso_configured, providers, dev_login_enabled }` shape the
    /// logged-out login page needs.
    #[test]
    fn auth_config_is_unauthenticated_and_reports_the_shape() {
        let gw = Gateway::builder(authn_empty(), human_login(), Arc::new(AllowAll))
            .with_auth_config(AuthPublicConfig {
                sso_configured: true,
                providers: vec![AuthProvider {
                    id: "oidc".into(),
                    label: "Single sign-on".into(),
                }],
                dev_login_enabled: false,
                token_login_enabled: true,
            })
            .build();
        // No Authorization header, no session cookie — still 200 (unauthenticated by construction).
        let resp = gw.handle(EdgeRequest::new(
            "GET",
            "/v1/auth/config",
            "",
            vec![],
            vec![],
        ));
        assert_eq!(
            resp.status(),
            200,
            "auth/config must be reachable logged-out"
        );
        let b = resp.json_body().unwrap();
        assert_eq!(b["sso_configured"], true);
        assert_eq!(b["dev_login_enabled"], false);
        assert_eq!(b["token_login_enabled"], true);
        assert_eq!(b["providers"][0]["id"], "oidc");
        assert_eq!(b["providers"][0]["label"], "Single sign-on");
    }

    /// `dev_login_enabled` faithfully reflects the composed config (env-driven at the composition
    /// root) — here the two opposite values, proving the field is not hardcoded.
    #[test]
    fn auth_config_dev_login_flag_reflects_the_composed_value() {
        for enabled in [true, false] {
            let gw = Gateway::builder(authn_empty(), human_login(), Arc::new(AllowAll))
                .with_auth_config(AuthPublicConfig {
                    sso_configured: false,
                    providers: vec![],
                    dev_login_enabled: enabled,
                    token_login_enabled: false,
                })
                .build();
            let resp = gw.handle(EdgeRequest::new(
                "GET",
                "/v1/auth/config",
                "",
                vec![],
                vec![],
            ));
            assert_eq!(resp.json_body().unwrap()["dev_login_enabled"], enabled);
        }
        // The default (no `.with_auth_config`) is fail-closed: SSO off, dev seam off.
        let dflt = gw().handle(EdgeRequest::new(
            "GET",
            "/v1/auth/config",
            "",
            vec![],
            vec![],
        ));
        let b = dflt.json_body().unwrap();
        assert_eq!(b["sso_configured"], false);
        assert_eq!(b["dev_login_enabled"], false);
    }

    /// The repo-lifecycle event mapping: a 2xx create → `repo.created` (slug from the response
    /// `applied.slug`); a 2xx wire push → `repo.pushed` (slug from the `{repo}` path param); a
    /// non-2xx or unrelated action → no frame (a failed create must NOT announce a repo).
    #[test]
    fn repo_lifecycle_event_maps_create_and_push_only_on_success() {
        let created = EdgeResponse::json(
            201,
            &json!({ "applied": { "action": "git.repo.create", "slug": "widgets" }, "durable": true }),
        );
        let empty = BTreeMap::new();
        assert_eq!(
            repo_lifecycle_event("git.repo.create", &empty, &created),
            Some(("repo.created", "widgets".to_string()))
        );

        let mut params = BTreeMap::new();
        params.insert("repo".to_string(), "widgets".to_string());
        let pushed = EdgeResponse::json(200, &json!({ "ok": true }));
        assert_eq!(
            repo_lifecycle_event("git.wire.receive_pack", &params, &pushed),
            Some(("repo.pushed", "widgets".to_string()))
        );

        // A FAILED create (non-2xx) announces nothing.
        let conflict = EdgeResponse::json(409, &json!({ "error": { "message": "exists" } }));
        assert_eq!(
            repo_lifecycle_event("git.repo.create", &empty, &conflict),
            None
        );
        // An unrelated action never announces.
        assert_eq!(repo_lifecycle_event("git.pr.view", &empty, &created), None);
    }

    /// End-to-end wiring: a successful create dispatched through the gateway BROADCASTS a
    /// `repo.created` typed frame onto the tenant firehose (`edge` stream, `tenant:<t>` scope) — the
    /// exact channel the first-run "waiting for your first push" affordance subscribes to.
    #[test]
    fn create_broadcasts_repo_created_on_the_tenant_firehose() {
        let gw = gw();
        // A subscriber on the acme tenant firehose (the same key the SSE route derives).
        let mut rx = gw
            .sse_hub()
            .subscribe("edge", &sse_scope_for_tenant("acme"))
            .into_receiver();
        // Drive the post-dispatch broadcast directly (the same call `handle_inner` makes after a
        // successful Normal dispatch), with a real verified-token tenant scope.
        let scope = TenantScope::from_verified_token(
            &Principal::stub(
                myelin_identity::PrincipalId("creator".into()),
                PrincipalKind::Human,
                TenantId("acme".into()),
            ),
            myelin_tenancy::Region("eu-west".into()),
        );
        let resp = EdgeResponse::json(
            201,
            &json!({ "applied": { "action": "git.repo.create", "slug": "widgets" } }),
        );
        gw.broadcast_repo_lifecycle("git.repo.create", &BTreeMap::new(), &scope, &resp);

        let frame = rx
            .try_recv()
            .expect("a repo.created frame reached the tenant subscriber");
        assert_eq!(frame.event.as_deref(), Some("repo.created"));
        let data: serde_json::Value = serde_json::from_str(&frame.data).unwrap();
        assert_eq!(data["type"], "repo.created");
        assert_eq!(data["slug"], "widgets");
    }

    #[test]
    fn login_refuses_not_mocks() {
        // The production human verifier is config-deferred (MR-012) → login REFUSES (503), never a
        // mock session (no Set-Cookie).
        let body =
            serde_json::to_vec(&json!({"scheme":"oidc","material":"acme|eu-west|subj-1"})).unwrap();
        let resp = gw().handle(EdgeRequest::new("POST", "/v1/auth/login", "", vec![], body));
        assert_eq!(
            resp.status(),
            503,
            "human login refuses-not-mocks until configured"
        );
    }

    #[test]
    fn malformed_login_body_is_400_no_panic() {
        let resp = gw().handle(EdgeRequest::new(
            "POST",
            "/v1/auth/login",
            "",
            vec![],
            b"{garbage".to_vec(),
        ));
        assert_eq!(resp.status(), 400);
    }

    // ── R2.2: the SSE scope registration-time contract ──────────────────────────────────────────

    /// **R2.2 Defect C — a resource-addressed SSE route CANNOT collapse to tenant-only scope.**
    /// Registering a pattern that addresses an object (`{repo}`) through the tenant-coarse
    /// `sse_route` is refused at COMPOSITION time (the boot panics; the stream-IDOR route never
    /// exists). The object-addressed registration path is `sse_route_scoped`.
    #[test]
    #[should_panic(expected = "sse_route_scoped")]
    fn object_addressed_pattern_cannot_register_tenant_coarse() {
        let _ = Gateway::builder(authn_empty(), human_login(), Arc::new(AllowAll)).sse_route(
            "/v1/t/{tenant}/repos/{repo}/events",
            "git.repo.events.subscribe",
            "git",
        );
    }

    /// The scoped registration demands the pattern actually carry the parameter the scope binds
    /// to — a typo'd/absent `{resource_param}` is a composition-time panic, not a silently
    /// tenant-coarse stream.
    #[test]
    #[should_panic(expected = "does not carry")]
    fn scoped_route_requires_the_resource_param_in_the_pattern() {
        let _ = Gateway::builder(authn_empty(), human_login(), Arc::new(AllowAll))
            .sse_route_scoped(
                "/v1/t/{tenant}/events",
                "git.repo.events.subscribe",
                "git",
                "repo",
            );
    }

    /// `tenant` is the IDOR-check parameter, never a resource id — binding the scope to it is
    /// refused (the tenant already prefixes every scope).
    #[test]
    #[should_panic(expected = "not a resource id")]
    fn scoped_route_refuses_tenant_as_the_resource_param() {
        let _ = Gateway::builder(authn_empty(), human_login(), Arc::new(AllowAll))
            .sse_route_scoped(
                "/v1/t/{tenant}/events",
                "edge.events.subscribe",
                "edge",
                "tenant",
            );
    }

    /// The legitimate registrations still compose: the tenant-wide `{tenant}`-only pattern through
    /// `sse_route` (today's `/v1/t/{tenant}/events`), and an object-addressed pattern through
    /// `sse_route_scoped`.
    #[test]
    fn legitimate_sse_registrations_compose() {
        let _ = Gateway::builder(authn_empty(), human_login(), Arc::new(AllowAll))
            .sse_route("/v1/t/{tenant}/events", "edge.events.subscribe", "edge")
            .sse_route_scoped(
                "/v1/t/{tenant}/repos/{repo}/events",
                "git.repo.events.subscribe",
                "git",
                "repo",
            )
            .build();
    }

    /// The per-object scope grammar is tenant-prefixed and distinct from the tenant-coarse scope
    /// (a per-object publish can never fan out tenant-wide, and vice versa), and the bounded-id
    /// validator refuses selector/separator smuggling.
    #[test]
    fn resource_scope_grammar_is_bounded_and_tenant_prefixed() {
        assert_eq!(sse_scope_for_tenant("acme"), "tenant:acme");
        assert_eq!(
            sse_scope_for_resource("acme", "repo", "widgets"),
            "tenant:acme/repo:widgets"
        );
        assert_ne!(
            sse_scope_for_resource("acme", "repo", "widgets"),
            sse_scope_for_tenant("acme")
        );
        // Bounded ids: a wildcard, a scope-separator, whitespace, empty, oversized — all refused.
        assert!(is_bounded_resource_id("widgets"));
        assert!(is_bounded_resource_id("repo-7_x.y"));
        assert!(!is_bounded_resource_id("*"));
        assert!(!is_bounded_resource_id("a/b"));
        assert!(!is_bounded_resource_id("a b"));
        assert!(!is_bounded_resource_id(""));
        assert!(!is_bounded_resource_id(&"x".repeat(129)));
    }
}

/// R4.0 item C — HTTP Basic → Bearer on the git smart-HTTP WIRE routes ONLY. `git push/clone http://…`
/// sends `Authorization: Basic base64(user:pass)` where the PASSWORD is the capability token; on the
/// git-wire routes that translates to the SAME PASETO verify path as Bearer. The JSON product API stays
/// Bearer-only. These seam-level tests drive the real gateway lifecycle with a real PASETO verifier +
/// a seeded principal (no socket), so the Basic scoping is proven without the runsc wire harness.
#[cfg(test)]
mod git_wire_basic_auth_tests {
    use super::*;
    use crate::authz::AllowAll;
    use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus};
    use myelin_identity_service::{
        CapabilityAuthenticator, CapabilityMintSpec, CellTokenAuthority, HumanSsoAuthenticator,
        PasetoCapabilityVerifier, PrincipalStore, RevocationStore,
    };
    use myelin_storage::KmsEngine;

    const REGION: &str = "eu-west";

    /// A gateway with a seeded `agent`-scheme principal (`subj-1` → `svc:founder`), a whoami (JSON)
    /// route and a git-wire `info/refs` route, plus a freshly-minted token for that principal. The
    /// default token scheme is `agent` (as the production edge sets it), so a bearer/basic credential
    /// with no scheme header resolves under `agent`.
    fn seeded_gateway() -> (Gateway, String) {
        seeded_gateway_with(
            myelin_identity_service::CredentialPurpose::OperatorBootstrap,
            &["edge.operator"],
            "jti-basic",
        )
    }

    fn seeded_gateway_with(
        purpose: myelin_identity_service::CredentialPurpose,
        authority: &[&str],
        jti: &str,
    ) -> (Gateway, String) {
        let cell = CellTokenAuthority::from_seed(&[3u8; 32], &[4u8; 32]).expect("cell");
        let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
        let scope = TenantScope::from_verified_token(
            &Principal::stub(
                PrincipalId("admin".into()),
                PrincipalKind::Human,
                TenantId("acme".into()),
            ),
            myelin_tenancy::Region(REGION.into()),
        );
        store
            .put_principal(
                &scope,
                PrincipalId("svc:founder".into()),
                PrincipalKind::Service,
                DataRole::Controller,
                PrincipalStatus::Active,
                None,
            )
            .expect("seed principal");
        store
            .link_credential(
                &scope,
                "agent",
                "subj-1",
                &PrincipalId("svc:founder".into()),
            )
            .expect("link credential");
        let revocations = RevocationStore::new();
        if purpose.is_run_scoped() {
            revocations.register_run_token_ttl(
                &scope,
                jti,
                myelin_events::Timestamp("2020-01-01T00:00:00Z".into()),
                myelin_events::Timestamp("2099-01-01T00:00:00Z".into()),
            );
        }
        let authn = Arc::new(CapabilityAuthenticator::with_verifier(
            store,
            Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
            revocations,
        ));
        let human = Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(
            Arc::new(KmsEngine::new()),
        )));
        let token = cell.mint(&CapabilityMintSpec {
            tenant: "acme".into(),
            region: REGION.into(),
            subject_key: "subj-1".into(),
            jti: jti.into(),
            exp_unix: 9_999_999_999,
            authority: authority.iter().map(|grant| (*grant).into()).collect(),
            dpop_jkt: None,
            purpose,
            audience: myelin_identity_service::CredentialAudience::Edge,
        });
        let gw = Gateway::builder(authn, human, Arc::new(AllowAll))
            .default_token_scheme("agent")
            .route(
                Method::Get,
                "/v1/whoami",
                "edge.whoami",
                Arc::new(WhoamiHandler),
            )
            .route(
                Method::Get,
                "/{tenant}/{region}/{repo}/info/refs",
                "git.wire.upload_pack",
                Arc::new(WhoamiHandler),
            )
            .build();
        (gw, token)
    }

    fn basic_header(user: &str, pass: &str) -> (String, String) {
        use base64::Engine as _;
        let b64 =
            base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}").as_bytes());
        ("authorization".to_string(), format!("Basic {b64}"))
    }

    const WIRE: &str = "/acme/eu-west/widgets/info/refs";

    /// **Basic auth (password = token) authorizes on the git wire — the SAME verify path as Bearer.**
    #[test]
    fn basic_password_token_authorizes_on_the_git_wire() {
        let (gw, token) = seeded_gateway();
        let resp = gw.handle(EdgeRequest::new(
            "GET",
            WIRE,
            "",
            vec![basic_header("x-access-token", &token)],
            vec![],
        ));
        assert_eq!(
            resp.status(),
            200,
            "the git wire accepts HTTP Basic where the password is the capability token"
        );
    }

    /// **Malformed / empty Basic is the SAME uniform 401 as a missing credential (no distinct oracle).**
    #[test]
    fn basic_garbage_is_same_as_missing_401() {
        let (gw, _t) = seeded_gateway();
        let cases: Vec<(String, String)> = vec![
            ("authorization".into(), "Basic !!!not-base64".into()), // malformed base64
            ("authorization".into(), "Basic ".into()),              // empty
            basic_header("user", ""),                               // empty password
            basic_header("user-no-colon", "x"), // (has colon via helper; sanity path still 401 unless token)
        ];
        for h in cases {
            let resp = gw.handle(EdgeRequest::new("GET", WIRE, "", vec![h], vec![]));
            assert_eq!(
                resp.status(),
                401,
                "malformed/empty/non-token Basic is the uniform missing-credential 401"
            );
        }
        // Totally missing credential → the identical 401.
        let none = gw.handle(EdgeRequest::new("GET", WIRE, "", vec![], vec![]));
        assert_eq!(none.status(), 401);
    }

    /// **Bearer is unchanged on the git wire** (Basic is only a fallback when no Bearer is present).
    #[test]
    fn bearer_unchanged_on_the_git_wire() {
        let (gw, token) = seeded_gateway();
        let resp = gw.handle(EdgeRequest::new(
            "GET",
            WIRE,
            "",
            vec![("authorization".into(), format!("Bearer {token}"))],
            vec![],
        ));
        assert_eq!(
            resp.status(),
            200,
            "Bearer still authorizes on the git wire"
        );
    }

    #[test]
    fn receive_pack_advertisement_requires_push_capability_not_pull() {
        let (gw, pull_only) = seeded_gateway_with(
            myelin_identity_service::CredentialPurpose::AgentRun {
                run_id: "run-wire".into(),
                delegation_snapshot: Some(11),
            },
            &["repo.pull"],
            "jti-pull-only",
        );
        let header = vec![("authorization".into(), format!("Bearer {pull_only}"))];
        let fetch = gw.handle(EdgeRequest::new(
            "GET",
            WIRE,
            "service=git-upload-pack",
            header.clone(),
            vec![],
        ));
        assert_eq!(
            fetch.status(),
            200,
            "pull capability may advertise upload-pack"
        );
        let push = gw.handle(EdgeRequest::new(
            "GET",
            WIRE,
            "service=git-receive-pack",
            header,
            vec![],
        ));
        assert_eq!(
            push.status(),
            403,
            "receive-pack advertisement requires repo.push even though the shared route is GET"
        );
    }

    #[test]
    fn wire_path_region_must_match_verified_region() {
        let (gw, token) = seeded_gateway();
        let resp = gw.handle(EdgeRequest::new(
            "GET",
            "/acme/us-east/widgets/info/refs",
            "service=git-upload-pack",
            vec![("authorization".into(), format!("Bearer {token}"))],
            vec![],
        ));
        assert_eq!(
            resp.status(),
            403,
            "path region is an assertion, never scope authority"
        );
    }

    /// **The JSON product API stays Bearer-only: Basic is refused (401) even with a valid token, but
    /// Bearer works.** The Basic seam is gated on the `git.wire.*` action verb, so it never leaks onto
    /// a JSON route.
    #[test]
    fn json_api_is_bearer_only_basic_refused() {
        let (gw, token) = seeded_gateway();
        let basic = gw.handle(EdgeRequest::new(
            "GET",
            "/v1/whoami",
            "",
            vec![basic_header("x", &token)],
            vec![],
        ));
        assert_eq!(
            basic.status(),
            401,
            "the JSON API refuses HTTP Basic (Bearer-only) even with a valid token"
        );
        let bearer = gw.handle(EdgeRequest::new(
            "GET",
            "/v1/whoami",
            "",
            vec![("authorization".into(), format!("Bearer {token}"))],
            vec![],
        ));
        assert_eq!(bearer.status(), 200, "Bearer still authorizes the JSON API");
    }
}

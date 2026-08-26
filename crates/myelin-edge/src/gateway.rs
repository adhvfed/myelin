use crate::auth_request::{
    parse_auth_request, require_auth_request_budget, DeviceApprovalRequest, DeviceClaimRequest,
    DeviceStartRequest, LoginRequest,
};
use crate::authz::{authorize_edge_action, human_session_authority};
use crate::catalogue::{Handler, HandlerCtx, Method};
use crate::device_auth::{
    ApprovalOutcome, ClaimOutcome, DeviceApproval, DeviceAuthorizationBroker,
    DeviceAuthorizationError, DEVICE_AUTHORIZATION_TTL_SECS,
};
use crate::error::EdgeError;
use crate::request::{EdgeRequest, EdgeResponse};
use crate::shed_governor::{run_class_header, EdgeShed, RUN_CLASS_HEADER};
use crate::sse::{SseEvent, SseHub};
use myelin_events::clock::ClockError;
use myelin_events::{IdMinter, UlidMinter};
use myelin_identity::{AuthzError, Credential, Principal, PrincipalKind};
use myelin_identity_service::{
    machine_scheme, CapabilityAuthenticator, CapabilityMintSpec, CellTokenAuthority,
    CredentialAudience, CredentialPurpose, DpopBinding, HumanSsoAuthenticator, RequestIdentity,
    VerifiedAssertion,
};
use myelin_storage::TenantScope;
use myelin_substrate::shed::{RunClass, Surface};
use myelin_substrate::{Authorizer, InjectedIdentity, PublicSurface};
use myelin_tenancy::{Region, TenantId};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;

const DEFAULT_TOKEN_SCHEME: &str = "pat";
const HUMAN_SESSION_TTL_SECS: i64 = 8 * 60 * 60;

#[derive(Clone)]
struct HumanSessionIssuer {
    cell: Arc<CellTokenAuthority>,
    jtis: Arc<UlidMinter>,
    now: Arc<dyn Fn() -> Result<i64, ClockError> + Send + Sync>,
}

impl HumanSessionIssuer {
    fn now_unix(&self) -> Result<i64, EdgeError> {
        (self.now)().map_err(|error| {
            EdgeError::Unavailable(format!("authentication clock unavailable: {error}"))
        })
    }

    fn mint(
        &self,
        principal: &Principal,
        assertion: &VerifiedAssertion,
    ) -> Result<(String, i64), EdgeError> {
        let now = self.now_unix()?;
        let expiry = assertion
            .expires_at_unix
            .unwrap_or_else(|| now.saturating_add(HUMAN_SESSION_TTL_SECS))
            .min(now.saturating_add(HUMAN_SESSION_TTL_SECS));
        if expiry <= now {
            return Err(EdgeError::Unauthorized(
                "verified login credential has expired".into(),
            ));
        }
        let token = self.cell.mint(&CapabilityMintSpec {
            tenant: principal.tenant.0.clone(),
            region: principal.region.0.clone(),
            subject_key: principal.principal_id.0.clone(),
            jti: format!("session-{}", self.jtis.mint().0),
            exp_unix: expiry,
            authority: human_session_authority(),
            dpop_jkt: None,
            purpose: CredentialPurpose::HumanSession,
            audience: CredentialAudience::Edge,
        });
        Ok((token, expiry))
    }

    fn mint_device_session(
        &self,
        approval: &DeviceApproval,
        session_jti: &str,
        authorization_expires_at_unix: i64,
    ) -> Result<(String, i64), EdgeError> {
        let now = self.now_unix()?;
        let authorization_started_at_unix = authorization_expires_at_unix
            .checked_sub(DEVICE_AUTHORIZATION_TTL_SECS)
            .ok_or_else(|| EdgeError::Unauthorized("the CLI login request is invalid".into()))?;
        let expiry = approval
            .source_expires_at_unix
            .min(authorization_started_at_unix.saturating_add(HUMAN_SESSION_TTL_SECS));
        if expiry <= now {
            return Err(EdgeError::Unauthorized(
                "the approving browser credential has expired".into(),
            ));
        }
        if !matches!(approval.principal.kind, PrincipalKind::Human)
            || approval.principal.status != myelin_identity::PrincipalStatus::Active
            || approval.authority.is_empty()
        {
            return Err(EdgeError::Unauthorized(
                "the approved identity is not eligible for a human session".into(),
            ));
        }
        if !session_jti.starts_with("session-device-")
            || session_jti.len() > 128
            || !session_jti.as_bytes().iter().all(u8::is_ascii_graphic)
        {
            return Err(EdgeError::Unauthorized(
                "the CLI login request has an invalid session identity".into(),
            ));
        }
        let token = self.cell.mint(&CapabilityMintSpec {
            tenant: approval.principal.tenant.0.clone(),
            region: approval.principal.region.0.clone(),
            subject_key: approval.principal.principal_id.0.clone(),
            jti: session_jti.to_string(),
            exp_unix: expiry,
            authority: approval.authority.clone(),
            dpop_jkt: None,
            purpose: CredentialPurpose::HumanSession,
            audience: CredentialAudience::Edge,
        });
        Ok((token, expiry))
    }
}

pub fn sse_scope_for_tenant(tenant: &str) -> String {
    format!("tenant:{tenant}")
}

pub fn sse_scope_for_resource(tenant: &str, param: &str, id: &str) -> String {
    format!("tenant:{tenant}/{param}:{id}")
}

#[derive(Clone, Debug, Default)]
pub struct AuthPublicConfig {
    pub sso_configured: bool,
    pub providers: Vec<AuthProvider>,
    pub dev_login_enabled: bool,
    pub token_login_enabled: bool,
}

#[derive(Clone, Debug)]
pub struct AuthProvider {
    pub id: String,
    pub label: String,
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn repo_lifecycle_event(
    action: &str,
    params: &BTreeMap<String, String>,
    resp: &EdgeResponse,
) -> Option<(&'static str, String)> {
    if !(200..300).contains(&resp.status()) {
        return None;
    }
    match action {
        "git.repo.create" => resp
            .json_body()
            .and_then(|v| {
                if v.get("created").and_then(|created| created.as_bool()) == Some(false) {
                    return None;
                }
                v.get("applied")
                    .and_then(|a| a.get("slug"))
                    .and_then(|s| s.as_str())
                    .map(str::to_string)
            })
            .map(|slug| ("repo.created", slug)),
        "git.wire.receive_pack" => params
            .get("repo")
            .and_then(|repo| repo.strip_suffix(".git"))
            .map(str::to_string)
            .map(|slug| ("repo.pushed", slug)),
        _ => None,
    }
}

/// Maps a successful `chat.message.post` dispatch to a live-delivery frame:
/// (conversation id, message id). References only - the encrypted body never
/// touches the stream; subscribers revalidate through the authorized read
/// path.
fn chat_message_event(
    action: &str,
    params: &BTreeMap<String, String>,
    resp: &EdgeResponse,
) -> Option<(String, String)> {
    if action != "chat.message.post" || resp.status() != 201 {
        return None;
    }
    let conversation = params.get("conversation")?;
    if !is_bounded_resource_id(conversation) {
        return None;
    }
    let message_id = resp.json_body().and_then(|v| {
        v.get("message_id")
            .and_then(|m| m.as_str())
            .map(str::to_string)
    })?;
    Some((conversation.clone(), message_id))
}

fn is_bounded_resource_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.contains('*')
        && !id.contains('/')
        && !id.chars().any(|c| c.is_whitespace() || c.is_control())
}

enum Seg {
    Lit(String),
    Param(String),
    Rest(String),
}

type RouteMatch = (usize, BTreeMap<String, String>);

enum RouteKind {
    Normal(Arc<dyn Handler>),
    Sse {
        stream: String,
        resource_param: Option<String>,
    },
}

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
                Some(name) if name.starts_with("...") => Seg::Rest(name[3..].to_string()),
                Some(name) => Seg::Param(name.to_string()),
                None => Seg::Lit(s.to_string()),
            },
        )
        .collect()
}

pub struct GatewayBuilder {
    authn: Arc<CapabilityAuthenticator>,
    human_login: Arc<HumanSsoAuthenticator>,
    authorizer: Arc<dyn Authorizer>,
    routes: Vec<Route>,
    default_scheme: String,
    sse: SseHub,
    public_surface: PublicSurface,
    auth_config: AuthPublicConfig,
    public_base_url: Option<String>,
    human_session_issuer: Option<HumanSessionIssuer>,
    device_authorization: Option<DeviceAuthorizationBroker>,
    shed: EdgeShed,
}

impl GatewayBuilder {
    pub fn with_shed(mut self, shed: EdgeShed) -> GatewayBuilder {
        self.shed = shed;
        self
    }

    /// A handle to the hub the built gateway will broadcast on. Handlers that
    /// serve their own SSE subscriptions (with per-resource authorization)
    /// clone this at registration time; the clone shares the built gateway's
    /// channels.
    pub fn sse_hub(&self) -> SseHub {
        self.sse.clone()
    }

    pub fn registered_actions(&self) -> impl Iterator<Item = &str> {
        self.routes.iter().map(|route| route.action.as_str())
    }

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
             the TENANT-COARSE scope - every in-tenant subscriber would receive every object's \
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
             id - the tenant already prefixes every SSE scope"
        );
        assert!(
            segs.iter()
                .any(|s| matches!(s, Seg::Param(name) if name == resource_param)),
            "sse_route_scoped(`{pattern}`): the pattern does not carry the `{{{resource_param}}}` \
             path parameter the subscription scope is bound to (R2.2 - an object-addressed stream \
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

    pub fn default_token_scheme(mut self, scheme: impl Into<String>) -> GatewayBuilder {
        self.default_scheme = scheme.into();
        self
    }

    pub fn with_auth_config(mut self, cfg: AuthPublicConfig) -> GatewayBuilder {
        self.auth_config = cfg;
        self
    }

    pub fn with_human_session_issuer(mut self, cell: Arc<CellTokenAuthority>) -> GatewayBuilder {
        self.human_session_issuer = Some(HumanSessionIssuer {
            cell,
            jtis: Arc::new(UlidMinter::new()),
            now: Arc::new(|| {
                myelin_events::clock::system_clock_reading().map(|reading| reading.unix_seconds())
            }),
        });
        self
    }

    pub fn with_device_authorization(
        mut self,
        broker: DeviceAuthorizationBroker,
    ) -> GatewayBuilder {
        self.device_authorization = Some(broker);
        self
    }

    pub fn with_public_base_url(
        mut self,
        public_base_url: impl Into<String>,
    ) -> Result<GatewayBuilder, String> {
        let public_base_url = public_base_url.into();
        self.public_base_url = Some(validate_public_base_url(&public_base_url)?);
        Ok(self)
    }

    pub fn build(self) -> Gateway {
        Gateway {
            authn: self.authn,
            human_login: self.human_login,
            authorizer: self.authorizer,
            routes: self.routes,
            default_scheme: self.default_scheme,
            sse: self.sse,
            public_surface: self.public_surface,
            auth_config: self.auth_config,
            public_base_url: self.public_base_url,
            human_session_issuer: self.human_session_issuer,
            device_authorization: self.device_authorization,
            shed: self.shed,
        }
    }
}

fn validate_public_base_url(public_base_url: &str) -> Result<String, String> {
    let uri = public_base_url
        .parse::<hyper::Uri>()
        .map_err(|_| "public base URL must be a valid absolute HTTP(S) URL".to_string())?;
    if !matches!(uri.scheme_str(), Some("http" | "https"))
        || uri.authority().is_none()
        || uri
            .authority()
            .is_some_and(|authority| authority.as_str().contains('@'))
        || uri.query().is_some()
    {
        return Err(
            "public base URL must be an absolute, credential-free, query-free HTTP(S) URL".into(),
        );
    }
    Ok(public_base_url.trim_end_matches('/').to_string())
}

const DEVICE_AUTHORIZATION_LIMIT_MESSAGE: &str =
    "too many CLI login requests are waiting; retry shortly";

fn map_device_authorization_error(error: DeviceAuthorizationError) -> EdgeError {
    match error {
        DeviceAuthorizationError::InvalidInput(message) => EdgeError::BadRequest(message.into()),
        DeviceAuthorizationError::RateLimited { .. } => {
            EdgeError::TooManyRequests(DEVICE_AUTHORIZATION_LIMIT_MESSAGE.into())
        }
        DeviceAuthorizationError::Clock(_) => {
            EdgeError::Unavailable("interactive CLI login clock is temporarily unavailable".into())
        }
        DeviceAuthorizationError::Store(_) => {
            EdgeError::Unavailable("interactive CLI login state is temporarily unavailable".into())
        }
    }
}

fn device_approval_response(outcome: ApprovalOutcome) -> Result<EdgeResponse, EdgeError> {
    match outcome {
        ApprovalOutcome::Approved | ApprovalOutcome::AlreadyApproved => Ok(no_store(
            EdgeResponse::json(200, &json!({ "approved": true })),
        )),
        ApprovalOutcome::Expired | ApprovalOutcome::NotFound => Err(EdgeError::NotFound(
            "that CLI login request was not found or has expired; start again from the CLI".into(),
        )),
    }
}

fn no_store(response: EdgeResponse) -> EdgeResponse {
    response
        .with_header("cache-control", "no-store")
        .with_header("pragma", "no-cache")
}

pub struct Gateway {
    authn: Arc<CapabilityAuthenticator>,
    human_login: Arc<HumanSsoAuthenticator>,
    authorizer: Arc<dyn Authorizer>,
    routes: Vec<Route>,
    default_scheme: String,
    sse: SseHub,
    public_surface: PublicSurface,
    auth_config: AuthPublicConfig,
    public_base_url: Option<String>,
    human_session_issuer: Option<HumanSessionIssuer>,
    device_authorization: Option<DeviceAuthorizationBroker>,
    shed: EdgeShed,
}

impl Gateway {
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
            public_surface: PublicSurface::default(),
            auth_config: AuthPublicConfig::default(),
            public_base_url: None,
            human_session_issuer: None,
            device_authorization: None,
            shed: EdgeShed::v1_floor(),
        }
    }

    pub fn sse_hub(&self) -> &SseHub {
        &self.sse
    }

    pub fn public_surface(&self) -> &PublicSurface {
        &self.public_surface
    }

    pub fn handle(&self, req: EdgeRequest) -> EdgeResponse {
        match self.handle_inner(&req) {
            Ok(resp) => resp,
            Err(e) => {
                let mut resp = EdgeResponse::error(&e);
                if e.status() == 401 && self.is_git_wire_route(&req) {
                    resp = resp.with_header("WWW-Authenticate", r#"Basic realm="Myelin""#);
                }
                resp
            }
        }
    }

    fn is_git_wire_route(&self, req: &EdgeRequest) -> bool {
        let Some(method) = Method::parse(&req.method) else {
            return false;
        };
        match self.match_route(method, &req.path) {
            Ok(Some((idx, _))) => self.routes[idx].action.starts_with("git.wire."),
            Ok(None) | Err(_) => false,
        }
    }

    fn handle_inner(&self, req: &EdgeRequest) -> Result<EdgeResponse, EdgeError> {
        let method = Method::parse(&req.method)
            .ok_or_else(|| EdgeError::BadRequest(format!("unsupported method `{}`", req.method)))?;

        match (method, req.path.as_str()) {
            (Method::Get, "/v1/auth/config") => return Ok(self.auth_config_response()),
            (Method::Post, "/v1/auth/login") => return self.login(req),
            (Method::Post, "/v1/auth/device/authorization") => {
                return self.begin_device_authorization(req)
            }
            (Method::Post, "/v1/auth/device/approval") => {
                return self.approve_device_authorization(req)
            }
            (Method::Post, "/v1/auth/device/token") => return self.claim_device_authorization(req),
            _ => {}
        }

        let (idx, params) = self.match_route(method, &req.path)?.ok_or_else(|| {
            EdgeError::NotFound(format!("no route for {} {}", method.as_str(), req.path))
        })?;
        let route = &self.routes[idx];

        let path_tenant = params.get("tenant").map(|t| TenantId(t.clone()));
        let path_region = params.get("region").map(|r| Region(r.clone()));

        let allow_basic = route.action.starts_with("git.wire.");

        let identity = self.authenticate(req, path_tenant.as_ref(), allow_basic)?;
        let scope = self.resolve_scope(
            &identity.principal,
            path_tenant.as_ref(),
            path_region.as_ref(),
        )?;
        debug_assert_eq!(scope, identity.scope);
        let authorization_action = if route.action == "git.wire.upload_pack"
            && req.query_param("service").as_deref() == Some("git-receive-pack")
        {
            "git.wire.receive_pack"
        } else {
            route.action.as_str()
        };
        if !authorize_edge_action(self.authorizer.as_ref(), &identity, authorization_action) {
            return Err(EdgeError::Forbidden(format!(
                "authorization denied for action `{}`",
                authorization_action
            )));
        }
        let run_class = RunClass::derive(
            &identity.principal.kind,
            run_class_header(req.header(RUN_CLASS_HEADER)),
        );
        let surface = if route.action.starts_with("git.wire.") {
            Surface::GitFrontDoor
        } else {
            Surface::HttpIntake
        };
        let _shed_permit = match self.shed.admit(surface, scope.tenant(), run_class) {
            Ok(permit) => permit,
            Err(retry_after_secs) => {
                return Ok(EdgeResponse::error(&EdgeError::TooManyRequests(format!(
                    "the {} lane for this tenant is at capacity; retry in {retry_after_secs}s",
                    run_class.lane()
                )))
                .with_header("Retry-After", retry_after_secs.to_string()));
            }
        };
        match &route.kind {
            RouteKind::Normal(handler) => {
                let ctx = HandlerCtx {
                    identity: &identity,
                    principal: &identity.principal,
                    scope: &scope,
                    params: &params,
                    request: req,
                };
                let resp = handler.handle(&ctx)?;
                self.broadcast_repo_lifecycle(&route.action, &params, &scope, &resp);
                self.broadcast_chat_lifecycle(&route.action, &params, &scope, &resp);
                Ok(resp)
            }
            RouteKind::Sse {
                stream,
                resource_param,
            } => {
                let sse_scope = match resource_param {
                    None => sse_scope_for_tenant(&scope.tenant().0),
                    Some(param) => {
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
                Ok(EdgeResponse::sse(
                    sub,
                    identity.capability().expires_at_unix,
                ))
            }
        }
    }

    fn authenticate(
        &self,
        req: &EdgeRequest,
        path_tenant: Option<&TenantId>,
        allow_basic: bool,
    ) -> Result<RequestIdentity, EdgeError> {
        let request_binding = self.public_base_url.as_ref().map(|base| DpopBinding {
            htm: req.method.clone(),
            htu: format!("{base}{}", req.path),
        });
        let authenticate = |credential: &Credential| match request_binding.as_ref() {
            Some(binding) => {
                self.authn
                    .authenticate_identity_for_request(credential, path_tenant, binding)
            }
            None => self.authn.authenticate_identity(credential, path_tenant),
        };
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
            return authenticate(&cred)
                .map_err(|_| EdgeError::Unauthorized("authentication failed".into()));
        }
        if allow_basic {
            if let Some((username, material)) = req.basic_credentials() {
                let scheme = match username.strip_prefix("myelin-") {
                    Some(scheme) if machine_scheme::is_machine(scheme) => scheme.to_string(),
                    Some(_) => return Err(EdgeError::Unauthorized("authentication failed".into())),
                    None => scheme_of(),
                };
                let cred = Credential { scheme, material };
                return authenticate(&cred)
                    .map_err(|_| EdgeError::Unauthorized("authentication failed".into()));
            }
        }
        Err(EdgeError::Unauthorized(
            "no credential presented (Bearer token required)".into(),
        ))
    }

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
                "cli_login_enabled": self.device_authorization.is_some(),
            }),
        )
    }

    fn begin_device_authorization(&self, req: &EdgeRequest) -> Result<EdgeResponse, EdgeError> {
        let broker = self.device_authorization.as_ref().ok_or_else(|| {
            EdgeError::Unavailable("interactive CLI login is not configured".into())
        })?;
        let body: DeviceStartRequest = parse_auth_request(&req.body, "device login start")?;
        let started = match broker.begin(&body.code_challenge) {
            Ok(started) => started,
            Err(DeviceAuthorizationError::RateLimited { retry_after_secs }) => {
                return Ok(no_store(
                    EdgeResponse::error(&EdgeError::TooManyRequests(
                        DEVICE_AUTHORIZATION_LIMIT_MESSAGE.into(),
                    ))
                    .with_header("retry-after", retry_after_secs.to_string()),
                ));
            }
            Err(error) => return Err(map_device_authorization_error(error)),
        };
        Ok(no_store(EdgeResponse::json(
            201,
            &json!({
                "device_code": started.device_code,
                "user_code": started.user_code,
                "verification_uri": started.verification_uri,
                "verification_uri_complete": started.verification_uri_complete,
                "expires_in": started.expires_in,
                "interval": started.interval,
            }),
        )))
    }

    fn approve_device_authorization(&self, req: &EdgeRequest) -> Result<EdgeResponse, EdgeError> {
        let broker = self.device_authorization.as_ref().ok_or_else(|| {
            EdgeError::Unavailable("interactive CLI login is not configured".into())
        })?;
        require_auth_request_budget(&req.body)?;
        let identity = self.authenticate(req, None, false)?;
        if !authorize_edge_action(
            self.authorizer.as_ref(),
            &identity,
            "edge.auth.device.approve",
        ) {
            return Err(EdgeError::Forbidden(
                "this credential cannot approve an interactive CLI login".into(),
            ));
        }
        let authority = match identity.capability().purpose {
            CredentialPurpose::HumanSession => {
                let session_authority = human_session_authority();
                identity
                    .capability()
                    .effective_authority
                    .grants()
                    .filter(|grant| session_authority.iter().any(|allowed| allowed == grant))
                    .map(str::to_string)
                    .collect()
            }
            CredentialPurpose::OperatorBootstrap if self.auth_config.dev_login_enabled => {
                human_session_authority()
            }
            _ => {
                return Err(EdgeError::Forbidden(
                    "only a human browser session can approve an interactive CLI login".into(),
                ))
            }
        };
        let body: DeviceApprovalRequest = parse_auth_request(&req.body, "device approval")?;
        let approval = DeviceApproval {
            principal: identity.principal.clone(),
            authority,
            source_expires_at_unix: identity.capability().expires_at_unix,
        };
        let outcome = broker
            .approve(&body.user_code, approval)
            .map_err(map_device_authorization_error)?;
        device_approval_response(outcome)
    }

    fn claim_device_authorization(&self, req: &EdgeRequest) -> Result<EdgeResponse, EdgeError> {
        let broker = self.device_authorization.as_ref().ok_or_else(|| {
            EdgeError::Unavailable("interactive CLI login is not configured".into())
        })?;
        let body: DeviceClaimRequest = parse_auth_request(&req.body, "device login claim")?;
        match broker
            .claim(&body.device_code, &body.code_verifier)
            .map_err(map_device_authorization_error)?
        {
            ClaimOutcome::Pending => Ok(no_store(EdgeResponse::json(
                202,
                &json!({
                    "status": "authorization_pending",
                    "interval": crate::device_auth::DEVICE_AUTHORIZATION_POLL_INTERVAL_SECS,
                }),
            ))),
            ClaimOutcome::Approved(claim) => {
                let issuer = self.human_session_issuer.as_ref().ok_or_else(|| {
                    EdgeError::Unavailable("human session issuer is unavailable".into())
                })?;
                let (access_token, expires_at) = issuer.mint_device_session(
                    &claim.approval,
                    &claim.session_jti,
                    claim.authorization_expires_at_unix,
                )?;
                Ok(no_store(EdgeResponse::json(
                    200,
                    &json!({
                        "access_token": access_token,
                        "token_type": "Bearer",
                        "scheme": myelin_identity_service::machine_scheme::SESSION,
                        "expires_at": expires_at,
                    }),
                )))
            }
            ClaimOutcome::Expired => Err(EdgeError::Unauthorized(
                "the CLI login request has expired".into(),
            )),
            ClaimOutcome::Invalid => Err(EdgeError::Unauthorized(
                "the CLI login request is invalid".into(),
            )),
        }
    }

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

    fn broadcast_chat_lifecycle(
        &self,
        action: &str,
        params: &BTreeMap<String, String>,
        scope: &TenantScope,
        resp: &EdgeResponse,
    ) {
        let Some((conversation, message_id)) = chat_message_event(action, params, resp) else {
            return;
        };
        let tenant = &scope.tenant().0;
        let data = json!({
            "type": "chat.message.posted",
            "conversation": conversation,
            "message_id": message_id,
            "at": now_millis(),
        })
        .to_string();
        self.sse.broadcast(
            "chat",
            &sse_scope_for_resource(tenant, "conversation", &conversation),
            SseEvent::typed("chat.message.posted", data),
        );
    }

    fn login(&self, req: &EdgeRequest) -> Result<EdgeResponse, EdgeError> {
        let body: LoginRequest = parse_auth_request(&req.body, "login")?;
        let material = if body.scheme == myelin_identity_service::scheme::OIDC {
            let nonce = body
                .nonce
                .as_deref()
                .ok_or_else(|| EdgeError::BadRequest("OIDC login body missing `nonce`".into()))?;
            myelin_identity_service::oidc_login_material(&body.material, nonce).map_err(|_| {
                EdgeError::BadRequest("OIDC login body has an invalid `nonce`".into())
            })?
        } else {
            body.material
        };
        let cred = Credential {
            scheme: body.scheme,
            material,
        };
        match self.human_login.authenticate_with_assertion(&cred, None) {
            Ok((principal, assertion)) => {
                let issuer = self.human_session_issuer.as_ref().ok_or_else(|| {
                    EdgeError::Unavailable(
                        "human login verified, but the session issuer is unavailable".into(),
                    )
                })?;
                let (access_token, expires_at) = issuer.mint(&principal, &assertion)?;
                Ok(EdgeResponse::json(
                    200,
                    &json!({
                        "access_token": access_token,
                        "token_type": "Bearer",
                        "scheme": myelin_identity_service::machine_scheme::SESSION,
                        "expires_at": expires_at,
                    }),
                ))
            }
            Err(AuthzError::NotYetImplemented(_)) | Err(AuthzError::Unavailable(_)) => {
                Err(EdgeError::Unavailable(
                    "human login is not configured (JWKS/trust-anchors pending - MR-012); refused, \
                     not mocked"
                        .into(),
                ))
            }
            Err(_) => Err(EdgeError::Unauthorized("login failed".into())),
        }
    }

    fn match_route(&self, method: Method, path: &str) -> Result<Option<RouteMatch>, EdgeError> {
        let parts: Vec<&str> = path
            .trim_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();
        'routes: for (i, r) in self.routes.iter().enumerate() {
            if r.method != method {
                continue;
            }
            let has_rest = matches!(r.segs.last(), Some(Seg::Rest(_)));
            if has_rest {
                if parts.len() + 1 < r.segs.len() {
                    continue;
                }
            } else if r.segs.len() != parts.len() {
                continue;
            }

            if r.segs.iter().enumerate().any(|(idx, seg)| {
                matches!(seg, Seg::Lit(literal) if parts.get(idx) != Some(&literal.as_str()))
            }) {
                continue;
            }

            let mut params = BTreeMap::new();
            for (idx, seg) in r.segs.iter().enumerate() {
                match seg {
                    Seg::Lit(_) => {}
                    Seg::Param(name) => match parts.get(idx) {
                        Some(part) => {
                            params.insert(name.clone(), decode_route_component(part)?);
                        }
                        None => continue 'routes,
                    },
                    Seg::Rest(name) => {
                        let rest = parts[idx..]
                            .iter()
                            .map(|part| decode_route_component(part))
                            .collect::<Result<Vec<_>, _>>()?
                            .join("/");
                        params.insert(name.clone(), rest);
                    }
                }
            }
            return Ok(Some((i, params)));
        }
        Ok(None)
    }
}

fn decode_route_component(raw: &str) -> Result<String, EdgeError> {
    let bytes = raw.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return Err(EdgeError::BadRequest(
                    "route parameter contains malformed percent encoding".into(),
                ));
            }
            index += 3;
        } else {
            index += 1;
        }
    }

    let decoded = percent_encoding::percent_decode_str(raw)
        .decode_utf8()
        .map_err(|_| EdgeError::BadRequest("route parameter is not valid UTF-8".into()))?;
    if decoded.chars().any(char::is_control) {
        return Err(EdgeError::BadRequest(
            "route parameter contains a control character".into(),
        ));
    }
    Ok(decoded.into_owned())
}

fn kind_label(kind: &PrincipalKind) -> &'static str {
    match kind {
        PrincipalKind::Human => "human",
        PrincipalKind::Agent { .. } => "agent",
        PrincipalKind::Service => "service",
    }
}

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
                "expires_at": ctx.identity.capability().expires_at_unix,
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

    #[test]
    fn a_broken_clock_cannot_mint_a_human_session() {
        let issuer = HumanSessionIssuer {
            cell: Arc::new(CellTokenAuthority::from_seed(&[7_u8; 32], &[9_u8; 32]).unwrap()),
            jtis: Arc::new(UlidMinter::new()),
            now: Arc::new(|| Err(ClockError::BeforeUnixEpoch)),
        };
        let principal = Principal::stub(
            myelin_identity::PrincipalId("person:alice".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        );
        let assertion = VerifiedAssertion {
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            scheme: myelin_identity_service::scheme::OIDC.into(),
            subject_key: "alice".into(),
            expires_at_unix: None,
        };

        assert!(matches!(
            issuer.mint(&principal, &assertion),
            Err(EdgeError::Unavailable(message)) if message.contains("clock unavailable")
        ));
    }

    #[test]
    fn public_base_url_validation_returns_errors_instead_of_panicking() {
        for invalid in [
            "not a URL",
            "ftp://myelin.example",
            "https://user:secret@myelin.example",
            "https://myelin.example?tenant=spoofed",
        ] {
            assert!(
                validate_public_base_url(invalid).is_err(),
                "accepted {invalid}"
            );
        }

        assert_eq!(
            validate_public_base_url("https://myelin.example/base/").as_deref(),
            Ok("https://myelin.example/base")
        );
    }

    fn human_login() -> Arc<HumanSsoAuthenticator> {
        Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(
            Arc::new(KmsEngine::new()),
        )))
    }

    fn authn_empty() -> Arc<CapabilityAuthenticator> {
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

    #[derive(Clone, Copy)]
    struct ExactDpopBindingVerifier;

    impl myelin_identity_service::TokenVerifier for ExactDpopBindingVerifier {
        fn verify(
            &self,
            _credential: &Credential,
        ) -> myelin_identity::Result<myelin_identity_service::CapabilityToken> {
            Err(AuthzError::FailClosed(
                "request binding was not carried to the token verifier".into(),
            ))
        }

        fn verify_for_request(
            &self,
            _credential: &Credential,
            binding: &DpopBinding,
        ) -> myelin_identity::Result<myelin_identity_service::CapabilityToken> {
            if binding.htm != "GET" || binding.htu != "https://myelin.example/base/v1/whoami" {
                return Err(AuthzError::FailClosed(format!(
                    "unexpected DPoP request binding: {} {}",
                    binding.htm, binding.htu
                )));
            }
            Ok(myelin_identity_service::CapabilityToken {
                tenant: TenantId("acme".into()),
                region: Region("eu-west".into()),
                kind: myelin_identity_service::MachineKind::Pat,
                subject_key: "pat-subject".into(),
                authority: myelin_identity_service::Authority::of(["edge.identity.read"]),
                jti: "pat-jti".into(),
                dpop_bound: true,
                purpose: myelin_identity_service::CredentialPurpose::Pat,
                audience: myelin_identity_service::CredentialAudience::Edge,
                exp_unix: myelin_events::clock::MAX_RFC3339_UNIX_SECONDS,
            })
        }
    }

    #[derive(Clone, Copy)]
    struct DistinctFailureVerifier;

    impl myelin_identity_service::TokenVerifier for DistinctFailureVerifier {
        fn verify(
            &self,
            credential: &Credential,
        ) -> myelin_identity::Result<myelin_identity_service::CapabilityToken> {
            if credential.material == "revoked" {
                Err(AuthzError::FailClosed(
                    "secret-jti was revoked in tenant acme".into(),
                ))
            } else {
                Err(AuthzError::Unavailable(
                    "postgres auth_replay connection refused".into(),
                ))
            }
        }

        fn verify_for_request(
            &self,
            credential: &Credential,
            _binding: &DpopBinding,
        ) -> myelin_identity::Result<myelin_identity_service::CapabilityToken> {
            self.verify(credential)
        }
    }

    #[test]
    fn bearer_failures_do_not_expose_verifier_or_storage_details() {
        let authn = Arc::new(CapabilityAuthenticator::with_verifier(
            PrincipalStore::new(Arc::new(KmsEngine::new())),
            Arc::new(DistinctFailureVerifier),
            RevocationStore::new(),
        ));
        let gateway = Gateway::builder(authn, human_login(), Arc::new(AllowAll))
            .route(
                Method::Get,
                "/v1/whoami",
                "edge.whoami",
                Arc::new(WhoamiHandler),
            )
            .build();
        let request = |material: &str| {
            EdgeRequest::new(
                "GET",
                "/v1/whoami",
                "",
                vec![("Authorization".into(), format!("Bearer {material}"))],
                vec![],
            )
        };

        let revoked = gateway.handle(request("revoked"));
        let unavailable = gateway.handle(request("store-down"));
        assert_eq!(revoked.status(), 401);
        assert_eq!(unavailable.status(), 401);
        assert_eq!(revoked.json_body(), unavailable.json_body());
        let body = revoked.json_body().unwrap().to_string();
        assert!(!body.contains("secret-jti"));
        assert!(!body.contains("postgres"));
        assert_eq!(
            revoked.json_body().unwrap()["error"]["message"],
            "authentication required"
        );
    }

    #[test]
    fn gateway_carries_canonical_request_binding_to_dpop_verifier() {
        let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
        let principal = Principal::stub(
            myelin_identity::PrincipalId("pat-principal".into()),
            PrincipalKind::Service,
            TenantId("acme".into()),
        );
        let scope = TenantScope::from_verified_token(&principal, Region("eu-west".into()));
        store
            .put_principal(
                &scope,
                principal.principal_id.clone(),
                PrincipalKind::Service,
                myelin_identity::DataRole::Controller,
                myelin_identity::PrincipalStatus::Active,
                None,
            )
            .unwrap();
        store
            .link_credential(&scope, "pat", "pat-subject", &principal.principal_id)
            .unwrap();
        let authn = Arc::new(CapabilityAuthenticator::with_verifier(
            store,
            Arc::new(ExactDpopBindingVerifier),
            RevocationStore::new(),
        ));
        let gateway = Gateway::builder(authn, human_login(), Arc::new(AllowAll))
            .with_public_base_url("https://myelin.example/base/")
            .expect("valid public base URL")
            .route(
                Method::Get,
                "/v1/whoami",
                "edge.whoami",
                Arc::new(WhoamiHandler),
            )
            .build();

        let response = gateway.handle(EdgeRequest::new(
            "GET",
            "/v1/whoami",
            "query-is-excluded-from-dpop-htu=1",
            vec![("Authorization".into(), "Bearer opaque-proof".into())],
            vec![],
        ));
        assert_eq!(response.status(), 200);
    }

    #[derive(Clone, Copy)]
    struct AnyBindingMachineVerifier;

    impl myelin_identity_service::TokenVerifier for AnyBindingMachineVerifier {
        fn verify(
            &self,
            _credential: &Credential,
        ) -> myelin_identity::Result<myelin_identity_service::CapabilityToken> {
            self.token()
        }

        fn verify_for_request(
            &self,
            _credential: &Credential,
            _binding: &DpopBinding,
        ) -> myelin_identity::Result<myelin_identity_service::CapabilityToken> {
            self.token()
        }
    }

    impl AnyBindingMachineVerifier {
        fn token(&self) -> myelin_identity::Result<myelin_identity_service::CapabilityToken> {
            Ok(myelin_identity_service::CapabilityToken {
                tenant: TenantId("acme".into()),
                region: Region("eu-west".into()),
                kind: myelin_identity_service::MachineKind::Pat,
                subject_key: "pat-subject".into(),
                authority: myelin_identity_service::Authority::of(["edge.identity.read"]),
                jti: "pat-jti".into(),
                dpop_bound: true,
                purpose: myelin_identity_service::CredentialPurpose::Pat,
                audience: myelin_identity_service::CredentialAudience::Edge,
                exp_unix: myelin_events::clock::MAX_RFC3339_UNIX_SECONDS,
            })
        }
    }

    fn machine_gateway(shed: crate::shed_governor::EdgeShed) -> Gateway {
        let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
        let principal = Principal::stub(
            myelin_identity::PrincipalId("pat-principal".into()),
            PrincipalKind::Service,
            TenantId("acme".into()),
        );
        let scope = TenantScope::from_verified_token(&principal, Region("eu-west".into()));
        store
            .put_principal(
                &scope,
                principal.principal_id.clone(),
                PrincipalKind::Service,
                myelin_identity::DataRole::Controller,
                myelin_identity::PrincipalStatus::Active,
                None,
            )
            .unwrap();
        store
            .link_credential(&scope, "pat", "pat-subject", &principal.principal_id)
            .unwrap();
        let authn = Arc::new(CapabilityAuthenticator::with_verifier(
            store,
            Arc::new(AnyBindingMachineVerifier),
            RevocationStore::new(),
        ));
        Gateway::builder(authn, human_login(), Arc::new(AllowAll))
            .route(
                Method::Get,
                "/v1/whoami",
                "edge.whoami",
                Arc::new(WhoamiHandler),
            )
            .with_shed(shed)
            .build()
    }

    fn machine_request() -> EdgeRequest {
        EdgeRequest::new(
            "GET",
            "/v1/whoami",
            "",
            vec![("Authorization".into(), "Bearer opaque-proof".into())],
            vec![],
        )
    }

    fn shed_budget(cap: u32, human: u32) -> myelin_substrate::shed::SurfaceBudget {
        myelin_substrate::shed::SurfaceBudget {
            per_tenant_in_flight_cap: cap,
            human_lane_reservation: human,
            retry_after_secs: 5,
        }
    }

    #[test]
    fn a_machine_request_sheds_with_429_and_retry_after_when_its_lane_is_exhausted() {
        // cap 1 with 1 reserved human slot leaves the machine lanes zero budget,
        // so the very first machine request must shed through the full path.
        let gateway = machine_gateway(crate::shed_governor::EdgeShed::with_budgets(
            shed_budget(1, 1),
            shed_budget(1, 1),
        ));
        let response = gateway.handle(machine_request());
        assert_eq!(response.status(), 429);
        assert_eq!(
            response.json_body().unwrap()["error"]["code"],
            "too_many_requests"
        );
        match &response {
            EdgeResponse::Bytes { headers, .. } => {
                assert!(
                    headers
                        .iter()
                        .any(|(name, value)| name == "Retry-After" && value == "5"),
                    "a shed carries the tuned Retry-After: {headers:?}"
                );
            }
            EdgeResponse::Sse { .. } => panic!("a shed is a plain response"),
        }
    }

    #[test]
    fn an_admitted_machine_request_releases_its_slot_for_the_next_one() {
        // cap 3 / human 1 leaves the batch-ci class (a Service principal)
        // exactly one in-flight slot: if the permit leaked, the second
        // sequential request would shed.
        let gateway = machine_gateway(crate::shed_governor::EdgeShed::with_budgets(
            shed_budget(3, 1),
            shed_budget(3, 1),
        ));
        for _ in 0..3 {
            let response = gateway.handle(machine_request());
            assert_eq!(
                response.status(),
                200,
                "sequential machine requests within budget all admit (the permit releases)"
            );
        }
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
    fn parse_pattern_recognizes_the_catch_all_rest_segment() {
        let segs = parse_pattern("/v1/git/repos/{repo}/tree/{ref}/{...path}");
        assert!(matches!(segs.last(), Some(Seg::Rest(n)) if n == "path"));
        assert!(matches!(&segs[3], Seg::Param(n) if n == "repo"));
        assert!(matches!(&segs[4], Seg::Lit(l) if l == "tree"));
        let single = parse_pattern("/v1/git/repos/{repo}/blob/{ref}/{path}");
        assert!(matches!(single.last(), Some(Seg::Param(n)) if n == "path"));
    }

    #[test]
    fn route_captures_decode_encoded_refs_and_unicode_paths_exactly_once() {
        let gateway = Gateway::builder(authn_empty(), human_login(), Arc::new(AllowAll))
            .route(
                Method::Get,
                "/v1/git/repos/{repo}/tree/{ref}/{...path}",
                "git.tree.read",
                Arc::new(WhoamiHandler),
            )
            .build();

        let (_, params) = gateway
            .match_route(
                Method::Get,
                "/v1/git/repos/my%20repo/tree/feature%2Ffoo/docs/hello%20world%23%E2%9C%93.md",
            )
            .expect("encoded captures are well formed")
            .expect("the raw literal segments match");
        assert_eq!(params.get("repo").map(String::as_str), Some("my repo"));
        assert_eq!(params.get("ref").map(String::as_str), Some("feature/foo"));
        assert_eq!(
            params.get("path").map(String::as_str),
            Some("docs/hello world#✓.md")
        );
    }

    #[test]
    fn malformed_non_utf8_and_control_route_captures_are_bad_requests() {
        let gateway = Gateway::builder(authn_empty(), human_login(), Arc::new(AllowAll))
            .route(
                Method::Get,
                "/v1/git/repos/{repo}",
                "git.repo.read",
                Arc::new(WhoamiHandler),
            )
            .build();

        for path in [
            "/v1/git/repos/bad%",
            "/v1/git/repos/bad%GG",
            "/v1/git/repos/bad%FF",
            "/v1/git/repos/bad%00name",
        ] {
            assert!(
                matches!(
                    gateway.match_route(Method::Get, path),
                    Err(EdgeError::BadRequest(_))
                ),
                "malformed capture must fail as a bad request: {path}"
            );
        }
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
    fn edge_accepts_browser_credentials_only_as_bounded_capabilities() {
        let gateway = gw();
        let cookie = gateway.handle(EdgeRequest::new(
            "GET",
            "/v1/whoami",
            "",
            vec![("cookie".into(), "myelin_session=obsolete".into())],
            vec![],
        ));
        assert_eq!(
            cookie.status(),
            401,
            "the web BFF's opaque cookie is meaningful only to the BFF; Edge requires its bounded capability"
        );

        for obsolete_path in ["/v1/auth/refresh", "/v1/auth/logout"] {
            let response =
                gateway.handle(EdgeRequest::new("POST", obsolete_path, "", vec![], vec![]));
            assert_eq!(
                response.status(),
                404,
                "Edge must not advertise a parallel cookie lifecycle at {obsolete_path}"
            );
        }
    }

    fn www_authenticate(resp: &EdgeResponse) -> Option<String> {
        match resp {
            EdgeResponse::Bytes { headers, .. } | EdgeResponse::Sse { headers, .. } => headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("www-authenticate"))
                .map(|(_, v)| v.clone()),
        }
    }

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

    #[test]
    fn f1_git_wire_401_carries_basic_challenge_json_api_does_not() {
        let gw = gw_with_wire();

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
        assert_eq!(
            wire.json_body().unwrap()["error"]["message"],
            "authentication required"
        );

        let json = gw.handle(EdgeRequest::new("GET", "/v1/whoami", "", vec![], vec![]));
        assert_eq!(json.status(), 401);
        assert_eq!(
            www_authenticate(&json),
            None,
            "the JSON API 401 must NOT carry a Basic challenge (would break web login)"
        );

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

        let miss = gw.handle(EdgeRequest::new("GET", "/v1/nope", "", vec![], vec![]));
        assert_eq!(miss.status(), 404);
        assert_eq!(www_authenticate(&miss), None);
    }

    #[test]
    fn forged_bearer_is_401_never_resolves() {
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
        assert_eq!(b["cli_login_enabled"], false);
    }

    #[test]
    fn device_login_start_limit_is_a_retryable_public_429() {
        use base64::Engine as _;

        let broker = DeviceAuthorizationBroker::memory("https://myelin.example/cli/auth")
            .unwrap()
            .with_clock(|| 1_800_000_000)
            .with_start_policy(1, 1, 60);
        let gateway = Gateway::builder(authn_empty(), human_login(), Arc::new(AllowAll))
            .with_device_authorization(broker)
            .build();
        let body = serde_json::to_vec(&json!({
            "code_challenge": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([3_u8; 32]),
        }))
        .unwrap();

        let first = gateway.handle(EdgeRequest::new(
            "POST",
            "/v1/auth/device/authorization",
            "",
            vec![],
            body.clone(),
        ));
        assert_eq!(first.status(), 201);

        let limited = gateway.handle(EdgeRequest::new(
            "POST",
            "/v1/auth/device/authorization",
            "",
            vec![],
            body,
        ));
        assert_eq!(limited.status(), 429);
        assert_eq!(
            limited.json_body().unwrap()["error"]["code"],
            "too_many_requests"
        );
        let EdgeResponse::Bytes { headers, .. } = &limited else {
            panic!("a device authorization response is JSON")
        };
        assert!(headers
            .iter()
            .any(|(name, value)| { name.eq_ignore_ascii_case("retry-after") && value == "600" }));
        assert!(headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("cache-control") && value == "no-store"
        }));
    }

    #[test]
    fn device_approval_does_not_reveal_whether_a_code_used_to_exist() {
        let Err(expired) = device_approval_response(ApprovalOutcome::Expired) else {
            panic!("an expired code must be absent")
        };
        let Err(unknown) = device_approval_response(ApprovalOutcome::NotFound) else {
            panic!("an unknown code must be absent")
        };

        assert_eq!(expired, unknown);
        assert_eq!(expired.status(), 404);
        assert_eq!(
            expired.client_message(),
            "that CLI login request was not found or has expired; start again from the CLI",
        );
    }

    #[test]
    fn browser_approval_mints_one_fresh_cli_session_without_transferring_its_token() {
        use base64::Engine as _;
        use myelin_identity::{DataRole, PrincipalId, PrincipalStatus};
        use sha2::{Digest as _, Sha256};

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let source_expiry = now + 300;
        let cell = Arc::new(CellTokenAuthority::from_seed(&[41_u8; 32], &[42_u8; 32]).unwrap());
        let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
        let principal = Principal::new(
            TenantId("acme".into()),
            Region("eu-west".into()),
            PrincipalId("p:alice".into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
        );
        let scope = TenantScope::from_verified_token(&principal, principal.region.clone());
        store
            .put_principal(
                &scope,
                principal.principal_id.clone(),
                PrincipalKind::Human,
                DataRole::Controller,
                PrincipalStatus::Active,
                None,
            )
            .unwrap();
        let browser_token = cell.mint(&CapabilityMintSpec {
            tenant: principal.tenant.0.clone(),
            region: principal.region.0.clone(),
            subject_key: principal.principal_id.0.clone(),
            jti: "browser-session".into(),
            exp_unix: source_expiry,
            authority: human_session_authority(),
            dpop_jkt: None,
            purpose: CredentialPurpose::HumanSession,
            audience: CredentialAudience::Edge,
        });
        let authn = Arc::new(CapabilityAuthenticator::with_verifier(
            store,
            Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
            RevocationStore::new(),
        ));
        let broker = DeviceAuthorizationBroker::memory("https://myelin.example/cli/auth").unwrap();
        let gateway = Gateway::builder(authn, human_login(), Arc::new(AllowAll))
            .with_human_session_issuer(cell)
            .with_device_authorization(broker)
            .route(
                Method::Get,
                "/v1/whoami",
                "edge.whoami",
                Arc::new(WhoamiHandler),
            )
            .build();

        let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([9_u8; 32]);
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(verifier.as_bytes()));
        let started = gateway.handle(EdgeRequest::new(
            "POST",
            "/v1/auth/device/authorization",
            "",
            vec![],
            serde_json::to_vec(&json!({ "code_challenge": challenge })).unwrap(),
        ));
        assert_eq!(started.status(), 201);
        let started = started.json_body().unwrap();
        let device_code = started["device_code"].as_str().unwrap();
        let user_code = started["user_code"].as_str().unwrap();
        assert_eq!(
            started["verification_uri"],
            "https://myelin.example/cli/auth"
        );

        let pending = gateway.handle(EdgeRequest::new(
            "POST",
            "/v1/auth/device/token",
            "",
            vec![],
            serde_json::to_vec(&json!({
                "device_code": device_code,
                "code_verifier": verifier,
            }))
            .unwrap(),
        ));
        assert_eq!(pending.status(), 202);

        let approved = gateway.handle(EdgeRequest::new(
            "POST",
            "/v1/auth/device/approval",
            "",
            vec![
                ("authorization".into(), format!("Bearer {browser_token}")),
                ("x-myelin-token-scheme".into(), "session".into()),
            ],
            serde_json::to_vec(&json!({ "user_code": user_code })).unwrap(),
        ));
        assert_eq!(approved.status(), 200);

        let claimed = gateway.handle(EdgeRequest::new(
            "POST",
            "/v1/auth/device/token",
            "",
            vec![],
            serde_json::to_vec(&json!({
                "device_code": device_code,
                "code_verifier": verifier,
            }))
            .unwrap(),
        ));
        assert_eq!(claimed.status(), 200);
        let claimed = claimed.json_body().unwrap();
        let cli_token = claimed["access_token"].as_str().unwrap().to_string();
        assert_ne!(
            cli_token, browser_token,
            "the browser token is never transferred"
        );
        assert_eq!(claimed["scheme"], "session");
        assert!(claimed["expires_at"].as_i64().unwrap() <= source_expiry);

        let whoami = gateway.handle(EdgeRequest::new(
            "GET",
            "/v1/whoami",
            "",
            vec![
                ("authorization".into(), format!("Bearer {cli_token}")),
                ("x-myelin-token-scheme".into(), "session".into()),
            ],
            vec![],
        ));
        assert_eq!(whoami.status(), 200);
        assert_eq!(whoami.json_body().unwrap()["principal_id"], "p:alice");

        let replay = gateway.handle(EdgeRequest::new(
            "POST",
            "/v1/auth/device/token",
            "",
            vec![],
            serde_json::to_vec(&json!({
                "device_code": device_code,
                "code_verifier": verifier,
            }))
            .unwrap(),
        ));
        assert_eq!(replay.status(), 200);
        assert_eq!(
            replay.json_body().unwrap()["access_token"],
            cli_token,
            "a response-lost retry returns the same session rather than minting another one"
        );
    }

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

    #[test]
    fn repo_lifecycle_event_maps_create_and_push_only_on_success() {
        let created = EdgeResponse::json(
            201,
            &json!({ "applied": { "action": "git.repo.create", "slug": "widgets" }, "created": true, "durable": true }),
        );
        let empty = BTreeMap::new();
        assert_eq!(
            repo_lifecycle_event("git.repo.create", &empty, &created),
            Some(("repo.created", "widgets".to_string()))
        );

        let mut params = BTreeMap::new();
        params.insert("repo".to_string(), "widgets.git".to_string());
        let pushed = EdgeResponse::json(200, &json!({ "ok": true }));
        assert_eq!(
            repo_lifecycle_event("git.wire.receive_pack", &params, &pushed),
            Some(("repo.pushed", "widgets".to_string()))
        );

        let conflict = EdgeResponse::json(409, &json!({ "error": { "message": "exists" } }));
        assert_eq!(
            repo_lifecycle_event("git.repo.create", &empty, &conflict),
            None
        );
        assert_eq!(repo_lifecycle_event("git.pr.view", &empty, &created), None);

        let repeated = EdgeResponse::json(
            200,
            &json!({ "applied": { "action": "git.repo.create", "slug": "widgets" }, "created": false, "durable": true }),
        );
        assert_eq!(
            repo_lifecycle_event("git.repo.create", &empty, &repeated),
            None
        );
    }

    #[test]
    fn chat_message_event_maps_only_a_successful_post_with_a_bounded_conversation_id() {
        let mut params = BTreeMap::new();
        params.insert(
            "conversation".to_string(),
            "01J0CONV000000000000000000".to_string(),
        );
        let posted = EdgeResponse::json(
            201,
            &json!({ "message_id": "01J0MSG0000000000000000000", "durable": true }),
        );
        assert_eq!(
            chat_message_event("chat.message.post", &params, &posted),
            Some((
                "01J0CONV000000000000000000".to_string(),
                "01J0MSG0000000000000000000".to_string()
            ))
        );

        // a failed post never reaches the stream
        let refused = EdgeResponse::json(404, &json!({ "error": { "message": "not found" } }));
        assert_eq!(
            chat_message_event("chat.message.post", &params, &refused),
            None
        );

        // other chat actions never broadcast
        assert_eq!(
            chat_message_event("chat.messages.list", &params, &posted),
            None
        );

        // a conversation id that cannot form a bounded scope is dropped, not broadcast coarse
        let mut hostile = BTreeMap::new();
        hostile.insert("conversation".to_string(), "a/b".to_string());
        assert_eq!(
            chat_message_event("chat.message.post", &hostile, &posted),
            None
        );

        // a response without a message id (shape drift) broadcasts nothing
        let shapeless = EdgeResponse::json(201, &json!({ "durable": true }));
        assert_eq!(
            chat_message_event("chat.message.post", &params, &shapeless),
            None
        );
    }

    #[test]
    fn create_broadcasts_repo_created_on_the_tenant_firehose() {
        let gw = gw();
        let mut rx = gw
            .sse_hub()
            .subscribe("edge", &sse_scope_for_tenant("acme"))
            .into_receiver();
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
        let body = serde_json::to_vec(&json!({
            "scheme":"oidc",
            "material":"acme|eu-west|subj-1",
            "nonce":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        }))
        .unwrap();
        let resp = gw().handle(EdgeRequest::new("POST", "/v1/auth/login", "", vec![], body));
        assert_eq!(
            resp.status(),
            503,
            "human login refuses-not-mocks until configured"
        );
    }

    #[derive(Clone, Copy)]
    struct SuccessfulOidcVerifier {
        expires_at: i64,
    }

    impl myelin_identity_service::CredentialVerifier for SuccessfulOidcVerifier {
        fn verify(&self, credential: &Credential) -> myelin_identity::Result<VerifiedAssertion> {
            assert_eq!(credential.scheme, myelin_identity_service::scheme::OIDC);
            Ok(VerifiedAssertion {
                tenant: TenantId("acme".into()),
                region: Region("eu-west".into()),
                scheme: myelin_identity_service::scheme::OIDC.into(),
                subject_key: "oidc-sub-1".into(),
                expires_at_unix: Some(self.expires_at),
            })
        }
    }

    #[test]
    fn verified_oidc_login_mints_a_bounded_human_session_capability() {
        use myelin_identity::{DataRole, PrincipalId, PrincipalStatus};

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let upstream_expiry = now + 120;
        let cell = Arc::new(CellTokenAuthority::from_seed(&[31u8; 32], &[32u8; 32]).unwrap());
        let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
        let scope = TenantScope::from_verified_token(
            &Principal::stub(
                PrincipalId("p:alice".into()),
                PrincipalKind::Human,
                TenantId("acme".into()),
            ),
            Region("eu-west".into()),
        );
        store
            .put_principal(
                &scope,
                PrincipalId("p:alice".into()),
                PrincipalKind::Human,
                DataRole::Controller,
                PrincipalStatus::Active,
                None,
            )
            .unwrap();
        store
            .link_credential(
                &scope,
                myelin_identity_service::scheme::OIDC,
                "oidc-sub-1",
                &PrincipalId("p:alice".into()),
            )
            .unwrap();
        let human = Arc::new(HumanSsoAuthenticator::with_verifier(
            store.clone(),
            Arc::new(SuccessfulOidcVerifier {
                expires_at: upstream_expiry,
            }),
        ));
        let authn = Arc::new(CapabilityAuthenticator::with_verifier(
            store,
            Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
            RevocationStore::new(),
        ));
        let gateway = Gateway::builder(authn, human, Arc::new(AllowAll))
            .with_human_session_issuer(cell)
            .route(
                Method::Get,
                "/v1/whoami",
                "edge.whoami",
                Arc::new(WhoamiHandler),
            )
            .build();
        let login = gateway.handle(EdgeRequest::new(
            "POST",
            "/v1/auth/login",
            "",
            vec![],
            serde_json::to_vec(&json!({
                "scheme": "oidc",
                "material": "signed-id-token",
                "nonce": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            }))
            .unwrap(),
        ));
        assert_eq!(login.status(), 200);
        let body = login.json_body().unwrap();
        assert_eq!(body["scheme"], "session");
        assert_eq!(body["expires_at"], upstream_expiry);
        let token = body["access_token"].as_str().unwrap();

        let whoami = gateway.handle(EdgeRequest::new(
            "GET",
            "/v1/whoami",
            "",
            vec![
                ("authorization".into(), format!("Bearer {token}")),
                ("x-myelin-token-scheme".into(), "session".into()),
            ],
            vec![],
        ));
        assert_eq!(whoami.status(), 200);
        let whoami_body = whoami.json_body().unwrap();
        assert_eq!(whoami_body["principal_id"], "p:alice");
        assert_eq!(whoami_body["expires_at"], upstream_expiry);
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

    #[test]
    fn oidc_login_requires_a_well_formed_browser_transaction_nonce() {
        for body in [
            json!({"scheme":"oidc", "material":"id-token"}),
            json!({"scheme":"oidc", "material":"id-token", "nonce":"short"}),
        ] {
            let resp = gw().handle(EdgeRequest::new(
                "POST",
                "/v1/auth/login",
                "",
                vec![],
                serde_json::to_vec(&body).unwrap(),
            ));
            assert_eq!(resp.status(), 400);
        }
    }

    #[test]
    #[should_panic(expected = "sse_route_scoped")]
    fn object_addressed_pattern_cannot_register_tenant_coarse() {
        let _ = Gateway::builder(authn_empty(), human_login(), Arc::new(AllowAll)).sse_route(
            "/v1/t/{tenant}/repos/{repo}/events",
            "git.repo.events.subscribe",
            "git",
        );
    }

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
        assert!(is_bounded_resource_id("widgets"));
        assert!(is_bounded_resource_id("repo-7_x.y"));
        assert!(!is_bounded_resource_id("*"));
        assert!(!is_bounded_resource_id("a/b"));
        assert!(!is_bounded_resource_id("a b"));
        assert!(!is_bounded_resource_id(""));
        assert!(!is_bounded_resource_id(&"x".repeat(129)));
    }
}

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
        let human_session = purpose == myelin_identity_service::CredentialPurpose::HumanSession;
        let agent_run = purpose.is_agent_run();
        let credential_scheme = if human_session { "session" } else { "agent" };
        let subject_key = if human_session {
            "svc:founder"
        } else if agent_run {
            "agent:00000000-0000-0000-0000-000000000001"
        } else {
            "subj-1"
        };
        let principal_kind = if human_session {
            PrincipalKind::Human
        } else if agent_run {
            PrincipalKind::Agent {
                runtime_ref: myelin_identity::RuntimeRef("external:mcp".into()),
                on_behalf_of: Some(PrincipalId("human:owner".into())),
            }
        } else {
            PrincipalKind::Service
        };
        let principal_id = if agent_run {
            PrincipalId(subject_key.into())
        } else {
            PrincipalId("svc:founder".into())
        };
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
                principal_id.clone(),
                principal_kind,
                DataRole::Controller,
                PrincipalStatus::Active,
                None,
            )
            .expect("seed principal");
        if !agent_run {
            store
                .link_credential(&scope, credential_scheme, subject_key, &principal_id)
                .expect("link credential");
        }
        let revocations = RevocationStore::new();
        if purpose.is_run_scoped() {
            revocations
                .register_run_token_ttl(
                    &scope,
                    jti,
                    myelin_events::Timestamp("2020-01-01T00:00:00Z".into()),
                    myelin_events::Timestamp("2099-01-01T00:00:00Z".into()),
                )
                .expect("record seeded run lifetime");
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
            subject_key: subject_key.into(),
            jti: jti.into(),
            exp_unix: 9_999_999_999,
            authority: authority.iter().map(|grant| (*grant).into()).collect(),
            dpop_jkt: None,
            purpose,
            audience: myelin_identity_service::CredentialAudience::Edge,
        });
        let gw = Gateway::builder(authn, human, Arc::new(AllowAll))
            .default_token_scheme("agent")
            .sse_route("/v1/t/{tenant}/events", "edge.events.subscribe", "edge")
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

    #[test]
    fn sse_response_carries_the_verified_capability_expiry() {
        let (gw, token) = seeded_gateway();
        let response = gw.handle(EdgeRequest::new(
            "GET",
            "/v1/t/acme/events",
            "",
            vec![("authorization".into(), format!("Bearer {token}"))],
            vec![],
        ));

        match response {
            EdgeResponse::Sse {
                expires_at_unix, ..
            } => assert_eq!(expires_at_unix, 9_999_999_999),
            EdgeResponse::Bytes { status, .. } => {
                panic!("expected an authenticated SSE response, got HTTP {status}")
            }
        }
    }

    fn basic_header(user: &str, pass: &str) -> (String, String) {
        use base64::Engine as _;
        let b64 =
            base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}").as_bytes());
        ("authorization".to_string(), format!("Basic {b64}"))
    }

    const WIRE: &str = "/acme/eu-west/widgets/info/refs";

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

    #[test]
    fn basic_username_selects_the_browser_session_scheme_for_the_git_wire() {
        let (gw, token) = seeded_gateway_with(
            myelin_identity_service::CredentialPurpose::HumanSession,
            &["repo.pull"],
            "jti-basic-session",
        );
        let response = gw.handle(EdgeRequest::new(
            "GET",
            WIRE,
            "",
            vec![basic_header("myelin-session", &token)],
            vec![],
        ));
        assert_eq!(
            response.status(),
            200,
            "the CLI's browser session authenticates stock Git without a second token"
        );
    }

    #[test]
    fn an_unknown_myelin_basic_scheme_cannot_fall_back_to_the_default() {
        let (gw, token) = seeded_gateway();
        let response = gw.handle(EdgeRequest::new(
            "GET",
            WIRE,
            "",
            vec![basic_header("myelin-unknown", &token)],
            vec![],
        ));
        assert_eq!(response.status(), 401);
    }

    #[test]
    fn basic_garbage_is_same_as_missing_401() {
        let (gw, _t) = seeded_gateway();
        let cases: Vec<(String, String)> = vec![
            ("authorization".into(), "Basic !!!not-base64".into()),
            ("authorization".into(), "Basic ".into()),
            basic_header("user", ""),
            basic_header("user-no-colon", "x"),
        ];
        for h in cases {
            let resp = gw.handle(EdgeRequest::new("GET", WIRE, "", vec![h], vec![]));
            assert_eq!(
                resp.status(),
                401,
                "malformed/empty/non-token Basic is the uniform missing-credential 401"
            );
        }
        let none = gw.handle(EdgeRequest::new("GET", WIRE, "", vec![], vec![]));
        assert_eq!(none.status(), 401);
    }

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
            myelin_identity_service::CredentialPurpose::HumanSession,
            &["repo.pull"],
            "jti-pull-only",
        );
        let header = vec![basic_header("myelin-session", &pull_only)];
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

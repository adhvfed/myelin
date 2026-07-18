//! # `myelin-edge` — the product/edge API gateway (MR-014 / E0.6, P-S14 transport)
//!
//! **The ONE HTTP edge the UI/CLI/MCP call.** This crate builds the real gateway transport + the API
//! conventions every subsystem follows. It REUSES the platform's real auth + tenant primitives — it
//! does not fork the AppSpec, reimplement auth, or admit mock crypto.
//!
//! ## Anti-duplication + dependency decision (the binding first step)
//! - **HTTP framework: hyper 1.x DIRECTLY (no axum).** `Cargo.lock` already carries
//!   `hyper`/`hyper-util`/`http-body-util`/`tower`/`bytes`/`tokio` as transitive deps (via the
//!   aws-smithy / rustls / sqlx stacks); promoting them to direct deps adds **zero new crates** — the
//!   same "promote transitive, no new crate" discipline the identity crate used for `ring`/`hmac`.
//!   `axum` is **not** in the lock and would pull a new subtree (`axum`/`axum-core`/`matchit`/…),
//!   against the frontend canon's minimal-deps stance (§0). The tiny router is hand-built.
//! - **Crate placement: a new `myelin-edge` crate (not a substrate module).** The edge must depend on
//!   `myelin-identity-service` (the real `PasetoCapabilityVerifier`/`CapabilityAuthenticator`) and
//!   `myelin-storage` (the tenant scope) — both ABOVE `myelin-substrate` in the root-last §2.9 DAG.
//!   Putting the edge INSIDE substrate (root-last) would be an illegal upward dependency. So the edge
//!   is a LEAF consumer like the other service crates, OUTSIDE the eleven-crate library DAG modelled
//!   by `crate_graph.rs` (`substrate_is_root()`/`identity_is_sink()` preserved — nothing depends back
//!   on it; the edge is the graph's terminal consumer, the one front door).
//! - **Reconciled with `PublicSurface`/`serve(AppSpec)`:** the edge REALISES the substrate's
//!   [`PublicSurface`](myelin_substrate::PublicSurface) request-lifecycle SHAPE (tenant-from-token +
//!   IDOR reject/audit) over a real listener — it EXTENDS that contract, calling
//!   `PublicSurface::resolve_tenant` for the IDOR floor and `TenantScope::from_verified_token` for the
//!   scope. The substrate's named-deferred "real gateway transport / listener" (P-S13/P-S14+) is what
//!   this crate now builds.
//!
//! ## The gateway design (the request lifecycle)
//! ```text
//! request → authenticate (Bearer capability token | session cookie)
//!         → resolve principal (tenant/region from the VERIFIED token, NEVER the path/client)
//!         → set tenant scope (TenantScope; reject + AUDIT a cross-tenant path as an IDOR)
//!         → authorize (re-authorize the action — "internal = safe" is never presumed; fail-closed)
//!         → dispatch to the subsystem handler
//!         → respond (JSON view-model | SSE stream | the {error:{message}} envelope)
//! ```
//! See [`gateway::Gateway`].
//!
//! ## Auth at the edge (security-critical)
//! - **Bearer capability token — FULLY REAL, end-to-end.** The `Authorization: Bearer <paseto>` token
//!   is verified through the real [`CapabilityAuthenticator`](myelin_identity_service::CapabilityAuthenticator)
//!   (PASETO v4.public Ed25519 + macaroon attenuation + DPoP sender-constraint + durable S7
//!   revocation + S1 lookup, MR-011). A forged/expired/revoked token → a uniform **401** (oracle-free
//!   — the client cannot tell which). The verified principal's tenant/region is the request scope —
//!   NEVER a client-supplied tenant/path (the cardinal IDOR rule).
//! - **Session cookie — the web path.** The httpOnly-cookie session ([`session::SessionStore`]) carries
//!   the Bearer token SERVER-SIDE; tokens never reach client JS. The login endpoint runs the real
//!   [`HumanSsoAuthenticator::production`](myelin_identity_service::HumanSsoAuthenticator) — and since
//!   the human verifier config (JWKS/trust-anchors) is **MR-012-deferred, login REFUSES loudly (503),
//!   never mocks** a session. Consequently every currently live session is capability-backed and is
//!   re-authenticated to a complete signed [`RequestIdentity`](myelin_identity_service::RequestIdentity);
//!   there is not yet a human-session credential-context variant. The cookie-session machinery +
//!   the 401→refresh semantics are real.
//!
//! ## The API conventions (the headline)
//! - **Error model:** the `{error:{message, code?}}` envelope ([`error::EdgeError`]) — the typed
//!   error→status map (400/401/403/404/409/422/503/500); NEVER leaks internal detail/PII.
//! - **Versioning** (`/v1`), **pagination** (uniform cursor/limit, capped), **the JSON view-model
//!   data contract**, **HTTP method semantics** ([`catalogue`], modelled on git `api.rs`).
//! - **SSE real-time** ([`sse`]) — the `EventSource` stream the UI consumes (frontend canon §6),
//!   scoped to the verified tenant (never a client `*` selector).
//! - **The plug-in convention** ([`catalogue`]): a subsystem registers `(method, pattern, action,
//!   handler)`; the gateway owns everything else. The per-subsystem handlers are **MR-015+**; this
//!   crate ships the gateway + conventions + ONE trivial [`gateway::WhoamiHandler`] proof.
//!
//! ## Refuse-not-mock / fail-closed
//! No edge path authenticates via mock crypto (the real PASETO verifier is injected). An unconfigured
//! auth mode (human login) refuses loudly. Auth/authorize failures are fail-closed (401/403). The
//! gateway is TOTAL over a malformed request (no panic — every parse is checked).

pub mod authz;
pub mod bootstrap;
pub mod catalogue;
pub mod error;
pub mod gateway;
pub mod git_durable;
pub mod git_edge;
pub mod git_effect;
pub mod git_receive_pack;
pub mod git_wire_exec;
pub mod git_wire_http;
pub mod issue_authz;
pub mod repo_authz;
pub mod repo_authz_live;
pub mod request;
pub mod server;
pub mod session;
pub mod sse;

// R2.6: `AllowAll` is a TEST DOUBLE (gated like the in-memory store doubles) — the production
// action authorizer is `AuthenticatedActionPolicy` over the `MOUNTED_EDGE_ACTIONS` allowlist.
#[cfg(any(test, feature = "test-support"))]
pub use authz::AllowAll;
pub use authz::{
    action_requirement, authorize_edge_action, AcceptedPurpose, ActionRequirement,
    AuthenticatedActionPolicy, ACTION_REQUIREMENTS, MOUNTED_EDGE_ACTIONS,
};
pub use bootstrap::{
    bootstrap_principal_and_mint, BootstrapError, BootstrapOutcome, BootstrapParams,
    BOOTSTRAP_AUTHORITY, BOOTSTRAP_SCHEME,
};
pub use catalogue::{
    page_envelope, Handler, HandlerCtx, Method, Page, API_VERSION, DEFAULT_PAGE_LIMIT,
    MAX_PAGE_LIMIT,
};
pub use error::{map_authz_error, EdgeError};
pub use gateway::{
    sse_scope_for_resource, sse_scope_for_tenant, AuthProvider, AuthPublicConfig, Gateway,
    GatewayBuilder, WhoamiHandler,
};
pub use git_durable::{register_git_durable, DurableGitBackend};
pub use git_edge::{register_git, GitEdgeState};
pub use git_effect::GitEffectApi;
pub use git_wire_exec::{production_git_core, production_git_core_default, GitWireExecutor};
pub use git_wire_http::register_git_wire;
pub use issue_authz::{
    spawn_issue_authorization_reconciler, IdentityIssueTupleWriter, IssueReconciliationConfig,
    IssueReconciliationHandle, IssueReconciliationMetrics, IssueReconciliationMetricsSnapshot,
    IssueReconciliationReport, StoreBackedIssueAuthorizer, ISSUE_RECONCILE_TENANTS_ENV,
};
pub use repo_authz::{AllowAllRepos, DenyAllRepos, GrantBackedRepos, RepoAccess, RepoAuthorizer};
pub use repo_authz_live::{
    repo_object_id, repo_object_ref, CheckBackedRepoAuthorizer, NoRepoBootstrap,
    RepoBootstrapGrants, TupleRepoBootstrap, REPO_ADMIN_RELATION,
};
pub use request::{EdgeRequest, EdgeResponse};
pub use server::serve_edge;
pub use session::{SessionRecord, SessionStore, SESSION_COOKIE};
pub use sse::{SseEvent, SseHub, SseSubscription};

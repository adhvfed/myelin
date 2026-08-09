pub mod authz;
pub mod bootstrap;
pub mod catalogue;
pub mod chat_http;
pub mod ci_http;
pub mod device_auth;
pub mod error;
pub mod gateway;
#[path = "git_durable/ci_surface.rs"]
mod git_ci_surface;
pub mod git_durable;
pub mod git_edge;
pub mod git_effect;
pub mod git_receive_pack;
pub mod git_wire_exec;
pub mod git_wire_http;
pub mod issue_authz;
pub mod issues_http;
pub mod knowledge_http;
pub mod notif_http;
pub mod project_http;
pub mod repo_authz;
pub mod repo_authz_live;
pub mod request;
pub mod secret_admin_cmd;
pub mod server;
pub mod session;
pub mod sse;
pub mod tool_http;

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
pub use chat_http::register_chat;
pub use ci_http::{register_ci, DurableCiReadApi};
pub use device_auth::{device_authorization_migrations, DeviceAuthorizationBroker};
pub use error::{map_authz_error, EdgeError};
pub use gateway::{
    sse_scope_for_resource, sse_scope_for_tenant, AuthProvider, AuthPublicConfig, Gateway,
    GatewayBuilder, WhoamiHandler,
};
pub use git_durable::{
    recover_placed_git_at_boot, register_git_durable, DurableGitBackend, GitBootRecoveryReport,
    GitCellBootRecoveryReport, GitDatabaseProviders,
};
pub use git_edge::{register_git, GitEdgeState};
pub use git_effect::GitEffectApi;
pub use git_wire_exec::{
    production_git_core, production_git_core_default, production_git_core_default_with_shutdown,
    production_git_core_with_issuer, production_git_core_with_shutdown,
    production_git_core_with_shutdown_and_issuer, GitWireCredentialIssuer,
    GitWireCredentialIssuerFactory, GitWireCredentialRequest, GitWireExecutor,
    IdentityGitWireCredentialIssuerFactory,
};
#[cfg(any(test, feature = "test-support"))]
pub use git_wire_exec::{test_git_wire_credential_issuer, test_git_wire_credential_issuer_factory};
pub use git_wire_http::register_git_wire;
pub use issue_authz::{
    spawn_issue_authorization_reconciler, IdentityIssueTupleWriter, IssueReconciliationConfig,
    IssueReconciliationHandle, IssueReconciliationMetrics, IssueReconciliationMetricsSnapshot,
    IssueReconciliationReport, StoreBackedIssueAuthorizer, ISSUE_RECONCILE_TENANTS_ENV,
};
pub use issues_http::{
    register_issues, MAX_ISSUE_IMPORT_JSON_BYTES, MAX_ISSUE_IMPORT_RECORDS, MAX_ISSUE_JSON_BYTES,
};
pub use knowledge_http::register_knowledge;
pub use notif_http::register_notif;
pub use project_http::register_projects;
pub use repo_authz::{AllowAllRepos, DenyAllRepos, GrantBackedRepos, RepoAccess, RepoAuthorizer};
pub use repo_authz_live::{
    repo_object_id, repo_object_ref, CheckBackedRepoAuthorizer, NoRepoBootstrap,
    RepoBootstrapGrants, TupleRepoBootstrap, REPO_ADMIN_RELATION,
};
pub use request::{EdgeRequest, EdgeResponse};
pub use secret_admin_cmd::{
    execute_secret_command, SecretCommand, SecretCommandError, SecretCommandOutput, SecretTarget,
};
pub use server::{
    serve_edge, serve_edge_until_shutdown, serve_edge_until_shutdown_with_probe, ReadinessCheck,
    ReadinessProbe, ShutdownOutcome,
};
pub use session::{SessionRecord, SessionStore, SESSION_COOKIE};
pub use sse::{SseEvent, SseHub, SseSubscription};
pub use tool_http::register_tools;

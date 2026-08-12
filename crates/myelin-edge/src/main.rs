use bytes::Bytes;
use http_body_util::{BodyExt, Empty, Limited};
use hyper::{Request, Uri};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use myelin_config::{Mode, OIDC_JWKS_MAX_BYTES};
use myelin_edge::{
    bootstrap_principal_and_mint, execute_secret_command, recover_placed_git_at_boot,
    register_agent_mcp, register_agents, register_chat, register_ci, register_git_durable,
    register_git_wire, register_issues, register_knowledge, register_notif, register_privacy,
    register_projects, register_refs, register_tools, serve_edge_until_shutdown_with_probe,
    spawn_issue_authorization_reconciler, AgentMcpAuthority, AgentMcpResources, AgentMcpServices,
    AuthProvider, AuthPublicConfig, AuthenticatedActionPolicy, BootstrapParams,
    CheckBackedRepoAuthorizer, DeviceAuthorizationBroker, DurableChatMutationApi,
    DurableChatReadApi, DurableCiReadApi, DurableGitBackend, DurableKnowledgeReadApi,
    DurableRefsReadApi, Gateway, GitDatabaseProviders, IssueReconciliationConfig, Method,
    ReadinessCheck, ReadinessProbe, SecretCommand, SecretCommandError, SecretTarget,
    ShutdownOutcome, StoreBackedIssueAuthorizer, TupleRepoBootstrap, WhoamiHandler,
};
use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{
    AuthzError, Credential, FragmentAdmit, Principal, PrincipalId, PrincipalKind, RevokeTarget,
};
use myelin_identity_service::{
    CapabilityAuthenticator, CellTokenAuthority, DpopReplayGuard, HumanSsoAuthenticator, JwkSet,
    OidcConfig, PasetoCapabilityVerifier, PrincipalStore, ReplayGuard, RevocationStore,
    StoreBackedCheck, TokenVerifier, TupleStore,
};
use myelin_notif::pg_inbox::PgInboxStore;
use myelin_storage::{
    all_durable_migrations, seal_key_from_env, BlobStore, DurableCellRootBacking,
    DurableKmsBacking, DurablePrincipalBacking, DurableReplayBacking, DurableRevocationBacking,
    DurableTupleBacking, HotTables, KmsEngine, PgBootstrap, PgOutboxBacking, SubstrateProvider,
    TenantScope,
};
use myelin_tenancy::{Region, TenantId};
use std::{
    env::VarError,
    fs::OpenOptions,
    io::Write,
    net::SocketAddr,
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

const EDGE_DEFAULT_TOKEN_SCHEME: &str = "agent";
const EDGE_SHUTDOWN_GRACE: Duration = Duration::from_secs(20);
const EDGE_READINESS_DEADLINE: Duration = Duration::from_secs(2);
const EDGE_READINESS_CACHE_TTL: Duration = Duration::from_secs(1);
const OIDC_JWKS_DEADLINE: Duration = Duration::from_secs(5);
static READINESS_PROBE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct EdgeReadiness {
    provider: SubstrateProvider,
    git_root: PathBuf,
    state: tokio::sync::Mutex<EdgeReadinessState>,
}

struct EdgeReadinessState {
    checked_at: Option<tokio::time::Instant>,
    ready: bool,
}

fn public_auth_config(
    sso_configured: bool,
    dev_login_enabled: bool,
    token_login_enabled: bool,
) -> AuthPublicConfig {
    AuthPublicConfig {
        sso_configured,
        providers: if sso_configured {
            vec![AuthProvider {
                id: "oidc".into(),
                label: "Single sign-on".into(),
            }]
        } else {
            Vec::new()
        },
        dev_login_enabled,
        token_login_enabled,
    }
}

fn validated_oidc_jwks_uri(raw: &str) -> Result<Uri, String> {
    if raw.contains('#') {
        return Err("MYELIN_OIDC_JWKS_URI must not contain a fragment".into());
    }
    let uri: Uri = raw
        .parse()
        .map_err(|_| "MYELIN_OIDC_JWKS_URI is not a valid absolute URI".to_string())?;
    if uri.scheme_str() != Some("https") {
        return Err("MYELIN_OIDC_JWKS_URI must use https".into());
    }
    let authority = uri
        .authority()
        .ok_or_else(|| "MYELIN_OIDC_JWKS_URI must include a host".to_string())?;
    if authority.as_str().contains('@') {
        return Err("MYELIN_OIDC_JWKS_URI must not contain credentials".into());
    }
    Ok(uri)
}

fn validated_oidc_issuer(raw: &str) -> Result<String, String> {
    if raw.contains('#') {
        return Err("MYELIN_OIDC_ISSUER must not contain a fragment".into());
    }
    let uri: Uri = raw
        .parse()
        .map_err(|_| "MYELIN_OIDC_ISSUER is not a valid absolute URI".to_string())?;
    if uri.scheme_str() != Some("https") {
        return Err("MYELIN_OIDC_ISSUER must use https".into());
    }
    let authority = uri
        .authority()
        .ok_or_else(|| "MYELIN_OIDC_ISSUER must include a host".to_string())?;
    if authority.as_str().contains('@') {
        return Err("MYELIN_OIDC_ISSUER must not contain credentials".into());
    }
    if uri
        .path_and_query()
        .is_some_and(|value| value.query().is_some())
    {
        return Err("MYELIN_OIDC_ISSUER must not contain a query string".into());
    }
    Ok(raw.to_string())
}

fn device_verification_uri(raw_origin: &str) -> Result<String, String> {
    let uri: Uri = raw_origin
        .parse()
        .map_err(|_| "MYELIN_WEB_PUBLIC_URL is not a valid absolute URI".to_string())?;
    if !matches!(uri.scheme_str(), Some("http" | "https"))
        || uri.authority().is_none()
        || uri
            .authority()
            .is_some_and(|authority| authority.as_str().contains('@'))
        || uri.query().is_some()
        || !matches!(uri.path(), "" | "/")
    {
        return Err(
            "MYELIN_WEB_PUBLIC_URL must be an absolute, credential-free HTTP(S) origin".into(),
        );
    }
    Ok(format!("{}/cli/auth", raw_origin.trim_end_matches('/')))
}

fn parse_oidc_jwks_response(
    status: hyper::StatusCode,
    content_type: Option<&str>,
    bytes: &[u8],
) -> Result<JwkSet, String> {
    if status != hyper::StatusCode::OK {
        return Err(format!("OIDC JWKS endpoint returned HTTP {status}"));
    }
    let media_type = content_type
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if !media_type.is_some_and(|value| {
        value.eq_ignore_ascii_case("application/jwk-set+json")
            || value.eq_ignore_ascii_case("application/json")
    }) {
        return Err("OIDC JWKS endpoint returned an unsupported content type".into());
    }
    let document = std::str::from_utf8(bytes)
        .map_err(|_| "OIDC JWKS endpoint returned non-UTF-8 content".to_string())?;
    let keys = JwkSet::from_jwks_json(document)
        .map_err(|_| "OIDC JWKS endpoint returned a malformed key set".to_string())?;
    if keys.is_empty() {
        return Err("OIDC JWKS endpoint returned no supported signing keys".into());
    }
    Ok(keys)
}

async fn fetch_oidc_jwks(uri: Uri) -> Result<JwkSet, String> {
    let connector = HttpsConnectorBuilder::new()
        .with_provider_and_native_roots(rustls::crypto::aws_lc_rs::default_provider())
        .map_err(|_| "could not load native TLS trust roots for OIDC JWKS".to_string())?
        .https_only()
        .enable_http1()
        .build();
    let client: Client<_, Empty<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .header("accept", "application/jwk-set+json, application/json;q=0.9")
        .body(Empty::new())
        .map_err(|_| "could not build OIDC JWKS request".to_string())?;

    tokio::time::timeout(OIDC_JWKS_DEADLINE, async {
        let response = client
            .request(request)
            .await
            .map_err(|_| "OIDC JWKS HTTPS request failed".to_string())?;
        if response
            .headers()
            .get(hyper::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|length| length > OIDC_JWKS_MAX_BYTES)
        {
            return Err("OIDC JWKS response exceeded the 1 MiB limit".into());
        }
        let status = response.status();
        let content_type = response
            .headers()
            .get(hyper::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let bytes = Limited::new(response.into_body(), OIDC_JWKS_MAX_BYTES)
            .collect()
            .await
            .map_err(|_| {
                "OIDC JWKS response exceeded the 1 MiB limit or could not be read".to_string()
            })?
            .to_bytes();
        parse_oidc_jwks_response(status, content_type.as_deref(), &bytes)
    })
    .await
    .map_err(|_| "OIDC JWKS request exceeded the 5-second deadline".to_string())?
}

fn oidc_jwks_refresh(
    handle: tokio::runtime::Handle,
    uri: Uri,
) -> impl Fn() -> Result<JwkSet, AuthzError> + Send + Sync + 'static {
    move || {
        tokio::task::block_in_place(|| handle.block_on(fetch_oidc_jwks(uri.clone()))).map_err(
            |_| {
                AuthzError::FailClosed("OIDC signing-key refresh is temporarily unavailable".into())
            },
        )
    }
}

impl ReadinessProbe for EdgeReadiness {
    fn check(&self) -> ReadinessCheck<'_> {
        Box::pin(async move {
            let mut state = self.state.lock().await;
            if state
                .checked_at
                .is_some_and(|checked| checked.elapsed() < EDGE_READINESS_CACHE_TTL)
            {
                return state.ready;
            }
            let provider = self.provider.clone();
            let git_root = self.git_root.clone();
            let database =
                tokio::time::timeout(EDGE_READINESS_DEADLINE, provider.database_is_ready());
            let filesystem = tokio::time::timeout(
                EDGE_READINESS_DEADLINE,
                tokio::task::spawn_blocking(move || git_root_is_writable(&git_root)),
            );
            let (database, filesystem) = tokio::join!(database, filesystem);
            state.ready = matches!(database, Ok(true)) && matches!(filesystem, Ok(Ok(true)));
            state.checked_at = Some(tokio::time::Instant::now());
            state.ready
        })
    }
}

fn git_root_is_writable(root: &Path) -> bool {
    let sequence = READINESS_PROBE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let probe = root.join(format!(
        ".myelin-readiness-{}-{started}-{sequence}",
        std::process::id(),
    ));
    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)?;
        file.write_all(b"ready")?;
        file.sync_data()?;
        std::fs::remove_file(&probe)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(probe);
    }
    result.is_ok()
}

struct ComposedCore {
    provider: SubstrateProvider,
    kms: Arc<KmsEngine>,
    cell: Arc<CellTokenAuthority>,
    ci_surface_cursor_key: zeroize::Zeroizing<[u8; 32]>,
    cell_id: String,
    handle: tokio::runtime::Handle,
}

#[derive(Debug, PartialEq, Eq)]
struct EdgeRuntimeConfig {
    cell_id: String,
    git_root: Option<PathBuf>,
    git_wire: Option<GitWireRuntime>,
    public_base_url: Option<String>,
    listen_addr: Option<String>,
    dev_login_enabled: bool,
    token_login_enabled: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct GitWireRuntime {
    rootfs: PathBuf,
    runsc: PathBuf,
}

impl EdgeRuntimeConfig {
    fn from_env(serving: bool) -> Result<Self, String> {
        let config = Self::from_reader(serving, std::env::var)?;
        if config.git_wire.is_some() {
            let limits = myelin_edge::GitWireExecutor::default_limits();
            validate_git_wire_host(&limits)?;
        }
        Ok(config)
    }

    fn from_reader(
        serving: bool,
        mut read: impl FnMut(&'static str) -> Result<String, VarError>,
    ) -> Result<Self, String> {
        let cell_id = required_runtime_value("MYELIN_CELL_ID", read("MYELIN_CELL_ID"))?;
        let git_root = if serving {
            let raw = required_runtime_value("MYELIN_GIT_ROOT", read("MYELIN_GIT_ROOT"))?;
            let path = PathBuf::from(raw);
            if !path.is_absolute() {
                return Err("MYELIN_GIT_ROOT must be an absolute persistent path".into());
            }
            if path
                .components()
                .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
            {
                return Err("MYELIN_GIT_ROOT must not contain `.` or `..` components".into());
            }
            if path.parent().is_none() {
                return Err("MYELIN_GIT_ROOT must not be the filesystem root".into());
            }
            let path = path.canonicalize().map_err(|error| {
                format!("MYELIN_GIT_ROOT must name an existing persistent directory: {error}")
            })?;
            if path.parent().is_none() {
                return Err("MYELIN_GIT_ROOT must not resolve to the filesystem root".into());
            }
            if !path.is_dir() {
                return Err("MYELIN_GIT_ROOT must name a directory".into());
            }
            let temp_dir = std::env::temp_dir()
                .canonicalize()
                .unwrap_or_else(|_| std::env::temp_dir());
            if path.starts_with(temp_dir) {
                return Err(
                    "MYELIN_GIT_ROOT must not live under the operating-system temp directory"
                        .into(),
                );
            }
            Some(path)
        } else {
            None
        };
        let git_wire_enabled = if serving {
            optional_runtime_switch(
                "MYELIN_GIT_WIRE_ENABLED",
                read("MYELIN_GIT_WIRE_ENABLED"),
                true,
            )?
        } else {
            false
        };
        let git_wire = if git_wire_enabled {
            let rootfs = validated_git_wire_rootfs(read("MYELIN_GVISOR_GIT_ROOTFS"))?;
            let runsc = validated_runsc(read("MYELIN_RUNSC_BIN"))?;
            Some(GitWireRuntime { rootfs, runsc })
        } else {
            None
        };
        let public_base_url = if serving {
            Some(validated_public_base_url(read("MYELIN_PUBLIC_BASE_URL"))?)
        } else {
            None
        };
        let listen_addr = if serving {
            Some(optional_runtime_value(
                "MYELIN_EDGE_ADDR",
                read("MYELIN_EDGE_ADDR"),
                "127.0.0.1:8080",
            )?)
        } else {
            None
        };
        let dev_login_requested = serving
            && optional_runtime_switch("MYELIN_DEV_LOGIN", read("MYELIN_DEV_LOGIN"), false)?;
        let dev_login_enabled = validated_dev_login(
            dev_login_requested,
            cfg!(debug_assertions),
            listen_addr.as_deref(),
        )?;
        let token_login_enabled = serving
            && optional_runtime_switch("MYELIN_TOKEN_LOGIN", read("MYELIN_TOKEN_LOGIN"), false)?;
        Ok(Self {
            cell_id,
            git_root,
            git_wire,
            public_base_url,
            listen_addr,
            dev_login_enabled,
            token_login_enabled,
        })
    }
}

fn validated_public_base_url(value: Result<String, VarError>) -> Result<String, String> {
    let raw = required_runtime_value("MYELIN_PUBLIC_BASE_URL", value)?;
    let uri = raw
        .parse::<hyper::Uri>()
        .map_err(|_| "MYELIN_PUBLIC_BASE_URL must be an absolute HTTP(S) URL".to_string())?;
    if !matches!(uri.scheme_str(), Some("http" | "https")) || uri.authority().is_none() {
        return Err("MYELIN_PUBLIC_BASE_URL must be an absolute HTTP(S) URL".into());
    }
    if uri
        .authority()
        .is_some_and(|authority| authority.as_str().contains('@'))
    {
        return Err("MYELIN_PUBLIC_BASE_URL must not contain credentials".into());
    }
    if uri.query().is_some() {
        return Err("MYELIN_PUBLIC_BASE_URL must not contain a query string".into());
    }
    Ok(raw.trim_end_matches('/').to_string())
}

fn validate_git_wire_host(limits: &myelin_ci_sandbox::ResourceLimits) -> Result<(), String> {
    let cgroup = myelin_ci_sandbox::MemoryCgroup::create(1024 * 1024, limits.cpu_millis)
        .map_err(|error| format!("Git wire sandbox host preflight failed: {error}"))?;
    drop(cgroup);
    Ok(())
}

fn validated_git_wire_rootfs(value: Result<String, VarError>) -> Result<PathBuf, String> {
    let raw = required_runtime_value("MYELIN_GVISOR_GIT_ROOTFS", value)?;
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err("MYELIN_GVISOR_GIT_ROOTFS must be an absolute path".into());
    }
    let path = path.canonicalize().map_err(|error| {
        format!("MYELIN_GVISOR_GIT_ROOTFS must name an existing directory: {error}")
    })?;
    if path.parent().is_none() || !path.is_dir() {
        return Err("MYELIN_GVISOR_GIT_ROOTFS must resolve to a non-root directory".into());
    }
    let git = path.join("usr/bin/git");
    if !is_executable_file(&git) {
        return Err(format!(
            "MYELIN_GVISOR_GIT_ROOTFS must contain executable {}",
            git.display()
        ));
    }
    Ok(path)
}

fn validated_executable(
    name: &'static str,
    value: Result<String, VarError>,
) -> Result<PathBuf, String> {
    let raw = required_runtime_value(name, value)?;
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err(format!("{name} must be an absolute path"));
    }
    let path = path
        .canonicalize()
        .map_err(|error| format!("{name} must name an existing executable: {error}"))?;
    if !is_executable_file(&path) {
        return Err(format!("{name} must name an executable file"));
    }
    Ok(path)
}

fn validated_runsc(value: Result<String, VarError>) -> Result<PathBuf, String> {
    let path = validated_executable("MYELIN_RUNSC_BIN", value)?;
    use myelin_ci_sandbox::gvisor::RunscProbeError;
    myelin_ci_sandbox::gvisor::probe_runsc_version(&path).map_err(|error| match error {
        RunscProbeError::UnsafeBinary(reason) => {
            format!("MYELIN_RUNSC_BIN failed executable metadata validation: {reason}")
        }
        RunscProbeError::CouldNotExecute => {
            "MYELIN_RUNSC_BIN could not execute its version probe".to_string()
        }
        RunscProbeError::NotRunsc => {
            "MYELIN_RUNSC_BIN did not identify itself as runsc".to_string()
        }
    })?;
    Ok(path)
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn required_runtime_value(
    name: &'static str,
    value: Result<String, VarError>,
) -> Result<String, String> {
    let value = value.map_err(|error| match error {
        VarError::NotPresent => format!("required env var {name} is not set"),
        VarError::NotUnicode(_) => format!("env var {name} is not valid UTF-8"),
    })?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("env var {name} must not be empty"));
    }
    Ok(trimmed.to_string())
}

fn optional_runtime_switch(
    name: &'static str,
    value: Result<String, VarError>,
    default: bool,
) -> Result<bool, String> {
    match value {
        Err(VarError::NotPresent) => Ok(default),
        Err(VarError::NotUnicode(_)) => Err(format!("env var {name} is not valid UTF-8")),
        Ok(value) => match value.trim() {
            "1" => Ok(true),
            "0" => Ok(false),
            _ => Err(format!("env var {name} must be `0` or `1`")),
        },
    }
}

fn optional_runtime_value(
    name: &'static str,
    value: Result<String, VarError>,
    default: &'static str,
) -> Result<String, String> {
    match value {
        Err(VarError::NotPresent) => Ok(default.into()),
        Err(VarError::NotUnicode(_)) => Err(format!("env var {name} is not valid UTF-8")),
        Ok(value) if value.trim().is_empty() => Err(format!("env var {name} must not be empty")),
        Ok(value) => Ok(value.trim().into()),
    }
}

fn validated_dev_login(
    requested: bool,
    debug_build: bool,
    listen_addr: Option<&str>,
) -> Result<bool, String> {
    if !requested {
        return Ok(false);
    }
    if !debug_build {
        return Err("MYELIN_DEV_LOGIN=1 is available only in debug builds".into());
    }
    let loopback = listen_addr
        .and_then(|value| value.parse::<SocketAddr>().ok())
        .is_some_and(|addr| addr.ip().is_loopback());
    if !loopback {
        return Err(
            "MYELIN_DEV_LOGIN=1 requires MYELIN_EDGE_ADDR to be a numeric loopback address".into(),
        );
    }
    Ok(true)
}

fn runtime_config_or_exit(serving: bool) -> EdgeRuntimeConfig {
    EdgeRuntimeConfig::from_env(serving).unwrap_or_else(|error| {
        eprintln!("edge: production runtime configuration refused to start: {error}");
        std::process::exit(1);
    })
}

async fn compose_core(cell_id: String) -> ComposedCore {
    let bootstrap = match PgBootstrap::from_env(Mode::RequireEnv).await {
        Ok(bootstrap) => bootstrap,
        Err(e) => {
            eprintln!("edge: database bootstrap refused to start: {e}");
            std::process::exit(1);
        }
    };
    let handle = tokio::runtime::Handle::current();
    if let Err(e) = bootstrap.migrate_foundation().await {
        eprintln!(
            "edge: cannot apply the substrate foundation migrations (outbox/consumer_dedup): {e}"
        );
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
    {
        eprintln!("edge: cannot apply the durable migration aggregate: {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .migrate(
            &myelin_edge::device_authorization_migrations(),
            &HotTables::none(),
        )
        .await
    {
        eprintln!("edge: cannot apply the interactive CLI login migration: {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .migrate(
            &myelin_issues::issues_migrations(),
            &myelin_issues::issues_hot_tables(),
        )
        .await
    {
        eprintln!("edge: cannot apply the Issues authorization-saga migrations: {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .migrate(&myelin_notif::migrations::migrations(), &HotTables::none())
        .await
    {
        eprintln!("edge: cannot apply the notification inbox migrations: {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .migrate(
            &myelin_chat::store::pg_conversation::chat_migrations(),
            &HotTables::none(),
        )
        .await
    {
        eprintln!("edge: cannot apply the Chat conversation and message migrations: {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .migrate(
            &myelin_knowledge::knowledge_page_migrations(),
            &HotTables::none(),
        )
        .await
    {
        eprintln!("edge: cannot apply the Knowledge page and block migrations: {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .verify_index_ready(myelin_chat::store::pg_conversation::CONVERSATION_RECENT_INDEX)
        .await
    {
        eprintln!("edge: Chat topic-list keyset index is not ready: {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .verify_index_ready(myelin_knowledge::KNOWLEDGE_PAGE_RECENT_INDEX)
        .await
    {
        eprintln!("edge: Knowledge page-list keyset index is not ready: {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .verify_index_ready("notif_inbox_recipient_keyset")
        .await
    {
        eprintln!("edge: notification inbox keyset index is not ready: {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .verify_index_ready(myelin_issues::ISSUE_RECENT_LIST_INDEX)
        .await
    {
        eprintln!("edge: Issues recent-list index is not ready: {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .verify_index_ready(myelin_issues::ISSUE_KEY_PREFIX_LIST_INDEX)
        .await
    {
        eprintln!("edge: Issues key-prefix list index is not ready: {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .migrate(
            &myelin_git::pg_pr_store::git_pr_migrations(),
            &myelin_git::pg_pr_store::git_pr_hot_tables(),
        )
        .await
    {
        eprintln!("edge: cannot apply the Git PR lifecycle migrations: {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .migrate(
            &myelin_git::check_status_store::check_status_migrations(),
            &myelin_git::check_status_store::check_status_hot_tables(),
        )
        .await
    {
        eprintln!("edge: cannot apply the Git check projection migrations: {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .migrate(
            &myelin_flow::migrations::migrations(),
            &HotTables::declare(["workflow_run"]),
        )
        .await
    {
        eprintln!("edge: cannot apply the CI Flow prerequisite migrations: {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .migrate(
            &myelin_ci_controlplane::ci_controlplane_migrations(),
            &myelin_ci_controlplane::ci_controlplane_hot_tables(),
        )
        .await
    {
        eprintln!("edge: cannot apply the CI run-surface migrations: {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap.verify_index_ready("git_pr_head_repo_idx").await {
        eprintln!("edge: Git PR provenance index is not ready: {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .verify_index_ready_exact(myelin_ci_controlplane::CI_RUN_SURFACE_INDEX_READINESS)
        .await
    {
        eprintln!("edge: CI run-list keyset index is not ready: {e}");
        std::process::exit(1);
    }
    if let Err(e) = bootstrap
        .verify_index_ready("git_pr_command_operation_scope_uidx")
        .await
    {
        eprintln!("edge: Git PR operation-scope index is not ready: {e}");
        std::process::exit(1);
    }
    let provider = match bootstrap.into_runtime().await {
        Ok(provider) => provider,
        Err(e) => {
            eprintln!("edge: database runtime handoff refused to start: {e}");
            std::process::exit(1);
        }
    };
    let seal_key = match seal_key_from_env() {
        Ok(k) => k,
        Err(e) => {
            eprintln!("edge: refused to start (durable-by-default requires the seal key): {e}");
            std::process::exit(1);
        }
    };
    let ci_surface_cursor_key =
        seal_key.derive_service_key("myelin 2026-07-24 ci run surface cursor v1");
    let kms_backing = DurableKmsBacking::new(provider.db_pool().clone(), cell_id.clone());
    let kms = match kms_backing.load_or_generate(&seal_key).await {
        Ok(engine) => Arc::new(engine),
        Err(e) => {
            eprintln!(
                "edge: KMS refused to start (fail-closed, never a silent in-memory engine): {e}"
            );
            std::process::exit(1);
        }
    };
    let cell_backing = DurableCellRootBacking::new(provider.db_pool().clone(), cell_id.clone());
    let cell_material = match cell_backing.load_or_generate(&seal_key).await {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "edge: cell token authority refused to start (fail-closed, never a fresh root that \
                 would orphan every minted token): {e}"
            );
            std::process::exit(1);
        }
    };
    let cell = Arc::new(
        CellTokenAuthority::from_material(&cell_material).unwrap_or_else(|e| {
            eprintln!("edge: durable cell token-authority material is invalid: {e:?}");
            std::process::exit(1);
        }),
    );
    ComposedCore {
        provider,
        kms,
        cell,
        ci_surface_cursor_key,
        cell_id,
        handle,
    }
}

#[tokio::main]
async fn main() {
    myelin_events::install_payload_free_panic_hook("edge");
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => {
            let runtime = runtime_config_or_exit(true);
            let core = compose_core(runtime.cell_id.clone()).await;
            serve(core, runtime).await;
        }
        Some("bootstrap") => {
            let runtime = runtime_config_or_exit(false);
            operator_bootstrap(compose_core(runtime.cell_id).await, &args[1..]).await;
        }
        Some("revoke") => {
            let runtime = runtime_config_or_exit(false);
            operator_revoke(compose_core(runtime.cell_id).await, &args[1..]).await;
        }
        Some("secret") => {
            let runtime = runtime_config_or_exit(false);
            operator_secret(compose_core(runtime.cell_id).await, &args[1..]).await;
        }
        Some(other) => {
            eprintln!(
                "edge: unknown subcommand `{other}` (expected: <none> = serve | bootstrap | revoke | secret)"
            );
            std::process::exit(2);
        }
    }
}

async fn serve(core: ComposedCore, runtime: EdgeRuntimeConfig) {
    let EdgeRuntimeConfig {
        cell_id: _,
        git_root,
        git_wire,
        public_base_url,
        listen_addr,
        dev_login_enabled,
        token_login_enabled,
    } = runtime;
    let git_root = git_root.expect("serving config carries a Git root");
    let public_base_url = public_base_url.expect("serving config carries a public base URL");
    let listen_addr = listen_addr.expect("serving config carries a listen address");
    let ComposedCore {
        provider,
        kms,
        cell,
        ci_surface_cursor_key,
        cell_id,
        handle,
    } = core;
    let readiness = Arc::new(EdgeReadiness {
        provider: provider.clone(),
        git_root: git_root.clone(),
        state: tokio::sync::Mutex::new(EdgeReadinessState {
            checked_at: None,
            ready: false,
        }),
    });
    if let Some(git_wire) = &git_wire {
        eprintln!(
            "edge: validated Git wire sandbox runtime (runsc={}, rootfs={})",
            git_wire.runsc.display(),
            git_wire.rootfs.display()
        );
    } else {
        eprintln!("edge: Git wire transport disabled by configuration");
    }

    let git_check_admission_provider =
        provider
            .auxiliary_runtime_lane(4)
            .await
            .unwrap_or_else(|error| {
                eprintln!("edge: protected-push admission lane refused to start: {error}");
                std::process::exit(1);
            });

    let git_outbox = OutboxStore::durable(Arc::new(PgOutboxBacking::new(
        provider.db_pool().clone(),
        handle.clone(),
    )));
    let git_minter: Arc<dyn myelin_events::IdMinter> = Arc::new(myelin_events::UlidMinter::new());

    let (authn, run_token_authorizer) =
        durable_capability_authenticator(&provider, &kms, &cell, &handle);
    let authn = Arc::new(authn);

    let oidc_settings = provider.config().oidc.clone();
    let human_store = PrincipalStore::with_pg(
        kms.clone(),
        DurablePrincipalBacking::new(provider.clone()),
        handle.clone(),
    );
    let mcp_principals = PrincipalStore::with_pg(
        kms.clone(),
        DurablePrincipalBacking::new(provider.clone()),
        handle.clone(),
    );
    let device_authorization = std::env::var("MYELIN_WEB_PUBLIC_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|origin| {
            let verification_uri = device_verification_uri(origin.trim()).unwrap_or_else(|error| {
                eprintln!("edge: invalid CLI login browser origin: {error}");
                std::process::exit(1);
            });
            DeviceAuthorizationBroker::with_pg(
                provider.db_pool().clone(),
                handle.clone(),
                verification_uri,
            )
            .unwrap_or_else(|error| {
                eprintln!("edge: invalid CLI login verification URI: {error}");
                std::process::exit(1);
            })
        });
    let auth_config = public_auth_config(
        oidc_settings.is_some(),
        dev_login_enabled,
        token_login_enabled,
    );
    let human_login = Arc::new(match oidc_settings {
        Some(oidc) => {
            let issuer = validated_oidc_issuer(&oidc.issuer).unwrap_or_else(|error| {
                eprintln!("edge: invalid OIDC issuer configuration: {error}");
                std::process::exit(1);
            });
            let jwks_uri = oidc
                .jwks_uri
                .as_deref()
                .map(validated_oidc_jwks_uri)
                .transpose()
                .unwrap_or_else(|error| {
                    eprintln!("edge: invalid OIDC JWKS configuration: {error}");
                    std::process::exit(1);
                });
            let jwks = match oidc.jwks_json.as_deref() {
                Some(document) => {
                    let keys = JwkSet::from_jwks_json(document).unwrap_or_else(|_| {
                        eprintln!("edge: the configured OIDC bootstrap JWKS is malformed");
                        std::process::exit(1);
                    });
                    if keys.is_empty() {
                        eprintln!(
                            "edge: the configured OIDC bootstrap JWKS has no supported signing keys"
                        );
                        std::process::exit(1);
                    }
                    keys
                }
                None => {
                    let uri = jwks_uri.clone().unwrap_or_else(|| {
                        eprintln!("edge: OIDC has neither a bootstrap JWKS nor a refresh URI");
                        std::process::exit(1);
                    });
                    fetch_oidc_jwks(uri).await.unwrap_or_else(|error| {
                        eprintln!("edge: could not fetch the initial OIDC JWKS: {error}");
                        std::process::exit(1);
                    })
                }
            };
            eprintln!(
                "edge: OIDC login wired ({} JWKS key(s), rotation={})",
                jwks.len(),
                if jwks_uri.is_some() {
                    "enabled"
                } else {
                    "disabled"
                }
            );
            let config = OidcConfig::new(issuer, oidc.audience);
            let replay =
                ReplayGuard::with_pg(DurableReplayBacking::new(provider.clone()), handle.clone());
            match jwks_uri {
                Some(uri) => HumanSsoAuthenticator::production_with_oidc_refresh(
                    human_store,
                    (config, jwks),
                    replay,
                    oidc_jwks_refresh(handle.clone(), uri),
                ),
                None => {
                    HumanSsoAuthenticator::production_with_oidc(human_store, (config, jwks), replay)
                }
            }
        }
        None => {
            eprintln!("edge: OIDC not configured - human login refuses (refuse-not-mock)");
            HumanSsoAuthenticator::production(human_store)
        }
    });

    let check =
        StoreBackedCheck::with_pg(provider.clone(), kms.clone(), cell.clone(), handle.clone());
    for admit in check.admit_git_fragment() {
        if let FragmentAdmit::Rejected { reason } = admit {
            eprintln!("edge: the Git ReBAC fragment did not admit (authz would deny everything): {reason}");
            std::process::exit(1);
        }
    }
    for admit in check.admit_issue_fragment() {
        if let FragmentAdmit::Rejected { reason } = admit {
            eprintln!(
                "edge: the Issues ReBAC fragment did not admit (issue authz would deny everything): {reason}"
            );
            std::process::exit(1);
        }
    }
    for admit in check.admit_chat_fragment() {
        if let FragmentAdmit::Rejected { reason } = admit {
            eprintln!(
                "edge: the Chat ReBAC fragment did not admit (private channels would deny everything): {reason}"
            );
            std::process::exit(1);
        }
    }
    for admit in check.admit_knowledge_fragment() {
        if let FragmentAdmit::Rejected { reason } = admit {
            eprintln!(
                "edge: the Knowledge ReBAC fragment did not admit (page access would deny everything): {reason}"
            );
            std::process::exit(1);
        }
    }
    let issue_authorizer = StoreBackedIssueAuthorizer::new(check.clone());
    let issue_store = Arc::new(myelin_issues::PgIssueStore::new(
        provider.clone(),
        kms.clone(),
        issue_authorizer.clone(),
    ));
    let issue_reconciliation_config =
        IssueReconciliationConfig::from_env(Region::new(provider.config().region.clone()))
            .unwrap_or_else(|error| {
                eprintln!("edge: Issues authorization reconciler refused to start: {error}");
                std::process::exit(1);
            });
    let issue_reconciler = spawn_issue_authorization_reconciler(
        issue_store.clone(),
        check.clone(),
        issue_reconciliation_config,
    );
    let thresholds = match myelin_substrate::Thresholds::load_canonical() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("edge: cannot load the canonical thresholds file (the fail-static bound for the git-wire authz): {e}");
            std::process::exit(1);
        }
    };
    let revocation_sla_secs = thresholds.revocation.sla_mins * 60;
    let repo_authz = match CheckBackedRepoAuthorizer::try_new(
        check.clone(),
        revocation_sla_secs,
        &thresholds.fail_static,
    ) {
        Ok(a) => Arc::new(a),
        Err(e) => {
            eprintln!(
                "edge: the git-wire repo authorizer refused to construct (staleness bound): {e:?}"
            );
            std::process::exit(1);
        }
    };
    let repo_bootstrap = Arc::new(TupleRepoBootstrap::new(check.tuples().clone()));
    let git_wire_credentials = Arc::new(myelin_edge::IdentityGitWireCredentialIssuerFactory::new(
        check.clone(),
    ));

    let git_shutdown = Arc::new(AtomicBool::new(false));
    let git_backend = Arc::new(
        DurableGitBackend::rooted(
            git_root,
            public_base_url.clone(),
            GitDatabaseProviders::new(provider.clone(), git_check_admission_provider),
            kms.clone(),
            handle.clone(),
            git_outbox,
            git_minter,
        )
        .unwrap_or_else(|error| {
            eprintln!("edge: PostgreSQL Git PR store refused to construct: {error}");
            std::process::exit(1);
        })
        .with_repo_authorizer(repo_authz.clone())
        .with_repo_bootstrap(repo_bootstrap)
        .with_git_wire_credential_issuer(git_wire_credentials)
        .with_git_shutdown_signal(git_shutdown.clone()),
    );
    let recovery = recover_placed_git_at_boot(&git_backend, &provider, &cell_id)
        .await
        .unwrap_or_else(|error| {
            eprintln!("edge: durable Git boot recovery failed: {error}");
            std::process::exit(1);
        });
    eprintln!(
        "edge: Git recovery complete (tenants={}, repos={}, refs={}, merges={})",
        recovery.tenants_recovered,
        recovery.repos_reconciled,
        recovery.refs_reapplied,
        recovery.merges_recovered
    );

    let builder = Gateway::builder(
        authn,
        human_login,
        Arc::new(AuthenticatedActionPolicy::mounted()),
    )
    .default_token_scheme(EDGE_DEFAULT_TOKEN_SCHEME)
    .with_human_session_issuer(cell.clone());
    let builder = match device_authorization {
        Some(broker) => builder.with_device_authorization(broker),
        None => {
            eprintln!("edge: interactive CLI login disabled - MYELIN_WEB_PUBLIC_URL is not set");
            builder
        }
    };
    let mut builder = builder
        .with_public_base_url(public_base_url)
        .unwrap_or_else(|error| {
            eprintln!("edge: invalid public base URL at gateway composition: {error}");
            std::process::exit(1);
        })
        .with_auth_config(auth_config)
        .route(
            Method::Get,
            "/v1/whoami",
            "edge.whoami",
            Arc::new(WhoamiHandler),
        )
        .route(
            Method::Get,
            "/v1/t/{tenant}/whoami",
            "edge.whoami",
            Arc::new(WhoamiHandler),
        )
        .sse_route("/v1/t/{tenant}/events", "edge.events.subscribe", "edge");
    builder = register_git_durable(builder, git_backend.clone());
    let ci_blobs: Arc<dyn BlobStore + Send + Sync> = Arc::from(provider.blob_store(handle.clone()));
    let ci_runs = myelin_ci_controlplane::CiRunStore::with_pg_surface_cursor_key(
        provider.db_pool().clone(),
        ci_surface_cursor_key,
    );
    let mcp_ci = DurableCiReadApi::new(
        ci_runs.clone(),
        git_backend.clone(),
        ci_blobs.clone(),
        handle.clone(),
    );
    builder = register_ci(
        builder,
        ci_runs,
        git_backend.clone(),
        ci_blobs,
        handle.clone(),
    );
    if git_wire.is_some() {
        builder = register_git_wire(builder, git_backend.clone());
    }
    let issue_mutations = myelin_edge::DurableIssueMutationApi::new(
        issue_store.clone(),
        myelin_identity_service::PgProjectStore::new(provider.clone()),
        issue_authorizer.clone(),
        issue_reconciler.wakeup(),
        handle.clone(),
    );
    builder = register_issues(builder, issue_mutations.clone());
    builder = register_projects(
        builder,
        myelin_identity_service::PgProjectStore::new(provider.clone()),
        check.clone(),
        handle.clone(),
    );
    builder = register_refs(
        builder,
        DurableRefsReadApi::new(provider.db_pool().clone(), repo_authz, handle.clone()),
    );
    let agent_registry = myelin_identity_service::PgAgentRegistry::new(provider.clone());
    let agent_sessions = myelin_identity_service::AgentSessionIssuer::new(
        provider.clone(),
        check.clone(),
        thresholds.fail_static.agent_token_ttl_secs,
    )
    .unwrap_or_else(|error| {
        eprintln!("edge: external agent session issuer refused to start: {error}");
        std::process::exit(1);
    });
    builder = register_agents(
        builder,
        agent_registry.clone(),
        agent_sessions.clone(),
        handle.clone(),
    );
    let inbox_store = Arc::new(PgInboxStore::new(provider.db_pool().clone()));
    let agent_traces = myelin_storage::DurableAgentTraceStore::with_runtime(
        provider.clone(),
        handle.clone(),
        kms.clone(),
    );
    builder = myelin_edge::register_triggers(
        builder,
        provider.clone(),
        myelin_storage::DurableAgentTriggerBacking::new(provider.clone()),
        agent_registry.clone(),
        agent_traces.clone(),
        inbox_store.clone(),
        handle.clone(),
    );
    builder = register_privacy(builder, agent_traces, handle.clone());
    let mcp_chat = DurableChatReadApi::new(
        provider.db_pool().clone(),
        handle.clone(),
        kms.clone(),
        check.clone(),
    );
    builder = register_agent_mcp(
        builder,
        AgentMcpServices::new(
            AgentMcpAuthority::new(
                agent_registry,
                agent_sessions,
                check.run_token_minter().clone(),
                Arc::new(run_token_authorizer),
                mcp_principals,
                myelin_storage::DurableAgentTriggerBacking::new(provider.clone()),
            ),
            provider.clone(),
            AgentMcpResources::new(
                git_backend.clone(),
                mcp_ci,
                issue_mutations,
                DurableKnowledgeReadApi::new(
                    provider.db_pool().clone(),
                    handle.clone(),
                    kms.clone(),
                ),
                mcp_chat.clone(),
                DurableChatMutationApi::new(mcp_chat),
            ),
            handle.clone(),
        ),
    );
    builder = register_tools(builder);
    builder = register_chat(
        builder,
        provider.db_pool().clone(),
        handle.clone(),
        kms.clone(),
        check.clone(),
    );
    builder = register_knowledge(
        builder,
        provider.db_pool().clone(),
        handle.clone(),
        kms.clone(),
    );
    builder = register_notif(
        builder,
        inbox_store,
        check.clone(),
        git_backend,
        handle.clone(),
    );
    let gateway = Arc::new(builder.build());

    let listener = match tokio::net::TcpListener::bind(&listen_addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("edge: failed to bind {listen_addr}: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("edge: listening on {listen_addr}");
    let git_shutdown_for_signal = git_shutdown.clone();
    let server_result = serve_edge_until_shutdown_with_probe(
        listener,
        gateway,
        readiness,
        async move {
            let result = shutdown_signal().await;
            git_shutdown_for_signal.store(true, Ordering::Release);
            result
        },
        EDGE_SHUTDOWN_GRACE,
    )
    .await
    .map_err(|error| format!("serve error: {error}"))
    .and_then(|(outcome, signal_result)| {
        if let ShutdownOutcome::Forced { connections } = outcome {
            eprintln!(
                "edge: forced {connections} active HTTP connection(s) closed after the {}s shutdown grace",
                EDGE_SHUTDOWN_GRACE.as_secs()
            );
        }
        signal_result
    });
    let reconciliation_result = issue_reconciler.shutdown().await;
    if let Err(e) = reconciliation_result {
        eprintln!("edge: Issues authorization reconciler did not drain cleanly: {e}");
        std::process::exit(1);
    }
    if let Err(e) = server_result {
        eprintln!("edge: {e}");
        std::process::exit(1);
    }
}

async fn shutdown_signal() -> Result<(), String> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .map_err(|error| format!("failed to install SIGTERM handler: {error}"))?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                result.map_err(|error| format!("failed while waiting for SIGINT: {error}"))
            }
            signal = terminate.recv() => {
                signal
                    .map(|_| ())
                    .ok_or_else(|| "SIGTERM stream closed unexpectedly".to_string())
            }
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .map_err(|error| format!("failed while waiting for shutdown signal: {error}"))
    }
}

async fn operator_bootstrap(core: ComposedCore, args: &[String]) {
    let ComposedCore {
        provider,
        kms,
        cell,
        handle,
        ..
    } = core;

    let tenant = required_flag(args, "--tenant");
    let principal = required_flag(args, "--principal");
    let issues_project = required_flag(args, "--issues-project");
    if !myelin_issues::api::is_canonical_uuid(&issues_project) {
        eprintln!("edge bootstrap: --issues-project must be a canonical lowercase UUID");
        std::process::exit(2);
    }
    let display_name = flag(args, "--display-name");
    let region = flag(args, "--region").unwrap_or_else(default_region);
    let ttl_days: u32 = flag(args, "--ttl-days")
        .map(|v| {
            v.parse().unwrap_or_else(|_| {
                eprintln!("edge bootstrap: --ttl-days must be a non-negative integer, got `{v}`");
                std::process::exit(2);
            })
        })
        .unwrap_or(30);

    let store = PrincipalStore::with_pg(
        kms.clone(),
        DurablePrincipalBacking::new(provider.clone()),
        handle.clone(),
    );
    let tuples = TupleStore::with_pg(DurableTupleBacking::new(provider.clone()), handle.clone());
    let now_unix = now_unix();
    let outcome = bootstrap_principal_and_mint(
        &store,
        &tuples,
        &cell,
        &BootstrapParams {
            tenant: &tenant,
            region: &region,
            principal: &principal,
            issues_project: &issues_project,
            display: display_name.as_deref(),
            ttl_days,
        },
        now_unix,
    )
    .unwrap_or_else(|e| {
        eprintln!("edge bootstrap: {e}");
        std::process::exit(1);
    });

    println!("{}", outcome.token);
    eprintln!("edge bootstrap: minted an operator capability token");
    eprintln!("  tenant       = {}", outcome.tenant);
    eprintln!("  region       = {}", outcome.region);
    eprintln!("  principal    = {}", outcome.principal_id);
    eprintln!("  issues grant = project:{issues_project}#reader");
    eprintln!("  subject_key  = {}", outcome.subject_key);
    eprintln!("  jti          = {}", outcome.jti);
    eprintln!("  expiry_unix  = {}", outcome.expiry_unix);
    eprintln!(
        "  scheme       = {} (send `Authorization: Bearer <token>` or, on the git wire, HTTP Basic \
         with the token as the password)",
        myelin_edge::BOOTSTRAP_SCHEME
    );
    eprintln!(
        "  revoke with  : edge revoke --jti {} --tenant {}",
        outcome.jti, outcome.tenant
    );
}

async fn operator_revoke(core: ComposedCore, args: &[String]) {
    let ComposedCore {
        provider, handle, ..
    } = core;

    let jti = required_flag(args, "--jti");
    let tenant = required_flag(args, "--tenant");
    let region = flag(args, "--region").unwrap_or_else(default_region);

    let revocations = RevocationStore::with_pg(
        DurableRevocationBacking::new(provider.clone()),
        handle.clone(),
    );
    let scope = TenantScope::from_verified_token(
        &Principal::stub(
            PrincipalId("revoke-operator".into()),
            PrincipalKind::Human,
            TenantId(tenant.clone()),
        ),
        Region(region.clone()),
    );
    revocations.revoke(&scope, &RevokeTarget::Jti(jti.clone()), now_rfc3339());
    eprintln!("edge revoke: token jti `{jti}` revoked in tenant `{tenant}` region `{region}` (durable S7 denylist - the deny survives restart)");
}

async fn operator_secret(core: ComposedCore, args: &[String]) {
    let ComposedCore {
        provider,
        kms,
        cell,
        handle,
        ..
    } = core;

    let Some(operation) = args.first().map(String::as_str) else {
        secret_usage_and_exit();
    };
    let operation_args = &args[1..];
    validate_secret_operation_args(operation, operation_args)
        .unwrap_or_else(|error| secret_error_and_exit(error));
    let tenant = required_flag(operation_args, "--tenant");
    let project = match operation {
        "list" => flag(operation_args, "--project"),
        "create" | "update" | "rotate" | "delete" | "grant-binding" | "revoke-binding" => {
            Some(required_flag(operation_args, "--project"))
        }
        _ => secret_usage_and_exit(),
    };
    let name = match operation {
        "list" => None,
        _ => Some(required_flag(operation_args, "--name")),
    };
    let scope = match operation {
        "grant-binding" | "revoke-binding" => Some(required_flag(operation_args, "--scope")),
        _ => None,
    };

    let target = || SecretTarget {
        tenant: &tenant,
        project: project
            .as_deref()
            .expect("non-list commands require project"),
        name: name.as_deref().expect("non-list commands require name"),
    };
    let command = match operation {
        "create" => SecretCommand::Create(target()),
        "update" => SecretCommand::Update(target()),
        "rotate" => SecretCommand::Rotate(target()),
        "delete" => SecretCommand::Delete(target()),
        "list" => SecretCommand::List {
            tenant: &tenant,
            project: project.as_deref(),
        },
        "grant-binding" => SecretCommand::GrantBinding {
            target: target(),
            scope: scope.as_deref().expect("binding command requires scope"),
        },
        "revoke-binding" => SecretCommand::RevokeBinding {
            target: target(),
            scope: scope.as_deref().expect("binding command requires scope"),
        },
        _ => unreachable!("operation was validated above"),
    };

    let credential = std::env::var("MYELIN_TOKEN")
        .ok()
        .filter(|token| !token.is_empty())
        .map(|material| Credential {
            scheme: std::env::var("MYELIN_TOKEN_SCHEME")
                .unwrap_or_else(|_| EDGE_DEFAULT_TOKEN_SCHEME.to_owned()),
            material,
        });

    let (authenticator, _) = durable_capability_authenticator(&provider, &kms, &cell, &handle);
    let identity = Arc::new(StoreBackedCheck::with_pg(
        provider.clone(),
        kms.clone(),
        cell,
        handle.clone(),
    ));
    for admission in identity.admit_git_fragment() {
        if let FragmentAdmit::Rejected { reason } = admission {
            eprintln!("edge secret: authorization schema is unavailable: {reason}");
            std::process::exit(1);
        }
    }
    for admission in identity.admit_ci_fragment() {
        if let FragmentAdmit::Rejected { reason } = admission {
            eprintln!("edge secret: authorization schema is unavailable: {reason}");
            std::process::exit(1);
        }
    }
    let secret_store = Arc::new(myelin_ci_controlplane::DurableCiSecretStore::with_pg(
        provider.db_pool().clone(),
        kms,
        Region::new(provider.config().region.clone()),
        handle,
    ));
    let mut stdin = std::io::stdin().lock();
    let output = execute_secret_command(
        &authenticator,
        identity,
        secret_store,
        credential,
        command,
        &mut stdin,
    )
    .unwrap_or_else(|error| secret_error_and_exit(error));
    println!("{}", output.render());
}

fn durable_capability_authenticator(
    provider: &SubstrateProvider,
    kms: &Arc<KmsEngine>,
    cell: &Arc<CellTokenAuthority>,
    handle: &tokio::runtime::Handle,
) -> (
    CapabilityAuthenticator,
    myelin_identity_service::mint::RunTokenAuthorizer,
) {
    let verifier: Arc<dyn TokenVerifier> = Arc::new(
        PasetoCapabilityVerifier::new(cell.trust_anchor()).with_replay_guard(
            DpopReplayGuard::with_pg(DurableReplayBacking::new(provider.clone()), handle.clone()),
        ),
    );
    let revocations = RevocationStore::with_pg(
        DurableRevocationBacking::new(provider.clone()),
        handle.clone(),
    );
    let authenticator = CapabilityAuthenticator::with_verifier(
        PrincipalStore::with_pg(
            kms.clone(),
            DurablePrincipalBacking::new(provider.clone()),
            handle.clone(),
        ),
        verifier.clone(),
        revocations.clone(),
    );
    (
        authenticator,
        myelin_identity_service::mint::RunTokenAuthorizer::new(verifier, revocations),
    )
}

fn secret_error_and_exit(error: SecretCommandError) -> ! {
    eprintln!("edge secret: {error}");
    std::process::exit(error.exit_code());
}

fn secret_usage_and_exit() -> ! {
    eprintln!(
        "usage: edge secret {{create|update|rotate|delete|list|grant-binding|revoke-binding}} \
         --tenant <tenant> [--project <uuid>] [--name <name>] [--scope project|job:<uuid>]"
    );
    std::process::exit(2);
}

fn validate_secret_operation_args(
    operation: &str,
    args: &[String],
) -> Result<(), SecretCommandError> {
    let allowed = match operation {
        "create" | "update" | "rotate" | "delete" => &["--tenant", "--project", "--name"][..],
        "list" => &["--tenant", "--project"][..],
        "grant-binding" | "revoke-binding" => &["--tenant", "--project", "--name", "--scope"][..],
        _ => return Err(SecretCommandError::BadParam("unknown secret operation")),
    };
    let mut seen = Vec::with_capacity(allowed.len());
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        if !argument.starts_with("--") {
            return Err(SecretCommandError::BadParam(
                "positional arguments are not permitted",
            ));
        }
        let (name, inline_value) = argument
            .split_once('=')
            .map_or((argument.as_str(), None), |(name, value)| {
                (name, Some(value))
            });
        if !allowed.contains(&name) {
            return Err(SecretCommandError::BadParam(
                "unknown flag is not permitted for this operation",
            ));
        }
        if seen.contains(&name) {
            return Err(SecretCommandError::BadParam(
                "duplicate flags are not permitted",
            ));
        }
        seen.push(name);

        match inline_value {
            Some("") => {
                return Err(SecretCommandError::BadParam(
                    "flag values must be non-empty",
                ));
            }
            Some(_) => index += 1,
            None => {
                let Some(value) = args.get(index + 1) else {
                    return Err(SecretCommandError::BadParam("flag requires a value"));
                };
                if value.starts_with("--") {
                    return Err(SecretCommandError::BadParam("flag requires a value"));
                }
                index += 2;
            }
        }
    }
    Ok(())
}

fn flag(args: &[String], name: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == name {
            return it.next().cloned();
        }
        if let Some(v) = a.strip_prefix(&format!("{name}=")) {
            return Some(v.to_string());
        }
    }
    None
}

fn required_flag(args: &[String], name: &str) -> String {
    flag(args, name).unwrap_or_else(|| {
        eprintln!("edge: missing required flag `{name}`");
        std::process::exit(2);
    })
}

fn default_region() -> String {
    std::env::var("MYELIN_REGION").unwrap_or_else(|_| "fr-par".to_string())
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn now_rfc3339() -> Timestamp {
    let dt = chrono::DateTime::from_timestamp(now_unix(), 0).unwrap_or_default();
    Timestamp(dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

#[cfg(test)]
mod runtime_config_tests {
    use super::*;
    use std::collections::HashMap;

    fn parse(
        serving: bool,
        values: &[(&'static str, String)],
    ) -> Result<EdgeRuntimeConfig, String> {
        let values = values.iter().cloned().collect::<HashMap<_, _>>();
        EdgeRuntimeConfig::from_reader(serving, |name| {
            values.get(name).cloned().ok_or(VarError::NotPresent)
        })
    }

    fn git_wire_fixture() -> (String, String) {
        let fixture_id = READINESS_PROBE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let rootfs = std::env::current_dir()
            .unwrap()
            .join(format!("target/edge-runtime-config-rootfs-{fixture_id}"));
        let guest_git = rootfs.join("usr/bin/git");
        std::fs::create_dir_all(guest_git.parent().unwrap()).unwrap();
        std::fs::copy("/bin/true", &guest_git).unwrap();
        let runsc = rootfs.join("runsc-fixture");
        std::fs::write(&runsc, "#!/bin/sh\necho 'runsc version test-fixture'\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&runsc, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        (
            rootfs.canonicalize().unwrap().display().to_string(),
            runsc.canonicalize().unwrap().display().to_string(),
        )
    }

    #[test]
    fn edge_secret_create_rejects_unknown_flags_and_positional_arguments() {
        let base = [
            "--tenant".to_string(),
            "tenant-a".to_string(),
            "--project".to_string(),
            "11111111-1111-4111-8111-111111111111".to_string(),
            "--name".to_string(),
            "DEPLOY_KEY".to_string(),
        ];
        assert!(validate_secret_operation_args("create", &base).is_ok());

        let mut unknown_flag = base.to_vec();
        unknown_flag.extend(["--material".to_string(), "SECRET".to_string()]);
        let error = validate_secret_operation_args("create", &unknown_flag).unwrap_err();
        assert_eq!(error.exit_code(), 2);
        assert_eq!(
            error.to_string(),
            "secret parameter error: unknown flag is not permitted for this operation"
        );
        assert!(!error.to_string().contains("SECRET"));

        let mut positional = base.to_vec();
        positional.push("SECRET".to_string());
        let error = validate_secret_operation_args("create", &positional).unwrap_err();
        assert_eq!(error.exit_code(), 2);
        assert_eq!(
            error.to_string(),
            "secret parameter error: positional arguments are not permitted"
        );
        assert!(!error.to_string().contains("SECRET"));
    }

    #[test]
    fn serving_requires_explicit_cell_and_persistent_git_root() {
        assert_eq!(
            parse(true, &[]).unwrap_err(),
            "required env var MYELIN_CELL_ID is not set"
        );
        assert_eq!(
            parse(true, &[("MYELIN_CELL_ID", "cell-eu-1".into())]).unwrap_err(),
            "required env var MYELIN_GIT_ROOT is not set"
        );

        let root = std::env::current_dir().unwrap().canonicalize().unwrap();
        let (rootfs, runsc) = git_wire_fixture();
        let config = parse(
            true,
            &[
                ("MYELIN_CELL_ID", " cell-eu-1 ".into()),
                ("MYELIN_GIT_ROOT", root.display().to_string()),
                ("MYELIN_GVISOR_GIT_ROOTFS", rootfs),
                ("MYELIN_RUNSC_BIN", runsc),
                (
                    "MYELIN_PUBLIC_BASE_URL",
                    "https://myelin.example/base/".into(),
                ),
            ],
        )
        .unwrap();
        assert_eq!(config.cell_id, "cell-eu-1");
        assert_eq!(config.git_root.as_deref(), Some(root.as_path()));
        assert!(config.git_wire.is_some());
        assert_eq!(
            config.public_base_url.as_deref(),
            Some("https://myelin.example/base")
        );
    }

    #[test]
    fn operator_commands_require_cell_identity_but_do_not_open_git_storage() {
        let config = parse(false, &[("MYELIN_CELL_ID", "cell-eu-1".into())]).unwrap();
        assert_eq!(config.cell_id, "cell-eu-1");
        assert_eq!(config.git_root, None);
        assert_eq!(config.git_wire, None);
        assert_eq!(config.public_base_url, None);
        assert_eq!(config.listen_addr, None);
        assert!(!config.dev_login_enabled);
        assert!(!config.token_login_enabled);
    }

    #[test]
    fn serving_public_base_url_is_absolute_and_query_free() {
        assert_eq!(
            validated_public_base_url(Ok("ftp://myelin.example".into())).unwrap_err(),
            "MYELIN_PUBLIC_BASE_URL must be an absolute HTTP(S) URL"
        );
        assert_eq!(
            validated_public_base_url(Ok("/relative".into())).unwrap_err(),
            "MYELIN_PUBLIC_BASE_URL must be an absolute HTTP(S) URL"
        );
        assert_eq!(
            validated_public_base_url(Ok("https://myelin.example/?tenant=acme".into()))
                .unwrap_err(),
            "MYELIN_PUBLIC_BASE_URL must not contain a query string"
        );
        assert_eq!(
            validated_public_base_url(Ok("https://operator:secret@myelin.example".into()))
                .unwrap_err(),
            "MYELIN_PUBLIC_BASE_URL must not contain credentials"
        );
    }

    #[test]
    fn configured_oidc_verifier_is_advertised_to_interactive_web_clients() {
        let config = public_auth_config(true, false, true);
        assert!(config.sso_configured);
        assert_eq!(config.providers.len(), 1);
        assert_eq!(config.providers[0].id, "oidc");
        assert!(!config.dev_login_enabled);
        assert!(config.token_login_enabled);
    }

    #[test]
    fn development_login_is_confined_to_debug_loopback_servers() {
        assert_eq!(validated_dev_login(false, false, None), Ok(false));
        assert_eq!(
            validated_dev_login(true, false, Some("127.0.0.1:8080")).unwrap_err(),
            "MYELIN_DEV_LOGIN=1 is available only in debug builds"
        );
        for public_bind in ["0.0.0.0:8080", "[::]:8080", "edge.internal:8080"] {
            assert_eq!(
                validated_dev_login(true, true, Some(public_bind)).unwrap_err(),
                "MYELIN_DEV_LOGIN=1 requires MYELIN_EDGE_ADDR to be a numeric loopback address"
            );
        }
        for loopback_bind in ["127.0.0.1:8080", "[::1]:8080"] {
            assert_eq!(
                validated_dev_login(true, true, Some(loopback_bind)),
                Ok(true)
            );
        }
    }

    #[test]
    fn oidc_jwks_uri_requires_credential_free_https() {
        let uri =
            validated_oidc_jwks_uri("https://idp.example.com/.well-known/jwks.json?version=2")
                .expect("provider HTTPS URI should be accepted");
        assert_eq!(uri.scheme_str(), Some("https"));
        assert_eq!(
            uri.authority().map(|value| value.host()),
            Some("idp.example.com")
        );

        for (raw, expected) in [
            ("http://idp.example.com/jwks", "must use https"),
            (
                "https://operator:secret@idp.example.com/jwks",
                "must not contain credentials",
            ),
            (
                "https://idp.example.com/jwks#old",
                "must not contain a fragment",
            ),
            ("/relative/jwks", "must use https"),
        ] {
            let error = validated_oidc_jwks_uri(raw).unwrap_err();
            assert!(
                error.contains(expected),
                "unexpected error for {raw}: {error}"
            );
            assert!(!error.contains("secret"), "URI credentials leaked: {error}");
        }
    }

    #[test]
    fn oidc_issuer_requires_a_query_free_https_identifier() {
        assert_eq!(
            validated_oidc_issuer("https://idp.example.com/tenant").unwrap(),
            "https://idp.example.com/tenant"
        );
        for (raw, expected) in [
            ("http://idp.example.com", "must use https"),
            ("https://user:secret@idp.example.com", "credentials"),
            ("https://idp.example.com?tenant=acme", "query string"),
            ("https://idp.example.com#issuer", "fragment"),
        ] {
            let error = validated_oidc_issuer(raw).unwrap_err();
            assert!(error.contains(expected));
            assert!(!error.contains("secret"));
        }
    }

    #[test]
    fn oidc_jwks_response_is_strict_and_body_opaque() {
        let document = br#"{"keys":[{"kty":"OKP","crv":"Ed25519","kid":"ed-1","alg":"EdDSA","x":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}]}"#;
        let keys = parse_oidc_jwks_response(
            hyper::StatusCode::OK,
            Some("Application/JWK-Set+JSON; charset=utf-8"),
            document,
        )
        .expect("valid RFC 7517 response should parse");
        assert_eq!(keys.len(), 1);

        for (status, content_type, body) in [
            (
                hyper::StatusCode::FOUND,
                Some("application/jwk-set+json"),
                document.as_slice(),
            ),
            (
                hyper::StatusCode::OK,
                Some("text/html"),
                document.as_slice(),
            ),
            (
                hyper::StatusCode::OK,
                Some("application/json"),
                b"TOP_SECRET malformed".as_slice(),
            ),
            (
                hyper::StatusCode::OK,
                Some("application/json"),
                br#"{"keys":[]}"#.as_slice(),
            ),
        ] {
            let error = parse_oidc_jwks_response(status, content_type, body).unwrap_err();
            assert!(
                !error.contains("TOP_SECRET"),
                "response body leaked: {error}"
            );
        }
    }

    #[test]
    fn serving_requires_an_explicit_usable_git_wire_sandbox() {
        let root = std::env::current_dir().unwrap().canonicalize().unwrap();
        let base = [
            ("MYELIN_CELL_ID", "cell-eu-1".into()),
            ("MYELIN_GIT_ROOT", root.display().to_string()),
        ];
        assert_eq!(
            parse(true, &base).unwrap_err(),
            "required env var MYELIN_GVISOR_GIT_ROOTFS is not set"
        );

        let (rootfs, _) = git_wire_fixture();
        let error = parse(
            true,
            &[
                base[0].clone(),
                base[1].clone(),
                ("MYELIN_GVISOR_GIT_ROOTFS", rootfs),
            ],
        )
        .unwrap_err();
        assert_eq!(error, "required env var MYELIN_RUNSC_BIN is not set");

        let error = parse(
            true,
            &[
                base[0].clone(),
                base[1].clone(),
                ("MYELIN_GVISOR_GIT_ROOTFS", git_wire_fixture().0),
                (
                    "MYELIN_RUNSC_BIN",
                    std::fs::canonicalize("/bin/true")
                        .unwrap()
                        .display()
                        .to_string(),
                ),
            ],
        )
        .unwrap_err();
        assert_eq!(error, "MYELIN_RUNSC_BIN did not identify itself as runsc");
    }

    #[test]
    fn serving_can_explicitly_disable_git_wire_without_sandbox_host_assets() {
        let root = std::env::current_dir().unwrap().canonicalize().unwrap();
        let config = parse(
            true,
            &[
                ("MYELIN_CELL_ID", "cell-dev".into()),
                ("MYELIN_GIT_ROOT", root.display().to_string()),
                ("MYELIN_GIT_WIRE_ENABLED", "0".into()),
                ("MYELIN_PUBLIC_BASE_URL", "http://127.0.0.1:8080".into()),
            ],
        )
        .unwrap();

        assert_eq!(config.git_wire, None);
    }

    #[test]
    fn git_wire_switch_rejects_ambiguous_values() {
        let root = std::env::current_dir().unwrap().canonicalize().unwrap();
        let error = parse(
            true,
            &[
                ("MYELIN_CELL_ID", "cell-dev".into()),
                ("MYELIN_GIT_ROOT", root.display().to_string()),
                ("MYELIN_GIT_WIRE_ENABLED", "false".into()),
            ],
        )
        .unwrap_err();

        assert_eq!(error, "env var MYELIN_GIT_WIRE_ENABLED must be `0` or `1`");
    }

    #[test]
    fn serving_rejects_ephemeral_or_ambiguous_git_roots() {
        let current = std::env::current_dir().unwrap();
        let cases = [
            ("relative/git".to_string(), "absolute persistent path"),
            ("/".to_string(), "filesystem root"),
            (
                std::env::temp_dir().display().to_string(),
                "operating-system temp directory",
            ),
            (
                current.join("state/../git").display().to_string(),
                "must not contain `.` or `..`",
            ),
            (
                current
                    .join("definitely-does-not-exist")
                    .display()
                    .to_string(),
                "existing persistent directory",
            ),
        ];

        for (root, expected) in cases {
            let error = parse(
                true,
                &[
                    ("MYELIN_CELL_ID", "cell-eu-1".into()),
                    ("MYELIN_GIT_ROOT", root),
                ],
            )
            .unwrap_err();
            assert!(error.contains(expected), "unexpected error: {error}");
        }
    }

    #[test]
    fn runtime_identity_rejects_empty_and_non_utf8_values() {
        assert_eq!(
            required_runtime_value("MYELIN_CELL_ID", Ok(" \t".into())).unwrap_err(),
            "env var MYELIN_CELL_ID must not be empty"
        );
        assert_eq!(
            required_runtime_value(
                "MYELIN_CELL_ID",
                Err(VarError::NotUnicode(std::ffi::OsString::from("invalid")))
            )
            .unwrap_err(),
            "env var MYELIN_CELL_ID is not valid UTF-8"
        );
    }

    #[test]
    fn git_readiness_proves_a_directory_is_durably_writable() {
        let root = std::env::current_dir().unwrap();
        assert!(git_root_is_writable(&root));
        assert!(!git_root_is_writable(&root.join("Cargo.toml")));
        assert!(!git_root_is_writable(
            &root.join("definitely-does-not-exist")
        ));
    }
}

use myelin_edge::{
    register_git_durable, serve_edge, AllowAll, DurableGitBackend, Gateway, Method, WhoamiHandler,
};
use myelin_identity::{DataRole, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_identity_service::{
    CapabilityAuthenticator, CapabilityMintSpec, CellTokenAuthority, HumanSsoAuthenticator,
    PasetoCapabilityVerifier, PrincipalStore, RevocationStore,
};
use myelin_storage::{KmsEngine, TenantScope};
use myelin_tenancy::{Region, TenantId};
use std::net::SocketAddr;
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::TempDir;
use tokio::net::TcpListener;

const REGION: &str = "eu-west";
const SCHEME: &str = "agent";

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn admin_scope(tenant: &str) -> TenantScope {
    TenantScope::from_verified_token(
        &myelin_identity::Principal::stub(
            PrincipalId("admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        ),
        Region(REGION.into()),
    )
}

fn seed_tenant(store: &PrincipalStore, tenant: &str) {
    let scope = admin_scope(tenant);
    store
        .put_principal(
            &scope,
            PrincipalId("svc:agent".into()),
            PrincipalKind::Service,
            DataRole::Controller,
            PrincipalStatus::Active,
            None,
        )
        .expect("seed principal");
    store
        .link_credential(&scope, SCHEME, "subj-1", &PrincipalId("svc:agent".into()))
        .expect("link credential");
}

fn build() -> (Arc<Gateway>, CellTokenAuthority, TempDir) {
    let cell = CellTokenAuthority::from_seed(&[7u8; 32], &[9u8; 32]).expect("cell authority");
    let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
    seed_tenant(&store, "acme");

    let authn = Arc::new(CapabilityAuthenticator::with_verifier(
        store,
        Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
        RevocationStore::new(),
    ));
    let human_login = Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(
        Arc::new(KmsEngine::new()),
    )));

    let root = tempfile::tempdir().expect("temporary Git root");
    let git = Arc::new(DurableGitBackend::rooted_inmem_for_test(root.path()));
    let mut builder = Gateway::builder(authn, human_login, Arc::new(AllowAll)).route(
        Method::Get,
        "/v1/whoami",
        "edge.whoami",
        Arc::new(WhoamiHandler::default()),
    );
    builder = register_git_durable(builder, git);
    (Arc::new(builder.build()), cell, root)
}

fn mint(cell: &CellTokenAuthority, tenant: &str, jti: &str) -> String {
    cell.mint(&CapabilityMintSpec {
        tenant: tenant.into(),
        region: REGION.into(),
        subject_key: "subj-1".into(),
        jti: jti.into(),
        issued_at_unix: Some(now()),
        exp_unix: now() + 3600,
        authority: vec!["edge.operator".into()],
        dpop_jkt: None,
        purpose: myelin_identity_service::CredentialPurpose::OperatorBootstrap,
        audience: myelin_identity_service::CredentialAudience::Edge,
    })
}

async fn spawn(gateway: Arc<Gateway>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = serve_edge(listener, gateway).await;
    });
    addr
}

fn run_cli(edge: &str, token: Option<&str>, args: &[&str]) -> (i32, String, String) {
    let bin = env!("CARGO_BIN_EXE_myelin");
    let mut cmd = Command::new(bin);
    cmd.env("MYELIN_EDGE", edge)
        .env("MYELIN_TOKEN_SCHEME", SCHEME)
        .env(
            "MYELIN_CONFIG_DIR",
            std::env::temp_dir().join("myelin-cli-it-empty"),
        );
    match token {
        Some(t) => {
            cmd.env("MYELIN_TOKEN", t);
        }
        None => {
            cmd.env_remove("MYELIN_TOKEN");
        }
    }
    cmd.args(args);
    let out = cmd.output().expect("spawn myelin binary");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forged_token_is_a_clean_unauthenticated_error_not_a_panic() {
    let (gw, _cell, _root) = build();
    let addr = spawn(gw).await;
    let edge = format!("http://{addr}");
    let forged = "acme|eu-west|subj-1|jti|0|agent:run";

    let (code, stdout, stderr) = run_cli(&edge, Some(forged), &["git", "repo", "list"]);
    assert_ne!(code, 0, "a forged token exits non-zero");
    assert_eq!(code, 3, "unauthenticated is the stable exit code 3");
    assert!(
        stderr.to_lowercase().contains("not authenticated") || stderr.contains("token invalid"),
        "a clean unauthenticated message (not a panic); stderr={stderr}"
    );
    assert!(!stderr.contains("panicked"), "never a panic");
    assert!(stdout.is_empty(), "no view-model on an auth failure");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_token_is_a_clean_unauthenticated_error() {
    let (gw, _cell, _root) = build();
    let addr = spawn(gw).await;
    let edge = format!("http://{addr}");

    let (code, _stdout, stderr) = run_cli(&edge, None, &["git", "repo", "list"]);
    assert_eq!(code, 3, "no token → exit 3");
    assert!(
        stderr.to_lowercase().contains("not authenticated"),
        "stderr={stderr}"
    );
    assert!(
        stderr.contains("login"),
        "the hint points at `myelin login`"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn whoami_renders_the_verified_principal() {
    let (gw, cell, _root) = build();
    let addr = spawn(gw).await;
    let edge = format!("http://{addr}");
    let token = mint(&cell, "acme", "jti-whoami");

    let (code, stdout, stderr) = run_cli(&edge, Some(&token), &["whoami"]);
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(
        stdout.contains("svc:agent"),
        "the verified principal id; got {stdout}"
    );
    assert!(
        stdout.contains("tenant=acme"),
        "the tenant is the verified token's; got {stdout}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_token_is_never_echoed_to_stdout_or_stderr() {
    let (gw, cell, _root) = build();
    let addr = spawn(gw).await;
    let edge = format!("http://{addr}");
    let token = mint(&cell, "acme", "jti-secret");

    let (_c, out, err) = run_cli(&edge, Some(&token), &["git", "repo", "list"]);
    assert!(
        !out.contains(&token) && !err.contains(&token),
        "token never appears on success"
    );

    let (_c2, out2, err2) = run_cli(&edge, Some(&token), &["git", "frobnicate"]);
    assert!(
        !out2.contains(&token) && !err2.contains(&token),
        "token never appears on an error"
    );
}

#[test]
fn issues_malformed_input_is_a_local_usage_exit() {
    let token = "NOT_USED_SENSITIVE_TOKEN";
    let (code, stdout, stderr) = run_cli(
        "http://127.0.0.1:1",
        Some(token),
        &["issues", "list", "--limit", "0"],
    );

    assert_eq!(code, 2);
    assert!(stdout.is_empty());
    assert!(stderr.contains("malformed limit"), "stderr={stderr}");
    assert!(!stderr.contains(token));
    assert!(
        !stderr.contains("could not reach"),
        "parsing precedes transport"
    );
}

/// A gateway that knows one person, and a browser-style session for them.
fn person_with_session() -> (Arc<Gateway>, String) {
    let cell = CellTokenAuthority::from_seed(&[5u8; 32], &[6u8; 32]).expect("cell authority");
    let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
    store
        .put_principal(
            &admin_scope("acme"),
            PrincipalId("person-ada".into()),
            PrincipalKind::Human,
            DataRole::Controller,
            PrincipalStatus::Active,
            None,
        )
        .expect("seed person");
    let authn = Arc::new(CapabilityAuthenticator::with_verifier(
        store,
        Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
        RevocationStore::new(),
    ));
    let human_login = Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(
        Arc::new(KmsEngine::new()),
    )));
    let gateway = Gateway::builder(authn, human_login, Arc::new(AllowAll))
        .default_token_scheme("session")
        .route(
            Method::Get,
            "/v1/whoami",
            "edge.whoami",
            Arc::new(WhoamiHandler::default()),
        )
        .build();
    let token = cell.mint(&CapabilityMintSpec {
        tenant: "acme".into(),
        region: REGION.into(),
        subject_key: "person-ada".into(),
        jti: "session-cli-logout".into(),
        issued_at_unix: Some(now()),
        exp_unix: now() + 3600,
        authority: myelin_edge::authz::human_session_authority(),
        dpop_jkt: None,
        purpose: myelin_identity_service::CredentialPurpose::HumanSession,
        audience: myelin_identity_service::CredentialAudience::Edge,
    });
    (Arc::new(gateway), token)
}

/// Runs `myelin auth logout --json` against a config directory holding `token` as a stored
/// session for `edge`.
fn logout_with_stored_session(edge: &str, token: &str) -> serde_json::Value {
    let config = tempfile::tempdir().expect("config dir");
    let config_dir = config.path().to_path_buf();
    let getenv = |name: &str| match name {
        "MYELIN_CONFIG_DIR" => Some(config_dir.display().to_string()),
        "MYELIN_TEST_CREDENTIAL_STORE" => Some("file".into()),
        _ => None,
    };
    myelin_cli::config::store_credential(token, "session", Some(edge), Some(now() + 3600), &getenv)
        .expect("store the session");
    let out = Command::new(env!("CARGO_BIN_EXE_myelin"))
        .env("MYELIN_CONFIG_DIR", &config_dir)
        .env("MYELIN_TEST_CREDENTIAL_STORE", "file")
        .env_remove("MYELIN_TOKEN")
        .env_remove("MYELIN_EDGE")
        .args(["--json", "auth", "logout"])
        .output()
        .expect("spawn myelin");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("logout JSON")
}

fn whoami_status(gateway: &Gateway, token: &str) -> u16 {
    gateway
        .handle(myelin_edge::EdgeRequest::new(
            "GET",
            "/v1/whoami",
            "",
            vec![("authorization".into(), format!("Bearer {token}"))],
            vec![],
        ))
        .status()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logging_out_ends_the_stored_session_at_the_edge_and_forgets_it() {
    let (gateway, token) = person_with_session();
    let addr = spawn(gateway.clone()).await;
    assert_eq!(whoami_status(&gateway, &token), 200);

    let outcome = logout_with_stored_session(&format!("http://{addr}"), &token);
    assert_eq!(outcome["session_ended"], true, "{outcome}");
    assert_eq!(outcome["stored_credential_removed"], true);
    assert_eq!(
        whoami_status(&gateway, &token),
        401,
        "the session is over at the edge"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logging_out_forgets_the_session_even_when_the_edge_cannot_be_reached() {
    let unreachable = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let (_gateway, token) = person_with_session();
    let outcome = logout_with_stored_session(&unreachable, &token);
    assert_eq!(outcome["session_ended"], false, "{outcome}");
    assert_eq!(outcome["stored_credential_removed"], true);
}

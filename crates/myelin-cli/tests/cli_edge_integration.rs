use myelin_edge::{
    register_git, serve_edge, AllowAll, Gateway, GitEdgeState, Method, WhoamiHandler,
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

fn build() -> (Arc<Gateway>, CellTokenAuthority) {
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

    let git_state = Arc::new(GitEdgeState::new().seed_demo("acme"));
    let mut builder = Gateway::builder(authn, human_login, Arc::new(AllowAll)).route(
        Method::Get,
        "/v1/whoami",
        "edge.whoami",
        Arc::new(WhoamiHandler),
    );
    builder = register_git(builder, git_state);
    (Arc::new(builder.build()), cell)
}

fn mint(cell: &CellTokenAuthority, tenant: &str, jti: &str) -> String {
    cell.mint(&CapabilityMintSpec {
        tenant: tenant.into(),
        region: REGION.into(),
        subject_key: "subj-1".into(),
        jti: jti.into(),
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
async fn git_repo_list_end_to_end_renders_the_viewmodel() {
    let (gw, cell) = build();
    let addr = spawn(gw).await;
    let edge = format!("http://{addr}");
    let token = mint(&cell, "acme", "jti-e2e");

    let (code, stdout, stderr) = run_cli(&edge, Some(&token), &["git", "repo", "list"]);
    assert_eq!(code, 0, "exit 0 on success; stderr={stderr}");
    assert!(
        stdout.contains("acme/myelin") && stdout.contains("[populated]"),
        "renders the repo ViewModel; got: {stdout}"
    );

    let (jc, jout, _) = run_cli(&edge, Some(&token), &["--json", "git", "repo", "list"]);
    assert_eq!(jc, 0);
    let v: serde_json::Value = serde_json::from_str(&jout).expect("--json emits valid JSON");
    assert_eq!(v["items"][0]["state"], "populated");
    assert!(v["items"][0]["slug"].as_str().unwrap().starts_with("acme/"));
    assert_eq!(v["page"]["limit"], 50);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forged_token_is_a_clean_unauthenticated_error_not_a_panic() {
    let (gw, _cell) = build();
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
    let (gw, _cell) = build();
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
    let (gw, cell) = build();
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
    let (gw, cell) = build();
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

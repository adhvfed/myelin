use myelin_cli::config::EdgeConfig;
use myelin_cli::dispatch::{EdgeCall, HttpMethod, RetryPolicy};
use myelin_edge::{
    register_git_durable, serve_edge, AllowAll, DurableGitBackend, Gateway, Method, WhoamiHandler,
};
use myelin_identity::{DataRole, Principal, PrincipalId, PrincipalKind, PrincipalStatus};
use myelin_identity_service::{
    CapabilityAuthenticator, CapabilityMintSpec, CellTokenAuthority, HumanSsoAuthenticator,
    PasetoCapabilityVerifier, PrincipalStore, RevocationStore,
};
use myelin_storage::{KmsEngine, TenantScope};
use myelin_tenancy::{Region, TenantId};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
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

fn temp_root(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    p.push(format!("myelin-cli-gt005-{tag}-{nanos}"));
    p
}

fn admin_scope(tenant: &str) -> TenantScope {
    TenantScope::from_verified_token(
        &Principal::stub(
            PrincipalId("admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        ),
        Region(REGION.into()),
    )
}

fn authenticated_agent(tenant: &str) -> Principal {
    Principal::new(
        TenantId(tenant.into()),
        Region(REGION.into()),
        PrincipalId("svc:agent".into()),
        PrincipalKind::Service,
        DataRole::Controller,
        PrincipalStatus::Active,
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

fn build(root: &Path) -> (Arc<Gateway>, CellTokenAuthority, Arc<DurableGitBackend>) {
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

    let backend = Arc::new(DurableGitBackend::rooted_inmem_for_test(root.to_path_buf()));
    let mut builder = Gateway::builder(authn, human_login, Arc::new(AllowAll)).route(
        Method::Get,
        "/v1/whoami",
        "edge.whoami",
        Arc::new(WhoamiHandler),
    );
    builder = register_git_durable(builder, backend.clone());
    (Arc::new(builder.build()), cell, backend)
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

fn run_cli(edge: &str, token: &str, args: &[&str]) -> (i32, String, String) {
    let bin = env!("CARGO_BIN_EXE_myelin");
    let mut cmd = Command::new(bin);
    cmd.env("MYELIN_EDGE", edge)
        .env("MYELIN_TOKEN_SCHEME", SCHEME)
        .env("MYELIN_TOKEN", token)
        .env(
            "MYELIN_CONFIG_DIR",
            std::env::temp_dir().join("myelin-cli-gt005-empty"),
        );
    cmd.args(args);
    let out = cmd.output().expect("spawn myelin binary");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn code_search_preserves_query_bytes_and_returns_durable_matches() {
    let root = temp_root("search");
    let (gw, cell, _be) = build(&root);
    let addr = spawn(gw).await;
    let edge = format!("http://{addr}");
    let token = mint(&cell, "acme", "jti-search");
    let config = EdgeConfig {
        url: edge.clone(),
        scheme: SCHEME.into(),
    };
    let create = EdgeCall {
        method: HttpMethod::Post,
        path: "/v1/git/repos".into(),
        query: None,
        payload: Some(
            serde_json::json!({ "slug": "searchable" })
                .to_string()
                .into_bytes(),
        ),
        idempotency_key: Some("test-search-create".into()),
        retry_policy: RetryPolicy::CallerKeyRequired,
    };
    myelin_cli::client::execute(&config, &token, &create)
        .await
        .expect("create searchable repository");
    let phrase = "two words &limit=100% = 世界";
    let write = EdgeCall {
        method: HttpMethod::Post,
        path: "/v1/git/repos/searchable/blob/main/README.md".into(),
        query: None,
        payload: Some(
            serde_json::json!({ "base_oid": "", "contents": format!("before\n{phrase}\nafter\n") })
                .to_string()
                .into_bytes(),
        ),
        idempotency_key: Some("test-search-write".into()),
        retry_policy: RetryPolicy::CallerKeyRequired,
    };
    myelin_cli::client::execute(&config, &token, &write)
        .await
        .expect("write searchable content");

    let (code, stdout, stderr) = run_cli(
        &edge,
        &token,
        &["git", "search", "code", phrase, "--repo", "searchable"],
    );

    assert_eq!(code, 0, "durable search succeeds; stderr={stderr}");
    assert!(
        stderr.is_empty(),
        "successful search has no stderr: {stderr}"
    );
    assert!(
        stdout.contains("searchable:README.md:2"),
        "match location is rendered: {stdout}"
    );
    assert!(
        stdout.contains(phrase),
        "the matching excerpt is rendered: {stdout}"
    );
    assert!(
        !stderr.contains("bad_request"),
        "query bytes were not reinterpreted"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn durable_create_open_view_round_trip() {
    let root = temp_root("rt");
    let (gw, cell, _be) = build(&root);
    let addr = spawn(gw).await;
    let edge = format!("http://{addr}");
    let token = mint(&cell, "acme", "jti-rt");

    let (code, out, err) = run_cli(
        &edge,
        &token,
        &[
            "--idempotency-key",
            "test-repo-create-alpha",
            "git",
            "repo",
            "create",
            "alpha",
        ],
    );
    assert_eq!(code, 0, "repo create succeeds; stderr={err}");
    assert!(
        out.contains("git.repo.create") || out.contains("alpha"),
        "create echoed; got {out}"
    );

    let (retry_code, retry_out, retry_err) = run_cli(
        &edge,
        &token,
        &[
            "--idempotency-key",
            "test-repo-create-alpha",
            "git",
            "repo",
            "create",
            "alpha",
        ],
    );
    assert_eq!(retry_code, 0, "create retry succeeds; stderr={retry_err}");
    assert!(
        retry_out.contains("\"created\":false"),
        "create retry reports the existing durable repository; got {retry_out}"
    );

    let (lc, lout, _) = run_cli(&edge, &token, &["--json", "git", "repo", "list"]);
    assert_eq!(lc, 0);
    let v: serde_json::Value = serde_json::from_str(&lout).expect("--json valid");
    let slugs: Vec<&str> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["slug"].as_str())
        .collect();
    assert!(
        slugs.iter().any(|s| s.contains("alpha")),
        "repo list reflects create; got {lout}"
    );
    let alpha = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["slug"] == "acme/alpha")
        .expect("created repository list row");
    assert_eq!(
        alpha
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        ["slug".to_string(), "state".to_string()]
            .into_iter()
            .collect(),
        "--json intentionally emits the lightweight empty summary row, not RepoHome"
    );

    let (oc, _oout, oerr) = run_cli(
        &edge,
        &token,
        &[
            "--idempotency-key",
            "test-pr-open-alpha",
            "git",
            "pr",
            "open",
            "alpha",
            "--title",
            "Alpha PR",
            "--head-oid",
            "deadbeef",
        ],
    );
    assert_eq!(oc, 0, "pr open succeeds; stderr={oerr}");

    let (vc, vout, verr) = run_cli(
        &edge,
        &token,
        &["--json", "git", "pr", "view", "alpha", "1"],
    );
    assert_eq!(vc, 0, "pr view succeeds; stderr={verr}");
    let pr: serde_json::Value = serde_json::from_str(&vout).expect("--json valid");
    assert_eq!(pr["number"], 1, "the durable PR round-trips; got {vout}");
    assert_eq!(pr["durable"], true);

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn merge_blocked_by_gate_is_a_clean_cli_error() {
    let root = temp_root("gate");
    let (gw, cell, be) = build(&root);
    let addr = spawn(gw).await;
    let edge = format!("http://{addr}");
    let token = mint(&cell, "acme", "jti-gate");

    assert_eq!(
        run_cli(
            &edge,
            &token,
            &[
                "--idempotency-key",
                "test-gate-repo-create-alpha",
                "git",
                "repo",
                "create",
                "alpha",
            ],
        )
        .0,
        0
    );
    be.set_branch_protection(
        "acme",
        REGION,
        "alpha",
        &serde_json::json!({
            "rulesets": [{
                "ref_pattern": "refs/heads/main",
                "required_contexts": ["ci/build"],
                "required_approvals": 1
            }]
        }),
    )
    .expect("set branch protection");
    assert_eq!(
        run_cli(
            &edge,
            &token,
            &[
                "--idempotency-key",
                "test-gate-pr-open-alpha",
                "git",
                "pr",
                "open",
                "alpha",
                "--title",
                "Alpha PR",
                "--head-oid",
                "deadbeef"
            ]
        )
        .0,
        0
    );

    let (code, stdout, stderr) = run_cli(
        &edge,
        &token,
        &[
            "--idempotency-key",
            "test-gate-pr-merge-alpha-1",
            "git",
            "pr",
            "merge",
            "alpha",
            "1",
        ],
    );
    assert_eq!(
        code, 1,
        "a gate-blocked merge is a clean edge error (exit 1); stderr={stderr}"
    );
    assert!(
        stderr.contains("merge blocked by policy"),
        "the gate reason is surfaced; got {stderr}"
    );
    assert!(!stderr.contains("panicked"), "never a panic");
    assert!(stdout.is_empty(), "no view-model on a blocked merge");

    let rec = be
        .get_pr("acme", REGION, "alpha", 1, &authenticated_agent("acme"))
        .unwrap()
        .unwrap();
    assert_ne!(
        format!("{:?}", rec.state),
        "Merged",
        "the CLI cannot bypass the server gate"
    );

    let _ = std::fs::remove_dir_all(&root);
}

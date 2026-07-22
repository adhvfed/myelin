//! # GT-005 — the `myelin` CLI through the DURABLE GT-003 edge: real end-to-end write proofs.
//!
//! Spins up the REAL `myelin-edge` gateway IN-PROCESS over a real TCP socket with Git registered via
//! `register_git_durable` (the on-disk DURABLE backend), mints a REAL PASETO capability token, then
//! runs the compiled `myelin` BINARY against the socket. Proves the operator surface:
//!  - `myelin git repo create <slug>` PERSISTS (a fresh durable read + `repo list` reflect it);
//!  - `myelin git pr open <repo>` opens a durable PR; `myelin git pr view <repo> <n>` reads it back;
//!  - `myelin git pr merge <repo> <n>` against an UNMET branch-protection gate is a CLEAN CLI error
//!    carrying the gate reason (exit 1) — the CLI REFLECTS the server gate, never bypasses it;
//!  - `--json` emits machine-readable output.
//!
//! The forged/missing-token clean auth errors (exit 3) are proven in `cli_edge_integration.rs` (the
//! same auth path); this file focuses on the durable write/read round-trips + the gate reflection.

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

/// The exact active service identity seeded for `subj-1`; it carries no synthetic admin identity.
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

/// Build the real gateway over a known cell seed with Git registered over the DURABLE backend. Returns
/// the gateway, the cell authority (to mint tokens), and the shared backend (to set repo-owned policy).
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

/// **THE DURABLE WRITE/READ ROUND-TRIP.** `repo create` persists (read back via `repo list`); `pr open`
/// + `pr view` round-trip a durable PR; `--json` works.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn durable_create_open_view_round_trip() {
    let root = temp_root("rt");
    let (gw, cell, _be) = build(&root);
    let addr = spawn(gw).await;
    let edge = format!("http://{addr}");
    let token = mint(&cell, "acme", "jti-rt");

    // repo create → exit 0, durable.
    let (code, out, err) = run_cli(&edge, &token, &["git", "repo", "create", "alpha"]);
    assert_eq!(code, 0, "repo create succeeds; stderr={err}");
    assert!(
        out.contains("git.repo.create") || out.contains("alpha"),
        "create echoed; got {out}"
    );

    // repo list reflects the new durable repo.
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
        .expect("created repository summary row");
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

    // pr open → a durable PR #1.
    let (oc, _oout, oerr) = run_cli(
        &edge,
        &token,
        &[
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

    // pr view reads it back (durable).
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

/// **THE GATE REFLECTS, NEVER BYPASSES.** A merge against an UNMET repo-owned branch-protection gate is
/// a CLEAN CLI error carrying the gate reason (exit 1) — not a panic, not a faked success.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn merge_blocked_by_gate_is_a_clean_cli_error() {
    let root = temp_root("gate");
    let (gw, cell, be) = build(&root);
    let addr = spawn(gw).await;
    let edge = format!("http://{addr}");
    let token = mint(&cell, "acme", "jti-gate");

    // Create the repo via the CLI, then set repo-owned branch protection requiring a never-green CI
    // context (the merge gate must block). Policy is repo-owned — never author input (the GT-003 fix).
    assert_eq!(
        run_cli(&edge, &token, &["git", "repo", "create", "alpha"]).0,
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

    // pr merge → the server gate BLOCKS → a clean CLI error (exit 1) naming the reason; never a panic.
    let (code, stdout, stderr) = run_cli(&edge, &token, &["git", "pr", "merge", "alpha", "1"]);
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

    // The gate was NOT bypassed: the PR is still open on disk.
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

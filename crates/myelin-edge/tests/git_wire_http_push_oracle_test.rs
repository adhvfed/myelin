use myelin_ci_sandbox::verified_gvisor_git_rootfs;
use myelin_edge::{
    register_git_wire, serve_edge, AllowAll, DurableGitBackend, Gateway, Method, WhoamiHandler,
};
use myelin_git::core::RepoLoc;
use myelin_git::durable::DurableGitStore;
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

fn runsc_bin() -> Option<String> {
    let bin = std::env::var("MYELIN_RUNSC_BIN").unwrap_or_else(|_| "runsc".to_string());
    if bin.contains('/') {
        return Path::new(&bin).exists().then_some(bin);
    }
    let path = std::env::var("PATH").ok()?;
    for dir in path.split(':') {
        if Path::new(dir).join(&bin).exists() {
            return Some(bin);
        }
    }
    None
}

fn host_git() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn require_or_skip(test: &str) -> bool {
    if runsc_bin().is_none() || !host_git() {
        if std::env::var("MYELIN_REQUIRE_RUNSC").as_deref() == Ok("1") {
            panic!("[{test}] MYELIN_REQUIRE_RUNSC=1 but runsc/host git absent - refusing a vacuous green.");
        }
        eprintln!("[{test}] SKIPPED: runsc/host git absent.");
        return false;
    }

    match verified_gvisor_git_rootfs() {
        Ok(_) => true,
        Err(error) if std::env::var("MYELIN_REQUIRE_RUNSC").as_deref() == Ok("1") => {
            panic!(
                "[{test}] MYELIN_REQUIRE_RUNSC=1 but the pinned production git rootfs is \
                 unavailable: {error} - refusing a vacuous green."
            );
        }
        Err(error) => {
            eprintln!("[{test}] SKIPPED: pinned production git rootfs unavailable: {error}");
            false
        }
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn temp_root(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let d = std::env::temp_dir().join(format!(
        "myelin-ct006d-{tag}-{}-{nanos}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir root");
    d
}

fn run_git(args: &[&str], cwd: Option<&Path>) {
    let mut c = Command::new("git");
    c.args(args);
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    let out = c.output().expect("run host git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn seed_principal(store: &PrincipalStore, tenant: &str, pid: &str, subject_key: &str) {
    let scope = TenantScope::from_verified_token(
        &Principal::stub(
            PrincipalId("admin".into()),
            PrincipalKind::Human,
            TenantId(tenant.into()),
        ),
        Region(REGION.into()),
    );
    store
        .put_principal(
            &scope,
            PrincipalId(pid.into()),
            PrincipalKind::Service,
            DataRole::Controller,
            PrincipalStatus::Active,
            None,
        )
        .expect("seed");
    store
        .link_credential(&scope, SCHEME, subject_key, &PrincipalId(pid.into()))
        .expect("link");
}

fn build(root: &Path) -> (Arc<Gateway>, CellTokenAuthority, Arc<DurableGitBackend>) {
    build_with_authz(
        root,
        Arc::new(DurableGitBackend::rooted_inmem_for_test(root.to_path_buf())),
    )
}

fn build_with_authz(
    root: &Path,
    backend: Arc<DurableGitBackend>,
) -> (Arc<Gateway>, CellTokenAuthority, Arc<DurableGitBackend>) {
    let _ = root;
    let cell = CellTokenAuthority::from_seed(&[7u8; 32], &[9u8; 32]).expect("cell");
    let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
    seed_principal(&store, "acme", "svc:agent", "subj-1");
    seed_principal(&store, "globex", "svc:agent", "subj-1");
    let authn = Arc::new(CapabilityAuthenticator::with_verifier(
        store,
        Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
        RevocationStore::new(),
    ));
    let human = Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(
        Arc::new(KmsEngine::new()),
    )));
    let builder = Gateway::builder(authn, human, Arc::new(AllowAll))
        .default_token_scheme(SCHEME)
        .route(
            Method::Get,
            "/v1/whoami",
            "edge.whoami",
            Arc::new(WhoamiHandler),
        );
    let builder = register_git_wire(builder, backend.clone());
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

fn make_work(root: &Path) -> PathBuf {
    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    run_git(&["init", "-q", "-b", "main"], Some(&work));
    run_git(
        &["config", "user.email", "anon-7@acme.noreply"],
        Some(&work),
    );
    run_git(&["config", "user.name", "anon-7@acme.noreply"], Some(&work));
    std::fs::write(work.join("README.md"), b"# ct006d push oracle\n").unwrap();
    run_git(&["add", "-A"], Some(&work));
    run_git(
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "initial",
        ],
        Some(&work),
    );
    work
}

fn git_push(
    addr: SocketAddr,
    token: Option<&str>,
    repo_url_path: &str,
    work: &Path,
) -> (bool, String, String) {
    let url = format!("http://{addr}{repo_url_path}");
    let mut c = Command::new("git");
    c.current_dir(work).env("GIT_TERMINAL_PROMPT", "0");
    if let Some(t) = token {
        c.arg("-c")
            .arg(format!("http.extraHeader=Authorization: Bearer {t}"));
    }
    c.args(["push", &url, "main"]);
    let out = c.output().expect("spawn git push");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn durable_tip(root: &Path, tenant: &str, slug: &str, refname: &str) -> Option<String> {
    let store = DurableGitStore::rooted(root.to_path_buf());
    let repo = store.open_repo(&RepoLoc::new(tenant, REGION, slug)).ok()?;
    repo.read_ref(refname).ok().flatten().map(|o| o.0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_git_push_lands_durably_rejects_secrets_and_refuses_cross_tenant() {
    if !require_or_skip("ct006d push oracle") {
        return;
    }

    let root = temp_root("push");
    let backend_for_create = DurableGitBackend::rooted_inmem_for_test(root.clone());
    backend_for_create
        .create_repo("acme", REGION, "widgets")
        .expect("create server repo");

    let (gw, cell, backend) = build(&root);
    let addr = spawn(gw).await;
    let token = mint(&cell, "acme", "jti-push");

    let work = make_work(&root);
    let pushed_oid = {
        let o = Command::new("git")
            .args(["-C", &work.to_string_lossy(), "rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };

    let depth_before = backend.outbox().outbox_depth();

    let (ok, so, se) = git_push(addr, Some(&token), "/acme/eu-west/widgets.git", &work);
    println!("=== git push (authenticated) ===\nsuccess={ok}\nstdout=\n{so}\nstderr=\n{se}");
    assert!(ok, "the authenticated push MUST succeed");

    let tip = durable_tip(&root, "acme", "widgets", "refs/heads/main");
    println!("durable re-open: refs/heads/main = {tip:?} (pushed {pushed_oid})");
    assert_eq!(
        tip.as_deref(),
        Some(pushed_oid.as_str()),
        "the pushed ref must survive a fresh re-open"
    );

    let bare = root.join("acme/eu-west/widgets.git");
    let fsck = Command::new("git")
        .args(["--git-dir", &bare.to_string_lossy(), "fsck", "--full"])
        .output()
        .unwrap();
    println!(
        "=== git fsck --full (server repo) ===\nstatus={:?}\nstderr=\n{}",
        fsck.status.code(),
        String::from_utf8_lossy(&fsck.stderr)
    );
    assert!(
        fsck.status.success(),
        "git fsck on the server repo must be clean"
    );

    let depth_after = backend.outbox().outbox_depth();
    println!("outbox depth: before={depth_before} after={depth_after} (expect +1)");
    assert_eq!(
        depth_after,
        depth_before + 1,
        "the accepted push emits exactly one git.ref.updated (0 ghost / 0 lost)"
    );

    let tip_before_reject = durable_tip(&root, "acme", "widgets", "refs/heads/main");
    let depth_before_reject = backend.outbox().outbox_depth();
    let planted_secret = [b"aws_key = AK".as_slice(), b"IAIOSFODNN7EXAMPLE\n"].concat();
    std::fs::write(work.join("creds.txt"), planted_secret).unwrap();
    run_git(&["add", "-A"], Some(&work));
    run_git(
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "oops a secret",
        ],
        Some(&work),
    );
    let (ok_s, so_s, se_s) = git_push(addr, Some(&token), "/acme/eu-west/widgets.git", &work);
    println!("=== git push (PLANTED SECRET) - must be rejected ===\nsuccess={ok_s}\nstdout=\n{so_s}\nstderr=\n{se_s}");
    assert!(!ok_s, "a push carrying a secret MUST be rejected");
    let tip_after_reject = durable_tip(&root, "acme", "widgets", "refs/heads/main");
    assert_eq!(
        tip_after_reject, tip_before_reject,
        "a rejected push MUST NOT move the ref (0 ghost)"
    );
    assert_eq!(
        backend.outbox().outbox_depth(),
        depth_before_reject,
        "a rejected push emits NO event"
    );
    let fsck2 = Command::new("git")
        .args(["--git-dir", &bare.to_string_lossy(), "fsck", "--full"])
        .output()
        .unwrap();
    assert!(
        fsck2.status.success(),
        "the server repo stays fsck-clean after a rejected push"
    );
    run_git(&["reset", "-q", "--hard", "HEAD~1"], Some(&work));

    let (ok_n, _so_n, se_n) = git_push(addr, None, "/acme/eu-west/widgets.git", &work);
    println!("=== git push (NO token) - must be refused ===\nsuccess={ok_n}\nstderr=\n{se_n}");
    assert!(!ok_n, "an unauthenticated push MUST be refused");

    let globex = mint(&cell, "globex", "jti-x");
    let tip_before_x = durable_tip(&root, "acme", "widgets", "refs/heads/main");
    std::fs::write(work.join("x.txt"), b"cross tenant attempt\n").unwrap();
    run_git(&["add", "-A"], Some(&work));
    run_git(
        &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "x"],
        Some(&work),
    );
    let (ok_x, _so_x, se_x) = git_push(addr, Some(&globex), "/acme/eu-west/widgets.git", &work);
    println!("=== git push (CROSS-TENANT globex→acme) - must be refused ===\nsuccess={ok_x}\nstderr=\n{se_x}");
    assert!(!ok_x, "a cross-tenant push MUST be refused");
    assert_eq!(
        durable_tip(&root, "acme", "widgets", "refs/heads/main"),
        tip_before_x,
        "a cross-tenant push MUST NOT move acme's ref"
    );

    println!("=== CT-006d EXTERNAL ORACLE PROVEN: real git push lands durably + secret-reject (0 ghost) + auth/cross-tenant refusal ===");
    let _ = std::fs::remove_dir_all(&root);
}

use myelin_edge::{
    register_git_durable, register_git_wire as register_wire_r21a, CheckBackedRepoAuthorizer,
    TupleRepoBootstrap,
};
use myelin_events::OutboxStore;
use myelin_identity::FragmentAdmit;
use myelin_identity_service::{StoreBackedCheck, TupleStore};
use myelin_substrate::FailStaticThreshold;

fn r21a_threshold() -> FailStaticThreshold {
    FailStaticThreshold {
        status: "OPEN - LEGAL".into(),
        owner: "DPO / Legal".into(),
        static_max_secs: None,
        static_max_default_secs: 300,
        agent_token_ttl_secs: 60,
        constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
    }
}

fn build_r21a(root: &Path) -> (Arc<Gateway>, CellTokenAuthority) {
    let cell = CellTokenAuthority::from_seed(&[21u8; 32], &[23u8; 32]).expect("cell");
    let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
    seed_principal(&store, "acme", "svc:creator", "subj-c");
    let authn = Arc::new(CapabilityAuthenticator::with_verifier(
        store,
        Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
        RevocationStore::new(),
    ));
    let human = Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(
        Arc::new(KmsEngine::new()),
    )));

    let check = StoreBackedCheck::new(TupleStore::new(OutboxStore::new()));
    for admit in check.admit_git_fragment() {
        assert!(matches!(admit, FragmentAdmit::Admitted { .. }), "{admit:?}");
    }
    let repo_authz = Arc::new(
        CheckBackedRepoAuthorizer::try_new(check.clone(), 300, &r21a_threshold())
            .expect("valid staleness bound"),
    );
    let repo_bootstrap = Arc::new(TupleRepoBootstrap::new(check.tuples().clone()));
    let backend = Arc::new(
        DurableGitBackend::rooted_inmem_for_test(root.to_path_buf())
            .with_repo_authorizer(repo_authz)
            .with_repo_bootstrap(repo_bootstrap),
    );
    let builder = Gateway::builder(authn, human, Arc::new(AllowAll))
        .default_token_scheme(SCHEME)
        .route(
            Method::Get,
            "/v1/whoami",
            "edge.whoami",
            Arc::new(WhoamiHandler),
        );
    let builder = register_git_durable(builder, backend.clone());
    let builder = register_wire_r21a(builder, backend);
    (Arc::new(builder.build()), cell)
}

fn mint_for(cell: &CellTokenAuthority, jti: &str, subject_key: &str) -> String {
    cell.mint(&CapabilityMintSpec {
        tenant: "acme".into(),
        region: REGION.into(),
        subject_key: subject_key.into(),
        jti: jti.into(),
        exp_unix: now() + 3600,
        authority: vec!["edge.operator".into()],
        dpop_jkt: None,
        purpose: myelin_identity_service::CredentialPurpose::OperatorBootstrap,
        audience: myelin_identity_service::CredentialAudience::Edge,
    })
}

fn http_post(addr: SocketAddr, path: &str, token: &str, body: &str) -> String {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(addr).expect("connect edge");
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).expect("write request");
    let mut out = String::new();
    s.read_to_string(&mut out).expect("read response");
    out
}

fn push_main(addr: SocketAddr, token: &str, work: &Path, force: bool) -> (bool, String, String) {
    let url = format!("http://{addr}/acme/eu-west/widgets.git");
    let mut c = Command::new("git");
    c.current_dir(work).env("GIT_TERMINAL_PROMPT", "0");
    c.arg("-c")
        .arg(format!("http.extraHeader=Authorization: Bearer {token}"));
    c.arg("push");
    if force {
        c.arg("--force");
    }
    c.args([url.as_str(), "main"]);
    let out = c.output().expect("spawn git push");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r0_2_branch_protection_rejects_force_push_through_the_live_wire() {
    if !require_or_skip("r2.1a R0.2 protected-ref oracle") {
        return;
    }

    let root = temp_root("r21a-r02");
    let (gw, cell) = build_r21a(&root);
    let addr = spawn(gw).await;
    let token = mint_for(&cell, "jti-r02", "subj-c");

    let resp = http_post(addr, "/v1/git/repos", &token, r#"{"slug":"widgets"}"#);
    assert!(
        resp.starts_with("HTTP/1.1 201"),
        "create-repo must 201: {resp}"
    );

    let work = make_work(&root);
    let (ok1, _o1, e1) = push_main(addr, &token, &work, false);
    println!("=== push A (ff) ===\nsuccess={ok1}\nstderr=\n{e1}");
    assert!(
        ok1,
        "the creator's first push lands (bootstrap grant → push)"
    );

    std::fs::write(work.join("b.txt"), b"second\n").unwrap();
    run_git(&["add", "-A"], Some(&work));
    run_git(
        &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "B"],
        Some(&work),
    );
    let (ok2, _o2, e2) = push_main(addr, &token, &work, false);
    println!("=== push B (ff) ===\nsuccess={ok2}\nstderr=\n{e2}");
    assert!(
        ok2,
        "a fast-forward push to main lands (the gate is not a blanket refusal)"
    );
    let tip_b = durable_tip(&root, "acme", "widgets", "refs/heads/main").expect("tip B");

    run_git(&["reset", "-q", "--hard", "HEAD~1"], Some(&work));
    std::fs::write(work.join("c.txt"), b"divergent\n").unwrap();
    run_git(&["add", "-A"], Some(&work));
    run_git(
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "C (divergent)",
        ],
        Some(&work),
    );
    let (ok_f, so_f, se_f) = push_main(addr, &token, &work, true);
    println!("=== FORCE push (divergent) - must be rejected by R0.2 ===\nsuccess={ok_f}\nstdout=\n{so_f}\nstderr=\n{se_f}");
    assert!(
        !ok_f,
        "a force push to the protected `main` MUST be rejected through the wire (R0.2 live)"
    );
    assert!(
        se_f.contains("remote rejected") || so_f.contains("remote rejected"),
        "the rejection is the server's per-ref `ng` (remote rejected), not a client-side refusal: {se_f}"
    );
    assert_eq!(
        durable_tip(&root, "acme", "widgets", "refs/heads/main").as_deref(),
        Some(tip_b.as_str()),
        "the protected ref tip MUST NOT move on the rejected force push"
    );

    println!("=== R2.1a/R0.2 ORACLE PROVEN: branch protection fires through the LIVE wire (force-push rejected, ref unmoved) ===");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn writer_direct_push_to_protected_ref_is_refused_over_the_wire() {
    if !require_or_skip("r2-exit writer→protected-push oracle") {
        return;
    }

    let root = temp_root("r2exit-writer");
    let backend = Arc::new(
        DurableGitBackend::rooted_inmem_for_test(root.to_path_buf()).with_repo_authorizer(
            Arc::new(myelin_edge::GrantBackedRepos::new().grant_write(
                "svc:agent",
                "acme",
                "widgets",
            )),
        ),
    );
    backend
        .create_repo("acme", REGION, "widgets")
        .expect("create server repo");
    backend
        .set_branch_protection(
            "acme",
            REGION,
            "widgets",
            &serde_json::json!({
                "rulesets": [{
                    "ref_pattern": "refs/heads/main",
                    "required_approvals": 1
                }]
            }),
        )
        .expect("set branch protection");

    let (gw, cell, backend) = build_with_authz(&root, backend);
    let addr = spawn(gw).await;
    let token = mint(&cell, "acme", "jti-writer");

    let depth_before = backend.outbox().outbox_depth();
    let work = make_work(&root);

    let (ok, so, se) = git_push(addr, Some(&token), "/acme/eu-west/widgets.git", &work);
    println!("=== writer direct push to PROTECTED main - must be refused ===\nsuccess={ok}\nstdout=\n{so}\nstderr=\n{se}");
    assert!(
        !ok,
        "a plain writer's direct push to a protected ref MUST be refused (needs protected_push OR a satisfied full ruleset)"
    );
    assert!(
        se.contains("remote rejected") || so.contains("remote rejected"),
        "the rejection is the server's per-ref `ng` (remote rejected): {se}"
    );
    assert_eq!(
        durable_tip(&root, "acme", "widgets", "refs/heads/main"),
        None,
        "the protected ref MUST NOT be created by the refused writer push (0 ghost)"
    );
    assert_eq!(
        backend.outbox().outbox_depth(),
        depth_before,
        "a refused push emits NO git.ref.updated event"
    );

    println!("=== R2-EXIT ORACLE PROVEN: writer→protected-branch direct push DENIED over the live wire ===");
    let _ = std::fs::remove_dir_all(&root);
}

fn basic_auth_header(user: &str, token: &str) -> String {
    use base64::Engine as _;
    let b64 =
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:{token}").as_bytes());
    format!("Authorization: Basic {b64}")
}

fn git_clone_basic(
    addr: SocketAddr,
    token: &str,
    repo_url_path: &str,
    dst: &Path,
) -> (bool, String) {
    let url = format!("http://{addr}{repo_url_path}");
    let out = Command::new("git")
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("-c")
        .arg(format!(
            "http.extraHeader={}",
            basic_auth_header("x-access-token", token)
        ))
        .args(["clone", &url, &dst.to_string_lossy()])
        .output()
        .expect("spawn git clone");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn git_push_basic(
    addr: SocketAddr,
    token: &str,
    repo_url_path: &str,
    work: &Path,
) -> (bool, String, String) {
    let url = format!("http://{addr}{repo_url_path}");
    let out = Command::new("git")
        .current_dir(work)
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("-c")
        .arg(format!(
            "http.extraHeader={}",
            basic_auth_header("x-access-token", token)
        ))
        .args(["push", &url, "main"])
        .output()
        .expect("spawn git push");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_git_clone_and_push_over_http_basic_auth() {
    if !require_or_skip("r4.0 basic-auth wire oracle") {
        return;
    }

    let root = temp_root("basic");
    let backend_for_create = DurableGitBackend::rooted_inmem_for_test(root.clone());
    backend_for_create
        .create_repo("acme", REGION, "widgets")
        .expect("create server repo");

    let (gw, cell, backend) = build(&root);
    let addr = spawn(gw).await;
    let token = mint(&cell, "acme", "jti-basic");

    let work = make_work(&root);
    let pushed_oid = {
        let o = Command::new("git")
            .args(["-C", &work.to_string_lossy(), "rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    let (ok, so, se) = git_push_basic(addr, &token, "/acme/eu-west/widgets.git", &work);
    println!(
        "=== git push (HTTP BASIC, password=token) ===\nsuccess={ok}\nstdout=\n{so}\nstderr=\n{se}"
    );
    assert!(
        ok,
        "the Basic-auth push MUST succeed (password = capability token)"
    );
    let tip = durable_tip(&root, "acme", "widgets", "refs/heads/main");
    assert_eq!(
        tip.as_deref(),
        Some(pushed_oid.as_str()),
        "the Basic-auth push landed durably"
    );

    let clone_dst = root.join("clone");
    let (okc, sec) = git_clone_basic(addr, &token, "/acme/eu-west/widgets.git", &clone_dst);
    println!("=== git clone (HTTP BASIC) ===\nsuccess={okc}\nstderr=\n{sec}");
    assert!(okc, "the Basic-auth clone MUST succeed: {sec}");
    let cloned_tip = {
        let o = Command::new("git")
            .args([
                "-C",
                &clone_dst.to_string_lossy(),
                "rev-parse",
                "origin/main",
            ])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    assert_eq!(
        cloned_tip, pushed_oid,
        "the Basic-auth clone recovered the pushed commit"
    );

    let garbage_dst = root.join("clone-garbage");
    let (okg, seg) = git_clone_basic(
        addr,
        "not-a-real-token",
        "/acme/eu-west/widgets.git",
        &garbage_dst,
    );
    println!("=== git clone (GARBAGE Basic password) - must be refused ===\nsuccess={okg}\nstderr=\n{seg}");
    assert!(
        !okg,
        "a garbage Basic password MUST be refused (uniform 401)"
    );
    let _ = backend;

    println!("=== R4.0 BASIC-AUTH ORACLE PROVEN: real git clone + push over HTTP Basic (password=token) lands durably; garbage refused ===");
    let _ = std::fs::remove_dir_all(&root);
}

fn credential_helper_arg(token: &str) -> String {
    format!(
        "credential.helper=!f() {{ echo username=x-access-token; echo \"password={token}\"; }}; f"
    )
}

fn git_clone_via_helper(
    addr: SocketAddr,
    token: &str,
    repo_url_path: &str,
    dst: &Path,
) -> (bool, String) {
    let url = format!("http://{addr}{repo_url_path}");
    let out = Command::new("git")
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("-c")
        .arg(credential_helper_arg(token))
        .args(["clone", &url, &dst.to_string_lossy()])
        .output()
        .expect("spawn git clone (helper)");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn git_push_via_helper(
    addr: SocketAddr,
    token: &str,
    repo_url_path: &str,
    work: &Path,
) -> (bool, String) {
    let url = format!("http://{addr}{repo_url_path}");
    let out = Command::new("git")
        .current_dir(work)
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("-c")
        .arg(credential_helper_arg(token))
        .args(["push", &url, "main"])
        .output()
        .expect("spawn git push (helper)");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f1_real_git_over_credential_helper_needs_the_basic_challenge() {
    if !require_or_skip("f1 credential-helper wire oracle") {
        return;
    }

    let root = temp_root("f1-helper");
    let backend_for_create = DurableGitBackend::rooted_inmem_for_test(root.clone());
    backend_for_create
        .create_repo("acme", REGION, "widgets")
        .expect("create server repo");

    let (gw, cell, backend) = build(&root);
    let addr = spawn(gw).await;
    let token = mint(&cell, "acme", "jti-f1-helper");

    let work = make_work(&root);
    let (ok_push, se_push) = git_push_via_helper(addr, &token, "/acme/eu-west/widgets.git", &work);
    println!("=== F1 git push (credential HELPER, no extraHeader) ===\nsuccess={ok_push}\nstderr=\n{se_push}");
    assert!(
        ok_push,
        "F1: a helper-driven push MUST succeed - the git-wire 401 now carries WWW-Authenticate: Basic \
         so git offers the credential (stderr: {se_push})"
    );

    let dst = root.join("clone-helper");
    let (ok_clone, se_clone) =
        git_clone_via_helper(addr, &token, "/acme/eu-west/widgets.git", &dst);
    println!("=== F1 git clone (credential HELPER) ===\nsuccess={ok_clone}\nstderr=\n{se_clone}");
    assert!(
        ok_clone,
        "F1: a helper-driven clone MUST succeed (stderr: {se_clone})"
    );
    let _ = backend;

    println!("=== F1 ORACLE PROVEN: real git over a credential helper authenticates (the 401 Basic challenge is present) ===");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f9_fresh_clone_checks_out_main_and_server_head_symref_is_main() {
    if !require_or_skip("f9 head-symref clone oracle") {
        return;
    }

    let root = temp_root("f9-head");
    let backend_for_create = DurableGitBackend::rooted_inmem_for_test(root.clone());
    backend_for_create
        .create_repo("acme", REGION, "widgets")
        .expect("create server repo");

    let (gw, cell, backend) = build(&root);
    let addr = spawn(gw).await;
    let token = mint(&cell, "acme", "jti-f9");

    let work = make_work(&root);
    let (ok, _so, se) = git_push(addr, Some(&token), "/acme/eu-west/widgets.git", &work);
    assert!(ok, "the setup push must land: {se}");

    let bare = root.join("acme/eu-west/widgets.git");
    let symref = Command::new("git")
        .args(["--git-dir", &bare.to_string_lossy(), "symbolic-ref", "HEAD"])
        .output()
        .unwrap();
    let symref_target = String::from_utf8_lossy(&symref.stdout).trim().to_string();
    println!("=== F9 server HEAD symref = {symref_target:?} ===");
    assert_eq!(
        symref_target, "refs/heads/main",
        "F9: the on-disk HEAD symref must target the pushed default branch"
    );

    let dst = root.join("f9-clone");
    let out = Command::new("git")
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("-c")
        .arg(format!("http.extraHeader=Authorization: Bearer {token}"))
        .args([
            "clone",
            &format!("http://{addr}/acme/eu-west/widgets.git"),
            &dst.to_string_lossy(),
        ])
        .output()
        .expect("spawn git clone");
    let clone_stderr = String::from_utf8_lossy(&out.stderr).to_string();
    println!(
        "=== F9 git clone ===\nsuccess={}\nstderr=\n{clone_stderr}",
        out.status.success()
    );
    assert!(
        out.status.success(),
        "the clone must succeed: {clone_stderr}"
    );
    assert!(
        !clone_stderr.contains("unable to checkout") && !clone_stderr.contains("nonexistent ref"),
        "F9: a clone must NOT warn about a dangling HEAD: {clone_stderr}"
    );
    assert!(
        dst.join("README.md").is_file(),
        "F9: the working tree checked out `main` (README.md present) - HEAD resolved on the server"
    );
    let _ = backend;

    println!("=== F9 ORACLE PROVEN: fresh clone checks out main; server HEAD symref = refs/heads/main ===");
    let _ = std::fs::remove_dir_all(&root);
}

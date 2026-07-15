//! # CT-006d EXTERNAL-ORACLE: a REAL `git push` against the Myelin smart-HTTP server
//!
//! The GT-006 WRITE-side done-bar. Binds a real edge listener, registers the git smart-HTTP wire
//! endpoints over the DURABLE on-disk backend, and drives the **host's REAL `git`** as a pushing client:
//!   1. a real `git push` of new commits over HTTP → SUCCEEDS; the objects + ref land DURABLY (a FRESH
//!      process / re-opened durable repo sees the new ref + objects — survives restart), `git fsck` is
//!      clean on the server repo, and ONE `git.ref.updated` outbox event was emitted (emit-iff-committed);
//!   2. a REJECTED push (a planted AWS-key secret) does NOT move the ref + emits NO event (0 ghost) — the
//!      ref tip + outbox depth are UNCHANGED;
//!   3. an UNAUTHENTICATED push is refused; a CROSS-TENANT push is refused (no ref move, no leak).
//!
//! Every pushed byte's pack is a REAL `runsc` run of REAL `git index-pack` inside the hardened gVisor
//! sandbox (`GvisorBackend::launch_git_receive_pack`), streamed back + migrated by the in-process
//! one-tx ref-CAS + outbox. Gated like CT-006c: `MYELIN_REQUIRE_RUNSC=1` ⇒ an absent capability is a
//! HARD failure. Run: `MYELIN_REQUIRE_RUNSC=1 cargo test -p myelin-edge --test git_wire_http_push_oracle_test -- --nocapture`.

use myelin_ci_sandbox::{resolved_gvisor_rootfs, ENV_GVISOR_GIT_ROOTFS};
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
use std::sync::OnceLock;
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
    if runsc_bin().is_some() && resolved_gvisor_rootfs().exists() && host_git() {
        return true;
    }
    if std::env::var("MYELIN_REQUIRE_RUNSC").as_deref() == Ok("1") {
        panic!("[{test}] MYELIN_REQUIRE_RUNSC=1 but runsc/base rootfs/host git absent — refusing a vacuous green.");
    }
    eprintln!("[{test}] SKIPPED: runsc/base rootfs/host git absent.");
    false
}

fn copy_file(src: &Path, dst: &Path) {
    if let Some(p) = dst.parent() {
        std::fs::create_dir_all(p).expect("mkdir -p");
    }
    std::fs::copy(src, dst).unwrap_or_else(|e| panic!("copy {src:?} -> {dst:?}: {e}"));
}

fn stage_lib(rootfs: &Path, soname: &str, host_path: &str) {
    let real = std::fs::canonicalize(host_path).unwrap_or_else(|_| PathBuf::from(host_path));
    let real_name = real.file_name().unwrap().to_string_lossy().to_string();
    for libdir in ["usr/lib", "lib"] {
        let dst_real = rootfs.join(libdir).join(&real_name);
        copy_file(&real, &dst_real);
        let link = rootfs.join(libdir).join(soname);
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&real_name, &link).expect("soname symlink");
    }
}

fn stage_git_rootfs(base: &Path) -> PathBuf {
    let staged =
        std::env::temp_dir().join(format!("myelin-ct006d-push-rootfs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staged);
    let st = Command::new("cp")
        .arg("-a")
        .arg(format!("{}/.", base.display()))
        .arg(&staged)
        .status()
        .expect("cp -a");
    assert!(st.success(), "cp -a base rootfs failed");
    copy_file(Path::new("/usr/bin/git"), &staged.join("usr/bin/git"));
    stage_lib(&staged, "libpcre2-8.so.0", "/usr/lib/libpcre2-8.so.0");
    stage_lib(&staged, "libz-ng.so.2", "/usr/lib/libz-ng.so.2");
    let core = staged.join("usr/lib/git-core");
    std::fs::create_dir_all(&core).expect("mkdir git-core");
    for helper in ["git-upload-pack", "git-receive-pack"] {
        let link = core.join(helper);
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink("../../bin/git", &link).expect("helper symlink");
    }
    std::fs::create_dir_all(staged.join("repo")).expect("mkdir /repo");
    staged
}

fn git_rootfs() -> Option<PathBuf> {
    static STAGED: OnceLock<Option<PathBuf>> = OnceLock::new();
    STAGED
        .get_or_init(|| {
            let base = resolved_gvisor_rootfs();
            if !base.exists() {
                return None;
            }
            let staged = stage_git_rootfs(&base);
            std::env::set_var(ENV_GVISOR_GIT_ROOTFS, &staged);
            Some(staged)
        })
        .clone()
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

/// Same as [`build`] but with a caller-supplied backend (so a test can inject a restrictive per-repo
/// authorizer — e.g. a write-only-but-NOT-protected_push grant to prove the R2-exit wire denial).
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
        authority: vec!["agent:run".into()],
        dpop_jkt: None,
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

/// Make a work repo with one initial commit (not yet pushed to the server).
fn make_work(root: &Path) -> PathBuf {
    let work = root.join("work");
    std::fs::create_dir_all(&work).unwrap();
    run_git(&["init", "-q", "-b", "main"], Some(&work));
    // The push pseudonymity gate (GIT-1) requires every pushed commit's author/committer identity to be
    // a `<pseudonym>@<tenant>.noreply` handle for the pushing tenant (acme) — a raw name/email is refused
    // BEFORE the ref moves. A real client does this one-time `git config`; the oracle does the same.
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

/// Run `git push <url> main` with an optional Bearer token. Returns (success, stdout, stderr).
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

/// Re-open the durable repo in a FRESH store (a new process would do the same) and read a ref's tip.
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
    let Some(_rootfs) = git_rootfs() else { return };

    let root = temp_root("push");
    // The server repo must exist (push to a non-existent repo is a 404). Create it durably.
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

    // ── 1. REAL `git push` over smart-HTTP with the Bearer token ──
    let (ok, so, se) = git_push(addr, Some(&token), "/acme/eu-west/widgets.git", &work);
    println!("=== git push (authenticated) ===\nsuccess={ok}\nstdout=\n{so}\nstderr=\n{se}");
    assert!(ok, "the authenticated push MUST succeed");

    // The ref + objects landed DURABLY: a FRESH store (≈ a restarted process) sees the new tip.
    let tip = durable_tip(&root, "acme", "widgets", "refs/heads/main");
    println!("durable re-open: refs/heads/main = {tip:?} (pushed {pushed_oid})");
    assert_eq!(
        tip.as_deref(),
        Some(pushed_oid.as_str()),
        "the pushed ref must survive a fresh re-open"
    );

    // `git fsck --full` on the SERVER bare repo is clean (the migrated objects are intact + connected).
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

    // emit-iff-committed: exactly ONE git.ref.updated row became durable for the accepted ref move.
    let depth_after = backend.outbox().outbox_depth();
    println!("outbox depth: before={depth_before} after={depth_after} (expect +1)");
    assert_eq!(
        depth_after,
        depth_before + 1,
        "the accepted push emits exactly one git.ref.updated (0 ghost / 0 lost)"
    );

    // ── 2. a REJECTED push (a planted AWS-key secret) does NOT move the ref + emits NO event ──
    let tip_before_reject = durable_tip(&root, "acme", "widgets", "refs/heads/main");
    let depth_before_reject = backend.outbox().outbox_depth();
    std::fs::write(work.join("creds.txt"), b"aws_key = AKIAIOSFODNN7EXAMPLE\n").unwrap();
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
    println!("=== git push (PLANTED SECRET) — must be rejected ===\nsuccess={ok_s}\nstdout=\n{so_s}\nstderr=\n{se_s}");
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
    // The server repo is still fsck-clean (no half-migrated corruption from the rejected push).
    let fsck2 = Command::new("git")
        .args(["--git-dir", &bare.to_string_lossy(), "fsck", "--full"])
        .output()
        .unwrap();
    assert!(
        fsck2.status.success(),
        "the server repo stays fsck-clean after a rejected push"
    );
    // Undo the local secret commit so the cross-tenant leg pushes a clean history.
    run_git(&["reset", "-q", "--hard", "HEAD~1"], Some(&work));

    // ── 3a. UNAUTHENTICATED push is refused ──
    let (ok_n, _so_n, se_n) = git_push(addr, None, "/acme/eu-west/widgets.git", &work);
    println!("=== git push (NO token) — must be refused ===\nsuccess={ok_n}\nstderr=\n{se_n}");
    assert!(!ok_n, "an unauthenticated push MUST be refused");

    // ── 3b. CROSS-TENANT push is refused (globex's token for acme's repo) ──
    let globex = mint(&cell, "globex", "jti-x");
    let tip_before_x = durable_tip(&root, "acme", "widgets", "refs/heads/main");
    std::fs::write(work.join("x.txt"), b"cross tenant attempt\n").unwrap();
    run_git(&["add", "-A"], Some(&work));
    run_git(
        &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "x"],
        Some(&work),
    );
    let (ok_x, _so_x, se_x) = git_push(addr, Some(&globex), "/acme/eu-west/widgets.git", &work);
    println!("=== git push (CROSS-TENANT globex→acme) — must be refused ===\nsuccess={ok_x}\nstderr=\n{se_x}");
    assert!(!ok_x, "a cross-tenant push MUST be refused");
    assert_eq!(
        durable_tip(&root, "acme", "widgets", "refs/heads/main"),
        tip_before_x,
        "a cross-tenant push MUST NOT move acme's ref"
    );

    println!("=== CT-006d EXTERNAL ORACLE PROVEN: real git push lands durably + secret-reject (0 ghost) + auth/cross-tenant refusal ===");
    let _ = std::fs::remove_dir_all(&root);
}

// ═══════════════ R2.1a — R0.2 LIVE: branch protection fires on the production-shaped wire ═══════════════
//
// R0.2 (`evaluate_protected_ref_push`) was proven at the handler tier; what was missing was the wire
// itself in the production composition (main.rs never called `register_git_wire`). This oracle runs
// the R2.1a composition — the live CheckEngine repo-authorizer + the creator→admin bootstrap grant +
// the mounted wire — and proves the protected-ref gate rejects a REAL `git push --force` to
// `refs/heads/main` (the default-protected ref) with the ref tip UNMOVED, while ordinary
// fast-forward pushes by the granted creator continue to land.

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
        status: "OPEN — LEGAL".into(),
        owner: "DPO / Legal".into(),
        static_max_secs: None,
        static_max_default_secs: 300,
        agent_token_ttl_secs: 60,
        constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
    }
}

/// The R2.1a production-shaped gateway (mirrors main.rs): live CheckEngine repo authz + bootstrap
/// grants + the durable routes (create-repo) + the wire.
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
        authority: vec!["agent:run".into()],
        dpop_jkt: None,
    })
}

/// A minimal HTTP/1.1 POST over a raw TcpStream (no client dep): returns the full response text.
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

/// `git push [--force] <url> main`; returns (success, stdout, stderr).
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
    let Some(_rootfs) = git_rootfs() else { return };

    let root = temp_root("r21a-r02");
    let (gw, cell) = build_r21a(&root);
    let addr = spawn(gw).await;
    let token = mint_for(&cell, "jti-r02", "subj-c");

    // The creator creates the repo THROUGH the edge (bootstrap grant written) and pushes twice —
    // ordinary fast-forwards land through the live-authz wire.
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

    // Rewrite history (divergent C from A) and FORCE-push → R0.2: `refs/heads/main` is protected
    // by default; a force push is rejected AT THE WIRE (per-ref `ng` — the whole atomic push
    // aborts) and the durable tip does not move.
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
    println!("=== FORCE push (divergent) — must be rejected by R0.2 ===\nsuccess={ok_f}\nstdout=\n{so_f}\nstderr=\n{se_f}");
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

// ═══════════════ R2-EXIT BLOCKER — a plain WRITER's direct push to a protected ref is DENIED ═══════
//
// The red-team exploit, over the REAL wire: a principal holding only a `write` grant (NO
// `admin`/`protected_push`) pushes DIRECTLY to a protected `main` whose repo-owned ruleset requires a
// human approval. Defect 2 (the wire consults R2.1's admin-only `RepoPermission::ProtectedPush` rung —
// a writer lacks it) + Defect 3 (the full ruleset, not just contexts — a direct push carries 0
// approvals) compose so the push is REJECTED at the wire (`ng`), the ref never moves, and NO event is
// emitted.

/// **THE EXPLOIT, FLIPPED TO DENIED (end-to-end).** A write-only principal's direct `git push` to a
/// protected `main` that requires an approval is refused over the wire; the ref is never created/moved.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn writer_direct_push_to_protected_ref_is_refused_over_the_wire() {
    if !require_or_skip("r2-exit writer→protected-push oracle") {
        return;
    }
    let Some(_rootfs) = git_rootfs() else { return };

    let root = temp_root("r2exit-writer");
    // A backend whose per-repo authorizer grants the pushing principal (svc:agent) WRITE only — NOT
    // protected_push (`writer` in the frozen lattice; `protected_push = admin`). So the wire Write gate
    // ADMITS the push, but the protected-ref gate does not find the admin bypass, and holds the direct
    // push to the full ruleset.
    let backend = Arc::new(
        DurableGitBackend::rooted_inmem_for_test(root.to_path_buf()).with_repo_authorizer(Arc::new(
            myelin_edge::GrantBackedRepos::new().grant_write("svc:agent", "acme", "widgets"),
        )),
    );
    backend
        .create_repo("acme", REGION, "widgets")
        .expect("create server repo");
    // Repo-owned protection: `main` requires ONE approval — unsatisfiable by a DIRECT push (no PR).
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
    println!("=== writer direct push to PROTECTED main — must be refused ===\nsuccess={ok}\nstdout=\n{so}\nstderr=\n{se}");
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

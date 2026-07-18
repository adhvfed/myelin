//! # CT-006c EXTERNAL-ORACLE: a REAL `git clone`/`git fetch` against the Myelin smart-HTTP server
//!
//! The GT-006 read-side done-bar. Binds a real edge listener, registers the git smart-HTTP wire
//! endpoints ([`myelin_edge::register_git_wire`]) over the DURABLE on-disk backend, and drives the
//! **host's REAL `git`** as the client:
//!   1. a real bare repo whose packfile is **> 256 KiB** (the old `SANDBOX_CAPTURE_BOUND` that silently
//!      truncated the wire — CT-006b FU-1) is `git clone`d over HTTP → the clone SUCCEEDS, `git fsck` is
//!      clean, and the cloned HEAD/content matches the origin (the big pack came through WHOLE — the
//!      streaming fix proven);
//!   2. a new origin commit is `git fetch`ed → the new commit arrives;
//!   3. an UNAUTHENTICATED clone (no token) is REFUSED; a CROSS-TENANT clone (another tenant's token) is
//!      REFUSED — no repo bytes, no existence leak.
//!
//! Every served byte is a REAL `runsc` run of REAL `git upload-pack` inside the hardened gVisor sandbox
//! (`GvisorBackend::launch_git_wire`), streamed to the client through the production `RoutedGitCore`.
//!
//! ## Gating
//! SKIPS gracefully when `runsc`/the busybox base rootfs/the host `git` are absent. With
//! `MYELIN_REQUIRE_RUNSC=1` an absent capability is a HARD failure (never a vacuous green). Run:
//! `MYELIN_REQUIRE_RUNSC=1 cargo test -p myelin-edge --test git_wire_http_clone_oracle_test -- --nocapture`.

use myelin_ci_sandbox::{resolved_gvisor_rootfs, ENV_GVISOR_GIT_ROOTFS};
use myelin_edge::{
    register_git_durable, register_git_wire, serve_edge, AllowAll, CheckBackedRepoAuthorizer,
    DurableGitBackend, Gateway, Method, TupleRepoBootstrap, WhoamiHandler,
};
use myelin_events::OutboxStore;
use myelin_identity::{
    DataRole, FragmentAdmit, Principal, PrincipalId, PrincipalKind, PrincipalStatus,
};
use myelin_identity_service::{
    CapabilityAuthenticator, CapabilityMintSpec, CellTokenAuthority, HumanSsoAuthenticator,
    PasetoCapabilityVerifier, PrincipalStore, RevocationStore, StoreBackedCheck, TupleStore,
};
use myelin_storage::{KmsEngine, TenantScope};
use myelin_substrate::FailStaticThreshold;
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

// ───────────────────────────── runsc / rootfs / git preconditions ─────────────────────────────

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
        panic!(
            "[{test}] MYELIN_REQUIRE_RUNSC=1 but `runsc`/the busybox base rootfs ({})/host git is \
             absent — CT-006c refuses a VACUOUS green.",
            resolved_gvisor_rootfs().display()
        );
    }
    eprintln!("[{test}] SKIPPED: `runsc`/base rootfs/host git absent.");
    false
}

// ───────────── stage a git-bearing rootfs (the CT-006a/b recipe, replicated) ─────────────

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
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_name, &link).expect("soname symlink");
    }
}

fn stage_git_rootfs(base: &Path) -> PathBuf {
    let staged =
        std::env::temp_dir().join(format!("myelin-ct006c-git-rootfs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staged);
    let st = Command::new("cp")
        .arg("-a")
        .arg(format!("{}/.", base.display()))
        .arg(&staged)
        .status()
        .expect("cp -a base rootfs");
    assert!(st.success(), "cp -a base rootfs failed");

    copy_file(Path::new("/usr/bin/git"), &staged.join("usr/bin/git"));
    stage_lib(&staged, "libpcre2-8.so.0", "/usr/lib/libpcre2-8.so.0");
    stage_lib(&staged, "libz-ng.so.2", "/usr/lib/libz-ng.so.2");
    let core = staged.join("usr/lib/git-core");
    std::fs::create_dir_all(&core).expect("mkdir git-core");
    for helper in ["git-upload-pack", "git-receive-pack"] {
        let link = core.join(helper);
        let _ = std::fs::remove_file(&link);
        #[cfg(unix)]
        std::os::unix::fs::symlink("../../bin/git", &link).expect("git-core helper symlink");
    }
    std::fs::create_dir_all(staged.join("repo")).expect("mkdir /repo mount point");
    std::fs::create_dir_all(staged.join("quarantine")).expect("mkdir /quarantine mount point");
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

// ───────────────────────────── host-git helpers (the external oracle) ─────────────────────────────

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
        "myelin-ct006c-{tag}-{}-{nanos}",
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
        "host git {args:?} failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Deterministic pseudo-random incompressible bytes (so the packfile size ≈ raw content — no zlib
/// shrink masks the size). A tiny xorshift; no external dep.
fn pseudo_random(n: usize, mut seed: u64) -> Vec<u8> {
    let mut v = Vec::with_capacity(n);
    while v.len() < n {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        v.extend_from_slice(&seed.to_le_bytes());
    }
    v.truncate(n);
    v
}

/// Build `<root>/<tenant>/<region>/<repo>.git` as a REAL bare repo carrying ~`payload_bytes` of
/// incompressible content (so its packfile clears 256 KiB). Returns the HEAD oid + the work dir (kept
/// so a follow-on commit + push can be made for the fetch leg).
fn make_big_repo(
    root: &Path,
    tenant: &str,
    region: &str,
    slug: &str,
    payload_bytes: usize,
) -> (String, PathBuf) {
    let bare = root.join(tenant).join(region).join(format!("{slug}.git"));
    std::fs::create_dir_all(bare.parent().unwrap()).expect("mkdir repo parent");
    // `-b main` so the bare repo's HEAD is a symref to refs/heads/main (else HEAD dangles at the
    // default branch and a real `git clone` can't check out — "remote HEAD refers to nonexistent ref").
    run_git(
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            &bare.to_string_lossy(),
        ],
        None,
    );

    let work = root.join(format!("work-{slug}"));
    std::fs::create_dir_all(&work).expect("mkdir work");
    run_git(&["init", "-q", "-b", "main"], Some(&work));
    run_git(&["config", "user.email", "t@t.t"], Some(&work));
    run_git(&["config", "user.name", "t"], Some(&work));
    // Split the payload across a few files so there are real trees/blobs.
    let per = payload_bytes / 4;
    for i in 0..4 {
        std::fs::write(
            work.join(format!("blob-{i}.bin")),
            pseudo_random(per, 0x9E37_79B9 + i as u64),
        )
        .expect("write blob");
    }
    std::fs::write(work.join("README.md"), b"# ct006c big-pack clone\n").expect("write readme");
    run_git(&["add", "-A"], Some(&work));
    run_git(
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "first big commit",
        ],
        Some(&work),
    );
    run_git(
        &["push", "-q", &bare.to_string_lossy(), "main"],
        Some(&work),
    );

    let out = Command::new("git")
        .args(["--git-dir", &bare.to_string_lossy(), "rev-parse", "main"])
        .output()
        .expect("git rev-parse");
    assert!(out.status.success(), "rev-parse: {out:?}");
    (
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
        work,
    )
}

// ───────────────────────────── the edge server (real auth, real listener) ─────────────────────────────

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
        .expect("seed principal");
    store
        .link_credential(&scope, SCHEME, subject_key, &PrincipalId(pid.into()))
        .expect("link credential");
}

/// Build a gateway: real PASETO auth (a fresh cell), the wire endpoints over a durable backend rooted at
/// `root`, default token scheme `agent` (so a plain `Authorization: Bearer` header authenticates).
fn build(root: &Path) -> (Arc<Gateway>, CellTokenAuthority) {
    let cell = CellTokenAuthority::from_seed(&[7u8; 32], &[9u8; 32]).expect("cell authority");
    let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
    seed_principal(&store, "acme", "svc:agent", "subj-1");
    seed_principal(&store, "globex", "svc:agent", "subj-1");

    let authn = Arc::new(CapabilityAuthenticator::with_verifier(
        store,
        Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
        RevocationStore::new(),
    ));
    let human_login = Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(
        Arc::new(KmsEngine::new()),
    )));
    let backend = Arc::new(DurableGitBackend::rooted_inmem_for_test(root.to_path_buf()));
    let builder = Gateway::builder(authn, human_login, Arc::new(AllowAll))
        .default_token_scheme(SCHEME)
        .route(
            Method::Get,
            "/v1/whoami",
            "edge.whoami",
            Arc::new(WhoamiHandler),
        );
    let builder = register_git_wire(builder, backend);
    (Arc::new(builder.build()), cell)
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

/// Run `git clone` over HTTP with an optional Bearer token. Returns (success, stdout, stderr).
fn git_clone(
    addr: SocketAddr,
    token: Option<&str>,
    repo_url_path: &str,
    dst: &Path,
) -> (bool, String, String) {
    let url = format!("http://{addr}{repo_url_path}");
    let mut c = Command::new("git");
    // Force the smart-HTTP transport; keep output deterministic.
    c.env("GIT_TERMINAL_PROMPT", "0");
    if let Some(t) = token {
        c.arg("-c")
            .arg(format!("http.extraHeader=Authorization: Bearer {t}"));
    }
    c.args(["clone", "--no-local", &url, &dst.to_string_lossy()]);
    let out = c.output().expect("spawn git clone");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

// ═══════════════════════════════ the external-oracle proof ═══════════════════════════════

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_git_clone_fetch_over_smart_http_with_auth() {
    if !require_or_skip("ct006c clone/fetch oracle") {
        return;
    }
    let Some(_rootfs) = git_rootfs() else { return };

    let root = temp_root("oracle");
    // ~2 MiB payload ⇒ a packfile WELL over the old 256 KiB SANDBOX_CAPTURE_BOUND (proves the fix).
    let (origin_head, work) = make_big_repo(&root, "acme", "eu-west", "widgets", 2 * 1024 * 1024);
    println!("=== CT-006c: origin HEAD = {origin_head} (a > 256 KiB packfile) ===");

    let (gw, cell) = build(&root);
    let addr = spawn(gw).await;
    let token = mint(&cell, "acme", "jti-clone");

    // ── 1. REAL `git clone` over smart-HTTP with the Bearer token ──
    let dst = root.join("clone-dst");
    let (ok, so, se) = git_clone(addr, Some(&token), "/acme/eu-west/widgets.git", &dst);
    println!("=== git clone (authenticated) ===\nsuccess={ok}\nstdout=\n{so}\nstderr=\n{se}");
    assert!(
        ok,
        "the authenticated clone of a > 256 KiB-pack repo MUST succeed (streaming fix)"
    );

    // `git fsck` the clone — proves the big pack arrived WHOLE + intact (no early-EOF truncation).
    let fsck = Command::new("git")
        .args(["-C", &dst.to_string_lossy(), "fsck", "--full"])
        .output()
        .unwrap();
    println!(
        "=== git fsck --full ===\nstatus={:?}\nstdout=\n{}\nstderr=\n{}",
        fsck.status.code(),
        String::from_utf8_lossy(&fsck.stdout),
        String::from_utf8_lossy(&fsck.stderr)
    );
    assert!(fsck.status.success(), "git fsck on the clone must be clean");

    // The cloned HEAD matches the origin (the wanted history came through).
    let head = Command::new("git")
        .args(["-C", &dst.to_string_lossy(), "rev-parse", "HEAD"])
        .output()
        .unwrap();
    let cloned_head = String::from_utf8_lossy(&head.stdout).trim().to_string();
    println!("cloned HEAD = {cloned_head}");
    assert_eq!(
        cloned_head, origin_head,
        "cloned HEAD must equal the origin HEAD"
    );
    assert!(
        dst.join("blob-0.bin").exists() && dst.join("README.md").exists(),
        "the cloned working tree must carry the repo content"
    );

    // ── 2. add a commit to the origin, then REAL `git fetch` gets it ──
    std::fs::write(work.join("new.txt"), b"a second commit\n").expect("write new file");
    run_git(&["add", "new.txt"], Some(&work));
    run_git(
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "second commit",
        ],
        Some(&work),
    );
    let bare = root.join("acme/eu-west/widgets.git");
    run_git(
        &["push", "-q", &bare.to_string_lossy(), "main"],
        Some(&work),
    );
    let origin_head2 = {
        let o = Command::new("git")
            .args(["--git-dir", &bare.to_string_lossy(), "rev-parse", "main"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };

    let mut fetch = Command::new("git");
    fetch
        .args(["-C", &dst.to_string_lossy()])
        .arg("-c")
        .arg(format!("http.extraHeader=Authorization: Bearer {token}"))
        .args(["fetch", "origin", "main"]);
    let fout = fetch.output().expect("git fetch");
    println!(
        "=== git fetch origin main ===\nstatus={:?}\nstdout=\n{}\nstderr=\n{}",
        fout.status.code(),
        String::from_utf8_lossy(&fout.stdout),
        String::from_utf8_lossy(&fout.stderr)
    );
    assert!(
        fout.status.success(),
        "the authenticated fetch must succeed"
    );
    let fetched = Command::new("git")
        .args(["-C", &dst.to_string_lossy(), "rev-parse", "FETCH_HEAD"])
        .output()
        .unwrap();
    let fetched_head = String::from_utf8_lossy(&fetched.stdout).trim().to_string();
    println!("FETCH_HEAD = {fetched_head} (origin now {origin_head2})");
    assert_eq!(
        fetched_head, origin_head2,
        "fetch must deliver the new origin commit"
    );

    // ── 3a. UNAUTHENTICATED clone is REFUSED (no token → 401, no bytes) ──
    let dst_noauth = root.join("clone-noauth");
    let (ok_n, so_n, se_n) = git_clone(addr, None, "/acme/eu-west/widgets.git", &dst_noauth);
    println!("=== git clone (NO token) — must be refused ===\nsuccess={ok_n}\nstdout=\n{so_n}\nstderr=\n{se_n}");
    assert!(!ok_n, "an unauthenticated clone MUST be refused");

    // ── 3b. CROSS-TENANT clone is REFUSED (globex's token for acme's repo → IDOR reject, no leak) ──
    let globex = mint(&cell, "globex", "jti-x");
    let dst_xtenant = root.join("clone-xtenant");
    let (ok_x, so_x, se_x) = git_clone(
        addr,
        Some(&globex),
        "/acme/eu-west/widgets.git",
        &dst_xtenant,
    );
    println!("=== git clone (CROSS-TENANT globex→acme) — must be refused ===\nsuccess={ok_x}\nstdout=\n{so_x}\nstderr=\n{se_x}");
    assert!(
        !ok_x,
        "a cross-tenant clone MUST be refused (no repo bytes, no existence leak)"
    );

    println!("=== CT-006c EXTERNAL ORACLE PROVEN: real git clone/fsck/fetch over smart-HTTP + auth/cross-tenant refusal ===");
    let _ = std::fs::remove_dir_all(&root);
}

// ═══════════════════════════ over-the-cap fail-loud (the streaming-bound guard) ═══════════════════════════

/// An upload-pack response that exceeds the GENEROUS wire cap is REFUSED LOUDLY (a `GitCoreError`),
/// NEVER returned as a silently-truncated `Ok` pack. Drives the production `RoutedGitCore` directly with
/// a small per-launch wire cap (the cap derives from `disk_bytes`; the guest `/tmp` is ample so the cap —
/// not an in-guest ENOSPC — is what fires) against a repo whose pack is far larger than the cap.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn over_cap_upload_pack_response_errors_cleanly() {
    use myelin_ci_sandbox::ResourceLimits;
    use myelin_edge::{production_git_core, GitWireExecutor};
    use myelin_git::core::{GitCore, RepoLoc, Service};

    if !require_or_skip("ct006c over-cap fail-loud") {
        return;
    }
    let Some(_rootfs) = git_rootfs() else { return };

    let root = temp_root("overcap");
    // ~16 MiB of incompressible content ⇒ a packfile ≫ the 8 MiB wire cap below.
    let (_head, _work) = make_big_repo(&root, "acme", "eu-west", "huge", 16 * 1024 * 1024);

    // A small WIRE cap (= disk_bytes) but an ample guest /tmp (8 MiB is plenty for upload-pack, which
    // streams the pack to stdout, not to /tmp) — so OUR host-side cap is what trips, not an in-guest
    // disk-full. The pack (~16 MiB) overruns the 8 MiB cap ⇒ fail-loud.
    let limits = ResourceLimits {
        cpu_millis: 2000,
        mem_bytes: 512 * 1024 * 1024,
        disk_bytes: 8 * 1024 * 1024,
        pids_max: 256,
        timeout_secs: 120,
    };
    let core = production_git_core(&root, limits, GitWireExecutor::serving_hooks());
    let repo = RepoLoc::new("acme", "eu-west", "huge");

    // A v0 stateless-rpc fetch request for HEAD — the serve will try to stream the full (oversize) pack.
    let bare = root.join("acme/eu-west/huge.git");
    let oid = {
        let o = Command::new("git")
            .args(["--git-dir", &bare.to_string_lossy(), "rev-parse", "main"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };
    let mut request = {
        let line =
            format!("want {oid} multi_ack_detailed no-progress ofs-delta agent=myelin/ct006c\n");
        let mut v = format!("{:04x}", line.len() + 4).into_bytes();
        v.extend_from_slice(line.as_bytes());
        v
    };
    request.extend_from_slice(b"0000");
    request.extend_from_slice(b"0009done\n");

    let result = core.serve(&repo, Service::UploadPack, request);
    println!("=== CT-006c over-cap serve result = {result:?} ===");
    assert!(
        result.is_err(),
        "an over-the-wire-cap upload-pack response MUST error cleanly (never a silently-truncated Ok pack)"
    );

    let _ = std::fs::remove_dir_all(&root);
}

// ═══════════════════════ R2.1a — R0.3 LIVE: the CheckEngine-backed per-repo authz on the wire ═══════════════════════
//
// The production composition (main.rs) now injects the CheckBackedRepoAuthorizer (Read → check(pull),
// Write → check(push) against the live Git ReBAC fragment) + the TupleRepoBootstrap creator→admin
// grant, and mounts register_git_wire. This oracle drives the SAME composition end-to-end through a
// real serve_edge listener with the host's REAL `git`:
//   RED  (pre-R2.1a: the AllowAllRepos default): mallory's clone/push of an un-granted repo SUCCEEDS
//        — the un-granted-repo-reach hole, live on the wire.
//   GREEN (this composition): mallory is denied 0-leak (404, never a 403 on the read path), the
//        creator's bootstrap grant admits clone+push, and a grant on repo A does not admit repo B.

/// The thresholds-file `[fail_static]` seed (mirrors live_check.rs / the production thresholds).
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

/// Build the R2.1a gateway: real PASETO auth, the durable git routes (create-repo → bootstrap
/// grant) AND the wire endpoints, over a backend carrying the REAL CheckEngine-backed authorizer —
/// the exact production shape (main.rs), with the in-memory S3 double standing in for PG.
fn build_r21a(root: &Path) -> (Arc<Gateway>, CellTokenAuthority) {
    let cell = CellTokenAuthority::from_seed(&[11u8; 32], &[13u8; 32]).expect("cell authority");
    let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
    // THREE distinct in-tenant principals: the creator (granted via bootstrap), mallory (NO grant —
    // the adversary), and a second creator (for the cross-repo isolation leg).
    seed_principal(&store, "acme", "svc:creator", "subj-c");
    seed_principal(&store, "acme", "svc:mallory", "subj-m");
    seed_principal(&store, "acme", "svc:creator2", "subj-c2");

    let authn = Arc::new(CapabilityAuthenticator::with_verifier(
        store,
        Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
        RevocationStore::new(),
    ));
    let human_login = Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(
        Arc::new(KmsEngine::new()),
    )));

    // The production check slot (in-memory S3 double) with the Git fragment ADMITTED — exactly what
    // main.rs composes over the durable store.
    let check = StoreBackedCheck::new(TupleStore::new(OutboxStore::new()));
    for admit in check.admit_git_fragment() {
        assert!(
            matches!(admit, FragmentAdmit::Admitted { .. }),
            "the Git fragment admits: {admit:?}"
        );
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
    let builder = Gateway::builder(authn, human_login, Arc::new(AllowAll))
        .default_token_scheme(SCHEME)
        .route(
            Method::Get,
            "/v1/whoami",
            "edge.whoami",
            Arc::new(WhoamiHandler),
        );
    let builder = register_git_durable(builder, backend.clone());
    let builder = register_git_wire(builder, backend);
    (Arc::new(builder.build()), cell)
}

fn mint_for(cell: &CellTokenAuthority, tenant: &str, jti: &str, subject_key: &str) -> String {
    cell.mint(&CapabilityMintSpec {
        tenant: tenant.into(),
        region: REGION.into(),
        subject_key: subject_key.into(),
        jti: jti.into(),
        exp_unix: now() + 3600,
        authority: vec!["agent:run".into()],
        dpop_jkt: None,
        purpose: myelin_identity_service::CredentialPurpose::OperatorBootstrap,
        audience: myelin_identity_service::CredentialAudience::Edge,
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

/// Make a work repo with one commit under the tenant pseudonym identity (the GIT-1 push gate).
fn r21a_work(root: &Path, tag: &str) -> PathBuf {
    let work = root.join(format!("work-{tag}"));
    std::fs::create_dir_all(&work).expect("mkdir work");
    run_git(&["init", "-q", "-b", "main"], Some(&work));
    run_git(
        &["config", "user.email", "anon-7@acme.noreply"],
        Some(&work),
    );
    run_git(&["config", "user.name", "anon-7@acme.noreply"], Some(&work));
    std::fs::write(work.join("README.md"), b"# r2.1a live-authz oracle\n").expect("write");
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

/// Run `git push <url> main` from `work` with a Bearer token. Returns (success, stdout, stderr).
fn r21a_push(
    addr: SocketAddr,
    token: &str,
    repo_url_path: &str,
    work: &Path,
) -> (bool, String, String) {
    let url = format!("http://{addr}{repo_url_path}");
    let mut c = Command::new("git");
    c.current_dir(work).env("GIT_TERMINAL_PROMPT", "0");
    c.arg("-c")
        .arg(format!("http.extraHeader=Authorization: Bearer {token}"));
    c.args(["push", &url, "main"]);
    let out = c.output().expect("spawn git push");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r2_1a_live_repo_authz_denies_ungranted_admits_creator_and_isolates_repos() {
    if !require_or_skip("r2.1a live repo-authz oracle") {
        return;
    }
    let Some(_rootfs) = git_rootfs() else { return };

    let root = temp_root("r21a");
    let (gw, cell) = build_r21a(&root);
    let addr = spawn(gw).await;
    let t_creator = mint_for(&cell, "acme", "jti-r21a-c", "subj-c");
    let t_mallory = mint_for(&cell, "acme", "jti-r21a-m", "subj-m");
    let t_creator2 = mint_for(&cell, "acme", "jti-r21a-c2", "subj-c2");

    // ── 1. the creator creates `widgets` THROUGH THE EDGE → 201 + the creator→admin bootstrap
    //       grant is written (the R2.1a make-or-break: without it, deny-by-default orphans the repo).
    let resp = http_post(addr, "/v1/git/repos", &t_creator, r#"{"slug":"widgets"}"#);
    println!(
        "=== create widgets (creator) ===\n{}",
        resp.lines().next().unwrap_or("")
    );
    assert!(
        resp.starts_with("HTTP/1.1 201"),
        "create-repo must 201: {resp}"
    );

    // ── 2. ALLOW path (write): the creator's REAL `git push` succeeds off the bootstrap grant
    //       (admin ⊆ push in the frozen fragment).
    let work = r21a_work(&root, "creator");
    let (ok, so, se) = r21a_push(addr, &t_creator, "/acme/eu-west/widgets.git", &work);
    println!("=== creator push ===\nsuccess={ok}\nstdout=\n{so}\nstderr=\n{se}");
    assert!(
        ok,
        "the creator MUST push its fresh repo (bootstrap grant → push)"
    );

    // ── 3. ALLOW path (read): the creator's REAL `git clone` succeeds (admin ⊆ pull).
    let dst = root.join("clone-creator");
    let (ok_c, so_c, se_c) = git_clone(addr, Some(&t_creator), "/acme/eu-west/widgets.git", &dst);
    println!("=== creator clone ===\nsuccess={ok_c}\nstdout=\n{so_c}\nstderr=\n{se_c}");
    assert!(
        ok_c,
        "the creator MUST clone its fresh repo (bootstrap grant → pull)"
    );

    // ── 4. DENY path (read, 0-leak): mallory — SAME tenant, NO grant — is refused with a 404
    //       (repo existence is NOT leaked: the read-deny posture is `not found`, never a 403).
    //       RED before R2.1a: with the AllowAllRepos production default this clone SUCCEEDED.
    let dst_m = root.join("clone-mallory");
    let (ok_m, so_m, se_m) = git_clone(addr, Some(&t_mallory), "/acme/eu-west/widgets.git", &dst_m);
    println!("=== mallory clone (NO grant) — must be 0-leak denied ===\nsuccess={ok_m}\nstdout=\n{so_m}\nstderr=\n{se_m}");
    assert!(
        !ok_m,
        "an un-granted in-tenant clone MUST be refused (the R0.3 hole, closed LIVE)"
    );
    assert!(
        se_m.contains("not found"),
        "the read denial is a 0-leak 404 (`repository not found`): {se_m}"
    );
    // The exact curl-phrase pin (`returned error: 403`) — NOT a bare "403" substring, which can
    // false-match the ephemeral listener PORT in the URL git prints.
    assert!(
        !se_m.contains("returned error: 403"),
        "a read denial must NOT leak existence via a 403: {se_m}"
    );

    // ── 5. DENY path (write): mallory's REAL `git push` is a fail-closed 403.
    let (ok_mp, _so_mp, se_mp) = r21a_push(addr, &t_mallory, "/acme/eu-west/widgets.git", &work);
    println!("=== mallory push (NO grant) — must be 403 ===\nsuccess={ok_mp}\nstderr=\n{se_mp}");
    assert!(!ok_mp, "an un-granted in-tenant push MUST be refused");
    assert!(
        se_mp.contains("returned error: 403"),
        "the write denial is a 403: {se_mp}"
    );

    // ── 6. CROSS-REPO isolation: creator2 creates `secrets`; the creator's grant on `widgets`
    //       does NOT admit `secrets` (0-leak 404 again).
    let resp2 = http_post(addr, "/v1/git/repos", &t_creator2, r#"{"slug":"secrets"}"#);
    assert!(
        resp2.starts_with("HTTP/1.1 201"),
        "create-repo secrets must 201: {resp2}"
    );
    let dst_x = root.join("clone-crossrepo");
    let (ok_x, _so_x, se_x) =
        git_clone(addr, Some(&t_creator), "/acme/eu-west/secrets.git", &dst_x);
    println!("=== creator clone of creator2's `secrets` — must be denied ===\nsuccess={ok_x}\nstderr=\n{se_x}");
    assert!(
        !ok_x,
        "a grant on repo A must NOT admit repo B (cross-repo isolation)"
    );
    assert!(
        se_x.contains("not found"),
        "the cross-repo denial is 0-leak: {se_x}"
    );

    println!("=== R2.1a ORACLE PROVEN: live CheckEngine repo-authz on the wire — 0-leak deny, bootstrap-grant allow, cross-repo isolation ===");
    let _ = std::fs::remove_dir_all(&root);
}

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

use myelin_edge::{
    register_git_wire, serve_edge, AllowAll, DurableGitBackend, GitCheckRepoAuthorizer, Gateway,
    Method, RepoGrantWriter, TupleStoreGrantWriter, WhoamiHandler,
};
use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::{
    DataRole, ObjectId, Principal, PrincipalId, PrincipalKind, PrincipalStatus, RelName,
    RelationTuple, TupleDelta,
};
use myelin_identity_service::{
    CapabilityAuthenticator, CapabilityMintSpec, CellTokenAuthority, HumanSsoAuthenticator,
    PasetoCapabilityVerifier, PrincipalStore, RevocationStore, StoreBackedCheck, TupleStore,
};
use myelin_ci_sandbox::{resolved_gvisor_rootfs, ENV_GVISOR_GIT_ROOTFS};
use myelin_git::core::RepoLoc;
use myelin_git::durable::DurableGitStore;
use myelin_git::live_check::GitCheckGate;
use myelin_substrate::FailStaticThreshold;
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
    Command::new("git").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
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
    let staged = std::env::temp_dir().join(format!("myelin-ct006d-push-rootfs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staged);
    let st = Command::new("cp").arg("-a").arg(format!("{}/.", base.display())).arg(&staged).status().expect("cp -a");
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
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

fn temp_root(tag: &str) -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let d = std::env::temp_dir().join(format!("myelin-ct006d-{tag}-{}-{nanos}", std::process::id()));
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
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn seed_principal(store: &PrincipalStore, tenant: &str, pid: &str, subject_key: &str) {
    let scope = TenantScope::from_verified_token(
        &Principal::stub(PrincipalId("admin".into()), PrincipalKind::Human, TenantId(tenant.into())),
        Region(REGION.into()),
    );
    store.put_principal(&scope, PrincipalId(pid.into()), PrincipalKind::Service, DataRole::Controller, PrincipalStatus::Active, None).expect("seed");
    store.link_credential(&scope, SCHEME, subject_key, &PrincipalId(pid.into())).expect("link");
}

fn build(root: &Path) -> (Arc<Gateway>, CellTokenAuthority, Arc<DurableGitBackend>) {
    let cell = CellTokenAuthority::from_seed(&[7u8; 32], &[9u8; 32]).expect("cell");
    let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
    seed_principal(&store, "acme", "svc:agent", "subj-1");
    seed_principal(&store, "globex", "svc:agent", "subj-1");
    let authn = Arc::new(CapabilityAuthenticator::with_verifier(
        store,
        Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
        RevocationStore::new(),
    ));
    let human = Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(Arc::new(KmsEngine::new()))));
    let backend = Arc::new(DurableGitBackend::rooted_inmem_for_test(root.to_path_buf()));
    let builder = Gateway::builder(authn, human, Arc::new(AllowAll))
        .default_token_scheme(SCHEME)
        .route(Method::Get, "/v1/whoami", "edge.whoami", Arc::new(WhoamiHandler));
    let builder = register_git_wire(builder, backend.clone());
    (Arc::new(builder.build()), cell, backend)
}

// ───────── the R2.1a LIVE-authorizer build: the push wire gated by the real Identity `check` ─────────

fn threshold() -> FailStaticThreshold {
    FailStaticThreshold {
        status: "OPEN — LEGAL".into(),
        owner: "DPO / Legal".into(),
        static_max_secs: None,
        static_max_default_secs: 300,
        agent_token_ttl_secs: 60,
        constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
    }
}

/// The VERIFIED wire principal the minted `subj-1` token resolves to (the seeded `svc:agent` in
/// `tenant`, region `eu-west`) — the subject the grants below target + the check scopes on.
fn wire_principal(tenant: &str) -> Principal {
    let mut p = Principal::stub(
        PrincipalId("svc:agent".into()),
        PrincipalKind::Service,
        TenantId(tenant.into()),
    );
    p.region = Region(REGION.into());
    p
}

/// Write a `repo:<slug>#<relation>@svc:agent` grant into the SHARED tuple store the live check reads.
fn grant(store: &TupleStore, tenant: &str, slug: &str, relation: &str) {
    let p = wire_principal(tenant);
    let scope = TenantScope::from_verified_token(&p, p.region.clone());
    let delta = TupleDelta::Add(RelationTuple {
        object: ObjectId(format!("repo:{slug}")),
        relation: RelName(relation.into()),
        subject: PrincipalId("svc:agent".into()),
        caveat: None,
    });
    store
        .write_tuples(&scope, &p, &[delta], None, None, Timestamp("2026-07-15T00:00:00Z".into()))
        .expect("write grant");
}

/// Build a gateway whose git wire is gated by the R2.1a LIVE per-repo authorizer + bootstrap-grant seam.
/// Returns the gateway, cell, the SHARED tuple store (to write grants), and the backend.
fn build_live(root: &Path) -> (Arc<Gateway>, CellTokenAuthority, TupleStore, Arc<DurableGitBackend>) {
    let cell = CellTokenAuthority::from_seed(&[7u8; 32], &[9u8; 32]).expect("cell");
    let store = PrincipalStore::new(Arc::new(KmsEngine::new()));
    seed_principal(&store, "acme", "svc:agent", "subj-1");
    seed_principal(&store, "globex", "svc:agent", "subj-1");
    let authn = Arc::new(CapabilityAuthenticator::with_verifier(
        store,
        Arc::new(PasetoCapabilityVerifier::new(cell.trust_anchor())),
        RevocationStore::new(),
    ));
    let human =
        Arc::new(HumanSsoAuthenticator::production(PrincipalStore::new(Arc::new(KmsEngine::new()))));

    let tuples = TupleStore::new(OutboxStore::new());
    let check = StoreBackedCheck::new(tuples.clone());
    for admit in check.admit_git_fragment() {
        assert!(
            matches!(admit, myelin_identity::FragmentAdmit::Admitted { .. }),
            "the Git fragment admits: {admit:?}"
        );
    }
    let gate = GitCheckGate::try_new(check, 300, &threshold()).expect("valid staleness bound");
    let authorizer = Arc::new(GitCheckRepoAuthorizer::new(gate, RevocationStore::new()));
    let grant_writer = Arc::new(TupleStoreGrantWriter::new(tuples.clone()));

    let backend = Arc::new(
        DurableGitBackend::rooted_inmem_for_test(root.to_path_buf())
            .with_repo_authorizer(authorizer)
            .with_grant_writer(grant_writer),
    );
    let builder = Gateway::builder(authn, human, Arc::new(AllowAll))
        .default_token_scheme(SCHEME)
        .route(Method::Get, "/v1/whoami", "edge.whoami", Arc::new(WhoamiHandler));
    let builder = register_git_wire(builder, backend.clone());
    (Arc::new(builder.build()), cell, tuples, backend)
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
    run_git(&["config", "user.email", "anon-7@acme.noreply"], Some(&work));
    run_git(&["config", "user.name", "anon-7@acme.noreply"], Some(&work));
    std::fs::write(work.join("README.md"), b"# ct006d push oracle\n").unwrap();
    run_git(&["add", "-A"], Some(&work));
    run_git(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", "initial"], Some(&work));
    work
}

/// Run `git push <url> main` with an optional Bearer token. Returns (success, stdout, stderr).
fn git_push(addr: SocketAddr, token: Option<&str>, repo_url_path: &str, work: &Path) -> (bool, String, String) {
    let url = format!("http://{addr}{repo_url_path}");
    let mut c = Command::new("git");
    c.current_dir(work).env("GIT_TERMINAL_PROMPT", "0");
    if let Some(t) = token {
        c.arg("-c").arg(format!("http.extraHeader=Authorization: Bearer {t}"));
    }
    c.args(["push", &url, "main"]);
    let out = c.output().expect("spawn git push");
    (out.status.success(), String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string())
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
        .create_repo("acme", REGION, "widgets", &wire_principal("acme"))
        .expect("create server repo");

    let (gw, cell, backend) = build(&root);
    let addr = spawn(gw).await;
    let token = mint(&cell, "acme", "jti-push");

    let work = make_work(&root);
    let pushed_oid = {
        let o = Command::new("git").args(["-C", &work.to_string_lossy(), "rev-parse", "HEAD"]).output().unwrap();
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
    assert_eq!(tip.as_deref(), Some(pushed_oid.as_str()), "the pushed ref must survive a fresh re-open");

    // `git fsck --full` on the SERVER bare repo is clean (the migrated objects are intact + connected).
    let bare = root.join("acme/eu-west/widgets.git");
    let fsck = Command::new("git").args(["--git-dir", &bare.to_string_lossy(), "fsck", "--full"]).output().unwrap();
    println!("=== git fsck --full (server repo) ===\nstatus={:?}\nstderr=\n{}", fsck.status.code(), String::from_utf8_lossy(&fsck.stderr));
    assert!(fsck.status.success(), "git fsck on the server repo must be clean");

    // emit-iff-committed: exactly ONE git.ref.updated row became durable for the accepted ref move.
    let depth_after = backend.outbox().outbox_depth();
    println!("outbox depth: before={depth_before} after={depth_after} (expect +1)");
    assert_eq!(depth_after, depth_before + 1, "the accepted push emits exactly one git.ref.updated (0 ghost / 0 lost)");

    // ── 2. a REJECTED push (a planted AWS-key secret) does NOT move the ref + emits NO event ──
    let tip_before_reject = durable_tip(&root, "acme", "widgets", "refs/heads/main");
    let depth_before_reject = backend.outbox().outbox_depth();
    std::fs::write(work.join("creds.txt"), b"aws_key = AKIAIOSFODNN7EXAMPLE\n").unwrap();
    run_git(&["add", "-A"], Some(&work));
    run_git(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", "oops a secret"], Some(&work));
    let (ok_s, so_s, se_s) = git_push(addr, Some(&token), "/acme/eu-west/widgets.git", &work);
    println!("=== git push (PLANTED SECRET) — must be rejected ===\nsuccess={ok_s}\nstdout=\n{so_s}\nstderr=\n{se_s}");
    assert!(!ok_s, "a push carrying a secret MUST be rejected");
    let tip_after_reject = durable_tip(&root, "acme", "widgets", "refs/heads/main");
    assert_eq!(tip_after_reject, tip_before_reject, "a rejected push MUST NOT move the ref (0 ghost)");
    assert_eq!(backend.outbox().outbox_depth(), depth_before_reject, "a rejected push emits NO event");
    // The server repo is still fsck-clean (no half-migrated corruption from the rejected push).
    let fsck2 = Command::new("git").args(["--git-dir", &bare.to_string_lossy(), "fsck", "--full"]).output().unwrap();
    assert!(fsck2.status.success(), "the server repo stays fsck-clean after a rejected push");
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
    run_git(&["-c", "commit.gpgsign=false", "commit", "-q", "-m", "x"], Some(&work));
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

// ═══════════ R2.1a: the R0 acceptance contract — un-granted repo push is DENIED (403, no ref move) ═══════════

/// **R2.1a — the LIVE per-repo authorizer over a real `git push` (the R0 write-side done-bar).** Drives
/// the host's REAL `git push` through the FULL gateway lifecycle with the wire gated by the real Identity
/// `check`:
///   (a) an in-tenant, authenticated principal with **NO grant** on the repo → `git push` gets **403** and
///       the ref does NOT move (no object ingested);
///   (b) after the **bootstrap admin grant** → the SAME push SUCCEEDS + lands DURABLY (a fresh re-open
///       sees the ref; the outbox gained exactly one `git.ref.updated`);
///   (c) a **read-only (`reader`) grant** → `git push` is still **403** (read confers pull, never push).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_authorizer_denies_ungranted_push_admits_admin_grant_reader_cannot_push() {
    if !require_or_skip("r2.1a live-authorizer push oracle") {
        return;
    }
    let Some(_rootfs) = git_rootfs() else { return };

    let root = temp_root("live-push");
    // Two server bare repos created WITHOUT the grant seam (a plain backend) → they exist on disk with
    // NO grant for the pushing principal (the deny-by-default starting state).
    let plain = DurableGitBackend::rooted_inmem_for_test(root.clone());
    plain
        .create_repo("acme", REGION, "widgets", &wire_principal("acme"))
        .expect("create widgets (no grant)");
    plain
        .create_repo("acme", REGION, "docs", &wire_principal("acme"))
        .expect("create docs (no grant)");

    let (gw, cell, tuples, _backend) = build_live(&root);
    let addr = spawn(gw).await;
    let token = mint(&cell, "acme", "jti-live-push");
    let work = make_work(&root);
    let pushed_oid = {
        let o = Command::new("git")
            .args(["-C", &work.to_string_lossy(), "rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };

    // ── (a) NO grant → push refused (403), the ref does NOT move ──
    let (ok_d, _so_d, se_d) = git_push(addr, Some(&token), "/acme/eu-west/widgets.git", &work);
    println!("=== git push (in-tenant, NO grant) — must be refused ===\nsuccess={ok_d}\nstderr=\n{se_d}");
    assert!(!ok_d, "an in-tenant principal with NO write grant MUST be refused the push");
    assert_eq!(
        durable_tip(&root, "acme", "widgets", "refs/heads/main"),
        None,
        "a denied push moves NO ref (no object ingested)"
    );

    // ── (b) bootstrap ADMIN grant → push succeeds + lands durably ──
    TupleStoreGrantWriter::new(tuples.clone())
        .grant_repo_admin(&wire_principal("acme"), &RepoLoc::new("acme", REGION, "widgets"))
        .expect("bootstrap admin grant");
    let depth_before = _backend.outbox().outbox_depth();
    let (ok_a, _so_a, se_a) = git_push(addr, Some(&token), "/acme/eu-west/widgets.git", &work);
    println!("=== git push (ADMIN grant) — must succeed ===\nsuccess={ok_a}\nstderr=\n{se_a}");
    assert!(ok_a, "an admin-granted principal pushes end-to-end through the live authorizer");
    assert_eq!(
        durable_tip(&root, "acme", "widgets", "refs/heads/main").as_deref(),
        Some(pushed_oid.as_str()),
        "the granted push lands durably (a fresh re-open sees the new tip)"
    );
    assert_eq!(
        _backend.outbox().outbox_depth(),
        depth_before + 1,
        "the accepted push emits exactly one git.ref.updated"
    );

    // ── (c) read-only (reader) grant → push still 403 (read ≠ write) ──
    grant(&tuples, "acme", "docs", "reader");
    let (ok_r, _so_r, se_r) = git_push(addr, Some(&token), "/acme/eu-west/docs.git", &work);
    println!("=== git push (READER grant) — must be refused (read ≠ write) ===\nsuccess={ok_r}\nstderr=\n{se_r}");
    assert!(!ok_r, "a read-only grant does NOT confer push (403)");
    assert_eq!(
        durable_tip(&root, "acme", "docs", "refs/heads/main"),
        None,
        "the reader-only push moved no ref"
    );

    println!("=== R2.1a PROVEN (push): un-granted push DENIED (403, 0 ref move); admin grant admits; reader cannot push ===");
    let _ = std::fs::remove_dir_all(&root);
}

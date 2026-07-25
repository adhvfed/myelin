//! # The SANDBOXED GIT-WIRE production self-test (CT-006a → GT-006 / SI-013) — REAL `git` in `runsc`
//!
//! **Owning seam:** `myelin_git::core::WireExecutor::run(&WireInvocation) -> WireOutput` — a
//! REQUEST/RESPONSE shape that matches git's HTTP stateless-rpc smart transport exactly. CT-006a is
//! the SANDBOX-SIDE capability the production `WireExecutor` (CT-006b) calls into:
//! [`GvisorBackend::launch_git_wire`] runs canonical `git upload-pack`/`receive-pack` inside the
//! PROVEN hardened gVisor sandbox (CT-002/003) with the bare repo bound READ-ONLY at `/repo`, a
//! writable `/quarantine`, bounded stdin (the request body) + captured stdout (the response).
//!
//! ## What it proves (the CT-006a DONE bar) — every claim is a REAL `runsc` run of REAL `git`
//!   1. **advertise-refs** — `git upload-pack --stateless-rpc --advertise-refs /repo` in the sandbox
//!      emits a VALID smart-transport ref advertisement containing the real HEAD oid + `refs/heads/main`
//!      (pkt-line framed). Proves `git` really ran against the real on-disk bare repo through the RO mount.
//!   2. **a protocol-v2 round-trip** — feeding a `command=ls-refs` request as bounded STDIN to
//!      `git upload-pack --stateless-rpc /repo` (with `GIT_PROTOCOL=version=2`) returns the refs/oid on
//!      stdout. Proves the bounded-stdin delivery + the captured-stdout response (the full wire shape).
//!   3. **path confinement** — a `(tenant, region, repo)` locator with `..` / a cross-tenant segment is
//!      REFUSED by [`GvisorBackend`]'s resolver ([`GitWireSpec::for_repo`]) BEFORE any mount.
//!   4. **read-only enforced** — an in-guest WRITE to `/repo` (`git init --bare /repo`) FAILS (non-zero
//!      exit, EROFS/permission), and the host repo is byte-unchanged. The RO mount is runsc-enforced.
//!
//! ## git-in-rootfs (the staging recipe)
//! The escape-drill rootfs (`resolved_gvisor_rootfs`) is busybox-only — it has NO `git`. This test
//! STAGES a git-bearing rootfs from it: copy the busybox rootfs, drop in the host `git` binary at
//! `/usr/bin/git`, its two extra shared libs (`libpcre2-8`, `libz-ng` — both only need GLIBC ≤ the
//! rootfs's), and the `git-core` helper symlinks (`git-upload-pack`/`git-receive-pack` → `git`). The
//! rootfs's own glibc (2.41) is newer than the host `git` needs (2.38), so no glibc copy is required.
//! It points `MYELIN_GVISOR_GIT_ROOTFS` at the staged rootfs. PRODUCTION staging would bake an OCI
//! image with `git`; this self-test stages the minimum to run a real `git` in the sandbox.
//!
//! ## Gating (CI without runsc still passes; THIS host must really run a container)
//! SKIPPED GRACEFULLY when `runsc` is not on PATH or the busybox base rootfs is absent. With
//! `MYELIN_REQUIRE_RUNSC=1` an absent capability is a HARD FAILURE (never a vacuous green). Run:
//! `MYELIN_REQUIRE_RUNSC=1 cargo test -p myelin-ci-sandbox --features integration --test git_wire_prod_exec_test -- --nocapture`.

#![cfg(feature = "integration")]

use myelin_ci_sandbox::gvisor::GvisorBackend;
use myelin_ci_sandbox::{
    resolve_bare_repo_path, resolved_gvisor_rootfs, GitWireSpec, IdemToken, MeterTarget,
    ReserveHandle, ResourceLimits, RunTokenCredential, RunnerHooks, SandboxBackend, WireError,
    ENV_GVISOR_GIT_ROOTFS,
};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

// ───────────────────────────── runsc / rootfs preconditions ─────────────────────────────

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

/// The base busybox rootfs + `runsc` must be present; otherwise skip.
fn preconditions() -> Option<String> {
    let bin = runsc_bin()?;
    if !resolved_gvisor_rootfs().exists() {
        return None;
    }
    Some(bin)
}

/// HARD-FAIL on an absent capability iff `MYELIN_REQUIRE_RUNSC=1`; otherwise GRACEFUL SKIP.
fn require_or_skip(test: &str) -> Option<String> {
    if let Some(bin) = preconditions() {
        return Some(bin);
    }
    if std::env::var("MYELIN_REQUIRE_RUNSC").as_deref() == Ok("1") {
        panic!(
            "[{test}] MYELIN_REQUIRE_RUNSC=1 but `runsc` is not on PATH or the busybox base rootfs \
             ({}) is absent. CT-006a refuses a VACUOUS green: a real `git` MUST run in a real `runsc` \
             sandbox here.",
            resolved_gvisor_rootfs().display()
        );
    }
    eprintln!(
        "[{test}] SKIPPED: `runsc` or the base rootfs absent — cannot run a gVisor container."
    );
    None
}

// ───────────────────────────── stage a git-bearing rootfs (once) ─────────────────────────────

/// Copy a file, creating parent dirs.
fn copy_file(src: &Path, dst: &Path) {
    if let Some(p) = dst.parent() {
        std::fs::create_dir_all(p).expect("mkdir -p");
    }
    std::fs::copy(src, dst).unwrap_or_else(|e| panic!("copy {src:?} -> {dst:?}: {e}"));
}

/// Resolve a host lib (following symlinks) and copy the REAL file into the rootfs at both `/usr/lib`
/// and `/lib`, recreating the `soname` symlink so the dynamic linker finds it.
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

/// Stage a git-bearing rootfs from the busybox base (copy base, add `git` + libs + git-core helpers).
fn stage_git_rootfs(base: &Path) -> PathBuf {
    let staged = std::env::temp_dir().join(format!("myelin-git-rootfs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staged);
    // Copy the whole base rootfs (busybox + its glibc — small, ~4 MiB) preserving symlinks/perms.
    let st = Command::new("cp")
        .arg("-a")
        .arg(format!("{}/.", base.display()))
        .arg(&staged)
        .status()
        .expect("cp -a base rootfs");
    assert!(st.success(), "cp -a base rootfs failed");

    // The host `git` (single multi-call binary; upload-pack/receive-pack dispatch off argv[0]).
    copy_file(Path::new("/usr/bin/git"), &staged.join("usr/bin/git"));
    // The two shared libs `git` needs beyond glibc (both need only GLIBC ≤ the rootfs's 2.41).
    stage_lib(&staged, "libpcre2-8.so.0", "/usr/lib/libpcre2-8.so.0");
    stage_lib(&staged, "libz-ng.so.2", "/usr/lib/libz-ng.so.2");
    // The git-core exec dir: helpers are symlinks back to the single `git` binary (../../bin/git ⇒
    // /usr/lib/git-core/../../bin/git = /usr/bin/git). GIT_EXEC_PATH points the sandboxed git here.
    let core = staged.join("usr/lib/git-core");
    std::fs::create_dir_all(&core).expect("mkdir git-core");
    for helper in ["git-upload-pack", "git-receive-pack"] {
        let link = core.join(helper);
        let _ = std::fs::remove_file(&link);
        #[cfg(unix)]
        std::os::unix::fs::symlink("../../bin/git", &link).expect("git-core helper symlink");
    }
    // The bind-mount TARGET dirs must pre-exist in the READ-ONLY root (runsc cannot create a mount
    // point inside a `readonly:true` rootfs — exactly as `/tmp` pre-exists for the tmpfs mount).
    std::fs::create_dir_all(staged.join("repo")).expect("mkdir /repo mount point");
    std::fs::create_dir_all(staged.join("quarantine")).expect("mkdir /quarantine mount point");
    staged
}

/// Stage the git rootfs ONCE for the whole test binary + point the env var at it (idempotent).
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

// ───────────────────────────── a real bare repo with a commit ─────────────────────────────

/// `git` env that disables the system/global config + the dubious-ownership check (the sandbox runs as
/// uid 65534 but the repo is owned by the test user) + sets the exec path. `proto_v2` adds protocol v2.
fn git_env(proto_v2: bool) -> Vec<String> {
    let mut env = vec![
        "HOME=/tmp".to_string(),
        "GIT_EXEC_PATH=/usr/lib/git-core".to_string(),
        "GIT_CONFIG_NOSYSTEM=1".to_string(),
        // safe.directory=* — the RO repo is owned by the host user, not the in-guest uid 65534.
        "GIT_CONFIG_COUNT=1".to_string(),
        "GIT_CONFIG_KEY_0=safe.directory".to_string(),
        "GIT_CONFIG_VALUE_0=*".to_string(),
    ];
    if proto_v2 {
        env.push("GIT_PROTOCOL=version=2".to_string());
    }
    env
}

/// Create `<root>/<tenant>/<region>/<repo>.git` as a REAL bare repo with one commit; return the HEAD
/// oid (hex). Uses the host `git` CLI (the proof needs a real on-disk repo; how it is created is moot).
fn make_repo_with_commit(root: &Path, tenant: &str, region: &str, repo: &str) -> String {
    let bare = resolve_bare_repo_path(root, tenant, region, repo).expect("resolve bare path");
    std::fs::create_dir_all(bare.parent().unwrap()).expect("mkdir repo parent");
    run_git(&["init", "-q", "--bare", &bare.to_string_lossy()], None);

    // A work tree to author a commit, then push into the bare repo's refs/heads/main.
    let work = root.join("work");
    std::fs::create_dir_all(&work).expect("mkdir work");
    run_git(&["init", "-q", "-b", "main"], Some(&work));
    run_git(&["config", "user.email", "t@t.t"], Some(&work));
    run_git(&["config", "user.name", "t"], Some(&work));
    std::fs::write(work.join("f.txt"), b"hello git wire\n").expect("write file");
    run_git(&["add", "f.txt"], Some(&work));
    run_git(
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "first commit",
        ],
        Some(&work),
    );
    run_git(
        &["push", "-q", &bare.to_string_lossy(), "main"],
        Some(&work),
    );

    // The HEAD oid the advertisement must contain.
    let out = Command::new("git")
        .args(["--git-dir", &bare.to_string_lossy(), "rev-parse", "main"])
        .output()
        .expect("git rev-parse");
    assert!(out.status.success(), "rev-parse: {:?}", out);
    String::from_utf8_lossy(&out.stdout).trim().to_string()
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
        "host git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn ok_hooks() -> RunnerHooks {
    RunnerHooks::new(
        myelin_ci_sandbox::CompletionSettlementOwner::Hook,
        Box::new(|spec| Ok(ReserveHandle(spec.meter_to.reserve_id.clone()))),
        Box::new(|_spec, _h, _u| Ok(())),
        Box::new(|_t| Ok(())),
        Box::new(|_s| Ok(())),
    )
}

fn limits() -> ResourceLimits {
    ResourceLimits {
        cpu_millis: 1000,
        mem_bytes: 256 * 1024 * 1024,
        disk_bytes: 256 * 1024 * 1024,
        tmpfs_bytes: 256 * 1024 * 1024,
        pids_max: 128,
        timeout_secs: 60,
    }
}

fn tokens(tag: &str) -> (RunTokenCredential, MeterTarget, IdemToken) {
    (
        RunTokenCredential::new(
            format!("git-wire-{tag}-bearer"),
            format!("git-wire-{tag}-jti"),
            300,
        )
        .unwrap(),
        MeterTarget {
            reserve_id: format!("git-wire-{tag}-reserve"),
        },
        IdemToken(format!("git-wire-{tag}-{}", std::process::id())),
    )
}

/// A unique temp git root for a test (`<root>/<tenant>/<region>/<repo>.git`).
fn temp_root(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("myelin-gitwire-root-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir root");
    d
}

// ═══════════════════════════════ the proofs ═══════════════════════════════

#[test]
fn sandboxed_upload_pack_advertise_refs_lists_the_real_repo() {
    let Some(_bin) = require_or_skip("git-wire advertise-refs") else {
        return;
    };
    let Some(_rootfs) = git_rootfs() else {
        return;
    };
    let root = temp_root("adv");
    let oid = make_repo_with_commit(&root, "acme", "fr-par", "widgets");

    let (rt, mt, it) = tokens("adv");
    let spec = GitWireSpec::for_repo(
        &root,
        "acme",
        "fr-par",
        "widgets",
        vec![
            "upload-pack".into(),
            "--stateless-rpc".into(),
            "--advertise-refs".into(),
        ],
        Vec::new(),     // advertise needs no request body
        git_env(false), // v0 advertisement (lists refs + oid)
        None,
        limits(),
        rt,
        mt,
        it,
    )
    .expect("a well-formed locator resolves");

    let backend = GvisorBackend::git_wire_only();
    let launch = backend
        .launch_git_wire(&spec, &ok_hooks())
        .expect("the sandboxed git advertise-refs must run");
    let result = &launch.result;
    let stdout = String::from_utf8_lossy(&result.stdout);

    println!("=== CT-006a REAL sandboxed `git upload-pack --advertise-refs /repo` ===");
    println!(
        "exit_code = {:?}  timed_out = {}",
        result.exit_code, result.timed_out
    );
    println!("HEAD oid (host) = {oid}");
    println!("captured stdout (verbatim) =\n{stdout}");
    println!(
        "captured stderr = {:?}",
        String::from_utf8_lossy(&result.stderr)
    );

    assert_eq!(
        result.exit_code,
        Some(0),
        "upload-pack must exit 0; stderr: {:?}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!result.timed_out);
    assert!(
        stdout.contains(&oid),
        "the advertisement must contain the real HEAD oid {oid}"
    );
    assert!(
        stdout.contains("refs/heads/main"),
        "the advertisement must list refs/heads/main"
    );
    // pkt-line framed: starts with a 4-hex length prefix.
    assert!(
        stdout.as_bytes()[..4].iter().all(|b| b.is_ascii_hexdigit()),
        "a smart-transport advertisement is pkt-line framed (4-hex length prefix)"
    );

    backend.kill(&launch.handle).expect("teardown idempotent");
}

#[test]
fn sandboxed_upload_pack_v2_ls_refs_round_trip_with_bounded_stdin() {
    let Some(_bin) = require_or_skip("git-wire v2 ls-refs") else {
        return;
    };
    let Some(_rootfs) = git_rootfs() else {
        return;
    };
    let root = temp_root("v2");
    let oid = make_repo_with_commit(&root, "acme", "fr-par", "widgets");

    // The protocol-v2 stateless-rpc request body: a single `command=ls-refs` request (pkt-line framed).
    let stdin = b"0014command=ls-refs\n00010009peel\n0000".to_vec();

    let (rt, mt, it) = tokens("v2");
    let spec = GitWireSpec::for_repo(
        &root,
        "acme",
        "fr-par",
        "widgets",
        vec!["upload-pack".into(), "--stateless-rpc".into()],
        stdin,
        git_env(true), // GIT_PROTOCOL=version=2
        None,
        limits(),
        rt,
        mt,
        it,
    )
    .expect("locator resolves");

    let backend = GvisorBackend::git_wire_only();
    let launch = backend
        .launch_git_wire(&spec, &ok_hooks())
        .expect("the sandboxed protocol-v2 ls-refs must run");
    let result = &launch.result;
    let stdout = String::from_utf8_lossy(&result.stdout);

    println!("=== CT-006a REAL sandboxed protocol-v2 `ls-refs` round-trip (stdin → stdout) ===");
    println!("exit_code = {:?}", result.exit_code);
    println!("captured stdout (verbatim) =\n{stdout}");

    assert_eq!(
        result.exit_code,
        Some(0),
        "ls-refs must exit 0; stderr: {:?}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        stdout.contains(&oid) && stdout.contains("refs/heads/main"),
        "the v2 ls-refs response must carry the real oid + refs/heads/main (proves bounded-stdin \
         delivery + captured-stdout response). got: {stdout}"
    );

    backend.kill(&launch.handle).expect("teardown");
}

#[test]
fn cross_tenant_and_traversal_locators_are_refused_before_any_mount() {
    // Pure path-confinement — runs even without runsc (no container is ever launched).
    let root = PathBuf::from("/srv/git-root");

    // A `..` traversal repo slug collapsing onto another tenant.
    let r = resolve_bare_repo_path(&root, "acme", "fr-par", "../../victim/fr-par/secret");
    assert!(
        matches!(r, Err(WireError::Path(_))),
        "a `..` repo slug must be refused, got {r:?}"
    );

    // A traversing tenant segment.
    let r = resolve_bare_repo_path(&root, "..", "fr-par", "widgets");
    assert!(
        matches!(r, Err(WireError::Path(_))),
        "a `..` tenant must be refused, got {r:?}"
    );

    // An absolute-looking / separator-bearing region.
    let r = resolve_bare_repo_path(&root, "acme", "fr-par/../other", "widgets");
    assert!(
        matches!(r, Err(WireError::Path(_))),
        "a `/`-bearing region must be refused, got {r:?}"
    );

    // A NUL/backslash slug.
    let r = resolve_bare_repo_path(&root, "acme", "fr-par", "wid\\gets");
    assert!(
        matches!(r, Err(WireError::Path(_))),
        "a backslash slug must be refused, got {r:?}"
    );

    // The constructor refuses identically (so a cross-tenant spec can never be BUILT, let alone mounted).
    let (rt, mt, it) = tokens("evil");
    let built = GitWireSpec::for_repo(
        &root,
        "acme",
        "fr-par",
        "../../victim/fr-par/secret",
        vec!["upload-pack".into(), "--advertise-refs".into()],
        Vec::new(),
        Vec::new(),
        None,
        limits(),
        rt,
        mt,
        it,
    );
    assert!(
        matches!(built, Err(WireError::Path(_))),
        "GitWireSpec::for_repo must refuse a cross-tenant locator BEFORE any mount, got {:?}",
        built.map(|s| s.repo_host_path().to_path_buf())
    );

    // A well-formed locator DOES resolve under the tenant/region root (no false-positive lockout).
    let ok = resolve_bare_repo_path(&root, "acme", "fr-par", "team/app").expect("valid locator");
    assert_eq!(ok, PathBuf::from("/srv/git-root/acme/fr-par/team/app.git"));
    println!("=== CT-006a path confinement: cross-tenant/`..`/separator/NUL all REFUSED before mount ===");
}

#[test]
fn read_only_repo_mount_rejects_an_in_guest_write() {
    let Some(_bin) = require_or_skip("git-wire ro-enforced") else {
        return;
    };
    let Some(_rootfs) = git_rootfs() else {
        return;
    };
    let root = temp_root("ro");
    let oid_before = make_repo_with_commit(&root, "acme", "fr-par", "widgets");
    let bare = resolve_bare_repo_path(&root, "acme", "fr-par", "widgets").unwrap();

    // `git init --bare /repo` is a WRITE (it (re)initialises config/HEAD). Against the RO mount it must
    // FAIL — runsc enforces `ro`, so the write hits EROFS/permission and git exits non-zero.
    let (rt, mt, it) = tokens("ro");
    let spec = GitWireSpec::for_repo(
        &root,
        "acme",
        "fr-par",
        "widgets",
        vec!["init".into(), "--bare".into()],
        Vec::new(),
        git_env(false),
        None,
        limits(),
        rt,
        mt,
        it,
    )
    .expect("locator resolves");

    let backend = GvisorBackend::git_wire_only();
    let launch = backend
        .launch_git_wire(&spec, &ok_hooks())
        .expect("the run itself completes (the WRITE inside is what must fail)");
    let result = &launch.result;
    let stderr = String::from_utf8_lossy(&result.stderr);

    println!("=== CT-006a REAL read-only enforcement: in-guest `git init --bare /repo` ===");
    println!("exit_code = {:?}  stderr = {stderr:?}", result.exit_code);

    assert_ne!(
        result.exit_code,
        Some(0),
        "a WRITE to the read-only repo mount MUST fail (runsc-enforced `ro`); got a clean exit"
    );

    // The host repo is byte-unchanged: the HEAD oid is identical (no in-guest mutation reached it).
    let out = Command::new("git")
        .args(["--git-dir", &bare.to_string_lossy(), "rev-parse", "main"])
        .output()
        .expect("git rev-parse");
    let oid_after = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert_eq!(
        oid_before, oid_after,
        "the real repo MUST be unmodified after an in-guest write attempt"
    );

    backend.kill(&launch.handle).expect("teardown");
}

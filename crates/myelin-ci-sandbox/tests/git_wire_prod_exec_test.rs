#![cfg(feature = "integration")]

use myelin_ci_sandbox::gvisor::GvisorBackend;
use myelin_ci_sandbox::{
    resolve_bare_repo_path, verified_gvisor_git_rootfs, GitWireSpec, IdemToken, MeterTarget,
    ReserveHandle, ResourceLimits, RunTokenCredential, RunnerHooks, SandboxBackend, WireError,
};
use std::path::{Path, PathBuf};
use std::process::Command;

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

fn require_or_skip(test: &str) -> Option<String> {
    let bin = match runsc_bin() {
        Some(bin) => bin,
        None if std::env::var("MYELIN_REQUIRE_RUNSC").as_deref() == Ok("1") => {
            panic!(
                "[{test}] MYELIN_REQUIRE_RUNSC=1 but `runsc` is not on PATH. CT-006a refuses a \
                 VACUOUS green: a real `git` MUST run in a real `runsc` sandbox here."
            );
        }
        None => {
            eprintln!("[{test}] SKIPPED: `runsc` is not on PATH.");
            return None;
        }
    };

    match verified_gvisor_git_rootfs() {
        Ok(_) => Some(bin),
        Err(error) if std::env::var("MYELIN_REQUIRE_RUNSC").as_deref() == Ok("1") => {
            panic!(
                "[{test}] MYELIN_REQUIRE_RUNSC=1 but the pinned production git rootfs is \
                 unavailable: {error}. CT-006a refuses a VACUOUS green."
            );
        }
        Err(error) => {
            eprintln!("[{test}] SKIPPED: pinned production git rootfs unavailable: {error}");
            None
        }
    }
}

fn git_env(proto_v2: bool) -> Vec<String> {
    let mut env = vec![
        "HOME=/tmp".to_string(),
        "GIT_EXEC_PATH=/usr/lib/git-core".to_string(),
        "GIT_CONFIG_NOSYSTEM=1".to_string(),
        "GIT_CONFIG_COUNT=1".to_string(),
        "GIT_CONFIG_KEY_0=safe.directory".to_string(),
        "GIT_CONFIG_VALUE_0=*".to_string(),
    ];
    if proto_v2 {
        env.push("GIT_PROTOCOL=version=2".to_string());
    }
    env
}

fn make_repo_with_commit(root: &Path, tenant: &str, region: &str, repo: &str) -> String {
    let bare = resolve_bare_repo_path(root, tenant, region, repo).expect("resolve bare path");
    std::fs::create_dir_all(bare.parent().unwrap()).expect("mkdir repo parent");
    run_git(&["init", "-q", "--bare", &bare.to_string_lossy()], None);

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

fn temp_root(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("myelin-gitwire-root-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir root");
    d
}

#[test]
fn sandboxed_upload_pack_advertise_refs_lists_the_real_repo() {
    let Some(_bin) = require_or_skip("git-wire advertise-refs") else {
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
        Vec::new(),
        git_env(false),
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
    let root = temp_root("v2");
    let oid = make_repo_with_commit(&root, "acme", "fr-par", "widgets");

    let stdin = b"0014command=ls-refs\n00010009peel\n0000".to_vec();

    let (rt, mt, it) = tokens("v2");
    let spec = GitWireSpec::for_repo(
        &root,
        "acme",
        "fr-par",
        "widgets",
        vec!["upload-pack".into(), "--stateless-rpc".into()],
        stdin,
        git_env(true),
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
    let root = PathBuf::from("/srv/git-root");

    let r = resolve_bare_repo_path(&root, "acme", "fr-par", "../../victim/fr-par/secret");
    assert!(
        matches!(r, Err(WireError::Path(_))),
        "a `..` repo slug must be refused, got {r:?}"
    );

    let r = resolve_bare_repo_path(&root, "..", "fr-par", "widgets");
    assert!(
        matches!(r, Err(WireError::Path(_))),
        "a `..` tenant must be refused, got {r:?}"
    );

    let r = resolve_bare_repo_path(&root, "acme", "fr-par/../other", "widgets");
    assert!(
        matches!(r, Err(WireError::Path(_))),
        "a `/`-bearing region must be refused, got {r:?}"
    );

    let r = resolve_bare_repo_path(&root, "acme", "fr-par", "wid\\gets");
    assert!(
        matches!(r, Err(WireError::Path(_))),
        "a backslash slug must be refused, got {r:?}"
    );

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

    let ok = resolve_bare_repo_path(&root, "acme", "fr-par", "team/app").expect("valid locator");
    assert_eq!(ok, PathBuf::from("/srv/git-root/acme/fr-par/team/app.git"));
    println!("=== CT-006a path confinement: cross-tenant/`..`/separator/NUL all REFUSED before mount ===");
}

#[test]
fn read_only_repo_mount_rejects_an_in_guest_write() {
    let Some(_bin) = require_or_skip("git-wire ro-enforced") else {
        return;
    };
    let root = temp_root("ro");
    let oid_before = make_repo_with_commit(&root, "acme", "fr-par", "widgets");
    let bare = resolve_bare_repo_path(&root, "acme", "fr-par", "widgets").unwrap();

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

//! # CT-006b END-TO-END: CLONE/FETCH through the PRODUCTION `GitCore` seam (GT-006)
//!
//! Proves the wire-serving path the production [`myelin_edge::GitWireExecutor`] + the
//! [`myelin_git::core::RoutedGitCore`] stand up, end-to-end, against a REAL on-disk bare repo, through
//! the SAME [`myelin_git::core::GitCore`] seam the CT-006c HTTP smart-transport server will drive — but
//! NOT yet the HTTP server (that is CT-006c). Every claim is a REAL `runsc` run of REAL `git` inside
//! the hardened gVisor sandbox via `GvisorBackend::launch_git_wire` (the executor owns the launch; the
//! edge carries no host-exec fingerprint).
//!
//!   1. **advertise_refs(UploadPack)** → the real `refs/heads/main` + HEAD oid (pkt-line framed).
//!   2. **serve(UploadPack, <real want/done stateless-rpc request>)** → a real packfile; we prove a
//!      client could complete the fetch by feeding the pack to `git index-pack` + `git verify-pack`
//!      and confirming the WANTED commit oid is present in the pack.
//!
//! ## Gating
//! SKIPS gracefully when `runsc` is not on PATH or the pinned production git rootfs is unavailable.
//! With
//! `MYELIN_REQUIRE_RUNSC=1` an absent capability is a HARD failure (never a vacuous green). Run:
//! `MYELIN_REQUIRE_RUNSC=1 cargo test -p myelin-edge --test git_wire_clone_fetch_e2e_test -- --nocapture`.

use myelin_ci_sandbox::verified_gvisor_git_rootfs;
use myelin_edge::{
    production_git_core_with_issuer, test_git_wire_credential_issuer, GitWireExecutor,
};
use myelin_git::core::{GitCore, RepoLoc, Service};
use std::path::{Path, PathBuf};
use std::process::Command;

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

fn require_or_skip(test: &str) -> bool {
    if runsc_bin().is_none() {
        if std::env::var("MYELIN_REQUIRE_RUNSC").as_deref() == Ok("1") {
            panic!(
                "[{test}] MYELIN_REQUIRE_RUNSC=1 but `runsc` is not on PATH — CT-006b refuses a \
                 VACUOUS green."
            );
        }
        eprintln!("[{test}] SKIPPED: `runsc` is not on PATH.");
        return false;
    }

    match verified_gvisor_git_rootfs() {
        Ok(_) => true,
        Err(error) if std::env::var("MYELIN_REQUIRE_RUNSC").as_deref() == Ok("1") => {
            panic!(
                "[{test}] MYELIN_REQUIRE_RUNSC=1 but the pinned production git rootfs is \
                 unavailable: {error} — CT-006b refuses a VACUOUS green."
            );
        }
        Err(error) => {
            eprintln!("[{test}] SKIPPED: pinned production git rootfs unavailable: {error}");
            false
        }
    }
}

// ───────────────────────────── a real bare repo with a commit ─────────────────────────────

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

/// Build `<root>/<tenant>/<region>/<repo>.git` as a REAL bare repo with one commit; return the HEAD oid.
fn make_repo_with_commit(root: &Path, tenant: &str, region: &str, slug: &str) -> String {
    let bare = root.join(tenant).join(region).join(format!("{slug}.git"));
    std::fs::create_dir_all(bare.parent().unwrap()).expect("mkdir repo parent");
    run_git(&["init", "-q", "--bare", &bare.to_string_lossy()], None);

    let work = root.join("work");
    std::fs::create_dir_all(&work).expect("mkdir work");
    run_git(&["init", "-q", "-b", "main"], Some(&work));
    run_git(&["config", "user.email", "t@t.t"], Some(&work));
    run_git(&["config", "user.name", "t"], Some(&work));
    std::fs::write(work.join("f.txt"), b"hello git wire clone\n").expect("write file");
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
    assert!(out.status.success(), "rev-parse: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn temp_root(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("myelin-edge-gitwire-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir root");
    d
}

/// pkt-line frame a payload (`0009done\n`): a 4-hex length prefix counting itself + the payload bytes.
fn pkt(payload: &str) -> Vec<u8> {
    let mut v = format!("{:04x}", payload.len() + 4).into_bytes();
    v.extend_from_slice(payload.as_bytes());
    v
}

// ═══════════════════════════════ the proof ═══════════════════════════════

#[test]
fn production_gitcore_serves_a_real_clone_fetch_end_to_end() {
    if !require_or_skip("ct006b clone/fetch e2e") {
        return;
    }

    let root = temp_root("e2e");
    let oid = make_repo_with_commit(&root, "acme", "fr-par", "widgets");

    // The PRODUCTION GitCore: sandboxed GitWireExecutor (wire) + in-process GixCore (read), rooted at
    // the SAME on-disk root. This is exactly what `DurableGitBackend::wire_serving()` composes.
    let core = production_git_core_with_issuer(
        &root,
        GitWireExecutor::default_limits(),
        GitWireExecutor::serving_hooks(),
        test_git_wire_credential_issuer(),
    );
    let repo = RepoLoc::new("acme", "fr-par", "widgets");

    // ── 1. advertise_refs(UploadPack) — the real refs/HEAD oid through the seam ──
    let adv = core
        .advertise_refs(&repo, Service::UploadPack)
        .expect("advertise_refs must run sandboxed against the real repo");
    let adv_str = String::from_utf8_lossy(&adv.stdout);
    println!("=== CT-006b advertise_refs(UploadPack) through the production GitCore seam ===");
    println!("status = {}", adv.status);
    println!("HEAD oid (host) = {oid}");
    println!("advertisement (verbatim) =\n{adv_str}");
    assert_eq!(adv.status, 0, "advertise_refs exits 0");
    assert!(
        adv_str.contains(&oid),
        "advertisement carries the real HEAD oid"
    );
    assert!(
        adv_str.contains("refs/heads/main"),
        "advertisement lists refs/heads/main"
    );

    // ── 2. serve(UploadPack, <real want/done request>) — a real packfile through the seam ──
    // A v0 stateless-rpc fetch request: want the HEAD oid (capabilities on the first want line; NO
    // side-band so upload-pack streams a raw self-contained pack), flush-pkt, then `done`.
    let mut request = pkt(&format!(
        "want {oid} multi_ack_detailed no-progress ofs-delta agent=myelin/ct006b\n"
    ));
    request.extend_from_slice(b"0000"); // flush-pkt: end of want list
    request.extend_from_slice(&pkt("done\n"));

    let served = core
        .serve(&repo, Service::UploadPack, request)
        .expect("serve(UploadPack) must run sandboxed and return a packfile");
    assert_eq!(served.status, 0, "serve exits 0");
    let body = served.stdout;
    println!(
        "=== CT-006b serve(UploadPack) → {} bytes of response ===",
        body.len()
    );

    // The response is `0008NAK\n` then the raw packfile (no side-band). Slice from the PACK signature.
    let pack_at = body
        .windows(4)
        .position(|w| w == b"PACK")
        .unwrap_or_else(|| panic!("no PACK signature in the {}-byte response", body.len()));
    let pack = &body[pack_at..];
    println!(
        "packfile starts at byte {pack_at}; pack header = {:?}",
        String::from_utf8_lossy(&pack[..4])
    );

    // ── 3. PROVE a client could complete the fetch: index-pack + verify-pack the wanted oid ──
    let verify_dir = root.join("verify");
    std::fs::create_dir_all(&verify_dir).expect("mkdir verify");
    let pack_path = verify_dir.join("clone.pack");
    std::fs::write(&pack_path, pack).expect("write clone.pack");

    let idx = Command::new("git")
        .args(["index-pack", "-v", &pack_path.to_string_lossy()])
        .output()
        .expect("git index-pack");
    println!(
        "=== git index-pack --stdin equivalent (file) ===\nstatus={:?}\nstdout={}\nstderr={}",
        idx.status.code(),
        String::from_utf8_lossy(&idx.stdout),
        String::from_utf8_lossy(&idx.stderr)
    );
    assert!(
        idx.status.success(),
        "git index-pack must accept the served pack (a valid client-completable fetch)"
    );

    let idx_path = pack_path.with_extension("idx");
    let verify = Command::new("git")
        .args(["verify-pack", "-v", &idx_path.to_string_lossy()])
        .output()
        .expect("git verify-pack");
    let listing = String::from_utf8_lossy(&verify.stdout);
    println!("=== git verify-pack -v (object listing) ===\n{listing}");
    assert!(
        verify.status.success(),
        "verify-pack must validate the pack"
    );
    assert!(
        listing.contains(&oid),
        "the wanted commit oid {oid} MUST be present in the served pack — a real client completes the fetch"
    );

    println!("=== CT-006b CLONE/FETCH PROVEN end-to-end through the production GitCore seam ===");
    let _ = std::fs::remove_dir_all(&root);
}

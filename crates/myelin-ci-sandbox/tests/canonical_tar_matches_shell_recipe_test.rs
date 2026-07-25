//! # Pure-Rust canonical-tree hasher vs. the real shell recipe (CT-007 gate 2/4, registry slice)
//!
//! `crates/myelin-ci-sandbox/src/canonical_tar.rs` reimplements, in pure Rust, the exact byte
//! stream `tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner --format=gnu -C <dir> -cf
//! - . | sha256sum` produces — so [`GvisorAssetRegistry::from_bindings`](
//! myelin_ci_sandbox::asset_registry::GvisorAssetRegistry::from_bindings) never has to shell out
//! to a host `tar` process from the trusted launch path (the `no-host-exec` architecture lint
//! forbids exactly that in production code). These tests are the ones ALLOWED to shell out — they
//! exist purely to prove the pure-Rust digest matches the real shell recipe; this whole file lives
//! under `tests/`, which `crates/myelin-lints/tests/workspace_clean.rs`'s live `no-host-exec` gate
//! already excludes (`**/tests/**` — test fixtures/comparison code, not production platform code).

use myelin_ci_sandbox::canonical_tree_sha256_hex;
use myelin_ci_sandbox::{LINUX_RUST_V1_ROOTFS_SHA256, LINUX_SMALL_V1_ROOTFS_SHA256};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

/// Recompute a directory's canonical-tree digest by ACTUALLY shelling out to the host `tar` +
/// `sha256sum` — the exact recipe `crates/myelin-lints/tests/runner_asset_digest_pin.rs` and
/// `scripts/dogfood.sh`'s `verify_ci_rootfs()` already use. Skips (returns `None`) if `tar` /
/// `sha256sum` aren't on PATH, matching this repo's graceful host-tool skip convention.
fn shell_recipe_digest(dir: &Path) -> Option<String> {
    let mut tar = Command::new("tar")
        .args([
            "--sort=name",
            "--mtime=@0",
            "--owner=0",
            "--group=0",
            "--numeric-owner",
            "--format=gnu",
            "-C",
        ])
        .arg(dir)
        .args(["-cf", "-", "."])
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    let tar_stdout = tar.stdout.take()?;
    let sha = Command::new("sha256sum")
        .stdin(Stdio::from(tar_stdout))
        .output()
        .ok()?;
    let tar_status = tar.wait().ok()?;
    if !tar_status.success() || !sha.status.success() {
        return None;
    }
    String::from_utf8_lossy(&sha.stdout)
        .split_whitespace()
        .next()
        .map(|s| s.to_string())
}

fn unique_temp_dir(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "myelin-canonical-tar-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

/// Build a synthetic tree exercising nested dirs, a regular file, a symlink, AND a hardlink pair
/// (the same shapes the two real staged runner assets contain), plus a path long enough to force
/// the GNU long-name (`L`) extension, a symlink whose TARGET is long enough to force the GNU
/// long-link (`K`) extension (the long-NAME path alone does not exercise this — a symlink's target
/// is a SEPARATE header field from its own name), and a same-parent
/// file-whose-name-is-a-prefix-of-a-directory-name case (the `etc/ca-certificates.conf` vs
/// `etc/ca-certificates/` shape that broke a naive flat-sort — a plain lexicographic sort of NAMES
/// puts `ca-certificates.conf` before `ca-certificates/` while `tar --sort=name`'s real ordering
/// does not treat the trailing `/` as absent, so a hasher that flattens paths without preserving
/// that distinction can silently disagree with the real recipe on trees containing this shape) —
/// permanently in the SYNTHETIC fixture (not just relying on the large real Rust asset happening to
/// contain it), so this regression is caught even on a host without that asset staged. Asserts the
/// pure-Rust digest matches the real `tar` recipe's own digest over the SAME tree, byte for byte.
/// SKIPPED (not failed) if `tar`/`sha256sum` are absent.
#[test]
fn matches_real_tar_recipe_over_a_synthetic_tree() {
    let dir = unique_temp_dir("selftest");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("sub")).unwrap();
    fs::write(dir.join("a.txt"), b"hello").unwrap();
    fs::write(dir.join("sub/file.txt"), b"world").unwrap();
    std::os::unix::fs::symlink("file.txt", dir.join("sub/link.txt")).unwrap();
    fs::hard_link(dir.join("a.txt"), dir.join("sub/hard.txt")).unwrap();
    // A name well past the 100-byte GNU header field, forcing the `././@LongLink` NAME extension.
    let long_name = "x".repeat(150);
    fs::create_dir_all(dir.join(&long_name)).unwrap();
    fs::write(dir.join(&long_name).join("f.txt"), b"deep").unwrap();
    // A symlink whose TARGET (not name) exceeds the 100-byte GNU header field, forcing the GNU `K`
    // long-link path — distinct from, and not exercised by, the long-NAME `L` case above.
    let long_target = format!("{}/payload.txt", "y".repeat(150));
    fs::create_dir_all(dir.join("longlink-target-dir").join("y".repeat(150))).unwrap();
    fs::write(
        dir.join("longlink-target-dir")
            .join("y".repeat(150))
            .join("payload.txt"),
        b"pointed-to by a long-target symlink",
    )
    .unwrap();
    std::os::unix::fs::symlink(&long_target, dir.join("longlink-target-dir/link-with-long-target"))
        .unwrap();
    // The same-parent file-name-is-a-prefix-of-a-directory-name shape (`ca-certificates.conf` vs
    // `ca-certificates/`) that broke a naive flat-sort.
    fs::create_dir_all(dir.join("etc").join("ca-certificates")).unwrap();
    fs::write(
        dir.join("etc").join("ca-certificates").join("cert.pem"),
        b"fake cert",
    )
    .unwrap();
    fs::write(
        dir.join("etc").join("ca-certificates.conf"),
        b"fake ca-certificates config",
    )
    .unwrap();

    let Some(expected) = shell_recipe_digest(&dir) else {
        eprintln!(
            "matches_real_tar_recipe_over_a_synthetic_tree: SKIPPED — `tar`/`sha256sum` not on PATH \
             on this host"
        );
        let _ = fs::remove_dir_all(&dir);
        return;
    };

    let actual = canonical_tree_sha256_hex(&dir).expect("hash the synthetic tree");
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(
        actual, expected,
        "pure-Rust canonical-tree digest must byte-match the shell tar|sha256sum recipe"
    );
}

#[test]
fn empty_directory_matches_shell_recipe() {
    let dir = unique_temp_dir("empty");
    fs::create_dir_all(&dir).unwrap();
    let actual = canonical_tree_sha256_hex(&dir).unwrap();
    if let Some(expected) = shell_recipe_digest(&dir) {
        assert_eq!(actual, expected);
    } else {
        eprintln!("empty_directory_matches_shell_recipe: SKIPPED — `tar`/`sha256sum` not on PATH");
    }
    let _ = fs::remove_dir_all(&dir);
}

/// **The RED-first proof this whole slice is built on.** Compute the pure-Rust digest of the REAL
/// staged `linux-rust-v1` asset (resolved the SAME way `runner-assets.toml`'s row resolves it: env
/// var `MYELIN_GVISOR_RUST_ROOTFS`, else the documented default path
/// `~/.local/share/gvisor-assets/rust-rootfs`) and assert it equals the digest ALREADY committed in
/// `runner-assets.toml` (`6feada1e0ef7b739d71c7f198b03dcaab494f35ea86182dd887d23f5df0c6083`).
/// Genuinely, honestly SKIPPED if the asset isn't staged on this machine (this repo's existing
/// runsc/KVM graceful-skip convention) — never a vacuous pass. `MYELIN_REQUIRE_RUST_ROOTFS_PIN=1`
/// hard-requires it (this is exercised for REAL on the founder dogfood host this was written on).
#[test]
fn pure_rust_digest_matches_the_committed_linux_rust_v1_pin() {
    let rootfs = myelin_ci_sandbox::resolved_gvisor_rust_rootfs();
    if !rootfs.is_dir() {
        if std::env::var("MYELIN_REQUIRE_RUST_ROOTFS_PIN").as_deref() == Ok("1") {
            panic!(
                "MYELIN_REQUIRE_RUST_ROOTFS_PIN=1 but the staged linux-rust-v1 rootfs ({}) is \
                 absent. Stage it with ./scripts/build-rust-rootfs.sh first.",
                rootfs.display()
            );
        }
        eprintln!(
            "pure_rust_digest_matches_the_committed_linux_rust_v1_pin: SKIPPED — the staged \
             linux-rust-v1 rootfs ({}) is absent on this machine.",
            rootfs.display()
        );
        return;
    }
    let canon = fs::canonicalize(&rootfs).expect("canonicalize the staged rust rootfs");
    let actual =
        canonical_tree_sha256_hex(&canon).expect("hash the real staged linux-rust-v1 rootfs");
    assert_eq!(
        actual, LINUX_RUST_V1_ROOTFS_SHA256,
        "the pure-Rust canonical-tree hasher must reproduce EXACTLY the digest already committed \
         in runner-assets.toml for linux-rust-v1 — a mismatch here means the byte-stream \
         construction is wrong and nothing downstream (the asset registry) can be trusted"
    );
    // Cross-check against the real shell recipe too, on this same real (large — >800 MiB) asset, to
    // prove the streaming implementation matches beyond just the small synthetic-tree cases above.
    if let Some(shell) = shell_recipe_digest(&canon) {
        assert_eq!(
            actual, shell,
            "must also match the real tar|sha256sum recipe over this asset"
        );
    }
}

/// Same proof for the base `linux-small-v1` asset that ALREADY powers the real founder-dogfood
/// pipeline (`.myelin/ci.toml`'s pinned `myelin.local/linux-small-v1-rootfs@sha256:f9bd3926...`) —
/// the asset the production registry (`myelin-ci-controlplane`'s `runner_bind.rs`) binds first.
#[test]
fn pure_rust_digest_matches_the_committed_linux_small_v1_pin() {
    let rootfs = myelin_ci_sandbox::resolved_gvisor_rootfs();
    if !rootfs.is_dir() {
        eprintln!(
            "pure_rust_digest_matches_the_committed_linux_small_v1_pin: SKIPPED — the staged base \
             rootfs ({}) is absent on this machine.",
            rootfs.display()
        );
        return;
    }
    let canon = fs::canonicalize(&rootfs).expect("canonicalize the staged base rootfs");
    let actual = canonical_tree_sha256_hex(&canon).expect("hash the real staged base rootfs");
    assert_eq!(
        actual, LINUX_SMALL_V1_ROOTFS_SHA256,
        "must reproduce the digest `.myelin/ci.toml` already pins for the founder pipeline's \
         linux-small-v1 rootfs"
    );
}

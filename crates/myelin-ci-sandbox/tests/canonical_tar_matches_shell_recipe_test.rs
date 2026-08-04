use myelin_ci_sandbox::canonical_tree_sha256_hex;
use myelin_ci_sandbox::{LINUX_RUST_V1_ROOTFS_SHA256, LINUX_SMALL_V1_ROOTFS_SHA256};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

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

#[test]
fn matches_real_tar_recipe_over_a_synthetic_tree() {
    let dir = unique_temp_dir("selftest");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("sub")).unwrap();
    fs::write(dir.join("a.txt"), b"hello").unwrap();
    fs::write(dir.join("sub/file.txt"), b"world").unwrap();
    std::os::unix::fs::symlink("file.txt", dir.join("sub/link.txt")).unwrap();
    fs::hard_link(dir.join("a.txt"), dir.join("sub/hard.txt")).unwrap();
    let long_name = "x".repeat(150);
    fs::create_dir_all(dir.join(&long_name)).unwrap();
    fs::write(dir.join(&long_name).join("f.txt"), b"deep").unwrap();
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
            "matches_real_tar_recipe_over_a_synthetic_tree: SKIPPED - `tar`/`sha256sum` not on PATH \
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
        eprintln!("empty_directory_matches_shell_recipe: SKIPPED - `tar`/`sha256sum` not on PATH");
    }
    let _ = fs::remove_dir_all(&dir);
}

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
            "pure_rust_digest_matches_the_committed_linux_rust_v1_pin: SKIPPED - the staged \
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
         in runner-assets.toml for linux-rust-v1 - a mismatch here means the byte-stream \
         construction is wrong and nothing downstream (the asset registry) can be trusted"
    );
    if let Some(shell) = shell_recipe_digest(&canon) {
        assert_eq!(
            actual, shell,
            "must also match the real tar|sha256sum recipe over this asset"
        );
    }
}

#[test]
fn pure_rust_digest_matches_the_committed_linux_small_v1_pin() {
    let rootfs = myelin_ci_sandbox::resolved_gvisor_rootfs();
    if !rootfs.is_dir() {
        eprintln!(
            "pure_rust_digest_matches_the_committed_linux_small_v1_pin: SKIPPED - the staged base \
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

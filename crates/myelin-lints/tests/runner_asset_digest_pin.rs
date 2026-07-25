//! CT-007 pre-registered cutover-floor GATE 2/4 (digest-pinned runner assets) — MECHANICAL
//! enforcement for the committed `runner-assets.toml` manifest.
//!
//! `planning/system-reviews/2026-06-26/12-ci-track-ledger.md` ("Pre-registered CT-007 cutover
//! floor", ~line 311, gate 2/4): "digest-pinned one-cell runner assets provide the actual
//! Rust/Node/browser/container capabilities [the 12 GitHub CI jobs] require without weakening
//! gVisor, egress, or privilege boundaries." `scripts/dogfood.sh`'s `verify_ci_rootfs()` is only a
//! MANUAL operator script (run by hand, over the `.myelin/ci.toml` founder pipeline's ONE rootfs);
//! this test is the mechanical, committed-CI-level equivalent for EVERY row of
//! `runner-assets.toml` (a distinct manifest — asset id → digest, not GitHub job → owner).
//!
//! For each `[[asset]]` row: if the staged directory it names (`env_var`, falling back to
//! `default_path`) exists on the CURRENT machine, recompute its canonical-tree sha256 with the
//! EXACT SAME recipe `verify_ci_rootfs()` uses (`tar --sort=name --mtime=@0 --owner=0 --group=0
//! --numeric-owner --format=gnu -C <dir> -cf - . | sha256sum`) and assert it matches the
//! committed `canonical_tree_sha256` pin — fail loudly with a clear mismatch message if not. If
//! the directory is ABSENT (any dev machine or CI runner without this asset staged), this is a
//! GENUINE, HONEST skip (printed, not silent) — matching this repo's existing runsc/KVM
//! graceful-skip convention. `MYELIN_REQUIRE_RUST_ROOTFS_PIN=1` forces a hard failure instead of a
//! skip, for hosts (like the founder dogfood host) where the asset is expected to be staged.

use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Deserialize)]
struct Manifest {
    asset: Vec<AssetRow>,
}

#[derive(Debug, Deserialize)]
struct AssetRow {
    id: String,
    #[allow(dead_code)]
    capability: String,
    #[allow(dead_code)]
    covers_job: String,
    #[allow(dead_code)]
    stage_script: String,
    env_var: String,
    default_path: String,
    #[allow(dead_code)]
    source_image: String,
    #[allow(dead_code)]
    source_image_digest: String,
    canonical_tree_sha256: String,
    #[allow(dead_code)]
    note: String,
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("myelin-lints lives under <workspace>/crates")
        .to_path_buf()
}

fn load_real_manifest() -> Manifest {
    let source = fs::read_to_string(workspace_root().join("runner-assets.toml"))
        .expect("read runner-assets.toml");
    toml::from_str(&source).expect("parse runner-assets.toml")
}

/// Resolve an asset row's staged directory: its `env_var` if set, else its documented
/// `default_path` (expanding a leading `~` against `$HOME`, since these paths are always
/// `~/.local/share/gvisor-assets/...` in this manifest).
fn resolve_staged_dir(row: &AssetRow) -> PathBuf {
    if let Ok(value) = std::env::var(&row.env_var) {
        return PathBuf::from(value);
    }
    if let Some(rest) = row.default_path.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(&row.default_path)
}

/// Recompute the canonical-tree sha256 of `dir` with the EXACT recipe
/// `scripts/dogfood.sh`'s `verify_ci_rootfs()` uses, so a mismatch here is provably the same
/// notion of "digest" an operator would see running that script by hand.
fn canonical_tree_sha256(dir: &Path) -> String {
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
        .expect("spawn tar");
    let tar_stdout = tar.stdout.take().expect("tar stdout piped");

    let sha = Command::new("sha256sum")
        .stdin(Stdio::from(tar_stdout))
        .output()
        .expect("run sha256sum");
    let tar_status = tar.wait().expect("wait on tar");
    assert!(tar_status.success(), "tar failed over {dir:?}");
    assert!(sha.status.success(), "sha256sum failed over {dir:?}");
    let stdout = String::from_utf8_lossy(&sha.stdout);
    stdout
        .split_whitespace()
        .next()
        .expect("sha256sum prints a hex digest")
        .to_string()
}

/// HARD-FAIL on an absent staged asset iff `MYELIN_REQUIRE_RUST_ROOTFS_PIN=1` (this gate refuses
/// a vacuous green on a host expected to carry the asset); otherwise GRACEFUL, HONEST SKIP.
fn require_or_skip(row: &AssetRow, dir: &Path) -> bool {
    if dir.is_dir() {
        return true;
    }
    if std::env::var("MYELIN_REQUIRE_RUST_ROOTFS_PIN").as_deref() == Ok("1") {
        panic!(
            "runner-asset `{}`: MYELIN_REQUIRE_RUST_ROOTFS_PIN=1 but the staged directory {} is \
             absent. Stage it with `{}` first.",
            row.id,
            dir.display(),
            row.stage_script
        );
    }
    eprintln!(
        "runner-asset `{}`: SKIPPED — staged directory {} is absent on this machine (this asset \
         is not staged here; MYELIN_REQUIRE_RUST_ROOTFS_PIN=1 would hard-fail instead of skip).",
        row.id,
        dir.display()
    );
    false
}

#[test]
fn staged_runner_assets_match_their_committed_digest_pin() {
    let manifest = load_real_manifest();
    assert!(
        !manifest.asset.is_empty(),
        "runner-assets.toml must contain at least one [[asset]] row"
    );

    let mut checked_any = false;
    for row in &manifest.asset {
        assert_eq!(
            row.canonical_tree_sha256.len(),
            64,
            "runner-asset `{}`: canonical_tree_sha256 must be a 64-hex-char sha256 digest, got `{}`",
            row.id,
            row.canonical_tree_sha256
        );
        assert!(
            row.canonical_tree_sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "runner-asset `{}`: canonical_tree_sha256 must be hex, got `{}`",
            row.id,
            row.canonical_tree_sha256
        );

        let dir = resolve_staged_dir(row);
        if !require_or_skip(row, &dir) {
            continue;
        }
        checked_any = true;

        let actual = canonical_tree_sha256(&dir);
        assert_eq!(
            actual, row.canonical_tree_sha256,
            "runner-asset `{}` staged at {} has DRIFTED from its committed pin in \
             runner-assets.toml: expected sha256:{}, computed sha256:{}. Either the staged tree \
             was mutated/rebuilt without updating the manifest, or the manifest is stale — re-run \
             `{}` and update `canonical_tree_sha256` if the drift is intentional.",
            row.id,
            dir.display(),
            row.canonical_tree_sha256,
            actual,
            row.stage_script
        );
    }

    if std::env::var("MYELIN_REQUIRE_RUST_ROOTFS_PIN").as_deref() == Ok("1") {
        assert!(
            checked_any,
            "MYELIN_REQUIRE_RUST_ROOTFS_PIN=1 but no runner-asset row's staged directory was \
             present — require_or_skip should have already hard-panicked; this is a bug in the \
             skip-detection above if reached"
        );
    }
}

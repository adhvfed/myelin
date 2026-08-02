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
//! Two checks, in order:
//! 1. **Manifest-internal consistency (unconditional — no staged directory required):** every row's
//!    `image` field (the `ImageRef` reference `GvisorAssetRegistry` would register it under) must be
//!    digest-pinned with the SAME sha256 hex digest as that row's own `canonical_tree_sha256` — a
//!    row whose `image` digest and `canonical_tree_sha256` disagree could never actually resolve
//!    through the registry (CT-007 gate 2/4's `GvisorAssetRegistry::from_bindings` would refuse
//!    it), so this is a real, useful check even on a host with the asset absent.
//! 2. **Staged-content match (conditional on the asset being staged):** if the staged directory the
//!    row names (`env_var`, falling back to `default_path`) exists on the CURRENT machine, recompute
//!    its canonical-tree sha256 with the pure-Rust hasher
//!    [`myelin_ci_sandbox::canonical_tree_sha256_hex`] — the SAME hasher
//!    `GvisorAssetRegistry::from_bindings` uses on the real production launch path, reproducing
//!    EXACTLY the same bytes the `tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner
//!    --format=gnu -C <dir> -cf - . | sha256sum` shell recipe `verify_ci_rootfs()` uses would produce
//!    (proven byte-for-byte in `crates/myelin-ci-sandbox/tests/canonical_tar_matches_shell_recipe_test.rs`)
//!    — and assert it matches the committed `canonical_tree_sha256` pin. If the directory is ABSENT
//!    (any dev machine or CI runner without this asset staged), this is a GENUINE, HONEST skip
//!    (printed, not silent) — matching this repo's existing runsc/KVM graceful-skip convention.
//!    `MYELIN_REQUIRE_RUST_ROOTFS_PIN=1` forces a hard failure instead of a skip, for hosts (like the
//!    founder dogfood host) where the asset is expected to be staged.
//!
//! This test does NOT shell out to a host `tar`/`sha256sum` process (a prior version did) — it calls
//! the pure-Rust `myelin-ci-sandbox` hasher as a dev-dependency instead, exercising the SAME code
//! path the production asset registry uses rather than a parallel shell-based reimplementation that
//! could silently drift from it.

use myelin_ci_sandbox::{canonical_tree_sha256_hex, file_sha256_hex};
use myelin_ci_sandbox::{
    CARGO_VENDOR_SMOKE_LOCK_SHA256, CARGO_VENDOR_SMOKE_TREE_SHA256, GVISOR_GIT_ROOTFS_SHA256,
    LINUX_RUST_V1_ROOTFS_SHA256, LINUX_SMALL_V1_ROOTFS_SHA256,
};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct Manifest {
    asset: Vec<AssetRow>,
}

#[derive(Debug, Deserialize)]
struct AssetRow {
    id: String,
    capability: String,
    covers_job: String,
    stage_script: String,
    env_var: String,
    default_path: String,
    source_image: String,
    source_image_digest: String,
    canonical_tree_sha256: String,
    image: String,
    #[serde(default)]
    lockfile_sha256: Option<String>,
    #[serde(default)]
    mount_destination: Option<String>,
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

/// A minimal view of `.myelin/ci.toml` — just enough to read the ONE founder-dogfood job's `image`
/// field, whose embedded digest must match `LINUX_SMALL_V1_ROOTFS_SHA256`.
#[derive(Debug, Deserialize)]
struct CiToml {
    jobs: Vec<CiJobRow>,
}

#[derive(Debug, Deserialize)]
struct CiJobRow {
    #[allow(dead_code)]
    name: String,
    image: String,
}

fn load_real_ci_toml() -> CiToml {
    let source =
        fs::read_to_string(workspace_root().join(".myelin/ci.toml")).expect("read .myelin/ci.toml");
    toml::from_str(&source).expect("parse .myelin/ci.toml")
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

/// Parse the sha256 hex digest out of an `@sha256:<hex>`-pinned `image` reference string. Returns
/// `None` (never a partial/garbage match) if the reference isn't pinned that way — the caller turns
/// that into a loud, specific assertion failure rather than a confusing downstream one.
fn parse_sha256_digest(image: &str) -> Option<&str> {
    let (_, after_at) = image.rsplit_once('@')?;
    let (algo, digest) = after_at.split_once(':')?;
    if algo != "sha256" {
        return None;
    }
    (digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit())).then_some(digest)
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
fn every_row_image_digest_matches_its_own_canonical_tree_sha256() {
    let manifest = load_real_manifest();
    assert!(
        !manifest.asset.is_empty(),
        "runner-assets.toml must contain at least one [[asset]] row"
    );
    for row in &manifest.asset {
        let parsed = parse_sha256_digest(&row.image).unwrap_or_else(|| {
            panic!(
                "runner-asset `{}`: `image` (`{}`) must be pinned as `...@sha256:<64-hex>` — a \
                 GvisorAssetRegistry entry can only ever resolve a sha256-pinned reference today",
                row.id, row.image
            )
        });
        assert_eq!(
            parsed, row.canonical_tree_sha256,
            "runner-asset `{}`: the digest embedded in `image` (`{}`) must equal this row's own \
             `canonical_tree_sha256` (`{}`) — a registry entry built from this row's `image` and \
             rootfs path could never verify at from_bindings() construction time otherwise (the digest a real \
             registry would check the staged directory against would never be the digest the \
             directory is actually pinned to)",
            row.id, row.image, row.canonical_tree_sha256
        );
    }
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
            row.canonical_tree_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit()),
            "runner-asset `{}`: canonical_tree_sha256 must be hex, got `{}`",
            row.id,
            row.canonical_tree_sha256
        );

        let dir = resolve_staged_dir(row);
        if !require_or_skip(row, &dir) {
            continue;
        }
        checked_any = true;

        let actual = canonical_tree_sha256_hex(&dir).unwrap_or_else(|e| {
            panic!(
                "runner-asset `{}`: failed to hash staged directory {}: {e}",
                row.id,
                dir.display()
            )
        });
        assert_eq!(
            actual,
            row.canonical_tree_sha256,
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

/// **UNCONDITIONAL (no staged directory required) — the source-file sync check.** Before this test,
/// `GVISOR_GIT_ROOTFS_SHA256`/`LINUX_RUST_V1_ROOTFS_SHA256`/
/// `LINUX_SMALL_V1_ROOTFS_SHA256` (`gvisor.rs`) duplicated
/// `runner-assets.toml`'s `canonical_tree_sha256`/`image` fields and `.myelin/ci.toml`'s `image`
/// field as separate hardcoded literals with NO mechanical link between any of them — a source-file
/// edit to one could leave every existing test green while production silently refused (or accepted
/// a wrong) newly-authored image. This asserts:
///   - `LINUX_RUST_V1_ROOTFS_SHA256` == `runner-assets.toml`'s `linux-rust-v1` row's
///     `canonical_tree_sha256` == that row's own `image` field's embedded `@sha256:` digest.
///   - `GVISOR_GIT_ROOTFS_SHA256` == the equivalent `git-v1` row values.
///   - `LINUX_SMALL_V1_ROOTFS_SHA256` == `.myelin/ci.toml`'s (single) job's `image` field's embedded
///     `@sha256:` digest.
#[test]
fn rust_and_small_rootfs_constants_are_mechanically_synced_to_their_toml_sources() {
    let manifest = load_real_manifest();
    let rust_row = manifest
        .asset
        .iter()
        .find(|row| row.id == "linux-rust-v1")
        .expect("runner-assets.toml must have a `linux-rust-v1` row");
    assert_eq!(
        LINUX_RUST_V1_ROOTFS_SHA256, rust_row.canonical_tree_sha256,
        "gvisor.rs's LINUX_RUST_V1_ROOTFS_SHA256 constant has drifted from runner-assets.toml's \
         `linux-rust-v1` row's `canonical_tree_sha256` — update whichever is stale"
    );
    let rust_embedded = parse_sha256_digest(&rust_row.image).unwrap_or_else(|| {
        panic!(
            "runner-assets.toml's `linux-rust-v1` row's `image` (`{}`) must be `...@sha256:<64-hex>`",
            rust_row.image
        )
    });
    assert_eq!(
        LINUX_RUST_V1_ROOTFS_SHA256, rust_embedded,
        "gvisor.rs's LINUX_RUST_V1_ROOTFS_SHA256 constant has drifted from the digest embedded in \
         runner-assets.toml's `linux-rust-v1` row's own `image` field"
    );

    let git_row = manifest
        .asset
        .iter()
        .find(|row| row.id == "git-v1")
        .expect("runner-assets.toml must have a `git-v1` row");
    assert_eq!(
        GVISOR_GIT_ROOTFS_SHA256, git_row.canonical_tree_sha256,
        "gvisor.rs's GVISOR_GIT_ROOTFS_SHA256 constant has drifted from runner-assets.toml's \
         `git-v1` row"
    );
    assert_eq!(
        Some(GVISOR_GIT_ROOTFS_SHA256),
        parse_sha256_digest(&git_row.image),
        "runner-assets.toml's `git-v1` image must carry the same git rootfs pin"
    );

    let ci = load_real_ci_toml();
    let job = ci
        .jobs
        .first()
        .expect(".myelin/ci.toml must have at least one [[jobs]] row");
    let small_embedded = parse_sha256_digest(&job.image).unwrap_or_else(|| {
        panic!(
            ".myelin/ci.toml's job `image` (`{}`) must be `...@sha256:<64-hex>`",
            job.image
        )
    });
    assert_eq!(
        LINUX_SMALL_V1_ROOTFS_SHA256, small_embedded,
        "gvisor.rs's LINUX_SMALL_V1_ROOTFS_SHA256 constant has drifted from the digest embedded in \
         .myelin/ci.toml's job `image` field"
    );
}

#[test]
fn cargo_vendor_manifest_row_is_mechanically_synced_to_code_and_fixture() {
    let manifest = load_real_manifest();
    let row = manifest
        .asset
        .iter()
        .find(|row| row.id == "cargo-vendor-smoke-v1")
        .expect("runner-assets.toml must have a `cargo-vendor-smoke-v1` row");

    assert_eq!(row.capability, "cargo-vendor");
    assert_eq!(row.covers_job, "build-test-clippy");
    assert_eq!(row.stage_script, "scripts/build-cargo-vendor-asset.sh");
    assert_eq!(row.env_var, "MYELIN_GVISOR_CARGO_VENDOR");
    assert_eq!(
        row.default_path,
        "~/.local/share/gvisor-assets/cargo-vendor-smoke-v1"
    );
    assert_eq!(
        row.source_image,
        "testing/fixtures/cargo-vendor-smoke/Cargo.lock"
    );
    assert_eq!(
        row.mount_destination.as_deref(),
        Some("/opt/myelin/cargo-vendor")
    );

    assert_eq!(
        row.canonical_tree_sha256, CARGO_VENDOR_SMOKE_TREE_SHA256,
        "the registry's Cargo vendor tree constant must match runner-assets.toml"
    );
    assert_eq!(
        parse_sha256_digest(&row.image),
        Some(CARGO_VENDOR_SMOKE_TREE_SHA256),
        "the Cargo vendor reference must carry the same complete-tree pin"
    );
    assert_eq!(
        row.lockfile_sha256.as_deref(),
        Some(CARGO_VENDOR_SMOKE_LOCK_SHA256),
        "the registered asset's lock key must match runner-assets.toml"
    );
    assert_eq!(
        row.source_image_digest,
        format!("sha256:{CARGO_VENDOR_SMOKE_LOCK_SHA256}"),
        "the source-image digest must name the same exact lockfile key"
    );

    let fixture_lock = workspace_root().join(&row.source_image);
    let actual_fixture_lock_sha256 = file_sha256_hex(&fixture_lock)
        .unwrap_or_else(|error| panic!("hash fixture lock {}: {error}", fixture_lock.display()));
    assert_eq!(
        actual_fixture_lock_sha256, CARGO_VENDOR_SMOKE_LOCK_SHA256,
        "the checked-in fixture Cargo.lock moved without an intentional asset rebuild/repin"
    );
}

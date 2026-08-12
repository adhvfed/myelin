use myelin_ci_sandbox::{canonical_tree_sha256_hex, file_sha256_hex};
use myelin_ci_sandbox::{
    CARGO_VENDOR_SMOKE_LOCK_SHA256, CARGO_VENDOR_SMOKE_TREE_SHA256,
    CARGO_VENDOR_WORKSPACE_LOCK_SHA256, CARGO_VENDOR_WORKSPACE_TREE_SHA256,
    GVISOR_GIT_ROOTFS_SHA256, LINUX_RUST_V1_ROOTFS_SHA256,
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

fn parse_sha256_digest(image: &str) -> Option<&str> {
    let (_, after_at) = image.rsplit_once('@')?;
    let (algo, digest) = after_at.split_once(':')?;
    if algo != "sha256" {
        return None;
    }
    (digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit())).then_some(digest)
}

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
        "runner-asset `{}`: SKIPPED - staged directory {} is absent on this machine (this asset \
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
                "runner-asset `{}`: `image` (`{}`) must be pinned as `...@sha256:<64-hex>` - a \
                 GvisorAssetRegistry entry can only ever resolve a sha256-pinned reference today",
                row.id, row.image
            )
        });
        assert_eq!(
            parsed, row.canonical_tree_sha256,
            "runner-asset `{}`: the digest embedded in `image` (`{}`) must equal this row's own \
             `canonical_tree_sha256` (`{}`) - a registry entry built from this row's `image` and \
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
             was mutated/rebuilt without updating the manifest, or the manifest is stale - re-run \
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
             present - require_or_skip should have already hard-panicked; this is a bug in the \
             skip-detection above if reached"
        );
    }
}

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
         `linux-rust-v1` row's `canonical_tree_sha256` - update whichever is stale"
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
    assert!(
        !ci.jobs.is_empty(),
        ".myelin/ci.toml must have at least one [[jobs]] row"
    );
    for job in &ci.jobs {
        let embedded = parse_sha256_digest(&job.image).unwrap_or_else(|| {
            panic!(
                ".myelin/ci.toml's job `image` (`{}`) must be `...@sha256:<64-hex>`",
                job.image
            )
        });
        assert_eq!(
            LINUX_RUST_V1_ROOTFS_SHA256, embedded,
            "gvisor.rs's LINUX_RUST_V1_ROOTFS_SHA256 constant has drifted from the digest embedded \
             in .myelin/ci.toml's `{}` job `image` field",
            job.name
        );
    }
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

#[test]
fn cargo_vendor_workspace_manifest_row_is_mechanically_synced_to_code_and_lockfile() {
    let manifest = load_real_manifest();
    let row = manifest
        .asset
        .iter()
        .find(|row| row.id == "cargo-vendor-workspace-v1")
        .expect("runner-assets.toml must have a `cargo-vendor-workspace-v1` row");

    assert_eq!(row.capability, "cargo-vendor");
    assert_eq!(row.covers_job, "build-test-clippy");
    assert_eq!(row.stage_script, "scripts/build-cargo-vendor-asset.sh");
    assert_eq!(row.env_var, "MYELIN_GVISOR_CARGO_VENDOR_WORKSPACE");
    assert_eq!(
        row.default_path,
        "~/.local/share/gvisor-assets/cargo-vendor-workspace-v1"
    );
    assert_eq!(row.source_image, "Cargo.lock");
    assert_eq!(
        row.mount_destination.as_deref(),
        Some("/opt/myelin/cargo-vendor")
    );

    assert_eq!(
        row.canonical_tree_sha256, CARGO_VENDOR_WORKSPACE_TREE_SHA256,
        "the registry's workspace Cargo vendor tree constant must match runner-assets.toml"
    );
    assert_eq!(
        parse_sha256_digest(&row.image),
        Some(CARGO_VENDOR_WORKSPACE_TREE_SHA256),
        "the workspace Cargo vendor reference must carry the same complete-tree pin"
    );
    assert_eq!(
        row.lockfile_sha256.as_deref(),
        Some(CARGO_VENDOR_WORKSPACE_LOCK_SHA256),
        "the registered workspace asset's lock key must match runner-assets.toml"
    );
    assert_eq!(
        row.source_image_digest,
        format!("sha256:{CARGO_VENDOR_WORKSPACE_LOCK_SHA256}"),
        "the source-image digest must name the same exact workspace lockfile key"
    );

    let root_lock = workspace_root().join(&row.source_image);
    let actual_root_lock_sha256 = file_sha256_hex(&root_lock)
        .unwrap_or_else(|error| panic!("hash root lock {}: {error}", root_lock.display()));
    assert_eq!(
        actual_root_lock_sha256, CARGO_VENDOR_WORKSPACE_LOCK_SHA256,
        "the workspace root Cargo.lock moved without an intentional asset rebuild/repin - the \
         vendor tree no longer matches the workspace it claims to vendor"
    );
}

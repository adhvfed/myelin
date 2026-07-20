use std::fs;
use std::path::PathBuf;

fn workspace() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("lint crate must live under workspace/crates")
        .to_path_buf()
}

#[test]
fn edge_release_bundle_is_locked_stripped_checksummed_and_uploaded() {
    let root = workspace();
    let cargo = fs::read_to_string(root.join("Cargo.toml")).expect("read workspace manifest");
    let script = fs::read_to_string(root.join("scripts/build-edge-release.sh"))
        .expect("read edge release builder");
    let workflow =
        fs::read_to_string(root.join(".github/workflows/ci.yml")).expect("read CI workflow");

    assert!(cargo.contains("[profile.release]"));
    assert!(cargo.contains("strip = \"symbols\""));
    assert!(cargo.contains("lto = \"thin\""));
    assert!(script.contains("cargo build --release --locked -p myelin-edge --bin edge"));
    assert!(script.contains("refusing to label a dirty checkout"));
    assert!(script.contains("sha256sum --check SHA256SUMS"));
    assert!(script.contains("gzip -n"));
    assert!(workflow.contains("run: scripts/build-edge-release.sh"));
    assert!(workflow.contains("path: target/release-bundles/*.tar.gz*"));
}

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn run_scanner(args: &[&str]) -> i32 {
    let bin = env!("CARGO_BIN_EXE_contract-coverage");
    let status = Command::new(bin)
        .args(args)
        .status()
        .expect("the contract-coverage binary must run");
    status
        .code()
        .expect("contract-coverage exits with a code, not a signal")
}

#[test]
fn contract_coverage_is_green_over_the_real_workspace() {
    let code = run_scanner(&[]);
    assert_eq!(
        code, 0,
        "contract-coverage must reconcile the real contract index with existing registered \
         artifacts and named deferred landings"
    );
}

#[test]
fn contract_coverage_fails_loudly_on_the_red_manifest_fixture() {
    let index = fixtures_dir().join("contract_index.fixture.md");
    let manifest = fixtures_dir().join("contract_coverage.red.fixture.toml");
    let code = run_scanner(&[
        "--index",
        index.to_str().unwrap(),
        "--manifest",
        manifest.to_str().unwrap(),
    ]);
    assert_ne!(
        code, 0,
        "contract-coverage must reject a missing registered artifact and an unnamed deferred floor"
    );
}

#[test]
fn contract_coverage_passes_on_the_green_manifest_fixture() {
    let index = fixtures_dir().join("contract_index.fixture.md");
    let manifest = fixtures_dir().join("contract_coverage.green.fixture.toml");
    let code = run_scanner(&[
        "--index",
        index.to_str().unwrap(),
        "--manifest",
        manifest.to_str().unwrap(),
    ]);
    assert_eq!(
        code, 0,
        "contract-coverage must accept existing registered artifacts and a named landing prompt"
    );
}

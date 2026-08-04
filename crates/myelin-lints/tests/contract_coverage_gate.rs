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
        "contract-coverage MUST be green over the real contract-index + manifest (0 \
         falsely-claimed rows). If this is red, ship the missing CDC pair or mark the row \
         deferred with its landing prompt - NEVER weaken the gate."
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
        "contract-coverage MUST exit non-zero on the red manifest fixture (a falsely-claimed pair \
         + an un-named deferred floor)"
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
        "contract-coverage MUST exit zero on the green manifest fixture (a real pair + a named \
         landing prompt)"
    );
}

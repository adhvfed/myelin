//! The CI-wiring gate test (EB-07 → P-019): prove the `lint-gate` binary — the thing the CI
//! `architecture-lints` job runs — FAILS LOUDLY with a NON-ZERO exit when a red fixture is
//! present, and exits ZERO on a clean (green) input. This is the EB-07 obligation: "assert the
//! workflow fails (loudly, non-zero exit) when the red fixture is present (no `|| true` swallow)".
//!
//! The gate is the process exit code itself, so it cannot be swallowed by a shell `||`. This test
//! is the dated green proof that the lint is wired into CI loud-never-swallowed.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Run the compiled `lint-gate` binary over `--no-exclude PATH` and return its exit status code.
/// `--no-exclude` disables the by-design `/fixtures/` exclusion so the fixture is actually scanned.
fn run_gate_over(path: &Path) -> i32 {
    let bin = env!("CARGO_BIN_EXE_lint-gate");
    let status = Command::new(bin)
        .arg("--no-exclude")
        .arg(path)
        .status()
        .expect("the lint-gate binary must run");
    status.code().expect("lint-gate exits with a code, not a signal")
}

#[test]
fn ci_gate_fails_loudly_on_the_no_raw_publish_red_fixture() {
    // The EB-07 red fixture (a write-path `publish_now` + a direct `transport.put`): the gate MUST
    // exit non-zero. This is the "CI fails on red" wiring proof — a non-zero exit cannot be `||
    // true`-swallowed because the exit code IS the gate.
    let red = fixtures_dir().join("no_raw_publish.red.rs.txt");
    let code = run_gate_over(&red);
    assert_ne!(code, 0, "lint-gate MUST exit non-zero on the no-raw-publish red fixture");
}

#[test]
fn ci_gate_passes_on_the_no_raw_publish_green_fixture() {
    // The green fixture (an `OutboxTx::emit` write path): the gate MUST exit zero — proving the
    // lint does not over-reject (both fixtures are the EB-07 pass condition).
    let green = fixtures_dir().join("no_raw_publish.green.rs.txt");
    let code = run_gate_over(&green);
    assert_eq!(code, 0, "lint-gate MUST exit zero on the no-raw-publish green fixture");
}

#[test]
fn ci_gate_is_clean_over_the_real_workspace() {
    // Belt-and-braces: the gate run with no args (the workspace `crates/*/src` tree, exclusions
    // honoured) exits zero — the live CI job is green on real code, not just fixtures.
    let bin = env!("CARGO_BIN_EXE_lint-gate");
    let status = Command::new(bin).status().expect("lint-gate runs with no args");
    assert!(status.success(), "lint-gate MUST be clean over the real workspace source");
}

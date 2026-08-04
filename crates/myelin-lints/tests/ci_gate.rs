use std::path::{Path, PathBuf};
use std::process::Command;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn flow_fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../myelin-flow/tests/fixtures")
}

fn run_gate_over(path: &Path) -> i32 {
    let bin = env!("CARGO_BIN_EXE_lint-gate");
    let status = Command::new(bin)
        .arg("--no-exclude")
        .arg(path)
        .status()
        .expect("the lint-gate binary must run");
    status
        .code()
        .expect("lint-gate exits with a code, not a signal")
}

fn run_gate_capture(path: &Path) -> (i32, String) {
    let bin = env!("CARGO_BIN_EXE_lint-gate");
    let out = Command::new(bin)
        .arg("--no-exclude")
        .arg(path)
        .output()
        .expect("the lint-gate binary must run");
    let code = out.status.code().expect("lint-gate exits with a code");
    (code, String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn per_lint_exclusion_suppresses_only_its_lint_and_the_file_still_trips_others() {
    let probe = fixtures_dir().join("perlint/myelin-flow/src/pg_executor.rs.txt");
    let (code, stderr) = run_gate_capture(&probe);
    assert_ne!(code, 0, "the file is still scanned by other lints, so it must fail loudly");
    assert!(
        stderr.contains("no-host-exec"),
        "the per-lint-excluded file MUST still trip a DIFFERENT lint (no-host-exec), got: {stderr}"
    );
    assert!(
        !stderr.contains("no-raw-ci-verdict"),
        "no-raw-ci-verdict MUST be suppressed for its own excluded seam file, got: {stderr}"
    );
}

#[test]
fn ci_gate_fails_loudly_on_the_no_raw_publish_red_fixture() {
    let red = fixtures_dir().join("no_raw_publish.red.rs.txt");
    let code = run_gate_over(&red);
    assert_ne!(
        code, 0,
        "lint-gate MUST exit non-zero on the no-raw-publish red fixture"
    );
}

#[test]
fn ci_gate_passes_on_the_no_raw_publish_green_fixture() {
    let green = fixtures_dir().join("no_raw_publish.green.rs.txt");
    let code = run_gate_over(&green);
    assert_eq!(
        code, 0,
        "lint-gate MUST exit zero on the no-raw-publish green fixture"
    );
}

#[test]
fn ci_gate_fails_loudly_on_the_eb08_write_path_red_fixture() {
    let red = fixtures_dir().join("no_cross_sync_cycle.eb08.red.rs.txt");
    let code = run_gate_over(&red);
    assert_ne!(
        code, 0,
        "lint-gate MUST exit non-zero on the EB-08 write-path red fixture"
    );
}

#[test]
fn ci_gate_passes_on_the_eb08_write_path_green_fixture() {
    let green = fixtures_dir().join("no_cross_sync_cycle.eb08.green.rs.txt");
    let code = run_gate_over(&green);
    assert_eq!(
        code, 0,
        "lint-gate MUST exit zero on the EB-08 write-path green fixture"
    );
}

#[test]
fn ci_gate_fails_loudly_on_the_eb09_stream_scope_red_fixture() {
    let red = fixtures_dir().join("tenant_predicate.eb09.red.rs.txt");
    let code = run_gate_over(&red);
    assert_ne!(
        code, 0,
        "lint-gate MUST exit non-zero on the EB-09 stream-scope red fixture"
    );
}

#[test]
fn ci_gate_passes_on_the_eb09_stream_scope_green_fixture() {
    let green = fixtures_dir().join("tenant_predicate.eb09.green.rs.txt");
    let code = run_gate_over(&green);
    assert_eq!(
        code, 0,
        "lint-gate MUST exit zero on the EB-09 stream-scope green fixture"
    );
}

#[test]
fn ci_gate_fails_loudly_on_the_flow_determinism_red_fixture() {
    let red = flow_fixtures_dir().join("flow_determinism.flow.red.rs.txt");
    let code = run_gate_over(&red);
    assert_ne!(
        code, 0,
        "lint-gate MUST exit non-zero on the flow-determinism red fixture"
    );
}

#[test]
fn ci_gate_passes_on_the_flow_determinism_green_fixture() {
    let green = flow_fixtures_dir().join("flow_determinism.flow.green.rs.txt");
    let code = run_gate_over(&green);
    assert_eq!(
        code, 0,
        "lint-gate MUST exit zero on the flow-determinism green fixture"
    );
}

#[test]
fn ci_gate_fails_loudly_on_the_no_raw_ci_verdict_red_fixture() {
    let red = fixtures_dir().join("no_raw_ci_verdict.red.rs.txt");
    let code = run_gate_over(&red);
    assert_ne!(
        code, 0,
        "lint-gate MUST exit non-zero on the no-raw-ci-verdict red fixture"
    );
}

#[test]
fn ci_gate_passes_on_the_no_raw_ci_verdict_green_fixture() {
    let green = fixtures_dir().join("no_raw_ci_verdict.green.rs.txt");
    let code = run_gate_over(&green);
    assert_eq!(
        code, 0,
        "lint-gate MUST exit zero on the no-raw-ci-verdict green fixture"
    );
}

#[test]
fn ci_gate_is_clean_over_the_real_workspace() {
    let bin = env!("CARGO_BIN_EXE_lint-gate");
    let status = Command::new(bin)
        .status()
        .expect("lint-gate runs with no args");
    assert!(
        status.success(),
        "lint-gate MUST be clean over the real workspace source"
    );
}

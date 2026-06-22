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

/// The `myelin-flow` crate's fixtures dir (a sibling crate). The flow-determinism RED+GREEN
/// fixtures (P-FLOW-08 / P-200) live there, expressed against the real `WfCtx` surface; this
/// CI-wiring test runs the SAME `lint-gate` binary CI runs over them, so the "the lint rejects the
/// red and admits the green" proof is end-to-end (the binary exit code, not just a unit assertion).
fn flow_fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../myelin-flow/tests/fixtures")
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
    status
        .code()
        .expect("lint-gate exits with a code, not a signal")
}

#[test]
fn ci_gate_fails_loudly_on_the_no_raw_publish_red_fixture() {
    // The EB-07 red fixture (a write-path `publish_now` + a direct `transport.put`): the gate MUST
    // exit non-zero. This is the "CI fails on red" wiring proof — a non-zero exit cannot be `||
    // true`-swallowed because the exit code IS the gate.
    let red = fixtures_dir().join("no_raw_publish.red.rs.txt");
    let code = run_gate_over(&red);
    assert_ne!(
        code, 0,
        "lint-gate MUST exit non-zero on the no-raw-publish red fixture"
    );
}

#[test]
fn ci_gate_passes_on_the_no_raw_publish_green_fixture() {
    // The green fixture (an `OutboxTx::emit` write path): the gate MUST exit zero — proving the
    // lint does not over-reject (both fixtures are the EB-07 pass condition).
    let green = fixtures_dir().join("no_raw_publish.green.rs.txt");
    let code = run_gate_over(&green);
    assert_eq!(
        code, 0,
        "lint-gate MUST exit zero on the no-raw-publish green fixture"
    );
}

#[test]
fn ci_gate_fails_loudly_on_the_eb08_write_path_red_fixture() {
    // EB-08 → P-044 (the Bus's owned slice of no-cross-sync-cycle, the write-path leg): the red
    // fixture (a sync cross-subsystem `call_sync` RPC in an `@write-path` merge gate — the "is it
    // green?" call) MUST make the gate exit NON-ZERO. The exit code IS the gate, so it cannot be
    // `|| true`-swallowed (EI-01 §5). This is the dated green proof that the EB-08 leg is wired into
    // CI loud-never-swallowed.
    let red = fixtures_dir().join("no_cross_sync_cycle.eb08.red.rs.txt");
    let code = run_gate_over(&red);
    assert_ne!(
        code, 0,
        "lint-gate MUST exit non-zero on the EB-08 write-path red fixture"
    );
}

#[test]
fn ci_gate_passes_on_the_eb08_write_path_green_fixture() {
    // The EB-08 green fixture (the merge gate reads its OWN cell-local projection, no sync RPC): the
    // gate MUST exit zero — proving the lint does not over-reject (both fixtures are the EB-08 pass
    // condition).
    let green = fixtures_dir().join("no_cross_sync_cycle.eb08.green.rs.txt");
    let code = run_gate_over(&green);
    assert_eq!(
        code, 0,
        "lint-gate MUST exit zero on the EB-08 write-path green fixture"
    );
}

#[test]
fn ci_gate_fails_loudly_on_the_eb09_stream_scope_red_fixture() {
    // EB-09 → P-045 (the Bus's owned slice of tenant-predicate, the subscribe/stream-scope leg): the
    // red fixture (an unscoped, wildcard-subject `subscribe` in a `@bus-stream` consumer) MUST make
    // the gate exit NON-ZERO. The exit code IS the gate, so it cannot be `|| true`-swallowed
    // (EI-01 §5). This is the dated green proof that the EB-09 leg is wired into CI loud-never-swallowed.
    let red = fixtures_dir().join("tenant_predicate.eb09.red.rs.txt");
    let code = run_gate_over(&red);
    assert_ne!(
        code, 0,
        "lint-gate MUST exit non-zero on the EB-09 stream-scope red fixture"
    );
}

#[test]
fn ci_gate_passes_on_the_eb09_stream_scope_green_fixture() {
    // The EB-09 green fixture (a bounded (tenant, subsystem) StreamScope subscribe): the gate MUST
    // exit zero — proving the lint does not over-reject (both fixtures are the EB-09 pass condition).
    let green = fixtures_dir().join("tenant_predicate.eb09.green.rs.txt");
    let code = run_gate_over(&green);
    assert_eq!(
        code, 0,
        "lint-gate MUST exit zero on the EB-09 stream-scope green fixture"
    );
}

#[test]
fn ci_gate_fails_loudly_on_the_flow_determinism_red_fixture() {
    // P-FLOW-08 / P-200 (the flow-determinism red+green fixtures, contract 1.6 / index 9.2): the
    // RED fixture (a `@workflow-body` reading SystemTime::now / rand:: / tokio::time::sleep /
    // Uuid::new_v4 — the non-deterministic-replay bug class) MUST make the gate exit NON-ZERO. The
    // exit code IS the gate, so it cannot be `|| true`-swallowed (EI-01 §5). This is the dated green
    // proof the flow-determinism lint REJECTS the red, wired into CI loud-never-swallowed.
    let red = flow_fixtures_dir().join("flow_determinism.flow.red.rs.txt");
    let code = run_gate_over(&red);
    assert_ne!(
        code, 0,
        "lint-gate MUST exit non-zero on the flow-determinism red fixture"
    );
}

#[test]
fn ci_gate_passes_on_the_flow_determinism_green_fixture() {
    // The flow-determinism GREEN fixture (the same digest workflow via ctx.now()/ctx.rand()/
    // ctx.activity(..)): the gate MUST exit ZERO — proving the lint ADMITS the WfCtx-routed body
    // (it does not over-reject). The green fixture is also COMPILE-checked against the real `WfCtx`
    // by `myelin-flow`'s `lint_fixtures::green_compiles` test (the "admits" half is an artifact).
    let green = flow_fixtures_dir().join("flow_determinism.flow.green.rs.txt");
    let code = run_gate_over(&green);
    assert_eq!(
        code, 0,
        "lint-gate MUST exit zero on the flow-determinism green fixture"
    );
}

#[test]
fn ci_gate_is_clean_over_the_real_workspace() {
    // Belt-and-braces: the gate run with no args (the workspace `crates/*/src` tree, exclusions
    // honoured) exits zero — the live CI job is green on real code, not just fixtures.
    let bin = env!("CARGO_BIN_EXE_lint-gate");
    let status = Command::new(bin)
        .status()
        .expect("lint-gate runs with no args");
    assert!(
        status.success(),
        "lint-gate MUST be clean over the real workspace source"
    );
}

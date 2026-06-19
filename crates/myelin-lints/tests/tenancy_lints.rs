//! The `residency-pin` write-boundary slice, the TENANCY ownership prompt (P-CP-03 → global P-026).
//!
//! `residency-pin` is one of the TWO lints Tenancy owns (contract-index 1.6). P-S11 → P-018 first
//! shipped it as the substrate-side store-construction scanner (a region-less pool is rejected),
//! and P-ST-04 → P-020 sharpened it to the real `myelin-storage` OLTP constructors. P-CP-03 is the
//! TENANCY slice — the genuinely-NEW second property the §4.3 residency-pin rule names: *every write
//! asserts `row.region == cell.region`*, with the cell's region threaded by the HARNESS, never taken
//! from a request field (refined-arch tenancy §4.3 / §5.3 layer 3). This is the **CP-D3 lint leg**.
//!
//! **Coherence note (EI-01 §7 — reconcile, never duplicate).** This prompt adds NO new lint id and
//! re-defines NO type. It EXTENDS the in-place [`myelin_lints::lints::residency_pin`] scanner with
//! the write-boundary fingerprint (a row region copied from `req.region` instead of `cell.region`,
//! keyed to a `// @residency-write` marker — the same loud, named marker discipline `@identity-sink`
//! / `@workflow-body` use) and adds the TENANCY-shaped red+green fixtures + this verdict test,
//! exactly mirroring the `storage_lints.rs` / `identity_lints.rs` precedent. The lint is SHARPENED,
//! never weakened (EI-01 §5): a region-mismatched write fails the build.
//!
//! These tests ARE the P-CP-03 fixtures (the TESTS field: "the two fixtures (1 red + 1 green) ARE
//! the tests"). They run loud over the Tenancy fixtures and assert the exact verdict; the CI-wiring
//! proof (the Tenancy red fixture ⇒ the `lint-gate` binary exits non-zero, no `|| true` swallow) is
//! the last test. No threshold is weakened.
//!
//! **Floor named.** The full RUNTIME CP-D3 drill (a live `row.region != cell.region` write REJECTED
//! at the write boundary + the `residency_verify` attestation) lands once the boundary exists in
//! **P-CP-12 / P-096** (+ Storage's store-layer enforcement P-ST-15 / P-102). This is the lint leg
//! (the compile-time rejection) only.

use std::path::{Path, PathBuf};
use std::process::Command;

use myelin_lints::engine::run;
use myelin_lints::lints::{all_twelve, residency_pin};
use myelin_lints::LintId;

const RED: &str = "residency_pin.tenancy.red.rs.txt";
const GREEN: &str = "residency_pin.tenancy.green.rs.txt";

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read_fixture(name: &str) -> String {
    let path = fixtures_dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("missing fixture {path:?}: {e}"))
}

#[test]
fn residency_pin_rejects_the_tenancy_write_boundary_red_fixture() {
    // REJECT: the write-boundary red fixture (row.region <- req.region) produces >= 1 violation,
    // fired by THE residency-pin lint and no other.
    let violations = residency_pin().run(&read_fixture(RED));
    assert!(
        !violations.is_empty(),
        "residency-pin MUST reject its tenancy write-boundary red fixture `{RED}`"
    );
    assert!(
        violations.iter().all(|v| v.lint == LintId("residency-pin")),
        "every violation on the tenancy red fixture must carry the residency-pin id"
    );
}

#[test]
fn residency_pin_admits_the_tenancy_write_boundary_green_fixture() {
    // ADMIT: the region-pinned write (row.region <- cell.region) produces 0 violations.
    let violations = residency_pin().run(&read_fixture(GREEN));
    assert!(
        violations.is_empty(),
        "residency-pin MUST admit its tenancy green fixture `{GREEN}`, but found: {violations:?}"
    );
}

#[test]
fn the_tenancy_red_fixture_trips_exactly_residency_pin() {
    // Cross-lint isolation: the tenancy write-boundary red fixture is caught by residency-pin and
    // NO OTHER of the twelve (so the whole-set gate attributes the failure correctly).
    let red = read_fixture(RED);
    let mut firing: Vec<LintId> = Vec::new();
    for lint in all_twelve() {
        if !lint.run(&red).is_empty() {
            firing.push(lint.id);
        }
    }
    assert_eq!(
        firing,
        vec![LintId("residency-pin")],
        "the tenancy red fixture must trip exactly residency-pin, but tripped: {firing:?}"
    );
}

#[test]
fn the_full_twelve_set_rejects_the_tenancy_red_and_admits_the_tenancy_green() {
    // The set-level gate (the form CI runs): run() over ALL twelve lints is Err on the tenancy red
    // fixture and Ok on the tenancy green fixture — loud, never swallowed (EI-01 §5).
    let all = all_twelve();
    assert!(
        run(&all, &read_fixture(RED)).is_err(),
        "the twelve-lint set must REJECT the tenancy write-boundary red fixture"
    );
    assert!(
        run(&all, &read_fixture(GREEN)).is_ok(),
        "the twelve-lint set must ADMIT the tenancy write-boundary green fixture"
    );
}

#[test]
fn residency_pin_reads_the_cell_region_from_the_harness_not_a_request_field() {
    // The TESTS-field assertion (P-CP-03): the lint reads the cell `region` from the harness-threaded
    // handle, NOT from a request field. The two fixtures differ ONLY in the region SOURCE — `ctx`
    // (the harness-threaded cell handle, admitted) vs `req` (a forgeable request field, rejected).
    let red = read_fixture(RED);
    let green = read_fixture(GREEN);
    assert!(
        red.contains("region: req.region"),
        "the red fixture's region source is a request field (the bug)"
    );
    assert!(
        green.contains("region: ctx.region"),
        "the green fixture's region source is the harness-threaded cell handle (correct)"
    );
    assert!(!residency_pin().run(&red).is_empty(), "request-sourced region must reject");
    assert!(residency_pin().run(&green).is_empty(), "cell-sourced region must admit");
}

#[test]
fn ci_gate_exits_non_zero_on_the_tenancy_red_fixture_and_zero_on_green() {
    // THE CI-WIRING PROOF (loud, never swallowed — EI-01 §5): the `lint-gate` binary the CI job runs
    // exits NON-ZERO over the tenancy red fixture and ZERO over the tenancy green fixture. A process
    // whose exit code IS the gate cannot be `|| true`-swallowed. `--no-exclude` disables the
    // by-design `/fixtures/` exclusion so the fixture is actually scanned.
    let bin = env!("CARGO_BIN_EXE_lint-gate");
    let run_over = |name: &str| -> i32 {
        Command::new(bin)
            .arg("--no-exclude")
            .arg(fixtures_dir().join(name))
            .status()
            .expect("the lint-gate binary must run")
            .code()
            .expect("lint-gate exits with a code, not a signal")
    };
    assert_ne!(
        run_over(RED),
        0,
        "lint-gate MUST exit non-zero on the tenancy write-boundary red fixture"
    );
    assert_eq!(
        run_over(GREEN),
        0,
        "lint-gate MUST exit zero on the tenancy write-boundary green fixture"
    );
}

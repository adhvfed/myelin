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
    let violations = residency_pin().run(&read_fixture(GREEN));
    assert!(
        violations.is_empty(),
        "residency-pin MUST admit its tenancy green fixture `{GREEN}`, but found: {violations:?}"
    );
}

#[test]
fn the_tenancy_red_fixture_trips_exactly_residency_pin() {
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
    assert!(
        !residency_pin().run(&red).is_empty(),
        "request-sourced region must reject"
    );
    assert!(
        residency_pin().run(&green).is_empty(),
        "cell-sourced region must admit"
    );
}

#[test]
fn ci_gate_exits_non_zero_on_the_tenancy_red_fixture_and_zero_on_green() {
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

use std::path::{Path, PathBuf};
use std::process::Command;

use myelin_lints::engine::run;
use myelin_lints::lints::{all_twelve, control_plane_pii_free, no_untagged_personal_data};
use myelin_lints::LintId;

const RED: &str = "control_plane_pii_free.tenancy.red.rs.txt";
const GREEN: &str = "control_plane_pii_free.tenancy.green.rs.txt";

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read_fixture(name: &str) -> String {
    let path = fixtures_dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("missing fixture {path:?}: {e}"))
}

#[test]
fn control_plane_pii_free_rejects_the_tenancy_frame_red_fixture() {
    let violations = control_plane_pii_free().run(&read_fixture(RED));
    assert!(
        !violations.is_empty(),
        "control-plane-pii-free MUST reject its tenancy frame red fixture `{RED}`"
    );
    assert!(
        violations
            .iter()
            .all(|v| v.lint == LintId("control-plane-pii-free")),
        "every violation on the tenancy red fixture must carry the control-plane-pii-free id"
    );
}

#[test]
fn control_plane_pii_free_admits_the_tenancy_frame_green_fixture() {
    let violations = control_plane_pii_free().run(&read_fixture(GREEN));
    assert!(
        violations.is_empty(),
        "control-plane-pii-free MUST admit its tenancy green fixture `{GREEN}`, but found: {violations:?}"
    );
}

#[test]
fn the_data_map_leg_catches_an_is_personal_field_no_name_fingerprint_would_miss() {
    let red = read_fixture(RED);
    assert!(
        red.contains("region_of_birth") && red.contains("#[personal_data"),
        "the red fixture's PII field is a #[personal_data]-tagged, non-name-fingerprinted column"
    );
    assert!(
        !red.contains("\n    email")
            && !red.contains("\n    name")
            && !red.contains("\n    display_name"),
        "the red fixture deliberately carries NO name-fingerprinted PII field - only the data-map \
         tag can catch it"
    );
    let violations = control_plane_pii_free().run(&red);
    assert!(
        violations.iter().any(|v| v.reason.contains("is_personal=true")),
        "the data-map leg must fire (reason names `is_personal=true`), proving the classification - \
         not the field name - caught the leak; got: {violations:?}"
    );
}

#[test]
fn the_tenancy_red_fixture_is_not_caught_by_no_untagged_personal_data() {
    let red = read_fixture(RED);
    assert!(
        no_untagged_personal_data().run(&red).is_empty(),
        "the tagged field must satisfy no-untagged-personal-data (it is tagged), isolating the \
         control-plane rule"
    );
}

#[test]
fn the_tenancy_red_fixture_trips_exactly_control_plane_pii_free() {
    let red = read_fixture(RED);
    let mut firing: Vec<LintId> = Vec::new();
    for lint in all_twelve() {
        if !lint.run(&red).is_empty() {
            firing.push(lint.id);
        }
    }
    assert_eq!(
        firing,
        vec![LintId("control-plane-pii-free")],
        "the tenancy red fixture must trip exactly control-plane-pii-free, but tripped: {firing:?}"
    );
}

#[test]
fn the_full_twelve_set_rejects_the_tenancy_red_and_admits_the_tenancy_green() {
    let all = all_twelve();
    assert!(
        run(&all, &read_fixture(RED)).is_err(),
        "the twelve-lint set must REJECT the tenancy frame red fixture"
    );
    assert!(
        run(&all, &read_fixture(GREEN)).is_ok(),
        "the twelve-lint set must ADMIT the tenancy frame green fixture"
    );
}

#[test]
fn the_lint_guards_the_real_frozen_cross_cell_pointer_frame() {
    let green = read_fixture(GREEN);
    assert!(
        green.contains("struct CrossCellPointer")
            && green.contains("subject: OpaqueSubjectId")
            && green.contains("home_cell: CellId"),
        "the green fixture must be the real frozen four-field CrossCellPointer frame"
    );
    assert!(
        control_plane_pii_free().run(&green).is_empty(),
        "the four-field frame must admit"
    );
    assert!(
        !control_plane_pii_free().run(&read_fixture(RED)).is_empty(),
        "the frame + an is_personal=true fifth field must be rejected"
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
        "lint-gate MUST exit non-zero on the tenancy frame red fixture"
    );
    assert_eq!(
        run_over(GREEN),
        0,
        "lint-gate MUST exit zero on the tenancy frame green fixture"
    );
}

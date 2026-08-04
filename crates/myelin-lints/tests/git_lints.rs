use std::path::{Path, PathBuf};
use std::process::Command;

use myelin_lints::engine::run;
use myelin_lints::lints::{all_twelve, no_untagged_personal_data};
use myelin_lints::LintId;

const GIT_RED: &str = "no_untagged_personal_data.git.red.rs.txt";
const GIT_GREEN: &str = "no_untagged_personal_data.git.green.rs.txt";

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read_fixture(name: &str) -> String {
    let path = fixtures_dir().join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("missing fixture {}: {e}", path.display()))
}

#[test]
fn the_git_red_fixture_is_rejected() {
    let violations = no_untagged_personal_data().run(&read_fixture(GIT_RED));
    assert_eq!(
        violations.len(),
        1,
        "exactly the untagged git body field must fire, got: {violations:?}"
    );
    assert!(
        violations[0].lint == LintId("no-untagged-personal-data"),
        "the git red-fixture violation must carry the no-untagged-personal-data id"
    );
    assert!(
        violations[0].reason.contains("comment_text"),
        "the violation must name the untagged `comment_text` body field, got: {:?}",
        violations[0]
    );
}

#[test]
fn the_git_green_fixture_is_admitted() {
    let violations = no_untagged_personal_data().run(&read_fixture(GIT_GREEN));
    assert!(
        violations.is_empty(),
        "no-untagged-personal-data MUST admit the fully-tagged git green fixture, but found: {violations:?}"
    );
}

#[test]
fn the_git_red_trips_exactly_its_own_lint() {
    let red = read_fixture(GIT_RED);
    let firing: Vec<LintId> = all_twelve()
        .into_iter()
        .filter(|lint| !lint.run(&red).is_empty())
        .map(|lint| lint.id)
        .collect();
    assert_eq!(
        firing,
        vec![LintId("no-untagged-personal-data")],
        "the git red fixture must trip exactly no-untagged-personal-data, but tripped: {firing:?}"
    );
}

#[test]
fn the_full_twelve_set_rejects_the_git_red_and_admits_the_git_green() {
    let all = all_twelve();
    assert!(
        run(&all, &read_fixture(GIT_RED)).is_err(),
        "the twelve-lint set must REJECT the git red fixture (untagged PII body)"
    );
    assert!(
        run(&all, &read_fixture(GIT_GREEN)).is_ok(),
        "the twelve-lint set must ADMIT the git green fixture (canonical six-tag tags)"
    );
}

#[test]
fn ci_gate_exits_non_zero_on_the_git_red_fixture_and_zero_on_green() {
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
        run_over(GIT_RED),
        0,
        "lint-gate MUST exit non-zero on the git red fixture (untagged PII body fails the build)"
    );
    assert_eq!(
        run_over(GIT_GREEN),
        0,
        "lint-gate MUST exit zero on the git green fixture (canonical six-tag tags admitted)"
    );
}

use myelin_lints::lints::{all_twelve, no_raw_publish};
use myelin_lints::{engine::run, LintId};
use std::path::{Path, PathBuf};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .join("myelin-lints/tests/fixtures")
}

fn read_fixture(name: &str) -> String {
    let path = fixtures_dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("missing fixture {path:?}: {e}"))
}

#[test]
fn knowledge_red_fixture_is_rejected_only_by_no_raw_publish() {
    let red = read_fixture("no_raw_publish.knowledge.red.rs.txt");
    let violations = no_raw_publish().run(&red);
    assert!(
        !violations.is_empty(),
        "the KN red fixture MUST be rejected by no-raw-publish"
    );
    assert!(
        violations
            .iter()
            .all(|v| v.lint == LintId("no-raw-publish")),
        "every violation carries the no-raw-publish id"
    );

    let firing: Vec<LintId> = all_twelve()
        .into_iter()
        .filter(|l| !l.run(&red).is_empty())
        .map(|l| l.id)
        .collect();
    assert_eq!(
        firing,
        vec![LintId("no-raw-publish")],
        "exactly no-raw-publish trips, no other"
    );
}

#[test]
fn knowledge_green_fixture_is_admitted_by_the_full_set() {
    let green = read_fixture("no_raw_publish.knowledge.green.rs.txt");
    assert!(
        no_raw_publish().run(&green).is_empty(),
        "no-raw-publish MUST admit the KN green fixture (the outbox-emit path)"
    );
    assert!(
        run(&all_twelve(), &green).is_ok(),
        "the twelve-lint set must ADMIT the KN green fixture (no lint may false-positive)"
    );
}

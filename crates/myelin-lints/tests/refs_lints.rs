use std::path::{Path, PathBuf};
use std::process::Command;

use myelin_lints::engine::run;
use myelin_lints::lints::{
    all_twelve, no_cross_db, no_cross_sync_cycle, no_raw_publish, tenant_predicate,
};
use myelin_lints::{Lint, LintId};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read_fixture(name: &str) -> String {
    let path = fixtures_dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("missing fixture {path:?}: {e}"))
}

struct RefsRow {
    lint: fn() -> Lint,
    id: LintId,
    red: &'static str,
    green: &'static str,
}

fn refs_rows() -> Vec<RefsRow> {
    vec![
        RefsRow {
            lint: tenant_predicate,
            id: LintId("tenant-predicate"),
            red: "tenant_predicate.refs.red.rs.txt",
            green: "tenant_predicate.refs.green.rs.txt",
        },
        RefsRow {
            lint: no_raw_publish,
            id: LintId("no-raw-publish"),
            red: "no_raw_publish.refs.red.rs.txt",
            green: "no_raw_publish.refs.green.rs.txt",
        },
        RefsRow {
            lint: no_cross_db,
            id: LintId("no-cross-db"),
            red: "no_cross_db.refs.red.rs.txt",
            green: "no_cross_db.refs.green.rs.txt",
        },
        RefsRow {
            lint: no_cross_sync_cycle,
            id: LintId("no-cross-sync-cycle"),
            red: "no_cross_sync_cycle.refs.red.rs.txt",
            green: "no_cross_sync_cycle.refs.green.rs.txt",
        },
    ]
}

#[test]
fn there_are_exactly_four_refs_lints() {
    assert_eq!(
        refs_rows().len(),
        4,
        "REF-P2 wires exactly the four Refs lints"
    );
}

#[test]
fn every_refs_red_fixture_is_rejected_by_its_own_lint() {
    for row in refs_rows() {
        let violations = (row.lint)().run(&read_fixture(row.red));
        assert!(
            !violations.is_empty(),
            "lint `{}` MUST reject its Refs red fixture `{}`, but found 0 violations",
            row.id,
            row.red
        );
        assert!(
            violations.iter().all(|v| v.lint == row.id),
            "lint `{}`'s violations on `{}` must all carry its own id",
            row.id,
            row.red
        );
    }
}

#[test]
fn every_refs_green_fixture_is_admitted_by_its_own_lint() {
    for row in refs_rows() {
        let violations = (row.lint)().run(&read_fixture(row.green));
        assert!(
            violations.is_empty(),
            "lint `{}` MUST admit its Refs green fixture `{}`, but found: {violations:?}",
            row.id,
            row.green
        );
    }
}

#[test]
fn each_refs_red_fixture_trips_exactly_its_own_lint() {
    for row in refs_rows() {
        let red = read_fixture(row.red);
        let mut firing: Vec<LintId> = Vec::new();
        for lint in all_twelve() {
            if !lint.run(&red).is_empty() {
                firing.push(lint.id);
            }
        }
        assert_eq!(
            firing,
            vec![row.id],
            "the Refs red fixture `{}` must trip exactly `{}`, but tripped: {firing:?}",
            row.red,
            row.id
        );
    }
}

#[test]
fn the_full_twelve_set_rejects_each_refs_red_and_admits_each_refs_green() {
    let all = all_twelve();
    for row in refs_rows() {
        assert!(
            run(&all, &read_fixture(row.red)).is_err(),
            "the twelve-lint set must REJECT the Refs red fixture `{}`",
            row.red
        );
        assert!(
            run(&all, &read_fixture(row.green)).is_ok(),
            "the twelve-lint set must ADMIT the Refs green fixture `{}` (no lint may false-positive)",
            row.green
        );
    }
}

#[test]
fn the_marker_scoped_refs_legs_are_inert_without_their_marker() {
    let red = read_fixture("no_cross_sync_cycle.refs.red.rs.txt");
    let unmarked = red.replace("@write-path", "(removed-marker)");
    assert!(
        no_cross_sync_cycle().run(&unmarked).is_empty(),
        "the no-cross-sync-cycle write-path leg must be INERT on an unmarked Refs source, so the lint \
         admits the whole current workspace until the Refs resolution write path lands (REF-P9..P11)"
    );
}

#[test]
fn ci_gate_exits_non_zero_on_each_refs_red_fixture_and_zero_on_each_green() {
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
    for row in refs_rows() {
        assert_ne!(
            run_over(row.red),
            0,
            "lint-gate MUST exit non-zero on the Refs red fixture `{}` (loud, never swallowed)",
            row.red
        );
        assert_eq!(
            run_over(row.green),
            0,
            "lint-gate MUST exit zero on the Refs green fixture `{}`",
            row.green
        );
    }
}

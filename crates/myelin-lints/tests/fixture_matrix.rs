//! THE fixture matrix (the P-S10 → P-017 required test, EXTENDED to twelve in P-S11 → P-018).
//!
//! For each of the TWELVE architecture lints: run its RED fixture and assert the lint REJECTS it
//! (≥1 violation, fired by THAT lint), run its GREEN fixture and assert the lint ADMITS it (0
//! violations). The full 12×(red-reject + green-admit) matrix is the dated green artifact.
//!
//! Plus the ratchet regression test: removing any one of the twelve lints' wiring must make the
//! matrix fail (the gate cannot be silently un-wired — EI-01 §5).

use myelin_lints::engine::run;
use myelin_lints::lints::{
    all_twelve, control_plane_pii_free, flow_determinism, forward_only_migration, no_cross_db,
    no_cross_sync_cycle, no_host_exec, no_llm_in_platform, no_raw_publish,
    no_untagged_personal_data, residency_pin, search_requires_acl_filter, tenant_predicate,
};
use myelin_lints::{Lint, LintId};

const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn read_fixture(name: &str) -> String {
    let path = format!("{FIXTURES_DIR}/{name}");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("missing fixture {path}: {e}"))
}

/// One row of the matrix: a lint + its red fixture file + its green fixture file.
struct MatrixRow {
    lint: fn() -> Lint,
    id: LintId,
    red: &'static str,
    green: &'static str,
}

/// The full TWELVE-row matrix, in §2.11 table order (the four load-bearing then the eight).
fn matrix() -> Vec<MatrixRow> {
    vec![
        // ---- the four load-bearing (P-S10 → P-017) ----
        MatrixRow {
            lint: tenant_predicate,
            id: LintId("tenant-predicate"),
            red: "tenant_predicate.red.rs.txt",
            green: "tenant_predicate.green.rs.txt",
        },
        MatrixRow {
            lint: no_raw_publish,
            id: LintId("no-raw-publish"),
            red: "no_raw_publish.red.rs.txt",
            green: "no_raw_publish.green.rs.txt",
        },
        MatrixRow {
            lint: no_host_exec,
            id: LintId("no-host-exec"),
            red: "no_host_exec.red.rs.txt",
            green: "no_host_exec.green.rs.txt",
        },
        MatrixRow {
            lint: no_untagged_personal_data,
            id: LintId("no-untagged-personal-data"),
            red: "no_untagged_personal_data.red.rs.txt",
            green: "no_untagged_personal_data.green.rs.txt",
        },
        // ---- the remaining eight (P-S11 → P-018) ----
        MatrixRow {
            lint: no_cross_db,
            id: LintId("no-cross-db"),
            red: "no_cross_db.red.rs.txt",
            green: "no_cross_db.green.rs.txt",
        },
        MatrixRow {
            lint: forward_only_migration,
            id: LintId("forward-only-migration"),
            red: "forward_only_migration.red.rs.txt",
            green: "forward_only_migration.green.rs.txt",
        },
        MatrixRow {
            lint: no_cross_sync_cycle,
            id: LintId("no-cross-sync-cycle"),
            red: "no_cross_sync_cycle.red.rs.txt",
            green: "no_cross_sync_cycle.green.rs.txt",
        },
        MatrixRow {
            lint: residency_pin,
            id: LintId("residency-pin"),
            red: "residency_pin.red.rs.txt",
            green: "residency_pin.green.rs.txt",
        },
        MatrixRow {
            lint: control_plane_pii_free,
            id: LintId("control-plane-pii-free"),
            red: "control_plane_pii_free.red.rs.txt",
            green: "control_plane_pii_free.green.rs.txt",
        },
        MatrixRow {
            lint: search_requires_acl_filter,
            id: LintId("search-requires-acl-filter"),
            red: "search_requires_acl_filter.red.rs.txt",
            green: "search_requires_acl_filter.green.rs.txt",
        },
        MatrixRow {
            lint: no_llm_in_platform,
            id: LintId("no-llm-in-platform"),
            red: "no_llm_in_platform.red.rs.txt",
            green: "no_llm_in_platform.green.rs.txt",
        },
        MatrixRow {
            lint: flow_determinism,
            id: LintId("flow-determinism"),
            red: "flow_determinism.red.rs.txt",
            green: "flow_determinism.green.rs.txt",
        },
    ]
}

#[test]
fn the_matrix_has_a_row_per_lint() {
    // 12/12: the matrix covers exactly the twelve lints, in §2.11 order.
    let rows = matrix();
    assert_eq!(rows.len(), 12, "the matrix must cover all twelve lints");
    let row_ids: Vec<LintId> = rows.iter().map(|r| r.id).collect();
    let lint_ids: Vec<LintId> = all_twelve().iter().map(|l| l.id).collect();
    assert_eq!(row_ids, lint_ids, "matrix rows must match the twelve lints in order");
}

#[test]
fn every_red_fixture_is_rejected() {
    // 12/12 REJECT: each lint's red fixture produces ≥1 violation, fired by THAT lint.
    for row in matrix() {
        let lint = (row.lint)();
        let src = read_fixture(row.red);
        let violations = lint.run(&src);
        assert!(
            !violations.is_empty(),
            "lint `{}` MUST reject its red fixture `{}`, but found 0 violations",
            row.id,
            row.red
        );
        assert!(
            violations.iter().all(|v| v.lint == row.id),
            "lint `{}`'s violations must all carry its own id",
            row.id
        );
    }
}

#[test]
fn every_green_fixture_is_admitted() {
    // 12/12 ADMIT: each lint's green fixture produces 0 violations.
    for row in matrix() {
        let lint = (row.lint)();
        let src = read_fixture(row.green);
        let violations = lint.run(&src);
        assert!(
            violations.is_empty(),
            "lint `{}` MUST admit its green fixture `{}`, but found: {:?}",
            row.id,
            row.green,
            violations
        );
    }
}

#[test]
fn the_full_twelve_lint_set_run_rejects_each_red_and_admits_each_green() {
    // The set-level gate (the form CI runs): run() over ALL twelve lints is Err on each red
    // fixture and Ok on each green fixture. This is the loud-never-swallowed surface.
    //
    // NOTE: each red fixture is crafted to trip EXACTLY ONE lint, and each GREEN fixture is
    // crafted so NO lint fires over it (not just the row's own lint) — so the whole-set run is
    // Ok. This is asserted explicitly below so a cross-lint false-positive is caught loudly.
    let all = all_twelve();
    for row in matrix() {
        assert!(
            run(&all, &read_fixture(row.red)).is_err(),
            "the twelve-lint set must REJECT red fixture `{}`",
            row.red
        );
        assert!(
            run(&all, &read_fixture(row.green)).is_ok(),
            "the twelve-lint set must ADMIT green fixture `{}` (no lint may false-positive on \
             another lint's green fixture)",
            row.green
        );
    }
}

#[test]
fn each_red_fixture_trips_exactly_its_own_lint() {
    // Cross-lint isolation: a red fixture for lint X must be caught by X and by NO OTHER lint.
    // This keeps the ratchet-regression test below sound (dropping X leaves X's red un-caught).
    for row in matrix() {
        let all = all_twelve();
        let red = read_fixture(row.red);
        let mut firing: Vec<LintId> = Vec::new();
        for lint in &all {
            if !lint.run(&red).is_empty() {
                firing.push(lint.id);
            }
        }
        assert_eq!(
            firing,
            vec![row.id],
            "red fixture `{}` must trip exactly its own lint `{}`, but tripped: {:?}",
            row.red,
            row.id,
            firing
        );
    }
}

#[test]
fn removing_any_lint_breaks_the_matrix() {
    // THE RATCHET REGRESSION TEST: if any one of the twelve lints is un-wired, that lint's red
    // fixture is no longer rejected by the remaining set — the matrix detects the missing gate.
    // Proves the gate cannot be silently un-wired (EI-01 §5).
    let rows = matrix();
    for (drop_idx, dropped) in rows.iter().enumerate() {
        // The set with lint `drop_idx` removed.
        let reduced: Vec<Lint> = all_twelve()
            .into_iter()
            .enumerate()
            .filter(|(i, _)| *i != drop_idx)
            .map(|(_, l)| l)
            .collect();
        let red = read_fixture(dropped.red);
        // With its lint removed, the red fixture that ONLY that lint catches is now admitted by
        // the reduced set → the gate is missing. (Each red fixture is crafted to trip exactly
        // one lint, asserted by `each_red_fixture_trips_exactly_its_own_lint`.)
        let full_violations = run(&all_twelve(), &red);
        assert!(full_violations.is_err(), "full set must reject {}", dropped.red);
        let reduced_violations = run(&reduced, &red);
        assert!(
            reduced_violations.is_ok(),
            "removing lint `{}` must leave its red fixture `{}` UN-caught (the ratchet \
             regression: an un-wired gate is detectable)",
            dropped.id,
            dropped.red
        );
    }
}

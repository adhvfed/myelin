use std::path::{Path, PathBuf};
use std::process::Command;

use myelin_lints::engine::run;
use myelin_lints::lints::{all_twelve, forward_only_migration, residency_pin};
use myelin_lints::{Lint, LintId};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read_fixture(name: &str) -> String {
    let path = fixtures_dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("missing fixture {path:?}: {e}"))
}

struct StorageRow {
    lint: fn() -> Lint,
    id: LintId,
    red: &'static str,
    green: &'static str,
}

fn storage_matrix() -> Vec<StorageRow> {
    vec![
        StorageRow {
            lint: forward_only_migration,
            id: LintId("forward-only-migration"),
            red: "forward_only_migration.storage.red.rs.txt",
            green: "forward_only_migration.storage.green.rs.txt",
        },
        StorageRow {
            lint: residency_pin,
            id: LintId("residency-pin"),
            red: "residency_pin.storage.red.rs.txt",
            green: "residency_pin.storage.green.rs.txt",
        },
    ]
}

#[test]
fn the_two_storage_lints_reject_their_storage_red_fixtures() {
    for row in storage_matrix() {
        let lint = (row.lint)();
        let violations = lint.run(&read_fixture(row.red));
        assert!(
            !violations.is_empty(),
            "storage lint `{}` MUST reject its red fixture `{}`",
            row.id,
            row.red
        );
        assert!(
            violations.iter().all(|v| v.lint == row.id),
            "storage lint `{}`'s violations must all carry its own id",
            row.id
        );
    }
}

#[test]
fn the_two_storage_lints_admit_their_storage_green_fixtures() {
    for row in storage_matrix() {
        let lint = (row.lint)();
        let violations = lint.run(&read_fixture(row.green));
        assert!(
            violations.is_empty(),
            "storage lint `{}` MUST admit its green fixture `{}`, but found: {:?}",
            row.id,
            row.green,
            violations
        );
    }
}

#[test]
fn each_storage_red_fixture_trips_exactly_its_own_lint() {
    for row in storage_matrix() {
        let mut firing: Vec<LintId> = Vec::new();
        let red = read_fixture(row.red);
        for lint in all_twelve() {
            if !lint.run(&red).is_empty() {
                firing.push(lint.id);
            }
        }
        assert_eq!(
            firing,
            vec![row.id],
            "storage red fixture `{}` must trip exactly its own lint `{}`, but tripped: {:?}",
            row.red,
            row.id,
            firing
        );
    }
}

#[test]
fn the_full_twelve_set_rejects_each_storage_red_and_admits_each_storage_green() {
    let all = all_twelve();
    for row in storage_matrix() {
        assert!(
            run(&all, &read_fixture(row.red)).is_err(),
            "the twelve-lint set must REJECT storage red fixture `{}`",
            row.red
        );
        assert!(
            run(&all, &read_fixture(row.green)).is_ok(),
            "the twelve-lint set must ADMIT storage green fixture `{}` (no lint may false-positive)",
            row.green
        );
    }
}

#[test]
fn residency_pin_is_sharpened_to_the_real_oltp_constructors() {
    let red_pool = "let p = OltpPool::open(cfg);";
    let red_coloc = "let db = ColocatedOltp::open(cfg, minter);";
    let green_pool = "let p = OltpPool::open(cfg.region(region));";
    assert!(
        !residency_pin().run(red_pool).is_empty(),
        "region-less OltpPool::open must reject"
    );
    assert!(
        !residency_pin().run(red_coloc).is_empty(),
        "region-less ColocatedOltp::open must reject"
    );
    assert!(
        residency_pin().run(green_pool).is_empty(),
        "region-pinned open must admit"
    );
}

#[test]
fn residency_pin_named_floor_waiver_is_loud_and_scoped() {
    let site_waived =
        "// @residency-cell-pinned (M0 floor -> P-ST-15)\nlet p = OltpPool::open(cfg);";
    let file_waived =
        "//! @residency-cell-pinned:file (the M0 pool model)\nfn f() { OltpPool::open(cfg); }";
    let unmarked = "fn f() { OltpPool::open(cfg); }";
    assert!(
        residency_pin().run(site_waived).is_empty(),
        "a named site waiver admits"
    );
    assert!(
        residency_pin().run(file_waived).is_empty(),
        "a named file waiver admits"
    );
    assert!(
        !residency_pin().run(unmarked).is_empty(),
        "an UNMARKED region-less open must still fire (the waiver is named, not a blanket skip)"
    );
}

#[test]
fn forward_only_migration_rejects_inplace_rewrite_admits_online_expand() {
    let red_inplace = "ALTER TABLE principals ALTER COLUMN email TYPE TEXT;";
    let red_drop = "DROP COLUMN email_old;";
    let green_expand = "ALTER TABLE principals ADD COLUMN email_v2 TEXT;";
    assert!(
        !forward_only_migration().run(red_inplace).is_empty(),
        "in-place rewrite must reject"
    );
    assert!(
        !forward_only_migration().run(red_drop).is_empty(),
        "DROP COLUMN must reject"
    );
    assert!(
        forward_only_migration().run(green_expand).is_empty(),
        "nullable add must admit"
    );
}

#[test]
fn ci_gate_exits_non_zero_on_a_storage_red_fixture_and_zero_on_green() {
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
        run_over("residency_pin.storage.red.rs.txt"),
        0,
        "lint-gate MUST exit non-zero on the storage residency-pin red fixture"
    );
    assert_ne!(
        run_over("forward_only_migration.storage.red.rs.txt"),
        0,
        "lint-gate MUST exit non-zero on the storage forward-only-migration red fixture"
    );
    assert_eq!(
        run_over("residency_pin.storage.green.rs.txt"),
        0,
        "lint-gate MUST exit zero on the storage residency-pin green fixture"
    );
    assert_eq!(
        run_over("forward_only_migration.storage.green.rs.txt"),
        0,
        "lint-gate MUST exit zero on the storage forward-only-migration green fixture"
    );
}

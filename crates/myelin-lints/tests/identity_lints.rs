use std::path::{Path, PathBuf};
use std::process::Command;

use myelin_lints::engine::run;
use myelin_lints::lints::{
    all_twelve, control_plane_pii_free, no_untagged_personal_data, residency_pin, tenant_predicate,
};
use myelin_lints::{Lint, LintId};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read_fixture(name: &str) -> String {
    let path = fixtures_dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("missing fixture {path:?}: {e}"))
}

struct IdentityRow {
    lint: fn() -> Lint,
    id: LintId,
    red: &'static str,
    green: &'static str,
}

fn identity_matrix() -> Vec<IdentityRow> {
    vec![
        IdentityRow {
            lint: tenant_predicate,
            id: LintId("tenant-predicate"),
            red: "tenant_predicate.identity.red.rs.txt",
            green: "tenant_predicate.identity.green.rs.txt",
        },
        IdentityRow {
            lint: no_untagged_personal_data,
            id: LintId("no-untagged-personal-data"),
            red: "no_untagged_personal_data.identity.red.rs.txt",
            green: "no_untagged_personal_data.identity.green.rs.txt",
        },
        IdentityRow {
            lint: residency_pin,
            id: LintId("residency-pin"),
            red: "residency_pin.identity.red.rs.txt",
            green: "residency_pin.identity.green.rs.txt",
        },
        IdentityRow {
            lint: control_plane_pii_free,
            id: LintId("control-plane-pii-free"),
            red: "control_plane_pii_free.identity.red.rs.txt",
            green: "control_plane_pii_free.identity.green.rs.txt",
        },
    ]
}

#[test]
fn the_four_id_lints_are_exactly_the_ratchet_unit() {
    let rows = identity_matrix();
    assert_eq!(rows.len(), 4, "the Id ratchet unit is exactly four lints");
    let ids: Vec<LintId> = rows.iter().map(|r| r.id).collect();
    assert_eq!(
        ids,
        vec![
            LintId("tenant-predicate"),
            LintId("no-untagged-personal-data"),
            LintId("residency-pin"),
            LintId("control-plane-pii-free"),
        ],
        "the four Id-relevant lints, in the P-ID-03 DELIVERABLE order"
    );
}

#[test]
fn the_four_id_lints_reject_their_identity_red_fixtures() {
    for row in identity_matrix() {
        let lint = (row.lint)();
        let violations = lint.run(&read_fixture(row.red));
        assert!(
            !violations.is_empty(),
            "Id lint `{}` MUST reject its red fixture `{}`",
            row.id,
            row.red
        );
        assert!(
            violations.iter().all(|v| v.lint == row.id),
            "Id lint `{}`'s violations must all carry its own id",
            row.id
        );
    }
}

#[test]
fn the_four_id_lints_admit_their_identity_green_fixtures() {
    for row in identity_matrix() {
        let lint = (row.lint)();
        let violations = lint.run(&read_fixture(row.green));
        assert!(
            violations.is_empty(),
            "Id lint `{}` MUST admit its green fixture `{}`, but found: {:?}",
            row.id,
            row.green,
            violations
        );
    }
}

#[test]
fn each_identity_red_fixture_trips_exactly_its_own_lint() {
    for row in identity_matrix() {
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
            "Identity red fixture `{}` must trip exactly its own lint `{}`, but tripped: {:?}",
            row.red,
            row.id,
            firing
        );
    }
}

#[test]
fn the_full_twelve_set_rejects_each_identity_red_and_admits_each_identity_green() {
    let all = all_twelve();
    for row in identity_matrix() {
        assert!(
            run(&all, &read_fixture(row.red)).is_err(),
            "the twelve-lint set must REJECT Identity red fixture `{}`",
            row.red
        );
        assert!(
            run(&all, &read_fixture(row.green)).is_ok(),
            "the twelve-lint set must ADMIT Identity green fixture `{}` (no lint may false-positive)",
            row.green
        );
    }
}

#[test]
fn control_plane_pii_free_guards_the_iam_event_projection() {
    let leaking = read_fixture("control_plane_pii_free.identity.red.rs.txt");
    let opaque = read_fixture("control_plane_pii_free.identity.green.rs.txt");
    assert!(
        !control_plane_pii_free().run(&leaking).is_empty(),
        "an iam.* projection leaking name/email MUST be rejected"
    );
    assert!(
        control_plane_pii_free().run(&opaque).is_empty(),
        "the opaque-id iam.* projection from P-ID-02 MUST be admitted"
    );
}

#[test]
fn ci_gate_exits_non_zero_on_an_identity_red_fixture_and_zero_on_green() {
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
    for row in identity_matrix() {
        assert_ne!(
            run_over(row.red),
            0,
            "lint-gate MUST exit non-zero on Identity red fixture `{}`",
            row.red
        );
        assert_eq!(
            run_over(row.green),
            0,
            "lint-gate MUST exit zero on Identity green fixture `{}`",
            row.green
        );
    }
}

//! The FOUR Id-relevant architecture lints, the Identity slice (P-ID-03 → global P-024).
//!
//! P-S10 → P-017 and P-S11 → P-018 first shipped the twelve architecture lints as the shared
//! substrate scanners + engine + the generic fixture matrix (`fixture_matrix.rs`). P-ST-04 → P-020
//! then shipped the STORAGE slice (`storage_lints.rs`): the two storage-relevant lints with
//! storage-shaped fixtures, sharpened to the real `myelin-storage` constructors. P-ID-03 is the
//! IDENTITY slice — the contract-1.6 / §2.11 four Id-relevant lints (`tenant-predicate`,
//! `no-untagged-personal-data`, `residency-pin`, `control-plane-pii-free`) wired as ONE ratchet
//! unit, each with an IDENTITY-shaped red fixture (an Id bug fingerprint the gate must reject) +
//! an Identity-shaped green fixture (the correct Id shape the gate must admit), so the
//! no-IDOR / no-untagged-PII / no-cross-region / control-plane-PII-free surface is closed for
//! Identity in one commit.
//!
//! **Coherence note (EI-01 §7 — reconcile, never duplicate).** The four lints, their engine, and
//! the generic red/green fixtures ALREADY exist (P-017/P-018); the iam.* opaque-id event projection
//! the `control-plane-pii-free` lint guards landed in P-ID-02 → P-023. This prompt adds NO new lint
//! and re-defines NO type: it reuses the in-place [`myelin_lints`] scanners and adds only the
//! genuinely-new Identity-shaped fixtures + this Identity verdict test, exactly mirroring the
//! `storage_lints.rs` precedent. The lints are exercised, never weakened (EI-01 §5).
//!
//! These tests ARE the P-ID-03 fixtures (the TESTS field: "the eight fixtures (4 red + 4 green) ARE
//! the tests"). They run loud over the Identity fixtures and assert the exact verdict; the CI-wiring
//! proof (an Identity red fixture ⇒ the `lint-gate` binary exits non-zero, no `|| true` swallow) is
//! the last test. No threshold is weakened: a lint is never softened to admit a red fixture.

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

/// One Identity-lint row: the lint + its Identity red fixture + its Identity green fixture.
struct IdentityRow {
    lint: fn() -> Lint,
    id: LintId,
    red: &'static str,
    green: &'static str,
}

/// The FOUR Id-relevant lints (contract-1.6 / §2.11), in the P-ID-03 DELIVERABLE order.
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
    // 4/4: the Identity ratchet unit is exactly the four contract-1.6 Id-relevant lints, in the
    // P-ID-03 DELIVERABLE order. (Guards against a row drifting off the named four.)
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
    // 4/4 REJECT: each Identity red fixture produces >= 1 violation, fired by THAT lint.
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
    // 4/4 ADMIT: each Identity green fixture produces 0 violations from ITS lint.
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
    // Cross-lint isolation over the Identity fixtures: an Identity red fixture for lint X is caught
    // by X and NO OTHER of the twelve (so the whole-set gate attributes the failure correctly, and
    // the green fixtures cannot be trivially passing by a different lint over-matching).
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
    // The set-level gate (the form CI runs): run() over ALL twelve lints is Err on each Identity red
    // fixture and Ok on each Identity green fixture — loud, never swallowed (EI-01 §5). No green
    // fixture may false-positive on another lint.
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
    // The P-ID-02 ↔ P-ID-03 seam: the `control-plane-pii-free` lint REJECTS an `iam.*` event
    // projection that leaks a name/email (the erasable profile crossing the immutable log) and
    // ADMITS the opaque-id projection P-ID-02 freezes (actor/subject by opaque `principal_id`
    // only, `contains_personal_data` false). This is the lint enforcing the GATE field of P-ID-02:
    // "the projection contains no PII field".
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
    // THE CI-WIRING PROOF (loud, never swallowed — EI-01 §5): the `lint-gate` binary the CI job
    // runs exits NON-ZERO over each Identity red fixture and ZERO over each Identity green fixture.
    // A process whose exit code IS the gate cannot be `|| true`-swallowed. `--no-exclude` disables
    // the by-design `/fixtures/` exclusion so the fixture is actually scanned.
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

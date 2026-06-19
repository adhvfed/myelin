//! The GDPR/Audit slice of the architecture-lint ratchet (P-GA-03 → P-051).
//!
//! GDPR owns the `no-untagged-personal-data` lint (contract-index 1.6). The lint, its engine, and
//! its substrate red/green fixtures were FIRST shipped by P-S10 → P-017 (the four load-bearing
//! lints — shared substrate). P-GA-03 is the GDPR owner's realization: the lint must enforce
//! presence of the ACTUAL frozen `#[personal_data(...)]` attribute — the full SIX-TAG keyword form
//! frozen in P-GA-02 / P-050 (`category | role | basis | retention | erasure | subject_locator`;
//! gdpr-and-audit §2.1) — "we forgot a store/field is a *structural* failure" (ADR-12.1; GA-D5).
//!
//! These tests run loud over the GDPR-owned fixtures and assert the exact verdict (red rejected,
//! green admitted); the CI-wiring test below proves the `lint-gate` binary (the thing the CI job
//! runs) exits NON-ZERO on the GDPR red fixture and ZERO on the green — loud-never-swallowed
//! (EI-01 §5), the GA-D5 dated green artifact.
//!
//! **What P-GA-03 SHARPENED (code-wins-over-docs, EI-01 §1).** The P-017 floor only recognized the
//! single-line attribute form; it FALSELY REJECTED the canonical MULTI-LINE tag the §2.1 / P-050
//! shape uses (the line above the field is the attribute's closing `)]`, not the `#[personal_data(`
//! opener). A lint that rejects the contract's own frozen attribute is the bug — every M1 store
//! using the real tag would have failed the build. The scanner now tracks the attribute's
//! multi-line bracket span; `multi_line_six_tag_attribute_is_admitted` is the regression that pins
//! the fix. The lint is SHARPENED (it admits the real frozen shape), never weakened: an UNtagged
//! PII field still fails (the red fixture proves it).

use std::path::{Path, PathBuf};
use std::process::Command;

use myelin_lints::engine::run;
use myelin_lints::lints::{all_twelve, no_untagged_personal_data};
use myelin_lints::LintId;

const GDPR_RED: &str = "no_untagged_personal_data.gdpr.red.rs.txt";
const GDPR_GREEN: &str = "no_untagged_personal_data.gdpr.green.rs.txt";

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn read_fixture(name: &str) -> String {
    let path = fixtures_dir().join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("missing fixture {}: {e}", path.display()))
}

#[test]
fn the_gdpr_red_fixture_is_rejected() {
    // GA-D5 (reject leg): an untagged PII field (a consent/DSR row whose `email`/`full_name`/`phone`
    // carry no `#[personal_data(...)]` tag) escapes the data-map + crypto-shred fan-out — the lint
    // MUST reject it, fired by THAT lint.
    let lint = no_untagged_personal_data();
    let violations = lint.run(&read_fixture(GDPR_RED));
    assert!(
        !violations.is_empty(),
        "no-untagged-personal-data MUST reject the GDPR red fixture (untagged PII), but found 0"
    );
    assert!(
        violations.iter().all(|v| v.lint == LintId("no-untagged-personal-data")),
        "every GDPR red-fixture violation must carry the no-untagged-personal-data id"
    );
}

#[test]
fn the_gdpr_green_fixture_is_admitted() {
    // GA-D5 (admit leg): every PII field carries the canonical six-tag `#[personal_data(...)]` tag,
    // so the lint MUST admit it (both verdicts are the GA-D5 pass condition — a lint that only
    // rejects is not proven).
    let lint = no_untagged_personal_data();
    let violations = lint.run(&read_fixture(GDPR_GREEN));
    assert!(
        violations.is_empty(),
        "no-untagged-personal-data MUST admit the GDPR green fixture (canonical six-tag tags), \
         but found: {violations:?}"
    );
}

#[test]
fn multi_line_six_tag_attribute_is_admitted() {
    // THE P-GA-03 REGRESSION (the sharpening this prompt ships): the lint MUST admit the EXACT
    // multi-line, six-tag `#[personal_data(...)]` shape frozen in P-GA-02 / P-050 + gdpr-and-audit
    // §2.1. The P-017 floor (immediately-preceding-line check) rejected this because the line above
    // the field is the attribute's closing `)]`, not the `#[personal_data(` opener. This inline
    // fixture pins the canonical shape so a future "simplification" back to the single-line-only
    // check is caught.
    let src = r#"
pub struct PrincipalRow {
    pub principal_id: PrincipalId,
    #[personal_data(
        category = ContactInfo,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "principal_id",
    )]
    pub email: EncryptedField<Email>,
    pub region: Region,
}
"#;
    let violations = no_untagged_personal_data().run(src);
    assert!(
        violations.is_empty(),
        "the canonical MULTI-LINE six-tag #[personal_data(...)] attribute (§2.1 / P-050) MUST be \
         admitted, but the lint flagged: {violations:?}"
    );
}

#[test]
fn an_untagged_field_after_a_multi_line_tag_still_fires() {
    // Sharpened-not-weakened: the multi-line span must NOT bleed onto the NEXT field. A tagged
    // field followed by an UNtagged PII field must still trip on the second field — the attribute
    // belonged to the first field only.
    let src = r#"
pub struct Row {
    #[personal_data(
        category = ContactInfo,
        subject_locator = "id",
    )]
    pub email: EncryptedField<Email>,
    pub phone: String,
}
"#;
    let violations = no_untagged_personal_data().run(src);
    assert_eq!(
        violations.len(),
        1,
        "exactly the UNtagged `phone` after a multi-line-tagged `email` must fire, got: {violations:?}"
    );
    assert!(
        violations[0].reason.contains("phone"),
        "the violation must name the untagged `phone` field, got: {:?}",
        violations[0]
    );
}

#[test]
fn the_gdpr_red_trips_exactly_its_own_lint() {
    // Cross-lint isolation: the GDPR red fixture must be caught by no-untagged-personal-data and by
    // NO OTHER of the twelve lints, so the whole-set CI gate rejects it for the right reason.
    let red = read_fixture(GDPR_RED);
    let mut firing: Vec<LintId> = Vec::new();
    for lint in all_twelve() {
        if !lint.run(&red).is_empty() {
            firing.push(lint.id);
        }
    }
    assert_eq!(
        firing,
        vec![LintId("no-untagged-personal-data")],
        "the GDPR red fixture must trip exactly no-untagged-personal-data, but tripped: {firing:?}"
    );
}

#[test]
fn the_full_twelve_set_rejects_the_gdpr_red_and_admits_the_gdpr_green() {
    // The set-level gate (the form CI runs): run() over ALL twelve lints is Err on the GDPR red and
    // Ok on the GDPR green (no lint false-positives on the canonical six-tag green).
    let all = all_twelve();
    assert!(
        run(&all, &read_fixture(GDPR_RED)).is_err(),
        "the twelve-lint set must REJECT the GDPR red fixture (untagged PII)"
    );
    assert!(
        run(&all, &read_fixture(GDPR_GREEN)).is_ok(),
        "the twelve-lint set must ADMIT the GDPR green fixture (canonical six-tag tags)"
    );
}

#[test]
fn ci_gate_exits_non_zero_on_the_gdpr_red_fixture_and_zero_on_green() {
    // THE GA-D5 CI-WIRING PROOF (loud, never swallowed — EI-01 §5): the `lint-gate` binary the CI
    // `architecture-lints` job runs exits NON-ZERO over the GDPR red fixture and ZERO over the GDPR
    // green fixture. A process whose exit code IS the gate cannot be `|| true`-swallowed.
    // `--no-exclude` disables the by-design `/fixtures/` exclusion so the fixture is actually scanned.
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
        run_over(GDPR_RED),
        0,
        "lint-gate MUST exit non-zero on the GDPR red fixture (untagged PII fails the build)"
    );
    assert_eq!(
        run_over(GDPR_GREEN),
        0,
        "lint-gate MUST exit zero on the GDPR green fixture (canonical six-tag tags admitted)"
    );
}

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
    let lint = no_untagged_personal_data();
    let violations = lint.run(&read_fixture(GDPR_RED));
    assert!(
        !violations.is_empty(),
        "no-untagged-personal-data MUST reject the GDPR red fixture (untagged PII), but found 0"
    );
    assert!(
        violations
            .iter()
            .all(|v| v.lint == LintId("no-untagged-personal-data")),
        "every GDPR red-fixture violation must carry the no-untagged-personal-data id"
    );
}

#[test]
fn the_gdpr_green_fixture_is_admitted() {
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

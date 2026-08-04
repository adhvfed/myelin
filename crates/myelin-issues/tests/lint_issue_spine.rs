use myelin_lints::lints::{forward_only_migration, no_untagged_personal_data, residency_pin};

#[test]
fn the_lint_admits_the_tagged_issue_schema() {
    let schema_src = include_str!("../src/schema.rs");
    let violations = no_untagged_personal_data().run(schema_src);
    assert!(
        violations.is_empty(),
        "no-untagged-personal-data MUST be GREEN on the Issues schema (every PII field is tagged): {violations:?}"
    );
}

#[test]
fn the_lint_rejects_an_untagged_pii_issue_field() {
    let red = "\
pub struct IssueRowBad {
    pub tenant: TenantId,
    pub region: Region,
    pub id: u128,
    pub email: String,
}";
    let violations = no_untagged_personal_data().run(red);
    assert!(
        !violations.is_empty(),
        "no-untagged-personal-data MUST reject an untagged PII field on an Issues row (the un-erasable-subject bug)"
    );
    assert!(
        violations
            .iter()
            .all(|v| v.lint.0 == "no-untagged-personal-data"),
        "every violation carries the no-untagged-personal-data id (no false attribution)"
    );
}

#[test]
fn the_lint_admits_the_same_field_once_tagged() {
    let green = "\
pub struct IssueRowGood {
    pub tenant: TenantId,
    pub region: Region,
    pub id: u128,
    #[personal_data(
        category = ContactInfo,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = \"reporter_pseudonym\",
    )]
    pub email: String,
}";
    assert!(
        no_untagged_personal_data().run(green).is_empty(),
        "no-untagged-personal-data MUST admit a correctly-tagged Issues PII field (0 false reject)"
    );
}

#[test]
fn residency_pin_is_green_over_the_issue_spine_source() {
    for src in [
        include_str!("../src/migrations.rs"),
        include_str!("../src/holder.rs"),
        include_str!("../src/app.rs"),
        include_str!("../src/schema.rs"),
    ] {
        assert!(
            residency_pin().run(src).is_empty(),
            "residency-pin MUST be GREEN over the Issues spine source (no request-derived region write)"
        );
    }
}

#[test]
fn forward_only_migration_is_green_over_the_issue_migrations() {
    let src = include_str!("../src/migrations.rs");
    assert!(
        forward_only_migration().run(src).is_empty(),
        "forward-only-migration MUST be GREEN over the issue-spine migration source (additive CREATEs only)"
    );
}

#[test]
fn forward_only_migration_holds_under_the_flexible_field_add() {
    let src = include_str!("../src/schemes.rs");
    assert!(
        forward_only_migration().run(src).is_empty(),
        "forward-only-migration MUST be GREEN over the flexible-field model source (zero-DDL - no ALTER on the hot issue table)"
    );
    for line in src.lines() {
        let code = line.trim_start();
        if code.starts_with("//") {
            continue;
        }
        let upper = code.to_ascii_uppercase();
        assert!(
            !upper.contains("ALTER TABLE") && !upper.contains("DROP TABLE"),
            "the flexible-field model performs no DDL in code (a custom field is a JSONB write): {line}"
        );
    }
}

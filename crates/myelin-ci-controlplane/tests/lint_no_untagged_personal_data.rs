use myelin_lints::lints::{forward_only_migration, no_untagged_personal_data, residency_pin};

#[test]
fn the_lint_admits_the_tagged_ci_schema() {
    let schema_src = include_str!("../src/schema.rs");
    let violations = no_untagged_personal_data().run(schema_src);
    assert!(
        violations.is_empty(),
        "no-untagged-personal-data MUST be GREEN on the CI schema (every PII field is tagged): {violations:?}"
    );
}

#[test]
fn the_lint_rejects_an_untagged_pii_ci_field() {
    let red = "\
pub struct CiRunRowBad {
    pub tenant: TenantId,
    pub region: Region,
    pub run_id: u128,
    pub email: String,
}";
    let violations = no_untagged_personal_data().run(red);
    assert!(
        !violations.is_empty(),
        "no-untagged-personal-data MUST reject an untagged PII field on a CI row (the un-erasable-subject bug)"
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
pub struct CiRunRowGood {
    pub tenant: TenantId,
    pub region: Region,
    pub run_id: u128,
    #[personal_data(
        category = ContactInfo,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = \"triggered_by\",
    )]
    pub email: String,
}";
    assert!(
        no_untagged_personal_data().run(green).is_empty(),
        "no-untagged-personal-data MUST admit a correctly-tagged CI PII field (0 false reject)"
    );
}

#[test]
fn residency_pin_is_green_over_the_ci_source() {
    for src in [
        include_str!("../src/migrations.rs"),
        include_str!("../src/schema.rs"),
        include_str!("../src/lib.rs"),
        // CI-P14 (P-357): the fleet's runner-WRITE boundary (the `@residency-write` site). The fleet
        include_str!("../src/fleet.rs"),
    ] {
        assert!(
            residency_pin().run(src).is_empty(),
            "residency-pin MUST be GREEN over the CI control-plane source (no request-derived region write)"
        );
    }
}

#[test]
fn forward_only_migration_is_green_over_the_ci_migrations() {
    let src = include_str!("../src/migrations.rs");
    assert!(
        forward_only_migration().run(src).is_empty(),
        "forward-only-migration MUST be GREEN over the CI migration source (additive CREATEs only)"
    );
}

//! **CI-P6 / P-349 — the CI subsystem's own red+green confirmation of the
//! `no-untagged-personal-data` lint (contract 1.6) over a CI type, plus the residency-pin /
//! forward-only confirmations.**
//!
//! **Reconciliation (coherence rule, EI-01 §7).** The lint + its engine were FIRST shipped by the
//! substrate prompts (P-S10/P-S11). CI-P6 CONFIRMS the gate in place over its OWN schema — the lint
//! home stays `myelin_lints::lints::no_untagged_personal_data`; this file is CI's dated proof that
//! the gate REJECTS an untagged PII field and ADMITS the tagged CI schema. (The shared lints crate
//! keeps its own red fixtures in `tests/fixtures/`; the workspace live scan excludes `crates/*/tests/`
//! so a red sample here does not turn the workspace scan red.)
//!
//! ## Why CI's PII surface is two pseudonym-subject fields, both tagged
//! Per arch 01 §4 ("Identity is referenced, never copied"), the ENTIRE PII surface of the CI schema
//! is `ci_run.triggered_by` + `deployment.approved_by` — opaque pseudonym subjects (contract 4.8),
//! tagged `Identifier`/`Pseudonymise` in `crate::schema`. Inline log PII lives in the log-tier BYTES
//! (per-subject DEK, Storage C1), not in a control-plane column. So the no-untagged lint is GREEN on
//! the CI schema because every PII field is tagged and nothing else is PII.

use myelin_lints::lints::{forward_only_migration, no_untagged_personal_data, residency_pin};

/// **GREEN — the lint ADMITS the real CI schema source (every PII field tagged).** The
/// `crate::schema` module's `#[personal_data(...)]`-tagged `triggered_by`/`approved_by` fields are
/// the CI PII surface; the lint finds 0 untagged PII fields over the real source.
#[test]
fn the_lint_admits_the_tagged_ci_schema() {
    let schema_src = include_str!("../src/schema.rs");
    let violations = no_untagged_personal_data().run(schema_src);
    assert!(
        violations.is_empty(),
        "no-untagged-personal-data MUST be GREEN on the CI schema (every PII field is tagged): {violations:?}"
    );
}

/// **RED — the lint REJECTS a deliberately-untagged PII CI field.** A CI row that adds a
/// PII-fingerprinted column (`email`) WITHOUT the `#[personal_data(...)]` tag is the bug class: an
/// untagged PII column leaves an un-erasable subject (ADR-12). The gate fires (it is not vacuous).
#[test]
fn the_lint_rejects_an_untagged_pii_ci_field() {
    // A deliberately-untagged PII field on a CI-shaped row — the red fingerprint.
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

/// **GREEN — the lint ADMITS the SAME CI field once `#[personal_data(...)]`-tagged.** Tagging the
/// PII field with the canonical six-tag classify-derive helper (the shape `crate::schema` uses) makes
/// the gate pass — proving the lint admits the correctly-tagged form, never a false reject.
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

/// **The residency-pin lint is GREEN over the CI control-plane source (no request-derived region
/// write).** CI-P6 writes no rows (schema only); there is no `req.region`-into-`row.region` write
/// anywhere in the crate, so the residency-pin lint admits the whole source — every future write
/// must pin `row.region == cell.region` (the behaviour bands carry the marked write sites).
#[test]
fn residency_pin_is_green_over_the_ci_source() {
    for src in [
        include_str!("../src/migrations.rs"),
        include_str!("../src/schema.rs"),
        include_str!("../src/lib.rs"),
    ] {
        assert!(
            residency_pin().run(src).is_empty(),
            "residency-pin MUST be GREEN over the CI control-plane source (no request-derived region write)"
        );
    }
}

/// **The forward-only-migration lint is GREEN over the CI migration source (no DROP/down).** The CI
/// migrations are additive CREATEs only; the lint finds no rollback/destructive DDL. (The hot-table
/// half — a blocking ALTER on a declared-hot table — is enforced by the runner at boot, proven in
/// the unit tests; here the source-scan half admits the additive create set.)
#[test]
fn forward_only_migration_is_green_over_the_ci_migrations() {
    let src = include_str!("../src/migrations.rs");
    assert!(
        forward_only_migration().run(src).is_empty(),
        "forward-only-migration MUST be GREEN over the CI migration source (additive CREATEs only)"
    );
}

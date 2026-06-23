//! **ISS-P05 / P-371 — the Issues subsystem's own red+green confirmation of the
//! `no-untagged-personal-data` lint (contract 1.6) over an Issues type, plus the residency-pin /
//! forward-only confirmations over the issue-spine migration source.**
//!
//! **Reconciliation (coherence rule, EI-01 §7).** The lints + their engine were FIRST shipped by the
//! substrate prompts (P-S10/P-S11). ISS-P05 CONFIRMS the gate in place over its OWN schema — the lint
//! home stays `myelin_lints::lints`; this file is Issues' dated proof that the gate REJECTS an
//! untagged PII Issues field and ADMITS the tagged Issues schema, and that the issue-spine migrations
//! are forward-only + residency-pin clean. (The shared lints crate keeps its own red fixtures; the
//! workspace live scan excludes `crates/*/tests/` so a red sample here does not turn the scan red.)

use myelin_lints::lints::{forward_only_migration, no_untagged_personal_data, residency_pin};

/// **GREEN — the lint ADMITS the real Issues schema source (every PII field tagged).** The
/// `crate::schema` module's `#[personal_data(...)]`-tagged pseudonym / free-text / OQ-H-worklog
/// fields are the Issues PII surface; the lint finds 0 untagged PII fields over the real source —
/// green from the first migration (the prompt's "no-untagged lint green from ISS-P05").
#[test]
fn the_lint_admits_the_tagged_issue_schema() {
    let schema_src = include_str!("../src/schema.rs");
    let violations = no_untagged_personal_data().run(schema_src);
    assert!(
        violations.is_empty(),
        "no-untagged-personal-data MUST be GREEN on the Issues schema (every PII field is tagged): {violations:?}"
    );
}

/// **RED — the lint REJECTS a deliberately-untagged PII Issues field.** An issue row that adds a
/// PII-fingerprinted column (`email`) WITHOUT the `#[personal_data(...)]` tag is the bug class: an
/// untagged PII column leaves an un-erasable subject (ADR-12). The gate fires (it is not vacuous).
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

/// **GREEN — the lint ADMITS the SAME Issues field once `#[personal_data(...)]`-tagged.** Tagging the
/// PII field with the canonical six-tag classify-derive helper (the shape `crate::schema` uses) makes
/// the gate pass — proving the lint admits the correctly-tagged form, never a false reject.
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

/// **The residency-pin lint is GREEN over the Issues spine source (no request-derived region
/// write).** ISS-P05 writes no rows (schema only); there is no `req.region`-into-`row.region` write
/// anywhere in the migration/holder/app source, so the residency-pin lint admits the whole source —
/// every future write (ISS-P06+) must pin `row.region == cell.region` (contract 1.6).
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

/// **The forward-only-migration lint is GREEN over the issue-spine migration source (no DROP/down).**
/// The Issues migrations are additive CREATEs only (`IF NOT EXISTS`); the lint finds no
/// rollback/destructive DDL. (The hot-table half — a blocking ALTER on a declared-hot table — is
/// enforced by the runner at boot, proven in `migrations.rs` unit tests; here the source-scan half
/// admits the additive create set.)
#[test]
fn forward_only_migration_is_green_over_the_issue_migrations() {
    let src = include_str!("../src/migrations.rs");
    assert!(
        forward_only_migration().run(src).is_empty(),
        "forward-only-migration MUST be GREEN over the issue-spine migration source (additive CREATEs only)"
    );
}

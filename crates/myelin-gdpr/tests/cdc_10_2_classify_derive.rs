//! # CDC 10.2 — the `#[personal_data(...)]` classify-derive (P-GA-07 → P-107)
//!
//! **Contract:** index row 10.2 (`#[personal_data(...)]` classify-derive — the five-tag
//! classification + the `subject_locator`, emitted into a generated registry the data map walks).
//! The attribute + enum NAMES were frozen at P-GA-02 (a no-op derive); the macro BODY lands here.
//! This is the consumer-driven contract test the coverage scanner (P-S21) reads both halves of:
//!
//! - **provider** = a schema struct that DERIVES `#[derive(PersonalData)]` and tags its PII fields
//!   with the frozen five-tag `#[personal_data(...)]` helper. The derive EMITS one generated
//!   registry entry per tagged field — `PersonalDataField { owning_struct, field, tags }` — exposed
//!   through the generated `HasPersonalData::personal_data_fields()` `&'static` slice. The provider
//!   is the SOURCE of the compile-time PII inventory (gdpr §2.1 / §2.2).
//! - **consumer** = a `data_map`-generator stand-in (the shape the real generator P-GA-09 takes): it
//!   WALKS `personal_data_fields()` over a registered holder and builds the machine-readable
//!   inventory — keying on the field path, reading the five tags + the `subject_locator`, routing
//!   the GD-4 crypto-shred key class + the `SpecialCategory` → DPIA flag (P-GA-08) off the entry.
//!   The consumer NEVER hand-writes the field list — *the map is the derive's output*, so "we forgot
//!   a field" is structurally impossible.
//!
//! The dated green artifact: the consumer reconstructs, from the derive's emitted registry alone, a
//! complete inventory of the provider's PII fields with every tag + locator + routing decision —
//! and an untagged PII field could not have compiled into the provider in the first place (the
//! derive's compile-time rejection, the `compile_fail` doc-test in the crate). If 10.2's registry
//! shape drifts, this stops compiling/passing — that is the contract.

use myelin_gdpr::{
    ErasureKeyClass, HasPersonalData, PersonalData, PersonalDataField, SpecialCategoryFlag,
};
use std::collections::BTreeMap;

// ── The PROVIDER side (10.2): a schema struct whose derive EMITS the registry ──────────────────

/// A provider schema row deriving the classify-derive — the same shape an M1 store (e.g.
/// `PrincipalProfile`) carries. Its tagged fields are the source the derive emits registry entries
/// from; the untagged non-PII `row_version` correctly emits nothing.
#[derive(PersonalData)]
#[allow(dead_code)]
struct ProfileRow {
    /// contact PII, per-subject crypto-shred (the GD-4 individual erasure lever).
    #[personal_data(
        category = ContactInfo,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = CryptoShred(subject_dek),
        subject_locator = "principal_id"
    )]
    email: String,
    /// an Art. 9 special-category field — the DPIA route (P-GA-08 consumes the flag).
    #[personal_data(
        category = SpecialCategory(health),
        role = PlatformOperational,
        basis = Consent(c-7),
        retention = Fixed(365d),
        erasure = CryptoShred(subject_dek),
        subject_locator = "principal_id"
    )]
    health_note: String,
    /// a non-PII operational column — carries no personal data, so no registry entry.
    row_version: u64,
}

// ── The CONSUMER side (10.2): the data-map-generator shape that WALKS the emitted registry ──────

/// One reconstructed inventory line (the shape the real `data_map()` generator P-GA-09 emits per
/// PII field). Built ENTIRELY from the derive's emitted registry — never hand-written.
#[derive(Debug, PartialEq, Eq)]
struct InventoryLine {
    field_path: String,
    category: String,
    erasure_key_class: Option<ErasureKeyClass>,
    special_category: Option<SpecialCategoryFlag>,
    subject_locator: String,
}

/// **The CONSUMER: a `data_map` generator stand-in.** It walks one holder type's
/// `personal_data_fields()` and builds the inventory — the exact shape P-GA-09 takes when it unions
/// this over every registered holder. It depends ONLY on the registry the derive emits.
fn generate_inventory<T: HasPersonalData>() -> BTreeMap<String, InventoryLine> {
    T::personal_data_fields()
        .iter()
        .map(|f: &PersonalDataField| {
            let field_path = format!("{}.{}", f.owning_struct, f.field);
            (
                field_path.clone(),
                InventoryLine {
                    field_path,
                    category: f.tags.category.to_string(),
                    erasure_key_class: f.erasure_key_class(),
                    special_category: f.is_special_category(),
                    subject_locator: f.tags.subject_locator.to_string(),
                },
            )
        })
        .collect()
}

/// The provider+consumer round-trip: the derive emits the registry; the generator reconstructs the
/// complete inventory from it; the inventory matches the provider's tags field-for-field.
#[test]
fn classify_derive_provider_emits_the_registry_the_data_map_consumer_walks() {
    let inventory = generate_inventory::<ProfileRow>();

    // Exactly the two TAGGED fields appear (the non-PII `row_version` emits no entry) — the map is
    // the derive's output, complete and no more.
    assert_eq!(inventory.len(), 2);
    assert!(inventory.contains_key("ProfileRow.email"));
    assert!(inventory.contains_key("ProfileRow.health_note"));
    assert!(!inventory.contains_key("ProfileRow.row_version"));

    // The email line: the GD-4 per-subject DEK key class is structural off the emitted tag.
    let email = &inventory["ProfileRow.email"];
    assert_eq!(
        *email,
        InventoryLine {
            field_path: "ProfileRow.email".into(),
            category: "ContactInfo".into(),
            erasure_key_class: Some(ErasureKeyClass::SubjectDek),
            special_category: None,
            subject_locator: "principal_id".into(),
        }
    );

    // The health line: the `SpecialCategory(health)` → DPIA flag is structural (P-GA-08 routes it).
    let health = &inventory["ProfileRow.health_note"];
    assert_eq!(
        health.special_category,
        Some(SpecialCategoryFlag { kind: "health" })
    );
    assert_eq!(health.erasure_key_class, Some(ErasureKeyClass::SubjectDek));

    // `subject_locator` is structural — the generator reads the same locator a holder's `locate`
    // uses (the provider's emitted accessor, consumed here).
    assert_eq!(
        ProfileRow::subject_locator("email"),
        Some("principal_id"),
        "the derive's structural subject_locator accessor (provider) resolves the column the \
         data-map consumer keys on"
    );
}

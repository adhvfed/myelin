use myelin_gdpr::{
    ErasureKeyClass, HasPersonalData, PersonalData, PersonalDataField, SpecialCategoryFlag,
};
use std::collections::BTreeMap;

#[derive(PersonalData)]
#[allow(dead_code)]
struct ProfileRow {
    #[personal_data(
        category = ContactInfo,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = CryptoShred(subject_dek),
        subject_locator = "principal_id"
    )]
    email: String,
    #[personal_data(
        category = SpecialCategory(health),
        role = PlatformOperational,
        basis = Consent(c-7),
        retention = Fixed(365d),
        erasure = CryptoShred(subject_dek),
        subject_locator = "principal_id"
    )]
    health_note: String,
    row_version: u64,
}

#[derive(Debug, PartialEq, Eq)]
struct InventoryLine {
    field_path: String,
    category: String,
    erasure_key_class: Option<ErasureKeyClass>,
    special_category: Option<SpecialCategoryFlag>,
    subject_locator: String,
}

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

#[test]
fn classify_derive_provider_emits_the_registry_the_data_map_consumer_walks() {
    let inventory = generate_inventory::<ProfileRow>();

    assert_eq!(inventory.len(), 2);
    assert!(inventory.contains_key("ProfileRow.email"));
    assert!(inventory.contains_key("ProfileRow.health_note"));
    assert!(!inventory.contains_key("ProfileRow.row_version"));

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

    let health = &inventory["ProfileRow.health_note"];
    assert_eq!(
        health.special_category,
        Some(SpecialCategoryFlag { kind: "health" })
    );
    assert_eq!(health.erasure_key_class, Some(ErasureKeyClass::SubjectDek));

    assert_eq!(
        ProfileRow::subject_locator("email"),
        Some("principal_id"),
        "the derive's structural subject_locator accessor (provider) resolves the column the \
         data-map consumer keys on"
    );
}

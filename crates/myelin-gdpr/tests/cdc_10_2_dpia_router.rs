use myelin_gdpr::{
    dpia_markers, DpiaMarker, DpiaRouter, DpiaVerdict, HasPersonalData, PersonalData,
};
use std::collections::BTreeSet;

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
}

#[test]
fn dpia_provider_emits_the_marker_the_router_consumes() {
    let current: BTreeSet<DpiaMarker> = dpia_markers::<ProfileRow>();
    assert_eq!(
        current.len(),
        1,
        "exactly the one special-category field emits a marker"
    );
    let marker = current.iter().next().unwrap();
    assert_eq!(marker.field_path, "ProfileRow.health_note");
    assert_eq!(marker.special_category_kind, "health");

    let router = DpiaRouter::new();
    let verdicts = router.route(&BTreeSet::new(), &current);
    assert_eq!(
        verdicts.len(),
        1,
        "the new special-category flow fires the DPIA gate"
    );
    match &verdicts[0] {
        DpiaVerdict::Required { marker, reason } => {
            assert_eq!(marker.field_path, "ProfileRow.health_note");
            assert!(reason.contains("DPIA required"));
            assert!(reason.contains("DPO"));
        }
    }

    assert!(router.route(&current, &current).is_empty());

    let special_count = ProfileRow::personal_data_fields()
        .iter()
        .filter(|f| f.is_special_category().is_some())
        .count();
    assert_eq!(current.len(), special_count);
}

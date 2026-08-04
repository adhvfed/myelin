use myelin_gdpr::{DpiaVerdict, PersonalData};
use myelin_gdpr_service::{
    check_against_baseline, data_map, CommittedBaseline, GateVerdict, HolderSchema,
};
use myelin_substrate::{Holder, HolderRegistration, StoreKind};
use myelin_tenancy::Region;

#[derive(PersonalData)]
#[allow(dead_code)]
struct PrincipalRow {
    #[personal_data(
        category = ContactInfo,
        role = TenantContent,
        basis = Contract,
        retention = UntilContractEnd,
        erasure = CryptoShred(subject_dek),
        subject_locator = "principal_id"
    )]
    email: String,
    row_version: u64,
}

#[derive(PersonalData)]
#[allow(dead_code)]
struct ProfileRow {
    #[personal_data(
        category = SpecialCategory(health),
        role = PlatformOperational,
        basis = Consent(c-1),
        retention = Fixed(365d),
        erasure = CryptoShred(subject_dek),
        subject_locator = "principal_id"
    )]
    health_note: String,
}

fn region() -> Region {
    Region("fr-par".into())
}

fn principal() -> HolderSchema {
    HolderSchema::from_schema::<PrincipalRow>(
        HolderRegistration {
            kind: StoreKind::Oltp,
            name: "identity_oltp",
        },
        Holder::H15Identity,
        region(),
    )
}

fn profile() -> HolderSchema {
    HolderSchema::from_schema::<ProfileRow>(
        HolderRegistration {
            kind: StoreKind::Oltp,
            name: "profile_oltp",
        },
        Holder::H15Identity,
        region(),
    )
}

#[test]
fn cdc_10_3_diff_gate_passes_unchanged_and_fails_a_changed_map_with_the_diff() {
    let baseline = CommittedBaseline::seal(data_map(&[principal()]));
    assert!(baseline.is_self_consistent());

    assert_eq!(
        check_against_baseline(&baseline, &data_map(&[principal()])),
        GateVerdict::Unchanged,
    );

    let current = data_map(&[principal(), profile()]);
    let verdict = check_against_baseline(&baseline, &current);
    assert!(!verdict.is_green(), "a changed map fails the gate");
    let d = verdict.diff().expect("the diff is surfaced for a DPO");

    assert_eq!(
        d.added_fields
            .iter()
            .map(|e| e.field_path.as_str())
            .collect::<Vec<_>>(),
        vec!["ProfileRow.health_note"],
    );
    assert_eq!(d.added_holders, vec!["oltp:profile_oltp".to_string()]);

    assert!(d.requires_dpia());
    assert!(matches!(
        &d.dpia_verdicts[0],
        DpiaVerdict::Required { marker, .. } if marker.field_path == "ProfileRow.health_note"
    ));
    assert!(d
        .summary()
        .contains("! DPIA REQUIRED ProfileRow.health_note"));
}

#[test]
fn cdc_10_3_re_sealing_after_dpo_review_makes_the_gate_green() {
    let baseline = CommittedBaseline::seal(data_map(&[principal()]));
    let changed = data_map(&[principal(), profile()]);
    assert!(!check_against_baseline(&baseline, &changed).is_green());

    let re_sealed = CommittedBaseline::seal(changed.clone());
    assert_eq!(
        check_against_baseline(&re_sealed, &changed),
        GateVerdict::Unchanged
    );
}

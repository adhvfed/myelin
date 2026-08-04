use myelin_gdpr::{DataRoleDefault, HasPersonalData, PersonalData, PersonalDataField};

#[derive(PersonalData)]
#[allow(dead_code)]
struct IssueRow {
    #[personal_data(
        category = Behavioural,
        role = TenantContent,
        basis = TBD_LEGAL,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "created_by_pseudonym",
        data_role_default = Restricted
    )]
    worklog_seconds: i64,
    #[personal_data(
        category = Content,
        role = TenantContent,
        basis = Contract,
        retention = TenantPolicy,
        erasure = CryptoShred(subject_dek),
        subject_locator = "created_by_pseudonym"
    )]
    title: String,
    row_version: u64,
}

fn cross_individual_allowed(field: &PersonalDataField, subject_opted_in: bool) -> bool {
    match field.data_role_default() {
        DataRoleDefault::Restricted => subject_opted_in,
        DataRoleDefault::Default => true,
    }
}

fn worklog_field() -> &'static PersonalDataField {
    IssueRow::personal_data_fields()
        .iter()
        .find(|f| f.field == "worklog_seconds")
        .expect("worklog_seconds is tagged")
}

fn title_field() -> &'static PersonalDataField {
    IssueRow::personal_data_fields()
        .iter()
        .find(|f| f.field == "title")
        .expect("title is tagged")
}

#[test]
fn provider_emits_and_consumer_reads_the_restricted_by_default_worklog_tag() {
    let w = worklog_field();
    assert_eq!(w.tags.data_role_default, "Restricted");
    assert_eq!(w.data_role_default(), DataRoleDefault::Restricted);
    assert!(w.is_restricted_by_default());
    assert!(w.is_behavioural(), "the worklog field is Behavioural");

    let t = title_field();
    assert_eq!(t.tags.data_role_default, "Default");
    assert_eq!(t.data_role_default(), DataRoleDefault::Default);
    assert!(!t.is_restricted_by_default());
}

#[test]
fn consumer_applies_the_default_deny_for_a_restricted_by_default_field() {
    let w = worklog_field();
    assert!(
        !cross_individual_allowed(w, false),
        "worklog denied by default (§2.4)"
    );
    assert!(
        cross_individual_allowed(w, true),
        "explicit opt-in lifts the deny"
    );

    let t = title_field();
    assert!(cross_individual_allowed(t, false));
    assert!(cross_individual_allowed(t, true));
}

#[test]
fn the_worklog_registry_entry_round_trips_with_the_data_role_default_tag() {
    let w = worklog_field();
    let json = serde_json::to_string(w).expect("serialize");
    assert!(
        json.contains("\"data_role_default\":\"Restricted\""),
        "the worklog tag serialises into the registry entry"
    );

    const NEW_ENTRY: &str = r#"{"owning_struct":"S","field":"f","tags":{"category":"Behavioural","role":"TenantContent","basis":"TBD_LEGAL","retention":"TenantPolicy","erasure":"CryptoShred(subject_dek)","subject_locator":"id","data_role_default":"Restricted"}}"#;
    let back: PersonalDataField = serde_json::from_str(NEW_ENTRY).expect("deserialize");
    assert_eq!(back.data_role_default(), DataRoleDefault::Restricted);

    const LEGACY: &str = r#"{"owning_struct":"S","field":"f","tags":{"category":"Content","role":"TenantContent","basis":"Contract","retention":"TenantPolicy","erasure":"CryptoShred(subject_dek)","subject_locator":"id"}}"#;
    let legacy_field: PersonalDataField = serde_json::from_str(LEGACY).expect("legacy deserialize");
    assert_eq!(
        legacy_field.data_role_default(),
        DataRoleDefault::Default,
        "a pre-P-GA-31 registry entry round-trips to Default (additive extension)"
    );
}

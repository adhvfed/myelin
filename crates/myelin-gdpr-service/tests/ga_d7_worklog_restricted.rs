use myelin_gdpr::{DpiaRouter, HasPersonalData, PersonalDataField};
use myelin_gdpr_service::worklog::{RollupEnablement, WorklogAnalyticsGate};
use myelin_issues::schema::Issue;

fn issue_field(name: &str) -> &'static PersonalDataField {
    Issue::personal_data_fields()
        .iter()
        .find(|f| f.field == name)
        .unwrap_or_else(|| panic!("Issue field `{name}` is tagged"))
}

#[test]
fn the_live_issues_worklog_fields_are_excluded_from_cross_individual_analytics_by_default() {
    let gate = WorklogAnalyticsGate::new();

    let restricted = WorklogAnalyticsGate::restricted_by_default_fields::<Issue>();
    let names: Vec<&str> = restricted.iter().map(|f| f.field).collect();
    assert!(
        names.contains(&"worklog_seconds") && names.contains(&"story_points"),
        "the live Issues worklog + story_points fields are restricted-by-default, got {names:?}"
    );

    let denied_without_optin = restricted
        .iter()
        .filter(|f| !gate.cross_individual_allowed(f, false))
        .count();
    assert_eq!(
        denied_without_optin,
        restricted.len(),
        "every restricted-by-default worklog field is DENIED cross-individual analytics by default"
    );

    let w = issue_field("worklog_seconds");
    assert!(
        gate.cross_individual_allowed(w, true),
        "an explicit per-subject opt-in lifts the default-deny"
    );

    let title = issue_field("title");
    assert!(
        gate.cross_individual_allowed(title, false),
        "an ordinary Content field is not subject to the OQ-H default-deny"
    );
}

#[test]
fn enabling_a_per_individual_rollup_surfaces_the_works_council_trigger() {
    let mut rollups = RollupEnablement::new();

    assert!(!rollups.is_enabled("acme", "per_person_velocity"));
    assert!(rollups.surfaced_triggers().is_empty());

    let trigger = rollups.enable("acme", "per_person_velocity");
    assert!(rollups.is_enabled("acme", "per_person_velocity"));
    assert!(
        trigger.reason.contains("works-council") && trigger.reason.contains("NOT auto-decided"),
        "the works-council consultation is surfaced as an obligation, not adjudicated"
    );
    assert_eq!(
        rollups.surfaced_triggers().len(),
        1,
        "the surfaced obligation is recorded (the green artifact)"
    );

    rollups.disable("acme", "per_person_velocity");
    assert_eq!(rollups.surfaced_triggers().len(), 1);
}

#[test]
fn a_special_category_worklog_field_routes_into_the_dpia_gate() {
    use myelin_gdpr::PersonalData;
    use std::collections::BTreeSet;

    #[derive(PersonalData)]
    #[allow(dead_code)]
    struct SensitiveWorklogRow {
        #[personal_data(
            category = SpecialCategory(health),
            role = TenantContent,
            basis = TBD_LEGAL,
            retention = TenantPolicy,
            erasure = CryptoShred(subject_dek),
            subject_locator = "created_by_pseudonym",
            data_role_default = Restricted
        )]
        sensitive_worklog: f64,
    }

    let markers = myelin_gdpr::dpia_markers::<SensitiveWorklogRow>();
    assert_eq!(
        markers.len(),
        1,
        "the special-category worklog field emits a DPIA marker"
    );

    let router = DpiaRouter::new();
    let verdicts = router.route(&BTreeSet::new(), &markers);
    assert_eq!(
        verdicts.len(),
        1,
        "the new special-category worklog flow fires the DPIA gate"
    );
    assert_eq!(
        verdicts[0].field_path(),
        "SensitiveWorklogRow.sensitive_worklog"
    );

    let f = SensitiveWorklogRow::personal_data_fields()[0];
    assert!(
        f.is_restricted_by_default(),
        "the special-category worklog field is restricted-by-default"
    );
}

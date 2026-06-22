//! # GA-D7 (worklog leg) — a restricted-by-default worklog field is excluded from cross-individual
//! analytics + the works-council trigger is surfaced on a rollup enablement (P-GA-31 → P-334)
//!
//! The SCHED drill for the worklog leg of GA-D7 (gdpr §2.4 OQ-H). It observes the §2.4 posture
//! end-to-end over the real classify-derive registry + the `myelin-gdpr-service` worklog gate:
//! 1. a restricted-by-default worklog field is EXCLUDED from cross-individual analytics by default
//!    (0 cross-individual analytics for a restricted subject unless an explicit opt-in is recorded);
//! 2. enabling a per-individual productivity rollup SURFACES the works-council consultation trigger
//!    (a surfaced obligation, never an auto-decision — §8);
//! 3. a SpecialCategory worklog field routes into the DPIA gate (reusing P-GA-08).
//!
//! The green artifact: the worklog field's restricted-by-default fact is read off the data map and
//! the default-deny is OBSERVED (0 cross-individual analytics), the surfaced trigger count is
//! recorded, and the DPIA route fires — all over the LIVE Issues schema tags.

use myelin_gdpr::{DpiaRouter, HasPersonalData, PersonalDataField};
use myelin_gdpr_service::worklog::{RollupEnablement, WorklogAnalyticsGate};
use myelin_issues::schema::Issue;

fn issue_field(name: &str) -> &'static PersonalDataField {
    Issue::personal_data_fields()
        .iter()
        .find(|f| f.field == name)
        .unwrap_or_else(|| panic!("Issue field `{name}` is tagged"))
}

/// **GA-D7 worklog face: the LIVE Issues `worklog_seconds` / `story_points` fields are
/// restricted-by-default and excluded from cross-individual analytics (gdpr §2.4).** Read off the
/// live `myelin-issues` schema registry — not a fixture — so the drill proves the real tag.
#[test]
fn the_live_issues_worklog_fields_are_excluded_from_cross_individual_analytics_by_default() {
    let gate = WorklogAnalyticsGate::new();

    // The real worklog fields carry the OQ-H restricted-by-default tag.
    let restricted = WorklogAnalyticsGate::restricted_by_default_fields::<Issue>();
    let names: Vec<&str> = restricted.iter().map(|f| f.field).collect();
    assert!(
        names.contains(&"worklog_seconds") && names.contains(&"story_points"),
        "the live Issues worklog + story_points fields are restricted-by-default, got {names:?}"
    );

    // GA-D7 reading: 0 cross-individual analytics for a restricted subject (no opt-in) across the
    // worklog fields — observed, not asserted.
    let denied_without_optin = restricted
        .iter()
        .filter(|f| !gate.cross_individual_allowed(f, false))
        .count();
    assert_eq!(
        denied_without_optin,
        restricted.len(),
        "every restricted-by-default worklog field is DENIED cross-individual analytics by default"
    );

    // With an explicit per-subject opt-in, the worklog field is admitted (the tenant-admin override).
    let w = issue_field("worklog_seconds");
    assert!(
        gate.cross_individual_allowed(w, true),
        "an explicit per-subject opt-in lifts the default-deny"
    );

    // An ordinary Issues field (title — Content) is NOT restricted by this gate (the OQ-H default
    // applies only to the worklog class).
    let title = issue_field("title");
    assert!(
        gate.cross_individual_allowed(title, false),
        "an ordinary Content field is not subject to the OQ-H default-deny"
    );
}

/// **GA-D7 worklog face: enabling a per-individual productivity rollup surfaces the works-council
/// consultation trigger (gdpr §2.4 / §8) — surfaced, not auto-decided.** Per-individual rollups are
/// OFF by default; enabling one records a surfaced obligation that is NEVER auto-cleared.
#[test]
fn enabling_a_per_individual_rollup_surfaces_the_works_council_trigger() {
    let mut rollups = RollupEnablement::new();

    // OFF by default — no rollup, no obligation.
    assert!(!rollups.is_enabled("acme", "per_person_velocity"));
    assert!(rollups.surfaced_triggers().is_empty());

    // Enable the rollup — the trigger is surfaced (the obligation).
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

    // Disabling does not unraise the historical obligation (append-only audit trail).
    rollups.disable("acme", "per_person_velocity");
    assert_eq!(rollups.surfaced_triggers().len(), 1);
}

/// **GA-D7 worklog face: a SpecialCategory worklog field routes into the DPIA gate (gdpr §2.3 / §2.4)
/// — reusing P-GA-08.** A worklog metric promoted to special-category fires the DPIA-required verdict
/// (surfaced for a DPO). The live Issues schema has no special-category worklog field yet, so this
/// drills the route over a representative special-category field, proving the worklog → DPIA seam.
#[test]
fn a_special_category_worklog_field_routes_into_the_dpia_gate() {
    use myelin_gdpr::PersonalData;
    use std::collections::BTreeSet;

    #[derive(PersonalData)]
    #[allow(dead_code)]
    struct SensitiveWorklogRow {
        // a worklog metric that became special-category (e.g. health-adjacent productivity) — the
        // DPIA route, restricted-by-default like the rest of the worklog class.
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

    // And the field is ALSO restricted-by-default (the worklog posture stacks with the DPIA route).
    let f = SensitiveWorklogRow::personal_data_fields()[0];
    assert!(
        f.is_restricted_by_default(),
        "the special-category worklog field is restricted-by-default"
    );
}

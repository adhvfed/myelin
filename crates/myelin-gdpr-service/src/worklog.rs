use std::collections::BTreeSet;

use myelin_gdpr::{DataRoleDefault, HasPersonalData, PersonalDataField};

pub const WORKLOG_CROSS_INDIVIDUAL_DENIED: (&str, &str) =
    ("gdpr.worklog_cross_individual_denied", "count");

pub const WORKS_COUNCIL_TRIGGERS_SURFACED: (&str, &str) =
    ("gdpr.works_council_triggers_surfaced", "count");

#[derive(Clone, Copy, Debug, Default)]
pub struct WorklogAnalyticsGate;

impl WorklogAnalyticsGate {
    #[must_use]
    pub fn new() -> WorklogAnalyticsGate {
        WorklogAnalyticsGate
    }

    #[must_use]
    pub fn cross_individual_allowed(
        &self,
        field: &PersonalDataField,
        subject_opted_in: bool,
    ) -> bool {
        match field.data_role_default() {
            DataRoleDefault::Restricted => subject_opted_in,
            DataRoleDefault::Default => true,
        }
    }

    #[must_use]
    pub fn restricted_by_default_fields<T: HasPersonalData>() -> Vec<&'static PersonalDataField> {
        T::personal_data_fields()
            .iter()
            .filter(|f| f.is_restricted_by_default())
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct WorksCouncilTrigger {
    pub tenant_token: String,
    pub rollup_id: String,
    pub reason: String,
}

impl WorksCouncilTrigger {
    fn for_rollup(tenant_token: &str, rollup_id: &str) -> WorksCouncilTrigger {
        WorksCouncilTrigger {
            tenant_token: tenant_token.to_string(),
            rollup_id: rollup_id.to_string(),
            reason: format!(
                "[OPEN - LEGAL] enabling per-individual productivity rollup `{rollup_id}` may \
                 require works-council consultation in applicable jurisdictions - surfaced for the \
                 tenant DPO / works-council, NOT auto-decided (gdpr §2.4 OQ-H)"
            ),
        }
    }
}

#[derive(Debug, Default)]
pub struct RollupEnablement {
    enabled: BTreeSet<(String, String)>,
    surfaced_triggers: Vec<WorksCouncilTrigger>,
}

impl RollupEnablement {
    #[must_use]
    pub fn new() -> RollupEnablement {
        RollupEnablement::default()
    }

    #[must_use]
    pub fn is_enabled(&self, tenant_token: &str, rollup_id: &str) -> bool {
        self.enabled
            .contains(&(tenant_token.to_string(), rollup_id.to_string()))
    }

    pub fn enable(&mut self, tenant_token: &str, rollup_id: &str) -> WorksCouncilTrigger {
        self.enabled
            .insert((tenant_token.to_string(), rollup_id.to_string()));
        let trigger = WorksCouncilTrigger::for_rollup(tenant_token, rollup_id);
        self.surfaced_triggers.push(trigger.clone());
        trigger
    }

    pub fn disable(&mut self, tenant_token: &str, rollup_id: &str) -> bool {
        self.enabled
            .remove(&(tenant_token.to_string(), rollup_id.to_string()))
    }

    #[must_use]
    pub fn surfaced_triggers(&self) -> &[WorksCouncilTrigger] {
        &self.surfaced_triggers
    }
}

pub const BUILD_TRAINING_FORECLOSURE: &str =
    "build-data-as-LLM-training foreclosed by default (gdpr §2.4 / AG-8) - no platform code path \
     feeds tenant content to model training; training-on-tenant-data is a separately-ratified \
     opt-in (a region-aware EU-hostable sub-processor, ADR-12.8), never a default";

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_gdpr::{DpiaMarker, DpiaRouter, DpiaVerdict, PersonalData};
    use std::collections::BTreeSet;

    #[derive(PersonalData)]
    #[allow(dead_code)]
    struct WorklogRow {
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
        #[personal_data(
            category = SpecialCategory(health),
            role = TenantContent,
            basis = TBD_LEGAL,
            retention = TenantPolicy,
            erasure = CryptoShred(subject_dek),
            subject_locator = "created_by_pseudonym",
            data_role_default = Restricted
        )]
        sensitive_metric: f64,
        row_version: u64,
    }

    fn worklog_field() -> &'static PersonalDataField {
        WorklogRow::personal_data_fields()
            .iter()
            .find(|f| f.field == "worklog_seconds")
            .expect("worklog_seconds is tagged")
    }

    fn title_field() -> &'static PersonalDataField {
        WorklogRow::personal_data_fields()
            .iter()
            .find(|f| f.field == "title")
            .expect("title is tagged")
    }

    #[test]
    fn worklog_field_carries_the_behavioural_restricted_by_default_per_subject_dek_tags() {
        let w = worklog_field();
        assert!(w.is_behavioural(), "worklog is category = Behavioural");
        assert!(
            w.is_restricted_by_default(),
            "worklog is data_role_default = Restricted (OQ-H)"
        );
        assert_eq!(
            w.data_role_default(),
            DataRoleDefault::Restricted,
            "the structural tag is read off the map"
        );
        assert_eq!(
            w.erasure_key_class(),
            Some(myelin_gdpr::ErasureKeyClass::SubjectDek),
            "worklog carries the same per-subject DEK crypto-shred"
        );

        let t = title_field();
        assert!(
            !t.is_restricted_by_default(),
            "an ordinary field is Default"
        );
        assert_eq!(t.data_role_default(), DataRoleDefault::Default);
        assert!(!t.is_behavioural(), "Content is not Behavioural");
    }

    #[test]
    fn the_gate_reads_the_restricted_by_default_field_set_off_the_map() {
        let fields = WorklogAnalyticsGate::restricted_by_default_fields::<WorklogRow>();
        let names: BTreeSet<&str> = fields.iter().map(|f| f.field).collect();
        assert_eq!(
            names,
            ["sensitive_metric", "worklog_seconds"]
                .into_iter()
                .collect(),
            "the map drives the restricted-by-default set (worklog + the special-category metric)"
        );
        assert!(
            !names.contains("title"),
            "the ordinary Content field is not in the restricted set"
        );
    }

    #[test]
    fn restricted_by_default_worklog_is_excluded_from_cross_individual_analytics_unless_opted_in() {
        let gate = WorklogAnalyticsGate::new();
        let w = worklog_field();

        assert!(
            !gate.cross_individual_allowed(w, false),
            "a restricted-by-default worklog field is DENIED cross-individual analytics by default"
        );
        assert!(
            gate.cross_individual_allowed(w, true),
            "an explicit per-subject opt-in lifts the default-deny"
        );

        let t = title_field();
        assert!(
            gate.cross_individual_allowed(t, false),
            "an ordinary field is allowed by the OQ-H gate (no default-deny)"
        );
        assert!(gate.cross_individual_allowed(t, true));
    }

    #[test]
    fn per_individual_rollups_are_off_by_default() {
        let rollups = RollupEnablement::new();
        assert!(
            !rollups.is_enabled("acme", "team_velocity"),
            "per-individual rollups are OFF by default (OQ-H)"
        );
        assert!(
            rollups.surfaced_triggers().is_empty(),
            "no consultation obligation until a rollup is explicitly enabled"
        );
    }

    #[test]
    fn enabling_a_rollup_surfaces_the_works_council_trigger_without_auto_deciding() {
        let mut rollups = RollupEnablement::new();

        let trigger = rollups.enable("acme", "team_velocity");
        assert!(rollups.is_enabled("acme", "team_velocity"));
        assert_eq!(trigger.tenant_token, "acme");
        assert_eq!(trigger.rollup_id, "team_velocity");
        assert!(
            trigger.reason.contains("works-council"),
            "the surfaced obligation names the works-council consultation"
        );
        assert!(
            trigger.reason.contains("OPEN - LEGAL") && trigger.reason.contains("NOT auto-decided"),
            "the trigger is surfaced, not auto-decided (§8)"
        );
        assert_eq!(
            rollups.surfaced_triggers().len(),
            1,
            "the surfaced obligation is recorded once"
        );

        assert!(rollups.disable("acme", "team_velocity"));
        assert!(
            !rollups.is_enabled("acme", "team_velocity"),
            "rollup is OFF again"
        );
        assert_eq!(
            rollups.surfaced_triggers().len(),
            1,
            "the historical consultation obligation is RETAINED (append-only audit trail)"
        );
    }

    #[test]
    fn rollup_enablement_is_keyed_per_tenant_and_rollup() {
        let mut rollups = RollupEnablement::new();
        rollups.enable("acme", "team_velocity");
        assert!(rollups.is_enabled("acme", "team_velocity"));
        assert!(!rollups.is_enabled("globex", "team_velocity"));
        assert!(!rollups.is_enabled("acme", "sprint_burndown"));
    }

    #[test]
    fn a_special_category_worklog_field_routes_into_the_dpia_gate() {
        let markers: BTreeSet<DpiaMarker> = myelin_gdpr::dpia_markers::<WorklogRow>();
        assert_eq!(
            markers.len(),
            1,
            "exactly the special-category worklog field emits a DPIA marker"
        );
        let marker = markers.iter().next().unwrap();
        assert_eq!(marker.field_path, "WorklogRow.sensitive_metric");
        assert_eq!(marker.special_category_kind, "health");

        let router = DpiaRouter::new();
        let verdicts = router.route(&BTreeSet::new(), &markers);
        assert_eq!(
            verdicts.len(),
            1,
            "a new special-category worklog flow fires the DPIA gate"
        );
        match &verdicts[0] {
            DpiaVerdict::Required { marker, reason } => {
                assert_eq!(marker.field_path, "WorklogRow.sensitive_metric");
                assert!(
                    reason.contains("DPO"),
                    "surfaced for a DPO, not auto-decided"
                );
            }
        }
    }

    #[test]
    fn build_data_as_llm_training_is_foreclosed_by_policy() {
        assert!(
            BUILD_TRAINING_FORECLOSURE.contains("foreclosed by default")
                && BUILD_TRAINING_FORECLOSURE.contains("separately-ratified opt-in"),
            "the foreclosure is documented: no default training-feed path"
        );
    }

}

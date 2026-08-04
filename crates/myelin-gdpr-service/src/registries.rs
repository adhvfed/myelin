use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_gdpr::{EraseScope, SubjectRef, TenantId};
use myelin_tenancy::Region;

pub const TRANSFER_GATE_EXTRA_EU_DENIALS: (&str, &str) =
    ("gdpr.transfer_gate_extra_eu_denials", "count");

pub const CONSENT_WITHDRAWALS: (&str, &str) = ("gdpr.consent_withdrawals", "count");

pub const SUBPROCESSOR_OBJECTIONS: (&str, &str) = ("gdpr.subprocessor_objections", "count");

const EEA_AREAS: &[&str] = &[
    "at", "be", "bg", "hr", "cy", "cz", "dk", "ee", "fi", "fr", "de", "gr", "hu", "ie", "it", "lv",
    "lt", "lu", "mt", "nl", "pl", "pt", "ro", "sk", "si", "es", "se",
    "is", "li", "no",
];

pub fn is_eea_region(region: &Region) -> bool {
    let code = region.as_str();
    let area = code.split('-').next().unwrap_or(code).to_ascii_lowercase();
    EEA_AREAS.contains(&area.as_str())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsentRecord {
    pub subject_token: String,
    pub tenant_token: String,
    pub activity: String,
    pub version: u64,
    pub in_force: bool,
    pub recorded_at_secs: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WithdrawalEffect {
    StoppedOnly,
    StoppedAndTriggersDeletion(EraseScope),
}

impl WithdrawalEffect {
    pub fn triggers_deletion(&self) -> bool {
        matches!(self, WithdrawalEffect::StoppedAndTriggersDeletion(_))
    }
}

#[derive(Default)]
pub struct ConsentRegistry {
    current: Mutex<BTreeMap<(String, String, String), ConsentRecord>>,
    history: Mutex<Vec<ConsentRecord>>,
    withdrawals: Mutex<u64>,
}

impl ConsentRegistry {
    pub fn new() -> ConsentRegistry {
        ConsentRegistry::default()
    }

    pub fn record(
        &self,
        subject: &SubjectRef,
        tenant: &TenantId,
        activity: &str,
        at_secs: u64,
    ) -> u64 {
        let key = (
            subject.principal.principal_id.0.clone(),
            tenant.0.clone(),
            activity.to_string(),
        );
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        let version = current.get(&key).map(|r| r.version + 1).unwrap_or(1);
        let record = ConsentRecord {
            subject_token: key.0.clone(),
            tenant_token: key.1.clone(),
            activity: key.2.clone(),
            version,
            in_force: true,
            recorded_at_secs: at_secs,
        };
        self.history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(record.clone());
        current.insert(key, record);
        version
    }

    pub fn in_force(&self, subject: &SubjectRef, tenant: &TenantId, activity: &str) -> bool {
        let key = (
            subject.principal.principal_id.0.clone(),
            tenant.0.clone(),
            activity.to_string(),
        );
        self.current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .map(|r| r.in_force)
            .unwrap_or(false)
    }

    pub fn withdraw(
        &self,
        subject: &SubjectRef,
        tenant: &TenantId,
        activity: &str,
        basis: WithdrawalBasis,
        at_secs: u64,
    ) -> WithdrawalEffect {
        let key = (
            subject.principal.principal_id.0.clone(),
            tenant.0.clone(),
            activity.to_string(),
        );
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        let next_version = current.get(&key).map(|r| r.version + 1).unwrap_or(1);
        let withdrawn = ConsentRecord {
            subject_token: key.0.clone(),
            tenant_token: key.1.clone(),
            activity: key.2.clone(),
            version: next_version,
            in_force: false,
            recorded_at_secs: at_secs,
        };
        self.history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(withdrawn.clone());
        current.insert(key, withdrawn);
        *self.withdrawals.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        drop(current);

        match basis {
            WithdrawalBasis::ControllerConsentOnly => {
                WithdrawalEffect::StoppedAndTriggersDeletion(EraseScope::Subject {
                    subject: subject.clone(),
                    tenant: tenant.clone(),
                })
            }
            WithdrawalBasis::HasOtherLawfulBasis => WithdrawalEffect::StoppedOnly,
        }
    }

    pub fn history_for(
        &self,
        subject: &SubjectRef,
        tenant: &TenantId,
        activity: &str,
    ) -> Vec<ConsentRecord> {
        let st = subject.principal.principal_id.0.clone();
        let tt = tenant.0.clone();
        let mut hist: Vec<ConsentRecord> = self
            .history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|r| r.subject_token == st && r.tenant_token == tt && r.activity == activity)
            .cloned()
            .collect();
        hist.sort_by_key(|r| r.version);
        hist
    }

    pub fn withdrawal_count(&self) -> u64 {
        *self.withdrawals.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WithdrawalBasis {
    ControllerConsentOnly,
    HasOtherLawfulBasis,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubProcessor {
    pub id: String,
    pub region: Region,
    pub dpa_ref: String,
    pub version: u64,
    pub objections: Vec<String>,
}

#[derive(Default)]
pub struct SubProcessorRegistry {
    entries: Mutex<BTreeMap<String, SubProcessor>>,
    objection_count: Mutex<u64>,
}

impl SubProcessorRegistry {
    pub fn new() -> SubProcessorRegistry {
        SubProcessorRegistry::default()
    }

    pub fn register(&self, id: &str, region: Region, dpa_ref: &str) -> u64 {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let (version, objections) = entries
            .get(id)
            .map(|e| (e.version + 1, e.objections.clone()))
            .unwrap_or((1, Vec::new()));
        entries.insert(
            id.to_string(),
            SubProcessor {
                id: id.to_string(),
                region,
                dpa_ref: dpa_ref.to_string(),
                version,
                objections,
            },
        );
        version
    }

    pub fn object(&self, tenant: &TenantId, subprocessor_id: &str) -> bool {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = entries.get_mut(subprocessor_id) else {
            return false;
        };
        let token = tenant.0.clone();
        if entry.objections.contains(&token) {
            return false;
        }
        entry.objections.push(token);
        *self
            .objection_count
            .lock()
            .unwrap_or_else(|e| e.into_inner()) += 1;
        true
    }

    pub fn list(&self) -> Vec<SubProcessor> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<SubProcessor> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
    }

    pub fn objection_count(&self) -> u64 {
        *self
            .objection_count
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferVerdict {
    Allowed,
    Denied,
}

impl TransferVerdict {
    pub fn is_allowed(&self) -> bool {
        matches!(self, TransferVerdict::Allowed)
    }
}

#[derive(Default)]
pub struct TransferGate {
    valid_transfer_mechanisms: Mutex<std::collections::BTreeSet<Region>>,
    extra_eu_denials: Mutex<u64>,
}

impl TransferGate {
    pub fn new() -> TransferGate {
        TransferGate::default()
    }

    pub fn record_transfer_mechanism(&self, region: Region) {
        self.valid_transfer_mechanisms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(region);
    }

    pub fn transfer_allowed(&self, target: &Region) -> TransferVerdict {
        if is_eea_region(target) {
            return TransferVerdict::Allowed;
        }
        let has_mechanism = self
            .valid_transfer_mechanisms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(target);
        if has_mechanism {
            TransferVerdict::Allowed
        } else {
            *self
                .extra_eu_denials
                .lock()
                .unwrap_or_else(|e| e.into_inner()) += 1;
            TransferVerdict::Denied
        }
    }

    pub fn extra_eu_denial_count(&self) -> u64 {
        *self
            .extra_eu_denials
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};

    fn t(s: &str) -> TenantId {
        TenantId::from_token(s)
    }

    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            t("acme"),
        ))
    }

    #[test]
    fn is_eea_region_admits_eu_eea_denies_extra_eu_and_unknown() {
        assert!(
            is_eea_region(&Region::new("fr-par")),
            "fr (France) is in the EU"
        );
        assert!(
            is_eea_region(&Region::new("nl-ams")),
            "nl (Netherlands) is in the EU"
        );
        assert!(
            is_eea_region(&Region::new("de-fra")),
            "de (Germany) is in the EU"
        );
        assert!(
            is_eea_region(&Region::new("no-osl")),
            "no (Norway) is EEA-EFTA"
        );
        assert!(
            is_eea_region(&Region::new("is-rey")),
            "is (Iceland) is EEA-EFTA"
        );
        assert!(!is_eea_region(&Region::new("us-east")), "us is extra-EU");
        assert!(
            !is_eea_region(&Region::new("uk-lon")),
            "uk (post-Brexit) is extra-EU"
        );
        assert!(
            !is_eea_region(&Region::new("xx-nowhere")),
            "an unknown area is extra-EU (fail-closed)"
        );
        assert!(
            is_eea_region(&Region::new("FR-PAR")),
            "the area match is case-insensitive"
        );
    }

    #[test]
    fn transfer_allowed_denies_extra_eu_by_default_admits_within_eu() {
        let gate = TransferGate::new();
        assert_eq!(
            gate.transfer_allowed(&Region::new("fr-par")),
            TransferVerdict::Allowed
        );
        assert_eq!(
            gate.transfer_allowed(&Region::new("nl-ams")),
            TransferVerdict::Allowed
        );
        assert!(gate.transfer_allowed(&Region::new("de-fra")).is_allowed());
        assert_eq!(
            gate.transfer_allowed(&Region::new("us-east")),
            TransferVerdict::Denied
        );
        assert_eq!(
            gate.transfer_allowed(&Region::new("ap-tokyo")),
            TransferVerdict::Denied
        );
        assert_eq!(
            gate.extra_eu_denial_count(),
            2,
            "0 default extra-EU transfers slipped through"
        );
    }

    #[test]
    fn an_extra_eu_target_with_a_recorded_mechanism_is_allowed() {
        let gate = TransferGate::new();
        assert_eq!(
            gate.transfer_allowed(&Region::new("us-east")),
            TransferVerdict::Denied
        );
        gate.record_transfer_mechanism(Region::new("us-east"));
        assert_eq!(
            gate.transfer_allowed(&Region::new("us-east")),
            TransferVerdict::Allowed
        );
        assert_eq!(
            gate.transfer_allowed(&Region::new("ap-tokyo")),
            TransferVerdict::Denied
        );
    }

    #[test]
    fn consent_withdrawal_propagates_and_triggers_deletion_for_controller_consent_only() {
        let reg = ConsentRegistry::new();
        let s = subject("u-1");
        let tenant = t("acme");
        let v = reg.record(&s, &tenant, "marketing-emails", 1000);
        assert_eq!(v, 1, "first consent is version 1");
        assert!(
            reg.in_force(&s, &tenant, "marketing-emails"),
            "consent is in force"
        );

        let effect = reg.withdraw(
            &s,
            &tenant,
            "marketing-emails",
            WithdrawalBasis::ControllerConsentOnly,
            2000,
        );
        assert!(
            effect.triggers_deletion(),
            "controller consent-only ⇒ may-trigger-deletion fired"
        );
        match effect {
            WithdrawalEffect::StoppedAndTriggersDeletion(EraseScope::Subject {
                subject,
                tenant: tn,
            }) => {
                assert_eq!(
                    subject.principal.principal_id.0, "u-1",
                    "the erase scope is the subject"
                );
                assert_eq!(tn.0, "acme");
            }
            other => panic!("expected StoppedAndTriggersDeletion(Subject), got {other:?}"),
        }
        assert!(
            !reg.in_force(&s, &tenant, "marketing-emails"),
            "the consent-path is stopped"
        );
        assert_eq!(
            reg.withdrawal_count(),
            1,
            "the withdrawal is observable (telemetry)"
        );

        reg.record(&s, &tenant, "analytics", 3000);
        reg.withdraw(
            &s,
            &tenant,
            "analytics",
            WithdrawalBasis::HasOtherLawfulBasis,
            4000,
        );
        assert_eq!(
            reg.withdrawal_count(),
            2,
            "the second withdrawal bumps the running total"
        );
    }

    #[test]
    fn consent_withdrawal_with_another_lawful_basis_stops_without_deletion() {
        let reg = ConsentRegistry::new();
        let s = subject("u-2");
        let tenant = t("acme");
        reg.record(&s, &tenant, "service-telemetry", 1000);
        let effect = reg.withdraw(
            &s,
            &tenant,
            "service-telemetry",
            WithdrawalBasis::HasOtherLawfulBasis,
            2000,
        );
        assert_eq!(
            effect,
            WithdrawalEffect::StoppedOnly,
            "another basis ⇒ stop only, no deletion"
        );
        assert!(!effect.triggers_deletion());
        assert!(
            !reg.in_force(&s, &tenant, "service-telemetry"),
            "the path is still stopped"
        );
    }

    #[test]
    fn consent_is_versioned_and_history_is_retained() {
        let reg = ConsentRegistry::new();
        let s = subject("u-3");
        let tenant = t("acme");
        assert_eq!(reg.record(&s, &tenant, "a", 1000), 1);
        assert_eq!(
            reg.record(&s, &tenant, "a", 2000),
            2,
            "re-record bumps the monotone version"
        );
        reg.withdraw(&s, &tenant, "a", WithdrawalBasis::HasOtherLawfulBasis, 3000);

        reg.record(&s, &tenant, "other-activity", 1500);
        reg.record(&subject("u-other"), &tenant, "a", 1500);
        reg.record(&s, &t("globex"), "a", 1500);

        let hist = reg.history_for(&s, &tenant, "a");
        assert_eq!(
            hist.len(),
            3,
            "exactly the (u-3, acme, a) versions retained - other subject/activity/tenant excluded"
        );
        assert_eq!(hist[0].version, 1);
        assert!(hist[0].in_force, "v1 was in force when recorded");
        assert_eq!(hist[2].version, 3);
        assert!(!hist[2].in_force, "the withdrawal version is not-in-force");
        assert_eq!(hist[2].recorded_at_secs, 3000, "timestamped");
        for r in &hist {
            assert_eq!(r.subject_token, "u-3");
            assert_eq!(r.tenant_token, "acme");
            assert_eq!(r.activity, "a");
        }
    }

    #[test]
    fn consent_is_granular_per_activity() {
        let reg = ConsentRegistry::new();
        let s = subject("u-4");
        let tenant = t("acme");
        reg.record(&s, &tenant, "emails", 1000);
        reg.record(&s, &tenant, "analytics", 1000);
        reg.withdraw(
            &s,
            &tenant,
            "emails",
            WithdrawalBasis::HasOtherLawfulBasis,
            2000,
        );
        assert!(!reg.in_force(&s, &tenant, "emails"), "emails withdrawn");
        assert!(
            reg.in_force(&s, &tenant, "analytics"),
            "analytics still in force (granular)"
        );
    }

    #[test]
    fn subprocessor_registry_records_region_dpa_ref_version_and_objection() {
        let reg = SubProcessorRegistry::new();
        let v = reg.register("eu-llm-adapter", Region::new("fr-par"), "DPA-2026-001");
        assert_eq!(v, 1, "first register is version 1");
        let entry = reg.get("eu-llm-adapter").expect("registered");
        assert_eq!(entry.region, Region::new("fr-par"), "region recorded");
        assert_eq!(entry.dpa_ref, "DPA-2026-001", "DPA ref recorded");
        assert!(entry.objections.is_empty(), "no objections yet");

        let v2 = reg.register("eu-llm-adapter", Region::new("nl-ams"), "DPA-2026-002");
        assert_eq!(v2, 2, "re-register bumps the monotone version");

        assert_eq!(
            reg.objection_count(),
            0,
            "no objections yet (count starts at 0)"
        );
        assert!(
            reg.object(&t("acme"), "eu-llm-adapter"),
            "the objection is newly recorded"
        );
        assert!(
            !reg.object(&t("acme"), "eu-llm-adapter"),
            "a duplicate objection is not double-recorded"
        );
        let entry = reg.get("eu-llm-adapter").expect("registered");
        assert_eq!(
            entry.objections,
            vec!["acme".to_string()],
            "the objection is surfaced on the entry"
        );
        assert_eq!(
            reg.objection_count(),
            1,
            "the objection is observable (telemetry)"
        );

        assert!(
            reg.object(&t("globex"), "eu-llm-adapter"),
            "a second tenant's objection is recorded"
        );
        assert_eq!(
            reg.objection_count(),
            2,
            "the second objection bumps the running total"
        );

        assert!(
            !reg.object(&t("acme"), "ghost-adapter"),
            "no entry to object to"
        );
    }

    #[test]
    fn reregister_preserves_a_standing_objection() {
        let reg = SubProcessorRegistry::new();
        reg.register("x", Region::new("us-east"), "DPA-1");
        reg.object(&t("acme"), "x");
        reg.register("x", Region::new("fr-par"), "DPA-2");
        let entry = reg.get("x").expect("registered");
        assert_eq!(entry.version, 2);
        assert_eq!(
            entry.objections,
            vec!["acme".to_string()],
            "the objection survives the re-version"
        );
    }

    #[test]
    fn subprocessor_registry_lists_entries() {
        let reg = SubProcessorRegistry::new();
        reg.register("a", Region::new("fr-par"), "DPA-a");
        reg.register("b", Region::new("us-east"), "DPA-b");
        let list = reg.list();
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|e| e.id == "a"));
        assert!(list.iter().any(|e| e.id == "b"));
    }

    #[test]
    fn telemetry_signal_names_and_units_are_anchored() {
        assert_eq!(
            TRANSFER_GATE_EXTRA_EU_DENIALS.0,
            "gdpr.transfer_gate_extra_eu_denials"
        );
        assert_eq!(TRANSFER_GATE_EXTRA_EU_DENIALS.1, "count");
        assert_eq!(CONSENT_WITHDRAWALS.0, "gdpr.consent_withdrawals");
        assert_eq!(CONSENT_WITHDRAWALS.1, "count");
        assert_eq!(SUBPROCESSOR_OBJECTIONS.0, "gdpr.subprocessor_objections");
        assert_eq!(SUBPROCESSOR_OBJECTIONS.1, "count");
    }
}

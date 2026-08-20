use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};
use myelin_substrate::{Holder, HolderRegistration, StoreKind};
use myelin_tenancy::Region;

use crate::datamap::HolderSchema;
use crate::holders::{CryptoShredKms, ShredKeyClass, ShredKeyHandle};
use crate::orchestration::{CanonicalErasePhase, RegisteredHolder};
use crate::posture::{
    reference_is_by_reference, SubsystemReference, CANONICAL_POSTURE, POSTURE_ANCHOR,
};

pub const CI_DB: &str = "ci_oltp";

pub const CI_SUBSYSTEM: &str = "ci";

pub fn ci_phase_of(holder_id: &str) -> Option<CanonicalErasePhase> {
    match holder_id {
        CI_DB => Some(CanonicalErasePhase::CryptoShredDek),
        _ => None,
    }
}

pub fn ci_holder_schemas(region: Region) -> Vec<HolderSchema> {
    vec![HolderSchema {
        registration: HolderRegistration {
            kind: StoreKind::Oltp,
            name: CI_DB,
        },
        holder: Holder::H2Ci,
        region,
        fields: &[],
    }]
}

pub fn ci_registrations() -> Vec<HolderRegistration> {
    vec![HolderRegistration {
        kind: StoreKind::Oltp,
        name: CI_DB,
    }]
}

pub const CI_INSTANCE: SubsystemReference = SubsystemReference {
    subsystem: CI_SUBSYSTEM,
    cited_anchor: POSTURE_ANCHOR,
    section_text:
        "Free-text / immutable-content erasure follows the platform posture in \
         00-reconciliation-decisions.md §X-7 / gdpr-and-audit.md §7 (contract 10.9). CI inline \
         log-line PII that is isolable to one subject is sealed under that subject's per-subject \
         CI-log DEK, so an erase crypto-shreds exactly their CI log content (live and backups) while \
         the run-graph structure survives; non-isolable interleaved PII rides the per-tenant fallback \
         and its residual is the ONE platform-posture residual.",
};

#[must_use]
pub fn ci_section_references_posture() -> bool {
    reference_is_by_reference(&CI_INSTANCE)
}

#[must_use]
pub const fn ci_residual() -> &'static str {
    CANONICAL_POSTURE.residual
}

fn subject_and_tenant(scope: &EraseScope) -> (String, String) {
    match scope {
        EraseScope::Subject { subject, tenant } => {
            (subject.principal.principal_id.0.clone(), tenant.0.clone())
        }
        EraseScope::Tenant(tenant) => ("*tenant*".to_string(), tenant.0.clone()),
    }
}

#[derive(Debug, Default)]
pub struct CiLogModel {
    run_graph: Mutex<BTreeMap<String, bool>>,
    erase_calls: Mutex<u32>,
}

impl CiLogModel {
    pub fn new() -> CiLogModel {
        CiLogModel::default()
    }

    pub fn index_run_graph_from_source(&self, subject_token: &str) {
        self.run_graph
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subject_token.to_string(), true);
    }

    pub fn run_graph_present(&self, subject_token: &str) -> bool {
        self.run_graph
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(subject_token)
            .copied()
            .unwrap_or(false)
    }

    pub fn erase_call_count(&self) -> u32 {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn note_erase(&self) {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
    }
}

pub struct CiLogHolder<'a> {
    model: &'a CiLogModel,
    kms: &'a dyn CryptoShredKms,
}

impl<'a> CiLogHolder<'a> {
    pub fn new(model: &'a CiLogModel, kms: &'a dyn CryptoShredKms) -> CiLogHolder<'a> {
        CiLogHolder { model, kms }
    }

    pub fn holder_id(&self) -> &'static str {
        CI_DB
    }

    fn subject_dek(subject_token: &str, tenant: &TenantId) -> ShredKeyHandle {
        ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject(subject_token.to_string()),
        }
    }

    fn tenant_dek(tenant: &TenantId) -> ShredKeyHandle {
        ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Tenant,
        }
    }
}

impl PersonalDataHolder for CiLogHolder<'_> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if self.kms.is_present(&Self::subject_dek(&sid, &tenant)) {
            "located:ci-log-segments-present"
        } else {
            "located:0-recoverable"
        };
        Ok(LocateReport {
            receipt: Receipt::content_addressed("locate", CI_DB, &sid, &tenant.0, outcome, None, 0),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export", CI_DB, &sid, &tenant.0, "exported", None, 0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed("rectify", CI_DB, &sid, "*", "rectified", None, 0),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if on {
            "restricted:set"
        } else {
            "restricted:clear"
        };
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed("restrict", CI_DB, &sid, "*", outcome, None, 0),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (sid, tenant_token) = subject_and_tenant(&scope);
        let tenant = TenantId::from_token(&tenant_token);
        self.model.note_erase();
        let (destroyed, outcome) = match &scope {
            EraseScope::Subject { .. } => (
                self.kms.destroy(&Self::subject_dek(&sid, &tenant)),
                "crypto_shred:per_subject_ci_log_dek:isolable_segments;structure_survives",
            ),
            EraseScope::Tenant(_) => {
                let d = self.kms.destroy(&Self::tenant_dek(&tenant));
                (
                    d,
                    "crypto_shred:per_tenant_ci_log_dek_fallback:tenant_offboard;structure_survives",
                )
            }
        };
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                CI_DB,
                &sid,
                &tenant_token,
                outcome,
                destroyed,
                0,
            ),
        })
    }
}

pub struct CiHolderRegistration;

impl CiHolderRegistration {
    pub fn register_ci<'a>(
        holders: Vec<(&'static str, &'a dyn PersonalDataHolder)>,
    ) -> Vec<RegisteredHolder<'a>> {
        holders
            .into_iter()
            .map(|(id, holder)| {
                let phase = ci_phase_of(id)
                    .unwrap_or_else(|| panic!("CI holder `{id}` has no canonical erase phase"));
                RegisteredHolder { id, phase, holder }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamap::data_map;
    use crate::holders::InMemoryShredKms;
    use crate::orchestration::UpstreamHolderOrchestrator;
    use crate::posture::restatement_markers;
    use crate::EraseChecklist;
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

    fn subject_scope(s: &str) -> EraseScope {
        EraseScope::Subject {
            subject: subject(s),
            tenant: t("acme"),
        }
    }

    fn region() -> Region {
        Region("fr-par".into())
    }

    fn provision_subject_dek(
        kms: &InMemoryShredKms,
        tenant: &TenantId,
        subject_token: &str,
        epoch: u64,
    ) {
        kms.provision(
            ShredKeyHandle {
                tenant: tenant.clone(),
                class: ShredKeyClass::Subject(subject_token.to_string()),
            },
            epoch,
        );
    }

    fn provision_tenant_dek(kms: &InMemoryShredKms, tenant: &TenantId, epoch: u64) {
        kms.provision(
            ShredKeyHandle {
                tenant: tenant.clone(),
                class: ShredKeyClass::Tenant,
            },
            epoch,
        );
    }

    #[test]
    fn ci_holder_appears_in_the_data_map_after_registration() {
        let inv = data_map(&ci_holder_schemas(region()));
        assert!(inv.holders.contains("oltp:ci_oltp"), "H2 CI is in the map");
        assert_eq!(inv.holder_count(), 1, "exactly the one CI holder");
        assert!(
            inv.coverage_gaps(&ci_registrations()).is_empty(),
            "the registered CI holder is in the map - 0 holders missed"
        );
    }

    #[test]
    fn a_registered_ci_holder_absent_from_the_map_is_a_coverage_gap() {
        let inv = data_map(&[]);
        let gaps = inv.coverage_gaps(&ci_registrations());
        assert_eq!(
            gaps,
            vec!["oltp:ci_oltp".to_string()],
            "the registered-but-unmapped CI holder is the coverage gap"
        );
    }

    #[test]
    fn ci_holder_declares_its_canonical_erase_phase() {
        assert_eq!(
            ci_phase_of(CI_DB),
            Some(CanonicalErasePhase::CryptoShredDek)
        );
        assert_eq!(ci_phase_of("not_the_ci_store"), None);
    }

    #[test]
    fn ci_holder_id_is_the_frozen_ci_oltp_address() {
        let kms = InMemoryShredKms::new();
        let model = CiLogModel::new();
        let holder = CiLogHolder::new(&model, &kms);
        assert_eq!(
            holder.holder_id(),
            CI_DB,
            "the holder id is the frozen ci_oltp address"
        );
        assert_eq!(holder.holder_id(), "ci_oltp");
        assert_eq!(
            ci_holder_schemas(region())[0].holder_id(),
            "oltp:ci_oltp",
            "the schema registers the holder under the same store name"
        );
    }

    #[test]
    fn the_fan_out_reaches_the_ci_holder_in_canonical_order() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-ci", 10);

        let model = CiLogModel::new();
        model.index_run_graph_from_source("u-ci");
        let ci_h = CiLogHolder::new(&model, &kms);

        let ci = CiHolderRegistration::register_ci(vec![(CI_DB, &ci_h as &dyn PersonalDataHolder)]);
        let orch = UpstreamHolderOrchestrator::new(ci);

        let ids = orch.holder_ids_in_order();
        assert!(ids.contains(&CI_DB), "H2 CI is in the fan-out");

        let checklist = EraseChecklist::new();
        let receipts = orch
            .fan_out_erase(&subject_scope("u-ci"), &checklist)
            .unwrap();
        assert_eq!(receipts.len(), 1, "the CI holder was reached");
        assert_eq!(
            orch.fanout_coverage(&checklist),
            1.0,
            "100% coverage of the CI holder"
        );
    }

    #[test]
    fn ci_d3_per_subject_dek_shred_reaches_isolable_log_pii_structure_survives() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-erase", 20);
        provision_subject_dek(&kms, &tenant, "u-other", 21);
        provision_tenant_dek(&kms, &tenant, 22);
        let model = CiLogModel::new();
        model.index_run_graph_from_source("u-erase");
        model.index_run_graph_from_source("u-other");

        let erase_dek = CiLogHolder::subject_dek("u-erase", &tenant);
        let other_dek = CiLogHolder::subject_dek("u-other", &tenant);
        let tenant_dek = CiLogHolder::tenant_dek(&tenant);
        assert!(
            kms.is_present(&erase_dek),
            "the subject's CI-log DEK is live before erase"
        );

        let holder = CiLogHolder::new(&model, &kms);
        let receipt = holder.erase(subject_scope("u-erase")).unwrap();

        assert!(
            !kms.is_present(&erase_dek),
            "the subject's per-subject CI-log DEK is destroyed"
        );
        assert_eq!(
            kms.recoverable_in_backup(&erase_dek),
            0,
            "0 recoverable in backups (crypto-shred reaches backups - CI-D3)"
        );
        assert!(
            kms.is_present(&other_dek),
            "a different subject's CI log survives (the per-subject reach, not a blunt per-tenant erase)"
        );
        assert!(
            kms.is_present(&tenant_dek),
            "the per-tenant fallback key survives a single-subject erase"
        );
        assert!(
            model.run_graph_present("u-erase"),
            "the run-graph structure survives the erase (§3.2 - structure survives, PII shredded)"
        );
        assert!(
            !model.run_graph_present("u-never-indexed"),
            "an un-indexed subject has no run-graph node (present is observably false)"
        );
        assert!(
            receipt.receipt.key_epoch_destroyed.is_some(),
            "the erase receipt records the destroyed per-subject-DEK epoch (CI-D3 telemetry)"
        );
        assert!(receipt.receipt.content_hash.starts_with("blake3:"));
        let expected = Receipt::content_addressed(
            "erase",
            CI_DB,
            "u-erase",
            &tenant.0,
            "crypto_shred:per_subject_ci_log_dek:isolable_segments;structure_survives",
            receipt.receipt.key_epoch_destroyed,
            0,
        );
        assert_eq!(
            receipt.receipt.content_hash, expected.content_hash,
            "the receipt names the per-subject CI-log DEK reach (the C1/P5 extension)"
        );
    }

    #[test]
    fn the_per_tenant_fallback_fires_on_a_tenant_offboarding() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-iso", 30);
        provision_tenant_dek(&kms, &tenant, 31);
        let model = CiLogModel::new();

        let subject_dek = CiLogHolder::subject_dek("u-iso", &tenant);
        let tenant_dek = CiLogHolder::tenant_dek(&tenant);
        let holder = CiLogHolder::new(&model, &kms);

        let receipt = holder.erase(EraseScope::Tenant(tenant.clone())).unwrap();

        assert!(
            !kms.is_present(&tenant_dek),
            "a tenant offboarding destroys the per-tenant CI-log DEK fallback"
        );
        assert_eq!(
            kms.recoverable_in_backup(&tenant_dek),
            0,
            "0 recoverable in backups"
        );
        let expected_tenant = Receipt::content_addressed(
            "erase",
            CI_DB,
            "*tenant*",
            &tenant.0,
            "crypto_shred:per_tenant_ci_log_dek_fallback:tenant_offboard;structure_survives",
            receipt.receipt.key_epoch_destroyed,
            0,
        );
        assert_eq!(
            receipt.receipt.content_hash, expected_tenant.content_hash,
            "the tenant-scope erase names the per-tenant fallback (the selection polarity)"
        );
        let subject_receipt = holder.erase(subject_scope("u-iso")).unwrap();
        let expected_subject = Receipt::content_addressed(
            "erase",
            CI_DB,
            "u-iso",
            &tenant.0,
            "crypto_shred:per_subject_ci_log_dek:isolable_segments;structure_survives",
            subject_receipt.receipt.key_epoch_destroyed,
            0,
        );
        assert_eq!(
            subject_receipt.receipt.content_hash, expected_subject.content_hash,
            "a subject erase names the per-subject reach (not the fallback)"
        );
        assert_ne!(
            receipt.receipt.content_hash,
            subject_receipt.receipt.content_hash
        );
        assert!(
            !kms.is_present(&subject_dek),
            "the subject's per-subject CI-log DEK is destroyed"
        );
    }

    #[test]
    fn ci_holder_erase_is_idempotent() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-idem", 40);
        let model = CiLogModel::new();
        model.index_run_graph_from_source("u-idem");
        let holder = CiLogHolder::new(&model, &kms);

        let first = holder.erase(subject_scope("u-idem")).unwrap();
        let second = holder.erase(subject_scope("u-idem")).unwrap();
        assert_eq!(first.receipt.operation, second.receipt.operation);
        assert!(
            second.receipt.key_epoch_destroyed.is_none(),
            "the re-erase destroyed no key"
        );
        assert!(
            model.run_graph_present("u-idem"),
            "the structure survives the re-erase too"
        );
        assert_eq!(model.erase_call_count(), 2, "both erase calls were counted");
    }

    #[test]
    fn ci_locate_reports_present_on_a_live_dek_and_zero_after_shred() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-loc", 50);
        let model = CiLogModel::new();
        let holder = CiLogHolder::new(&model, &kms);

        let present = holder.locate(&subject("u-loc"), tenant.clone()).unwrap();
        let expected_present = Receipt::content_addressed(
            "locate",
            CI_DB,
            "u-loc",
            &tenant.0,
            "located:ci-log-segments-present",
            None,
            0,
        );
        assert_eq!(present.receipt.content_hash, expected_present.content_hash);

        holder.erase(subject_scope("u-loc")).unwrap();
        let after = holder.locate(&subject("u-loc"), tenant.clone()).unwrap();
        let expected_zero = Receipt::content_addressed(
            "locate",
            CI_DB,
            "u-loc",
            &tenant.0,
            "located:0-recoverable",
            None,
            0,
        );
        assert_eq!(
            after.receipt.content_hash, expected_zero.content_hash,
            "after the per-subject CI-log DEK shred, locate reports 0-recoverable"
        );
        assert_ne!(present.receipt.content_hash, after.receipt.content_hash);
    }

    #[test]
    fn the_ci_instance_references_the_posture_and_does_not_restate() {
        assert_eq!(CI_INSTANCE.subsystem, "ci");
        assert_eq!(
            CI_INSTANCE.cited_anchor, POSTURE_ANCHOR,
            "the CI instance cites the ONE anchor"
        );
        assert!(
            ci_section_references_posture(),
            "the CI erasure section is a valid BY-REFERENCE instantiation (cites + does not restate)"
        );
        let lowered = CI_INSTANCE.section_text.to_ascii_lowercase();
        for marker in restatement_markers() {
            assert!(
                !lowered.contains(&marker.to_ascii_lowercase()),
                "the CI section must not restate the canonical marker {marker:?}"
            );
        }
    }

    #[test]
    fn a_restating_ci_section_would_be_rejected() {
        let restating = SubsystemReference {
            subsystem: "ci",
            cited_anchor: POSTURE_ANCHOR,
            section_text: "CI erasure: per-subject DEK crypto-shred renders isolable log-line PII \
                 unrecoverable; the documented lawful-basis limit covers interleaved mentions ...",
        };
        assert!(
            !reference_is_by_reference(&restating),
            "a CI section that restates the posture (a canonical marker) is rejected - X-7"
        );
    }

    #[test]
    fn ci_residual_is_the_one_platform_posture_residual() {
        assert_eq!(
            ci_residual(),
            CANONICAL_POSTURE.residual,
            "the CI residual IS the single-source canonical residual (not a CI-specific restatement)"
        );
        assert!(
            ci_residual().contains("AUTHOR's DEK") && ci_residual().contains("not the subject's"),
            "the residual is third-party / interleaved PII under the AUTHOR's DEK - not shreddable by the subject's key"
        );
    }
}

use myelin_gdpr::{EraseScope, SubjectRef, TenantId};
use myelin_substrate::Clock;

use crate::dsr::{DsrId, DsrKind, DsrOrchestrator, Initiator, Posture};
use crate::fanout::{FanOutDriver, FanOutOutcome, LegalHoldRegistry};
use crate::orchestration::{EraseChecklist, UpstreamHolderOrchestrator};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TenantDsrError {
    CrossTenantSubject {
        calling_tenant: String,
        subject_tenant: String,
    },
    Orchestrator(crate::dsr::DsrError),
}

impl std::fmt::Display for TenantDsrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TenantDsrError::CrossTenantSubject {
                calling_tenant,
                subject_tenant,
            } => write!(
                f,
                "Art. 28 cross-tenant DSR refused: tenant `{calling_tenant}` may only act for its \
                 own data subjects, but the subject lives under tenant `{subject_tenant}` (§4.4)"
            ),
            TenantDsrError::Orchestrator(e) => write!(f, "DSR orchestrator error: {e}"),
        }
    }
}

impl std::error::Error for TenantDsrError {}

impl From<crate::dsr::DsrError> for TenantDsrError {
    fn from(e: crate::dsr::DsrError) -> TenantDsrError {
        TenantDsrError::Orchestrator(e)
    }
}

pub type Result<T> = std::result::Result<T, TenantDsrError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OffboardingCertificate {
    pub tenant: TenantId,
    pub dsr_id: DsrId,
    pub completion: crate::fanout::DsrCompletionReceipt,
}

pub struct TenantDsrSurface<'a, C: Clock> {
    dsr: &'a DsrOrchestrator<C>,
    holds: &'a LegalHoldRegistry,
}

impl<'a, C: Clock> TenantDsrSurface<'a, C> {
    pub fn new(
        dsr: &'a DsrOrchestrator<C>,
        holds: &'a LegalHoldRegistry,
    ) -> TenantDsrSurface<'a, C> {
        TenantDsrSurface { dsr, holds }
    }

    fn art28_scope_ok(calling_tenant: &TenantId, subject: &SubjectRef) -> Result<()> {
        let subject_tenant = &subject.principal.tenant;
        if subject_tenant == calling_tenant {
            Ok(())
        } else {
            Err(TenantDsrError::CrossTenantSubject {
                calling_tenant: calling_tenant.0.clone(),
                subject_tenant: subject_tenant.0.clone(),
            })
        }
    }

    pub fn submit_for_my_subject(
        &self,
        calling_tenant: &TenantId,
        kind: DsrKind,
        subject: SubjectRef,
    ) -> Result<DsrId> {
        Self::art28_scope_ok(calling_tenant, &subject)?;
        let scope = EraseScope::Subject {
            subject: subject.clone(),
            tenant: calling_tenant.clone(),
        };
        let id = self.dsr.dsr_submit(
            kind,
            calling_tenant.clone(),
            subject,
            scope,
            Posture::Processor,
            Initiator::TenantInstructed,
        );
        Ok(id)
    }

    pub fn drive_tenant_subject_dsr(
        &self,
        id: &DsrId,
        inventory: &crate::datamap::Inventory,
        upstream: &UpstreamHolderOrchestrator<'_>,
        checklist: &EraseChecklist,
    ) -> Result<FanOutOutcome> {
        let admitted = self.dsr.validate(id)?;
        debug_assert!(
            admitted,
            "a tenant-instructed (Art. 28) DSR is never posture-refused (§4.4)"
        );
        let driver = FanOutDriver::new(self.dsr, self.holds);
        Ok(driver.drive(id, inventory, upstream, checklist)?)
    }

    pub fn offboard_tenant(
        &self,
        tenant: &TenantId,
        inventory: &crate::datamap::Inventory,
        upstream: &UpstreamHolderOrchestrator<'_>,
        checklist: &EraseChecklist,
    ) -> Result<OffboardingCertificate> {
        let id = self.dsr.dsr_submit(
            DsrKind::Erasure,
            tenant.clone(),
            tenant_subject(tenant),
            EraseScope::Tenant(tenant.clone()),
            Posture::Processor,
            Initiator::Myelin,
        );
        let admitted = self.dsr.validate(&id)?;
        debug_assert!(
            admitted,
            "a tenant offboarding (EraseScope::Tenant) is an authorised erase (§4.4)"
        );
        let driver = FanOutDriver::new(self.dsr, self.holds);
        let outcome = driver.drive(&id, inventory, upstream, checklist)?;
        let completion = outcome.receipt().clone();
        Ok(OffboardingCertificate {
            tenant: tenant.clone(),
            dsr_id: id,
            completion,
        })
    }

    pub fn restrict_subject(
        &self,
        calling_tenant: &TenantId,
        subject: SubjectRef,
        inventory: &crate::datamap::Inventory,
        upstream: &UpstreamHolderOrchestrator<'_>,
        checklist: &EraseChecklist,
    ) -> Result<FanOutOutcome> {
        let id = self.submit_for_my_subject(calling_tenant, DsrKind::Restriction, subject)?;
        self.drive_tenant_subject_dsr(&id, inventory, upstream, checklist)
    }

    pub fn rectify_subject(
        &self,
        calling_tenant: &TenantId,
        subject: SubjectRef,
        inventory: &crate::datamap::Inventory,
        upstream: &UpstreamHolderOrchestrator<'_>,
        checklist: &EraseChecklist,
    ) -> Result<FanOutOutcome> {
        let id = self.submit_for_my_subject(calling_tenant, DsrKind::Rectification, subject)?;
        self.drive_tenant_subject_dsr(&id, inventory, upstream, checklist)
    }

    pub fn portability_for_subject(
        &self,
        calling_tenant: &TenantId,
        subject: SubjectRef,
        inventory: &crate::datamap::Inventory,
        upstream: &UpstreamHolderOrchestrator<'_>,
        checklist: &EraseChecklist,
    ) -> Result<FanOutOutcome> {
        let id = self.submit_for_my_subject(calling_tenant, DsrKind::Portability, subject)?;
        self.drive_tenant_subject_dsr(&id, inventory, upstream, checklist)
    }
}

fn tenant_subject(tenant: &TenantId) -> SubjectRef {
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    SubjectRef::new(Principal::stub(
        PrincipalId("*tenant*".into()),
        PrincipalKind::Human,
        tenant.clone(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    use crate::datamap::{Inventory, InventoryEntry};
    use crate::dsr::DsrState;
    use crate::fanout::HoldScope;
    use crate::holders::{InMemoryShredKms, ShredKeyClass, ShredKeyHandle};
    use crate::orchestration::{holder_ids, SeamHolder};
    use myelin_gdpr::PersonalDataHolder;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_substrate::TestClock;

    fn t(s: &str) -> TenantId {
        TenantId::from_token(s)
    }

    fn subject_in(tenant: &TenantId, id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            tenant.clone(),
        ))
    }

    fn kms_with_all_holder_keys(tenant: &TenantId, base_epoch: u64) -> InMemoryShredKms {
        let kms = InMemoryShredKms::new();
        for (i, id) in [
            holder_ids::IDENTITY,
            holder_ids::BLOB,
            holder_ids::AUTHZ_TUPLES,
            holder_ids::BUS,
            holder_ids::CACHE,
            holder_ids::BACKUP,
        ]
        .iter()
        .enumerate()
        {
            kms.provision(
                ShredKeyHandle {
                    tenant: tenant.clone(),
                    class: ShredKeyClass::Subject((*id).to_string()),
                },
                base_epoch + i as u64,
            );
        }
        kms
    }

    fn seam_holders(kms: &InMemoryShredKms) -> Vec<(&'static str, SeamHolder<'_>)> {
        [
            holder_ids::IDENTITY,
            holder_ids::BLOB,
            holder_ids::AUTHZ_TUPLES,
            holder_ids::BUS,
            holder_ids::CACHE,
            holder_ids::BACKUP,
        ]
        .into_iter()
        .map(|id| {
            (
                id,
                SeamHolder::new(id, ShredKeyClass::Subject(id.to_string()), kms),
            )
        })
        .collect()
    }

    fn inventory() -> Inventory {
        let mut holders = BTreeSet::new();
        holders.insert("identity".to_string());
        holders.insert("search_index:search_index".to_string());
        Inventory {
            entries: vec![InventoryEntry {
                field_path: "PrincipalRow.email".into(),
                holder_id: "identity".into(),
                holder: "H15".into(),
                region: "fr-par".into(),
                category: "ContactInfo".into(),
                role: "PlatformOperational".into(),
                basis: "Contract".into(),
                retention: "UntilContractEnd".into(),
                erasure: "CryptoShred(subject_dek)".into(),
                subject_locator: "principal_id".into(),
            }],
            holders,
            dpia_markers: BTreeSet::new(),
        }
    }

    #[test]
    fn art28_tenant_dsr_over_own_subject_is_admitted_and_completes() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 100);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(1_700_000_000));
        let holds = LegalHoldRegistry::new();
        let surface = TenantDsrSurface::new(&dsr, &holds);

        let id = surface
            .submit_for_my_subject(&tenant, DsrKind::Erasure, subject_in(&tenant, "u1"))
            .expect("a tenant may act for its own subject (Art. 28)");
        let checklist = EraseChecklist::new();
        let outcome = surface
            .drive_tenant_subject_dsr(&id, &inventory(), &upstream, &checklist)
            .unwrap();

        assert!(
            matches!(outcome, FanOutOutcome::Erased(_)),
            "tenant-instructed erase admitted + driven"
        );
        assert_eq!(dsr.state_of(&id).unwrap(), DsrState::Completed);
        assert_eq!(
            upstream.fanout_coverage(&checklist),
            1.0,
            "100% fan-out over the holder list"
        );
    }

    #[test]
    fn art28_refuses_a_dsr_over_another_tenants_subject() {
        let acme = t("acme");
        let evil = t("evil-corp");
        let dsr = DsrOrchestrator::new(TestClock::at(0));
        let holds = LegalHoldRegistry::new();
        let surface = TenantDsrSurface::new(&dsr, &holds);

        let err = surface
            .submit_for_my_subject(&evil, DsrKind::Erasure, subject_in(&acme, "victim"))
            .unwrap_err();
        assert_eq!(
            err,
            TenantDsrError::CrossTenantSubject {
                calling_tenant: "evil-corp".into(),
                subject_tenant: "acme".into(),
            },
            "a cross-tenant Art. 28 request is refused (the IDOR face)"
        );
    }

    #[test]
    fn art28_scope_ok_admits_same_tenant_refuses_different_tenant() {
        let acme = t("acme");
        assert!(
            TenantDsrSurface::<TestClock>::art28_scope_ok(&acme, &subject_in(&acme, "u")).is_ok()
        );
        assert!(TenantDsrSurface::<TestClock>::art28_scope_ok(
            &acme,
            &subject_in(&t("other"), "u")
        )
        .is_err());
    }

    #[test]
    fn tenant_offboarding_fans_erase_tenant_over_the_holder_list_and_seals_a_certificate() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 200);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(1_700_000_000));
        let holds = LegalHoldRegistry::new();
        let surface = TenantDsrSurface::new(&dsr, &holds);

        let checklist = EraseChecklist::new();
        let cert = surface
            .offboard_tenant(&tenant, &inventory(), &upstream, &checklist)
            .expect("a tenant offboarding is an authorised erase");

        assert_eq!(cert.tenant, tenant);
        assert_eq!(
            cert.completion.scope_token, "acme",
            "tenant-granularity offboarding (no subject)"
        );
        assert_eq!(cert.completion.outcome, "erased");
        assert_eq!(
            cert.completion.holder_receipts.len(),
            6,
            "all six holders shredded for offboarding"
        );
        assert_eq!(
            cert.completion.holder_receipts[0].holder_id,
            holder_ids::IDENTITY,
            "Identity FIRST"
        );
        assert!(
            cert.completion.content_hash.starts_with("blake3:"),
            "content-addressed (§4.2)"
        );
        assert_eq!(
            upstream.fanout_coverage(&checklist),
            1.0,
            "100% fan-out (the §4.4 GATE)"
        );
        assert_eq!(dsr.state_of(&cert.dsr_id).unwrap(), DsrState::Completed);
        for hr in &cert.completion.holder_receipts {
            assert!(
                hr.receipt.receipt.key_epoch_destroyed.is_some(),
                "tenant-KEK shred recorded"
            );
        }
    }

    #[test]
    fn tenant_offboarding_is_resumable_a_worker_kill_redrives_only_un_receipted_holders() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 300);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(0));
        let holds = LegalHoldRegistry::new();
        let surface = TenantDsrSurface::new(&dsr, &holds);
        let checklist = EraseChecklist::new();

        let first_two: Vec<(&'static str, &dyn PersonalDataHolder)> = holders
            .iter()
            .filter(|(id, _)| *id == holder_ids::IDENTITY || *id == holder_ids::BLOB)
            .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
            .collect();
        let partial = UpstreamHolderOrchestrator::register_m1_upstream(first_two);
        partial
            .fan_out_erase(&EraseScope::Tenant(tenant.clone()), &checklist)
            .unwrap();
        assert_eq!(
            checklist.done_count(),
            2,
            "the crash left two holders receipted"
        );
        let calls_after_partial: Vec<u32> =
            holders.iter().map(|(_, h)| h.erase_call_count()).collect();

        let cert = surface
            .offboard_tenant(&tenant, &inventory(), &upstream, &checklist)
            .unwrap();
        for (i, (id, _)) in holders.iter().enumerate() {
            if *id == holder_ids::IDENTITY || *id == holder_ids::BLOB {
                assert_eq!(
                    holders[i].1.erase_call_count(),
                    calls_after_partial[i],
                    "holder {id} already receipted ⇒ NOT re-shredded (0 double-shred)"
                );
            } else {
                assert_eq!(
                    holders[i].1.erase_call_count(),
                    1,
                    "holder {id} shredded on resume"
                );
            }
        }
        assert_eq!(
            cert.completion.holder_receipts.len(),
            6,
            "the certificate has the complete holder set"
        );
        assert_eq!(upstream.fanout_coverage(&checklist), 1.0);
    }

    #[test]
    fn tenant_offboarding_under_a_tenant_hold_is_deferred_then_resumes_when_cleared() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 400);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(0));
        let holds = LegalHoldRegistry::new();
        holds.set(HoldScope::Tenant("acme".into()), true);
        let surface = TenantDsrSurface::new(&dsr, &holds);
        let checklist = EraseChecklist::new();

        let cert = surface
            .offboard_tenant(&tenant, &inventory(), &upstream, &checklist)
            .unwrap();
        assert_eq!(
            cert.completion.outcome, "deferred:legal_hold",
            "offboarding deferred under hold"
        );
        assert!(
            cert.completion.holder_receipts.is_empty(),
            "no holder shredded under hold"
        );
        assert_eq!(upstream.fanout_coverage(&checklist), 0.0);

        holds.set(HoldScope::Tenant("acme".into()), false);
        let cert2 = surface
            .offboard_tenant(&tenant, &inventory(), &upstream, &checklist)
            .unwrap();
        assert_eq!(cert2.completion.outcome, "erased");
        assert_eq!(upstream.fanout_coverage(&checklist), 1.0);
    }

    #[test]
    fn restrict_rectify_portability_route_through_the_orchestrator() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 500);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(0));
        let holds = LegalHoldRegistry::new();
        holds.set(HoldScope::Tenant("acme".into()), true);
        let surface = TenantDsrSurface::new(&dsr, &holds);

        let r = surface
            .restrict_subject(
                &tenant,
                subject_in(&tenant, "u-r"),
                &inventory(),
                &upstream,
                &EraseChecklist::new(),
            )
            .unwrap();
        assert!(
            matches!(r, FanOutOutcome::ReadRightServed(_)),
            "restriction is not an erase (not suspended)"
        );
        assert_eq!(r.receipt().outcome, "restriction");

        let rec = surface
            .rectify_subject(
                &tenant,
                subject_in(&tenant, "u-rec"),
                &inventory(),
                &upstream,
                &EraseChecklist::new(),
            )
            .unwrap();
        assert_eq!(rec.receipt().outcome, "rectification");

        let p = surface
            .portability_for_subject(
                &tenant,
                subject_in(&tenant, "u-p"),
                &inventory(),
                &upstream,
                &EraseChecklist::new(),
            )
            .unwrap();
        assert!(
            matches!(p, FanOutOutcome::ReadRightServed(_)),
            "portability is a read right (never suspended)"
        );
        assert_eq!(p.receipt().outcome, "portability");
    }

    #[test]
    fn non_erasure_rights_are_also_art28_scoped() {
        let acme = t("acme");
        let evil = t("evil");
        let kms = kms_with_all_holder_keys(&acme, 600);
        let holders = seam_holders(&kms);
        let upstream = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let dsr = DsrOrchestrator::new(TestClock::at(0));
        let holds = LegalHoldRegistry::new();
        let surface = TenantDsrSurface::new(&dsr, &holds);

        let victim = subject_in(&acme, "victim");
        assert!(matches!(
            surface.restrict_subject(
                &evil,
                victim.clone(),
                &inventory(),
                &upstream,
                &EraseChecklist::new()
            ),
            Err(TenantDsrError::CrossTenantSubject { .. })
        ));
        assert!(matches!(
            surface.rectify_subject(
                &evil,
                victim.clone(),
                &inventory(),
                &upstream,
                &EraseChecklist::new()
            ),
            Err(TenantDsrError::CrossTenantSubject { .. })
        ));
        assert!(matches!(
            surface.portability_for_subject(
                &evil,
                victim,
                &inventory(),
                &upstream,
                &EraseChecklist::new()
            ),
            Err(TenantDsrError::CrossTenantSubject { .. })
        ));
    }

    #[test]
    fn cross_tenant_error_renders_pii_free() {
        let e = TenantDsrError::CrossTenantSubject {
            calling_tenant: "evil".into(),
            subject_tenant: "acme".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("evil") && msg.contains("acme") && msg.contains("Art. 28"));
    }
}

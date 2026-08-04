use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_gdpr::{EraseReceipt, EraseScope, PersonalDataHolder, SubjectRef, TenantId};

pub mod holder_ids {
    pub const IDENTITY: &str = "identity";
    pub const BLOB: &str = "blob_store";
    pub const AUTHZ_TUPLES: &str = "authz_tuples";
    pub const BUS: &str = "event_bus";
    pub const CACHE: &str = "cache_cdn";
    pub const BACKUP: &str = "backups";
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum CanonicalErasePhase {
    IdentityPseudonymMap = 0,
    CryptoShredDek = 1,
    PurgeAndTombstoneDerived = 2,
    BusErase = 3,
    CachesAndDerivedCopies = 4,
    Backups = 5,
}

pub fn canonical_phase_of(holder_id: &str) -> Option<CanonicalErasePhase> {
    match holder_id {
        holder_ids::IDENTITY => Some(CanonicalErasePhase::IdentityPseudonymMap),
        holder_ids::BLOB => Some(CanonicalErasePhase::CryptoShredDek),
        holder_ids::AUTHZ_TUPLES => Some(CanonicalErasePhase::PurgeAndTombstoneDerived),
        holder_ids::BUS => Some(CanonicalErasePhase::BusErase),
        holder_ids::CACHE => Some(CanonicalErasePhase::CachesAndDerivedCopies),
        holder_ids::BACKUP => Some(CanonicalErasePhase::Backups),
        _ => None,
    }
}

pub struct RegisteredHolder<'a> {
    pub id: &'static str,
    pub phase: CanonicalErasePhase,
    pub holder: &'a dyn PersonalDataHolder,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HolderReceipt {
    pub holder_id: &'static str,
    pub phase: CanonicalErasePhase,
    pub receipt: EraseReceipt,
}

#[derive(Debug, Default)]
pub struct EraseChecklist {
    done: Mutex<BTreeMap<&'static str, HolderReceipt>>,
}

impl EraseChecklist {
    pub fn new() -> EraseChecklist {
        EraseChecklist {
            done: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn is_done(&self, holder_id: &str) -> bool {
        self.done
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(holder_id)
    }

    fn record(&self, hr: HolderReceipt) {
        self.done
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(hr.holder_id, hr);
    }

    pub fn receipts_in_order(&self) -> Vec<HolderReceipt> {
        let mut v: Vec<HolderReceipt> = self
            .done
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        v.sort_by(|a, b| a.phase.cmp(&b.phase).then(a.holder_id.cmp(b.holder_id)));
        v
    }

    pub fn done_count(&self) -> usize {
        self.done.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

pub const ERASURE_FANOUT_COVERAGE: (&str, &str) = ("gdpr.erasure_fanout_coverage", "ratio");

pub const CRYPTO_SHRED_LAG: (&str, &str) = ("gdpr.crypto_shred_lag", "ms");

pub struct UpstreamHolderOrchestrator<'a> {
    holders: Vec<RegisteredHolder<'a>>,
}

impl<'a> UpstreamHolderOrchestrator<'a> {
    pub fn new(mut holders: Vec<RegisteredHolder<'a>>) -> UpstreamHolderOrchestrator<'a> {
        holders.sort_by(|a, b| a.phase.cmp(&b.phase).then(a.id.cmp(b.id)));
        UpstreamHolderOrchestrator { holders }
    }

    pub fn register_m1_upstream(
        holders: Vec<(&'static str, &'a dyn PersonalDataHolder)>,
    ) -> UpstreamHolderOrchestrator<'a> {
        let registered = holders
            .into_iter()
            .map(|(id, holder)| {
                let phase = canonical_phase_of(id)
                    .unwrap_or_else(|| panic!("holder `{id}` has no canonical erase phase (§4.1)"));
                RegisteredHolder { id, phase, holder }
            })
            .collect();
        UpstreamHolderOrchestrator::new(registered)
    }

    pub fn holder_ids_in_order(&self) -> Vec<&'static str> {
        self.holders.iter().map(|h| h.id).collect()
    }

    pub fn registered_count(&self) -> usize {
        self.holders.len()
    }

    pub fn fan_out_erase(
        &self,
        scope: &EraseScope,
        checklist: &EraseChecklist,
    ) -> myelin_gdpr::Result<Vec<HolderReceipt>> {
        for rh in &self.holders {
            if checklist.is_done(rh.id) {
                continue;
            }
            let receipt = rh.holder.erase(scope.clone())?;
            checklist.record(HolderReceipt {
                holder_id: rh.id,
                phase: rh.phase,
                receipt,
            });
        }
        Ok(checklist.receipts_in_order())
    }

    pub fn fanout_coverage(&self, checklist: &EraseChecklist) -> f64 {
        if self.holders.is_empty() {
            return 1.0;
        }
        checklist.done_count() as f64 / self.holders.len() as f64
    }
}

pub struct SeamHolder<'a> {
    id: &'static str,
    key_class: crate::holders::ShredKeyClass,
    kms: &'a dyn crate::holders::CryptoShredKms,
    erase_calls: Mutex<u32>,
}

impl<'a> SeamHolder<'a> {
    pub fn new(
        id: &'static str,
        key_class: crate::holders::ShredKeyClass,
        kms: &'a dyn crate::holders::CryptoShredKms,
    ) -> SeamHolder<'a> {
        SeamHolder {
            id,
            key_class,
            kms,
            erase_calls: Mutex::new(0),
        }
    }

    pub fn id(&self) -> &'static str {
        self.id
    }

    pub fn erase_call_count(&self) -> u32 {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl PersonalDataHolder for SeamHolder<'_> {
    fn locate(
        &self,
        subject: &SubjectRef,
        tenant: TenantId,
    ) -> myelin_gdpr::Result<myelin_gdpr::LocateReport> {
        let sid = subject.principal.principal_id.0.clone();
        let handle = crate::holders::ShredKeyHandle {
            tenant: tenant.clone(),
            class: self.key_class.clone(),
        };
        let outcome = if self.kms.is_present(&handle) {
            "located:present"
        } else {
            "located:0-recoverable"
        };
        Ok(myelin_gdpr::LocateReport {
            receipt: myelin_gdpr::Receipt::content_addressed(
                "locate", self.id, &sid, &tenant.0, outcome, None, 0,
            ),
        })
    }

    fn export(
        &self,
        subject: &SubjectRef,
        tenant: TenantId,
    ) -> myelin_gdpr::Result<myelin_gdpr::PortableBundle> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(myelin_gdpr::PortableBundle {
            receipt: myelin_gdpr::Receipt::content_addressed(
                "export", self.id, &sid, &tenant.0, "exported", None, 0,
            ),
        })
    }

    fn rectify(
        &self,
        subject: &SubjectRef,
        _patch: myelin_gdpr::Patch,
    ) -> myelin_gdpr::Result<myelin_gdpr::RectifyReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(myelin_gdpr::RectifyReceipt {
            receipt: myelin_gdpr::Receipt::content_addressed(
                "rectify",
                self.id,
                &sid,
                "*",
                "rectified",
                None,
                0,
            ),
        })
    }

    fn restrict(
        &self,
        subject: &SubjectRef,
        on: bool,
    ) -> myelin_gdpr::Result<myelin_gdpr::RestrictReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if on {
            "restricted:set"
        } else {
            "restricted:clear"
        };
        Ok(myelin_gdpr::RestrictReceipt {
            receipt: myelin_gdpr::Receipt::content_addressed(
                "restrict", self.id, &sid, "*", outcome, None, 0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> myelin_gdpr::Result<EraseReceipt> {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        let (subject_token, tenant) = match &scope {
            EraseScope::Subject { subject, tenant } => {
                (subject.principal.principal_id.0.clone(), tenant.clone())
            }
            EraseScope::Tenant(tenant) => ("*tenant*".to_string(), tenant.clone()),
        };
        let handle = crate::holders::ShredKeyHandle {
            tenant: tenant.clone(),
            class: self.key_class.clone(),
        };
        let destroyed_epoch = self.kms.destroy(&handle);
        Ok(EraseReceipt {
            receipt: myelin_gdpr::Receipt::content_addressed(
                "erase",
                self.id,
                &subject_token,
                &tenant.0,
                "crypto_shred:own_key_class",
                destroyed_epoch,
                0,
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::holders::{CryptoShredKms, InMemoryShredKms, ShredKeyClass, ShredKeyHandle};
    use myelin_gdpr::DsrError;
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

    fn seam_holders<'a>(kms: &'a InMemoryShredKms) -> Vec<(&'static str, SeamHolder<'a>)> {
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

    #[test]
    fn fan_out_calls_holders_in_the_canonical_erase_order_identity_first() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 100);
        let holders = seam_holders(&kms);
        let orch = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );

        assert_eq!(
            orch.holder_ids_in_order(),
            vec![
                holder_ids::IDENTITY,
                holder_ids::BLOB,
                holder_ids::AUTHZ_TUPLES,
                holder_ids::BUS,
                holder_ids::CACHE,
                holder_ids::BACKUP,
            ],
            "Identity is erased FIRST; backups LAST (§4.1)"
        );

        let checklist = EraseChecklist::new();
        let scope = EraseScope::Subject {
            subject: subject("u-order"),
            tenant: tenant.clone(),
        };
        let receipts = orch.fan_out_erase(&scope, &checklist).unwrap();

        let order: Vec<&str> = receipts.iter().map(|r| r.holder_id).collect();
        assert_eq!(
            order[0],
            holder_ids::IDENTITY,
            "Identity (pseudonym map) is erased FIRST"
        );
        assert_eq!(
            order.last(),
            Some(&holder_ids::BACKUP),
            "backups are erased LAST"
        );
        assert_eq!(
            order,
            vec![
                holder_ids::IDENTITY,
                holder_ids::BLOB,
                holder_ids::AUTHZ_TUPLES,
                holder_ids::BUS,
                holder_ids::CACHE,
                holder_ids::BACKUP,
            ]
        );
    }

    #[test]
    fn registration_order_does_not_affect_the_canonical_erase_order() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 200);
        let holders = seam_holders(&kms);
        let mut seams: Vec<(&'static str, &dyn PersonalDataHolder)> = holders
            .iter()
            .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
            .collect();
        seams.reverse();
        seams.swap(1, 3);
        let orch = UpstreamHolderOrchestrator::register_m1_upstream(seams);
        assert_eq!(
            orch.holder_ids_in_order()[0],
            holder_ids::IDENTITY,
            "however the list is ordered, Identity (phase 0) is erased FIRST"
        );
        assert_eq!(
            orch.holder_ids_in_order().last(),
            Some(&holder_ids::BACKUP),
            "backups (phase 5) are always LAST"
        );
    }

    #[test]
    fn m1_holder_orchestration_floor_is_green_full_coverage_in_order() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 300);
        let holders = seam_holders(&kms);
        let orch = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let checklist = EraseChecklist::new();
        let scope = EraseScope::Subject {
            subject: subject("u-floor"),
            tenant: tenant.clone(),
        };

        assert_eq!(orch.fanout_coverage(&checklist), 0.0);

        let receipts = orch.fan_out_erase(&scope, &checklist).unwrap();
        assert_eq!(
            receipts.len(),
            6,
            "all six M1 upstream holders were reached"
        );

        assert_eq!(
            orch.fanout_coverage(&checklist),
            1.0,
            "erasure_fanout_coverage over the M1 holder set reads 100%"
        );

        for r in &receipts {
            assert_eq!(r.receipt.receipt.operation, "erase");
            assert!(r.receipt.receipt.content_hash.starts_with("blake3:"));
            assert!(
                r.receipt.receipt.key_epoch_destroyed.is_some(),
                "holder {} recorded the destroyed key epoch (the GD-4 audit trail)",
                r.holder_id
            );
        }

        for id in orch.holder_ids_in_order() {
            let handle = ShredKeyHandle {
                tenant: tenant.clone(),
                class: ShredKeyClass::Subject(id.to_string()),
            };
            assert_eq!(
                kms.recoverable_in_backup(&handle),
                0,
                "holder {id}: 0 recoverable after the canonical fan-out"
            );
        }
    }

    #[test]
    fn fan_out_is_resumable_redrives_only_un_receipted_holders() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 400);
        let holders = seam_holders(&kms);
        let orch = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let checklist = EraseChecklist::new();
        let scope = EraseScope::Subject {
            subject: subject("u-resume"),
            tenant: tenant.clone(),
        };

        let first_three: Vec<(&'static str, &dyn PersonalDataHolder)> = holders
            .iter()
            .filter(|(id, _)| {
                *id == holder_ids::IDENTITY
                    || *id == holder_ids::BLOB
                    || *id == holder_ids::AUTHZ_TUPLES
            })
            .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
            .collect();
        let partial = UpstreamHolderOrchestrator::register_m1_upstream(first_three);
        partial.fan_out_erase(&scope, &checklist).unwrap();
        assert_eq!(
            checklist.done_count(),
            3,
            "the crash left three holders receipted"
        );

        let calls_after_partial: BTreeMap<&str, u32> = holders
            .iter()
            .map(|(id, h)| (*id, h.erase_call_count()))
            .collect();

        let receipts = orch.fan_out_erase(&scope, &checklist).unwrap();

        for id in [
            holder_ids::IDENTITY,
            holder_ids::BLOB,
            holder_ids::AUTHZ_TUPLES,
        ] {
            let h = &holders.iter().find(|(hid, _)| *hid == id).unwrap().1;
            assert_eq!(
                h.erase_call_count(),
                calls_after_partial[id],
                "holder {id} was already receipted ⇒ NOT re-called on resume"
            );
        }
        for id in [holder_ids::BUS, holder_ids::CACHE, holder_ids::BACKUP] {
            let h = &holders.iter().find(|(hid, _)| *hid == id).unwrap().1;
            assert_eq!(h.erase_call_count(), 1, "holder {id} was driven on resume");
        }
        assert_eq!(receipts.len(), 6);
        assert_eq!(orch.fanout_coverage(&checklist), 1.0);
        assert_eq!(receipts[0].holder_id, holder_ids::IDENTITY);
    }

    #[test]
    fn re_running_a_complete_fan_out_is_an_idempotent_no_op() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 500);
        let holders = seam_holders(&kms);
        let orch = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let checklist = EraseChecklist::new();
        let scope = EraseScope::Subject {
            subject: subject("u-idem"),
            tenant: tenant.clone(),
        };
        let first = orch.fan_out_erase(&scope, &checklist).unwrap();
        let calls_after_first: Vec<u32> =
            holders.iter().map(|(_, h)| h.erase_call_count()).collect();

        let second = orch.fan_out_erase(&scope, &checklist).unwrap();
        let calls_after_second: Vec<u32> =
            holders.iter().map(|(_, h)| h.erase_call_count()).collect();

        assert_eq!(
            first, second,
            "an idempotent re-drive returns the SAME receipts"
        );
        assert_eq!(
            calls_after_first, calls_after_second,
            "no holder's erase was re-called on the idempotent re-drive"
        );
    }

    #[test]
    fn a_holder_error_fails_the_fan_out_but_leaves_a_resumable_checklist() {
        struct FailingHolder {
            calls: Mutex<u32>,
            fail: Mutex<bool>,
        }
        impl PersonalDataHolder for FailingHolder {
            fn locate(
                &self,
                _s: &SubjectRef,
                _t: TenantId,
            ) -> myelin_gdpr::Result<myelin_gdpr::LocateReport> {
                unreachable!()
            }
            fn export(
                &self,
                _s: &SubjectRef,
                _t: TenantId,
            ) -> myelin_gdpr::Result<myelin_gdpr::PortableBundle> {
                unreachable!()
            }
            fn rectify(
                &self,
                _s: &SubjectRef,
                _p: myelin_gdpr::Patch,
            ) -> myelin_gdpr::Result<myelin_gdpr::RectifyReceipt> {
                unreachable!()
            }
            fn restrict(
                &self,
                _s: &SubjectRef,
                _on: bool,
            ) -> myelin_gdpr::Result<myelin_gdpr::RestrictReceipt> {
                unreachable!()
            }
            fn erase(&self, _scope: EraseScope) -> myelin_gdpr::Result<EraseReceipt> {
                *self.calls.lock().unwrap() += 1;
                if *self.fail.lock().unwrap() {
                    return Err(DsrError("bus holder unavailable".into()));
                }
                Ok(EraseReceipt {
                    receipt: myelin_gdpr::Receipt::content_addressed(
                        "erase",
                        holder_ids::BUS,
                        "u-fail",
                        "acme",
                        "crypto_shred",
                        Some(9),
                        0,
                    ),
                })
            }
        }

        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 600);
        let id_h = SeamHolder::new(
            holder_ids::IDENTITY,
            ShredKeyClass::Subject(holder_ids::IDENTITY.into()),
            &kms,
        );
        let blob_h = SeamHolder::new(
            holder_ids::BLOB,
            ShredKeyClass::Subject(holder_ids::BLOB.into()),
            &kms,
        );
        let authz_h = SeamHolder::new(
            holder_ids::AUTHZ_TUPLES,
            ShredKeyClass::Subject(holder_ids::AUTHZ_TUPLES.into()),
            &kms,
        );
        let bus_h = FailingHolder {
            calls: Mutex::new(0),
            fail: Mutex::new(true),
        };
        let cache_h = SeamHolder::new(
            holder_ids::CACHE,
            ShredKeyClass::Subject(holder_ids::CACHE.into()),
            &kms,
        );
        let backup_h = SeamHolder::new(
            holder_ids::BACKUP,
            ShredKeyClass::Subject(holder_ids::BACKUP.into()),
            &kms,
        );

        let orch = UpstreamHolderOrchestrator::register_m1_upstream(vec![
            (holder_ids::IDENTITY, &id_h as &dyn PersonalDataHolder),
            (holder_ids::BLOB, &blob_h),
            (holder_ids::AUTHZ_TUPLES, &authz_h),
            (holder_ids::BUS, &bus_h),
            (holder_ids::CACHE, &cache_h),
            (holder_ids::BACKUP, &backup_h),
        ]);
        let checklist = EraseChecklist::new();
        let scope = EraseScope::Subject {
            subject: subject("u-fail"),
            tenant: tenant.clone(),
        };

        let err = orch.fan_out_erase(&scope, &checklist);
        assert!(err.is_err(), "a holder error fails the whole fan-out");
        assert_eq!(
            checklist.done_count(),
            3,
            "the pre-failure holders are receipted"
        );
        assert!(checklist.is_done(holder_ids::IDENTITY));
        assert!(checklist.is_done(holder_ids::AUTHZ_TUPLES));
        assert!(
            !checklist.is_done(holder_ids::BUS),
            "the failed holder is NOT receipted"
        );
        assert_eq!(
            cache_h.erase_call_count(),
            0,
            "we do not continue past a failed holder"
        );
        assert_eq!(backup_h.erase_call_count(), 0);

        *bus_h.fail.lock().unwrap() = false;
        let receipts = orch.fan_out_erase(&scope, &checklist).unwrap();
        assert_eq!(receipts.len(), 6, "the retry completes the erase");
        assert_eq!(orch.fanout_coverage(&checklist), 1.0);
        assert_eq!(id_h.erase_call_count(), 1);
        assert_eq!(authz_h.erase_call_count(), 1);
        assert_eq!(*bus_h.calls.lock().unwrap(), 2);
        assert_eq!(cache_h.erase_call_count(), 1);
        assert_eq!(backup_h.erase_call_count(), 1);
    }

    #[test]
    fn orchestrator_touches_holders_only_through_the_trait_object() {
        let tenant = t("acme");
        let kms = kms_with_all_holder_keys(&tenant, 700);
        let seam = SeamHolder::new(
            holder_ids::BLOB,
            ShredKeyClass::Subject(holder_ids::BLOB.into()),
            &kms,
        );
        let owned = crate::holders::GdprOwnStoreHolder::new(&kms);
        let registered = vec![
            RegisteredHolder {
                id: holder_ids::IDENTITY,
                phase: CanonicalErasePhase::IdentityPseudonymMap,
                holder: &owned as &dyn PersonalDataHolder,
            },
            RegisteredHolder {
                id: holder_ids::BLOB,
                phase: CanonicalErasePhase::CryptoShredDek,
                holder: &seam,
            },
        ];
        let orch = UpstreamHolderOrchestrator::new(registered);
        assert_eq!(orch.registered_count(), 2);
        assert_eq!(orch.holder_ids_in_order()[0], holder_ids::IDENTITY);
    }

    #[test]
    fn canonical_phase_map_is_pinned_for_the_six_m1_holders() {
        assert_eq!(
            canonical_phase_of(holder_ids::IDENTITY),
            Some(CanonicalErasePhase::IdentityPseudonymMap)
        );
        assert_eq!(
            canonical_phase_of(holder_ids::BLOB),
            Some(CanonicalErasePhase::CryptoShredDek)
        );
        assert_eq!(
            canonical_phase_of(holder_ids::AUTHZ_TUPLES),
            Some(CanonicalErasePhase::PurgeAndTombstoneDerived)
        );
        assert_eq!(
            canonical_phase_of(holder_ids::BUS),
            Some(CanonicalErasePhase::BusErase)
        );
        assert_eq!(
            canonical_phase_of(holder_ids::CACHE),
            Some(CanonicalErasePhase::CachesAndDerivedCopies)
        );
        assert_eq!(
            canonical_phase_of(holder_ids::BACKUP),
            Some(CanonicalErasePhase::Backups)
        );
        assert_eq!(canonical_phase_of("not_a_holder"), None);
        assert!(CanonicalErasePhase::IdentityPseudonymMap < CanonicalErasePhase::CryptoShredDek);
        assert!(CanonicalErasePhase::Backups > CanonicalErasePhase::CachesAndDerivedCopies);
    }

    #[test]
    fn telemetry_signal_names_and_units_are_pinned() {
        assert_eq!(ERASURE_FANOUT_COVERAGE.0, "gdpr.erasure_fanout_coverage");
        assert_eq!(ERASURE_FANOUT_COVERAGE.1, "ratio");
        assert_eq!(CRYPTO_SHRED_LAG.0, "gdpr.crypto_shred_lag");
        assert_eq!(CRYPTO_SHRED_LAG.1, "ms");
    }

    #[test]
    fn seam_holder_id_accessor_returns_the_holder_id() {
        let kms = InMemoryShredKms::new();
        let h = SeamHolder::new(holder_ids::BLOB, ShredKeyClass::Subject("u".into()), &kms);
        assert_eq!(h.id(), holder_ids::BLOB);
        assert_eq!(h.id(), "blob_store");
    }

    #[test]
    fn tenant_offboarding_fans_out_in_canonical_order() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        for id in [holder_ids::IDENTITY, holder_ids::BLOB, holder_ids::BACKUP] {
            kms.provision(
                ShredKeyHandle {
                    tenant: tenant.clone(),
                    class: ShredKeyClass::Subject(id.to_string()),
                },
                1,
            );
        }
        let holders: Vec<(&'static str, SeamHolder)> =
            [holder_ids::IDENTITY, holder_ids::BLOB, holder_ids::BACKUP]
                .into_iter()
                .map(|id| {
                    (
                        id,
                        SeamHolder::new(id, ShredKeyClass::Subject(id.to_string()), &kms),
                    )
                })
                .collect();
        let orch = UpstreamHolderOrchestrator::register_m1_upstream(
            holders
                .iter()
                .map(|(id, h)| (*id, h as &dyn PersonalDataHolder))
                .collect(),
        );
        let checklist = EraseChecklist::new();
        let receipts = orch
            .fan_out_erase(&EraseScope::Tenant(tenant.clone()), &checklist)
            .unwrap();
        assert_eq!(
            receipts[0].holder_id,
            holder_ids::IDENTITY,
            "Identity first for offboarding too"
        );
        assert_eq!(orch.fanout_coverage(&checklist), 1.0);
    }
}

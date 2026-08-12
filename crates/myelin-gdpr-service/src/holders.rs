use std::collections::BTreeSet;
use std::sync::Mutex;

use myelin_gdpr::{
    DsrError, EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle,
    Receipt, RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ShredKeyClass {
    Tenant,
    Subject(String),
    AuditKey,
}

impl ShredKeyClass {
    pub fn token(&self) -> String {
        match self {
            ShredKeyClass::Tenant => "tenant".to_string(),
            ShredKeyClass::Subject(id) => format!("subject:{id}"),
            ShredKeyClass::AuditKey => "audit_key".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShredKeyHandle {
    pub tenant: TenantId,
    pub class: ShredKeyClass,
}

pub trait CryptoShredKms {
    fn destroy(&self, handle: &ShredKeyHandle) -> Option<u64>;

    fn is_present(&self, handle: &ShredKeyHandle) -> bool;

    fn recoverable_in_backup(&self, handle: &ShredKeyHandle) -> usize;
}

#[derive(Debug, Default)]
pub struct InMemoryShredKms {
    keys: Mutex<std::collections::BTreeMap<ShredKeyHandle, u64>>,
}

impl InMemoryShredKms {
    pub fn new() -> InMemoryShredKms {
        InMemoryShredKms {
            keys: Mutex::new(std::collections::BTreeMap::new()),
        }
    }

    pub fn provision(&self, handle: ShredKeyHandle, epoch: u64) {
        self.keys.lock().unwrap().insert(handle, epoch);
    }
}

impl CryptoShredKms for InMemoryShredKms {
    fn destroy(&self, handle: &ShredKeyHandle) -> Option<u64> {
        self.keys.lock().unwrap().remove(handle)
    }
    fn is_present(&self, handle: &ShredKeyHandle) -> bool {
        self.keys.lock().unwrap().contains_key(handle)
    }
    fn recoverable_in_backup(&self, handle: &ShredKeyHandle) -> usize {
        usize::from(self.keys.lock().unwrap().contains_key(handle))
    }
}

pub const GDPR_OWN_STORE: &str = "gdpr_own_store";

pub struct GdprOwnStoreHolder<'a> {
    kms: &'a dyn CryptoShredKms,
}

impl<'a> GdprOwnStoreHolder<'a> {
    pub fn new(kms: &'a dyn CryptoShredKms) -> GdprOwnStoreHolder<'a> {
        GdprOwnStoreHolder { kms }
    }

    pub fn store_name(&self) -> &'static str {
        GDPR_OWN_STORE
    }

    fn subject_id(subject: &SubjectRef) -> String {
        subject.principal.principal_id.0.clone()
    }

    fn subject_key_handles(subject: &SubjectRef, tenant: &TenantId) -> Vec<ShredKeyHandle> {
        let sid = Self::subject_id(subject);
        vec![ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject(sid),
        }]
    }
}

impl PersonalDataHolder for GdprOwnStoreHolder<'_> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = Self::subject_id(subject);
        let recoverable = Self::subject_key_handles(subject, &tenant)
            .iter()
            .filter(|h| self.kms.is_present(h))
            .count();
        let outcome = if recoverable == 0 {
            "located:0-recoverable"
        } else {
            "located:present"
        };
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                GDPR_OWN_STORE,
                &sid,
                &tenant.0,
                outcome,
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        let sid = Self::subject_id(subject);
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                GDPR_OWN_STORE,
                &sid,
                &tenant.0,
                "exported",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        let sid = Self::subject_id(subject);
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                GDPR_OWN_STORE,
                &sid,
                "*",
                "rectified",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        let sid = Self::subject_id(subject);
        let outcome = if on {
            "restricted:set"
        } else {
            "restricted:clear"
        };
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                GDPR_OWN_STORE,
                &sid,
                "*",
                outcome,
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        match scope {
            EraseScope::Subject { subject, tenant } => {
                let sid = Self::subject_id(&subject);
                let handle = ShredKeyHandle {
                    tenant: tenant.clone(),
                    class: ShredKeyClass::Subject(sid.clone()),
                };
                let destroyed_epoch = self.kms.destroy(&handle);
                Ok(EraseReceipt {
                    receipt: Receipt::content_addressed(
                        "erase",
                        GDPR_OWN_STORE,
                        &sid,
                        &tenant.0,
                        "crypto_shred:subject_consent_dek",
                        destroyed_epoch,
                        0,
                    ),
                })
            }
            EraseScope::Tenant(tenant) => {
                let handle = ShredKeyHandle {
                    tenant: tenant.clone(),
                    class: ShredKeyClass::Tenant,
                };
                let destroyed_epoch = self.kms.destroy(&handle);
                Ok(EraseReceipt {
                    receipt: Receipt::content_addressed(
                        "erase",
                        GDPR_OWN_STORE,
                        "*tenant*",
                        &tenant.0,
                        "crypto_shred:tenant_register_dek",
                        destroyed_epoch,
                        0,
                    ),
                })
            }
        }
    }
}

pub const AUDIT_CARVE_OUT_STORE: &str = "audit_log_carve_out";

pub struct AuditCarveOutHolder<'a> {
    kms: &'a dyn CryptoShredKms,
}

impl<'a> AuditCarveOutHolder<'a> {
    pub fn new(kms: &'a dyn CryptoShredKms) -> AuditCarveOutHolder<'a> {
        AuditCarveOutHolder { kms }
    }

    pub fn store_name(&self) -> &'static str {
        AUDIT_CARVE_OUT_STORE
    }
}

impl PersonalDataHolder for AuditCarveOutHolder<'_> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                AUDIT_CARVE_OUT_STORE,
                &sid,
                &tenant.0,
                "located:minimised-pseudonym-record",
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                AUDIT_CARVE_OUT_STORE,
                &sid,
                &tenant.0,
                "exported:minimised",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, _subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Err(DsrError(
            "audit carve-out (H16): an audit entry is NEVER rewritten/rectified - that breaks the \
             Haber–Stornetta hash-chain (gdpr §6.4). The real identity was never in the entry (it \
             lived in Id's erasable pseudonym map); rectification of identity is the pseudonym \
             shred, not an entry edit."
                .to_string(),
        ))
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if on {
            "restricted:set"
        } else {
            "restricted:clear"
        };
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                AUDIT_CARVE_OUT_STORE,
                &sid,
                "*",
                outcome,
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        match scope {
            EraseScope::Subject { subject, tenant } => {
                let sid = subject.principal.principal_id.0.clone();
                Ok(EraseReceipt {
                    receipt: Receipt::content_addressed(
                        "erase",
                        AUDIT_CARVE_OUT_STORE,
                        &sid,
                        &tenant.0,
                        "carve_out:retained-minimised-record:never-rewritten",
                        None,
                        0,
                    ),
                })
            }
            EraseScope::Tenant(tenant) => {
                let handle = ShredKeyHandle {
                    tenant: tenant.clone(),
                    class: ShredKeyClass::AuditKey,
                };
                let destroyed_epoch = self.kms.destroy(&handle);
                Ok(EraseReceipt {
                    receipt: Receipt::content_addressed(
                        "erase",
                        AUDIT_CARVE_OUT_STORE,
                        "*tenant*",
                        &tenant.0,
                        "carve_out:audit_key_crypto_shred:never-rewritten",
                        destroyed_epoch,
                        0,
                    ),
                })
            }
        }
    }
}

pub fn gdpr_owned_holder_ids() -> BTreeSet<&'static str> {
    [GDPR_OWN_STORE, AUDIT_CARVE_OUT_STORE]
        .into_iter()
        .collect()
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

    fn kms_with_subject_dek(
        subject: &SubjectRef,
        tenant: &TenantId,
        epoch: u64,
    ) -> InMemoryShredKms {
        let kms = InMemoryShredKms::new();
        kms.provision(
            ShredKeyHandle {
                tenant: tenant.clone(),
                class: ShredKeyClass::Subject(subject.principal.principal_id.0.clone()),
            },
            epoch,
        );
        kms
    }

    #[test]
    fn h18_erase_crypto_shreds_and_locate_then_finds_zero_recoverable() {
        let tenant = t("acme");
        let subj = subject("u-1");
        let kms = kms_with_subject_dek(&subj, &tenant, 7);
        let holder = GdprOwnStoreHolder::new(&kms);

        let before = holder.locate(&subj, tenant.clone()).unwrap();
        assert_eq!(
            before.receipt.operation, "locate",
            "locate receipt names the op"
        );

        let scope = EraseScope::Subject {
            subject: subj.clone(),
            tenant: tenant.clone(),
        };
        let receipt = holder.erase(scope).unwrap();
        assert_eq!(
            receipt.receipt.key_epoch_destroyed,
            Some(7),
            "the erase receipt records the destroyed key epoch (the GD-4 audit trail)"
        );
        assert!(receipt.receipt.content_hash.starts_with("blake3:"));

        let handle = ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject(subj.principal.principal_id.0.clone()),
        };
        assert!(
            !kms.is_present(&handle),
            "the consent DEK is destroyed (live)"
        );
        assert_eq!(
            kms.recoverable_in_backup(&handle),
            0,
            "0 recoverable in backup (§7.5: destroyed AND excluded from backup)"
        );

        let after = holder.locate(&subj, tenant.clone()).unwrap();
        assert_ne!(
            after.receipt.content_hash, before.receipt.content_hash,
            "the post-erase locate verdict differs (0-recoverable vs present)"
        );
    }

    #[test]
    fn h18_erase_is_idempotent_returning_the_same_receipt() {
        let tenant = t("acme");
        let subj = subject("u-twice");
        let kms = kms_with_subject_dek(&subj, &tenant, 3);
        let holder = GdprOwnStoreHolder::new(&kms);

        let scope = || EraseScope::Subject {
            subject: subj.clone(),
            tenant: tenant.clone(),
        };
        let r1 = holder.erase(scope()).unwrap();
        assert_eq!(r1.receipt.key_epoch_destroyed, Some(3));

        let r2 = holder.erase(scope()).unwrap();
        assert_eq!(
            r2.receipt.key_epoch_destroyed, None,
            "a re-erase destroys nothing (the key was already gone) - a no-op success"
        );
        let handle = ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject(subj.principal.principal_id.0.clone()),
        };
        assert_eq!(kms.recoverable_in_backup(&handle), 0);
    }

    #[test]
    fn h18_re_erase_with_a_stable_destroyed_epoch_is_byte_identical() {
        struct ReaffirmKms;
        impl CryptoShredKms for ReaffirmKms {
            fn destroy(&self, _h: &ShredKeyHandle) -> Option<u64> {
                Some(9)
            }
            fn is_present(&self, _h: &ShredKeyHandle) -> bool {
                false
            }
            fn recoverable_in_backup(&self, _h: &ShredKeyHandle) -> usize {
                0
            }
        }
        let holder = GdprOwnStoreHolder::new(&ReaffirmKms);
        let tenant = t("acme");
        let subj = subject("u-stable");
        let scope = || EraseScope::Subject {
            subject: subj.clone(),
            tenant: tenant.clone(),
        };
        let r1 = holder.erase(scope()).unwrap();
        let r2 = holder.erase(scope()).unwrap();
        assert_eq!(
            r1.receipt, r2.receipt,
            "an idempotent re-erase returns the SAME receipt"
        );
        assert_eq!(r1.receipt.key_epoch_destroyed, Some(9));
    }

    #[test]
    fn h18_tenant_offboarding_shreds_the_tenant_register_dek() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        kms.provision(
            ShredKeyHandle {
                tenant: tenant.clone(),
                class: ShredKeyClass::Tenant,
            },
            42,
        );
        let holder = GdprOwnStoreHolder::new(&kms);
        let receipt = holder.erase(EraseScope::Tenant(tenant.clone())).unwrap();
        assert_eq!(receipt.receipt.key_epoch_destroyed, Some(42));
        let handle = ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Tenant,
        };
        assert_eq!(
            kms.recoverable_in_backup(&handle),
            0,
            "tenant register DEK gone"
        );
    }

    #[test]
    fn h18_non_erase_ops_return_content_addressed_receipts_without_a_destroyed_epoch() {
        let tenant = t("acme");
        let subj = subject("u-ops");
        let kms = kms_with_subject_dek(&subj, &tenant, 1);
        let holder = GdprOwnStoreHolder::new(&kms);
        for r in [
            holder.locate(&subj, tenant.clone()).unwrap().receipt,
            holder.export(&subj, tenant.clone()).unwrap().receipt,
            holder.rectify(&subj, Patch("p".into())).unwrap().receipt,
            holder.restrict(&subj, true).unwrap().receipt,
        ] {
            assert!(
                r.content_hash.starts_with("blake3:"),
                "{} is content-addressed",
                r.operation
            );
            assert_eq!(
                r.key_epoch_destroyed, None,
                "a non-erase op destroys no key"
            );
        }
    }

    #[test]
    fn h16_erase_retains_the_minimised_record_and_never_rewrites() {
        let kms = InMemoryShredKms::new();
        let holder = AuditCarveOutHolder::new(&kms);
        let tenant = t("acme");
        let subj = subject("u-audit");
        let receipt = holder
            .erase(EraseScope::Subject {
                subject: subj.clone(),
                tenant: tenant.clone(),
            })
            .unwrap();
        assert_eq!(receipt.receipt.operation, "erase");
        assert_eq!(receipt.receipt.key_epoch_destroyed, None);
        assert!(receipt.receipt.content_hash.starts_with("blake3:"));
    }

    #[test]
    fn h16_rectify_is_refused_as_a_chain_break() {
        let kms = InMemoryShredKms::new();
        let holder = AuditCarveOutHolder::new(&kms);
        let subj = subject("u-rect");
        match holder.rectify(&subj, Patch("x".into())) {
            Err(DsrError(msg)) => assert!(
                msg.contains("NEVER rewritten") && msg.contains("hash-chain"),
                "an audit rectify must be refused as a chain-break: {msg}"
            ),
            Ok(_) => panic!("the audit log must NEVER rewrite an entry (gdpr §6.4)"),
        }
    }

    #[test]
    fn h16_tenant_offboarding_expires_via_audit_key_crypto_shred() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        kms.provision(
            ShredKeyHandle {
                tenant: tenant.clone(),
                class: ShredKeyClass::AuditKey,
            },
            5,
        );
        let holder = AuditCarveOutHolder::new(&kms);
        let receipt = holder.erase(EraseScope::Tenant(tenant.clone())).unwrap();
        assert_eq!(receipt.receipt.key_epoch_destroyed, Some(5));
        let handle = ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::AuditKey,
        };
        assert_eq!(
            kms.recoverable_in_backup(&handle),
            0,
            "audit-key shredded at retention end"
        );
    }

    #[test]
    fn recoverable_in_backup_reads_nonzero_for_a_present_key() {
        let kms = InMemoryShredKms::new();
        let handle = ShredKeyHandle {
            tenant: t("acme"),
            class: ShredKeyClass::Subject("u-present".into()),
        };
        kms.provision(handle.clone(), 1);
        assert_eq!(
            kms.recoverable_in_backup(&handle),
            1,
            "a present key IS recoverable in backup"
        );
        assert!(kms.is_present(&handle));
        assert_eq!(
            kms.destroy(&handle),
            Some(1),
            "destroy returns the destroyed epoch"
        );
        assert_eq!(
            kms.recoverable_in_backup(&handle),
            0,
            "destroyed ⇒ 0 recoverable"
        );
        assert!(!kms.is_present(&handle));
    }

    #[test]
    fn h18_locate_verdict_distinguishes_present_from_zero_recoverable() {
        let tenant = t("acme");
        let subj = subject("u-loc");
        let kms = kms_with_subject_dek(&subj, &tenant, 1);
        let holder = GdprOwnStoreHolder::new(&kms);
        let present = holder.locate(&subj, tenant.clone()).unwrap().receipt;
        holder
            .erase(EraseScope::Subject {
                subject: subj.clone(),
                tenant: tenant.clone(),
            })
            .unwrap();
        let after = holder.locate(&subj, tenant.clone()).unwrap().receipt;
        assert_ne!(
            present.content_hash, after.content_hash,
            "the present verdict and the 0-recoverable verdict must differ (the `== 0` branch is \
             load-bearing)"
        );

        let sid = subj.principal.principal_id.0.clone();
        let expect_present = Receipt::content_addressed(
            "locate",
            GDPR_OWN_STORE,
            &sid,
            &tenant.0,
            "located:present",
            None,
            0,
        );
        let expect_zero = Receipt::content_addressed(
            "locate",
            GDPR_OWN_STORE,
            &sid,
            &tenant.0,
            "located:0-recoverable",
            None,
            0,
        );
        assert_eq!(
            present, expect_present,
            "a present DEK ⇒ the `located:present` verdict"
        );
        assert_eq!(
            after, expect_zero,
            "an erased DEK ⇒ the `located:0-recoverable` verdict"
        );
    }

    #[test]
    fn key_class_tokens_and_holder_names_are_stable() {
        assert_eq!(ShredKeyClass::Tenant.token(), "tenant");
        assert_eq!(ShredKeyClass::Subject("u".into()).token(), "subject:u");
        assert_eq!(ShredKeyClass::AuditKey.token(), "audit_key");
        let kms = InMemoryShredKms::new();
        assert_eq!(GdprOwnStoreHolder::new(&kms).store_name(), GDPR_OWN_STORE);
        assert_eq!(GdprOwnStoreHolder::new(&kms).store_name(), "gdpr_own_store");
        assert_eq!(
            AuditCarveOutHolder::new(&kms).store_name(),
            AUDIT_CARVE_OUT_STORE
        );
        assert_eq!(
            AuditCarveOutHolder::new(&kms).store_name(),
            "audit_log_carve_out"
        );
    }

    #[test]
    fn the_gdpr_owned_holder_set_is_h18_and_h16() {
        let ids = gdpr_owned_holder_ids();
        assert!(
            ids.contains(GDPR_OWN_STORE),
            "H18 (GDPR own stores) is covered"
        );
        assert!(
            ids.contains(AUDIT_CARVE_OUT_STORE),
            "H16 (audit carve-out) is covered"
        );
        assert_eq!(
            ids.len(),
            2,
            "P-GA-05 covers exactly the GDPR-OWNED holders (H18 + H16)"
        );
    }

    #[test]
    fn holders_are_object_safe_behind_dyn() {
        let kms = InMemoryShredKms::new();
        let h18 = GdprOwnStoreHolder::new(&kms);
        let h16 = AuditCarveOutHolder::new(&kms);
        let holders: Vec<&dyn PersonalDataHolder> = vec![&h18, &h16];
        let subj = subject("u-dyn");
        for h in holders {
            assert!(h.locate(&subj, t("acme")).is_ok());
        }
    }
}

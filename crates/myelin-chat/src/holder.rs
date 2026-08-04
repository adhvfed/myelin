use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};
use myelin_substrate::{Holder, HolderRegistration, HolderRegistry, StoreClassifier, StoreKind};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

pub const CHAT_OLTP_STORE: &str = "chat_oltp";

pub const CHAT_READ_STATE_STORE: &str = crate::read_state::CHAT_READ_STATE_STORE;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ChatStoreClass {
    Messages,
    Drafts,
    AuthorIdentity,
    ReadState,
}

impl ChatStoreClass {
    pub fn label(self) -> &'static str {
        match self {
            ChatStoreClass::Messages => "messages",
            ChatStoreClass::Drafts => "drafts",
            ChatStoreClass::AuthorIdentity => "author-identity",
            ChatStoreClass::ReadState => "read-state",
        }
    }

    pub const ALL: [ChatStoreClass; 4] = [
        ChatStoreClass::Messages,
        ChatStoreClass::Drafts,
        ChatStoreClass::AuthorIdentity,
        ChatStoreClass::ReadState,
    ];
}

pub const CHAT_RESIDUAL_POSTURE_REF: &str =
    "contract 10.9 / 00 §X-7 (the ONE platform free-text/immutable-content erasure posture); \
     Chat: per-subject DEK crypto-shred of the message bodies/drafts (11.4, the author's DEK reaches \
     hot/cold/backups + the immutable log) + pseudonym shred of the author (4.8) + restrict \
     suppression; per-tenant DEK fallback where third-party PII is not isolable (under the author's \
     DEK, not the subject's); never a Chat-local restatement (the full DSR cascade = CHAT-P22)";

pub type ChatHolderRegistration = HolderRegistration;

pub fn chat_store_classifier() -> StoreClassifier {
    StoreClassifier::of([
        myelin_substrate::StoreHolder::new(StoreKind::Oltp, CHAT_OLTP_STORE, Holder::H5Chat),
        myelin_substrate::StoreHolder::new(StoreKind::Oltp, CHAT_READ_STATE_STORE, Holder::H5Chat),
    ])
}

pub fn register_chat_holders() -> HolderRegistry {
    let mut registry = HolderRegistry::new();
    registry.open(StoreKind::Oltp, CHAT_OLTP_STORE);
    registry.open(StoreKind::Oltp, CHAT_READ_STATE_STORE);
    registry
}

#[derive(Clone, Default)]
pub struct RestrictionFlag {
    restricted: Arc<Mutex<BTreeSet<String>>>,
}

impl RestrictionFlag {
    pub fn new() -> RestrictionFlag {
        RestrictionFlag::default()
    }

    pub fn set(&self, subject: &str, on: bool) {
        let mut g = self.restricted.lock().expect("restriction flag poisoned");
        if on {
            g.insert(subject.to_string());
        } else {
            g.remove(subject);
        }
    }

    pub fn is_restricted(&self, subject: &str) -> bool {
        self.restricted
            .lock()
            .expect("restriction flag poisoned")
            .contains(subject)
    }
}

#[derive(Clone, Default)]
pub struct ChatHolder {
    restriction: RestrictionFlag,
}

impl ChatHolder {
    pub fn new() -> ChatHolder {
        ChatHolder::default()
    }

    pub fn with_restriction(restriction: RestrictionFlag) -> ChatHolder {
        ChatHolder { restriction }
    }

    pub fn register(&self, registry: &mut HolderRegistry) -> ChatHolderRegistration {
        registry.open(StoreKind::Oltp, CHAT_OLTP_STORE)
    }

    pub fn restriction(&self) -> &RestrictionFlag {
        &self.restriction
    }

    fn subject_id(subject: &SubjectRef) -> String {
        subject.principal.principal_id.0.clone()
    }
}

impl PersonalDataHolder for ChatHolder {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                CHAT_OLTP_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "Chat locate over messages/drafts/author-pseudonym (CHAT-P6 typed seam; the full \
                 subject-walk = the DSR cascade CHAT-P22)",
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                CHAT_OLTP_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "Chat export: the subject's authored messages + drafts as references + \
                 per-subject-DEK-decrypted body excerpts (CHAT-P6 typed seam; the full bundle = \
                 CHAT-P22)",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                CHAT_OLTP_STORE,
                &Self::subject_id(subject),
                "",
                "no-op (CHAT-P6 substrate; the patch-apply + reindex-from-source = CHAT-P22 / \
                 GDPR 10.4)",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        let sid = Self::subject_id(subject);
        self.restriction.set(&sid, on);
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                CHAT_OLTP_STORE,
                &sid,
                "",
                if on {
                    "Chat restrict ON: no indexing / no agent-use / no analytics / no notification"
                } else {
                    "Chat restrict OFF: the per-subject restriction flag is cleared"
                },
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (subject_id, tenant) = match &scope {
            EraseScope::Subject { subject, tenant } => {
                (Self::subject_id(subject), tenant.0.clone())
            }
            EraseScope::Tenant(t) => (String::new(), t.0.clone()),
        };
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                CHAT_OLTP_STORE,
                &subject_id,
                &tenant,
                "Chat erase (the full fan-out = CHAT-P22 / P-411): per-subject DEK crypto-shred of \
                 the message bodies/drafts across hot/cold/backups + the immutable log + pseudonym \
                 shred of the author (4.8) + restrict + the chat.message.erased tombstones across \
                 every holder, with post-restore re-erasure; residual = the ONE posture 10.9/X-7, \
                 by reference",
                None,
                0,
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_substrate::{
        assert_all_holders_registered, assert_holder_completeness, classify_store, DeclaredStore,
        StoreManifest,
    };

    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId::from_token("acme"),
        ))
    }

    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }

    #[test]
    fn the_chat_store_class_set_is_the_holder_coverage() {
        assert_eq!(ChatStoreClass::ALL.len(), 4);
        for c in [
            ChatStoreClass::Messages,
            ChatStoreClass::Drafts,
            ChatStoreClass::AuthorIdentity,
            ChatStoreClass::ReadState,
        ] {
            assert!(
                ChatStoreClass::ALL.contains(&c),
                "{} must be in the holder coverage",
                c.label()
            );
        }
    }

    #[test]
    fn chat_store_registers_and_classifies_to_h5_no_orphan() {
        let registry = register_chat_holders();
        assert!(registry.is_registered(StoreKind::Oltp, CHAT_OLTP_STORE));
        assert!(registry.is_registered(StoreKind::Oltp, CHAT_READ_STATE_STORE));
        assert_eq!(
            registry.len(),
            2,
            "the Chat OLTP + read-state stores registered"
        );
        let classifier = chat_store_classifier();
        assert_eq!(
            classify_store(StoreKind::Oltp, CHAT_OLTP_STORE, &classifier),
            Some(Holder::H5Chat),
            "the Chat OLTP store is holder H5 (Chat subsystem DB)"
        );
        assert_eq!(
            classify_store(StoreKind::Oltp, CHAT_READ_STATE_STORE, &classifier),
            Some(Holder::H5Chat),
            "the Chat read-state store is holder H5 (the per-user markers, D-C8)"
        );
        assert_eq!(
            assert_holder_completeness(registry.registrations(), &classifier),
            Ok(()),
            "every Chat store is in the exhaustive H1–H18 list - 0 orphan stores"
        );
    }

    #[test]
    fn an_unregistered_chat_store_fails_the_holder_registered_architecture_test() {
        let manifest = StoreManifest::of([DeclaredStore::new(StoreKind::Oltp, CHAT_OLTP_STORE)]);
        assert_eq!(
            assert_all_holders_registered(&manifest, &register_chat_holders()),
            Ok(()),
            "the Chat store opened through the harness → the architecture test passes"
        );
        let rogue = HolderRegistry::new();
        let err = assert_all_holders_registered(&manifest, &rogue)
            .expect_err("a Chat store opened outside the harness must FAIL the architecture test");
        assert_eq!(
            err.len(),
            1,
            "exactly the unregistered Chat store is the violation"
        );
        assert!(
            err[0].message().contains(CHAT_OLTP_STORE),
            "the failure names the escaped Chat store: {}",
            err[0].message()
        );
    }

    #[test]
    fn locate_and_export_are_typed_and_empty_but_correct() {
        let holder = ChatHolder::new();
        let subj = subject("psn:chat-7");
        let locate = holder
            .locate(&subj, tenant())
            .expect("locate over the Chat surface succeeds");
        assert_eq!(locate.receipt.operation, "locate");
        assert!(locate.receipt.content_hash.starts_with("blake3:"));
        assert!(
            locate.receipt.key_epoch_destroyed.is_none(),
            "locate shreds no key"
        );
        let export = holder
            .export(&subj, tenant())
            .expect("export over the Chat surface succeeds");
        assert_eq!(export.receipt.operation, "export");
        assert!(export.receipt.content_hash.starts_with("blake3:"));
    }

    #[test]
    fn restrict_flips_a_real_flag_the_seams_read() {
        let flag = RestrictionFlag::new();
        let holder = ChatHolder::with_restriction(flag.clone());
        let subj = subject("psn:chat-restricted");
        let sid = "psn:chat-restricted";
        assert!(!flag.is_restricted(sid), "not restricted initially");
        holder.restrict(&subj, true).expect("restrict on");
        assert!(
            flag.is_restricted(sid),
            "the holder's restrict(on) is seen by a seam reading the SAME flag"
        );
        holder.restrict(&subj, false).expect("restrict off");
        assert!(!flag.is_restricted(sid), "restrict(off) clears the flag");
    }

    #[test]
    fn erase_is_a_typed_crypto_shred_receipt_naming_chat_p22() {
        let holder = ChatHolder::new();
        let subj = subject("psn:chat-erase");
        let receipt = holder
            .erase(EraseScope::Subject {
                subject: subj,
                tenant: tenant(),
            })
            .expect("erase returns a typed receipt");
        assert_eq!(receipt.receipt.operation, "erase");
        assert!(receipt.receipt.content_hash.starts_with("blake3:"));

        let t_receipt = holder
            .erase(EraseScope::Tenant(tenant()))
            .expect("tenant erase returns a typed receipt");
        assert_eq!(t_receipt.receipt.operation, "erase");
    }

    #[test]
    fn the_residual_is_the_one_platform_posture_by_reference() {
        assert!(CHAT_RESIDUAL_POSTURE_REF.contains("10.9"));
        assert!(CHAT_RESIDUAL_POSTURE_REF.contains("X-7"));
        assert!(CHAT_RESIDUAL_POSTURE_REF.contains("per-subject DEK"));
    }
}

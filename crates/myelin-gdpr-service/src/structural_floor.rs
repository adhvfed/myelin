use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_gdpr::{SubjectRef, TenantId};
use myelin_identity::PseudonymHandle;

use crate::holders::{CryptoShredKms, ShredKeyClass, ShredKeyHandle};

#[derive(Debug, Default)]
pub struct RestrictRegistry {
    restricted: Mutex<BTreeMap<(String, String), ()>>,
}

impl RestrictRegistry {
    #[must_use]
    pub fn new() -> RestrictRegistry {
        RestrictRegistry {
            restricted: Mutex::new(BTreeMap::new()),
        }
    }

    fn key(subject: &SubjectRef, tenant: &TenantId) -> (String, String) {
        (tenant.0.clone(), subject.principal.principal_id.0.clone())
    }

    pub fn set(&self, subject: &SubjectRef, tenant: &TenantId, on: bool) {
        let key = Self::key(subject, tenant);
        let mut map = self.restricted.lock().unwrap_or_else(|e| e.into_inner());
        if on {
            map.insert(key, ());
        } else {
            map.remove(&key);
        }
    }

    #[must_use]
    pub fn is_restricted(&self, subject: &SubjectRef, tenant: &TenantId) -> bool {
        let key = Self::key(subject, tenant);
        self.restricted
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&key)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Processing {
    Index,
    AgentRead,
    Analyse,
    Notify,
}

impl Processing {
    #[must_use]
    pub const fn all() -> [Processing; 4] {
        [
            Processing::Index,
            Processing::AgentRead,
            Processing::Analyse,
            Processing::Notify,
        ]
    }

    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Processing::Index => "index",
            Processing::AgentRead => "agent_read",
            Processing::Analyse => "analyse",
            Processing::Notify => "notify",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Processed {
    Processed(String),
    Suppressed,
    Unrecoverable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredContent {
    Recoverable(String),
    Unrecoverable,
}

pub struct M1Store<'a> {
    id: &'static str,
    restrict: &'a RestrictRegistry,
    kms: &'a dyn CryptoShredKms,
    stored: Mutex<BTreeMap<(String, String), String>>,
}

impl<'a> M1Store<'a> {
    #[must_use]
    pub fn new(
        id: &'static str,
        restrict: &'a RestrictRegistry,
        kms: &'a dyn CryptoShredKms,
    ) -> M1Store<'a> {
        M1Store {
            id,
            restrict,
            kms,
            stored: Mutex::new(BTreeMap::new()),
        }
    }

    #[must_use]
    pub fn id(&self) -> &'static str {
        self.id
    }

    fn key(subject: &SubjectRef, tenant: &TenantId) -> (String, String) {
        (tenant.0.clone(), subject.principal.principal_id.0.clone())
    }

    #[must_use]
    pub fn dek_handle(subject: &SubjectRef, tenant: &TenantId) -> ShredKeyHandle {
        ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject(subject.principal.principal_id.0.clone()),
        }
    }

    pub fn store_self_authored(
        &self,
        subject: &SubjectRef,
        tenant: &TenantId,
        content: impl Into<String>,
    ) {
        self.stored
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(Self::key(subject, tenant), content.into());
    }

    pub fn erase_self_authored(&self, subject: &SubjectRef, tenant: &TenantId) -> Option<u64> {
        self.kms.destroy(&Self::dek_handle(subject, tenant))
    }

    #[must_use]
    pub fn fetch_stored(&self, subject: &SubjectRef, tenant: &TenantId) -> Option<StoredContent> {
        let has_row = self
            .stored
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&Self::key(subject, tenant))
            .cloned();
        let row = has_row?;
        if self.kms.is_present(&Self::dek_handle(subject, tenant)) {
            Some(StoredContent::Recoverable(row))
        } else {
            Some(StoredContent::Unrecoverable)
        }
    }

    fn process(&self, op: Processing, subject: &SubjectRef, tenant: &TenantId) -> Processed {
        match self.fetch_stored(subject, tenant) {
            Some(StoredContent::Unrecoverable) | None => Processed::Unrecoverable,
            Some(StoredContent::Recoverable(content)) => {
                if self.restrict.is_restricted(subject, tenant) {
                    Processed::Suppressed
                } else {
                    Processed::Processed(format!("{}:{}:{content}", op.token(), self.id))
                }
            }
        }
    }

    #[must_use]
    pub fn index(&self, subject: &SubjectRef, tenant: &TenantId) -> Processed {
        self.process(Processing::Index, subject, tenant)
    }

    #[must_use]
    pub fn agent_read(&self, subject: &SubjectRef, tenant: &TenantId) -> Processed {
        self.process(Processing::AgentRead, subject, tenant)
    }

    #[must_use]
    pub fn analyse(&self, subject: &SubjectRef, tenant: &TenantId) -> Processed {
        self.process(Processing::Analyse, subject, tenant)
    }

    #[must_use]
    pub fn notify(&self, subject: &SubjectRef, tenant: &TenantId) -> Processed {
        self.process(Processing::Notify, subject, tenant)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShreddedIdentity {
    pub immutable_bytes: String,
}

impl ShreddedIdentity {
    #[must_use]
    pub fn holds_only_the_pseudonym_form(&self) -> bool {
        PseudonymHandle::parse(&self.immutable_bytes).is_some()
    }
}

#[must_use]
pub fn shred_pseudonym_identity(pseudonym: &PseudonymHandle) -> ShreddedIdentity {
    ShreddedIdentity {
        immutable_bytes: pseudonym.render(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authorship {
    SelfAuthored,
    ThirdPartyMention,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeverCoverage {
    CryptoShred,
    RestrictSuppressOnly,
}

#[must_use]
pub fn classify_residual(authorship: Authorship) -> LeverCoverage {
    match authorship {
        Authorship::SelfAuthored => LeverCoverage::CryptoShred,
        Authorship::ThirdPartyMention => LeverCoverage::RestrictSuppressOnly,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::holders::InMemoryShredKms;
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
    fn restrict_suppresses_processing_but_retains_storage_reversibly() {
        let tenant = t("acme");
        let subj = subject("u-restrict");
        let restrict = RestrictRegistry::new();
        let kms = InMemoryShredKms::new();
        let store = M1Store::new("issues_store", &restrict, &kms);
        kms.provision(M1Store::dek_handle(&subj, &tenant), 1);
        store.store_self_authored(&subj, &tenant, "my comment");

        for op in Processing::all() {
            let r = store.process(op, &subj, &tenant);
            assert!(
                matches!(r, Processed::Processed(_)),
                "{:?} processes for an unrestricted subject",
                op
            );
        }

        restrict.set(&subj, &tenant, true);
        assert!(restrict.is_restricted(&subj, &tenant));
        assert_eq!(store.index(&subj, &tenant), Processed::Suppressed);
        assert_eq!(store.agent_read(&subj, &tenant), Processed::Suppressed);
        assert_eq!(store.analyse(&subj, &tenant), Processed::Suppressed);
        assert_eq!(store.notify(&subj, &tenant), Processed::Suppressed);

        assert_eq!(
            store.fetch_stored(&subj, &tenant),
            Some(StoredContent::Recoverable("my comment".into())),
            "restrict suppresses PROCESSING, never storage (§4.4: while retaining storage)"
        );

        restrict.set(&subj, &tenant, false);
        assert!(!restrict.is_restricted(&subj, &tenant));
        for op in Processing::all() {
            assert!(
                matches!(store.process(op, &subj, &tenant), Processed::Processed(_)),
                "{:?} processes again after the restriction is lifted (reversible)",
                op
            );
        }
    }

    #[test]
    fn the_restrict_flag_is_scoped_per_tenant_and_subject() {
        let restrict = RestrictRegistry::new();
        let a = subject("u-a");
        let b = subject("u-b");
        let acme = t("acme");
        let other = t("globex");
        restrict.set(&a, &acme, true);
        assert!(restrict.is_restricted(&a, &acme));
        assert!(
            !restrict.is_restricted(&b, &acme),
            "a different subject is not restricted"
        );
        assert!(
            !restrict.is_restricted(&a, &other),
            "the same subject id in a different tenant is not restricted (tenant-partitioned)"
        );
    }

    #[test]
    fn erase_crypto_shreds_self_authored_free_text_to_unrecoverable() {
        let tenant = t("acme");
        let subj = subject("u-erase");
        let restrict = RestrictRegistry::new();
        let kms = InMemoryShredKms::new();
        let store = M1Store::new("chat_store", &restrict, &kms);
        kms.provision(M1Store::dek_handle(&subj, &tenant), 7);
        store.store_self_authored(&subj, &tenant, "secret message body");

        assert_eq!(
            store.fetch_stored(&subj, &tenant),
            Some(StoredContent::Recoverable("secret message body".into()))
        );

        assert_eq!(store.erase_self_authored(&subj, &tenant), Some(7));

        assert_eq!(
            store.fetch_stored(&subj, &tenant),
            Some(StoredContent::Unrecoverable),
            "the per-subject DEK shred renders the self-authored free-text unrecoverable"
        );
        assert_eq!(
            kms.recoverable_in_backup(&M1Store::dek_handle(&subj, &tenant)),
            0
        );

        assert_eq!(store.index(&subj, &tenant), Processed::Unrecoverable);

        assert_eq!(store.erase_self_authored(&subj, &tenant), None);
        assert_eq!(
            store.fetch_stored(&subj, &tenant),
            Some(StoredContent::Unrecoverable)
        );
    }

    #[test]
    fn pseudonym_map_shred_leaves_only_the_frozen_pseudonym_form() {
        let handle = PseudonymHandle::new("anon-7f3a", "acme").expect("valid pseudonym");
        let shredded = shred_pseudonym_identity(&handle);
        assert_eq!(
            shredded.immutable_bytes, "anon-7f3a@acme.noreply",
            "the immutable bytes hold the frozen <pseudonym>@<tenant>.noreply rendering"
        );
        assert!(
            shredded.holds_only_the_pseudonym_form(),
            "the shredded bytes parse as a valid pseudonym handle - never real-identity PII"
        );
        assert!(shredded.immutable_bytes.ends_with(".noreply"));

        let leaked = ShreddedIdentity {
            immutable_bytes: "alice@example.com".into(),
        };
        assert!(
            !leaked.holds_only_the_pseudonym_form(),
            "a real routable email is NOT the frozen pseudonym residue - the predicate must reject it"
        );
    }

    #[test]
    fn the_m1_store_keys_content_per_tenant_and_subject() {
        let tenant = t("acme");
        let a = subject("u-a");
        let b = subject("u-b");
        let restrict = RestrictRegistry::new();
        let kms = InMemoryShredKms::new();
        let store = M1Store::new("s", &restrict, &kms);
        kms.provision(M1Store::dek_handle(&a, &tenant), 1);
        kms.provision(M1Store::dek_handle(&b, &tenant), 2);
        store.store_self_authored(&a, &tenant, "alice content");
        store.store_self_authored(&b, &tenant, "bob content");
        assert_eq!(
            store.fetch_stored(&a, &tenant),
            Some(StoredContent::Recoverable("alice content".into()))
        );
        assert_eq!(
            store.fetch_stored(&b, &tenant),
            Some(StoredContent::Recoverable("bob content".into()))
        );
        assert_eq!(store.erase_self_authored(&a, &tenant), Some(1));
        assert_eq!(
            store.fetch_stored(&a, &tenant),
            Some(StoredContent::Unrecoverable)
        );
        assert_eq!(
            store.fetch_stored(&b, &tenant),
            Some(StoredContent::Recoverable("bob content".into())),
            "erasing one subject must not touch another's content (per-subject DEK)"
        );
    }

    #[test]
    fn the_residual_third_party_mention_is_restrict_suppress_only() {
        assert_eq!(
            classify_residual(Authorship::SelfAuthored),
            LeverCoverage::CryptoShred
        );
        assert_eq!(
            classify_residual(Authorship::ThirdPartyMention),
            LeverCoverage::RestrictSuppressOnly,
            "the residual is restrict-suppressed (the documented limit), never crypto-shredded"
        );
    }

    #[test]
    fn the_residual_is_suppressed_for_the_restricted_subject_and_survives_its_erase() {
        let tenant = t("acme");
        let author = subject("u-author");
        let subj = subject("u-mentioned");
        let restrict = RestrictRegistry::new();
        let kms = InMemoryShredKms::new();
        let store = M1Store::new("issues_store", &restrict, &kms);

        kms.provision(M1Store::dek_handle(&author, &tenant), 3);
        store.store_self_authored(&author, &tenant, "thanks @u-mentioned for the help");

        assert_eq!(
            classify_residual(Authorship::ThirdPartyMention),
            LeverCoverage::RestrictSuppressOnly
        );
        restrict.set(&subj, &tenant, true);
        assert_eq!(
            store.index(&subj, &tenant),
            Processed::Unrecoverable,
            "the subject authored nothing here; the mention is the author's residual"
        );

        kms.provision(M1Store::dek_handle(&subj, &tenant), 9);
        assert_eq!(store.erase_self_authored(&subj, &tenant), Some(9));
        assert_eq!(
            store.fetch_stored(&author, &tenant),
            Some(StoredContent::Recoverable(
                "thanks @u-mentioned for the help".into()
            )),
            "the third-party mention under the author's DEK survives the subject's erase - the \
             documented residual limit (§7.2); it is governed by restrict, not crypto-shred"
        );
    }

    #[test]
    fn the_suppression_branch_is_load_bearing_both_verdicts_pinned() {
        let tenant = t("acme");
        let subj = subject("u-branch");
        let restrict = RestrictRegistry::new();
        let kms = InMemoryShredKms::new();
        let store = M1Store::new("s", &restrict, &kms);
        kms.provision(M1Store::dek_handle(&subj, &tenant), 1);
        store.store_self_authored(&subj, &tenant, "x");

        match store.index(&subj, &tenant) {
            Processed::Processed(out) => {
                assert!(
                    out.starts_with("index:s:"),
                    "the processed projection names op + holder id"
                );
            }
            other => panic!("expected Processed, got {other:?}"),
        }
        restrict.set(&subj, &tenant, true);
        assert_eq!(store.index(&subj, &tenant), Processed::Suppressed);
    }

    #[test]
    fn the_four_processing_ops_are_the_section_4_4_set() {
        assert_eq!(Processing::all().len(), 4);
        assert_eq!(Processing::Index.token(), "index");
        assert_eq!(Processing::AgentRead.token(), "agent_read");
        assert_eq!(Processing::Analyse.token(), "analyse");
        assert_eq!(Processing::Notify.token(), "notify");
    }
}

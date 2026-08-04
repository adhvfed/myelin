use std::sync::{Arc, Mutex};

use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId as GdprTenantId,
};
use myelin_substrate::{
    Holder, HolderRegistration, HolderRegistry, StoreClassifier, StoreHolder, StoreKind,
};
use myelin_tenancy::TenantId;

use crate::router::InboxProjection;

pub const NOTIF_OLTP_STORE: &str = "notif_oltp";

pub type NotifHolderRegistration = HolderRegistration;

pub fn notif_store_classifier() -> StoreClassifier {
    StoreClassifier::of([StoreHolder::new(
        StoreKind::Oltp,
        NOTIF_OLTP_STORE,
        Holder::H13NotificationHistory,
    )])
}

pub fn register_notif_holder() -> HolderRegistry {
    let mut registry = HolderRegistry::new();
    registry.open(StoreKind::Oltp, NOTIF_OLTP_STORE);
    registry
}

#[derive(Clone, Default)]
pub struct RestrictSet {
    inner: Arc<Mutex<std::collections::HashSet<String>>>,
}

impl RestrictSet {
    pub fn new() -> RestrictSet {
        RestrictSet::default()
    }

    pub fn set(&self, subject_id: &str, on: bool) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if on {
            g.insert(subject_id.to_string());
        } else {
            g.remove(subject_id);
        }
    }

    pub fn is_restricted(&self, subject_id: &str) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(subject_id)
    }
}

#[derive(Clone)]
pub struct NotifBacking {
    inbox: InboxProjection,
    restrict: RestrictSet,
}

impl NotifBacking {
    pub fn new(inbox: InboxProjection) -> NotifBacking {
        NotifBacking {
            inbox,
            restrict: RestrictSet::new(),
        }
    }

    pub fn with_restrict(inbox: InboxProjection, restrict: RestrictSet) -> NotifBacking {
        NotifBacking { inbox, restrict }
    }

    pub fn restrict_set(&self) -> &RestrictSet {
        &self.restrict
    }
}

#[derive(Clone, Default)]
pub struct NotifHistoryHolder {
    backing: Option<NotifBacking>,
}

impl NotifHistoryHolder {
    pub fn with_inbox(inbox: InboxProjection) -> NotifHistoryHolder {
        NotifHistoryHolder {
            backing: Some(NotifBacking::new(inbox)),
        }
    }

    pub fn with_backing(backing: NotifBacking) -> NotifHistoryHolder {
        NotifHistoryHolder {
            backing: Some(backing),
        }
    }

    pub fn register(&self, registry: &mut HolderRegistry) -> NotifHolderRegistration {
        registry.open(StoreKind::Oltp, NOTIF_OLTP_STORE)
    }

    pub fn restrict_set(&self) -> Option<&RestrictSet> {
        self.backing.as_ref().map(|b| b.restrict_set())
    }

    fn subject_id(subject: &SubjectRef) -> String {
        subject.principal.principal_id.0.clone()
    }

    fn count_appearances(&self, tenant: &GdprTenantId, subject_id: &str) -> usize {
        let Some(b) = &self.backing else {
            return 0;
        };
        let t = TenantId(tenant.0.clone());
        b.inbox
            .snapshot_for_tenant(&t)
            .iter()
            .filter(|row| row.references_subject(subject_id))
            .count()
    }
}

impl PersonalDataHolder for NotifHistoryHolder {
    fn locate(&self, subject: &SubjectRef, tenant: GdprTenantId) -> DsrResult<LocateReport> {
        let sid = Self::subject_id(subject);
        let count = self.count_appearances(&tenant, &sid);
        let outcome = format!(
            "located {count} inbox items naming the subject (recipient pseudonym + referenced-actor \
             refs, references-not-payloads - no stored name)"
        );
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                NOTIF_OLTP_STORE,
                &sid,
                &tenant.0,
                &outcome,
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: GdprTenantId) -> DsrResult<PortableBundle> {
        let sid = Self::subject_id(subject);
        let count = self.count_appearances(&tenant, &sid);
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                NOTIF_OLTP_STORE,
                &sid,
                &tenant.0,
                &format!(
                    "references-not-payloads bundle: {count} inbox appearances, no free-text body"
                ),
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                NOTIF_OLTP_STORE,
                &Self::subject_id(subject),
                "",
                "no-op (references-not-payloads - rectify via reindex-from-source + read-time re-resolve, NOTIF-P17)",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        let sid = Self::subject_id(subject);
        let applied = match &self.backing {
            Some(b) => {
                b.restrict.set(&sid, on);
                true
            }
            None => false,
        };
        let outcome = if applied {
            format!("restrict={on} recorded in the suppression set (new routing/delivery suppressed; indexing/agent-use too)")
        } else {
            format!("restrict={on} no-op (no live routing; suppression lands with routing/delivery NOTIF-P10/P16)")
        };
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                NOTIF_OLTP_STORE,
                &sid,
                "",
                &outcome,
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (sid, tenant) = match &scope {
            EraseScope::Subject { subject, tenant } => {
                (Self::subject_id(subject), tenant.0.clone())
            }
            EraseScope::Tenant(t) => (String::new(), t.0.clone()),
        };
        let count = match &scope {
            EraseScope::Subject { tenant, .. } => self.count_appearances(tenant, &sid),
            EraseScope::Tenant(_) => 0,
        };
        let outcome = match &scope {
            EraseScope::Subject { .. } => format!(
                "structural erase: {count} inbox appearances tombstone for free (refs-not-payloads; \
                 Identity 4.8 pseudonym-shred makes the opaque id unresolvable) - 0 PII columns mutated; \
                 off-cell residual + inline-PII DEK shred = X-7/10.9 (NOTIF-P27); replay NOTIF-P17"
            ),
            EraseScope::Tenant(_) => {
                "tenant crypto-shred: destroy the per-tenant DEK (11.3/11.4) - every inbox row unrecoverable".into()
            }
        };
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                NOTIF_OLTP_STORE,
                &sid,
                &tenant,
                &outcome,
                None,
                0,
            ),
        })
    }
}

pub fn notif_history_holder() -> Option<Holder> {
    myelin_substrate::classify_store(StoreKind::Oltp, NOTIF_OLTP_STORE, &notif_store_classifier())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::{InboxProjection, RoutedInboxItem};
    use crate::{Class, Reason};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_refs::ArtifactRef;
    use myelin_substrate::{assert_holder_completeness, classify_store};
    use myelin_tenancy::Region;

    fn subject(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            GdprTenantId::from_token("acme"),
        ))
    }

    fn tenant() -> GdprTenantId {
        GdprTenantId::from_token("acme")
    }

    fn t() -> TenantId {
        TenantId::from_token("acme")
    }

    fn row(recipient: &str, subject: &str, actor: &str, dedup_key: &str) -> RoutedInboxItem {
        RoutedInboxItem {
            tenant: t(),
            region: Region::new("fr-par"),
            item_id: format!("itm-{dedup_key}"),
            recipient: recipient.into(),
            subject: ArtifactRef(format!("myelin://acme/issues/issue/{subject}")),
            reason: Reason::Mentioned,
            class: Class::Direct,
            origin_event: ArtifactRef(format!("myelin://acme/identity/principal/{actor}")),
            dedup_key: dedup_key.into(),
            coalesce_count: 1,
            state: "unread".into(),
            snooze_until: None,
        }
    }

    #[test]
    fn notif_registers_its_store_as_a_holder() {
        let registry = register_notif_holder();
        assert!(registry.is_registered(StoreKind::Oltp, NOTIF_OLTP_STORE));
        assert_eq!(registry.len(), 1, "exactly the one Notif store registered");
    }

    #[test]
    fn re_registration_is_idempotent() {
        let mut registry = register_notif_holder();
        NotifHistoryHolder::default().register(&mut registry);
        assert_eq!(
            registry.len(),
            1,
            "re-opening the same Notif store does not double-register"
        );
    }

    #[test]
    fn notif_store_classifies_to_h13_no_orphan() {
        let registry = register_notif_holder();
        let classifier = notif_store_classifier();
        assert_eq!(
            classify_store(StoreKind::Oltp, NOTIF_OLTP_STORE, &classifier),
            Some(Holder::H13NotificationHistory),
            "the Notif OLTP store is holder H13"
        );
        assert_eq!(notif_history_holder(), Some(Holder::H13NotificationHistory));
        assert_eq!(
            assert_holder_completeness(registry.registrations(), &classifier),
            Ok(()),
            "the Notif store is in the exhaustive H1–H18 list - 0 orphan stores"
        );
    }

    #[test]
    fn structural_erase_tombstones_a_refs_stored_item_with_zero_pii_mutation() {
        let inbox = InboxProjection::new();
        inbox.upsert_for_test(row("u-erase", "PROJ-1", "u-other", "own"));
        inbox.upsert_for_test(row("u-bob", "PROJ-2", "u-erase", "byref"));
        inbox.upsert_for_test(row("u-carol", "PROJ-3", "u-dave", "control"));

        let holder = NotifHistoryHolder::with_inbox(inbox.clone());

        let before: Vec<RoutedInboxItem> = inbox.snapshot_for_tenant(&t());
        let subj_rows_before: Vec<&RoutedInboxItem> = before
            .iter()
            .filter(|r| r.references_subject("u-erase"))
            .collect();
        assert_eq!(
            subj_rows_before.len(),
            2,
            "locate finds both appearances (own + by-ref)"
        );

        let loc = holder
            .locate(&subject("u-erase"), tenant())
            .expect("locate succeeds");
        assert!(loc.receipt.content_hash.starts_with("blake3:"));
        assert!(
            loc.receipt.key_epoch_destroyed.is_none(),
            "locate shreds no key"
        );

        let scope = EraseScope::Subject {
            subject: subject("u-erase"),
            tenant: tenant(),
        };
        let er = holder
            .erase(scope.clone())
            .expect("structural erase succeeds");
        assert!(
            er.receipt.key_epoch_destroyed.is_none(),
            "0 keys shredded at the inbox surface (refs-only)"
        );

        let after: Vec<RoutedInboxItem> = inbox.snapshot_for_tenant(&t());
        let mut before_sorted = before.clone();
        let mut after_sorted = after.clone();
        before_sorted.sort_by(|a, b| a.item_id.cmp(&b.item_id));
        after_sorted.sort_by(|a, b| a.item_id.cmp(&b.item_id));
        assert_eq!(
            after_sorted, before_sorted,
            "the refs-stored items tombstone for FREE - 0 PII columns mutated (references-not-payloads)"
        );
        assert_eq!(
            after.len(),
            3,
            "no row deleted either - the appearance stays, only resolution changes"
        );

        let er2 = holder.erase(scope).expect("re-erase is idempotent");
        assert_eq!(er, er2, "the same erase scope yields the identical receipt");
    }

    #[test]
    fn locate_counts_real_appearances_backed_vs_unbacked() {
        let unbacked = NotifHistoryHolder::default();
        assert_eq!(
            unbacked.count_appearances(&tenant(), "u-x"),
            0,
            "unbacked → empty-but-correct"
        );

        let inbox = InboxProjection::new();
        inbox.upsert_for_test(row("u-x", "PROJ-1", "u-y", "a"));
        inbox.upsert_for_test(row("u-z", "PROJ-2", "u-x", "b"));
        inbox.upsert_for_test(row("u-z", "PROJ-3", "u-q", "c"));
        let backed = NotifHistoryHolder::with_inbox(inbox);
        assert_eq!(
            backed.count_appearances(&tenant(), "u-x"),
            2,
            "both structural appearances counted"
        );
        assert_eq!(
            backed.count_appearances(&tenant(), "u-none"),
            0,
            "an absent subject → 0"
        );
    }

    #[test]
    fn restrict_writes_the_shared_suppression_set() {
        let restrict = RestrictSet::new();
        let backing = NotifBacking::with_restrict(InboxProjection::new(), restrict.clone());
        let holder = NotifHistoryHolder::with_backing(backing);
        let subj = subject("u-r");

        assert!(!restrict.is_restricted("u-r"), "not restricted initially");
        holder.restrict(&subj, true).expect("restrict on succeeds");
        assert!(
            restrict.is_restricted("u-r"),
            "the holder recorded the restriction in the shared set"
        );
        holder
            .restrict(&subj, false)
            .expect("restrict off succeeds");
        assert!(!restrict.is_restricted("u-r"), "restrict off clears it");

        let unbacked = NotifHistoryHolder::default();
        assert!(
            unbacked.restrict(&subj, true).is_ok(),
            "unbacked restrict is a no-op receipt"
        );
    }

    #[test]
    fn unbacked_holder_is_empty_but_correct() {
        let holder = NotifHistoryHolder::default();
        let subj = subject("u-1");
        let loc = holder
            .locate(&subj, tenant())
            .expect("locate over empty surface succeeds");
        assert_eq!(loc.receipt.operation, "locate");
        let exp = holder
            .export(&subj, tenant())
            .expect("export of empty bundle succeeds");
        assert_eq!(exp.receipt.operation, "export");
        let rec = holder
            .rectify(&subj, Patch("x".into()))
            .expect("rectify no-op succeeds");
        assert_eq!(rec.receipt.operation, "rectify");
    }

    #[test]
    fn restrict_set_accessors_return_the_shared_set() {
        let restrict = RestrictSet::new();
        let backing = NotifBacking::with_restrict(InboxProjection::new(), restrict.clone());
        backing.restrict_set().set("u-shared", true);
        assert!(
            restrict.is_restricted("u-shared"),
            "the backing accessor is the shared set, not a fresh one"
        );

        let holder = NotifHistoryHolder::with_backing(backing);
        let via_holder = holder
            .restrict_set()
            .expect("backed holder exposes its restrict set");
        assert!(
            via_holder.is_restricted("u-shared"),
            "the holder accessor is the SAME shared set"
        );
        via_holder.set("u-shared", false);
        assert!(
            !restrict.is_restricted("u-shared"),
            "a write through the holder accessor reaches the shared set"
        );

        assert!(
            NotifHistoryHolder::default().restrict_set().is_none(),
            "unbacked → no restrict set"
        );
    }

    #[test]
    fn holder_is_object_safe() {
        let holders: Vec<Box<dyn PersonalDataHolder>> =
            vec![Box::new(NotifHistoryHolder::default())];
        let subj = subject("u-3");
        for h in &holders {
            assert!(
                h.locate(&subj, tenant()).is_ok(),
                "the holder responds to the contract"
            );
        }
    }
}

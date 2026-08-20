use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};
use myelin_substrate::{Holder, HolderRegistration, StoreKind};
use myelin_tenancy::Region;

use crate::datamap::HolderSchema;
use crate::derivative_erasure::{NotifHistoryModel, RefsGraphModel, RefsResolve, SearchIndexModel};
use crate::holders::{CryptoShredKms, ShredKeyClass, ShredKeyHandle};
use crate::orchestration::{CanonicalErasePhase, RegisteredHolder};
use crate::posture::{
    reference_is_by_reference, SubsystemReference, CANONICAL_POSTURE, POSTURE_ANCHOR,
};

pub const ISSUES_DB: &str = "issue_oltp";

pub const CHAT_DB: &str = "chat_oltp";

pub const ISSUES_SUBSYSTEM: &str = "issues";

pub const CHAT_SUBSYSTEM: &str = "chat";

pub fn issues_chat_phase_of(holder_id: &str) -> Option<CanonicalErasePhase> {
    match holder_id {
        ISSUES_DB => Some(CanonicalErasePhase::CryptoShredDek),
        CHAT_DB => Some(CanonicalErasePhase::CryptoShredDek),
        _ => None,
    }
}

pub fn issues_chat_holder_schemas(region: Region) -> Vec<HolderSchema> {
    vec![
        HolderSchema {
            registration: HolderRegistration {
                kind: StoreKind::Oltp,
                name: ISSUES_DB,
            },
            holder: Holder::H3Issues,
            region: region.clone(),
            fields: &[],
        },
        HolderSchema {
            registration: HolderRegistration {
                kind: StoreKind::Oltp,
                name: CHAT_DB,
            },
            holder: Holder::H5Chat,
            region,
            fields: &[],
        },
    ]
}

pub fn issues_chat_registrations() -> Vec<HolderRegistration> {
    vec![
        HolderRegistration {
            kind: StoreKind::Oltp,
            name: ISSUES_DB,
        },
        HolderRegistration {
            kind: StoreKind::Oltp,
            name: CHAT_DB,
        },
    ]
}

pub const ISSUES_INSTANCE: SubsystemReference = SubsystemReference {
    subsystem: ISSUES_SUBSYSTEM,
    cited_anchor: POSTURE_ANCHOR,
    section_text:
        "Issues free-text / immutable-content erasure follows the platform posture in \
         00-reconciliation-decisions.md §X-7 / gdpr-and-audit.md §7 (contract 10.9). An erase \
         crypto-shreds the subject's per-subject Issues free-text key, and fans out to the \
         change-log, comments, attachments, OLAP (which also honours restriction), Search and Refs; \
         the issue topology structure survives. The worklog field rides the same per-subject key.",
};

pub const CHAT_INSTANCE: SubsystemReference = SubsystemReference {
    subsystem: CHAT_SUBSYSTEM,
    cited_anchor: POSTURE_ANCHOR,
    section_text:
        "Chat free-text / immutable-content erasure follows the platform posture in \
         00-reconciliation-decisions.md §X-7 / gdpr-and-audit.md §7 (contract 10.9). An erase \
         crypto-shreds the subject's per-subject Chat message-body key across hot and cold segments \
         and backups; mentions of the subject render the erased-user sentinel; read-state, drafts \
         and the unfurl cache are purged; the cascade fans to Search, Refs and Notif.",
};

#[must_use]
pub fn issues_section_references_posture() -> bool {
    reference_is_by_reference(&ISSUES_INSTANCE)
}

#[must_use]
pub fn chat_section_references_posture() -> bool {
    reference_is_by_reference(&CHAT_INSTANCE)
}

#[must_use]
pub const fn issues_residual() -> &'static str {
    CANONICAL_POSTURE.residual
}

#[must_use]
pub const fn chat_residual() -> &'static str {
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

#[derive(Debug, Default)]
pub struct IssuesStoreModel {
    topology: Mutex<BTreeMap<String, bool>>,
    olap_suppressed: Mutex<BTreeMap<String, bool>>,
    erase_calls: Mutex<u32>,
}

impl IssuesStoreModel {
    pub fn new() -> IssuesStoreModel {
        IssuesStoreModel::default()
    }

    pub fn index_topology_from_source(&self, subject_token: &str) {
        self.topology
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subject_token.to_string(), true);
    }

    pub fn topology_present(&self, subject_token: &str) -> bool {
        self.topology
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(subject_token)
            .copied()
            .unwrap_or(false)
    }

    pub fn olap_suppressed(&self, subject_token: &str) -> bool {
        self.olap_suppressed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(subject_token)
            .copied()
            .unwrap_or(false)
    }

    pub fn erase_call_count(&self) -> u32 {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn suppress_olap(&self, subject_token: &str, on: bool) {
        self.olap_suppressed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subject_token.to_string(), on);
    }

    fn note_erase(&self) {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
    }
}

pub struct IssuesStoreHolder<'a> {
    model: &'a IssuesStoreModel,
    kms: &'a dyn CryptoShredKms,
}

impl<'a> IssuesStoreHolder<'a> {
    pub fn new(model: &'a IssuesStoreModel, kms: &'a dyn CryptoShredKms) -> IssuesStoreHolder<'a> {
        IssuesStoreHolder { model, kms }
    }

    pub fn holder_id(&self) -> &'static str {
        ISSUES_DB
    }
}

impl PersonalDataHolder for IssuesStoreHolder<'_> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if self.kms.is_present(&subject_dek(&sid, &tenant)) {
            "located:issue-free-text-present"
        } else {
            "located:0-recoverable"
        };
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate", ISSUES_DB, &sid, &tenant.0, outcome, None, 0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export", ISSUES_DB, &sid, &tenant.0, "exported", None, 0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                ISSUES_DB,
                &sid,
                "*",
                "rectified",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        self.model.suppress_olap(&sid, on);
        let outcome = if on {
            "restricted:set:olap-suppressed"
        } else {
            "restricted:clear"
        };
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed("restrict", ISSUES_DB, &sid, "*", outcome, None, 0),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (sid, tenant_token) = subject_and_tenant(&scope);
        let tenant = TenantId::from_token(&tenant_token);
        self.model.note_erase();
        let (destroyed, outcome) = match &scope {
            EraseScope::Subject { .. } => {
                self.model.suppress_olap(&sid, true);
                (
                    self.kms.destroy(&subject_dek(&sid, &tenant)),
                    "crypto_shred:per_subject_issues_free_text_dek;olap_suppressed;structure_survives",
                )
            }
            EraseScope::Tenant(_) => (
                self.kms.destroy(&tenant_dek(&tenant)),
                "crypto_shred:per_tenant_issues_dek_fallback:tenant_offboard;structure_survives",
            ),
        };
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                ISSUES_DB,
                &sid,
                &tenant_token,
                outcome,
                destroyed,
                0,
            ),
        })
    }
}

#[derive(Debug, Default)]
pub struct ChatStoreModel {
    topology: Mutex<BTreeMap<String, bool>>,
    read_state_present: Mutex<BTreeMap<String, bool>>,
    erase_calls: Mutex<u32>,
}

impl ChatStoreModel {
    pub fn new() -> ChatStoreModel {
        ChatStoreModel::default()
    }

    pub fn index_from_source(&self, subject_token: &str) {
        self.topology
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subject_token.to_string(), true);
        self.read_state_present
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subject_token.to_string(), true);
    }

    pub fn topology_present(&self, subject_token: &str) -> bool {
        self.topology
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(subject_token)
            .copied()
            .unwrap_or(false)
    }

    pub fn read_state_present(&self, subject_token: &str) -> bool {
        self.read_state_present
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(subject_token)
            .copied()
            .unwrap_or(false)
    }

    pub fn erase_call_count(&self) -> u32 {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn purge_read_state(&self, subject_token: &str) {
        self.read_state_present
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subject_token.to_string(), false);
    }

    fn note_erase(&self) {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
    }
}

pub struct ChatStoreHolder<'a> {
    model: &'a ChatStoreModel,
    kms: &'a dyn CryptoShredKms,
}

impl<'a> ChatStoreHolder<'a> {
    pub fn new(model: &'a ChatStoreModel, kms: &'a dyn CryptoShredKms) -> ChatStoreHolder<'a> {
        ChatStoreHolder { model, kms }
    }

    pub fn holder_id(&self) -> &'static str {
        CHAT_DB
    }
}

impl PersonalDataHolder for ChatStoreHolder<'_> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if self.kms.is_present(&subject_dek(&sid, &tenant)) {
            "located:chat-bodies-present"
        } else {
            "located:0-recoverable"
        };
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate", CHAT_DB, &sid, &tenant.0, outcome, None, 0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export", CHAT_DB, &sid, &tenant.0, "exported", None, 0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                CHAT_DB,
                &sid,
                "*",
                "rectified",
                None,
                0,
            ),
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
            receipt: Receipt::content_addressed("restrict", CHAT_DB, &sid, "*", outcome, None, 0),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (sid, tenant_token) = subject_and_tenant(&scope);
        let tenant = TenantId::from_token(&tenant_token);
        self.model.note_erase();
        let (destroyed, outcome) = match &scope {
            EraseScope::Subject { .. } => {
                self.model.purge_read_state(&sid);
                (
                    self.kms.destroy(&subject_dek(&sid, &tenant)),
                    "crypto_shred:per_subject_chat_body_dek:hot_and_cold;read_state_purged;structure_survives",
                )
            }
            EraseScope::Tenant(_) => (
                self.kms.destroy(&tenant_dek(&tenant)),
                "crypto_shred:per_tenant_chat_dek_fallback:tenant_offboard;structure_survives",
            ),
        };
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                CHAT_DB,
                &sid,
                &tenant_token,
                outcome,
                destroyed,
                0,
            ),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuesCascadeReceipt {
    pub subject_token: String,
    pub primary_shredded: bool,
    pub olap_suppressed: bool,
    pub embeddings_purged: bool,
    pub refs_tombstoned: bool,
    pub structure_survives: bool,
    pub holder_receipts: Vec<EraseReceipt>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatCascadeReceipt {
    pub subject_token: String,
    pub bodies_shredded: bool,
    pub read_state_purged: bool,
    pub notif_humanised: bool,
    pub embeddings_purged: bool,
    pub refs_tombstoned: bool,
    pub structure_survives: bool,
    pub holder_receipts: Vec<EraseReceipt>,
}

pub struct IssuesChatCascadeDriver;

impl IssuesChatCascadeDriver {
    pub fn register_issues_chat<'a>(
        holders: Vec<(&'static str, &'a dyn PersonalDataHolder)>,
    ) -> Vec<RegisteredHolder<'a>> {
        holders
            .into_iter()
            .map(|(id, holder)| {
                let phase = issues_chat_phase_of(id).unwrap_or_else(|| {
                    panic!("Issues/Chat holder `{id}` has no canonical erase phase")
                });
                RegisteredHolder { id, phase, holder }
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn fan_out_issue_erase(
        scope: &EraseScope,
        issues: &IssuesStoreModel,
        issues_holder: &dyn PersonalDataHolder,
        search: &SearchIndexModel,
        search_holder: &dyn PersonalDataHolder,
        refs: &RefsGraphModel,
        refs_holder: &dyn PersonalDataHolder,
        kms: &dyn CryptoShredKms,
    ) -> DsrResult<IssuesCascadeReceipt> {
        let (sid, tenant_token) = subject_and_tenant(scope);
        let tenant = TenantId::from_token(&tenant_token);
        let primary_receipt = issues_holder.erase(scope.clone())?;
        let search_receipt = search_holder.erase(scope.clone())?;
        let refs_receipt = refs_holder.erase(scope.clone())?;

        let primary_shredded = !kms.is_present(&subject_dek(&sid, &tenant));
        let olap_suppressed = issues.olap_suppressed(&sid);
        let embeddings_purged = search.reidentify_hits(&sid) == 0;
        let refs_tombstoned = matches!(refs.resolve(&sid), RefsResolve::Tombstone);
        let structure_survives = issues.topology_present(&sid);
        Ok(IssuesCascadeReceipt {
            subject_token: sid,
            primary_shredded,
            olap_suppressed,
            embeddings_purged,
            refs_tombstoned,
            structure_survives,
            holder_receipts: vec![primary_receipt, search_receipt, refs_receipt],
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn fan_out_chat_erase(
        scope: &EraseScope,
        chat: &ChatStoreModel,
        chat_holder: &dyn PersonalDataHolder,
        search: &SearchIndexModel,
        search_holder: &dyn PersonalDataHolder,
        refs: &RefsGraphModel,
        refs_holder: &dyn PersonalDataHolder,
        notif: &NotifHistoryModel,
        notif_holder: &dyn PersonalDataHolder,
        kms: &dyn CryptoShredKms,
    ) -> DsrResult<ChatCascadeReceipt> {
        let (sid, tenant_token) = subject_and_tenant(scope);
        let tenant = TenantId::from_token(&tenant_token);
        let primary_receipt = chat_holder.erase(scope.clone())?;
        let search_receipt = search_holder.erase(scope.clone())?;
        let refs_receipt = refs_holder.erase(scope.clone())?;
        let notif_receipt = notif_holder.erase(scope.clone())?;

        let bodies_shredded = !kms.is_present(&subject_dek(&sid, &tenant));
        let read_state_purged = !chat.read_state_present(&sid);
        let embeddings_purged = search.reidentify_hits(&sid) == 0;
        let refs_tombstoned = matches!(refs.resolve(&sid), RefsResolve::Tombstone);
        let notif_humanised = notif.erase_call_count() > 0;
        let structure_survives = chat.topology_present(&sid);
        Ok(ChatCascadeReceipt {
            subject_token: sid,
            bodies_shredded,
            read_state_purged,
            notif_humanised,
            embeddings_purged,
            refs_tombstoned,
            structure_survives,
            holder_receipts: vec![primary_receipt, search_receipt, refs_receipt, notif_receipt],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamap::data_map;
    use crate::derivative_erasure::{
        NotifHistoryHolder, RefsGraphHolder, SearchIndexHolder, ERASED_USER,
    };
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

    fn provision_subject_dek(kms: &InMemoryShredKms, tenant: &TenantId, sid: &str, epoch: u64) {
        kms.provision(
            ShredKeyHandle {
                tenant: tenant.clone(),
                class: ShredKeyClass::Subject(sid.to_string()),
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
    fn issues_chat_holders_appear_in_the_data_map_after_registration() {
        let inv = data_map(&issues_chat_holder_schemas(region()));
        assert!(
            inv.holders.contains("oltp:issue_oltp"),
            "H3 Issues is in the map"
        );
        assert!(
            inv.holders.contains("oltp:chat_oltp"),
            "H5 Chat is in the map"
        );
        assert_eq!(inv.holder_count(), 2, "exactly the two consumer holders");
        assert!(
            inv.coverage_gaps(&issues_chat_registrations()).is_empty(),
            "the registered Issues/Chat holders are in the map - 0 holders missed"
        );
    }

    #[test]
    fn registered_issues_chat_holders_absent_from_the_map_are_coverage_gaps() {
        let inv = data_map(&[]);
        let gaps = inv.coverage_gaps(&issues_chat_registrations());
        assert_eq!(
            gaps,
            vec!["oltp:chat_oltp".to_string(), "oltp:issue_oltp".to_string()],
            "the registered-but-unmapped Issues/Chat holders are the coverage gaps"
        );
    }

    #[test]
    fn issues_chat_holders_declare_their_canonical_erase_phase() {
        assert_eq!(
            issues_chat_phase_of(ISSUES_DB),
            Some(CanonicalErasePhase::CryptoShredDek)
        );
        assert_eq!(
            issues_chat_phase_of(CHAT_DB),
            Some(CanonicalErasePhase::CryptoShredDek)
        );
        assert_eq!(issues_chat_phase_of("not_a_consumer_store"), None);
    }

    #[test]
    fn consumer_holder_ids_are_the_frozen_addresses() {
        let kms = InMemoryShredKms::new();
        let issues_model = IssuesStoreModel::new();
        let chat_model = ChatStoreModel::new();
        assert_eq!(
            IssuesStoreHolder::new(&issues_model, &kms).holder_id(),
            "issue_oltp"
        );
        assert_eq!(
            ChatStoreHolder::new(&chat_model, &kms).holder_id(),
            "chat_oltp"
        );
        let schemas = issues_chat_holder_schemas(region());
        assert_eq!(schemas[0].holder_id(), "oltp:issue_oltp");
        assert_eq!(schemas[1].holder_id(), "oltp:chat_oltp");
    }

    #[test]
    fn the_fan_out_reaches_the_consumer_holders_in_canonical_order() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-c", 10);
        let issues_model = IssuesStoreModel::new();
        let chat_model = ChatStoreModel::new();
        issues_model.index_topology_from_source("u-c");
        chat_model.index_from_source("u-c");
        let ih = IssuesStoreHolder::new(&issues_model, &kms);
        let ch = ChatStoreHolder::new(&chat_model, &kms);

        let regd = IssuesChatCascadeDriver::register_issues_chat(vec![
            (ISSUES_DB, &ih as &dyn PersonalDataHolder),
            (CHAT_DB, &ch as &dyn PersonalDataHolder),
        ]);
        let orch = UpstreamHolderOrchestrator::new(regd);

        let ids = orch.holder_ids_in_order();
        assert!(ids.contains(&ISSUES_DB), "H3 Issues is in the fan-out");
        assert!(ids.contains(&CHAT_DB), "H5 Chat is in the fan-out");

        let checklist = EraseChecklist::new();
        let receipts = orch
            .fan_out_erase(&subject_scope("u-c"), &checklist)
            .unwrap();
        assert_eq!(receipts.len(), 2, "both consumer holders were reached");
        assert_eq!(
            orch.fanout_coverage(&checklist),
            1.0,
            "100% coverage of the consumer holders"
        );
    }

    #[test]
    #[should_panic(expected = "has no canonical erase phase")]
    fn registering_an_undeclared_holder_panics() {
        let kms = InMemoryShredKms::new();
        let model = IssuesStoreModel::new();
        let holder = IssuesStoreHolder::new(&model, &kms);
        let _ = IssuesChatCascadeDriver::register_issues_chat(vec![(
            "bogus_store",
            &holder as &dyn PersonalDataHolder,
        )]);
    }

    #[test]
    fn iss_d11_per_subject_dek_shred_plus_cascade_structure_survives() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-iss", 20);
        provision_subject_dek(&kms, &tenant, "u-keep", 21);
        let issues = IssuesStoreModel::new();
        issues.index_topology_from_source("u-iss");
        let search = SearchIndexModel::new();
        let refs = RefsGraphModel::new();
        search.index_from_source("u-iss", "alice@example.com");
        refs.add_edge_from_source("u-iss", "issue:42");

        let ih = IssuesStoreHolder::new(&issues, &kms);
        let sh = SearchIndexHolder::new(&search);
        let rh = RefsGraphHolder::new(&refs);

        let erase_dek = subject_dek("u-iss", &tenant);
        let keep_dek = subject_dek("u-keep", &tenant);
        assert!(kms.is_present(&erase_dek), "the DEK is live before erase");

        let receipt = IssuesChatCascadeDriver::fan_out_issue_erase(
            &subject_scope("u-iss"),
            &issues,
            &ih,
            &search,
            &sh,
            &refs,
            &rh,
            &kms,
        )
        .unwrap();

        assert!(
            receipt.primary_shredded,
            "the primary free-text DEK is shredded"
        );
        assert!(!kms.is_present(&erase_dek), "the DEK is destroyed (live)");
        assert_eq!(
            kms.recoverable_in_backup(&erase_dek),
            0,
            "0 recoverable in backups (crypto-shred reaches backups - ISS-D11)"
        );
        assert!(receipt.olap_suppressed, "OLAP honours restriction (11.6)");
        assert!(
            receipt.embeddings_purged,
            "Search embeddings purged (not hidden)"
        );
        assert!(
            receipt.refs_tombstoned,
            "Refs tombstoned (0 recoverable, no 500)"
        );
        assert!(
            receipt.structure_survives,
            "the issue topology structure survives"
        );
        assert_eq!(receipt.holder_receipts.len(), 3, "primary + Search + Refs");
        assert!(
            kms.is_present(&keep_dek),
            "a different subject's data survives (the per-subject reach)"
        );
        assert_eq!(search.reidentify_hits("u-iss"), 0);
        assert_eq!(refs.recoverable_edges("u-iss"), 0);
        let expected = Receipt::content_addressed(
            "erase",
            ISSUES_DB,
            "u-iss",
            &tenant.0,
            "crypto_shred:per_subject_issues_free_text_dek;olap_suppressed;structure_survives",
            receipt.holder_receipts[0].receipt.key_epoch_destroyed,
            0,
        );
        assert_eq!(
            receipt.holder_receipts[0].receipt.content_hash, expected.content_hash,
            "the primary receipt names the per-subject Issues free-text DEK reach"
        );
    }

    #[test]
    fn the_issues_per_tenant_fallback_fires_on_a_tenant_offboarding() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-iso", 30);
        provision_tenant_dek(&kms, &tenant, 31);
        let model = IssuesStoreModel::new();
        let holder = IssuesStoreHolder::new(&model, &kms);

        let receipt = holder.erase(EraseScope::Tenant(tenant.clone())).unwrap();
        assert!(
            !kms.is_present(&tenant_dek(&tenant)),
            "a tenant offboarding destroys the per-tenant Issues DEK fallback"
        );
        let expected_tenant = Receipt::content_addressed(
            "erase",
            ISSUES_DB,
            "*tenant*",
            &tenant.0,
            "crypto_shred:per_tenant_issues_dek_fallback:tenant_offboard;structure_survives",
            receipt.receipt.key_epoch_destroyed,
            0,
        );
        assert_eq!(receipt.receipt.content_hash, expected_tenant.content_hash);
        let subj = holder.erase(subject_scope("u-iso")).unwrap();
        assert_ne!(
            receipt.receipt.content_hash, subj.receipt.content_hash,
            "the subject/tenant selection is load-bearing, not a constant string"
        );
        assert!(!kms.is_present(&subject_dek("u-iso", &tenant)));
    }

    #[test]
    fn chat_d8_per_subject_body_dek_reaches_hot_and_cold_plus_cascade() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-chat", 40);
        let chat = ChatStoreModel::new();
        chat.index_from_source("u-chat");
        let search = SearchIndexModel::new();
        let refs = RefsGraphModel::new();
        let notif = NotifHistoryModel::new();
        search.index_from_source("u-chat", "bob's message");
        refs.add_edge_from_source("u-chat", "msg:7");
        notif.add_item_from_source("inbox-x", "u-chat");

        let ch = ChatStoreHolder::new(&chat, &kms);
        let sh = SearchIndexHolder::new(&search);
        let rh = RefsGraphHolder::new(&refs);
        let nh = NotifHistoryHolder::new(&notif);

        let body_dek = subject_dek("u-chat", &tenant);
        assert!(
            chat.read_state_present("u-chat"),
            "read-state present before erase"
        );

        let receipt = IssuesChatCascadeDriver::fan_out_chat_erase(
            &subject_scope("u-chat"),
            &chat,
            &ch,
            &search,
            &sh,
            &refs,
            &rh,
            &notif,
            &nh,
            &kms,
        )
        .unwrap();

        assert!(receipt.bodies_shredded, "the message-body DEK is shredded");
        assert!(
            !kms.is_present(&body_dek),
            "the body DEK is destroyed (live)"
        );
        assert_eq!(
            kms.recoverable_in_backup(&body_dek),
            0,
            "0 recoverable in backups - hot AND cold AND backups (CHAT-D8)"
        );
        assert!(
            receipt.read_state_purged,
            "read-state / drafts / unfurl-cache purged"
        );
        assert!(!chat.read_state_present("u-chat"), "read-state is gone");
        assert!(receipt.notif_humanised, "Notif humanised mentions");
        assert!(receipt.embeddings_purged, "Search embeddings purged");
        assert!(receipt.refs_tombstoned, "Refs tombstoned");
        assert!(receipt.structure_survives, "the channel topology survives");
        assert_eq!(
            receipt.holder_receipts.len(),
            4,
            "primary + Search + Refs + Notif"
        );
        assert_eq!(
            notif.render_mention("inbox-x").as_deref(),
            Some(ERASED_USER)
        );
        let expected = Receipt::content_addressed(
            "erase",
            CHAT_DB,
            "u-chat",
            &tenant.0,
            "crypto_shred:per_subject_chat_body_dek:hot_and_cold;read_state_purged;structure_survives",
            receipt.holder_receipts[0].receipt.key_epoch_destroyed,
            0,
        );
        assert_eq!(
            receipt.holder_receipts[0].receipt.content_hash, expected.content_hash,
            "the primary receipt names the per-subject hot+cold body DEK reach"
        );
    }

    #[test]
    fn the_chat_per_tenant_fallback_fires_on_a_tenant_offboarding() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-iso", 50);
        provision_tenant_dek(&kms, &tenant, 51);
        let model = ChatStoreModel::new();
        let holder = ChatStoreHolder::new(&model, &kms);

        let receipt = holder.erase(EraseScope::Tenant(tenant.clone())).unwrap();
        assert!(!kms.is_present(&tenant_dek(&tenant)));
        let expected_tenant = Receipt::content_addressed(
            "erase",
            CHAT_DB,
            "*tenant*",
            &tenant.0,
            "crypto_shred:per_tenant_chat_dek_fallback:tenant_offboard;structure_survives",
            receipt.receipt.key_epoch_destroyed,
            0,
        );
        assert_eq!(receipt.receipt.content_hash, expected_tenant.content_hash);
        let subj = holder.erase(subject_scope("u-iso")).unwrap();
        assert_ne!(receipt.receipt.content_hash, subj.receipt.content_hash);
        assert!(!kms.is_present(&subject_dek("u-iso", &tenant)));
    }

    #[test]
    fn consumer_holders_erase_is_idempotent_structure_survives() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-idem", 60);
        let issues = IssuesStoreModel::new();
        issues.index_topology_from_source("u-idem");
        let ih = IssuesStoreHolder::new(&issues, &kms);

        let first = ih.erase(subject_scope("u-idem")).unwrap();
        let second = ih.erase(subject_scope("u-idem")).unwrap();
        assert_eq!(first.receipt.operation, second.receipt.operation);
        assert!(
            second.receipt.key_epoch_destroyed.is_none(),
            "the re-erase destroyed no key"
        );
        assert!(
            issues.topology_present("u-idem"),
            "the structure survives the re-erase"
        );
        assert_eq!(issues.erase_call_count(), 2, "both erase calls counted");
    }

    #[test]
    fn locate_reports_present_on_a_live_dek_and_zero_after_shred() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-loc", 70);
        let issues = IssuesStoreModel::new();
        let chat = ChatStoreModel::new();
        let ih = IssuesStoreHolder::new(&issues, &kms);
        let ch = ChatStoreHolder::new(&chat, &kms);

        let issues_present = ih.locate(&subject("u-loc"), tenant.clone()).unwrap();
        assert_eq!(
            issues_present.receipt.content_hash,
            Receipt::content_addressed(
                "locate",
                ISSUES_DB,
                "u-loc",
                &tenant.0,
                "located:issue-free-text-present",
                None,
                0
            )
            .content_hash
        );
        let chat_present = ch.locate(&subject("u-loc"), tenant.clone()).unwrap();
        assert_eq!(
            chat_present.receipt.content_hash,
            Receipt::content_addressed(
                "locate",
                CHAT_DB,
                "u-loc",
                &tenant.0,
                "located:chat-bodies-present",
                None,
                0
            )
            .content_hash
        );

        ih.erase(subject_scope("u-loc")).unwrap();
        let issues_after = ih.locate(&subject("u-loc"), tenant.clone()).unwrap();
        assert_eq!(
            issues_after.receipt.content_hash,
            Receipt::content_addressed(
                "locate",
                ISSUES_DB,
                "u-loc",
                &tenant.0,
                "located:0-recoverable",
                None,
                0
            )
            .content_hash
        );
        assert_ne!(
            issues_present.receipt.content_hash,
            issues_after.receipt.content_hash
        );
    }

    #[test]
    fn issues_restrict_is_honoured_into_olap() {
        let kms = InMemoryShredKms::new();
        let issues = IssuesStoreModel::new();
        let ih = IssuesStoreHolder::new(&issues, &kms);
        assert!(
            !issues.olap_suppressed("u-r"),
            "not suppressed before restrict"
        );
        ih.restrict(&subject("u-r"), true).unwrap();
        assert!(
            issues.olap_suppressed("u-r"),
            "a restricted subject is excluded from cross-individual OLAP analytics (11.6)"
        );
        ih.restrict(&subject("u-r"), false).unwrap();
        assert!(
            !issues.olap_suppressed("u-r"),
            "clearing restriction re-enables"
        );
    }

    #[test]
    fn the_instances_reference_the_posture_and_do_not_restate() {
        assert_eq!(ISSUES_INSTANCE.subsystem, "issues");
        assert_eq!(CHAT_INSTANCE.subsystem, "chat");
        assert_eq!(ISSUES_INSTANCE.cited_anchor, POSTURE_ANCHOR);
        assert_eq!(CHAT_INSTANCE.cited_anchor, POSTURE_ANCHOR);
        assert!(
            issues_section_references_posture(),
            "the Issues erasure section is a valid by-reference instantiation"
        );
        assert!(
            chat_section_references_posture(),
            "the Chat erasure section is a valid by-reference instantiation"
        );
        for instance in [&ISSUES_INSTANCE, &CHAT_INSTANCE] {
            let lowered = instance.section_text.to_ascii_lowercase();
            for marker in restatement_markers() {
                assert!(
                    !lowered.contains(&marker.to_ascii_lowercase()),
                    "the {} section must not restate the canonical marker {marker:?}",
                    instance.subsystem
                );
            }
        }
    }

    #[test]
    fn a_restating_consumer_section_would_be_rejected() {
        let restating = SubsystemReference {
            subsystem: "issues",
            cited_anchor: POSTURE_ANCHOR,
            section_text: "Issues erasure: per-subject DEK crypto-shred renders free-text \
                 unrecoverable; the documented lawful-basis limit covers third-party mentions ...",
        };
        assert!(
            !reference_is_by_reference(&restating),
            "a section that restates the posture (a canonical marker) is rejected - X-7"
        );
    }

    #[test]
    fn consumer_residuals_are_the_one_platform_posture_residual() {
        assert_eq!(issues_residual(), CANONICAL_POSTURE.residual);
        assert_eq!(chat_residual(), CANONICAL_POSTURE.residual);
        assert!(
            issues_residual().contains("AUTHOR's DEK") && issues_residual().contains("not the subject's"),
            "the residual is third-party PII under the AUTHOR's DEK - not shreddable by the subject's key"
        );
    }
}

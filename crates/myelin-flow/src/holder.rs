use std::sync::{Arc, Mutex};

use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId as GdprTenantId,
};
use myelin_storage::kms::KmsEngine;
use myelin_substrate::{
    Holder, HolderRegistration, HolderRegistry, StoreClassifier, StoreHolder, StoreKind,
};
use myelin_tenancy::{Region, TenantId};

use crate::crypto_shred::{history_row_has_inline_pii, WfCryptoShred};
use crate::engine::FlowTelemetry;
use crate::wfctx::WfJournal;

pub const FLOW_OLTP_STORE: &str = crate::SERVICE_NAME;

pub type FlowHolderRegistration = HolderRegistration;

pub fn flow_store_classifier() -> StoreClassifier {
    StoreClassifier::of([StoreHolder::new(
        StoreKind::Oltp,
        FLOW_OLTP_STORE,
        Holder::H8EventBus,
    )])
}

pub fn register_flow_holder() -> HolderRegistry {
    let mut registry = HolderRegistry::new();
    registry.open(StoreKind::Oltp, FLOW_OLTP_STORE);
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
pub struct FlowBacking {
    journal: WfJournal,
    restrict: RestrictSet,
    shred: Option<ShredWiring>,
}

#[derive(Clone)]
pub struct ShredWiring {
    kms: std::sync::Arc<KmsEngine>,
    region: Region,
    telemetry: FlowTelemetry,
}

impl FlowBacking {
    pub fn new(journal: WfJournal) -> FlowBacking {
        FlowBacking {
            journal,
            restrict: RestrictSet::new(),
            shred: None,
        }
    }

    pub fn with_restrict(journal: WfJournal, restrict: RestrictSet) -> FlowBacking {
        FlowBacking {
            journal,
            restrict,
            shred: None,
        }
    }

    pub fn with_crypto_shred(
        mut self,
        kms: std::sync::Arc<KmsEngine>,
        region: Region,
        telemetry: FlowTelemetry,
    ) -> FlowBacking {
        self.shred = Some(ShredWiring {
            kms,
            region,
            telemetry,
        });
        self
    }

    pub fn restrict_set(&self) -> &RestrictSet {
        &self.restrict
    }
}

#[derive(Clone, Default)]
pub struct WfHistoryHolder {
    backing: Option<FlowBacking>,
}

impl WfHistoryHolder {
    pub fn with_journal(journal: WfJournal) -> WfHistoryHolder {
        WfHistoryHolder {
            backing: Some(FlowBacking::new(journal)),
        }
    }

    pub fn with_backing(backing: FlowBacking) -> WfHistoryHolder {
        WfHistoryHolder {
            backing: Some(backing),
        }
    }

    pub fn with_crypto_shred(
        journal: WfJournal,
        kms: std::sync::Arc<KmsEngine>,
        region: Region,
        telemetry: FlowTelemetry,
    ) -> WfHistoryHolder {
        WfHistoryHolder {
            backing: Some(FlowBacking::new(journal).with_crypto_shred(kms, region, telemetry)),
        }
    }

    pub fn register(&self, registry: &mut HolderRegistry) -> FlowHolderRegistration {
        registry.open(StoreKind::Oltp, FLOW_OLTP_STORE)
    }

    pub fn restrict_set(&self) -> Option<&RestrictSet> {
        self.backing.as_ref().map(|b| b.restrict_set())
    }

    fn subject_id(subject: &SubjectRef) -> String {
        subject.principal.principal_id.0.clone()
    }

    fn row_references_subject(row: &crate::schema::WfHistoryRow, subject_id: &str) -> bool {
        let in_refs = row
            .result
            .as_ref()
            .map(|refs| {
                refs.iter().any(|r| {
                    r.0.ends_with(&format!("/principal/{subject_id}"))
                        || r.0.contains(&format!("/principal/{subject_id}/"))
                })
            })
            .unwrap_or(false);
        let in_key_ref = row
            .result_key_ref
            .as_ref()
            .map(|k| {
                k.ends_with(&format!("/subject/{subject_id}"))
                    || k.contains(&format!("/subject/{subject_id}/"))
            })
            .unwrap_or(false);
        in_refs || in_key_ref
    }

    fn count_appearances(&self, tenant: &GdprTenantId, subject_id: &str) -> usize {
        let Some(b) = &self.backing else {
            return 0;
        };
        let t = TenantId(tenant.0.clone());
        b.journal
            .history_in_tenant(&t)
            .iter()
            .filter(|row| Self::row_references_subject(row, subject_id))
            .count()
    }

    fn count_inline_pii_rows(&self, tenant: &GdprTenantId, subject_id: &str) -> usize {
        let Some(b) = &self.backing else {
            return 0;
        };
        let t = TenantId(tenant.0.clone());
        b.journal
            .history_in_tenant(&t)
            .iter()
            .filter(|row| history_row_has_inline_pii(row, subject_id))
            .count()
    }
}

impl PersonalDataHolder for WfHistoryHolder {
    fn locate(&self, subject: &SubjectRef, tenant: GdprTenantId) -> DsrResult<LocateReport> {
        let sid = Self::subject_id(subject);
        let count = self.count_appearances(&tenant, &sid);
        let outcome = format!(
            "located {count} wf_history rows naming the subject (referenced-actor result refs + \
             inline-PII result_key_ref, references-not-payloads - no stored name)"
        );
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                FLOW_OLTP_STORE,
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
                FLOW_OLTP_STORE,
                &sid,
                &tenant.0,
                &format!(
                    "references-not-payloads bundle: {count} wf_history appearances, no free-text body"
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
                FLOW_OLTP_STORE,
                &Self::subject_id(subject),
                "",
                "no-op (references-not-payloads - rectify via replay-from-source + read-time \
                 re-resolve, P-FLOW-05)",
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
            format!(
                "restrict={on} recorded in the suppression set (new dispatch suppressed; \
                 indexing/agent-use too)"
            )
        } else {
            format!(
                "restrict={on} no-op (no live dispatch; suppression consult lands with the replay/\
                 lease loop P-FLOW-05)"
            )
        };
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                FLOW_OLTP_STORE,
                &sid,
                "",
                &outcome,
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let sid = match &scope {
            EraseScope::Subject { subject, .. } => Self::subject_id(subject),
            EraseScope::Tenant(_) => String::new(),
        };

        if let Some(b) = &self.backing {
            if let Some(w) = &b.shred {
                let inline_pii_rows = match &scope {
                    EraseScope::Subject { tenant, .. } => self.count_inline_pii_rows(tenant, &sid),
                    EraseScope::Tenant(_) => 0,
                };
                let cascade = WfCryptoShred::with_telemetry(&w.kms, w.region.clone(), &w.telemetry);
                let report = cascade.shred_subject(&scope, inline_pii_rows, 0, 0);
                return Ok(crate::crypto_shred::aggregate_receipt(&report, &scope));
            }
        }

        let tenant = match &scope {
            EraseScope::Subject { tenant, .. } => tenant.0.clone(),
            EraseScope::Tenant(t) => t.0.clone(),
        };
        let count = match &scope {
            EraseScope::Subject { tenant, .. } => self.count_appearances(tenant, &sid),
            EraseScope::Tenant(_) => 0,
        };
        let outcome = match &scope {
            EraseScope::Subject { .. } => format!(
                "structural erase: {count} wf_history appearances tombstone for free (refs-not-\
                 payloads; Identity §4.8 pseudonym-shred makes the opaque id unresolvable) - 0 PII \
                 columns mutated; the inline-PII per-subject-DEK crypto-shred reach is wired via \
                 WfHistoryHolder::with_crypto_shred (P-FLOW-24); replay P-FLOW-05"
            ),
            EraseScope::Tenant(_) => {
                "tenant crypto-shred: destroy the per-tenant DEK (11.3/11.4) - \
                 every workflow row unrecoverable"
                    .into()
            }
        };
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                FLOW_OLTP_STORE,
                &sid,
                &tenant,
                &outcome,
                None,
                0,
            ),
        })
    }
}

pub fn flow_history_holder() -> Option<Holder> {
    myelin_substrate::classify_store(StoreKind::Oltp, FLOW_OLTP_STORE, &flow_store_classifier())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::WfHistoryRow;
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

    fn history_row(run_id: &str, seq: i64, actor: &str, key_subject: Option<&str>) -> WfHistoryRow {
        WfHistoryRow {
            tenant: t(),
            region: Region::new("fr-par"),
            run_id: run_id.into(),
            seq,
            kind: "activity_completed".into(),
            command_id: format!("agent.run:{seq}"),
            result: Some(vec![ArtifactRef(format!(
                "myelin://acme/identity/principal/{actor}"
            ))]),
            result_key_ref: key_subject.map(|s| format!("kms://acme/subject/{s}")),
        }
    }

    #[test]
    fn flow_registers_its_store_as_a_holder() {
        let registry = register_flow_holder();
        assert!(registry.is_registered(StoreKind::Oltp, FLOW_OLTP_STORE));
        assert_eq!(registry.len(), 1, "exactly the one flow store registered");
    }

    #[test]
    fn flow_store_name_matches_the_boot_registered_name() {
        assert_eq!(FLOW_OLTP_STORE, crate::SERVICE_NAME);
    }

    #[test]
    fn re_registration_is_idempotent() {
        let mut registry = register_flow_holder();
        WfHistoryHolder::default().register(&mut registry);
        assert_eq!(
            registry.len(),
            1,
            "re-opening the same flow store does not double-register"
        );
    }

    #[test]
    fn flow_store_classifies_to_h8_no_orphan() {
        let registry = register_flow_holder();
        let classifier = flow_store_classifier();
        assert_eq!(
            classify_store(StoreKind::Oltp, FLOW_OLTP_STORE, &classifier),
            Some(Holder::H8EventBus),
            "the flow OLTP store is holder H8 (the §5.5 references-not-payloads reconcile)"
        );
        assert_eq!(flow_history_holder(), Some(Holder::H8EventBus));
        assert_eq!(
            assert_holder_completeness(registry.registrations(), &classifier),
            Ok(()),
            "the flow store is in the exhaustive H1–H18 list - 0 orphan stores"
        );
    }

    #[test]
    fn locate_counts_real_appearances_backed_vs_unbacked() {
        let unbacked = WfHistoryHolder::default();
        assert_eq!(
            unbacked.count_appearances(&tenant(), "u-x"),
            0,
            "unbacked → empty-but-correct"
        );

        let journal = WfJournal::new();
        journal.append_history_for_test(history_row("run-1", 0, "u-x", None));
        journal.append_history_for_test(history_row("run-2", 0, "u-y", Some("u-x")));
        journal.append_history_for_test(history_row("run-3", 0, "u-y", None));
        let backed = WfHistoryHolder::with_journal(journal);
        assert_eq!(
            backed.count_appearances(&tenant(), "u-x"),
            2,
            "both structural appearances counted (result ref + inline-PII key ref)"
        );
        assert_eq!(
            backed.count_appearances(&tenant(), "u-none"),
            0,
            "an absent subject → 0"
        );
    }

    #[test]
    fn structural_erase_tombstones_refs_stored_rows_with_zero_pii_mutation() {
        let journal = WfJournal::new();
        journal.append_history_for_test(history_row("run-1", 0, "u-erase", None));
        journal.append_history_for_test(history_row("run-2", 0, "u-bob", Some("u-erase")));
        journal.append_history_for_test(history_row("run-3", 0, "u-carol", None));

        let holder = WfHistoryHolder::with_journal(journal.clone());

        let before: Vec<WfHistoryRow> = journal.history_in_tenant(&t());
        let subj_before: Vec<&WfHistoryRow> = before
            .iter()
            .filter(|r| WfHistoryHolder::row_references_subject(r, "u-erase"))
            .collect();
        assert_eq!(
            subj_before.len(),
            2,
            "locate finds both appearances (result ref + key ref)"
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
            "0 keys shredded at the flow surface (refs-stored; inline-PII DEK shred is P-FLOW-24)"
        );

        let after: Vec<WfHistoryRow> = journal.history_in_tenant(&t());
        assert_eq!(
            after, before,
            "the refs-stored rows tombstone for FREE - 0 PII columns mutated (references-not-payloads)"
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
    fn restrict_writes_the_shared_suppression_set() {
        let restrict = RestrictSet::new();
        let backing = FlowBacking::with_restrict(WfJournal::new(), restrict.clone());
        let holder = WfHistoryHolder::with_backing(backing);
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

        let unbacked = WfHistoryHolder::default();
        assert!(
            unbacked.restrict(&subj, true).is_ok(),
            "unbacked restrict is a no-op receipt"
        );
    }

    #[test]
    fn unbacked_holder_is_empty_but_correct() {
        let holder = WfHistoryHolder::default();
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
    fn export_reports_the_appearance_count() {
        let journal = WfJournal::new();
        journal.append_history_for_test(history_row("run-1", 0, "u-e", None));
        journal.append_history_for_test(history_row("run-2", 0, "u-e", None));
        let holder = WfHistoryHolder::with_journal(journal);
        let exp = holder
            .export(&subject("u-e"), tenant())
            .expect("export succeeds");
        assert!(exp.receipt.content_hash.starts_with("blake3:"));
        assert!(
            exp.receipt.key_epoch_destroyed.is_none(),
            "export shreds no key"
        );
    }

    #[test]
    fn locate_is_tenant_scoped() {
        let journal = WfJournal::new();
        journal.append_history_for_test(history_row("run-1", 0, "u-x", None));
        let holder = WfHistoryHolder::with_journal(journal);
        assert_eq!(
            holder.count_appearances(&GdprTenantId::from_token("acme"), "u-x"),
            1
        );
        assert_eq!(
            holder.count_appearances(&GdprTenantId::from_token("other"), "u-x"),
            0,
            "the acme row does not count for tenant `other` - the scan is tenant-first"
        );
    }

    #[test]
    fn tenant_erase_reports_the_per_tenant_dek_lever() {
        let holder = WfHistoryHolder::with_journal(WfJournal::new());
        let er = holder
            .erase(EraseScope::Tenant(tenant()))
            .expect("tenant erase succeeds");
        assert_eq!(er.receipt.operation, "erase");
        assert!(
            er.receipt.key_epoch_destroyed.is_none(),
            "the per-subject DEK shred is P-FLOW-24"
        );
    }

    #[test]
    fn restrict_set_accessors_return_the_shared_set() {
        let restrict = RestrictSet::new();
        let backing = FlowBacking::with_restrict(WfJournal::new(), restrict.clone());
        backing.restrict_set().set("u-shared", true);
        assert!(
            restrict.is_restricted("u-shared"),
            "the backing accessor is the shared set"
        );

        let holder = WfHistoryHolder::with_backing(backing);
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
            "a write through the holder accessor reaches it"
        );

        assert!(
            WfHistoryHolder::default().restrict_set().is_none(),
            "unbacked → no restrict set"
        );
    }

    #[test]
    fn holder_is_object_safe() {
        let holders: Vec<Box<dyn PersonalDataHolder>> = vec![Box::new(WfHistoryHolder::default())];
        let subj = subject("u-3");
        for h in &holders {
            assert!(
                h.locate(&subj, tenant()).is_ok(),
                "the holder responds to the contract"
            );
        }
    }
}

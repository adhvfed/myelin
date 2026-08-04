use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_events::{InlinePiiShredder, PiiKeyRef, ShredError};
use myelin_gdpr::{
    DsrError, EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle,
    Receipt, RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};

use crate::holder::{AGENT_OLTP_STORE, AGENT_TRACE_STORE};

pub fn subject_dek_ref(tenant: &str, subject: &str) -> PiiKeyRef {
    PiiKeyRef(format!("kms://{tenant}/0/subject:{subject}"))
}

fn subject_id(subject: &SubjectRef) -> String {
    subject.principal.principal_id.0.clone()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FreeTextRow {
    pub run_id: u128,
    pub column: &'static str,
    pub subject: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunAttribution {
    pub run_id: u128,
    pub subject: String,
}

#[derive(Default)]
pub struct AgentFabricStore {
    free_text: Vec<FreeTextRow>,
    attributions: Vec<RunAttribution>,
    tombstoned_runs: std::collections::BTreeSet<u128>,
    pseudonymised_runs: std::collections::BTreeSet<u128>,
}

impl AgentFabricStore {
    pub fn new() -> AgentFabricStore {
        AgentFabricStore::default()
    }

    pub fn write_free_text(&mut self, run_id: u128, column: &'static str, subject: &str) {
        self.free_text.push(FreeTextRow {
            run_id,
            column,
            subject: subject.to_string(),
        });
    }

    pub fn write_attribution(&mut self, run_id: u128, subject: &str) {
        self.attributions.push(RunAttribution {
            run_id,
            subject: subject.to_string(),
        });
    }

    fn rows_for(&self, subject: &str) -> Vec<&FreeTextRow> {
        self.free_text
            .iter()
            .filter(|r| r.subject == subject)
            .collect()
    }

    fn attributions_for(&self, subject: &str) -> Vec<&RunAttribution> {
        self.attributions
            .iter()
            .filter(|r| r.subject == subject)
            .collect()
    }

    pub fn is_tombstoned(&self, run_id: u128) -> bool {
        self.tombstoned_runs.contains(&run_id)
    }

    pub fn is_pseudonymised(&self, run_id: u128) -> bool {
        self.pseudonymised_runs.contains(&run_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FabricLocateReport {
    pub subject: String,
    pub free_text_rows: Vec<(u128, &'static str)>,
    pub attribution_runs: Vec<u128>,
    pub subject_dek: PiiKeyRef,
    pub memory_seam: Option<&'static str>,
}

pub struct AgentFabricHolder<S: InlinePiiShredder> {
    tenant: TenantId,
    store: Mutex<AgentFabricStore>,
    shredder: S,
    at_ms: u64,
}

impl<S: InlinePiiShredder> AgentFabricHolder<S> {
    pub fn new(tenant: TenantId, store: AgentFabricStore, shredder: S) -> AgentFabricHolder<S> {
        AgentFabricHolder {
            tenant,
            store: Mutex::new(store),
            shredder,
            at_ms: 0,
        }
    }

    pub fn shredder(&self) -> &S {
        &self.shredder
    }

    pub fn locate_fabric(&self, subject: &SubjectRef) -> FabricLocateReport {
        let subj = subject_id(subject);
        let store = self.store.lock().expect("fabric store poisoned");
        let free_text_rows = store
            .rows_for(&subj)
            .iter()
            .map(|r| (r.run_id, r.column))
            .collect();
        let attribution_runs = store
            .attributions_for(&subj)
            .iter()
            .map(|r| r.run_id)
            .collect();
        FabricLocateReport {
            subject: subj.clone(),
            free_text_rows,
            attribution_runs,
            subject_dek: subject_dek_ref(&self.tenant.0, &subj),
            memory_seam: None,
        }
    }

    pub fn erase_fabric(&self, subject: &SubjectRef) -> Result<FabricEraseReceipt, ShredError> {
        let subj = subject_id(subject);
        let dek = subject_dek_ref(&self.tenant.0, &subj);
        self.shredder.destroy_key(&dek)?;

        let mut store = self.store.lock().expect("fabric store poisoned");
        let run_ids: Vec<u128> = store.rows_for(&subj).iter().map(|r| r.run_id).collect();
        for run_id in &run_ids {
            store.tombstoned_runs.insert(*run_id);
        }
        let attribution_ids: Vec<u128> = store
            .attributions_for(&subj)
            .iter()
            .map(|r| r.run_id)
            .collect();
        for run_id in &attribution_ids {
            store.pseudonymised_runs.insert(*run_id);
        }
        let free_text_shredded = run_ids.len();
        let attribution_pseudonymised = attribution_ids.len();
        drop(store);

        let recoverable = usize::from(self.shredder.is_live(&dek));

        Ok(FabricEraseReceipt {
            subject: subj,
            tenant: self.tenant.clone(),
            dek: dek.clone(),
            dek_destroyed: !self.shredder.is_live(&dek),
            free_text_shredded,
            attribution_pseudonymised,
            recoverable,
            memory_embeddings_purged: 0,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FabricEraseReceipt {
    pub subject: String,
    pub tenant: TenantId,
    pub dek: PiiKeyRef,
    pub dek_destroyed: bool,
    pub free_text_shredded: usize,
    pub attribution_pseudonymised: usize,
    pub recoverable: usize,
    pub memory_embeddings_purged: usize,
}

impl FabricEraseReceipt {
    pub fn is_green(&self) -> bool {
        self.dek_destroyed && self.recoverable == 0
    }
}

impl<S: InlinePiiShredder> PersonalDataHolder for AgentFabricHolder<S> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let report = self.locate_fabric(subject);
        let trace_rows = report
            .free_text_rows
            .iter()
            .filter(|(_, col)| *col == "trace.trace_body")
            .count();
        let outcome = format!(
            "located {} free-text rows ({trace_rows} in {AGENT_TRACE_STORE}) + {} attribution edges over the per-subject DEK {} (memory: {})",
            report.free_text_rows.len(),
            report.attribution_runs.len(),
            report.subject_dek.0,
            report
                .memory_seam
                .unwrap_or("named seam - v1 stateless except trace (AG-P25)"),
        );
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                AGENT_OLTP_STORE,
                &report.subject,
                &tenant.0,
                &outcome,
                None,
                self.at_ms,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        let report = self.locate_fabric(subject);
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                AGENT_OLTP_STORE,
                &report.subject,
                &tenant.0,
                &format!(
                    "portable bundle: {} free-text rows over the per-subject DEK",
                    report.free_text_rows.len()
                ),
                None,
                self.at_ms,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                AGENT_OLTP_STORE,
                &subject_id(subject),
                &self.tenant.0,
                "rectify-by-rewrite over the content-addressed source (trace write AG-P19)",
                None,
                self.at_ms,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                AGENT_OLTP_STORE,
                &subject_id(subject),
                &self.tenant.0,
                &format!("suppress agent-use on={on} (honoured-everywhere proof GDPR M2 P-GA-25)"),
                None,
                self.at_ms,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let subject = match &scope {
            EraseScope::Subject { subject, .. } => subject.clone(),
            EraseScope::Tenant(_) => {
                return Ok(EraseReceipt {
                    receipt: Receipt::content_addressed(
                        "erase",
                        AGENT_OLTP_STORE,
                        "",
                        &self.tenant.0,
                        "tenant offboarding → per-tenant KEK destroy (storage §5.3); Fabric rides it",
                        None,
                        self.at_ms,
                    ),
                });
            }
        };
        let receipt = self
            .erase_fabric(&subject)
            .map_err(|e| DsrError(format!("Fabric erase INCOMPLETE: {e}")))?;
        if !receipt.is_green() {
            return Err(DsrError(format!(
                "Fabric erase RED: {} recoverable free-text remain for {}",
                receipt.recoverable, receipt.subject
            )));
        }
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                AGENT_OLTP_STORE,
                &receipt.subject,
                &self.tenant.0,
                &format!(
                    "crypto-shred per-subject DEK ({} free-text shredded, {} attribution → pseudonym, 0 recoverable; memory seam AG-P25)",
                    receipt.free_text_shredded, receipt.attribution_pseudonymised
                ),
                Some(0),
                self.at_ms,
            ),
        })
    }
}

#[derive(Clone, Default)]
pub struct FabricErasureLedger {
    entries: Arc<Mutex<BTreeMap<String, PiiKeyRef>>>,
}

impl FabricErasureLedger {
    pub fn new() -> FabricErasureLedger {
        FabricErasureLedger::default()
    }

    pub fn record(&self, subject: &str, dek: PiiKeyRef) {
        self.entries
            .lock()
            .expect("fabric erasure ledger poisoned")
            .insert(subject.to_string(), dek);
    }

    pub fn is_erased(&self, subject: &str) -> bool {
        self.entries
            .lock()
            .expect("fabric erasure ledger poisoned")
            .contains_key(subject)
    }

    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .expect("fabric erasure ledger poisoned")
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn re_erase_after_restore<S: InlinePiiShredder>(
        &self,
        shredder: &S,
    ) -> Result<FabricReErasureReceipt, ShredError> {
        let entries: Vec<(String, PiiKeyRef)> = self
            .entries
            .lock()
            .expect("fabric erasure ledger poisoned")
            .iter()
            .map(|(s, k)| (s.clone(), k.clone()))
            .collect();

        let keys_resurrected_by_restore = entries
            .iter()
            .filter(|(_, dek)| shredder.is_live(dek))
            .count();

        for (_, dek) in &entries {
            shredder.destroy_key(dek)?;
        }

        let resurrected = entries
            .iter()
            .filter(|(_, dek)| shredder.is_live(dek))
            .count();

        Ok(FabricReErasureReceipt {
            re_erased_subjects: entries.len(),
            keys_resurrected_by_restore,
            resurrected,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FabricReErasureReceipt {
    pub re_erased_subjects: usize,
    pub keys_resurrected_by_restore: usize,
    pub resurrected: usize,
}

impl FabricReErasureReceipt {
    pub fn is_green(&self) -> bool {
        self.resurrected == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::InMemoryShredder;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId as TyTenantId;

    fn tenant() -> TenantId {
        TyTenantId("acme".into())
    }

    fn subject_ref(id: &str) -> SubjectRef {
        SubjectRef::new(Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            tenant(),
        ))
    }

    fn seeded_holder(subject: &str) -> AgentFabricHolder<InMemoryShredder> {
        let mut store = AgentFabricStore::new();
        store.write_free_text(1, "proposed_effect.input_payload", subject);
        store.write_free_text(1, "hitl_gate.risk_summary", subject);
        store.write_free_text(1, "trace.trace_body", subject);
        store.write_attribution(1, subject);
        let shredder = InMemoryShredder::new();
        shredder.seal(&subject_dek_ref("acme", subject));
        AgentFabricHolder::new(tenant(), store, shredder)
    }

    #[test]
    fn locate_walks_free_text_attribution_and_names_the_memory_seam() {
        let holder = seeded_holder("psn:alice");
        let report = holder.locate_fabric(&subject_ref("psn:alice"));
        assert_eq!(
            report.free_text_rows.len(),
            3,
            "3 free-text PII rows located"
        );
        assert_eq!(
            report.attribution_runs,
            vec![1],
            "1 attribution edge located"
        );
        assert_eq!(report.subject_dek, subject_dek_ref("acme", "psn:alice"));
        assert!(
            report.memory_seam.is_none(),
            "v1 has no embedding store - the memory leg is the NAMED SEAM (AG-P25)"
        );
    }

    #[test]
    fn erase_crypto_shreds_the_dek_zero_recoverable_attribution_to_pseudonym() {
        let holder = seeded_holder("psn:alice");
        let dek = subject_dek_ref("acme", "psn:alice");
        assert!(holder.shredder().is_live(&dek), "the DEK is live pre-erase");

        let receipt = holder
            .erase_fabric(&subject_ref("psn:alice"))
            .expect("the Fabric erase succeeds (KMS reachable)");

        assert_eq!(receipt.recoverable, 0, "0 recoverable free-text post-erase");
        assert!(receipt.dek_destroyed, "the per-subject DEK is destroyed");
        assert!(
            !holder.shredder().is_live(&dek),
            "the DEK does NOT resolve after the crypto-shred (live + backups)"
        );
        assert_eq!(
            receipt.free_text_shredded, 3,
            "all 3 free-text rows shredded"
        );
        assert_eq!(
            receipt.attribution_pseudonymised, 1,
            "the attribution edge → opaque pseudonym (4.8)"
        );
        assert_eq!(
            receipt.memory_embeddings_purged, 0,
            "0 embeddings purged - the named memory seam (AG-P25)"
        );
        assert!(receipt.is_green(), "the Fabric erase leg is GREEN");

        let store = holder.store.lock().unwrap();
        assert!(store.is_tombstoned(1), "the run's free-text is tombstoned");
        assert!(
            store.is_pseudonymised(1),
            "the attribution is pseudonymised"
        );
    }

    #[test]
    fn erase_is_idempotent_and_loud_on_kms_failure() {
        let holder = seeded_holder("psn:bob");
        holder
            .erase_fabric(&subject_ref("psn:bob"))
            .expect("first erase");
        let r2 = holder
            .erase_fabric(&subject_ref("psn:bob"))
            .expect("re-erase is a no-op success");
        assert_eq!(r2.recoverable, 0, "still 0 recoverable on the re-erase");

        let mut store = AgentFabricStore::new();
        store.write_free_text(2, "trace.trace_body", "psn:carol");
        let shredder = InMemoryShredder::new();
        let dek = subject_dek_ref("acme", "psn:carol");
        shredder.seal(&dek);
        shredder.make_unreachable(&dek);
        let holder2 = AgentFabricHolder::new(tenant(), store, shredder);
        let err = holder2
            .erase_fabric(&subject_ref("psn:carol"))
            .expect_err("an unreachable KMS makes the erase LOUD, never a silent green");
        assert!(matches!(err, ShredError::KmsUnavailable(_)));
    }

    #[test]
    fn personal_data_holder_erase_body_records_the_destroyed_key_epoch() {
        let holder = seeded_holder("psn:dave");
        let receipt = holder
            .erase(EraseScope::Subject {
                subject: subject_ref("psn:dave"),
                tenant: tenant(),
            })
            .expect("the holder erase body succeeds");
        assert_eq!(receipt.receipt.operation, "erase");
        assert_eq!(
            receipt.receipt.key_epoch_destroyed,
            Some(0),
            "the erase records the destroyed per-subject DEK epoch (the GD-4 audit trail)"
        );
        assert!(receipt.receipt.content_hash.starts_with("blake3:"));
    }

    #[test]
    fn locate_and_export_bodies_are_real_receipts() {
        let holder = seeded_holder("psn:erin");
        let locate = holder
            .locate(&subject_ref("psn:erin"), tenant())
            .expect("locate");
        assert_eq!(locate.receipt.operation, "locate");
        assert!(locate.receipt.content_hash.starts_with("blake3:"));
        let export = holder
            .export(&subject_ref("psn:erin"), tenant())
            .expect("export");
        assert_eq!(export.receipt.operation, "export");
    }

    #[test]
    fn the_fabric_holder_is_object_safe() {
        let mut store = AgentFabricStore::new();
        store.write_free_text(9, "trace.trace_body", "psn:frank");
        let shredder = InMemoryShredder::new();
        shredder.seal(&subject_dek_ref("acme", "psn:frank"));
        let holder: Box<dyn PersonalDataHolder> =
            Box::new(AgentFabricHolder::new(tenant(), store, shredder));
        assert!(holder.locate(&subject_ref("psn:frank"), tenant()).is_ok());
        assert!(holder
            .erase(EraseScope::Subject {
                subject: subject_ref("psn:frank"),
                tenant: tenant()
            })
            .is_ok());
    }

    #[test]
    fn post_restore_re_erasure_keeps_the_dek_destroyed() {
        let subject = "psn:grace";
        let dek = subject_dek_ref("acme", subject);
        let shredder = InMemoryShredder::new();
        shredder.seal(&dek);
        let mut store = AgentFabricStore::new();
        store.write_free_text(3, "trace.trace_body", subject);
        let holder = AgentFabricHolder::new(tenant(), store, shredder.clone());
        let ledger = FabricErasureLedger::new();

        let r = holder.erase_fabric(&subject_ref(subject)).expect("erase");
        ledger.record(subject, r.dek.clone());
        assert!(!shredder.is_live(&dek), "the DEK is destroyed post-erase");
        assert!(ledger.is_erased(subject), "the ledger remembers the erase");

        shredder.seal(&dek);
        assert!(shredder.is_live(&dek), "the restore resurrected the DEK");

        let receipt = ledger
            .re_erase_after_restore(&shredder)
            .expect("re-erasure runs");
        assert_eq!(
            receipt.keys_resurrected_by_restore, 1,
            "the restore brought back 1 DEK (the honest signal)"
        );
        assert_eq!(
            receipt.resurrected, 0,
            "0 resurrected post re-erasure - GREEN"
        );
        assert!(receipt.is_green());
        assert!(
            !shredder.is_live(&dek),
            "the DEK is destroyed again after the re-erasure pass"
        );
    }

    #[test]
    fn the_erasure_ledger_is_pii_free_and_idempotent() {
        let ledger = FabricErasureLedger::new();
        assert!(ledger.is_empty());
        let dek = subject_dek_ref("acme", "psn:heidi");
        ledger.record("psn:heidi", dek.clone());
        ledger.record("psn:heidi", dek.clone());
        assert_eq!(ledger.len(), 1, "one subject recorded (idempotent)");
        assert!(ledger.is_erased("psn:heidi"));
        assert!(!ledger.is_erased("psn:never-erased"));
    }
}

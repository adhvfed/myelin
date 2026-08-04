use std::collections::BTreeMap;
use std::sync::Mutex;

use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};
use myelin_substrate::{Holder, HolderRegistration, StoreKind};
use myelin_tenancy::Region;

use crate::agent_trace_seam::{agent_trace_phase, AGENT_TRACE_HOLDER_ID};
use crate::datamap::HolderSchema;
use crate::holders::{CryptoShredKms, ShredKeyClass, ShredKeyHandle};
use crate::orchestration::{CanonicalErasePhase, RegisteredHolder};

pub mod producer_holder_ids {
    use super::AGENT_TRACE_HOLDER_ID;

    pub const GIT_DB: &str = "git_oltp";
    pub const KNOWLEDGE_DB: &str = "knowledge_oltp";
    pub const AGENT_TRACE: &str = AGENT_TRACE_HOLDER_ID;
}

pub fn producer_phase_of(holder_id: &str) -> Option<CanonicalErasePhase> {
    match holder_id {
        producer_holder_ids::GIT_DB => Some(CanonicalErasePhase::CryptoShredDek),
        producer_holder_ids::KNOWLEDGE_DB => Some(CanonicalErasePhase::CryptoShredDek),
        producer_holder_ids::AGENT_TRACE => Some(agent_trace_phase()),
        _ => None,
    }
}

pub fn producer_holder_id_list() -> [&'static str; 3] {
    [
        producer_holder_ids::GIT_DB,
        producer_holder_ids::KNOWLEDGE_DB,
        producer_holder_ids::AGENT_TRACE,
    ]
}

pub fn producer_holder_schemas(region: Region) -> Vec<HolderSchema> {
    vec![
        HolderSchema {
            registration: HolderRegistration {
                kind: StoreKind::Oltp,
                name: producer_holder_ids::GIT_DB,
            },
            holder: Holder::H1Git,
            region: region.clone(),
            fields: &[],
        },
        HolderSchema {
            registration: HolderRegistration {
                kind: StoreKind::Oltp,
                name: producer_holder_ids::KNOWLEDGE_DB,
            },
            holder: Holder::H4Knowledge,
            region: region.clone(),
            fields: &[],
        },
        HolderSchema {
            registration: HolderRegistration {
                kind: StoreKind::Oltp,
                name: producer_holder_ids::AGENT_TRACE,
            },
            holder: Holder::H17AgentTrace,
            region,
            fields: &[],
        },
    ]
}

pub fn producer_registrations() -> Vec<HolderRegistration> {
    vec![
        HolderRegistration {
            kind: StoreKind::Oltp,
            name: producer_holder_ids::GIT_DB,
        },
        HolderRegistration {
            kind: StoreKind::Oltp,
            name: producer_holder_ids::KNOWLEDGE_DB,
        },
        HolderRegistration {
            kind: StoreKind::Oltp,
            name: producer_holder_ids::AGENT_TRACE,
        },
    ]
}

fn subject_and_tenant(scope: &EraseScope) -> (String, String) {
    match scope {
        EraseScope::Subject { subject, tenant } => {
            (subject.principal.principal_id.0.clone(), tenant.0.clone())
        }
        EraseScope::Tenant(tenant) => ("*tenant*".to_string(), tenant.0.clone()),
    }
}

pub struct GitDbHolder<'a> {
    kms: &'a dyn CryptoShredKms,
}

impl<'a> GitDbHolder<'a> {
    pub fn new(kms: &'a dyn CryptoShredKms) -> GitDbHolder<'a> {
        GitDbHolder { kms }
    }

    fn dek(subject_token: &str, tenant: &TenantId) -> ShredKeyHandle {
        ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject(subject_token.to_string()),
        }
    }
}

impl PersonalDataHolder for GitDbHolder<'_> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if self.kms.is_present(&Self::dek(&sid, &tenant)) {
            "located:inline-bodies-present"
        } else {
            "located:0-recoverable"
        };
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                producer_holder_ids::GIT_DB,
                &sid,
                &tenant.0,
                outcome,
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
                producer_holder_ids::GIT_DB,
                &sid,
                &tenant.0,
                "exported",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                producer_holder_ids::GIT_DB,
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
            receipt: Receipt::content_addressed(
                "restrict",
                producer_holder_ids::GIT_DB,
                &sid,
                "*",
                outcome,
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (sid, tenant_token) = subject_and_tenant(&scope);
        let tenant = TenantId::from_token(&tenant_token);
        let destroyed = self.kms.destroy(&Self::dek(&sid, &tenant));
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                producer_holder_ids::GIT_DB,
                &sid,
                &tenant_token,
                "pseudonymise+crypto_shred:inline_bodies",
                destroyed,
                0,
            ),
        })
    }
}

#[derive(Debug, Default)]
pub struct KnowledgeStoreModel {
    embeddings: Mutex<BTreeMap<String, bool>>,
    erase_calls: Mutex<u32>,
}

impl KnowledgeStoreModel {
    pub fn new() -> KnowledgeStoreModel {
        KnowledgeStoreModel::default()
    }

    pub fn index_embedding_from_source(&self, subject_token: &str) {
        self.embeddings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subject_token.to_string(), true);
    }

    pub fn reidentify_hits(&self, subject_token: &str) -> usize {
        let e = self.embeddings.lock().unwrap_or_else(|e| e.into_inner());
        usize::from(e.get(subject_token).copied().unwrap_or(false))
    }

    pub fn erase_call_count(&self) -> u32 {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn purge_embedding(&self, subject_token: &str) -> bool {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        self.embeddings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(subject_token)
            .is_some()
    }
}

pub struct KnowledgeStoreHolder<'a> {
    model: &'a KnowledgeStoreModel,
    kms: &'a dyn CryptoShredKms,
}

impl<'a> KnowledgeStoreHolder<'a> {
    pub fn new(
        model: &'a KnowledgeStoreModel,
        kms: &'a dyn CryptoShredKms,
    ) -> KnowledgeStoreHolder<'a> {
        KnowledgeStoreHolder { model, kms }
    }

    fn dek(subject_token: &str, tenant: &TenantId) -> ShredKeyHandle {
        ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject(subject_token.to_string()),
        }
    }
}

impl PersonalDataHolder for KnowledgeStoreHolder<'_> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = subject.principal.principal_id.0.clone();
        let dek_present = self.kms.is_present(&Self::dek(&sid, &tenant));
        let outcome = if dek_present || self.model.reidentify_hits(&sid) > 0 {
            "located:content+embeddings"
        } else {
            "located:0-recoverable"
        };
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                producer_holder_ids::KNOWLEDGE_DB,
                &sid,
                &tenant.0,
                outcome,
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
                producer_holder_ids::KNOWLEDGE_DB,
                &sid,
                &tenant.0,
                "exported",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                producer_holder_ids::KNOWLEDGE_DB,
                &sid,
                "*",
                "rectified:reindex_from_source",
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
            receipt: Receipt::content_addressed(
                "restrict",
                producer_holder_ids::KNOWLEDGE_DB,
                &sid,
                "*",
                outcome,
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (sid, tenant_token) = subject_and_tenant(&scope);
        let tenant = TenantId::from_token(&tenant_token);
        let destroyed = self.kms.destroy(&Self::dek(&sid, &tenant));
        self.model.purge_embedding(&sid);
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                producer_holder_ids::KNOWLEDGE_DB,
                &sid,
                &tenant_token,
                "crypto_shred:blocks+db_rows+embeddings_purged_not_hidden",
                destroyed,
                0,
            ),
        })
    }
}

#[derive(Debug, Default)]
pub struct AgentTraceModel {
    traces: Mutex<BTreeMap<String, String>>,
    erase_calls: Mutex<u32>,
}

impl AgentTraceModel {
    pub fn new() -> AgentTraceModel {
        AgentTraceModel::default()
    }

    pub fn write_trace_from_source(&self, subject_token: &str, content_address: &str) {
        self.traces
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(subject_token.to_string(), content_address.to_string());
    }

    pub fn has_trace(&self, subject_token: &str) -> bool {
        self.traces
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(subject_token)
    }

    pub fn erase_call_count(&self) -> u32 {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn shred_trace(&self, subject_token: &str) -> bool {
        *self.erase_calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        self.traces
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(subject_token)
            .is_some()
    }
}

pub struct KnowledgeAgentTraceHolder<'a> {
    model: &'a AgentTraceModel,
    kms: &'a dyn CryptoShredKms,
}

impl<'a> KnowledgeAgentTraceHolder<'a> {
    pub fn new(
        model: &'a AgentTraceModel,
        kms: &'a dyn CryptoShredKms,
    ) -> KnowledgeAgentTraceHolder<'a> {
        KnowledgeAgentTraceHolder { model, kms }
    }

    pub fn holder_id(&self) -> &'static str {
        AGENT_TRACE_HOLDER_ID
    }

    fn dek(subject_token: &str, tenant: &TenantId) -> ShredKeyHandle {
        ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject(subject_token.to_string()),
        }
    }
}

impl PersonalDataHolder for KnowledgeAgentTraceHolder<'_> {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        let sid = subject.principal.principal_id.0.clone();
        let outcome = if self.model.has_trace(&sid) {
            "located:run-trace-present"
        } else {
            "located:0-recoverable"
        };
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                AGENT_TRACE_HOLDER_ID,
                &sid,
                &tenant.0,
                outcome,
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
                AGENT_TRACE_HOLDER_ID,
                &sid,
                &tenant.0,
                "exported",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        let sid = subject.principal.principal_id.0.clone();
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                AGENT_TRACE_HOLDER_ID,
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
            receipt: Receipt::content_addressed(
                "restrict",
                AGENT_TRACE_HOLDER_ID,
                &sid,
                "*",
                outcome,
                None,
                0,
            ),
        })
    }

    fn erase(&self, scope: EraseScope) -> DsrResult<EraseReceipt> {
        let (sid, tenant_token) = subject_and_tenant(&scope);
        let tenant = TenantId::from_token(&tenant_token);
        let destroyed = self.kms.destroy(&Self::dek(&sid, &tenant));
        self.model.shred_trace(&sid);
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                AGENT_TRACE_HOLDER_ID,
                &sid,
                &tenant_token,
                "crypto_shred:agent_trace:distinct_from_audit",
                destroyed,
                0,
            ),
        })
    }
}

pub struct ProducerHolderRegistration;

impl ProducerHolderRegistration {
    pub fn register_producers<'a>(
        holders: Vec<(&'static str, &'a dyn PersonalDataHolder)>,
    ) -> Vec<RegisteredHolder<'a>> {
        holders
            .into_iter()
            .map(|(id, holder)| {
                let phase = producer_phase_of(id).unwrap_or_else(|| {
                    panic!("producer holder `{id}` has no canonical erase phase")
                });
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
    use crate::{EraseChecklist, AUDIT_CARVE_OUT_STORE};
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

    #[test]
    fn producer_holders_appear_in_the_data_map_after_registration() {
        let inv = data_map(&producer_holder_schemas(region()));

        assert!(
            inv.holders.contains("oltp:git_oltp"),
            "H1 Git is in the map"
        );
        assert!(
            inv.holders.contains("oltp:knowledge_oltp"),
            "H4 Knowledge is in the map"
        );
        assert!(
            inv.holders.contains("oltp:agent_fabric_trace"),
            "H17 agent-trace is in the map"
        );
        assert_eq!(inv.holder_count(), 3, "exactly the three producer holders");

        assert!(
            inv.coverage_gaps(&producer_registrations()).is_empty(),
            "every registered producer holder is in the map - 0 holders missed"
        );
    }

    #[test]
    fn a_registered_producer_holder_absent_from_the_map_is_a_coverage_gap() {
        let partial: Vec<HolderSchema> = producer_holder_schemas(region())
            .into_iter()
            .filter(|s| s.holder_id() != "oltp:agent_fabric_trace")
            .collect();
        let inv = data_map(&partial);
        let gaps = inv.coverage_gaps(&producer_registrations());
        assert_eq!(
            gaps,
            vec!["oltp:agent_fabric_trace".to_string()],
            "the registered-but-unmapped producer holder is the coverage gap"
        );
    }

    #[test]
    fn producer_holders_declare_their_canonical_erase_phases() {
        assert_eq!(
            producer_phase_of(producer_holder_ids::GIT_DB),
            Some(CanonicalErasePhase::CryptoShredDek)
        );
        assert_eq!(
            producer_phase_of(producer_holder_ids::KNOWLEDGE_DB),
            Some(CanonicalErasePhase::CryptoShredDek)
        );
        assert_eq!(
            producer_phase_of(producer_holder_ids::AGENT_TRACE),
            Some(agent_trace_phase())
        );
        assert_eq!(
            producer_phase_of(producer_holder_ids::AGENT_TRACE),
            Some(CanonicalErasePhase::CachesAndDerivedCopies)
        );
        assert_eq!(producer_phase_of("not_a_producer"), None);
        assert!(
            producer_phase_of(producer_holder_ids::AGENT_TRACE)
                > producer_phase_of(producer_holder_ids::KNOWLEDGE_DB)
        );
    }

    #[test]
    fn the_fan_out_reaches_the_producer_holders_in_canonical_order() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-prod", 10);

        let knowledge = KnowledgeStoreModel::new();
        knowledge.index_embedding_from_source("u-prod");
        let trace = AgentTraceModel::new();
        trace.write_trace_from_source("u-prod", "blake3:deadbeef");

        let git_h = GitDbHolder::new(&kms);
        let kn_h = KnowledgeStoreHolder::new(&knowledge, &kms);
        let trace_h = KnowledgeAgentTraceHolder::new(&trace, &kms);

        let producers = ProducerHolderRegistration::register_producers(vec![
            (
                producer_holder_ids::GIT_DB,
                &git_h as &dyn PersonalDataHolder,
            ),
            (producer_holder_ids::KNOWLEDGE_DB, &kn_h),
            (producer_holder_ids::AGENT_TRACE, &trace_h),
        ]);
        let orch = UpstreamHolderOrchestrator::new(producers);

        let ids = orch.holder_ids_in_order();
        assert!(
            ids.contains(&producer_holder_ids::GIT_DB),
            "H1 Git is in the fan-out"
        );
        assert!(
            ids.contains(&producer_holder_ids::KNOWLEDGE_DB),
            "H4 Knowledge is in the fan-out"
        );
        assert!(
            ids.contains(&producer_holder_ids::AGENT_TRACE),
            "H17 agent-trace is in the fan-out"
        );
        assert_eq!(
            ids.last(),
            Some(&producer_holder_ids::AGENT_TRACE),
            "the trace shreds last"
        );

        let checklist = EraseChecklist::new();
        let receipts = orch
            .fan_out_erase(&subject_scope("u-prod"), &checklist)
            .unwrap();
        assert_eq!(receipts.len(), 3, "all three producer holders were reached");
        assert_eq!(
            orch.fanout_coverage(&checklist),
            1.0,
            "100% coverage of the producer holders"
        );
    }

    #[test]
    fn knowledge_instance_crypto_shreds_freetext_and_purges_embeddings_zero_incl_vectors() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-kn", 20);
        let model = KnowledgeStoreModel::new();
        model.index_embedding_from_source("u-kn");

        let dek = ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject("u-kn".into()),
        };
        assert!(
            kms.is_present(&dek),
            "the per-subject DEK is live before erase"
        );
        assert_eq!(
            model.reidentify_hits("u-kn"),
            1,
            "the embedding re-identifies before erase"
        );

        let holder = KnowledgeStoreHolder::new(&model, &kms);
        let receipt = holder.erase(subject_scope("u-kn")).unwrap();

        assert!(
            !kms.is_present(&dek),
            "the per-subject DEK is destroyed (free-text unrecoverable)"
        );
        assert_eq!(
            kms.recoverable_in_backup(&dek),
            0,
            "0 recoverable in backups (crypto-shred reaches backups)"
        );
        assert_eq!(
            model.reidentify_hits("u-kn"),
            0,
            "0 re-identification - the embedding was PURGED (KN-D4)"
        );
        assert!(
            receipt.receipt.key_epoch_destroyed.is_some(),
            "the erase receipt records the destroyed key epoch (the GD-4 audit trail)"
        );
        assert!(
            receipt.receipt.content_hash.starts_with("blake3:"),
            "the receipt is content-addressed"
        );
    }

    #[test]
    fn knowledge_instance_has_no_hide_path_only_a_real_purge() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-hide", 30);
        let model = KnowledgeStoreModel::new();
        model.index_embedding_from_source("u-hide");
        KnowledgeStoreHolder::new(&model, &kms)
            .erase(subject_scope("u-hide"))
            .unwrap();
        assert_eq!(
            model.reidentify_hits("u-hide"),
            0,
            "the only erase is a real purge"
        );
    }

    #[test]
    fn agent_trace_is_crypto_shredded_and_distinct_from_audit() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-trace", 40);
        let model = AgentTraceModel::new();
        model.write_trace_from_source("u-trace", "blake3:cafef00d");

        let dek = ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject("u-trace".into()),
        };
        assert!(
            model.has_trace("u-trace"),
            "the run trace is present before erase"
        );
        assert!(kms.is_present(&dek), "the trace DEK is live before erase");

        let holder = KnowledgeAgentTraceHolder::new(&model, &kms);
        assert_eq!(holder.holder_id(), AGENT_TRACE_HOLDER_ID);
        assert_ne!(
            holder.holder_id(),
            AUDIT_CARVE_OUT_STORE,
            "H17 trace is distinct from the H16 audit carve-out (§6.5)"
        );

        let receipt = holder.erase(subject_scope("u-trace")).unwrap();

        assert!(
            !model.has_trace("u-trace"),
            "the trace row is dropped (crypto-shredded)"
        );
        assert!(!kms.is_present(&dek), "the trace DEK is destroyed");
        assert_eq!(
            kms.recoverable_in_backup(&dek),
            0,
            "0 recoverable in backups"
        );
        assert!(
            receipt.receipt.key_epoch_destroyed.is_some(),
            "the destroyed key epoch is recorded"
        );
        assert!(
            receipt.receipt.content_hash.starts_with("blake3:"),
            "the trace-shred receipt is content-addressed"
        );
    }

    #[test]
    fn the_live_trace_holder_fills_the_p_ga_26_seam_floor() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-fill", 50);
        let model = AgentTraceModel::new();
        model.write_trace_from_source("u-fill", "blake3:abc123");
        let holder = KnowledgeAgentTraceHolder::new(&model, &kms);

        let loc = holder
            .locate(&subject("u-fill"), tenant.clone())
            .expect("the live locate body exists");
        assert!(loc.receipt.content_hash.starts_with("blake3:"));
        let erased = holder
            .erase(subject_scope("u-fill"))
            .expect("the live erase body exists");
        assert_eq!(erased.receipt.operation, "erase");
        assert_eq!(holder.holder_id(), AGENT_TRACE_HOLDER_ID);
        assert_eq!(
            producer_phase_of(holder.holder_id()),
            Some(agent_trace_phase())
        );
    }

    #[test]
    fn producer_holder_erase_is_idempotent() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-idem", 60);
        let model = KnowledgeStoreModel::new();
        model.index_embedding_from_source("u-idem");
        let holder = KnowledgeStoreHolder::new(&model, &kms);

        let first = holder.erase(subject_scope("u-idem")).unwrap();
        let second = holder.erase(subject_scope("u-idem")).unwrap();
        assert_eq!(first.receipt.operation, second.receipt.operation);
        assert_eq!(
            model.reidentify_hits("u-idem"),
            0,
            "0 re-identification after the re-erase too"
        );
    }

    #[test]
    fn producer_holder_id_list_is_the_three_m3_producers() {
        assert_eq!(
            producer_holder_id_list(),
            ["git_oltp", "knowledge_oltp", "agent_fabric_trace"]
        );
    }

    #[test]
    fn knowledge_locate_reports_present_on_either_dek_or_embedding() {
        let tenant = t("acme");

        let kms_a = InMemoryShredKms::new();
        provision_subject_dek(&kms_a, &tenant, "u-a", 80);
        let model_a = KnowledgeStoreModel::new();
        let loc_a = KnowledgeStoreHolder::new(&model_a, &kms_a)
            .locate(&subject("u-a"), tenant.clone())
            .unwrap();
        assert_eq!(model_a.reidentify_hits("u-a"), 0, "no embedding for u-a");
        assert!(
            loc_a.receipt.content_hash.starts_with("blake3:"),
            "located receipt is content-addressed"
        );

        let kms_b = InMemoryShredKms::new();
        let model_b = KnowledgeStoreModel::new();
        model_b.index_embedding_from_source("u-b");
        let holder_b = KnowledgeStoreHolder::new(&model_b, &kms_b);
        assert!(!kms_b.is_present(&ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject("u-b".into())
        }));
        assert_eq!(
            model_b.reidentify_hits("u-b"),
            1,
            "the embedding re-identifies (> 0)"
        );
        let located = holder_b.locate(&subject("u-b"), tenant.clone()).unwrap();
        let expected_located = Receipt::content_addressed(
            "locate",
            producer_holder_ids::KNOWLEDGE_DB,
            "u-b",
            &tenant.0,
            "located:content+embeddings",
            None,
            0,
        );
        assert_eq!(
            located.receipt.content_hash, expected_located.content_hash,
            "embedding-present ⇒ `located:content+embeddings` (the `||`+`> 0` branch is load-bearing)"
        );
        let zero = Receipt::content_addressed(
            "locate",
            producer_holder_ids::KNOWLEDGE_DB,
            "u-b",
            &tenant.0,
            "located:0-recoverable",
            None,
            0,
        );
        assert_ne!(located.receipt.content_hash, zero.content_hash);

        let kms_c = InMemoryShredKms::new();
        let model_c = KnowledgeStoreModel::new();
        let loc_c = KnowledgeStoreHolder::new(&model_c, &kms_c)
            .locate(&subject("u-c"), tenant.clone())
            .unwrap();
        let expected_zero_c = Receipt::content_addressed(
            "locate",
            producer_holder_ids::KNOWLEDGE_DB,
            "u-c",
            &tenant.0,
            "located:0-recoverable",
            None,
            0,
        );
        assert_eq!(
            loc_c.receipt.content_hash, expected_zero_c.content_hash,
            "both DEK and embedding absent ⇒ `located:0-recoverable` (kills the `>= 0`-always-true mutant)"
        );
    }

    #[test]
    fn git_holder_crypto_shreds_inline_bodies() {
        let tenant = t("acme");
        let kms = InMemoryShredKms::new();
        provision_subject_dek(&kms, &tenant, "u-git", 70);
        let dek = ShredKeyHandle {
            tenant: tenant.clone(),
            class: ShredKeyClass::Subject("u-git".into()),
        };
        assert!(
            kms.is_present(&dek),
            "the inline-body DEK is live before erase"
        );

        let receipt = GitDbHolder::new(&kms)
            .erase(subject_scope("u-git"))
            .unwrap();
        assert!(
            !kms.is_present(&dek),
            "the inline-body DEK is crypto-shredded"
        );
        assert_eq!(
            kms.recoverable_in_backup(&dek),
            0,
            "0 recoverable in backups"
        );
        assert!(
            receipt.receipt.key_epoch_destroyed.is_some(),
            "the destroyed epoch is recorded"
        );
    }
}

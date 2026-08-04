use myelin_gdpr::{
    EraseReceipt, EraseScope, LocateReport, Patch, PersonalDataHolder, PortableBundle, Receipt,
    RectifyReceipt, RestrictReceipt, Result as DsrResult, SubjectRef, TenantId,
};
use myelin_substrate::{Holder, HolderRegistration, HolderRegistry, StoreClassifier, StoreKind};

pub const AGENT_OLTP_STORE: &str = "agent_fabric_oltp";

pub const AGENT_TRACE_STORE: &str = "agent_fabric_trace";

pub type AgentHolderRegistration = HolderRegistration;

pub fn agent_store_classifier() -> StoreClassifier {
    StoreClassifier::of([
        myelin_substrate::StoreHolder::new(
            StoreKind::Oltp,
            AGENT_OLTP_STORE,
            Holder::H11AgentMemory,
        ),
        myelin_substrate::StoreHolder::new(
            StoreKind::Oltp,
            AGENT_TRACE_STORE,
            Holder::H17AgentTrace,
        ),
    ])
}

pub fn register_agent_holders() -> HolderRegistry {
    let mut registry = HolderRegistry::new();
    registry.open(StoreKind::Oltp, AGENT_OLTP_STORE);
    registry.open(StoreKind::Oltp, AGENT_TRACE_STORE);
    registry
}

fn floor_note(store: &str) -> String {
    format!(
        "Agent-Fabric `{store}` is the AG-P3 REGISTRATION SEAM (the holder is registered + tagged so \
         the DSAR fan-out reaches it). The real bodies - locate over the run/trace rows naming the \
         subject; erase = crypto-shred the per-subject DEK (the CryptoShred(subject_dek) tags) + \
         pseudonym-shred the agent_principal/on_behalf_of attribution edges + tombstone (drill \
         AG-D10) - land in AG-P23 (→ P-479); the trace holder body (content-addressed write into \
         Knowledge + erasure) in AG-P19 (→ P-268, KN-D11/KN-D12)."
    )
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AgentOltpHolder;

impl AgentOltpHolder {
    pub fn register(&self, registry: &mut HolderRegistry) -> AgentHolderRegistration {
        registry.open(StoreKind::Oltp, AGENT_OLTP_STORE)
    }

    fn subject_id(subject: &SubjectRef) -> String {
        subject.principal.principal_id.0.clone()
    }
}

impl PersonalDataHolder for AgentOltpHolder {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                AGENT_OLTP_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "registration-seam (AG-P3: holder registered + tagged; locate body AG-P23 → P-479)",
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                AGENT_OLTP_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "registration-seam (AG-P3: holder registered + tagged; export body AG-P23 → P-479)",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                AGENT_OLTP_STORE,
                &Self::subject_id(subject),
                "",
                "no-op (AG-P3 registration seam; rectify body AG-P23 → P-479)",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                AGENT_OLTP_STORE,
                &Self::subject_id(subject),
                "",
                &format!(
                    "no-op on={on} (AG-P3 registration seam; suppression body AG-P23 → P-479)"
                ),
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
        let _ = floor_note(AGENT_OLTP_STORE);
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                AGENT_OLTP_STORE,
                &subject_id,
                &tenant,
                "no-op (AG-P3 registration seam; structural crypto-shred + pseudonym shred AG-P23 → P-479)",
                None,
                0,
            ),
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AgentTraceHolder;

impl AgentTraceHolder {
    pub fn register(&self, registry: &mut HolderRegistry) -> AgentHolderRegistration {
        registry.open(StoreKind::Oltp, AGENT_TRACE_STORE)
    }

    fn subject_id(subject: &SubjectRef) -> String {
        subject.principal.principal_id.0.clone()
    }
}

impl PersonalDataHolder for AgentTraceHolder {
    fn locate(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<LocateReport> {
        Ok(LocateReport {
            receipt: Receipt::content_addressed(
                "locate",
                AGENT_TRACE_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "registration-seam (AG-P3: trace holder registered + tagged; body AG-P19 → P-268 / fan-out AG-P23)",
                None,
                0,
            ),
        })
    }

    fn export(&self, subject: &SubjectRef, tenant: TenantId) -> DsrResult<PortableBundle> {
        Ok(PortableBundle {
            receipt: Receipt::content_addressed(
                "export",
                AGENT_TRACE_STORE,
                &Self::subject_id(subject),
                &tenant.0,
                "registration-seam (AG-P3: trace holder registered + tagged; export body AG-P23 → P-479)",
                None,
                0,
            ),
        })
    }

    fn rectify(&self, subject: &SubjectRef, _patch: Patch) -> DsrResult<RectifyReceipt> {
        Ok(RectifyReceipt {
            receipt: Receipt::content_addressed(
                "rectify",
                AGENT_TRACE_STORE,
                &Self::subject_id(subject),
                "",
                "no-op (AG-P3 registration seam; trace is content-addressed - rectify-by-rewrite AG-P19 → P-268)",
                None,
                0,
            ),
        })
    }

    fn restrict(&self, subject: &SubjectRef, on: bool) -> DsrResult<RestrictReceipt> {
        Ok(RestrictReceipt {
            receipt: Receipt::content_addressed(
                "restrict",
                AGENT_TRACE_STORE,
                &Self::subject_id(subject),
                "",
                &format!(
                    "no-op on={on} (AG-P3 registration seam; suppression body AG-P23 → P-479)"
                ),
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
        let _ = floor_note(AGENT_TRACE_STORE);
        Ok(EraseReceipt {
            receipt: Receipt::content_addressed(
                "erase",
                AGENT_TRACE_STORE,
                &subject_id,
                &tenant,
                "no-op (AG-P3 registration seam; trace crypto-shred AG-D10 → AG-P23 / P-479; write AG-P19)",
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
    fn agent_registers_both_stores_as_holders() {
        let registry = register_agent_holders();
        assert!(registry.is_registered(StoreKind::Oltp, AGENT_OLTP_STORE));
        assert!(registry.is_registered(StoreKind::Oltp, AGENT_TRACE_STORE));
        assert_eq!(
            registry.len(),
            2,
            "exactly the two Agent-Fabric stores registered"
        );
    }

    #[test]
    fn re_registration_is_idempotent() {
        let mut registry = register_agent_holders();
        AgentOltpHolder.register(&mut registry);
        AgentTraceHolder.register(&mut registry);
        assert_eq!(
            registry.len(),
            2,
            "re-opening the same Fabric stores does not double-register"
        );
    }

    #[test]
    fn agent_stores_classify_to_h11_and_h17_no_orphan() {
        let registry = register_agent_holders();
        let classifier = agent_store_classifier();
        assert_eq!(
            classify_store(StoreKind::Oltp, AGENT_OLTP_STORE, &classifier),
            Some(Holder::H11AgentMemory),
            "the Fabric OLTP schema is holder H11 (agent operational state)"
        );
        assert_eq!(
            classify_store(StoreKind::Oltp, AGENT_TRACE_STORE, &classifier),
            Some(Holder::H17AgentTrace),
            "the execution-trace store is holder H17 (distinct from the audit log)"
        );
        assert_eq!(
            assert_holder_completeness(registry.registrations(), &classifier),
            Ok(()),
            "every Fabric store is in the exhaustive H1–H18 list - 0 orphan stores"
        );
    }

    #[test]
    fn unregistered_fabric_store_fails_the_harness_check() {
        let manifest = StoreManifest::of([
            DeclaredStore::new(StoreKind::Oltp, AGENT_OLTP_STORE),
            DeclaredStore::new(StoreKind::Oltp, AGENT_TRACE_STORE),
        ]);
        let good = register_agent_holders();
        assert_eq!(
            assert_all_holders_registered(&manifest, &good),
            Ok(()),
            "both Fabric stores opened through the harness → the architecture test passes"
        );
        let mut rogue = HolderRegistry::new();
        rogue.open(StoreKind::Oltp, AGENT_OLTP_STORE);
        let err = assert_all_holders_registered(&manifest, &rogue).expect_err(
            "a Fabric store opened outside the harness must FAIL the architecture test",
        );
        assert_eq!(
            err.len(),
            1,
            "exactly the unregistered trace store is the violation"
        );
        assert!(
            err[0].message().contains(AGENT_TRACE_STORE),
            "the failure names the escaped Fabric store: {}",
            err[0].message()
        );
    }

    #[test]
    fn holder_bodies_are_empty_but_correct_and_name_their_floor() {
        for holder in [
            &AgentOltpHolder as &dyn PersonalDataHolder,
            &AgentTraceHolder as &dyn PersonalDataHolder,
        ] {
            let subj = subject("psn:agent-7");
            let locate = holder
                .locate(&subj, tenant())
                .expect("locate over the seam succeeds");
            assert_eq!(locate.receipt.operation, "locate");
            assert!(locate.receipt.content_hash.starts_with("blake3:"));
            assert!(
                locate.receipt.key_epoch_destroyed.is_none(),
                "locate shreds no key"
            );

            let export = holder
                .export(&subj, tenant())
                .expect("export over the seam succeeds");
            assert_eq!(export.receipt.operation, "export");
            assert!(export.receipt.content_hash.starts_with("blake3:"));
        }
    }

    #[test]
    fn erase_is_a_no_op_receipt_idempotent_and_names_ag_p23() {
        for holder in [
            &AgentOltpHolder as &dyn PersonalDataHolder,
            &AgentTraceHolder as &dyn PersonalDataHolder,
        ] {
            let scope = EraseScope::Subject {
                subject: subject("psn:agent-7"),
                tenant: tenant(),
            };
            let r1 = holder
                .erase(scope.clone())
                .expect("seam erase succeeds (no-op)");
            let r2 = holder.erase(scope).expect("seam erase is idempotent");
            assert_eq!(
                r1, r2,
                "the same erase scope yields the identical content-addressed receipt"
            );
            assert!(
                r1.receipt.key_epoch_destroyed.is_none(),
                "no DEK shredded (body is AG-P23)"
            );
            assert_eq!(r1.receipt.operation, "erase");
            assert!(r1.receipt.content_hash.starts_with("blake3:"));
        }
    }

    #[test]
    fn floor_note_names_the_follow_on_prompts() {
        let note = floor_note(AGENT_OLTP_STORE);
        assert!(
            note.contains("AG-P23"),
            "names the DSR fan-out follow-on: {note}"
        );
        assert!(
            note.contains("AG-P19"),
            "names the trace holder-body follow-on: {note}"
        );
        assert!(note.contains("AG-D10"), "names the erasure drill: {note}");
    }

    #[test]
    fn holders_are_object_safe() {
        let holders: Vec<Box<dyn PersonalDataHolder>> =
            vec![Box::new(AgentOltpHolder), Box::new(AgentTraceHolder)];
        let subj = subject("psn:agent-9");
        for h in &holders {
            assert!(
                h.locate(&subj, tenant()).is_ok(),
                "each holder responds to the contract"
            );
        }
    }
}

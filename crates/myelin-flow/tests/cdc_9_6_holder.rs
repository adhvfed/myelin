use myelin_flow::schema::WfHistoryRow;
use myelin_flow::{
    flow_history_holder, register_flow_holder, WfHistoryHolder, WfJournal, FLOW_OLTP_STORE,
};
use myelin_gdpr::{EraseScope, PersonalDataHolder, SubjectRef, TenantId as GdprTenantId};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_refs::ArtifactRef;
use myelin_substrate::{assert_holder_completeness, Holder, StoreKind};
use myelin_tenancy::{Region, TenantId};

fn tenant_gdpr() -> GdprTenantId {
    GdprTenantId::from_token("acme")
}

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        GdprTenantId::from_token("acme"),
    ))
}

fn row(run_id: &str, seq: i64, actor: &str) -> WfHistoryRow {
    WfHistoryRow {
        tenant: tenant(),
        region: Region::new("fr-par"),
        run_id: run_id.into(),
        seq,
        kind: "activity_completed".into(),
        command_id: format!("agent.run:{seq}"),
        result: Some(vec![ArtifactRef(format!(
            "myelin://acme/identity/principal/{actor}"
        ))]),
        result_key_ref: None,
    }
}

#[test]
fn provider_flow_store_registers_and_classifies_h8() {
    let registry = register_flow_holder();
    assert!(registry.is_registered(StoreKind::Oltp, FLOW_OLTP_STORE));
    assert_eq!(flow_history_holder(), Some(Holder::H8EventBus));
    assert_eq!(
        assert_holder_completeness(
            registry.registrations(),
            &myelin_flow::flow_store_classifier()
        ),
        Ok(()),
        "the flow store is in the exhaustive H1–H18 list - 0 orphan"
    );
}

#[test]
fn consumer_dsr_orchestrator_locates_and_exports_via_the_trait() {
    let journal = WfJournal::new();
    journal.append_history_for_test(row("run-1", 0, "u-subject"));
    journal.append_history_for_test(row("run-2", 0, "u-subject"));
    journal.append_history_for_test(row("run-3", 0, "u-other"));
    let holder = WfHistoryHolder::with_journal(journal);

    let holders: Vec<Box<dyn PersonalDataHolder>> = vec![Box::new(holder)];
    let subj = subject("u-subject");
    for h in &holders {
        let loc = h.locate(&subj, tenant_gdpr()).expect("locate succeeds");
        assert_eq!(loc.receipt.operation, "locate");
        assert!(loc.receipt.content_hash.starts_with("blake3:"));
        assert!(
            loc.receipt.key_epoch_destroyed.is_none(),
            "locate shreds no key"
        );
        let exp = h.export(&subj, tenant_gdpr()).expect("export succeeds");
        assert_eq!(exp.receipt.operation, "export");
        assert!(exp.receipt.content_hash.starts_with("blake3:"));
    }

    let empty: Box<dyn PersonalDataHolder> = Box::new(WfHistoryHolder::default());
    assert!(
        empty.locate(&subj, tenant_gdpr()).is_ok(),
        "unbacked locate is empty-but-correct"
    );
    assert!(
        empty.export(&subj, tenant_gdpr()).is_ok(),
        "unbacked export is empty-but-correct"
    );
}

#[test]
fn consumer_dsr_orchestrator_erases_structurally() {
    let journal = WfJournal::new();
    journal.append_history_for_test(row("run-1", 0, "u-erase"));
    journal.append_history_for_test(row("run-2", 0, "u-keep"));
    let holder = WfHistoryHolder::with_journal(journal.clone());

    let before = journal.history_in_tenant(&tenant());
    let scope = EraseScope::Subject {
        subject: subject("u-erase"),
        tenant: tenant_gdpr(),
    };
    let er = holder
        .erase(scope.clone())
        .expect("structural erase succeeds");
    assert!(
        er.receipt.key_epoch_destroyed.is_none(),
        "0 keys shredded at the flow surface (refs-stored; inline-PII DEK shred is P-FLOW-23)"
    );

    let after = journal.history_in_tenant(&tenant());
    assert_eq!(
        after, before,
        "references-not-payloads: 0 PII columns mutated on erase"
    );

    let er2 = holder.erase(scope).expect("re-erase is idempotent");
    assert_eq!(
        er, er2,
        "the same erase scope yields the identical content-addressed receipt"
    );
}

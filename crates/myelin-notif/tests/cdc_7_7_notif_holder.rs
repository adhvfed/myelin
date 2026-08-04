use myelin_gdpr::{EraseScope, LocateReport, PersonalDataHolder, SubjectRef, TenantId};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_notif::{
    notif_store_classifier, register_notif_holder, Class, InboxProjection, NotifHistoryHolder,
    Reason, RoutedInboxItem, NOTIF_OLTP_STORE,
};
use myelin_substrate::{assert_holder_completeness, classify_store, Holder, StoreKind};
use myelin_tenancy::{Region, TenantId as TenancyTenantId};

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

fn row(recipient: &str, subject: &str, actor: &str, dedup_key: &str) -> RoutedInboxItem {
    RoutedInboxItem {
        tenant: TenancyTenantId::from_token("acme"),
        region: Region::new("fr-par"),
        item_id: format!("itm-{dedup_key}"),
        recipient: recipient.into(),
        subject: myelin_refs::ArtifactRef(format!("myelin://acme/issues/issue/{subject}")),
        reason: Reason::Mentioned,
        class: Class::Direct,
        origin_event: myelin_refs::ArtifactRef(format!("myelin://acme/identity/principal/{actor}")),
        dedup_key: dedup_key.into(),
        coalesce_count: 1,
        state: "unread".into(),
        snooze_until: None,
    }
}

struct DsrOrchestratorConsumer<'a> {
    holders: Vec<&'a dyn PersonalDataHolder>,
}

impl<'a> DsrOrchestratorConsumer<'a> {
    fn new(holders: Vec<&'a dyn PersonalDataHolder>) -> Self {
        DsrOrchestratorConsumer { holders }
    }

    fn fan_out_locate(&self, subject: &SubjectRef, tenant: TenantId) -> Vec<LocateReport> {
        self.holders
            .iter()
            .map(|h| {
                h.locate(subject, tenant.clone())
                    .expect("a Notif holder locate succeeds")
            })
            .collect()
    }

    fn fan_out_erase(&self, scope: EraseScope) -> usize {
        for h in &self.holders {
            h.erase(scope.clone())
                .expect("a Notif holder erase succeeds");
        }
        self.holders.len()
    }
}

#[test]
fn dsr_orchestrator_fans_locate_and_erase_out_to_the_notif_holder_via_the_contract() {
    let inbox = InboxProjection::new();
    inbox.upsert_for_test(row("u-cdc", "PROJ-1", "u-other", "own"));
    inbox.upsert_for_test(row("u-bob", "PROJ-2", "u-cdc", "byref"));
    let holder = NotifHistoryHolder::with_inbox(inbox.clone());

    let consumer = DsrOrchestratorConsumer::new(vec![&holder]);
    let subj = subject("u-cdc");

    let reports = consumer.fan_out_locate(&subj, tenant());
    assert_eq!(
        reports.len(),
        1,
        "the Notif holder responded to locate via the contract"
    );
    assert_eq!(reports[0].receipt.operation, "locate");
    assert!(
        reports[0].receipt.content_hash.starts_with("blake3:"),
        "content-addressed receipt"
    );
    assert!(
        reports[0].receipt.key_epoch_destroyed.is_none(),
        "locate shreds no key"
    );

    let before = inbox.snapshot_for_tenant(&tenant());

    let erased = consumer.fan_out_erase(EraseScope::Subject {
        subject: subj.clone(),
        tenant: tenant(),
    });
    assert_eq!(erased, 1, "the Notif holder honoured the erase contract");

    let mut a = inbox.snapshot_for_tenant(&tenant());
    let mut b = before;
    a.sort_by(|x, y| x.item_id.cmp(&y.item_id));
    b.sort_by(|x, y| x.item_id.cmp(&y.item_id));
    assert_eq!(
        a, b,
        "the refs-stored items tombstone for free - 0 PII columns mutated (the 7.7 property)"
    );
}

#[test]
fn notif_holder_store_registers_and_classifies_with_zero_orphans() {
    let registry = register_notif_holder();
    let classifier = notif_store_classifier();
    assert_eq!(
        classify_store(StoreKind::Oltp, NOTIF_OLTP_STORE, &classifier),
        Some(Holder::H13NotificationHistory),
        "the Notif OLTP store is holder H13"
    );
    assert_eq!(
        assert_holder_completeness(registry.registrations(), &classifier),
        Ok(()),
        "every Notif store is in the exhaustive H1–H18 list - 0 orphan stores"
    );
}

#[test]
fn notif_holder_export_is_empty_but_correct() {
    let holder = NotifHistoryHolder::default();
    let bundle = holder
        .export(&subject("u-1"), tenant())
        .expect("export of an empty bundle succeeds (no inbox populated yet)");
    assert_eq!(bundle.receipt.operation, "export");
    assert!(bundle.receipt.content_hash.starts_with("blake3:"));
}

#[test]
fn cdc_7_7_erase_restrict_completed_via_the_residual_instanced() {
    use myelin_notif::{
        build_idem_key, erase_residual, redact_for_offcell, EuSovereignAdapter, HumanisedString,
        InMemoryDeliveryShredder, InlineDeliveryShredder, NotifErasureLedger, OffCellResidual,
        RecordingEuTransport, RestrictSet,
    };
    use std::sync::Arc;

    let transport = RecordingEuTransport::new("eu-mailer");
    let provider = EuSovereignAdapter::new(
        myelin_notif::prefs::Channel::Email,
        Region::new("fr-par"),
        Arc::new(transport.clone()),
    );
    let shredder = InMemoryDeliveryShredder::new();
    let restrict = RestrictSet::new();
    let ledger = NotifErasureLedger::new();

    let idem = build_idem_key("itm-1", myelin_notif::prefs::Channel::Email);
    let summary = HumanisedString {
        text: "you were mentioned".into(),
        links: vec!["myelin://acme/issues/issue/PROJ-1".into()],
        icon: "mention".into(),
    };
    provider
        .try_send(&redact_for_offcell(summary, Class::Direct), &idem)
        .expect("off-cell delivery accepted (EU region)");
    let provider_ref = provider.provider_ref_for(&idem).expect("provider_ref");
    let dek = myelin_events::PiiKeyRef("kms://acme/0/subject:u-erase".into());
    shredder.seal(&dek);

    let receipt = erase_residual(
        "u-erase",
        &TenancyTenantId::from_token("acme"),
        &[OffCellResidual {
            idem_key: idem,
            inline_pii_key: Some(dek.clone()),
        }],
        &shredder,
        &restrict,
        &provider,
        &ledger,
        myelin_events::Timestamp("2026-06-25T00:00:00Z".into()),
    )
    .expect("the residual erase succeeds");

    assert!(
        receipt.is_green(),
        "0 recoverable PII + restrict applied (7.7 erase complete)"
    );
    assert!(
        !shredder.is_live(&dek),
        "the inline-PII DEK is crypto-shredded (11.4)"
    );
    assert!(
        transport.was_erased(&provider_ref),
        "the off-cell copy was erasure-requested"
    );
    assert!(
        restrict.is_restricted("u-erase"),
        "restrict suppresses new routing (10.1)"
    );
    assert!(
        ledger.is_erased("u-erase"),
        "the erase receipt is in the ledger (10.8)"
    );
}

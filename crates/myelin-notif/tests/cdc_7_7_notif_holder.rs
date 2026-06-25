//! # CDC 7.7 — the Notif side of `PersonalDataHolder{locate, export, rectify, restrict, erase}`
//! (the HOLDER HALF; NOTIF-P4 → P-182)
//!
//! **Contract:** index row 7.7 (`PersonalDataHolder` + `replay` — references-not-payloads; erasing a
//! person tombstones their appearance; inbox rebuilt by reindex-from-source). The `PersonalDataHolder`
//! SIGNATURE was frozen at P-GA-01 (`myelin-gdpr`); the GDPR-owned holder bodies landed at P-GA-05.
//! THIS file ships the **Notif side** of 7.7 — Notif as holder **H13 (`NotificationHistory`)**, the
//! HOLDER HALF (registration + locate/export/rectify/restrict + the structural references-not-payloads
//! erase). The **replay half** (`replay(scope, since)` — the inbox rebuilt by reindex-from-source) is
//! **NOTIF-P17** (P-195); the **off-cell-payload erasure residual** (X-7 / 10.9) is **NOTIF-P27**
//! (P-469). This CDC pair is what the contract-coverage scanner (P-S21) reads for the Notif holder
//! seam.
//!
//! - **PROVIDER** = the Notif holder ([`NotifHistoryHolder`], H13) IMPLEMENTING the five-operation
//!   10.1 contract over the inbox. Backed by a live [`myelin_notif::InboxProjection`] it runs the REAL
//!   §3.9 structural body (the references-not-payloads erase — 0 PII-column mutation on refs-stored
//!   items); unbacked it is empty-but-correct. It registers its store through the substrate registry
//!   (1.4) and classifies to H13 — 0 orphans.
//! - **CONSUMER** = a minimal DSR-orchestrator stand-in that holds the Notif holder behind
//!   `dyn PersonalDataHolder`, fans `locate` + `erase` out to it via the contract, and NEVER reaches
//!   into the inbox store directly (the no-cross-store-read law, gdpr §3.1). This is the shape the real
//!   orchestrator (P-GA-11/P-GA-12) takes when it fans a DSR out to the Notif holder.
//!
//! The dated green artifact: the consumer fans `locate(subject)` + `erase(subject)` out to the Notif
//! holder; each returns a content-addressed receipt; the structural erase mutates 0 PII columns; the
//! holder classifies to H13 with 0 orphan stores. If 7.7's body shape drifts, this stops
//! compiling/passing — that is the contract.

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

/// A refs-stored inbox row in `acme` (references-not-payloads): the subject appears as `recipient`
/// and/or a referenced actor in `origin_event`, NEVER as a stored name.
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

/// **The CONSUMER side (7.7 / 10.1): a DSR-orchestrator shape that fans out to the Notif holder via
/// the contract.** It holds the holder behind `dyn PersonalDataHolder` and calls the contract — it
/// never reaches into the inbox store. This is the shape the real orchestrator (P-GA-11/P-GA-12)
/// takes; the property pinned here is "the orchestrator touches the Notif store ONLY through the
/// holder contract".
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

/// **provider + consumer wired together (the 7.7 Notif CDC pair).** The orchestrator (consumer) fans
/// `locate` then `erase` out to the H13 Notif holder (provider) over a LIVE inbox projection; the
/// structural references-not-payloads erase tombstones the subject's appearances with 0 PII-column
/// mutation. This is the dated green artifact for the Notif side of 7.7 (holder half).
#[test]
fn dsr_orchestrator_fans_locate_and_erase_out_to_the_notif_holder_via_the_contract() {
    let inbox = InboxProjection::new();
    inbox.upsert_for_test(row("u-cdc", "PROJ-1", "u-other", "own")); // the subject's own inbox
    inbox.upsert_for_test(row("u-bob", "PROJ-2", "u-cdc", "byref")); // the subject named by ref
    let holder = NotifHistoryHolder::with_inbox(inbox.clone());

    let consumer = DsrOrchestratorConsumer::new(vec![&holder]);
    let subj = subject("u-cdc");

    // locate: the holder responds with a content-addressed receipt over the structural surface.
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

    // The exact stored bytes BEFORE erase.
    let before = inbox.snapshot_for_tenant(&tenant());

    // erase: the structural references-not-payloads erase — 0 PII columns mutated.
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
        "the refs-stored items tombstone for free — 0 PII columns mutated (the 7.7 property)"
    );
}

/// **The provider registers + classifies (contract 1.4 + gdpr §3.2): 0 orphan Notif stores.** The
/// Notif OLTP store classifies to **H13 (`NotificationHistory`)** — it is in the exhaustive H1–H18
/// list, so the M5 DSAR fan-out cannot silently miss notification history (the §3.9 bug).
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
        "every Notif store is in the exhaustive H1–H18 list — 0 orphan stores"
    );
}

/// **The holder is empty-but-correct unbacked (the registration-only surface), not an error.**
/// `export` over a tenant the router has not populated returns an empty bundle with a content-
/// addressed receipt — a real, callable holder, not a `todo!()`/`Err`.
#[test]
fn notif_holder_export_is_empty_but_correct() {
    let holder = NotifHistoryHolder::default();
    let bundle = holder
        .export(&subject("u-1"), tenant())
        .expect("export of an empty bundle succeeds (no inbox populated yet)");
    assert_eq!(bundle.receipt.operation, "export");
    assert!(bundle.receipt.content_hash.starts_with("blake3:"));
}

/// **CDC 7.7 erase/restrict COMPLETED — the residual instanced (X-7 / 10.9 by reference, NOTIF-P27).**
/// The holder-half structural erase tombstones the inbox appearance for free; the residual completes
/// 7.7 erase/restrict by reaching the ONE inline-PII case (an already-sent off-cell redacted summary):
/// the per-subject DEK is crypto-shredded (11.4) → 0 recoverable, a provider-side erasure-request is
/// issued (the named sub-processor obligation), and restrict suppresses NEW routing (10.1). Notif
/// restates NO platform posture — it instances the 10.9 residual by reference. This CDC pins that the
/// erase/restrict half is now COMPLETE (the residual reached), not merely the structural tombstone.
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

    // An off-cell redacted summary was sent for the subject (the one inline-PII case).
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

    // The residual erase COMPLETES 7.7 erase/restrict (the X-7 / 10.9 instancing).
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

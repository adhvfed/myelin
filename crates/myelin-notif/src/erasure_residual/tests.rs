use super::*;
use crate::eu_provider::{EuSovereignAdapter, RecordingEuTransport};
use crate::holder::RestrictSet;
use crate::prefs::Channel;
use crate::{Class, HumanisedString, RedactedMessage};
use myelin_tenancy::Region;
use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

fn ts() -> Timestamp {
    Timestamp("2026-06-25T00:00:00Z".into())
}

fn key(s: &str) -> PiiKeyRef {
    PiiKeyRef(format!("kms://acme/epoch-1/subject:{s}"))
}

fn summary() -> HumanisedString {
    HumanisedString {
        text: "you were mentioned on PROJ-1".into(),
        links: vec!["myelin://acme/issues/issue/PROJ-1".into()],
        icon: "mention".into(),
    }
}

fn redacted_msg() -> RedactedMessage {
    crate::redact_for_offcell(summary(), Class::Direct)
}

fn eu_adapter() -> (EuSovereignAdapter, RecordingEuTransport) {
    let transport = RecordingEuTransport::new("eu-mailer");
    let adapter = EuSovereignAdapter::new(
        Channel::Email,
        Region("fr-par".into()),
        Arc::new(transport.clone()),
    );
    (adapter, transport)
}

#[test]
fn crypto_shred_destroys_the_dek_idempotently() {
    let shredder = InMemoryDeliveryShredder::new();
    let k = key("u-erase");
    shredder.seal(&k);
    assert!(shredder.is_live(&k), "a sealed key is live");

    shredder.destroy_key(&k).expect("destroy succeeds");
    assert!(
        !shredder.is_live(&k),
        "the shredded key is dead (unrecoverable)"
    );

    shredder
        .destroy_key(&k)
        .expect("re-destroy of a dead key is a no-op success");
    assert!(!shredder.is_live(&k), "still dead");

    let other = key("u-bob");
    shredder.seal(&other);
    shredder.destroy_key(&k).expect("destroy");
    assert!(
        shredder.is_live(&other),
        "an unrelated subject's key stays live"
    );
}

#[test]
fn crypto_shred_is_loud_on_kms_failure() {
    let shredder = InMemoryDeliveryShredder::new();
    let k = key("u-erase");
    shredder.seal(&k);
    shredder.make_unreachable(&k);

    let err = shredder
        .destroy_key(&k)
        .expect_err("an unreachable KMS fails loudly");
    assert_eq!(err, DeliveryShredError::KmsUnavailable(k.clone()));
    assert!(err.to_string().contains("erase INCOMPLETE"));
    assert!(
        shredder.is_live(&k),
        "the key is STILL live after a failed destroy - never silently assumed erased"
    );
}

#[test]
fn erasure_ledger_records_and_merges_idempotently() {
    let ledger = NotifErasureLedger::new();
    assert!(ledger.is_empty());
    assert!(
        !ledger.is_erased("u-erase"),
        "never-seen subject is not erased"
    );

    ledger.record("u-erase", &[key("a")], &["eu-mailer:itm-1".into()], ts());
    assert!(
        ledger.is_erased("u-erase"),
        "the fact-of-erasure is recorded"
    );
    assert_eq!(ledger.len(), 1);
    let e = ledger.entry("u-erase").expect("entry present");
    assert_eq!(e.shredded_keys, vec![key("a")]);
    assert_eq!(
        e.provider_erasures_requested,
        vec!["eu-mailer:itm-1".to_string()]
    );

    ledger.record(
        "u-erase",
        &[key("a"), key("b")],
        &["eu-mailer:itm-1".into(), "eu-mailer:itm-2".into()],
        Timestamp("2026-07-01T00:00:00Z".into()),
    );
    assert_eq!(ledger.len(), 1, "still one subject entry");
    let e = ledger.entry("u-erase").expect("entry present");
    assert_eq!(
        e.shredded_keys,
        vec![key("a"), key("b")],
        "new key merged, no dup"
    );
    assert_eq!(
        e.provider_erasures_requested,
        vec!["eu-mailer:itm-1".to_string(), "eu-mailer:itm-2".to_string()],
        "new provider_ref merged, no dup"
    );
    assert_eq!(
        e.erased_at,
        ts(),
        "the EARLIEST erase timestamp is kept (the audit truth)"
    );
}

#[test]
fn chained_deliver_offcell_then_erase_is_unrecoverable_and_purged_and_ledgered() {
    let (provider, transport) = eu_adapter();
    let shredder = InMemoryDeliveryShredder::new();
    let restrict = RestrictSet::new();
    let ledger = NotifErasureLedger::new();

    let idem = crate::build_idem_key("itm-1", Channel::Email);
    let receipt = provider
        .try_send(&redacted_msg(), &idem)
        .expect("off-cell delivery accepted (EU region)");
    assert!(receipt.accepted);
    let provider_ref = provider
        .provider_ref_for(&idem)
        .expect("the off-cell copy has a durable provider_ref");

    let dek = key("u-erase");
    shredder.seal(&dek);
    assert!(
        shredder.is_live(&dek),
        "the inline-PII column is recoverable BEFORE erase"
    );

    let residuals = vec![OffCellResidual {
        idem_key: idem.clone(),
        inline_pii_key: Some(dek.clone()),
    }];

    let er = erase_residual(
        "u-erase",
        &tenant(),
        &residuals,
        &shredder,
        &restrict,
        &provider,
        &ledger,
        ts(),
    )
    .expect("the structural erase succeeds");

    assert_eq!(
        er.recoverable_remaining, 0,
        "0 inline-PII columns recoverable"
    );
    assert!(
        er.is_green(),
        "NOTIF-D6 green: 0 recoverable PII + restrict applied"
    );

    assert!(
        !shredder.is_live(&dek),
        "the inline-PII delivery DEK is destroyed"
    );
    assert_eq!(er.shredded_keys, vec![dek], "the DEK was crypto-shredded");

    assert!(
        transport.was_erased(&provider_ref),
        "the sub-processor copy was requested-erased"
    );
    assert_eq!(
        er.provider_erasures_requested,
        vec![provider_ref.clone()],
        "the provider erasure is recorded on the receipt"
    );

    assert!(ledger.is_erased("u-erase"), "the erase is in the ledger");
    let entry = ledger.entry("u-erase").expect("ledger entry present");
    assert_eq!(entry.provider_erasures_requested, vec![provider_ref]);

    assert!(
        restrict.is_restricted("u-erase"),
        "the subject's new routing is suppressed"
    );
}

#[test]
fn erase_with_no_residual_still_restricts_and_ledgers_zero_recoverable() {
    let (provider, _transport) = eu_adapter();
    let shredder = InMemoryDeliveryShredder::new();
    let restrict = RestrictSet::new();
    let ledger = NotifErasureLedger::new();

    let er = erase_residual(
        "u-incell",
        &tenant(),
        &[],
        &shredder,
        &restrict,
        &provider,
        &ledger,
        ts(),
    )
    .expect("an in-cell-only erase succeeds");

    assert!(
        er.restrict_applied,
        "restrict applied even with nothing to shred"
    );
    assert!(restrict.is_restricted("u-incell"));
    assert_eq!(er.recoverable_remaining, 0, "0 recoverable");
    assert!(er.shredded_keys.is_empty(), "no DEK to shred");
    assert!(
        er.provider_erasures_requested.is_empty(),
        "no off-cell copy to purge"
    );
    assert!(er.is_green(), "NOTIF-D6 green");
    assert!(
        ledger.is_erased("u-incell"),
        "the fact-of-erasure is still ledgered"
    );
}

#[test]
fn offcell_residual_without_inline_pii_purges_but_shreds_nothing() {
    let (provider, transport) = eu_adapter();
    let shredder = InMemoryDeliveryShredder::new();
    let restrict = RestrictSet::new();
    let ledger = NotifErasureLedger::new();

    let idem = crate::build_idem_key("itm-9", Channel::Email);
    provider
        .try_send(&redacted_msg(), &idem)
        .expect("delivered");
    let provider_ref = provider.provider_ref_for(&idem).expect("provider_ref");

    let residuals = vec![OffCellResidual {
        idem_key: idem,
        inline_pii_key: None,
    }];
    let er = erase_residual(
        "u-erase",
        &tenant(),
        &residuals,
        &shredder,
        &restrict,
        &provider,
        &ledger,
        ts(),
    )
    .expect("erase succeeds");

    assert!(er.shredded_keys.is_empty(), "nothing to crypto-shred");
    assert_eq!(er.recoverable_remaining, 0);
    assert!(
        transport.was_erased(&provider_ref),
        "the off-cell copy is still purged (the redacted bytes the sub-processor holds)"
    );
    assert!(er.is_green());
}

#[test]
fn erase_is_loud_and_incomplete_on_kms_failure() {
    let (provider, _transport) = eu_adapter();
    let shredder = InMemoryDeliveryShredder::new();
    let restrict = RestrictSet::new();
    let ledger = NotifErasureLedger::new();

    let idem = crate::build_idem_key("itm-1", Channel::Email);
    provider
        .try_send(&redacted_msg(), &idem)
        .expect("delivered");
    let dek = key("u-erase");
    shredder.seal(&dek);
    shredder.make_unreachable(&dek);

    let residuals = vec![OffCellResidual {
        idem_key: idem,
        inline_pii_key: Some(dek.clone()),
    }];
    let err = erase_residual(
        "u-erase",
        &tenant(),
        &residuals,
        &shredder,
        &restrict,
        &provider,
        &ledger,
        ts(),
    )
    .expect_err("an unreachable KMS makes the erase INCOMPLETE");

    assert!(
        matches!(err, ResidualEraseError::Shred(_)),
        "a loud shred failure"
    );
    assert!(err.to_string().contains("INCOMPLETE"));
    assert!(
        shredder.is_live(&dek),
        "the key is STILL live - never silently assumed erased"
    );
    assert!(restrict.is_restricted("u-erase"));
}

#[test]
fn erase_is_loud_when_the_subprocessor_rejects_the_erasure() {
    #[derive(Clone)]
    struct RejectingTransport;
    impl crate::eu_provider::EuTransport for RejectingTransport {
        fn transport_id(&self) -> &str {
            "eu-rejecter"
        }
        fn submit(
            &self,
            _m: &RedactedMessage,
            idem_key: &str,
            _r: &Region,
        ) -> crate::eu_provider::TransportReceipt {
            crate::eu_provider::TransportReceipt {
                provider_ref: format!("eu-rejecter:{idem_key}"),
                accepted: true,
            }
        }
        fn request_erasure(&self, _provider_ref: &str) -> bool {
            false
        }
    }

    let provider = EuSovereignAdapter::new(
        Channel::Email,
        Region("fr-par".into()),
        Arc::new(RejectingTransport),
    );
    let shredder = InMemoryDeliveryShredder::new();
    let restrict = RestrictSet::new();
    let ledger = NotifErasureLedger::new();

    let idem = crate::build_idem_key("itm-1", Channel::Email);
    provider
        .try_send(&redacted_msg(), &idem)
        .expect("delivered");
    let residuals = vec![OffCellResidual {
        idem_key: idem,
        inline_pii_key: None,
    }];
    let err = erase_residual(
        "u-erase",
        &tenant(),
        &residuals,
        &shredder,
        &restrict,
        &provider,
        &ledger,
        ts(),
    )
    .expect_err("a rejected provider erasure makes the erase INCOMPLETE");
    assert!(
        matches!(err, ResidualEraseError::ProviderErasure(_)),
        "a loud provider-erasure failure"
    );
}

#[test]
fn erase_is_idempotent_on_re_run() {
    let (provider, _transport) = eu_adapter();
    let shredder = InMemoryDeliveryShredder::new();
    let restrict = RestrictSet::new();
    let ledger = NotifErasureLedger::new();

    let idem = crate::build_idem_key("itm-1", Channel::Email);
    provider
        .try_send(&redacted_msg(), &idem)
        .expect("delivered");
    let dek = key("u-erase");
    shredder.seal(&dek);
    let residuals = vec![OffCellResidual {
        idem_key: idem,
        inline_pii_key: Some(dek.clone()),
    }];

    let er1 = erase_residual(
        "u-erase",
        &tenant(),
        &residuals,
        &shredder,
        &restrict,
        &provider,
        &ledger,
        ts(),
    )
    .expect("first erase");
    assert!(er1.is_green());

    let er2 = erase_residual(
        "u-erase",
        &tenant(),
        &residuals,
        &shredder,
        &restrict,
        &provider,
        &ledger,
        ts(),
    )
    .expect("re-erase is idempotent");
    assert_eq!(
        er2.recoverable_remaining, 0,
        "still 0 recoverable on re-erase"
    );
    assert!(er2.is_green());
    assert_eq!(
        ledger.len(),
        1,
        "still one ledger entry (idempotent record)"
    );
}

#[test]
fn ledger_is_empty_reflects_real_state() {
    let ledger = NotifErasureLedger::new();
    assert!(ledger.is_empty(), "a fresh ledger is empty");
    ledger.record("u-x", &[key("a")], &[], ts());
    assert!(!ledger.is_empty(), "after a record the ledger is NOT empty");
}

#[test]
fn is_green_requires_both_zero_recoverable_and_restrict() {
    let base = |recoverable: usize, restrict_applied: bool| ResidualEraseReceipt {
        subject_id: "u-x".into(),
        tenant: tenant(),
        restrict_applied,
        shredded_keys: vec![],
        provider_erasures_requested: vec![],
        recoverable_remaining: recoverable,
    };
    assert!(base(0, true).is_green(), "0 recoverable + restrict → green");
    assert!(
        !base(1, true).is_green(),
        "recoverable > 0 → NOT green (even with restrict applied) - the 0-PII threshold"
    );
    assert!(
        !base(0, false).is_green(),
        "restrict NOT applied → NOT green (even with 0 recoverable) - the suppression is required"
    );
    assert!(!base(1, false).is_green(), "neither → NOT green");
}

#[test]
fn erasure_residual_is_a_named_deliverable() {
    assert_eq!(ERASURE_RESIDUAL_PROMPT, "NOTIF-P27");
}

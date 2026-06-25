//! # Unit tests for the erasure residual instanced — the X-7 posture for Notif (NOTIF-P27 / P-469)
//!
//! Exercises the erase path to the ≥ 80% mutation floor on `erasure_residual.rs`: the per-subject DEK
//! crypto-shred (idempotent destroy + is-live + loud KMS failure), the PII-free non-shred-erasable
//! erasure ledger (record + idempotent merge + is-erased), and the four-leg erase orchestration (the
//! chained test EI-01 §4: deliver an off-cell redacted item → erase the subject → assert the inline-PII
//! column is unrecoverable AND a provider-side erasure was issued AND the receipt is in the ledger).
//! The drill-harness scenario for NOTIF-D6 lives in `tests/drill_notif_d6_erasure.rs`.

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

/// An EU-sovereign adapter over a deterministic recording transport (fr-par = EU).
fn eu_adapter() -> (EuSovereignAdapter, RecordingEuTransport) {
    let transport = RecordingEuTransport::new("eu-mailer");
    let adapter = EuSovereignAdapter::new(
        Channel::Email,
        Region("fr-par".into()),
        Arc::new(transport.clone()),
    );
    (adapter, transport)
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// (1) The per-subject DEK crypto-shred seam (contract 11.4)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **A sealed inline-PII delivery DEK is live until shredded, then dead — idempotently (11.4).** The
/// crypto-shred is the lever: destroy the per-subject key → the sealed delivery column is unrecoverable
/// ciphertext. Idempotent (a re-destroy of a dead key succeeds — re-erasure after a restore).
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

    // Idempotent: a re-destroy of an already-dead key still succeeds (re-erasure after restore).
    shredder
        .destroy_key(&k)
        .expect("re-destroy of a dead key is a no-op success");
    assert!(!shredder.is_live(&k), "still dead");

    // An untouched key is unaffected (the destroy is per-key, not a global wipe).
    let other = key("u-bob");
    shredder.seal(&other);
    shredder.destroy_key(&k).expect("destroy");
    assert!(
        shredder.is_live(&other),
        "an unrelated subject's key stays live"
    );
}

/// **A KMS that cannot be reached fails LOUDLY — the erase is INCOMPLETE, never silently assumed-done
/// (EI-01 §3).** The shredder surfaces `KmsUnavailable`; the key stays live (still recoverable).
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
        "the key is STILL live after a failed destroy — never silently assumed erased"
    );
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// (2) The PII-free, non-shred-erasable erasure ledger (contract 10.8)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **The erasure ledger records the fact-of-erasure (10.8) and merges idempotently.** A record marks
/// the subject erased; a re-record MERGES new key/provider refs into the existing entry (re-erasure
/// after a restore re-applies cleanly, never duplicating). An un-recorded subject is "never seen".
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

    // Re-record MERGES new refs (no duplicates), keeps the entry single (idempotent on subject).
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

// ════════════════════════════════════════════════════════════════════════════════════════════
// (3) The four-leg erase orchestration — the X-7 posture instanced (the chained test, EI-01 §4)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// **THE CHAINED TEST (EI-01 §4): deliver an off-cell redacted item → erase the subject → the
/// inline-PII column is unrecoverable (DEK destroyed) AND a provider-side erasure-request was issued
/// AND the receipt is in the erasure ledger.** This is the X-7 posture instanced for Notif end-to-end.
#[test]
fn chained_deliver_offcell_then_erase_is_unrecoverable_and_purged_and_ledgered() {
    let (provider, transport) = eu_adapter();
    let shredder = InMemoryDeliveryShredder::new();
    let restrict = RestrictSet::new();
    let ledger = NotifErasureLedger::new();

    // DELIVER an off-cell redacted item for the subject (the one place Notif emits free text off-cell).
    let idem = crate::build_idem_key("itm-1", Channel::Email);
    let receipt = provider
        .try_send(&redacted_msg(), &idem)
        .expect("off-cell delivery accepted (EU region)");
    assert!(receipt.accepted);
    let provider_ref = provider
        .provider_ref_for(&idem)
        .expect("the off-cell copy has a durable provider_ref");

    // The inline-PII delivery column is sealed under a per-subject DEK.
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

    // ERASE the subject's notification residual (the four legs).
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

    // THE GATE: 0 recoverable PII (NOTIF-D6 threshold) + the suppression applied.
    assert_eq!(
        er.recoverable_remaining, 0,
        "0 inline-PII columns recoverable"
    );
    assert!(
        er.is_green(),
        "NOTIF-D6 green: 0 recoverable PII + restrict applied"
    );

    // (2) The inline-PII delivery column is UNRECOVERABLE (DEK destroyed).
    assert!(
        !shredder.is_live(&dek),
        "the inline-PII delivery DEK is destroyed"
    );
    assert_eq!(er.shredded_keys, vec![dek], "the DEK was crypto-shredded");

    // (3) A provider-side erasure-request was ISSUED for the already-sent off-cell copy.
    assert!(
        transport.was_erased(&provider_ref),
        "the sub-processor copy was requested-erased"
    );
    assert_eq!(
        er.provider_erasures_requested,
        vec![provider_ref.clone()],
        "the provider erasure is recorded on the receipt"
    );

    // (4) The receipt is in the erasure LEDGER (10.8) — provable + survives a restore.
    assert!(ledger.is_erased("u-erase"), "the erase is in the ledger");
    let entry = ledger.entry("u-erase").expect("ledger entry present");
    assert_eq!(entry.provider_erasures_requested, vec![provider_ref]);

    // (1) restrict was applied (new routing/delivery suppressed).
    assert!(
        restrict.is_restricted("u-erase"),
        "the subject's new routing is suppressed"
    );
}

/// **`restrict` is applied FIRST — the subject is suppressed even if there is nothing to shred.** An
/// erase with NO off-cell residuals (an in-cell-only subject) still records the suppression + a
/// receipt + reports 0 recoverable (the structural references-not-payloads tombstone does the rest).
#[test]
fn erase_with_no_residual_still_restricts_and_ledgers_zero_recoverable() {
    let (provider, _transport) = eu_adapter();
    let shredder = InMemoryDeliveryShredder::new();
    let restrict = RestrictSet::new();
    let ledger = NotifErasureLedger::new();

    let er = erase_residual(
        "u-incell",
        &tenant(),
        &[], // no off-cell payload — an in-cell-only subject
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

/// **A redacted summary with NO inline PII (a fully-tombstoned summary) shreds no key but still
/// purges the off-cell copy.** The `inline_pii_key = None` case: the provider-side erasure still
/// fires (the sub-processor holds a copy of the redacted bytes), and 0 remain recoverable.
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
        inline_pii_key: None, // the summary carried no inline PII (fully tombstoned)
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

/// **A KMS failure makes the erase LOUD + INCOMPLETE — never a silent partial erase (EI-01 §3).** The
/// crypto-shred leg surfaces `Shred(KmsUnavailable)`; the erase returns `Err` (the DSR is not done).
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
    shredder.make_unreachable(&dek); // the KMS is down for this key

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
        "the key is STILL live — never silently assumed erased"
    );
    // restrict WAS applied first (the suppression holds even though the shred failed).
    assert!(restrict.is_restricted("u-erase"));
}

/// **A sub-processor REJECTING the provider-side erasure surfaces LOUDLY — the un-purged copy is the
/// residual, never silently swallowed (EI-01 §3).** A transport that rejects the erasure request makes
/// the erase return `ProviderErasure`.
#[test]
fn erase_is_loud_when_the_subprocessor_rejects_the_erasure() {
    // A transport that ACCEPTS submits but REJECTS erasure requests.
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
            false // the sub-processor REJECTS — the copy is un-purged
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

/// **The erase is idempotent — a re-erase re-applies cleanly and still reports 0 recoverable.** The
/// shred is idempotent, the provider de-dupes, the ledger merges — a second run is safe (re-erasure
/// after a restore).
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

    // Re-erase: the DEK is already dead, the provider already purged (NothingToErase now), the ledger
    // merges — still green, still 0 recoverable.
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

/// **The ledger `is_empty` reflects the real state (not a constant).** Empty before any record, NOT
/// empty after — pins both arms (a mutation to a constant `true` is caught by the non-empty arm).
#[test]
fn ledger_is_empty_reflects_real_state() {
    let ledger = NotifErasureLedger::new();
    assert!(ledger.is_empty(), "a fresh ledger is empty");
    ledger.record("u-x", &[key("a")], &[], ts());
    assert!(!ledger.is_empty(), "after a record the ledger is NOT empty");
}

/// **`is_green` is the AND of both gate predicates (0 recoverable AND restrict applied) — not a
/// constant, not an OR.** Pins all three arms: green when both hold, NOT green when EITHER fails. A
/// mutation to `true`, or `&&`→`||`, is caught.
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
        "recoverable > 0 → NOT green (even with restrict applied) — the 0-PII threshold"
    );
    assert!(
        !base(0, false).is_green(),
        "restrict NOT applied → NOT green (even with 0 recoverable) — the suppression is required"
    );
    assert!(!base(1, false).is_green(), "neither → NOT green");
}

/// **The X-7 instancing is a named, visible deliverable.** The prompt-id constant pins NOTIF-P27 so
/// the residual is never a silent claim.
#[test]
fn erasure_residual_is_a_named_deliverable() {
    assert_eq!(ERASURE_RESIDUAL_PROMPT, "NOTIF-P27");
}

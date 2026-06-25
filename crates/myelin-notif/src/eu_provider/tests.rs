//! # Unit tests for the EU-sovereign provider adapter (NOTIF-P26 / P-468)
//!
//! Exercises the mandatory-core decision logic to the ≥ 80% mutation floor on `eu_provider.rs`: the
//! EU-region guard (refuse to egress from a non-EU region), the stable-`provider_ref` idempotency (a
//! re-submit after provider-ack is a no-op that returns the SAME ref), the RedactedMessage
//! minimisation (only a redacted summary + link crosses the boundary), and the
//! provider-side-erasure-request hook (an already-sent off-cell payload is purgeable). The
//! whole-system NOTIF-D9 re-run under the real provider lives in
//! `tests/drill_notif_d9_real_provider.rs`.

use super::*;
use crate::prefs::Channel;
use crate::{Class, DeliveryAdapter, HumanisedString, RedactedMessage};
use myelin_tenancy::Region;

fn eu_region() -> Region {
    Region("fr-par".into())
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

fn adapter(region: Region) -> (EuSovereignAdapter, RecordingEuTransport) {
    let transport = RecordingEuTransport::new("eu-mailer");
    let adapter = EuSovereignAdapter::new(Channel::Email, region, Arc::new(transport.clone()));
    (adapter, transport)
}

// ---- the EU-region guard (the sovereignty invariant) -------------------------------------------

#[test]
fn guard_region_accepts_an_eu_region() {
    let (adapter, _) = adapter(eu_region());
    assert!(
        adapter.guard_region().is_ok(),
        "fr-par (EU) is allowed to egress"
    );
    assert!(crate::delivery::is_eu_region(adapter.region()));
}

#[test]
fn guard_region_refuses_a_non_eu_region_loudly() {
    let (adapter, transport) = adapter(Region("us-east".into()));
    let err = adapter.guard_region().unwrap_err();
    assert_eq!(err, EuProviderError::NonEuRegion("us-east".into()));
    assert!(err.to_string().contains("us-east"));
    // try_send refuses BEFORE the vendor is ever called — 0 off-cell egress from a non-EU region.
    let refused = adapter
        .try_send(&redacted_msg(), "itm-1:email")
        .unwrap_err();
    assert_eq!(refused, EuProviderError::NonEuRegion("us-east".into()));
    assert_eq!(
        transport.submit_count("itm-1:email"),
        0,
        "the vendor was NEVER called from a non-EU region (no extra-EU leak)"
    );
}

#[test]
fn deliver_adapter_send_from_non_eu_region_is_a_refusal_not_a_silent_success() {
    let (adapter, transport) = adapter(Region("us-east".into()));
    // The DeliveryAdapter::send shape returns a Receipt; a non-EU region is a refusal (accepted=false),
    // never a silent off-cell egress.
    let receipt = adapter.send(&redacted_msg(), "itm-1:email");
    assert!(
        !receipt.accepted,
        "a non-EU region refuses (accepted=false)"
    );
    assert_eq!(receipt.idem_key, "itm-1:email");
    assert_eq!(
        transport.submit_count("itm-1:email"),
        0,
        "no egress happened"
    );
}

// ---- the stable-provider_ref idempotency (a re-submit is a no-op) -------------------------------

#[test]
fn submit_is_idempotent_a_resubmit_returns_the_same_provider_ref() {
    let (adapter, transport) = adapter(eu_region());
    let first = adapter.try_send(&redacted_msg(), "itm-1:email").unwrap();
    assert!(first.accepted);
    let ref_1 = adapter.provider_ref_for("itm-1:email").unwrap();

    // A re-submit of the SAME idem_key returns the SAME provider_ref and does NOT re-send (the
    // provider-side half of the exactly-one property).
    let retry = adapter.try_send(&redacted_msg(), "itm-1:email").unwrap();
    assert!(retry.accepted);
    let ref_2 = adapter.provider_ref_for("itm-1:email").unwrap();
    assert_eq!(
        ref_1, ref_2,
        "the same idem_key yields the SAME stable provider_ref"
    );
    assert_eq!(
        transport.submit_count("itm-1:email"),
        1,
        "the vendor was asked to submit exactly ONCE (idempotent on idem_key)"
    );
}

#[test]
fn distinct_idem_keys_get_distinct_provider_refs() {
    let (adapter, _) = adapter(eu_region());
    adapter.try_send(&redacted_msg(), "itm-1:email").unwrap();
    adapter.try_send(&redacted_msg(), "itm-2:email").unwrap();
    let r1 = adapter.provider_ref_for("itm-1:email").unwrap();
    let r2 = adapter.provider_ref_for("itm-2:email").unwrap();
    assert_ne!(
        r1, r2,
        "distinct items get distinct provider refs (never collapsed)"
    );
}

#[test]
fn a_bounce_is_not_remembered_so_a_retry_resubmits() {
    let transport = RecordingEuTransport::new("eu-mailer").with_bounce("itm-1:email");
    let adapter = EuSovereignAdapter::new(Channel::Email, eu_region(), Arc::new(transport.clone()));
    let bounced = adapter.try_send(&redacted_msg(), "itm-1:email").unwrap();
    assert!(!bounced.accepted, "the marked key bounces");
    // A bounce leaves NO provider_ref (nothing was accepted) — there is no copy to erase.
    assert!(
        adapter.provider_ref_for("itm-1:email").is_none(),
        "a bounce is not remembered (no accepted copy)"
    );
}

// ---- the RedactedMessage minimisation (only a summary + link crosses the boundary) -------------

#[test]
fn the_adapter_only_ever_carries_a_redacted_message() {
    // Structural: the send signature takes a RedactedMessage (rendered: HumanisedString + class) —
    // there is NO `body` field, so a full body cannot cross the boundary by construction (Art. 5(1)(c)).
    let (adapter, _) = adapter(eu_region());
    let msg = redacted_msg();
    assert_eq!(msg.rendered.text, "you were mentioned on PROJ-1");
    assert_eq!(msg.class, Class::Direct);
    let receipt = adapter.send(&msg, "itm-1:email");
    assert!(receipt.accepted, "the redacted summary delivers");
}

// ---- the provider-side-erasure-request hook (the §10 row 2 sub-processor obligation) -----------

#[test]
fn request_provider_erasure_purges_an_already_sent_payload() {
    let (adapter, transport) = adapter(eu_region());
    let sent = adapter.try_send(&redacted_msg(), "itm-1:email").unwrap();
    assert!(sent.accepted);
    let provider_ref = adapter.provider_ref_for("itm-1:email").unwrap();
    assert!(!transport.was_erased(&provider_ref), "not yet erased");

    // The hook issues a provider-side erasure request against the durable provider_ref.
    let outcome = adapter.request_provider_erasure("itm-1:email").unwrap();
    assert_eq!(
        outcome,
        ProviderErasureOutcome::Requested {
            provider_ref: provider_ref.clone()
        }
    );
    assert!(
        transport.was_erased(&provider_ref),
        "the sub-processor was asked to purge its copy (NOTIF-P27 hook)"
    );
    // After the request, our handle is dropped (the copy is being purged) — a second request is a no-op.
    assert!(adapter.provider_ref_for("itm-1:email").is_none());
    assert_eq!(
        adapter.request_provider_erasure("itm-1:email").unwrap(),
        ProviderErasureOutcome::NothingToErase
    );
}

#[test]
fn request_provider_erasure_for_an_unsent_key_is_a_surfaced_noop() {
    let (adapter, _) = adapter(eu_region());
    // Nothing was sent off-cell for this key (e.g. an in-cell item or a never-delivered one) — there
    // is NO sub-processor copy to erase. A surfaced no-op, not an error.
    assert_eq!(
        adapter.request_provider_erasure("never:email").unwrap(),
        ProviderErasureOutcome::NothingToErase
    );
}

#[test]
fn request_provider_erasure_surfaces_a_vendor_rejection_loudly() {
    // A transport that REJECTS erasure requests — the un-purged copy is the residual, surfaced loudly.
    struct RejectingTransport(RecordingEuTransport);
    impl EuTransport for RejectingTransport {
        fn transport_id(&self) -> &str {
            self.0.transport_id()
        }
        fn submit(&self, m: &RedactedMessage, k: &str, r: &Region) -> TransportReceipt {
            self.0.submit(m, k, r)
        }
        fn request_erasure(&self, _provider_ref: &str) -> bool {
            false // the vendor rejects — the copy is un-purged (the residual).
        }
    }
    let adapter = EuSovereignAdapter::new(
        Channel::Email,
        eu_region(),
        Arc::new(RejectingTransport(RecordingEuTransport::new("eu-mailer"))),
    );
    adapter.try_send(&redacted_msg(), "itm-1:email").unwrap();
    let provider_ref = adapter.provider_ref_for("itm-1:email").unwrap();
    let err = adapter.request_provider_erasure("itm-1:email").unwrap_err();
    assert_eq!(err, EuProviderError::ErasureRejected(provider_ref));
    assert!(err.to_string().contains("erasure"));
}

// ---- the adapter id + region accessors ---------------------------------------------------------

#[test]
fn adapter_id_and_channel_and_region_accessors() {
    let (adapter, _) = adapter(eu_region());
    assert_eq!(adapter.channel(), "email");
    assert_eq!(adapter.region().as_str(), "fr-par");
    assert_eq!(
        adapter.adapter_id(),
        "eu:eu-mailer:email",
        "the adapter id surfaces the sub-processor identity"
    );
}

// ---- the [OPEN — LEGAL] flag is present + dated + unresolved -----------------------------------

#[test]
fn open_legal_flag_is_present_dated_and_unresolved() {
    let flag = OPEN_LEGAL_PROVIDER_DPA;
    assert_eq!(flag.id, "NOTIF-P26-OPEN-LEGAL");
    assert!(
        !flag.resolved,
        "the provider+DPA selection is NOT silently flipped to done"
    );
    assert_eq!(
        flag.raised, "2026-06-25",
        "the flag is dated (a scorecard row, not a silent claim)"
    );
    assert_eq!(flag.owner, "counsel / DPO");
    assert!(
        flag.subject.contains("EU-sovereign delivery vendor"),
        "the decision-shaped surface is the vendor + DPA selection"
    );
    assert!(
        flag.engineering_posture_ships
            .contains("provider-side-erasure-request hook"),
        "the engineering posture (incl. the erasure hook) ships now"
    );
}

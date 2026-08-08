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

#[test]
fn submit_is_idempotent_a_resubmit_returns_the_same_provider_ref() {
    let (adapter, transport) = adapter(eu_region());
    let first = adapter.try_send(&redacted_msg(), "itm-1:email").unwrap();
    assert!(first.accepted);
    let ref_1 = adapter.provider_ref_for("itm-1:email").unwrap();

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
    assert!(
        adapter.provider_ref_for("itm-1:email").is_none(),
        "a bounce is not remembered (no accepted copy)"
    );
}

#[test]
fn the_adapter_only_ever_carries_a_redacted_message() {
    let (adapter, _) = adapter(eu_region());
    let msg = redacted_msg();
    assert_eq!(msg.rendered.text, "you were mentioned on PROJ-1");
    assert_eq!(msg.class, Class::Direct);
    let receipt = adapter.send(&msg, "itm-1:email");
    assert!(receipt.accepted, "the redacted summary delivers");
}

#[test]
fn request_provider_erasure_purges_an_already_sent_payload() {
    let (adapter, transport) = adapter(eu_region());
    let sent = adapter.try_send(&redacted_msg(), "itm-1:email").unwrap();
    assert!(sent.accepted);
    let provider_ref = adapter.provider_ref_for("itm-1:email").unwrap();
    assert!(!transport.was_erased(&provider_ref), "not yet erased");

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
    assert!(adapter.provider_ref_for("itm-1:email").is_none());
    assert_eq!(
        adapter.request_provider_erasure("itm-1:email").unwrap(),
        ProviderErasureOutcome::NothingToErase
    );
}

#[test]
fn request_provider_erasure_for_an_unsent_key_is_a_surfaced_noop() {
    let (adapter, _) = adapter(eu_region());
    assert_eq!(
        adapter.request_provider_erasure("never:email").unwrap(),
        ProviderErasureOutcome::NothingToErase
    );
}

#[test]
fn request_provider_erasure_surfaces_a_vendor_rejection_loudly() {
    struct RejectingTransport(RecordingEuTransport);
    impl EuTransport for RejectingTransport {
        fn transport_id(&self) -> &str {
            self.0.transport_id()
        }
        fn submit(&self, m: &RedactedMessage, k: &str, r: &Region) -> TransportReceipt {
            self.0.submit(m, k, r)
        }
        fn request_erasure(&self, _provider_ref: &str) -> bool {
            false
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

#[test]
fn open_legal_flag_records_when_the_unresolved_decision_was_raised() {
    let flag = OPEN_LEGAL_PROVIDER_DPA;
    assert_eq!(flag.id, "NOTIF-P26-OPEN-LEGAL");
    assert!(
        !flag.resolved,
        "the provider+DPA selection is NOT silently flipped to done"
    );
    assert_eq!(
        flag.raised, "2026-06-25",
        "the unresolved decision records when it was raised"
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

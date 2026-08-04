use super::*;
use crate::prefs::Channel;
use crate::{Class, DeliveryAdapter, HumanisedString, RedactedMessage};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn region() -> Region {
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
    redact_for_offcell(summary(), Class::Direct)
}

#[test]
fn in_app_is_the_only_in_cell_channel() {
    assert!(
        Channel::InApp.is_in_cell(),
        "in_app is the in-cell inbox push (never egresses)"
    );
    assert!(!Channel::InApp.is_off_cell());
    for c in [
        Channel::WebPush,
        Channel::MobilePush,
        Channel::Email,
        Channel::Desktop,
    ] {
        assert!(
            c.is_off_cell(),
            "{} egresses → off-cell, redacted",
            c.token()
        );
        assert!(!c.is_in_cell());
    }
}

#[test]
fn idem_key_is_stable_per_item_channel_and_channel_scoped() {
    assert_eq!(
        build_idem_key("itm-1", Channel::Email),
        build_idem_key("itm-1", Channel::Email)
    );
    assert_eq!(build_idem_key("itm-1", Channel::Email), "itm-1:email");
    assert_ne!(
        build_idem_key("itm-1", Channel::Email),
        build_idem_key("itm-1", Channel::WebPush)
    );
    assert_ne!(
        build_idem_key("itm-1", Channel::Email),
        build_idem_key("itm-2", Channel::Email)
    );
}

#[test]
fn redact_for_offcell_carries_summary_and_link_not_the_body() {
    let msg = redact_for_offcell(summary(), Class::Direct);
    assert_eq!(msg.rendered.text, "you were mentioned on PROJ-1");
    assert_eq!(
        msg.rendered.links,
        vec!["myelin://acme/issues/issue/PROJ-1".to_string()]
    );
    assert_eq!(msg.class, Class::Direct);
}

#[test]
fn ledger_record_is_first_writer_wins_on_idem_key() {
    let ledger = DeliveryLedger::new();
    let rec = |accepted| DeliveryRecord {
        item_id: "itm-1".into(),
        channel: Channel::Email,
        idem_key: "itm-1:email".into(),
        redacted: true,
        accepted,
        adapter: "fr-par:email".into(),
    };
    assert!(ledger.record(&tenant(), rec(true)), "first record wins");
    assert!(
        !ledger.record(&tenant(), rec(false)),
        "the duplicate is collapsed (no-op)"
    );
    assert!(
        ledger.get(&tenant(), "itm-1:email").unwrap().accepted,
        "the first row is preserved"
    );
    assert_eq!(
        ledger.effective_count(&tenant()),
        1,
        "exactly one effective delivery"
    );
}

#[test]
fn ledger_is_tenant_partitioned() {
    let ledger = DeliveryLedger::new();
    let rec = DeliveryRecord {
        item_id: "itm-1".into(),
        channel: Channel::Email,
        idem_key: "itm-1:email".into(),
        redacted: true,
        accepted: true,
        adapter: "fr-par:email".into(),
    };
    assert!(ledger.record(&TenantId("acme".into()), rec.clone()));
    assert!(ledger.record(&TenantId("globex".into()), rec));
    assert_eq!(ledger.effective_count(&TenantId("acme".into())), 1);
    assert_eq!(ledger.effective_count(&TenantId("globex".into())), 1);
    assert!(!ledger.contains(&TenantId("globex".into()), "nope"));
}

#[test]
fn mock_adapter_records_each_send_and_accepts_by_default() {
    let mock = MockAdapter::new(Channel::Email, region());
    assert_eq!(mock.channel(), "email");
    assert_eq!(mock.region().as_str(), "fr-par");
    let r = mock.send(&redacted_msg(), "itm-1:email");
    assert!(r.accepted, "the mock accepts by default");
    assert_eq!(r.idem_key, "itm-1:email");
    assert_eq!(
        mock.send_count("itm-1:email"),
        1,
        "the provider was invoked once"
    );
    assert_eq!(mock.sent_log(), vec!["itm-1:email".to_string()]);
}

#[test]
fn mock_adapter_bounces_a_marked_key() {
    let mock = MockAdapter::new(Channel::Email, region()).with_bounce("itm-1:email");
    let r = mock.send(&redacted_msg(), "itm-1:email");
    assert!(!r.accepted, "the marked key bounces (accepted=false)");
}

#[test]
fn deliver_is_idempotent_a_retry_after_ack_is_a_noop() {
    let ledger = DeliveryLedger::new();
    let mock = MockAdapter::new(Channel::Email, region());
    let fabric =
        DeliveryFabric::new(ledger.clone()).with_adapter(std::sync::Arc::new(mock.clone()));
    let msg = redacted_msg();

    let out = fabric
        .deliver(&tenant(), "itm-1", Channel::Email, &msg)
        .unwrap();
    assert!(
        matches!(out, DeliveryOutcome::Delivered(_)),
        "first deliver is a new effective delivery"
    );
    assert_eq!(
        mock.send_count("itm-1:email"),
        1,
        "provider invoked exactly once"
    );

    let retry = fabric
        .deliver(&tenant(), "itm-1", Channel::Email, &msg)
        .unwrap();
    assert_eq!(
        retry,
        DeliveryOutcome::AlreadyDelivered { accepted: true },
        "the retry is collapsed by UNIQUE(tenant, idem_key) - no re-deliver"
    );
    assert_eq!(
        mock.send_count("itm-1:email"),
        1,
        "the provider was NOT invoked again (exactly once)"
    );

    assert_eq!(
        effective_delivery_count(&ledger, &tenant(), "itm-1", Channel::Email),
        1
    );
    assert_eq!(ledger.effective_count(&tenant()), 1);
}

#[test]
fn deliver_records_redacted_true_for_offcell_and_false_for_in_app() {
    let ledger = DeliveryLedger::new();
    let fabric = DeliveryFabric::with_mock(ledger.clone(), region());

    fabric
        .deliver(&tenant(), "itm-1", Channel::Email, &redacted_msg())
        .unwrap();
    let email_row = ledger.get(&tenant(), "itm-1:email").unwrap();
    assert!(
        email_row.redacted,
        "off-cell email is redacted (delivery.redacted=true)"
    );

    let in_app_msg = RedactedMessage {
        rendered: summary(),
        class: Class::Direct,
    };
    fabric
        .deliver(&tenant(), "itm-1", Channel::InApp, &in_app_msg)
        .unwrap();
    let in_app_row = ledger.get(&tenant(), "itm-1:in_app").unwrap();
    assert!(
        !in_app_row.redacted,
        "in_app stays in-cell (redacted=false, no off-cell egress)"
    );
}

#[test]
fn deliver_to_two_channels_is_two_distinct_deliveries() {
    let ledger = DeliveryLedger::new();
    let fabric = DeliveryFabric::with_mock(ledger.clone(), region());
    fabric
        .deliver(&tenant(), "itm-1", Channel::Email, &redacted_msg())
        .unwrap();
    fabric
        .deliver(&tenant(), "itm-1", Channel::WebPush, &redacted_msg())
        .unwrap();
    assert_eq!(ledger.effective_count(&tenant()), 2);
    assert_eq!(
        effective_delivery_count(&ledger, &tenant(), "itm-1", Channel::Email),
        1
    );
    assert_eq!(
        effective_delivery_count(&ledger, &tenant(), "itm-1", Channel::WebPush),
        1
    );
}

#[test]
fn deliver_a_bounce_is_recorded_and_still_idempotent() {
    let ledger = DeliveryLedger::new();
    let mock = MockAdapter::new(Channel::Email, region()).with_bounce("itm-1:email");
    let fabric =
        DeliveryFabric::new(ledger.clone()).with_adapter(std::sync::Arc::new(mock.clone()));

    let out = fabric
        .deliver(&tenant(), "itm-1", Channel::Email, &redacted_msg())
        .unwrap();
    assert!(
        matches!(out, DeliveryOutcome::Bounced(_)),
        "a rejected message is a bounce"
    );
    assert_eq!(ledger.effective_count(&tenant()), 1);
    let retry = fabric
        .deliver(&tenant(), "itm-1", Channel::Email, &redacted_msg())
        .unwrap();
    assert_eq!(retry, DeliveryOutcome::AlreadyDelivered { accepted: false });
    assert_eq!(
        mock.send_count("itm-1:email"),
        1,
        "the bounced send is not re-invoked on retry"
    );
}

#[test]
fn deliver_to_an_unregistered_channel_errors_loudly() {
    let ledger = DeliveryLedger::new();
    let fabric = DeliveryFabric::new(ledger).with_adapter(std::sync::Arc::new(MockAdapter::new(
        Channel::Email,
        region(),
    )));
    let err = fabric
        .deliver(&tenant(), "itm-1", Channel::WebPush, &redacted_msg())
        .unwrap_err();
    assert_eq!(
        err,
        DeliveryError::NoAdapter("web_push"),
        "no adapter → loud error, no silent drop"
    );
    assert!(err.to_string().contains("web_push"));
}

#[test]
fn eu_preferring_posture_recognises_eu_regions() {
    assert!(
        is_eu_region(&Region("fr-par".into())),
        "fr-par (Scaleway Paris) is EU"
    );
    assert!(is_eu_region(&Region("nl-ams".into())), "nl-ams is EU");
    assert!(is_eu_region(&Region("eu-west".into())), "eu-* is EU");
    assert!(is_eu_region(&Region("de-fra".into())), "de-* is EU");
    assert!(
        !is_eu_region(&Region("us-east".into())),
        "us-east is NOT EU"
    );
    assert!(!is_eu_region(&Region("ap-tokyo".into())), "ap-* is NOT EU");
    let mock = MockAdapter::new(Channel::Email, Region("fr-par".into()));
    assert!(
        is_eu_region(mock.region()),
        "the mock delivers from an EU region (EU-preferring)"
    );
}

#[test]
fn channel_token_round_trips() {
    for c in [
        Channel::InApp,
        Channel::WebPush,
        Channel::MobilePush,
        Channel::Email,
        Channel::Desktop,
    ] {
        assert_eq!(
            channel_from_token(c.token()),
            Some(c),
            "{} round-trips",
            c.token()
        );
    }
    assert_eq!(
        channel_from_token("not-a-channel"),
        None,
        "an unknown token is None"
    );
}

#[test]
fn effective_delivery_count_is_zero_for_an_undelivered_item() {
    let ledger = DeliveryLedger::new();
    assert_eq!(
        effective_delivery_count(&ledger, &tenant(), "never", Channel::Email),
        0
    );
    let fabric = DeliveryFabric::with_mock(ledger.clone(), region());
    fabric
        .deliver(&tenant(), "itm-1", Channel::Email, &redacted_msg())
        .unwrap();
    assert_eq!(
        effective_delivery_count(&ledger, &tenant(), "itm-1", Channel::Email),
        1
    );
    assert_eq!(
        effective_delivery_count(&ledger, &tenant(), "itm-1", Channel::WebPush),
        0
    );
}

#[test]
fn mock_send_count_is_zero_before_any_send_and_counts_repeats() {
    let mock = MockAdapter::new(Channel::Email, region());
    assert_eq!(mock.send_count("itm-1:email"), 0);
    mock.send(&redacted_msg(), "itm-1:email");
    mock.send(&redacted_msg(), "itm-1:email");
    assert_eq!(mock.send_count("itm-1:email"), 2);
    assert_eq!(mock.send_count("other:email"), 0);
}

#[test]
fn fabric_ledger_accessor_returns_the_shared_ledger() {
    let ledger = DeliveryLedger::new();
    let fabric = DeliveryFabric::with_mock(ledger.clone(), region());
    fabric
        .deliver(&tenant(), "itm-1", Channel::Email, &redacted_msg())
        .unwrap();
    assert_eq!(fabric.ledger().effective_count(&tenant()), 1);
    assert!(fabric.ledger().contains(&tenant(), "itm-1:email"));
}

#[test]
fn with_mock_registers_an_adapter_for_every_channel() {
    let fabric = DeliveryFabric::with_mock(DeliveryLedger::new(), region());
    for c in [
        Channel::InApp,
        Channel::WebPush,
        Channel::MobilePush,
        Channel::Email,
        Channel::Desktop,
    ] {
        assert!(
            fabric.adapter(c).is_some(),
            "the mock fabric has an adapter for {}",
            c.token()
        );
        assert!(
            is_eu_region(fabric.adapter(c).unwrap().region()),
            "EU-preferring per channel"
        );
    }
}

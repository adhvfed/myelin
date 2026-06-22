//! # Unit tests for the delivery fabric (NOTIF-P16 / P-194)
//!
//! Exercises the mandatory-core decision logic to the ≥ 80% mutation floor on `delivery.rs`: the
//! idem_key collapse (a retry after provider-ack is a no-op — exactly one effective delivery), the
//! redaction discipline (off-cell carries the summary + link, never the body; in-app stays in-cell),
//! the first-writer-wins ledger, the bounce path, and the EU-preferring posture. The whole-system
//! NOTIF-D9 drill (the crash between provider-ack and ledger-write) lives in `tests/drill_notif_d9.rs`.

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

// ---- the in-cell / off-cell channel split (the in-app-stays-in-cell invariant) -----------------

#[test]
fn in_app_is_the_only_in_cell_channel() {
    assert!(
        Channel::InApp.is_in_cell(),
        "in_app is the in-cell inbox push (never egresses)"
    );
    assert!(!Channel::InApp.is_off_cell());
    // Every other channel egresses → MUST be redacted.
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

// ---- the idempotency key (stable, channel-scoped, PII-free) -------------------------------------

#[test]
fn idem_key_is_stable_per_item_channel_and_channel_scoped() {
    // Stable: the same (item, channel) yields the same key (a retry produces the same dedup key).
    assert_eq!(
        build_idem_key("itm-1", Channel::Email),
        build_idem_key("itm-1", Channel::Email)
    );
    assert_eq!(build_idem_key("itm-1", Channel::Email), "itm-1:email");
    // Channel-scoped: the same item to two channels is two DISTINCT deliveries (never collapsed).
    assert_ne!(
        build_idem_key("itm-1", Channel::Email),
        build_idem_key("itm-1", Channel::WebPush)
    );
    // Item-scoped: different items never collapse.
    assert_ne!(
        build_idem_key("itm-1", Channel::Email),
        build_idem_key("itm-2", Channel::Email)
    );
}

// ---- the redaction discipline (off-cell carries summary + link, NEVER the body) ----------------

#[test]
fn redact_for_offcell_carries_summary_and_link_not_the_body() {
    let msg = redact_for_offcell(summary(), Class::Direct);
    // The off-cell payload is the humanised SUMMARY (viewer-safe) + the deep link + the class.
    assert_eq!(msg.rendered.text, "you were mentioned on PROJ-1");
    assert_eq!(
        msg.rendered.links,
        vec!["myelin://acme/issues/issue/PROJ-1".to_string()]
    );
    assert_eq!(msg.class, Class::Direct);
    // A RedactedMessage carries NO `body` field at all — the full body cannot cross the boundary by
    // construction (the type has only {rendered: HumanisedString, class}). This is a structural test.
}

// ---- the delivery ledger (UNIQUE(tenant, idem_key) first-writer-wins) ---------------------------

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
    // The FIRST record wins (true); a second on the SAME idem_key is REJECTED (false) — the
    // UNIQUE(tenant, idem_key) collapse. The first record's row is preserved (accepted=true stays).
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
    // The SAME idem_key under a DIFFERENT tenant is a distinct row (tenant-partitioned).
    assert!(ledger.record(&TenantId("globex".into()), rec));
    assert_eq!(ledger.effective_count(&TenantId("acme".into())), 1);
    assert_eq!(ledger.effective_count(&TenantId("globex".into())), 1);
    assert!(!ledger.contains(&TenantId("globex".into()), "nope"));
}

// ---- the deterministic mock adapter (record-only, exactly-once provider call) ------------------

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

// ---- THE IDEM-KEY COLLAPSE: a retry after provider-ack is a NO-OP (the core invariant) ----------

#[test]
fn deliver_is_idempotent_a_retry_after_ack_is_a_noop() {
    let ledger = DeliveryLedger::new();
    let mock = MockAdapter::new(Channel::Email, region());
    let fabric =
        DeliveryFabric::new(ledger.clone()).with_adapter(std::sync::Arc::new(mock.clone()));
    let msg = redacted_msg();

    // First delivery → the provider is invoked, the ledger row recorded, exactly one effective.
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

    // RETRY on the SAME (item, channel) → collapsed by the ledger; the provider is NOT re-invoked.
    let retry = fabric
        .deliver(&tenant(), "itm-1", Channel::Email, &msg)
        .unwrap();
    assert_eq!(
        retry,
        DeliveryOutcome::AlreadyDelivered { accepted: true },
        "the retry is collapsed by UNIQUE(tenant, idem_key) — no re-deliver"
    );
    assert_eq!(
        mock.send_count("itm-1:email"),
        1,
        "the provider was NOT invoked again (exactly once)"
    );

    // Exactly ONE effective delivery per (item, channel) — the NOTIF-D9 threshold (never 2).
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

    // Off-cell email → redacted=true (the §3.6 PII-minimisation flag set on the row).
    fabric
        .deliver(&tenant(), "itm-1", Channel::Email, &redacted_msg())
        .unwrap();
    let email_row = ledger.get(&tenant(), "itm-1:email").unwrap();
    assert!(
        email_row.redacted,
        "off-cell email is redacted (delivery.redacted=true)"
    );

    // In-app → redacted=false (stays in-cell; no off-cell egress, no redaction flag).
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
    // Two distinct effective deliveries (the same item to two channels is never collapsed).
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
    // A bounce IS recorded (so a blind retry does not re-deliver) — still exactly one effective row.
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
    // A fabric with ONLY an email adapter — delivering to web_push must error (never a silent drop).
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

// ---- the EU-preferring posture (the §3.6 FLOOR) -------------------------------------------------

#[test]
fn eu_preferring_posture_recognises_eu_regions() {
    assert!(
        is_eu_region(&Region("fr-par".into())),
        "fr-par (Scaleway Paris) is EU"
    );
    assert!(is_eu_region(&Region("nl-ams".into())), "nl-ams is EU");
    assert!(is_eu_region(&Region("eu-west".into())), "eu-* is EU");
    assert!(is_eu_region(&Region("de-fra".into())), "de-* is EU");
    // Conservative: an unknown / non-EU region is NOT assumed EU.
    assert!(
        !is_eu_region(&Region("us-east".into())),
        "us-east is NOT EU"
    );
    assert!(!is_eu_region(&Region("ap-tokyo".into())), "ap-* is NOT EU");
    // The mock adapter is EU-preferring (the v1 dev runtime delivers from an EU region).
    let mock = MockAdapter::new(Channel::Email, Region("fr-par".into()));
    assert!(
        is_eu_region(mock.region()),
        "the mock delivers from an EU region (EU-preferring)"
    );
}

// ---- channel <-> token round-trip --------------------------------------------------------------

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
    // A never-delivered (item, channel) reads 0 (not 1) — the signal distinguishes 0/1.
    assert_eq!(
        effective_delivery_count(&ledger, &tenant(), "never", Channel::Email),
        0
    );
    // And it reads exactly 1 once delivered (never 2 — the per-key boolean count).
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
    // 0 before any send (distinguishes the count from a constant 1).
    assert_eq!(mock.send_count("itm-1:email"), 0);
    mock.send(&redacted_msg(), "itm-1:email");
    mock.send(&redacted_msg(), "itm-1:email");
    // A raw adapter (no fabric dedupe) counts BOTH calls — proving the count is a real tally.
    assert_eq!(mock.send_count("itm-1:email"), 2);
    // An unrelated key reads 0.
    assert_eq!(mock.send_count("other:email"), 0);
}

#[test]
fn fabric_ledger_accessor_returns_the_shared_ledger() {
    let ledger = DeliveryLedger::new();
    let fabric = DeliveryFabric::with_mock(ledger.clone(), region());
    fabric
        .deliver(&tenant(), "itm-1", Channel::Email, &redacted_msg())
        .unwrap();
    // The fabric's own ledger accessor sees the recorded row (it is the SAME shared ledger, not a
    // fresh default) — a mutant that returns a default empty ledger would read 0 here.
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

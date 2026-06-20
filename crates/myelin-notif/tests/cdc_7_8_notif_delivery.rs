//! # CDC — contract 7.8 `DeliveryAdapter` (the EU-sovereign delivery fabric) (P-194)
//!
//! **Architecture:** `notifications.md` §3.6 (the EU-sovereign delivery fabric: ONE trait
//! `DeliveryAdapter{channel, region, send(RedactedMessage, idem_key), receipts}` — EU-preferring,
//! region-aware, swappable; PII-minimised off-cell payloads (`RedactedMessage` = summary + deep link,
//! `delivery.redacted=true`, GDPR Art. 5(1)(c)); in-app stays in-cell; at-least-once + idempotent on
//! `UNIQUE(idem_key)`). **Contract:** **7.8** `DeliveryAdapter{channel, region, send(RedactedMessage,
//! idem_key), receipts}` (OWNED — the trait was frozen as a carrier in NOTIF-P1; the BODY is here).
//!
//! This CDC pins the 7.8 seam from BOTH sides:
//!
//! - **PROVIDER (Notif owns 7.8):** the [`DeliveryFabric`] delivers at-least-once + idempotent on
//!   `UNIQUE(tenant, idem_key)` — a retried send collapses to exactly one effective delivery; an
//!   off-cell channel carries ONLY a `RedactedMessage` (`delivery.redacted=true`); the in-app channel
//!   stays in-cell. The region-aware adapter is EU-preferring.
//! - **CONSUMER (a channel adapter — mock now, the real EU provider NOTIF-P25 later):** a concrete
//!   adapter implements the SAME `DeliveryAdapter` trait (the strategy-pattern swap point); the
//!   fabric dispatches to it by channel and records its receipt. A drift on the trait signature
//!   (`channel`/`region`/`send(RedactedMessage, idem_key) -> Receipt`) breaks THIS build.
//!
//! Both halves agree on the WIRE: the trait signature, the `idem_key` collapse, and the off-cell
//! redaction discipline. The real EU provider (NOTIF-P25/P26) is a named floor; here the deterministic
//! mock proves the PROPERTIES (idempotency, redaction, EU-preferring) against the seam.

use myelin_notif::prefs::Channel;
use myelin_notif::{
    build_idem_key, is_eu_region, redact_for_offcell, Class, DeliveryAdapter, DeliveryFabric,
    DeliveryLedger, DeliveryOutcome, HumanisedString, MockAdapter, RedactedMessage, Receipt,
};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn summary() -> HumanisedString {
    HumanisedString {
        text: "you were mentioned on PROJ-1".into(),
        links: vec!["myelin://acme/issues/issue/PROJ-1".into()],
        icon: "mention".into(),
    }
}

// === PROVIDER side: the fabric delivers at-least-once + idempotent, EU-preferring, redacted ===

/// **PROVIDER — the fabric is at-least-once + idempotent on `UNIQUE(tenant, idem_key)`.**
#[test]
fn provider_fabric_is_at_least_once_and_idempotent() {
    let ledger = DeliveryLedger::new();
    let mock = MockAdapter::new(Channel::Email, Region("fr-par".into()));
    let fabric = DeliveryFabric::new(ledger.clone()).with_adapter(Arc::new(mock.clone()));
    let msg = redact_for_offcell(summary(), Class::Direct);

    let first = fabric.deliver(&tenant(), "itm-1", Channel::Email, &msg).unwrap();
    assert!(matches!(first, DeliveryOutcome::Delivered(_)));
    // The retry collapses (UNIQUE(tenant, idem_key)) — the provider is invoked exactly once.
    let retry = fabric.deliver(&tenant(), "itm-1", Channel::Email, &msg).unwrap();
    assert_eq!(retry, DeliveryOutcome::AlreadyDelivered { accepted: true });
    assert_eq!(mock.send_count(&build_idem_key("itm-1", Channel::Email)), 1);
    assert_eq!(ledger.effective_count(&tenant()), 1, "exactly one effective delivery");
}

/// **PROVIDER — off-cell carries a RedactedMessage (`redacted=true`); in-app stays in-cell.**
#[test]
fn provider_offcell_is_redacted_in_app_is_in_cell() {
    let ledger = DeliveryLedger::new();
    let fabric = DeliveryFabric::with_mock(ledger.clone(), Region("fr-par".into()));
    fabric
        .deliver(&tenant(), "itm-1", Channel::Email, &redact_for_offcell(summary(), Class::Direct))
        .unwrap();
    assert!(ledger.get(&tenant(), "itm-1:email").unwrap().redacted, "off-cell is redacted");
    fabric
        .deliver(
            &tenant(),
            "itm-1",
            Channel::InApp,
            &RedactedMessage { rendered: summary(), class: Class::Direct },
        )
        .unwrap();
    assert!(!ledger.get(&tenant(), "itm-1:in_app").unwrap().redacted, "in_app stays in-cell");
}

/// **PROVIDER — the adapter is region-aware + EU-preferring (the §3.6 posture).**
#[test]
fn provider_adapter_is_eu_preferring() {
    let mock = MockAdapter::new(Channel::Email, Region("fr-par".into()));
    assert_eq!(mock.channel(), "email");
    assert!(is_eu_region(mock.region()), "the adapter delivers from an EU region (EU-preferring)");
}

// === CONSUMER side: a concrete adapter implements the SAME trait (the strategy-pattern swap) ===

/// **CONSUMER — a real-shaped adapter implements 7.8 and the fabric dispatches to it.** This stands
/// in for the production EU provider (NOTIF-P25): it implements the SAME trait signature, so the swap
/// is a config change, never a code change (ADR-12.8 strategy pattern). A drift breaks THIS build.
#[test]
fn consumer_a_custom_adapter_satisfies_the_frozen_trait() {
    use std::sync::Mutex;
    struct EuProviderStub {
        region: Region,
        calls: Mutex<usize>,
    }
    impl DeliveryAdapter for EuProviderStub {
        fn channel(&self) -> &str {
            "email"
        }
        fn region(&self) -> &Region {
            &self.region
        }
        fn send(&self, _message: &RedactedMessage, idem_key: &str) -> Receipt {
            *self.calls.lock().unwrap() += 1;
            Receipt { idem_key: idem_key.to_string(), accepted: true }
        }
    }
    let stub = Arc::new(EuProviderStub { region: Region("nl-ams".into()), calls: Mutex::new(0) });
    let ledger = DeliveryLedger::new();
    let fabric = DeliveryFabric::new(ledger).with_adapter(stub.clone());
    // The fabric dispatches the email channel to the custom adapter (the swap point).
    let out = fabric
        .deliver(&tenant(), "itm-1", Channel::Email, &redact_for_offcell(summary(), Class::Direct))
        .unwrap();
    assert!(matches!(out, DeliveryOutcome::Delivered(_)));
    assert_eq!(*stub.calls.lock().unwrap(), 1, "the custom EU-region adapter was dispatched to");
    assert!(is_eu_region(stub.region()), "the swapped-in adapter is EU-preferring (nl-ams)");
}

/// **The wire both sides agree on: the `idem_key` is `<item>:<channel>` (the collapse key).**
#[test]
fn the_idem_key_wire_is_frozen() {
    assert_eq!(build_idem_key("itm-1", Channel::Email), "itm-1:email");
    assert_eq!(build_idem_key("itm-1", Channel::InApp), "itm-1:in_app");
}

//! # CDC — contract 7.8 `DeliveryAdapter` under the REAL EU-sovereign provider (NOTIF-P26 / P-468)
//!
//! **Architecture:** `notifications.md` §3.6 (the EU-sovereign delivery fabric — ONE trait
//! `DeliveryAdapter{channel, region, send(RedactedMessage, idem_key), receipts}`, EU-preferring,
//! region-aware, swappable; off-cell redacted; at-least-once + idempotent) + §10 row 2 (the concrete
//! production EU provider — `[OPEN — LEGAL]`). **Contract:** **7.8** (CONSUMED — the real provider
//! swaps into the SAME frozen trait via the strategy pattern, NO shape change).
//!
//! This CDC pins the 7.8 seam from the REAL-PROVIDER side: the production [`EuSovereignAdapter`]
//! implements the SAME `DeliveryAdapter` signature the mock did (`channel`/`region`/`send` →
//! `Receipt`), the [`DeliveryFabric`] dispatches to it by channel, and the same at-least-once +
//! idempotent + EU-preferring + off-cell-redacted PROPERTIES hold. A drift on the trait signature
//! breaks THIS build (the swap-is-a-config-change, never-a-code-change mandate, ADR-12.8).

use myelin_notif::prefs::Channel;
use myelin_notif::{
    build_idem_key, is_eu_region, redact_for_offcell, Class, DeliveryAdapter, DeliveryFabric,
    DeliveryLedger, DeliveryOutcome, EuSovereignAdapter, HumanisedString, RecordingEuTransport,
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

/// **The real EU provider satisfies the frozen 7.8 trait + the at-least-once + idempotent property.**
#[test]
fn real_provider_is_at_least_once_and_idempotent_over_the_frozen_trait() {
    let ledger = DeliveryLedger::new();
    let transport = RecordingEuTransport::new("eu-mailer");
    // The real adapter registers over the SAME `DeliveryAdapter` trait the mock used (the swap point).
    let real: Arc<dyn DeliveryAdapter + Send + Sync> = Arc::new(EuSovereignAdapter::new(
        Channel::Email,
        Region("fr-par".into()),
        Arc::new(transport.clone()),
    ));
    let fabric = DeliveryFabric::new(ledger.clone()).with_adapter(real);
    let msg = redact_for_offcell(summary(), Class::Direct);

    let first = fabric
        .deliver(&tenant(), "itm-1", Channel::Email, &msg)
        .unwrap();
    assert!(matches!(first, DeliveryOutcome::Delivered(_)));
    let retry = fabric
        .deliver(&tenant(), "itm-1", Channel::Email, &msg)
        .unwrap();
    assert_eq!(retry, DeliveryOutcome::AlreadyDelivered { accepted: true });
    assert_eq!(
        transport.submit_count(&build_idem_key("itm-1", Channel::Email)),
        1,
        "the vendor was submitted to exactly once (idempotent under the real provider)"
    );
    assert_eq!(
        ledger.effective_count(&tenant()),
        1,
        "exactly one effective delivery"
    );
}

/// **The real provider is region-aware + EU-preferring (the §3.6/§10 sovereignty posture).**
#[test]
fn real_provider_is_eu_preferring() {
    let real = EuSovereignAdapter::new(
        Channel::Email,
        Region("nl-ams".into()),
        Arc::new(RecordingEuTransport::new("eu-mailer")),
    );
    assert_eq!(real.channel(), "email");
    assert!(
        is_eu_region(real.region()),
        "the real provider egresses from an EU region"
    );
    assert!(real.guard_region().is_ok());
}

/// **The real provider carries ONLY a RedactedMessage off-cell (`redacted=true`) — the wire is frozen.**
#[test]
fn real_provider_offcell_is_redacted() {
    let ledger = DeliveryLedger::new();
    let real: Arc<dyn DeliveryAdapter + Send + Sync> = Arc::new(EuSovereignAdapter::new(
        Channel::Email,
        Region("fr-par".into()),
        Arc::new(RecordingEuTransport::new("eu-mailer")),
    ));
    let fabric = DeliveryFabric::new(ledger.clone()).with_adapter(real);
    fabric
        .deliver(
            &tenant(),
            "itm-1",
            Channel::Email,
            &redact_for_offcell(summary(), Class::Direct),
        )
        .unwrap();
    assert!(
        ledger.get(&tenant(), "itm-1:email").unwrap().redacted,
        "off-cell is redacted under the real provider (delivery.redacted=true)"
    );
}

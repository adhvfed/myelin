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

#[test]
fn real_provider_is_at_least_once_and_idempotent_over_the_frozen_trait() {
    let ledger = DeliveryLedger::new();
    let transport = RecordingEuTransport::new("eu-mailer");
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

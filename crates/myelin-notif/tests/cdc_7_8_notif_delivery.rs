use myelin_notif::prefs::Channel;
use myelin_notif::{
    build_idem_key, is_eu_region, redact_for_offcell, Class, DeliveryAdapter, DeliveryFabric,
    DeliveryLedger, DeliveryOutcome, HumanisedString, MockAdapter, Receipt, RedactedMessage,
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
fn provider_fabric_is_at_least_once_and_idempotent() {
    let ledger = DeliveryLedger::new();
    let mock = MockAdapter::new(Channel::Email, Region("fr-par".into()));
    let fabric = DeliveryFabric::new(ledger.clone()).with_adapter(Arc::new(mock.clone()));
    let msg = redact_for_offcell(summary(), Class::Direct);

    let first = fabric
        .deliver(&tenant(), "itm-1", Channel::Email, &msg)
        .unwrap();
    assert!(matches!(first, DeliveryOutcome::Delivered(_)));
    let retry = fabric
        .deliver(&tenant(), "itm-1", Channel::Email, &msg)
        .unwrap();
    assert_eq!(retry, DeliveryOutcome::AlreadyDelivered { accepted: true });
    assert_eq!(mock.send_count(&build_idem_key("itm-1", Channel::Email)), 1);
    assert_eq!(
        ledger.effective_count(&tenant()),
        1,
        "exactly one effective delivery"
    );
}

#[test]
fn provider_offcell_is_redacted_in_app_is_in_cell() {
    let ledger = DeliveryLedger::new();
    let fabric = DeliveryFabric::with_mock(ledger.clone(), Region("fr-par".into()));
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
        "off-cell is redacted"
    );
    fabric
        .deliver(
            &tenant(),
            "itm-1",
            Channel::InApp,
            &RedactedMessage {
                rendered: summary(),
                class: Class::Direct,
            },
        )
        .unwrap();
    assert!(
        !ledger.get(&tenant(), "itm-1:in_app").unwrap().redacted,
        "in_app stays in-cell"
    );
}

#[test]
fn provider_adapter_is_eu_preferring() {
    let mock = MockAdapter::new(Channel::Email, Region("fr-par".into()));
    assert_eq!(mock.channel(), "email");
    assert!(
        is_eu_region(mock.region()),
        "the adapter delivers from an EU region (EU-preferring)"
    );
}

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
            Receipt {
                idem_key: idem_key.to_string(),
                accepted: true,
            }
        }
    }
    let stub = Arc::new(EuProviderStub {
        region: Region("nl-ams".into()),
        calls: Mutex::new(0),
    });
    let ledger = DeliveryLedger::new();
    let fabric = DeliveryFabric::new(ledger).with_adapter(stub.clone());
    let out = fabric
        .deliver(
            &tenant(),
            "itm-1",
            Channel::Email,
            &redact_for_offcell(summary(), Class::Direct),
        )
        .unwrap();
    assert!(matches!(out, DeliveryOutcome::Delivered(_)));
    assert_eq!(
        *stub.calls.lock().unwrap(),
        1,
        "the custom EU-region adapter was dispatched to"
    );
    assert!(
        is_eu_region(stub.region()),
        "the swapped-in adapter is EU-preferring (nl-ams)"
    );
}

#[test]
fn the_idem_key_wire_is_frozen() {
    assert_eq!(build_idem_key("itm-1", Channel::Email), "itm-1:email");
    assert_eq!(build_idem_key("itm-1", Channel::InApp), "itm-1:in_app");
}

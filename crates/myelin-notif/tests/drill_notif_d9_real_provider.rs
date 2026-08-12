use myelin_notif::prefs::Channel;
use myelin_notif::{
    build_idem_key, effective_delivery_count, redact_for_offcell, Class, DeliveryFabric,
    DeliveryLedger, DeliveryOutcome, EuSovereignAdapter, HumanisedString, ProviderErasureOutcome,
    RecordingEuTransport,
};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

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

#[test]
fn notif_d9_real_provider_crash_after_ledger_write_retry_is_a_noop_exactly_one() {
    let ledger = DeliveryLedger::new();
    let transport = RecordingEuTransport::new("eu-mailer");
    let real = EuSovereignAdapter::new(Channel::Email, region(), Arc::new(transport.clone()));
    let fabric_a = DeliveryFabric::new(ledger.clone()).with_adapter(Arc::new(real));
    let msg = redact_for_offcell(summary(), Class::Direct);

    let out = fabric_a
        .deliver(&tenant(), "itm-1", Channel::Email, &msg)
        .unwrap();
    assert!(
        matches!(out, DeliveryOutcome::Delivered(_)),
        "the first deliver is a new effective delivery (real provider)"
    );
    assert_eq!(
        transport.submit_count(&build_idem_key("itm-1", Channel::Email)),
        1,
        "the vendor was asked to submit once"
    );

    drop(fabric_a);

    let real_b = EuSovereignAdapter::new(Channel::Email, region(), Arc::new(transport.clone()));
    let fabric_b = DeliveryFabric::new(ledger.clone()).with_adapter(Arc::new(real_b));
    let retry = fabric_b
        .deliver(&tenant(), "itm-1", Channel::Email, &msg)
        .unwrap();
    assert_eq!(
        retry,
        DeliveryOutcome::AlreadyDelivered { accepted: true },
        "the retry after the crash is collapsed by UNIQUE(tenant, idem_key) - no re-submit (real provider)"
    );

    assert_eq!(
        effective_delivery_count(&ledger, &tenant(), "itm-1", Channel::Email),
        1,
        "exactly 1 effective delivery per (item, channel) under the REAL provider (NOTIF-D9 re-run)"
    );
    assert_eq!(
        transport.submit_count(&build_idem_key("itm-1", Channel::Email)),
        1,
        "the recovered retry did NOT re-submit to the vendor (idempotent on the ledger)"
    );
    assert!(
        ledger
            .get(&tenant(), &build_idem_key("itm-1", Channel::Email))
            .unwrap()
            .redacted,
        "off-cell stays redacted under the real provider"
    );
}

#[test]
fn notif_d9_real_provider_vendor_dedupes_on_idem_key() {
    let transport = RecordingEuTransport::new("eu-mailer");
    let real = EuSovereignAdapter::new(Channel::Email, region(), Arc::new(transport.clone()));
    let idem = build_idem_key("itm-1", Channel::Email);
    let msg = redact_for_offcell(summary(), Class::Direct);

    let r1 = real.try_send(&msg, &idem).unwrap();
    let r2 = real.try_send(&msg, &idem).unwrap();
    assert!(r1.accepted && r2.accepted);
    assert_eq!(
        real.provider_ref_for(&idem),
        Some(format!("eu-mailer:{idem}")),
        "the SAME stable provider_ref for both submits (vendor de-dupe)"
    );
    assert_eq!(
        transport.submit_count(&idem),
        1,
        "the vendor was asked to submit exactly once across the racing retries (NOTIF-D9)"
    );
}

#[test]
fn notif_d9_real_provider_offcell_payload_is_purgeable_via_the_erasure_hook() {
    let transport = RecordingEuTransport::new("eu-mailer");
    let real = EuSovereignAdapter::new(Channel::Email, region(), Arc::new(transport.clone()));
    let idem = build_idem_key("itm-1", Channel::Email);
    real.try_send(&redact_for_offcell(summary(), Class::Direct), &idem)
        .unwrap();
    let provider_ref = real.provider_ref_for(&idem).unwrap();
    assert!(!transport.was_erased(&provider_ref));

    let outcome = real.request_provider_erasure(&idem).unwrap();
    assert_eq!(
        outcome,
        ProviderErasureOutcome::Requested {
            provider_ref: provider_ref.clone()
        }
    );
    assert!(
        transport.was_erased(&provider_ref),
        "the sub-processor was asked to purge the already-sent off-cell payload (NOTIF-P27 hook)"
    );
}

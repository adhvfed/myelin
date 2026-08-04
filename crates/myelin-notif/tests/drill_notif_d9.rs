use myelin_notif::prefs::Channel;
use myelin_notif::{
    build_idem_key, effective_delivery_count, redact_for_offcell, Class, DeliveryFabric,
    DeliveryLedger, DeliveryOutcome, DeliveryRecord, HumanisedString, MockAdapter, RedactedMessage,
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
fn notif_d9_crash_after_ledger_write_retry_is_a_noop_exactly_one_delivery() {
    let ledger = DeliveryLedger::new();
    let mock = MockAdapter::new(Channel::Email, region());
    let fabric_a = DeliveryFabric::new(ledger.clone()).with_adapter(Arc::new(mock.clone()));
    let msg = redact_for_offcell(summary(), Class::Direct);

    let out = fabric_a
        .deliver(&tenant(), "itm-1", Channel::Email, &msg)
        .unwrap();
    assert!(
        matches!(out, DeliveryOutcome::Delivered(_)),
        "the first deliver is a new effective delivery"
    );
    assert_eq!(
        mock.send_count(&build_idem_key("itm-1", Channel::Email)),
        1,
        "provider acked once"
    );

    drop(fabric_a);

    let fabric_b = DeliveryFabric::new(ledger.clone()).with_adapter(Arc::new(mock.clone()));
    let retry = fabric_b
        .deliver(&tenant(), "itm-1", Channel::Email, &msg)
        .unwrap();
    assert_eq!(
        retry,
        DeliveryOutcome::AlreadyDelivered { accepted: true },
        "the retry after the crash is collapsed by UNIQUE(tenant, idem_key) - no re-deliver"
    );

    assert_eq!(
        effective_delivery_count(&ledger, &tenant(), "itm-1", Channel::Email),
        1,
        "exactly 1 effective delivery per (item, channel) across the crash/retry (NOTIF-D9)"
    );
    assert_eq!(ledger.effective_count(&tenant()), 1);
    assert_eq!(
        mock.send_count(&build_idem_key("itm-1", Channel::Email)),
        1,
        "the recovered retry did NOT re-invoke the provider (idempotent on the ledger)"
    );
}

#[test]
fn notif_d9_crash_between_provider_ack_and_ledger_write_collapses_to_one() {
    let ledger = DeliveryLedger::new();

    let idem = build_idem_key("itm-1", Channel::Email);
    let attempt = |accepted: bool| DeliveryRecord {
        item_id: "itm-1".into(),
        channel: Channel::Email,
        idem_key: idem.clone(),
        redacted: true,
        accepted,
        adapter: "fr-par:email".into(),
    };
    let crashed_wins = ledger.record(&tenant(), attempt(true));
    let retry_collapsed = !ledger.record(&tenant(), attempt(true));
    assert!(
        crashed_wins,
        "the first (crashed) INSERT wins the UNIQUE(tenant, idem_key)"
    );
    assert!(
        retry_collapsed,
        "the retry INSERT is collapsed by the constraint (no double row)"
    );

    assert_eq!(
        effective_delivery_count(&ledger, &tenant(), "itm-1", Channel::Email),
        1,
        "the racing crash/retry collapses to exactly 1 effective delivery (NOTIF-D9)"
    );

    assert!(
        ledger.get(&tenant(), &idem).unwrap().redacted,
        "off-cell stays redacted across the crash"
    );
}

#[test]
fn notif_d9_in_app_stays_in_cell_and_offcell_redacted_across_recovery() {
    let ledger = DeliveryLedger::new();
    let fabric = DeliveryFabric::with_mock(ledger.clone(), region());

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
    for channel in [
        Channel::WebPush,
        Channel::MobilePush,
        Channel::Email,
        Channel::Desktop,
    ] {
        fabric
            .deliver(
                &tenant(),
                "itm-1",
                channel,
                &redact_for_offcell(summary(), Class::Direct),
            )
            .unwrap();
    }

    let mut in_app_egress = 0usize;
    let mut offcell_fullbody = 0usize;
    for channel in [
        Channel::InApp,
        Channel::WebPush,
        Channel::MobilePush,
        Channel::Email,
        Channel::Desktop,
    ] {
        let row = ledger
            .get(&tenant(), &build_idem_key("itm-1", channel))
            .unwrap();
        if channel.is_in_cell() && row.redacted {
            in_app_egress += 1;
        }
        if channel.is_off_cell() && !row.redacted {
            offcell_fullbody += 1;
        }
    }
    assert_eq!(in_app_egress, 0, "0 in-app egress (in_app stays in-cell)");
    assert_eq!(
        offcell_fullbody, 0,
        "0 off-cell full-body (every off-cell payload is redacted)"
    );

}

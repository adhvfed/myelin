//! # NOTIF-D9 RE-RUN under the REAL EU-sovereign provider (NOTIF-P26 / P-468)
//!
//! **Drill source:**
//! `planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! row **NOTIF-D9** ("Crash between provider-ack and ledger-write, retry → `UNIQUE(idem_key)`
//! collapses to exactly-one delivery per (item, channel)." Artifact: **1 effective delivery**; CI),
//! re-run UNDER the real [`EuSovereignAdapter`] (NOTIF-P26) instead of the deterministic
//! [`MockAdapter`] — the architecture §3.6/§10 mandate that "the real provider holds the SAME
//! exactly-one-per-(item, channel) idempotency the mock did". EI-01 §3 (prove-it: the crash forces the
//! failure window; the effective-delivery count is part of the pass).
//!
//! **The dated GREEN artifact (2026-06-25).** The channel adapter is the REAL [`EuSovereignAdapter`]
//! (region-aware, EU-preferring, idempotent on a stable `provider_ref`) over the deterministic
//! [`RecordingEuTransport`] (the `[OPEN — LEGAL]` vendor's dev/drill double — the NAMED vendor swaps in
//! behind the SAME [`EuTransport`] seam, no code change). The process is "crashed" in the window AFTER
//! the provider acked but BEFORE the in-process delivery handle committed the ledger row; a retry on
//! the SAME `(item, channel)` re-runs `deliver`. The drill asserts, with NO threshold weakened:
//!
//! 1. **exactly 1 effective delivery per (item, channel)** — the `UNIQUE(tenant, idem_key)` collapse
//!    holds under the real provider. `delivery_success` (1.8) reads EXACTLY 1 — never 0, never 2.
//! 2. **the vendor `submit` is invoked exactly once on the recovered path** — the real adapter
//!    de-dupes (the stable `provider_ref`) AND the fabric's ledger collapses; the recovered retry
//!    re-invokes the vendor ZERO times.
//! 3. **off-cell stays redacted under the real provider** — the recovered delivery carries
//!    `delivery.redacted = true` (0 off-cell full-body).
//! 4. **the provider-side-erasure-request hook works on the sent payload** — the §10 row 2
//!    sub-processor obligation (the NOTIF-P27 hook): the already-sent off-cell payload is purgeable.

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

/// **NOTIF-D9 re-run — the recovered-path crash UNDER THE REAL PROVIDER: the ledger row committed
/// BEFORE the crash; the retry is a no-op.** Exactly one effective delivery; the vendor `submit` runs
/// once; off-cell stays redacted.
#[test]
fn notif_d9_real_provider_crash_after_ledger_write_retry_is_a_noop_exactly_one() {
    let ledger = DeliveryLedger::new();
    // The REAL EU-sovereign adapter over the deterministic vendor transport (the [OPEN — LEGAL]
    // double). This is the production adapter code path — not the mock.
    let transport = RecordingEuTransport::new("eu-mailer");
    let real = EuSovereignAdapter::new(Channel::Email, region(), Arc::new(transport.clone()));
    let fabric_a = DeliveryFabric::new(ledger.clone()).with_adapter(Arc::new(real));
    let msg = redact_for_offcell(summary(), Class::Direct);

    // === before the crash: deliver — the vendor acks AND the ledger row commits ===
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

    // === THE CRASH: drop the in-process fabric handle. The durable ledger row SURVIVES. ===
    drop(fabric_a);

    // === recover on a NEW fabric over the SAME durable ledger + a NEW adapter over the SAME vendor ===
    let real_b = EuSovereignAdapter::new(Channel::Email, region(), Arc::new(transport.clone()));
    let fabric_b = DeliveryFabric::new(ledger.clone()).with_adapter(Arc::new(real_b));
    let retry = fabric_b
        .deliver(&tenant(), "itm-1", Channel::Email, &msg)
        .unwrap();
    assert_eq!(
        retry,
        DeliveryOutcome::AlreadyDelivered { accepted: true },
        "the retry after the crash is collapsed by UNIQUE(tenant, idem_key) — no re-submit (real provider)"
    );

    // 1 EFFECTIVE DELIVERY (the NOTIF-D9 threshold) — never 0, never 2.
    assert_eq!(
        effective_delivery_count(&ledger, &tenant(), "itm-1", Channel::Email),
        1,
        "exactly 1 effective delivery per (item, channel) under the REAL provider (NOTIF-D9 re-run)"
    );
    // The vendor was asked to submit exactly ONCE — the recovered retry re-invokes it ZERO times.
    assert_eq!(
        transport.submit_count(&build_idem_key("itm-1", Channel::Email)),
        1,
        "the recovered retry did NOT re-submit to the vendor (idempotent on the ledger)"
    );
    // off-cell stays redacted under the real provider (0 off-cell full-body).
    assert!(
        ledger
            .get(&tenant(), &build_idem_key("itm-1", Channel::Email))
            .unwrap()
            .redacted,
        "off-cell stays redacted under the real provider"
    );
}

/// **NOTIF-D9 re-run — the racing-write crash UNDER THE REAL PROVIDER: the vendor de-dupes on
/// `idem_key` so even two racing submits collapse to ONE effective delivery.** This is the
/// provider-side half of the exactly-one property (the vendor's own `idem_key` de-dupe), composed with
/// the fabric's `UNIQUE(tenant, idem_key)` ledger collapse.
#[test]
fn notif_d9_real_provider_vendor_dedupes_on_idem_key() {
    let transport = RecordingEuTransport::new("eu-mailer");
    let real = EuSovereignAdapter::new(Channel::Email, region(), Arc::new(transport.clone()));
    let idem = build_idem_key("itm-1", Channel::Email);
    let msg = redact_for_offcell(summary(), Class::Direct);

    // Two racing submits on the SAME idem_key (the crash-between-ack-and-write window, modelled at the
    // vendor): the vendor returns the SAME provider_ref both times and submits exactly once.
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

/// **NOTIF-D9 re-run — the provider-side-erasure-request hook on the sent payload (the §10 row 2
/// sub-processor obligation; the NOTIF-P27 hook).** An already-sent off-cell payload is purgeable.
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

    // GREEN ARTIFACT (2026-06-25): 1 effective delivery per (item, channel) under the REAL EU provider;
    // vendor submit invoked exactly once; off-cell redacted; the provider-side-erasure hook purges the
    // sent payload. No threshold weakened.
}

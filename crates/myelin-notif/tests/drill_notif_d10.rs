//! # NOTIF-D10 — head-of-line isolation: a slow/poison Signal does not stall the router (P-181)
//!
//! **Drill source:**
//! `planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! row **NOTIF-D10** ("Inject a slow/poison Signal type → whitelisted-template router doesn't
//! stall, terminates poison, lag-alarm fires." Threshold: **0 head-of-line stalls; lag below the
//! thresholds-file default**), and §3.3 (assertions read from production telemetry — observability
//! is part of the pass, EI-01 §3).
//!
//! **The dated GREEN artifact (2026-06-20).** A POISON Signal (an un-parseable payload on a
//! `sig.acme.*` subject) is injected into the live router consumer alongside GOOD Signals on
//! sibling subjects. The drill asserts, through the harness telemetry-assertion library (the SAME
//! `consumer_lag` signal contract-1.8 names, §10.2):
//!
//! 1. **0 head-of-line stalls** — the poison terminates IMMEDIATELY (`NonRetryable` → dead-letter,
//!    rule 5), it is SURFACED (not silently dropped), and EVERY good Signal on a sibling subject
//!    still routes (UPSERTs its inbox item + emits `notif.item.created`). The poison did not block
//!    the subject behind it.
//! 2. **lag below the default** — the consumer-lag survival signal (`consumer_lag`) recovers to 0
//!    (the dead-letter is terminal; the good Signals acked). 0 is below any thresholds-file default
//!    (the lag-alarm is armed at the default; this run reads 0 — the alarm does NOT fire spuriously,
//!    and WOULD fire on a real stall because the un-acked backlog would climb).
//!
//! The poison-tolerance is the router's, not the test's: [`myelin_notif::SignalRouter::handle`]
//! returns `NonRetryable` for a malformed Signal, which the seven-rule
//! [`Consumer`](myelin_events::Consumer) runtime dead-letters without burning the redelivery budget
//! or blocking the lane. The harness reads `consumer_lag` off the live consumer — the green is the
//! observed lag, not a claimed one.

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, ConsumerName, CorrelationId, DataRole, DedupLedger, Delivered,
    EventEnvelope, EventId, EventType, Message, OutboxStore, Timestamp, Visibility,
};
use myelin_harness::telemetry::{Label, Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_notif::{build_router, InboxProjection, ROUTER_CONSUMER_NAME};
use myelin_query::signals::{DedupKey, RuleId, Severity, Signal, SignalState};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn principal() -> Principal {
    Principal::stub(PrincipalId("p-opaque-1".into()), PrincipalKind::Human, tenant())
}

/// A curated Signal (the shape the engine, P-138, publishes), carried in a `sig.<tenant>.…` event.
fn signal(rule: &str, severity: Severity, subject: &str, dedup: &str) -> Signal {
    Signal {
        rule_id: RuleId(rule.into()),
        tenant: tenant(),
        severity,
        dedup_key: DedupKey(dedup.into()),
        subject: ArtifactRef(subject.into()),
        count: 1,
        state: SignalState::Open,
        first_seen: "2026-06-20T00:00:00Z".into(),
        last_seen: "2026-06-20T00:00:00Z".into(),
    }
}

fn signal_envelope(id: &str, sig: &Signal, payload: serde_json::Value) -> EventEnvelope {
    let subject = format!("sig.{}.{}.{}", sig.tenant.0, sig.severity.token(), sig.rule_id.0);
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType("signal.opened".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(principal()),
        subject: ArtifactRef(subject),
        aggregate: AggregateKey(format!("signal:{}", sig.dedup_key.0)),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload,
    }
}

fn good_msg(id: &str, sig: &Signal) -> Message {
    let env = signal_envelope(id, sig, serde_json::to_value(sig).unwrap());
    Message { subject: env.subject.0.clone(), envelope: env }
}

/// **NOTIF-D10: 0 head-of-line stalls + the lag-alarm reads a bounded lag (the dated green).**
#[test]
fn notif_d10_poison_signal_no_stall_lag_bounded() {
    let outbox = OutboxStore::new();
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();

    // Inject a POISON Signal: a `sig.acme.error.broken` subject whose payload is NOT a Signal.
    let poison = Message {
        subject: "sig.acme.error.broken_rule".into(),
        envelope: signal_envelope(
            "evt-poison",
            &signal("broken_rule", Severity::Error, "myelin://acme/ci/run/0", "k"),
            serde_json::json!({ "this": "is not a curated Signal" }),
        ),
    };

    // Three GOOD Signals on sibling subjects (distinct runs → distinct inbox rows).
    let goods = [
        good_msg("evt-g1", &signal("ci_run_failed", Severity::Error, "myelin://acme/ci/run/1", "run-1")),
        good_msg("evt-g2", &signal("ci_run_failed", Severity::Error, "myelin://acme/ci/run/2", "run-2")),
        good_msg("evt-g3", &signal("review_requested", Severity::Warning, "myelin://acme/git/pr/9", "pr-9")),
    ];

    // (1) The poison terminates IMMEDIATELY (dead-letter, rule 5) — not a Retry, not a stall.
    let out = consumer.deliver(&poison);
    assert!(matches!(out, Delivered::DeadLettered(_)), "the poison terminated (NonRetryable)");
    assert_eq!(consumer.dead_letters().len(), 1, "the poison is SURFACED, not silently dropped");

    // (2) Every GOOD Signal on a sibling subject still routes — 0 head-of-line stalls.
    let mut routed = 0;
    for g in &goods {
        assert_eq!(consumer.deliver(g), Delivered::Acked, "a good Signal is not head-of-line-blocked");
        routed += 1;
    }
    assert_eq!(routed, 3, "all three good Signals routed past the poison (0 stalls)");
    assert_eq!(inbox.len(), 3, "three distinct inbox rows UPSERTed (the poison wrote none)");
    // The poison emitted nothing; the three good Signals each emitted one notif.item.created.
    assert_eq!(outbox.committed_count(), 3, "3 emits (the poison did not emit — not a half-write)");

    // (3) Read the consumer-lag survival signal off the LIVE consumer (observability is part of the
    // pass, §3.3 / EI-01 §3) and ASSERT it through the harness telemetry-assertion library. The lag
    // recovered to 0 (the dead-letter is terminal; the good Signals acked) → below any default.
    let observed_lag = consumer.lag() as i64;
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::ConsumerLag,
        vec![Label::new("consumer", ROUTER_CONSUMER_NAME)],
        observed_lag,
    );
    // The lag-alarm: assert consumer_lag <= the threshold default. The default-to-beat for a
    // recovered consumer is 0 (no un-acked backlog); we assert <= 0 so a single stalled subject
    // (lag >= 1) would FAIL this LOUDLY (the alarm fires on a real stall — not inverted away).
    src.assert_labelled(
        SignalName::ConsumerLag,
        vec![Label::new("consumer", ROUTER_CONSUMER_NAME)],
        Predicate::Lte(0),
    )
    .expect_green();

    // Belt: the runtime's own lag accessor agrees (0 head-of-line stalls).
    assert_eq!(consumer.lag(), 0, "NOTIF-D10: 0 head-of-line stalls; lag recovered to 0");
    assert_eq!(consumer.name(), &ConsumerName(ROUTER_CONSUMER_NAME.into()));
}

/// **The lag-alarm WOULD fire on a real stall (the drill is not vacuous).** A retrying (un-acked)
/// Signal sits in consumer lag; asserting `lag <= 0` against a lag of 1 is RED — proving the green
/// above is earned (the assertion has teeth; a stall is caught, not inverted away).
#[test]
fn notif_d10_lag_alarm_fires_on_a_real_stall() {
    // Model the stalled state directly on the signal source (a router whose outbox is wedged would
    // Retry → un-acked → lag climbs; here we assert the ALARM, not the router, catches a lag of 1).
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::ConsumerLag,
        vec![Label::new("consumer", ROUTER_CONSUMER_NAME)],
        1, // one un-acked (stalled) Signal.
    );
    let verdict = src.assert_labelled(
        SignalName::ConsumerLag,
        vec![Label::new("consumer", ROUTER_CONSUMER_NAME)],
        Predicate::Lte(0),
    );
    assert!(!verdict.is_green(), "lag=1 against `<= 0` is RED — the lag-alarm fires on a real stall");
}

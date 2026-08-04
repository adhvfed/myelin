use myelin_events::{
    Actor, AggregateKey, ArtifactRef, ConsumerName, CorrelationId, DataRole, DedupLedger,
    Delivered, EventEnvelope, EventId, EventType, Message, OutboxStore, Timestamp, Visibility,
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
    Principal::stub(
        PrincipalId("p-opaque-1".into()),
        PrincipalKind::Human,
        tenant(),
    )
}

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
    let subject = format!(
        "sig.{}.{}.{}",
        sig.tenant.0,
        sig.severity.token(),
        sig.rule_id.0
    );
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
    Message {
        subject: env.subject.0.clone(),
        envelope: env,
    }
}

#[test]
fn notif_d10_poison_signal_no_stall_lag_bounded() {
    let outbox = OutboxStore::new();
    let inbox = InboxProjection::new();
    let consumer =
        build_router(&tenant(), inbox.clone(), outbox.clone(), DedupLedger::new()).unwrap();

    let poison = Message {
        subject: "sig.acme.error.broken_rule".into(),
        envelope: signal_envelope(
            "evt-poison",
            &signal(
                "broken_rule",
                Severity::Error,
                "myelin://acme/ci/run/0",
                "k",
            ),
            serde_json::json!({ "this": "is not a curated Signal" }),
        ),
    };

    let goods = [
        good_msg(
            "evt-g1",
            &signal(
                "ci_run_failed",
                Severity::Error,
                "myelin://acme/ci/run/1",
                "run-1",
            ),
        ),
        good_msg(
            "evt-g2",
            &signal(
                "ci_run_failed",
                Severity::Error,
                "myelin://acme/ci/run/2",
                "run-2",
            ),
        ),
        good_msg(
            "evt-g3",
            &signal(
                "review_requested",
                Severity::Warning,
                "myelin://acme/git/pr/9",
                "pr-9",
            ),
        ),
    ];

    let out = consumer.deliver(&poison);
    assert!(
        matches!(out, Delivered::DeadLettered(_)),
        "the poison terminated (NonRetryable)"
    );
    assert_eq!(
        consumer.dead_letters().len(),
        1,
        "the poison is SURFACED, not silently dropped"
    );

    let mut routed = 0;
    for g in &goods {
        assert_eq!(
            consumer.deliver(g),
            Delivered::Acked,
            "a good Signal is not head-of-line-blocked"
        );
        routed += 1;
    }
    assert_eq!(
        routed, 3,
        "all three good Signals routed past the poison (0 stalls)"
    );
    assert_eq!(
        inbox.len(),
        3,
        "three distinct inbox rows UPSERTed (the poison wrote none)"
    );
    assert_eq!(
        outbox.committed_count(),
        3,
        "3 emits (the poison did not emit - not a half-write)"
    );

    let observed_lag = consumer.lag() as i64;
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::ConsumerLag,
        vec![Label::new("consumer", ROUTER_CONSUMER_NAME)],
        observed_lag,
    );
    src.assert_labelled(
        SignalName::ConsumerLag,
        vec![Label::new("consumer", ROUTER_CONSUMER_NAME)],
        Predicate::Lte(0),
    )
    .expect_green();

    assert_eq!(
        consumer.lag(),
        0,
        "NOTIF-D10: 0 head-of-line stalls; lag recovered to 0"
    );
    assert_eq!(consumer.name(), &ConsumerName(ROUTER_CONSUMER_NAME.into()));
}

#[test]
fn notif_d10_lag_alarm_fires_on_a_real_stall() {
    let mut src = SignalSource::new();
    src.set_labelled(
        SignalName::ConsumerLag,
        vec![Label::new("consumer", ROUTER_CONSUMER_NAME)],
        1,
    );
    let verdict = src.assert_labelled(
        SignalName::ConsumerLag,
        vec![Label::new("consumer", ROUTER_CONSUMER_NAME)],
        Predicate::Lte(0),
    );
    assert!(
        !verdict.is_green(),
        "lag=1 against `<= 0` is RED - the lag-alarm fires on a real stall"
    );
}

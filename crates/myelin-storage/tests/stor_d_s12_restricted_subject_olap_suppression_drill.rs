use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Region, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_storage::{AnalyticsAggregate, OlapAnalytics, OlapBusConsumer, RestrictionGateSignal};
use myelin_tenancy::TenantId;

fn region() -> Region {
    Region("fr-par".into())
}

fn tenant() -> TenantId {
    TenantId("01J0ACME".into())
}

fn envelope(event_id: &str, aggregate: &str, subject: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: EventType("issues.issue.created".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            tenant(),
        )),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey(aggregate.into()),
        causation_id: None,
        correlation_id: CorrelationId("root".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:01Z".into()),
        payload: serde_json::json!({ "ref": aggregate }),
    }
}

fn fed_consumer() -> OlapBusConsumer {
    let mut consumer = OlapBusConsumer::boot(region());
    for (id, agg, subj) in [
        ("e1", "issue:PROJ-1", "subj:alice"),
        ("e2", "issue:PROJ-2", "subj:bob"),
        ("e3", "issue:PROJ-3", "subj:alice"),
    ] {
        consumer
            .ingest(&envelope(id, agg, subj))
            .expect("an in-region bus event is admitted");
    }
    consumer
}

#[test]
fn d_s12_restricted_subject_absent_from_every_aggregate_zero_leak() {
    let mut consumer = fed_consumer();
    assert_eq!(
        OlapAnalytics::over(consumer.store()).velocity(),
        3,
        "all three items contribute before restriction"
    );

    consumer.store_mut().set_restricted("subj:alice", true);

    let analytics = OlapAnalytics::over(consumer.store());

    let cfd = analytics.cfd();
    assert_eq!(cfd.len(), 1, "CFD: only bob's item survives");
    assert!(cfd.contains_key("issue:PROJ-2"), "bob's item present");
    assert!(!cfd.contains_key("issue:PROJ-1"), "alice excluded from CFD");
    assert!(!cfd.contains_key("issue:PROJ-3"), "alice excluded from CFD");

    assert_eq!(
        analytics.cycle_time_sample_size(),
        1,
        "alice out of cycle-time"
    );
    assert_eq!(analytics.velocity(), 1, "alice out of velocity");
    assert_eq!(
        analytics.delivery_health_wip(),
        1,
        "alice out of delivery-health"
    );

    let audit = analytics.leak_audit();
    assert_eq!(
        audit.olap_restricted_subject_leak, 0,
        "GATE: olap_restricted_subject_leak == 0 (a restricted subject leaked into analytics is a §3.4 breach)"
    );
    assert_eq!(
        audit.per_aggregate.len(),
        AnalyticsAggregate::ALL.len(),
        "the gate ran over every C5 aggregate"
    );
    assert!(audit.leaked_subjects.is_empty(), "no leaked subjects");

    let signal = RestrictionGateSignal::from_audit("issue_analytics_olap", &audit, 1);
    assert!(
        signal.is_green(),
        "D-S12 green: olap_restricted_subject_leak == 0, all four aggregates, ≥1 restricted: {signal:?}"
    );
}

#[test]
fn d_s12_restriction_lifts_subject_reappears_no_reindex() {
    let mut consumer = fed_consumer();
    consumer.store_mut().set_restricted("subj:alice", true);
    assert_eq!(OlapAnalytics::over(consumer.store()).velocity(), 1);

    consumer.store_mut().set_restricted("subj:alice", false);
    assert_eq!(
        OlapAnalytics::over(consumer.store()).velocity(),
        3,
        "alice reappears the instant restriction lifts - filter-at-query-time, no reindex"
    );
}

#[test]
fn d_s12_gate_is_non_vacuous_and_leak_is_measured() {
    let consumer = fed_consumer();
    let analytics = OlapAnalytics::over(consumer.store());
    let audit = analytics.leak_audit();
    let vacuous = RestrictionGateSignal::from_audit("issue_analytics_olap", &audit, 0);
    assert!(
        !vacuous.is_green(),
        "a run that restricted 0 subjects proves nothing - the gate reads RED"
    );
    assert_eq!(
        analytics.contributing_subjects().len(),
        2,
        "alice + bob contribute"
    );
}

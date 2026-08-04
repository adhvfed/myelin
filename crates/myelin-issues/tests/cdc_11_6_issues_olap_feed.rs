use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventHandler,
    EventId, EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_issues::events;
use myelin_issues::{IssueOlapConsumer, RestrictionFlag};
use myelin_storage::olap::{OlapApply, OlapEvent, OlapReadStore};
use myelin_tenancy::{Region, TenantId};

fn region() -> Region {
    Region("fr-par".into())
}

fn ev(id: &str, type_token: &str, subject: &str, aggregate: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(id.into()),
        type_: EventType(type_token.into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId(subject.into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        subject: ArtifactRef(subject.into()),
        aggregate: AggregateKey(aggregate.into()),
        causation_id: None,
        correlation_id: CorrelationId(id.into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-23T10:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T10:00:01Z".into()),
        payload: serde_json::json!({}),
    }
}

#[test]
fn provider_olap_read_store_ingests_from_envelope() {
    let mut store = OlapReadStore::pinned_to(region());
    let envelope = ev(
        "01J-1",
        events::SLA_MET,
        "myelin://acme/issue/issue/ENG-1",
        "issue:ENG-1",
    );
    let olap_event = OlapEvent::from_envelope(&envelope);
    assert_eq!(olap_event.event_id, "01J-1");
    assert_eq!(olap_event.region, region());
    assert_eq!(olap_event.aggregate_row, "issue:ENG-1");
    assert_eq!(store.apply(&olap_event).unwrap(), OlapApply::Fresh);
    assert_eq!(
        store.apply(&olap_event).unwrap(),
        OlapApply::Duplicate,
        "redelivery is a no-op"
    );
    assert_eq!(store.doc_count(), 1);
}

#[test]
fn consumer_issue_feed_drives_the_shared_frame() {
    let c = IssueOlapConsumer::new(region(), RestrictionFlag::new());
    let subjects: Vec<String> = c.subjects().iter().map(|s| s.0.clone()).collect();
    assert!(subjects.contains(&events::SLA_MET.to_string()));
    assert!(subjects.contains(&events::ISSUE_TRANSITIONED.to_string()));
    assert!(subjects.contains(&events::CYCLE_COMPLETED.to_string()));
    assert!(subjects.iter().all(|s| s != "*"), "never `*` (BUS-3)");
    c.handle(&ev(
        "01J-1",
        events::SLA_MET,
        "myelin://acme/issue/issue/ENG-1",
        "issue:ENG-1",
    ), &mut myelin_events::HandlerTx::none());
    assert_eq!(
        c.doc_count(),
        1,
        "the consumer projected one analytics doc into the shared frame"
    );
    assert_eq!(c.oltp_read_count(), 0);
}

#[test]
fn consumer_honours_the_restriction_flag() {
    let flag = RestrictionFlag::new();
    let c = IssueOlapConsumer::new(region(), flag.clone());
    c.handle(&ev("a1", events::SLA_MET, "psn:alice", "issue:A"), &mut myelin_events::HandlerTx::none());
    c.handle(&ev("b1", events::SLA_MET, "psn:bob", "issue:B"), &mut myelin_events::HandlerTx::none());
    c.analytics(|a| assert_eq!(a.velocity(), 2, "both contribute unrestricted"));
    flag.set("psn:alice", true);
    c.analytics(|a| {
        assert_eq!(a.velocity(), 1, "alice excluded → only bob");
        assert_eq!(
            a.leak_audit().restricted_subject_leak,
            0,
            "the restriction is honoured (0 leak)"
        );
    });
}

#[test]
fn consumer_and_provider_project_the_same_read_model() {
    let c = IssueOlapConsumer::new(region(), RestrictionFlag::new());
    c.handle(&ev("e1", events::SLA_MET, "psn:a", "issue:1"), &mut myelin_events::HandlerTx::none());
    c.handle(&ev("e2", events::ISSUE_TRANSITIONED, "psn:b", "issue:2"), &mut myelin_events::HandlerTx::none());

    let mut raw = OlapReadStore::pinned_to(region());
    raw.apply(&OlapEvent::from_envelope(&ev(
        "e1",
        events::SLA_MET,
        "psn:a",
        "issue:1",
    )))
    .unwrap();
    raw.apply(&OlapEvent::from_envelope(&ev(
        "e2",
        events::ISSUE_TRANSITIONED,
        "psn:b",
        "issue:2",
    )))
    .unwrap();

    assert_eq!(
        c.parity_bytes(),
        raw.parity_bytes(),
        "the Issues consumer feeds the SAME frozen frame projection - one OLAP model, no fork"
    );
}

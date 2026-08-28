use myelin_events::{
    consume, Actor, AggregateKey, ArtifactRef, ConsumerName, ConsumerSpec, CorrelationId, DataRole,
    DedupLedger, Delivered, EventEnvelope, EventHandler, EventId, EventType, HandleOutcome,
    Message, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_issues::events::ISSUE_UPDATED;
use myelin_issues::projection_feeder::{CollectionKey, FacetKey, ProjectionFeeder};
use myelin_tenancy::{Region, TenantId};

const BUG_TYPE_ID: &str = "22222222-2222-2222-2222-222222222222";

fn principal() -> Principal {
    Principal::stub(
        PrincipalId("p".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

fn updated_event(event_id: &str, type_id: &str, changed_facets: &[&str]) -> EventEnvelope {
    let issue = "myelin://acme/issue/issue/ENG-1";
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: EventType(ISSUE_UPDATED.into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("eu-west".into()),
        actor: Actor(principal()),
        subject: ArtifactRef(issue.into()),
        aggregate: AggregateKey("issue:ENG-1".into()),
        causation_id: None,
        correlation_id: CorrelationId("root".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-21T10:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T10:00:01Z".into()),
        payload: serde_json::json!({
            "issue": issue,
            "issue_local_id": "ENG-1",
            "type_id": type_id,
            "changed_facets": changed_facets,
        }),
    }
}

#[test]
fn producer_issues_authors_a_star_free_issue_updated_handler() {
    let feeder = ProjectionFeeder::new();
    let subjects = feeder.subjects();
    assert_eq!(subjects.len(), 1, "the feeder binds exactly one subject");
    assert_eq!(subjects[0].0, ISSUE_UPDATED);
    assert_eq!(subjects[0].0, "issue.issue.updated");
    assert!(
        subjects.iter().all(|s| s.0 != "*" && s.0 != ">"),
        "the feeder NEVER binds a wildcard subscription (BUS-3)"
    );
    let outcome = feeder.handle(
        &updated_event("p-1", BUG_TYPE_ID, &["severity"]),
        &mut myelin_events::HandlerTx::none(),
    );
    assert_eq!(outcome, HandleOutcome::Done);
}

#[test]
fn consumer_bus_admits_and_drives_the_feeder() {
    let feeder = ProjectionFeeder::new();
    let spec = ConsumerSpec::new(
        ConsumerName("issues.projection_feeder".into()),
        &[ISSUE_UPDATED],
    );
    let consumer =
        consume(spec, feeder, DedupLedger::new()).expect("the *-free feeder whitelist must bind");

    let msg = Message {
        subject: ISSUE_UPDATED.into(),
        envelope: updated_event("c-1", BUG_TYPE_ID, &["severity"]),
    };
    assert_eq!(
        consumer.deliver(&msg),
        Delivered::Acked,
        "a well-formed issue.updated acks (Done)"
    );
    assert_eq!(consumer.deliver(&msg), Delivered::Deduplicated);
    assert_eq!(consumer.lag(), 0, "no backlog after a clean ack + dedup");
}

#[test]
fn consumer_drives_the_measured_promotion_through_the_runtime() {
    let feeder = ProjectionFeeder::new();
    let coll = CollectionKey::new("acme", BUG_TYPE_ID);
    for _ in 0..20 {
        feeder.record_view_execution(&coll, &["severity"]);
    }
    for _ in 0..80 {
        feeder.record_view_execution(&coll, &[]);
    }
    let spec = ConsumerSpec::new(
        ConsumerName("issues.projection_feeder".into()),
        &[ISSUE_UPDATED],
    );
    let consumer = consume(spec, feeder, DedupLedger::new()).expect("binds");

    let msg = Message {
        subject: ISSUE_UPDATED.into(),
        envelope: updated_event("c-2", BUG_TYPE_ID, &["severity"]),
    };
    assert_eq!(consumer.deliver(&msg), Delivered::Acked);
    let facet = FacetKey::new("acme", BUG_TYPE_ID, "severity");
    assert!(
        consumer.handler().is_promoted(&facet),
        "the hot facet is promoted off the bus (Tier 2) once its issue.updated delivers"
    );
    assert_eq!(
        consumer.handler().catalog_snapshot().posture("severity"),
        myelin_issues::IndexPosture::GeneratedIndex
    );
}

#[test]
fn a_wildcard_subscription_is_rejected() {
    let feeder = ProjectionFeeder::new();
    let spec = ConsumerSpec::new(ConsumerName("issues.bad".into()), &["*"]);
    assert!(
        consume(spec, feeder, DedupLedger::new()).is_err(),
        "a `*` subscription must be rejected at registration (BUS-3)"
    );
}

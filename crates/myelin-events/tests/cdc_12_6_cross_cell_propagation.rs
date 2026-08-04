use myelin_events::{
    assert_cell_agnostic, pointer_for_propagation, Actor, AggregateKey, ArtifactRef, ArtifactType,
    CellId, CorrelationId, CrossCellPointer, CrossCellPropagator, CrossCellStream, DataRole,
    EventEnvelope, EventId, EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

fn envelope() -> EventEnvelope {
    EventEnvelope {
        event_id: EventId("01J0EVT".into()),
        type_: EventType("issues.issue.created".into()),
        schema_ver: 1,
        tenant: TenantId("acme".into()),
        region: Region("fr-par".into()),
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )),
        subject: ArtifactRef("myelin://01J0ACME/issues/issue/42".into()),
        aggregate: AggregateKey("issue:PROJ-1".into()),
        causation_id: None,
        correlation_id: CorrelationId("01J0CHAIN".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: true,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-24T00:00:01Z".into()),
        payload: serde_json::json!({ "assignee_email": "alice@example.com" }),
    }
}

#[test]
fn provider_propagator_emits_the_canonical_four_field_frame_no_payload() {
    let pointer = pointer_for_propagation(
        &envelope(),
        CrossCellStream::IssuePortfolio,
        CellId::from_token("cell-a"),
    );
    let json = serde_json::to_value(&pointer).expect("provider emits canonical frame");
    let obj = json.as_object().expect("frame is a JSON object");

    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["correlation_id", "home_cell", "subject", "type"],
        "the propagator emits EXACTLY the four §6.1 fields - no payload/PII/authz state"
    );
    let wire = serde_json::to_string(&pointer).expect("serialises");
    assert!(
        !wire.contains("alice@example.com"),
        "the payload PII never crosses: {wire}"
    );
    assert!(
        !wire.contains("payload"),
        "there is no payload field on the frame: {wire}"
    );
}

#[test]
fn consumer_reads_back_only_the_four_frozen_fields_routes_by_the_opaque_pointer() {
    let provider = CrossCellPropagator::new(CellId::from_token("cell-a"));
    let fanned = provider.fan_out(&envelope(), &[CellId::from_token("cell-b")]);
    assert_eq!(fanned.len(), 1);
    let wire = serde_json::to_string(&fanned[0].pointer).expect("provider emits canonical frame");

    let consumer: CrossCellPointer =
        serde_json::from_str(&wire).expect("consumer reads the canonical frame");

    let routed: &ArtifactRef = assert_cell_agnostic(&consumer);
    assert_eq!(routed.0, "myelin://01J0ACME/issues/issue/42");

    assert_eq!(consumer.artifact_type(), &ArtifactType::Issue);
    assert_eq!(
        consumer.correlation_id(),
        &CorrelationId("01J0CHAIN".into())
    );
    assert_eq!(consumer.home_cell().as_str(), "cell-a");

    assert_eq!(
        consumer, fanned[0].pointer,
        "the CDC wire shape is conformant both ways"
    );
}

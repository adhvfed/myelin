//! CDC pair for contract 12.6 — the Bus's cross-cell EVENT-PROPAGATION half (EB-25 / P-438, M5).
//!
//! EB-14 pinned the four-field [`myelin_events::CrossCellPointer`] frame from the Bus side
//! (`tests/cdc_12_6_crosscell.rs` is the frame's serde-conformance pair). EB-25 BUILDS the
//! event-propagation half: the Bus mints the pointer FROM an [`EventEnvelope`] and fans it out to the
//! tenant's other cells. This CDC pair proves that produced pointer-event conforms to the SAME frozen
//! wire shape the resolution half (control-plane `cross_cell_bridge`, P-429) consumes:
//!
//! - the **provider** (the Bus's propagator) mints a [`PropagatedPointer`] from an envelope and emits
//!   its [`CrossCellPointer`] to the canonical four-field wire shape — the payload is structurally
//!   absent;
//! - the **consumer** (standing in for the control-plane bridge carrying + the home cell resolving)
//!   deserialises that exact wire shape and reads back ONLY the four frozen fields, routing by the
//!   opaque pointer — it cannot reach the originating payload (there is no field for it).
//!
//! If the propagation ever leaked the payload onto the wire (a fifth field), the provider's emitted
//! shape would not be the four-field frame the consumer agrees on, and this pair would stop agreeing.

use myelin_events::{
    assert_cell_agnostic, pointer_for_propagation, Actor, AggregateKey, ArtifactRef, ArtifactType,
    CellId, CorrelationId, CrossCellPointer, CrossCellPropagator, CrossCellStream, DataRole,
    EventEnvelope, EventId, EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

/// A cross-cell-relevant envelope with a PII-bearing payload (the payload MUST NOT cross).
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

/// PROVIDER side: the Bus's propagator mints the cross-cell pointer from an envelope and emits it to
/// its canonical four-field wire shape — EXACTLY `subject`/`type`/`correlation_id`/`home_cell`, no
/// payload.
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
        "the propagator emits EXACTLY the four §6.1 fields — no payload/PII/authz state"
    );
    // The payload PII is structurally absent from the produced pointer.
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

/// CONSUMER side: the control-plane bridge (carrying the pointer + resolving cell-local) deserialises
/// the propagator-emitted wire shape and reads back ONLY the four frozen fields, routing by the
/// opaque pointer (`assert_cell_agnostic`) — it cannot reach the originating payload.
#[test]
fn consumer_reads_back_only_the_four_frozen_fields_routes_by_the_opaque_pointer() {
    let provider = CrossCellPropagator::new(CellId::from_token("cell-a"));
    let fanned = provider.fan_out(&envelope(), &[CellId::from_token("cell-b")]);
    assert_eq!(fanned.len(), 1);
    let wire = serde_json::to_string(&fanned[0].pointer).expect("provider emits canonical frame");

    let consumer: CrossCellPointer =
        serde_json::from_str(&wire).expect("consumer reads the canonical frame");

    // The consumer routes by the OPAQUE subject (cell-agnostic), never a cell-bound row / payload.
    let routed: &ArtifactRef = assert_cell_agnostic(&consumer);
    assert_eq!(routed.0, "myelin://01J0ACME/issues/issue/42");

    assert_eq!(consumer.artifact_type(), &ArtifactType::Issue);
    assert_eq!(
        consumer.correlation_id(),
        &CorrelationId("01J0CHAIN".into())
    );
    assert_eq!(consumer.home_cell().as_str(), "cell-a");

    // The pair agrees both ways — the propagated pointer conforms to the frozen frame.
    assert_eq!(
        consumer, fanned[0].pointer,
        "the CDC wire shape is conformant both ways"
    );
}

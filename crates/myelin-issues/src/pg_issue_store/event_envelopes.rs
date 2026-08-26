use crate::events::{ISSUE_AUTHORIZATION_REQUESTED, ISSUE_CLOSED, ISSUE_CREATED};
use myelin_events::{
    derive_envelope, Actor, AggregateKey, DataRole, EmitContext, EventDraft, EventEnvelope,
    EventId, EventType, Timestamp, Visibility,
};
use myelin_identity::{Principal, Zookie};
use myelin_tenancy::ArtifactRef;
use sqlx::types::Uuid;

use super::RelationRecord;

pub(super) fn issue_relation(
    actor: &Principal,
    record: &RelationRecord,
    event_type: &str,
    event_id: EventId,
    timestamp: Timestamp,
) -> EventEnvelope {
    let source = ArtifactRef(record.source_ref.clone());
    let target = ArtifactRef(record.target_ref.clone());
    derive_envelope(
        EventDraft {
            type_: EventType(event_type.into()),
            subject: source.clone(),
            aggregate: myelin_refs::edge_aggregate_key(&source, &target),
            payload: serde_json::json!({
                "relation_id": record.relation_id.to_string(),
                "source": source.0,
                "target": target.0,
                "rel": record.relation,
                "rel_class": "lifecycle",
            }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        },
        EmitContext {
            event_id,
            tenant: actor.tenant.clone(),
            region: actor.region.clone(),
            actor: Actor(actor.clone()),
            schema_ver: 1,
            occurred_at: timestamp.clone(),
            recorded_at: timestamp,
            caused_by: None,
        },
        None,
    )
}

pub(super) fn authorization_requested(
    actor: &Principal,
    issue_id: Uuid,
    project_id: Uuid,
    issue_object: &str,
    project_userset: &str,
    event_id: EventId,
    timestamp: Timestamp,
) -> EventEnvelope {
    derive_envelope(
        EventDraft {
            type_: EventType(ISSUE_AUTHORIZATION_REQUESTED.into()),
            subject: issue_subject(actor.tenant.as_str(), issue_id),
            aggregate: AggregateKey(format!("issue:{issue_id}")),
            payload: serde_json::json!({
                "issue_id": issue_id.to_string(),
                "project_id": project_id.to_string(),
                "issue_object": issue_object,
                "relation": "parent_project",
                "project_userset": project_userset,
            }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        },
        EmitContext {
            event_id,
            tenant: actor.tenant.clone(),
            region: actor.region.clone(),
            actor: Actor(actor.clone()),
            schema_ver: 1,
            occurred_at: timestamp.clone(),
            recorded_at: timestamp,
            caused_by: None,
        },
        None,
    )
}

pub(super) fn issue_created(
    event_id: EventId,
    issue_id: Uuid,
    key: &str,
    project_id: Uuid,
    zookie: &Zookie,
    request: &EventEnvelope,
    recorded_at: Timestamp,
) -> EventEnvelope {
    derive_envelope(
        EventDraft {
            type_: EventType(ISSUE_CREATED.into()),
            subject: issue_subject(request.tenant.as_str(), issue_id),
            aggregate: AggregateKey(format!("issue:{issue_id}")),
            payload: serde_json::json!({
                "issue_id": issue_id.to_string(),
                "issue_key": key,
                "project_id": project_id.to_string(),
                "authorization_zookie": zookie.0,
            }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        },
        EmitContext {
            event_id,
            tenant: request.tenant.clone(),
            region: request.region.clone(),
            actor: request.actor.clone(),
            schema_ver: 1,
            occurred_at: request.occurred_at.clone(),
            recorded_at,
            caused_by: request.caused_by.clone(),
        },
        Some(request),
    )
}

pub(super) fn issue_closed(
    actor: &Principal,
    issue_id: Uuid,
    key: &str,
    previous_state: &str,
    event_id: EventId,
    timestamp: Timestamp,
) -> EventEnvelope {
    derive_envelope(
        EventDraft {
            type_: EventType(ISSUE_CLOSED.into()),
            subject: issue_subject(actor.tenant.as_str(), issue_id),
            aggregate: AggregateKey(format!("issue:{issue_id}")),
            payload: serde_json::json!({
                "issue_id": issue_id.to_string(),
                "issue_key": key,
                "from": previous_state,
                "to": "Done",
                "category": "completed",
            }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        },
        EmitContext {
            event_id,
            tenant: actor.tenant.clone(),
            region: actor.region.clone(),
            actor: Actor(actor.clone()),
            schema_ver: 1,
            occurred_at: timestamp.clone(),
            recorded_at: timestamp,
            caused_by: None,
        },
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn validate_authorization_request(
    request: &EventEnvelope,
    tenant: &str,
    region: &str,
    issue_id: Uuid,
    project_id: Uuid,
    issue_object: &str,
    project_userset: &str,
    relation: &str,
    request_event_id: &str,
    created_by_principal: &str,
) -> Result<(), String> {
    let expected_subject = issue_subject(tenant, issue_id);
    let expected_aggregate = AggregateKey(format!("issue:{issue_id}"));
    let expected_issue_id = issue_id.to_string();
    let expected_project_id = project_id.to_string();
    let payload = &request.payload;
    let valid = request.type_.0 == ISSUE_AUTHORIZATION_REQUESTED
        && request.event_id.0 == request_event_id
        && request.tenant.as_str() == tenant
        && request.region.as_str() == region
        && request.actor.0.tenant.as_str() == tenant
        && request.actor.0.region.as_str() == region
        && request.actor.0.principal_id.0 == created_by_principal
        && request.subject == expected_subject
        && request.aggregate == expected_aggregate
        && !request.contains_personal_data
        && request.pii_key_ref.is_none()
        && payload.get("issue_id").and_then(serde_json::Value::as_str)
            == Some(expected_issue_id.as_str())
        && payload
            .get("project_id")
            .and_then(serde_json::Value::as_str)
            == Some(expected_project_id.as_str())
        && payload
            .get("issue_object")
            .and_then(serde_json::Value::as_str)
            == Some(issue_object)
        && payload
            .get("project_userset")
            .and_then(serde_json::Value::as_str)
            == Some(project_userset)
        && payload.get("relation").and_then(serde_json::Value::as_str) == Some(relation);
    if valid {
        Ok(())
    } else {
        Err("authorization request envelope does not match staged issue binding".into())
    }
}

fn issue_subject(tenant: &str, issue_id: Uuid) -> ArtifactRef {
    ArtifactRef(format!("myelin://{tenant}/issue/issue/{issue_id}"))
}

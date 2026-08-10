use myelin_events::{ArtifactRef, EventEnvelope};
use myelin_tenancy::TenantId;

use crate::humanise::reason_template_key;
use crate::pg_inbox::{DurableInboxItem, InboxUpsert};
use crate::ranking::reason_base_class;
use crate::router::RoutedInboxItem;
use crate::storm_control::subject_root_of;
use crate::Reason;

const APPROVAL_NAMESPACE: &str = "automation-approval";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutomationApprovalAction {
    pub automation_id: String,
    pub event_id: String,
}

pub fn automation_approval_item_id(
    tenant: &TenantId,
    recipient: &str,
    automation_id: &str,
    event_id: &str,
) -> String {
    let mut material = Vec::new();
    for component in [&tenant.0, recipient, automation_id, event_id] {
        material.extend_from_slice(component.as_bytes());
        material.push(0);
    }
    format!(
        "approval-{}",
        &blake3::hash(&material).to_hex().as_str()[..24]
    )
}

pub fn pending_automation_approval(
    automation_id: &str,
    recipient: &str,
    event: &EventEnvelope,
) -> InboxUpsert {
    let item_id =
        automation_approval_item_id(&event.tenant, recipient, automation_id, &event.event_id.0);
    let reason = Reason::ApprovalRequested;
    let trigger_ref = ArtifactRef(format!(
        "myelin://{}/identity/trigger/{automation_id}",
        event.tenant.0
    ));
    InboxUpsert {
        item: RoutedInboxItem {
            tenant: event.tenant.clone(),
            region: event.region.clone(),
            item_id,
            recipient: recipient.to_string(),
            subject: event.subject.clone(),
            reason,
            class: reason_base_class(reason).1,
            origin_event: ArtifactRef(format!(
                "myelin://{}/bus/event/{}",
                event.tenant.0, event.event_id.0
            )),
            dedup_key: format!("{APPROVAL_NAMESPACE}:{automation_id}:{}", event.event_id.0),
            coalesce_count: 1,
            state: "unread".into(),
            snooze_until: None,
        },
        subject_root: ArtifactRef(subject_root_of(&event.subject.0)),
        template_key: reason_template_key(reason).to_string(),
        template_args: vec![event.subject.clone(), trigger_ref],
        occurred_at: event.occurred_at.0.clone(),
        dek_ref: format!("kms://{}/notif/inbox", event.tenant.0),
    }
}

pub fn automation_approval_action(item: &DurableInboxItem) -> Option<AutomationApprovalAction> {
    if item.item.reason != Reason::ApprovalRequested {
        return None;
    }
    let trigger_prefix = format!("myelin://{}/identity/trigger/", item.item.tenant.0);
    let automation_id = item.template_args.get(1)?.0.strip_prefix(&trigger_prefix)?;
    let event_prefix = format!("myelin://{}/bus/event/", item.item.tenant.0);
    let event_id = item.item.origin_event.0.strip_prefix(&event_prefix)?;
    if automation_id.is_empty() || event_id.is_empty() || automation_id.contains('/') {
        return None;
    }
    Some(AutomationApprovalAction {
        automation_id: automation_id.to_string(),
        event_id: event_id.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, DataRole, EventId, EventType, Region, TenantId,
        Timestamp, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::ArtifactRef;

    fn event() -> EventEnvelope {
        EventEnvelope {
            event_id: EventId("issue-owner-updated-1".into()),
            type_: EventType("issue.issue.updated".into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("eu-north".into()),
            actor: Actor(Principal::stub(
                PrincipalId("issues-service".into()),
                PrincipalKind::Service,
                TenantId("acme".into()),
            )),
            subject: ArtifactRef("myelin://acme/issue/issue/ENG-41".into()),
            aggregate: AggregateKey("issue:ENG-41".into()),
            causation_id: None,
            correlation_id: CorrelationId("issue-owner-updated-1".into()),
            caused_by: None,
            depth: 1,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-08-10T10:00:00Z".into()),
            recorded_at: Timestamp("2026-08-10T10:00:01Z".into()),
            payload: serde_json::json!({ "change_kind": "ownership" }),
        }
    }

    #[test]
    fn one_pending_firing_has_one_stable_actionable_inbox_identity() {
        let first = pending_automation_approval(
            "44444444-4444-4444-8444-444444444444",
            "founder",
            &event(),
        );
        let second = pending_automation_approval(
            "44444444-4444-4444-8444-444444444444",
            "founder",
            &event(),
        );
        assert_eq!(
            first, second,
            "redelivery cannot mint a second approval item"
        );
        let durable = DurableInboxItem {
            item: first.item,
            subject_root: first.subject_root,
            template_key: first.template_key,
            template_args: first.template_args,
            occurred_at: first.occurred_at,
            dek_ref: first.dek_ref,
            priority: 90,
        };
        assert_eq!(
            automation_approval_action(&durable),
            Some(AutomationApprovalAction {
                automation_id: "44444444-4444-4444-8444-444444444444".into(),
                event_id: "issue-owner-updated-1".into(),
            })
        );
    }
}

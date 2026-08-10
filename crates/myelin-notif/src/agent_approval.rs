use myelin_events::ArtifactRef;
use myelin_storage::hitl_gate_durable::{gate_id_from_ref_token, gate_ref_token, GateRecord};
use myelin_tenancy::{Region, TenantId};

use crate::humanise::reason_template_key;
use crate::pg_inbox::{DurableInboxItem, InboxUpsert};
use crate::ranking::reason_base_class;
use crate::router::RoutedInboxItem;
use crate::storm_control::subject_root_of;
use crate::Reason;

const APPROVAL_NAMESPACE: &str = "agent-effect-approval";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentEffectApprovalAction {
    pub gate_id: String,
    pub run_id: String,
}

pub fn agent_effect_approval_item_id(tenant: &TenantId, recipient: &str, gate_id: &str) -> String {
    let mut material = Vec::new();
    for component in [&tenant.0, recipient, gate_id] {
        material.extend_from_slice(component.as_bytes());
        material.push(0);
    }
    format!(
        "agent-approval-{}",
        &blake3::hash(&material).to_hex().as_str()[..24]
    )
}

pub fn pending_agent_effect_approval(
    tenant: &TenantId,
    region: &Region,
    recipient: &str,
    gate: &GateRecord,
) -> InboxUpsert {
    let run_ref = ArtifactRef(format!("myelin://{}/agent/run/{}", tenant.0, gate.run_id));
    let gate_ref = ArtifactRef(format!(
        "{}:hitl-gate:{}",
        run_ref.0,
        gate_ref_token(&gate.gate_id)
    ));
    let subject = ArtifactRef(gate.card_ref.clone().unwrap_or_else(|| run_ref.0.clone()));
    let reason = Reason::ApprovalRequested;
    InboxUpsert {
        item: RoutedInboxItem {
            tenant: tenant.clone(),
            region: region.clone(),
            item_id: agent_effect_approval_item_id(tenant, recipient, &gate.gate_id),
            recipient: recipient.to_string(),
            subject: subject.clone(),
            reason,
            class: reason_base_class(reason).1,
            origin_event: gate_ref.clone(),
            dedup_key: format!("{APPROVAL_NAMESPACE}:{}", gate.gate_id),
            coalesce_count: 1,
            state: "unread".into(),
            snooze_until: None,
        },
        subject_root: ArtifactRef(subject_root_of(&subject.0)),
        template_key: reason_template_key(reason).to_string(),
        template_args: vec![subject, run_ref, gate_ref],
        occurred_at: chrono::DateTime::from_timestamp(gate.opened_at_unix, 0)
            .map(|timestamp| timestamp.to_rfc3339())
            .unwrap_or_else(|| gate.opened_at_unix.to_string()),
        dek_ref: format!("kms://{}/notif/inbox", tenant.0),
    }
}

pub fn agent_effect_approval_action(item: &DurableInboxItem) -> Option<AgentEffectApprovalAction> {
    if item.item.reason != Reason::ApprovalRequested || item.template_args.len() != 3 {
        return None;
    }
    let run_prefix = format!("myelin://{}/agent/run/", item.item.tenant.0);
    let run_id = item.template_args.get(1)?.0.strip_prefix(&run_prefix)?;
    let gate_prefix = format!("{run_prefix}{run_id}:hitl-gate:");
    let gate_token = item.template_args.get(2)?.0.strip_prefix(&gate_prefix)?;
    let gate_id = gate_id_from_ref_token(gate_token)?;
    if run_id.is_empty() || run_id.contains('/') || item.item.origin_event != item.template_args[2]
    {
        return None;
    }
    Some(AgentEffectApprovalAction {
        gate_id,
        run_id: run_id.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_storage::hitl_gate_durable::GateState;

    fn gate() -> GateRecord {
        GateRecord {
            gate_id: "gate:0123456789abcdef0123456789abcdef".into(),
            run_id: "run-7".into(),
            effect_id: "opaque-effect".into(),
            risk_summary: b"Merge acme/web#42".to_vec(),
            cost_estimate: 0,
            approver_filter: vec!["founder".into()],
            state: GateState::Waiting,
            card_ref: Some("myelin://acme/git/pr/acme/web:42".into()),
            requested_by: "agent:reviewer".into(),
            decided_by: None,
            opened_at_unix: 1_786_352_400,
            decided_at_unix: None,
            expires_at_unix: 1_786_356_000,
            approval_consumed_at_unix: None,
        }
    }

    #[test]
    fn one_agent_effect_gate_has_one_exact_human_action() {
        let tenant = TenantId("acme".into());
        let region = Region("eu-north".into());
        let first = pending_agent_effect_approval(&tenant, &region, "founder", &gate());
        let replay = pending_agent_effect_approval(&tenant, &region, "founder", &gate());
        assert_eq!(
            first, replay,
            "MCP redelivery cannot duplicate the decision"
        );
        myelin_refs::parse_scoped(&first.item.origin_event.0)
            .expect("an inbox origin is a canonical ArtifactRef");

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
            agent_effect_approval_action(&durable),
            Some(AgentEffectApprovalAction {
                gate_id: "gate:0123456789abcdef0123456789abcdef".into(),
                run_id: "run-7".into(),
            })
        );
    }
}

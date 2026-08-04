use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::crosscell::{ArtifactType, CellId, CrossCellPointer, OpaqueSubjectId};
use crate::{CorrelationId, EventEnvelope, EventType};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CrossCellStream {
    IssuePortfolio,
    KnowledgeCollab,
    ChatCrossOrg,
}

impl CrossCellStream {
    #[must_use]
    pub fn artifact_type(self) -> ArtifactType {
        match self {
            CrossCellStream::IssuePortfolio => ArtifactType::Issue,
            CrossCellStream::KnowledgeCollab => ArtifactType::Page,
            CrossCellStream::ChatCrossOrg => ArtifactType::Channel,
        }
    }

    #[must_use]
    pub fn classify(event_type: &EventType) -> Option<CrossCellStream> {
        let mut segments = event_type.0.split('.');
        let subsystem = segments.next()?;
        let artifact = segments.next();
        match (subsystem, artifact) {
            ("issues", _) => Some(CrossCellStream::IssuePortfolio),
            ("knowledge", _) => Some(CrossCellStream::KnowledgeCollab),
            ("chat", _) => Some(CrossCellStream::ChatCrossOrg),
            _ => None,
        }
    }
}

#[must_use]
pub fn pointer_for_propagation(
    envelope: &EventEnvelope,
    stream: CrossCellStream,
    home_cell: CellId,
) -> CrossCellPointer {
    CrossCellPointer::new(
        OpaqueSubjectId::from_ref(envelope.subject.clone()),
        stream.artifact_type(),
        envelope.correlation_id.clone(),
        home_cell,
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropagatedPointer {
    pub to_cell: CellId,
    pub pointer: CrossCellPointer,
    pub stream: CrossCellStream,
}

#[derive(Clone)]
pub struct CrossCellPropagator {
    home_cell: CellId,
    pointers_propagated: Arc<AtomicU64>,
    pii_fields_crossed: Arc<AtomicU64>,
}

impl CrossCellPropagator {
    #[must_use]
    pub fn new(home_cell: CellId) -> CrossCellPropagator {
        CrossCellPropagator {
            home_cell,
            pointers_propagated: Arc::new(AtomicU64::new(0)),
            pii_fields_crossed: Arc::new(AtomicU64::new(0)),
        }
    }

    #[must_use]
    pub fn home_cell(&self) -> &CellId {
        &self.home_cell
    }

    pub fn fan_out(
        &self,
        envelope: &EventEnvelope,
        member_cells: &[CellId],
    ) -> Vec<PropagatedPointer> {
        let Some(stream) = CrossCellStream::classify(&envelope.type_) else {
            return Vec::new();
        };
        let pointer = pointer_for_propagation(envelope, stream, self.home_cell.clone());
        member_cells
            .iter()
            .filter(|to| **to != self.home_cell)
            .map(|to| {
                self.pointers_propagated.fetch_add(1, Ordering::SeqCst);
                PropagatedPointer {
                    to_cell: to.clone(),
                    pointer: pointer.clone(),
                    stream,
                }
            })
            .collect()
    }

    #[must_use]
    pub fn pointers_propagated(&self) -> u64 {
        self.pointers_propagated.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn pii_fields_crossed(&self) -> u64 {
        self.pii_fields_crossed.load(Ordering::SeqCst)
    }
}

impl core::fmt::Debug for CrossCellPropagator {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CrossCellPropagator")
            .field("home_cell", &self.home_cell.as_str())
            .field("pointers_propagated", &self.pointers_propagated())
            .field("pii_fields_crossed", &self.pii_fields_crossed())
            .finish()
    }
}

#[must_use]
pub fn propagated_carried_fields(
    propagated: &PropagatedPointer,
) -> (
    &CellId,
    &OpaqueSubjectId,
    &ArtifactType,
    &CorrelationId,
    &CellId,
) {
    (
        &propagated.to_cell,
        propagated.pointer.subject(),
        propagated.pointer.artifact_type(),
        propagated.pointer.correlation_id(),
        propagated.pointer.home_cell(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Actor, AggregateKey, ArtifactRef, DataRole, EventId, Timestamp, Visibility};
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

    fn envelope_with_payload_pii(type_: &str, subject: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId("01J0EVT".into()),
            type_: EventType(type_.into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                TenantId("acme".into()),
            )),
            subject: ArtifactRef(subject.into()),
            aggregate: AggregateKey("issue:PROJ-1".into()),
            causation_id: None,
            correlation_id: CorrelationId("01J0CORR".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: true,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-24T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-24T00:00:01Z".into()),
            payload: serde_json::json!({ "assignee_email": "alice@example.com", "body": "secret" }),
        }
    }

    #[test]
    fn pointer_for_propagation_carries_only_the_four_frozen_fields_no_payload() {
        let env =
            envelope_with_payload_pii("issues.issue.created", "myelin://01J0ACME/issues/issue/42");
        let p = pointer_for_propagation(
            &env,
            CrossCellStream::IssuePortfolio,
            CellId::from_token("cell-fr-par-1"),
        );

        assert_eq!(
            p.subject().artifact_ref().0,
            "myelin://01J0ACME/issues/issue/42"
        );
        assert_eq!(p.artifact_type(), &ArtifactType::Issue);
        assert_eq!(p.correlation_id(), &CorrelationId("01J0CORR".into()));
        assert_eq!(p.home_cell().as_str(), "cell-fr-par-1");

        let wire = serde_json::to_string(&p).expect("pointer serialises");
        assert!(
            !wire.contains("alice@example.com"),
            "the payload email NEVER crosses: {wire}"
        );
        assert!(
            !wire.contains("secret"),
            "the payload body NEVER crosses: {wire}"
        );
        assert!(
            !wire.contains("payload"),
            "there is no payload field on the frame: {wire}"
        );
    }

    #[test]
    fn pointer_rides_the_envelope_causal_root() {
        let env =
            envelope_with_payload_pii("issues.issue.created", "myelin://01J0ACME/issues/issue/42");
        let p = pointer_for_propagation(
            &env,
            CrossCellStream::IssuePortfolio,
            CellId::from_token("cell-fr-par-1"),
        );
        assert_eq!(p.correlation_id(), &env.correlation_id);
    }

    #[test]
    fn classify_routes_iss_kn_chat_and_skips_others() {
        assert_eq!(
            CrossCellStream::classify(&EventType("issues.issue.created".into())),
            Some(CrossCellStream::IssuePortfolio)
        );
        assert_eq!(
            CrossCellStream::classify(&EventType("knowledge.page.updated".into())),
            Some(CrossCellStream::KnowledgeCollab)
        );
        assert_eq!(
            CrossCellStream::classify(&EventType("chat.message.created".into())),
            Some(CrossCellStream::ChatCrossOrg)
        );
        assert_eq!(
            CrossCellStream::classify(&EventType("git.ref.updated".into())),
            None
        );
        assert_eq!(
            CrossCellStream::classify(&EventType("ci.check.updated".into())),
            None
        );
        assert_eq!(
            CrossCellStream::classify(&EventType("identity.principal.created".into())),
            None
        );
    }

    #[test]
    fn stream_artifact_types_are_the_floor_follow_on_kinds() {
        assert_eq!(
            CrossCellStream::IssuePortfolio.artifact_type(),
            ArtifactType::Issue
        );
        assert_eq!(
            CrossCellStream::KnowledgeCollab.artifact_type(),
            ArtifactType::Page
        );
        assert_eq!(
            CrossCellStream::ChatCrossOrg.artifact_type(),
            ArtifactType::Channel
        );
    }

    #[test]
    fn fan_out_produces_one_pointer_per_other_member_cell() {
        let prop = CrossCellPropagator::new(CellId::from_token("cell-a"));
        let env =
            envelope_with_payload_pii("issues.issue.created", "myelin://01J0ACME/issues/issue/42");
        let members = vec![
            CellId::from_token("cell-a"),
            CellId::from_token("cell-b"),
            CellId::from_token("cell-c"),
        ];
        let fanned = prop.fan_out(&env, &members);

        let dests: Vec<&str> = fanned.iter().map(|p| p.to_cell.as_str()).collect();
        assert_eq!(dests, vec!["cell-b", "cell-c"]);
        assert_eq!(prop.pointers_propagated(), 2);
        for pp in &fanned {
            let (to, subject, kind, corr, home) = propagated_carried_fields(pp);
            assert_eq!(kind, &ArtifactType::Issue);
            assert_eq!(
                subject.artifact_ref().0,
                "myelin://01J0ACME/issues/issue/42"
            );
            assert_eq!(corr, &CorrelationId("01J0CORR".into()));
            assert_eq!(home.as_str(), "cell-a");
            assert!(matches!(to.as_str(), "cell-b" | "cell-c"));
            let wire = serde_json::to_string(&pp.pointer).expect("serialises");
            assert!(!wire.contains("alice@example.com"));
            assert!(!wire.contains("secret"));
        }
        assert_eq!(prop.pii_fields_crossed(), 0);
    }

    #[test]
    fn non_cross_cell_event_is_not_propagated() {
        let prop = CrossCellPropagator::new(CellId::from_token("cell-a"));
        let env = envelope_with_payload_pii("git.ref.updated", "myelin://01J0ACME/git/repo/r1");
        let fanned = prop.fan_out(&env, &[CellId::from_token("cell-b")]);
        assert!(
            fanned.is_empty(),
            "a non-cross-cell event is never propagated"
        );
        assert_eq!(prop.pointers_propagated(), 0);
        assert_eq!(prop.pii_fields_crossed(), 0);
    }

    #[test]
    fn single_home_cell_tenant_propagates_nothing() {
        let prop = CrossCellPropagator::new(CellId::from_token("cell-a"));
        let env =
            envelope_with_payload_pii("issues.issue.created", "myelin://01J0ACME/issues/issue/42");
        let fanned = prop.fan_out(&env, &[CellId::from_token("cell-a")]);
        assert!(
            fanned.is_empty(),
            "a single-home-cell tenant has nowhere to propagate"
        );
        assert_eq!(prop.pointers_propagated(), 0);
    }

    #[test]
    fn kn_collab_and_chat_cross_org_fan_out_under_their_kinds() {
        let prop = CrossCellPropagator::new(CellId::from_token("cell-a"));
        let members = vec![CellId::from_token("cell-b")];

        let kn = envelope_with_payload_pii(
            "knowledge.page.updated",
            "myelin://01J0ACME/knowledge/page/9",
        );
        let kn_fan = prop.fan_out(&kn, &members);
        assert_eq!(kn_fan.len(), 1);
        assert_eq!(kn_fan[0].stream, CrossCellStream::KnowledgeCollab);
        assert_eq!(kn_fan[0].pointer.artifact_type(), &ArtifactType::Page);

        let chat =
            envelope_with_payload_pii("chat.message.created", "myelin://01J0ACME/chat/channel/3");
        let chat_fan = prop.fan_out(&chat, &members);
        assert_eq!(chat_fan.len(), 1);
        assert_eq!(chat_fan[0].stream, CrossCellStream::ChatCrossOrg);
        assert_eq!(chat_fan[0].pointer.artifact_type(), &ArtifactType::Channel);
    }

    #[test]
    fn propagator_debug_is_pii_free() {
        let prop = CrossCellPropagator::new(CellId::from_token("cell-a"));
        let env =
            envelope_with_payload_pii("issues.issue.created", "myelin://01J0ACME/issues/issue/42");
        let _ = prop.fan_out(&env, &[CellId::from_token("cell-b")]);
        let dbg = format!("{prop:?}");
        assert!(
            dbg.contains("cell-a"),
            "Debug shows the home cell id: {dbg}"
        );
        assert!(
            dbg.contains("pointers_propagated"),
            "Debug shows the counter: {dbg}"
        );
        assert!(
            !dbg.contains("alice@example.com"),
            "Debug leaks no payload PII: {dbg}"
        );
        assert!(!dbg.contains("secret"), "Debug leaks no payload PII: {dbg}");
    }
}

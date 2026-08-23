use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_events::crosscell_propagation::{
    pointer_for_propagation, CrossCellStream, PropagatedPointer,
};
use myelin_events::{
    Actor, ArtifactRef, CellId, CorrelationId, CrossCellPointer, DataRole, EventEnvelope, EventId,
    EventType, OpaqueSubjectId, Timestamp, Visibility,
};
use myelin_identity::Principal;
use myelin_tenancy::{Region, TenantId};

use crate::conversation::Membership;
use crate::events::CHAT_MESSAGE_CREATED;
use crate::store::ConversationId;

#[must_use]
pub fn channel_ref(conv: &ConversationId) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{}/chat/channel/{}",
        conv.tenant, conv.conversation_id
    ))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FederatedMember {
    pub principal_id: String,
    pub home_cell: CellId,
}

impl FederatedMember {
    #[must_use]
    pub fn new(principal_id: impl Into<String>, home_cell: CellId) -> FederatedMember {
        FederatedMember {
            principal_id: principal_id.into(),
            home_cell,
        }
    }

    #[must_use]
    pub fn from_membership(m: &Membership, home_cell: CellId) -> FederatedMember {
        FederatedMember::new(m.principal_id.clone(), home_cell)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossOrgPointer {
    pub to_cell: CellId,
    pub pointer: CrossCellPointer,
}

impl CrossOrgPointer {
    #[must_use]
    pub fn subject(&self) -> &ArtifactRef {
        self.pointer.subject().artifact_ref()
    }

    #[must_use]
    pub fn home_cell(&self) -> &CellId {
        self.pointer.home_cell()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ChannelProjection {
    Rendered {
        subject: ArtifactRef,
        rendered: String,
    },
    Tombstone {
        subject: ArtifactRef,
    },
}

impl ChannelProjection {
    #[must_use]
    pub fn subject(&self) -> &ArtifactRef {
        match self {
            ChannelProjection::Rendered { subject, .. }
            | ChannelProjection::Tombstone { subject } => subject,
        }
    }

    #[must_use]
    pub fn is_rendered(&self) -> bool {
        matches!(self, ChannelProjection::Rendered { .. })
    }
}

pub trait CellLocalChannelResolution {
    fn resolve_in_home_cell(
        &self,
        pointer: &CrossOrgPointer,
        viewer: &Principal,
    ) -> ChannelProjection;
}

#[derive(Clone)]
pub struct CrossOrgChannel {
    home_cell: CellId,
    events_fanned_out: Arc<AtomicU64>,
    raw_rows_crossed: Arc<AtomicU64>,
}

impl CrossOrgChannel {
    #[must_use]
    pub fn new(home_cell: CellId) -> CrossOrgChannel {
        CrossOrgChannel {
            home_cell,
            events_fanned_out: Arc::new(AtomicU64::new(0)),
            raw_rows_crossed: Arc::new(AtomicU64::new(0)),
        }
    }

    #[must_use]
    pub fn home_cell(&self) -> &CellId {
        &self.home_cell
    }

    pub fn fan_out_channel_event(
        &self,
        conv: &ConversationId,
        correlation_id: &CorrelationId,
        members: &[FederatedMember],
    ) -> Vec<CrossOrgPointer> {
        let envelope = self.channel_event_envelope(conv, correlation_id);
        let pointer = pointer_for_propagation(
            &envelope,
            CrossCellStream::ChatCrossOrg,
            self.home_cell.clone(),
        );
        self.distinct_member_cells(members)
            .into_iter()
            .filter(|to| *to != self.home_cell)
            .map(|to| {
                self.events_fanned_out.fetch_add(1, Ordering::SeqCst);
                CrossOrgPointer {
                    to_cell: to,
                    pointer: pointer.clone(),
                }
            })
            .collect()
    }

    #[must_use]
    pub fn dsr_member_cells(&self, members: &[FederatedMember]) -> Vec<CellId> {
        let mut cells = self.distinct_member_cells(members);
        if !cells.contains(&self.home_cell) {
            cells.push(self.home_cell.clone());
            cells.sort();
        }
        cells
    }

    fn distinct_member_cells(&self, members: &[FederatedMember]) -> Vec<CellId> {
        members
            .iter()
            .map(|m| m.home_cell.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    #[must_use]
    pub fn resolve_cell_local(
        &self,
        pointer: &CrossOrgPointer,
        viewer: &Principal,
        resolver: &dyn CellLocalChannelResolution,
    ) -> ChannelProjection {
        resolver.resolve_in_home_cell(pointer, viewer)
    }

    fn channel_event_envelope(
        &self,
        conv: &ConversationId,
        correlation_id: &CorrelationId,
    ) -> EventEnvelope {
        let subject = channel_ref(conv);
        EventEnvelope {
            event_id: EventId(format!("xorg-{}", conv.conversation_id)),
            type_: EventType(CHAT_MESSAGE_CREATED.into()),
            schema_ver: 1,
            tenant: TenantId(conv.tenant.clone()),
            region: Region(conv.region.clone()),
            actor: Actor(Principal::stub(
                myelin_identity::PrincipalId("xorg-fanout".into()),
                myelin_identity::PrincipalKind::Service,
                TenantId(conv.tenant.clone()),
            )),
            subject,
            aggregate: crate::events::channel_aggregate(&conv.conversation_id),
            causation_id: None,
            correlation_id: correlation_id.clone(),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-25T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-25T00:00:01Z".into()),
            payload: serde_json::json!({}),
        }
    }

    #[must_use]
    pub fn events_fanned_out(&self) -> u64 {
        self.events_fanned_out.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn raw_rows_crossed(&self) -> u64 {
        self.raw_rows_crossed.load(Ordering::SeqCst)
    }
}

impl core::fmt::Debug for CrossOrgChannel {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CrossOrgChannel")
            .field("home_cell", &self.home_cell.as_str())
            .field("events_fanned_out", &self.events_fanned_out())
            .field("raw_rows_crossed", &self.raw_rows_crossed())
            .finish()
    }
}

#[must_use]
pub fn fanned_out_carried_fields(
    fanned: &CrossOrgPointer,
) -> (
    &CellId,
    &OpaqueSubjectId,
    &myelin_events::ArtifactType,
    &CorrelationId,
    &CellId,
) {
    (
        &fanned.to_cell,
        fanned.pointer.subject(),
        fanned.pointer.artifact_type(),
        fanned.pointer.correlation_id(),
        fanned.pointer.home_cell(),
    )
}

#[must_use]
pub fn as_propagated(fanned: &CrossOrgPointer) -> PropagatedPointer {
    PropagatedPointer {
        to_cell: fanned.to_cell.clone(),
        pointer: fanned.pointer.clone(),
        stream: CrossCellStream::ChatCrossOrg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};

    fn conv() -> ConversationId {
        ConversationId::new("acme", "fr-par", "01J0CHAN")
    }

    fn viewer() -> Principal {
        Principal::stub(
            PrincipalId("viewer-opaque".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn roster() -> Vec<FederatedMember> {
        vec![
            FederatedMember::new("psn:creator", CellId::from_token("cell-fr-par-1")),
            FederatedMember::new("psn:de-member-1", CellId::from_token("cell-de-1")),
            FederatedMember::new("psn:de-member-2", CellId::from_token("cell-de-1")),
            FederatedMember::new("psn:nl-member", CellId::from_token("cell-nl-1")),
        ]
    }

    struct HomeCellResolver {
        allowed: Vec<String>,
        rendered: String,
    }

    impl CellLocalChannelResolution for HomeCellResolver {
        fn resolve_in_home_cell(
            &self,
            pointer: &CrossOrgPointer,
            viewer: &Principal,
        ) -> ChannelProjection {
            let subject = pointer.subject().clone();
            if self.allowed.iter().any(|id| id == &viewer.principal_id.0) {
                ChannelProjection::Rendered {
                    subject,
                    rendered: self.rendered.clone(),
                }
            } else {
                ChannelProjection::Tombstone { subject }
            }
        }
    }

    #[test]
    fn cross_org_fan_out_carries_only_the_pointer_zero_raw_rows() {
        let xorg = CrossOrgChannel::new(CellId::from_token("cell-fr-par-1"));
        let corr = CorrelationId("event-causal-root".into());
        let fanned = xorg.fan_out_channel_event(&conv(), &corr, &roster());

        let dests: Vec<&str> = fanned.iter().map(|p| p.to_cell.as_str()).collect();
        assert_eq!(dests, vec!["cell-de-1", "cell-nl-1"]);
        assert_eq!(xorg.events_fanned_out(), 2);

        for pp in &fanned {
            let (to, subject, kind, corr_field, home) = fanned_out_carried_fields(pp);
            assert_eq!(
                subject.artifact_ref().0,
                "myelin://acme/chat/channel/01J0CHAN"
            );
            assert_eq!(kind, &myelin_events::ArtifactType::Channel);
            assert_eq!(corr_field, &corr);
            assert_eq!(home.as_str(), "cell-fr-par-1");
            assert!(matches!(to.as_str(), "cell-de-1" | "cell-nl-1"));

            let wire = serde_json::to_string(&pp.pointer).expect("pointer serialises");
            let json: serde_json::Value = serde_json::from_str(&wire).expect("valid json");
            let mut keys: Vec<&str> = json
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(keys, ["correlation_id", "home_cell", "subject", "type"]);
            assert!(
                !wire.contains("payload"),
                "no payload/body field on the frame: {wire}"
            );
        }
        assert_eq!(xorg.raw_rows_crossed(), 0);
    }

    #[test]
    fn membership_spanning_cells_reaches_every_other_member_cell() {
        let xorg = CrossOrgChannel::new(CellId::from_token("cell-a"));
        let members = vec![
            FederatedMember::new("p1", CellId::from_token("cell-a")),
            FederatedMember::new("p2", CellId::from_token("cell-b")),
            FederatedMember::new("p3", CellId::from_token("cell-c")),
            FederatedMember::new("p4", CellId::from_token("cell-d")),
        ];
        let corr = CorrelationId("root".into());
        let fanned = xorg.fan_out_channel_event(&conv(), &corr, &members);

        let dests: Vec<&str> = fanned.iter().map(|p| p.to_cell.as_str()).collect();
        assert_eq!(
            dests,
            vec!["cell-b", "cell-c", "cell-d"],
            "every OTHER member cell reached"
        );
        for pp in &fanned {
            assert_eq!(
                pp.pointer.correlation_id(),
                &corr,
                "rides the event causal-root"
            );
        }
    }

    #[test]
    fn single_home_cell_channel_fans_out_nothing() {
        let xorg = CrossOrgChannel::new(CellId::from_token("cell-a"));
        let members = vec![
            FederatedMember::new("p1", CellId::from_token("cell-a")),
            FederatedMember::new("p2", CellId::from_token("cell-a")),
        ];
        let fanned = xorg.fan_out_channel_event(&conv(), &CorrelationId("root".into()), &members);
        assert!(
            fanned.is_empty(),
            "a single-home-cell channel has nowhere to fan out"
        );
        assert_eq!(xorg.events_fanned_out(), 0);
        assert_eq!(xorg.raw_rows_crossed(), 0);
    }

    #[test]
    fn resolution_stays_cell_local_only_the_projection_crosses() {
        let xorg = CrossOrgChannel::new(CellId::from_token("cell-fr-par-1"));
        let fanned = xorg.fan_out_channel_event(
            &conv(),
            &CorrelationId("root".into()),
            &[FederatedMember::new(
                "psn:de-member-1",
                CellId::from_token("cell-de-1"),
            )],
        );
        let pointer = &fanned[0];

        let home = HomeCellResolver {
            allowed: vec!["viewer-opaque".into()],
            rendered: "#cross-org-incident · active (rendered in fr-par-1, viewer-scoped)".into(),
        };
        let proj = xorg.resolve_cell_local(pointer, &viewer(), &home);
        assert!(
            proj.is_rendered(),
            "an allowed viewer gets the rendered projection"
        );
        assert_eq!(proj.subject().0, "myelin://acme/chat/channel/01J0CHAN");
        if let ChannelProjection::Rendered { rendered, .. } = &proj {
            assert!(rendered.contains("rendered in fr-par-1"));
        }
    }

    #[test]
    fn unauthorised_cross_org_viewer_resolves_to_a_tombstone() {
        let xorg = CrossOrgChannel::new(CellId::from_token("cell-fr-par-1"));
        let fanned = xorg.fan_out_channel_event(
            &conv(),
            &CorrelationId("root".into()),
            &[FederatedMember::new(
                "psn:de-member-1",
                CellId::from_token("cell-de-1"),
            )],
        );
        let home = HomeCellResolver {
            allowed: vec![],
            rendered: "secret-channel-name".into(),
        };
        let proj = xorg.resolve_cell_local(&fanned[0], &viewer(), &home);
        assert!(
            !proj.is_rendered(),
            "an unauthorised viewer gets a tombstone"
        );
        assert!(matches!(proj, ChannelProjection::Tombstone { .. }));
        let wire = serde_json::to_string(&proj).expect("projection serialises");
        assert!(!wire.contains("secret-channel-name"));
    }

    #[test]
    fn multi_cell_dsr_iterates_every_member_cell_zero_missed() {
        let xorg = CrossOrgChannel::new(CellId::from_token("cell-fr-par-1"));
        let cells = xorg.dsr_member_cells(&roster());
        let names: Vec<&str> = cells.iter().map(CellId::as_str).collect();
        assert_eq!(names, vec!["cell-de-1", "cell-fr-par-1", "cell-nl-1"]);
        assert_eq!(
            cells.len(),
            3,
            "0 member cells missed by the DSR enumeration"
        );
    }

    #[test]
    fn dsr_always_includes_the_home_cell() {
        let xorg = CrossOrgChannel::new(CellId::from_token("cell-home"));
        let members = vec![
            FederatedMember::new("p1", CellId::from_token("cell-b")),
            FederatedMember::new("p2", CellId::from_token("cell-c")),
        ];
        let cells = xorg.dsr_member_cells(&members);
        let names: Vec<&str> = cells.iter().map(CellId::as_str).collect();
        assert!(
            names.contains(&"cell-home"),
            "the home cell is always a DSR holder: {names:?}"
        );
        assert!(names.contains(&"cell-b") && names.contains(&"cell-c"));
    }

    #[test]
    fn cdc_12_6_chat_consumer_reads_only_the_four_fields() {
        let xorg = CrossOrgChannel::new(CellId::from_token("cell-fr-par-1"));
        let fanned = xorg.fan_out_channel_event(
            &conv(),
            &CorrelationId("root".into()),
            &[FederatedMember::new(
                "psn:de-member-1",
                CellId::from_token("cell-de-1"),
            )],
        );
        let provider = &fanned[0];

        let wire = serde_json::to_string(&provider.pointer).expect("provider emits the frame");
        let json: serde_json::Value = serde_json::from_str(&wire).expect("valid json");
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["correlation_id", "home_cell", "subject", "type"]);

        let consumer: CrossCellPointer =
            serde_json::from_str(&wire).expect("consumer reads the frame");
        assert_eq!(
            consumer, provider.pointer,
            "the CDC wire shape is conformant both ways"
        );

        let propagated = as_propagated(provider);
        assert_eq!(propagated.stream, CrossCellStream::ChatCrossOrg);
        assert_eq!(
            propagated.pointer.artifact_type(),
            &myelin_events::ArtifactType::Channel
        );
    }

    #[test]
    fn cross_org_debug_is_pii_free() {
        let xorg = CrossOrgChannel::new(CellId::from_token("cell-fr-par-1"));
        let _ = xorg.fan_out_channel_event(
            &conv(),
            &CorrelationId("root".into()),
            &[FederatedMember::new(
                "psn:de-secret-member",
                CellId::from_token("cell-de-1"),
            )],
        );
        let dbg = format!("{xorg:?}");
        assert!(
            dbg.contains("cell-fr-par-1"),
            "Debug shows the home cell id: {dbg}"
        );
        assert!(
            dbg.contains("events_fanned_out"),
            "Debug shows the counter: {dbg}"
        );
        assert!(
            !dbg.contains("psn:de-secret-member"),
            "Debug leaks no member id: {dbg}"
        );
    }

    #[test]
    fn cross_org_channel_built_on_the_non_foreclosing_conversation_model() {
        let m = Membership::member(conv(), "psn:foreign-org-principal");
        let fed = FederatedMember::from_membership(&m, CellId::from_token("cell-de-1"));
        assert_eq!(fed.principal_id, "psn:foreign-org-principal");
        assert_eq!(fed.home_cell.as_str(), "cell-de-1");
        assert_eq!(
            channel_ref(&conv()).0,
            "myelin://acme/chat/channel/01J0CHAN"
        );
    }
}

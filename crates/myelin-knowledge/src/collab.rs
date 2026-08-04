use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_content::events::KNOWLEDGE_DOC_UPDATED;
use myelin_events::crosscell_propagation::{
    pointer_for_propagation, CrossCellStream, PropagatedPointer,
};
use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CellId, CorrelationId, CrossCellPointer, DataRole,
    EventEnvelope, EventId, EventType, OpaqueSubjectId, Timestamp, Visibility,
};
use myelin_identity::Principal;
use myelin_tenancy::{Region, TenantId};

use crate::emit::page_ref;
use crate::transport::DocOp;

#[derive(Clone, Debug)]
pub struct CrossCellDocOp<'a> {
    pub tenant: &'a TenantId,
    pub page_id: &'a str,
    pub op: &'a DocOp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossCellDocPointer {
    pub to_cell: CellId,
    pub pointer: CrossCellPointer,
}

impl CrossCellDocPointer {
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
pub enum DocProjection {
    Rendered {
        subject: ArtifactRef,
        rendered: String,
    },
    Tombstone {
        subject: ArtifactRef,
    },
}

impl DocProjection {
    #[must_use]
    pub fn subject(&self) -> &ArtifactRef {
        match self {
            DocProjection::Rendered { subject, .. } | DocProjection::Tombstone { subject } => {
                subject
            }
        }
    }

    #[must_use]
    pub fn is_rendered(&self) -> bool {
        matches!(self, DocProjection::Rendered { .. })
    }
}

pub trait CellLocalDocResolution {
    fn resolve_in_home_cell(
        &self,
        pointer: &CrossCellDocPointer,
        viewer: &Principal,
    ) -> DocProjection;
}

#[derive(Clone)]
pub struct CrossCellCollab {
    home_cell: CellId,
    ops_fanned_out: Arc<AtomicU64>,
    cross_cell_pii_crossed: Arc<AtomicU64>,
}

impl CrossCellCollab {
    #[must_use]
    pub fn new(home_cell: CellId) -> CrossCellCollab {
        CrossCellCollab {
            home_cell,
            ops_fanned_out: Arc::new(AtomicU64::new(0)),
            cross_cell_pii_crossed: Arc::new(AtomicU64::new(0)),
        }
    }

    #[must_use]
    pub fn home_cell(&self) -> &CellId {
        &self.home_cell
    }

    pub fn fan_out_doc_op(
        &self,
        doc_op: &CrossCellDocOp<'_>,
        correlation_id: &CorrelationId,
        member_cells: &[CellId],
    ) -> Vec<CrossCellDocPointer> {
        let envelope = self.doc_updated_envelope(doc_op, correlation_id);
        let pointer = pointer_for_propagation(
            &envelope,
            CrossCellStream::KnowledgeCollab,
            self.home_cell.clone(),
        );
        member_cells
            .iter()
            .filter(|to| **to != self.home_cell)
            .map(|to| {
                self.ops_fanned_out.fetch_add(1, Ordering::SeqCst);
                CrossCellDocPointer {
                    to_cell: to.clone(),
                    pointer: pointer.clone(),
                }
            })
            .collect()
    }

    fn doc_updated_envelope(
        &self,
        doc_op: &CrossCellDocOp<'_>,
        correlation_id: &CorrelationId,
    ) -> EventEnvelope {
        let subject = page_ref(doc_op.tenant, doc_op.page_id);
        EventEnvelope {
            event_id: EventId(format!("doc-op-{}", doc_op.op.op_id.wire())),
            type_: EventType(KNOWLEDGE_DOC_UPDATED.into()),
            schema_ver: 1,
            tenant: doc_op.tenant.clone(),
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                myelin_identity::PrincipalId("collab-fanout".into()),
                myelin_identity::PrincipalKind::Service,
                doc_op.tenant.clone(),
            )),
            subject: subject.clone(),
            aggregate: AggregateKey(format!("page:{}", doc_op.page_id)),
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
    pub fn resolve_cell_local(
        &self,
        pointer: &CrossCellDocPointer,
        viewer: &Principal,
        resolver: &dyn CellLocalDocResolution,
    ) -> DocProjection {
        resolver.resolve_in_home_cell(pointer, viewer)
    }

    #[must_use]
    pub fn ops_fanned_out(&self) -> u64 {
        self.ops_fanned_out.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn cross_cell_pii_crossed(&self) -> u64 {
        self.cross_cell_pii_crossed.load(Ordering::SeqCst)
    }
}

impl core::fmt::Debug for CrossCellCollab {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CrossCellCollab")
            .field("home_cell", &self.home_cell.as_str())
            .field("ops_fanned_out", &self.ops_fanned_out())
            .field("cross_cell_pii_crossed", &self.cross_cell_pii_crossed())
            .finish()
    }
}

#[must_use]
pub fn fanned_out_carried_fields(
    fanned: &CrossCellDocPointer,
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
pub fn as_propagated(fanned: &CrossCellDocPointer) -> PropagatedPointer {
    PropagatedPointer {
        to_cell: fanned.to_cell.clone(),
        pointer: fanned.pointer.clone(),
        stream: CrossCellStream::KnowledgeCollab,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{OpId, OpKind};
    use myelin_identity::{PrincipalId, PrincipalKind};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn viewer() -> Principal {
        Principal::stub(
            PrincipalId("viewer-opaque".into()),
            PrincipalKind::Human,
            tenant(),
        )
    }

    fn op_with_pii() -> DocOp {
        let mut op = DocOp::cas(
            OpId::new("client-1", 7),
            "author-opaque",
            OpKind::Insert,
            b"alice@example.com SECRET BODY".to_vec(),
        );
        op.pii_key_ref = Some("dek:page-9:run-3".into());
        op
    }

    fn doc_op<'a>(op: &'a DocOp, page_id: &'a str) -> CrossCellDocOp<'a> {
        CrossCellDocOp {
            tenant: Box::leak(Box::new(tenant())),
            page_id,
            op,
        }
    }

    struct HomeCellResolver {
        allowed: Vec<String>,
        rendered: String,
    }

    impl CellLocalDocResolution for HomeCellResolver {
        fn resolve_in_home_cell(
            &self,
            pointer: &CrossCellDocPointer,
            viewer: &Principal,
        ) -> DocProjection {
            let subject = pointer.subject().clone();
            if self.allowed.iter().any(|id| id == &viewer.principal_id.0) {
                DocProjection::Rendered {
                    subject,
                    rendered: self.rendered.clone(),
                }
            } else {
                DocProjection::Tombstone { subject }
            }
        }
    }

    #[test]
    fn cross_cell_fan_out_carries_only_the_pointer_zero_pii() {
        let collab = CrossCellCollab::new(CellId::from_token("cell-fr-par-1"));
        let op = op_with_pii();
        let dop = doc_op(&op, "page-9");
        let members = vec![
            CellId::from_token("cell-fr-par-1"),
            CellId::from_token("cell-de-1"),
            CellId::from_token("cell-nl-1"),
        ];
        let corr = CorrelationId("op-causal-root".into());
        let fanned = collab.fan_out_doc_op(&dop, &corr, &members);

        let dests: Vec<&str> = fanned.iter().map(|p| p.to_cell.as_str()).collect();
        assert_eq!(dests, vec!["cell-de-1", "cell-nl-1"]);
        assert_eq!(collab.ops_fanned_out(), 2);

        for pp in &fanned {
            let (to, subject, kind, corr_field, home) = fanned_out_carried_fields(pp);
            assert_eq!(
                subject.artifact_ref().0,
                "myelin://acme/knowledge/page/page-9"
            );
            assert_eq!(kind, &myelin_events::ArtifactType::Page);
            assert_eq!(corr_field, &corr);
            assert_eq!(home.as_str(), "cell-fr-par-1");
            assert!(matches!(to.as_str(), "cell-de-1" | "cell-nl-1"));

            let wire = serde_json::to_string(&pp.pointer).expect("pointer serialises");
            assert!(
                !wire.contains("alice@example.com"),
                "payload email NEVER crosses: {wire}"
            );
            assert!(
                !wire.contains("SECRET"),
                "payload body NEVER crosses: {wire}"
            );
            assert!(!wire.contains("dek:"), "the DEK ref NEVER crosses: {wire}");
            assert!(
                !wire.contains("payload"),
                "no payload field on the frame: {wire}"
            );
        }
        assert_eq!(collab.cross_cell_pii_crossed(), 0);
    }

    #[test]
    fn single_cell_pin_lifted_fan_out_reaches_every_other_member_cell() {
        let collab = CrossCellCollab::new(CellId::from_token("cell-a"));
        let op = op_with_pii();
        let dop = doc_op(&op, "page-1");
        let members = vec![
            CellId::from_token("cell-a"),
            CellId::from_token("cell-b"),
            CellId::from_token("cell-c"),
            CellId::from_token("cell-d"),
        ];
        let corr = CorrelationId("root".into());
        let fanned = collab.fan_out_doc_op(&dop, &corr, &members);

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
                "rides the op causal-root"
            );
        }
    }

    #[test]
    fn single_home_cell_tenant_fans_out_nothing() {
        let collab = CrossCellCollab::new(CellId::from_token("cell-a"));
        let op = op_with_pii();
        let dop = doc_op(&op, "page-1");
        let fanned = collab.fan_out_doc_op(
            &dop,
            &CorrelationId("root".into()),
            &[CellId::from_token("cell-a")],
        );
        assert!(
            fanned.is_empty(),
            "a single-home-cell tenant has nowhere to fan out"
        );
        assert_eq!(collab.ops_fanned_out(), 0);
        assert_eq!(collab.cross_cell_pii_crossed(), 0);
    }

    #[test]
    fn resolution_stays_cell_local_only_the_projection_crosses() {
        let collab = CrossCellCollab::new(CellId::from_token("cell-fr-par-1"));
        let op = op_with_pii();
        let dop = doc_op(&op, "page-9");
        let fanned = collab.fan_out_doc_op(
            &dop,
            &CorrelationId("root".into()),
            &[CellId::from_token("cell-de-1")],
        );
        let pointer = &fanned[0];

        let home = HomeCellResolver {
            allowed: vec!["viewer-opaque".into()],
            rendered: "Doc title + body (rendered in fr-par-1, viewer-scoped)".into(),
        };
        let proj = collab.resolve_cell_local(pointer, &viewer(), &home);
        assert!(
            proj.is_rendered(),
            "an allowed viewer gets the rendered projection"
        );
        assert_eq!(proj.subject().0, "myelin://acme/knowledge/page/page-9");
        if let DocProjection::Rendered { rendered, .. } = &proj {
            assert!(rendered.contains("rendered in fr-par-1"));
            assert!(
                !rendered.contains("alice@example.com"),
                "no payload PII in the projection"
            );
            assert!(
                !rendered.contains("dek:"),
                "no DEK material in the projection"
            );
        }
    }

    #[test]
    fn unauthorised_cross_cell_viewer_resolves_to_a_tombstone() {
        let collab = CrossCellCollab::new(CellId::from_token("cell-fr-par-1"));
        let op = op_with_pii();
        let dop = doc_op(&op, "page-9");
        let fanned = collab.fan_out_doc_op(
            &dop,
            &CorrelationId("root".into()),
            &[CellId::from_token("cell-de-1")],
        );
        let home = HomeCellResolver {
            allowed: vec![],
            rendered: "should never be returned".into(),
        };
        let proj = collab.resolve_cell_local(&fanned[0], &viewer(), &home);
        assert!(
            !proj.is_rendered(),
            "an unauthorised viewer gets a tombstone"
        );
        assert!(matches!(proj, DocProjection::Tombstone { .. }));
        let wire = serde_json::to_string(&proj).expect("projection serialises");
        assert!(!wire.contains("alice@example.com"));
        assert!(!wire.contains("should never be returned"));
    }

    #[test]
    fn cdc_12_6_knowledge_consumer_reads_only_the_four_fields() {
        let collab = CrossCellCollab::new(CellId::from_token("cell-fr-par-1"));
        let op = op_with_pii();
        let dop = doc_op(&op, "page-9");
        let fanned = collab.fan_out_doc_op(
            &dop,
            &CorrelationId("root".into()),
            &[CellId::from_token("cell-de-1")],
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
        assert_eq!(propagated.stream, CrossCellStream::KnowledgeCollab);
        assert_eq!(
            propagated.pointer.artifact_type(),
            &myelin_events::ArtifactType::Page
        );
    }

    #[test]
    fn collab_debug_is_pii_free() {
        let collab = CrossCellCollab::new(CellId::from_token("cell-fr-par-1"));
        let op = op_with_pii();
        let dop = doc_op(&op, "page-9");
        let _ = collab.fan_out_doc_op(
            &dop,
            &CorrelationId("root".into()),
            &[CellId::from_token("cell-de-1")],
        );
        let dbg = format!("{collab:?}");
        assert!(
            dbg.contains("cell-fr-par-1"),
            "Debug shows the home cell id: {dbg}"
        );
        assert!(
            dbg.contains("ops_fanned_out"),
            "Debug shows the counter: {dbg}"
        );
        assert!(
            !dbg.contains("alice@example.com"),
            "Debug leaks no payload PII: {dbg}"
        );
        assert!(!dbg.contains("dek:"), "Debug leaks no DEK material: {dbg}");
    }
}

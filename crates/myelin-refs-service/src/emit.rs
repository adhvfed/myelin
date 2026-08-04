use myelin_content::InlineNode;
use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventDraft, EventEnvelope, EventId, EventType, OutboxTx,
    Result, Visibility,
};
use myelin_identity::Principal;

use crate::edge_builder::RelClass;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EdgeRel {
    Mentions,
    Links,
    Embeds,
}

impl EdgeRel {
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeRel::Mentions => "mentions",
            EdgeRel::Links => "links",
            EdgeRel::Embeds => "embeds",
        }
    }
}

pub fn edge_aggregate_key(source: &ArtifactRef, target: &ArtifactRef) -> AggregateKey {
    AggregateKey(format!("edge:{}->{}", source.0, target.0))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeDraft {
    pub source: ArtifactRef,
    pub target: ArtifactRef,
    pub rel: EdgeRel,
    pub rel_class: RelClass,
}

fn principal_member_ref(p: &Principal) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{}/identity/member/{}",
        p.tenant.0, p.principal_id.0
    ))
}

pub fn extract_edges(source: &ArtifactRef, doc: &[InlineNode]) -> Vec<EdgeDraft> {
    doc.iter()
        .map(|node| {
            let (target, rel) = match node {
                InlineNode::Mention(principal) => {
                    (principal_member_ref(principal), EdgeRel::Mentions)
                }
                InlineNode::ArtifactRefNode(target) => (target.clone(), EdgeRel::Links),
                InlineNode::Embed(target) => (target.clone(), EdgeRel::Embeds),
            };
            EdgeDraft {
                source: source.clone(),
                target,
                rel,
                rel_class: RelClass::Reference,
            }
        })
        .collect()
}

pub const REFS_EDGE_CREATED: &str = "refs.edge.created";

fn edge_event_draft(edge: &EdgeDraft) -> EventDraft {
    EventDraft {
        type_: EventType(REFS_EDGE_CREATED.into()),
        subject: edge.source.clone(),
        aggregate: edge_aggregate_key(&edge.source, &edge.target),
        payload: serde_json::json!({
            "source": edge.source.0,
            "target": edge.target.0,
            "rel": edge.rel.as_str(),
            "rel_class": edge.rel_class.as_str(),
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

pub fn emit_edges(
    tx: &mut dyn OutboxTx,
    source: &ArtifactRef,
    doc: &[InlineNode],
    content_event: &EventEnvelope,
) -> Result<Vec<EventId>> {
    let edges = extract_edges(source, doc);
    let mut ids = Vec::with_capacity(edges.len());
    for edge in &edges {
        let id = tx.emit(edge_event_draft(edge), Some(content_event))?;
        ids.push(id);
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn source_doc() -> ArtifactRef {
        ArtifactRef("myelin://acme/chat/message/m1".into())
    }

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p-opaque-7".into()),
            PrincipalKind::Human,
            tenant(),
        )
    }

    #[test]
    fn each_node_kind_yields_one_edge_with_correct_rel_and_class() {
        let src = source_doc();
        let target = ArtifactRef("myelin://acme/knowledge/page/7c2".into());
        let doc = vec![
            InlineNode::Mention(principal()),
            InlineNode::ArtifactRefNode(target.clone()),
            InlineNode::Embed(target.clone()),
        ];
        let edges = extract_edges(&src, &doc);
        assert_eq!(edges.len(), 3, "N structured nodes → N edges");

        assert_eq!(edges[0].rel, EdgeRel::Mentions);
        assert_eq!(edges[0].rel.as_str(), "mentions");
        assert_eq!(edges[0].rel_class, RelClass::Reference);
        assert_eq!(edges[0].source, src);
        assert_eq!(
            edges[0].target.0, "myelin://acme/identity/member/p-opaque-7",
            "mention target is the pseudonymous member URN, never the name"
        );

        assert_eq!(edges[1].rel, EdgeRel::Links);
        assert_eq!(edges[1].rel.as_str(), "links");
        assert_eq!(edges[1].target, target);

        assert_eq!(edges[2].rel, EdgeRel::Embeds);
        assert_eq!(edges[2].rel.as_str(), "embeds");
        assert_eq!(edges[2].target, target);
    }

    #[test]
    fn document_with_no_ref_nodes_yields_zero_edges() {
        let edges = extract_edges(&source_doc(), &[]);
        assert!(edges.is_empty(), "no structured nodes → no edges");
    }

    #[test]
    fn edge_event_draft_is_refs_edge_created_with_the_triple() {
        let target = ArtifactRef("myelin://acme/knowledge/page/7c2#block-3".into());
        let edge = EdgeDraft {
            source: source_doc(),
            target: target.clone(),
            rel: EdgeRel::Embeds,
            rel_class: RelClass::Reference,
        };
        let draft = edge_event_draft(&edge);
        assert_eq!(draft.type_.0, "refs.edge.created");
        assert_eq!(
            draft.subject,
            source_doc(),
            "the subject is the referencing content"
        );
        assert_eq!(draft.payload["source"], "myelin://acme/chat/message/m1");
        assert_eq!(draft.payload["target"], target.0);
        assert_eq!(draft.payload["rel"], "embeds");
        assert_eq!(draft.payload["rel_class"], "reference");
        assert!(
            !draft.contains_personal_data,
            "references-not-payloads: no inline PII"
        );
        assert!(draft.pii_key_ref.is_none());
    }

    #[test]
    fn refs_edge_created_token_is_frozen() {
        assert_eq!(REFS_EDGE_CREATED, "refs.edge.created");
    }
}

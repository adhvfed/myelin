use myelin_content::{parse_inline, serialize_inline, Inline, InlineNode};
use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventDraft, EventEnvelope, EventId, EventType, OutboxTx,
    Result as BusResult, Visibility,
};
use myelin_identity::Principal;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Body {
    pub md: String,
    pub nodes: Vec<InlineNode>,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CasConflict {
    pub expected: u64,
    pub actual: u64,
}

impl std::fmt::Display for CasConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "single-author CAS conflict: edit expected revision {} but the body is at {}",
            self.expected, self.actual
        )
    }
}

impl std::error::Error for CasConflict {}

impl Body {
    pub fn new(md: impl Into<String>, nodes: Vec<InlineNode>) -> Body {
        Body {
            md: md.into(),
            nodes,
            revision: 0,
        }
    }

    pub fn empty() -> Body {
        Body::default()
    }

    pub fn parse(&self) -> Inline {
        parse_inline(&self.md, &self.nodes)
    }

    pub fn render(&self) -> String {
        serialize_inline(&self.parse())
    }

    pub fn round_trips(&self) -> bool {
        self.render() == self.md
    }

    pub fn structured_nodes(&self) -> &[InlineNode] {
        &self.nodes
    }

    pub fn cas_edit(
        &mut self,
        expected_revision: u64,
        md: impl Into<String>,
        nodes: Vec<InlineNode>,
    ) -> Result<u64, CasConflict> {
        if expected_revision != self.revision {
            return Err(CasConflict {
                expected: expected_revision,
                actual: self.revision,
            });
        }
        self.md = md.into();
        self.nodes = nodes;
        self.revision += 1;
        Ok(self.revision)
    }
}

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

pub const REL_CLASS_REFERENCE: &str = "reference";

pub const REFS_EDGE_CREATED: &str = "refs.edge.created";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyEdge {
    pub source: ArtifactRef,
    pub target: ArtifactRef,
    pub rel: EdgeRel,
}

pub fn edge_aggregate_key(source: &ArtifactRef, target: &ArtifactRef) -> AggregateKey {
    AggregateKey(format!("edge:{}->{}", source.0, target.0))
}

fn principal_member_ref(p: &Principal) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{}/identity/member/{}",
        p.tenant.0, p.principal_id.0
    ))
}

pub fn extract_body_edges(source: &ArtifactRef, nodes: &[InlineNode]) -> Vec<BodyEdge> {
    nodes
        .iter()
        .map(|node| {
            let (target, rel) = match node {
                InlineNode::Mention(principal) => {
                    (principal_member_ref(principal), EdgeRel::Mentions)
                }
                InlineNode::ArtifactRefNode(target) => (target.clone(), EdgeRel::Links),
                InlineNode::Embed(target) => (target.clone(), EdgeRel::Embeds),
            };
            BodyEdge {
                source: source.clone(),
                target,
                rel,
            }
        })
        .collect()
}

fn edge_event_draft(edge: &BodyEdge) -> EventDraft {
    EventDraft {
        type_: EventType(REFS_EDGE_CREATED.into()),
        subject: edge.source.clone(),
        aggregate: edge_aggregate_key(&edge.source, &edge.target),
        payload: serde_json::json!({
            "source": edge.source.0,
            "target": edge.target.0,
            "rel": edge.rel.as_str(),
            "rel_class": REL_CLASS_REFERENCE,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

pub fn emit_body_edges(
    tx: &mut dyn OutboxTx,
    source: &ArtifactRef,
    nodes: &[InlineNode],
    content_event: &EventEnvelope,
) -> BusResult<Vec<EventId>> {
    let edges = extract_body_edges(source, nodes);
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
    use myelin_content::OBJ;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }

    fn comment_source() -> ArtifactRef {
        crate::subs::mint_pr_comment("acme", "repo7", 42, "cAbc").unwrap()
    }

    fn alice() -> Principal {
        Principal::stub(
            PrincipalId("p-opaque-alice".into()),
            PrincipalKind::Human,
            tenant(),
        )
    }

    #[test]
    fn body_round_trips_byte_identical() {
        let body = Body::new(
            format!("**bold** and *italic* with `code` and a {OBJ} mention"),
            vec![InlineNode::Mention(alice())],
        );
        assert!(body.round_trips(), "render(parse(md)) must === md");
        assert_eq!(body.render(), body.md);
    }

    #[test]
    fn empty_body_round_trips_and_has_no_edges() {
        let body = Body::empty();
        assert!(body.round_trips());
        assert!(extract_body_edges(&comment_source(), body.structured_nodes()).is_empty());
    }

    #[test]
    fn round_trips_is_false_on_a_non_canonical_body() {
        let non_canonical = Body::new("a*b", vec![]);
        assert!(
            !non_canonical.round_trips(),
            "a non-canonical body must NOT round-trip"
        );
        assert_eq!(
            non_canonical.render(),
            "a\\*b",
            "the serializer canonicalises the literal `*`"
        );
        assert!(
            Body::new("a\\*b", vec![]).round_trips(),
            "the canonical form IS a fixed point"
        );
    }

    #[test]
    fn cas_conflict_display_names_the_revisions() {
        let msg = CasConflict {
            expected: 0,
            actual: 3,
        }
        .to_string();
        assert!(
            msg.contains('0') && msg.contains('3'),
            "the message names both revisions: {msg}"
        );
        assert!(
            msg.to_lowercase().contains("cas"),
            "the message is the CAS-conflict surface: {msg}"
        );
    }

    #[test]
    fn body_cas_edit_rejects_stale_revision() {
        let mut body = Body::new("v0", vec![]);
        assert_eq!(body.revision, 0);
        assert_eq!(body.cas_edit(0, "v1", vec![]).unwrap(), 1);
        assert_eq!(body.md, "v1");
        let conflict = body.cas_edit(0, "v2-stale", vec![]).unwrap_err();
        assert_eq!(
            conflict,
            CasConflict {
                expected: 0,
                actual: 1
            }
        );
        assert_eq!(
            body.md, "v1",
            "a rejected CAS edit does NOT mutate the body"
        );
        assert_eq!(body.revision, 1);
    }

    #[test]
    fn each_node_kind_yields_one_edge_with_correct_rel_and_target() {
        let src = comment_source();
        let page = ArtifactRef("myelin://acme/knowledge/page/7c2".into());
        let issue = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());
        let nodes = vec![
            InlineNode::Mention(alice()),
            InlineNode::ArtifactRefNode(issue.clone()),
            InlineNode::Embed(page.clone()),
        ];
        let edges = extract_body_edges(&src, &nodes);
        assert_eq!(edges.len(), 3, "N structured nodes → N edges (1 per node)");

        assert_eq!(edges[0].rel, EdgeRel::Mentions);
        assert_eq!(edges[0].rel.as_str(), "mentions");
        assert_eq!(edges[0].source, src);
        assert_eq!(
            edges[0].target.0, "myelin://acme/identity/member/p-opaque-alice",
            "mention target is the pseudonymous member URN, never the name"
        );

        assert_eq!(edges[1].rel, EdgeRel::Links);
        assert_eq!(edges[1].rel.as_str(), "links");
        assert_eq!(edges[1].target, issue);

        assert_eq!(edges[2].rel, EdgeRel::Embeds);
        assert_eq!(edges[2].rel.as_str(), "embeds");
        assert_eq!(edges[2].target, page);
    }

    #[test]
    fn prose_closes_trailer_is_not_a_content_edge() {
        let body = Body::new("Closes ENG-1 and fixes the bug.", vec![]);
        assert!(body.round_trips());
        let edges = extract_body_edges(&comment_source(), body.structured_nodes());
        assert!(
            edges.is_empty(),
            "a prose `Closes` is NOT a content edge (that is GIT-P19's mirror)"
        );
    }

    #[test]
    fn edge_event_draft_is_refs_edge_created_with_the_triple() {
        let src = comment_source();
        let target = ArtifactRef("myelin://acme/knowledge/page/7c2#block-3".into());
        let edge = BodyEdge {
            source: src.clone(),
            target: target.clone(),
            rel: EdgeRel::Embeds,
        };
        let draft = edge_event_draft(&edge);
        assert_eq!(draft.type_.0, "refs.edge.created");
        assert_eq!(draft.subject, src, "the subject is the referencing body");
        assert_eq!(draft.payload["source"], src.0);
        assert_eq!(draft.payload["target"], target.0);
        assert_eq!(draft.payload["rel"], "embeds");
        assert_eq!(draft.payload["rel_class"], "reference");
        assert_eq!(draft.aggregate.0, format!("edge:{}->{}", src.0, target.0));
        assert!(
            !draft.contains_personal_data,
            "references-not-payloads: no inline PII"
        );
        assert!(draft.pii_key_ref.is_none());
        assert_eq!(draft.data_role, DataRole::Controller);
    }

    #[test]
    fn frozen_tokens_match_the_refs_wire_shape() {
        assert_eq!(REFS_EDGE_CREATED, "refs.edge.created");
        assert_eq!(REL_CLASS_REFERENCE, "reference");
        assert_eq!(EdgeRel::Mentions.as_str(), "mentions");
        assert_eq!(EdgeRel::Links.as_str(), "links");
        assert_eq!(EdgeRel::Embeds.as_str(), "embeds");
    }
}

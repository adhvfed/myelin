use myelin_events::{ArtifactRef, DataRole, EventDraft, EventType, Visibility};
use myelin_identity::PrincipalId;
use myelin_tenancy::TenantId;

pub const REFS_EDGE_CREATED: &str = "refs.edge.created";
pub const REFS_EDGE_REMOVED: &str = "refs.edge.removed";
pub const REL_CLASS_REFERENCE: &str = "reference";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeChange {
    Created,
    Removed,
}

impl EdgeChange {
    fn event_type(self) -> &'static str {
        match self {
            EdgeChange::Created => REFS_EDGE_CREATED,
            EdgeChange::Removed => REFS_EDGE_REMOVED,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReferenceRel {
    Mentions,
    Links,
    Embeds,
}

impl ReferenceRel {
    pub fn as_str(self) -> &'static str {
        match self {
            ReferenceRel::Mentions => "mentions",
            ReferenceRel::Links => "links",
            ReferenceRel::Embeds => "embeds",
        }
    }
}

pub fn identity_member_ref(tenant: &TenantId, principal: &PrincipalId) -> ArtifactRef {
    ArtifactRef(format!(
        "myelin://{}/identity/member/{}",
        tenant.0, principal.0
    ))
}

pub fn reference_edge_draft(
    source: &ArtifactRef,
    target: &ArtifactRef,
    relation: ReferenceRel,
    change: EdgeChange,
) -> EventDraft {
    EventDraft {
        type_: EventType(change.event_type().into()),
        subject: source.clone(),
        aggregate: crate::edge_aggregate_key(source, target),
        payload: serde_json::json!({
            "source": source.0,
            "target": target.0,
            "rel": relation.as_str(),
            "rel_class": REL_CLASS_REFERENCE,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;

    #[test]
    fn reference_drafts_share_one_canonical_vocabulary() {
        let source = ArtifactRef("myelin://acme/knowledge/page/01JPAGE".into());
        let target = ArtifactRef("myelin://acme/issue/issue/ENG-1".into());

        for (relation, token) in [
            (ReferenceRel::Mentions, "mentions"),
            (ReferenceRel::Links, "links"),
            (ReferenceRel::Embeds, "embeds"),
        ] {
            let created = reference_edge_draft(&source, &target, relation, EdgeChange::Created);
            let removed = reference_edge_draft(&source, &target, relation, EdgeChange::Removed);
            assert_eq!(created.type_.0, REFS_EDGE_CREATED);
            assert_eq!(removed.type_.0, REFS_EDGE_REMOVED);
            assert_eq!(created.payload["rel"], token);
            assert_eq!(created.payload["rel_class"], REL_CLASS_REFERENCE);
            assert_eq!(
                created.aggregate,
                crate::edge_aggregate_key(&source, &target)
            );
        }
    }

    #[test]
    fn mentions_target_the_canonical_identity_member_artifact() {
        let principal = Principal::stub(
            PrincipalId("alice".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        );
        assert_eq!(
            identity_member_ref(&principal.tenant, &principal.principal_id).0,
            "myelin://acme/identity/member/alice"
        );
    }
}

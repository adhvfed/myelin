use crate::EdgeError;
use myelin_chat::conversation::Conversation;
use myelin_chat::membership::{channel_object, permissions};
use myelin_events::Timestamp;
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, ObjectId, Permission, Principal,
    PrincipalId, RelName, RelationTuple, TupleDelta, Zookie,
};
use myelin_identity_service::{project_ref, StoreBackedCheck};
use myelin_storage::TenantScope;
use myelin_tenancy::ArtifactRef;

const PARENT_PROJECT: &str = "parent_project";

#[derive(Clone)]
pub(crate) struct ChatAuthorization {
    identity: StoreBackedCheck,
}

impl ChatAuthorization {
    pub(crate) fn new(identity: StoreBackedCheck) -> Self {
        Self { identity }
    }

    pub(crate) fn may_view_project(&self, principal: &Principal, project_id: &str) -> bool {
        self.allows(
            principal,
            "view",
            &project_ref(&principal.tenant.0, project_id),
        )
    }

    pub(crate) fn may_read_channel(
        &self,
        principal: &Principal,
        conversation: &Conversation,
    ) -> bool {
        self.may_use_channel(principal, conversation, permissions::READ)
    }

    pub(crate) fn may_post_to_channel(
        &self,
        principal: &Principal,
        conversation: &Conversation,
    ) -> bool {
        self.may_use_channel(principal, conversation, permissions::POST)
    }

    pub(crate) fn bind_public_project(
        &self,
        actor: &Principal,
        conversation: &Conversation,
        occurred_at: Timestamp,
    ) -> Result<Zookie, EdgeError> {
        let project_id = conversation.parent_project.as_deref().ok_or_else(|| {
            EdgeError::Internal("a public Chat conversation has no parent project".into())
        })?;
        if conversation.id.tenant != actor.tenant.0 || conversation.id.region != actor.region.0 {
            return Err(EdgeError::Internal(
                "a Chat authorization grant crossed its verified tenant scope".into(),
            ));
        }
        if !self.may_view_project(actor, project_id) {
            return Err(EdgeError::NotFound("project not found".into()));
        }

        let channel = ObjectId(channel_object(&conversation.id.conversation_id));
        let project_view = PrincipalId(format!("project:{project_id}#view"));
        let relation = |name: &str| RelationTuple {
            object: channel.clone(),
            relation: RelName(name.into()),
            subject: project_view.clone(),
            caveat: None,
        };
        let deltas = [
            TupleDelta::Add(relation(PARENT_PROJECT)),
            TupleDelta::Add(relation("member")),
        ];
        let scope = TenantScope::from_verified_token(actor, actor.region.clone());
        self.identity
            .tuples()
            .write_tuples(&scope, actor, &deltas, None, None, occurred_at)
            .map_err(|error| {
                EdgeError::Internal(format!(
                    "Chat authorization could not be persisted: {error}"
                ))
            })
    }

    fn may_use_channel(
        &self,
        principal: &Principal,
        conversation: &Conversation,
        permission: &str,
    ) -> bool {
        if conversation.acl_zookie.is_none() {
            return false;
        }
        self.allows(
            principal,
            permission,
            &ArtifactRef(channel_object(&conversation.id.conversation_id)),
        )
    }

    fn allows(&self, principal: &Principal, permission: &str, object: &ArtifactRef) -> bool {
        let consistency = Consistency {
            at_least: Zookie(String::new()),
            mode: ConsistencyMode::Strong,
        };
        matches!(
            self.identity.check(
                principal,
                &Permission(permission.into()),
                object,
                &consistency,
                None,
            ),
            Ok(Decision::Allow)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_chat::conversation::{Conversation, ConversationKind};
    use myelin_chat::store::ConversationId;
    use myelin_events::OutboxStore;
    use myelin_identity::{FragmentAdmit, PrincipalKind};
    use myelin_identity_service::TupleStore;
    use myelin_tenancy::TenantId;

    const PROJECT_ID: &str = "11111111-1111-1111-1111-111111111111";

    fn principal(id: &str) -> Principal {
        Principal::stub(
            PrincipalId(id.into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn conversation(actor: &Principal) -> Conversation {
        let id = ConversationId::new(
            actor.tenant.0.clone(),
            actor.region.0.clone(),
            "01J00000000000000000000000",
        );
        Conversation {
            home_cell: Conversation::home_cell_for(&id),
            id,
            kind: ConversationKind::ChannelPublic,
            parent_project: Some(PROJECT_ID.into()),
            name: Some("engineering".into()),
            topic: Some("release".into()),
            linked_ref: None,
            pinned_canvas: None,
            retention_days: None,
            archived: false,
            created_by: actor.principal_id.0.clone(),
            acl_zookie: None,
        }
    }

    #[test]
    fn a_public_room_follows_its_projects_live_viewers() {
        let owner = principal("owner");
        let collaborator = principal("collaborator");
        let outsider = principal("outsider");
        let tuples = TupleStore::new(OutboxStore::new());
        let identity = StoreBackedCheck::new(tuples.clone());
        for admitted in identity.admit_chat_fragment() {
            assert!(matches!(admitted, FragmentAdmit::Admitted { .. }));
        }
        let scope = TenantScope::from_verified_token(&owner, owner.region.clone());
        let project = ObjectId(format!("project:{PROJECT_ID}"));
        let grant = |subject: &Principal, relation: &str| RelationTuple {
            object: project.clone(),
            relation: RelName(relation.into()),
            subject: subject.principal_id.clone(),
            caveat: None,
        };
        tuples
            .write_tuples(
                &scope,
                &owner,
                &[TupleDelta::Add(grant(&owner, "writer"))],
                None,
                None,
                Timestamp("2026-08-11T00:00:00Z".into()),
            )
            .unwrap();

        let authorization = ChatAuthorization::new(identity);
        let mut room = conversation(&owner);
        room.acl_zookie = Some(
            authorization
                .bind_public_project(&owner, &room, Timestamp("2026-08-11T00:00:01Z".into()))
                .unwrap()
                .0,
        );

        assert!(!authorization.may_read_channel(&collaborator, &room));
        tuples
            .write_tuples(
                &scope,
                &owner,
                &[TupleDelta::Add(grant(&collaborator, "reader"))],
                None,
                None,
                Timestamp("2026-08-11T00:00:02Z".into()),
            )
            .unwrap();
        assert!(authorization.may_read_channel(&collaborator, &room));
        assert!(authorization.may_post_to_channel(&collaborator, &room));
        assert!(!authorization.may_read_channel(&outsider, &room));
        assert!(!authorization.may_post_to_channel(&outsider, &room));

        tuples
            .write_tuples(
                &scope,
                &owner,
                &[TupleDelta::Remove(grant(&collaborator, "reader"))],
                None,
                None,
                Timestamp("2026-08-11T00:00:03Z".into()),
            )
            .unwrap();
        assert!(!authorization.may_read_channel(&collaborator, &room));
        assert!(!authorization.may_post_to_channel(&collaborator, &room));
    }
}

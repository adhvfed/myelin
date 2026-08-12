use myelin_events::{AggregateKey, DataRole, EventDraft, EventType, OutboxTx as _, Visibility};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, Permission, Precondition, Principal,
    RelName, RelationTuple, TupleDelta, Zookie,
};

use crate::conversation::{
    Conversation, ConversationError, ConversationStore, Membership, MembershipRole,
};
use crate::events::{
    CHAT_CHANNEL_ARCHIVED, CHAT_CHANNEL_CREATED, CHAT_CHANNEL_LINKED, CHAT_CHANNEL_MEMBER_ADDED,
    CHAT_CHANNEL_MEMBER_REMOVED,
};
use crate::rebac_fragment::object_types;
use crate::store::{ConversationId, OutboxTx};

pub mod permissions {
    pub const POST: &str = "post";
    pub const READ: &str = "read";
    pub const MANAGE: &str = "manage";
}

#[derive(Debug, PartialEq, Eq)]
pub enum MembershipError {
    NotFound(String),
    TupleWrite(String),
    Denied { permission: String, channel: String },
    Emit(String),
    Store(String),
}

impl core::fmt::Display for MembershipError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MembershipError::NotFound(id) => write!(f, "conversation {id} not found"),
            MembershipError::TupleWrite(e) => write!(f, "write_tuples failed: {e}"),
            MembershipError::Denied {
                permission,
                channel,
            } => write!(f, "permission `{permission}` denied on channel {channel}"),
            MembershipError::Emit(e) => write!(f, "outbox emit failed: {e}"),
            MembershipError::Store(e) => write!(f, "conversation store failed: {e}"),
        }
    }
}

impl std::error::Error for MembershipError {}

impl From<ConversationError> for MembershipError {
    fn from(e: ConversationError) -> MembershipError {
        match e {
            ConversationError::NotFound(id) => MembershipError::NotFound(id),
            other => MembershipError::Store(other.to_string()),
        }
    }
}

pub type Result<T> = core::result::Result<T, MembershipError>;

pub trait MembershipTupleWriter {
    fn write_membership_tuples(
        &self,
        deltas: &[TupleDelta],
        precondition: Option<&Precondition>,
    ) -> core::result::Result<Zookie, String>;
}

pub struct MembershipGate<I: IdentityService> {
    id: I,
}

impl<I: IdentityService> MembershipGate<I> {
    pub fn new(id: I) -> MembershipGate<I> {
        MembershipGate { id }
    }

    pub fn check_channel(
        &self,
        subject: &Principal,
        permission: &str,
        channel_id: &ConversationId,
        at_zookie: Option<&str>,
    ) -> Result<()> {
        let object = myelin_tenancy::ArtifactRef(channel_object(&channel_id.conversation_id));
        let at = Consistency {
            at_least: Zookie(at_zookie.unwrap_or("").to_string()),
            mode: ConsistencyMode::Strong,
        };
        let permission_tok = Permission(permission.to_string());
        match self.id.check(subject, &permission_tok, &object, &at, None) {
            Ok(Decision::Allow) => Ok(()),
            Ok(Decision::Deny) | Ok(Decision::Conditional) | Err(_) => {
                Err(MembershipError::Denied {
                    permission: permission.to_string(),
                    channel: channel_id.conversation_id.clone(),
                })
            }
        }
    }

    pub fn check_send(
        &self,
        subject: &Principal,
        channel_id: &ConversationId,
        at_zookie: Option<&str>,
    ) -> Result<()> {
        self.check_channel(subject, permissions::POST, channel_id, at_zookie)
    }

    pub fn check_manage(
        &self,
        subject: &Principal,
        channel_id: &ConversationId,
        at_zookie: Option<&str>,
    ) -> Result<()> {
        self.check_channel(subject, permissions::MANAGE, channel_id, at_zookie)
    }
}

pub fn channel_object(channel_id: &str) -> String {
    format!("{}:{}", object_types::CHANNEL, channel_id)
}

pub struct MembershipService<W: MembershipTupleWriter> {
    writer: W,
}

impl<W: MembershipTupleWriter> MembershipService<W> {
    pub fn new(writer: W) -> MembershipService<W> {
        MembershipService { writer }
    }

    const MEMBER_REL: &'static str = "member";
    const WATCHER_REL: &'static str = "watcher";

    fn member_tuples(channel_id: &str, m: &Membership, add: bool) -> Vec<TupleDelta> {
        let object = channel_object(channel_id);
        let mut deltas = Vec::with_capacity(2);
        let member_tuple = RelationTuple {
            object: myelin_identity::ObjectId(object.clone()),
            relation: RelName(Self::MEMBER_REL.to_string()),
            subject: myelin_identity::PrincipalId(m.principal_id.clone()),
            caveat: None,
        };
        let watcher_tuple = RelationTuple {
            object: myelin_identity::ObjectId(object),
            relation: RelName(Self::WATCHER_REL.to_string()),
            subject: myelin_identity::PrincipalId(m.principal_id.clone()),
            caveat: None,
        };
        if add {
            deltas.push(TupleDelta::Add(member_tuple));
            if m.is_watcher {
                deltas.push(TupleDelta::Add(watcher_tuple));
            }
        } else {
            deltas.push(TupleDelta::Remove(member_tuple));
            deltas.push(TupleDelta::Remove(watcher_tuple));
        }
        deltas
    }

    pub fn add_member(
        &self,
        tx: &mut OutboxTx,
        store: &dyn ConversationStore,
        m: Membership,
    ) -> Result<Zookie> {
        let channel_id = m.conv.clone();
        store.get(&channel_id)?;
        let deltas = Self::member_tuples(&channel_id.conversation_id, &m, true);
        let zookie = self
            .writer
            .write_membership_tuples(&deltas, None)
            .map_err(MembershipError::TupleWrite)?;
        store
            .stamp_acl_zookie(&channel_id, &zookie.0)
            .map_err(MembershipError::from)?;
        store.join(m.clone()).map_err(MembershipError::from)?;
        tx.stage_state_change(format!(
            "chat.channel.member_added:{}:{}",
            channel_id.conversation_id, m.principal_id
        ));
        self.emit_member_event(
            tx,
            CHAT_CHANNEL_MEMBER_ADDED,
            &channel_id,
            &m.principal_id,
            m.role,
        )?;
        Ok(zookie)
    }

    pub fn remove_member(
        &self,
        tx: &mut OutboxTx,
        store: &dyn ConversationStore,
        channel_id: &ConversationId,
        principal_id: &str,
        role: MembershipRole,
        is_watcher: bool,
    ) -> Result<Zookie> {
        store.get(channel_id)?;
        let m = Membership {
            conv: channel_id.clone(),
            principal_id: principal_id.to_string(),
            role,
            is_watcher,
            notif_pref: serde_json::Value::Null,
        };
        let deltas = Self::member_tuples(&channel_id.conversation_id, &m, false);
        let zookie = self
            .writer
            .write_membership_tuples(&deltas, None)
            .map_err(MembershipError::TupleWrite)?;
        store
            .stamp_acl_zookie(channel_id, &zookie.0)
            .map_err(MembershipError::from)?;
        store
            .leave(channel_id, principal_id)
            .map_err(MembershipError::from)?;
        tx.stage_state_change(format!(
            "chat.channel.member_removed:{}:{}",
            channel_id.conversation_id, principal_id
        ));
        self.emit_member_event(
            tx,
            CHAT_CHANNEL_MEMBER_REMOVED,
            channel_id,
            principal_id,
            role,
        )?;
        Ok(zookie)
    }

    pub fn create_channel(
        &self,
        tx: &mut OutboxTx,
        store: &dyn ConversationStore,
        conv: Conversation,
    ) -> Result<()> {
        let channel_id = conv.id.clone();
        let linked_ref = conv.linked_ref.clone();
        store.create(conv).map_err(MembershipError::from)?;
        tx.stage_state_change(format!(
            "chat.channel.created:{}",
            channel_id.conversation_id
        ));
        self.emit_channel_event(tx, CHAT_CHANNEL_CREATED, &channel_id, None)?;
        if let Some(linked) = linked_ref {
            self.emit_channel_event(tx, CHAT_CHANNEL_LINKED, &channel_id, Some(linked))?;
        }
        Ok(())
    }

    pub fn archive_channel(&self, tx: &mut OutboxTx, channel_id: &ConversationId) -> Result<()> {
        tx.stage_state_change(format!(
            "chat.channel.archived:{}",
            channel_id.conversation_id
        ));
        self.emit_channel_event(tx, CHAT_CHANNEL_ARCHIVED, channel_id, None)
    }

    pub fn link_channel(
        &self,
        tx: &mut OutboxTx,
        channel_id: &ConversationId,
        linked_ref: impl Into<String>,
    ) -> Result<()> {
        tx.stage_state_change(format!(
            "chat.channel.linked:{}",
            channel_id.conversation_id
        ));
        self.emit_channel_event(tx, CHAT_CHANNEL_LINKED, channel_id, Some(linked_ref.into()))
    }

    pub fn read_consistency(conv: &Conversation) -> Consistency {
        Consistency {
            at_least: Zookie(conv.acl_zookie.clone().unwrap_or_default()),
            mode: ConsistencyMode::Strong,
        }
    }

    fn emit_member_event(
        &self,
        tx: &mut OutboxTx,
        event_type: &str,
        channel_id: &ConversationId,
        principal_id: &str,
        role: MembershipRole,
    ) -> Result<()> {
        let subject = crate::subs::mint_channel(&channel_id.tenant, &channel_id.conversation_id)
            .map_err(|e| MembershipError::Emit(format!("mint channel ref: {e}")))?;
        let draft = EventDraft {
            type_: EventType(event_type.to_string()),
            subject,
            aggregate: AggregateKey(channel_id.conversation_id.clone()),
            payload: serde_json::json!({
                "conversation_id": channel_id.conversation_id,
                "principal": principal_id,
                "role": role.as_token(),
            }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        };
        tx.emit(draft, None)
            .map_err(|e| MembershipError::Emit(format!("emit {event_type}: {e:?}")))?;
        Ok(())
    }

    fn emit_channel_event(
        &self,
        tx: &mut OutboxTx,
        event_type: &str,
        channel_id: &ConversationId,
        linked_ref: Option<String>,
    ) -> Result<()> {
        let subject = crate::subs::mint_channel(&channel_id.tenant, &channel_id.conversation_id)
            .map_err(|e| MembershipError::Emit(format!("mint channel ref: {e}")))?;
        let mut payload = serde_json::json!({
            "conversation_id": channel_id.conversation_id,
        });
        if let Some(linked) = linked_ref {
            payload["linked_ref"] = serde_json::Value::String(linked);
        }
        let draft = EventDraft {
            type_: EventType(event_type.to_string()),
            subject,
            aggregate: AggregateKey(channel_id.conversation_id.clone()),
            payload,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        };
        tx.emit(draft, None)
            .map_err(|e| MembershipError::Emit(format!("emit {event_type}: {e:?}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;

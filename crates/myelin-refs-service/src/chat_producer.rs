use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_chat::rebac_fragment::object_types::CHANNEL;
use myelin_chat::subs::{mint_message, mint_thread, CHAT_SUBSYSTEM};
use myelin_content::InlineNode;
use myelin_events::{ArtifactRef, EventEnvelope, EventId, OutboxTx, Result as BusResult};
use myelin_identity::{Decision, Permission, Principal};
use myelin_refs::{sub_kind, ParseError, Sub};
use myelin_tenancy::{Region, TenantId};

use crate::emit::{emit_edges, EdgeDraft};
use crate::ladder::SubState;
use crate::resolve::{OwnerProjection, ProjectApi, ProjectApiError, ProjectOutcome, ResolveMode};
use crate::SubAnchorResolver;

pub const CHAT_OWNER_TOKEN: &str = CHAT_SUBSYSTEM;

pub const CHAT_CHANNEL_TYPE: &str = CHANNEL;

pub struct ChatEdgeProducer;

impl ChatEdgeProducer {
    pub fn message_root(tenant: &str, message_id: &str) -> Result<ArtifactRef, ParseError> {
        mint_message(tenant, message_id).map(|minted| myelin_refs::strip_sub(&minted))
    }

    pub fn thread_root(tenant: &str, thread_root_id: &str) -> Result<ArtifactRef, ParseError> {
        mint_thread(tenant, thread_root_id).map(|minted| myelin_refs::strip_sub(&minted))
    }

    pub fn emit_chat_edges(
        &self,
        tx: &mut dyn OutboxTx,
        source: &ArtifactRef,
        body: &[InlineNode],
        content_event: &EventEnvelope,
    ) -> BusResult<Vec<EventId>> {
        emit_edges(tx, source, body, content_event)
    }

    pub fn chat_edges(&self, source: &ArtifactRef, body: &[InlineNode]) -> Vec<EdgeDraft> {
        crate::extract_edges(source, body)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatAnchorState {
    Live,
    Deleted,
    Erased,
}

impl ChatAnchorState {
    fn into_sub_state(self, projection: OwnerProjection) -> SubState {
        match self {
            ChatAnchorState::Live => SubState::Live(projection),
            ChatAnchorState::Deleted => SubState::Gone,
            ChatAnchorState::Erased => SubState::Erased,
        }
    }
}

#[derive(Clone, Default)]
pub struct ChatOwner {
    acl: Arc<Mutex<BTreeMap<String, Decision>>>,
    anchors: Arc<Mutex<BTreeMap<String, ChatAnchorState>>>,
}

impl ChatOwner {
    pub fn new() -> ChatOwner {
        ChatOwner::default()
    }

    fn acl_key(
        tenant: &TenantId,
        region: &Region,
        viewer: &Principal,
        root: &ArtifactRef,
    ) -> String {
        format!(
            "{}|{}|{}|{}",
            tenant.0, region.0, viewer.principal_id.0, root.0
        )
    }

    pub fn grant_view(
        &self,
        tenant: &TenantId,
        region: &Region,
        viewer: &Principal,
        root: &ArtifactRef,
    ) {
        self.acl
            .lock()
            .unwrap()
            .insert(Self::acl_key(tenant, region, viewer, root), Decision::Allow);
    }

    pub fn record_anchor(&self, ref_: &ArtifactRef, state: ChatAnchorState) {
        self.anchors.lock().unwrap().insert(ref_.0.clone(), state);
    }

    fn projection(ref_: &ArtifactRef) -> OwnerProjection {
        OwnerProjection {
            title: "a chat message".into(),
            state: "live".into(),
            icon: "chat".into(),
            render_hint: "embed".into(),
            sub_anchor: sub_kind(ref_).is_some().then(|| ref_.0.clone()),
            flag: None,
        }
    }

    fn resolve_chat_sub(&self, ref_: &ArtifactRef, sub: Option<&Sub>) -> SubState {
        let projection = Self::projection(ref_);
        match sub {
            None => SubState::Live(projection),
            Some(Sub::Message(_)) | Some(Sub::Thread(_)) => self
                .anchors
                .lock()
                .unwrap()
                .get(&ref_.0)
                .copied()
                .map(|s| s.into_sub_state(projection.clone()))
                .unwrap_or(SubState::Gone),
            Some(_) => SubState::Live(projection),
        }
    }
}

impl ProjectApi for ChatOwner {
    fn check_view(
        &self,
        tenant: &TenantId,
        region: &Region,
        object: &ArtifactRef,
        viewer: &Principal,
        _permission: &Permission,
    ) -> std::result::Result<Decision, ProjectApiError> {
        let key = Self::acl_key(tenant, region, viewer, object);
        Ok(self
            .acl
            .lock()
            .unwrap()
            .get(&key)
            .copied()
            .unwrap_or(Decision::Deny))
    }

    fn project(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        ref_: &ArtifactRef,
        _viewer: &Principal,
        _mode: ResolveMode,
    ) -> std::result::Result<ProjectOutcome, ProjectApiError> {
        let sub = sub_kind(ref_);
        Ok(self.resolve_chat_sub(ref_, sub.as_ref()).into_outcome())
    }
}

impl SubAnchorResolver for ChatOwner {
    fn resolve_sub(&self, ref_: &ArtifactRef, sub: Option<&Sub>) -> SubState {
        self.resolve_chat_sub(ref_, sub)
    }
}

#[cfg(test)]
mod tests;

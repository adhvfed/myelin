use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_content::events::{KNOWLEDGE_PAGE_CREATED, KNOWLEDGE_PAGE_MOVED};
use myelin_content::InlineNode;
use myelin_events::{ArtifactRef, EventEnvelope, EventId, OutboxTx, Result as BusResult};
use myelin_identity::{Decision, Permission, Principal};
use myelin_refs::{sub_kind, Sub};
use myelin_tenancy::{Region, TenantId};

use crate::edge_builder::{EdgeProjection, EdgeRow};
use crate::emit::{emit_edges, EdgeDraft};
use crate::ladder::SubState;
use crate::mirror::{mirror_edges, reconverge, LifecycleRel, MirrorError, SyntheticTypedEvent};
use crate::resolve::{OwnerProjection, ProjectApi, ProjectApiError, ProjectOutcome, ResolveMode};
use crate::SubAnchorResolver;

pub const KN_OWNER_TOKEN: &str = "knowledge";

pub struct KnEdgeProducer;

impl KnEdgeProducer {
    pub fn page_root(tenant: &str, page_id: &str) -> ArtifactRef {
        ArtifactRef(format!("myelin://{tenant}/knowledge/page/{page_id}"))
    }

    pub fn block_root(tenant: &str, block_id: &str) -> ArtifactRef {
        ArtifactRef(format!("myelin://{tenant}/knowledge/block/{block_id}"))
    }

    pub fn emit_kn_edges(
        &self,
        tx: &mut dyn OutboxTx,
        source: &ArtifactRef,
        body: &[InlineNode],
        content_event: &EventEnvelope,
    ) -> BusResult<Vec<EventId>> {
        emit_edges(tx, source, body, content_event)
    }

    pub fn kn_edges(&self, source: &ArtifactRef, body: &[InlineNode]) -> Vec<EdgeDraft> {
        crate::extract_edges(source, body)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KnAnchorState {
    Live,
    Moved,
    Edited,
    Deleted,
    Erased,
}

impl KnAnchorState {
    fn into_sub_state(self, projection: OwnerProjection) -> SubState {
        match self {
            KnAnchorState::Live => SubState::Live(projection),
            KnAnchorState::Moved => SubState::Moved(projection),
            KnAnchorState::Edited => SubState::Outdated(projection),
            KnAnchorState::Deleted => SubState::Gone,
            KnAnchorState::Erased => SubState::Erased,
        }
    }
}

#[derive(Clone, Default)]
pub struct KnOwner {
    acl: Arc<Mutex<BTreeMap<String, Decision>>>,
    anchors: Arc<Mutex<BTreeMap<String, KnAnchorState>>>,
}

impl KnOwner {
    pub fn new() -> KnOwner {
        KnOwner::default()
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

    pub fn record_anchor(&self, ref_: &ArtifactRef, state: KnAnchorState) {
        self.anchors.lock().unwrap().insert(ref_.0.clone(), state);
    }

    fn projection(ref_: &ArtifactRef) -> OwnerProjection {
        OwnerProjection {
            title: "a Knowledge artifact".into(),
            state: "live".into(),
            icon: "knowledge".into(),
            render_hint: "embed".into(),
            sub_anchor: sub_kind(ref_).is_some().then(|| ref_.0.clone()),
            flag: None,
        }
    }

    fn resolve_kn_sub(&self, ref_: &ArtifactRef, sub: Option<&Sub>) -> SubState {
        let projection = Self::projection(ref_);
        match sub {
            None => SubState::Live(projection),
            Some(Sub::Block(_))
            | Some(Sub::Heading(_))
            | Some(Sub::Row(_))
            | Some(Sub::Field(_)) => {
                self.anchors
                    .lock()
                    .unwrap()
                    .get(&ref_.0)
                    .copied()
                    .map(|s| s.into_sub_state(projection.clone()))
                    .unwrap_or(SubState::Gone)
            }
            Some(_) => SubState::Live(projection),
        }
    }
}

impl ProjectApi for KnOwner {
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
        Ok(self.resolve_kn_sub(ref_, sub.as_ref()).into_outcome())
    }
}

impl SubAnchorResolver for KnOwner {
    fn resolve_sub(&self, ref_: &ArtifactRef, sub: Option<&Sub>) -> SubState {
        self.resolve_kn_sub(ref_, sub)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageParentEvent {
    pub parent: ArtifactRef,
    pub child: ArtifactRef,
    pub origin_event_id: String,
    pub origin_event_type: String,
    pub origin_actor: String,
    pub zookie: Option<String>,
}

impl PageParentEvent {
    pub const LIFECYCLE_TRIGGERS: &'static [&'static str] =
        &[KNOWLEDGE_PAGE_CREATED, KNOWLEDGE_PAGE_MOVED];

    pub fn is_lifecycle_trigger(&self) -> bool {
        Self::LIFECYCLE_TRIGGERS.contains(&self.origin_event_type.as_str())
    }

    fn as_typed_event(&self) -> SyntheticTypedEvent {
        SyntheticTypedEvent {
            source: self.parent.clone(),
            target: self.child.clone(),
            rel: LifecycleRel::Parent,
            origin_event: self.origin_event_id.clone(),
            origin_actor: self.origin_actor.clone(),
            zookie: self.zookie.clone(),
        }
    }
}

pub fn mirror_page_parent(
    tenant: &TenantId,
    ev: &PageParentEvent,
) -> Result<Vec<EdgeRow>, MirrorError> {
    if !ev.is_lifecycle_trigger() {
        return Err(MirrorError::UnknownRel(ev.origin_event_type.clone()));
    }
    Ok(mirror_edges(tenant, &ev.as_typed_event()))
}

pub fn project_page_parent(
    proj: &EdgeProjection,
    tenant: &TenantId,
    region: &Region,
    ev: &PageParentEvent,
) -> Result<Vec<String>, MirrorError> {
    let rows = mirror_page_parent(tenant, ev)?;
    let ids: Vec<String> = rows.iter().map(|r| r.edge_id.clone()).collect();
    for row in rows {
        proj.upsert(tenant, region, row);
    }
    Ok(ids)
}

pub fn reconverge_page_tree(
    proj: &EdgeProjection,
    tenant: &TenantId,
    region: &Region,
    typed_snapshot: &[PageParentEvent],
    covered_children: &[ArtifactRef],
    reindex_event_id: &str,
) -> Result<(usize, usize), MirrorError> {
    let mut typed: Vec<SyntheticTypedEvent> = Vec::with_capacity(typed_snapshot.len());
    for ev in typed_snapshot {
        if !ev.is_lifecycle_trigger() {
            return Err(MirrorError::UnknownRel(ev.origin_event_type.clone()));
        }
        typed.push(ev.as_typed_event());
    }
    reconverge(
        proj,
        tenant,
        region,
        &typed,
        covered_children,
        reindex_event_id,
    )
}

pub fn kn_replay_scope(grain: KnReplayGrain) -> String {
    match grain {
        KnReplayGrain::Page(id) => format!("page:{id}"),
        KnReplayGrain::Block { page, id } => format!("block:{page}/{id}"),
        KnReplayGrain::Subtree(page) => format!("subtree:{page}"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KnReplayGrain {
    Page(String),
    Block {
        page: String,
        id: String,
    },
    Subtree(String),
}

#[cfg(test)]
mod tests;

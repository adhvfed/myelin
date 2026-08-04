use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_identity::{Decision, Permission, Principal};
use myelin_issues::events::{RELATION_CREATED, RELATION_REMOVED, RELATION_SNAPSHOT};
use myelin_refs::{sub_kind, ArtifactRef, Sub};
use myelin_tenancy::{Region, TenantId};

use crate::edge_builder::{EdgeProjection, EdgeRow};
use crate::ladder::SubState;
use crate::mirror::{mirror_edges, reconverge, LifecycleRel, MirrorError, SyntheticTypedEvent};
use crate::resolve::{OwnerProjection, ProjectApi, ProjectApiError, ProjectOutcome, ResolveMode};
use crate::SubAnchorResolver;

pub const ISSUE_OWNER_TOKEN: &str = "issue";

pub struct IssueEdgeProducer;

impl IssueEdgeProducer {
    pub fn issue_root(tenant: &str, key: &str) -> ArtifactRef {
        ArtifactRef(format!("myelin://{tenant}/issue/issue/{key}"))
    }

    pub fn initiative_root(tenant: &str, key: &str) -> ArtifactRef {
        ArtifactRef(format!("myelin://{tenant}/issue/initiative/{key}"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueAnchorState {
    Live,
    Moved,
    Edited,
    Deleted,
    Erased,
}

impl IssueAnchorState {
    fn into_sub_state(self, projection: OwnerProjection) -> SubState {
        match self {
            IssueAnchorState::Live => SubState::Live(projection),
            IssueAnchorState::Moved => SubState::Moved(projection),
            IssueAnchorState::Edited => SubState::Outdated(projection),
            IssueAnchorState::Deleted => SubState::Gone,
            IssueAnchorState::Erased => SubState::Erased,
        }
    }
}

#[derive(Clone, Default)]
pub struct IssueOwner {
    acl: Arc<Mutex<BTreeMap<String, Decision>>>,
    anchors: Arc<Mutex<BTreeMap<String, IssueAnchorState>>>,
}

impl IssueOwner {
    pub fn new() -> IssueOwner {
        IssueOwner::default()
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

    pub fn record_anchor(&self, ref_: &ArtifactRef, state: IssueAnchorState) {
        self.anchors.lock().unwrap().insert(ref_.0.clone(), state);
    }

    fn projection(ref_: &ArtifactRef) -> OwnerProjection {
        OwnerProjection {
            title: "an Issues artifact".into(),
            state: "live".into(),
            icon: "issue".into(),
            render_hint: "embed".into(),
            sub_anchor: sub_kind(ref_).is_some().then(|| ref_.0.clone()),
            flag: None,
        }
    }

    fn resolve_issue_sub(&self, ref_: &ArtifactRef, sub: Option<&Sub>) -> SubState {
        let projection = Self::projection(ref_);
        match sub {
            None => SubState::Live(projection),
            Some(Sub::Field(_)) | Some(Sub::Row(_)) => {
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

impl ProjectApi for IssueOwner {
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
        Ok(self.resolve_issue_sub(ref_, sub.as_ref()).into_outcome())
    }
}

impl SubAnchorResolver for IssueOwner {
    fn resolve_sub(&self, ref_: &ArtifactRef, sub: Option<&Sub>) -> SubState {
        self.resolve_issue_sub(ref_, sub)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueRelationEvent {
    pub source: ArtifactRef,
    pub target: ArtifactRef,
    pub rel: String,
    pub origin_event_id: String,
    pub origin_event_type: String,
    pub origin_actor: String,
    pub zookie: Option<String>,
}

impl IssueRelationEvent {
    pub const LIFECYCLE_TRIGGERS: &'static [&'static str] =
        &[RELATION_CREATED, RELATION_REMOVED, RELATION_SNAPSHOT];

    pub fn is_lifecycle_trigger(&self) -> bool {
        Self::LIFECYCLE_TRIGGERS.contains(&self.origin_event_type.as_str())
    }

    fn as_typed_event(&self) -> Result<SyntheticTypedEvent, MirrorError> {
        if !self.is_lifecycle_trigger() {
            return Err(MirrorError::UnknownRel(self.origin_event_type.clone()));
        }
        let rel = LifecycleRel::parse(&self.rel)
            .ok_or_else(|| MirrorError::UnknownRel(self.rel.clone()))?;
        Ok(SyntheticTypedEvent {
            source: self.source.clone(),
            target: self.target.clone(),
            rel,
            origin_event: self.origin_event_id.clone(),
            origin_actor: self.origin_actor.clone(),
            zookie: self.zookie.clone(),
        })
    }
}

pub fn mirror_issue_relation(
    tenant: &TenantId,
    ev: &IssueRelationEvent,
) -> Result<Vec<EdgeRow>, MirrorError> {
    Ok(mirror_edges(tenant, &ev.as_typed_event()?))
}

pub fn project_issue_relation(
    proj: &EdgeProjection,
    tenant: &TenantId,
    region: &Region,
    ev: &IssueRelationEvent,
) -> Result<Vec<String>, MirrorError> {
    let rows = mirror_issue_relation(tenant, ev)?;
    let ids: Vec<String> = rows.iter().map(|r| r.edge_id.clone()).collect();
    for row in rows {
        proj.upsert(tenant, region, row);
    }
    Ok(ids)
}

pub fn reconverge_issue_relations(
    proj: &EdgeProjection,
    tenant: &TenantId,
    region: &Region,
    typed_snapshot: &[IssueRelationEvent],
    covered_roots: &[ArtifactRef],
    reindex_event_id: &str,
) -> Result<(usize, usize), MirrorError> {
    let mut typed: Vec<SyntheticTypedEvent> = Vec::with_capacity(typed_snapshot.len());
    for ev in typed_snapshot {
        typed.push(ev.as_typed_event()?);
    }
    reconverge(
        proj,
        tenant,
        region,
        &typed,
        covered_roots,
        reindex_event_id,
    )
}

#[cfg(test)]
mod tests;

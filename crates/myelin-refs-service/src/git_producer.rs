use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_content::InlineNode;
use myelin_events::{ArtifactRef, EventEnvelope, EventId, OutboxTx, Result as BusResult};
use myelin_git::subs::GIT_SUBSYSTEM;
use myelin_identity::{Decision, Permission, Principal};
use myelin_refs::{strip_sub, sub_kind, Sub};
use myelin_tenancy::{Region, TenantId};

use crate::emit::{emit_edges, EdgeDraft};
use crate::ladder::{resolve_line_range, LineRangeState, MintedLineRange, SubState};
use crate::resolve::{OwnerProjection, ProjectApi, ProjectApiError, ProjectOutcome, ResolveMode};
use crate::SubAnchorResolver;

pub struct GitEdgeProducer;

impl GitEdgeProducer {
    pub fn commit_root(tenant: &str, repo: &str, oid: &str) -> ArtifactRef {
        ArtifactRef(format!("myelin://{tenant}/git/commit/{repo}:{oid}"))
    }

    pub fn pr_root(tenant: &str, repo: &str, pr_number: u64) -> ArtifactRef {
        ArtifactRef(format!("myelin://{tenant}/git/pr/{repo}:{pr_number}"))
    }

    pub fn emit_git_edges(
        &self,
        tx: &mut dyn OutboxTx,
        source: &ArtifactRef,
        body: &[InlineNode],
        content_event: &EventEnvelope,
    ) -> BusResult<Vec<EventId>> {
        emit_edges(tx, source, body, content_event)
    }

    pub fn git_edges(&self, source: &ArtifactRef, body: &[InlineNode]) -> Vec<EdgeDraft> {
        crate::extract_edges(source, body)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommentState {
    Live,
    Moved,
    Resolved,
    Gone,
    Erased,
}

impl CommentState {
    fn into_sub_state(self, projection: OwnerProjection) -> SubState {
        match self {
            CommentState::Live => SubState::Live(projection),
            CommentState::Moved => SubState::Moved(projection),
            CommentState::Resolved => SubState::Outdated(projection),
            CommentState::Gone => SubState::Gone,
            CommentState::Erased => SubState::Erased,
        }
    }
}

#[derive(Clone, Debug)]
struct GitBlobAnchor {
    minted: MintedLineRange,
    current_oid: String,
    current_lines: Vec<String>,
}

#[derive(Clone, Default)]
pub struct GitOwner {
    acl: Arc<Mutex<BTreeMap<String, Decision>>>,
    line_ranges: Arc<Mutex<BTreeMap<String, GitBlobAnchor>>>,
    comments: Arc<Mutex<BTreeMap<String, CommentState>>>,
    checks: Arc<Mutex<BTreeMap<String, CommentState>>>,
}

impl GitOwner {
    pub fn new() -> GitOwner {
        GitOwner::default()
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

    pub fn record_line_range(
        &self,
        ref_: &ArtifactRef,
        minted: MintedLineRange,
        current_oid: &str,
        current_lines: &[&str],
    ) {
        self.line_ranges.lock().unwrap().insert(
            ref_.0.clone(),
            GitBlobAnchor {
                minted,
                current_oid: current_oid.to_string(),
                current_lines: current_lines.iter().map(|l| l.to_string()).collect(),
            },
        );
    }

    pub fn record_comment(&self, ref_: &ArtifactRef, state: CommentState) {
        self.comments.lock().unwrap().insert(ref_.0.clone(), state);
    }

    pub fn record_check(&self, ref_: &ArtifactRef, state: CommentState) {
        self.checks.lock().unwrap().insert(ref_.0.clone(), state);
    }

    fn projection(ref_: &ArtifactRef) -> OwnerProjection {
        OwnerProjection {
            title: "a Git artifact".into(),
            state: "live".into(),
            icon: "git".into(),
            render_hint: "embed".into(),
            sub_anchor: sub_kind(ref_).is_some().then(|| ref_.0.clone()),
            flag: None,
        }
    }

    fn resolve_git_sub(&self, ref_: &ArtifactRef, sub: Option<&Sub>) -> SubState {
        let projection = Self::projection(ref_);
        match sub {
            None => SubState::Live(projection),
            Some(Sub::LineRange { .. }) => match self.line_ranges.lock().unwrap().get(&ref_.0) {
                Some(anchor) => {
                    let current: Vec<&str> =
                        anchor.current_lines.iter().map(String::as_str).collect();
                    let state = resolve_line_range(&anchor.minted, &anchor.current_oid, &current);
                    let mut p = projection;
                    p.sub_anchor = Some(self.render_line_anchor(ref_, &state));
                    line_state_into_sub(state, p)
                }
                None => SubState::Gone,
            },
            Some(Sub::Comment(_)) | Some(Sub::Thread(_)) => self
                .comments
                .lock()
                .unwrap()
                .get(&ref_.0)
                .copied()
                .unwrap_or(CommentState::Live)
                .into_sub_state(projection),
            Some(Sub::Check(_)) | Some(Sub::Step(_)) => self
                .checks
                .lock()
                .unwrap()
                .get(&ref_.0)
                .copied()
                .unwrap_or(CommentState::Live)
                .into_sub_state(projection),
            Some(_) => SubState::Live(projection),
        }
    }

    fn render_line_anchor(&self, ref_: &ArtifactRef, state: &LineRangeState) -> String {
        let root = strip_sub(ref_);
        match state {
            LineRangeState::Exact => ref_.0.clone(),
            LineRangeState::Rebased { new_start, new_end } => {
                format!("{}#L{new_start}-L{new_end}", root.0)
            }
            LineRangeState::Partial {
                surviving_start,
                surviving_end,
            } => {
                format!("{}#L{surviving_start}-L{surviving_end}", root.0)
            }
            LineRangeState::ContentGone => root.0,
        }
    }
}

fn line_state_into_sub(state: LineRangeState, projection: OwnerProjection) -> SubState {
    state.into_sub_state(projection)
}

impl ProjectApi for GitOwner {
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
        Ok(self.resolve_git_sub(ref_, sub.as_ref()).into_outcome())
    }
}

impl SubAnchorResolver for GitOwner {
    fn resolve_sub(&self, ref_: &ArtifactRef, sub: Option<&Sub>) -> SubState {
        self.resolve_git_sub(ref_, sub)
    }
}

pub fn git_replay_scope(grain: GitReplayGrain) -> String {
    match grain {
        GitReplayGrain::Repo(id) => format!("repo:{id}"),
        GitReplayGrain::Blob { repo, oid } => format!("blob:{repo}/{oid}"),
        GitReplayGrain::Pr { repo, number } => format!("pr:{repo}/{number}"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitReplayGrain {
    Repo(String),
    Blob { repo: String, oid: String },
    Pr { repo: String, number: u64 },
}

pub const GIT_OWNER_TOKEN: &str = GIT_SUBSYSTEM;

#[cfg(test)]
mod tests;

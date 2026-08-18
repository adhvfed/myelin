use std::collections::{HashMap, HashSet, VecDeque};

use myelin_content::inline::InlineNode;
use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventDraft, EventEnvelope, EventId, EventType, OutboxTx,
    Result as BusResult, Visibility,
};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, Permission, Principal, Zookie,
};
use myelin_query::FieldValue;
use myelin_search::{ProjectFetchError, ProjectFetcher, SearchProjection};
use myelin_tenancy::{Region, TenantId};

use crate::events;

pub const ISSUE_SUBSYSTEM: &str = "issue";

pub const VIEW: &str = "view";

pub const TRAVERSE_MAX_DEPTH: usize = 16;

pub fn issue_root_ref(tenant: &str, key: &str) -> ArtifactRef {
    myelin_refs::parse(&format!("myelin://{tenant}/{ISSUE_SUBSYSTEM}/issue/{key}"))
        .expect("Issues mints a grammatical canonical ArtifactRef (contract 5.1)")
}

pub fn comment_sub_ref(
    issue_root: &ArtifactRef,
    opaque_id: &str,
) -> Result<ArtifactRef, myelin_refs::ParseError> {
    myelin_refs::mint(issue_root, myelin_refs::Sub::Comment(opaque_id.to_string()))
}

pub fn block_sub_ref(
    issue_root: &ArtifactRef,
    opaque_id: &str,
) -> Result<ArtifactRef, myelin_refs::ParseError> {
    myelin_refs::mint(issue_root, myelin_refs::Sub::Block(opaque_id.to_string()))
}

pub fn field_sub_ref(
    issue_root: &ArtifactRef,
    field_id: &str,
) -> Result<ArtifactRef, myelin_refs::ParseError> {
    myelin_refs::mint(issue_root, myelin_refs::Sub::Field(field_id.to_string()))
}

pub fn row_sub_ref(
    issue_root: &ArtifactRef,
    row_id: &str,
) -> Result<ArtifactRef, myelin_refs::ParseError> {
    myelin_refs::mint(issue_root, myelin_refs::Sub::Row(row_id.to_string()))
}

pub use myelin_refs::{REFS_EDGE_CREATED, REL_CLASS_REFERENCE};

pub const REL_CLASS_LIFECYCLE: &str = "lifecycle";

pub fn edge_aggregate_key(source: &ArtifactRef, target: &ArtifactRef) -> AggregateKey {
    myelin_refs::edge_aggregate_key(source, target)
}

fn node_rel(node: &InlineNode) -> myelin_refs::ReferenceRel {
    match node {
        InlineNode::Mention(_) => myelin_refs::ReferenceRel::Mentions,
        InlineNode::ArtifactRefNode(_) => myelin_refs::ReferenceRel::Links,
        InlineNode::Embed(_) => myelin_refs::ReferenceRel::Embeds,
    }
}

fn node_target(node: &InlineNode, tenant: &TenantId) -> ArtifactRef {
    match node {
        InlineNode::Mention(principal) => {
            myelin_refs::identity_member_ref(tenant, &principal.principal_id)
        }
        InlineNode::ArtifactRefNode(r) | InlineNode::Embed(r) => r.clone(),
    }
}

fn content_edge_draft(source: &ArtifactRef, node: &InlineNode, tenant: &TenantId) -> EventDraft {
    let target = node_target(node, tenant);
    myelin_refs::reference_edge_draft(
        source,
        &target,
        node_rel(node),
        myelin_refs::EdgeChange::Created,
    )
}

pub fn emit_content_edges(
    tx: &mut dyn OutboxTx,
    tenant: &TenantId,
    source: &ArtifactRef,
    nodes: &[InlineNode],
    content_cause: Option<&EventEnvelope>,
) -> BusResult<Vec<EventId>> {
    let mut ids = Vec::with_capacity(nodes.len());
    for node in nodes {
        let id = tx.emit(content_edge_draft(source, node, tenant), content_cause)?;
        ids.push(id);
    }
    Ok(ids)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueLifecycleRel {
    Parent,
    Blocks,
    BlockedBy,
    Closes,
    DependsOn,
    Relates,
}

impl IssueLifecycleRel {
    pub fn as_str(self) -> &'static str {
        match self {
            IssueLifecycleRel::Parent => "parent",
            IssueLifecycleRel::Blocks => "blocks",
            IssueLifecycleRel::BlockedBy => "blocked_by",
            IssueLifecycleRel::Closes => "closes",
            IssueLifecycleRel::DependsOn => "depends_on",
            IssueLifecycleRel::Relates => "relates",
        }
    }

    pub fn from_token(token: &str) -> Option<IssueLifecycleRel> {
        match token {
            "parent" => Some(IssueLifecycleRel::Parent),
            "blocks" => Some(IssueLifecycleRel::Blocks),
            "blocked_by" => Some(IssueLifecycleRel::BlockedBy),
            "closes" => Some(IssueLifecycleRel::Closes),
            "depends_on" => Some(IssueLifecycleRel::DependsOn),
            "relates" => Some(IssueLifecycleRel::Relates),
            _ => None,
        }
    }
}

fn relation_draft(
    source: &ArtifactRef,
    target: &ArtifactRef,
    rel: IssueLifecycleRel,
    created: bool,
) -> EventDraft {
    let type_ = if created {
        events::RELATION_CREATED
    } else {
        events::RELATION_REMOVED
    };
    EventDraft {
        type_: EventType(type_.into()),
        subject: source.clone(),
        aggregate: edge_aggregate_key(source, target),
        payload: serde_json::json!({
            "source": source.0,
            "target": target.0,
            "rel": rel.as_str(),
            "rel_class": REL_CLASS_LIFECYCLE,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

pub fn emit_relation_edge(
    tx: &mut dyn OutboxTx,
    source: &ArtifactRef,
    target: &ArtifactRef,
    rel: IssueLifecycleRel,
    created: bool,
    cause: Option<&EventEnvelope>,
) -> BusResult<EventId> {
    tx.emit(relation_draft(source, target, rel, created), cause)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelationEdge {
    pub source: ArtifactRef,
    pub target: ArtifactRef,
    pub rel: IssueLifecycleRel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraversedNode {
    pub node: ArtifactRef,
    pub depth: usize,
}

#[derive(Clone, Debug, Default)]
pub struct IssueRelationGraph {
    forward: HashMap<String, Vec<RelationEdge>>,
}

impl IssueRelationGraph {
    pub fn new() -> IssueRelationGraph {
        IssueRelationGraph::default()
    }

    pub fn add_edge(&mut self, source: &ArtifactRef, target: &ArtifactRef, rel: IssueLifecycleRel) {
        self.forward
            .entry(source.0.clone())
            .or_default()
            .push(RelationEdge {
                source: source.clone(),
                target: target.clone(),
                rel,
            });
    }

    pub fn traverse(
        &self,
        root: &ArtifactRef,
        rel: Option<IssueLifecycleRel>,
    ) -> Vec<TraversedNode> {
        let mut out = Vec::new();
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(root.0.clone());
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        queue.push_back((root.0.clone(), 0));
        while let Some((node, depth)) = queue.pop_front() {
            if depth >= TRAVERSE_MAX_DEPTH {
                continue;
            }
            if let Some(edges) = self.forward.get(&node) {
                for edge in edges {
                    if rel.is_some_and(|r| r != edge.rel) {
                        continue;
                    }
                    if visited.insert(edge.target.0.clone()) {
                        let child_depth = depth + 1;
                        out.push(TraversedNode {
                            node: edge.target.clone(),
                            depth: child_depth,
                        });
                        queue.push_back((edge.target.0.clone(), child_depth));
                    }
                }
            }
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projection {
    pub title: String,
    pub state: String,
    pub category: String,
    pub icon: String,
    pub render_hint: String,
    pub sub_anchor: Option<SubAnchor>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubAnchor {
    pub kind: String,
    pub sub_id: String,
    pub rung: LadderRung,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LadderRung {
    Live,
    Moved,
    Outdated,
}

impl LadderRung {
    pub fn as_str(self) -> &'static str {
        match self {
            LadderRung::Live => "live",
            LadderRung::Moved => "moved",
            LadderRung::Outdated => "outdated",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tombstone {
    pub reason: TombstoneReason,
    pub root: ArtifactRef,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TombstoneReason {
    Denied,
    RootGone,
    SubGone,
    Erased,
}

impl Tombstone {
    pub fn display_text(&self) -> &'static str {
        "(not available)"
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Projected {
    Visible(Projection),
    Tombstoned(Tombstone),
}

impl Projected {
    pub fn is_visible(&self) -> bool {
        matches!(self, Projected::Visible(_))
    }

    pub fn is_tombstone(&self) -> bool {
        matches!(self, Projected::Tombstoned(_))
    }

    pub fn title(&self) -> Option<&str> {
        match self {
            Projected::Visible(p) => Some(&p.title),
            Projected::Tombstoned(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectError {
    NotAnIssueArtifact { reference: String },
    UnknownIssueType { ty: String },
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectError::NotAnIssueArtifact { reference } => write!(
                f,
                "not an Issues artifact: `{reference}` - Issues' projector does not own this ref"
            ),
            ProjectError::UnknownIssueType { ty } => {
                write!(f, "unknown Issues artifact type `{ty}`")
            }
        }
    }
}

impl std::error::Error for ProjectError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IssueArtifactType {
    Issue,
    Epic,
    Sprint,
    Field,
    Comment,
    Relation,
    Initiative,
}

fn classify(r: &ArtifactRef) -> Result<IssueArtifactType, ProjectError> {
    let rest =
        r.0.strip_prefix("myelin://")
            .ok_or_else(|| ProjectError::NotAnIssueArtifact {
                reference: r.0.clone(),
            })?;
    let scope = rest.split('#').next().unwrap_or(rest);
    let segments: Vec<&str> = scope.split('/').collect();
    if segments.len() != 4 || segments[1] != ISSUE_SUBSYSTEM {
        return Err(ProjectError::NotAnIssueArtifact {
            reference: r.0.clone(),
        });
    }
    match segments[2] {
        "issue" => Ok(IssueArtifactType::Issue),
        "epic" => Ok(IssueArtifactType::Epic),
        "sprint" => Ok(IssueArtifactType::Sprint),
        "field" => Ok(IssueArtifactType::Field),
        "comment" => Ok(IssueArtifactType::Comment),
        "relation" => Ok(IssueArtifactType::Relation),
        "initiative" => Ok(IssueArtifactType::Initiative),
        other => Err(ProjectError::UnknownIssueType {
            ty: other.to_string(),
        }),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueMeta {
    pub title: String,
    pub state: String,
    pub state_category: String,
    pub icon: String,
    pub assignee: Option<String>,
    pub priority: i64,
    pub type_rank: i64,
    pub project_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubState {
    Live,
    Moved,
    Outdated,
    Gone,
}

#[derive(Clone, Debug, Default)]
pub struct IssueProjectionStore {
    roots: HashMap<String, IssueMeta>,
    subs: HashMap<String, SubState>,
    erased: HashSet<String>,
    restricted: HashSet<String>,
}

impl IssueProjectionStore {
    pub fn new() -> IssueProjectionStore {
        IssueProjectionStore::default()
    }

    pub fn put_issue(&mut self, root: &ArtifactRef, meta: IssueMeta) {
        self.roots.insert(root.0.clone(), meta);
    }

    pub fn put_sub_state(&mut self, sub_ref: &ArtifactRef, state: SubState) {
        self.subs.insert(sub_ref.0.clone(), state);
    }

    pub fn mark_erased(&mut self, reference: &ArtifactRef) {
        self.erased.insert(reference.0.clone());
    }

    pub fn mark_restricted(&mut self, reference: &ArtifactRef) {
        self.restricted.insert(reference.0.clone());
    }
}

pub struct Projector<I: IdentityService> {
    id: I,
    store: IssueProjectionStore,
}

impl<I: IdentityService> Projector<I> {
    pub fn new(id: I, store: IssueProjectionStore) -> Projector<I> {
        Projector { id, store }
    }

    pub fn store_mut(&mut self) -> &mut IssueProjectionStore {
        &mut self.store
    }

    pub fn project(
        &self,
        reference: &ArtifactRef,
        viewer: &Principal,
        zookie: Zookie,
    ) -> Result<Projected, ProjectError> {
        let ty = classify(reference)?;
        let root = myelin_refs::strip_sub(reference);

        let at = Consistency {
            at_least: zookie,
            mode: ConsistencyMode::Strong,
        };
        let permission = Permission(VIEW.to_string());
        match self.id.check(viewer, &permission, &root, &at, None) {
            Ok(Decision::Allow) => {}
            Ok(Decision::Deny) | Ok(Decision::Conditional) | Err(_) => {
                return Ok(Projected::Tombstoned(Tombstone {
                    reason: TombstoneReason::Denied,
                    root,
                }));
            }
        }

        if self.store.erased.contains(&root.0) || self.store.erased.contains(&reference.0) {
            return Ok(Projected::Tombstoned(Tombstone {
                reason: TombstoneReason::Erased,
                root,
            }));
        }
        if self.store.restricted.contains(&root.0) || self.store.restricted.contains(&reference.0) {
            return Ok(Projected::Tombstoned(Tombstone {
                reason: TombstoneReason::Erased,
                root,
            }));
        }

        let meta = match self.store.roots.get(&root.0) {
            Some(m) => m.clone(),
            None => {
                return Ok(Projected::Tombstoned(Tombstone {
                    reason: TombstoneReason::RootGone,
                    root,
                }));
            }
        };

        let sub_anchor = match myelin_refs::sub_kind(reference) {
            Some(sub) => {
                let state = self
                    .store
                    .subs
                    .get(&reference.0)
                    .copied()
                    .unwrap_or(SubState::Live);
                match sub_state_to_rung(state) {
                    Some(rung) => Some(SubAnchor {
                        kind: sub.kind().label().to_string(),
                        sub_id: sub_opaque_id(&sub),
                        rung,
                    }),
                    None => {
                        return Ok(Projected::Tombstoned(Tombstone {
                            reason: TombstoneReason::SubGone,
                            root,
                        }));
                    }
                }
            }
            None => None,
        };

        Ok(Projected::Visible(Projection {
            title: meta.title,
            state: meta.state,
            category: meta.state_category,
            icon: meta.icon,
            render_hint: icon_for(ty).to_string(),
            sub_anchor,
        }))
    }
}

fn sub_state_to_rung(state: SubState) -> Option<LadderRung> {
    match state {
        SubState::Live => Some(LadderRung::Live),
        SubState::Moved => Some(LadderRung::Moved),
        SubState::Outdated => Some(LadderRung::Outdated),
        SubState::Gone => None,
    }
}

fn sub_opaque_id(sub: &myelin_refs::Sub) -> String {
    use myelin_refs::Sub;
    match sub {
        Sub::Comment(id)
        | Sub::Block(id)
        | Sub::Field(id)
        | Sub::Row(id)
        | Sub::Heading(id)
        | Sub::Thread(id)
        | Sub::Message(id)
        | Sub::View(id)
        | Sub::Check(id) => id.clone(),
        Sub::CommitCheck {
            commit_oid,
            context,
        } => format!("commit-{commit_oid}/check-{context}"),
        Sub::CommitCiResult { commit_oid } => format!("commit-{commit_oid}/ci-result"),
        Sub::Step(n) => n.to_string(),
        Sub::LineRange { start, end } => format!("L{start}-L{end}"),
    }
}

fn icon_for(ty: IssueArtifactType) -> &'static str {
    match ty {
        IssueArtifactType::Issue
        | IssueArtifactType::Epic
        | IssueArtifactType::Sprint
        | IssueArtifactType::Field
        | IssueArtifactType::Comment
        | IssueArtifactType::Relation
        | IssueArtifactType::Initiative => "issue",
    }
}

pub struct IssueProjectFetcher {
    store: IssueProjectionStore,
}

impl IssueProjectFetcher {
    pub fn new(store: IssueProjectionStore) -> IssueProjectFetcher {
        IssueProjectFetcher { store }
    }

    pub fn store_mut(&mut self) -> &mut IssueProjectionStore {
        &mut self.store
    }

    fn build(&self, reference: &ArtifactRef) -> Result<SearchProjection, ProjectFetchError> {
        let root = myelin_refs::strip_sub(reference);
        if self.store.erased.contains(&root.0)
            || self.store.erased.contains(&reference.0)
            || self.store.restricted.contains(&root.0)
            || self.store.restricted.contains(&reference.0)
        {
            return Err(ProjectFetchError::Gone);
        }
        let meta = self
            .store
            .roots
            .get(&root.0)
            .ok_or(ProjectFetchError::Gone)?;

        let mut fields = std::collections::BTreeMap::new();
        fields.insert(
            crate::declares::FACET_STATE_CATEGORY.to_string(),
            FieldValue::Select(meta.state_category.clone()),
        );
        fields.insert(
            crate::declares::FACET_PRIORITY.to_string(),
            FieldValue::Int(meta.priority),
        );
        if let Some(assignee) = &meta.assignee {
            fields.insert(
                crate::declares::FACET_ASSIGNEE.to_string(),
                FieldValue::Principal(assignee.clone()),
            );
        }
        fields.insert(
            crate::declares::FACET_TYPE_RANK.to_string(),
            FieldValue::Int(meta.type_rank),
        );
        fields.insert(
            crate::declares::FACET_PROJECT_ID.to_string(),
            FieldValue::Relation(meta.project_id.clone()),
        );

        Ok(SearchProjection {
            text: meta.title.clone(),
            fields,
            lang: None,
        })
    }
}

impl ProjectFetcher for IssueProjectFetcher {
    fn project(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        ref_: &ArtifactRef,
    ) -> Result<SearchProjection, ProjectFetchError> {
        self.build(ref_)
    }
}

#[cfg(test)]
#[path = "refs_glue/tests.rs"]
mod tests;

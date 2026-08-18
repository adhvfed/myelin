use myelin_content::events::{
    KNOWLEDGE_PAGE_PARENT_SET, KNOWLEDGE_RELATION_CREATED, KNOWLEDGE_RELATION_REMOVED,
};
use myelin_content::inline::InlineNode;
use myelin_events::{
    AggregateKey, ArtifactRef, DataRole, EventDraft, EventEnvelope, EventId, EventType, OutboxTx,
    Result as BusResult, Visibility,
};
use myelin_identity::{
    Consistency, ConsistencyMode, Decision, IdentityService, Permission, Principal, Zookie,
};
use myelin_tenancy::TenantId;
use std::collections::{HashMap, HashSet};

use crate::block_tree::PageId;
use crate::database::{DbRelation, RelationKind};

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
    block_cause: Option<&EventEnvelope>,
) -> BusResult<Vec<EventId>> {
    let mut ids = Vec::with_capacity(nodes.len());
    for node in nodes {
        let id = tx.emit(content_edge_draft(source, node, tenant), block_cause)?;
        ids.push(id);
    }
    Ok(ids)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KnowledgeLifecycleRel {
    Parent,
    Relates,
    RollupSource,
}

impl KnowledgeLifecycleRel {
    pub fn as_str(self) -> &'static str {
        match self {
            KnowledgeLifecycleRel::Parent => "parent",
            KnowledgeLifecycleRel::Relates => "relates",
            KnowledgeLifecycleRel::RollupSource => "rollup_source",
        }
    }
}

fn rel_of(kind: RelationKind) -> KnowledgeLifecycleRel {
    match kind {
        RelationKind::Relates => KnowledgeLifecycleRel::Relates,
        RelationKind::RollupSource => KnowledgeLifecycleRel::RollupSource,
    }
}

fn page_urn(tenant: &TenantId, page: &PageId) -> ArtifactRef {
    ArtifactRef(format!("myelin://{}/knowledge/page/{}", tenant.0, page.0))
}

fn row_urn(tenant: &TenantId, row_id: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://{}/knowledge/row/{}", tenant.0, row_id))
}

fn parent_set_draft(child: &ArtifactRef, parent: &ArtifactRef) -> EventDraft {
    EventDraft {
        type_: EventType(KNOWLEDGE_PAGE_PARENT_SET.into()),
        subject: child.clone(),
        aggregate: AggregateKey(child.0.clone()),
        payload: serde_json::json!({
            "source": child.0,
            "target": parent.0,
            "rel": KnowledgeLifecycleRel::Parent.as_str(),
            "rel_class": REL_CLASS_LIFECYCLE,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

pub fn emit_page_parent_set(
    tx: &mut dyn OutboxTx,
    tenant: &TenantId,
    child: &PageId,
    parent: &PageId,
    cause: Option<&EventEnvelope>,
) -> BusResult<EventId> {
    let child_ref = page_urn(tenant, child);
    let parent_ref = page_urn(tenant, parent);
    tx.emit(parent_set_draft(&child_ref, &parent_ref), cause)
}

fn relation_draft(tenant: &TenantId, relation: &DbRelation, created: bool) -> EventDraft {
    let source = row_urn(tenant, &relation.src_row);
    let target = relation.dst_ref.clone();
    let type_ = if created {
        KNOWLEDGE_RELATION_CREATED
    } else {
        KNOWLEDGE_RELATION_REMOVED
    };
    EventDraft {
        type_: EventType(type_.into()),
        subject: source.clone(),
        aggregate: edge_aggregate_key(&source, &target),
        payload: serde_json::json!({
            "source": source.0,
            "target": target.0,
            "rel": rel_of(relation.rel).as_str(),
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
    tenant: &TenantId,
    relation: &DbRelation,
    created: bool,
    cause: Option<&EventEnvelope>,
) -> BusResult<EventId> {
    tx.emit(relation_draft(tenant, relation, created), cause)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projection {
    pub title: String,
    pub state: String,
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
    NotAKnowledgeArtifact { reference: String },
    UnknownKnowledgeType { ty: String },
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectError::NotAKnowledgeArtifact { reference } => write!(
                f,
                "not a Knowledge artifact: `{reference}` - Knowledge's projector does not own this ref"
            ),
            ProjectError::UnknownKnowledgeType { ty } => {
                write!(f, "unknown Knowledge artifact type `{ty}`")
            }
        }
    }
}

impl std::error::Error for ProjectError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KnArtifactType {
    Page,
    Block,
    Database,
    Row,
    View,
}

fn classify(r: &ArtifactRef) -> Result<KnArtifactType, ProjectError> {
    let rest =
        r.0.strip_prefix("myelin://")
            .ok_or_else(|| ProjectError::NotAKnowledgeArtifact {
                reference: r.0.clone(),
            })?;
    let scope = rest.split('#').next().unwrap_or(rest);
    let segments: Vec<&str> = scope.split('/').collect();
    if segments.len() != 4 || segments[1] != "knowledge" {
        return Err(ProjectError::NotAKnowledgeArtifact {
            reference: r.0.clone(),
        });
    }
    match segments[2] {
        "page" => Ok(KnArtifactType::Page),
        "block" => Ok(KnArtifactType::Block),
        "database" => Ok(KnArtifactType::Database),
        "row" => Ok(KnArtifactType::Row),
        "view" => Ok(KnArtifactType::View),
        other => Err(ProjectError::UnknownKnowledgeType {
            ty: other.to_string(),
        }),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageMeta {
    pub title: String,
    pub state: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubState {
    Live,
    Moved,
    Outdated,
    Gone,
}

#[derive(Clone, Debug, Default)]
pub struct PageStore {
    roots: HashMap<String, PageMeta>,
    subs: HashMap<String, SubState>,
    erased: HashSet<String>,
    restricted: HashSet<String>,
}

impl PageStore {
    pub fn new() -> PageStore {
        PageStore::default()
    }

    pub fn put_root(&mut self, root: &ArtifactRef, meta: PageMeta) {
        self.roots.insert(root.0.clone(), meta);
    }

    pub fn put_sub_state(&mut self, sub_ref: &ArtifactRef, state: SubState) {
        self.subs.insert(sub_ref.0.clone(), state);
    }

    pub fn mark_erased(&mut self, reference: &ArtifactRef) {
        self.erased.insert(reference.0.clone());
    }

    pub fn is_erased(&self, reference: &ArtifactRef) -> bool {
        self.erased.contains(&reference.0)
    }

    pub fn mark_restricted(&mut self, reference: &ArtifactRef) {
        self.restricted.insert(reference.0.clone());
    }
}

pub const READ: &str = "read";

pub struct Projector<I: IdentityService> {
    id: I,
    store: PageStore,
}

impl<I: IdentityService> Projector<I> {
    pub fn new(id: I, store: PageStore) -> Projector<I> {
        Projector { id, store }
    }

    pub fn store_mut(&mut self) -> &mut PageStore {
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
        let permission = Permission(READ.to_string());
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
            icon: icon_for(ty).to_string(),
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
        Sub::Block(id)
        | Sub::Heading(id)
        | Sub::Row(id)
        | Sub::Field(id)
        | Sub::Comment(id)
        | Sub::Thread(id)
        | Sub::Message(id)
        | Sub::View(id)
        | Sub::Check(id) => id.clone(),
        Sub::Step(n) => n.to_string(),
        Sub::LineRange { start, end } => format!("L{start}-L{end}"),
        Sub::CommitCheck {
            commit_oid,
            context,
        } => format!("commit-{commit_oid}/check-{context}"),
        Sub::CommitCiResult { commit_oid } => format!("commit-{commit_oid}/ci-result"),
    }
}

fn icon_for(ty: KnArtifactType) -> &'static str {
    match ty {
        KnArtifactType::Page => "page",
        KnArtifactType::Block => "block",
        KnArtifactType::Database => "database",
        KnArtifactType::Row => "row",
        KnArtifactType::View => "view",
    }
}

#[cfg(test)]
mod tests;

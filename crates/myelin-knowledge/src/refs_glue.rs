//! # `refs_glue` — Knowledge's Refs glue: the `#sub` `project(ref, viewer)` 4-step tombstone ladder,
//! the three inline-node `refs.edge.created` producer, and the TE-7 typed-edge mirror (KN-P19 / P-309,
//! M3)
//!
//! **The KN-M3d refs-glue deliverable.** [`crate::subs`] (KN-P10 / P-300) already shipped the `b`/`h`
//! `#sub` MINTS + the Refs registration; this module ships the GLUE the named floors there pointed at:
//!
//! 1. **`project(ref, viewer)` + the one 4-step tombstone ladder (5.6 / 5.7, owned).** The ONLY way
//!    Refs/Search/Notif read a Knowledge artifact (no cross-DB), per-viewer permission-checked: a
//!    confidential page degrades to a content-free [`Tombstone`] for an unauthorized viewer — the
//!    title NEVER leaks (the project-leak counter = 0). The `#sub` sub-anchor resolver runs the frozen
//!    ladder (permission → root → sub `LIVE`/`MOVED`/`OUTDATED`/`GONE` → `ERASED`); a tombstone ALWAYS
//!    carries the root so an embed degrades to "this referenced <page>" rather than vanishing.
//! 2. **The three inline-node `refs.edge.created` producer (5.4, produced).** `mention` /
//!    `artifact_ref` / `embed` content nodes ([`myelin_content::inline::InlineNode`]) emit ONE
//!    `refs.edge.created` per node on persist, via the OUTBOX, **NOT coalesced** (a discrete edge
//!    fact). The three nodes are the uniform producers across Chat/Issues/Knowledge (X-2); there is
//!    NO standalone edge-write API.
//! 3. **The TE-7 typed-edge mirror (5.5, owned source of truth).** The SAME transaction that writes a
//!    `page_parent` ([`crate::block_tree::PageTree`]) / `db_relation` ([`crate::database::RelationStore`])
//!    typed row emits `knowledge.page.parent_set` / `knowledge.relation.created`/`.removed` so Refs
//!    projects the lifecycle edge. The typed table is TRUTH; Refs holds the rebuildable projection.
//!
//! **Owning architecture doc (read in full before changing this):**
//! `04-subsystem-architectures/knowledge-platform/architecture/03-events-contracts-and-glue.md`
//! §2.1 (the `#sub` grammar + the 4-step tombstone ladder — a tombstone always carries the root; a
//! confidential page → tombstone, never leaks), §2.2 (`project(ref, viewer)` — the frozen
//! `{title, state, icon, render_hint, sub_anchor?}` shape), §3.1 (the TE-7 typed-edge mirror:
//! `db_relation` → `knowledge.relation.*` → Refs lifecycle edge; `page_parent` →
//! `knowledge.page.parent_set` → Refs parent edge; the typed table is truth).
//! **Reconciliation:** `00-reconciliation-decisions.md` X-4 (the frozen `#sub` grammar + the ONE
//! 4-step ladder covering all three content shapes; `field-` is new; `h` has no hyphen).
//! **Contracts:** `contract-index.md` rows 5.6 (project, owned), 5.7 (the `#sub` stable-id mint +
//! the ladder, owned), 5.5 (TE-7 mirror, owned source of truth), 5.2/5.3/5.4 (resolve/backlinks/
//! traverse + edge events, consumed/produced).
//!
//! ## Why permission-FIRST (the 0-leak invariant — EI-01 §3 prove-it)
//! [`Projector::project`] runs the permission check on the `#sub`-stripped ROOT (a sub is never more
//! visible than its parent) BEFORE any field of the artifact is read into the projection, so a denied
//! viewer's result is a [`Tombstone`] with NO title/content ever fetched on the deny path. An Id
//! transport hiccup fails CLOSED to a tombstone (never a leak). This is the mandatory-core leak
//! surface — the mutation floor is stated below.
//!
//! ## Mutation-score floor (mandatory-core, EI-01 §3 / prove-it)
//! `project(ref, viewer)` is the **mandatory-core leak surface** (a project leak IS the failure). The
//! floor for the project/ladder path is **≥ 90% of viable mutants caught**
//! (`cargo mutants -p myelin-knowledge -f crates/myelin-knowledge/src/refs_glue.rs`) — **MEASURED
//! 2026-06-22: 31 viable mutants, 31 caught (100%)**. The load-bearing logic — the permission-first
//! gate (deny ⇒ tombstone, root-keyed), each ladder rung (`LIVE`/`MOVED`/`OUTDATED`/`GONE`/`ERASED`),
//! the erased/restricted OR-guards (root OR full-ref), each rung token, and the TE-7 forward-edge emit
//! shape — each has a test a mutation flips: the permission-deny mutant by
//! `unauthorized_viewer_gets_a_tombstone_carrying_the_root_never_the_title`; the ERASED rung by
//! `an_erased_page_projects_to_an_erased_tombstone`; the GONE/MOVED/OUTDATED rungs by
//! `the_sub_anchor_ladder_lives_moves_outdates_and_gones`; the restriction/erasure `||`→`&&` mutants by
//! `a_restricted_sub_urn_tombstones_even_when_the_root_is_not` /
//! `an_erased_sub_urn_tombstones_even_when_the_root_is_not`; the mirror emit by the `te7_*` tests. The
//! world-scale corpus-under-load drill is a later band (KN-P32).
//!
//! ## Named floors (VISION §3 / EI-01 §1)
//! - **The replay/reindex that REBUILDS the Refs projection from source is KN-P20 (P-310).** This
//!   module is the steady-state producer (the same-tx emit) + the live `project` resolver; the
//!   `replay(scope)` block-granular `*.snapshot` re-emit (the only recovery path; the TE-7
//!   drift-correction that reconverges Refs to the typed table) is KN-P20. Named.
//! - **The Search feed (`declare_indexable` + the permission-filtered query/semantic) is KN-P21
//!   (P-311).** The `> 5%` search-block prune (KQ-10) is the measured, parallel follow-on that lands
//!   with the search feed. Named.
//! - **The live OLTP store + the per-viewer ABAC `check` body** the projector reads is the KN-P05 /
//!   KN-P16 store-wiring (the SAME entity shapes the live store hydrates — the projection logic is
//!   store-agnostic; the in-memory [`PageStore`] here is the floor). Named.
//! - **The Refs SERVICE-side inverse pairing + the both-directions projection** is the Refs mirror
//!   consumer (`myelin_refs_service::mirror`, REF-P14) — Knowledge emits the FORWARD typed event only
//!   (the SAME boundary that makes Git's `typed_edges` the producer half; Knowledge is a producer LEAF
//!   and cannot depend on the Refs service crate, §2.9 acyclic DAG). Named.

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

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 1. THE INLINE-NODE refs.edge.created PRODUCER (contract 5.4 — produced, NOT coalesced)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The frozen `refs.edge.created` event type (contract 5.4 — the emit-side token).** The ONLY
/// edge-creation event a content-node producer emits; the `rel_class` field distinguishes a
/// `reference`-class content edge from a `lifecycle`-class TE-7 mirror edge. A named constant so the
/// CDC asserts the NAME, never a literal (EI-01 §3). Byte-identical to
/// `myelin_git::body::REFS_EDGE_CREATED` + `myelin_refs_service::emit::REFS_EDGE_CREATED` — the ONE
/// wire shape the Refs edge-builder ingests across all three producers (X-2).
pub const REFS_EDGE_CREATED: &str = "refs.edge.created";

/// **The frozen `rel_class` token a CONTENT-node (reference) edge carries (contract 5.4 / Refs §3.2).**
/// A `mention`/`artifact_ref`/`embed` node is ALWAYS `reference`-class (NEVER `lifecycle` — the two
/// never alias; a TE-7 mirror edge is [`REL_CLASS_LIFECYCLE`]). Byte-identical to
/// `myelin_git::body::REL_CLASS_REFERENCE`.
pub const REL_CLASS_REFERENCE: &str = "reference";

/// **The frozen `rel_class` token a TYPED-EDGE (TE-7 lifecycle) mirror edge carries (contract 5.5 /
/// Refs §3.2).** A `page_parent` / `db_relation` mirror edge is ALWAYS `lifecycle` (NEVER `reference`).
/// Byte-identical to `myelin_git::typed_edges::REL_CLASS_LIFECYCLE`.
pub const REL_CLASS_LIFECYCLE: &str = "lifecycle";

/// **The shared edge-aggregate-key convention `edge:<source>-><target>` (EB-03 ordering anchor).**
/// Every `refs.edge.*` event for ONE logical edge shares this aggregate so an edge's create → remove
/// sequence is per-aggregate ordered. Byte-identical to `myelin_git::body::edge_aggregate_key` +
/// `myelin_refs_service::emit::edge_aggregate_key` — Knowledge's content + TE-7 edges share the SAME
/// ordering aggregate the Git producers + the Refs mirror use (one ordering key across producers).
/// PII-free (opaque URNs only).
pub fn edge_aggregate_key(source: &ArtifactRef, target: &ArtifactRef) -> AggregateKey {
    AggregateKey(format!("edge:{}->{}", source.0, target.0))
}

/// The per-`InlineNode` reference relation token (the `rel` column, §3.2 reference-class vocabulary).
/// A `mention` is a `mentions` edge; an `artifact_ref` is a `references` edge; an `embed` is an
/// `embeds` edge. The token strings are byte-identical to the Refs edge-builder's reference-class
/// vocabulary (the CDC pins the equivalence).
fn node_rel(node: &InlineNode) -> &'static str {
    match node {
        InlineNode::Mention(_) => "mentions",
        InlineNode::ArtifactRefNode(_) => "references",
        InlineNode::Embed(_) => "embeds",
    }
}

/// The edge TARGET URN for an inline node. A `mention(Principal)` targets the principal's identity URN
/// (`myelin://<tenant>/identity/principal/<id>` — the SAME URN Notif's inbox keys on); an
/// `artifact_ref`/`embed` targets the referenced artifact verbatim. The `mention` target is opaque
/// (the `principal_id`, never PII — references-not-payloads).
fn node_target(node: &InlineNode, tenant: &TenantId) -> ArtifactRef {
    match node {
        InlineNode::Mention(p) => ArtifactRef(format!(
            "myelin://{}/identity/principal/{}",
            tenant.0, p.principal_id.0
        )),
        InlineNode::ArtifactRefNode(r) | InlineNode::Embed(r) => r.clone(),
    }
}

/// Build the canonical `refs.edge.created` [`EventDraft`] for one content-node edge (the
/// references-not-payloads `source`/`target`/`rel`/`rel_class` triple + the shared edge aggregate).
/// `rel_class = reference` (NEVER lifecycle). `contains_personal_data = false`: a `mention` carries
/// only the opaque `principal_id` URN (the human ↔ pseudonym map is Identity's erasable record), so no
/// inline PII rides the envelope (contract 2.7).
fn content_edge_draft(source: &ArtifactRef, node: &InlineNode, tenant: &TenantId) -> EventDraft {
    let target = node_target(node, tenant);
    EventDraft {
        type_: EventType(REFS_EDGE_CREATED.into()),
        // The referencing side (the page/block the node sits in) is the event subject.
        subject: source.clone(),
        aggregate: edge_aggregate_key(source, &target),
        payload: serde_json::json!({
            "source": source.0,
            "target": target.0,
            "rel": node_rel(node),
            // A content-node edge is ALWAYS reference-class (§3.2; never lifecycle).
            "rel_class": REL_CLASS_REFERENCE,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

/// **Emit ONE `refs.edge.created` per inline `mention`/`artifact_ref`/`embed` node, IN THE SAME
/// TRANSACTION as the block persist (contract 5.4 — the content-node producer; NOT coalesced).**
///
/// `tx` is the OPEN outbox transaction the caller staged the block write + its
/// `knowledge.block.created`/`.updated` event into; `block_cause` is that block event (the CAUSE — so
/// the causal triple carries by construction, AG-6). `source` is the page/block URN the nodes sit in
/// (the referencing side). For each structured node, this calls [`OutboxTx::emit`]`(draft, cause)` —
/// the ONE sanctioned emit verb (the `no-raw-publish` lint; there is NO standalone edge-write API).
///
/// **NOT coalesced (a discrete edge fact, §1.4):** unlike `knowledge.page.updated` (debounced), an
/// edge is a discrete fact — one event per persisted node, emitted directly. **Emit-iff-committed
/// (KN-D7):** the rows are BUFFERED into `tx`; an aborted block persist drops them with it (no edge
/// without its committed node). This fn performs NO commit (the caller owns the lifecycle). Returns the
/// minted `event_id`s in node order.
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

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 2. THE TE-7 TYPED-EDGE MIRROR (contract 5.5 — owned source of truth; same-tx emit)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// The TE-7 lifecycle relation a Knowledge typed-row emits (a SUBSET of the frozen §3.3 vocabulary
/// `closes/blocks/blocked_by/depends_on/parent/assigns/relates`). Knowledge owns exactly the
/// page-tree `parent` (the `page_parent` typed table) + the db `relates`/`rollup_source` (the
/// `db_relation` typed table). The token strings are byte-identical to the Refs mirror's lifecycle
/// vocabulary (the CDC pins the equivalence); Knowledge produces the SAME wire tokens the Refs mirror
/// ingests, it does not author a second vocabulary. PII-free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KnowledgeLifecycleRel {
    /// `parent` — a `page_parent` typed row (a sub-page nests under a parent page). Refs projects the
    /// forward `parent` edge AND fixes the inverse `child` pairing (contract 5.5; the inverse is the
    /// Refs mirror's, not Knowledge's).
    Parent,
    /// `relates` — a plain two-way `db_relation` field. SYMMETRIC; Refs projects the inverse swap.
    Relates,
    /// `rollup_source` — the `db_relation` a read-time rollup aggregates over (KN-P18). Refs projects a
    /// `rollup_source` lifecycle edge.
    RollupSource,
}

impl KnowledgeLifecycleRel {
    /// The frozen `rel` column token (`'parent' | 'relates' | 'rollup_source'`, §3.2/§3.3 vocabulary).
    /// PII-free. Byte-identical to the Refs mirror lifecycle-rel tokens.
    pub fn as_str(self) -> &'static str {
        match self {
            KnowledgeLifecycleRel::Parent => "parent",
            KnowledgeLifecycleRel::Relates => "relates",
            KnowledgeLifecycleRel::RollupSource => "rollup_source",
        }
    }
}

/// Map a [`RelationKind`] (the `db_relation.rel` column) to its TE-7 lifecycle rel. The wire tokens
/// already agree (`relates`/`rollup_source`); this is the typed bridge so the emit path reads the one
/// vocabulary.
fn rel_of(kind: RelationKind) -> KnowledgeLifecycleRel {
    match kind {
        RelationKind::Relates => KnowledgeLifecycleRel::Relates,
        RelationKind::RollupSource => KnowledgeLifecycleRel::RollupSource,
    }
}

/// Build the page-root URN `myelin://<tenant>/knowledge/page/<page_id>` (the TE-7 `parent` edge's
/// endpoints are pages).
fn page_urn(tenant: &TenantId, page: &PageId) -> ArtifactRef {
    ArtifactRef(format!("myelin://{}/knowledge/page/{}", tenant.0, page.0))
}

/// Build the row-root URN `myelin://<tenant>/knowledge/row/<row_id>` (the `db_relation` source row).
fn row_urn(tenant: &TenantId, row_id: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://{}/knowledge/row/{}", tenant.0, row_id))
}

/// Build the `knowledge.page.parent_set` [`EventDraft`] (the TE-7 `page_parent` → Refs `parent` edge,
/// §3.1). The references-not-payloads payload carries the `source` (the child page), the `target` (the
/// parent page), the `rel = parent`, and `rel_class = lifecycle`. The aggregate is the child PAGE (the
/// §4 ordering partition — a page's parent-set events are per-page ordered). The Refs mirror projects
/// the forward `parent` edge + fixes the inverse `child`.
fn parent_set_draft(child: &ArtifactRef, parent: &ArtifactRef) -> EventDraft {
    EventDraft {
        type_: EventType(KNOWLEDGE_PAGE_PARENT_SET.into()),
        subject: child.clone(),
        // The aggregate is the child page (the page's ordering partition, contract 2.3).
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

/// **Emit `knowledge.page.parent_set` for a `page_parent` typed-row write, IN THE SAME TRANSACTION as
/// the typed-row write (contract 5.5 — the TE-7 mirror; the typed table is TRUTH).**
///
/// Call this from the SAME `tx` that wrote the [`PageTree::set_parent`] row (the typed table is the
/// source of truth; this emits the event Refs projects). `cause` threads the causal triple when this
/// rides a larger transaction. Returns the minted `event_id`.
///
/// **0 typed-row-without-edge:** because the emit shares `tx` with the typed-row write, the
/// `page_parent` row and its mirror event co-commit (emit-iff-committed) — an aborted re-parent drops
/// the buffered event with it; a committed re-parent always carries its mirror edge.
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

/// Build the `knowledge.relation.created`/`.removed` [`EventDraft`] for a `db_relation` typed-row
/// write (the TE-7 mirror, §3.1). `created` selects the event type. The payload carries `source` (the
/// db source row URN), `target` (the `dst_ref` verbatim — may be cross-subsystem), `rel`
/// (`relates`/`rollup_source`), `rel_class = lifecycle`. The aggregate is the shared
/// `edge:<source>-><target>` so the relate → unrelate sequence is per-aggregate ordered.
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

/// **Emit `knowledge.relation.created` / `.removed` for a `db_relation` typed-row write/delete, IN THE
/// SAME TRANSACTION as the typed-row write (contract 5.5 — the TE-7 mirror; the typed table is TRUTH).**
///
/// Call this from the SAME `tx` that [`RelationStore::relate`]/[`RelationStore::unrelate`] mutated the
/// `db_relation` table in (the typed table is the source of truth; this emits the event Refs
/// projects). `created` is `true` for a relate, `false` for an unrelate. Returns the minted
/// `event_id`. The forward edge is emitted only; the Refs mirror projects the inverse swap.
///
/// **0 typed-row-without-edge:** the `db_relation` row and its mirror event co-commit
/// (emit-iff-committed).
pub fn emit_relation_edge(
    tx: &mut dyn OutboxTx,
    tenant: &TenantId,
    relation: &DbRelation,
    created: bool,
    cause: Option<&EventEnvelope>,
) -> BusResult<EventId> {
    tx.emit(relation_draft(tenant, relation, created), cause)
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 3. THE PROJECTION SHAPE + THE 4-STEP TOMBSTONE LADDER (contracts 5.6 / 5.7 / §2.1, §2.2)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// A **per-viewer projection** of a Knowledge artifact (contract 5.6, §2.2 — the frozen
/// `{title, state, icon, render_hint, sub_anchor?}` shape). Built ONLY after the per-viewer permission
/// check passes; a denied/erased/restricted viewer gets a [`Tombstone`] instead, never this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projection {
    /// The artifact title (the page title; the parent-page title for a block sub-anchor; the database/
    /// row/view label). NEVER rendered for an unauthorized viewer (the 0-leak invariant — the deny path
    /// returns a tombstone that never reads this field).
    pub title: String,
    /// The artifact state token (`live`/`published`/`archived`).
    pub state: String,
    /// The icon token the UI renders (`page`/`block`/`database`/`row`/`view`). A frozen vocabulary.
    pub icon: String,
    /// The render hint (§2.2 `render_hint`) — the kind label Refs/Search/Notif render. `page`/`block`/
    /// `database`/`row`/`view`.
    pub render_hint: String,
    /// The sub-anchor projection — set when the projected ref carried a `#sub` (a block/heading/row/
    /// field/comment/thread excerpt), carrying the ladder rung (`live`/`moved`/`outdated`). `None` for
    /// a bare-root projection. A `GONE`/`ERASED` sub never reaches here (it is a tombstone).
    pub sub_anchor: Option<SubAnchor>,
}

/// A projected sub-anchor (§2.2 `sub_anchor`) — the stable opaque sub-id + the ladder rung the
/// resolver landed on. A `MOVED` block still resolves (the `block_id` is stable across tree moves); an
/// `OUTDATED` block resolves partially (its anchored range shifted). `GONE`/`ERASED` are tombstones,
/// never a `SubAnchor`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubAnchor {
    /// The `#sub` kind label (`b`/`h`/`row-`/`field-`/`comment-`/`thread-`).
    pub kind: String,
    /// The stable opaque sub-id (the `block_id`/`row_id`/`comment_id`) the anchor resolved to.
    pub sub_id: String,
    /// The ladder rung the sub-anchor resolved on (`live`/`moved`/`outdated`).
    pub rung: LadderRung,
}

/// The 4-step tombstone ladder's sub-resolution RUNG for a still-resolvable sub-anchor (§2.1). `GONE`
/// and `ERASED` are NOT rungs here — they degrade to a [`Tombstone`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LadderRung {
    /// `LIVE` — the block/row/comment resolves exactly (the unfurl/embed).
    Live,
    /// `MOVED` — the block moved in the tree but the stable `block_id` still resolves (a tree move, not
    /// a 3-way diff — Knowledge's anchors are stable opaque ids, §2.1).
    Moved,
    /// `OUTDATED` — an edited block whose anchored range shifted; the projection is partial.
    Outdated,
}

impl LadderRung {
    /// The frozen rung token (`live`/`moved`/`outdated`).
    pub fn as_str(self) -> &'static str {
        match self {
            LadderRung::Live => "live",
            LadderRung::Moved => "moved",
            LadderRung::Outdated => "outdated",
        }
    }
}

/// A **tombstone** — the projection of an artifact the viewer may NOT see, or whose sub-anchor is
/// gone/erased (contract 5.6, §2.1). It carries NO title/content (the 0-leak invariant) — only the
/// permission-checked ROOT, so an embed degrades to "this referenced <page> (the specific block is no
/// longer available)" rather than vanishing. A tombstone ALWAYS carries the root (§2.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tombstone {
    /// Why the projection is a tombstone — for the AUDIT log, NEVER rendered to the viewer.
    pub reason: TombstoneReason,
    /// **The `#sub`-stripped ROOT URN** (§2.1 — a tombstone always carries the root). For a `Denied`
    /// tombstone this is the stripped root URN ONLY (an opaque scope, never a title) so a backlink can
    /// render "(not available)" while still pointing at the parent; for `RootGone`/`SubGone`/`Erased`
    /// it is the root the embed degrades to. NEVER a title (the URN structure is not the content).
    pub root: ArtifactRef,
}

/// Why a projection degraded to a [`Tombstone`] (§2.1 — the 4-step ladder reasons; the audit reason,
/// never leaked to the viewer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TombstoneReason {
    /// Step 1 — `check(viewer, read, root)` denied. The deny path NEVER reads the artifact's title (0
    /// leak). The frozen ladder's first rung (§2.1).
    Denied,
    /// Step 2 — the root page/database does not exist (a dangling reference). The embed degrades to the
    /// (gone) root URN.
    RootGone,
    /// Step 3 `GONE` — the root resolves, but the sub-anchor (block/row/comment) is dead. The embed
    /// shows the parent page (§2.1 — "the page resolves; the block is dead, embed shows the page").
    SubGone,
    /// Step 4 `ERASED` (any level) — pseudonym-/crypto-shred made the content unrenderable (KN-D4).
    Erased,
}

impl Tombstone {
    /// The generic, content-free text the VIEWER sees (never the title/state/reason). The same string
    /// regardless of reason — a denied viewer cannot distinguish "denied" from "erased" (no
    /// information leaks through the tombstone text either).
    pub fn display_text(&self) -> &'static str {
        "(not available)"
    }
}

/// The result of [`Projector::project`]: a per-viewer [`Projection`] (authorised + present + live/
/// moved/outdated sub) or a [`Tombstone`] (denied / root-gone / sub-gone / erased). The two-variant
/// shape IS the §2.2 contract (`Projection | Tombstone`) — a projector NEVER returns a bare title to a
/// denied viewer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Projected {
    /// The authorised, present projection.
    Visible(Projection),
    /// The denied / root-gone / sub-gone / erased tombstone (no leaked content; carries the root).
    Tombstoned(Tombstone),
}

impl Projected {
    /// `true` iff this is a visible projection.
    pub fn is_visible(&self) -> bool {
        matches!(self, Projected::Visible(_))
    }

    /// `true` iff this is a tombstone.
    pub fn is_tombstone(&self) -> bool {
        matches!(self, Projected::Tombstoned(_))
    }

    /// The projected title IF visible, else `None`. The 0-leak helper: a tombstone has no title, so a
    /// caller asserting "an unauthorized viewer never gets the title" reads `None` here.
    pub fn title(&self) -> Option<&str> {
        match self {
            Projected::Visible(p) => Some(&p.title),
            Projected::Tombstoned(_) => None,
        }
    }
}

/// A loud, typed projection error (a malformed / non-Knowledge ref) — distinct from a [`Tombstone`]
/// (which is a SUCCESSFUL projection of a hidden/gone artifact). An error means the ref is not a
/// Knowledge artifact at all; a tombstone means it is, but is hidden/gone for this viewer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectError {
    /// The ref is not a Knowledge artifact (wrong subsystem / malformed scope).
    NotAKnowledgeArtifact {
        /// The offending reference string.
        reference: String,
    },
    /// The `<type>` token is not a Knowledge type the projector owns.
    UnknownKnowledgeType {
        /// The rejected type token.
        ty: String,
    },
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectError::NotAKnowledgeArtifact { reference } => write!(
                f,
                "not a Knowledge artifact: `{reference}` — Knowledge's projector does not own this ref"
            ),
            ProjectError::UnknownKnowledgeType { ty } => {
                write!(f, "unknown Knowledge artifact type `{ty}`")
            }
        }
    }
}

impl std::error::Error for ProjectError {}

/// The frozen Knowledge artifact types `project` projects (the `<type>` token of the canonical
/// `ArtifactRef`). A closed set — Knowledge is the resolver-owner of exactly these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KnArtifactType {
    Page,
    Block,
    Database,
    Row,
    View,
}

/// Classify a parsed Knowledge `ArtifactRef` to its [`KnArtifactType`] (reading the `<subsystem>`/
/// `<type>` scope segments — never a render-time display form).
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

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 4. THE PAGE STORE (the KN-P05 live-store floor — in-memory here) + the sub-anchor resolver inputs
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// A page's projectable metadata (title + publish/archive state). The live `page` OLTP row is KN-P05;
/// here it is the projection input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageMeta {
    /// The page title — the projection title (a confidential page's title NEVER leaks; it is read only
    /// AFTER the permission check passes).
    pub title: String,
    /// The page state (`live`/`published`/`archived`).
    pub state: String,
}

/// The resolved state of a `#sub` sub-anchor against the live store (the ladder's step-3 input). The
/// store reports which rung a sub-id is on; the projector maps it to a [`SubAnchor`] (live/moved/
/// outdated) or a [`Tombstone`] (gone). For Knowledge the `MOVED` case is a TREE move (the stable
/// `block_id` still resolves), not a 3-way diff — §2.1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubState {
    /// The block/row/comment resolves exactly.
    Live,
    /// The block moved in the tree; the stable `block_id` still resolves.
    Moved,
    /// An edited block whose anchored range shifted; resolves partially.
    Outdated,
    /// The sub-artifact is dead (the root resolves, the sub does not) → a `SubGone` tombstone.
    Gone,
}

/// The in-memory **page store** the projector reads (the KN-P05 live-OLTP-store FLOOR). Keyed by the
/// canonical ROOT `ArtifactRef` string — the SAME entity shapes the live store hydrates, so the
/// projection logic is store-agnostic. Carries the erased/restricted flags the §2.1 erasure-/
/// restriction-safe tombstone reads, and the per-sub-URN sub-anchor state for the ladder's step 3.
#[derive(Clone, Debug, Default)]
pub struct PageStore {
    /// Page/database/row/view metadata by canonical ROOT ref string.
    roots: HashMap<String, PageMeta>,
    /// The sub-anchor resolved state by FULL sub-URN string (the ladder's step-3 input).
    subs: HashMap<String, SubState>,
    /// The set of canonical ref strings (root OR sub-URN) that have been ERASED (a `*.erased`
    /// tombstone) — projecting one returns an `Erased` tombstone, never the (shredded) content.
    erased: HashSet<String>,
    /// The set of canonical ref strings whose subject is RESTRICTED (the GDPR `restrict` flag) —
    /// projecting one returns a tombstone (the restriction window suppression, §6).
    restricted: HashSet<String>,
}

impl PageStore {
    /// A fresh empty store.
    pub fn new() -> PageStore {
        PageStore::default()
    }

    /// Insert a root artifact's projectable metadata keyed by its canonical ROOT ref.
    pub fn put_root(&mut self, root: &ArtifactRef, meta: PageMeta) {
        self.roots.insert(root.0.clone(), meta);
    }

    /// Set a sub-anchor's resolved state keyed by its FULL sub-URN (the ladder's step-3 input).
    pub fn put_sub_state(&mut self, sub_ref: &ArtifactRef, state: SubState) {
        self.subs.insert(sub_ref.0.clone(), state);
    }

    /// Mark a canonical ref (root or sub-URN) ERASED (a `*.erased` tombstone, §2.1 step 4).
    pub fn mark_erased(&mut self, reference: &ArtifactRef) {
        self.erased.insert(reference.0.clone());
    }

    /// Whether a canonical ref (root or sub-URN) is marked ERASED (a `*.erased` tombstone). The KN-P26
    /// erase floor asserts the backlink tombstone took effect; the projector reads the same set to
    /// degrade the ref to an `Erased` tombstone (§2.1 step 4 — never the shredded content).
    pub fn is_erased(&self, reference: &ArtifactRef) -> bool {
        self.erased.contains(&reference.0)
    }

    /// Mark a canonical ref's subject RESTRICTED (the GDPR `restrict` flag, §6).
    pub fn mark_restricted(&mut self, reference: &ArtifactRef) {
        self.restricted.insert(reference.0.clone());
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 5. THE PROJECTOR — project(ref, viewer): the frozen 4-step tombstone ladder, permission FIRST
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// The `read` permission the projector checks before reading a Knowledge artifact (§2.1 step 1 —
/// `check(viewer, read, root)`). Spelled once.
pub const READ: &str = "read";

/// **The Knowledge `project(ref, viewer)` projector (contract 5.6 — the KN-P19 deliverable).** The
/// ONLY way Refs/Search/Notif read a Knowledge artifact (no cross-DB). Holds the [`IdentityService`]
/// (the per-viewer permission source) + the [`PageStore`] (the own-DB read — the KN-P05 store floor
/// in-memory here). Generic over `I: IdentityService` so the service wires the real Id resolver and
/// tests wire a deterministic one.
pub struct Projector<I: IdentityService> {
    id: I,
    store: PageStore,
}

impl<I: IdentityService> Projector<I> {
    /// Compose the projector over the Id dependency + the page store.
    pub fn new(id: I, store: PageStore) -> Projector<I> {
        Projector { id, store }
    }

    /// A borrow of the underlying store (for the service / drills to seed or inspect).
    pub fn store_mut(&mut self) -> &mut PageStore {
        &mut self.store
    }

    /// **`project(ref, viewer) -> Projection | Tombstone` (contract 5.6 / 5.7, the frozen 4-step
    /// tombstone ladder §2.1).**
    ///
    /// The ladder is the load-bearing invariant (the project-leak gate):
    /// 1. **permission FIRST** — `check(viewer, read, root)` on the `#sub`-stripped ROOT (a sub is
    ///    never more visible than its parent). Deny / Conditional / Id-error all fail CLOSED to a
    ///    `Denied` tombstone carrying the root — NO field of the artifact read (0 leak).
    /// 2. **root resolve** — the page/database exists? No ⇒ a `RootGone` tombstone carrying the root.
    /// 3. **ERASED (any level)** — the root or the sub is erased ⇒ an `Erased` tombstone. (Checked
    ///    before the sub-resolve so a shredded sub never renders.) Restriction is suppressed likewise.
    /// 4. **sub resolve** — for a `#sub` ref, the store's [`SubState`]: `Live`/`Moved`/`Outdated`
    ///    project a [`SubAnchor`] on that rung; `Gone` ⇒ a `SubGone` tombstone carrying the root (the
    ///    page resolves; the embed shows the parent).
    ///
    /// A tombstone ALWAYS carries the `#sub`-stripped root (§2.1). The `zookie` is the read-consistency
    /// fence (a strong, zookie-stamped read for a security-sensitive projection).
    pub fn project(
        &self,
        reference: &ArtifactRef,
        viewer: &Principal,
        zookie: Zookie,
    ) -> Result<Projected, ProjectError> {
        // Classify FIRST — a non-Knowledge / unknown-type ref is a loud error (not a tombstone).
        let ty = classify(reference)?;
        let root = myelin_refs::strip_sub(reference);

        // ── STEP 1: PERMISSION FIRST (the 0-leak gate, root-keyed). A Deny / Conditional / Id-error
        //    ALL fail closed to a `Denied` tombstone carrying the ROOT, with NO artifact field read.
        let at = Consistency {
            at_least: zookie,
            mode: ConsistencyMode::Strong,
        };
        let permission = Permission(READ.to_string());
        match self.id.check(viewer, &permission, &root, &at, None) {
            Ok(Decision::Allow) => { /* authorised — descend the ladder */ }
            Ok(Decision::Deny) | Ok(Decision::Conditional) | Err(_) => {
                return Ok(Projected::Tombstoned(Tombstone {
                    reason: TombstoneReason::Denied,
                    root,
                }));
            }
        }

        // ── STEP 4 (erasure, checked early so a shredded artifact never renders, §2.1 step 4). Keyed
        //    on the ROOT and the full ref — an erased page tombstones its sub-anchors too. Restriction
        //    (the GDPR suppression window, §6) degrades to the SAME content-free tombstone.
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

        // ── STEP 2: ROOT RESOLVE — the page/database exists? No ⇒ a `RootGone` tombstone (the root URN
        //    is opaque scope, never a title).
        let meta = match self.store.roots.get(&root.0) {
            Some(m) => m.clone(),
            None => {
                return Ok(Projected::Tombstoned(Tombstone {
                    reason: TombstoneReason::RootGone,
                    root,
                }));
            }
        };

        // ── STEP 3: SUB RESOLVE (for a `#sub` ref) — map the store's SubState to a rung or a SubGone
        //    tombstone. A bare-root ref has no sub-anchor (None).
        let sub_anchor = match myelin_refs::sub_kind(reference) {
            Some(sub) => {
                let state = self
                    .store
                    .subs
                    .get(&reference.0)
                    .copied()
                    // An un-tracked sub defaults to LIVE (the common case — a freshly-minted anchor).
                    .unwrap_or(SubState::Live);
                match sub_state_to_rung(state) {
                    Some(rung) => Some(SubAnchor {
                        kind: sub.kind().label().to_string(),
                        sub_id: sub_opaque_id(&sub),
                        rung,
                    }),
                    // GONE: the root resolves, the sub is dead → a SubGone tombstone carrying the root.
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

        // ── BUILD THE PER-VIEWER PROJECTION (§2.2 — only now read the title; the deny path never did).
        Ok(Projected::Visible(Projection {
            title: meta.title,
            state: meta.state,
            icon: icon_for(ty).to_string(),
            render_hint: icon_for(ty).to_string(),
            sub_anchor,
        }))
    }
}

/// Map a [`SubState`] to its still-resolvable [`LadderRung`], or `None` for `GONE` (a tombstone).
fn sub_state_to_rung(state: SubState) -> Option<LadderRung> {
    match state {
        SubState::Live => Some(LadderRung::Live),
        SubState::Moved => Some(LadderRung::Moved),
        SubState::Outdated => Some(LadderRung::Outdated),
        SubState::Gone => None,
    }
}

/// The opaque sub-id body of a parsed [`myelin_refs::Sub`] (the `block_id`/`row_id`/`comment_id`). For
/// the kinds Knowledge resolves; a non-Knowledge sub renders its body verbatim.
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
        | Sub::Check(id) => id.clone(),
        Sub::Step(n) => n.to_string(),
        Sub::LineRange { start, end } => format!("L{start}-L{end}"),
    }
}

/// The frozen `icon`/`render_hint` token for a Knowledge artifact type (§2.2 vocabulary).
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

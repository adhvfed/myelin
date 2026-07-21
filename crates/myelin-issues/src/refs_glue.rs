//! # `refs_glue` — Issues' Refs wiring: the `#sub` mints, `project(ref,viewer)` (the 4-step tombstone
//! ladder), the inline-node `refs.edge.created` producer, the TE-7 typed-edge mirror, the bounded
//! cycle-safe traverse, and the `issue.*` Search projection emitter (ISS-P17 / P-383, M4)
//!
//! **The M4-I3 Refs/Search wiring slice.** [`crate::declares`] (ISS-P04 / P-243) already DECLARED the
//! `issue.*` Search `IndexSpec` (the facets-projection schema) + registered the `#sub` mints' OWNERSHIP
//! is implicit in the frozen grammar; [`crate::content`] (ISS-P10) named the "structured-node →
//! `refs.edge.created` emission" as this prompt's Refs-producer band. This module ships the GLUE the
//! named floors there pointed at — the LIVE emitter + the LIVE wiring:
//!
//! 1. **The `#sub` mints Issues owns (5.7, owned).** `comment-`/`b`/`field-`/`row-` — STABLE opaque ids
//!    minted through the ONE [`myelin_refs::mint`] codec (0 ungrammatical mints by construction). Refs
//!    stores the full sub-URN + the [`myelin_refs::strip_sub`] root, so a broken sub-anchor still
//!    resolves to the parent issue.
//! 2. **`project(ref, viewer)` + the one 4-step tombstone ladder (5.6 owned / 5.7).** The ONLY way
//!    Refs/Search/Notif read an Issues artifact (no cross-DB), per-viewer pre-permission-checked: a
//!    confidential issue degrades to a content-free [`Tombstone`] carrying the ROOT — the title NEVER
//!    leaks (the project-leak counter = 0, the ISS-D3 slice re-asserted at the unfurl boundary).
//! 3. **The three inline-node `refs.edge.created` producer (5.4, produced).** `mention` /
//!    `artifact_ref` / `embed` content nodes ([`myelin_content::inline::InlineNode`]) emit ONE
//!    `refs.edge.created` per node on persist, via the OUTBOX, NOT coalesced (a discrete edge fact).
//!    The three nodes are the uniform producers across Chat/Issues/Knowledge (X-2); there is NO
//!    standalone edge-write API.
//! 4. **The TE-7 typed-edge mirror (5.5, owned source of truth).** The SAME transaction that writes an
//!    `issue_relation` typed row emits `issue.relation.created`/`.removed` so Refs projects the
//!    lifecycle edge. The typed table is TRUTH; Refs holds the rebuildable projection + fixes the
//!    inverse pairing (one event yields BOTH projection directions).
//! 5. **The bounded cycle-safe traverse (5.3).** A depth-16 BFS over the `issue_relation` forward edges
//!    ([`IssueRelationGraph::traverse`]) — cycle-safe (a visited-set prunes a `blocked_by` cycle), so
//!    the impact/hierarchy walk terminates even on a malformed cyclic graph.
//! 6. **The `issue.*` Search projection emitter (6.3, the LIVE emitter; reindex 6.4).** The
//!    [`IssueProjectFetcher`] is the `project(ref)` → [`myelin_search::SearchProjection`] push body the
//!    incremental indexer fetches through (the SAME [`Projector::project`] cross-DB read — ACL-filtered,
//!    restriction-/erasure-safe); `reindex(scope)` rebuilds it via the ONE replay-from-source path
//!    ([`crate::replay`], contract 2.6) — the only rebuild path.
//!
//! **Owning architecture doc (read in full before changing this):**
//! `04-subsystem-architectures/issue-tracker/architecture/03-events-contracts-and-glue.md`
//! §2 (the `ArtifactRef` + the `#sub` mints `comment-`/`b`/`field-`/`row-`), §3 (`project(ref,viewer)`
//! — the frozen `{title, state, icon, render_hint, sub_anchor?}` shape; permission FIRST; a
//! confidential issue → tombstone carrying the root, never the title), §4 (`replay(scope, since)` —
//! reindex-from-source). **Reconciliation:** `00-reconciliation-decisions.md` X-4/OQ-D (the unified
//! `#sub` grammar + the ONE 4-step ladder), OQ-I (cell-local resolution — the cross-cell bridge is the
//! M5 follow-on). **Contracts:** `contract-index.md` rows 5.6 (project, owned), 5.1/5.2/5.7 (the
//! ArtifactRef / resolve-tombstone / `#sub` mints), 5.4 (the inline-node edges), 5.3 (traverse), 5.5
//! (the TE-7 mirror), 6.3/6.4 (the issue.* projection emitter + reindex).
//!
//! ## Why permission-FIRST (the 0-leak invariant — EI-01 §3 prove-it)
//! [`Projector::project`] runs the permission check on the `#sub`-stripped ROOT (a sub is never more
//! visible than its parent) BEFORE any field of the issue is read into the projection, so a denied
//! viewer's result is a [`Tombstone`] carrying ONLY the root — NO title/content ever fetched on the
//! deny path. An Id transport hiccup fails CLOSED to a tombstone (never a leak). This is the
//! mandatory-core leak surface — the mutation floor is stated below.
//!
//! ## Mutation-score floor (mandatory-core, EI-01 §3 / prove-it)
//! `project(ref, viewer)` is the **mandatory-core leak surface** (a project leak IS the failure). The
//! floor for the project/ladder path is **≥ 90% of viable mutants caught**
//! (`cargo mutants -p myelin-issues -f crates/myelin-issues/src/refs_glue.rs`). **Measured 2026-06-23:
//! 45 caught / 48 viable = 93.75% (≥ 90% — floor MET).** The load-bearing logic — the permission-first
//! gate (deny ⇒ tombstone, root-keyed, **never the title**), the erased/restricted `||`-over-(root,
//! full-ref) guards (BOTH the projector's and the emitter's arms), each ladder rung
//! (`LIVE`/`MOVED`/`OUTDATED`/`GONE`/`ERASED`), the rung tokens, the TE-7 forward-edge emit shape, and
//! the cycle-safe traverse depth bound — each has a test a mutation flips:
//! `unauthorized_viewer_gets_a_tombstone_carrying_the_root_never_the_title`,
//! `an_erased_issue_projects_to_an_erased_tombstone`,
//! `a_restricted_sub_urn_tombstones_even_when_the_root_is_not` /
//! `an_erased_sub_urn_tombstones_even_when_the_root_is_not`,
//! `emitter_excludes_an_erased_or_restricted_sub_even_when_root_is_clean`,
//! `rung_tokens_and_projected_accessors_are_pinned`, `the_sub_anchor_ladder_*`, the `te7_*` mirror
//! tests, and `traverse_is_cycle_safe_and_depth_bounded`. The residual 3 misses are NOT the production
//! leak surface: the `ProjectError` `Display::fmt` (a diagnostic string, never a leak gate) + the two
//! `store_mut` test-seam accessors (test scaffolding, not the production project/index path). The
//! world-scale corpus-under-load + the cross-cell drill are later bands (ISS-P32/P-397).
//!
//! ## Named floors (VISION §3 / EI-01 §1)
//! - **The cross-cell projection bridge is the M5 follow-on (ISS-P32 / P-397).** This module resolves
//!   CELL-LOCAL only (OQ-I): a viewer in cell A renders a pointer to a cell-B issue by asking cell B to
//!   `project` THERE, permission-checked there; only the rendered projection (or a tombstone) crosses.
//!   The [`CrossCellPointer`](myelin_tenancy) bridge + the portfolio-rollup view that aggregates
//!   cross-cell projections land in ISS-P32. Named.
//! - **The Refs SERVICE-side inverse pairing + the both-directions projection** is the Refs mirror
//!   consumer (`myelin_refs_service::mirror`, REF-P14) — Issues emits the FORWARD typed event only (the
//!   SAME boundary that makes Git/Knowledge the producer half; Issues is a producer LEAF and cannot
//!   depend on the Refs service crate, §2.9 acyclic DAG). Named.
//! - **The live OLTP store + the per-viewer ABAC `check` body** the projector reads is the ISS-P05/P-13
//!   store-wiring (the SAME entity shapes the live store hydrates — the projection logic is
//!   store-agnostic; the in-memory [`IssueProjectionStore`] here is the floor). Named.

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

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 0. FROZEN NAMES (§2/§3 — never a stray literal)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// The canonical Issues subsystem token in the `myelin://<t>/issue/<type>/<id>` URN (Bus §6.2). The
/// SAME token [`crate::events`] anchors every `issue.*` event on + [`crate::declares::ISSUE_SUBSYSTEM`].
pub const ISSUE_SUBSYSTEM: &str = "issue";

/// The `view` permission the projector checks before reading an Issues artifact (§3: `project` runs
/// `Id.check(viewer, view, issue.ref())` FIRST; the frozen ReBAC `issue.view = (parent_project->read -
/// confidential) + confidential_grant`). Spelled once; mirrors [`crate::planner::ISSUE_VIEW_PERMISSION`].
pub const VIEW: &str = "view";

/// The maximum traverse depth (contract 5.3 — the bounded cycle-safe walk, depth 16). A walk that
/// would exceed this STOPS (the recursion is bounded; a malformed deep/cyclic graph never blows the
/// stack or hangs). Named once so the bound is the frozen contract value, never a stray literal.
pub const TRAVERSE_MAX_DEPTH: usize = 16;

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 1. THE #sub MINTS Issues OWNS (contract 5.7, §2) — comment- / b / field- / row-
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// Mint the canonical issue **root** `ArtifactRef`: `myelin://<tenant>/issue/issue/<key>` (§2),
/// VALIDATED through the ONE [`myelin_refs`] codec (0 ungrammatical mints — the `<key>` is the STORED
/// canonical `<PROJECTKEY>-<seqno>` id, never the `#<seqno>` display projection, REF-3). This is the
/// validating mint for the Refs-wiring surface; [`crate::write_path::issue_ref`] is the ISS-P06
/// write-path root builder (the SAME URN shape — this one re-parses through the codec so a malformed
/// key is rejected loudly at mint time).
pub fn issue_root_ref(tenant: &str, key: &str) -> ArtifactRef {
    myelin_refs::parse(&format!("myelin://{tenant}/{ISSUE_SUBSYSTEM}/issue/{key}"))
        .expect("Issues mints a grammatical canonical ArtifactRef (contract 5.1)")
}

/// **The `#comment-<opaqueid>` mint (contract 5.7, §2).** Attach a STABLE opaque comment/review-thread
/// id to an issue ROOT: `myelin://<t>/issue/issue/<key>#comment-<opaqueid>`. The opaque id is immutable
/// and never reused (the stability obligation is Issues', §2). Through the ONE Refs codec
/// ([`myelin_refs::mint`]), so an empty opaque body or a sub-of-a-sub is rejected LOUDLY.
pub fn comment_sub_ref(
    issue_root: &ArtifactRef,
    opaque_id: &str,
) -> Result<ArtifactRef, myelin_refs::ParseError> {
    myelin_refs::mint(issue_root, myelin_refs::Sub::Comment(opaque_id.to_string()))
}

/// **The `#b<opaqueid>` description-block mint (contract 5.7, §2).** A `myelin-content` block within
/// the issue DESCRIPTION body, stable across edits (the same `block_id` survives an edit so an embed
/// never dangles, §2).
pub fn block_sub_ref(
    issue_root: &ArtifactRef,
    opaque_id: &str,
) -> Result<ArtifactRef, myelin_refs::ParseError> {
    myelin_refs::mint(issue_root, myelin_refs::Sub::Block(opaque_id.to_string()))
}

/// **The `#field-<opaqueid>` field-value mint (contract 5.7, §2).** A field value on the issue — the
/// STABLE `field_id` UUID, NOT the display name (§2 — a renamed field never dangles an embed).
pub fn field_sub_ref(
    issue_root: &ArtifactRef,
    field_id: &str,
) -> Result<ArtifactRef, myelin_refs::ParseError> {
    myelin_refs::mint(issue_root, myelin_refs::Sub::Field(field_id.to_string()))
}

/// **The `#row-<opaqueid>` issue-as-row mint (contract 5.7, §2).** The issue rendered as a database row
/// in a `db_view` (issue-as-row); the stable opaque row id.
pub fn row_sub_ref(
    issue_root: &ArtifactRef,
    row_id: &str,
) -> Result<ArtifactRef, myelin_refs::ParseError> {
    myelin_refs::mint(issue_root, myelin_refs::Sub::Row(row_id.to_string()))
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 2. THE INLINE-NODE refs.edge.created PRODUCER (contract 5.4 — produced, NOT coalesced)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The frozen `refs.edge.created` event type (contract 5.4 — the emit-side token).** The ONLY
/// edge-creation event a content-node producer emits; the `rel_class` field distinguishes a
/// `reference`-class content edge from a `lifecycle`-class TE-7 mirror edge. Byte-identical to
/// `myelin_knowledge::refs_glue::REFS_EDGE_CREATED` + `myelin_git::body::REFS_EDGE_CREATED` — the ONE
/// wire shape the Refs edge-builder ingests across all three producers (X-2).
pub const REFS_EDGE_CREATED: &str = "refs.edge.created";

/// **The frozen `rel_class` token a CONTENT-node (reference) edge carries (contract 5.4 / Refs §3.2).**
/// A `mention`/`artifact_ref`/`embed` node is ALWAYS `reference`-class (NEVER `lifecycle`). Byte-
/// identical to the Knowledge/Git producers' `REL_CLASS_REFERENCE`.
pub const REL_CLASS_REFERENCE: &str = "reference";

/// **The frozen `rel_class` token a TYPED-EDGE (TE-7 lifecycle) mirror edge carries (contract 5.5 /
/// Refs §3.2).** An `issue_relation` mirror edge is ALWAYS `lifecycle` (NEVER `reference`). Byte-
/// identical to the Knowledge/Git producers' `REL_CLASS_LIFECYCLE`.
pub const REL_CLASS_LIFECYCLE: &str = "lifecycle";

/// **The shared edge-aggregate-key convention `edge:<source>-><target>` (EB-03 ordering anchor).**
/// Every `refs.edge.*` event for ONE logical edge shares this aggregate so an edge's create → remove
/// sequence is per-aggregate ordered. Byte-identical to the Knowledge/Git/Refs-mirror edge aggregate —
/// Issues' content + TE-7 edges share the SAME ordering aggregate. PII-free (opaque URNs only).
pub fn edge_aggregate_key(source: &ArtifactRef, target: &ArtifactRef) -> AggregateKey {
    AggregateKey(format!("edge:{}->{}", source.0, target.0))
}

/// The per-`InlineNode` reference relation token (the `rel` column, §3.2 reference-class vocabulary).
/// A `mention` → `mentions`; an `artifact_ref` → `references`; an `embed` → `embeds`. The token strings
/// are byte-identical to the Refs edge-builder's reference-class vocabulary (the CDC pins it).
fn node_rel(node: &InlineNode) -> &'static str {
    match node {
        InlineNode::Mention(_) => "mentions",
        InlineNode::ArtifactRefNode(_) => "references",
        InlineNode::Embed(_) => "embeds",
    }
}

/// The edge TARGET URN for an inline node. A `mention(Principal)` targets the principal's identity URN
/// (`myelin://<tenant>/identity/principal/<id>` — the SAME URN Notif's inbox keys on, opaque, never
/// PII); an `artifact_ref`/`embed` targets the referenced artifact verbatim (references-not-payloads).
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
/// `rel_class = reference` (NEVER lifecycle). `contains_personal_data = false`: a `mention` carries only
/// the opaque `principal_id` URN (the human ↔ pseudonym map is Identity's erasable record), so no inline
/// PII rides the envelope (contract 2.7).
fn content_edge_draft(source: &ArtifactRef, node: &InlineNode, tenant: &TenantId) -> EventDraft {
    let target = node_target(node, tenant);
    EventDraft {
        type_: EventType(REFS_EDGE_CREATED.into()),
        // The referencing side (the issue body/comment the node sits in) is the event subject.
        subject: source.clone(),
        aggregate: edge_aggregate_key(source, &target),
        payload: serde_json::json!({
            "source": source.0,
            "target": target.0,
            "rel": node_rel(node),
            "rel_class": REL_CLASS_REFERENCE,
        }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

/// **Emit ONE `refs.edge.created` per inline `mention`/`artifact_ref`/`embed` node, IN THE SAME
/// TRANSACTION as the body/comment persist (contract 5.4 — the content-node producer; NOT coalesced).**
///
/// `tx` is the OPEN outbox transaction the caller staged the body/comment write + its
/// `issue.issue.updated`/`issue.comment.*` event into; `content_cause` is that content event (the CAUSE
/// — so the causal triple carries by construction, AG-6). `source` is the issue body/comment URN the
/// nodes sit in (the referencing side). For each structured node, this calls [`OutboxTx::emit`]`(draft,
/// cause)` — the ONE sanctioned emit verb (the `no-raw-publish` lint; there is NO standalone edge-write
/// API). The [`crate::content::IssueContent::structured_nodes`] walk supplies `nodes`.
///
/// **NOT coalesced (a discrete edge fact):** one event per persisted node, emitted directly.
/// **Emit-iff-committed:** the rows are BUFFERED into `tx`; an aborted persist drops them with it (no
/// edge without its committed node). This fn performs NO commit. Returns the minted `event_id`s in node
/// order.
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

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 3. THE TE-7 TYPED-EDGE MIRROR (contract 5.5 — owned source of truth; same-tx emit)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// The TE-7 lifecycle relation an Issues `issue_relation` typed-row emits (the frozen §3.3 vocabulary
/// `closes/blocks/blocked_by/depends_on/parent/relates` — byte-identical to the `issue_relation.rel`
/// CHECK vocabulary, [`crate::migrations`]). Issues owns the FULL issue-lifecycle edge set (UNLIKE Git
/// which mints only `closes`/`relates`). The token strings are byte-identical to the Refs mirror's
/// lifecycle vocabulary; Issues produces the SAME wire tokens the Refs mirror ingests, never a second
/// vocabulary. PII-free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueLifecycleRel {
    /// `parent` — the issue-hierarchy parent edge (a sub-task under a parent issue). Refs projects the
    /// forward `parent` edge AND fixes the inverse `child` pairing.
    Parent,
    /// `blocks` — this issue blocks the target (the inverse of `blocked_by`).
    Blocks,
    /// `blocked_by` — this issue is blocked by the target (the flagship "remind me when unblocked"
    /// trigger reads this, arch §10).
    BlockedBy,
    /// `closes` — a PR/commit-rooted edge: merging the target closes this issue (the Git producer mints
    /// the Git side; Issues mints the issue↔issue close-relation side).
    Closes,
    /// `depends_on` — this issue depends on the target.
    DependsOn,
    /// `relates` — a plain two-way relation. SYMMETRIC; Refs projects the inverse swap.
    Relates,
}

impl IssueLifecycleRel {
    /// The frozen `rel` column token (byte-identical to the `issue_relation.rel` CHECK vocabulary +
    /// the Refs mirror lifecycle-rel tokens). PII-free.
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

    /// Parse the `issue_relation.rel` column token back to its lifecycle rel (the typed bridge so the
    /// emit/traverse path reads the one vocabulary). Returns `None` for an unknown token (a
    /// non-CHECK-vocabulary value can never reach the live table — the CHECK rejects it).
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

/// Build the `issue.relation.created`/`.removed` [`EventDraft`] for an `issue_relation` typed-row write
/// (the TE-7 mirror, §3.1 / contract 5.5). `created` selects the event type. The payload carries
/// `source` (the `src_issue` issue URN), `target` (the `dst_ref` verbatim — may be cross-subsystem, e.g.
/// a Git PR for a `closes` edge), `rel`, `rel_class = lifecycle`. The aggregate is the shared
/// `edge:<source>-><target>` so the relate → unrelate sequence is per-aggregate ordered. ONE event
/// yields BOTH projection directions (the Refs mirror fixes the inverse — contract 5.5).
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

/// **Emit `issue.relation.created` / `.removed` for an `issue_relation` typed-row write/delete, IN THE
/// SAME TRANSACTION as the typed-row write (contract 5.5 — the TE-7 mirror; the typed table is TRUTH).**
///
/// Call this from the SAME `tx` that wrote/deleted the `issue_relation` row (the typed table is the
/// source of truth; this emits the event Refs projects). `created` is `true` for a relate, `false` for
/// an unrelate. `cause` threads the causal triple when this rides a larger transaction. Returns the
/// minted `event_id`. The FORWARD edge is emitted only; the Refs mirror projects the inverse swap (one
/// event → both directions).
///
/// **0 typed-row-without-edge:** because the emit shares `tx` with the typed-row write, the
/// `issue_relation` row and its mirror event co-commit (emit-iff-committed) — an aborted relation write
/// drops the buffered event with it; a committed write always carries its mirror edge.
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

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 4. THE BOUNDED CYCLE-SAFE TRAVERSE over issue_relation (contract 5.3, §4)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// One forward `issue_relation` edge for the traverse (the `src_issue --rel--> dst_ref` typed row — the
/// TE-7 source of truth). The graph is the in-memory model of the `issue_relation` table the live walk
/// reads via the `issue_rel_src` index ([`crate::migrations`]); the bounded-cycle-safe shape is
/// store-agnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelationEdge {
    /// The source issue URN (the `src_issue` row's canonical ref).
    pub source: ArtifactRef,
    /// The target URN (the `dst_ref` — may be cross-subsystem for a `closes` edge).
    pub target: ArtifactRef,
    /// The lifecycle relation.
    pub rel: IssueLifecycleRel,
}

/// One node reached by the traverse, with the BFS depth it was first reached at (depth 0 = the root).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraversedNode {
    /// The reached artifact URN.
    pub node: ArtifactRef,
    /// The depth (hops from the root) the node was first reached at (≤ [`TRAVERSE_MAX_DEPTH`]).
    pub depth: usize,
}

/// **The in-memory `issue_relation` forward-edge graph (the TE-7 source-of-truth model the
/// traverse walks).** Keyed by the source URN → its forward edges (the `issue_rel_src` index). The live
/// walk reads the table; this is the store-agnostic bounded-cycle-safe traversal the impact/hierarchy
/// surface drives.
#[derive(Clone, Debug, Default)]
pub struct IssueRelationGraph {
    forward: HashMap<String, Vec<RelationEdge>>,
}

impl IssueRelationGraph {
    /// A fresh empty graph.
    pub fn new() -> IssueRelationGraph {
        IssueRelationGraph::default()
    }

    /// Add one forward edge (the projection of an `issue.relation.created` mirror event / an
    /// `issue_relation` row).
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

    /// **The bounded cycle-safe traverse (contract 5.3 — depth 16).** A BFS from `root` over the
    /// forward edges, optionally restricted to a single `rel` (e.g. `blocked_by` for the
    /// unblock-trigger walk). CYCLE-SAFE: a visited-set prunes a revisit, so a `blocked_by` cycle
    /// (A blocks B blocks A) terminates rather than looping forever. DEPTH-BOUNDED: a node deeper than
    /// [`TRAVERSE_MAX_DEPTH`] is NOT expanded (the walk stops at the bound — a malformed deep chain
    /// never blows the stack or hangs). The root itself is NOT returned (the walk yields the reachable
    /// set, not the seed). Deterministic order (insertion-order BFS).
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
            // Do not expand past the bound (depth 16 — contract 5.3). A node AT the bound is reported
            // but its children are not enqueued.
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

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 5. THE PROJECTION SHAPE + THE 4-STEP TOMBSTONE LADDER (contracts 5.6 / 5.7 / §3)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// A **per-viewer projection** of an Issues artifact (contract 5.6, §3 — the frozen
/// `{title, state, category, icon, render_hint, sub_anchor?}` shape). Built ONLY after the per-viewer
/// permission check passes; a denied/erased/restricted viewer gets a [`Tombstone`] instead, never this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projection {
    /// The issue title (free-text — erasure-safe; tombstones if erased). NEVER rendered for an
    /// unauthorized viewer (the 0-leak invariant — the deny path returns a tombstone that never reads
    /// this field).
    pub title: String,
    /// The NAMED state (humanised — `In Progress`/`Done`/…).
    pub state: String,
    /// The FIXED state category (the cross-sub "is it done?" — `unstarted`/`started`/`completed`/
    /// `cancelled`, [`crate::workflow::StateCategory`]).
    pub category: String,
    /// The icon token the UI renders (the type icon — `issue`/`epic`/`bug`/…).
    pub icon: String,
    /// The render hint (§3 `render_hint`) — always `issue` (Refs picks the chip/embed render).
    pub render_hint: String,
    /// The sub-anchor projection — set when the projected ref carried a `#sub`
    /// (`comment-`/`b`/`field-`/`row-`), carrying the ladder rung (`live`/`moved`/`outdated`). `None`
    /// for a bare-root projection. A `GONE`/`ERASED` sub never reaches here (it is a tombstone).
    pub sub_anchor: Option<SubAnchor>,
}

/// A projected sub-anchor (§3 `sub_anchor`) — the `#sub` kind label + the stable opaque sub-id + the
/// ladder rung the resolver landed on. A `MOVED` block still resolves (the `block_id` is stable across
/// edits); an `OUTDATED` block resolves partially. `GONE`/`ERASED` are tombstones, never a `SubAnchor`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubAnchor {
    /// The `#sub` kind label (`comment-`/`b`/`field-`/`row-`).
    pub kind: String,
    /// The stable opaque sub-id (the `comment_id`/`block_id`/`field_id`/`row_id`) the anchor resolved
    /// to.
    pub sub_id: String,
    /// The ladder rung the sub-anchor resolved on (`live`/`moved`/`outdated`).
    pub rung: LadderRung,
}

/// The 4-step tombstone ladder's sub-resolution RUNG for a still-resolvable sub-anchor (§3 / X-4).
/// `GONE` and `ERASED` are NOT rungs here — they degrade to a [`Tombstone`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LadderRung {
    /// `LIVE` — the comment/block/field/row resolves exactly (the unfurl/embed).
    Live,
    /// `MOVED` — the sub moved but the stable opaque id still resolves (an edit, not a delete).
    Moved,
    /// `OUTDATED` — an edited sub whose anchored content shifted; the projection is partial.
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

/// A **tombstone** — the projection of an issue the viewer may NOT see, or whose sub-anchor is
/// gone/erased (contract 5.6, §3 / X-4). It carries NO title/content (the 0-leak invariant) — only the
/// permission-checked ROOT, so an embed degrades to "this referenced ENG-1421 (the specific part is no
/// longer available)" rather than vanishing. A tombstone ALWAYS carries the root (§3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tombstone {
    /// Why the projection is a tombstone — for the AUDIT log, NEVER rendered to the viewer.
    pub reason: TombstoneReason,
    /// **The `#sub`-stripped ROOT URN** (§3 — a tombstone always carries the root). For a `Denied`
    /// tombstone this is the stripped root URN ONLY (an opaque scope, never a title) so a backlink can
    /// render "(not available)" while still pointing at the parent issue; for `RootGone`/`SubGone`/
    /// `Erased` it is the root the embed degrades to. NEVER a title (the URN structure is not the
    /// content).
    pub root: ArtifactRef,
}

/// Why a projection degraded to a [`Tombstone`] (§3 / X-4 — the 4-step ladder reasons; the audit
/// reason, never leaked to the viewer).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TombstoneReason {
    /// Step 1 — `check(viewer, view, root)` denied (a confidential issue the viewer can't read). The
    /// deny path NEVER reads the issue's title (0 leak). The frozen ladder's first rung.
    Denied,
    /// Step 2 — the root issue does not exist (a dangling reference). The embed degrades to the (gone)
    /// root URN.
    RootGone,
    /// Step 3 `GONE` — the root resolves, but the sub-anchor (comment/block/field/row) is dead. The
    /// embed shows the parent issue ("the issue resolves; the specific part is no longer available").
    SubGone,
    /// Step 4 `ERASED` (any level) — pseudonym-/crypto-shred made the content unrenderable (the
    /// per-subject DEK destroyed / the `issue.*.erased` tombstone). Restriction degrades likewise.
    Erased,
}

impl Tombstone {
    /// The generic, content-free text the VIEWER sees (never the title/state/reason). The same string
    /// regardless of reason — a denied viewer cannot distinguish "denied" from "erased".
    pub fn display_text(&self) -> &'static str {
        "(not available)"
    }
}

/// The result of [`Projector::project`]: a per-viewer [`Projection`] (authorised + present +
/// live/moved/outdated sub) or a [`Tombstone`] (denied / root-gone / sub-gone / erased). The
/// two-variant shape IS the §3 contract (`Projection | Tombstone`) — a projector NEVER returns a bare
/// title to a denied viewer.
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

/// A loud, typed projection error (a malformed / non-Issues ref) — distinct from a [`Tombstone`] (which
/// is a SUCCESSFUL projection of a hidden/gone issue). An error means the ref is not an Issues artifact
/// at all; a tombstone means it is, but is hidden/gone for this viewer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectError {
    /// The ref is not an Issues artifact (wrong subsystem / malformed scope).
    NotAnIssueArtifact {
        /// The offending reference string.
        reference: String,
    },
    /// The `<type>` token is not an Issues type the projector owns.
    UnknownIssueType {
        /// The rejected type token.
        ty: String,
    },
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectError::NotAnIssueArtifact { reference } => write!(
                f,
                "not an Issues artifact: `{reference}` — Issues' projector does not own this ref"
            ),
            ProjectError::UnknownIssueType { ty } => {
                write!(f, "unknown Issues artifact type `{ty}`")
            }
        }
    }
}

impl std::error::Error for ProjectError {}

/// The frozen Issues artifact types `project` projects (the `<type>` token of the canonical
/// `ArtifactRef`, §2). A closed set — Issues is the resolver-owner of exactly these (the seed +
/// the registered `initiative`).
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

/// Classify a parsed Issues `ArtifactRef` to its [`IssueArtifactType`] (reading the
/// `<subsystem>`/`<type>` scope segments — never a render-time display form).
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

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 6. THE ISSUE PROJECTION STORE (the ISS-P05/P-13 live-store floor — in-memory here)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// An issue's projectable metadata (the §3 projection input + the Search-projection facets). The live
/// `issue` OLTP row hydrates these; the projector reads only the title/state/category/type-icon + the
/// structured facets — never the encrypted free-text body bytes (the DEK-sealed content is out of the
/// projection — it is decrypted at render, not at project-time for a third party).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueMeta {
    /// The issue title — the projection title (a confidential issue's title NEVER leaks; it is read
    /// only AFTER the permission check passes).
    pub title: String,
    /// The NAMED state (`In Progress`/`Done`/…).
    pub state: String,
    /// The FIXED state category (`unstarted`/`started`/`completed`/`cancelled`).
    pub state_category: String,
    /// The type icon token (`issue`/`epic`/`bug`/…).
    pub icon: String,
    /// The pseudonymous assignee (the `assignee` facet — equality-only, NEVER the real id, EI-04 §1).
    /// `None` if unassigned.
    pub assignee: Option<String>,
    /// The numeric priority (the ordered `priority` facet).
    pub priority: i64,
    /// The denormalised type rank (the board↔roadmap partitioning facet).
    pub type_rank: i64,
    /// The parent project URN (the `project_id` relation facet — the per-project board filter).
    pub project_id: String,
}

/// The resolved state of a `#sub` sub-anchor against the live store (the ladder's step-3 input). The
/// store reports which rung a sub-id is on; the projector maps it to a [`SubAnchor`] (live/moved/
/// outdated) or a [`Tombstone`] (gone).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubState {
    /// The comment/block/field/row resolves exactly.
    Live,
    /// The sub moved but the stable opaque id still resolves.
    Moved,
    /// An edited sub whose anchored content shifted; resolves partially.
    Outdated,
    /// The sub-artifact is dead (the root resolves, the sub does not) → a `SubGone` tombstone.
    Gone,
}

/// The in-memory **issue projection store** the projector reads (the ISS-P05/P-13 live-OLTP-store
/// FLOOR). Keyed by the canonical ROOT `ArtifactRef` string — the SAME entity shapes the live store
/// hydrates, so the projection logic is store-agnostic. Carries the erased/restricted flags the §3
/// erasure-/restriction-safe tombstone reads, and the per-sub-URN sub-anchor state for the ladder's
/// step 3.
#[derive(Clone, Debug, Default)]
pub struct IssueProjectionStore {
    /// Issue metadata by canonical ROOT ref string.
    roots: HashMap<String, IssueMeta>,
    /// The sub-anchor resolved state by FULL sub-URN string (the ladder's step-3 input).
    subs: HashMap<String, SubState>,
    /// The set of canonical ref strings (root OR sub-URN) that have been ERASED (an `issue.*.erased`
    /// tombstone) — projecting one returns an `Erased` tombstone, never the (shredded) content.
    erased: HashSet<String>,
    /// The set of canonical ref strings whose subject is RESTRICTED (the GDPR `restrict` flag) —
    /// projecting one returns a tombstone (and the index excludes it, §3 restriction-safe).
    restricted: HashSet<String>,
}

impl IssueProjectionStore {
    /// A fresh empty store.
    pub fn new() -> IssueProjectionStore {
        IssueProjectionStore::default()
    }

    /// Insert an issue's projectable metadata keyed by its canonical ROOT ref.
    pub fn put_issue(&mut self, root: &ArtifactRef, meta: IssueMeta) {
        self.roots.insert(root.0.clone(), meta);
    }

    /// Set a sub-anchor's resolved state keyed by its FULL sub-URN (the ladder's step-3 input).
    pub fn put_sub_state(&mut self, sub_ref: &ArtifactRef, state: SubState) {
        self.subs.insert(sub_ref.0.clone(), state);
    }

    /// Mark a canonical ref (root or sub-URN) ERASED (an `issue.*.erased` tombstone, §3 step 4).
    pub fn mark_erased(&mut self, reference: &ArtifactRef) {
        self.erased.insert(reference.0.clone());
    }

    /// Mark a canonical ref's subject RESTRICTED (the GDPR `restrict` flag).
    pub fn mark_restricted(&mut self, reference: &ArtifactRef) {
        self.restricted.insert(reference.0.clone());
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 7. THE PROJECTOR — project(ref, viewer): the frozen 4-step tombstone ladder, permission FIRST
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The Issues `project(ref, viewer)` projector (contract 5.6 — the ISS-P17 deliverable, OWNED).** The
/// ONLY way Refs/Search/Notif read an Issues artifact (no cross-DB). Backs the PR context pane, the chat
/// unfurl, the knowledge embed, the inbox humanisation, the search snippet. Holds the
/// [`IdentityService`] (the per-viewer permission source) + the [`IssueProjectionStore`] (the own-DB
/// read — the ISS-P05/P-13 store floor in-memory here). Generic over `I: IdentityService` so the service
/// wires the real Id resolver and tests wire a deterministic one.
pub struct Projector<I: IdentityService> {
    id: I,
    store: IssueProjectionStore,
}

impl<I: IdentityService> Projector<I> {
    /// Compose the projector over the Id dependency + the issue projection store.
    pub fn new(id: I, store: IssueProjectionStore) -> Projector<I> {
        Projector { id, store }
    }

    /// A borrow of the underlying store (for the service / drills to seed or inspect).
    pub fn store_mut(&mut self) -> &mut IssueProjectionStore {
        &mut self.store
    }

    /// **`project(ref, viewer) -> Projection | Tombstone` (contract 5.6 / 5.7, the frozen 4-step
    /// tombstone ladder §3 / X-4).**
    ///
    /// The ladder is the load-bearing invariant (the project-leak gate — the ISS-D3 slice):
    /// 1. **permission FIRST** — `check(viewer, view, root)` on the `#sub`-stripped ROOT (a sub is never
    ///    more visible than its parent). Deny / Conditional / Id-error ALL fail CLOSED to a `Denied`
    ///    tombstone carrying the root — NO field of the issue read (0 leak; the confidential issue
    ///    returns a tombstone carrying the root, NEVER the title).
    /// 2. **ERASED/restricted (any level)** — the root or the sub is erased/restricted ⇒ an `Erased`
    ///    tombstone. (Checked before the sub-resolve so a shredded sub never renders.)
    /// 3. **root resolve** — the issue exists? No ⇒ a `RootGone` tombstone carrying the root.
    /// 4. **sub resolve** — for a `#sub` ref, the store's [`SubState`]: `Live`/`Moved`/`Outdated` project
    ///    a [`SubAnchor`] on that rung; `Gone` ⇒ a `SubGone` tombstone carrying the root (the issue
    ///    resolves; the embed shows the parent).
    ///
    /// A tombstone ALWAYS carries the `#sub`-stripped root (§3). The `zookie` is the read-consistency
    /// fence (a strong, zookie-stamped read for a security-sensitive projection).
    pub fn project(
        &self,
        reference: &ArtifactRef,
        viewer: &Principal,
        zookie: Zookie,
    ) -> Result<Projected, ProjectError> {
        // Classify FIRST — a non-Issues / unknown-type ref is a loud error (not a tombstone).
        let ty = classify(reference)?;
        let root = myelin_refs::strip_sub(reference);

        // ── STEP 1: PERMISSION FIRST (the 0-leak gate, root-keyed). A Deny / Conditional / Id-error
        //    ALL fail closed to a `Denied` tombstone carrying the ROOT, with NO issue field read.
        let at = Consistency {
            at_least: zookie,
            mode: ConsistencyMode::Strong,
        };
        let permission = Permission(VIEW.to_string());
        match self.id.check(viewer, &permission, &root, &at, None) {
            Ok(Decision::Allow) => { /* authorised — descend the ladder */ }
            Ok(Decision::Deny) | Ok(Decision::Conditional) | Err(_) => {
                return Ok(Projected::Tombstoned(Tombstone {
                    reason: TombstoneReason::Denied,
                    root,
                }));
            }
        }

        // ── STEP 2 (erasure/restriction, checked early so a shredded issue never renders, §3 step 4).
        //    Keyed on the ROOT and the full ref — an erased issue tombstones its sub-anchors too.
        //    Restriction (the GDPR suppression window) degrades to the SAME content-free tombstone.
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

        // ── STEP 3: ROOT RESOLVE — the issue exists? No ⇒ a `RootGone` tombstone (the root URN is
        //    opaque scope, never a title).
        let meta = match self.store.roots.get(&root.0) {
            Some(m) => m.clone(),
            None => {
                return Ok(Projected::Tombstoned(Tombstone {
                    reason: TombstoneReason::RootGone,
                    root,
                }));
            }
        };

        // ── STEP 4: SUB RESOLVE (for a `#sub` ref) — map the store's SubState to a rung or a SubGone
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

        // ── BUILD THE PER-VIEWER PROJECTION (§3 — only now read the title; the deny path never did).
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

/// Map a [`SubState`] to its still-resolvable [`LadderRung`], or `None` for `GONE` (a tombstone).
fn sub_state_to_rung(state: SubState) -> Option<LadderRung> {
    match state {
        SubState::Live => Some(LadderRung::Live),
        SubState::Moved => Some(LadderRung::Moved),
        SubState::Outdated => Some(LadderRung::Outdated),
        SubState::Gone => None,
    }
}

/// The opaque sub-id body of a parsed [`myelin_refs::Sub`] (the `comment_id`/`block_id`/`field_id`/
/// `row_id`). For the kinds Issues resolves; a non-Issues sub renders its body verbatim.
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

/// The frozen `render_hint` token for an Issues artifact type (§3 — always `issue`-family; Refs picks
/// the chip render). The PROJECTION `render_hint` is `issue` per §3; the per-type icon is on the meta.
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

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 8. THE issue.* SEARCH PROJECTION EMITTER (contract 6.3 — the LIVE emitter; reindex 6.4)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **The Issues `ProjectFetcher` — the LIVE `issue.*` Search projection emitter (contract 6.3, the
/// ISS-P17 deliverable that fills the [`crate::declares`] floor).** The incremental indexer fetches a
/// doc's body through THIS — the SAME [`Projector::project`] cross-DB read (no second projection path,
/// no owner-DB read by Search). The returned [`SearchProjection`] carries the full-text `title` body +
/// the typed facets the [`crate::declares::issue_facets_projection_spec`] declared. ACL-filtered +
/// restriction-/erasure-safe: a restricted/erased issue projects to [`ProjectFetchError::Gone`] (the
/// index removes the doc — no leak via a search result/count/rank), exactly mirroring the project-time
/// tombstone.
///
/// Search passes a viewer-NEUTRAL system identity at index time (index-time fetch is the OBJECT's
/// content, not a viewer's redacted view — Identity computes the per-viewer reachable set at query time
/// via the ISS-P13 push-down + the `acl_object_type = "issue"` anchor). So the indexer never tombstones
/// on a per-viewer denial here; it skips a RESTRICTED/ERASED doc (the index-time twin of the tombstone).
pub struct IssueProjectFetcher {
    store: IssueProjectionStore,
}

impl IssueProjectFetcher {
    /// Compose the fetcher over the issue projection store (the same store the [`Projector`] reads).
    pub fn new(store: IssueProjectionStore) -> IssueProjectFetcher {
        IssueProjectFetcher { store }
    }

    /// A borrow of the underlying store (for the service / drills to seed).
    pub fn store_mut(&mut self) -> &mut IssueProjectionStore {
        &mut self.store
    }

    /// Build the `issue.*` [`SearchProjection`] for an issue ROOT (the title full-text body + the typed
    /// facets the 6.3 spec declared). Returns [`ProjectFetchError::Gone`] for a restricted/erased/
    /// missing issue (the index removes the doc — restriction-/erasure-safe). The facet KEYS are
    /// byte-identical to [`crate::declares`]'s `FACET_*` constants (the spec is the schema, this is the
    /// row).
    fn build(&self, reference: &ArtifactRef) -> Result<SearchProjection, ProjectFetchError> {
        let root = myelin_refs::strip_sub(reference);
        // Restriction-/erasure-safe (the index never carries a restricted/erased issue — the §3
        // restriction-safe property, the index-time twin of the project tombstone).
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
            // The full-text body is the title (the 6.3 spec's facets are the columnar half; the body
            // arrives here — the props/comment free-text join is the projection-feeder follow-on).
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

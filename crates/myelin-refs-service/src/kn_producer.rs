//! # `kn_producer` — Refs consumes the REAL Knowledge producer edges + block/heading/row sub-anchors
//! + the FIRST real lifecycle mirror (`page_parent`) (REF-P18 / P-259, M3).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/reference-graph.md`
//! - §4.6 (the ONE 4-step ladder on REAL Knowledge sub-anchors — a tombstone ALWAYS carries the root;
//!   a stable block resolves LIVE, an edited block OUTDATED, a deleted block GONE),
//! - §3.3 (the TE-7 typed-edge mirror — **KN `page_parent` is the FIRST real lifecycle mirror**: the
//!   typed re-parent table is the source of truth, Refs holds the rebuildable `parent`/`child`
//!   inverse-paired projection, and **the typed table always wins on drift**),
//! - §4.7 (sub-artifact-granular replay — a scoped KN reindex re-emits the page-subtree at BLOCK
//!   granularity so the block anchors RE-DERIVE, never a stale positional index).
//!
//! **Contracts implemented (consumer side, to the frozen shapes; the engine is UNCHANGED):**
//! - **5.4** consume the REAL Knowledge producer reference edges — KN block/heading/row embeds emit
//!   `refs.edge.created` via the THREE structured content nodes ([`myelin_content::InlineNode`]),
//!   extracted through the SAME [`crate::extract_edges`] seam the synthetic M2 producer used. The KN
//!   producer is no longer synthetic — it is a KN page/block body modelled as a `myelin-content`
//!   document whose structured ref nodes are the edges.
//! - **5.6** consume Knowledge's `project(ref, viewer)` sub-anchor resolver — [`KnOwner`] is a REAL
//!   [`crate::ProjectApi`] over KN content; its `project` classifies a KN `#sub` (a `b<id>` block, an
//!   `h<id>` heading, a `row-<id>` row, a `field-<id>` field) into the frozen
//!   [`crate::ProjectOutcome`] the chokepoint maps onto a [`crate::Resolution`].
//! - **5.7** the `#sub` kinds on REAL KN sub-anchors — Refs resolves KN's `b`/`h`/`row-`/`field-`
//!   mints through the ONE [`crate::ladder`]: stable → LIVE, edited → OUTDATED, moved → MOVED,
//!   deleted → GONE, erased → ERASED. The SAME vocabulary a Git line-range / a Chat message degrades
//!   through (one ladder).
//! - **5.5** project KN's `page_parent` typed-lifecycle events as `rel_class='lifecycle'` edges with
//!   the REF-P14 inverse pairing (`parent↔child`) — **the FIRST time the TE-7 mirror discipline runs
//!   over a REAL typed table** ([`mirror_page_parent`]). An out-of-band re-parent + a scoped reindex
//!   reconverges Refs to the typed table (the typed table wins, [`reconverge_page_tree`]).
//! - **2.6** drive KN page-subtree replay at BLOCK granularity — [`kn_replay_scope`] selects the KN
//!   sub-artifact grain (`page:` / `block:` / `subtree:`) KN's `replay(scope, since)` re-emits, so a
//!   scoped reindex re-derives the block anchors at block grain (never a stale positional index).
//!
//! ## Why this is a CONSUMER module, not a new engine (EI-01 §7 coherence — the engine is UNCHANGED)
//! REF-P18's deliverable is to WIRE Refs to the real KN producer + the FIRST real lifecycle mirror —
//! NOT to build a second resolver/ladder/mirror-engine. So this module:
//! - reuses [`crate::extract_edges`] / [`crate::edge_builder`] for ingest (the §4.1 producer #1 seam),
//! - reuses [`crate::resolve::ResolveService`] + [`crate::ladder`] for resolution (the ONE chokepoint),
//! - reuses [`crate::mirror`] (`mirror_edges`/`reconverge`/[`crate::LifecycleRel`]) for the page_parent
//!   typed mirror — the SAME REF-P14 discipline + inverse pairing, now over a REAL typed table,
//! - reuses [`crate::reindex`] for replay (the ONE recovery path).
//!
//! It adds ONLY the KN-specific glue: the KN source-URN construction for edges ([`KnEdgeProducer`]),
//! the KN `ProjectApi`/sub-anchor resolution body ([`KnOwner`]) the engine calls, and the
//! `page_parent` → [`crate::LifecycleRel::Parent`] event mapping ([`mirror_page_parent`]). No Refs type
//! is re-defined; no parallel second ladder/mirror is minted — the page_parent mirror is the FIRST
//! caller of the REF-P14 `mirror_edges`/`reconverge` over a real typed table (replacing, for KN
//! `page_parent`, the [`crate::SyntheticTypedEvent`] M2 stand-in).
//!
//! ## Floors named (VISION §3 / EI-01 §1 — the prompt's named floors)
//! - **In-cell single-home-cell graph build.** Cross-cell fan-out is **R-M5 (REF-P26)** — this wiring
//!   builds + resolves the KN graph in the artifact's home cell; the C-5 cross-cell semantics are
//!   already frozen in [`crate::resolve`]. Named here because the KN corpus is single-cell at M3.
//! - **The second real mirror (Issues `issue_relation`) + the CI check seam + Chat unfurls are R-M4**
//!   (REF-P20 / REF-P19 / REF-P21). `page_parent` is the FIRST real mirror; the SyntheticTypedEvent
//!   stand-in remains the floor for the OTHER lifecycle rels (`closes`/`blocks`/`depends_on`/…) until
//!   their owning subsystems ship the typed tables. Named so the page_parent mirror is not mistaken for
//!   the full TE-7 mirror over every subsystem.

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

/// The canonical Knowledge subsystem token (the §6.1 grammar prefix Knowledge owns; re-exported from
/// the durable taxonomy so a KN-edge drill asserts against the ONE token, never a literal). Derived
/// from the frozen [`KNOWLEDGE_PAGE_CREATED`] token's subsystem segment.
pub const KN_OWNER_TOKEN: &str = "knowledge";

/// **The Knowledge reference-edge producer (contract 5.4 EMIT side — the REAL KN producer, no longer
/// synthetic).** A KN page / block body IS a `myelin-content` document: its structured inline nodes
/// ([`InlineNode::ArtifactRefNode`] for an inline page/issue link, [`InlineNode::Mention`] for an
/// `@`-mention, [`InlineNode::Embed`] for an embedded page/block) are the edges. This producer
/// constructs the KN SOURCE URN (the page/block root) and drives the SAME [`emit_edges`] seam the M2
/// synthetic writer used — so the WIRING is unchanged; only the CALLER is now the real KN surface.
///
/// References-not-payloads: the source/target are opaque KN/Issue/Identity URNs; an `@`-mention's
/// target is the PSEUDONYMOUS `member` URN (erasure-safe, §4.6). No page prose is held — only the
/// structured ref nodes are read (the reliability guarantee, EI-04 §2.4: structured-node extraction,
/// never a regex over the page body).
pub struct KnEdgeProducer;

impl KnEdgeProducer {
    /// The canonical Knowledge **page root** `myelin://<tenant>/knowledge/page/<id>` — the source of a
    /// page-body reference edge (an inline link / embed in a page).
    pub fn page_root(tenant: &str, page_id: &str) -> ArtifactRef {
        ArtifactRef(format!("myelin://{tenant}/knowledge/page/{page_id}"))
    }

    /// The canonical Knowledge **block root** `myelin://<tenant>/knowledge/block/<id>` — the source of
    /// a block-body reference edge (an inline link / embed anchored within a specific block).
    pub fn block_root(tenant: &str, block_id: &str) -> ArtifactRef {
        ArtifactRef(format!("myelin://{tenant}/knowledge/block/{block_id}"))
    }

    /// **Emit the reference edges of a real KN page/block body, in the SAME outbox transaction as the
    /// KN content write (contract 5.4 — emit-iff-committed, REF-D7 producer half).** `source` is the KN
    /// page/block root; `body` is the structured `myelin-content` document; `content_event` is the
    /// `knowledge.page.*` / `knowledge.block.*` event being emitted in the SAME transaction (the CAUSE —
    /// the correlation root carries, `depth +1`, the loop-guard stamp). One `refs.edge.created` per
    /// structured ref node. Returns the minted ids.
    ///
    /// The edges become durable IFF the caller commits the KN content transaction — an aborted save
    /// drops the buffered edge rows with it (no edge without its KN content). This is the same guarantee
    /// the synthetic M2 producer proved; the only change is the real KN caller + source URN.
    pub fn emit_kn_edges(
        &self,
        tx: &mut dyn OutboxTx,
        source: &ArtifactRef,
        body: &[InlineNode],
        content_event: &EventEnvelope,
    ) -> BusResult<Vec<EventId>> {
        // The ONE sanctioned producer seam (§4.1 producer #1; no standalone edge-write API). Refs
        // extracts one edge per structured node and emits via OutboxTx::emit — unchanged from REF-P8.
        emit_edges(tx, source, body, content_event)
    }

    /// The extracted (un-emitted) KN reference edges of a body — exposed so a drill can assert the edge
    /// SET a real KN page/block body produces (the leak/IDOR re-confirmation corpus, REF-D1/D2) without
    /// driving the outbox. Reuses the ONE [`crate::extract_edges`] seam.
    pub fn kn_edges(&self, source: &ArtifactRef, body: &[InlineNode]) -> Vec<EdgeDraft> {
        crate::extract_edges(source, body)
    }
}

/// The state of a Knowledge block / heading / row / field sub-anchor (§4.6). The owner ([`KnOwner`])
/// records these; the resolver maps them onto the frozen [`SubState`] so a real KN sub-anchor degrades
/// through the SAME ladder as a Git line-range / a Chat message. A KN block id is a **stable opaque
/// id** (the stability obligation is Knowledge's, §3.5) — so an EDITED block keeps its id and resolves
/// OUTDATED (not GONE); only a DELETED block is GONE.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KnAnchorState {
    /// The block/heading/row/field resolves exactly — its content is unchanged (LIVE).
    Live,
    /// The block MOVED within the page (re-ordered / re-parented in the doc tree) but the stable opaque
    /// id survives — render the moved anchor, flagged `moved` (MOVED). The id is immutable, so a moved
    /// block is MOVED, not GONE.
    Moved,
    /// The block was EDITED (its content changed) but the stable id survives — render the anchor,
    /// flagged `outdated` (OUTDATED). The embed shows the current block content with the staleness flag.
    Edited,
    /// The block/row was DELETED — the root page still resolves, the sub anchor is gone (GONE ⇒
    /// `Tombstone{ sub_gone, root }`; the embed shows the parent page).
    Deleted,
    /// The block/page was ERASED (crypto-shred of the page's per-subject DEK, contract 2.7) —
    /// unrenderable (ERASED ⇒ `Tombstone{ erased }`).
    Erased,
}

impl KnAnchorState {
    /// Map a KN block/heading/row/field state onto the frozen §4.6 [`SubState`], carrying the owner
    /// projection on the renderable arms. The ONE vocabulary — a KN sub-anchor degrades identically to a
    /// Git line-range / a Chat message / a CI check.
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

/// **The REAL Knowledge owner — a [`ProjectApi`] + [`SubAnchorResolver`] over KN content (contracts
/// 5.6 / 5.7).** This is the producer half Refs' chokepoint calls: `check_view` is Knowledge's
/// authoritative permission verdict (4.2, reached over the resilient client in production — here the
/// recorded ACL — the KN ReBAC fragment's page-tree override, REF-P249), and `project` classifies a KN
/// `#sub` into the frozen [`ProjectOutcome`]:
///
/// - a `b<id>` block / `h<id>` heading / `row-<id>` row / `field-<id>` field → the recorded
///   [`KnAnchorState`] (stable→LIVE, moved→MOVED, edited→OUTDATED, deleted→GONE, erased→ERASED),
/// - a bare root (no `#sub`) → LIVE (the page/block itself).
///
/// Refs NEVER reads Knowledge's DB — it only calls this seam. The leak invariant is the chokepoint's:
/// this owner is reached ONLY on the permission-allowed branch (the chokepoint gates it).
///
/// Cloneable: every map is held behind an [`Arc`] so a clone shares the SAME recorded state (the
/// resolve chokepoint holds the owner behind an `Arc<dyn ProjectApi>`; a clone lets the test record
/// into the same owner the service resolves through). Tenant-first; no cross-tenant key.
#[derive(Clone, Default)]
pub struct KnOwner {
    /// `(tenant|region|principal|root)` → the authoritative `view` decision (4.2). The recorded ACL the
    /// production wire replaces with Identity's `check` over the resilient client (the KN page-tree
    /// override + row + field caveat, REF-P249). Default-deny.
    acl: Arc<Mutex<BTreeMap<String, Decision>>>,
    /// `full-ref-urn` → the KN block/heading/row/field anchor state (the §4.6 sub-anchor state).
    anchors: Arc<Mutex<BTreeMap<String, KnAnchorState>>>,
}

impl KnOwner {
    /// A fresh KN owner (default-deny ACL; no anchors recorded — an unscripted bare root is LIVE).
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

    /// Grant a viewer the `view` permission on a KN root (the recorded ACL — the KN ReBAC fragment's
    /// page-tree `view` grant, modelled here; production is Identity's `check` over the page-tree
    /// override + row + field caveat, REF-P249).
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

    /// Record a KN block/heading/row/field anchor state (`b`/`h`/`row-`/`field-`) — the §4.6 sub-anchor
    /// state. An edit that keeps the stable id records [`KnAnchorState::Edited`]; a delete records
    /// [`KnAnchorState::Deleted`].
    pub fn record_anchor(&self, ref_: &ArtifactRef, state: KnAnchorState) {
        self.anchors.lock().unwrap().insert(ref_.0.clone(), state);
    }

    /// The default owner projection a renderable KN anchor carries (a render-safe title — the leak
    /// invariant already gates this; the owner is reached only on the allowed branch). PII-free.
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

    /// Resolve a KN `#sub` anchor on `ref_` into the frozen [`SubState`] (the §4.6 step-3 owner resolver
    /// — the SUB-ANCHOR RESOLUTION REF-P18 ships). Dispatched by the KN `#sub` KIND through the ONE Refs
    /// grammar (`sub_kind`):
    /// - `b<id>` block / `h<id>` heading / `row-<id>` row / `field-<id>` field → the recorded
    ///   [`KnAnchorState`] (stable→LIVE, moved→MOVED, edited→OUTDATED, deleted→GONE),
    /// - bare root → LIVE.
    fn resolve_kn_sub(&self, ref_: &ArtifactRef, sub: Option<&Sub>) -> SubState {
        let projection = Self::projection(ref_);
        match sub {
            // A bare root (no #sub) is LIVE — the page/block itself, no sub-anchor to degrade.
            None => SubState::Live(projection),
            // The KN sub-anchor kinds — the recorded §4.6 state. A block/heading/row/field id is a
            // STABLE opaque id, so an edited block is OUTDATED (id survives), a deleted block is GONE.
            Some(Sub::Block(_))
            | Some(Sub::Heading(_))
            | Some(Sub::Row(_))
            | Some(Sub::Field(_)) => {
                self.anchors
                    .lock()
                    .unwrap()
                    .get(&ref_.0)
                    .copied()
                    // No recorded state for a sub Refs minted → defensively GONE (a real owner always
                    // has the mint-time state for a ref it minted; an unscripted anchor is treated as
                    // gone rather than guessed LIVE — REF-3, never resolve to content it cannot back).
                    .map(|s| s.into_sub_state(projection.clone()))
                    .unwrap_or(SubState::Gone)
            }
            // Any other kind on a KN ref is not a KN-owned mint — KN renders the bare root LIVE rather
            // than guess (REF-3 — never guess scope; the grammar already rejected an unknown kind).
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
        // The authoritative `view` verdict on the ROOT (the chokepoint passes the #sub-stripped root).
        // Default-deny: an unrecorded grant is a Deny (so a viewer with no page-tree `view` is
        // tombstoned, never leaked — the REF-D1 leak invariant on the KN corpus).
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
        // The owner classifies the KN #sub into the frozen ProjectOutcome through the ONE ladder
        // mapping (SubState::into_outcome). Called ONLY on the permission-allowed branch.
        let sub = sub_kind(ref_);
        Ok(self.resolve_kn_sub(ref_, sub.as_ref()).into_outcome())
    }
}

impl SubAnchorResolver for KnOwner {
    /// The §4.6 step-3 sub-anchor resolver (the SAME logic `project` runs) — exposed so a drill can
    /// drive the ladder directly (REF-D9) through [`crate::resolve_sub_outcome`] without the full
    /// chokepoint. ONE source of truth: it delegates to [`KnOwner::resolve_kn_sub`].
    fn resolve_sub(&self, ref_: &ArtifactRef, sub: Option<&Sub>) -> SubState {
        self.resolve_kn_sub(ref_, sub)
    }
}

// =================================================================================================
// 5.5 — the FIRST real lifecycle mirror: KN `page_parent` (the TE-7 mirror over a REAL typed table)
// =================================================================================================

/// **A KN `page_parent` typed-lifecycle event — the FIRST real TE-7 mirror input (§3.3 / contract
/// 5.5).** A KN page re-parent (a `knowledge.page.created` under a parent, or a `knowledge.page.moved`
/// re-parent) writes the typed `page_parent` row + emits this typed lifecycle event in the SAME
/// transaction (producer #2). It carries the `(parent, child)` page roots + provenance. PII-free:
/// `parent`/`child` are opaque KN page `ArtifactRef` URNs; `origin_actor` is the PSEUDONYMOUS Principal
/// ref (erasure-safe, §4.6). This is the REAL typed event the M2 [`SyntheticTypedEvent`] stood in for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageParentEvent {
    /// The PARENT page root (`myelin://<tenant>/knowledge/page/<parent>`).
    pub parent: ArtifactRef,
    /// The CHILD page root (`myelin://<tenant>/knowledge/page/<child>`) re-parented under `parent`.
    pub child: ArtifactRef,
    /// The provenance event id (audit) — `knowledge.page.created` (initial parent) or
    /// `knowledge.page.moved` (re-parent). Validated against the frozen KN durable taxonomy.
    pub origin_event_id: String,
    /// The frozen KN durable event TYPE that drove this re-parent ([`KNOWLEDGE_PAGE_CREATED`] or
    /// [`KNOWLEDGE_PAGE_MOVED`]) — the lifecycle trigger (asserted in [`PageParentEvent::lifecycle_token`]).
    pub origin_event_type: String,
    /// The PSEUDONYMOUS Principal ref that authored the re-parent (erasure-safe; never the name).
    pub origin_actor: String,
    /// The consistency token at write time (§4.4).
    pub zookie: Option<String>,
}

impl PageParentEvent {
    /// The frozen KN durable event tokens that drive a `page_parent` re-parent: a page created under a
    /// parent ([`KNOWLEDGE_PAGE_CREATED`]) or a page moved/re-parented ([`KNOWLEDGE_PAGE_MOVED`]). A
    /// `page_parent` event whose `origin_event_type` is outside this set is NOT a re-parent trigger.
    pub const LIFECYCLE_TRIGGERS: &'static [&'static str] =
        &[KNOWLEDGE_PAGE_CREATED, KNOWLEDGE_PAGE_MOVED];

    /// Whether this event's `origin_event_type` is a recognised KN page_parent lifecycle trigger
    /// (REF-3: a re-parent off an unrecognised token is rejected, never mirrored on a guess).
    pub fn is_lifecycle_trigger(&self) -> bool {
        Self::LIFECYCLE_TRIGGERS.contains(&self.origin_event_type.as_str())
    }

    /// Lower a real KN `page_parent` event onto the REF-P14 [`SyntheticTypedEvent`] shape (the ONE
    /// mirror input the discipline consumes) with `rel = parent` — the `parent` page is the `source`,
    /// the `child` page the `target` (so the mirror projects a `parent` edge `parent→child` AND its
    /// frozen inverse `child` edge `child→parent`). This is the ONLY mapping; the page_parent mirror is
    /// the FIRST real caller of the SAME `mirror_edges`/`reconverge` the synthetic M2 events exercised.
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

/// **Mirror ONE real KN `page_parent` event into BOTH inverse-paired `lifecycle`-class edges (§3.3 —
/// the FIRST real TE-7 mirror, contract 5.5).** Returns the `parent` edge (`parent→child`) AND its
/// frozen inverse `child` edge (`child→parent`) — so cross-subsystem traversal of the KN page tree in
/// either direction ("what is the parent of X?" / "what are the children of Y?") is one Refs query.
/// Every returned edge is `rel_class = Lifecycle`. Reuses the ONE REF-P14 [`mirror_edges`] — the
/// page_parent mirror does NOT invent a second projection; it is the FIRST caller over a real table.
///
/// REF-3: a `page_parent` event off an unrecognised lifecycle trigger ([`PageParentEvent::is_lifecycle_trigger`]
/// is false) is REJECTED ([`MirrorError::UnknownRel`] carrying the offending token), never mirrored on a
/// guess.
pub fn mirror_page_parent(
    tenant: &TenantId,
    ev: &PageParentEvent,
) -> Result<Vec<EdgeRow>, MirrorError> {
    if !ev.is_lifecycle_trigger() {
        return Err(MirrorError::UnknownRel(ev.origin_event_type.clone()));
    }
    Ok(mirror_edges(tenant, &ev.as_typed_event()))
}

/// **Project a real KN `page_parent` event into the edge projection (the consumer-side first mirror).**
/// Validates the lifecycle trigger (rejects an unrecognised token — REF-3), then upserts BOTH
/// inverse-paired `parent`/`child` lifecycle edges via the deterministic `edge_id` (idempotent — a
/// replay is one pair, not duplicates). This is the discipline the REF-P6 edge-builder runs for
/// `knowledge.page.*` re-parent events — now over the REAL `page_parent` typed table (the FIRST real
/// mirror, replacing the M2 [`SyntheticTypedEvent`] stand-in for KN). Tenant-first (no cross-tenant
/// path). Returns the minted edge ids.
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

/// **Reconverge the KN page-tree mirror to the typed `page_parent` table — the typed table always wins
/// (§3.3 / §4.7; drill REF-D4 TE-7 half, the FIRST real reconvergence).** Given the AUTHORITATIVE typed
/// snapshot of `page_parent` rows for a scope (the re-parent events a scoped reindex re-emits) + the
/// child page roots the snapshot covers, reconverge: (1) re-project every snapshot event's
/// `parent`/`child` edge pair (the typed truth becomes live); (2) tombstone any lifecycle edge inbound
/// to a covered child root that the typed snapshot does NOT back (a stale re-parent the typed table no
/// longer authorises). After reconvergence the live KN page-tree lifecycle set for the scope == exactly
/// the typed snapshot — an out-of-band re-parent that drifted the projection is corrected to the typed
/// table. Reuses the ONE REF-P14 [`reconverge`]; returns `(re-projected-pairs, tombstoned-drift)` for
/// the drill's quantified gate. Tenant-first.
pub fn reconverge_page_tree(
    proj: &EdgeProjection,
    tenant: &TenantId,
    region: &Region,
    typed_snapshot: &[PageParentEvent],
    covered_children: &[ArtifactRef],
    reindex_event_id: &str,
) -> Result<(usize, usize), MirrorError> {
    // Lower the real page_parent snapshot onto the ONE REF-P14 reconverge input (rel = parent). A
    // non-trigger event in the snapshot is rejected LOUDLY (REF-3) before any projection mutation.
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

// =================================================================================================
// 2.6 — the KN page-subtree replay grain (sub-artifact-granular, BLOCK granularity)
// =================================================================================================

/// **Build the KN page-subtree reindex scope (contract 2.6 — sub-artifact-granular replay at BLOCK
/// granularity).** Selects the KN sub-artifact grain Knowledge's `replay(scope, since)` re-emits, so a
/// scoped reindex re-derives the block anchors at BLOCK grain (never a stale positional index):
/// - `PageScope` → `page:<id>` (a whole page + its blocks),
/// - `BlockScope` → `block:<page>/<id>` (a single block — the block anchor re-derives),
/// - `SubtreeScope` → `subtree:<page>` (a page subtree — the page_parent mirror reconverges).
///
/// Returns the selector string Knowledge's `replay` parses (the `myelin_events::SnapshotScope`
/// selector Knowledge's [`myelin_content::replay`] page-subtree replay consumes). The Refs reindex
/// consumer ([`crate::reindex`]) ingests the re-emitted snapshots through the SAME builder `handle`
/// (cold == live) — one code path, no KN-DB backdoor.
pub fn kn_replay_scope(grain: KnReplayGrain) -> String {
    match grain {
        KnReplayGrain::Page(id) => format!("page:{id}"),
        KnReplayGrain::Block { page, id } => format!("block:{page}/{id}"),
        KnReplayGrain::Subtree(page) => format!("subtree:{page}"),
    }
}

/// The KN sub-artifact grain a reindex scope selects (contract 2.6 — BLOCK granularity). Composed into
/// the selector Knowledge's `replay` parses. Refs names the grain; Knowledge owns the replay body (the
/// page-subtree re-emit at block grain — `knowledge.block.snapshot`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KnReplayGrain {
    /// A whole page + its blocks (`page:<id>`).
    Page(String),
    /// A single block — the block anchor re-derives at block grain (`block:<page>/<id>`).
    Block {
        /// The page id.
        page: String,
        /// The block id.
        id: String,
    },
    /// A page subtree — the page_parent mirror reconverges (`subtree:<page>`).
    Subtree(String),
}

#[cfg(test)]
mod tests;

//! # `issues_producer` — Issues lifecycle edges: the SECOND real TE-7 mirror (`issue_relation`)
//! + the `<PROJECTKEY>-<seqno>` key + the `field-`/`row-` sub-anchors (REF-P20 / P-336, M4).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/reference-graph.md`
//! - §3.3 (the TE-7 typed-edge mirror — **Issues `issue_relation` is the SECOND real lifecycle
//!   mirror**: the typed relation table is the source of truth, Refs holds the rebuildable
//!   inverse-paired projection over the FULL lifecycle vocabulary
//!   `closes`/`blocks`/`blocked_by`/`depends_on`/`parent`/`assigns`/`relates`, and **the typed table
//!   always wins on drift**),
//! - §4.5 (the recursive-CTE lineage traverse the second mirror enables — the spec-to-ship lineage
//!   `initiative → child issues → PRs → commits → CI → deploy → chat decision` is ONE Refs traverse,
//!   not a five-way fan-out),
//! - §4.6 (the ONE 4-step ladder on REAL Issues sub-anchors — a tombstone ALWAYS carries the root;
//!   a stable field resolves LIVE, an edited field OUTDATED, a deleted field/row GONE).
//!
//! **Contracts implemented (consumer side, to the FROZEN shapes; the Refs engine is UNCHANGED):**
//! - **5.5** project Issues' `issue.relation.*` typed-lifecycle events as `rel_class='lifecycle'`
//!   edges with the REF-P14 inverse pairing — **the SECOND time the TE-7 mirror discipline runs over
//!   a REAL typed table** ([`mirror_issue_relation`]). An out-of-band edit to an `issue_relation` row
//!   plus a scoped reindex reconverges Refs to the typed table (the typed table wins,
//!   [`reconverge_issue_relations`]). Unlike KN's `page_parent` (the FIRST mirror, ONE rel
//!   `parent`), Issues exercises the WHOLE lifecycle vocabulary (`closes`/`blocks`/`blocked_by`/
//!   `depends_on`/`parent`/`assigns`/`relates`), so the inverse pairing is proven on every shape
//!   (`blocks↔blocked_by`, `parent↔child`, the symmetric `relates`, the `None`-inverse directionals).
//! - **5.6** resolve Issues' `project(ref, viewer)` for the `<PROJECTKEY>-<seqno>` key + sub-anchors —
//!   [`IssueOwner`] is a REAL [`crate::ProjectApi`] over Issues' content; its `project` classifies an
//!   Issues `#sub` (a `field-<id>` field, a `row-<id>` row) into the frozen [`crate::ProjectOutcome`].
//! - **5.7** the `field-`/`row-` `#sub` kinds on REAL Issues sub-anchors — Refs resolves Issues'
//!   `field-`/`row-` mints through the ONE [`crate::ladder`]: stable → LIVE, edited → OUTDATED,
//!   moved → MOVED, deleted → GONE, erased → ERASED. The SAME vocabulary a Git line-range / a KN
//!   block / a CI check degrades through (one ladder).
//!
//! ## Why this is a CONSUMER module, not a new engine (EI-01 §7 coherence — the engine is UNCHANGED)
//! Exactly like the REF-P18 [`crate::kn_producer`] (the FIRST mirror) and the REF-P19
//! [`crate::ci_producer`], REF-P20's deliverable is to WIRE Refs to the REAL Issues producer + the
//! SECOND real lifecycle mirror — NOT to build a second resolver/ladder/mirror-engine. So this module:
//! - reuses [`crate::resolve::ResolveService`] + [`crate::ladder`] for resolution (the ONE chokepoint),
//! - reuses [`crate::mirror`] (`mirror_edges`/`reconverge`/[`crate::LifecycleRel`]) for the
//!   `issue_relation` typed mirror — the SAME REF-P14 discipline + inverse pairing, now over the
//!   SECOND REAL typed table (replacing, for Issues `issue_relation`, the
//!   [`crate::SyntheticTypedEvent`] M2 stand-in),
//! - reuses Issues' canonical `issue.relation.*` event tokens by NAME (`myelin_issues::events`) — the
//!   names anchor (X-5), never a literal.
//!
//! It adds ONLY the Issues-specific glue: the Issues source-URN construction for the
//! `<PROJECTKEY>-<seqno>` key ([`IssueEdgeProducer`]), the Issues `ProjectApi`/sub-anchor resolution
//! body ([`IssueOwner`]) the engine calls, and the `issue.relation.*` → [`crate::LifecycleRel`] event
//! mapping ([`mirror_issue_relation`]). No Refs type is re-defined; no parallel second ladder/mirror is
//! minted — the `issue_relation` mirror is the SECOND caller of the REF-P14 `mirror_edges`/`reconverge`
//! over a real typed table.
//!
//! ## Floors named (VISION §3 / EI-01 §1 — the prompt's named floors)
//! - **No new Refs floor — the engine is FIXED at M2.** The ladder, the grammar, the chokepoint, the
//!   mirror discipline are all M2-frozen; this prompt adds ONLY the Issues sub-anchor resolution + the
//!   `issue_relation` mirror wiring. Chat unfurls (the maximal consumer) are REF-P21 (named so this
//!   consumer is not mistaken for the complete five-producer corpus).
//! - **In-cell single-home-cell graph build.** Cross-cell fan-out is **R-M5 (REF-P26)** — this wiring
//!   builds + resolves the Issues graph in the artifact's home cell; the C-5 cross-cell semantics are
//!   already frozen in [`crate::resolve`].

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

/// The canonical Issues subsystem token (the §6.1 grammar prefix Issues owns; re-exported from the
/// durable taxonomy so an Issues drill asserts against the ONE token, never a literal). Derived from
/// the frozen [`RELATION_CREATED`] token's subsystem segment.
pub const ISSUE_OWNER_TOKEN: &str = "issue";

/// **The Issues source-URN producer for the `<PROJECTKEY>-<seqno>` key (contract 5.6 — C-3).** The
/// canonical Issues `<id>` segment is the project-prefix + monotonic number (e.g. `ENG-1421`), a REAL
/// URN component (§3.1 / C-3); the short display form `#1421` is render-time only (§4.8) and is NOT a
/// stored scope. This producer constructs the Issues issue / initiative roots Refs stores edges + sub-
/// anchors against. References-not-payloads: every URN is opaque; no issue title/body prose is held.
pub struct IssueEdgeProducer;

impl IssueEdgeProducer {
    /// The canonical Issues **issue root** `myelin://<tenant>/issue/issue/<PROJECTKEY>-<seqno>` — the
    /// root an `issue_relation` edge / a `field-`/`row-` sub-anchor hangs off. `key` is the FROZEN
    /// C-3 `<PROJECTKEY>-<seqno>` key (e.g. `ENG-1421`), stored canonical (the short `#1421` display is
    /// never the stored scope, §4.8).
    pub fn issue_root(tenant: &str, key: &str) -> ArtifactRef {
        ArtifactRef(format!("myelin://{tenant}/issue/issue/{key}"))
    }

    /// The canonical Issues **initiative root** `myelin://<tenant>/issue/initiative/<key>` — the root
    /// of the spec-to-ship lineage traverse (§4.5; the `initiative` type token is the sanctioned §6.2
    /// extension). An `initiative → child issues` `parent` relation hangs the lineage off this root.
    pub fn initiative_root(tenant: &str, key: &str) -> ArtifactRef {
        ArtifactRef(format!("myelin://{tenant}/issue/initiative/{key}"))
    }
}

/// The state of an Issues field / row sub-anchor (§4.6). The owner ([`IssueOwner`]) records these; the
/// resolver maps them onto the frozen [`SubState`] so a real Issues sub-anchor degrades through the
/// SAME ladder as a Git line-range / a KN block / a CI check. An Issues field/row id is a **stable
/// opaque id** (the stability obligation is Issues', §3.5) — so an EDITED field keeps its id and
/// resolves OUTDATED (not GONE); only a DELETED field/row is GONE.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueAnchorState {
    /// The field/row resolves exactly — its content is unchanged (LIVE).
    Live,
    /// The field/row MOVED (re-ordered within the issue scheme / re-keyed in a view) but the stable
    /// opaque id survives — render the moved anchor, flagged `moved` (MOVED). The id is immutable, so
    /// a moved field is MOVED, not GONE.
    Moved,
    /// The field/row was EDITED (its value changed) but the stable id survives — render the anchor,
    /// flagged `outdated` (OUTDATED). The embed shows the current field value with the staleness flag.
    Edited,
    /// The field/row was DELETED (removed from the scheme / the issue) — the root issue still resolves,
    /// the sub anchor is gone (GONE ⇒ `Tombstone{ sub_gone, root }`; the embed shows the parent issue).
    Deleted,
    /// The issue (and so its fields/rows) was ERASED (crypto-shred of the issue's per-subject DEK,
    /// contract 2.7) — unrenderable (ERASED ⇒ `Tombstone{ erased }`).
    Erased,
}

impl IssueAnchorState {
    /// Map an Issues field/row state onto the frozen §4.6 [`SubState`], carrying the owner projection
    /// on the renderable arms. The ONE vocabulary — an Issues sub-anchor degrades identically to a
    /// Git line-range / a KN block / a CI check.
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

/// **The REAL Issues owner — a [`ProjectApi`] + [`SubAnchorResolver`] over Issues' content (contracts
/// 5.6 / 5.7).** This is the producer half Refs' chokepoint calls when resolving an Issues `#sub`:
///
/// - a `field-<id>` field / `row-<id>` row → the recorded [`IssueAnchorState`] (stable→LIVE,
///   moved→MOVED, edited→OUTDATED, deleted→GONE, erased→ERASED),
/// - a bare root (no `#sub`) → LIVE (the issue / initiative itself).
///
/// Refs NEVER reads Issues' DB — it only calls this seam. The leak invariant is the chokepoint's:
/// this owner is reached ONLY on the permission-allowed branch (the chokepoint gates it). The
/// `check_view` verdict is Issues' authoritative permission decision (4.2 — production is Identity's
/// `check` over the Issues ReBAC fragment's confidential-exclusion + field/transition caveats,
/// REF-P322; here the recorded ACL).
///
/// Cloneable: every map is held behind an [`Arc`] so a clone shares the SAME recorded state (the
/// resolve chokepoint holds the owner behind an `Arc<dyn ProjectApi>`; a clone lets the test record
/// into the same owner the service resolves through). Tenant-first; no cross-tenant key.
#[derive(Clone, Default)]
pub struct IssueOwner {
    /// `(tenant|region|principal|root)` → the authoritative `view` decision (4.2). The recorded ACL the
    /// production wire replaces with Identity's `check` over the resilient client (the Issues
    /// confidential-exclusion + field/transition caveats, REF-P322). Default-deny.
    acl: Arc<Mutex<BTreeMap<String, Decision>>>,
    /// `full-ref-urn` → the Issues field/row anchor state (the §4.6 sub-anchor state).
    anchors: Arc<Mutex<BTreeMap<String, IssueAnchorState>>>,
}

impl IssueOwner {
    /// A fresh Issues owner (default-deny ACL; no anchors recorded — an unscripted bare root is LIVE).
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

    /// Grant a viewer the `view` permission on an Issues root (the recorded ACL — the Issues ReBAC
    /// fragment's `view` grant, modelled here; production is Identity's `check` over the
    /// confidential-exclusion + field/transition caveats, REF-P322).
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

    /// Record an Issues field/row anchor state (`field-`/`row-`) — the §4.6 sub-anchor state. An edit
    /// that keeps the stable id records [`IssueAnchorState::Edited`]; a delete records
    /// [`IssueAnchorState::Deleted`].
    pub fn record_anchor(&self, ref_: &ArtifactRef, state: IssueAnchorState) {
        self.anchors.lock().unwrap().insert(ref_.0.clone(), state);
    }

    /// The default owner projection a renderable Issues anchor carries (a render-safe title — the leak
    /// invariant already gates this; the owner is reached only on the allowed branch). PII-free.
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

    /// Resolve an Issues `#sub` anchor on `ref_` into the frozen [`SubState`] (the §4.6 step-3 owner
    /// resolver — the SUB-ANCHOR RESOLUTION REF-P20 ships). Dispatched by the Issues `#sub` KIND through
    /// the ONE Refs grammar (`sub_kind`):
    /// - `field-<id>` field / `row-<id>` row → the recorded [`IssueAnchorState`] (stable→LIVE,
    ///   moved→MOVED, edited→OUTDATED, deleted→GONE),
    /// - bare root → LIVE.
    fn resolve_issue_sub(&self, ref_: &ArtifactRef, sub: Option<&Sub>) -> SubState {
        let projection = Self::projection(ref_);
        match sub {
            // A bare root (no #sub) is LIVE — the issue / initiative itself, no sub-anchor to degrade.
            None => SubState::Live(projection),
            // The Issues sub-anchor kinds — the recorded §4.6 state. A field/row id is a STABLE opaque
            // id, so an edited field is OUTDATED (id survives), a deleted field/row is GONE.
            Some(Sub::Field(_)) | Some(Sub::Row(_)) => {
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
            // Any other kind on an Issues ref is not an Issues-owned mint — Issues renders the bare root
            // LIVE rather than guess (REF-3 — never guess scope; the grammar already rejected unknowns).
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
        // The authoritative `view` verdict on the ROOT (the chokepoint passes the #sub-stripped root).
        // Default-deny: an unrecorded grant is a Deny (so a viewer with no Issues read — e.g. a
        // confidential issue's non-member — is tombstoned, never leaked — the REF-D1 leak invariant on
        // the Issues corpus; supports the confidential-exclusion fragment, REF-P322).
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
        // The owner classifies the Issues #sub into the frozen ProjectOutcome through the ONE ladder
        // mapping (SubState::into_outcome). Called ONLY on the permission-allowed branch.
        let sub = sub_kind(ref_);
        Ok(self.resolve_issue_sub(ref_, sub.as_ref()).into_outcome())
    }
}

impl SubAnchorResolver for IssueOwner {
    /// The §4.6 step-3 sub-anchor resolver (the SAME logic `project` runs) — exposed so a drill can
    /// drive the ladder directly (REF-D9) through [`crate::resolve_sub_outcome`] without the full
    /// chokepoint. ONE source of truth: it delegates to [`IssueOwner::resolve_issue_sub`].
    fn resolve_sub(&self, ref_: &ArtifactRef, sub: Option<&Sub>) -> SubState {
        self.resolve_issue_sub(ref_, sub)
    }
}

// =================================================================================================
// 5.5 — the SECOND real lifecycle mirror: Issues `issue_relation` (the TE-7 mirror over a REAL table)
// =================================================================================================

/// **An Issues `issue_relation` typed-lifecycle event — the SECOND real TE-7 mirror input (§3.3 /
/// contract 5.5).** An Issues relation write (`issue.relation.created`, or a reindex
/// `issue.relation.snapshot`) writes the typed `issue_relation` row + emits this typed lifecycle event
/// in the SAME transaction (producer #2). It carries the `(source, target, rel)` triple over the FULL
/// lifecycle vocabulary + provenance. PII-free: `source`/`target` are opaque Issues `ArtifactRef`
/// URNs; `origin_actor` is the PSEUDONYMOUS Principal ref (erasure-safe, §4.6). This is the REAL typed
/// event the M2 [`SyntheticTypedEvent`] stood in for — and unlike KN's `page_parent` (the FIRST mirror,
/// one rel), Issues exercises the WHOLE vocabulary, so every inverse shape is mirrored over a real
/// table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssueRelationEvent {
    /// The SOURCE issue root (`myelin://<tenant>/issue/issue/<PROJECTKEY>-<seqno>`) — the referencing
    /// side of the typed relation row (e.g. issue ENG-1 in "ENG-1 `blocks` ENG-2").
    pub source: ArtifactRef,
    /// The TARGET issue root — the referenced side of the typed relation row (e.g. issue ENG-2).
    pub target: ArtifactRef,
    /// The lifecycle relation token the typed row carries (`closes`/`blocks`/`blocked_by`/`depends_on`/
    /// `parent`/`assigns`/`relates`). Validated against the frozen [`LifecycleRel::parse`] — an unknown
    /// relation is REJECTED (REF-3), never guessed.
    pub rel: String,
    /// The provenance event id (audit) — which `issue.relation.*` event wrote this.
    pub origin_event_id: String,
    /// The frozen Issues durable event TYPE that drove this relation
    /// ([`RELATION_CREATED`]/[`RELATION_REMOVED`]/[`RELATION_SNAPSHOT`]) — the lifecycle trigger
    /// (asserted in [`IssueRelationEvent::is_lifecycle_trigger`]).
    pub origin_event_type: String,
    /// The PSEUDONYMOUS Principal ref that authored the relation write (erasure-safe; never the name).
    pub origin_actor: String,
    /// The consistency token at write time (§4.4).
    pub zookie: Option<String>,
}

impl IssueRelationEvent {
    /// The frozen Issues durable event tokens that drive an `issue_relation` mirror: a relation created
    /// ([`RELATION_CREATED`]), removed ([`RELATION_REMOVED`]), or re-emitted by a reindex
    /// ([`RELATION_SNAPSHOT`]). A relation event whose `origin_event_type` is outside this set is NOT a
    /// recognised mirror trigger (REF-3 — never mirrored on a guess).
    pub const LIFECYCLE_TRIGGERS: &'static [&'static str] =
        &[RELATION_CREATED, RELATION_REMOVED, RELATION_SNAPSHOT];

    /// Whether this event's `origin_event_type` is a recognised Issues relation lifecycle trigger.
    pub fn is_lifecycle_trigger(&self) -> bool {
        Self::LIFECYCLE_TRIGGERS.contains(&self.origin_event_type.as_str())
    }

    /// Lower a real Issues `issue_relation` event onto the REF-P14 [`SyntheticTypedEvent`] shape (the
    /// ONE mirror input the discipline consumes), parsing `rel` against the frozen lifecycle vocabulary.
    /// REJECTS (REF-3) if the trigger is unrecognised OR the relation token is outside the frozen
    /// vocabulary — never mirrored on a guess. The `issue_relation` mirror is the SECOND real caller of
    /// the SAME `mirror_edges`/`reconverge` the synthetic M2 events exercised.
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

/// **Mirror ONE real Issues `issue_relation` event into BOTH inverse-paired `lifecycle`-class edges
/// (§3.3 — the SECOND real TE-7 mirror, contract 5.5).** Returns the forward lifecycle edge AND its
/// frozen inverse edge (endpoints swapped) — so cross-subsystem traversal in either direction
/// ("what does ENG-1 block?" / "what is ENG-2 blocked by?") is one Refs query. Every returned edge is
/// `rel_class = Lifecycle`. Reuses the ONE REF-P14 [`mirror_edges`] — the `issue_relation` mirror does
/// NOT invent a second projection; it is the SECOND caller over a real table, exercising the WHOLE
/// vocabulary (`blocks↔blocked_by`, `parent↔child`, the symmetric `relates`, the `None`-inverse
/// `closes`/`depends_on`/`assigns`).
///
/// REF-3: an event off an unrecognised trigger OR carrying a relation token outside the frozen
/// vocabulary is REJECTED ([`MirrorError::UnknownRel`] carrying the offending token), never mirrored on
/// a guess.
pub fn mirror_issue_relation(
    tenant: &TenantId,
    ev: &IssueRelationEvent,
) -> Result<Vec<EdgeRow>, MirrorError> {
    Ok(mirror_edges(tenant, &ev.as_typed_event()?))
}

/// **Project a real Issues `issue_relation` event into the edge projection (the consumer-side second
/// mirror).** Validates the lifecycle trigger + the relation token (rejects unrecognised/unknown —
/// REF-3), then upserts BOTH inverse-paired lifecycle edges via the deterministic `edge_id`
/// (idempotent — a replay is one pair, not duplicates). This is the discipline the REF-P6 edge-builder
/// runs for `issue.relation.*` events — now over the REAL `issue_relation` typed table (the SECOND real
/// mirror, replacing the M2 [`SyntheticTypedEvent`] stand-in for Issues). Tenant-first (no cross-tenant
/// path). Returns the minted edge ids.
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

/// **Reconverge the Issues relation mirror to the typed `issue_relation` table — the typed table always
/// wins (§3.3 / §4.7; drill REF-D4 TE-7 half, the SECOND real reconvergence).** Given the AUTHORITATIVE
/// typed snapshot of `issue_relation` rows for a scope (the relations a scoped reindex re-emits) + the
/// issue roots the snapshot covers, reconverge: (1) re-project every snapshot event's inverse-paired
/// edges (the typed truth becomes live); (2) tombstone any lifecycle edge inbound to a covered root
/// that the typed snapshot does NOT back (a stale relation the typed table no longer authorises — e.g.
/// an out-of-band edit that removed a `blocks` row). After reconvergence the live Issues lifecycle set
/// for the scope == exactly the typed snapshot — an out-of-band `issue_relation` edit that drifted the
/// projection is corrected to the typed table. Reuses the ONE REF-P14 [`reconverge`]; returns
/// `(re-projected-pairs, tombstoned-drift)` for the drill's quantified gate. Tenant-first.
pub fn reconverge_issue_relations(
    proj: &EdgeProjection,
    tenant: &TenantId,
    region: &Region,
    typed_snapshot: &[IssueRelationEvent],
    covered_roots: &[ArtifactRef],
    reindex_event_id: &str,
) -> Result<(usize, usize), MirrorError> {
    // Lower the real issue_relation snapshot onto the ONE REF-P14 reconverge input. A non-trigger /
    // unknown-rel event in the snapshot is rejected LOUDLY (REF-3) before any projection mutation.
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

//! # `git_producer` — Refs consumes the REAL Git producer edges + content-anchored line-range
//! sub-anchors + per-blob/ref replay (REF-P17 / P-258, M3).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/reference-graph.md`
//! - §3.5 (**Git line-ranges are content-anchored** — BLAKE3 fingerprint + 3-way context match →
//!   exact/rebased/partial/tombstone; the `comment-`/`thread-` review-thread anchors share the one
//!   ladder),
//! - §4.6 (the ONE 4-step ladder on REAL sub-anchors — a tombstone ALWAYS carries the root),
//! - §4.7 (sub-artifact-granular replay — a scoped Git reindex re-emits at the right grain so the
//!   content-anchored line-range anchors RE-DERIVE, never a stale raw line number).
//!
//! **Contracts implemented (consumer side, to the frozen shapes; the engine is UNCHANGED):**
//! - **5.4** consume the REAL Git producer reference edges (commit-trailer / PR-link / "Closes
//!   <issue>") — they emit `refs.edge.created` via the THREE structured content nodes
//!   ([`myelin_content::InlineNode`]), extracted through the SAME [`crate::extract_edges`] seam the
//!   synthetic M2 producer used. The Git producer is no longer synthetic — it is a Git PR/commit
//!   body modelled as a `myelin-content` document whose structured ref nodes are the edges.
//! - **5.6** consume Git's `project(ref, viewer)` sub-anchor resolver — [`GitOwner`] is a REAL
//!   [`crate::ProjectApi`] over Git content; its `project` classifies a Git `#sub` (a `L<a>-L<b>`
//!   line range through the engine's [`crate::resolve_line_range`], a `comment-`/`thread-` review
//!   anchor through its live/moved/resolved state) into the frozen [`crate::ProjectOutcome`] the
//!   chokepoint maps onto a [`crate::Resolution`].
//! - **5.7** the `#sub` kinds on REAL Git sub-anchors — Refs resolves Git's `comment-`/`thread-`/
//!   `L<a>-L<b>` mints (minted by [`myelin_git::subs`], the GIT-P4 producer half) through the ONE
//!   [`crate::ladder`]. The CI-owned `check-`/`step-` kinds are USED (not built): Git's
//!   `check_status` projection + `details_ref` (`#step-<n>`) resolve through the SAME ladder; CI's
//!   PRODUCER half lands in R-M4 (REF-P19) — here Refs ships the consumer/projection awaiting it.
//! - **2.6** drive Git per-blob/ref replay — [`git_replay_scope`] selects the Git sub-artifact grain
//!   (`repo:` / `blob:` / `pr:`) Git's [`myelin_git::replay::GitReindexSource`] re-emits, so a scoped
//!   reindex re-derives the content-anchored line-range anchors at blob grain (never a stale line #).
//! - **4.9** the Git ReBAC fragment flows through `list_objects` — the backlink read
//!   ([`crate::BacklinkRead`]) lowers the FROZEN `SetExpr` over `edge.source_root`, reusing REF-P11;
//!   a viewer with no `repo->pull` sees 0 PR/repo backlinks (the GIT-D11 leak-free list).
//!
//! ## Why this is a CONSUMER module, not a new engine (EI-01 §7 coherence — the engine is UNCHANGED)
//! REF-P17's deliverable is to WIRE Refs to the real Git producer + RE-CONFIRM the invariants on the
//! Git corpus — NOT to build a second resolver/ladder/edge-engine. So this module:
//! - reuses [`crate::extract_edges`] / [`crate::edge_builder`] for ingest (the §4.1 producer #1 seam),
//! - reuses [`crate::resolve::ResolveService`] + [`crate::ladder`] for resolution (the ONE chokepoint),
//! - reuses [`crate::resolve_line_range`] for the §3.5 content-anchored classifier (the ONE algorithm),
//! - reuses [`crate::reindex`] for replay (the ONE recovery path),
//! - reuses [`crate::BacklinkRead`] for the leak-free list (the ONE `SetExpr` lowering).
//!
//! It adds ONLY the Git-specific glue: the Git source-URN construction for edges
//! ([`GitEdgeProducer`]), and the Git `ProjectApi`/sub-anchor resolution body ([`GitOwner`]) that the
//! engine calls. No Refs type is re-defined; no parallel second ladder is minted.
//!
//! ## Floors named (VISION §3 / EI-01 §1 — the prompt's named floors)
//! - **In-cell single-home-cell graph build.** Cross-cell fan-out is **R-M5 (REF-P26)** — this wiring
//!   builds + resolves the Git graph in the artifact's home cell; the C-5 cross-cell semantics are
//!   already frozen in [`crate::resolve`] (a cross-cell target resolves in its home cell, only the
//!   filtered projection/tombstone crosses). Named here because the Git corpus is single-cell at M3.
//! - **Git pseudonymous-by-default commit authors as `origin_actor`.** A Git edge's `origin_actor` is
//!   the PSEUDONYM ([`myelin_git::commit`], GIT-P25 / P-257) baked into the immutable commit bytes —
//!   never the erasable real identity. The audited history-rewrite erasure path (a *body* expunge,
//!   contract 10.6) is **R-M5 / on-demand**. Both are GIT deliverables Refs DEPENDS on; named here
//!   because they gate Refs' clean erasure surface (REF-D5 / GIT-D2) — Refs holds only the opaque
//!   pseudonymous `origin_actor`, so Identity's 4.8 pseudonym shred makes a Git edge's author
//!   unresolvable with NO edge mutation (§4.6).
//! - **CI's `check-`/`step-` PRODUCER half is R-M4 (REF-P19).** Git's `check_status` projection +
//!   `details_ref` (`#step-<n>`) RESOLVE through the ladder here (the SUB-ANCHOR resolution, Refs'
//!   half); the seam that produces `ci.check.updated` is CI's, landing in REF-P19. Named so the
//!   consumer/projection here is not mistaken for the closed Git↔CI seam (that is REF-P335 / X-1).

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
use crate::resolve::{
    OwnerProjection, ProjectApi, ProjectApiError, ProjectOutcome, ResolveMode,
};
use crate::SubAnchorResolver;

/// **The Git reference-edge producer (contract 5.4 EMIT side — the REAL Git producer, no longer
/// synthetic).** A Git PR description / commit message / "Closes <issue>" body IS a `myelin-content`
/// document: its structured inline nodes ([`InlineNode::ArtifactRefNode`] for a PR-link or a commit
/// trailer's `<repo>#<pr>`, [`InlineNode::Mention`] for an `@`-reviewer, [`InlineNode::Embed`] for an
/// inline embed) are the edges. This producer constructs the Git SOURCE URN (the PR/commit root) and
/// drives the SAME [`emit_edges`] seam the M2 synthetic writer used — so the WIRING is unchanged; only
/// the CALLER is now the real Git surface.
///
/// References-not-payloads: the source/target are opaque Git/Issue/Identity URNs; a `@`-reviewer's
/// target is the PSEUDONYMOUS `member` URN (erasure-safe, §4.6). No commit-message free text is held —
/// only the structured ref nodes are read (the reliability guarantee, EI-04 §2.4: structured-node
/// extraction, never a regex over the commit message prose).
pub struct GitEdgeProducer;

impl GitEdgeProducer {
    /// The canonical Git **commit root** `myelin://<tenant>/git/commit/<repo>:<oid>` — the source of a
    /// commit-trailer reference edge ("Closes ENG-12" in a commit message). The `<repo>:<oid>`
    /// composed key is git's stable canonical key (architecture §2 / Δ7) — never a display short-sha.
    pub fn commit_root(tenant: &str, repo: &str, oid: &str) -> ArtifactRef {
        ArtifactRef(format!("myelin://{tenant}/git/commit/{repo}:{oid}"))
    }

    /// The canonical Git **PR root** `myelin://<tenant>/git/pr/<repo>:<n>` — the source of a PR-link /
    /// "Closes <issue>" reference edge in the PR description. Reuses git's canonical `<repo>:<n>` key.
    pub fn pr_root(tenant: &str, repo: &str, pr_number: u64) -> ArtifactRef {
        ArtifactRef(format!("myelin://{tenant}/git/pr/{repo}:{pr_number}"))
    }

    /// **Emit the reference edges of a real Git PR/commit body, in the SAME outbox transaction as the
    /// Git content write (contract 5.4 — emit-iff-committed, REF-D7 producer half).** `source` is the
    /// Git PR/commit root (built above); `body` is the structured `myelin-content` document of the
    /// PR description / commit message; `content_event` is the `git.pr.*` / `git.ref.updated` event
    /// being emitted in the SAME transaction (the CAUSE — the correlation root carries, `depth +1`,
    /// the loop-guard stamp). One `refs.edge.created` per structured ref node. Returns the minted ids.
    ///
    /// The edges become durable IFF the caller commits the Git content transaction — an aborted push /
    /// PR-open drops the buffered edge rows with it (no edge without its Git content). This is the same
    /// guarantee the synthetic M2 producer proved; the only change is the real Git caller + source URN.
    pub fn emit_git_edges(
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

    /// The extracted (un-emitted) Git reference edges of a body — exposed so a drill can assert the
    /// edge SET a real Git PR/commit body produces (the leak/IDOR re-confirmation corpus, REF-D1/D2)
    /// without driving the outbox. Reuses the ONE [`crate::extract_edges`] seam.
    pub fn git_edges(&self, source: &ArtifactRef, body: &[InlineNode]) -> Vec<EdgeDraft> {
        crate::extract_edges(source, body)
    }
}

/// The state of a Git PR review-comment / review-thread anchor (`comment-`/`thread-` kinds, §3.5). The
/// owner ([`GitOwner`]) records these; the resolver maps them onto the frozen [`SubState`] so a real
/// Git review anchor degrades through the SAME ladder as a line-range. (The live `project` body — the
/// inline / resolved-thread render — is GIT-P18; the Refs SUB-ANCHOR resolution + ladder mapping is
/// REF-P17, here.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommentState {
    /// The comment/thread anchor resolves exactly (LIVE).
    Live,
    /// The anchored comment was edited but the stable opaque id survives — render the moved anchor,
    /// flagged `moved` (MOVED). A comment id is immutable (the stability obligation is git's, §3.5), so
    /// a moved-by-edit comment is MOVED, not GONE.
    Moved,
    /// The review thread was RESOLVED (collapsed) — render the partial, flagged `outdated` (OUTDATED).
    Resolved,
    /// The comment/thread was DELETED — the root PR still resolves, the sub anchor is gone (GONE ⇒
    /// `Tombstone{ sub_gone, root }`; the embed shows the parent PR).
    Gone,
    /// The comment/thread was ERASED (crypto-shred of the body) — unrenderable (ERASED).
    Erased,
}

impl CommentState {
    /// Map a Git comment/thread state onto the frozen §4.6 [`SubState`], carrying the owner projection
    /// on the renderable arms. The ONE vocabulary — a Git review anchor degrades identically to a
    /// line-range / a KN block / a Chat message.
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

/// One minted Git line-range anchor + its current blob (the §3.5 content-anchor + the resolution
/// input). The owner holds the [`MintedLineRange`] (the BLAKE3 fingerprint + the mint-time blob oid)
/// for each `L<a>-L<b>` ref and the CURRENT blob (oid + lines) it resolves against. The resolver runs
/// the engine's [`resolve_line_range`] over them — Refs reuses the ONE content-anchoring algorithm; it
/// does not invent a second one.
#[derive(Clone, Debug)]
struct GitBlobAnchor {
    /// The mint-time content anchor (fingerprints + blob oid) — content-anchored, not positional.
    minted: MintedLineRange,
    /// The CURRENT blob oid (the exact-match short-circuit on resolve).
    current_oid: String,
    /// The CURRENT blob lines (the 3-way context match target). Stored as owned strings; in production
    /// the owner reads them from the blob store (REF-P17 feeds the real bytes; here the test feeds them).
    current_lines: Vec<String>,
}

/// **The REAL Git owner — a [`ProjectApi`] + [`SubAnchorResolver`] over Git content (contracts 5.6 /
/// 5.7).** This is the producer half Refs' chokepoint calls: `check_view` is Git's authoritative
/// permission verdict (4.2, reached over the resilient client in production — here the recorded ACL),
/// and `project` classifies a Git `#sub` into the frozen [`ProjectOutcome`]:
///
/// - a `L<a>-L<b>` line range → the engine's [`resolve_line_range`] (exact→LIVE, rebased→MOVED,
///   partial→OUTDATED, content_gone→GONE) — the §3.5 content-anchored classifier,
/// - a `comment-`/`thread-` review anchor → the recorded [`CommentState`] (the GIT-P18 render is the
///   owner's live body; the Refs sub-anchor RESOLUTION + ladder mapping is here),
/// - a `check-`/`step-` CI anchor → the `check_status` projection state (USED, not built — CI's
///   producer is REF-P19; here Refs resolves the SUB-ANCHOR through the ladder),
/// - a bare root (no `#sub`) → LIVE (the PR/commit/blob itself).
///
/// Refs NEVER reads Git's DB — it only calls this seam. The leak invariant is the chokepoint's: this
/// owner is reached ONLY on the permission-allowed branch (the chokepoint gates it).
///
/// Cloneable: every map is held behind an [`Arc`] so a clone shares the SAME recorded state (the
/// resolve chokepoint holds the owner behind an `Arc<dyn ProjectApi>`; a clone lets the test record
/// into the same owner the service resolves through). Tenant-first; no cross-tenant key.
#[derive(Clone, Default)]
pub struct GitOwner {
    /// `(tenant|region|principal|root)` → the authoritative `view` decision (4.2). The recorded ACL the
    /// production wire replaces with Identity's `check` over the resilient client. Default-deny.
    acl: Arc<Mutex<BTreeMap<String, Decision>>>,
    /// `full-ref-urn` → the Git line-range anchor (the §3.5 content anchor + current blob).
    line_ranges: Arc<Mutex<BTreeMap<String, GitBlobAnchor>>>,
    /// `full-ref-urn` → the Git comment/thread anchor state (the §3.5 review-anchor state).
    comments: Arc<Mutex<BTreeMap<String, CommentState>>>,
    /// `full-ref-urn` → the Git `check-`/`step-` CI anchor state (USED via the `check_status`
    /// projection; CI's producer is REF-P19). Default LIVE for a present check.
    checks: Arc<Mutex<BTreeMap<String, CommentState>>>,
}

impl GitOwner {
    /// A fresh Git owner (default-deny ACL; no anchors recorded — everything resolves through the
    /// recorded state, an unscripted bare root is LIVE).
    pub fn new() -> GitOwner {
        GitOwner::default()
    }

    fn acl_key(tenant: &TenantId, region: &Region, viewer: &Principal, root: &ArtifactRef) -> String {
        format!("{}|{}|{}|{}", tenant.0, region.0, viewer.principal_id.0, root.0)
    }

    /// Grant a viewer the `view` permission on a Git root (the recorded ACL — the GIT-D11 / 4.9
    /// fragment's `repo->pull` grant, modelled here; production is Identity's `check`).
    pub fn grant_view(&self, tenant: &TenantId, region: &Region, viewer: &Principal, root: &ArtifactRef) {
        self.acl
            .lock()
            .unwrap()
            .insert(Self::acl_key(tenant, region, viewer, root), Decision::Allow);
    }

    /// Record a Git line-range anchor: the mint-time content anchor (fingerprints + blob oid) + the
    /// CURRENT blob (oid + lines) it resolves against. A force-push that rewrites the blob updates the
    /// CURRENT side (the same `ref_`), so the next resolve re-derives the anchor (MOVED/OUTDATED/GONE).
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

    /// Record a Git comment/thread anchor state (`comment-`/`thread-`) — the §3.5 review-anchor state.
    pub fn record_comment(&self, ref_: &ArtifactRef, state: CommentState) {
        self.comments.lock().unwrap().insert(ref_.0.clone(), state);
    }

    /// Record a Git `check-`/`step-` CI anchor state (the `check_status` projection state; USED — CI's
    /// producer is REF-P19). Reuses the [`CommentState`] degrade vocabulary (a superseded check is
    /// OUTDATED, a pruned run is GONE).
    pub fn record_check(&self, ref_: &ArtifactRef, state: CommentState) {
        self.checks.lock().unwrap().insert(ref_.0.clone(), state);
    }

    /// The default owner projection a renderable Git anchor carries (a render-safe title — the leak
    /// invariant already gates this; the owner is reached only on the allowed branch).
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

    /// Resolve a Git `#sub` anchor on `ref_` into the frozen [`SubState`] (the §4.6 step-3 owner
    /// resolver — the SUB-ANCHOR RESOLUTION REF-P17 ships). Dispatched by the Git `#sub` KIND through
    /// the ONE Refs grammar (`sub_kind`):
    /// - `L<a>-L<b>` → the engine's [`resolve_line_range`] over the recorded content anchor,
    /// - `comment-`/`thread-` → the recorded [`CommentState`],
    /// - `check-`/`step-` → the recorded `check_status` state (USED; CI producer REF-P19),
    /// - bare root → LIVE.
    fn resolve_git_sub(&self, ref_: &ArtifactRef, sub: Option<&Sub>) -> SubState {
        let projection = Self::projection(ref_);
        match sub {
            // A bare root (no #sub) is LIVE — the PR/commit/blob itself, no sub-anchor to degrade.
            None => SubState::Live(projection),
            // The content-anchored line range — the §3.5 classifier (exact/rebased/partial/content_gone).
            Some(Sub::LineRange { .. }) => match self.line_ranges.lock().unwrap().get(&ref_.0) {
                Some(anchor) => {
                    let current: Vec<&str> = anchor.current_lines.iter().map(String::as_str).collect();
                    let state =
                        resolve_line_range(&anchor.minted, &anchor.current_oid, &current);
                    // Carry the (shift-adjusted) range onto the renderable arms so the projection
                    // reflects the resolved position (MOVED/OUTDATED), never a stale raw line number.
                    let mut p = projection;
                    p.sub_anchor = Some(self.render_line_anchor(ref_, &state));
                    line_state_into_sub(state, p)
                }
                // No anchor recorded for a `L<a>-L<b>` ref → the content is gone (defensive; a real
                // owner always has the mint-time anchor for a ref it minted).
                None => SubState::Gone,
            },
            // A Git review comment / thread anchor — the recorded §3.5 review-anchor state.
            Some(Sub::Comment(_)) | Some(Sub::Thread(_)) => self
                .comments
                .lock()
                .unwrap()
                .get(&ref_.0)
                .copied()
                .unwrap_or(CommentState::Live)
                .into_sub_state(projection),
            // A CI `check-`/`step-` anchor — USED via Git's `check_status` projection (CI producer is
            // REF-P19). Resolved through the SAME ladder (C-6 — first-class `#sub` kinds).
            Some(Sub::Check(_)) | Some(Sub::Step(_)) => self
                .checks
                .lock()
                .unwrap()
                .get(&ref_.0)
                .copied()
                .unwrap_or(CommentState::Live)
                .into_sub_state(projection),
            // Any other kind on a Git ref is not a Git-owned mint — Git renders the bare root LIVE
            // rather than guess (REF-3 — never guess scope; the grammar already rejected an unknown).
            Some(_) => SubState::Live(projection),
        }
    }

    /// The render-time anchor string for a resolved line-range state (the shifted range for MOVED, the
    /// surviving sub-range for OUTDATED, the minted ref for EXACT) — so the projection never shows a
    /// stale raw line number (§4.7: a scoped reindex re-derives this). PII-free (a line range).
    fn render_line_anchor(&self, ref_: &ArtifactRef, state: &LineRangeState) -> String {
        let root = strip_sub(ref_);
        match state {
            LineRangeState::Exact => ref_.0.clone(),
            LineRangeState::Rebased { new_start, new_end } => {
                format!("{}#L{new_start}-L{new_end}", root.0)
            }
            LineRangeState::Partial { surviving_start, surviving_end } => {
                format!("{}#L{surviving_start}-L{surviving_end}", root.0)
            }
            // content_gone carries no anchor (the tombstone carries the root, the chokepoint's job).
            LineRangeState::ContentGone => root.0,
        }
    }
}

/// Map a [`LineRangeState`] onto a [`SubState`] carrying the owner projection (the §3.5 → §4.6 bridge,
/// reusing the engine's mapping — there is ONE mapping; this is the call into it with the projection).
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
        // The authoritative `view` verdict on the ROOT (the chokepoint passes the #sub-stripped root).
        // Default-deny: an unrecorded grant is a Deny (so a viewer with no `repo->pull` is tombstoned,
        // never leaked — the GIT-D11 / REF-D1 leak invariant on the Git corpus).
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
        // The owner classifies the Git #sub into the frozen ProjectOutcome through the ONE ladder
        // mapping (SubState::into_outcome). Called ONLY on the permission-allowed branch.
        let sub = sub_kind(ref_);
        Ok(self.resolve_git_sub(ref_, sub.as_ref()).into_outcome())
    }
}

impl SubAnchorResolver for GitOwner {
    /// The §4.6 step-3 sub-anchor resolver (the SAME logic `project` runs) — exposed so a drill can
    /// drive the ladder directly (REF-D9) through [`crate::resolve_sub_outcome`] without the full
    /// chokepoint. ONE source of truth: it delegates to [`GitOwner::resolve_git_sub`].
    fn resolve_sub(&self, ref_: &ArtifactRef, sub: Option<&Sub>) -> SubState {
        self.resolve_git_sub(ref_, sub)
    }
}

/// **Build the Git per-blob/ref reindex scope (contract 2.6 — sub-artifact-granular replay).** Selects
/// the Git sub-artifact grain Git's [`myelin_git::replay::GitReindexSource`] re-emits, so a scoped
/// reindex re-derives the content-anchored line-range anchors at BLOB grain (never a stale line #):
/// - `RepoScope` → `repo:<id>` (a whole repo),
/// - `BlobScope` → `blob:<repo>/<oid>` (a single blob — the line-range anchor re-derives),
/// - `PrScope` → `pr:<repo>/<n>` (a single PR — the comment/thread anchors re-derive).
///
/// Returns the selector string Git's `replay(scope, since)` parses. The Refs reindex consumer
/// ([`crate::reindex`]) ingests the re-emitted snapshots through the SAME builder `handle` (cold ==
/// live) — one code path, no Git-DB backdoor.
pub fn git_replay_scope(grain: GitReplayGrain) -> String {
    match grain {
        GitReplayGrain::Repo(id) => format!("repo:{id}"),
        GitReplayGrain::Blob { repo, oid } => format!("blob:{repo}/{oid}"),
        GitReplayGrain::Pr { repo, number } => format!("pr:{repo}/{number}"),
    }
}

/// The Git sub-artifact grain a reindex scope selects (contract 2.6). Mirrors
/// [`myelin_git::replay::GitReplayKind`] (the Git producer's grain), composed into the selector Git's
/// `replay` parses. Refs names the grain; Git owns the replay body (the re-emit at that grain).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitReplayGrain {
    /// A whole repo (`repo:<id>`).
    Repo(String),
    /// A single blob — the line-range anchor re-derives at blob grain (`blob:<repo>/<oid>`).
    Blob {
        /// The repo id.
        repo: String,
        /// The blob oid.
        oid: String,
    },
    /// A single PR — the comment/thread anchors re-derive (`pr:<repo>/<n>`).
    Pr {
        /// The repo id.
        repo: String,
        /// The PR number.
        number: u64,
    },
}

/// The canonical Git subsystem token (re-exported so a Git-edge drill asserts against the ONE token
/// git owns, never a literal — [`myelin_git::subs::GIT_SUBSYSTEM`]).
pub const GIT_OWNER_TOKEN: &str = GIT_SUBSYSTEM;

#[cfg(test)]
mod tests;

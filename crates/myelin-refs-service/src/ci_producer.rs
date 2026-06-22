//! # `ci_producer` — the Git↔CI CheckStatus seam closes: Refs resolves the `check-`/`step-`
//! CI sub-anchors (Refs' half of X-1) (REF-P19 / P-335, M4).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/reference-graph.md`
//! - §3.5 (the `check-<context>` / `step-<n>` kinds are first-class members of the ONE `#sub`
//!   grammar — C-6 — so CI's `CheckStatus` subject and the `details_ref` jump-to-failure resolve
//!   through the SAME ladder as every other sub-anchor),
//! - §4.6 (the ONE 4-step ladder on the REAL CI sub-anchors — a tombstone ALWAYS carries the root).
//!
//! Reconciliation `00-reconciliation-decisions.md` X-1 (the Git↔CI CheckStatus seam — CI is the
//! producer half; Refs resolves the `check-`/`step-` sub-anchors), C-6 (the `check-`/`step-` kinds).
//!
//! **Contracts (consumer side, to the FROZEN shapes; the Refs engine is UNCHANGED — fixed at M2):**
//! - **5.9** — the Git↔CI `CheckStatus` seam, CI's producer half closes (X-1). CI emits
//!   `ci.check.updated` per `(commit_oid, context)` with monotonic `run_attempt` supersession
//!   (`myelin_events::check_seam` carries it; `myelin_git::check_status` decodes + supersedes it);
//!   the `details_ref = #step-<n>` jump-to-failure resolves through the Refs ladder + the 11.8
//!   sealed CI log segments (`myelin_storage::CiLogTier`). **Refs' role is the SUB-ANCHOR resolution
//!   of `check-<context>` / `step-<n>`** — the seam itself (out-of-order supersession,
//!   fork-success-neutral, the merge-queue wake) is the Git+CI X-1 deliverable (GIT-D10/CI-D8); Refs
//!   proves only that the check/step anchors resolve correctly through the ONE ladder.
//! - **5.7** — the `check-`/`step-` kinds on REAL CI sub-anchors (the `Sub::Check`/`Sub::Step` mints
//!   the REF-P1 codec already froze) resolve through the ONE [`crate::ladder`].
//! - **11.8 (CONSUMED)** — the sealed CI log segments + the `(job, step, byte-range)` index a
//!   `#step-<n>` `details_ref` resolves through ([`myelin_storage::CiLogTier::resolve_step_anchor`]).
//!
//! ## Why this is a CONSUMER module, not a new engine (EI-01 §7 coherence — the engine is UNCHANGED)
//! Exactly like the REF-P17 [`crate::git_producer`] and the REF-P18 [`crate::kn_producer`], REF-P19's
//! deliverable is to WIRE Refs to the REAL CI producer half and RE-CONFIRM the ladder invariants on
//! the CI sub-anchors — NOT to build a second resolver / supersession rule / log index. So this
//! module:
//! - reuses [`crate::resolve::ResolveService`] + [`crate::ladder`] for resolution (the ONE chokepoint),
//! - reuses the FROZEN [`myelin_events::check_seam::CheckSeamOrder`] for the per-aggregate ordering
//!   the supersession rests on (the Bus guarantees the order; it does NOT evaluate the rule),
//! - reuses the FROZEN [`myelin_git::check_status::CheckStatusProjection`] for the monotonic
//!   `run_attempt` supersession (the merge-gate truth's owner — Refs does NOT re-derive it),
//! - reuses the FROZEN [`myelin_storage::CiLogTier`] for the 11.8 `#step-<n>` jump-to-failure
//!   resolution through the sealed log segments (the heaviest log consumer's index — Refs does NOT
//!   re-build it).
//!
//! It adds ONLY the Refs-specific glue: the [`CiOwner`] ([`crate::ProjectApi`] +
//! [`crate::SubAnchorResolver`]) that maps a CI `#sub` KIND through the ONE Refs grammar onto the
//! frozen [`SubState`]:
//! - a `check-<context>` anchor → the CURRENT [`CheckState`] off the supersession projection (the
//!   latest by `run_attempt`), lowered onto the ladder (`success`→LIVE, an in-flight/queued→OUTDATED,
//!   a superseded/pruned check→GONE),
//! - a `step-<n>` anchor → LIVE iff the 11.8 [`CiLogTier`] resolves the `details_ref` to the exact
//!   failing step's bytes; an unknown step → GONE (the root run still resolves; the embed shows the
//!   parent run), a crypto-shredded segment → ERASED.
//!
//! No Refs type is re-defined; no parallel second ladder, supersession rule, or log index is minted.
//!
//! ## Out-of-order supersession honoured AT THE SUB-ANCHOR LEVEL (the GATE)
//! An out-of-order `ci.check.updated` (a higher `run_attempt` arriving before a lower one, or a stale
//! lower attempt re-delivered late) resolves the `check-<context>` sub-anchor to the LATEST by
//! `run_attempt` — never the physically-last-arrived. [`CiOwner::ingest_check`] applies each decoded
//! fact through the [`CheckStatusProjection`] supersession rule, so the sub-anchor's CURRENT state is
//! deterministic regardless of arrival order (the Bus's per-aggregate order + Git's monotonic rule,
//! consumed — not re-invented). This is the Refs half of the X-1 monotonic supersession.
//!
//! ## Floors named (VISION §3 / EI-01 §1 — the prompt's named floors)
//! - **No new Refs floor.** The Refs engine is FIXED at M2 (the ladder, the grammar, the chokepoint);
//!   this prompt adds ONLY the CI sub-anchor resolution wiring. The Issues `issue_relation` second
//!   mirror is REF-P20; Chat unfurls are REF-P21 (named so this consumer is not mistaken for them).
//! - **The seam's NON-Refs half is Git+CI (GIT-D10/CI-D8).** The merge gate, the fork-success-neutral
//!   gating, the `approve_untrusted_ci` endorsement, and the merge-queue durable wake are the X-1
//!   Git+CI deliverable — Refs resolves only that the `check-`/`step-` anchors point at the right
//!   state. Named so this module is not mistaken for the closed merge gate.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use myelin_events::ArtifactRef;
use myelin_git::check_status::{
    ApplyOutcome, CheckState, CheckStatus, CheckStatusProjection, CheckStatusRow,
};
use myelin_identity::{Decision, Permission, Principal};
use myelin_refs::{sub_kind, Sub};
use myelin_tenancy::{Region, TenantId};

use crate::ladder::SubState;
use crate::resolve::{OwnerProjection, ProjectApi, ProjectApiError, ProjectOutcome, ResolveMode};
use crate::SubAnchorResolver;

/// The canonical CI subsystem token — re-exported so a CI-anchor drill asserts against the ONE token
/// CI owns, never a literal. (CI's subsystem prefix on every `ci.*` token + every `ci/...` URN.)
pub const CI_OWNER_TOKEN: &str = "ci";

/// **A `step-<n>` `details_ref` resolution seam — the 11.8 sealed-CI-log-segment lookup the Refs
/// ladder consults.** Refs does NOT re-build the `(job, step, byte-range)` index (that is the
/// [`myelin_storage::CiLogTier`], P-328 / 11.8 C2); it CONSULTS it through this seam so a `#step-<n>`
/// jump-to-failure resolves to a LIVE / GONE / ERASED state in the ONE ladder vocabulary.
///
/// The production wire is `CiLogTier::resolve_step_anchor` over the resilient client (Refs never reads
/// CI's store directly); the test feeds a recorded [`StepResolution`]. `Send + Sync` so a
/// [`crate::resolve::ResolveService`] can hold the owner behind an `Arc`.
pub trait StepAnchorResolver: Send + Sync {
    /// Resolve a `#step-<n>` `details_ref` (`myelin://<tenant>/ci/run/<run>#step-<n>`) through the
    /// 11.8 sealed CI log segments → its [`StepResolution`]. Called ONLY on the permission-allowed +
    /// root-present branch (the chokepoint gates it).
    fn resolve_step(&self, anchor: &ArtifactRef) -> StepResolution;
}

/// **The resolution of a `#step-<n>` `details_ref` through the 11.8 sealed CI log segments.** The
/// jump-to-failure either resolves to the exact failing step's bytes (LIVE), points at a step the
/// index never saw / a pruned run (GONE — the root run still resolves, the embed shows the parent),
/// or hits a crypto-shredded segment (ERASED — the per-subject/per-tenant DEK was destroyed). The ONE
/// ladder vocabulary, instantiated for the CI log seam.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepResolution {
    /// The `#step-<n>` resolves to the exact failing step's bytes — LIVE. Carries the resolved
    /// byte-length so the projection reflects the jump-to-failure target (PII-free: a length, never
    /// the log body).
    Live { byte_len: u64 },
    /// The step the index never saw / a pruned run — GONE (`Tombstone{ sub_gone, root }`; the root
    /// run still resolves, the embed shows the parent run, never a hard 404).
    Gone,
    /// The CI log segment was crypto-shredded (the per-subject / per-tenant DEK destroyed) — ERASED.
    Erased,
}

impl StepResolution {
    /// Map the 11.8 step-resolution onto the frozen §4.6 [`SubState`], carrying the owner projection on
    /// the LIVE arm. The ONE vocabulary — a CI `#step-<n>` degrades identically to a Git line-range / a
    /// KN block / a Chat message.
    fn into_sub_state(self, mut projection: OwnerProjection) -> SubState {
        match self {
            StepResolution::Live { byte_len } => {
                projection.state = format!("failing-step ({byte_len} bytes)");
                SubState::Live(projection)
            }
            StepResolution::Gone => SubState::Gone,
            StepResolution::Erased => SubState::Erased,
        }
    }
}

/// Map a CURRENT [`CheckState`] (off the supersession projection) onto the frozen §4.6 [`SubState`] —
/// the ONE ladder mapping for a `check-<context>` sub-anchor (C-6). The `success`/terminal states
/// render LIVE; an in-flight `queued`/`in_progress` is OUTDATED (the check is still settling — the
/// embed shows it as not-final, never a hard fail); a check the projection no longer carries (a
/// pruned/superseded-away context) is GONE.
fn check_state_into_sub_state(state: CheckState, projection: OwnerProjection) -> SubState {
    match state {
        // A terminal check (success/failure/error/neutral/cancelled) is LIVE — the anchor renders the
        // current verdict. (Refs resolves only that the anchor points at the right STATE; whether a
        // `failure` blocks the merge gate is Git's decision, not Refs' — GIT-D10.)
        CheckState::Success
        | CheckState::Failure
        | CheckState::Error
        | CheckState::Neutral
        | CheckState::Cancelled => SubState::Live(projection),
        // An in-flight check is OUTDATED — the anchor renders but flags "not yet final" (the check is
        // still settling). The embed shows the parent commit's check as in-progress, never a hard 404.
        CheckState::Queued | CheckState::InProgress => SubState::Outdated(projection),
    }
}

/// **The REAL CI owner — a [`ProjectApi`] + [`SubAnchorResolver`] over the CI producer half (contracts
/// 5.9 / 5.7 / 11.8).** This is the producer half Refs' chokepoint calls when resolving a CI `#sub`:
///
/// - a `check-<context>` anchor → the CURRENT [`CheckState`] off the [`CheckStatusProjection`] (the
///   latest by monotonic `run_attempt` — out-of-order supersession honoured AT THE SUB-ANCHOR LEVEL),
///   lowered onto the ONE ladder ([`check_state_into_sub_state`]),
/// - a `step-<n>` `details_ref` → the 11.8 sealed-CI-log-segment resolution ([`StepAnchorResolver`]),
///   lowered onto the ONE ladder,
/// - a bare root (no `#sub`) → LIVE (the run / check artifact itself).
///
/// Refs NEVER reads CI's DB — it only consults these seams (the supersession projection it feeds from
/// the decoded `ci.check.updated` facts; the [`StepAnchorResolver`] over the resilient client). The
/// leak invariant is the chokepoint's: this owner is reached ONLY on the permission-allowed branch.
///
/// Cloneable: every map is held behind an [`Arc`] so a clone shares the SAME recorded state (the
/// resolve chokepoint holds the owner behind an `Arc<dyn ProjectApi>`; a clone lets the test feed the
/// same owner the service resolves through). Tenant-first; no cross-tenant key.
#[derive(Clone)]
pub struct CiOwner {
    /// `(tenant|region|principal|root)` → the authoritative `view` decision (4.2). The recorded ACL the
    /// production wire replaces with Identity's `check` over the resilient client. Default-deny.
    acl: Arc<Mutex<BTreeMap<String, Decision>>>,
    /// `check-<context>` full-ref-urn → the CURRENT [`CheckState`] off the supersession projection. Fed
    /// by [`Self::ingest_check`] (which applies the monotonic `run_attempt` rule), so the recorded
    /// state is ALWAYS the latest-by-attempt — never the physically-last-arrived.
    checks: Arc<Mutex<BTreeMap<String, CheckState>>>,
    /// The shared monotonic `run_attempt` supersession projection (the FROZEN Git-owned rule —
    /// REUSED, never re-derived). One current row per `(commit_oid, context)`. Feeding it is how the
    /// out-of-order supersession is honoured at the sub-anchor level.
    projection: Arc<Mutex<CheckStatusProjection>>,
    /// The 11.8 sealed-CI-log-segment resolver a `#step-<n>` `details_ref` consults — held behind an
    /// `Arc<dyn>` so the production `CiLogTier` (over the resilient client) and a test recorder share
    /// the SAME seam. `None` until wired (a `#step-<n>` then resolves GONE — defensive, never a leak).
    step_resolver: Arc<Mutex<Option<Arc<dyn StepAnchorResolver>>>>,
}

impl Default for CiOwner {
    fn default() -> Self {
        CiOwner {
            acl: Arc::new(Mutex::new(BTreeMap::new())),
            checks: Arc::new(Mutex::new(BTreeMap::new())),
            projection: Arc::new(Mutex::new(CheckStatusProjection::new())),
            step_resolver: Arc::new(Mutex::new(None)),
        }
    }
}

impl CiOwner {
    /// A fresh CI owner (default-deny ACL; no checks ingested; no step resolver wired). Everything
    /// resolves through the fed state — an unscripted bare root is LIVE.
    pub fn new() -> CiOwner {
        CiOwner::default()
    }

    /// The canonical CI **check root** `myelin://<tenant>/ci/check/<commit>` — the root a
    /// `check-<context>` sub-anchor hangs off (the commit-anchored check artifact). Refs stores the
    /// full sub-URN + this `#sub`-stripped root (§3.2), so the tombstone always carries it (§4.6).
    pub fn check_root(tenant: &str, commit: &str) -> ArtifactRef {
        ArtifactRef(format!("myelin://{tenant}/ci/check/{commit}"))
    }

    /// The canonical CI `check-<context>` **sub-anchor ref**
    /// `myelin://<tenant>/ci/check/<commit>#check-<context>` — the first-class `#sub` kind (C-6) Refs
    /// resolves through the ONE ladder. Built so a drill names the anchor against the codec, never a
    /// literal.
    pub fn check_anchor(tenant: &str, commit: &str, context: &str) -> ArtifactRef {
        ArtifactRef(format!(
            "myelin://{tenant}/ci/check/{commit}#check-{context}"
        ))
    }

    /// The canonical CI **run root** `myelin://<tenant>/ci/run/<run>` — the root a `step-<n>`
    /// `details_ref` jump-to-failure hangs off (the producing run). A `step-<n>` tombstone carries it.
    pub fn run_root(tenant: &str, run: &str) -> ArtifactRef {
        ArtifactRef(format!("myelin://{tenant}/ci/run/{run}"))
    }

    /// The canonical CI `step-<n>` **`details_ref` anchor** `myelin://<tenant>/ci/run/<run>#step-<n>`
    /// — the X-1 jump-to-failure sub-anchor (OQ-D) Refs resolves through the 11.8 sealed log segments.
    pub fn step_anchor(tenant: &str, run: &str, step_no: u32) -> ArtifactRef {
        ArtifactRef(format!("myelin://{tenant}/ci/run/{run}#step-{step_no}"))
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

    /// Grant a viewer the `view` permission on a CI root (the recorded ACL — the CI ReBAC fragment's
    /// `repo->pull`-equivalent grant, modelled here; production is Identity's `check`).
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

    /// **Ingest a decoded `ci.check.updated` fact (the REAL CI producer half, 5.9) through the
    /// monotonic `run_attempt` supersession rule — the Refs sub-anchor's CURRENT-state feed.** Applies
    /// the fact through the FROZEN [`CheckStatusProjection`] (Git's monotonic rule, REUSED), then
    /// records the CURRENT (latest-by-attempt) state for the `check-<context>` sub-anchor `ref_` so the
    /// next resolve sees the latest verdict — NEVER the physically-last-arrived. Returns the
    /// [`ApplyOutcome`] (loud: `Superseded` if it became current, `DroppedStale` if a late lower
    /// attempt was dropped) so a drill asserts the out-of-order supersession at the sub-anchor level.
    ///
    /// `ref_` is the `check-<context>` sub-anchor URN the fact's `(commit_oid, context)` projects onto
    /// (the caller builds it via [`Self::check_anchor`] — Refs stores the full sub-URN, §3.2). The fact
    /// is the CI-owned struct (`myelin_git::check_status::CheckStatus`, decoded from the opaque
    /// `ci.check.updated` payload the Bus carries).
    pub fn ingest_check(&self, ref_: &ArtifactRef, fact: &CheckStatus) -> ApplyOutcome {
        let outcome = self.projection.lock().unwrap().apply(fact);
        // Record the CURRENT (post-supersession) state for the sub-anchor — read back from the
        // projection's current row, so a dropped-stale fact does NOT regress the recorded state.
        let key = fact.key();
        if let Some(row) = self.projection.lock().unwrap().current(&key) {
            self.checks
                .lock()
                .unwrap()
                .insert(ref_.0.clone(), row.state);
        }
        outcome
    }

    /// The CURRENT projection row for a `(commit_oid, context)` key — exposed so a drill reads the
    /// latest-by-attempt verdict directly (the supersession high-water mark) without resolving.
    pub fn current_row(&self, fact: &CheckStatus) -> Option<CheckStatusRow> {
        self.projection
            .lock()
            .unwrap()
            .current(&fact.key())
            .cloned()
    }

    /// Wire the 11.8 sealed-CI-log-segment resolver a `#step-<n>` `details_ref` consults (the
    /// production `CiLogTier` over the resilient client; a test feeds a recorder). One source of truth
    /// for the jump-to-failure resolution — Refs does NOT re-build the `(job, step, byte-range)` index.
    pub fn wire_step_resolver(&self, resolver: Arc<dyn StepAnchorResolver>) {
        *self.step_resolver.lock().unwrap() = Some(resolver);
    }

    /// The default owner projection a renderable CI anchor carries (a render-safe title — the leak
    /// invariant already gates this; the owner is reached only on the allowed branch). PII-free.
    fn projection(ref_: &ArtifactRef) -> OwnerProjection {
        OwnerProjection {
            title: "a CI check".into(),
            state: "live".into(),
            icon: "ci".into(),
            render_hint: "embed".into(),
            sub_anchor: sub_kind(ref_).is_some().then(|| ref_.0.clone()),
            flag: None,
        }
    }

    /// Resolve a CI `#sub` anchor on `ref_` into the frozen [`SubState`] (the §4.6 step-3 owner
    /// resolver — the SUB-ANCHOR RESOLUTION REF-P19 ships). Dispatched by the CI `#sub` KIND through the
    /// ONE Refs grammar (`sub_kind`):
    /// - `check-<context>` → the CURRENT [`CheckState`] off the supersession projection (latest by
    ///   `run_attempt`), lowered onto the ladder,
    /// - `step-<n>` → the 11.8 sealed-CI-log-segment resolution (the `details_ref` jump-to-failure),
    /// - bare root → LIVE.
    fn resolve_ci_sub(&self, ref_: &ArtifactRef, sub: Option<&Sub>) -> SubState {
        let projection = Self::projection(ref_);
        match sub {
            // A bare root (no #sub) is LIVE — the run / check artifact itself.
            None => SubState::Live(projection),
            // A `check-<context>` anchor — the CURRENT (latest-by-attempt) state off the supersession
            // projection. A check the projection never saw resolves LIVE-default (the producer half
            // hasn't reported it yet — never a hard fail; an in-flight check is the OUTDATED arm once
            // reported). Resolved through the SAME ladder (C-6 — first-class `#sub` kind).
            Some(Sub::Check(_)) => {
                let state = self
                    .checks
                    .lock()
                    .unwrap()
                    .get(&ref_.0)
                    .copied()
                    .unwrap_or(CheckState::InProgress);
                check_state_into_sub_state(state, projection)
            }
            // A `step-<n>` `details_ref` jump-to-failure — resolved through the 11.8 sealed CI log
            // segments. No resolver wired → GONE (defensive; the root run still resolves, the embed
            // shows the parent run — never a leak, never a hard 404).
            Some(Sub::Step(_)) => match self.step_resolver.lock().unwrap().as_ref() {
                Some(resolver) => resolver.resolve_step(ref_).into_sub_state(projection),
                None => SubState::Gone,
            },
            // Any other kind on a CI ref is not a CI-owned mint — CI renders the bare root LIVE rather
            // than guess (REF-3 — never guess scope; the grammar already rejected an unknown).
            Some(_) => SubState::Live(projection),
        }
    }
}

impl ProjectApi for CiOwner {
    fn check_view(
        &self,
        tenant: &TenantId,
        region: &Region,
        object: &ArtifactRef,
        viewer: &Principal,
        _permission: &Permission,
    ) -> std::result::Result<Decision, ProjectApiError> {
        // The authoritative `view` verdict on the ROOT (the chokepoint passes the #sub-stripped root).
        // Default-deny: an unrecorded grant is a Deny (so a viewer with no CI read sees a tombstone,
        // never the check/step state leaked — the leak invariant on the CI corpus).
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
        // The owner classifies the CI #sub into the frozen ProjectOutcome through the ONE ladder
        // mapping (SubState::into_outcome). Called ONLY on the permission-allowed branch.
        let sub = sub_kind(ref_);
        Ok(self.resolve_ci_sub(ref_, sub.as_ref()).into_outcome())
    }
}

impl SubAnchorResolver for CiOwner {
    /// The §4.6 step-3 sub-anchor resolver (the SAME logic `project` runs) — exposed so a drill can
    /// drive the ladder directly (REF-D9) through [`crate::resolve_sub_outcome`] without the full
    /// chokepoint. ONE source of truth: it delegates to [`CiOwner::resolve_ci_sub`].
    fn resolve_sub(&self, ref_: &ArtifactRef, sub: Option<&Sub>) -> SubState {
        self.resolve_ci_sub(ref_, sub)
    }
}

#[cfg(test)]
mod tests;

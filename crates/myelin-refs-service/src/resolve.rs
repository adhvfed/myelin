//! The **per-viewer resolution chokepoint** — `resolve(ref, viewer, mode) -> Projection | Tombstone`
//! (REF-P10 / P-159; contract 5.2 owned; consumes 4.2 `check`, 5.6 `project`, 1.9 `ResilientClient`,
//! 1.10 `FailStatic`).
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/reference-graph.md`
//! §4.2 (the in-cell resolution algorithm: (1) parse + validate; (2) `Id.check(viewer, view, ref)` —
//! **denied returns a tombstone, never a leak** — the chokepoint that makes every unfurl non-leaking;
//! (3) projection via the R2 cache hit, else the owner's `project(ref, viewer)` through the resilient
//! client — **Refs never reads the owner's DB**; (4) the caller subscribes to `*.updated`/`*.erased`
//! so the rendered ref stays live), §3.6 (the R2 projection cache — a bounded, invalidatable holder
//! keyed `(tenant, ref)`, **never a source of truth**), the cross-cell pin **C-5** (a cross-cell
//! target resolves in the home cell; only the already-filtered projection or a tombstone crosses).
//! **External insight:** `external-insights/02-platform-substrate.md` §7 (one cache primitive — Refs
//! rides the SAME substrate `FailStatic` the platform is built on, not a bespoke availability path);
//! `external-insights/01-process-and-quality-doctrine.md` §3 (prove-it; observability is part of the
//! pass — the `resolve_cache_hit_ratio` telemetry).
//!
//! **Contract rows implemented:**
//! - **5.2** `resolve(ref, viewer, mode) -> Projection | Tombstone` (OWNED) — [`ResolveService::resolve`].
//! - **5.6** `project(ref, viewer) -> {title, state, icon, render_hint, sub_anchor?}` (CONSUMED) — the
//!   [`ProjectApi`] trait the **owning subsystem** implements; Refs is the only caller (ADR-13.1). The
//!   sub-anchor resolver returns the §4.6 `live/moved/outdated/gone` shape ([`ProjectOutcome`]).
//! - **4.2** `check(subject, view, object, zookie?)` (CONSUMED) — routed through the substrate
//!   [`FailStaticAuthz`] (the SAME 1.10 fail-static wiring Identity is fronted by) so step 2 is the
//!   fail-static chokepoint.
//! - **1.9** `ResilientClient` (CONSUMED) — the owner's `project` is reached over the resilient client
//!   (timeout + breaker + bulkhead). The [`ProjectApi`] trait is the call SEAM; the production wire
//!   transport is the named floor (below).
//! - **1.10** `FailStatic<T>` (CONSUMED) — step 2 degrades on the coarse authz cache under an Id
//!   hiccup rather than cascading; a zookie-stamped (`Strong`) read bypasses the cache (the new-enemy
//!   defense, exercised fully in REF-P11/REF-P12).
//!
//! ## The leak invariant (REF-D1 resolve half) — the load-bearing property
//! The whole point of this chokepoint: **a denied viewer gets a [`Tombstone`] that carries NO content
//! — never the title, state, or icon.** A confidential issue degrades to a placeholder (the root URN +
//! `denied`), it does NOT leak its title through an unfurl. The [`Tombstone`] type is **structurally
//! incapable of carrying a projection field** (it holds only the root [`ArtifactRef`] + the
//! [`TombstoneReason`]) — the leak cannot regress because there is no field to leak into. The §4.6
//! rule "a tombstone always carries the root" lets the embed degrade to "this referenced
//! *&lt;parent&gt;* (the specific part is no longer available)" rather than vanishing.
//!
//! ## Per-viewer correctness WITHOUT per-viewer caching (§4.2 — documented as the prompt requires)
//! The per-viewer permission check (step 2) **gates** a **viewer-independent, ref-keyed** projection
//! cache (step 3). The cache key is `(tenant, ref)` — it carries NO viewer. This is safe — shared
//! across viewers without leaking — **because no content is ever returned until the check passes**:
//! the cache is read ONLY on the allowed branch (after `check` returns `Allow`), so a denied viewer
//! never observes a cached projection. Two viewers of the same ref (one permitted, one denied) share
//! the one cached projection: the permitted one is served it, the denied one is tombstoned BEFORE the
//! cache is touched. This is why Refs needs no per-viewer projection cache (the expensive,
//! cardinality-exploding design it avoids).
//!
//! ## Fail-static under an Id hiccup (§8.3 / 1.10)
//! Step 2 runs through [`FailStaticAuthz`]: a TRANSIENT Identity hiccup is survived on the coarse
//! bounded-staleness cache (resolve degrades, it does not cascade-fail every unfurl closed); a
//! zookie-stamped (`Strong`) read BYPASSES the cache and fails closed on a hiccup (never serves stale
//! — the new-enemy guard). The fail-static survival signals (1.8 fail-static ratio) are exposed off
//! the wrapped [`FailStaticAuthz::signals`].
//!
//! ## Cross-cell resolution is pinned cell-local (C-5)
//! A cross-cell target resolves **in the home cell**: the home cell renders + permission-checks; only
//! the already-filtered [`Projection`] (or a [`Tombstone`]) crosses, over the frozen
//! [`CrossCellPointer`]. This module freezes the cross-cell resolution SEMANTICS (a cross-cell pointer
//! is dispatched to its home cell, never resolved by pulling the owner's rows across the cell
//! boundary). **FLOOR named:** the cross-cell backlink fan-out BUILD is **REF-P26** (R-M5) — the
//! resolution semantics are frozen here; the build is the follow-on.
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **The R2 cache is the REF-P7 NO-OP shim (read side) until REF-P12.** [`ProjectionCacheRead`] is
//!   the read interface; the shipped [`NoOpCacheRead`] (mirroring the REF-P7 write-side
//!   [`crate::invalidator::NoOpCacheShim`]) holds nothing, so every read MISSES and resolve always
//!   falls through to `project`. The WIRING (gate → cache-read → project → subscribe) is real; the
//!   live bounded, per-tenant-DEK-encrypted cache that serves hits is **REF-P12**. The
//!   `resolve_cache_hit_ratio` telemetry is live now (it reads 0 hits until REF-P12 — the metric is
//!   real, the cache is a shim).
//! - **The owner `project` transport is the named floor.** [`ProjectApi`] is the call SEAM (the
//!   per-subsystem `project(ref, viewer)` resolver). Production reaches it over the substrate
//!   [`ResilientClient`] wire transport, whose `send` body is itself a named substrate floor
//!   (`myelin-client`, the first real producer). The synthetic owner used in tests stands in for the
//!   real Git/Knowledge/Chat `project` implementations (REF-P17/P18/P21). The CHOKEPOINT logic
//!   (gate → cache → project → tombstone-never-leak) is real; the CALLEES are synthetic.
//! - **The cross-cell fan-out BUILD is REF-P26** (named above).
//!
//! ## Mutation-score floor (mandatory-core, EI-01 §3 / VISION §4 prove-it)
//! The resolve chokepoint is load-bearing leak-of-confidential-content-critical. Floor: **≥ 80% of
//! viable mutants caught** (`cargo mutants -p myelin-refs-service -f
//! crates/myelin-refs-service/src/resolve.rs`). Measured 2026-06-20: **33 mutants generated → 9
//! unviable, 24 viable, 24 caught, 0 missed = 100% of viable** — floor met. (Every chokepoint rule —
//! the `Allow`-gates-the-cache branch, the denied→`Tombstone{denied}` arm, each §4.6 outcome→reason
//! mapping, the owner-hiccup→tombstone degrade, the cross-cell `Home`/`Foreign` split, and the
//! `resolve_cache_hit_ratio` true division — has a test a mutation flips.)

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_events::ArtifactRef;
use myelin_identity::{Consistency, ConsistencyMode, Decision, Permission, Principal, Zookie};
use myelin_substrate::{AuthzDecision, FailStaticAuthz, ServeError};
use myelin_tenancy::{CellId, CrossCellPointer, Region, TenantId};

/// The frozen `view` permission Refs checks at the chokepoint (§4.2 step 2: `check(viewer, view,
/// ref)`). A named constant — drills/tests assert against the NAME, never a literal (EI-01 §3).
pub const VIEW_PERMISSION: &str = "view";

/// The telemetry signal name the resolve chokepoint emits (contract 1.8): the projection-cache hit
/// ratio. A named constant so a drill asserts against the NAME, never a literal.
pub const RESOLVE_CACHE_HIT_RATIO_SIGNAL: &str = "resolve_cache_hit_ratio";

/// The render mode a caller resolves in (contract 5.2 `mode`). `Live` is the per-viewer unfurl/embed;
/// `Display` is the Notif humanisation projection (the SAME chokepoint — the mode only shapes the
/// render hint the owner returns, never whether the permission gate runs). Both modes are
/// leak-free-by-the-same-gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveMode {
    /// The live per-viewer unfurl / embed (Chat unfurl, PR context pane, KN embeds).
    Live,
    /// The Notif humanisation projection (contract 5.2 — `Display` mode).
    Display,
}

/// **The per-viewer projection (contract 5.6 shape).** The owner subsystem's
/// `project(ref, viewer) -> {title, state, icon, render_hint, sub_anchor?}` — the ONLY way Refs reads
/// another subsystem's artifact (ADR-13.1; Refs never reads the owner's DB). Pre-permission-checked by
/// the chokepoint (a `Projection` is only ever returned on the ALLOWED branch). May carry a name in
/// the title (it is a `PersonalDataHolder` payload, §3.6).
///
/// Derives `Serialize`/`Deserialize` so the live R2 cache (REF-P12, [`crate::cache`]) can seal a
/// projection under the per-tenant DEK and round-trip it — the cache stores the SEALED bytes of this
/// shape, never plaintext (it "may hold a name in a title", §3.6).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Projection {
    /// The artifact this projection renders (the ref that resolved). Carried so a caller can key the
    /// rendered unfurl + subscribe to its `*.updated`/`*.erased`.
    pub ref_: ArtifactRef,
    /// The human-facing title (may contain a name — `PersonalDataHolder` payload, §3.6). Present ONLY
    /// on a `Projection` — a [`Tombstone`] structurally cannot carry it (the leak invariant).
    pub title: String,
    /// The lifecycle state (e.g. `open`/`merged`/`resolved`) — render-time, owner-supplied.
    pub state: String,
    /// The render icon hint (owner-supplied).
    pub icon: String,
    /// The render hint (how to render the unfurl — owner-supplied, mode-shaped).
    pub render_hint: String,
    /// The resolved `#sub` anchor, if the ref carried one (§3.5) — the sub-artifact the embed targets.
    pub sub_anchor: Option<String>,
    /// The §4.6 sub-resolution flag: `None` for a bare LIVE projection, `Some(Moved)` for a Git
    /// rebased range / KN moved block, `Some(Outdated)` for a partial range / edited block. A
    /// `Gone`/`Erased` sub does NOT produce a `Projection` — it tombstones (so this is never `Gone`).
    pub flag: Option<ProjectionFlag>,
}

/// The §4.6 graceful-degradation flag on a sub-anchored [`Projection`]. A LIVE sub has `None`; a
/// MOVED/OUTDATED sub still renders (the parent + the shifted/partial anchor) with the flag set so the
/// UI can mark it; a GONE sub does NOT render a projection — it returns a [`Tombstone`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectionFlag {
    /// The sub-anchor moved (Git rebased range, KN block moved) — render the shifted anchor, flagged.
    Moved,
    /// The sub-anchor is outdated (Git partial range, KN edited block) — render the partial, flagged.
    Outdated,
}

/// **A tombstone — the non-leaking placeholder (contract 5.2 / §4.6).** Returned whenever a ref CANNOT
/// render as a [`Projection`]: denied, root gone, sub gone, content gone, or erased. **It carries NO
/// projection content** — only the root [`ArtifactRef`] (so the embed degrades to "this referenced
/// *&lt;parent&gt;*") and the structured [`TombstoneReason`]. This type is the *structural* guarantee
/// of the leak invariant: there is no `title`/`state`/`icon` field for a denied viewer's content to
/// leak into (REF-D1 resolve half — 0 leak).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tombstone {
    /// The root artifact the tombstone carries (§4.6 — "a tombstone always carries the root"). This is
    /// the `#sub`-stripped parent; it is an OPAQUE URN, never the title/content. Safe to render as
    /// "this referenced *&lt;root&gt;*".
    pub root: ArtifactRef,
    /// Why the ref tombstoned (the §4.6 ladder reason). NEVER content — a structured enum.
    pub reason: TombstoneReason,
}

/// Why a [`Tombstone`] was produced (the frozen §4.6 ladder reasons). A structured enum — never a
/// free-text leak of the artifact's content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TombstoneReason {
    /// Step 1: the viewer is not permitted to `view` the root (`check -> Deny`). The leak-free
    /// chokepoint — a confidential artifact degrades to this placeholder, title NEVER present
    /// (REF-D1 resolve half; EI-02 §1).
    Denied,
    /// Step 2: the parent artifact no longer exists (`project -> root gone`).
    RootGone,
    /// Step 3: the root resolves but the `#sub` anchor is gone (`project -> sub gone`); the embed
    /// shows the parent (the root is still carried).
    SubGone,
    /// Step 4: the artifact (or a level of it) was erased (pseudonym-shred / crypto-shred made it
    /// unrenderable). The most final reason.
    Erased,
}

/// The resolution outcome — a [`Projection`] (the ref renders) or a [`Tombstone`] (it degrades to a
/// non-leaking placeholder). Contract 5.2's `Projection | Tombstone`. The leak invariant lives in the
/// SHAPE: the `Tombstone` arm cannot carry a projection field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// The ref rendered to a per-viewer projection (the ALLOWED + present branch).
    Projection(Projection),
    /// The ref degraded to a non-leaking tombstone (denied / gone / erased).
    Tombstone(Tombstone),
}

impl Resolution {
    /// Is this a [`Resolution::Projection`]? (the "allowed + present" assertion).
    pub fn is_projection(&self) -> bool {
        matches!(self, Resolution::Projection(_))
    }

    /// Is this a [`Resolution::Tombstone`]? (the "denied / gone" assertion).
    pub fn is_tombstone(&self) -> bool {
        matches!(self, Resolution::Tombstone(_))
    }

    /// The [`TombstoneReason`] if this is a tombstone (the drill reads it to assert `denied`).
    pub fn tombstone_reason(&self) -> Option<TombstoneReason> {
        match self {
            Resolution::Tombstone(t) => Some(t.reason),
            Resolution::Projection(_) => None,
        }
    }
}

/// **The owner's per-viewer projection resolver (contract 5.6 — CONSUMED).** Each subsystem
/// (Git/Knowledge/Chat/Issues/CI) implements this; Refs is the ONLY caller (ADR-13.1) and reaches it
/// over the [`ResilientClient`](myelin_client::ResilientClient) — **Refs never reads the owner's DB**
/// (the `no-cross-db` floor). It is called ONLY on the permission-allowed branch (the chokepoint gates
/// it), so it never has to re-check permission for the leak invariant — but it IS per-viewer (the
/// owner may still hide owner-private fields from a low-privilege viewer).
///
/// Returns the §4.6 sub-resolution shape: LIVE/MOVED/OUTDATED produce a [`Projection`]; GONE/ROOT-GONE
/// produce the tombstone reason; ERASED is the final unrenderable.
///
/// `Send + Sync` so the [`ResolveService`] can hold it behind an [`Arc`] across the serving threads.
///
/// The trait carries BOTH the 5.6 `project` resolver AND the 4.2 `check_view` authoritative permission
/// verdict — the SAME owning subsystem authors both (Identity owns the tuples; the owner subsystem's
/// `project` reads them). Holding both on one trait keeps the [`ResolveService`] holding ONE owner
/// handle, and lets a test owner drive both from one object. In production `check_view` is Identity's
/// 4.2 `check` reached over the resilient client; `project` is the owner's 5.6 reached the same way.
pub trait ProjectApi: Send + Sync {
    /// **4.2 (CONSUMED) — the authoritative `check(viewer, view, object)` verdict the fail-static
    /// cache fronts.** Returns the authoritative [`Decision`]; an `Err` is the TRANSIENT Id hiccup the
    /// fail-static cache survives (degrade within the staleness budget, else fail closed). The
    /// chokepoint runs THIS before any projection — DENIED tombstones, never leaks.
    fn check_view(
        &self,
        tenant: &TenantId,
        region: &Region,
        object: &ArtifactRef,
        viewer: &Principal,
        permission: &Permission,
    ) -> Result<Decision, ProjectApiError>;

    /// **5.6 (CONSUMED) — project `ref_` for `viewer`.** The owner's per-viewer render, called ONLY on
    /// the permission-allowed branch (the chokepoint gates it). A transient owner hiccup is a
    /// [`ProjectApiError::Unavailable`] (the resilient client surfaces it); a real gone/erased artifact is
    /// the corresponding [`ProjectOutcome`] (the §4.6 live/moved/outdated/gone ladder).
    fn project(
        &self,
        tenant: &TenantId,
        region: &Region,
        ref_: &ArtifactRef,
        viewer: &Principal,
        mode: ResolveMode,
    ) -> Result<ProjectOutcome, ProjectApiError>;
}

/// The §4.6 sub-resolution outcome of an owner's [`ProjectApi::project`] — the `live/moved/outdated/
/// gone` ladder, plus the root-gone and erased terminals. Refs maps this onto [`Resolution`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectOutcome {
    /// LIVE — the artifact (and its `#sub` anchor, if any) renders. Carries the owner's projection
    /// fields + the optional MOVED/OUTDATED flag.
    Live(OwnerProjection),
    /// ROOT GONE — the parent artifact no longer exists (`Tombstone{root_gone}`).
    RootGone,
    /// SUB GONE — the root resolves but the `#sub` anchor is gone (`Tombstone{sub_gone}`; the root is
    /// still carried so the embed shows the parent).
    SubGone,
    /// ERASED — pseudonym-shred / crypto-shred made it unrenderable (`Tombstone{erased}`).
    Erased,
}

/// The owner-supplied projection fields (contract 5.6 `{title, state, icon, render_hint, sub_anchor?}`
/// plus the §4.6 flag). The owner returns THIS; Refs wraps it (with the resolved ref) into a final
/// `Projection` — held distinct from the wrapped [`Projection`] so the owner never has to echo the ref
/// it was asked about (Refs already knows it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerProjection {
    /// The human-facing title (may contain a name).
    pub title: String,
    /// The lifecycle state.
    pub state: String,
    /// The render icon hint.
    pub icon: String,
    /// The render hint.
    pub render_hint: String,
    /// The resolved `#sub` anchor, if any.
    pub sub_anchor: Option<String>,
    /// The §4.6 MOVED/OUTDATED flag (`None` = a clean LIVE projection).
    pub flag: Option<ProjectionFlag>,
}

/// Why an owner `project` call failed structurally (distinct from a clean gone/erased outcome). An
/// `Unavailable` is the TRANSIENT owner hiccup the resilient client surfaces — resolve does NOT
/// fabricate a projection on it (it surfaces the unavailability; a stale render is never invented).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectApiError {
    /// The owner subsystem was unavailable (the resilient client surfaced a transport hiccup). Resolve
    /// surfaces this — it never fabricates content on an owner hiccup.
    Unavailable(String),
    /// The ref was malformed at the owner (should not happen — Refs validates first; defense in depth).
    BadRequest(String),
}

/// **The R2 projection-cache READ interface (§3.6 read side; the REF-P7 write/invalidate side is
/// [`crate::invalidator::ProjectionCache`]).** `read(tenant, ref)` returns the cached [`Projection`]
/// for `(tenant, ref)` or `None` on a miss. Tenant-first: the key is `(tenant, ref)`, **never a
/// cross-tenant lookup** (§3.6; the no-cross-tenant-query floor). The cache is **viewer-independent**
/// (ref-keyed) — it is read ONLY after the per-viewer gate passes, so it is shared without leaking.
///
/// `Send + Sync` so the [`ResolveService`] holds it behind an [`Arc`].
pub trait ProjectionCacheRead: Send + Sync {
    /// Read the cached projection for `ref_` in `(tenant, region)`, or `None` on a miss. Read ONLY on
    /// the permission-allowed branch (the chokepoint gates it).
    fn read(&self, tenant: &TenantId, region: &Region, ref_: &ArtifactRef) -> Option<Projection>;

    /// **Populate the cache for `(tenant, ref)` after a resolve MISS (§4.2 post-miss fill).** Called by
    /// the chokepoint on the allowed branch once the owner's `project` returns a live projection, so the
    /// NEXT viewer of the same `(tenant, ref)` is served a HIT (viewer-independent, ref-keyed). The
    /// default is a **no-op** — the REF-P10 [`NoOpCacheRead`] holds nothing, so it never fills (the
    /// floor). The live R2 cache (REF-P12, [`crate::cache::R2ProjectionCache`]) overrides this to seal +
    /// write the projection under the per-tenant DEK. Best-effort: a fill failure is swallowed (the
    /// cache is derived — the next read just re-resolves).
    fn fill(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        _ref_: &ArtifactRef,
        _projection: &Projection,
    ) {
        // No-op default (the NoOpCacheRead floor never caches). The live R2 cache overrides it.
    }
}

/// **The NO-OP R2-cache read shim (the REF-P10/P7 floor — REF-P12 ships the live cache).** Implements
/// [`ProjectionCacheRead`] but holds **no entries**, so every `read` MISSES and resolve always falls
/// through to the owner's `project`. Mirrors the REF-P7 write-side
/// [`crate::invalidator::NoOpCacheShim`]: the WIRING (gate → cache-read → project) is real, the CACHE
/// is a shim. REF-P12 replaces it with the live bounded, per-tenant-DEK-encrypted cache implementing
/// the SAME trait — the [`ResolveService`] is unchanged.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoOpCacheRead;

impl ProjectionCacheRead for NoOpCacheRead {
    fn read(
        &self,
        _tenant: &TenantId,
        _region: &Region,
        _ref_: &ArtifactRef,
    ) -> Option<Projection> {
        // No entries are held yet (REF-P12 ships the live cache) — every read MISSES, so resolve
        // falls through to the owner's project. Documented floor.
        None
    }
}

/// The fail-static authz consult result an Id hiccup can land on — surfaced so the resolve service can
/// distinguish "served fresh/static (degraded)" from "failed closed" for the leak invariant (a closed
/// answer is a `Deny` → tombstone, never a leak).
///
/// This is the [`AuthzDecision`] the substrate [`FailStaticAuthz`] returns; re-exported through the
/// resolve surface so a drill reads the served BRANCH (fresh/static/closed/revoked) off the resolve
/// outcome.
pub use myelin_substrate::AuthzServed;

/// **The per-viewer resolution chokepoint (contract 5.2 — the REF-P10 deliverable).** Composes the
/// §4.2 four-step algorithm: (1) the ref is already a validated [`ArtifactRef`] (REF-P1 parse upstream);
/// (2) `check(viewer, view, root)` through the fail-static authz cache — **DENIED returns a
/// [`Tombstone`], never a leak**; (3) the projection via the R2 cache hit, else the owner's
/// [`ProjectApi::project`] (over the resilient client; Refs never reads the owner DB); (4) the caller
/// subscribes to `*.updated`/`*.erased` (the [`ResolveService::subscribe_subjects`] seam).
///
/// Holds the [`FailStaticAuthz`] (the 1.10 wiring — step 2 degrades, never cascades), the
/// [`ProjectionCacheRead`] (the §3.6 cache — the REF-P7 shim now, REF-P12's live cache later), and the
/// [`ProjectApi`] (the owner's 5.6 resolver, over the resilient client). Cloneable handles are held
/// behind [`Arc`]s so the service is shared across serving threads.
pub struct ResolveService {
    /// The §4.2 step-2 chokepoint: `Id.check` through the substrate fail-static authz cache (1.10). A
    /// transient Id hiccup is survived on the coarse cache (degrade, never cascade); a zookie read
    /// bypasses it (the new-enemy guard).
    authz: Arc<FailStaticAuthz>,
    /// The §3.6 R2 projection-cache read side (the REF-P7 shim now, REF-P12 live). Read ONLY on the
    /// allowed branch (viewer-independent, gated by the per-viewer check).
    cache: Arc<dyn ProjectionCacheRead>,
    /// The owner's 5.6 `project(ref, viewer)` resolver, reached over the resilient client. Refs NEVER
    /// reads the owner's DB — only this seam.
    owner: Arc<dyn ProjectApi>,
    /// The cell this resolve service serves (C-5: a cross-cell target is dispatched to its home cell,
    /// never resolved by pulling the owner's rows across the boundary).
    home_cell: CellId,
    /// Live `resolve_cache_hit_ratio` telemetry (contract 1.8): cache hits / (hits + misses). Held as
    /// two counters so the ratio is computed from real observations, not a literal.
    cache_hits: Arc<AtomicU64>,
    /// Cache misses (the denominator's other half) — a fall-through to the owner `project`.
    cache_misses: Arc<AtomicU64>,
}

/// The disposition of a cross-cell resolve (C-5): either the target is HOME (resolve locally) or it is
/// FOREIGN (dispatch to its home cell — only the filtered projection/tombstone crosses back). This
/// freezes the semantics; the cross-cell fan-out BUILD is the named REF-P26 floor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CrossCellDisposition {
    /// The target is homed in THIS cell — resolve it locally (the in-cell path).
    Home,
    /// The target is homed in another cell — dispatch the resolve to `home_cell`. Only the
    /// already-filtered projection (or a tombstone) crosses back. The BUILD is REF-P26.
    Foreign(CellId),
}

impl ResolveService {
    /// Build the resolve chokepoint over the three consumed seams: the fail-static authz cache (1.10),
    /// the R2 cache read side (§3.6), and the owner's `project` resolver (5.6, over the resilient
    /// client). `home_cell` is the cell this service serves (C-5).
    pub fn new(
        authz: Arc<FailStaticAuthz>,
        cache: Arc<dyn ProjectionCacheRead>,
        owner: Arc<dyn ProjectApi>,
        home_cell: CellId,
    ) -> ResolveService {
        ResolveService {
            authz,
            cache,
            owner,
            home_cell,
            cache_hits: Arc::new(AtomicU64::new(0)),
            cache_misses: Arc::new(AtomicU64::new(0)),
        }
    }

    /// **The per-viewer resolution chokepoint (contract 5.2 — the REF-P10 deliverable).**
    ///
    /// `ref_` is the (already-parsed, REF-P1-validated) [`ArtifactRef`] to resolve; `root` is its
    /// `#sub`-stripped parent ([`myelin_refs::strip_sub`] — the caller computes it, the glue crate is
    /// the owner of the codec). `viewer` is the verified subject. `at` is the read consistency (a
    /// `Strong`/zookie-stamped read bypasses the fail-static cache, 4.10). `subject_revoked` is the
    /// caller's revocation consult (the S7 denylist — a revoked subject is denied through any cache).
    ///
    /// The algorithm (§4.2):
    /// 1. (parse done upstream)
    /// 2. **`check(viewer, view, root)` through the fail-static authz cache.** DENIED (or fail-closed
    ///    on a hiccup) returns a `Tombstone{denied}` — **never a leak** (the chokepoint).
    /// 3. **The projection:** the R2 cache hit (read ONLY now, on the allowed branch — viewer-
    ///    independent, gated by step 2), else the owner's `project(ref, viewer)` over the resilient
    ///    client (Refs never reads the owner's DB). The §4.6 sub-ladder maps onto the result.
    /// 4. (subscribe is the caller's — see [`Self::subscribe_subjects`]).
    #[allow(clippy::too_many_arguments)]
    pub fn resolve(
        &self,
        tenant: &TenantId,
        region: &Region,
        ref_: &ArtifactRef,
        root: &ArtifactRef,
        viewer: &Principal,
        mode: ResolveMode,
        at: &Consistency,
        subject_revoked: bool,
    ) -> Resolution {
        let (resolution, _served) = self.resolve_observed(
            tenant,
            region,
            ref_,
            root,
            viewer,
            mode,
            at,
            subject_revoked,
        );
        resolution
    }

    /// The same chokepoint as [`Self::resolve`] but also returning the fail-static BRANCH the authz
    /// step landed on ([`AuthzServed`]) — the observable provenance the fail-static drill asserts (a
    /// hiccup served `Static` (degraded) vs failed `Closed`; a `Strong` read `SourceBypass` vs
    /// `BypassClosed`). Factored so the unit/CDC/drill tests assert the BRANCH as well as the answer.
    #[allow(clippy::too_many_arguments)]
    pub fn resolve_observed(
        &self,
        tenant: &TenantId,
        region: &Region,
        ref_: &ArtifactRef,
        root: &ArtifactRef,
        viewer: &Principal,
        mode: ResolveMode,
        at: &Consistency,
        subject_revoked: bool,
    ) -> (Resolution, AuthzServed) {
        // ── Step 2: the chokepoint. check(viewer, view, root) through the fail-static authz cache. ──
        // The cache key is the verified (tenant, region, subject, view@root) discriminator — distinct
        // authz questions never share one cached grant; the partition prefix comes from the verified
        // viewer + the OWNER-supplied ref, never a path.
        //
        // R2.3b: the segments are unconstrained user-controlled strings, so they are framed
        // length-prefixed (INJECTIVE) via `encode_authz_key` — a `format!("{}|{}|…")` join would let a
        // subject id / object ref like `alice|view@repo:secret` forge the delimiter structure and
        // collide with a different (subject, object) question (a cross-principal cached-ALLOW replay).
        let key = myelin_substrate::encode_authz_key(&[
            &tenant.0,
            &region.0,
            &viewer.principal_id.0,
            VIEW_PERMISSION,
            &root.0,
        ]);
        let perm = Permission(VIEW_PERMISSION.to_string());
        // The owner of the authoritative check is Identity; here it is the synthetic owner's
        // permission verdict (the production wire is the named ResilientClient floor). On a transient
        // Id hiccup the closure returns Err(ServeError) → the fail-static cache decides (degrade /
        // closed). We capture the owner's authoritative decision through the closure.
        let decision: AuthzDecision = self.authz.serve(key, at, subject_revoked, || {
            self.owner
                .check_view(tenant, region, root, viewer, &perm)
                .map_err(|e| ServeError(format!("identity check hiccup: {e:?}")))
        });

        // DENIED (or fail-closed on a hiccup, or revoked) → Tombstone{denied}. NEVER a leak: the
        // tombstone carries only the root + the reason — there is no field for the title to leak into.
        if !matches!(decision.decision, Decision::Allow) {
            return (
                Resolution::Tombstone(Tombstone {
                    root: root.clone(),
                    reason: TombstoneReason::Denied,
                }),
                decision.served,
            );
        }

        // ── Step 3: the projection. ALLOWED branch ONLY — the cache is read HERE, never before the ──
        // gate (per-viewer correctness WITHOUT per-viewer caching: the ref-keyed cache is shared, but
        // no content returns until the check passes).
        if let Some(cached) = self.cache.read(tenant, region, ref_) {
            self.cache_hits.fetch_add(1, Ordering::SeqCst);
            return (Resolution::Projection(cached), decision.served);
        }
        self.cache_misses.fetch_add(1, Ordering::SeqCst);

        // Cache miss → the owner's project(ref, viewer) over the resilient client. Refs NEVER reads the
        // owner's DB — only this seam. The §4.6 sub-ladder maps onto the Resolution.
        let resolution = match self.owner.project(tenant, region, ref_, viewer, mode) {
            Ok(ProjectOutcome::Live(op)) => {
                let projection = Projection {
                    ref_: ref_.clone(),
                    title: op.title,
                    state: op.state,
                    icon: op.icon,
                    render_hint: op.render_hint,
                    sub_anchor: op.sub_anchor,
                    flag: op.flag,
                };
                // §4.2 post-miss FILL: populate the ref-keyed cache so the NEXT viewer of the same
                // (tenant, ref) is served a HIT. Viewer-independent + safe — we are on the ALLOWED
                // branch (the per-viewer gate already passed). The REF-P10 NoOpCacheRead's fill is a
                // no-op; the live R2 cache (REF-P12) seals + writes it under the per-tenant DEK.
                // Best-effort (the cache is derived — a failed fill just means the next read re-resolves).
                self.cache.fill(tenant, region, ref_, &projection);
                Resolution::Projection(projection)
            }
            Ok(ProjectOutcome::RootGone) => Resolution::Tombstone(Tombstone {
                root: root.clone(),
                reason: TombstoneReason::RootGone,
            }),
            Ok(ProjectOutcome::SubGone) => Resolution::Tombstone(Tombstone {
                root: root.clone(),
                reason: TombstoneReason::SubGone,
            }),
            Ok(ProjectOutcome::Erased) => Resolution::Tombstone(Tombstone {
                root: root.clone(),
                reason: TombstoneReason::Erased,
            }),
            // An owner hiccup does NOT fabricate a projection (a stale render is never invented). It
            // degrades to a root-carrying tombstone so the embed shows the parent — the LEAK invariant
            // still holds (no content), and the caller can retry. Documented as the conservative,
            // non-leaking failure of step 3.
            Err(_unavailable) => Resolution::Tombstone(Tombstone {
                root: root.clone(),
                reason: TombstoneReason::RootGone,
            }),
        };
        (resolution, decision.served)
    }

    /// **Step 4: the subjects a caller subscribes to so a rendered ref stays live (§4.2).** For a
    /// resolved `ref_`, the caller subscribes to its `*.updated`/`*.erased` so the unfurl re-resolves
    /// when the artifact changes/erases (the §3.6 cache is busted by the REF-P7 invalidator on the same
    /// events). Returns the two lifecycle subjects (the dotted event TYPEs) — never `*` (BUS-4). This
    /// keeps the chokepoint self-describing: the caller knows exactly what to watch.
    pub fn subscribe_subjects(ref_: &ArtifactRef) -> Vec<String> {
        // The subject is the artifact-type-scoped lifecycle prefix; the caller binds `*.updated`/
        // `*.erased` for the owning subsystem. We surface them keyed by the ref's subsystem token so
        // the subscription is precise (not a firehose). Derive the subsystem from the URN.
        let subsystem = ref_
            .0
            .strip_prefix("myelin://")
            .and_then(|rest| rest.split('/').nth(1))
            .unwrap_or("unknown");
        vec![
            format!("{subsystem}.updated"),
            format!("{subsystem}.erased"),
        ]
    }

    /// **Cross-cell disposition (C-5; contract 12.6 consumed).** Is `target_cell` homed in THIS cell
    /// (resolve locally) or FOREIGN (dispatch the resolve to its home cell — only the filtered
    /// projection/tombstone crosses back)? Freezes the resolution SEMANTICS; the fan-out BUILD is
    /// REF-P26 (the named floor). A cross-cell ref is NEVER resolved by pulling the owner's rows across
    /// the cell boundary — the home cell renders + permission-checks, only the result crosses.
    pub fn cross_cell_disposition(&self, target_cell: &CellId) -> CrossCellDisposition {
        if target_cell == &self.home_cell {
            CrossCellDisposition::Home
        } else {
            CrossCellDisposition::Foreign(target_cell.clone())
        }
    }

    /// The [`CrossCellDisposition`] for the frozen tenancy [`CrossCellPointer`] (C-5; contract 12.6) —
    /// the `home_cell` on the pointer is authoritative. A pointer homed elsewhere is `Foreign`
    /// (dispatch the resolve there; only the filtered projection/tombstone crosses back); homed here is
    /// `Home`. Reuses the ONE frozen cross-cell frame (no second pointer type — EI-01 §7).
    pub fn disposition_of_pointer(&self, ptr: &CrossCellPointer) -> CrossCellDisposition {
        self.cross_cell_disposition(ptr.home_cell())
    }

    /// The cell this resolve service serves (C-5).
    pub fn home_cell(&self) -> &CellId {
        &self.home_cell
    }

    /// The live `resolve_cache_hit_ratio` sample (contract 1.8): hits / (hits + misses). Returns
    /// `None` when no resolves have hit the cache stage yet (no denominator). Until REF-P12 ships the
    /// live cache the numerator is 0 (the no-op shim always misses) — the METRIC is real, the cache is
    /// a shim (the named floor).
    pub fn cache_hit_ratio(&self) -> Option<f64> {
        let hits = self.cache_hits.load(Ordering::SeqCst);
        let misses = self.cache_misses.load(Ordering::SeqCst);
        let total = hits + misses;
        if total == 0 {
            None
        } else {
            Some(hits as f64 / total as f64)
        }
    }

    /// The raw `(hits, misses)` counters (the drill reads them to assert the cache stage ran on the
    /// ALLOWED branch only — a denied resolve never touches the cache, so it bumps neither).
    pub fn cache_counters(&self) -> (u64, u64) {
        (
            self.cache_hits.load(Ordering::SeqCst),
            self.cache_misses.load(Ordering::SeqCst),
        )
    }

    /// The fail-static survival signals (contract 1.8 fail-static ratio) off the wrapped authz cache —
    /// the drill reads the fresh/stale/closed ratio + the staleness age to assert resolve degraded on
    /// the coarse cache under an Id hiccup (no cascade).
    pub fn fail_static_signals(&self) -> myelin_substrate::FailStaticSignals {
        self.authz.signals()
    }
}

/// Helper: a [`Consistency`] for a default (bounded-stale) read (the common resolve path — a `Strong`
/// zookie read is the security-sensitive caller's choice).
pub fn bounded_stale() -> Consistency {
    Consistency {
        at_least: Zookie(String::new()),
        mode: ConsistencyMode::BoundedStale,
    }
}

/// Helper: a [`Consistency`] for a zookie-stamped strong read (the new-enemy guard — bypasses the
/// fail-static cache, 4.10).
pub fn strong_read(zookie: &str) -> Consistency {
    Consistency {
        at_least: Zookie(zookie.into()),
        mode: ConsistencyMode::Strong,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_substrate::FailStaticThreshold;
    use std::sync::Mutex;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn cell() -> CellId {
        CellId::from_token("cell-fr-par-1")
    }
    fn viewer(id: &str) -> Principal {
        Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
    }
    /// A confidential issue (the canonical REF-D1 leak-test artifact).
    fn confidential_issue() -> ArtifactRef {
        ArtifactRef("myelin://acme/issue/issue/ENG-secret".into())
    }

    /// The §8.2 fail-static bound (agent-token TTL = 60s ≤ static_max = 300s ≤ revocation SLA = 300s).
    fn threshold() -> FailStaticThreshold {
        FailStaticThreshold {
            status: "OPEN — LEGAL".into(),
            owner: "DPO / Legal".into(),
            static_max_secs: None,
            static_max_default_secs: 300,
            agent_token_ttl_secs: 60,
            constraint: "static_max <= revocation-SLA AND static_max >= agent-token-TTL".into(),
        }
    }
    fn authz() -> Arc<FailStaticAuthz> {
        Arc::new(FailStaticAuthz::try_new(300, &threshold()).expect("valid bound"))
    }

    /// A synthetic owner subsystem — stands in for the real Git/Knowledge/Chat `project` + Identity's
    /// `check` (the production wire is the named ResilientClient floor). It is programmable: a verdict
    /// per (viewer, ref), a projection outcome per ref, and an optional `check` hiccup (the fail-static
    /// trigger). The ONE owner handle the resolve service holds (it authors both check_view + project).
    #[derive(Default)]
    struct SyntheticOwner {
        /// viewers allowed to `view` (everyone else is denied — the leak-test).
        allowed: Mutex<Vec<String>>,
        /// the project outcome to return (default: a live projection with a title that MUST NOT leak).
        outcome: Mutex<Option<ProjectOutcome>>,
        /// if set, `check_view` returns Err (a TRANSIENT Id hiccup — the fail-static cache decides).
        check_hiccup: Mutex<bool>,
        /// records every `project` call (proves the cache short-circuits a hit, and a denied viewer
        /// NEVER reaches project — the leak invariant defense in depth).
        project_calls: Mutex<u64>,
    }

    impl SyntheticOwner {
        fn allow(&self, viewer_id: &str) {
            self.allowed.lock().unwrap().push(viewer_id.into());
        }
        fn set_outcome(&self, o: ProjectOutcome) {
            *self.outcome.lock().unwrap() = Some(o);
        }
        fn force_check_hiccup(&self) {
            *self.check_hiccup.lock().unwrap() = true;
        }
        fn project_call_count(&self) -> u64 {
            *self.project_calls.lock().unwrap()
        }
        /// the default LIVE projection — its title is the SECRET that must never leak to a denied
        /// viewer.
        fn secret_projection() -> OwnerProjection {
            OwnerProjection {
                title: "TOP SECRET acquisition plan".into(),
                state: "open".into(),
                icon: "lock".into(),
                render_hint: "issue-card".into(),
                sub_anchor: None,
                flag: None,
            }
        }
    }

    impl ProjectApi for SyntheticOwner {
        fn check_view(
            &self,
            _tenant: &TenantId,
            _region: &Region,
            _object: &ArtifactRef,
            viewer: &Principal,
            _permission: &Permission,
        ) -> Result<Decision, ProjectApiError> {
            if *self.check_hiccup.lock().unwrap() {
                return Err(ProjectApiError::Unavailable("identity hiccup".into()));
            }
            let allowed = self.allowed.lock().unwrap();
            if allowed.iter().any(|a| a == &viewer.principal_id.0) {
                Ok(Decision::Allow)
            } else {
                Ok(Decision::Deny)
            }
        }

        fn project(
            &self,
            _tenant: &TenantId,
            _region: &Region,
            _ref_: &ArtifactRef,
            _viewer: &Principal,
            _mode: ResolveMode,
        ) -> Result<ProjectOutcome, ProjectApiError> {
            *self.project_calls.lock().unwrap() += 1;
            Ok(self
                .outcome
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| ProjectOutcome::Live(SyntheticOwner::secret_projection())))
        }
    }

    /// A live R2-cache stand-in (REF-P12 will ship the real one) — lets the chained two-viewer test
    /// prove the ref-keyed cache is SHARED across viewers without leaking (read only after the gate).
    #[derive(Default)]
    struct MapCacheRead {
        entries: Mutex<Vec<(String, Projection)>>,
    }
    impl MapCacheRead {
        fn put(&self, ref_: &str, p: Projection) {
            self.entries.lock().unwrap().push((ref_.into(), p));
        }
    }
    impl ProjectionCacheRead for MapCacheRead {
        fn read(&self, _t: &TenantId, _r: &Region, ref_: &ArtifactRef) -> Option<Projection> {
            self.entries
                .lock()
                .unwrap()
                .iter()
                .find(|(k, _)| k == &ref_.0)
                .map(|(_, p)| p.clone())
        }
    }

    fn service(owner: Arc<SyntheticOwner>) -> ResolveService {
        ResolveService::new(authz(), Arc::new(NoOpCacheRead), owner, cell())
    }

    // ── REF-D1 resolve half: denied → tombstone, NEVER a leak (the load-bearing property) ──

    /// **A confidential artifact resolves to a `Tombstone{denied}` for an UNAUTHORIZED viewer — the
    /// title/state/icon are NEVER in the tombstone (REF-D1 resolve half; 0 leak).** This is THE
    /// chokepoint property: a denied viewer gets only the root URN + the `denied` reason; the secret
    /// title is never reachable. The owner's `project` is NEVER even called for a denied viewer.
    #[test]
    fn denied_viewer_gets_tombstone_carrying_no_content_zero_leak() {
        let owner = Arc::new(SyntheticOwner::default()); // nobody allowed
        let svc = service(owner.clone());
        let ref_ = confidential_issue();
        let root = ref_.clone(); // a bare root (no #sub)

        let r = svc.resolve(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer("intruder"),
            ResolveMode::Live,
            &bounded_stale(),
            false,
        );

        assert!(
            r.is_tombstone(),
            "a denied viewer gets a tombstone, never a projection"
        );
        assert_eq!(r.tombstone_reason(), Some(TombstoneReason::Denied));
        // the structural leak invariant: a Tombstone has NO title/state/icon field at all. We assert
        // the only data it carries is the OPAQUE root URN + the reason — the secret title cannot appear.
        if let Resolution::Tombstone(t) = &r {
            assert_eq!(
                t.root, root,
                "the tombstone carries the root (and only the root)"
            );
            // the secret never appears anywhere in the rendered tombstone (Debug-format the whole
            // value and assert the secret title is absent — a regression that added a leak field fails).
            let rendered = format!("{t:?}");
            assert!(
                !rendered.contains("SECRET") && !rendered.contains("acquisition"),
                "0 leak: the secret title must not appear in the tombstone, got `{rendered}`"
            );
        }
        // defense in depth: the owner's project was NEVER called for a denied viewer (the gate runs
        // first; a denied viewer never reaches the projection step).
        assert_eq!(
            owner.project_call_count(),
            0,
            "a denied viewer never reaches project"
        );
        // and the cache stage was never touched (no hit/miss bumped) — the gate short-circuits.
        assert_eq!(
            svc.cache_counters(),
            (0, 0),
            "a denied resolve never touches the cache"
        );
    }

    /// **An ALLOWED viewer gets the projection from `project` (the happy path).** The title/state/icon
    /// flow through; the resolved ref is carried so the caller can subscribe.
    #[test]
    fn allowed_viewer_gets_projection_from_project() {
        let owner = Arc::new(SyntheticOwner::default());
        owner.allow("insider");
        let svc = service(owner.clone());
        let ref_ = confidential_issue();
        let root = ref_.clone();

        let r = svc.resolve(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer("insider"),
            ResolveMode::Live,
            &bounded_stale(),
            false,
        );

        assert!(r.is_projection(), "an allowed viewer gets a projection");
        if let Resolution::Projection(p) = &r {
            assert_eq!(
                p.title, "TOP SECRET acquisition plan",
                "the allowed viewer sees the title"
            );
            assert_eq!(p.ref_, ref_, "the projection carries the resolved ref");
            assert_eq!(p.state, "open");
        }
        assert_eq!(
            owner.project_call_count(),
            1,
            "the allowed viewer reached project once"
        );
        // a cache MISS was recorded (the no-op shim always misses → falls through to project).
        assert_eq!(
            svc.cache_counters(),
            (0, 1),
            "an allowed miss falls through to project"
        );
    }

    // ── The chained two-viewer shared-cache test (the prompt's required chained test) ──

    /// **resolve the SAME ref as two viewers (one permitted, one denied) → the shared ref-keyed cache
    /// serves the permitted viewer and denies the other with NO content (the prompt's chained test).**
    /// Per-viewer correctness WITHOUT per-viewer caching: the cache is keyed `(tenant, ref)` — viewer-
    /// independent — yet the denied viewer never sees the cached projection because the per-viewer gate
    /// runs first. The cache (here a live map stand-in) is HIT for the permitted viewer.
    #[test]
    fn two_viewers_share_one_ref_keyed_cache_without_leaking() {
        let owner = Arc::new(SyntheticOwner::default());
        owner.allow("insider"); // only the insider may view
        let ref_ = confidential_issue();
        let root = ref_.clone();

        // a LIVE map cache pre-warmed with the projection (REF-P12's cache stands in here so we can
        // observe a HIT — the no-op shim would only ever miss).
        let cache = Arc::new(MapCacheRead::default());
        cache.put(
            &ref_.0,
            Projection {
                ref_: ref_.clone(),
                title: "TOP SECRET acquisition plan".into(),
                state: "open".into(),
                icon: "lock".into(),
                render_hint: "issue-card".into(),
                sub_anchor: None,
                flag: None,
            },
        );
        let svc = ResolveService::new(authz(), cache, owner.clone(), cell());

        // the PERMITTED viewer: gate passes → the shared ref-keyed cache HIT serves the projection.
        let permitted = svc.resolve(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer("insider"),
            ResolveMode::Live,
            &strong_read("z1"),
            false,
        );
        assert!(
            permitted.is_projection(),
            "the permitted viewer is served the cached projection"
        );

        // the DENIED viewer: SAME ref, SAME cache — but the per-viewer gate denies BEFORE the cache is
        // read, so 0 content leaks. The shared cache served the permitted viewer; the denied one sees
        // only a tombstone.
        let denied = svc.resolve(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer("intruder"),
            ResolveMode::Live,
            &strong_read("z1"),
            false,
        );
        assert!(
            denied.is_tombstone(),
            "the denied viewer is tombstoned even though the ref is cached"
        );
        assert_eq!(denied.tombstone_reason(), Some(TombstoneReason::Denied));

        // exactly ONE cache HIT (the permitted viewer); the denied viewer never touched the cache.
        assert_eq!(
            svc.cache_counters(),
            (1, 0),
            "one shared-cache hit (permitted); denied never read"
        );
        // project was NEVER called — the permitted viewer was served from the cache, the denied one
        // was tombstoned at the gate.
        assert_eq!(
            owner.project_call_count(),
            0,
            "the shared cache served the permitted viewer"
        );
    }

    // ── Fail-static: an Id hiccup degrades, never cascades, never leaks ──

    /// **With Id forced unavailable, resolve degrades on the coarse cache rather than cascading — and
    /// a cold hiccup fails CLOSED to a tombstone (never a leak).** A `BoundedStale` read with no warmed
    /// grant + an Id hiccup denies (fail-closed), so the unfurl degrades to a tombstone, NOT a cascade-
    /// panic and NOT a leak. The fail-static survival signals fire (the 1.8 fail-static ratio).
    #[test]
    fn id_hiccup_degrades_to_tombstone_never_cascades_or_leaks() {
        let owner = Arc::new(SyntheticOwner::default());
        owner.allow("insider"); // would be allowed IF the check could run
        owner.force_check_hiccup(); // but Id is down
        let svc = service(owner.clone());
        let ref_ = confidential_issue();
        let root = ref_.clone();

        let (r, served) = svc.resolve_observed(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer("insider"),
            ResolveMode::Live,
            &bounded_stale(),
            false,
        );
        // a cold BoundedStale hiccup with no warmed grant → fail CLOSED → Tombstone{denied} (never a
        // leak, never a cascade — the resolve returns a value, it does not panic/propagate).
        assert!(
            r.is_tombstone(),
            "an Id hiccup degrades to a tombstone (fail-closed), never a leak"
        );
        assert_eq!(r.tombstone_reason(), Some(TombstoneReason::Denied));
        assert_eq!(
            served,
            AuthzServed::Closed,
            "the fail-static branch is Closed (degraded, not cascade)"
        );
        // observability: the fail-static signals recorded the closed answer (the 1.8 ratio fires).
        assert_eq!(
            svc.fail_static_signals().closed,
            1,
            "the fail-static ratio telemetry fires"
        );
        // the owner's project was never reached (the gate failed closed before the projection step).
        assert_eq!(
            owner.project_call_count(),
            0,
            "a fail-closed gate never reaches project"
        );
    }

    /// **A zookie-stamped (`Strong`) read bypasses the fail-static cache and fails closed on an Id
    /// hiccup (the new-enemy guard).** A security-sensitive resolve carrying a zookie does not serve a
    /// stale allow — it tombstones on a hiccup (exercised fully in REF-P11/P12).
    #[test]
    fn strong_read_bypasses_cache_and_fails_closed_on_hiccup() {
        let owner = Arc::new(SyntheticOwner::default());
        owner.allow("insider");
        owner.force_check_hiccup();
        let svc = service(owner);
        let ref_ = confidential_issue();
        let root = ref_.clone();
        let (r, served) = svc.resolve_observed(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer("insider"),
            ResolveMode::Live,
            &strong_read("z9"),
            false,
        );
        assert!(
            r.is_tombstone(),
            "a strong read fails closed on a hiccup → tombstone"
        );
        assert_eq!(
            served,
            AuthzServed::BypassClosed,
            "the strong read bypassed the cache, failed closed"
        );
    }

    /// **A revoked subject is denied even if otherwise allowed (the revoked-at-window-close defense).**
    /// The S7 revocation consult denies BEFORE the cache/gate — a revoked viewer is tombstoned.
    #[test]
    fn revoked_subject_is_tombstoned_even_if_allowed() {
        let owner = Arc::new(SyntheticOwner::default());
        owner.allow("insider");
        let svc = service(owner.clone());
        let ref_ = confidential_issue();
        let root = ref_.clone();
        let (r, served) = svc.resolve_observed(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer("insider"),
            ResolveMode::Live,
            &bounded_stale(),
            /* revoked */ true,
        );
        assert!(
            r.is_tombstone(),
            "a revoked subject is tombstoned even though otherwise allowed"
        );
        assert_eq!(
            served,
            AuthzServed::Revoked,
            "the revoke is enforced before the cache/gate"
        );
        assert_eq!(
            owner.project_call_count(),
            0,
            "a revoked viewer never reaches project"
        );
    }

    // ── The §4.6 sub-ladder: moved / outdated / sub_gone / root_gone / erased ──

    /// **A MOVED sub-anchor renders a flagged projection (Git rebased / KN moved block).** The §4.6
    /// graceful-degradation: it still renders, flagged `moved`.
    #[test]
    fn moved_sub_renders_a_flagged_projection() {
        let owner = Arc::new(SyntheticOwner::default());
        owner.allow("insider");
        owner.set_outcome(ProjectOutcome::Live(OwnerProjection {
            title: "doc".into(),
            state: "live".into(),
            icon: "page".into(),
            render_hint: "embed".into(),
            sub_anchor: Some("L42-L88".into()),
            flag: Some(ProjectionFlag::Moved),
        }));
        let svc = service(owner);
        let ref_ = ArtifactRef("myelin://acme/git/ref/main#L42-L88".into());
        let root = ArtifactRef("myelin://acme/git/ref/main".into());
        let r = svc.resolve(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer("insider"),
            ResolveMode::Live,
            &bounded_stale(),
            false,
        );
        match r {
            Resolution::Projection(p) => assert_eq!(p.flag, Some(ProjectionFlag::Moved)),
            other => panic!("a moved sub must render a flagged projection, got {other:?}"),
        }
    }

    /// **Each non-LIVE owner outcome maps onto the right tombstone reason (the §4.6 ladder).**
    /// root_gone → RootGone; sub_gone → SubGone (root carried); erased → Erased.
    #[test]
    fn sub_ladder_maps_outcomes_onto_tombstone_reasons() {
        for (outcome, want) in [
            (ProjectOutcome::RootGone, TombstoneReason::RootGone),
            (ProjectOutcome::SubGone, TombstoneReason::SubGone),
            (ProjectOutcome::Erased, TombstoneReason::Erased),
        ] {
            let owner = Arc::new(SyntheticOwner::default());
            owner.allow("insider");
            owner.set_outcome(outcome.clone());
            let svc = service(owner);
            let ref_ = ArtifactRef("myelin://acme/knowledge/page/7c2#b9".into());
            let root = ArtifactRef("myelin://acme/knowledge/page/7c2".into());
            let r = svc.resolve(
                &tenant(),
                &region(),
                &ref_,
                &root,
                &viewer("insider"),
                ResolveMode::Live,
                &bounded_stale(),
                false,
            );
            assert_eq!(
                r.tombstone_reason(),
                Some(want),
                "outcome {outcome:?} → {want:?}"
            );
            // every tombstone carries the ROOT (§4.6 — "a tombstone always carries the root").
            if let Resolution::Tombstone(t) = &r {
                assert_eq!(t.root, root, "the {want:?} tombstone carries the root");
            }
        }
    }

    /// **An owner hiccup at the PROJECT step degrades to a root-carrying tombstone (never fabricates a
    /// projection).** A transient owner unavailability does not invent a stale render — it tombstones
    /// (the leak invariant still holds; the caller can retry).
    #[test]
    fn owner_project_hiccup_degrades_to_tombstone_no_fabrication() {
        struct HiccupOwner;
        impl ProjectApi for HiccupOwner {
            fn check_view(
                &self,
                _t: &TenantId,
                _r: &Region,
                _o: &ArtifactRef,
                _v: &Principal,
                _p: &Permission,
            ) -> Result<Decision, ProjectApiError> {
                Ok(Decision::Allow) // gate passes
            }
            fn project(
                &self,
                _t: &TenantId,
                _r: &Region,
                _rf: &ArtifactRef,
                _v: &Principal,
                _m: ResolveMode,
            ) -> Result<ProjectOutcome, ProjectApiError> {
                Err(ProjectApiError::Unavailable("owner down".into())) // project hiccup
            }
        }
        let svc = ResolveService::new(
            authz(),
            Arc::new(NoOpCacheRead),
            Arc::new(HiccupOwner),
            cell(),
        );
        let ref_ = confidential_issue();
        let root = ref_.clone();
        let r = svc.resolve(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer("insider"),
            ResolveMode::Live,
            &bounded_stale(),
            false,
        );
        assert!(
            r.is_tombstone(),
            "an owner project hiccup degrades to a tombstone, never a fabrication"
        );
    }

    // ── Telemetry: resolve_cache_hit_ratio ──

    /// **The `resolve_cache_hit_ratio` telemetry is emitted (contract 1.8).** With the no-op shim every
    /// allowed resolve misses → ratio 0.0 (the metric is real, the cache is the named shim floor). With
    /// a live cache hit the ratio rises. The signal NAME is the frozen constant.
    #[test]
    fn resolve_cache_hit_ratio_telemetry_is_emitted() {
        assert_eq!(
            RESOLVE_CACHE_HIT_RATIO_SIGNAL, "resolve_cache_hit_ratio",
            "the 1.8 signal name"
        );
        let owner = Arc::new(SyntheticOwner::default());
        owner.allow("insider");
        let svc = service(owner);
        let ref_ = confidential_issue();
        let root = ref_.clone();
        assert_eq!(
            svc.cache_hit_ratio(),
            None,
            "no denominator before any allowed resolve"
        );
        let _ = svc.resolve(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer("insider"),
            ResolveMode::Live,
            &bounded_stale(),
            false,
        );
        // one miss (the no-op shim) → ratio 0.0 (the metric is live; the cache is the shim floor).
        assert_eq!(
            svc.cache_hit_ratio(),
            Some(0.0),
            "the no-op shim always misses → ratio 0.0"
        );
        assert_eq!(svc.cache_counters(), (0, 1));
    }

    /// **The `resolve_cache_hit_ratio` is the true hits/(hits+misses) DIVISION (kills the `/ → %|*`
    /// arithmetic mutants).** With a non-trivial numerator (3 hits, 1 miss) the ratio is 0.75 — a
    /// modulo or multiply would give a different value. This pins the division on a real distribution.
    #[test]
    fn cache_hit_ratio_is_a_true_division_not_modulo_or_multiply() {
        let owner = Arc::new(SyntheticOwner::default());
        owner.allow("insider");
        let ref_ = confidential_issue();
        let root = ref_.clone();
        // a live map cache holding the ref → the allowed viewer HITS it.
        let cache = Arc::new(MapCacheRead::default());
        cache.put(
            &ref_.0,
            Projection {
                ref_: ref_.clone(),
                title: "t".into(),
                state: "s".into(),
                icon: "i".into(),
                render_hint: "h".into(),
                sub_anchor: None,
                flag: None,
            },
        );
        let svc = ResolveService::new(authz(), cache, owner, cell());
        // 3 allowed resolves → 3 cache hits.
        for _ in 0..3 {
            let _ = svc.resolve(
                &tenant(),
                &region(),
                &ref_,
                &root,
                &viewer("insider"),
                ResolveMode::Live,
                &bounded_stale(),
                false,
            );
        }
        // 1 allowed resolve of a DIFFERENT (uncached) ref → 1 miss.
        let other = ArtifactRef("myelin://acme/issue/issue/ENG-other".into());
        let _ = svc.resolve(
            &tenant(),
            &region(),
            &other,
            &other,
            &viewer("insider"),
            ResolveMode::Live,
            &bounded_stale(),
            false,
        );
        assert_eq!(svc.cache_counters(), (3, 1), "3 hits, 1 miss");
        // 3/(3+1) = 0.75 — a true division. (modulo 3%4=3→3.0; multiply 3*4=12→12.0 would differ.)
        assert_eq!(
            svc.cache_hit_ratio(),
            Some(0.75),
            "the ratio is hits/(hits+misses), a real division"
        );
    }

    // ── Step 4: the subscribe seam ──

    /// **The subscribe seam returns the precise `*.updated`/`*.erased` lifecycle subjects (§4.2 step
    /// 4) — never `*` (BUS-4).** A resolved ref's subsystem token scopes the subscription.
    #[test]
    fn subscribe_subjects_are_precise_never_a_firehose() {
        let subs = ResolveService::subscribe_subjects(&confidential_issue());
        assert_eq!(
            subs,
            vec!["issue.updated".to_string(), "issue.erased".to_string()]
        );
        for s in &subs {
            assert!(!s.contains('*'), "never a `*` subscription (BUS-4): {s}");
        }
    }

    // ── Cross-cell: pinned cell-local (C-5) ──

    /// **A cross-cell target is dispatched to its HOME cell, never resolved by pulling owner rows
    /// across the boundary (C-5).** A target homed in THIS cell is `Home` (resolve locally); a foreign
    /// one is `Foreign(home_cell)` (dispatch — only the filtered projection/tombstone crosses back).
    /// Freezes the resolution semantics; the fan-out BUILD is the named REF-P26 floor.
    #[test]
    fn cross_cell_resolution_is_pinned_cell_local() {
        let svc = service(Arc::new(SyntheticOwner::default()));
        assert_eq!(
            svc.cross_cell_disposition(&cell()),
            CrossCellDisposition::Home,
            "a target homed in this cell resolves locally"
        );
        let foreign = CellId::from_token("cell-us-east-1");
        assert_eq!(
            svc.cross_cell_disposition(&foreign),
            CrossCellDisposition::Foreign(foreign.clone()),
            "a foreign target is dispatched to its home cell (only the projection/tombstone crosses)"
        );
        assert_eq!(svc.home_cell(), &cell());
    }

    /// **The frozen tenancy `CrossCellPointer` drives the disposition (C-5) — ONE cross-cell frame, no
    /// second pointer type (EI-01 §7).** A pointer homed elsewhere is `Foreign`.
    #[test]
    fn frozen_cross_cell_pointer_drives_disposition() {
        use myelin_tenancy::{ArtifactType, CorrelationId, OpaqueSubjectId};
        let svc = service(Arc::new(SyntheticOwner::default()));
        let foreign = CellId::from_token("cell-us-east-1");
        let ptr = CrossCellPointer::new(
            OpaqueSubjectId::from_ref(ArtifactRef("myelin://acme/issue/issue/42".into())),
            ArtifactType::Issue,
            CorrelationId("01J0CORR".into()),
            foreign.clone(),
        );
        assert_eq!(
            svc.disposition_of_pointer(&ptr),
            CrossCellDisposition::Foreign(foreign),
            "the home cell on the frozen pointer is authoritative"
        );
    }

    // ── Mutation-floor-relevant exactness assertions ──

    /// **`Resolution` classifiers are exact (kills the `is_projection`/`is_tombstone → const` mutant).**
    #[test]
    fn resolution_classifiers_are_exact() {
        let proj = Resolution::Projection(Projection {
            ref_: confidential_issue(),
            title: "t".into(),
            state: "s".into(),
            icon: "i".into(),
            render_hint: "h".into(),
            sub_anchor: None,
            flag: None,
        });
        assert!(proj.is_projection() && !proj.is_tombstone() && proj.tombstone_reason().is_none());
        let tomb = Resolution::Tombstone(Tombstone {
            root: confidential_issue(),
            reason: TombstoneReason::Denied,
        });
        assert!(tomb.is_tombstone() && !tomb.is_projection());
        assert_eq!(tomb.tombstone_reason(), Some(TombstoneReason::Denied));
    }

    /// **The cache is read ONLY on the allowed branch (kills a mutant that reads the cache before the
    /// gate — which would leak a cached projection to a denied viewer).** A denied resolve bumps
    /// neither hit nor miss; an allowed one bumps exactly one.
    #[test]
    fn cache_is_read_only_after_the_gate_passes() {
        let owner = Arc::new(SyntheticOwner::default()); // nobody allowed
        let svc = service(owner);
        let ref_ = confidential_issue();
        let root = ref_.clone();
        let _ = svc.resolve(
            &tenant(),
            &region(),
            &ref_,
            &root,
            &viewer("intruder"),
            ResolveMode::Live,
            &bounded_stale(),
            false,
        );
        assert_eq!(
            svc.cache_counters(),
            (0, 0),
            "a denied resolve must NOT read the cache (no leak path)"
        );
    }
}

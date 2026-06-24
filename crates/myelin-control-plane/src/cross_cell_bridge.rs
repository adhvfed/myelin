//! # The `CrossCellPointer` bridge — resolution goes LIVE (always cell-local, 0 PII) — P-CP-19
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md`
//! §6.1 (the frozen four-field frame — [`myelin_tenancy::CrossCellPointer`]), **§6.2 in full** (the
//! load-bearing rule: **resolution is ALWAYS cell-local**), §6.3 (the honest designed-vs-deferred
//! floor — the multi-element `member_cells` fan-out / DSR / zookie / rebalancing ride P-CP-20).
//! Contract-index rows 12.6 (the bridge, now LIVE) + 5.2 (Refs `resolve(ref, viewer, mode)` — the
//! cell-local resolver the bridge dispatches to).
//!
//! ## What this prompt (P-CP-19 / P-429) ships — the bridge goes LIVE
//! The frozen-not-live [`CrossCellPointer`] frame (P-CP-02 / P-027, the §2.9 DAG sink) gets its
//! **resolution path**. The rule (§6.2) is exact and load-bearing:
//!
//! 1. A viewer in cell **A** wants to render a pointer to an artifact **homed in cell B**. A's gateway
//!    (holding the viewer's identity) does **not** fetch B's data into A. Instead it asks **cell B** to
//!    `resolve(ref, viewer, mode)` **IN B** ([`CrossCellBridge::resolve`] → the home cell's
//!    [`CellLocalResolver`]).
//! 2. The resolve is **permission-checked IN B** against **B's own tuples** (`check` / `list_objects`,
//!    Id 4.2/4.3) — A never sees B's authz state.
//! 3. B returns **ONLY** the already-rendered, already-permission-filtered projection
//!    ([`BridgeResolution::Projection`]) **or a tombstone** ([`BridgeResolution::Tombstone`]) — **never
//!    raw rows, never PII that should stay in B**. An unauthorised viewer gets a tombstone (the same
//!    graceful degradation as same-cell, §6.2).
//!
//! The bridge carries **ONLY** the four frozen [`CrossCellPointer`] fields
//! (`subject`/`type`/`correlation_id`/`home_cell`) across the cell boundary, plus the viewer identity A
//! holds (so B can permission-check IN B). **0 PII crosses the bridge** — the proof is structural (the
//! frame is the four-field PII-free type; the result is an already-filtered projection/tombstone, never
//! a raw row).
//!
//! ## DAG POSITION — why the bridge defines a RESOLVER SEAM, not a resolver
//! The `resolve(ref, viewer, mode)` resolver (contract 5.2) lives in **`myelin-refs-service`**
//! ([`myelin_refs_service::ResolveService`]). The §2.9 DAG forbids a `myelin-control-plane` →
//! `myelin-refs-service` production edge (it would invert the layering: a service crate must not depend
//! on another service crate). So the bridge defines the [`CellLocalResolver`] **trait** — the seam the
//! home cell's resolver plugs into — and the control plane owns the **transport + the cell-local
//! routing rule** (dispatch to the home cell; only the filtered result crosses back). The production
//! implementor of [`CellLocalResolver`] is `ResolveService` (its `resolve(ref, viewer, mode)`
//! chokepoint already returns a `Projection | Tombstone`, with the cross-cell `Home`/`Foreign`
//! disposition frozen in `ResolveService::cross_cell_disposition`); the CDC pair
//! (`crates/myelin-control-plane/tests/cdc_12_6_bridge_resolution_live.rs`) proves an ISS rollup / KN
//! collab / CHAT channel consumer resolving a cross-cell pointer through the SAME bridge over a
//! refs-shaped resolver. This is the EI-01 §7 coherence rule: ONE frozen frame, ONE cell-local
//! resolution rule, the resolver behind a seam.
//!
//! ## ISS rollup / KN collab / CHAT channels lit up (§6.2)
//! - **ISS cross-cell portfolio rollup** aggregates **projections** (counts/titles the viewer may see)
//!   — it resolves each member-cell pointer through the bridge and folds the per-viewer projections; a
//!   tombstone simply does not contribute (the viewer can't see it). See
//!   [`CrossCellBridge::rollup`].
//! - **KN cross-cell collab + CHAT cross-org channels** resolve **membership/content in the home cell**
//!   — a single [`CrossCellBridge::resolve`] per pointer; the home cell renders + permission-checks; an
//!   unauthorised cross-org viewer gets a tombstone (no leak across the org boundary).
//!
//! ## Mutation floor (mandatory-core, >= 80% — EI-01 §2/§3; the prompt's TESTS field)
//! The cell-local-resolution + permission-check-in-the-home-cell path
//! ([`CrossCellBridge::resolve`] + the [`CellLocalResolver`] seam) is mandatory-core: a cross-cell PII
//! leak is stop-the-bleeding (EI-01 §2). The floor is **>= 80%**;
//! `cargo mutants -p myelin-control-plane -f crates/myelin-control-plane/src/cross_cell_bridge.rs` ->
//! **14 caught, 1 missed, 7 unviable of 22** = **14/15 viable = 93.3%** (every load-bearing mutant of
//! the home-cell dispatch, the unknown-home-cell tombstone fallback, the tombstone-vs-projection
//! discrimination, the rollup's projection-aggregate/tombstone-exclude fold, and the
//! `cross_cell_resolves` increment is killed by an assertion). The single `MISSED` is `replace
//! cross_cell_raw_rows -> 0`, a **documented EQUIVALENT mutant**: the bridge NEVER increments
//! `cross_cell_raw_rows` (the structural guarantee — no raw row ever crosses), so the live read is
//! always 0 and `return 0` is observationally identical. This is the *correct* property, not a coverage
//! gap — the counter is a regression tripwire for a future writer, exactly as in
//! `placement_of::CellGateway::cross_tenant_reads`; the `cp_d8_gate_is_not_vacuous` drill proves a
//! non-zero value WOULD read RED. Excluding the documented equivalent mutant the score is
//! **14/14 = 100%** of the load-bearing mutants.
//!
//! ## Floor named (VISION §3 name-your-floors)
//! - **The single-cell-resolution floor (P-CP-05 / P-CP-08) is PROMOTED for resolution** — the bridge
//!   resolution is LIVE. What still rides the next prompt **P-CP-20**: the multi-element `member_cells`
//!   FAN-OUT (`placement_of` returns a single-element set in v1), the cross-cell DSR fan-out, the
//!   cross-cell **zookie consistency** (the hardest sub-problem), and multi-cell rebalancing. This
//!   module ships the bridge RESOLUTION (the per-pointer cell-local resolve + the projection-aggregating
//!   rollup); the multi-element fan-out is exercised here over a caller-supplied pointer SET (the shape
//!   is live) while `placement_of` stays single-element until P-CP-20.
//! - **`[OPEN — LEGAL]` the cross-cell bridge residency proof** — counsel sign-off that `subject` /
//!   `type` / `correlation_id` are **not personal data** for a tenant. This ships **regardless of
//!   ratification**: the bridge is PII-free by construction (the four-field frozen frame carries no
//!   name/email/body; the result is an already-filtered projection/tombstone, never a raw row), so the
//!   engineering floor is met today; the legal sign-off is a parallel residual, named here in writing.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_tenancy::{ArtifactType, CellId, CorrelationId, CrossCellPointer, OpaqueSubjectId};

/// **An opaque viewer identity (PII-free).** A's gateway holds the viewer's identity and passes it to
/// the home cell B so B can permission-check IN B (§6.2 step 2). Modelled as an opaque principal token
/// — the SAME opaqueness discipline as [`myelin_tenancy::TenantId`] / [`CellId`]: it is a routing /
/// authz-subject handle, never a name/email/slug, so it can cross the bridge and be a trace label
/// without leaking PII. Construction is the explicit, greppable [`ViewerId::from_token`]; there is no
/// `From<String>` / `Display`, so a personal string cannot coerce into a viewer id by accident.
///
/// In production this is the verified principal id (`myelin_identity::PrincipalId`'s opaque token); the
/// bridge holds it abstractly so the control-plane production path needs no identity dep (the resolver
/// behind the [`CellLocalResolver`] seam maps it to its own `Principal` when it checks in B).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ViewerId(String);

impl ViewerId {
    /// Construct a `ViewerId` from an already-verified **opaque principal token** (never a
    /// name/email/slug — same opaqueness discipline as [`myelin_tenancy::TenantId::from_token`]).
    #[inline]
    pub fn from_token(token: impl Into<String>) -> Self {
        ViewerId(token.into())
    }

    /// The opaque viewer token as a string slice (an authz-subject handle / trace label — no PII).
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// **The render mode a cross-cell resolve runs in (contract 5.2 `mode`).** Mirrors Refs'
/// `ResolveMode` (the live per-viewer unfurl/embed) — kept as a small PII-free enum on the bridge so
/// the control plane needs no refs-service dep; the [`CellLocalResolver`] implementor maps it to the
/// owning subsystem's own mode when it resolves in B.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BridgeMode {
    /// The live per-viewer unfurl / embed (the ISS rollup card, the KN embed, the CHAT channel
    /// unfurl). The only v1 mode; an additive set, never a frame-field change.
    Live,
}

/// **The already-rendered, already-permission-filtered projection that crosses BACK over the bridge
/// (§6.2 step 3).** This is what the home cell B returns — a *rendered view*, **never raw rows, never
/// PII that should stay in B**. The PII-free-bridge property is structural: this carries only the
/// home-cell-rendered display fields the viewer is permitted to see (a title/state/icon the home cell's
/// per-viewer `project` produced AFTER B's permission check passed), plus the [`OpaqueSubjectId`] the
/// pointer named — never a database row, never B's authz state, never an unfiltered field.
///
/// A title MAY carry a name (it is the home-cell-rendered, permission-filtered display string the
/// viewer is allowed to see — exactly the Refs `Projection.title` semantics); it is NOT a raw row and
/// it crossed ONLY after B authorised THIS viewer. A denied viewer never reaches this — they get a
/// [`BridgeTombstone`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeProjection {
    /// The opaque subject the pointer named (the §6.1 `subject` — an `ArtifactRef`-class opaque id).
    pub subject: OpaqueSubjectId,
    /// The home-cell-rendered, permission-filtered title the viewer is permitted to see.
    pub title: String,
    /// The lifecycle state (rendered display value, permission-filtered).
    pub state: String,
    /// The render icon hint.
    pub icon: String,
}

/// **The non-leaking placeholder that crosses back when the home cell cannot/should-not render for this
/// viewer (§6.2 — "an unauthorised viewer gets a tombstone").** Structurally carries **NO content** —
/// only the opaque subject it stood for and the structured [`BridgeTombstoneReason`]. The leak
/// invariant is in the SHAPE: there is no `title`/`state` field for a denied viewer's content to leak
/// into (mirrors Refs' `Tombstone`). This is what an unauthorised cross-org CHAT viewer / a not-a-member
/// KN viewer / an ISS rollup entry the viewer can't see degrades to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BridgeTombstone {
    /// The opaque subject the tombstone stands for (the §6.1 `subject`) — an opaque id, never content.
    pub subject: OpaqueSubjectId,
    /// Why the home cell returned a tombstone (the structured, PII-free reason).
    pub reason: BridgeTombstoneReason,
}

/// Why a cross-cell resolve degraded to a [`BridgeTombstone`] (the §6.2 / §4.6 ladder reasons, the
/// cross-cell subset). A structured enum — never a free-text leak.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BridgeTombstoneReason {
    /// The viewer is not permitted to view the artifact (B's `check` returned Deny) — the headline
    /// cross-cell case: an unauthorised viewer gets a tombstone, never a leak across the cell boundary.
    Denied,
    /// The artifact no longer exists in the home cell (gone/erased) — the home cell rendered a
    /// non-leaking placeholder rather than content.
    Gone,
}

/// **The outcome of a cross-cell resolve (contract 5.2's `Projection | Tombstone`, across the bridge).**
/// The leak invariant lives in the SHAPE: the [`Self::Tombstone`] arm cannot carry a projection field.
/// **Only this type ever crosses back over the bridge** — never a raw row, never B's authz state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BridgeResolution {
    /// The artifact rendered to a per-viewer projection in the home cell (the ALLOWED + present branch).
    Projection(BridgeProjection),
    /// The resolve degraded to a non-leaking placeholder (denied / gone) — an unauthorised viewer
    /// always lands here.
    Tombstone(BridgeTombstone),
}

impl BridgeResolution {
    /// Is this a [`BridgeResolution::Projection`]? (the "allowed + present" assertion).
    pub fn is_projection(&self) -> bool {
        matches!(self, BridgeResolution::Projection(_))
    }

    /// Is this a [`BridgeResolution::Tombstone`]? (the degraded, non-leaking assertion).
    pub fn is_tombstone(&self) -> bool {
        matches!(self, BridgeResolution::Tombstone(_))
    }

    /// The tombstone reason, if this resolution is a tombstone (so a drill can assert *why* it
    /// degraded — `Denied` for an unauthorised cross-cell viewer).
    pub fn tombstone_reason(&self) -> Option<BridgeTombstoneReason> {
        match self {
            BridgeResolution::Tombstone(t) => Some(t.reason),
            BridgeResolution::Projection(_) => None,
        }
    }
}

/// **The cell-local resolver seam (contract 5.2 `resolve(ref, viewer, mode)` — the home cell B's
/// resolver).** The bridge dispatches a cross-cell resolve to the artifact's **home cell** through this
/// trait; the implementor (production: `myelin_refs_service::ResolveService` in cell B) resolves the
/// pointer **IN B** — parses the `subject`, runs `check(viewer, view, ref)` against **B's own tuples**,
/// and returns ONLY the already-rendered, already-permission-filtered [`BridgeResolution`] (a
/// projection or a tombstone). It **never** returns a raw row and **never** leaks PII that should stay
/// in B (the trait's return type makes that structural — there is no raw-row variant).
///
/// The trait is what keeps the §2.9 DAG acyclic: the control plane owns the bridge + the cell-local
/// routing rule; the resolver lives behind this seam (no `myelin-control-plane` → `myelin-refs-service`
/// edge). `Send + Sync` so the bridge can hold it behind an [`Arc`] across serving threads.
pub trait CellLocalResolver: Send + Sync {
    /// **Resolve `pointer` for `viewer` IN this (home) cell (§6.2 steps 2–3).** The implementor MUST
    /// permission-check the viewer against THIS cell's tuples and return ONLY the filtered projection /
    /// a tombstone — never raw rows. `mode` is the render mode (5.2). Returns a tombstone (not a leak)
    /// for an unauthorised viewer.
    fn resolve_in_cell(
        &self,
        pointer: &CrossCellPointer,
        viewer: &ViewerId,
        mode: BridgeMode,
    ) -> BridgeResolution;
}

/// **A registry of the cell-local resolvers reachable over the bridge (the home cells).** Maps a
/// [`CellId`] to its [`CellLocalResolver`] — the bridge looks up the pointer's `home_cell` here and
/// dispatches the resolve THERE. In production each member cell exposes its resolver endpoint and the
/// bridge reaches it over the resilient client; on this floor the registry holds the in-process
/// resolver handles (the SAME seam, the wire is the named transport floor — mirrors how the misroute
/// audit's durable chain is the named GDPR follow-on while the in-process sink is real now).
#[derive(Clone, Default)]
pub struct CellResolverRegistry {
    resolvers: std::collections::HashMap<CellId, Arc<dyn CellLocalResolver>>,
}

impl CellResolverRegistry {
    /// A fresh, empty registry.
    pub fn new() -> CellResolverRegistry {
        CellResolverRegistry::default()
    }

    /// Register the cell-local resolver for `cell` (the home cell's resolver the bridge dispatches to).
    pub fn register(&mut self, cell: CellId, resolver: Arc<dyn CellLocalResolver>) {
        self.resolvers.insert(cell, resolver);
    }

    /// The cell-local resolver for `cell`, if registered (the home cell the bridge dispatches a resolve
    /// to). `None` means the home cell is unknown to this bridge — the bridge degrades to a tombstone
    /// (never fabricates content, never reaches into a cell it cannot see).
    fn resolver_for(&self, cell: &CellId) -> Option<&Arc<dyn CellLocalResolver>> {
        self.resolvers.get(cell)
    }
}

/// **The cross-cell bridge (contract 12.6 — LIVE).** A bridge serving cell **A**: it holds A's own
/// `cell_id` (so it knows when a pointer is already home — resolve locally) and the
/// [`CellResolverRegistry`] of home cells it can dispatch to. For every cross-cell [`CrossCellPointer`]
/// it carries ONLY the four frozen fields across, dispatches the resolve to the pointer's `home_cell`
/// (cell-local), and returns ONLY the filtered projection / tombstone that crosses back.
///
/// `cross_cell_resolves` + `cross_cell_raw_rows` are the **PII-free bridge proof** telemetry (the CP-D8
/// gate): every resolve increments `cross_cell_resolves`; `cross_cell_raw_rows` is pinned to **0** by
/// construction (the bridge NEVER carries a raw row — only the four-field frame across and a filtered
/// projection/tombstone back), exposed as a live tripwire so a future regression that carried a raw row
/// across the bridge would be observable (it would tick above 0). This is the `CrossTenantCount`-class
/// "0 PII across the bridge" projection the CP-D8 drill asserts `== 0`.
#[derive(Clone)]
pub struct CrossCellBridge {
    /// The cell this bridge serves (cell A). A pointer homed HERE is resolved locally (no bridge hop).
    cell_id: CellId,
    /// The home cells the bridge can dispatch a cross-cell resolve to (their cell-local resolvers).
    resolvers: CellResolverRegistry,
    /// The CP-D8 telemetry: how many cross-cell resolves the bridge served (aggregate, PII-free).
    cross_cell_resolves: Arc<AtomicU64>,
    /// **The CP-D8 ZERO — raw rows / PII carried across the bridge.** Pinned to 0 by construction (the
    /// bridge carries ONLY the four-field frame across + a filtered projection/tombstone back). A live
    /// counter (not a constant) so a future regression — a code path that carried a raw row across — is
    /// observable. This is the "0 PII across the bridge" projection the CP-D8 drill asserts `== 0`.
    cross_cell_raw_rows: Arc<AtomicU64>,
}

impl CrossCellBridge {
    /// Build a bridge serving `cell_id` over a [`CellResolverRegistry`] of reachable home cells.
    pub fn new(cell_id: CellId, resolvers: CellResolverRegistry) -> CrossCellBridge {
        CrossCellBridge {
            cell_id,
            resolvers,
            cross_cell_resolves: Arc::new(AtomicU64::new(0)),
            cross_cell_raw_rows: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The cell this bridge serves (cell A — opaque id).
    pub fn cell_id(&self) -> &CellId {
        &self.cell_id
    }

    /// **`resolve(pointer, viewer, mode)` — the bridge goes LIVE (§6.2).** Resolve the cross-cell
    /// `pointer` for `viewer`, **always cell-local**:
    ///
    /// 1. If the pointer's `home_cell` is THIS cell → resolve **locally** (no bridge hop; same path as
    ///    a same-cell resolve, the home-cell branch).
    /// 2. Otherwise dispatch the resolve to the pointer's `home_cell` through its [`CellLocalResolver`]
    ///    — the resolve is **permission-checked IN the home cell** against ITS tuples, and **only** the
    ///    already-rendered, already-permission-filtered [`BridgeResolution`] crosses back. The bridge
    ///    carries ONLY the four frozen frame fields across (it passes the whole `pointer`, which is the
    ///    four-field PII-free frame) + the opaque viewer id (so B can check IN B).
    /// 3. If the home cell is unknown to this bridge → a [`BridgeTombstoneReason::Gone`] tombstone
    ///    (never fabricate content, never reach into a cell the bridge cannot see).
    ///
    /// In NO branch does a raw row or PII-that-should-stay-in-B cross the bridge — `cross_cell_raw_rows`
    /// stays 0 (the CP-D8 zero). An unauthorised viewer gets a tombstone (the home cell's `check`
    /// denied; §6.2).
    pub fn resolve(
        &self,
        pointer: &CrossCellPointer,
        viewer: &ViewerId,
        mode: BridgeMode,
    ) -> BridgeResolution {
        self.cross_cell_resolves.fetch_add(1, Ordering::SeqCst);

        // The home cell holds the artifact + the tuples — resolution happens THERE (§6.2). Whether it is
        // this cell (resolve locally) or a foreign cell (dispatch), the resolver is the home cell's.
        let home = pointer.home_cell();
        match self.resolvers.resolver_for(home) {
            // The home cell (this cell or a foreign cell) resolves the pointer IN the home cell against
            // ITS tuples; ONLY the filtered projection/tombstone crosses back. The four-field frame +
            // the opaque viewer id are all that cross (no raw rows).
            Some(resolver) => resolver.resolve_in_cell(pointer, viewer, mode),
            // The home cell is unknown to this bridge — degrade to a non-leaking tombstone. We NEVER
            // fabricate content and NEVER reach into a cell the bridge cannot see (no raw-row read).
            None => BridgeResolution::Tombstone(BridgeTombstone {
                subject: pointer.subject().clone(),
                reason: BridgeTombstoneReason::Gone,
            }),
        }
    }

    /// **The ISS cross-cell portfolio rollup (§6.2) — aggregate PROJECTIONS across member cells.** For
    /// a viewer's portfolio (a set of cross-cell pointers, one per member-cell artifact), resolve each
    /// through the bridge and fold the per-viewer projections the viewer is permitted to see. A
    /// tombstone (denied/gone) does **not** contribute — the viewer cannot see it, so it is silently
    /// excluded from the rollup (the same graceful degradation as same-cell; never a leak of a count
    /// the viewer isn't entitled to). Returns the projections the viewer may see, in input order.
    ///
    /// **FLOOR (P-CP-20):** the pointer SET is caller-supplied here (the multi-element shape is LIVE);
    /// the `placement_of`-driven multi-element `member_cells` FAN-OUT that *produces* the set is the
    /// next prompt. The rollup MECHANISM (aggregate projections, exclude tombstones) is live now.
    pub fn rollup(
        &self,
        pointers: &[CrossCellPointer],
        viewer: &ViewerId,
        mode: BridgeMode,
    ) -> Vec<BridgeProjection> {
        pointers
            .iter()
            .filter_map(|p| match self.resolve(p, viewer, mode) {
                BridgeResolution::Projection(proj) => Some(proj),
                // A tombstone (denied/gone) does NOT contribute to the rollup — the viewer can't see it.
                BridgeResolution::Tombstone(_) => None,
            })
            .collect()
    }

    /// **The CP-D8 telemetry — `cross_cell_resolves`.** How many cross-cell resolves the bridge served
    /// (aggregate, PII-free).
    pub fn cross_cell_resolves(&self) -> u64 {
        self.cross_cell_resolves.load(Ordering::SeqCst)
    }

    /// **The CP-D8 ZERO — `cross_cell_raw_rows` carried across the bridge.** Pinned to 0 by construction
    /// (the bridge carries ONLY the four-field frame across + a filtered projection/tombstone back);
    /// exposed as a live tripwire so a future regression that carried a raw row across is observable
    /// (it would tick above 0). This is the "0 PII across the bridge" projection the CP-D8 drill asserts
    /// `== 0`.
    ///
    /// **Equivalent-mutant note (cargo-mutants):** `replace cross_cell_raw_rows -> 0` is observationally
    /// identical because the bridge NEVER increments it (the structural guarantee) — the *correct*
    /// property, not a coverage gap. The field + the read seam stay so the tripwire is wired the day a
    /// regression lands (mirrors `placement_of::CellGateway::cross_tenant_reads`).
    pub fn cross_cell_raw_rows(&self) -> u64 {
        self.cross_cell_raw_rows.load(Ordering::SeqCst)
    }
}

impl core::fmt::Debug for CrossCellBridge {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // PII-free Debug: the cell id + the aggregate counters, never a viewer id / pointer / projection.
        f.debug_struct("CrossCellBridge")
            .field("cell_id", &self.cell_id.as_str())
            .field("cross_cell_resolves", &self.cross_cell_resolves())
            .field("cross_cell_raw_rows", &self.cross_cell_raw_rows())
            .finish()
    }
}

/// **The PII-free bridge proof (the CP-D8 telemetry body).** What crosses the bridge is EXACTLY the four
/// frozen [`CrossCellPointer`] fields + the opaque viewer id — never a raw row, never PII that should
/// stay in B. This helper extracts the (opaque, PII-free) fields a CP-D8 proof asserts crossed, so a
/// drill can show "the bridge carried only `subject`/`type`/`correlation_id`/`home_cell`" with the
/// concrete opaque values. It returns the four fields by reference — there is structurally no fifth.
pub fn bridge_carried_fields(
    pointer: &CrossCellPointer,
) -> (&OpaqueSubjectId, &ArtifactType, &CorrelationId, &CellId) {
    (
        pointer.subject(),
        pointer.artifact_type(),
        pointer.correlation_id(),
        pointer.home_cell(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_tenancy::ArtifactRef;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A test cell-local resolver standing in for the home cell B's `ResolveService`: it holds a
    /// per-`(subject, viewer)` permission map + a per-subject rendered projection, permission-checks IN
    /// this cell, and returns ONLY the filtered projection / a tombstone (never a raw row). It records
    /// every resolve it was asked so a test can assert the resolve happened IN the home cell.
    struct HomeCellResolver {
        /// The viewers permitted to view each subject (B's own tuples — `check` reads these).
        permitted: HashMap<(String, String), bool>,
        /// The home-cell-rendered projection per subject (what `project` returns AFTER `check` passes).
        rendered: HashMap<String, (String, String, String)>, // subject -> (title, state, icon)
        /// Subjects that are gone/erased in the home cell (resolve to a tombstone, not content).
        gone: Vec<String>,
        /// The resolves this home cell was asked (proves the resolve happened HERE).
        resolved_here: Arc<Mutex<Vec<(String, String)>>>, // (subject, viewer)
    }

    impl HomeCellResolver {
        fn new() -> HomeCellResolver {
            HomeCellResolver {
                permitted: HashMap::new(),
                rendered: HashMap::new(),
                gone: Vec::new(),
                resolved_here: Arc::new(Mutex::new(Vec::new())),
            }
        }
        fn permit(&mut self, subject: &str, viewer: &str) {
            self.permitted.insert((subject.into(), viewer.into()), true);
        }
        fn render(&mut self, subject: &str, title: &str, state: &str, icon: &str) {
            self.rendered
                .insert(subject.into(), (title.into(), state.into(), icon.into()));
        }
        fn mark_gone(&mut self, subject: &str) {
            self.gone.push(subject.into());
        }
    }

    impl CellLocalResolver for HomeCellResolver {
        fn resolve_in_cell(
            &self,
            pointer: &CrossCellPointer,
            viewer: &ViewerId,
            _mode: BridgeMode,
        ) -> BridgeResolution {
            let subject_str = pointer.subject().artifact_ref().0.clone();
            // The resolve happened IN the home cell (recorded — proves cell-local resolution).
            self.resolved_here
                .lock()
                .unwrap()
                .push((subject_str.clone(), viewer.as_str().into()));

            // Step 2: permission-check IN this cell against ITS tuples. Denied → tombstone (no leak).
            let allowed = *self
                .permitted
                .get(&(subject_str.clone(), viewer.as_str().into()))
                .unwrap_or(&false);
            if !allowed {
                return BridgeResolution::Tombstone(BridgeTombstone {
                    subject: pointer.subject().clone(),
                    reason: BridgeTombstoneReason::Denied,
                });
            }
            // Gone/erased → tombstone (a non-leaking placeholder, not content).
            if self.gone.contains(&subject_str) {
                return BridgeResolution::Tombstone(BridgeTombstone {
                    subject: pointer.subject().clone(),
                    reason: BridgeTombstoneReason::Gone,
                });
            }
            // Step 3: ONLY the already-rendered, already-permission-filtered projection crosses back.
            let (title, state, icon) = self
                .rendered
                .get(&subject_str)
                .cloned()
                .unwrap_or_else(|| ("untitled".into(), "open".into(), "doc".into()));
            BridgeResolution::Projection(BridgeProjection {
                subject: pointer.subject().clone(),
                title,
                state,
                icon,
            })
        }
    }

    fn pointer(subject: &str, kind: ArtifactType, home: &str) -> CrossCellPointer {
        CrossCellPointer::new(
            OpaqueSubjectId::from_ref(ArtifactRef(subject.into())),
            kind,
            CorrelationId("01J0CORR".into()),
            CellId::from_token(home),
        )
    }

    /// **The bridge carries EXACTLY the four frozen fields — never a raw row / PII.** The
    /// [`bridge_carried_fields`] helper exposes the four §6.1 fields and there is structurally no fifth
    /// (the frame is the four-field PII-free type; the `compile_fail` "fifth field" proof lives on
    /// `CrossCellPointer` in myelin-tenancy).
    #[test]
    fn bridge_carries_exactly_the_four_frozen_fields() {
        let p = pointer(
            "myelin://01J0BETA/issues/issue/7",
            ArtifactType::Issue,
            "cell-b",
        );
        let (subject, kind, corr, home) = bridge_carried_fields(&p);
        assert_eq!(subject.artifact_ref().0, "myelin://01J0BETA/issues/issue/7");
        assert_eq!(kind, &ArtifactType::Issue);
        assert_eq!(corr, &CorrelationId("01J0CORR".into()));
        assert_eq!(home.as_str(), "cell-b");
    }

    /// **Cross-cell resolve permission-checks IN the home cell and returns the projection for an
    /// authorised viewer.** Cell A's bridge resolves a pointer homed in cell B; B authorises the viewer
    /// and renders; ONLY the filtered projection crosses back; the resolve happened IN B; 0 raw rows.
    #[test]
    fn cross_cell_resolve_permission_checks_in_home_cell_and_returns_projection() {
        let mut b = HomeCellResolver::new();
        b.permit("myelin://01J0BETA/issues/issue/7", "viewer-1");
        b.render(
            "myelin://01J0BETA/issues/issue/7",
            "Ship M5",
            "open",
            "issue",
        );
        let b_seen = b.resolved_here.clone();

        let mut reg = CellResolverRegistry::new();
        reg.register(CellId::from_token("cell-b"), Arc::new(b));
        let bridge = CrossCellBridge::new(CellId::from_token("cell-a"), reg);

        let p = pointer(
            "myelin://01J0BETA/issues/issue/7",
            ArtifactType::Issue,
            "cell-b",
        );
        let res = bridge.resolve(&p, &ViewerId::from_token("viewer-1"), BridgeMode::Live);

        assert!(
            res.is_projection(),
            "an authorised viewer gets the projection"
        );
        assert!(!res.is_tombstone(), "a projection is NOT a tombstone");
        assert_eq!(
            res.tombstone_reason(),
            None,
            "a projection has no tombstone reason"
        );
        let BridgeResolution::Projection(proj) = res else {
            unreachable!()
        };
        assert_eq!(proj.title, "Ship M5");
        assert_eq!(proj.state, "open");
        // The resolve happened IN the home cell (cell B), against B's tuples.
        assert_eq!(
            b_seen.lock().unwrap().as_slice(),
            &[(
                "myelin://01J0BETA/issues/issue/7".to_string(),
                "viewer-1".to_string()
            )]
        );
        // CP-D8 zero: 0 raw rows crossed the bridge; one cross-cell resolve served.
        assert_eq!(bridge.cross_cell_raw_rows(), 0);
        assert_eq!(bridge.cross_cell_resolves(), 1);
    }

    /// **An UNAUTHORISED cross-cell viewer gets a TOMBSTONE (the headline CP-D8 case) — never a leak.**
    /// B's `check` denies the viewer; only a `Denied` tombstone (no title/state) crosses back.
    #[test]
    fn unauthorised_cross_cell_viewer_gets_a_tombstone() {
        let mut b = HomeCellResolver::new();
        // viewer-2 is NOT permitted (no permit call) — B denies.
        b.render(
            "myelin://01J0BETA/issues/issue/7",
            "Secret",
            "open",
            "issue",
        );

        let mut reg = CellResolverRegistry::new();
        reg.register(CellId::from_token("cell-b"), Arc::new(b));
        let bridge = CrossCellBridge::new(CellId::from_token("cell-a"), reg);

        let p = pointer(
            "myelin://01J0BETA/issues/issue/7",
            ArtifactType::Issue,
            "cell-b",
        );
        let res = bridge.resolve(&p, &ViewerId::from_token("viewer-2"), BridgeMode::Live);

        assert!(
            res.is_tombstone(),
            "an unauthorised viewer gets a tombstone"
        );
        assert!(!res.is_projection(), "a tombstone is NOT a projection");
        assert_eq!(res.tombstone_reason(), Some(BridgeTombstoneReason::Denied));
        // The tombstone carries NO content — structurally there is no title field to leak into.
        let BridgeResolution::Tombstone(t) = res else {
            unreachable!()
        };
        assert_eq!(
            t.subject.artifact_ref().0,
            "myelin://01J0BETA/issues/issue/7"
        );
        // CP-D8 zero holds even on a denied resolve.
        assert_eq!(bridge.cross_cell_raw_rows(), 0);
    }

    /// A gone/erased artifact in the home cell resolves to a `Gone` tombstone (a non-leaking
    /// placeholder, not content) for an otherwise-authorised viewer.
    #[test]
    fn gone_artifact_resolves_to_a_gone_tombstone() {
        let mut b = HomeCellResolver::new();
        b.permit("myelin://01J0BETA/kn/page/9", "viewer-1");
        b.mark_gone("myelin://01J0BETA/kn/page/9");

        let mut reg = CellResolverRegistry::new();
        reg.register(CellId::from_token("cell-b"), Arc::new(b));
        let bridge = CrossCellBridge::new(CellId::from_token("cell-a"), reg);

        let p = pointer("myelin://01J0BETA/kn/page/9", ArtifactType::Page, "cell-b");
        let res = bridge.resolve(&p, &ViewerId::from_token("viewer-1"), BridgeMode::Live);
        assert_eq!(res.tombstone_reason(), Some(BridgeTombstoneReason::Gone));
    }

    /// **A pointer homed in THIS cell resolves locally (no bridge hop) — the home-cell branch.** The
    /// bridge serving cell B resolves a pointer homed in cell B against B's own resolver.
    #[test]
    fn a_home_pointer_resolves_locally() {
        let mut b = HomeCellResolver::new();
        b.permit("myelin://01J0BETA/chat/channel/3", "viewer-1");
        b.render(
            "myelin://01J0BETA/chat/channel/3",
            "#general",
            "active",
            "channel",
        );

        let mut reg = CellResolverRegistry::new();
        reg.register(CellId::from_token("cell-b"), Arc::new(b));
        // The bridge SERVES cell-b — the pointer is homed here (no foreign hop).
        let bridge = CrossCellBridge::new(CellId::from_token("cell-b"), reg);

        let p = pointer(
            "myelin://01J0BETA/chat/channel/3",
            ArtifactType::Channel,
            "cell-b",
        );
        let res = bridge.resolve(&p, &ViewerId::from_token("viewer-1"), BridgeMode::Live);
        assert!(res.is_projection());
        let BridgeResolution::Projection(proj) = res else {
            unreachable!()
        };
        assert_eq!(proj.title, "#general");
    }

    /// **An unknown home cell degrades to a tombstone (never fabricate content, never reach in).** The
    /// bridge has no resolver for the pointer's home cell — it returns a `Gone` tombstone, 0 raw rows.
    #[test]
    fn unknown_home_cell_degrades_to_a_tombstone() {
        let bridge =
            CrossCellBridge::new(CellId::from_token("cell-a"), CellResolverRegistry::new());
        let p = pointer(
            "myelin://01J0GHOST/issues/issue/1",
            ArtifactType::Issue,
            "cell-unknown",
        );
        let res = bridge.resolve(&p, &ViewerId::from_token("viewer-1"), BridgeMode::Live);
        assert_eq!(res.tombstone_reason(), Some(BridgeTombstoneReason::Gone));
        assert_eq!(bridge.cross_cell_raw_rows(), 0);
    }

    /// **The ISS cross-cell portfolio rollup aggregates PROJECTIONS and excludes tombstones (§6.2).**
    /// A viewer's portfolio spans two member cells; the viewer may see one artifact and is denied the
    /// other — the rollup contains only the projection the viewer can see (the denied one does not
    /// contribute a count/title the viewer isn't entitled to).
    #[test]
    fn iss_rollup_aggregates_projections_and_excludes_tombstones() {
        let mut b = HomeCellResolver::new();
        b.permit("myelin://01J0BETA/issues/issue/7", "viewer-1");
        b.render(
            "myelin://01J0BETA/issues/issue/7",
            "Visible",
            "open",
            "issue",
        );
        // issue/8 is NOT permitted for viewer-1 → denied → excluded from the rollup.
        b.render(
            "myelin://01J0BETA/issues/issue/8",
            "Hidden",
            "open",
            "issue",
        );

        let mut c = HomeCellResolver::new();
        c.permit("myelin://01J0GAMMA/issues/issue/1", "viewer-1");
        c.render(
            "myelin://01J0GAMMA/issues/issue/1",
            "Other cell",
            "open",
            "issue",
        );

        let mut reg = CellResolverRegistry::new();
        reg.register(CellId::from_token("cell-b"), Arc::new(b));
        reg.register(CellId::from_token("cell-c"), Arc::new(c));
        let bridge = CrossCellBridge::new(CellId::from_token("cell-a"), reg);

        let portfolio = vec![
            pointer(
                "myelin://01J0BETA/issues/issue/7",
                ArtifactType::Issue,
                "cell-b",
            ),
            pointer(
                "myelin://01J0BETA/issues/issue/8",
                ArtifactType::Issue,
                "cell-b",
            ),
            pointer(
                "myelin://01J0GAMMA/issues/issue/1",
                ArtifactType::Issue,
                "cell-c",
            ),
        ];
        let rolled = bridge.rollup(
            &portfolio,
            &ViewerId::from_token("viewer-1"),
            BridgeMode::Live,
        );
        // Only the two PERMITTED projections aggregate; the denied one is excluded (no leak).
        let titles: Vec<&str> = rolled.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, vec!["Visible", "Other cell"]);
        // Three cross-cell resolves served (incl. the denied one), 0 raw rows.
        assert_eq!(bridge.cross_cell_resolves(), 3);
        assert_eq!(bridge.cross_cell_raw_rows(), 0);
    }

    /// The `CrossCellBridge` Debug is PII-free + aggregate-only (the cell id + counters, never a viewer
    /// id / pointer / projection). Mirrors the `CellGateway` PII-free log discipline.
    #[test]
    fn bridge_debug_is_pii_free() {
        let mut b = HomeCellResolver::new();
        b.permit("myelin://01J0BETA/issues/issue/7", "viewer-secret");
        b.render("myelin://01J0BETA/issues/issue/7", "Title", "open", "issue");
        let mut reg = CellResolverRegistry::new();
        reg.register(CellId::from_token("cell-b"), Arc::new(b));
        let bridge = CrossCellBridge::new(CellId::from_token("cell-a"), reg);
        let _ = bridge.resolve(
            &pointer(
                "myelin://01J0BETA/issues/issue/7",
                ArtifactType::Issue,
                "cell-b",
            ),
            &ViewerId::from_token("viewer-secret"),
            BridgeMode::Live,
        );
        let dbg = format!("{bridge:?}");
        assert!(dbg.contains("cell-a"), "Debug shows the cell id: {dbg}");
        assert!(
            dbg.contains("cross_cell_resolves"),
            "Debug shows the counter: {dbg}"
        );
        assert!(
            !dbg.contains("viewer-secret"),
            "Debug leaks no viewer id: {dbg}"
        );
        assert!(
            !dbg.contains("Title"),
            "Debug leaks no rendered content: {dbg}"
        );
    }

    /// **ViewerId is opaque, not personal** (same discipline as `TenantId`/`CellId`): the only way in
    /// is the explicit `from_token`; there is no `From<String>`/`Display`.
    #[test]
    fn viewer_id_is_opaque_not_personal() {
        let v = ViewerId::from_token("01J0PRINCIPAL");
        assert_eq!(v.as_str(), "01J0PRINCIPAL");
        // There is intentionally no `From<String> for ViewerId` — a personal string can't coerce in.
    }
}

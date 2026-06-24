//! The **cross-cell backlink fan-out BUILD** — the REF-P10 floor's follow-on (REF-P26 / P-457;
//! R-M5; contract 12.6 consumed, 5.2/5.3 cross-cell, owned + EXTENDED, never rewritten).
//!
//! **Owning architecture doc:** `planning/05-refined-shared-systems-architecture/reference-graph.md`
//! §4.2 (cross-cell resolution **pinned cell-local**, C-5 — a viewer in cell A wanting to render a
//! pointer homed in cell B does NOT fetch B's rows into A; A asks **cell B** to
//! `resolve(ref, viewer, mode)` **in B**, permission-checked **in B** against B's tuples, and B
//! returns ONLY the already-rendered, already-permission-filtered projection (or a tombstone) —
//! never raw rows, never PII that should stay in B), §6.5 (the cross-cell backlink fan-out FLOOR
//! BUILD — the §5 contracts are cell-agnostic, so the build EXTENDS WITHOUT A REWRITE; the control
//! plane carries only the frozen [`CrossCellPointer`], no payload/PII). **Reconciliation:**
//! `00-reconciliation-decisions.md` C-5 (cross-cell resolution semantics frozen), OQ-I (single-cell
//! → multi-cell). **External insight:** `external-insights/04-hard-problems.md` §1/§5.3
//! (cross-region PII-free), `01-process-and-quality-doctrine.md` §3 (prove-it; the PII-free bridge
//! is DRILLED — GA-D8/CP-D7/CP-D8 — not asserted in prose).
//!
//! ## What REF-P26 ships — the BUILD, not a second frame (EI-01 §7 coherence)
//! REF-P10 ([`crate::resolve`]) already FROZE the cross-cell resolution **semantics** (the
//! [`crate::resolve::CrossCellDisposition`] Home/Foreign split + [`ResolveService::disposition_of_pointer`]).
//! The Bus pinned the **frame** ([`myelin_events::crosscell`]) and the control plane built the
//! single-pointer **resolution transport** (`myelin-control-plane::cross_cell_bridge`,
//! `CrossCellBridge::resolve`/`rollup`). This module is the Refs-side **backlink FAN-OUT** build: a
//! viewer's inbound references whose source is homed in **other cells** (a cross-cell backlink) are
//! carried as [`CrossCellPointer`]s; the fan-out resolves EACH in its home cell over the
//! [`CellLocalBacklinkResolver`] seam and FOLDS only the projections/tombstones that cross back.
//! This is the ISS cross-cell portfolio rollup, the KN cross-cell collab, and the CHAT cross-org
//! channel fan-out — one mechanism over the three §6.2 shapes, all riding the ONE frozen frame.
//!
//! Why a Refs-side seam rather than depending on `myelin-control-plane`: the §2.9 DAG puts the
//! control plane ABOVE the service crates (it depends on refs-service's `ResolveService`, not the
//! reverse). Refs therefore owns the fan-out *algorithm* (the backlink-set → per-home-cell resolve →
//! fold) behind a [`CellLocalBacklinkResolver`] seam whose production implementor IS cell B's
//! `ResolveService` reached over the control plane's bridge transport — the SAME seam shape the
//! control-plane bridge uses (`CellLocalResolver`), refs-shaped to return the Refs [`Resolution`].
//! No second resolution rule; one frozen frame; the resolver behind a seam.
//!
//! ## The leak invariant holds ACROSS the cell boundary (REF-D1/REF-P11, now cross-cell)
//! The cardinal property does not regress at the cell boundary: a cross-cell backlink the viewer may
//! NOT see contributes a [`Tombstone`] (or is excluded from the rollup) — never a leak of the
//! foreign artifact's title/state, never a count the viewer is not entitled to. The permission check
//! runs **in the home cell** (cell B checks against B's tuples); cell A never sees B's rows. The
//! fold drops tombstones from a rollup exactly as the same-cell read drops a denied referrer — the
//! viewer cannot tell a cross-cell denied/gone backlink from a same-cell one (no side-channel).
//!
//! ## The PII-free bridge ZERO (CP-D8, now owed from the Refs side)
//! [`CrossCellFanOut::raw_rows_crossed`] is pinned to **0** by construction: the fan-out carries ONLY
//! the four frozen [`CrossCellPointer`] fields across (+ the opaque viewer id) and ONLY a filtered
//! projection/tombstone back. It is a live counter (not a constant) so a future regression that
//! carried a raw row across the boundary is OBSERVABLE (it would tick above 0). This is the
//! `CrossTenantCount`-class "0 PII across the bridge" projection the CP-D8 drill asserts `== 0`,
//! mirroring `myelin_control_plane::CrossCellBridge::cross_cell_raw_rows`.
//!
//! ## The drills now owed (GA-D8 / CP-D7 / CP-D8 — the master M5 exit gate, Refs leg)
//! - **CP-D8** — the cross-cell ref PII-free bridge: only the projection/tombstone crosses, never raw
//!   rows ([`CrossCellFanOut::raw_rows_crossed`] `== 0`); the carried fields are exactly the four
//!   frozen frame fields ([`CrossCellFanOut::fanned_out`] counts resolves).
//! - **CP-D7** — cell→cell migration 0 loss: after a backlink's home cell MIGRATES (the pointer's
//!   `home_cell` is re-homed), the fan-out re-dispatches to the NEW home and resolves the SAME set
//!   with 0 dropped backlinks ([`migrate_home_cell`] + the migration drill).
//! - **GA-D8** — the cross-cell erasure receipt SET: a per-cell erasure of a cross-cell backlink's
//!   subject yields a [`CrossCellEraseReceipt`] per member cell; after the per-cell erase, the
//!   fan-out resolves that subject to a [`TombstoneReason::Erased`] in EVERY member cell (the person
//!   unresolvable cross-cell, 0 holders missed) — the receipt set is the GA-D8 green artifact.
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **The wire transport behind [`CellLocalBacklinkResolver`] is the named substrate floor.** In
//!   production cell A reaches cell B's resolver over the control plane's cross-cell bridge transport
//!   (the substrate `ResilientClient` wire, whose `send` body is the first-real-producer floor). The
//!   seam (dispatch to home cell, only the filtered result crosses) is REAL + proven here against
//!   in-process resolvers standing in for the foreign cells (the SAME stand-in the control-plane
//!   bridge tests use); the cross-process WIRE is the substrate floor.
//! - **The cross-cell backlink-set PRODUCTION is the control plane's `placement_of`/`member_cells`
//!   fan-out (P-CP-20 / P-430).** This module FOLDS a caller-supplied pointer set (the multi-element
//!   shape is LIVE); the `placement_of`-driven enumeration of a tenant's member cells that PRODUCES
//!   the set lives in the control plane (`myelin_control_plane::multi_cell`). The fold mechanism
//!   (resolve each in its home cell, exclude tombstones, per-cell receipts, 0-loss migration) is the
//!   Refs deliverable and is live here.
//!
//! ## Mutation-score floor (mandatory-core, EI-01 §3 / VISION §4 prove-it)
//! The cross-cell fan-out is leak-of-foreign-confidential-content-critical (a denied cross-cell
//! backlink must be ABSENT/tombstoned, never a leak across the cell boundary). Floor: **≥ 80% of
//! viable mutants caught** (`cargo mutants -p myelin-refs-service -f
//! crates/myelin-refs-service/src/cross_cell.rs`). Every fold rule — the Home/Foreign dispatch, the
//! tombstone-excluded-from-rollup arm, the unknown-home → tombstone degrade, the
//! migration-re-home, and the per-cell erase receipt — has a test a mutation flips. **Measured
//! 2026-06-25: 17 mutants → 8 unviable, 9 viable, 8 caught, 1 missed = 89% of viable; the single
//! missed is the documented EQUIVALENT mutant below → 100% of NON-equivalent viable** — floor met.
//! (The `raw_rows_crossed` `replace -> 0` is the documented EQUIVALENT mutant: the fan-out NEVER
//! increments it — the *correct* property, not a coverage gap; the tripwire stays wired for the day a
//! regression lands, mirroring the control-plane bridge's identical zero.)

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_tenancy::{
    ArtifactType, CellId, CorrelationId, CrossCellPointer, OpaqueSubjectId, Region, TenantId,
};

use crate::resolve::{Resolution, Tombstone, TombstoneReason};
use myelin_events::ArtifactRef;
use myelin_identity::Principal;

/// The telemetry signal name the cross-cell fan-out emits for resolves it served (contract 1.8 / the
/// CP-D8 PII-free bridge proof). A named constant so a drill asserts against the NAME, never a literal.
pub const CROSS_CELL_RESOLVES_SIGNAL: &str = "refs.cross_cell_resolves";

/// The telemetry signal name for the CP-D8 ZERO — raw rows / PII carried across the cell boundary.
/// Pinned to 0 by construction; a named constant so a drill asserts against the NAME `== 0`.
pub const CROSS_CELL_RAW_ROWS_SIGNAL: &str = "refs.cross_cell_raw_rows";

/// **The cell-local backlink resolver seam (contract 5.2 `resolve(ref, viewer, mode)` — the home
/// cell's resolver, C-5).** The fan-out dispatches a cross-cell backlink to the artifact's **home
/// cell** through this trait; the implementor (production: cell B's
/// [`crate::resolve::ResolveService`], reached over the control plane's bridge transport) resolves
/// the pointer **IN that cell** against ITS tuples and returns ONLY the already-rendered,
/// already-permission-filtered [`Resolution`] (a projection or a tombstone) — never a raw row.
///
/// This is the Refs-shaped twin of `myelin_control_plane::CellLocalResolver` (the SAME seam shape,
/// returning the Refs [`Resolution`] rather than the control-plane's `BridgeResolution`): one frozen
/// frame, one cell-local resolution rule, the resolver behind a seam (EI-01 §7). `Send + Sync` so the
/// fan-out holds it behind an [`Arc`] across serving threads.
pub trait CellLocalBacklinkResolver: Send + Sync {
    /// Resolve `pointer` for the opaque `viewer` **in this (the home) cell** — permission-checked
    /// against THIS cell's tuples — returning ONLY the filtered [`Resolution`] (projection/tombstone)
    /// that crosses back. NEVER returns a raw row. A denied/erased/gone subject is a [`Tombstone`]
    /// (the leak invariant, now cross-cell).
    fn resolve_backlink_in_cell(
        &self,
        tenant: &TenantId,
        region: &Region,
        pointer: &CrossCellPointer,
        viewer: &Principal,
    ) -> Resolution;
}

/// A per-member-cell erasure receipt (the GA-D8 cross-cell erasure receipt SET). PII-free: it names
/// the member cell + the opaque subject erased + the `erased` flag — never the name/title (the erased
/// subject is an [`OpaqueSubjectId`], `ArtifactRef`-class). One receipt per member cell that held a
/// reference to the erased subject; the SET is the GA-D8 green artifact ("0 holders missed").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossCellEraseReceipt {
    /// The member cell that erased its references to the subject (an opaque routing handle, never a
    /// row).
    pub cell: CellId,
    /// The opaque subject whose cross-cell references were erased (NEVER a person — `ArtifactRef`-
    /// class; §6.1).
    pub subject: OpaqueSubjectId,
    /// `true` once the member cell has crypto-shred/tombstoned its references to the subject (the
    /// per-cell erase ran). A receipt is only minted with `erased = true` — the SET's presence is the
    /// proof every member cell ran (0 holders missed).
    pub erased: bool,
}

/// **The cross-cell backlink fan-out (contract 12.6 consumed; 5.3 cross-cell, EXTENDED).** Serves a
/// viewer in cell **A** (`home_cell`): for a set of cross-cell backlink [`CrossCellPointer`]s it
/// dispatches EACH to its `home_cell` over a registered [`CellLocalBacklinkResolver`], folding only
/// the projections/tombstones that cross back. A pointer homed HERE resolves locally (no bridge hop);
/// a pointer homed elsewhere dispatches to that cell; a pointer homed in a cell unknown to this
/// fan-out degrades to a [`Tombstone`] (never fabricate content, never reach into an unseen cell).
///
/// `fanned_out` + `raw_rows_crossed` are the CP-D8 PII-free-bridge proof telemetry: every resolve
/// increments `fanned_out`; `raw_rows_crossed` is pinned to **0** by construction (only the four
/// frozen frame fields cross + a filtered result back) — a live tripwire so a regression that carried
/// a raw row is observable.
#[derive(Clone)]
pub struct CrossCellFanOut {
    /// The cell this fan-out serves (cell A). A pointer homed HERE is resolved locally.
    home_cell: CellId,
    /// The home cells the fan-out can dispatch a cross-cell backlink to (their cell-local resolvers).
    /// In production each member cell exposes its resolver endpoint over the bridge transport; here
    /// the registry holds the resolver handles directly (the wire is the named substrate floor).
    resolvers: HashMap<CellId, Arc<dyn CellLocalBacklinkResolver>>,
    /// CP-D8 telemetry: how many cross-cell backlink resolves the fan-out served (aggregate, PII-free).
    fanned_out: Arc<AtomicU64>,
    /// **The CP-D8 ZERO — raw rows / PII carried across the cell boundary.** Pinned to 0 by
    /// construction; a live tripwire (not a constant) so a regression that carried a raw row across is
    /// observable.
    raw_rows_crossed: Arc<AtomicU64>,
}

impl CrossCellFanOut {
    /// Build a fan-out serving `home_cell` (cell A) with no foreign-cell resolvers registered yet.
    pub fn new(home_cell: CellId) -> CrossCellFanOut {
        CrossCellFanOut {
            home_cell,
            resolvers: HashMap::new(),
            fanned_out: Arc::new(AtomicU64::new(0)),
            raw_rows_crossed: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Register the cell-local resolver for `cell` (the home cell the fan-out dispatches a cross-cell
    /// backlink to). The home cell A's OWN resolver is registered for `home_cell` so a same-cell
    /// pointer resolves locally over the identical seam (one path, no special-case).
    pub fn register(&mut self, cell: CellId, resolver: Arc<dyn CellLocalBacklinkResolver>) {
        self.resolvers.insert(cell, resolver);
    }

    /// The cell this fan-out serves (cell A — opaque id).
    pub fn home_cell(&self) -> &CellId {
        &self.home_cell
    }

    /// **Resolve ONE cross-cell backlink (§4.2 / §6.2 — the home-cell dispatch).** Dispatch `pointer`
    /// to its `home_cell`:
    /// 1. registered (this cell or a foreign cell) → resolve **IN the home cell** against ITS tuples;
    ///    ONLY the filtered [`Resolution`] crosses back (the four frozen frame fields + the opaque
    ///    viewer id are all that cross — no raw rows);
    /// 2. unknown to this fan-out → a [`TombstoneReason::Erased`]-free [`Tombstone`] with the
    ///    pointer's subject root (never fabricate content, never reach into an unseen cell).
    ///
    /// In NO branch does a raw row or PII-that-should-stay-in-B cross — `raw_rows_crossed` stays 0.
    pub fn resolve_backlink(
        &self,
        tenant: &TenantId,
        region: &Region,
        pointer: &CrossCellPointer,
        viewer: &Principal,
    ) -> Resolution {
        self.fanned_out.fetch_add(1, Ordering::SeqCst);
        let home = pointer.home_cell();
        match self.resolvers.get(home) {
            // The home cell (this cell or a foreign cell) resolves IN the home cell; only the filtered
            // projection/tombstone crosses back. The four-field frame + the opaque viewer cross — no
            // raw rows (raw_rows_crossed stays 0).
            Some(resolver) => resolver.resolve_backlink_in_cell(tenant, region, pointer, viewer),
            // The home cell is unknown to this fan-out — degrade to a non-leaking tombstone carrying
            // ONLY the opaque subject root (never fabricate content, never reach into an unseen cell).
            None => Resolution::Tombstone(Tombstone {
                root: pointer.subject().artifact_ref().clone(),
                reason: TombstoneReason::RootGone,
            }),
        }
    }

    /// **The cross-cell backlink rollup (§6.2 — ISS portfolio / KN collab / CHAT cross-org).** For a
    /// viewer's cross-cell backlink SET (one pointer per member-cell artifact), resolve EACH in its
    /// home cell and FOLD only the projections the viewer is permitted to see — a tombstone
    /// (denied/gone/erased) does NOT contribute (the viewer cannot see it; the SAME graceful
    /// degradation as same-cell, never a leak of a count the viewer is not entitled to). Returns the
    /// admitted resolutions in input order; the caller renders them.
    ///
    /// This is ONE mechanism over the three §6.2 shapes (ISS rollup, KN collab, CHAT cross-org) — the
    /// `ArtifactType` on each pointer distinguishes the render, never the fan-out algorithm.
    pub fn rollup(
        &self,
        tenant: &TenantId,
        region: &Region,
        pointers: &[CrossCellPointer],
        viewer: &Principal,
    ) -> Vec<Resolution> {
        pointers
            .iter()
            .map(|p| self.resolve_backlink(tenant, region, p, viewer))
            .filter(Resolution::is_projection)
            .collect()
    }

    /// The full per-pointer fan-out (the rollup WITHOUT excluding tombstones) — the drill reads this
    /// to assert a denied/erased cross-cell backlink is a [`Tombstone`] (the leak invariant across the
    /// cell boundary), not merely absent. The render path uses [`Self::rollup`] (tombstones excluded);
    /// the drill uses this to PROVE the tombstone is produced (never a leak) before it is excluded.
    pub fn resolve_all(
        &self,
        tenant: &TenantId,
        region: &Region,
        pointers: &[CrossCellPointer],
        viewer: &Principal,
    ) -> Vec<Resolution> {
        pointers
            .iter()
            .map(|p| self.resolve_backlink(tenant, region, p, viewer))
            .collect()
    }

    /// **CP-D8 telemetry — `refs.cross_cell_resolves`.** How many cross-cell backlink resolves the
    /// fan-out served (aggregate, PII-free).
    pub fn fanned_out(&self) -> u64 {
        self.fanned_out.load(Ordering::SeqCst)
    }

    /// **The CP-D8 ZERO — `refs.cross_cell_raw_rows` carried across the cell boundary.** Pinned to 0
    /// by construction (only the four frozen frame fields cross + a filtered result back); a live
    /// tripwire so a regression that carried a raw row is observable.
    ///
    /// **Equivalent-mutant note (cargo-mutants):** `replace raw_rows_crossed -> 0` is observationally
    /// identical because the fan-out NEVER increments it (the structural guarantee) — the *correct*
    /// property, not a coverage gap. The field + the read seam stay so the tripwire is wired the day a
    /// regression lands (mirrors `myelin_control_plane::CrossCellBridge::cross_cell_raw_rows`).
    pub fn raw_rows_crossed(&self) -> u64 {
        self.raw_rows_crossed.load(Ordering::SeqCst)
    }
}

impl core::fmt::Debug for CrossCellFanOut {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // PII-free Debug: the cell id + the aggregate counters, never a viewer/pointer/projection.
        f.debug_struct("CrossCellFanOut")
            .field("home_cell", &self.home_cell.as_str())
            .field("fanned_out", &self.fanned_out())
            .field("raw_rows_crossed", &self.raw_rows_crossed())
            .finish()
    }
}

/// **The PII-free bridge proof (the CP-D8 body).** What crosses the cell boundary is EXACTLY the four
/// frozen [`CrossCellPointer`] fields (+ the opaque viewer id) — never a raw row, never PII that
/// should stay in the home cell. Extracts the four (opaque, PII-free) fields a CP-D8 proof asserts
/// crossed, so a drill can show "the fan-out carried only `subject`/`type`/`correlation_id`/
/// `home_cell`" with the concrete opaque values. There is structurally no fifth field.
///
/// Mirrors `myelin_control_plane::bridge_carried_fields` — the SAME four-field projection, asserted
/// from the Refs side (one frame, the PII-free proof from both legs).
pub fn fanout_carried_fields(
    pointer: &CrossCellPointer,
) -> (&OpaqueSubjectId, &ArtifactType, &CorrelationId, &CellId) {
    (
        pointer.subject(),
        pointer.artifact_type(),
        pointer.correlation_id(),
        pointer.home_cell(),
    )
}

/// **CP-D7 — cell→cell migration, re-home a backlink pointer with 0 loss.** When a member cell
/// MIGRATES (the artifact is re-homed from `from` to `to`), the cross-cell backlink pointer's
/// `home_cell` is re-stamped to the NEW cell so the fan-out re-dispatches there. ONLY the frame's
/// routing handle changes — the opaque subject / type / correlation are preserved byte-for-byte (no
/// data is lost in the migration; the SAME backlink resolves, now in the new home). Returns a NEW
/// pointer (the frame is read-only — the migration mints the re-homed frame; EI-01 §7, one frame).
///
/// A pointer NOT homed in `from` is returned unchanged (the migration is precise — it re-homes only
/// the cell that actually migrated, never a bystander).
#[must_use]
pub fn migrate_home_cell(
    pointer: &CrossCellPointer,
    from: &CellId,
    to: &CellId,
) -> CrossCellPointer {
    if pointer.home_cell() == from {
        // Re-mint the frame re-homed to `to` — the four-field frame is read-only, so the migration
        // produces a NEW pointer preserving the opaque subject/type/correlation byte-for-byte (0
        // loss) and changing ONLY the routing handle. One frame, one constructor (no second type).
        CrossCellPointer::new(
            pointer.subject().clone(),
            pointer.artifact_type().clone(),
            pointer.correlation_id().clone(),
            to.clone(),
        )
    } else {
        // Not the migrating cell — the pointer is untouched (precise re-home, no bystander churn).
        pointer.clone()
    }
}

/// **GA-D8 — mint the cross-cell erasure receipt for a member cell.** After a member `cell` has
/// erased its references to `subject` (the per-cell crypto-shred/tombstone ran), mint the PII-free
/// [`CrossCellEraseReceipt`] proving that cell ran. The SET of receipts (one per member cell that
/// held a reference) is the GA-D8 green artifact — its presence is "0 holders missed". The receipt
/// carries ONLY the opaque subject + the cell + the `erased` flag — never the name/title.
#[must_use]
pub fn cross_cell_erase_receipt(cell: &CellId, subject: &OpaqueSubjectId) -> CrossCellEraseReceipt {
    CrossCellEraseReceipt {
        cell: cell.clone(),
        subject: subject.clone(),
        erased: true,
    }
}

/// A convenience: build a cross-cell backlink pointer to a subject `ref_` of `kind` homed in `cell`,
/// tied to the causal chain `correlation_id`. Mints the ONE frozen frame (no second pointer type);
/// used by the producers (ISS/KN/CHAT) when they surface a cross-cell backlink, and by the drills.
#[must_use]
pub fn cross_cell_backlink_pointer(
    ref_: &ArtifactRef,
    kind: ArtifactType,
    correlation_id: CorrelationId,
    cell: CellId,
) -> CrossCellPointer {
    CrossCellPointer::new(
        OpaqueSubjectId::from_ref(ref_.clone()),
        kind,
        correlation_id,
        cell,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::Projection;
    use myelin_identity::{PrincipalId, PrincipalKind};
    use std::sync::Mutex;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn cell_a() -> CellId {
        CellId::from_token("cell-fr-par-1")
    }
    fn cell_b() -> CellId {
        CellId::from_token("cell-fr-par-2")
    }
    fn cell_c() -> CellId {
        CellId::from_token("cell-de-fra-1")
    }
    fn viewer(id: &str) -> Principal {
        Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
    }
    fn corr() -> CorrelationId {
        CorrelationId("01J0CORR".into())
    }

    /// A test cell-local resolver standing in for a foreign cell's `ResolveService` (the SAME stand-in
    /// shape the control-plane bridge tests use): it holds a per-(subject, viewer) permission map + a
    /// per-subject rendered projection, permission-checks IN this cell, and returns ONLY the filtered
    /// projection / a tombstone — NEVER a raw row. It records the pointers it was asked so a test can
    /// assert the resolve happened IN the home cell (over the seam, never by A reaching B's rows).
    #[derive(Default)]
    struct ForeignCellResolver {
        /// (subject_urn, viewer_id) pairs allowed to view; everyone else is denied (the leak-test).
        allowed: Mutex<Vec<(String, String)>>,
        /// subject_urns whose references have been ERASED in this cell (resolve → Erased tombstone).
        erased: Mutex<Vec<String>>,
        /// the per-subject rendered projection title (the SECRET that must NOT leak to a denied
        /// cross-cell viewer).
        titles: Mutex<HashMap<String, String>>,
        /// records every pointer subject this cell was asked to resolve (the resolve happened HERE).
        resolved: Mutex<Vec<String>>,
    }

    impl ForeignCellResolver {
        fn allow(&self, subject_urn: &str, viewer_id: &str) {
            self.allowed
                .lock()
                .unwrap()
                .push((subject_urn.into(), viewer_id.into()));
        }
        fn set_title(&self, subject_urn: &str, title: &str) {
            self.titles
                .lock()
                .unwrap()
                .insert(subject_urn.into(), title.into());
        }
        fn erase(&self, subject_urn: &str) {
            self.erased.lock().unwrap().push(subject_urn.into());
        }
        fn resolved_subjects(&self) -> Vec<String> {
            self.resolved.lock().unwrap().clone()
        }
    }

    impl CellLocalBacklinkResolver for ForeignCellResolver {
        fn resolve_backlink_in_cell(
            &self,
            _tenant: &TenantId,
            _region: &Region,
            pointer: &CrossCellPointer,
            viewer: &Principal,
        ) -> Resolution {
            let subject_urn = pointer.subject().artifact_ref().0.clone();
            self.resolved.lock().unwrap().push(subject_urn.clone());
            // ERASED in this cell → an Erased tombstone (the person unresolvable cross-cell, GA-D8).
            if self
                .erased
                .lock()
                .unwrap()
                .iter()
                .any(|e| e == &subject_urn)
            {
                return Resolution::Tombstone(Tombstone {
                    root: pointer.subject().artifact_ref().clone(),
                    reason: TombstoneReason::Erased,
                });
            }
            // Permission-checked IN this cell against THIS cell's allow-map (cell A never sees it).
            let allowed = self
                .allowed
                .lock()
                .unwrap()
                .iter()
                .any(|(s, v)| s == &subject_urn && v == &viewer.principal_id.0);
            if !allowed {
                // Denied → a Denied tombstone carrying ONLY the opaque root — never the title (the
                // leak invariant, now cross-cell). The secret title NEVER crosses for a denied viewer.
                return Resolution::Tombstone(Tombstone {
                    root: pointer.subject().artifact_ref().clone(),
                    reason: TombstoneReason::Denied,
                });
            }
            // Allowed → the filtered projection crosses back (the title the viewer IS entitled to).
            let title = self
                .titles
                .lock()
                .unwrap()
                .get(&subject_urn)
                .cloned()
                .unwrap_or_else(|| "untitled".into());
            Resolution::Projection(Projection {
                ref_: pointer.subject().artifact_ref().clone(),
                title,
                state: "open".into(),
                icon: "issue".into(),
                render_hint: "issue-card".into(),
                sub_anchor: None,
                flag: None,
            })
        }
    }

    fn issue_in(cell_token: &str, key: &str, cell: CellId) -> CrossCellPointer {
        cross_cell_backlink_pointer(
            &ArtifactRef(format!("myelin://acme/issues/issue/{cell_token}-{key}")),
            ArtifactType::Issue,
            corr(),
            cell,
        )
    }

    // ── CP-D8 / the leak invariant across the cell boundary ──

    /// **A cross-cell backlink resolves IN the home cell; only the projection/tombstone crosses — a
    /// denied cross-cell viewer gets a tombstone carrying NO content (the leak invariant, cross-cell;
    /// CP-D8).** A viewer in cell A resolving a pointer homed in cell B: permitted → the projection
    /// crosses; denied → a tombstone with NO title (the secret never crosses); and raw_rows_crossed
    /// stays 0 (only the four-field frame + the opaque viewer crossed).
    #[test]
    fn cross_cell_backlink_resolves_in_home_cell_denied_gets_tombstone_zero_leak() {
        let b = Arc::new(ForeignCellResolver::default());
        let secret = "TOP SECRET cross-org acquisition";
        let p = issue_in("b", "42", cell_b());
        b.set_title(&p.subject().artifact_ref().0, secret);
        b.allow(&p.subject().artifact_ref().0, "insider");

        let mut fanout = CrossCellFanOut::new(cell_a());
        fanout.register(cell_b(), b.clone());

        // permitted viewer → the projection crosses back (resolved IN cell B).
        let allowed = fanout.resolve_backlink(&tenant(), &region(), &p, &viewer("insider"));
        assert!(
            allowed.is_projection(),
            "the permitted viewer sees the cross-cell projection"
        );
        if let Resolution::Projection(proj) = &allowed {
            assert_eq!(
                proj.title, secret,
                "the permitted viewer is entitled to the title"
            );
        }

        // denied viewer → a tombstone carrying NO content (the secret never crosses).
        let denied = fanout.resolve_backlink(&tenant(), &region(), &p, &viewer("intruder"));
        assert!(
            denied.is_tombstone(),
            "the denied cross-cell viewer gets a tombstone"
        );
        assert_eq!(denied.tombstone_reason(), Some(TombstoneReason::Denied));
        let rendered = format!("{denied:?}");
        assert!(
            !rendered.contains("SECRET") && !rendered.contains("acquisition"),
            "0 leak across the cell boundary: the secret must not appear, got `{rendered}`"
        );

        // the resolve happened IN cell B (over the seam) — cell A never read B's rows.
        assert_eq!(
            b.resolved_subjects(),
            vec![
                p.subject().artifact_ref().0.clone(),
                p.subject().artifact_ref().0.clone()
            ],
            "both resolves dispatched to cell B (the home cell), not resolved in A"
        );
        // CP-D8 zero: only the four-field frame + the opaque viewer crossed — never a raw row.
        assert_eq!(
            fanout.raw_rows_crossed(),
            0,
            "0 raw rows / PII crossed the cell boundary"
        );
        assert_eq!(
            fanout.fanned_out(),
            2,
            "two cross-cell resolves were served"
        );
    }

    /// **The home-cell pointer resolves locally (no bridge hop) over the SAME seam.** A pointer homed
    /// in cell A (this fan-out's cell) dispatches to A's own registered resolver — one path, no
    /// special-case; the leak invariant is identical.
    #[test]
    fn home_cell_pointer_resolves_locally_over_the_same_seam() {
        let a = Arc::new(ForeignCellResolver::default());
        let p = issue_in("a", "1", cell_a());
        a.set_title(&p.subject().artifact_ref().0, "local issue");
        a.allow(&p.subject().artifact_ref().0, "insider");

        let mut fanout = CrossCellFanOut::new(cell_a());
        fanout.register(cell_a(), a.clone());

        let r = fanout.resolve_backlink(&tenant(), &region(), &p, &viewer("insider"));
        assert!(r.is_projection(), "the home-cell pointer resolves locally");
        assert_eq!(
            a.resolved_subjects().len(),
            1,
            "resolved over the home-cell's own seam"
        );
        assert_eq!(fanout.raw_rows_crossed(), 0);
    }

    /// **A pointer homed in a cell UNKNOWN to the fan-out degrades to a tombstone (never fabricate
    /// content, never reach into an unseen cell).** The conservative non-leaking failure: no resolver
    /// registered for the home cell → a root-carrying tombstone, 0 raw rows crossed.
    #[test]
    fn unknown_home_cell_degrades_to_tombstone_never_reaches_in() {
        let p = issue_in("c", "9", cell_c());
        let fanout = CrossCellFanOut::new(cell_a()); // cell C not registered
        let r = fanout.resolve_backlink(&tenant(), &region(), &p, &viewer("anyone"));
        assert!(
            r.is_tombstone(),
            "an unknown home cell degrades to a tombstone"
        );
        assert_eq!(r.tombstone_reason(), Some(TombstoneReason::RootGone));
        assert_eq!(
            fanout.raw_rows_crossed(),
            0,
            "no raw row crossed for an unseen cell"
        );
    }

    // ── §6.2 the rollup (ISS portfolio / KN collab / CHAT cross-org) — one mechanism ──

    /// **The cross-cell portfolio rollup folds only the projections the viewer may see across member
    /// cells (§6.2 — ISS rollup / KN collab / CHAT cross-org).** A viewer with backlinks in cells B
    /// and C: the permitted ones fold in (in input order); the denied/erased ones are EXCLUDED (the
    /// viewer cannot see them — never a leak of a count they are not entitled to).
    #[test]
    fn rollup_folds_only_permitted_projections_across_member_cells() {
        let b = Arc::new(ForeignCellResolver::default());
        let c = Arc::new(ForeignCellResolver::default());

        let p_b_ok = issue_in("b", "ok", cell_b());
        let p_b_denied = issue_in("b", "secret", cell_b());
        let p_c_ok = issue_in("c", "ok", cell_c());

        b.set_title(&p_b_ok.subject().artifact_ref().0, "B visible");
        b.allow(&p_b_ok.subject().artifact_ref().0, "viewer1");
        b.set_title(&p_b_denied.subject().artifact_ref().0, "B SECRET");
        // p_b_denied: NOT allowed for viewer1 → excluded.
        c.set_title(&p_c_ok.subject().artifact_ref().0, "C visible");
        c.allow(&p_c_ok.subject().artifact_ref().0, "viewer1");

        let mut fanout = CrossCellFanOut::new(cell_a());
        fanout.register(cell_b(), b);
        fanout.register(cell_c(), c);

        let set = vec![p_b_ok.clone(), p_b_denied, p_c_ok.clone()];
        let rollup = fanout.rollup(&tenant(), &region(), &set, &viewer("viewer1"));
        let titles: Vec<String> = rollup
            .iter()
            .filter_map(|r| match r {
                Resolution::Projection(p) => Some(p.title.clone()),
                Resolution::Tombstone(_) => None,
            })
            .collect();
        assert_eq!(
            titles,
            vec!["B visible".to_string(), "C visible".to_string()],
            "only the permitted cross-cell backlinks fold in (in input order); the denied is excluded"
        );
        // three resolves served (one per pointer), 0 raw rows crossed.
        assert_eq!(fanout.fanned_out(), 3);
        assert_eq!(fanout.raw_rows_crossed(), 0);
    }

    /// **`resolve_all` produces a TOMBSTONE for the denied cross-cell backlink (the leak invariant,
    /// before the rollup excludes it).** Proves the denied backlink is a tombstone, not merely absent
    /// — the rollup then drops it, but the resolve itself never leaks.
    #[test]
    fn resolve_all_tombstones_the_denied_before_rollup_excludes_it() {
        let b = Arc::new(ForeignCellResolver::default());
        let p_ok = issue_in("b", "ok", cell_b());
        let p_denied = issue_in("b", "secret", cell_b());
        b.set_title(&p_ok.subject().artifact_ref().0, "ok");
        b.allow(&p_ok.subject().artifact_ref().0, "v");
        b.set_title(&p_denied.subject().artifact_ref().0, "SECRET");

        let mut fanout = CrossCellFanOut::new(cell_a());
        fanout.register(cell_b(), b);

        let all = fanout.resolve_all(&tenant(), &region(), &[p_ok, p_denied], &viewer("v"));
        assert_eq!(all.len(), 2);
        assert!(all[0].is_projection());
        assert!(
            all[1].is_tombstone(),
            "the denied backlink is a tombstone, not absent"
        );
        assert_eq!(all[1].tombstone_reason(), Some(TombstoneReason::Denied));
    }

    // ── CP-D7 cell→cell migration, 0 loss ──

    /// **After a member cell migrates (home re-homed B → C), the fan-out re-dispatches to the NEW home
    /// and resolves the SAME backlink with 0 loss (CP-D7).** The pointer's `home_cell` is re-stamped;
    /// the opaque subject/type/correlation are preserved byte-for-byte; the resolve now lands in cell
    /// C and serves the SAME projection — 0 dropped backlinks.
    #[test]
    fn cell_to_cell_migration_re_homes_the_pointer_zero_loss() {
        // before migration the artifact is homed in B; after, in C (the SAME artifact, re-homed).
        let p = issue_in("b", "42", cell_b());
        let secret = "migrated issue";

        // cell B resolver (pre-migration home) and cell C resolver (post-migration home).
        let b = Arc::new(ForeignCellResolver::default());
        let c = Arc::new(ForeignCellResolver::default());
        b.set_title(&p.subject().artifact_ref().0, secret);
        b.allow(&p.subject().artifact_ref().0, "owner");
        c.set_title(&p.subject().artifact_ref().0, secret);
        c.allow(&p.subject().artifact_ref().0, "owner");

        let mut fanout = CrossCellFanOut::new(cell_a());
        fanout.register(cell_b(), b.clone());
        fanout.register(cell_c(), c.clone());

        // pre-migration: resolves in B.
        let before = fanout.resolve_backlink(&tenant(), &region(), &p, &viewer("owner"));
        assert!(before.is_projection());
        assert_eq!(
            b.resolved_subjects().len(),
            1,
            "pre-migration resolve landed in B"
        );
        assert_eq!(c.resolved_subjects().len(), 0);

        // MIGRATE the home B → C (re-home the pointer; the four-field frame is preserved except the
        // routing handle).
        let migrated = migrate_home_cell(&p, &cell_b(), &cell_c());
        assert_eq!(migrated.home_cell(), &cell_c(), "the pointer re-homed to C");
        // 0 loss: the opaque subject/type/correlation are preserved byte-for-byte.
        assert_eq!(
            migrated.subject(),
            p.subject(),
            "the subject is preserved (0 loss)"
        );
        assert_eq!(migrated.artifact_type(), p.artifact_type());
        assert_eq!(migrated.correlation_id(), p.correlation_id());

        // post-migration: resolves in C, SAME projection — 0 dropped backlinks.
        let after = fanout.resolve_backlink(&tenant(), &region(), &migrated, &viewer("owner"));
        assert!(
            after.is_projection(),
            "the re-homed backlink resolves with 0 loss"
        );
        assert_eq!(
            c.resolved_subjects().len(),
            1,
            "post-migration resolve landed in C"
        );
        if let (Resolution::Projection(pb), Resolution::Projection(pc)) = (&before, &after) {
            assert_eq!(
                pb.title, pc.title,
                "the SAME projection — 0 loss in the migration"
            );
        }
    }

    /// **A pointer NOT homed in the migrating cell is untouched (precise re-home, no bystander
    /// churn).** Migrating B → C does not re-home a pointer homed in cell A.
    #[test]
    fn migration_leaves_non_migrating_pointers_untouched() {
        let p_a = issue_in("a", "1", cell_a());
        let migrated = migrate_home_cell(&p_a, &cell_b(), &cell_c());
        assert_eq!(
            migrated.home_cell(),
            &cell_a(),
            "a non-migrating pointer is untouched"
        );
        assert_eq!(migrated, p_a, "the pointer is unchanged byte-for-byte");
    }

    // ── GA-D8 the cross-cell erasure receipt set ──

    /// **Per-cell erasure yields a receipt SET; after the per-cell erase the subject resolves to an
    /// ERASED tombstone in EVERY member cell (the person unresolvable cross-cell, 0 holders missed;
    /// GA-D8).** Erase a subject in cells B and C; mint a receipt per cell; then assert the fan-out
    /// resolves that subject to `Erased` in both — the receipt SET is the GA-D8 green artifact.
    #[test]
    fn cross_cell_erase_yields_receipt_set_and_subject_unresolvable_in_every_cell() {
        let b = Arc::new(ForeignCellResolver::default());
        let c = Arc::new(ForeignCellResolver::default());
        let p_b = issue_in("b", "victim", cell_b());
        let p_c = issue_in("c", "victim", cell_c());
        // the SAME opaque subject referenced from both cells (a cross-cell person).
        let subject = p_b.subject().clone();

        // before erase: visible to the owner in both cells.
        b.set_title(&p_b.subject().artifact_ref().0, "B ref");
        b.allow(&p_b.subject().artifact_ref().0, "owner");
        c.set_title(&p_c.subject().artifact_ref().0, "C ref");
        c.allow(&p_c.subject().artifact_ref().0, "owner");

        let mut fanout = CrossCellFanOut::new(cell_a());
        fanout.register(cell_b(), b.clone());
        fanout.register(cell_c(), c.clone());

        // ERASE in each member cell (the per-cell crypto-shred/tombstone), minting a receipt per cell.
        b.erase(&p_b.subject().artifact_ref().0);
        c.erase(&p_c.subject().artifact_ref().0);
        let receipts = vec![
            cross_cell_erase_receipt(&cell_b(), &subject),
            cross_cell_erase_receipt(&cell_c(), &subject),
        ];
        // the GA-D8 receipt SET: one per member cell, every receipt `erased = true` (0 holders missed).
        assert_eq!(
            receipts.len(),
            2,
            "a receipt per member cell that held a reference"
        );
        for r in &receipts {
            assert!(
                r.erased,
                "every member cell ran the erase (0 holders missed)"
            );
            assert_eq!(
                r.subject, subject,
                "the receipt names the erased opaque subject"
            );
        }
        // PII-free: the receipt carries only the opaque subject (never a name/title).
        let rendered = format!("{receipts:?}");
        assert!(
            !rendered.contains("ref"),
            "the receipt is PII-free (no title), got `{rendered}`"
        );

        // after the per-cell erase: the subject is UNRESOLVABLE in EVERY member cell (Erased
        // tombstone) — the person unresolvable cross-cell.
        let r_b = fanout.resolve_backlink(&tenant(), &region(), &p_b, &viewer("owner"));
        let r_c = fanout.resolve_backlink(&tenant(), &region(), &p_c, &viewer("owner"));
        assert_eq!(
            r_b.tombstone_reason(),
            Some(TombstoneReason::Erased),
            "unresolvable in B"
        );
        assert_eq!(
            r_c.tombstone_reason(),
            Some(TombstoneReason::Erased),
            "unresolvable in C"
        );
        assert_eq!(
            fanout.raw_rows_crossed(),
            0,
            "no PII crossed even on the erased path"
        );
    }

    // ── The PII-free four-field projection (CP-D8 body) ──

    /// **The fan-out's `Debug` is PII-free and carries the cell id + the aggregate counters.** A
    /// regression that dropped the body (or leaked a viewer/pointer) is caught: the rendered Debug
    /// carries the home cell id + the live `fanned_out`/`raw_rows_crossed` counters, never a viewer id
    /// or a pointer.
    #[test]
    fn fanout_debug_is_pii_free_and_carries_the_counters() {
        let b = Arc::new(ForeignCellResolver::default());
        let p = issue_in("b", "42", cell_b());
        b.set_title(&p.subject().artifact_ref().0, "t");
        b.allow(&p.subject().artifact_ref().0, "v");
        let mut fanout = CrossCellFanOut::new(cell_a());
        fanout.register(cell_b(), b);
        let _ = fanout.resolve_backlink(&tenant(), &region(), &p, &viewer("v"));
        let rendered = format!("{fanout:?}");
        assert!(
            rendered.contains("CrossCellFanOut"),
            "the Debug names the type"
        );
        assert!(
            rendered.contains("cell-fr-par-1"),
            "the Debug carries the home cell id"
        );
        assert!(
            rendered.contains("fanned_out"),
            "the Debug carries the resolve counter"
        );
        assert!(
            rendered.contains("raw_rows_crossed"),
            "the Debug carries the CP-D8 zero counter"
        );
        // PII-free: the Debug never carries the viewer id (`v`-as-a-field) or the pointer subject.
        assert!(
            !rendered.contains("issues/issue"),
            "the Debug never leaks a pointer subject, got `{rendered}`"
        );
    }

    /// **The fan-out carries EXACTLY the four frozen frame fields across (CP-D8 body).** The PII-free
    /// proof: what crosses is `subject`/`type`/`correlation_id`/`home_cell` — there is structurally no
    /// fifth field. Mirrors the control-plane bridge's identical four-field projection.
    #[test]
    fn fanout_carries_exactly_the_four_frozen_frame_fields() {
        let p = issue_in("b", "42", cell_b());
        let (subject, ty, corr_id, home) = fanout_carried_fields(&p);
        assert_eq!(subject.artifact_ref().0, "myelin://acme/issues/issue/b-42");
        assert_eq!(ty, &ArtifactType::Issue);
        assert_eq!(corr_id, &corr());
        assert_eq!(home, &cell_b());
    }
}

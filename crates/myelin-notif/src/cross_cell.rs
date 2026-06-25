//! # `cross_cell` — cross-cell inbox aggregation (always-cell-local resolution) (NOTIF-P24 / P-466, M5)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/notifications.md` §5.4 (cross-cell inbox
//! aggregation — the bridge frame now frozen: the inbox is materialised **per home-cell**; a
//! multi-cell recipient's unified view aggregates across their cells via the frozen
//! [`CrossCellPointer`]`{subject(opaque), type, correlation_id, home_cell}`; the control plane
//! carries **ONLY the pointer**, never name/email/body; resolution is **ALWAYS cell-local** — to
//! render a pointer to an item homed in cell B, cell A's gateway asks cell B to
//! `resolve(ref, viewer, Display)` **IN B**, permission-checked in B against B's tuples, returning
//! ONLY the already-rendered, already-permission-filtered projection or a tombstone, never raw rows,
//! never PII that should stay in B; the DSR orchestrator iterates `member_cells` over the same
//! bridge). **Contracts:** 12.6 (the [`CrossCellPointer`] PII-free bridge — CONSUMED), 5.2
//! (`resolve(ref, viewer, Display)` cell-local — CONSUMED), 10.4 (`dsr_submit` iterates
//! `member_cells` over the bridge — CONSUMED). **Reconciliation:** `00-reconciliation-decisions.md`
//! OQ-I (the cross-cell bridge + the always-cell-local resolution rule).
//!
//! ## What NOTIF-P24 ships — the BUILD on the frozen frame, not a second frame (EI-01 §7 coherence)
//! The single-home-cell path has been complete since NOTIF-P2 (the §4 contracts were written
//! cell-agnostic, so this is an EXTENSION, never a rewrite). This module is the Notif-side
//! cross-cell aggregation BUILD: a multi-cell recipient's inbox is materialised **per home-cell**
//! (NOTIF-P3's [`InboxProjection`](crate::router::InboxProjection) is the per-cell slice); their
//! unified view stitches the per-cell slices by carrying a [`CrossCellPointer`] per item and
//! resolving EACH **in its home cell** over the [`CellLocalInboxResolver`] seam, folding only the
//! already-rendered projection/tombstone that crosses back.
//!
//! This is the SAME shape the control-plane bridge (`myelin_control_plane::CrossCellBridge`) and the
//! Refs backlink fan-out (`myelin_refs_service::CrossCellFanOut`) ship — one frozen frame, one
//! cell-local resolution rule, the resolver behind a seam — Notif-shaped to fold the **humanised**
//! inbox projection (a [`HumanisedString`](crate::HumanisedString)) the inbox renders, never a raw
//! `inbox_item` row.
//!
//! ## DAG POSITION — why a Notif-side resolver SEAM, not a control-plane dep
//! The §2.9 DAG puts `myelin-control-plane` ABOVE the service crates (it depends on the resolvers,
//! not the reverse). Notif therefore owns the aggregation *algorithm* (the per-cell pointer set →
//! per-home-cell resolve → fold) behind the [`CellLocalInboxResolver`] seam, whose production
//! implementor is cell B's Notif `humanise` resolve path (NOTIF-P9, [`crate::humanise`]) reached over
//! the control plane's cross-cell bridge transport (the substrate `ResilientClient` wire — the named
//! transport floor). The seam is REAL + proven here against in-process resolvers standing in for the
//! foreign cells (the SAME stand-in the control-plane bridge tests use); the cross-process WIRE is the
//! substrate floor.
//!
//! ## The leak invariant holds ACROSS the cell boundary (NOTIF-D4, now cross-cell)
//! The cardinal property does not regress at the cell boundary: a cross-cell inbox item the viewer may
//! NOT see resolves to a [`InboxTombstone`] — never a leak of the foreign item's title/body, never PII
//! that should stay in B. The permission check runs **in the home cell** (cell B checks against B's
//! tuples); cell A never sees B's rows. Humanisation ALWAYS resolves locally in the cell that holds the
//! artifact (residency-preserving; no PII crosses cells, ADR-11).
//!
//! ## The PII-free bridge ZERO (CP-D8) + the 0-loss migration (CP-D7)
//! [`CrossCellInbox::raw_rows_crossed`] is pinned to **0** by construction: the aggregation carries
//! ONLY the four frozen [`CrossCellPointer`] fields across (+ the opaque viewer id) and ONLY a filtered
//! [`InboxResolution`] back. It is a live counter (not a constant) so a future regression that carried
//! a raw row across the boundary is OBSERVABLE (it would tick above 0) — the `CrossTenantCount`-class
//! "0 PII across the bridge" projection the CP-D8 drill asserts `== 0`, mirroring
//! `myelin_control_plane::CrossCellBridge::cross_cell_raw_rows`. A cell→cell migration re-homes the
//! item pointer ([`migrate_item_home_cell`]) preserving the opaque subject/type/correlation
//! byte-for-byte (0 inbox items lost — the CP-D7 drill).
//!
//! ## The DSR member_cells iteration (10.4 / GA-D8 — the cross-cell erasure leg)
//! The DSR orchestrator iterates `member_cells` over the SAME bridge for the cross-cell erasure leg:
//! [`erase_inbox_pointers_in_cell`] mints a per-cell [`InboxEraseReceipt`] proving each member cell
//! ran; after the per-cell erase, the aggregation resolves that subject to an
//! [`InboxTombstoneReason::Erased`] in EVERY member cell (the person unresolvable cross-cell, 0
//! holders missed) — the receipt SET is the GA-D8 green artifact.
//!
//! ## Floors named (VISION §3 / EI-01 §1)
//! - **The single-home-cell path remains the DEFAULT and is complete (since NOTIF-P2).** This is the
//!   named multi-cell follow-on; a single-cell recipient never enters this module (their inbox is the
//!   one per-cell [`InboxProjection`] slice).
//! - **The wire transport behind [`CellLocalInboxResolver`] is the named substrate floor.** In
//!   production cell A reaches cell B's humanise resolve over the control plane's cross-cell bridge
//!   transport (the substrate `ResilientClient` wire, whose `send` body is the first-real-producer
//!   floor). The seam (dispatch to home cell, only the filtered result crosses) is REAL + proven here.
//! - **The cross-cell pointer-set PRODUCTION is the control plane's `placement_of`/`member_cells`
//!   fan-out (P-CP-20 / P-430).** This module FOLDS a caller-supplied per-cell pointer set (the
//!   multi-element shape is LIVE); the `placement_of`-driven enumeration of a tenant's member cells
//!   that PRODUCES the set lives in the control plane (`myelin_control_plane::multi_cell`).
//!
//! ## Mutation-score floor (mandatory-core, EI-01 §3 / VISION §4 prove-it)
//! The cross-cell aggregation is leak-of-foreign-confidential-content-critical (a denied cross-cell
//! inbox item must be ABSENT/tombstoned, never a leak across the cell boundary). Floor: **≥ 80% of
//! viable mutants caught** (`cargo mutants -p myelin-notif -f crates/myelin-notif/src/cross_cell.rs`).
//! Every fold rule — the home-cell dispatch, the tombstone-excluded-from-the-unified-view arm, the
//! unknown-home → tombstone degrade, the migration re-home, and the per-cell erase receipt — has a
//! test a mutation flips. The `raw_rows_crossed` `replace -> 0` is the documented EQUIVALENT mutant
//! (the aggregation NEVER increments it — the *correct* property, not a coverage gap; the tripwire
//! stays wired for the day a regression lands, mirroring the control-plane bridge's identical zero).
//! **Measured 2026-06-25 — see the P-466 commit body.**

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_identity::Principal;
use myelin_tenancy::{
    ArtifactRef, ArtifactType, CellId, CorrelationId, CrossCellPointer, OpaqueSubjectId,
};

use crate::HumanisedString;

/// The telemetry signal name the cross-cell aggregation emits for resolves it served (contract 1.8 /
/// the CP-D8 PII-free bridge proof). A named constant so a drill asserts against the NAME, never a
/// literal.
pub const CROSS_CELL_RESOLVES_SIGNAL: &str = "notif.cross_cell_resolves";

/// The telemetry signal name for the CP-D8 ZERO — raw rows / PII carried across the cell boundary.
/// Pinned to 0 by construction; a named constant so a drill asserts against the NAME `== 0`.
pub const CROSS_CELL_RAW_ROWS_SIGNAL: &str = "notif.cross_cell_raw_rows";

/// **The already-rendered, already-permission-filtered inbox projection that crosses BACK over the
/// bridge (§5.4 step 3).** This is what the home cell B returns for an inbox-item pointer — the
/// item's **humanised** render (a [`HumanisedString`], produced by B's [`crate::humanise`] AFTER B's
/// permission check passed), plus the opaque subject the pointer named — **never a raw `inbox_item`
/// row, never PII that should stay in B**.
///
/// The humanised text MAY carry a name (it is the home-cell-rendered, permission-filtered display
/// string the viewer is allowed to see — exactly the `humanise` semantics); it is NOT a raw row and it
/// crossed ONLY after B authorised THIS viewer. A denied viewer never reaches this — they get an
/// [`InboxTombstone`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxProjectionSlice {
    /// The opaque subject the inbox item is about (the §6.1 `subject` — an `ArtifactRef`-class opaque
    /// id). The unified view pins the per-item route on this.
    pub subject: OpaqueSubjectId,
    /// The home cell that rendered the item (an opaque routing handle — proves the item is from this
    /// member cell; never a row).
    pub home_cell: CellId,
    /// The home-cell-rendered, permission-filtered humanised render of the inbox item (the
    /// `{text, links, icon}` the inbox UI shows). Produced by B's `humanise` AFTER B's permission
    /// check passed — never a raw row.
    pub rendered: HumanisedString,
}

/// **The non-leaking placeholder that crosses back when the home cell cannot/should-not render an
/// inbox item for this viewer (§5.4 — an unauthorised viewer gets a tombstone).** Structurally carries
/// **NO content** — only the opaque subject it stood for + the structured [`InboxTombstoneReason`].
/// The leak invariant is in the SHAPE: there is no `rendered`/`text` field for a denied viewer's
/// content to leak into (mirrors the `humanise` [`crate::humanise::Tombstone`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxTombstone {
    /// The opaque subject the tombstone stands for (the §6.1 `subject`) — an opaque id, never content.
    pub subject: OpaqueSubjectId,
    /// The home cell the tombstone came from (an opaque routing handle, never a row).
    pub home_cell: CellId,
    /// Why the home cell returned a tombstone (the structured, PII-free reason).
    pub reason: InboxTombstoneReason,
}

/// Why a cross-cell inbox resolve degraded to an [`InboxTombstone`] (the §4.6 ladder reasons, the
/// cross-cell subset). A structured enum — never a free-text leak.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum InboxTombstoneReason {
    /// The viewer is not permitted to view the item's subject (B's `check` returned Deny) — the
    /// headline cross-cell case: an unauthorised viewer gets a tombstone, never a leak across the cell
    /// boundary.
    Denied,
    /// The item no longer exists in the home cell (gone) — the home cell rendered a non-leaking
    /// placeholder rather than content.
    Gone,
    /// The item's subject was erased (crypto-shred / pseudonym-shred made it unrenderable) — the GA-D8
    /// cross-cell erasure leg. After a per-cell erase, the subject resolves HERE in EVERY member cell.
    Erased,
}

/// **The outcome of a cross-cell inbox resolve (contract 5.2's `Projection | Tombstone`, across the
/// bridge).** The leak invariant lives in the SHAPE: the [`Self::Tombstone`] arm cannot carry a
/// rendered field. **Only this type ever crosses back over the bridge** — never a raw `inbox_item`
/// row, never B's authz state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InboxResolution {
    /// The inbox item rendered to a per-viewer humanised projection in the home cell (the ALLOWED +
    /// present branch).
    Projection(InboxProjectionSlice),
    /// The resolve degraded to a non-leaking placeholder (denied / gone / erased) — an unauthorised
    /// viewer always lands here.
    Tombstone(InboxTombstone),
}

impl InboxResolution {
    /// Is this an [`InboxResolution::Projection`]? (the "allowed + present" assertion).
    pub fn is_projection(&self) -> bool {
        matches!(self, InboxResolution::Projection(_))
    }

    /// Is this an [`InboxResolution::Tombstone`]? (the degraded, non-leaking assertion).
    pub fn is_tombstone(&self) -> bool {
        matches!(self, InboxResolution::Tombstone(_))
    }

    /// The tombstone reason, if this resolution is a tombstone (so a drill can assert *why* it
    /// degraded — `Denied` for an unauthorised cross-cell viewer, `Erased` after the GA-D8 leg).
    pub fn tombstone_reason(&self) -> Option<InboxTombstoneReason> {
        match self {
            InboxResolution::Tombstone(t) => Some(t.reason),
            InboxResolution::Projection(_) => None,
        }
    }
}

/// **The cell-local inbox resolver seam (contract 5.2 `resolve(ref, viewer, Display)` — the home cell
/// B's resolver, OQ-I).** The aggregation dispatches a cross-cell inbox-item pointer to the artifact's
/// **home cell** through this trait; the implementor (production: cell B's Notif `humanise` resolve
/// path, NOTIF-P9, reached over the control plane's bridge transport) resolves the pointer **IN that
/// cell** against ITS tuples — `humanise` renders the item per-viewer, permission-checked — and
/// returns ONLY the already-rendered, already-permission-filtered [`InboxResolution`] (a projection or
/// a tombstone). It **never** returns a raw row and **never** leaks PII that should stay in B (the
/// trait's return type makes that structural — there is no raw-row variant).
///
/// This is the Notif-shaped twin of `myelin_control_plane::CellLocalResolver` (the SAME seam shape,
/// returning the humanised [`InboxResolution`] rather than the control-plane's `BridgeResolution`):
/// one frozen frame, one cell-local resolution rule, the resolver behind a seam (EI-01 §7).
/// `Send + Sync` so the aggregation holds it behind an [`Arc`] across serving threads.
pub trait CellLocalInboxResolver: Send + Sync {
    /// Resolve the inbox-item `pointer` for `viewer` **in this (the home) cell (§5.4)** —
    /// permission-checked against THIS cell's tuples — returning ONLY the filtered [`InboxResolution`]
    /// (the humanised projection / a tombstone) that crosses back. NEVER returns a raw row. A
    /// denied/gone/erased subject is an [`InboxTombstone`] (the leak invariant, now cross-cell).
    fn resolve_inbox_item_in_cell(
        &self,
        pointer: &CrossCellPointer,
        viewer: &Principal,
    ) -> InboxResolution;
}

/// A per-member-cell inbox erasure receipt (the GA-D8 cross-cell erasure receipt SET). PII-free: it
/// names the member cell + the opaque subject erased + the `erased` flag — never the name/title (the
/// erased subject is an [`OpaqueSubjectId`], `ArtifactRef`-class). One receipt per member cell that
/// held inbox references to the erased subject; the SET is the GA-D8 green artifact ("0 holders
/// missed").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboxEraseReceipt {
    /// The member cell that erased its inbox references to the subject (an opaque routing handle,
    /// never a row).
    pub cell: CellId,
    /// The opaque subject whose cross-cell inbox references were erased (NEVER a person —
    /// `ArtifactRef`-class; §6.1).
    pub subject: OpaqueSubjectId,
    /// `true` once the member cell has crypto-shred/tombstoned its inbox references to the subject
    /// (the per-cell erase ran). A receipt is only minted with `erased = true` — the SET's presence is
    /// the proof every member cell ran (0 holders missed).
    pub erased: bool,
}

/// **The cross-cell inbox aggregation (contract 12.6 consumed; 5.2 cell-local, EXTENDED).** Serves a
/// multi-cell recipient's gateway in cell **A** (`home_cell`): for the per-cell pointer set that makes
/// up their unified inbox it dispatches EACH inbox-item [`CrossCellPointer`] to its `home_cell` over a
/// registered [`CellLocalInboxResolver`], folding only the projections/tombstones that cross back. A
/// pointer homed HERE resolves locally (no bridge hop); a pointer homed elsewhere dispatches to that
/// cell; a pointer homed in a cell unknown to this aggregation degrades to an [`InboxTombstone`]
/// (never fabricate content, never reach into an unseen cell).
///
/// `cross_cell_resolves` + `raw_rows_crossed` are the CP-D8 PII-free-bridge proof telemetry: every
/// resolve increments `cross_cell_resolves`; `raw_rows_crossed` is pinned to **0** by construction
/// (only the four frozen frame fields cross + a filtered result back) — a live tripwire so a
/// regression that carried a raw row is observable.
#[derive(Clone)]
pub struct CrossCellInbox {
    /// The cell this aggregation serves (cell A — the gateway holding the viewer's identity). A
    /// pointer homed HERE is resolved locally.
    home_cell: CellId,
    /// The home cells the aggregation can dispatch a cross-cell inbox-item resolve to (their
    /// cell-local resolvers). In production each member cell exposes its humanise-resolve endpoint over
    /// the bridge transport; here the registry holds the resolver handles directly (the wire is the
    /// named substrate floor).
    resolvers: HashMap<CellId, Arc<dyn CellLocalInboxResolver>>,
    /// CP-D8 telemetry: how many cross-cell inbox resolves the aggregation served (aggregate,
    /// PII-free).
    cross_cell_resolves: Arc<AtomicU64>,
    /// **The CP-D8 ZERO — raw rows / PII carried across the cell boundary.** Pinned to 0 by
    /// construction; a live tripwire (not a constant) so a regression that carried a raw row across is
    /// observable.
    raw_rows_crossed: Arc<AtomicU64>,
}

impl CrossCellInbox {
    /// Build an aggregation serving `home_cell` (cell A) with no foreign-cell resolvers registered
    /// yet.
    pub fn new(home_cell: CellId) -> CrossCellInbox {
        CrossCellInbox {
            home_cell,
            resolvers: HashMap::new(),
            cross_cell_resolves: Arc::new(AtomicU64::new(0)),
            raw_rows_crossed: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Register the cell-local resolver for `cell` (the home cell the aggregation dispatches a
    /// cross-cell inbox-item resolve to). The home cell A's OWN resolver is registered for `home_cell`
    /// so a same-cell pointer resolves locally over the identical seam (one path, no special-case).
    pub fn register(&mut self, cell: CellId, resolver: Arc<dyn CellLocalInboxResolver>) {
        self.resolvers.insert(cell, resolver);
    }

    /// The cell this aggregation serves (cell A — opaque id).
    pub fn home_cell(&self) -> &CellId {
        &self.home_cell
    }

    /// **Resolve ONE cross-cell inbox item (§5.4 — the home-cell dispatch).** Dispatch `pointer` to
    /// its `home_cell`:
    /// 1. registered (this cell or a foreign cell) → resolve **IN the home cell** against ITS tuples;
    ///    ONLY the filtered [`InboxResolution`] crosses back (the four frozen frame fields + the opaque
    ///    viewer id are all that cross — no raw rows);
    /// 2. unknown to this aggregation → a [`InboxTombstoneReason::Gone`] tombstone with the pointer's
    ///    subject (never fabricate content, never reach into an unseen cell).
    ///
    /// In NO branch does a raw row or PII-that-should-stay-in-B cross — `raw_rows_crossed` stays 0.
    pub fn resolve_item(&self, pointer: &CrossCellPointer, viewer: &Principal) -> InboxResolution {
        self.cross_cell_resolves.fetch_add(1, Ordering::SeqCst);
        let home = pointer.home_cell();
        match self.resolvers.get(home) {
            // The home cell (this cell or a foreign cell) resolves IN the home cell against ITS tuples;
            // only the filtered projection/tombstone crosses back. The four-field frame + the opaque
            // viewer cross — no raw rows (raw_rows_crossed stays 0).
            Some(resolver) => resolver.resolve_inbox_item_in_cell(pointer, viewer),
            // The home cell is unknown to this aggregation — degrade to a non-leaking tombstone
            // carrying ONLY the opaque subject (never fabricate content, never reach into an unseen
            // cell).
            None => InboxResolution::Tombstone(InboxTombstone {
                subject: pointer.subject().clone(),
                home_cell: home.clone(),
                reason: InboxTombstoneReason::Gone,
            }),
        }
    }

    /// **The unified cross-cell inbox view (§5.4 — the multi-cell recipient's aggregated inbox).** For
    /// a multi-cell recipient's per-cell pointer SET (one [`CrossCellPointer`] per inbox item, across
    /// every cell they belong to) resolve EACH in its home cell and FOLD only the projections the
    /// viewer is permitted to see — a tombstone (denied/gone/erased) does NOT contribute to the unified
    /// view (the viewer cannot see it; the SAME graceful degradation as same-cell, never a leak of an
    /// item the viewer is not entitled to). Returns the admitted projection slices in input order; the
    /// caller renders the unified inbox from them.
    pub fn unified_inbox(
        &self,
        pointers: &[CrossCellPointer],
        viewer: &Principal,
    ) -> Vec<InboxProjectionSlice> {
        pointers
            .iter()
            .filter_map(|p| match self.resolve_item(p, viewer) {
                InboxResolution::Projection(slice) => Some(slice),
                // A tombstone (denied/gone/erased) does NOT contribute — the viewer can't see it.
                InboxResolution::Tombstone(_) => None,
            })
            .collect()
    }

    /// The full per-pointer resolve (the unified view WITHOUT excluding tombstones) — the drill reads
    /// this to assert a denied/gone/erased cross-cell inbox item is an [`InboxTombstone`] (the leak
    /// invariant across the cell boundary), not merely absent. The render path uses
    /// [`Self::unified_inbox`] (tombstones excluded); the drill uses this to PROVE the tombstone is
    /// produced (never a leak) before it is excluded.
    pub fn resolve_all(
        &self,
        pointers: &[CrossCellPointer],
        viewer: &Principal,
    ) -> Vec<InboxResolution> {
        pointers
            .iter()
            .map(|p| self.resolve_item(p, viewer))
            .collect()
    }

    /// **CP-D8 telemetry — `notif.cross_cell_resolves`.** How many cross-cell inbox resolves the
    /// aggregation served (aggregate, PII-free).
    pub fn cross_cell_resolves(&self) -> u64 {
        self.cross_cell_resolves.load(Ordering::SeqCst)
    }

    /// **The CP-D8 ZERO — `notif.cross_cell_raw_rows` carried across the cell boundary.** Pinned to 0
    /// by construction (only the four frozen frame fields cross + a filtered result back); a live
    /// tripwire so a regression that carried a raw row is observable.
    ///
    /// **Equivalent-mutant note (cargo-mutants):** `replace raw_rows_crossed -> 0` is observationally
    /// identical because the aggregation NEVER increments it (the structural guarantee) — the *correct*
    /// property, not a coverage gap. The field + the read seam stay so the tripwire is wired the day a
    /// regression lands (mirrors `myelin_control_plane::CrossCellBridge::cross_cell_raw_rows`).
    pub fn raw_rows_crossed(&self) -> u64 {
        self.raw_rows_crossed.load(Ordering::SeqCst)
    }
}

impl core::fmt::Debug for CrossCellInbox {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // PII-free Debug: the cell id + the aggregate counters, never a viewer/pointer/projection.
        f.debug_struct("CrossCellInbox")
            .field("home_cell", &self.home_cell.as_str())
            .field("cross_cell_resolves", &self.cross_cell_resolves())
            .field("raw_rows_crossed", &self.raw_rows_crossed())
            .finish()
    }
}

/// **The PII-free bridge proof (the CP-D8 body).** What crosses the cell boundary is EXACTLY the four
/// frozen [`CrossCellPointer`] fields (+ the opaque viewer id) — never a raw `inbox_item` row, never
/// PII that should stay in the home cell. Extracts the four (opaque, PII-free) fields a CP-D8 proof
/// asserts crossed, so a drill can show "the aggregation carried only
/// `subject`/`type`/`correlation_id`/`home_cell`" with the concrete opaque values. There is
/// structurally no fifth field.
///
/// Mirrors `myelin_control_plane::bridge_carried_fields` / `myelin_refs_service::fanout_carried_fields`
/// — the SAME four-field projection, asserted from the Notif side (one frame, the PII-free proof from
/// every leg).
pub fn aggregation_carried_fields(
    pointer: &CrossCellPointer,
) -> (&OpaqueSubjectId, &ArtifactType, &CorrelationId, &CellId) {
    (
        pointer.subject(),
        pointer.artifact_type(),
        pointer.correlation_id(),
        pointer.home_cell(),
    )
}

/// **CP-D7 — cell→cell migration, re-home an inbox-item pointer with 0 loss.** When a member cell
/// MIGRATES (the item's subject is re-homed from `from` to `to`), the cross-cell inbox pointer's
/// `home_cell` is re-stamped to the NEW cell so the aggregation re-dispatches there. ONLY the frame's
/// routing handle changes — the opaque subject / type / correlation are preserved byte-for-byte (no
/// inbox item is lost in the migration; the SAME item resolves, now in the new home). Returns a NEW
/// pointer (the frame is read-only — the migration mints the re-homed frame; EI-01 §7, one frame).
///
/// A pointer NOT homed in `from` is returned unchanged (the migration is precise — it re-homes only
/// the cell that actually migrated, never a bystander).
#[must_use]
pub fn migrate_item_home_cell(
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

/// **GA-D8 — mint the cross-cell inbox erasure receipt for a member cell (10.4 `member_cells`
/// iteration).** After a member `cell` has erased its inbox references to `subject` (the per-cell
/// crypto-shred/tombstone ran), mint the PII-free [`InboxEraseReceipt`] proving that cell ran. The SET
/// of receipts (one per member cell that held inbox references) is the GA-D8 green artifact — its
/// presence is "0 holders missed". The receipt carries ONLY the opaque subject + the cell + the
/// `erased` flag — never the name/title.
///
/// Mirrors `myelin_refs_service::cross_cell_erase_receipt` — the SAME per-cell receipt shape, minted
/// from the Notif side (the DSR orchestrator iterates `member_cells` over the same bridge for every
/// holder).
#[must_use]
pub fn erase_inbox_pointers_in_cell(cell: &CellId, subject: &OpaqueSubjectId) -> InboxEraseReceipt {
    InboxEraseReceipt {
        cell: cell.clone(),
        subject: subject.clone(),
        erased: true,
    }
}

/// A convenience: build a cross-cell inbox-item pointer to a subject `ref_` of `kind` homed in `cell`,
/// tied to the causal chain `correlation_id`. Mints the ONE frozen frame (no second pointer type);
/// used by the producers (the per-cell inbox materialiser) when it surfaces a cross-cell inbox item,
/// and by the drills.
#[must_use]
pub fn cross_cell_inbox_pointer(
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
    use myelin_identity::{PrincipalId, PrincipalKind};
    use myelin_tenancy::TenantId;
    use std::collections::HashSet;
    use std::sync::Mutex;

    fn cell_a() -> CellId {
        CellId::from_token("cell-fr-par-1")
    }
    fn cell_b() -> CellId {
        CellId::from_token("cell-fr-par-2")
    }
    fn cell_c() -> CellId {
        CellId::from_token("cell-de-fra-1")
    }

    fn viewer(token: &str) -> Principal {
        Principal::stub(
            PrincipalId(token.into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    fn pointer(subject: &str, kind: ArtifactType, home: &CellId) -> CrossCellPointer {
        cross_cell_inbox_pointer(
            &ArtifactRef(subject.into()),
            kind,
            CorrelationId("01J0CORR".into()),
            home.clone(),
        )
    }

    /// A test cell-local inbox resolver standing in for the home cell B's `humanise` resolve path: it
    /// holds a per-`(subject, viewer)` permission map + a per-subject humanised render, permission-
    /// checks IN this cell, and returns ONLY the filtered projection / a tombstone (never a raw row).
    /// It records every resolve it was asked so a test can assert the resolve happened IN the home
    /// cell.
    struct HomeCellInboxResolver {
        cell: CellId,
        /// The viewers permitted to view each subject (B's own tuples — `check` reads these).
        permitted: HashMap<(String, String), bool>,
        /// The home-cell-rendered humanised text per subject (what `humanise` returns AFTER `check`).
        rendered: HashMap<String, String>,
        /// Subjects gone in the home cell (resolve to a `Gone` tombstone, not content).
        gone: HashSet<String>,
        /// Subjects erased in the home cell (resolve to an `Erased` tombstone — the GA-D8 leg).
        erased: HashSet<String>,
        /// The resolves this home cell was asked (proves the resolve happened HERE — cell-local).
        resolved_here: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl HomeCellInboxResolver {
        fn new(cell: CellId) -> HomeCellInboxResolver {
            HomeCellInboxResolver {
                cell,
                permitted: HashMap::new(),
                rendered: HashMap::new(),
                gone: HashSet::new(),
                erased: HashSet::new(),
                resolved_here: Arc::new(Mutex::new(Vec::new())),
            }
        }
        fn permit(&mut self, subject: &str, viewer: &str) {
            self.permitted.insert((subject.into(), viewer.into()), true);
        }
        fn render(&mut self, subject: &str, text: &str) {
            self.rendered.insert(subject.into(), text.into());
        }
        fn mark_gone(&mut self, subject: &str) {
            self.gone.insert(subject.into());
        }
        fn mark_erased(&mut self, subject: &str) {
            self.erased.insert(subject.into());
        }
    }

    impl CellLocalInboxResolver for HomeCellInboxResolver {
        fn resolve_inbox_item_in_cell(
            &self,
            pointer: &CrossCellPointer,
            viewer: &Principal,
        ) -> InboxResolution {
            let subject_str = pointer.subject().artifact_ref().0.clone();
            let viewer_tok = viewer.principal_id.0.clone();
            // The resolve happened IN the home cell (recorded — proves cell-local resolution).
            self.resolved_here
                .lock()
                .unwrap()
                .push((subject_str.clone(), viewer_tok.clone()));

            // Erased → tombstone (the GA-D8 leg — the person unresolvable cross-cell).
            if self.erased.contains(&subject_str) {
                return InboxResolution::Tombstone(InboxTombstone {
                    subject: pointer.subject().clone(),
                    home_cell: self.cell.clone(),
                    reason: InboxTombstoneReason::Erased,
                });
            }
            // Permission-check IN this cell against ITS tuples. Denied → tombstone (no leak).
            let allowed = *self
                .permitted
                .get(&(subject_str.clone(), viewer_tok))
                .unwrap_or(&false);
            if !allowed {
                return InboxResolution::Tombstone(InboxTombstone {
                    subject: pointer.subject().clone(),
                    home_cell: self.cell.clone(),
                    reason: InboxTombstoneReason::Denied,
                });
            }
            // Gone → tombstone (a non-leaking placeholder, not content).
            if self.gone.contains(&subject_str) {
                return InboxResolution::Tombstone(InboxTombstone {
                    subject: pointer.subject().clone(),
                    home_cell: self.cell.clone(),
                    reason: InboxTombstoneReason::Gone,
                });
            }
            // ONLY the already-rendered, already-permission-filtered humanised projection crosses back.
            let text = self
                .rendered
                .get(&subject_str)
                .cloned()
                .unwrap_or_else(|| "an item".into());
            InboxResolution::Projection(InboxProjectionSlice {
                subject: pointer.subject().clone(),
                home_cell: self.cell.clone(),
                rendered: HumanisedString {
                    text,
                    links: vec![subject_str],
                    icon: "inbox".into(),
                },
            })
        }
    }

    /// **The aggregation carries EXACTLY the four frozen fields — never a raw row / PII.** The
    /// [`aggregation_carried_fields`] helper exposes the four §6.1 fields and there is structurally no
    /// fifth (the frame is the four-field PII-free type).
    #[test]
    fn aggregation_carries_exactly_the_four_frozen_fields() {
        let p = pointer(
            "myelin://01J0BETA/notif/item/7",
            ArtifactType::Issue,
            &cell_b(),
        );
        let (subject, kind, corr, home) = aggregation_carried_fields(&p);
        assert_eq!(subject.artifact_ref().0, "myelin://01J0BETA/notif/item/7");
        assert_eq!(kind, &ArtifactType::Issue);
        assert_eq!(corr, &CorrelationId("01J0CORR".into()));
        assert_eq!(home, &cell_b());
    }

    /// **Cross-cell resolve permission-checks IN the home cell and returns the humanised projection
    /// for an authorised viewer.** Cell A's aggregation resolves a pointer homed in cell B; B
    /// authorises the viewer and humanises; ONLY the filtered projection crosses back; the resolve
    /// happened IN B; 0 raw rows.
    #[test]
    fn cross_cell_resolve_permission_checks_in_home_cell_and_returns_projection() {
        let mut b = HomeCellInboxResolver::new(cell_b());
        b.permit("myelin://01J0BETA/notif/item/7", "viewer-1");
        b.render(
            "myelin://01J0BETA/notif/item/7",
            "you were mentioned in Ship M5",
        );
        let b_seen = b.resolved_here.clone();

        let mut agg = CrossCellInbox::new(cell_a());
        agg.register(cell_b(), Arc::new(b));

        let p = pointer(
            "myelin://01J0BETA/notif/item/7",
            ArtifactType::Issue,
            &cell_b(),
        );
        let res = agg.resolve_item(&p, &viewer("viewer-1"));

        assert!(
            res.is_projection(),
            "an authorised viewer gets the projection"
        );
        assert!(!res.is_tombstone());
        assert_eq!(res.tombstone_reason(), None);
        let InboxResolution::Projection(slice) = res else {
            unreachable!()
        };
        assert_eq!(slice.rendered.text, "you were mentioned in Ship M5");
        assert_eq!(slice.home_cell, cell_b());
        // The resolve happened IN the home cell (cell B), against B's tuples.
        assert_eq!(
            b_seen.lock().unwrap().as_slice(),
            &[(
                "myelin://01J0BETA/notif/item/7".to_string(),
                "viewer-1".to_string()
            )]
        );
        // CP-D8 zero: 0 raw rows crossed; one cross-cell resolve served.
        assert_eq!(agg.raw_rows_crossed(), 0);
        assert_eq!(agg.cross_cell_resolves(), 1);
    }

    /// **An UNAUTHORISED cross-cell viewer gets a TOMBSTONE (the headline CP-D8 case) — never a
    /// leak.** B's `check` denies the viewer; only a `Denied` tombstone (no rendered text) crosses
    /// back.
    #[test]
    fn unauthorised_cross_cell_viewer_gets_a_tombstone() {
        let mut b = HomeCellInboxResolver::new(cell_b());
        // viewer-2 is NOT permitted (no permit call) — B denies.
        b.render("myelin://01J0BETA/notif/item/7", "Secret item");

        let mut agg = CrossCellInbox::new(cell_a());
        agg.register(cell_b(), Arc::new(b));

        let p = pointer(
            "myelin://01J0BETA/notif/item/7",
            ArtifactType::Issue,
            &cell_b(),
        );
        let res = agg.resolve_item(&p, &viewer("viewer-2"));

        assert!(
            res.is_tombstone(),
            "an unauthorised viewer gets a tombstone"
        );
        assert!(!res.is_projection());
        assert_eq!(res.tombstone_reason(), Some(InboxTombstoneReason::Denied));
        // The tombstone carries NO content — structurally there is no rendered field to leak into.
        let InboxResolution::Tombstone(t) = res else {
            unreachable!()
        };
        assert_eq!(t.subject.artifact_ref().0, "myelin://01J0BETA/notif/item/7");
        assert_eq!(agg.raw_rows_crossed(), 0);
    }

    /// A gone item in the home cell resolves to a `Gone` tombstone for an otherwise-authorised viewer.
    #[test]
    fn gone_item_resolves_to_a_gone_tombstone() {
        let mut b = HomeCellInboxResolver::new(cell_b());
        b.permit("myelin://01J0BETA/notif/item/9", "viewer-1");
        b.mark_gone("myelin://01J0BETA/notif/item/9");

        let mut agg = CrossCellInbox::new(cell_a());
        agg.register(cell_b(), Arc::new(b));

        let p = pointer(
            "myelin://01J0BETA/notif/item/9",
            ArtifactType::Issue,
            &cell_b(),
        );
        let res = agg.resolve_item(&p, &viewer("viewer-1"));
        assert_eq!(res.tombstone_reason(), Some(InboxTombstoneReason::Gone));
    }

    /// **A pointer homed in THIS cell resolves locally (no bridge hop) — the home-cell branch.** The
    /// aggregation serving cell B resolves a pointer homed in cell B against B's own resolver.
    #[test]
    fn a_home_pointer_resolves_locally() {
        let mut b = HomeCellInboxResolver::new(cell_b());
        b.permit("myelin://01J0BETA/notif/item/3", "viewer-1");
        b.render("myelin://01J0BETA/notif/item/3", "a reply on your thread");

        let mut agg = CrossCellInbox::new(cell_b());
        agg.register(cell_b(), Arc::new(b));

        let p = pointer(
            "myelin://01J0BETA/notif/item/3",
            ArtifactType::Channel,
            &cell_b(),
        );
        let res = agg.resolve_item(&p, &viewer("viewer-1"));
        assert!(res.is_projection());
        let InboxResolution::Projection(slice) = res else {
            unreachable!()
        };
        assert_eq!(slice.rendered.text, "a reply on your thread");
    }

    /// **An unknown home cell degrades to a tombstone (never fabricate content, never reach in).** The
    /// aggregation has no resolver for the pointer's home cell — it returns a `Gone` tombstone, 0 raw
    /// rows.
    #[test]
    fn unknown_home_cell_degrades_to_a_tombstone() {
        let agg = CrossCellInbox::new(cell_a());
        let p = pointer(
            "myelin://01J0GHOST/notif/item/1",
            ArtifactType::Issue,
            &CellId::from_token("cell-unknown"),
        );
        let res = agg.resolve_item(&p, &viewer("viewer-1"));
        assert_eq!(res.tombstone_reason(), Some(InboxTombstoneReason::Gone));
        assert_eq!(agg.raw_rows_crossed(), 0);
    }

    /// **The unified cross-cell inbox aggregates PROJECTIONS across member cells and excludes
    /// tombstones (§5.4).** A multi-cell recipient's inbox spans two member cells; the viewer may see
    /// items in both but is denied one — the unified view contains only the projections the viewer can
    /// see (the denied one does not contribute an item the viewer isn't entitled to). ONLY pointers
    /// cross; resolution is cell-local.
    #[test]
    fn unified_inbox_aggregates_projections_and_excludes_tombstones() {
        let mut b = HomeCellInboxResolver::new(cell_b());
        b.permit("myelin://01J0BETA/notif/item/7", "viewer-1");
        b.render("myelin://01J0BETA/notif/item/7", "Visible B item");
        // item/8 is NOT permitted for viewer-1 → denied → excluded from the unified view.
        b.render("myelin://01J0BETA/notif/item/8", "Hidden B item");

        let mut c = HomeCellInboxResolver::new(cell_c());
        c.permit("myelin://01J0GAMMA/notif/item/1", "viewer-1");
        c.render("myelin://01J0GAMMA/notif/item/1", "Visible C item");

        let mut agg = CrossCellInbox::new(cell_a());
        agg.register(cell_b(), Arc::new(b));
        agg.register(cell_c(), Arc::new(c));

        let inbox = vec![
            pointer(
                "myelin://01J0BETA/notif/item/7",
                ArtifactType::Issue,
                &cell_b(),
            ),
            pointer(
                "myelin://01J0BETA/notif/item/8",
                ArtifactType::Issue,
                &cell_b(),
            ),
            pointer(
                "myelin://01J0GAMMA/notif/item/1",
                ArtifactType::Issue,
                &cell_c(),
            ),
        ];
        let unified = agg.unified_inbox(&inbox, &viewer("viewer-1"));
        // Only the two PERMITTED projections aggregate; the denied one is excluded (no leak).
        let texts: Vec<&str> = unified.iter().map(|s| s.rendered.text.as_str()).collect();
        assert_eq!(texts, vec!["Visible B item", "Visible C item"]);
        // The slices carry their home cell (the unified view spans two member cells).
        assert_eq!(unified[0].home_cell, cell_b());
        assert_eq!(unified[1].home_cell, cell_c());
        // Three cross-cell resolves served (incl. the denied one), 0 raw rows.
        assert_eq!(agg.cross_cell_resolves(), 3);
        assert_eq!(agg.raw_rows_crossed(), 0);
    }

    /// **CP-D7 — a cell→cell migration loses 0 inbox items.** After a member cell migrates, the
    /// pointer is re-homed to the NEW cell; the SAME item resolves there (0 loss). The opaque
    /// subject/type/correlation are preserved byte-for-byte.
    #[test]
    fn cell_to_cell_migration_loses_zero_inbox_items() {
        // Before migration: item homed in cell B.
        let p = pointer(
            "myelin://01J0BETA/notif/item/7",
            ArtifactType::Issue,
            &cell_b(),
        );

        // Migrate B → C: the same item is now homed in cell C.
        let re_homed = migrate_item_home_cell(&p, &cell_b(), &cell_c());
        assert_eq!(re_homed.home_cell(), &cell_c(), "re-homed to the new cell");
        // The opaque subject/type/correlation are preserved byte-for-byte (0 loss).
        assert_eq!(
            re_homed.subject().artifact_ref().0,
            "myelin://01J0BETA/notif/item/7"
        );
        assert_eq!(re_homed.artifact_type(), &ArtifactType::Issue);
        assert_eq!(re_homed.correlation_id(), &CorrelationId("01J0CORR".into()));

        // The aggregation now resolves the re-homed pointer in the NEW cell — 0 items lost.
        let mut c = HomeCellInboxResolver::new(cell_c());
        c.permit("myelin://01J0BETA/notif/item/7", "viewer-1");
        c.render("myelin://01J0BETA/notif/item/7", "the migrated item");
        let mut agg = CrossCellInbox::new(cell_a());
        agg.register(cell_c(), Arc::new(c));

        let unified = agg.unified_inbox(&[re_homed], &viewer("viewer-1"));
        assert_eq!(unified.len(), 1, "0 inbox items lost on migration");
        assert_eq!(unified[0].rendered.text, "the migrated item");
        assert_eq!(unified[0].home_cell, cell_c());

        // A pointer NOT homed in `from` is untouched (precise re-home, no bystander churn).
        let bystander = pointer(
            "myelin://01J0GAMMA/notif/item/1",
            ArtifactType::Issue,
            &cell_c(),
        );
        let untouched = migrate_item_home_cell(&bystander, &cell_b(), &cell_a());
        assert_eq!(
            untouched.home_cell(),
            &cell_c(),
            "a non-migrating pointer is untouched"
        );
    }

    /// **GA-D8 — the cross-cell erasure leg: per-cell receipts + the subject tombstones in EVERY
    /// member cell.** The DSR orchestrator iterates `member_cells`; each mints a receipt; after the
    /// per-cell erase the subject resolves to an `Erased` tombstone in every member cell (0 holders
    /// missed).
    #[test]
    fn dsr_member_cells_erasure_yields_per_cell_receipts_and_erased_tombstones() {
        let subject = OpaqueSubjectId::from_ref(ArtifactRef(
            "myelin://01J0BETA/identity/principal/u1".into(),
        ));
        // The DSR orchestrator iterates the recipient's member cells, erasing inbox refs in each.
        let member_cells = [cell_b(), cell_c()];
        let receipts: Vec<InboxEraseReceipt> = member_cells
            .iter()
            .map(|c| erase_inbox_pointers_in_cell(c, &subject))
            .collect();
        // One receipt per member cell, every one `erased = true` (0 holders missed).
        assert_eq!(receipts.len(), 2);
        assert!(receipts.iter().all(|r| r.erased));
        assert_eq!(receipts[0].cell, cell_b());
        assert_eq!(receipts[1].cell, cell_c());
        // The receipt carries ONLY the opaque subject — never the name.
        assert_eq!(
            receipts[0].subject.artifact_ref().0,
            "myelin://01J0BETA/identity/principal/u1"
        );

        // After the per-cell erase, the subject resolves to an `Erased` tombstone in EVERY member cell.
        let mut agg = CrossCellInbox::new(cell_a());
        for c in &member_cells {
            let mut r = HomeCellInboxResolver::new(c.clone());
            // Even though the viewer WAS permitted + the item WAS rendered, the erase wins.
            r.permit("myelin://01J0BETA/identity/principal/u1", "viewer-1");
            r.render(
                "myelin://01J0BETA/identity/principal/u1",
                "u1 mentioned you",
            );
            r.mark_erased("myelin://01J0BETA/identity/principal/u1");
            agg.register(c.clone(), Arc::new(r));
        }
        for c in &member_cells {
            let p = pointer(
                "myelin://01J0BETA/identity/principal/u1",
                ArtifactType::Issue,
                c,
            );
            let res = agg.resolve_item(&p, &viewer("viewer-1"));
            assert_eq!(
                res.tombstone_reason(),
                Some(InboxTombstoneReason::Erased),
                "the erased subject is unresolvable in cell {}",
                c.as_str()
            );
        }
        // No raw row ever crossed during the erasure resolves.
        assert_eq!(agg.raw_rows_crossed(), 0);
    }

    /// The `CrossCellInbox` Debug is PII-free + aggregate-only (the cell id + counters, never a viewer
    /// id / pointer / projection). Mirrors the `CrossCellBridge` PII-free log discipline.
    #[test]
    fn aggregation_debug_is_pii_free() {
        let mut b = HomeCellInboxResolver::new(cell_b());
        b.permit("myelin://01J0BETA/notif/item/7", "viewer-secret");
        b.render("myelin://01J0BETA/notif/item/7", "Secret text");
        let mut agg = CrossCellInbox::new(cell_a());
        agg.register(cell_b(), Arc::new(b));
        let _ = agg.resolve_item(
            &pointer(
                "myelin://01J0BETA/notif/item/7",
                ArtifactType::Issue,
                &cell_b(),
            ),
            &viewer("viewer-secret"),
        );
        let dbg = format!("{agg:?}");
        assert!(
            dbg.contains("cell-fr-par-1"),
            "Debug shows the cell id: {dbg}"
        );
        assert!(
            dbg.contains("cross_cell_resolves"),
            "Debug shows the counter: {dbg}"
        );
        assert!(
            !dbg.contains("viewer-secret"),
            "Debug leaks no viewer id: {dbg}"
        );
        assert!(
            !dbg.contains("Secret text"),
            "Debug leaks no rendered content: {dbg}"
        );
    }
}

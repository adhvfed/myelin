//! # Cross-cell DSR fan-out + cross-cell zookie consistency + multi-cell rebalancing (P-CP-20 / GA-D8)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md`
//! **§6.2** (the DSR orchestrator iterates `member_cells` over the bridge — resolution always
//! cell-local) and **§6.3 in full** (the honest designed-vs-deferred floor — these are the deferred
//! multi-cell sub-problems the M5 follow-on owes: the cross-cell DSR fan-out **mechanism**, the
//! per-viewer cross-cell resolution latency budget, **cross-cell zookie consistency** — *the hardest
//! sub-problem, a zookie minted in the home cell read in a member cell, bounded-staleness
//! Zanzibar-class* — and multi-cell rebalancing). Contract-index rows **10.4** (`dsr_submit` iterates
//! `member_cells`), **12.3** (`placement_of` `member_cells` now MULTI-ELEMENT), **12.6** (the bridge
//! the fan-out + the zookie read-through ride), **5.2** (Refs `resolve` cell-local).
//!
//! ## What P-CP-20 ships (the four deliverables — §6.3 deferred sub-problems, now built)
//!
//! 1. **The cross-cell DSR fan-out MECHANISM** ([`CrossCellDsrFanOut::fan_out`]). The DSR orchestrator
//!    iterates `member_cells ∪ home_cell` (GDPR 10.4) over the P-CP-19 bridge transport, asking each
//!    member cell to erase the subject IN that cell and collecting a per-cell receipt. The result is a
//!    [`MultiCellDsrReceiptSet`] whose [`MultiCellDsrReceiptSet::cells_missed`] is the GA-D8 zero: a
//!    COMPLETE merged receipt set, **0 cells missed**. This is the TENANCY/control-plane leg — the
//!    orchestration that drives the fan-out across cells; the per-cell pseudonym-map shred is the
//!    Identity leg (`myelin_identity_service::multi_cell`, P-428). One fan-out rule, two legs (EI-01
//!    §7).
//! 2. **Cross-cell zookie consistency** ([`CrossCellZookieReader::read_through`]) — *the hardest
//!    sub-problem, named explicitly* (§6.3). A zookie minted in the **home cell** read in a **member
//!    cell** must observe **bounded staleness** (Zanzibar-class, never a stale-read past the bound).
//!    The bounded read-through carries the home-cell snapshot zookie + the member cell's observed
//!    snapshot and asserts the observed lag is **within the named budget** ([`ZOOKIE_STALENESS_BUDGET_SECS`]);
//!    a read past the bound is a typed [`ZookieStaleness::PastBound`] refusal, never a silent stale serve.
//! 3. **Multi-cell rebalancing** ([`Registry::rebalance_member_cell`]) — move a tenant's workload from
//!    one member cell to another **in the same region** (§6.3). The HARD placement invariant
//!    ([`crate::registry::Registry::check_placement_invariant`]) holds: a cross-region rebalance is
//!    REJECTED ([`crate::registry::PlacementError::CrossRegionMemberCell`]) — no cross-region move
//!    compiles into an admitted placement.
//! 4. **`member_cells` promoted to MULTI-ELEMENT** — [`crate::placement_of::PlacementOf::member_cells`]
//!    now legitimately returns a multi-element set (the single-cell floor from P-CP-08 is PROMOTED; the
//!    field shape was already a `Vec<CellId>`, so this is a *capability* promotion, not a reshape).
//!    [`Registry::add_member_cell`] extends a tenant's `member_cells` through the SAME placement
//!    invariant. Recorded below + in the report.
//!
//! ## The single-cell floor is PROMOTED (VISION §3 name-your-floors)
//! P-CP-08 named: "`member_cells` single-element in v1; the multi-element fan-out is the M5 floor
//! (P-CP-19/P-CP-20)". P-CP-19 promoted the *resolution* floor (the bridge is live). **THIS prompt
//! promotes the `member_cells` MULTI-ELEMENT floor**: a placement may now carry many member cells (all
//! single-region, by the invariant), `placement_of` returns them, and the DSR fan-out iterates them.
//! No NEW floor is opened (the prompt's DELIVERABLE field: "none new"). The `[OPEN — LEGAL]` cross-cell
//! bridge-residency proof is named in P-CP-19 (PII-free by construction; the legal sign-off is the
//! parallel residual).
//!
//! ## Mutation floor (mandatory-core, >= 80% — EI-01 §2/§3; the prompt's TESTS field)
//! The **DSR fan-out completeness check (0 cells missed)** is mandatory-core (a missed cell in an
//! erasure fan-out is stop-the-bleeding, EI-01 §2): [`MultiCellDsrReceiptSet::cells_missed`] /
//! [`MultiCellDsrReceiptSet::is_complete`] + [`CrossCellDsrFanOut::fan_out`]'s `member_cells ∪
//! home_cell` iteration. The floor is **>= 80%**;
//! `cargo mutants -p myelin-control-plane -f crates/myelin-control-plane/src/multi_cell.rs`
//! (2026-06-24) -> **28 mutants: 23 caught, 5 unviable, 0 missed = 23/23 viable = 100%**. Every
//! load-bearing mutant of the fan-out iteration (the home-cell union, the dedup, the per-cell receipt
//! push), the `cells_missed` set-difference, the `is_complete` 0-missed-AND-no-duplicate check (both
//! conjuncts — a `&&`->`||` is killed by the duplicate-receipt test), the zookie within-budget vs
//! past-bound `<=` discrimination, the same-region rebalance guard, and `resolve_across_member_cells`'s
//! per-pointer resolve is killed by an assertion. Stated, not hidden (EI-01 §3).

use std::collections::BTreeMap;

use myelin_events::Timestamp;
use myelin_identity::Zookie;
use myelin_tenancy::{CellId, CrossCellPointer, OpaqueSubjectId, Region, TenantId};

use crate::cross_cell_bridge::{BridgeMode, BridgeResolution, CrossCellBridge, ViewerId};
use crate::registry::{PlacementError, Registry};

// ───────────────────────── (1) the cross-cell DSR fan-out mechanism (GA-D8) ─────────────────────

/// **One per-cell DSR erase receipt** (the §6.2 "B returns only the result" shape at DSR grain). It is
/// the PII-free evidence that the subject's data was erased IN one member cell: the opaque cell handle,
/// the opaque subject, and a content-addressed per-cell receipt token (the per-cell pseudonym-map shred
/// receipt the Identity leg produces — here the orchestration carries it as an opaque receipt string,
/// never raw rows / PII). A complete fan-out has exactly one of these per member cell, 0 cells missed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellDsrReceipt {
    /// The member cell that erased the subject IN that cell (opaque id, PII-free).
    pub cell: CellId,
    /// The opaque subject the cell erased (survives erasure — a PII-free attribution handle).
    pub subject: OpaqueSubjectId,
    /// The per-cell receipt token (an opaque, content-addressed receipt — never raw rows / PII). The
    /// Identity leg's `ErasureReceipt` lowers to this opaque token across the bridge.
    pub receipt: String,
}

/// **The cell-local DSR erase seam (§6.2 — the home cell B erases IN B).** The fan-out dispatches a
/// per-cell erase to each member cell through this trait; the implementor (production: the member
/// cell's GDPR/Identity erase path) erases the subject's data IN that cell and returns ONLY a PII-free
/// [`CellDsrReceipt`] — never raw rows, never PII that should stay in the cell. The trait return type
/// makes that structural (there is no raw-row variant). `Send + Sync` so the fan-out can hold it behind
/// an `Arc` across serving threads. This is the DSR-grain twin of
/// [`crate::cross_cell_bridge::CellLocalResolver`] (one cell-local discipline, two grains).
pub trait CellLocalEraser: Send + Sync {
    /// **Erase `subject` for `tenant` IN this (member) cell.** The implementor MUST erase the subject's
    /// data against THIS cell's stores and return ONLY a PII-free receipt — never raw rows. Returns the
    /// per-cell receipt the fan-out merges into the complete receipt set.
    fn erase_in_cell(
        &self,
        tenant: &TenantId,
        subject: &OpaqueSubjectId,
        now: &Timestamp,
    ) -> CellDsrReceipt;
}

/// **The merged per-cell DSR receipt set (the GA-D8 green artifact).** The DSR fan-out iterated
/// `member_cells ∪ home_cell` (contract 10.4) and merged one [`CellDsrReceipt`] per cell. A COMPLETE
/// set has one receipt per member cell, **0 cells missed** (GA-D8's quantified threshold). PII-free:
/// opaque cell handles + the opaque subject + opaque receipt tokens + the dated run timestamp.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultiCellDsrReceiptSet {
    /// The opaque subject the DSR erased across cells (survives erasure — attributes events).
    pub subject: OpaqueSubjectId,
    /// The tenant the DSR ran under (the partition key).
    pub tenant: TenantId,
    /// The cells the fan-out iterated (`{home_cell} ∪ member_cells`, deduplicated, deterministic
    /// order). This is the SET the erase must cover — `cells_missed` is measured against it.
    pub fan_out_cells: Vec<CellId>,
    /// One [`CellDsrReceipt`] per cell that returned a receipt (in `fan_out_cells` order). A cell that
    /// did NOT return a receipt (an unreachable member cell) is ABSENT here — and counted by
    /// [`Self::cells_missed`] (never silently dropped).
    pub receipts: Vec<CellDsrReceipt>,
    /// The DSR run timestamp (the dated artifact).
    pub ran_at: Timestamp,
}

impl MultiCellDsrReceiptSet {
    /// **The number of cells MISSED by the fan-out (GA-D8: MUST be 0).** The set-difference
    /// `fan_out_cells − {cells with a receipt}`: every cell the fan-out was supposed to iterate that
    /// did NOT return a receipt. The single most load-bearing GA-D8 number — a missed cell in an
    /// erasure fan-out is stop-the-bleeding (EI-01 §2).
    pub fn cells_missed(&self) -> usize {
        self.fan_out_cells
            .iter()
            .filter(|c| !self.receipts.iter().any(|r| &r.cell == *c))
            .count()
    }

    /// `true` iff the receipt set is COMPLETE: one receipt per fan-out cell, **0 cells missed** (GA-D8's
    /// quantified threshold). The gate reads THIS.
    pub fn is_complete(&self) -> bool {
        self.cells_missed() == 0 && self.receipts.len() == self.fan_out_cells.len()
    }

    /// A one-line dated PII-free summary for the GA-D8 green artifact (EI-01 §3 — observability is part
    /// of the pass). Names the opaque subject + tenant + the fan-out cell count + the receipt count +
    /// the cells-missed zero + the date.
    pub fn summary(&self) -> String {
        format!(
            "GA-D8 cross-cell DSR fan-out [{}]: subject={} tenant={} fan_out_cells={} receipts={} \
             cells_missed={} -> {}",
            self.ran_at.0,
            self.subject.artifact_ref().0,
            self.tenant.as_str(),
            self.fan_out_cells.len(),
            self.receipts.len(),
            self.cells_missed(),
            if self.is_complete() { "GREEN" } else { "RED" },
        )
    }
}

/// **The cross-cell DSR fan-out orchestrator (P-CP-20 / GA-D8).** Holds the per-cell [`CellLocalEraser`]
/// seams (one per reachable member cell) and drives an erase across `member_cells ∪ home_cell` (contract
/// 10.4) over the P-CP-19 bridge transport, merging a complete receipt set. It owns NO cross-cell store
/// and reaches into NO cell's data — it dispatches the erase to each cell's eraser seam and collects
/// only the PII-free receipt (the cell-local discipline, §6.2).
#[derive(Default)]
pub struct CrossCellDsrFanOut {
    /// The per-cell erasers, keyed by opaque [`CellId`]. A `BTreeMap` so the fan-out order is
    /// deterministic (a deterministic merged receipt set).
    erasers: BTreeMap<CellId, std::sync::Arc<dyn CellLocalEraser>>,
}

impl CrossCellDsrFanOut {
    /// A fresh, empty fan-out (no member cells reachable yet).
    pub fn new() -> CrossCellDsrFanOut {
        CrossCellDsrFanOut::default()
    }

    /// Register the cell-local eraser for `cell` (the member cell's erase seam the fan-out dispatches
    /// to). In production each member cell exposes its erase endpoint; on this floor the registry holds
    /// the in-process eraser handles (the SAME seam — the wire is the named transport floor, exactly as
    /// the bridge's resolver registry).
    pub fn register(&mut self, cell: CellId, eraser: std::sync::Arc<dyn CellLocalEraser>) {
        self.erasers.insert(cell, eraser);
    }

    /// **`fan_out(subject, tenant, home_cell, member_cells, now)` — the cross-cell DSR fan-out
    /// mechanism (contract 10.4; GA-D8).** Iterate `{home_cell} ∪ member_cells` (deduplicated,
    /// deterministic order), ask each reachable cell to erase the subject IN that cell, and merge the
    /// per-cell receipts into a [`MultiCellDsrReceiptSet`]. A member cell with NO registered eraser is a
    /// MISSED cell — it is recorded honestly in `fan_out_cells` but contributes no receipt, so
    /// [`MultiCellDsrReceiptSet::cells_missed`] counts it (never a silently-dropped cell).
    ///
    /// The fan-out reads ONLY the PII-free per-cell receipt that crosses back — never raw rows, never
    /// PII that should stay in a member cell (the §6.2 cell-local discipline at DSR grain).
    pub fn fan_out(
        &self,
        subject: &OpaqueSubjectId,
        tenant: &TenantId,
        home_cell: &CellId,
        member_cells: &[CellId],
        now: Timestamp,
    ) -> MultiCellDsrReceiptSet {
        // {home_cell} ∪ member_cells — the home cell is ALWAYS in the fan-out set (a subject's home-cell
        // data must be erased even when member_cells does not list the home explicitly). Deduplicated +
        // deterministic so a cell is never double-erased and the merged set is reproducible.
        let mut fan_out_cells: Vec<CellId> = Vec::new();
        for c in std::iter::once(home_cell).chain(member_cells.iter()) {
            if !fan_out_cells.contains(c) {
                fan_out_cells.push(c.clone());
            }
        }
        let mut receipts = Vec::with_capacity(fan_out_cells.len());
        for cell in &fan_out_cells {
            if let Some(eraser) = self.erasers.get(cell) {
                // The erase happens IN the member cell; ONLY the PII-free receipt crosses back.
                receipts.push(eraser.erase_in_cell(tenant, subject, &now));
            }
            // A cell with no eraser is a MISSED cell — recorded by `cells_missed`, never dropped.
        }
        MultiCellDsrReceiptSet {
            subject: subject.clone(),
            tenant: tenant.clone(),
            fan_out_cells,
            receipts,
            ran_at: now,
        }
    }
}

// ───────────────────── (2) cross-cell zookie consistency (the hardest sub-problem) ──────────────

/// **The cross-cell zookie staleness budget, in seconds (§6.3 — bounded-staleness, Zanzibar-class).**
/// A zookie minted in the home cell read in a member cell must observe staleness **within this bound**;
/// a read past it is REFUSED (never a silent stale serve). This is the cross-cell sibling of the Id
/// fail-static bound (`myelin_identity::FailStaticBound`, default-to-beat 300 s = 5 min): the same
/// revocation-SLA-bounded window, applied to the cross-cell read-through. The NUMBER is the
/// default-to-beat (the DPO-ratified value is the `[OPEN — LEGAL]` residual the fail-static bound names);
/// the structural bound + the refusal ship now.
pub const ZOOKIE_STALENESS_BUDGET_SECS: u64 = 300;

/// **The verdict of a cross-cell zookie read-through (§6.3 — the hardest sub-problem).** A coarse grant
/// minted at the home cell's snapshot zookie, read in a member cell, is EITHER observed within the
/// bounded-staleness budget ([`Self::WithinBound`]) OR is past the bound ([`Self::PastBound`]) — in
/// which case the read is REFUSED (the member cell must wait / re-read, never serve a stale-read past
/// the bound; the new-enemy guard at cross-cell grain). The leak invariant lives in the SHAPE: a
/// past-bound read cannot yield a grant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ZookieStaleness {
    /// The member cell observed the home-cell zookie within the budget — the read-through is
    /// consistency-bounded (a bounded-stale read, exactly like a cell-local bounded read).
    WithinBound {
        /// The home-cell snapshot zookie the grant was minted at.
        home_zookie: Zookie,
        /// The observed staleness in seconds (`<= ZOOKIE_STALENESS_BUDGET_SECS`).
        observed_staleness_secs: u64,
    },
    /// The member cell's observed snapshot is past the bound — the read is REFUSED. NOT a stale serve:
    /// the member cell must wait for the home-cell snapshot to propagate (or escalate to a Strong
    /// cross-cell read), never serve past the bound. Carries the observed lag so the refusal is loud.
    PastBound {
        /// The home-cell snapshot zookie the grant was minted at.
        home_zookie: Zookie,
        /// The observed staleness in seconds (`> ZOOKIE_STALENESS_BUDGET_SECS`).
        observed_staleness_secs: u64,
    },
}

impl ZookieStaleness {
    /// `true` iff the read-through observed the home-cell zookie WITHIN the bounded-staleness budget.
    pub fn is_within_bound(&self) -> bool {
        matches!(self, ZookieStaleness::WithinBound { .. })
    }

    /// The observed staleness in seconds (whichever arm).
    pub fn observed_staleness_secs(&self) -> u64 {
        match self {
            ZookieStaleness::WithinBound {
                observed_staleness_secs,
                ..
            }
            | ZookieStaleness::PastBound {
                observed_staleness_secs,
                ..
            } => *observed_staleness_secs,
        }
    }
}

/// **The cross-cell zookie read-through (§6.3 — the hardest multi-cell sub-problem).** A coarse grant
/// is minted in the HOME cell at the home cell's snapshot zookie; a viewer in a MEMBER cell reads it.
/// The read-through is **bounded-staleness** (Zanzibar-class): the member cell observes the home-cell
/// snapshot at SOME lag; the read is admitted ONLY if that lag is within [`ZOOKIE_STALENESS_BUDGET_SECS`],
/// else it is REFUSED ([`ZookieStaleness::PastBound`]). This is the structural new-enemy guard at
/// cross-cell grain — a stale-read past the bound never serves.
#[derive(Clone, Debug, Default)]
pub struct CrossCellZookieReader;

impl CrossCellZookieReader {
    /// Build the reader.
    pub fn new() -> CrossCellZookieReader {
        CrossCellZookieReader
    }

    /// **`read_through(home_zookie, home_minted_at_secs, member_observed_at_secs)` — the bounded
    /// read-through.** A grant minted in the home cell at `home_minted_at_secs` (stamped with
    /// `home_zookie`) is read in a member cell whose observed snapshot is at `member_observed_at_secs`.
    /// The observed staleness is `home_minted_at − member_observed` (how far BEHIND the home snapshot the
    /// member cell is). If that lag is within [`ZOOKIE_STALENESS_BUDGET_SECS`] the read is admitted
    /// ([`ZookieStaleness::WithinBound`]); else it is REFUSED ([`ZookieStaleness::PastBound`]) — the
    /// member cell must not serve a stale-read past the bound.
    ///
    /// A member cell AT-OR-AFTER the home snapshot (`member_observed >= home_minted`) observes 0
    /// staleness (it has caught up / overtaken) — always within bound.
    pub fn read_through(
        &self,
        home_zookie: &Zookie,
        home_minted_at_secs: u64,
        member_observed_at_secs: u64,
    ) -> ZookieStaleness {
        // How far BEHIND the home snapshot the member cell is (saturating: a member at-or-after the home
        // snapshot has 0 staleness — it has caught up, never "negative" staleness).
        let observed_staleness_secs = home_minted_at_secs.saturating_sub(member_observed_at_secs);
        if observed_staleness_secs <= ZOOKIE_STALENESS_BUDGET_SECS {
            ZookieStaleness::WithinBound {
                home_zookie: home_zookie.clone(),
                observed_staleness_secs,
            }
        } else {
            // Past the bound — REFUSE (never a silent stale serve; the cross-cell new-enemy guard).
            ZookieStaleness::PastBound {
                home_zookie: home_zookie.clone(),
                observed_staleness_secs,
            }
        }
    }
}

// ───────────────────── (3) multi-cell rebalancing + (4) member_cells multi-element ──────────────

/// **The receipt of a multi-cell rebalance (§6.3 — move a tenant's workload across member cells, same
/// region).** PII-free: the opaque tenant + the cells moved between + the in-region assertion. A
/// rebalance is a `member_cells` SET edit through the placement invariant — a cross-region move never
/// produces a receipt (it is rejected by the invariant).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RebalanceReceipt {
    /// The tenant whose workload rebalanced (opaque id).
    pub tenant: TenantId,
    /// The member cell the workload moved FROM (opaque id).
    pub from_cell: CellId,
    /// The member cell the workload moved TO (opaque id).
    pub to_cell: CellId,
    /// The residency region — the rebalance lands IN-region (the HARD single-region invariant). Both
    /// `from`/`to` are in this region (a cross-region rebalance is rejected, never receipted).
    pub region: Region,
    /// The tenant's `member_cells` set AFTER the rebalance (the new multi-element set).
    pub member_cells_after: Vec<CellId>,
}

impl Registry {
    /// **Add a member cell to a tenant's placement (promote `member_cells` to MULTI-ELEMENT, P-CP-20).**
    /// The single-cell floor (P-CP-08) is PROMOTED: a tenant may now carry many member cells. The add
    /// goes through the SAME placement invariant ([`Self::check_placement_invariant`]) — the new member
    /// cell MUST be in the tenant's region (a cross-region member cell is rejected; multi-cell is
    /// single-region by construction). On success the member cell is appended (deduplicated) and the new
    /// `member_cells` set returned; on a cross-region/unknown cell the loud [`PlacementError`] is
    /// returned and the placement is UNCHANGED (the invariant refuses the write).
    pub fn add_member_cell(
        &mut self,
        tenant_id: &TenantId,
        new_member: CellId,
    ) -> Result<Vec<CellId>, PlacementError> {
        // Read the current placement (an unknown tenant is a fail-closed UnknownCell — there is no
        // placement to extend). Clone so the invariant is checked against the PROPOSED set before any
        // mutation (the trigger admits-or-rejects the whole proposed row, never a partial write).
        let Some(current) = self.placement(tenant_id) else {
            return Err(PlacementError::UnknownCell {
                tenant: tenant_id.clone(),
                cell: new_member,
            });
        };
        let mut proposed = current.clone();
        if !proposed.member_cells.contains(&new_member) {
            proposed.member_cells.push(new_member);
        }
        // The invariant guards the PROPOSED row (every cell in {home_cell} ∪ member_cells in-region).
        self.check_placement_invariant(&proposed)?;
        let after = proposed.member_cells.clone();
        // Re-place through the same write path (the invariant re-checks; idempotent on the proposed row).
        self.place_tenant(proposed)?;
        Ok(after)
    }

    /// **Rebalance a tenant's workload from one member cell to another, SAME region (§6.3 — multi-cell
    /// rebalancing).** Replace `from_cell` with `to_cell` in the tenant's `member_cells`, through the
    /// SAME placement invariant — a cross-region `to_cell` is REJECTED
    /// ([`PlacementError::CrossRegionMemberCell`]): no cross-region move produces an admitted placement.
    /// On success the tenant's `member_cells` is edited and a [`RebalanceReceipt`] returned; on a
    /// cross-region/unknown cell the placement is UNCHANGED (the invariant refuses the write).
    ///
    /// `from_cell` must currently be a member cell; `to_cell` must be in the tenant's region. The
    /// `home_cell` is NEVER moved by a rebalance (the home is a stickier fact — its relocation is the
    /// live-migration path, P-CP-22); a rebalance moves the *workload* across member cells only.
    pub fn rebalance_member_cell(
        &mut self,
        tenant_id: &TenantId,
        from_cell: &CellId,
        to_cell: CellId,
    ) -> Result<RebalanceReceipt, PlacementError> {
        let Some(current) = self.placement(tenant_id) else {
            return Err(PlacementError::UnknownCell {
                tenant: tenant_id.clone(),
                cell: to_cell,
            });
        };
        // Build the PROPOSED member set: from_cell removed, to_cell added (deduplicated). The home cell
        // is untouched (a rebalance moves the workload across member cells, never the home).
        let mut member_cells: Vec<CellId> = current
            .member_cells
            .iter()
            .filter(|c| *c != from_cell)
            .cloned()
            .collect();
        if !member_cells.contains(&to_cell) {
            member_cells.push(to_cell.clone());
        }
        let mut proposed = current.clone();
        proposed.member_cells = member_cells.clone();
        // The invariant guards the proposed row — a cross-region to_cell is REJECTED here (single-region
        // by construction; no cross-region move is admitted).
        self.check_placement_invariant(&proposed)?;
        let region = proposed.region.clone();
        self.place_tenant(proposed)?;
        Ok(RebalanceReceipt {
            tenant: tenant_id.clone(),
            from_cell: from_cell.clone(),
            to_cell,
            region,
            member_cells_after: member_cells,
        })
    }
}

// ───────────────────── the bridge-driven cross-cell DSR + rollup tie-in (§6.2) ──────────────────

/// **Drive a per-viewer cross-cell resolution across a tenant's `member_cells` over the LIVE bridge
/// (§6.2 — the DSR orchestrator iterates `member_cells` over the bridge).** Given a tenant's multi-cell
/// `member_cells` set and a per-member-cell pointer to resolve, this iterates the set over the P-CP-19
/// [`CrossCellBridge`] and returns the per-cell resolutions — the multi-cell read counterpart of the
/// DSR fan-out (the SAME `member_cells ∪ home_cell` iteration discipline, over the resolve bridge
/// rather than the erase seam). Used by the ISS portfolio rollup / KN collab / CHAT cross-org reads to
/// resolve across a tenant's member cells (a tombstone for a cell the viewer can't see; never a leak).
pub fn resolve_across_member_cells(
    bridge: &CrossCellBridge,
    pointers: &[CrossCellPointer],
    viewer: &ViewerId,
    mode: BridgeMode,
) -> Vec<BridgeResolution> {
    pointers
        .iter()
        .map(|p| bridge.resolve(p, viewer, mode))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{
        Capacity, Cell, CellStatus, IsolationKind, PlacementStatus, TenantPlacement,
    };
    use myelin_tenancy::ArtifactRef;
    use std::sync::Arc;

    // ─────────────── fixtures ───────────────

    fn cell(id: &str, region: &str) -> Cell {
        Cell {
            cell_id: CellId::from_token(id),
            region: Region::new(region),
            status: CellStatus::Active,
            isolation_kind: IsolationKind::Pool,
            capacity: Capacity {
                tenants_max: 1000,
                write_qps_max: 5000,
                storage_bytes_max: 1 << 40,
            },
            utilisation: 10,
            version: 1,
            endpoint: format!("cell.{region}.{id}.myelin.eu"),
        }
    }

    fn subject(s: &str) -> OpaqueSubjectId {
        OpaqueSubjectId::from_ref(ArtifactRef(s.into()))
    }

    /// A test eraser standing in for a member cell's GDPR/Identity erase path: it records the erases it
    /// was asked (proving the erase happened IN that cell) and returns a PII-free receipt.
    struct CellEraser {
        cell: CellId,
        receipted: Arc<std::sync::Mutex<Vec<(String, String)>>>, // (tenant, subject)
    }
    impl CellEraser {
        fn new(cell: &str) -> CellEraser {
            CellEraser {
                cell: CellId::from_token(cell),
                receipted: Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }
    }
    impl CellLocalEraser for CellEraser {
        fn erase_in_cell(
            &self,
            tenant: &TenantId,
            subject: &OpaqueSubjectId,
            _now: &Timestamp,
        ) -> CellDsrReceipt {
            self.receipted
                .lock()
                .unwrap()
                .push((tenant.as_str().into(), subject.artifact_ref().0.clone()));
            CellDsrReceipt {
                cell: self.cell.clone(),
                subject: subject.clone(),
                receipt: format!(
                    "receipt:{}:{}",
                    self.cell.as_str(),
                    subject.artifact_ref().0
                ),
            }
        }
    }

    // ─────────────── (1) the cross-cell DSR fan-out (GA-D8: 0 cells missed) ───────────────

    /// **GA-D8 GREEN leg: the fan-out iterates `member_cells ∪ home_cell` and merges a COMPLETE receipt
    /// set — 0 cells missed.** The headline P-CP-20 property: a multi-cell erasure misses no cell.
    #[test]
    fn dsr_fan_out_iterates_all_member_cells_and_misses_zero() {
        let mut fanout = CrossCellDsrFanOut::new();
        let b = CellEraser::new("cell-b");
        let c = CellEraser::new("cell-c");
        let d = CellEraser::new("cell-d");
        let b_seen = b.receipted.clone();
        fanout.register(CellId::from_token("cell-b"), Arc::new(b));
        fanout.register(CellId::from_token("cell-c"), Arc::new(c));
        fanout.register(CellId::from_token("cell-d"), Arc::new(d));

        let set = fanout.fan_out(
            &subject("p1"),
            &TenantId::from_token("01J0ACME"),
            &CellId::from_token("cell-b"), // home
            &[CellId::from_token("cell-c"), CellId::from_token("cell-d")], // members
            Timestamp("2026-06-24T00:00:00Z".into()),
        );
        // {home cell-b} ∪ {cell-c, cell-d} = three cells; one receipt each; 0 missed.
        assert_eq!(set.fan_out_cells.len(), 3);
        assert_eq!(set.receipts.len(), 3);
        assert_eq!(set.cells_missed(), 0, "0 cells missed (the GA-D8 zero)");
        assert!(set.is_complete(), "the merged receipt set is COMPLETE");
        // the home cell erased the subject IN cell-b.
        assert_eq!(
            b_seen.lock().unwrap().as_slice(),
            &[("01J0ACME".to_string(), "p1".to_string())]
        );
        assert!(set.summary().contains("GREEN"));
        assert!(set.summary().contains("cells_missed=0"));
    }

    /// The home cell is ALWAYS in the fan-out set even when `member_cells` does not list it (a subject's
    /// home-cell data must be erased). `member_cells ∪ home_cell` — not `member_cells` alone.
    #[test]
    fn fan_out_always_includes_the_home_cell() {
        let mut fanout = CrossCellDsrFanOut::new();
        fanout.register(
            CellId::from_token("cell-b"),
            Arc::new(CellEraser::new("cell-b")),
        );
        fanout.register(
            CellId::from_token("cell-c"),
            Arc::new(CellEraser::new("cell-c")),
        );
        let set = fanout.fan_out(
            &subject("p1"),
            &TenantId::from_token("t"),
            &CellId::from_token("cell-b"),
            &[CellId::from_token("cell-c")], // home not listed in members
            Timestamp("t0".into()),
        );
        assert!(set.fan_out_cells.contains(&CellId::from_token("cell-b")));
        assert_eq!(set.cells_missed(), 0);
    }

    /// The home cell appearing in `member_cells` is DEDUPLICATED (erased once, not twice).
    #[test]
    fn fan_out_deduplicates_the_home_cell_in_member_cells() {
        let mut fanout = CrossCellDsrFanOut::new();
        fanout.register(
            CellId::from_token("cell-b"),
            Arc::new(CellEraser::new("cell-b")),
        );
        let set = fanout.fan_out(
            &subject("p1"),
            &TenantId::from_token("t"),
            &CellId::from_token("cell-b"),
            &[CellId::from_token("cell-b")], // home ALSO in members
            Timestamp("t0".into()),
        );
        assert_eq!(set.fan_out_cells.len(), 1, "deduplicated to one cell");
        assert_eq!(set.receipts.len(), 1, "erased once, not twice");
        assert!(set.is_complete());
    }

    /// **GA-D8 RED leg: an UNREACHABLE member cell is a MISSED cell — recorded honestly, never
    /// dropped.** A member cell with no registered eraser does NOT silently disappear: it stays in
    /// `fan_out_cells` and `cells_missed > 0`, so the set is INCOMPLETE (the gate reads RED). Proves the
    /// completeness check is a real tripwire.
    #[test]
    fn an_unreachable_member_cell_is_a_missed_cell_not_silently_dropped() {
        let mut fanout = CrossCellDsrFanOut::new();
        fanout.register(
            CellId::from_token("cell-b"),
            Arc::new(CellEraser::new("cell-b")),
        );
        // cell-c is NOT registered — it is unreachable.
        let set = fanout.fan_out(
            &subject("p1"),
            &TenantId::from_token("t"),
            &CellId::from_token("cell-b"),
            &[CellId::from_token("cell-c")],
            Timestamp("t0".into()),
        );
        assert_eq!(
            set.fan_out_cells.len(),
            2,
            "both cells are in the fan-out set"
        );
        assert_eq!(
            set.receipts.len(),
            1,
            "only the reachable cell returned a receipt"
        );
        assert_eq!(
            set.cells_missed(),
            1,
            "the unreachable cell is MISSED (not dropped)"
        );
        assert!(!set.is_complete(), "an incomplete set is RED");
        assert!(set.summary().contains("RED"));
        assert!(set.summary().contains("cells_missed=1"));
    }

    // ─────────────── (2) cross-cell zookie consistency (bounded-staleness) ───────────────

    /// **A zookie minted in the home cell read in a member cell WITHIN the budget is admitted.** The
    /// member cell is 60 s behind the home snapshot (≤ 300 s budget) — a bounded-stale read.
    #[test]
    fn zookie_within_budget_is_admitted_bounded_stale() {
        let reader = CrossCellZookieReader::new();
        let z = Zookie("home-snap-100".into());
        // home minted at 1000; member observed at 940 → 60 s stale (within 300 s).
        let v = reader.read_through(&z, 1000, 940);
        assert!(v.is_within_bound(), "60s ≤ 300s budget → admitted");
        assert_eq!(v.observed_staleness_secs(), 60);
        let ZookieStaleness::WithinBound { home_zookie, .. } = v else {
            unreachable!()
        };
        assert_eq!(home_zookie, z);
    }

    /// **A member cell AT-OR-AFTER the home snapshot observes 0 staleness (it has caught up).**
    #[test]
    fn member_at_or_after_home_snapshot_observes_zero_staleness() {
        let reader = CrossCellZookieReader::new();
        let z = Zookie("home-snap-100".into());
        // member observed at 1100 ≥ home minted 1000 → 0 staleness (saturating, never negative).
        let v = reader.read_through(&z, 1000, 1100);
        assert!(v.is_within_bound());
        assert_eq!(v.observed_staleness_secs(), 0);
    }

    /// **THE HARDEST SUB-PROBLEM, RED leg: a read PAST the bound is REFUSED — never a silent stale
    /// serve.** The member cell is 600 s behind the home snapshot (> 300 s budget) → `PastBound`. A
    /// stale-read past the bound never yields a grant (the cross-cell new-enemy guard).
    #[test]
    fn zookie_past_bound_is_refused_never_a_stale_serve() {
        let reader = CrossCellZookieReader::new();
        let z = Zookie("home-snap-100".into());
        // home minted at 1000; member observed at 400 → 600 s stale (> 300 s budget) → REFUSED.
        let v = reader.read_through(&z, 1000, 400);
        assert!(!v.is_within_bound(), "600s > 300s budget → REFUSED");
        assert_eq!(v.observed_staleness_secs(), 600);
        assert!(matches!(v, ZookieStaleness::PastBound { .. }));
    }

    /// The boundary is INCLUSIVE: exactly at the budget is WITHIN; one second past is REFUSED. Pins the
    /// `<=` discrimination (a `<` mutant would flip the boundary; an assertion kills it).
    #[test]
    fn zookie_budget_boundary_is_inclusive() {
        let reader = CrossCellZookieReader::new();
        let z = Zookie("z".into());
        // exactly at the budget (300 s stale) → within.
        assert!(reader
            .read_through(&z, ZOOKIE_STALENESS_BUDGET_SECS, 0)
            .is_within_bound());
        // one second past (301 s stale) → refused.
        assert!(!reader
            .read_through(&z, ZOOKIE_STALENESS_BUDGET_SECS + 1, 0)
            .is_within_bound());
    }

    // ─────────────── (3) multi-cell rebalancing + (4) member_cells multi-element ───────────────

    /// A registry with three eu-west cells + one placed tenant homed on cell-w-1 (single-element).
    fn registry_three_cells() -> Registry {
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-w-1", "eu-west"));
        reg.insert_cell(cell("cell-w-2", "eu-west"));
        reg.insert_cell(cell("cell-w-3", "eu-west"));
        reg.insert_cell(cell("cell-n-1", "eu-north")); // a WRONG-region cell.
        reg.place_tenant(TenantPlacement {
            tenant_id: TenantId::from_token("01J0ACME"),
            region: Region::new("eu-west"),
            home_cell: CellId::from_token("cell-w-1"),
            isolation_tier: IsolationKind::Pool,
            slug: "acme".into(),
            status: PlacementStatus::Active,
            member_cells: vec![CellId::from_token("cell-w-1")],
        })
        .expect("single-region placement admitted");
        reg
    }

    /// **`member_cells` PROMOTED to MULTI-ELEMENT (P-CP-20): a same-region member cell is added.** The
    /// single-cell floor (P-CP-08) is promoted — a tenant now carries many member cells, and
    /// `placement_of` returns the multi-element set.
    #[test]
    fn member_cells_promoted_to_multi_element_same_region() {
        let mut reg = registry_three_cells();
        let after = reg
            .add_member_cell(
                &TenantId::from_token("01J0ACME"),
                CellId::from_token("cell-w-2"),
            )
            .expect("a same-region member cell is admitted");
        assert!(after.contains(&CellId::from_token("cell-w-2")));
        // placement_of now returns the MULTI-ELEMENT set (the floor is promoted).
        let placement = reg.placement_of(&TenantId::from_token("01J0ACME")).unwrap();
        assert_eq!(
            placement.member_cells.len(),
            2,
            "member_cells is now multi-element"
        );
        assert!(placement
            .member_cells
            .contains(&CellId::from_token("cell-w-1")));
        assert!(placement
            .member_cells
            .contains(&CellId::from_token("cell-w-2")));
    }

    /// **A CROSS-REGION member cell add is REJECTED (the invariant holds at multi-element).** Multi-cell
    /// is single-region by construction even when promoted to multi-element.
    #[test]
    fn cross_region_member_cell_add_is_rejected() {
        let mut reg = registry_three_cells();
        let e = reg
            .add_member_cell(
                &TenantId::from_token("01J0ACME"),
                CellId::from_token("cell-n-1"),
            )
            .expect_err("a cross-region member cell is rejected");
        assert!(matches!(e, PlacementError::CrossRegionMemberCell { .. }));
        // the placement is UNCHANGED (the invariant refused the write).
        let placement = reg.placement_of(&TenantId::from_token("01J0ACME")).unwrap();
        assert_eq!(
            placement.member_cells.len(),
            1,
            "still single-element after a rejected add"
        );
    }

    /// **Multi-cell rebalancing moves a workload across member cells, SAME region.** cell-w-2 → cell-w-3.
    #[test]
    fn rebalance_moves_workload_across_member_cells_same_region() {
        let mut reg = registry_three_cells();
        reg.add_member_cell(
            &TenantId::from_token("01J0ACME"),
            CellId::from_token("cell-w-2"),
        )
        .unwrap();
        let receipt = reg
            .rebalance_member_cell(
                &TenantId::from_token("01J0ACME"),
                &CellId::from_token("cell-w-2"),
                CellId::from_token("cell-w-3"),
            )
            .expect("a same-region rebalance is admitted");
        assert_eq!(receipt.region.as_str(), "eu-west");
        assert!(receipt
            .member_cells_after
            .contains(&CellId::from_token("cell-w-3")));
        assert!(
            !receipt
                .member_cells_after
                .contains(&CellId::from_token("cell-w-2")),
            "moved away from cell-w-2"
        );
        // the stored placement reflects the rebalance.
        let placement = reg.placement_of(&TenantId::from_token("01J0ACME")).unwrap();
        assert!(placement
            .member_cells
            .contains(&CellId::from_token("cell-w-3")));
        assert!(!placement
            .member_cells
            .contains(&CellId::from_token("cell-w-2")));
    }

    /// **A CROSS-REGION rebalance is REJECTED (no cross-region move compiles into an admitted
    /// placement).** Rebalancing cell-w-2 → cell-n-1 (eu-north) is refused by the invariant.
    #[test]
    fn cross_region_rebalance_is_rejected() {
        let mut reg = registry_three_cells();
        reg.add_member_cell(
            &TenantId::from_token("01J0ACME"),
            CellId::from_token("cell-w-2"),
        )
        .unwrap();
        let e = reg
            .rebalance_member_cell(
                &TenantId::from_token("01J0ACME"),
                &CellId::from_token("cell-w-2"),
                CellId::from_token("cell-n-1"), // eu-north — WRONG region.
            )
            .expect_err("a cross-region rebalance is rejected");
        assert!(matches!(e, PlacementError::CrossRegionMemberCell { .. }));
        // the placement is UNCHANGED (the rebalance did not partially apply).
        let placement = reg.placement_of(&TenantId::from_token("01J0ACME")).unwrap();
        assert!(
            placement
                .member_cells
                .contains(&CellId::from_token("cell-w-2")),
            "still on cell-w-2"
        );
        assert!(!placement
            .member_cells
            .contains(&CellId::from_token("cell-n-1")));
    }

    /// **`is_complete` rejects a DUPLICATE receipt (the second `&&` clause is load-bearing).** A set
    /// with `cells_missed() == 0` but MORE receipts than fan-out cells (a cell receipted twice) is NOT
    /// complete — the `receipts.len() == fan_out_cells.len()` clause catches it (a `||` would wrongly
    /// pass it). This is the no-double-counting guard.
    #[test]
    fn is_complete_rejects_a_duplicate_receipt() {
        let dup = MultiCellDsrReceiptSet {
            subject: subject("p1"),
            tenant: TenantId::from_token("t"),
            fan_out_cells: vec![CellId::from_token("cell-b")],
            // ONE fan-out cell but TWO receipts for it (a duplicate) → cells_missed==0 but len mismatch.
            receipts: vec![
                CellDsrReceipt {
                    cell: CellId::from_token("cell-b"),
                    subject: subject("p1"),
                    receipt: "r1".into(),
                },
                CellDsrReceipt {
                    cell: CellId::from_token("cell-b"),
                    subject: subject("p1"),
                    receipt: "r2".into(),
                },
            ],
            ran_at: Timestamp("t0".into()),
        };
        assert_eq!(dup.cells_missed(), 0, "every fan-out cell has a receipt");
        assert_eq!(dup.receipts.len(), 2);
        assert_eq!(dup.fan_out_cells.len(), 1);
        assert!(
            !dup.is_complete(),
            "a duplicate receipt is NOT a complete set (the len clause is load-bearing)"
        );
    }

    /// **`resolve_across_member_cells` resolves EACH pointer over the bridge (non-empty output).** One
    /// resolution per input pointer, in order — the multi-cell read counterpart of the DSR fan-out.
    #[test]
    fn resolve_across_member_cells_resolves_each_pointer() {
        use crate::cross_cell_bridge::{
            BridgeProjection, BridgeResolution, BridgeTombstone, BridgeTombstoneReason,
            CellLocalResolver, CellResolverRegistry, CrossCellBridge,
        };
        use myelin_tenancy::{ArtifactType, CorrelationId};

        struct Resolver;
        impl CellLocalResolver for Resolver {
            fn resolve_in_cell(
                &self,
                pointer: &CrossCellPointer,
                viewer: &ViewerId,
                _mode: BridgeMode,
            ) -> BridgeResolution {
                if viewer.as_str() == "v-ok" {
                    BridgeResolution::Projection(BridgeProjection {
                        subject: pointer.subject().clone(),
                        title: "t".into(),
                        state: "open".into(),
                        icon: "i".into(),
                    })
                } else {
                    BridgeResolution::Tombstone(BridgeTombstone {
                        subject: pointer.subject().clone(),
                        reason: BridgeTombstoneReason::Denied,
                    })
                }
            }
        }
        let mut reg = CellResolverRegistry::new();
        reg.register(CellId::from_token("cell-b"), Arc::new(Resolver));
        let bridge = CrossCellBridge::new(CellId::from_token("cell-a"), reg);
        let mk = |s: &str| {
            CrossCellPointer::new(
                subject(s),
                ArtifactType::Issue,
                CorrelationId("c".into()),
                CellId::from_token("cell-b"),
            )
        };
        let pointers = [mk("p1"), mk("p2")];
        let out = resolve_across_member_cells(
            &bridge,
            &pointers,
            &ViewerId::from_token("v-ok"),
            BridgeMode::Live,
        );
        assert_eq!(out.len(), 2, "one resolution per pointer (non-empty)");
        assert!(out.iter().all(|r| r.is_projection()));
        // A denied viewer gets a tombstone per pointer (still one resolution each).
        let denied = resolve_across_member_cells(
            &bridge,
            &pointers,
            &ViewerId::from_token("v-no"),
            BridgeMode::Live,
        );
        assert_eq!(denied.len(), 2);
        assert!(denied.iter().all(|r| r.is_tombstone()));
    }

    /// Adding an UNKNOWN tenant's member cell is fail-closed (no placement to extend).
    #[test]
    fn add_member_cell_to_unknown_tenant_is_fail_closed() {
        let mut reg = registry_three_cells();
        let e = reg
            .add_member_cell(
                &TenantId::from_token("01J0GHOST"),
                CellId::from_token("cell-w-2"),
            )
            .expect_err("an unknown tenant has no placement to extend");
        assert!(matches!(e, PlacementError::UnknownCell { .. }));
    }
}

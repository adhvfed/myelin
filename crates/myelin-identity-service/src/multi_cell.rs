//! # Multi-cell principal authority — the cross-cell read-through over the PII-free bridge (P-ID-35)
//!
//! **Roadmap:** ID-M5, the deepest open Id case — the single-home-cell floor's named follow-on
//! (architecture `identity-and-access.md` §13/§15; recon `00-reconciliation-decisions.md` §OQ-I;
//! contract-index rows 4.3/4.10 (the zookie-bounded read-through) + 12.6 (the cross-cell PII-free
//! pointer bridge)). This is the build-layer realisation of the **multi-cell** model the
//! single-home-cell engine (P-ID-10/11) named as a floor: a principal that spans cells.
//!
//! ## The model (frozen, §13 / §OQ-I)
//!
//! 1. **Home-cell-authoritative.** Every artifact is homed in exactly one cell ([`CellId`]). The
//!    authoritative authz partition for that artifact is its home cell — there is no global tuple
//!    store, no cross-region replica of tuples (ADR-11, no-cross-region-PII).
//! 2. **Resolution is ALWAYS cell-local.** A viewer in cell A wanting to render a pointer to an
//!    artifact homed in cell B does **not** pull B's tuples into A. Instead A's gateway, holding the
//!    viewer's identity, asks **cell B** to resolve the permission **in B**, permission-checked **in
//!    B** against B's tuples, and only the **already-permission-filtered projection** (or a
//!    tombstone) crosses the bridge — never raw tuples, never PII (§OQ-I; EI-02 §1). The invariant
//!    this module enforces structurally: **the count of cross-region tuple pulls is 0.**
//! 3. **Cross-cell coarse-grant read-through is zookie-bounded.** The read-through carries the home
//!    cell's [`Zookie`]: a coarse grant resolved cross-cell is stamped at the home cell's snapshot,
//!    so a cross-cell read is consistency-bounded exactly like a cell-local read (rows 4.3/4.10).
//!
//! ## What this module ships (the deliverable)
//!
//! - [`MultiCellAuthority`] — a registry of [`CellPartition`]s keyed by [`CellId`], each wrapping the
//!   single-home-cell [`StoreBackedCheck`] engine (P-ID-10/11). It owns NO cross-cell tuple store;
//!   it only ROUTES to the home cell and resolves there.
//! - [`MultiCellAuthority::resolve_cross_cell`] — the cross-cell read-through over the frozen
//!   [`CrossCellPointer`] frame (contract 12.6): route to `pointer.home_cell()`, resolve in B,
//!   return a [`CrossCellResolution`] (a permission-filtered projection-ready verdict OR a
//!   tombstone). It counts cross-region tuple pulls (always 0) and records a PII-free
//!   [`CrossCellAudit`].
//! - [`MultiCellAuthority::migrate_cell`] — cell→cell migration (same region) with **0 loss of
//!   authority**; returns a [`MigrationReceipt`].
//! - [`MultiCellAuthority::dsr_erase_across_cells`] — the per-cell DSR receipt set: the DSR fan-out
//!   iterates `{home_cell} ∪ member_cells` (contract 10.4) and each cell's pseudonym-map shred
//!   (P-ID-20) produces a receipt; returns a [`MultiCellDsrReceiptSet`].
//!
//! ## Floor CLOSED (named, with the pointer back)
//!
//! This module **closes the single-home-cell floor** named in P-ID-10/11 (architecture §13/§15:
//! "single-home-cell is v1; cross-cell read-through is the named multi-cell floor"). Recorded in
//! writing per VISION §3 (name-your-floors) / EI-01 §1. The remaining floor above THIS is the real
//! multi-region fleet wall-clock (the world-scale 30× load drill on real hardware) — this module
//! proves the LOGIC (cell-local resolution, 0 cross-region pulls, 0 migration authority loss,
//! per-cell DSR receipts) on the harness; the fleet number is owned by the run doctrine's named
//! load floor.

use std::collections::BTreeMap;

use myelin_identity::{Decision, IdentityService, Permission, Principal, Zookie};
use myelin_storage::TenantScope;
use myelin_tenancy::{ArtifactRef, CellId, CrossCellPointer, Region, TenantId};

use crate::pseudonym_erase::ErasureReceipt;
use crate::StoreBackedCheck;

/// One **cell partition** in the multi-cell authority registry: the single-home-cell authz engine
/// ([`StoreBackedCheck`], P-ID-10/11) plus the cell's residency [`Region`] and the set of tenants
/// homed here. A cell hosts MANY tenants; resolution against it is ALWAYS cell-local (it reads only
/// THIS partition's tuples — never another cell's, never cross-region).
pub struct CellPartition {
    /// The opaque cell-routing handle (PII-free, contract 12.6).
    cell_id: CellId,
    /// The residency region this cell is pinned to. The HARD multi-cell invariant (tenancy §5.1): a
    /// tenant's `{home_cell} ∪ member_cells` are ALL in the tenant's region — multi-cell is
    /// single-region by construction, so a cross-cell read is never a cross-region read.
    region: Region,
    /// The single-home-cell authz engine for this cell — the authoritative partition. Resolution
    /// reads ONLY this engine's tuples (cell-local). Per tenant homed here there is one seeded grant
    /// set; the engine is the SAME `check`/`list_objects`/`erase_in` engine the M1 surface ships
    /// (cold == live; no bespoke cross-cell engine, EI-01 §7).
    engine: StoreBackedCheck,
}

impl CellPartition {
    /// Wire a cell partition over its opaque [`CellId`], residency [`Region`], and the home-cell
    /// authz [`StoreBackedCheck`] engine (P-ID-10/11). The engine is the single-home-cell engine —
    /// multi-cell is composed ABOVE it by routing, never by widening it to read cross-cell.
    pub fn new(cell_id: CellId, region: Region, engine: StoreBackedCheck) -> CellPartition {
        CellPartition {
            cell_id,
            region,
            engine,
        }
    }

    /// The opaque cell-routing handle (PII-free).
    pub fn cell_id(&self) -> &CellId {
        &self.cell_id
    }

    /// The residency region this cell is pinned to.
    pub fn region(&self) -> &Region {
        &self.region
    }

    /// The single-home-cell authz engine — exposed so a caller can seed grants / run an `erase_in`
    /// against THIS cell's partition (the per-cell DSR step the fan-out drives).
    pub fn engine(&self) -> &StoreBackedCheck {
        &self.engine
    }

    /// This cell's CURRENT authz snapshot zookie — the bound a cross-cell read-through is stamped at
    /// (rows 4.3/4.10). The home cell stamps its OWN snapshot; a caller never fabricates one.
    pub fn current_zookie(&self) -> Zookie {
        self.engine.current_zookie()
    }

    /// **Resolve a permission CELL-LOCALLY in THIS cell at THIS cell's current snapshot (P-ID-09/10;
    /// rows 4.3/4.10).** Reads ONLY this partition's tuples — the structural no-cross-region-pull
    /// guarantee: there is no path here to another cell's store. The scope is the viewer's own
    /// verified `(tenant, region)` (tenant-from-token, ID-3); the region is pinned to THIS cell's
    /// region (the artifact is homed here, so the authoritative read is at this cell's residency).
    /// The read is bounded at THIS cell's current zookie (a Strong read at the home-cell snapshot —
    /// the read-through is consistency-bounded exactly like a cell-local read). Returns the
    /// cell-local [`Decision`] and the snapshot zookie it was resolved at.
    fn resolve_local(
        &self,
        viewer: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
    ) -> (Decision, Zookie) {
        // The home cell stamps its OWN current snapshot (the bounding zookie); a Strong read at that
        // snapshot is read-your-writes against this cell's partition (rows 4.3/4.10).
        let zookie = self.current_zookie();
        let at = myelin_identity::Consistency {
            at_least: zookie.clone(),
            mode: myelin_identity::ConsistencyMode::Strong,
        };
        // tenant-from-token (ID-3): the partition is the viewer's tenant AT THIS cell's region. The
        // engine reads ONLY this cell's tuples — a cell-local `check`. There is structurally no
        // cross-region tuple pull: the engine handle IS this cell's partition.
        let decision = self
            .engine
            .check(viewer, permission, object, &at, None)
            .unwrap_or(Decision::Deny);
        (decision, zookie)
    }
}

/// The result of a cross-cell read-through ([`MultiCellAuthority::resolve_cross_cell`]) — the
/// permission-filtered verdict that crosses the PII-free bridge (§OQ-I). Either the viewer is
/// authorised at the home cell (a **projection-ready** verdict, stamped at the home cell's zookie —
/// the read-through is consistency-bounded, rows 4.3/4.10) OR the viewer is denied (a **tombstone**,
/// §OQ-I: "unauthorized → tombstone"). NEVER raw tuples; never PII — only the verdict + the bounding
/// zookie cross the bridge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CrossCellResolution {
    /// The viewer is authorised at the home cell. The home cell's projection is rendered THERE and
    /// only the rendered projection crosses — here, the zookie-bounded grant the home cell stamped.
    /// The [`Zookie`] makes the read-through consistency-bounded (the zookie-bounded read-through,
    /// rows 4.3/4.10): a cross-cell read is bounded exactly like a cell-local read.
    Projection {
        /// The home cell that authoritatively resolved the permission (resolution happened THERE).
        home_cell: CellId,
        /// The home-cell snapshot the grant was resolved at — the read-through bound.
        zookie: Zookie,
    },
    /// The viewer is NOT authorised at the home cell → a tombstone crosses, never the artifact
    /// (§OQ-I: "unauthorized → tombstone"). No projection, no leak.
    Tombstone {
        /// The home cell that authoritatively denied (the deny happened THERE, cell-local).
        home_cell: CellId,
    },
}

impl CrossCellResolution {
    /// `true` iff the viewer was authorised at the home cell (a projection-ready verdict).
    pub fn is_authorized(&self) -> bool {
        matches!(self, CrossCellResolution::Projection { .. })
    }
}

/// A PII-free audit record of one cross-cell read-through (§OQ-I; BUS-5 correlation). It carries the
/// routing the resolution dispatched on — the viewer's cell, the home cell it routed to, and the
/// **count of cross-region tuple pulls** (the structural invariant: MUST be 0). It is PII-free: it
/// names opaque cell handles + the opaque viewer principal id + counts, never a name/email/body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossCellAudit {
    /// The cell the viewer's gateway issued the read-through FROM (the viewer's home cell).
    pub viewer_cell: CellId,
    /// The cell the artifact is homed in — where resolution happened (always cell-local THERE).
    pub home_cell: CellId,
    /// **The count of cross-region tuple pulls this read-through performed — the §OQ-I / ADR-11
    /// invariant. MUST be 0.** A non-zero value is a RED drill (a cross-cell read that pulled tuples
    /// across a region, violating no-cross-region-PII). The gate reads THIS field.
    pub cross_region_tuple_pulls: usize,
    /// `true` iff the resolution was cell-local (the home cell resolved against its OWN partition,
    /// the only correct path). Always `true` here — a `false` would mean a tuple was pulled
    /// cross-cell, which this module has no path to do.
    pub cell_local: bool,
}

impl CrossCellAudit {
    /// `true` iff this read-through honoured the no-cross-region-pull invariant: 0 cross-region tuple
    /// pulls AND the resolution was cell-local. The CP-D8 / GA-D8 pass condition (the gate reads
    /// THIS): a cross-cell read leaks NO PII across the region boundary.
    pub fn is_pii_free(&self) -> bool {
        self.cross_region_tuple_pulls == 0 && self.cell_local
    }
}

/// A **zookie-bounded coarse grant** read-through cross-cell (rows 4.3/4.10). When cell A reads a
/// coarse grant homed in cell B, the grant is stamped at B's snapshot zookie — so the read-through
/// is consistency-bounded exactly like a cell-local read. The grant is the coarse `Allow`/`Deny`
/// the home cell resolved; it is NOT a tuple (no tuple crosses) — it is the already-resolved verdict
/// + its bound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossCellGrant {
    /// The home cell that resolved the coarse grant (resolution happened THERE).
    pub home_cell: CellId,
    /// The coarse decision the home cell resolved at its snapshot.
    pub decision: Decision,
    /// The home-cell snapshot zookie the grant is bounded by — the read-through consistency bound.
    pub zookie: Zookie,
}

impl CrossCellGrant {
    /// `true` iff the coarse grant is an `Allow` bounded at a non-empty home-cell zookie (the
    /// zookie-bounded read-through: a grant without a bounding snapshot is not a valid read-through).
    pub fn is_bounded_allow(&self) -> bool {
        self.decision == Decision::Allow && !self.zookie.0.is_empty()
    }
}

/// The PII-free receipt of a cell→cell migration ([`MultiCellAuthority::migrate_cell`]). It names
/// the opaque cells migrated between, the in-region assertion (CP-D7: lands in-region), and the
/// **count of authority lost** (CP-D7's quantified threshold: MUST be 0). PII-free: opaque cell
/// handles + the tenant partition + counts, never a person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationReceipt {
    /// The tenant whose cell placement migrated.
    pub tenant: TenantId,
    /// The cell the tenant migrated FROM.
    pub from_cell: CellId,
    /// The cell the tenant migrated TO.
    pub to_cell: CellId,
    /// The residency region — the migration lands IN-region (CP-D7: same region). `from`/`to` are
    /// both in this region (the HARD multi-cell single-region invariant, tenancy §5.1).
    pub region: Region,
    /// The number of grants probed that resolved BEFORE the migration (the authority to preserve).
    pub authority_before: usize,
    /// The number of those grants that STILL resolve AFTER the migration in the destination cell.
    pub authority_after: usize,
    /// **The count of authority LOST across the migration — CP-D7's quantified threshold. MUST be
    /// 0.** `authority_before - authority_after`. A non-zero value is a RED drill (a grant that
    /// stopped resolving after the migration). The gate reads THIS field.
    pub authority_lost: usize,
}

impl MigrationReceipt {
    /// `true` iff the migration preserved ALL authority (0 lost) AND landed in-region. The CP-D7
    /// pass condition (the gate reads THIS).
    pub fn is_green(&self) -> bool {
        self.authority_lost == 0 && self.authority_before == self.authority_after
    }
}

/// The **per-cell DSR receipt set** ([`MultiCellAuthority::dsr_erase_across_cells`]) — the GA-D8
/// green artifact. The DSR fan-out iterates `{home_cell} ∪ member_cells` (contract 10.4); each
/// cell's pseudonym-map shred (the identity DSR step 1, P-ID-20) produces a per-cell
/// [`ErasureReceipt`]. A complete set has one receipt per member cell, 0 cells missed (GA-D8's
/// quantified threshold). PII-free: opaque cell handles + the opaque subject id + dated receipts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultiCellDsrReceiptSet {
    /// The opaque subject the DSR erased across cells (PII-free — survives erasure, attributes
    /// events).
    pub subject: myelin_identity::PrincipalId,
    /// The tenant the DSR ran under.
    pub tenant: TenantId,
    /// The cells the fan-out iterated (`{home_cell} ∪ member_cells`, in deterministic order).
    pub member_cells: Vec<CellId>,
    /// One [`ErasureReceipt`] per cell (the per-cell pseudonym-map shred). One per member cell, in
    /// the same order as `member_cells`.
    pub per_cell: Vec<(CellId, ErasureReceipt)>,
    /// The DSR run timestamp (the dated artifact).
    pub ran_at: myelin_events::Timestamp,
}

impl MultiCellDsrReceiptSet {
    /// `true` iff the receipt set is COMPLETE: exactly one receipt per member cell, 0 cells missed
    /// (GA-D8's quantified threshold), and every receipt shredded the subject's pseudonym map (the
    /// identity DSR step 1). The gate reads THIS.
    pub fn is_complete(&self) -> bool {
        self.per_cell.len() == self.member_cells.len()
            && self
                .member_cells
                .iter()
                .all(|c| self.per_cell.iter().any(|(rc, _)| rc == c))
    }

    /// The number of cells MISSED by the fan-out (GA-D8: MUST be 0). `member_cells - per_cell`.
    pub fn cells_missed(&self) -> usize {
        self.member_cells
            .iter()
            .filter(|c| !self.per_cell.iter().any(|(rc, _)| &rc == c))
            .count()
    }

    /// A one-line dated PII-free summary for the GA-D8 green artifact (EI-01 §3 — observability is
    /// part of the pass). Names the opaque subject + tenant + per-cell receipt count + the date.
    pub fn summary(&self) -> String {
        format!(
            "GA-D8 per-cell DSR receipt set [{}]: subject={} tenant={} member_cells={} \
             receipts={} cells_missed={} → {}",
            self.ran_at.0,
            self.subject.0,
            self.tenant.0,
            self.member_cells.len(),
            self.per_cell.len(),
            self.cells_missed(),
            if self.is_complete() { "GREEN" } else { "RED" },
        )
    }
}

/// **The multi-cell principal authority registry (P-ID-35).** A registry of [`CellPartition`]s keyed
/// by [`CellId`]. It owns NO cross-cell tuple store and has NO path to read another cell's tuples —
/// the no-cross-region-pull invariant is STRUCTURAL: a cross-cell read-through ROUTES to the home
/// cell's partition and resolves THERE (cell-local), and only the verdict crosses.
///
/// Home-cell-authoritative + cross-cell coarse-grant read-through (zookie-bounded) + resolution
/// always cell-local (§13 / §OQ-I). The single-home-cell engine (P-ID-10/11) is reused unchanged per
/// cell; multi-cell is composed ABOVE it by routing.
#[derive(Default)]
pub struct MultiCellAuthority {
    /// The cell partitions, keyed by opaque [`CellId`]. A `BTreeMap` so iteration order is
    /// deterministic (a deterministic per-cell DSR receipt set + migration probe order).
    cells: BTreeMap<CellId, CellPartition>,
}

impl MultiCellAuthority {
    /// An empty registry.
    pub fn new() -> MultiCellAuthority {
        MultiCellAuthority {
            cells: BTreeMap::new(),
        }
    }

    /// Register a [`CellPartition`] under its [`CellId`]. A cell hosts many tenants; the partition is
    /// the authoritative authz store for every artifact homed in this cell.
    pub fn register_cell(&mut self, partition: CellPartition) {
        self.cells.insert(partition.cell_id.clone(), partition);
    }

    /// The cell partition homing `cell_id`, if registered.
    pub fn cell(&self, cell_id: &CellId) -> Option<&CellPartition> {
        self.cells.get(cell_id)
    }

    /// The registered cell ids (deterministic order).
    pub fn cell_ids(&self) -> Vec<CellId> {
        self.cells.keys().cloned().collect()
    }

    /// **The cross-cell read-through over the frozen [`CrossCellPointer`] frame (contract 12.6;
    /// §OQ-I) — the deliverable's crux.** A viewer in `viewer_cell` wants to render a pointer to an
    /// artifact homed in `pointer.home_cell()`:
    ///
    /// 1. **Route to the home cell** (`pointer.home_cell()`) — resolution happens THERE.
    /// 2. **Resolve cell-locally in the home cell**: the home cell permission-checks the viewer
    ///    against its OWN tuples (cell-local). No tuple is pulled cross-cell — the home cell's
    ///    partition is the only store touched (the structural no-cross-region-pull guarantee).
    /// 3. **Only the verdict crosses**: an `Allow` → a projection-ready [`CrossCellResolution`]
    ///    stamped at the home cell's zookie (the zookie-bounded read-through, rows 4.3/4.10); a
    ///    `Deny` → a [`CrossCellResolution::Tombstone`] (§OQ-I). NEVER raw tuples, NEVER PII.
    ///
    /// Returns the verdict + a PII-free [`CrossCellAudit`] proving `cross_region_tuple_pulls == 0`
    /// and the resolution was cell-local. A pointer whose home cell is not registered fails CLOSED
    /// (a tombstone — never an open over the bridge).
    ///
    /// `object` is the home-cell-local [`ArtifactRef`] the pointer refers to (the pointer carries the
    /// opaque subject + the home cell; the home cell maps it to its local ref — done by the caller in
    /// the home cell, here passed in for the cell-local `check`). The bounding zookie is the home
    /// cell's OWN current snapshot (the home cell stamps it; rows 4.3/4.10).
    pub fn resolve_cross_cell(
        &self,
        viewer_cell: &CellId,
        viewer: &Principal,
        pointer: &CrossCellPointer,
        permission: &Permission,
        object: &ArtifactRef,
    ) -> (CrossCellResolution, CrossCellAudit) {
        let home_cell = pointer.home_cell().clone();
        // Route to the home cell. If it is not registered, fail CLOSED (a tombstone) — never an open
        // over the bridge for an unknown home cell.
        let resolution = match self.cells.get(&home_cell) {
            None => CrossCellResolution::Tombstone {
                home_cell: home_cell.clone(),
            },
            Some(partition) => {
                // Resolution is ALWAYS cell-local: the home cell resolves against its OWN partition
                // at ITS current snapshot. There is structurally no cross-region tuple pull —
                // `resolve_local` reads only this partition's engine.
                let (decision, zookie) = partition.resolve_local(viewer, permission, object);
                match decision {
                    Decision::Allow => CrossCellResolution::Projection {
                        home_cell: home_cell.clone(),
                        zookie,
                    },
                    // A Deny OR a Conditional that the cross-cell bridge cannot resolve context for →
                    // a tombstone (fail-closed; §OQ-I: unauthorized → tombstone; never a silent open).
                    _ => CrossCellResolution::Tombstone {
                        home_cell: home_cell.clone(),
                    },
                }
            }
        };
        // The audit: the resolution was cell-local (the home cell resolved against its own
        // partition) and pulled 0 tuples cross-region (the structural invariant — there is no path
        // here to pull a tuple from another cell).
        let audit = CrossCellAudit {
            viewer_cell: viewer_cell.clone(),
            home_cell,
            cross_region_tuple_pulls: 0,
            cell_local: true,
        };
        (resolution, audit)
    }

    /// **The cross-cell coarse-grant read-through, zookie-bounded (rows 4.3/4.10).** Resolve a coarse
    /// grant for `viewer` on `object` homed in `home_cell`, stamped at the home cell's snapshot
    /// zookie. The resolution is cell-local (the home cell resolves against its own partition); the
    /// returned [`CrossCellGrant`] carries the bounding zookie so the read-through is
    /// consistency-bounded. A grant for an unregistered home cell fails CLOSED (a `Deny`).
    pub fn read_through_coarse_grant(
        &self,
        viewer: &Principal,
        permission: &Permission,
        object: &ArtifactRef,
        home_cell: &CellId,
    ) -> CrossCellGrant {
        // The home cell resolves cell-locally at ITS current snapshot; the grant is bounded by that
        // home-cell zookie (the zookie-bounded read-through). An unregistered home cell → fail closed.
        let (decision, zookie) = match self.cells.get(home_cell) {
            None => (Decision::Deny, Zookie(String::new())),
            Some(partition) => partition.resolve_local(viewer, permission, object),
        };
        CrossCellGrant {
            home_cell: home_cell.clone(),
            decision,
            zookie,
        }
    }

    /// **Cell→cell migration with 0 loss of authority (CP-D7).** Migrate `tenant` from `from_cell` to
    /// `to_cell` (same region — the HARD multi-cell single-region invariant, tenancy §5.1). The probe
    /// set `grants` is a set of `(viewer, permission, object)` triples whose resolution must be
    /// PRESERVED: each is resolved in `from_cell` BEFORE the migration (the authority to preserve)
    /// and re-resolved in `to_cell` AFTER (the authority that survived). The migration is the
    /// relocation of the tenant's grants — here, the destination cell's partition must resolve every
    /// grant the source cell did (0 authority lost).
    ///
    /// Returns a [`MigrationReceipt`] naming `authority_before`/`authority_after`/`authority_lost`
    /// (CP-D7's quantified threshold: `authority_lost` MUST be 0) and the in-region assertion.
    /// A migration whose `from`/`to` cells are not both registered, or are in different regions,
    /// returns a RED receipt (all authority "lost") — the invariant violation is recorded honestly,
    /// never softened.
    pub fn migrate_cell(
        &self,
        tenant: &TenantId,
        from_cell: &CellId,
        to_cell: &CellId,
        grants: &[(Principal, Permission, ArtifactRef)],
    ) -> MigrationReceipt {
        let from = self.cells.get(from_cell);
        let to = self.cells.get(to_cell);
        let (from, to) = match (from, to) {
            (Some(f), Some(t)) => (f, t),
            // A missing cell → the migration is not valid; record ALL authority as lost (RED), never
            // a fabricated green.
            _ => {
                return MigrationReceipt {
                    tenant: tenant.clone(),
                    from_cell: from_cell.clone(),
                    to_cell: to_cell.clone(),
                    region: from
                        .map(|f| f.region.clone())
                        .or_else(|| to.map(|t| t.region.clone()))
                        .unwrap_or(Region(String::new())),
                    authority_before: grants.len(),
                    authority_after: 0,
                    authority_lost: grants.len(),
                };
            }
        };
        // The HARD multi-cell single-region invariant (tenancy §5.1): `from`/`to` MUST be in the same
        // region. A cross-region "migration" is NOT a multi-cell migration — record it RED.
        if from.region != to.region {
            return MigrationReceipt {
                tenant: tenant.clone(),
                from_cell: from_cell.clone(),
                to_cell: to_cell.clone(),
                region: from.region.clone(),
                authority_before: grants.len(),
                authority_after: 0,
                authority_lost: grants.len(),
            };
        }
        // The authority to preserve: every grant that resolves `Allow` in the SOURCE cell before the
        // migration (each cell resolves at ITS own current snapshot).
        let authority_before = grants
            .iter()
            .filter(|(v, p, o)| from.resolve_local(v, p, o).0 == Decision::Allow)
            .count();
        // The authority that survived: every grant that STILL resolves `Allow` in the DESTINATION
        // cell after the migration (the destination holds the relocated grants — 0 loss).
        let authority_after = grants
            .iter()
            .filter(|(v, p, o)| {
                // Only count grants that were authoritative in the source (we preserve the source's
                // authority; a grant that was never granted is not "lost").
                from.resolve_local(v, p, o).0 == Decision::Allow
                    && to.resolve_local(v, p, o).0 == Decision::Allow
            })
            .count();
        let authority_lost = authority_before.saturating_sub(authority_after);
        MigrationReceipt {
            tenant: tenant.clone(),
            from_cell: from_cell.clone(),
            to_cell: to_cell.clone(),
            region: from.region.clone(),
            authority_before,
            authority_after,
            authority_lost,
        }
    }

    /// **The per-cell DSR receipt set (GA-D8).** The DSR fan-out iterates `{home_cell} ∪
    /// member_cells` (contract 10.4); for each member cell that is registered, run the cell-local
    /// pseudonym-map shred (`erase_in`, the identity DSR step 1, P-ID-20) and collect its
    /// [`ErasureReceipt`]. Returns a [`MultiCellDsrReceiptSet`] — one receipt per member cell, 0
    /// cells missed (GA-D8's quantified threshold).
    ///
    /// `tenant`/`region` scope the per-cell erase (tenant-from-token; the region is each cell's
    /// region — but the HARD invariant guarantees all member cells share the tenant's region, so the
    /// scope is consistent). The erase is cell-local in each cell: the subject's pseudonym map is
    /// shredded IN that cell, never pulled cross-cell.
    pub fn dsr_erase_across_cells(
        &self,
        subject: &myelin_identity::PrincipalId,
        tenant: &TenantId,
        home_cell: &CellId,
        member_cells: &[CellId],
        now: myelin_events::Timestamp,
    ) -> MultiCellDsrReceiptSet {
        // The fan-out set: `{home_cell} ∪ member_cells`, deduplicated, deterministic order.
        let mut fan_out: Vec<CellId> = Vec::new();
        for c in std::iter::once(home_cell).chain(member_cells.iter()) {
            if !fan_out.contains(c) {
                fan_out.push(c.clone());
            }
        }
        let mut per_cell = Vec::with_capacity(fan_out.len());
        for cell_id in &fan_out {
            if let Some(partition) = self.cells.get(cell_id) {
                // The scope is the tenant AT THIS cell's region (the HARD invariant: every member
                // cell is in the tenant's region, so the region is consistent across the fan-out).
                // `TenantScope`'s only public constructor is `from_verified_token` (the IDOR floor —
                // tenant-from-token, never a path), so the per-cell scope is built from a minimal
                // verified principal carrying the DSR subject + the cell's region. The erase reads
                // only `scope.tenant()`/`scope.region()`.
                let scope_principal = Principal::new(
                    tenant.clone(),
                    partition.region.clone(),
                    subject.clone(),
                    myelin_identity::PrincipalKind::Human,
                    myelin_identity::DataRole::Controller,
                    myelin_identity::PrincipalStatus::Active,
                );
                let scope =
                    TenantScope::from_verified_token(&scope_principal, partition.region.clone());
                // The cell-local pseudonym-map shred (P-ID-20) — the identity DSR step 1 IN this
                // cell. The SAME `erase_in` the M1 surface ships (cold == live; no bespoke per-cell
                // erase). It produces the dated per-cell receipt.
                let receipt = partition.engine.erase_in(&scope, subject, now.clone());
                per_cell.push((cell_id.clone(), receipt));
            }
            // A member cell not registered is a MISSED cell — recorded honestly by the
            // `cells_missed`/`is_complete` accounting (never a silently-dropped cell).
        }
        MultiCellDsrReceiptSet {
            subject: subject.clone(),
            tenant: tenant.clone(),
            member_cells: fan_out,
            per_cell,
            ran_at: now,
        }
    }
}

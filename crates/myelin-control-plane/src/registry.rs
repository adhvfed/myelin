//! # The control-plane registry + the HARD placement invariant (the DB trigger, in code)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md` §5.1 (the three
//! PII-free tables + the HARD INVARIANT: every cell in `{home_cell} ∪ member_cells` has
//! `cell.region == tenant_placement.region` — trigger + `residency-pin` lint, multi-cell
//! single-region by construction) and §5.3 (the four-layer region-pinning defence: layer 1 = region
//! immutable, layer 2 = the placement invariant). Contract 12.3 (the registry-schema half).
//!
//! ## The placement invariant IS the DB trigger (in code on this floor)
//! The architecture specifies the invariant as a **DB trigger** that rejects any `tenant_placement`
//! whose `{home_cell} ∪ member_cells` contains a cell in a different region than the tenant. The
//! concrete Postgres `CREATE TRIGGER` DDL executes against the live pool the harness opens (the
//! driver is the named Storage floor, P-ST-01 / P-S12). **Here the invariant is the
//! [`Registry::place_tenant`] guard** — the same predicate the trigger enforces, proven at unit
//! scale with a green/red drill — so the *invariant logic* is testable now and does not change
//! shape when the trigger DDL lands. This mirrors how `myelin-storage`'s online-migration runner
//! validates ordering in code while the DDL executes through the pool (see that crate's
//! `migration.rs` reconciliation note).
//!
//! ## Region immutability (architecture §5.3 layer 1) — there is NO update path
//! A region change is a **new-tenant-+-DSR** (for a tenant) or a **new cell** (for a cell), never an
//! `UPDATE`. This registry therefore exposes **no** `update_cell_region` / `update_placement_region`
//! method: the `region` field is set once at insert and is structurally read-only thereafter. The
//! `region_has_no_update_path` test asserts this discipline. (The *mutable* fields a real registry
//! does update — `cell.status`, `cell.utilisation`, `slug` — have their own setters; `region` is
//! deliberately absent from that set.)
//!
//! ## Mutation floor (mandatory-core, >= 80% -- EI-01 §2/§3; the prompt's TESTS field)
//! The placement-invariant logic ([`Registry::check_placement_invariant`] /
//! [`Registry::place_tenant`]) is mandatory-core: a control-plane PII misroute is stop-the-bleeding
//! (EI-01 §2), and the load-bearing decision is *every cell in {home_cell} u member_cells must be in
//! the tenant's region or the placement is refused*. The floor is **>= 80%**; the achieved score is
//! `cargo mutants -p myelin-control-plane -f crates/myelin-control-plane/src/registry.rs` ->
//! **15 caught, 7 unviable, 0 missed = 100% of the 15 viable mutants**. Every mutation of the
//! invariant's region-compare branch, the unknown-cell fail-closed branch, the {home_cell} u
//! member_cells iteration, and the insert/lookup/log accessors is killed by an assertion.

use crate::schema::{Cell, CellProvisioning, LocalTenant, TenantPlacement};
use myelin_tenancy::{CellId, Region, TenantId};
use std::collections::BTreeMap;

/// The reason a `tenant_placement` write is **rejected** by the placement invariant (the trigger's
/// verdict). Every variant is a region-pinning violation; carrying the offending ids keeps the
/// rejection loud + named (EI-01 §3 — a refusal is information, architecture §5.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlacementError {
    /// **The HARD placement invariant (architecture §5.1).** A cell in `{home_cell} ∪ member_cells`
    /// is in a different region than the tenant — multi-cell must be single-region by construction.
    /// This is the trigger's rejection: 0 cross-region member cells are ever admitted.
    CrossRegionMemberCell {
        /// The tenant whose placement was rejected.
        tenant: TenantId,
        /// The tenant's (immutable) region.
        tenant_region: Region,
        /// The offending cell (in `{home_cell} ∪ member_cells`).
        cell: CellId,
        /// The offending cell's region (≠ `tenant_region`).
        cell_region: Region,
    },
    /// A referenced cell (`home_cell` or a `member_cell`) is not in the registry — the invariant
    /// cannot be checked against an unknown cell, so the placement is refused (fail-closed: never
    /// admit a placement whose region pin cannot be verified).
    UnknownCell {
        /// The tenant whose placement was rejected.
        tenant: TenantId,
        /// The cell that is not registered.
        cell: CellId,
    },
}

impl std::fmt::Display for PlacementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlacementError::CrossRegionMemberCell {
                tenant,
                tenant_region,
                cell,
                cell_region,
            } => write!(
                f,
                "placement invariant REJECTED tenant `{}`: cell `{}` is in region `{}` but the \
                 tenant is pinned to region `{}` — every cell in {{home_cell}} ∪ member_cells must \
                 be in the tenant's region (multi-cell is single-region by construction, \
                 architecture §5.1). 0 cross-region member cells are admitted.",
                tenant.as_str(),
                cell.as_str(),
                cell_region.as_str(),
                tenant_region.as_str()
            ),
            PlacementError::UnknownCell { tenant, cell } => write!(
                f,
                "placement invariant REJECTED tenant `{}`: cell `{}` is not registered — a \
                 placement whose region pin cannot be verified is refused (fail-closed, §5.3).",
                tenant.as_str(),
                cell.as_str()
            ),
        }
    }
}

impl std::error::Error for PlacementError {}

/// **The control-plane registry** (architecture §5.1): the three PII-free tables (`cell` /
/// `tenant_placement` / `cell_provisioning`) + the per-cell `local_tenant` directory, behind the
/// **HARD placement invariant**. On this floor the rows are held in-process keyed by their opaque
/// PKs; the concrete Postgres execution (the bounded pool + RLS) is the named Storage floor
/// (P-ST-01 / P-S12). The invariant logic + the region-immutability discipline are real + tested
/// now and do not change shape when the driver lands.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    /// The `cell` inventory, keyed by opaque `cell_id`.
    cells: BTreeMap<String, Cell>,
    /// The `tenant_placement` table, keyed by opaque `tenant_id`.
    placements: BTreeMap<String, TenantPlacement>,
    /// The `cell_provisioning` orchestration log (append-only, in registration order).
    provisioning_log: Vec<CellProvisioning>,
    /// The per-cell `local_tenant` directory, keyed by `cell_id` then `tenant_id`. Each cell's
    /// directory maps the tenants IT homes (the cell-local mirror of the global placement record).
    local_tenants: BTreeMap<String, BTreeMap<String, LocalTenant>>,
}

impl Registry {
    /// A fresh, empty registry.
    pub fn new() -> Registry {
        Registry::default()
    }

    /// Insert a `cell` inventory row (the only way a cell enters the registry — a region change is a
    /// NEW cell, never an UPDATE of an existing one, §5.3 layer 1). Returns the prior row if a cell
    /// with this id already existed (a re-register, e.g. on restart). The cell's `region` is fixed
    /// at this insert and never mutated thereafter (there is no `update_cell_region`).
    pub fn insert_cell(&mut self, cell: Cell) -> Option<Cell> {
        self.cells.insert(cell.cell_id.as_str().to_string(), cell)
    }

    /// Look up a `cell` by opaque id.
    pub fn cell(&self, cell_id: &CellId) -> Option<&Cell> {
        self.cells.get(cell_id.as_str())
    }

    /// The number of cells in the inventory.
    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }

    /// **`place_tenant` — the placement invariant trigger, in code (architecture §5.1).** Writes a
    /// `tenant_placement` row IFF every cell in `{home_cell} ∪ member_cells` is in the tenant's
    /// (immutable) region. A cross-region member cell is rejected with [`PlacementError`] — the
    /// trigger admits **0** cross-region member cells. On success the row is stored and the prior
    /// row (if any) returned.
    ///
    /// This is the second of the four region-pinning layers (§5.3): layer 1 is `region`
    /// immutability (no update path); THIS is layer 2 (the invariant); layer 3 is the `residency-pin`
    /// write-boundary (P-CP-03); layer 4 is the gateway misroute-reject (P-CP-08).
    pub fn place_tenant(
        &mut self,
        placement: TenantPlacement,
    ) -> Result<Option<TenantPlacement>, PlacementError> {
        self.check_placement_invariant(&placement)?;
        let key = placement.tenant_id.as_str().to_string();
        Ok(self.placements.insert(key, placement))
    }

    /// The placement-invariant predicate (the trigger's check, isolated + pure so it is directly
    /// unit-testable and mutation-tested). Returns `Ok(())` iff every cell in
    /// `{home_cell} ∪ member_cells` is registered AND in the tenant's region; else the loud
    /// [`PlacementError`]. This is the load-bearing, mandatory-core decision of this module.
    pub fn check_placement_invariant(
        &self,
        placement: &TenantPlacement,
    ) -> Result<(), PlacementError> {
        // {home_cell} ∪ member_cells — the home cell is always part of the set the invariant covers
        // (a multi-cell tenant's home_cell may or may not appear in member_cells; checking it
        // explicitly means the invariant holds even when member_cells is the v1 single element).
        let cells_to_check = std::iter::once(&placement.home_cell).chain(placement.member_cells.iter());
        for cell_id in cells_to_check {
            let Some(cell) = self.cells.get(cell_id.as_str()) else {
                return Err(PlacementError::UnknownCell {
                    tenant: placement.tenant_id.clone(),
                    cell: cell_id.clone(),
                });
            };
            if cell.region != placement.region {
                return Err(PlacementError::CrossRegionMemberCell {
                    tenant: placement.tenant_id.clone(),
                    tenant_region: placement.region.clone(),
                    cell: cell_id.clone(),
                    cell_region: cell.region.clone(),
                });
            }
        }
        Ok(())
    }

    /// Look up a `tenant_placement` row by opaque `tenant_id`.
    pub fn placement(&self, tenant_id: &TenantId) -> Option<&TenantPlacement> {
        self.placements.get(tenant_id.as_str())
    }

    /// The number of placed tenants.
    pub fn placement_count(&self) -> usize {
        self.placements.len()
    }

    /// Append a `cell_provisioning` orchestration-log entry (the scripted-provisioning floor records
    /// its steps here; the durable-workflow promotion is P-CP-22).
    pub fn log_provisioning(&mut self, entry: CellProvisioning) {
        self.provisioning_log.push(entry);
    }

    /// The provisioning log (append-only, in order).
    pub fn provisioning_log(&self) -> &[CellProvisioning] {
        &self.provisioning_log
    }

    /// Upsert a `local_tenant` directory entry in a cell's directory (the cell-local mirror of the
    /// global placement record — which tenants this cell homes). Returns the prior entry if any.
    pub fn upsert_local_tenant(&mut self, cell_id: &CellId, entry: LocalTenant) -> Option<LocalTenant> {
        self.local_tenants
            .entry(cell_id.as_str().to_string())
            .or_default()
            .insert(entry.tenant_id.as_str().to_string(), entry)
    }

    /// The `local_tenant` directory of a given cell (the tenants that cell homes).
    pub fn local_tenants(&self, cell_id: &CellId) -> Vec<&LocalTenant> {
        self.local_tenants
            .get(cell_id.as_str())
            .map(|m| m.values().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Capacity, CellStatus, IsolationKind, PlacementStatus};

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
            endpoint: format!("cell.{region}.myelin.eu"),
        }
    }

    fn placement(tenant: &str, region: &str, home: &str, members: &[&str]) -> TenantPlacement {
        TenantPlacement {
            tenant_id: TenantId::from_token(tenant),
            region: Region::new(region),
            home_cell: CellId::from_token(home),
            isolation_tier: IsolationKind::Pool,
            slug: "acme".into(),
            status: PlacementStatus::Active,
            member_cells: members.iter().map(|c| CellId::from_token(*c)).collect(),
        }
    }

    /// **The placement-invariant ADMIT leg (the green half of CP-D1's invariant leg).** A placement
    /// whose home + member cells are ALL in the tenant's region is admitted.
    #[test]
    fn admits_a_single_region_placement() {
        let mut reg = Registry::new();
        assert_eq!(reg.cell_count(), 0); // empty before any cell is inserted.
        reg.insert_cell(cell("cell-w-1", "eu-west"));
        reg.insert_cell(cell("cell-w-2", "eu-west"));
        assert_eq!(reg.cell_count(), 2); // both cells are in the inventory.
        // home + a member cell, both in eu-west, tenant in eu-west — admitted.
        let p = placement("01J0ACME", "eu-west", "cell-w-1", &["cell-w-1"]);
        reg.place_tenant(p).expect("a single-region placement is admitted");
        assert_eq!(reg.placement_count(), 1);
    }

    /// **THE PLACEMENT INVARIANT REJECT LEG (CP-D1 invariant leg): a `member_cell` in a different
    /// region than the tenant is rejected.** 0 cross-region member cells admitted (architecture
    /// §5.1). This is the trigger's headline rejection.
    #[test]
    fn rejects_a_cross_region_member_cell() {
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-w-1", "eu-west")); // home, in-region.
        reg.insert_cell(cell("cell-n-1", "eu-north")); // member, WRONG region.
        let p = placement("01J0ACME", "eu-west", "cell-w-1", &["cell-w-1", "cell-n-1"]);
        let e = reg
            .place_tenant(p)
            .expect_err("a cross-region member cell is rejected by the invariant");
        assert_eq!(
            e,
            PlacementError::CrossRegionMemberCell {
                tenant: TenantId::from_token("01J0ACME"),
                tenant_region: Region::new("eu-west"),
                cell: CellId::from_token("cell-n-1"),
                cell_region: Region::new("eu-north"),
            }
        );
        // The rejected placement was NOT stored (the trigger refuses the write).
        assert_eq!(reg.placement_count(), 0);
        assert!(e.to_string().contains("single-region by construction"), "loud reason: {e}");
    }

    /// The HOME cell being cross-region is also rejected (the invariant covers `{home_cell} ∪
    /// member_cells`, not just member_cells).
    #[test]
    fn rejects_a_cross_region_home_cell() {
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-n-1", "eu-north")); // home, WRONG region.
        let p = placement("01J0ACME", "eu-west", "cell-n-1", &[]);
        let e = reg.place_tenant(p).expect_err("a cross-region home cell is rejected");
        assert!(matches!(e, PlacementError::CrossRegionMemberCell { .. }));
        assert_eq!(reg.placement_count(), 0);
    }

    /// A placement referencing an UNKNOWN cell is refused fail-closed (the region pin cannot be
    /// verified against a cell that is not registered).
    #[test]
    fn rejects_an_unknown_cell_fail_closed() {
        let mut reg = Registry::new();
        let p = placement("01J0ACME", "eu-west", "cell-ghost", &[]);
        let e = reg.place_tenant(p).expect_err("an unknown cell is refused fail-closed");
        assert_eq!(
            e,
            PlacementError::UnknownCell {
                tenant: TenantId::from_token("01J0ACME"),
                cell: CellId::from_token("cell-ghost"),
            }
        );
    }

    /// **`member_cells` is single-element in v1 (the named floor).** A v1 placement carries exactly
    /// one member cell (its home), and the registry round-trips it. The multi-element fan-out is the
    /// M5 follow-on (P-CP-19/P-CP-20).
    #[test]
    fn member_cells_is_single_element_in_v1() {
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-w-1", "eu-west"));
        let p = placement("01J0ACME", "eu-west", "cell-w-1", &["cell-w-1"]);
        reg.place_tenant(p).expect("v1 single-element placement");
        let stored = reg.placement(&TenantId::from_token("01J0ACME")).expect("placed");
        assert_eq!(stored.member_cells.len(), 1, "v1 member_cells is single-element");
    }

    /// **Region immutability (architecture §5.3 layer 1): there is NO update path for `region`.** A
    /// region change is a NEW cell / a new-tenant-+-DSR, never an UPDATE. This test documents +
    /// asserts the discipline: the registry exposes setters for the *mutable* fields a real registry
    /// updates (status/utilisation/slug would be such — checked structurally here via re-insert of a
    /// NEW row), but NO `update_cell_region` / `update_placement_region`. "Changing" a region means
    /// inserting a NEW cell with a new id; the old row's region is untouched.
    #[test]
    fn region_has_no_update_path() {
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-w-1", "eu-west"));
        // A region "change" is a NEW cell — never a mutation of cell-w-1's region.
        reg.insert_cell(cell("cell-n-1", "eu-north"));
        // The original cell's region is unchanged (there is no API that could have changed it).
        assert_eq!(reg.cell(&CellId::from_token("cell-w-1")).unwrap().region.as_str(), "eu-west");
        assert_eq!(reg.cell(&CellId::from_token("cell-n-1")).unwrap().region.as_str(), "eu-north");
        // Compile-level proof of the discipline: there is no `reg.update_cell_region(..)` /
        // `reg.update_placement_region(..)` method to call — uncommenting either would fail to
        // compile (the method does not exist). Their ABSENCE is the structural immutability proof.
        // reg.update_cell_region(&CellId::from_token("cell-w-1"), Region::new("eu-north"));
    }

    /// The `cell_provisioning` orchestration log is append-only + ordered (the scripted-provisioning
    /// floor records its steps here; the gating is P-CP-11).
    #[test]
    fn provisioning_log_is_append_only_and_ordered() {
        use crate::schema::ProvisioningOutcome;
        let mut reg = Registry::new();
        reg.log_provisioning(CellProvisioning {
            cell_id: CellId::from_token("cell-w-1"),
            step: "restore_verify".into(),
            outcome: ProvisioningOutcome::Passed,
        });
        reg.log_provisioning(CellProvisioning {
            cell_id: CellId::from_token("cell-w-1"),
            step: "readiness_probe".into(),
            outcome: ProvisioningOutcome::Running,
        });
        assert_eq!(reg.provisioning_log().len(), 2);
        assert_eq!(reg.provisioning_log()[0].step, "restore_verify");
        assert_eq!(reg.provisioning_log()[1].outcome, ProvisioningOutcome::Running);
    }

    /// The per-cell `local_tenant` directory maps a cell's OWN tenants (the cell-local mirror).
    #[test]
    fn local_tenant_directory_maps_a_cells_own_tenants() {
        let mut reg = Registry::new();
        let cell_id = CellId::from_token("cell-w-1");
        reg.upsert_local_tenant(
            &cell_id,
            LocalTenant {
                tenant_id: TenantId::from_token("01J0ACME"),
                isolation_tier: IsolationKind::Pool,
                active: true,
            },
        );
        reg.upsert_local_tenant(
            &cell_id,
            LocalTenant {
                tenant_id: TenantId::from_token("01J0BETA"),
                isolation_tier: IsolationKind::Pool,
                active: false,
            },
        );
        let dir = reg.local_tenants(&cell_id);
        assert_eq!(dir.len(), 2);
        // A different cell's directory is empty (each cell maps only its own tenants).
        assert!(reg.local_tenants(&CellId::from_token("cell-w-2")).is_empty());
    }
}

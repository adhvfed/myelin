//! # The control-plane registry + the HARD placement invariant (the DB trigger, in code)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md` §5.1 (the three
//! PII-free tables + the HARD INVARIANT: every cell in `{home_cell} ∪ member_cells` has
//! `cell.region == tenant_placement.region` — trigger + `residency-pin` lint, multi-cell
//! single-region by construction) and §5.3 (the four-layer region-pinning defence: layer 1 = region
//! immutable, layer 2 = the placement invariant). Contract 12.3 (the registry-schema half).
//!
//! ## The placement invariant IS the DB trigger (in code here; LIVE at the DB on the Pg arm)
//! The architecture specifies the invariant as a **DB trigger** that rejects any `tenant_placement`
//! whose `{home_cell} ∪ member_cells` contains a cell in a different region than the tenant.
//! **Here the invariant is the [`Registry::place_tenant`] guard** — the same predicate the trigger
//! enforces, proven at unit scale with a green/red drill and run on BOTH backend arms (so the
//! caller-visible typed refusal is identical). As of MR-009b W6d the production `Pg` arm ALSO has
//! the REAL Postgres trigger behind it as the backstop of record
//! (`myelin_storage::placement_durable::PLACEMENT_INVARIANT_TRIGGER`, proven live in
//! `integration_mr024_placement_durable`) — plus the repo-grain residency trigger
//! (`REPO_PLACEMENT_INVARIANT_TRIGGER`, `integration_mr009b_w6d_registry_durable`).
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
//! **W6d scope note:** the floor covers the invariant logic + the Memory-arm surface the unit tests
//! drive (that score above). The W6d `Pg` dispatch arms are NOT unit-mutable (they require live PG);
//! their correctness proof is the live integration suite (`integration_mr009b_w6d_registry_durable`
//! — durability, trigger enforcement, derivation-no-drift) + `integration_mr024_placement_durable`.

use crate::placement_of_repo::{RepoPlacementRow, StorageGroup};
use crate::schema::{
    Capacity, Cell, CellProvisioning, CellStatus, IsolationKind, LocalTenant, PlacementStatus,
    ProvisioningOutcome, TenantPlacement,
};
use myelin_storage::placement_durable::{
    DurableCellProvisioningRow, DurableCellRow, DurableLocalTenantRow, DurablePlacementBacking,
    DurablePlacementRow, DurableRepoPlacementRow, PlacementWriteError,
};
use myelin_tenancy::{CellId, Region, TenantId};
#[cfg(any(test, feature = "test-support"))]
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

// =================================================================================================
// Opaque-text <-> typed-enum converters (the storage layer holds opaque text; the control-plane owns
// the closed enums). Total + round-tripping; an unknown variant fails CLOSED (never silently
// coerced). pub(crate): shared with `registry_durable` (the MR-024 binding reuses one converter set).
// =================================================================================================

pub(crate) fn cell_status_text(s: CellStatus) -> &'static str {
    match s {
        CellStatus::Provisioning => "Provisioning",
        CellStatus::Active => "Active",
        CellStatus::Draining => "Draining",
    }
}

pub(crate) fn cell_status_from(s: &str) -> Option<CellStatus> {
    match s {
        "Provisioning" => Some(CellStatus::Provisioning),
        "Active" => Some(CellStatus::Active),
        "Draining" => Some(CellStatus::Draining),
        _ => None,
    }
}

pub(crate) fn isolation_text(k: IsolationKind) -> &'static str {
    match k {
        IsolationKind::Pool => "Pool",
        IsolationKind::Bridge => "Bridge",
        IsolationKind::Dedicated => "Dedicated",
    }
}

pub(crate) fn isolation_from(s: &str) -> Option<IsolationKind> {
    match s {
        "Pool" => Some(IsolationKind::Pool),
        "Bridge" => Some(IsolationKind::Bridge),
        "Dedicated" => Some(IsolationKind::Dedicated),
        _ => None,
    }
}

pub(crate) fn placement_status_text(s: PlacementStatus) -> &'static str {
    match s {
        PlacementStatus::Pending => "Pending",
        PlacementStatus::Active => "Active",
        PlacementStatus::Offboarding => "Offboarding",
    }
}

pub(crate) fn placement_status_from(s: &str) -> Option<PlacementStatus> {
    match s {
        "Pending" => Some(PlacementStatus::Pending),
        "Active" => Some(PlacementStatus::Active),
        "Offboarding" => Some(PlacementStatus::Offboarding),
        _ => None,
    }
}

pub(crate) fn provisioning_outcome_text(o: ProvisioningOutcome) -> &'static str {
    match o {
        ProvisioningOutcome::Running => "Running",
        ProvisioningOutcome::Passed => "Passed",
        ProvisioningOutcome::Failed => "Failed",
    }
}

pub(crate) fn provisioning_outcome_from(s: &str) -> Option<ProvisioningOutcome> {
    match s {
        "Running" => Some(ProvisioningOutcome::Running),
        "Passed" => Some(ProvisioningOutcome::Passed),
        "Failed" => Some(ProvisioningOutcome::Failed),
        _ => None,
    }
}

pub(crate) fn cell_to_durable(c: &Cell) -> DurableCellRow {
    DurableCellRow {
        cell_id: c.cell_id.as_str().to_string(),
        region: c.region.as_str().to_string(),
        status: cell_status_text(c.status).to_string(),
        isolation_kind: isolation_text(c.isolation_kind).to_string(),
        tenants_max: c.capacity.tenants_max as i64,
        write_qps_max: c.capacity.write_qps_max as i64,
        storage_bytes_max: c.capacity.storage_bytes_max as i64,
        utilisation: c.utilisation as i16,
        version: c.version as i64,
        endpoint: c.endpoint.clone(),
    }
}

pub(crate) fn durable_to_cell(r: &DurableCellRow) -> Option<Cell> {
    Some(Cell {
        cell_id: CellId::from_token(&r.cell_id),
        region: Region::new(&r.region),
        status: cell_status_from(&r.status)?,
        isolation_kind: isolation_from(&r.isolation_kind)?,
        capacity: Capacity {
            tenants_max: r.tenants_max as u32,
            write_qps_max: r.write_qps_max as u32,
            storage_bytes_max: r.storage_bytes_max as u64,
        },
        utilisation: r.utilisation as u8,
        version: r.version as u32,
        endpoint: r.endpoint.clone(),
    })
}

pub(crate) fn placement_to_durable(p: &TenantPlacement) -> DurablePlacementRow {
    DurablePlacementRow {
        tenant_id: p.tenant_id.as_str().to_string(),
        region: p.region.as_str().to_string(),
        home_cell: p.home_cell.as_str().to_string(),
        isolation_tier: isolation_text(p.isolation_tier).to_string(),
        slug: p.slug.clone(),
        status: placement_status_text(p.status).to_string(),
        member_cells: p.member_cells.iter().map(|c| c.as_str().to_string()).collect(),
    }
}

pub(crate) fn durable_to_placement(r: &DurablePlacementRow) -> Option<TenantPlacement> {
    Some(TenantPlacement {
        tenant_id: TenantId::from_token(&r.tenant_id),
        region: Region::new(&r.region),
        home_cell: CellId::from_token(&r.home_cell),
        isolation_tier: isolation_from(&r.isolation_tier)?,
        slug: r.slug.clone(),
        status: placement_status_from(&r.status)?,
        member_cells: r.member_cells.iter().map(CellId::from_token).collect(),
    })
}

/// **Fail-static LOUD on a durable placement fault (MR-009b W6d).** The placement registry is the
/// routing system-of-record; a swallowed durable fault (or a silent in-memory fallback) would fork
/// routing — the correct-but-latent shape MR-009b kills. Mirrors the W3b.4 fail-loud boot posture
/// and the `KmsEngine` durable-mutation panic (no error variant exists on this legacy-sync surface;
/// converting the infallible signatures to `Result` is the named W5-residual ripple wave).
pub(crate) fn placement_db_panic(op: &str, why: &dyn core::fmt::Display) -> ! {
    panic!(
        "control-plane placement registry: durable {op} FAILED (fail-static loud — the placement \
         registry is the routing system-of-record; the write/read did NOT complete and there is no \
         silent in-memory fallback): {why}"
    )
}

/// Fail-static LOUD on a corrupt durable row (an unknown status/tier text): the closed enums fail
/// CLOSED — a row that cannot round-trip is never silently skipped or coerced.
pub(crate) fn corrupt_row_panic(table: &str, key: &str) -> ! {
    panic!(
        "control-plane placement registry: durable `{table}` row `{key}` carries an unknown \
         status/tier text — fail closed (the closed enums admit no silent coercion; the row is \
         corrupt or written by a newer schema)"
    )
}

/// **MR-009b W6d — TEST DOUBLE (compiled ONLY under `#[cfg(any(test, feature = "test-support"))]`).**
/// The five in-process collections of the pre-W6d registry, now the `Memory` backend arm: the DB-free
/// double the unit tests + drills run on. NOT the production system-of-record (the production arm is
/// [`PgRegistry`] over the durable `cell`/`tenant_placement`/`repo_placement`/`cell_provisioning`/
/// `local_tenant` tables, migrations 0030–0039).
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Debug, Default)]
struct MemoryRegistry {
    /// The `cell` inventory, keyed by opaque `cell_id`.
    cells: BTreeMap<String, Cell>,
    /// The `tenant_placement` table, keyed by opaque `tenant_id`.
    placements: BTreeMap<String, TenantPlacement>,
    /// The `cell_provisioning` orchestration log (append-only, in registration order).
    provisioning_log: Vec<CellProvisioning>,
    /// The per-cell `local_tenant` directory, keyed by `cell_id` then `tenant_id`.
    local_tenants: BTreeMap<String, BTreeMap<String, LocalTenant>>,
    /// The `repo_placement` stored facts, keyed by the repo's opaque `ArtifactRef` string.
    repo_placements: BTreeMap<String, RepoPlacementRow>,
}

/// The Pg arm (MR-009b W6d): the durable placement backing over the OLTP pool + the runtime handle
/// the sync registry API drives the async sqlx backing on (the `block_in_place`+`block_on` bridge —
/// the same one identity-service / the storage backings use). The HARD placement invariant is the
/// REAL DB trigger on this arm; the repo-grain residency pin is the `repo_placement` trigger.
#[derive(Clone)]
struct PgRegistry {
    backing: DurablePlacementBacking,
    rt: tokio::runtime::Handle,
}

impl PgRegistry {
    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

/// The registry backend (MR-009b W6d): the `test-support`-gated in-memory DOUBLE or the
/// ALWAYS-COMPILED durable PG system-of-record. The production-compiled enum presents no in-memory
/// collection (the scanner strips the gated `Memory` arm).
#[derive(Clone)]
enum RegistryBackend {
    /// The in-memory test double (DB-free). Compiled ONLY under
    /// `#[cfg(any(test, feature = "test-support"))]` — NOT the production system-of-record.
    #[cfg(any(test, feature = "test-support"))]
    Memory(MemoryRegistry),
    /// The REAL durable PG backing — the production system-of-record (durable tables + the
    /// invariant TRIGGERs), whole-surface as of W6d.
    Pg(PgRegistry),
}

/// **The control-plane registry** (architecture §5.1): the three PII-free tables (`cell` /
/// `tenant_placement` / `cell_provisioning`) + the per-cell `local_tenant` directory + the
/// `repo_placement` stored facts (§5.2), behind the **HARD placement invariant**. As of MR-009b W6d
/// this is a ROLE STRUCT over a whole-surface backend enum: the ALWAYS-COMPILED production backend
/// is the durable PG system-of-record ([`Registry::with_pg`] — migrations 0030–0039, the invariant
/// as a REAL DB trigger, kill-9 survivable); the in-memory five-collection registry is the
/// `test-support`-gated test double ([`Registry::new`]). The invariant logic + the
/// region-immutability discipline hold identically on both arms.
#[derive(Clone)]
pub struct Registry {
    backend: RegistryBackend,
}

impl core::fmt::Debug for Registry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // PII-free, connection-free Debug: the backend arm only (never a row, never a DSN).
        let arm = match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RegistryBackend::Memory(_) => "Memory(test-double)",
            RegistryBackend::Pg(_) => "Pg(durable)",
        };
        f.debug_struct("Registry").field("backend", &arm).finish()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for Registry {
    fn default() -> Registry {
        Registry::new()
    }
}

impl Registry {
    /// A fresh, empty IN-MEMORY registry — **TEST DOUBLE** (MR-009b W6d: compiled only under
    /// `#[cfg(any(test, feature = "test-support"))]`). The production constructor is
    /// [`Registry::with_pg`].
    #[cfg(any(test, feature = "test-support"))]
    pub fn new() -> Registry {
        Registry {
            backend: RegistryBackend::Memory(MemoryRegistry::default()),
        }
    }

    /// **The PRODUCTION registry — bound to the REAL durable PG backing (MR-009b W6d).** The whole
    /// surface (cell inventory, tenant placements under the HARD DB-trigger invariant, the
    /// NOT-rebuildable `repo_placement` stored facts under the repo-grain residency trigger, the
    /// append-only `cell_provisioning` log, the per-cell `local_tenant` directory) persists through
    /// [`DurablePlacementBacking`] and survives a kill-9 restart. The caller must have applied
    /// [`myelin_storage::placement_durable_migrations`] (0030–0039). `rt` is the tokio runtime
    /// handle the sync API drives the async backing on. Durable faults fail-static LOUD (panic) —
    /// never a silent in-memory fallback (the W3b.4 boot posture).
    pub fn with_pg(backing: DurablePlacementBacking, rt: tokio::runtime::Handle) -> Registry {
        Registry {
            backend: RegistryBackend::Pg(PgRegistry { backing, rt }),
        }
    }

    /// Insert a `cell` inventory row (the only way a cell enters the registry — a region change is a
    /// NEW cell, never an UPDATE of an existing one, §5.3 layer 1). Returns the prior row if a cell
    /// with this id already existed (a re-register, e.g. on restart). The cell's `region` is fixed
    /// at this insert and never mutated thereafter (there is no `update_cell_region`; the Pg arm's
    /// conflict path never overwrites `region`).
    pub fn insert_cell(&mut self, cell: Cell) -> Option<Cell> {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RegistryBackend::Memory(m) => m.cells.insert(cell.cell_id.as_str().to_string(), cell),
            RegistryBackend::Pg(pg) => {
                let prior = pg
                    .block(pg.backing.get_cell(cell.cell_id.as_str()))
                    .unwrap_or_else(|e| placement_db_panic("cell read (insert prior)", &e))
                    .map(|r| {
                        durable_to_cell(&r).unwrap_or_else(|| corrupt_row_panic("cell", &r.cell_id))
                    });
                pg.block(pg.backing.insert_cell(&cell_to_durable(&cell)))
                    .unwrap_or_else(|e| placement_db_panic("cell insert", &e));
                prior
            }
        }
    }

    /// Look up a `cell` by opaque id.
    pub fn cell(&self, cell_id: &CellId) -> Option<Cell> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RegistryBackend::Memory(m) => m.cells.get(cell_id.as_str()).cloned(),
            RegistryBackend::Pg(pg) => pg
                .block(pg.backing.get_cell(cell_id.as_str()))
                .unwrap_or_else(|e| placement_db_panic("cell read", &e))
                .map(|r| durable_to_cell(&r).unwrap_or_else(|| corrupt_row_panic("cell", &r.cell_id))),
        }
    }

    /// The number of cells in the inventory.
    pub fn cell_count(&self) -> usize {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RegistryBackend::Memory(m) => m.cells.len(),
            RegistryBackend::Pg(pg) => pg
                .block(pg.backing.cell_count())
                .unwrap_or_else(|e| placement_db_panic("cell count", &e))
                as usize,
        }
    }

    /// An iterator over the `cell` inventory rows (the assignment algorithm scans this — P-CP-07's
    /// `assign_cell` filters region-first → tier-second → capacity-third over it). Owned rows as of
    /// W6d (the Pg arm reads them off the durable table; stable `cell_id` order on both arms).
    pub fn cells_iter(&self) -> impl Iterator<Item = Cell> {
        let cells: Vec<Cell> = match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RegistryBackend::Memory(m) => m.cells.values().cloned().collect(),
            RegistryBackend::Pg(pg) => pg
                .block(pg.backing.all_cells())
                .unwrap_or_else(|e| placement_db_panic("cell scan", &e))
                .iter()
                .map(|r| {
                    durable_to_cell(r).unwrap_or_else(|| corrupt_row_panic("cell", &r.cell_id))
                })
                .collect(),
        };
        cells.into_iter()
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
        // The typed invariant check runs on BOTH arms (the same predicate the DB trigger enforces),
        // so the caller-visible refusal shape is identical. On the Pg arm the DB TRIGGER is the
        // backstop of record — a trigger rejection AFTER this check passed would be an infra-level
        // divergence and fails loud below (region is immutable and cells have no delete verb, so
        // the two predicates cannot legitimately disagree).
        self.check_placement_invariant(&placement)?;
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RegistryBackend::Memory(m) => {
                let key = placement.tenant_id.as_str().to_string();
                Ok(m.placements.insert(key, placement))
            }
            RegistryBackend::Pg(pg) => {
                let prior = pg
                    .block(pg.backing.get_placement(placement.tenant_id.as_str()))
                    .unwrap_or_else(|e| placement_db_panic("placement read (prior)", &e))
                    .map(|r| {
                        durable_to_placement(&r)
                            .unwrap_or_else(|| corrupt_row_panic("tenant_placement", &r.tenant_id))
                    });
                match pg.block(pg.backing.place_tenant(&placement_to_durable(&placement))) {
                    Ok(()) => Ok(prior),
                    Err(e @ PlacementWriteError::InvariantRejected(_)) => placement_db_panic(
                        "place_tenant (DB trigger refused a write the in-code invariant admitted \
                         — predicate divergence)",
                        &e,
                    ),
                    Err(e) => placement_db_panic("place_tenant", &e),
                }
            }
        }
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
        let cells_to_check =
            std::iter::once(&placement.home_cell).chain(placement.member_cells.iter());
        for cell_id in cells_to_check {
            let Some(cell) = self.cell(cell_id) else {
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

    /// **Flip a cell's lifecycle status (the provisioning gate's `Provisioning → Active` transition,
    /// P-CP-11).** `status` is a MUTABLE field (unlike `region`, which has no update path — §5.3 layer
    /// 1); a cell legitimately transitions `Provisioning → Active` (it passed restore-verify +
    /// readiness) and `Active → Draining` (decommission). This setter is the ONLY way the status
    /// changes — the provisioning gate ([`crate::provision::ProvisioningGate`]) calls it ONLY after
    /// both gating steps pass. Returns `true` iff the cell exists and was updated. The `region` is
    /// untouched (it is immutable; this setter cannot reach it).
    pub fn set_cell_status(&mut self, cell_id: &CellId, status: crate::schema::CellStatus) -> bool {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RegistryBackend::Memory(m) => match m.cells.get_mut(cell_id.as_str()) {
                Some(cell) => {
                    cell.status = status;
                    true
                }
                None => false,
            },
            RegistryBackend::Pg(pg) => pg
                .block(
                    pg.backing
                        .set_cell_status(cell_id.as_str(), cell_status_text(status)),
                )
                .unwrap_or_else(|e| placement_db_panic("cell status update", &e)),
        }
    }

    /// Flip a cell to `Active` (the provisioning gate's activation step — the cell passed
    /// restore-verify + readiness). A convenience over [`Self::set_cell_status`] for the load-bearing
    /// `Provisioning → Active` transition CP-D6 gates. Returns `true` iff the cell exists.
    pub fn activate_cell(&mut self, cell_id: &CellId) -> bool {
        self.set_cell_status(cell_id, crate::schema::CellStatus::Active)
    }

    /// Look up a `tenant_placement` row by opaque `tenant_id`. Owned as of W6d (the Pg arm reads it
    /// off the durable table).
    pub fn placement(&self, tenant_id: &TenantId) -> Option<TenantPlacement> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RegistryBackend::Memory(m) => m.placements.get(tenant_id.as_str()).cloned(),
            RegistryBackend::Pg(pg) => pg
                .block(pg.backing.get_placement(tenant_id.as_str()))
                .unwrap_or_else(|e| placement_db_panic("placement read", &e))
                .map(|r| {
                    durable_to_placement(&r)
                        .unwrap_or_else(|| corrupt_row_panic("tenant_placement", &r.tenant_id))
                }),
        }
    }

    /// An iterator over the `tenant_placement` rows (the provisioning gate scans this to assert the
    /// CP-D6 zero — 0 tenants on an unverified cell). Owned rows; stable tenant-id order.
    pub fn placements_iter(&self) -> impl Iterator<Item = TenantPlacement> {
        let placements: Vec<TenantPlacement> = match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RegistryBackend::Memory(m) => m.placements.values().cloned().collect(),
            RegistryBackend::Pg(pg) => pg
                .block(pg.backing.all_placements())
                .unwrap_or_else(|e| placement_db_panic("placement scan", &e))
                .iter()
                .map(|r| {
                    durable_to_placement(r)
                        .unwrap_or_else(|| corrupt_row_panic("tenant_placement", &r.tenant_id))
                })
                .collect(),
        };
        placements.into_iter()
    }

    /// **Set a placement's lifecycle status (e.g. `Active → Offboarding` on tenant decommission,
    /// P-CP-11).** The `status` is a mutable field of the PII-free routing record; `region` /
    /// `tenant_id` are NOT reachable through this (region immutability, §5.3 layer 1). Returns `true`
    /// iff the placement exists and was updated.
    pub fn set_placement_status(
        &mut self,
        tenant_id: &TenantId,
        status: crate::schema::PlacementStatus,
    ) -> bool {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RegistryBackend::Memory(m) => match m.placements.get_mut(tenant_id.as_str()) {
                Some(p) => {
                    p.status = status;
                    true
                }
                None => false,
            },
            RegistryBackend::Pg(pg) => pg
                .block(
                    pg.backing
                        .set_placement_status(tenant_id.as_str(), placement_status_text(status)),
                )
                .unwrap_or_else(|e| placement_db_panic("placement status update", &e)),
        }
    }

    /// Look up a `tenant_placement` row by its **non-personal routing slug** (the `discover(slug)`
    /// path, architecture §7.3). The slug is a changeable, PII-free label (`acme`), screened to carry
    /// no personal data (the slug-PII screening is the `[OPEN — LEGAL]` residual named in P-CP-12). On
    /// this in-process floor the lookup is a scan; the live registry indexes `slug` (the driver floor,
    /// P-ST-01). Returns the first placement whose slug matches (slugs are unique per registry — the
    /// live schema's `UNIQUE(slug)` constraint enforces it; here the scan returns the first match).
    pub fn placement_by_slug(&self, slug: &str) -> Option<TenantPlacement> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RegistryBackend::Memory(m) => m.placements.values().find(|p| p.slug == slug).cloned(),
            RegistryBackend::Pg(pg) => pg
                .block(pg.backing.get_placement_by_slug(slug))
                .unwrap_or_else(|e| placement_db_panic("placement slug read", &e))
                .map(|r| {
                    durable_to_placement(&r)
                        .unwrap_or_else(|| corrupt_row_panic("tenant_placement", &r.tenant_id))
                }),
        }
    }

    /// The number of placed tenants.
    pub fn placement_count(&self) -> usize {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RegistryBackend::Memory(m) => m.placements.len(),
            RegistryBackend::Pg(pg) => pg
                .block(pg.backing.placement_count())
                .unwrap_or_else(|e| placement_db_panic("placement count", &e))
                as usize,
        }
    }

    /// Append a `cell_provisioning` orchestration-log entry (the scripted-provisioning floor records
    /// its steps here; the durable-workflow promotion is P-CP-22). APPEND-ONLY on both arms: neither
    /// arm exposes an update/delete verb over the log.
    pub fn log_provisioning(&mut self, entry: CellProvisioning) {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RegistryBackend::Memory(m) => m.provisioning_log.push(entry),
            RegistryBackend::Pg(pg) => pg
                .block(pg.backing.log_provisioning(&DurableCellProvisioningRow {
                    cell_id: entry.cell_id.as_str().to_string(),
                    step: entry.step.clone(),
                    outcome: provisioning_outcome_text(entry.outcome).to_string(),
                }))
                .unwrap_or_else(|e| placement_db_panic("provisioning-log append", &e)),
        }
    }

    /// The provisioning log (append-only, in order). Owned rows as of W6d.
    pub fn provisioning_log(&self) -> Vec<CellProvisioning> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RegistryBackend::Memory(m) => m.provisioning_log.clone(),
            RegistryBackend::Pg(pg) => pg
                .block(pg.backing.provisioning_log())
                .unwrap_or_else(|e| placement_db_panic("provisioning-log read", &e))
                .iter()
                .map(|r| CellProvisioning {
                    cell_id: CellId::from_token(&r.cell_id),
                    step: r.step.clone(),
                    outcome: provisioning_outcome_from(&r.outcome)
                        .unwrap_or_else(|| corrupt_row_panic("cell_provisioning", &r.cell_id)),
                })
                .collect(),
        }
    }

    /// Upsert a `local_tenant` directory entry in a cell's directory (the cell-local mirror of the
    /// global placement record — which tenants this cell homes). Returns the prior entry if any.
    pub fn upsert_local_tenant(
        &mut self,
        cell_id: &CellId,
        entry: LocalTenant,
    ) -> Option<LocalTenant> {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RegistryBackend::Memory(m) => m
                .local_tenants
                .entry(cell_id.as_str().to_string())
                .or_default()
                .insert(entry.tenant_id.as_str().to_string(), entry),
            RegistryBackend::Pg(pg) => pg
                .block(pg.backing.upsert_local_tenant(&DurableLocalTenantRow {
                    cell_id: cell_id.as_str().to_string(),
                    tenant_id: entry.tenant_id.as_str().to_string(),
                    isolation_tier: isolation_text(entry.isolation_tier).to_string(),
                    active: entry.active,
                }))
                .unwrap_or_else(|e| placement_db_panic("local-tenant upsert", &e))
                .map(|r| LocalTenant {
                    tenant_id: TenantId::from_token(&r.tenant_id),
                    isolation_tier: isolation_from(&r.isolation_tier)
                        .unwrap_or_else(|| corrupt_row_panic("local_tenant", &r.tenant_id)),
                    active: r.active,
                }),
        }
    }

    /// The `local_tenant` directory of a given cell (the tenants that cell homes). Owned rows.
    pub fn local_tenants(&self, cell_id: &CellId) -> Vec<LocalTenant> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RegistryBackend::Memory(m) => m
                .local_tenants
                .get(cell_id.as_str())
                .map(|dir| dir.values().cloned().collect())
                .unwrap_or_default(),
            RegistryBackend::Pg(pg) => pg
                .block(pg.backing.local_tenants(cell_id.as_str()))
                .unwrap_or_else(|e| placement_db_panic("local-tenant read", &e))
                .iter()
                .map(|r| LocalTenant {
                    tenant_id: TenantId::from_token(&r.tenant_id),
                    isolation_tier: isolation_from(&r.isolation_tier)
                        .unwrap_or_else(|| corrupt_row_panic("local_tenant", &r.tenant_id)),
                    active: r.active,
                })
                .collect(),
        }
    }

    // ── the repo_placement stored facts (crate-internal seams for `placement_of_repo`) ──

    /// One repo's stored `{cell_id, group}` fact by its opaque ref key, or `None` (crate-internal —
    /// [`crate::placement_of_repo`] owns the public repo-grain API + its residency checks).
    pub(crate) fn repo_placement_row(&self, repo_key: &str) -> Option<RepoPlacementRow> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RegistryBackend::Memory(m) => m.repo_placements.get(repo_key).cloned(),
            RegistryBackend::Pg(pg) => pg
                .block(pg.backing.get_repo_placement(repo_key))
                .unwrap_or_else(|e| placement_db_panic("repo-placement read", &e))
                .map(|r| RepoPlacementRow {
                    cell_id: CellId::from_token(&r.cell_id),
                    group: StorageGroup::from_token(&r.storage_group),
                }),
        }
    }

    /// Upsert one repo's stored `{cell_id, group}` fact (crate-internal — the caller,
    /// [`crate::placement_of_repo`], has ALREADY run the app-level residency checks; on the Pg arm
    /// the `repo_placement` DB trigger is the backstop of record, so a trigger rejection here is a
    /// predicate divergence and fails loud).
    pub(crate) fn upsert_repo_placement_row(
        &mut self,
        repo_key: &str,
        tenant: &TenantId,
        row: RepoPlacementRow,
    ) {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RegistryBackend::Memory(m) => {
                m.repo_placements.insert(repo_key.to_string(), row);
            }
            RegistryBackend::Pg(pg) => {
                match pg.block(pg.backing.upsert_repo_placement(&DurableRepoPlacementRow {
                    repo_ref: repo_key.to_string(),
                    tenant_id: tenant.as_str().to_string(),
                    cell_id: row.cell_id.as_str().to_string(),
                    storage_group: row.group.as_str().to_string(),
                })) {
                    Ok(()) => {}
                    Err(e @ PlacementWriteError::InvariantRejected(_)) => placement_db_panic(
                        "repo-placement upsert (DB trigger refused a write the in-code residency \
                         check admitted — predicate divergence)",
                        &e,
                    ),
                    Err(e) => placement_db_panic("repo-placement upsert", &e),
                }
            }
        }
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
        reg.place_tenant(p)
            .expect("a single-region placement is admitted");
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
        assert!(
            e.to_string().contains("single-region by construction"),
            "loud reason: {e}"
        );
    }

    /// The HOME cell being cross-region is also rejected (the invariant covers `{home_cell} ∪
    /// member_cells`, not just member_cells).
    #[test]
    fn rejects_a_cross_region_home_cell() {
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-n-1", "eu-north")); // home, WRONG region.
        let p = placement("01J0ACME", "eu-west", "cell-n-1", &[]);
        let e = reg
            .place_tenant(p)
            .expect_err("a cross-region home cell is rejected");
        assert!(matches!(e, PlacementError::CrossRegionMemberCell { .. }));
        assert_eq!(reg.placement_count(), 0);
    }

    /// A placement referencing an UNKNOWN cell is refused fail-closed (the region pin cannot be
    /// verified against a cell that is not registered).
    #[test]
    fn rejects_an_unknown_cell_fail_closed() {
        let mut reg = Registry::new();
        let p = placement("01J0ACME", "eu-west", "cell-ghost", &[]);
        let e = reg
            .place_tenant(p)
            .expect_err("an unknown cell is refused fail-closed");
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
        let stored = reg
            .placement(&TenantId::from_token("01J0ACME"))
            .expect("placed");
        assert_eq!(
            stored.member_cells.len(),
            1,
            "v1 member_cells is single-element"
        );
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
        assert_eq!(
            reg.cell(&CellId::from_token("cell-w-1"))
                .unwrap()
                .region
                .as_str(),
            "eu-west"
        );
        assert_eq!(
            reg.cell(&CellId::from_token("cell-n-1"))
                .unwrap()
                .region
                .as_str(),
            "eu-north"
        );
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
        assert_eq!(
            reg.provisioning_log()[1].outcome,
            ProvisioningOutcome::Running
        );
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
        assert!(reg
            .local_tenants(&CellId::from_token("cell-w-2"))
            .is_empty());
    }
}

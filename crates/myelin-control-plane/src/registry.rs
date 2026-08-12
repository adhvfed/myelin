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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlacementError {
    CrossRegionMemberCell {
        tenant: TenantId,
        tenant_region: Region,
        cell: CellId,
        cell_region: Region,
    },
    UnknownCell {
        tenant: TenantId,
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
                 tenant is pinned to region `{}` - every cell in {{home_cell}} ∪ member_cells must \
                 be in the tenant's region (multi-cell is single-region by construction, \
                 architecture §5.1). 0 cross-region member cells are admitted.",
                tenant.as_str(),
                cell.as_str(),
                cell_region.as_str(),
                tenant_region.as_str()
            ),
            PlacementError::UnknownCell { tenant, cell } => write!(
                f,
                "placement invariant REJECTED tenant `{}`: cell `{}` is not registered - a \
                 placement whose region pin cannot be verified is refused (fail-closed, §5.3).",
                tenant.as_str(),
                cell.as_str()
            ),
        }
    }
}

impl std::error::Error for PlacementError {}

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
        member_cells: p
            .member_cells
            .iter()
            .map(|c| c.as_str().to_string())
            .collect(),
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

pub(crate) fn placement_db_panic(op: &str, why: &dyn core::fmt::Display) -> ! {
    panic!(
        "control-plane placement registry: durable {op} FAILED (fail-static loud - the placement \
         registry is the routing system-of-record; the write/read did NOT complete and there is no \
         silent in-memory fallback): {why}"
    )
}

pub(crate) fn corrupt_row_panic(table: &str, key: &str) -> ! {
    panic!(
        "control-plane placement registry: durable `{table}` row `{key}` carries an unknown \
         status/tier text - fail closed (the closed enums admit no silent coercion; the row is \
         corrupt or written by a newer schema)"
    )
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Debug, Default)]
struct MemoryRegistry {
    cells: BTreeMap<String, Cell>,
    placements: BTreeMap<String, TenantPlacement>,
    provisioning_log: Vec<CellProvisioning>,
    local_tenants: BTreeMap<String, BTreeMap<String, LocalTenant>>,
    repo_placements: BTreeMap<String, RepoPlacementRow>,
}

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

#[derive(Clone)]
enum RegistryBackend {
    #[cfg(any(test, feature = "test-support"))]
    Memory(MemoryRegistry),
    Pg(PgRegistry),
}

#[derive(Clone)]
pub struct Registry {
    backend: RegistryBackend,
}

impl core::fmt::Debug for Registry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
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
    #[cfg(any(test, feature = "test-support"))]
    pub fn new() -> Registry {
        Registry {
            backend: RegistryBackend::Memory(MemoryRegistry::default()),
        }
    }

    pub fn with_pg(backing: DurablePlacementBacking, rt: tokio::runtime::Handle) -> Registry {
        Registry {
            backend: RegistryBackend::Pg(PgRegistry { backing, rt }),
        }
    }

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

    pub fn cell(&self, cell_id: &CellId) -> Option<Cell> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            RegistryBackend::Memory(m) => m.cells.get(cell_id.as_str()).cloned(),
            RegistryBackend::Pg(pg) => pg
                .block(pg.backing.get_cell(cell_id.as_str()))
                .unwrap_or_else(|e| placement_db_panic("cell read", &e))
                .map(|r| {
                    durable_to_cell(&r).unwrap_or_else(|| corrupt_row_panic("cell", &r.cell_id))
                }),
        }
    }

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

    pub fn place_tenant(
        &mut self,
        placement: TenantPlacement,
    ) -> Result<Option<TenantPlacement>, PlacementError> {
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
                         - predicate divergence)",
                        &e,
                    ),
                    Err(e) => placement_db_panic("place_tenant", &e),
                }
            }
        }
    }

    pub fn check_placement_invariant(
        &self,
        placement: &TenantPlacement,
    ) -> Result<(), PlacementError> {
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

    pub fn activate_cell(&mut self, cell_id: &CellId) -> bool {
        self.set_cell_status(cell_id, crate::schema::CellStatus::Active)
    }

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
                         check admitted - predicate divergence)",
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

    #[test]
    fn admits_a_single_region_placement() {
        let mut reg = Registry::new();
        assert_eq!(reg.cell_count(), 0);
        reg.insert_cell(cell("cell-w-1", "eu-west"));
        reg.insert_cell(cell("cell-w-2", "eu-west"));
        assert_eq!(reg.cell_count(), 2);
        let p = placement("01J0ACME", "eu-west", "cell-w-1", &["cell-w-1"]);
        reg.place_tenant(p)
            .expect("a single-region placement is admitted");
        assert_eq!(reg.placement_count(), 1);
    }

    #[test]
    fn rejects_a_cross_region_member_cell() {
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-w-1", "eu-west"));
        reg.insert_cell(cell("cell-n-1", "eu-north"));
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
        assert_eq!(reg.placement_count(), 0);
        assert!(
            e.to_string().contains("single-region by construction"),
            "loud reason: {e}"
        );
    }

    #[test]
    fn rejects_a_cross_region_home_cell() {
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-n-1", "eu-north"));
        let p = placement("01J0ACME", "eu-west", "cell-n-1", &[]);
        let e = reg
            .place_tenant(p)
            .expect_err("a cross-region home cell is rejected");
        assert!(matches!(e, PlacementError::CrossRegionMemberCell { .. }));
        assert_eq!(reg.placement_count(), 0);
    }

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

    #[test]
    fn region_has_no_update_path() {
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-w-1", "eu-west"));
        reg.insert_cell(cell("cell-n-1", "eu-north"));
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
    }

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
        assert!(reg
            .local_tenants(&CellId::from_token("cell-w-2"))
            .is_empty());
    }
}

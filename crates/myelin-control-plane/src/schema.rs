use myelin_tenancy::{CellId, Region, TenantId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CellStatus {
    Provisioning,
    Active,
    Draining,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IsolationKind {
    Pool,
    Bridge,
    Dedicated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Capacity {
    pub tenants_max: u32,
    pub write_qps_max: u32,
    pub storage_bytes_max: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub cell_id: CellId,
    pub region: Region,
    pub status: CellStatus,
    pub isolation_kind: IsolationKind,
    pub capacity: Capacity,
    pub utilisation: u8,
    pub version: u32,
    pub endpoint: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantPlacement {
    pub tenant_id: TenantId,
    pub region: Region,
    pub home_cell: CellId,
    pub isolation_tier: IsolationKind,
    pub slug: String,
    pub status: PlacementStatus,
    pub member_cells: Vec<CellId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PlacementStatus {
    Pending,
    Active,
    Offboarding,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellProvisioning {
    pub cell_id: CellId,
    pub step: String,
    pub outcome: ProvisioningOutcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProvisioningOutcome {
    Running,
    Passed,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalTenant {
    pub tenant_id: TenantId,
    pub isolation_tier: IsolationKind,
    pub active: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_schema_is_opaque_only() {
        let region = Region::new("eu-west");
        let cell = Cell {
            cell_id: CellId::from_token("cell-eu-west-1"),
            region: region.clone(),
            status: CellStatus::Active,
            isolation_kind: IsolationKind::Pool,
            capacity: Capacity {
                tenants_max: 1000,
                write_qps_max: 5000,
                storage_bytes_max: 1 << 40,
            },
            utilisation: 42,
            version: 7,
            endpoint: "cell.eu-west.myelin.eu".into(),
        };
        assert_eq!(cell.region.as_str(), "eu-west");
        assert_eq!(cell.status, CellStatus::Active);

        let placement = TenantPlacement {
            tenant_id: TenantId::from_token("01J0ACME"),
            region: region.clone(),
            home_cell: cell.cell_id.clone(),
            isolation_tier: IsolationKind::Pool,
            slug: "acme".into(),
            status: PlacementStatus::Active,
            member_cells: vec![cell.cell_id.clone()],
        };
        assert_eq!(placement.slug, "acme");
        assert_eq!(placement.member_cells.len(), 1);

        let log = CellProvisioning {
            cell_id: cell.cell_id.clone(),
            step: "restore_verify".into(),
            outcome: ProvisioningOutcome::Passed,
        };
        assert_eq!(log.outcome, ProvisioningOutcome::Passed);

        let directory = LocalTenant {
            tenant_id: placement.tenant_id.clone(),
            isolation_tier: IsolationKind::Pool,
            active: true,
        };
        assert!(directory.active);
    }
}

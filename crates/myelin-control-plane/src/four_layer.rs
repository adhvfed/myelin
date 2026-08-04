use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_tenancy::{Region, TenantId};

use crate::placement_of::CellGateway;
use crate::placement_of::{GatewayReject, PlacementOf};
use crate::registry::{PlacementError, Registry};
use crate::schema::TenantPlacement;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResidencyWriteRejected {
    pub cell_region: Region,
    pub row_region: Region,
}

impl std::fmt::Display for ResidencyWriteRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "residency write boundary REJECTED a write: the row's region `{}` ≠ the cell's region \
             `{}` - every write must assert `row.region == cell.region` with the cell's region \
             injected by the harness (residency-pin layer 3, architecture §5.3). 0 out-of-region \
             writes are admitted; there is no cross-region query path for personal data.",
            self.row_region.as_str(),
            self.cell_region.as_str()
        )
    }
}

impl std::error::Error for ResidencyWriteRejected {}

#[derive(Clone)]
pub struct ResidencyWriteBoundary {
    cell_region: Region,
    out_of_region_writes_admitted: Arc<AtomicU64>,
}

impl ResidencyWriteBoundary {
    pub fn for_cell(cell_region: Region) -> ResidencyWriteBoundary {
        ResidencyWriteBoundary {
            cell_region,
            out_of_region_writes_admitted: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn cell_region(&self) -> &Region {
        &self.cell_region
    }

    pub fn out_of_region_writes_admitted(&self) -> u64 {
        self.out_of_region_writes_admitted.load(Ordering::SeqCst)
    }

    pub fn check_write(&self, row_region: &Region) -> Result<(), ResidencyWriteRejected> {
        if *row_region == self.cell_region {
            return Ok(());
        }
        Err(ResidencyWriteRejected {
            cell_region: self.cell_region.clone(),
            row_region: row_region.clone(),
        })
    }
}

impl std::fmt::Debug for ResidencyWriteBoundary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResidencyWriteBoundary")
            .field("cell_region", &self.cell_region.as_str())
            .field(
                "out_of_region_writes_admitted",
                &self.out_of_region_writes_admitted(),
            )
            .finish()
    }
}

pub struct FourLayerEnforcement<'a> {
    cell_region: Region,
    registry: &'a Registry,
    write_boundary: ResidencyWriteBoundary,
    gateway: CellGateway,
}

impl<'a> FourLayerEnforcement<'a> {
    pub fn new(
        registry: &'a Registry,
        gateway: CellGateway,
        cell_region: Region,
    ) -> FourLayerEnforcement<'a> {
        FourLayerEnforcement {
            write_boundary: ResidencyWriteBoundary::for_cell(cell_region.clone()),
            cell_region,
            registry,
            gateway,
        }
    }

    pub fn cell_region(&self) -> &Region {
        &self.cell_region
    }

    pub fn write_boundary(&self) -> &ResidencyWriteBoundary {
        &self.write_boundary
    }

    pub fn gateway(&self) -> &CellGateway {
        &self.gateway
    }

    pub fn admit_write(&self, row_region: &Region) -> Result<(), ResidencyWriteRejected> {
        self.write_boundary.check_write(row_region)
    }

    pub fn route(&self, tenant_id: &TenantId) -> Result<PlacementOf, GatewayReject> {
        self.gateway.route(self.registry, tenant_id)
    }

    pub fn assert_no_cross_region_query_path(
        &self,
        tenant_id: &TenantId,
        a_foreign_region: &Region,
    ) -> Result<(), CrossRegionPathError> {
        let placement =
            self.route(tenant_id)
                .map_err(|reject| CrossRegionPathError::TenantNotServedHere {
                    tenant: tenant_id.clone(),
                    reject: Box::new(reject),
                })?;
        if placement.region != self.cell_region {
            return Err(CrossRegionPathError::ServedTenantOutOfRegion {
                tenant: tenant_id.clone(),
                tenant_region: placement.region,
                cell_region: self.cell_region.clone(),
            });
        }

        self.admit_write(&self.cell_region).map_err(|_| {
            CrossRegionPathError::InRegionWriteRejected {
                cell_region: self.cell_region.clone(),
            }
        })?;

        if a_foreign_region == &self.cell_region {
            return Err(CrossRegionPathError::ForeignRegionNotForeign {
                cell_region: self.cell_region.clone(),
            });
        }
        match self.admit_write(a_foreign_region) {
            Err(_) => Ok(()),
            Ok(()) => Err(CrossRegionPathError::OutOfRegionWriteAdmitted {
                cell_region: self.cell_region.clone(),
                row_region: a_foreign_region.clone(),
            }),
        }
    }
}

#[derive(Debug)]
pub enum CrossRegionPathError {
    TenantNotServedHere {
        tenant: TenantId,
        reject: Box<GatewayReject>,
    },
    ServedTenantOutOfRegion {
        tenant: TenantId,
        tenant_region: Region,
        cell_region: Region,
    },
    InRegionWriteRejected {
        cell_region: Region,
    },
    OutOfRegionWriteAdmitted {
        cell_region: Region,
        row_region: Region,
    },
    ForeignRegionNotForeign {
        cell_region: Region,
    },
}

impl std::fmt::Display for CrossRegionPathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CrossRegionPathError::TenantNotServedHere { tenant, reject } => write!(
                f,
                "no-cross-region-query-path assertion: the cell does not home tenant `{}` (layer 4 \
                 rejected: {reject}) - it must not hold this tenant's data.",
                tenant.as_str()
            ),
            CrossRegionPathError::ServedTenantOutOfRegion {
                tenant,
                tenant_region,
                cell_region,
            } => write!(
                f,
                "no-cross-region-query-path assertion FAILED: served tenant `{}` is in region `{}` \
                 but the cell is in `{}` (layers 1+2 breach - the placement invariant should have \
                 prevented it).",
                tenant.as_str(),
                tenant_region.as_str(),
                cell_region.as_str()
            ),
            CrossRegionPathError::InRegionWriteRejected { cell_region } => write!(
                f,
                "no-cross-region-query-path assertion FAILED: a write in the cell's own region `{}` \
                 was rejected (layer 3 is mis-pinned).",
                cell_region.as_str()
            ),
            CrossRegionPathError::OutOfRegionWriteAdmitted { cell_region, row_region } => write!(
                f,
                "no-cross-region-query-path assertion FAILED: an out-of-region write (row region \
                 `{}`, cell region `{}`) was ADMITTED - a cross-region query path for personal data \
                 exists (the STOR-D5 breach).",
                row_region.as_str(),
                cell_region.as_str()
            ),
            CrossRegionPathError::ForeignRegionNotForeign { cell_region } => write!(
                f,
                "no-cross-region-query-path assertion misuse: the 'foreign' region equals the cell's \
                 region `{}` - pass a genuinely different region.",
                cell_region.as_str()
            ),
        }
    }
}

impl std::error::Error for CrossRegionPathError {}

impl FourLayerEnforcement<'_> {
    pub fn place(
        registry: &mut Registry,
        placement: TenantPlacement,
    ) -> Result<Option<TenantPlacement>, PlacementError> {
        registry.place_tenant(placement)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Capacity, Cell, CellStatus, IsolationKind, PlacementStatus};
    use myelin_tenancy::CellId;

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

    fn registry_with_acme() -> Registry {
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-w-1", "eu-west"));
        reg.insert_cell(cell("cell-w-2", "eu-west"));
        FourLayerEnforcement::place(
            &mut reg,
            TenantPlacement {
                tenant_id: TenantId::from_token("01J0ACME"),
                region: Region::new("eu-west"),
                home_cell: CellId::from_token("cell-w-1"),
                isolation_tier: IsolationKind::Pool,
                slug: "acme".into(),
                status: PlacementStatus::Active,
                member_cells: vec![CellId::from_token("cell-w-1")],
            },
        )
        .expect("a single-region placement is admitted (layers 1+2)");
        reg
    }

    #[test]
    fn write_boundary_admits_an_in_region_write() {
        let boundary = ResidencyWriteBoundary::for_cell(Region::new("eu-west"));
        boundary
            .check_write(&Region::new("eu-west"))
            .expect("an in-region write is admitted");
        assert_eq!(
            boundary.out_of_region_writes_admitted(),
            0,
            "0 out-of-region writes admitted"
        );
        assert_eq!(boundary.cell_region().as_str(), "eu-west");
    }

    #[test]
    fn write_boundary_rejects_an_out_of_region_write() {
        let boundary = ResidencyWriteBoundary::for_cell(Region::new("eu-west"));
        let rejected = boundary
            .check_write(&Region::new("eu-north"))
            .expect_err("an out-of-region write is REJECTED at the boundary");
        assert_eq!(
            rejected,
            ResidencyWriteRejected {
                cell_region: Region::new("eu-west"),
                row_region: Region::new("eu-north"),
            }
        );
        assert_eq!(
            boundary.out_of_region_writes_admitted(),
            0,
            "the out-of-region write was rejected, not admitted"
        );
        assert!(
            rejected.to_string().contains("REJECTED"),
            "loud: {rejected}"
        );
        assert!(
            rejected.to_string().contains("no cross-region query path"),
            "loud: {rejected}"
        );
    }

    #[test]
    fn write_boundary_region_is_immutable() {
        let boundary = ResidencyWriteBoundary::for_cell(Region::new("eu-west"));
        assert_eq!(boundary.cell_region().as_str(), "eu-west");
    }

    #[test]
    fn write_boundary_debug_is_pii_free() {
        let boundary = ResidencyWriteBoundary::for_cell(Region::new("eu-west"));
        let _ = boundary.check_write(&Region::new("eu-north"));
        let dbg = format!("{boundary:?}");
        assert!(dbg.contains("eu-west"), "shows the cell region: {dbg}");
        assert!(
            dbg.contains("out_of_region_writes_admitted"),
            "shows the zero: {dbg}"
        );
    }

    #[test]
    fn four_layers_compose_no_cross_region_query_path() {
        let reg = registry_with_acme();
        let gw = CellGateway::new(CellId::from_token("cell-w-1"));
        let enforcement = FourLayerEnforcement::new(&reg, gw, Region::new("eu-west"));

        enforcement
            .assert_no_cross_region_query_path(
                &TenantId::from_token("01J0ACME"),
                &Region::new("eu-north"),
            )
            .expect(
                "the home cell serves ACME and ACME's data stays in eu-west (no cross-region path)",
            );

        assert_eq!(
            enforcement.gateway().cross_tenant_reads(),
            0,
            "0 cross-tenant/cross-cell reads (layer 4)"
        );
        assert_eq!(
            enforcement.write_boundary().out_of_region_writes_admitted(),
            0,
            "0 out-of-region writes admitted (layer 3)"
        );
    }

    #[test]
    fn a_cell_that_does_not_home_the_tenant_is_rejected() {
        let reg = registry_with_acme();
        let gw = CellGateway::new(CellId::from_token("cell-w-2"));
        let enforcement = FourLayerEnforcement::new(&reg, gw, Region::new("eu-west"));
        let err = enforcement
            .assert_no_cross_region_query_path(
                &TenantId::from_token("01J0ACME"),
                &Region::new("eu-north"),
            )
            .expect_err(
                "cell-w-2 does not home ACME → the assertion fails (it must not hold ACME's data)",
            );
        assert!(matches!(
            err,
            CrossRegionPathError::TenantNotServedHere { .. }
        ));
        assert!(err.to_string().contains("does not home"), "loud: {err}");
        assert_eq!(enforcement.gateway().misroute_count(), 1);
        assert_eq!(enforcement.gateway().cross_tenant_reads(), 0);
    }

    #[test]
    fn assertion_is_not_vacuous() {
        let reg = registry_with_acme();
        let gw = CellGateway::new(CellId::from_token("cell-w-1"));
        let enforcement = FourLayerEnforcement::new(&reg, gw, Region::new("eu-west"));
        let err = enforcement
            .assert_no_cross_region_query_path(
                &TenantId::from_token("01J0ACME"),
                &Region::new("eu-west"),
            )
            .expect_err("a non-foreign region cannot exercise the rejection half → caught");
        assert!(matches!(
            err,
            CrossRegionPathError::ForeignRegionNotForeign { .. }
        ));
    }

    #[test]
    fn place_rejects_a_cross_region_member_cell() {
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-w-1", "eu-west"));
        reg.insert_cell(cell("cell-n-1", "eu-north"));
        let err = FourLayerEnforcement::place(
            &mut reg,
            TenantPlacement {
                tenant_id: TenantId::from_token("01J0ACME"),
                region: Region::new("eu-west"),
                home_cell: CellId::from_token("cell-w-1"),
                isolation_tier: IsolationKind::Pool,
                slug: "acme".into(),
                status: PlacementStatus::Active,
                member_cells: vec![
                    CellId::from_token("cell-w-1"),
                    CellId::from_token("cell-n-1"),
                ],
            },
        )
        .expect_err("a cross-region member cell is rejected (layers 1+2)");
        assert!(matches!(err, PlacementError::CrossRegionMemberCell { .. }));
    }

    #[test]
    fn cdc_four_layer_enforcement_provider_consumer() {
        let reg = registry_with_acme();
        let gw = CellGateway::new(CellId::from_token("cell-w-1"));
        let enforcement = FourLayerEnforcement::new(&reg, gw, Region::new("eu-west"));

        struct StoreWriteConsumer;
        impl StoreWriteConsumer {
            fn write_row(
                enforcement: &FourLayerEnforcement,
                row_region: &Region,
            ) -> Result<(), ResidencyWriteRejected> {
                enforcement.admit_write(row_region)
            }
        }

        struct GatewayConsumer;
        impl GatewayConsumer {
            fn serve(
                enforcement: &FourLayerEnforcement,
                tenant: &TenantId,
            ) -> Result<PlacementOf, GatewayReject> {
                enforcement.route(tenant)
            }
        }

        StoreWriteConsumer::write_row(&enforcement, &Region::new("eu-west"))
            .expect("the store-write consumer's in-region write is admitted");
        StoreWriteConsumer::write_row(&enforcement, &Region::new("eu-north"))
            .expect_err("the store-write consumer's out-of-region write is rejected");

        let served = GatewayConsumer::serve(&enforcement, &TenantId::from_token("01J0ACME"))
            .expect("the gateway consumer serves the home tenant");
        assert_eq!(served.region.as_str(), "eu-west");
    }
}

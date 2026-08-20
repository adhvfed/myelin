use myelin_storage::{DurableMisrouteAuditBacking, DurablePlacementBacking, PlacementWriteError};
use myelin_tenancy::{CellId, TenantId};

use crate::placement_of::{MisrouteAuditRecord, PlacementOf};
#[cfg(any(test, feature = "test-support"))]
use crate::registry::Registry;
use crate::registry::{corrupt_row_panic, placement_db_panic};
use crate::registry_codec::{
    decode_cell, decode_placement, encode_cell, encode_placement, validate_cell,
};
use crate::schema::{Cell, TenantPlacement};

#[derive(Clone)]
struct PgPlacement {
    placement: DurablePlacementBacking,
    audit: DurableMisrouteAuditBacking,
    rt: tokio::runtime::Handle,
}

impl PgPlacement {
    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

#[derive(Clone)]
enum PlacementBackend {
    #[cfg(any(test, feature = "test-support"))]
    Memory(Registry),
    Pg(PgPlacement),
}

#[derive(Clone)]
pub struct DurablePlacementRegistry {
    backend: PlacementBackend,
}

impl DurablePlacementRegistry {
    #[cfg(any(test, feature = "test-support"))]
    pub fn in_memory() -> DurablePlacementRegistry {
        DurablePlacementRegistry {
            backend: PlacementBackend::Memory(Registry::new()),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn from_registry(reg: Registry) -> DurablePlacementRegistry {
        DurablePlacementRegistry {
            backend: PlacementBackend::Memory(reg),
        }
    }

    pub fn with_pg(
        placement: DurablePlacementBacking,
        audit: DurableMisrouteAuditBacking,
        rt: tokio::runtime::Handle,
    ) -> DurablePlacementRegistry {
        DurablePlacementRegistry {
            backend: PlacementBackend::Pg(PgPlacement {
                placement,
                audit,
                rt,
            }),
        }
    }

    pub fn insert_cell(&mut self, cell: Cell) -> Result<(), PlacementWriteError> {
        validate_cell(&cell).map_err(|why| PlacementWriteError::InvalidValue(why.to_string()))?;
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PlacementBackend::Memory(reg) => {
                reg.insert_cell(cell);
                Ok(())
            }
            PlacementBackend::Pg(pg) => {
                let row = encode_cell(&cell)
                    .expect("validate_cell established that the cell is durably representable");
                pg.block(pg.placement.insert_cell(&row))
                    .map_err(|e| PlacementWriteError::Db(e.to_string()))
            }
        }
    }

    pub fn cell(&self, cell_id: &CellId) -> Option<Cell> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PlacementBackend::Memory(reg) => reg.cell(cell_id),
            PlacementBackend::Pg(pg) => pg
                .block(pg.placement.get_cell(cell_id.as_str()))
                .unwrap_or_else(|e| placement_db_panic("cell read", &e))
                .map(|r| {
                    decode_cell(&r)
                        .unwrap_or_else(|why| corrupt_row_panic("cell", &r.cell_id, &why))
                }),
        }
    }

    pub fn place_tenant(&mut self, placement: TenantPlacement) -> Result<(), PlacementWriteError> {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PlacementBackend::Memory(reg) => match reg.place_tenant(placement) {
                Ok(_) => Ok(()),
                Err(e) => Err(PlacementWriteError::InvariantRejected(e.to_string())),
            },
            PlacementBackend::Pg(pg) => {
                pg.block(pg.placement.place_tenant(&encode_placement(&placement)))
            }
        }
    }

    pub fn placement(&self, tenant_id: &TenantId) -> Option<TenantPlacement> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PlacementBackend::Memory(reg) => reg.placement(tenant_id),
            PlacementBackend::Pg(pg) => pg
                .block(pg.placement.get_placement(tenant_id.as_str()))
                .unwrap_or_else(|e| placement_db_panic("placement read", &e))
                .map(|r| {
                    decode_placement(&r).unwrap_or_else(|why| {
                        corrupt_row_panic("tenant_placement", &r.tenant_id, &why)
                    })
                }),
        }
    }

    pub fn placement_of(&self, tenant_id: &TenantId) -> Option<PlacementOf> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PlacementBackend::Memory(reg) => reg.placement_of(tenant_id),
            PlacementBackend::Pg(_) => self.placement(tenant_id).map(|p| PlacementOf {
                region: p.region,
                home_cell: p.home_cell,
                member_cells: p.member_cells,
                isolation_tier: p.isolation_tier,
                status: p.status,
            }),
        }
    }

    pub fn record_misroute(&self, rec: &MisrouteAuditRecord) -> Result<(), PlacementWriteError> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PlacementBackend::Memory(_) => Ok(()),
            PlacementBackend::Pg(pg) => pg
                .block(pg.audit.record(
                    rec.tenant_id.as_str(),
                    rec.received_by_cell.as_str(),
                    rec.home_cell.as_ref().map(|c| c.as_str()),
                ))
                .map_err(|e| PlacementWriteError::Db(e.to_string())),
        }
    }

    pub fn audited_misroute_count(&self) -> i64 {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PlacementBackend::Memory(_) => 0,
            PlacementBackend::Pg(pg) => pg
                .block(pg.audit.count())
                .unwrap_or_else(|e| placement_db_panic("misroute-audit count", &e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Capacity, CellStatus, IsolationKind, PlacementStatus};
    use myelin_tenancy::Region;

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
    fn memory_arm_round_trips_and_enforces_the_invariant() {
        let mut reg = DurablePlacementRegistry::in_memory();
        reg.insert_cell(cell("cell-w-1", "eu-west"))
            .expect("insert cell");
        reg.place_tenant(placement("01J0ACME", "eu-west", "cell-w-1", &["cell-w-1"]))
            .expect("a single-region placement is admitted");
        let answer = reg
            .placement_of(&TenantId::from_token("01J0ACME"))
            .expect("placed");
        assert_eq!(answer.home_cell.as_str(), "cell-w-1");
        assert_eq!(answer.region.as_str(), "eu-west");

        reg.insert_cell(cell("cell-n-1", "eu-north"))
            .expect("insert north cell");
        let e = reg
            .place_tenant(placement(
                "01J0BETA",
                "eu-west",
                "cell-w-1",
                &["cell-w-1", "cell-n-1"],
            ))
            .expect_err("a cross-region member cell is rejected");
        assert!(
            matches!(e, PlacementWriteError::InvariantRejected(_)),
            "got {e}"
        );
    }

    #[test]
    fn converters_round_trip_through_opaque_text() {
        let c = cell("cell-w-1", "eu-west");
        assert_eq!(decode_cell(&encode_cell(&c).unwrap()).unwrap(), c);
        let p = placement("01J0ACME", "eu-west", "cell-w-1", &["cell-w-1"]);
        assert_eq!(decode_placement(&encode_placement(&p)).unwrap(), p);
    }

    #[test]
    fn an_unrepresentable_cell_is_a_typed_rejection_before_storage() {
        let mut reg = DurablePlacementRegistry::in_memory();
        let mut impossible = cell("cell-too-large", "eu-west");
        impossible.capacity.storage_bytes_max = i64::MAX as u64 + 1;

        let error = reg
            .insert_cell(impossible)
            .expect_err("the database boundary must not wrap an oversized u64");
        assert!(
            matches!(error, PlacementWriteError::InvalidValue(_)),
            "the caller receives an actionable value rejection: {error}"
        );
        assert_eq!(
            reg.cell(&CellId::from_token("cell-too-large")),
            None,
            "a rejected cell never reaches even the in-memory test backing"
        );
    }
}

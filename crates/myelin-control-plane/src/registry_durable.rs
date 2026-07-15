//! # Binding the control-plane placement registry to real Postgres (MR-024, SI-011/SI-028)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md` §5.1/§5.3.
//!
//! This is the **`Pg | Memory` backend enum** that binds the placement registry to durable storage —
//! the consumer side of MR-024 (the real PG tables + the HARD-invariant TRIGGER + the durable audit
//! sink live in [`myelin_storage::placement_durable`]). It mirrors how
//! `myelin_identity_service::principal_store::PrincipalStore` binds to
//! `myelin_storage::DurablePrincipalBacking` (the `with_pg` arm), and how the events `DedupLedger`
//! binds to `DurableDedupBacking`.
//!
//! ## EXTEND, never fork — the Memory variant IS the canonical [`Registry`]
//! [`PlacementBackend::Memory`] wraps the SAME [`crate::registry::Registry`] every drill + route uses
//! (the in-memory test-double, with the placement invariant checked in code). [`PlacementBackend::Pg`]
//! is the REAL durable system-of-record. There is NO second registry type or second invariant: on the
//! Pg arm the invariant is the DB TRIGGER (the same predicate, enforced at the database); on the
//! Memory arm it is [`Registry::check_placement_invariant`]. The no-update-region rule is preserved on
//! both arms (no `update_*_region` method anywhere; the Pg trigger also rejects a region change).
//!
//! ## Isolation posture (decided + justified)
//! The placement registry is cross-tenant ROUTING infra (it routes ALL tenants to cells), PII-free
//! (opaque ids only). It is NOT a per-request tenant data store, so the Pg arm does NOT use the
//! per-request RLS/`with_tenant_tx` convention (that is for `principal`/`rebac_tuple`); it connects
//! to the OLTP pool directly — see the NAMED `tenant-predicate` exclusion in
//! `myelin-storage/src/placement_durable.rs` / `tests/workspace_clean.rs`.
//!
//! ## Status as of MR-009b W6d (the durable-by-default flip)
//! Compiled UNCONDITIONALLY (the `integration` feature is a test-selector only). The canonical
//! [`Registry`] is itself now a role struct whose ALWAYS-COMPILED production backend is the durable
//! PG whole surface (`Registry::with_pg` — cell + tenant_placement + repo_placement +
//! cell_provisioning + local_tenant, migrations 0030–0039), so THIS binding's `Memory(Registry)`
//! arm is the `test-support`-gated double and its `with_pg` arm remains the narrow MR-024 surface
//! (cell + tenant_placement + misroute_audit) the MR-024 live proofs exercise.

use myelin_storage::{DurableMisrouteAuditBacking, DurablePlacementBacking, PlacementWriteError};
use myelin_tenancy::{CellId, TenantId};

use crate::placement_of::{MisrouteAuditRecord, PlacementOf};
// One converter set for the whole crate (W6d): the opaque-text <-> typed-enum mappers live in
// `crate::registry` (the role-struct's own Pg arm uses them) and are reused here.
use crate::registry::{cell_to_durable, durable_to_cell, durable_to_placement, placement_to_durable};
#[cfg(any(test, feature = "test-support"))]
use crate::registry::Registry;
use crate::schema::{Cell, TenantPlacement};

// =================================================================================================
// The Pg backing handle + the sync→async bridge.
// =================================================================================================

/// The Pg arm: the durable placement backing + the durable misroute-audit sink + the runtime handle
/// the sync registry API drives the async sqlx backing on (the `block_in_place`+`block_on` bridge,
/// the same one identity-service / the storage backings use).
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

/// The registry backend: the in-memory test-double (the canonical [`Registry`]) or the REAL durable
/// PG backing. Splitting the backing out of the role struct is what makes the Pg arm a clean swap.
#[derive(Clone)]
enum PlacementBackend {
    /// The in-memory test-double — the canonical [`Registry`] on ITS OWN `test-support`-gated
    /// Memory arm (MR-009b W6d: compiled only under `#[cfg(any(test, feature = "test-support"))]`;
    /// NOT the production system-of-record).
    #[cfg(any(test, feature = "test-support"))]
    Memory(Registry),
    /// The REAL durable PG backing (MR-024) — the system-of-record (durable tables + the invariant
    /// TRIGGER + the durable audit sink).
    Pg(PgPlacement),
}

/// **A durable-capable control-plane placement registry (MR-024).** Binds the placement registry to
/// real Postgres via a `Pg | Memory` backend enum. The Memory arm REUSES the canonical [`Registry`];
/// the Pg arm is the durable system-of-record. The load-bearing routing surface (cell inventory +
/// `place_tenant` under the invariant + `placement`/`placement_of`) and the durable misroute audit
/// are bound here; production boot selecting the Pg arm is MR-009.
#[derive(Clone)]
pub struct DurablePlacementRegistry {
    backend: PlacementBackend,
}

impl DurablePlacementRegistry {
    /// The in-memory test-double over a fresh canonical [`Registry`] — **TEST DOUBLE** (MR-009b
    /// W6d: compiled only under `#[cfg(any(test, feature = "test-support"))]`).
    #[cfg(any(test, feature = "test-support"))]
    pub fn in_memory() -> DurablePlacementRegistry {
        DurablePlacementRegistry {
            backend: PlacementBackend::Memory(Registry::new()),
        }
    }

    /// Wrap an existing canonical [`Registry`] as the Memory arm — **TEST DOUBLE** (gated as
    /// [`Self::in_memory`]).
    #[cfg(any(test, feature = "test-support"))]
    pub fn from_registry(reg: Registry) -> DurablePlacementRegistry {
        DurablePlacementRegistry {
            backend: PlacementBackend::Memory(reg),
        }
    }

    /// **Bind the registry to the REAL durable PG backing (MR-024 / SI-011/SI-028).** The cell
    /// inventory + tenant placements persist through the durable [`DurablePlacementBacking`] (the
    /// HARD invariant enforced by the DB TRIGGER) and the misroute audit through
    /// [`DurableMisrouteAuditBacking`] — both survive a process restart. `rt` is the tokio runtime
    /// handle the sync API drives the async backing on. Production boot wiring is MR-009.
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

    /// Insert (upsert) a `cell` inventory row. `region` is immutable on both arms (the Pg conflict
    /// path never overwrites it; the Memory arm has no `update_cell_region`).
    pub fn insert_cell(&mut self, cell: Cell) -> Result<(), PlacementWriteError> {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PlacementBackend::Memory(reg) => {
                reg.insert_cell(cell);
                Ok(())
            }
            PlacementBackend::Pg(pg) => pg
                .block(pg.placement.insert_cell(&cell_to_durable(&cell)))
                .map_err(|e| PlacementWriteError::Db(e.to_string())),
        }
    }

    /// Look up a `cell` by opaque id.
    pub fn cell(&self, cell_id: &CellId) -> Option<Cell> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PlacementBackend::Memory(reg) => reg.cell(cell_id),
            PlacementBackend::Pg(pg) => pg
                .block(pg.placement.get_cell(cell_id.as_str()))
                .ok()
                .flatten()
                .and_then(|r| durable_to_cell(&r)),
        }
    }

    /// **`place_tenant` — write a `tenant_placement` row IFF the HARD placement invariant holds.** On
    /// the Memory arm the invariant is [`Registry::check_placement_invariant`]; on the Pg arm it is
    /// the REAL DB TRIGGER. A cross-region / unknown cell is rejected on BOTH arms (fail-closed).
    pub fn place_tenant(
        &mut self,
        placement: TenantPlacement,
    ) -> Result<(), PlacementWriteError> {
        match &mut self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PlacementBackend::Memory(reg) => match reg.place_tenant(placement) {
                Ok(_) => Ok(()),
                // Surface the in-code invariant rejection in the SAME shape the Pg trigger does.
                Err(e) => Err(PlacementWriteError::InvariantRejected(e.to_string())),
            },
            PlacementBackend::Pg(pg) => {
                pg.block(pg.placement.place_tenant(&placement_to_durable(&placement)))
            }
        }
    }

    /// Look up a `tenant_placement` row by opaque tenant id.
    pub fn placement(&self, tenant_id: &TenantId) -> Option<TenantPlacement> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PlacementBackend::Memory(reg) => reg.placement(tenant_id),
            PlacementBackend::Pg(pg) => pg
                .block(pg.placement.get_placement(tenant_id.as_str()))
                .ok()
                .flatten()
                .and_then(|r| durable_to_placement(&r)),
        }
    }

    /// **`placement_of(tenant_id)` — the PII-free routing answer** (`{region, home_cell,
    /// member_cells, isolation_tier, status}`), read off the authoritative `tenant_placement` row.
    /// `None` for an unplaced tenant. The same frozen tuple [`Registry::placement_of`] returns.
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

    /// **Record a rejected misroute into the durable audit sink (SI-028).** On the Pg arm the record
    /// survives a process restart. On the Memory arm there is no in-process sink here (the canonical
    /// [`crate::placement_of::MisrouteAudit`] is the in-memory test-double the gateway uses) — this is
    /// a no-op so the binding is uniform; the durable trail is the Pg arm's job.
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

    /// How many misroutes the durable audit sink holds (0 on the Memory arm — see
    /// [`Self::record_misroute`]).
    pub fn audited_misroute_count(&self) -> i64 {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            PlacementBackend::Memory(_) => 0,
            PlacementBackend::Pg(pg) => pg.block(pg.audit.count()).unwrap_or(0),
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

    /// The Memory arm round-trips through the canonical [`Registry`] (the default test-double) and
    /// enforces the in-code invariant — the same behaviour the Pg arm enforces via the DB trigger.
    #[test]
    fn memory_arm_round_trips_and_enforces_the_invariant() {
        let mut reg = DurablePlacementRegistry::in_memory();
        reg.insert_cell(cell("cell-w-1", "eu-west")).expect("insert cell");
        reg.place_tenant(placement("01J0ACME", "eu-west", "cell-w-1", &["cell-w-1"]))
            .expect("a single-region placement is admitted");
        let answer = reg
            .placement_of(&TenantId::from_token("01J0ACME"))
            .expect("placed");
        assert_eq!(answer.home_cell.as_str(), "cell-w-1");
        assert_eq!(answer.region.as_str(), "eu-west");

        // The invariant rejects a cross-region member cell on the Memory arm too (fail-closed).
        reg.insert_cell(cell("cell-n-1", "eu-north")).expect("insert north cell");
        let e = reg
            .place_tenant(placement("01J0BETA", "eu-west", "cell-w-1", &["cell-w-1", "cell-n-1"]))
            .expect_err("a cross-region member cell is rejected");
        assert!(matches!(e, PlacementWriteError::InvariantRejected(_)), "got {e}");
    }

    /// The converters round-trip the typed enums through opaque text (the storage boundary) exactly.
    #[test]
    fn converters_round_trip_through_opaque_text() {
        let c = cell("cell-w-1", "eu-west");
        assert_eq!(durable_to_cell(&cell_to_durable(&c)).unwrap(), c);
        let p = placement("01J0ACME", "eu-west", "cell-w-1", &["cell-w-1"]);
        assert_eq!(durable_to_placement(&placement_to_durable(&p)).unwrap(), p);
    }
}

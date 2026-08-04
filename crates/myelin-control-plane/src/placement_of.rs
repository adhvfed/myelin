use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
#[cfg(any(test, feature = "test-support"))]
use std::sync::Mutex;

use myelin_storage::placement_durable::DurableMisrouteAuditBacking;
use myelin_tenancy::{CellId, Region, TenantId};

use crate::registry::Registry;
use crate::schema::{IsolationKind, PlacementStatus};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacementOf {
    pub region: Region,
    pub home_cell: CellId,
    pub member_cells: Vec<CellId>,
    pub isolation_tier: IsolationKind,
    pub status: PlacementStatus,
}

impl Registry {
    pub fn placement_of(&self, tenant_id: &TenantId) -> Option<PlacementOf> {
        let row = self.placement(tenant_id)?;
        Some(PlacementOf {
            region: row.region.clone(),
            home_cell: row.home_cell.clone(),
            member_cells: row.member_cells.clone(),
            isolation_tier: row.isolation_tier,
            status: row.status,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Misroute {
    pub tenant_id: TenantId,
    pub correct_cell: CellId,
    pub correct_cell_endpoint: String,
}

impl core::fmt::Display for Misroute {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "misroute: tenant `{}` is not hosted by this cell - REJECTED (not proxied) + REDIRECTED \
             to its home cell `{}` at `{}` (§5.3 layer 4 / §7.3; there is no cross-region query path \
             for personal data). 0 cross-tenant/cross-cell rows read.",
            self.tenant_id.as_str(),
            self.correct_cell.as_str(),
            self.correct_cell_endpoint
        )
    }
}

impl std::error::Error for Misroute {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GatewayReject {
    Misroute(Misroute),
    NoSuchTenant {
        tenant_id: TenantId,
    },
}

impl core::fmt::Display for GatewayReject {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            GatewayReject::Misroute(m) => write!(f, "{m}"),
            GatewayReject::NoSuchTenant { tenant_id } => write!(
                f,
                "no-route: the control plane knows no placement for tenant `{}` - REJECTED (not \
                 served, not proxied); no redirect target (a stale/unknown tenant id). 0 \
                 cross-tenant/cross-cell rows read.",
                tenant_id.as_str()
            ),
        }
    }
}

impl std::error::Error for GatewayReject {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MisrouteAuditRecord {
    pub tenant_id: TenantId,
    pub received_by_cell: CellId,
    pub home_cell: Option<CellId>,
}

#[derive(Clone)]
struct PgMisrouteAudit {
    backing: DurableMisrouteAuditBacking,
    rt: tokio::runtime::Handle,
}

impl PgMisrouteAudit {
    fn block<F: std::future::Future>(&self, fut: F) -> F::Output {
        tokio::task::block_in_place(|| self.rt.block_on(fut))
    }
}

#[derive(Clone)]
enum MisrouteAuditBackend {
    #[cfg(any(test, feature = "test-support"))]
    Memory(Arc<Mutex<Vec<MisrouteAuditRecord>>>),
    Pg(PgMisrouteAudit),
}

#[derive(Clone)]
pub struct MisrouteAudit {
    backend: MisrouteAuditBackend,
}

impl core::fmt::Debug for MisrouteAudit {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let arm = match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            MisrouteAuditBackend::Memory(_) => "Memory(test-double)",
            MisrouteAuditBackend::Pg(_) => "Pg(durable)",
        };
        f.debug_struct("MisrouteAudit").field("backend", &arm).finish()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for MisrouteAudit {
    fn default() -> MisrouteAudit {
        MisrouteAudit::new()
    }
}

impl MisrouteAudit {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new() -> MisrouteAudit {
        MisrouteAudit {
            backend: MisrouteAuditBackend::Memory(Arc::new(Mutex::new(Vec::new()))),
        }
    }

    pub fn with_pg(backing: DurableMisrouteAuditBacking, rt: tokio::runtime::Handle) -> MisrouteAudit {
        MisrouteAudit {
            backend: MisrouteAuditBackend::Pg(PgMisrouteAudit { backing, rt }),
        }
    }

    fn record(&self, rec: MisrouteAuditRecord) {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            MisrouteAuditBackend::Memory(records) => {
                records.lock().unwrap_or_else(|e| e.into_inner()).push(rec);
            }
            MisrouteAuditBackend::Pg(pg) => pg
                .block(pg.backing.record(
                    rec.tenant_id.as_str(),
                    rec.received_by_cell.as_str(),
                    rec.home_cell.as_ref().map(|c| c.as_str()),
                ))
                .unwrap_or_else(|e| {
                    panic!(
                        "misroute audit: durable record FAILED (fail-static loud - an unrecorded \
                         misroute is silently-lost layer-4 evidence; the write did NOT land): {e}"
                    )
                }),
        }
    }

    pub(crate) fn record_misroute(&self, rec: MisrouteAuditRecord) {
        self.record(rec);
    }

    pub fn records(&self) -> Vec<MisrouteAuditRecord> {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            MisrouteAuditBackend::Memory(records) => {
                records.lock().unwrap_or_else(|e| e.into_inner()).clone()
            }
            MisrouteAuditBackend::Pg(pg) => pg
                .block(pg.backing.records())
                .unwrap_or_else(|e| {
                    panic!("misroute audit: durable read FAILED (fail-static loud): {e}")
                })
                .iter()
                .map(|r| MisrouteAuditRecord {
                    tenant_id: TenantId::from_token(&r.tenant_id),
                    received_by_cell: CellId::from_token(&r.received_by_cell),
                    home_cell: r.home_cell.as_deref().map(CellId::from_token),
                })
                .collect(),
        }
    }

    pub fn count(&self) -> usize {
        match &self.backend {
            #[cfg(any(test, feature = "test-support"))]
            MisrouteAuditBackend::Memory(records) => {
                records.lock().unwrap_or_else(|e| e.into_inner()).len()
            }
            MisrouteAuditBackend::Pg(pg) => pg
                .block(pg.backing.count())
                .unwrap_or_else(|e| {
                    panic!("misroute audit: durable count FAILED (fail-static loud): {e}")
                }) as usize,
        }
    }
}

#[derive(Clone)]
pub struct CellGateway {
    cell_id: CellId,
    audit: MisrouteAudit,
    misroute_count: Arc<AtomicU64>,
    cross_tenant_reads: Arc<AtomicU64>,
}

impl CellGateway {
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(cell_id: CellId) -> CellGateway {
        CellGateway::with_audit(cell_id, MisrouteAudit::new())
    }

    pub fn with_audit(cell_id: CellId, audit: MisrouteAudit) -> CellGateway {
        CellGateway {
            cell_id,
            audit,
            misroute_count: Arc::new(AtomicU64::new(0)),
            cross_tenant_reads: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn cell_id(&self) -> &CellId {
        &self.cell_id
    }

    pub fn audit(&self) -> &MisrouteAudit {
        &self.audit
    }

    pub fn misroute_count(&self) -> u64 {
        self.misroute_count.load(Ordering::SeqCst)
    }

    pub fn cross_tenant_reads(&self) -> u64 {
        self.cross_tenant_reads.load(Ordering::SeqCst)
    }

    pub(crate) fn bump_misroute_count(&self) {
        self.misroute_count.fetch_add(1, Ordering::SeqCst);
    }

    pub fn route(
        &self,
        registry: &Registry,
        tenant_id: &TenantId,
    ) -> Result<PlacementOf, GatewayReject> {
        let Some(placement) = registry.placement_of(tenant_id) else {
            self.misroute_count.fetch_add(1, Ordering::SeqCst);
            self.audit.record(MisrouteAuditRecord {
                tenant_id: tenant_id.clone(),
                received_by_cell: self.cell_id.clone(),
                home_cell: None,
            });
            return Err(GatewayReject::NoSuchTenant {
                tenant_id: tenant_id.clone(),
            });
        };

        if placement.home_cell == self.cell_id {
            return Ok(placement);
        }

        self.misroute_count.fetch_add(1, Ordering::SeqCst);
        self.audit.record(MisrouteAuditRecord {
            tenant_id: tenant_id.clone(),
            received_by_cell: self.cell_id.clone(),
            home_cell: Some(placement.home_cell.clone()),
        });
        let correct_cell_endpoint = registry
            .cell(&placement.home_cell)
            .map(|c| c.endpoint.clone())
            .unwrap_or_else(|| format!("cell-unresolved:{}", placement.home_cell.as_str()));
        Err(GatewayReject::Misroute(Misroute {
            tenant_id: tenant_id.clone(),
            correct_cell: placement.home_cell,
            correct_cell_endpoint,
        }))
    }
}

impl core::fmt::Debug for CellGateway {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CellGateway")
            .field("cell_id", &self.cell_id.as_str())
            .field("misroute_count", &self.misroute_count())
            .field("cross_tenant_reads", &self.cross_tenant_reads())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{
        Capacity, Cell, CellStatus, IsolationKind, PlacementStatus, TenantPlacement,
    };

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

    fn registry_two_cells() -> Registry {
        let mut reg = Registry::new();
        reg.insert_cell(cell("cell-w-1", "eu-west"));
        reg.insert_cell(cell("cell-w-2", "eu-west"));
        reg.place_tenant(TenantPlacement {
            tenant_id: TenantId::from_token("01J0ACME"),
            region: Region::new("eu-west"),
            home_cell: CellId::from_token("cell-w-1"),
            isolation_tier: IsolationKind::Pool,
            slug: "acme".into(),
            status: PlacementStatus::Active,
            member_cells: vec![CellId::from_token("cell-w-1")],
        })
        .expect("a single-region placement is admitted");
        reg
    }

    #[test]
    fn placement_of_returns_the_frozen_routing_tuple() {
        let reg = registry_two_cells();
        let answer = reg
            .placement_of(&TenantId::from_token("01J0ACME"))
            .expect("a placed tenant resolves to a placement_of answer");
        assert_eq!(answer.region.as_str(), "eu-west");
        assert_eq!(answer.home_cell.as_str(), "cell-w-1");
        assert_eq!(
            answer.member_cells.len(),
            1,
            "v1 member_cells is single-element (the floor)"
        );
        assert_eq!(answer.member_cells[0].as_str(), "cell-w-1");
        assert_eq!(answer.isolation_tier, IsolationKind::Pool);
        assert_eq!(answer.status, PlacementStatus::Active);
    }

    #[test]
    fn placement_of_unknown_tenant_is_none() {
        let reg = registry_two_cells();
        assert!(reg
            .placement_of(&TenantId::from_token("01J0GHOST"))
            .is_none());
    }

    #[test]
    fn gateway_accepts_a_tenant_it_hosts() {
        let reg = registry_two_cells();
        let gw = CellGateway::new(CellId::from_token("cell-w-1"));
        let answer = gw
            .route(&reg, &TenantId::from_token("01J0ACME"))
            .expect("the home cell serves its own tenant");
        assert_eq!(answer.home_cell.as_str(), "cell-w-1");
        assert_eq!(gw.misroute_count(), 0, "an accept is not a misroute");
        assert_eq!(gw.audit().count(), 0, "nothing to audit on an accept");
        assert_eq!(
            gw.cross_tenant_reads(),
            0,
            "the home cell serving its own tenant is not cross-tenant"
        );
    }

    #[test]
    fn gateway_rejects_and_redirects_a_misrouted_tenant() {
        let reg = registry_two_cells();
        let gw = CellGateway::new(CellId::from_token("cell-w-2"));
        let reject = gw
            .route(&reg, &TenantId::from_token("01J0ACME"))
            .expect_err("cell-w-2 does not host this tenant → rejected, not served");

        assert_eq!(
            reject,
            GatewayReject::Misroute(Misroute {
                tenant_id: TenantId::from_token("01J0ACME"),
                correct_cell: CellId::from_token("cell-w-1"),
                correct_cell_endpoint: "cell.eu-west.cell-w-1.myelin.eu".into(),
            }),
            "the misroute redirects to the home cell-endpoint"
        );
        assert_eq!(gw.audit().count(), 1, "the misroute was audited");
        assert_eq!(
            gw.audit().records()[0],
            MisrouteAuditRecord {
                tenant_id: TenantId::from_token("01J0ACME"),
                received_by_cell: CellId::from_token("cell-w-2"),
                home_cell: Some(CellId::from_token("cell-w-1")),
            },
            "the audit record is the PII-free misroute evidence (opaque ids only)"
        );
        assert_eq!(
            gw.misroute_count(),
            1,
            "misroute_count increments on a rejected misroute"
        );
        assert_eq!(
            gw.cross_tenant_reads(),
            0,
            "0 cross-tenant/cross-cell rows read (the CP-D2 zero)"
        );
        assert!(
            reject.to_string().contains("REJECTED (not proxied)"),
            "loud: {reject}"
        );
    }

    #[test]
    fn gateway_rejects_an_unknown_tenant_with_no_redirect() {
        let reg = registry_two_cells();
        let gw = CellGateway::new(CellId::from_token("cell-w-1"));
        let reject = gw
            .route(&reg, &TenantId::from_token("01J0GHOST"))
            .expect_err("an unknown tenant is rejected (no route)");
        assert_eq!(
            reject,
            GatewayReject::NoSuchTenant {
                tenant_id: TenantId::from_token("01J0GHOST")
            }
        );
        assert_eq!(
            gw.audit().count(),
            1,
            "the unknown-tenant rejection is audited"
        );
        assert_eq!(
            gw.audit().records()[0].home_cell,
            None,
            "no redirect target for an unknown tenant"
        );
        assert_eq!(gw.misroute_count(), 1);
        assert_eq!(gw.cross_tenant_reads(), 0);
        assert!(
            reject.to_string().contains("no redirect target"),
            "loud: {reject}"
        );
    }

    #[test]
    fn a_misroute_redirect_is_then_served_by_the_home_cell() {
        let reg = registry_two_cells();
        let wrong = CellGateway::new(CellId::from_token("cell-w-2"));
        let GatewayReject::Misroute(redirect) = wrong
            .route(&reg, &TenantId::from_token("01J0ACME"))
            .expect_err("the wrong cell rejects + redirects")
        else {
            panic!("expected a misroute redirect");
        };
        let home = CellGateway::new(redirect.correct_cell.clone());
        let served = home
            .route(&reg, &TenantId::from_token("01J0ACME"))
            .expect("the home cell serves the redirected request");
        assert_eq!(served.home_cell, redirect.correct_cell);
        assert_eq!(
            home.misroute_count(),
            0,
            "the home cell does not misroute its own tenant"
        );
        assert_eq!(wrong.cross_tenant_reads(), 0);
        assert_eq!(home.cross_tenant_reads(), 0);
    }

    #[test]
    fn every_misroute_is_audited_and_counted() {
        let mut reg = registry_two_cells();
        reg.place_tenant(TenantPlacement {
            tenant_id: TenantId::from_token("01J0BETA"),
            region: Region::new("eu-west"),
            home_cell: CellId::from_token("cell-w-2"),
            isolation_tier: IsolationKind::Pool,
            slug: "beta".into(),
            status: PlacementStatus::Active,
            member_cells: vec![CellId::from_token("cell-w-2")],
        })
        .expect("placed");
        let gw = CellGateway::new(CellId::from_token("cell-w-1"));
        assert!(gw.route(&reg, &TenantId::from_token("01J0BETA")).is_err());
        assert!(gw.route(&reg, &TenantId::from_token("01J0BETA")).is_err());
        assert!(gw.route(&reg, &TenantId::from_token("01J0GHOST")).is_err());
        assert_eq!(
            gw.misroute_count(),
            3,
            "each misroute (incl. unknown) is counted"
        );
        assert_eq!(
            gw.audit().count(),
            3,
            "each misroute is audited (append-only, none swallowed)"
        );
        assert_eq!(
            gw.cross_tenant_reads(),
            0,
            "still 0 cross-tenant reads across all misroutes"
        );
    }

    #[test]
    fn cell_gateway_debug_is_pii_free() {
        let reg = registry_two_cells();
        let gw = CellGateway::new(CellId::from_token("cell-w-2"));
        let _ = gw.route(&reg, &TenantId::from_token("01J0ACME"));
        let dbg = format!("{gw:?}");
        assert!(
            dbg.contains("cell-w-2"),
            "the Debug shows the cell id: {dbg}"
        );
        assert!(
            dbg.contains("misroute_count"),
            "the Debug shows the aggregate count: {dbg}"
        );
        assert!(
            !dbg.contains("01J0ACME"),
            "the Debug leaks no tenant id: {dbg}"
        );
    }
}

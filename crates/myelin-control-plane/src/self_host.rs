use myelin_storage::placement_durable::{DurableMisrouteAuditBacking, DurablePlacementBacking};
use myelin_tenancy::{CellId, Region, TenantId};

use crate::four_layer::{CrossRegionPathError, FourLayerEnforcement, ResidencyWriteRejected};
use crate::place::{CounterMinter, PlaceError, PlacementAnswer, PlacementService};
use crate::placement_of::{CellGateway, GatewayReject, MisrouteAudit, PlacementOf};
use crate::registry::Registry;
use crate::residency_verify::{
    residency_verify, ResidencyMismatch, ResidencySigningKey, ResidencyStoreClass,
    SignedAttestation, StoreRegionReport,
};
use crate::schema::{Capacity, Cell, CellStatus, IsolationKind};

#[derive(Debug)]
pub struct DegenerateControlPlane {
    registry: Registry,
    cell_id: CellId,
    region: Region,
    audit: MisrouteAudit,
}

impl DegenerateControlPlane {
    #[cfg(any(test, feature = "test-support"))]
    pub fn bootstrap(cell_id: CellId, region: Region) -> DegenerateControlPlane {
        let endpoint = format!("cell.{}.{}.local", region.as_str(), cell_id.as_str());
        Self::with_endpoint(cell_id, region, endpoint)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn with_endpoint(
        cell_id: CellId,
        region: Region,
        endpoint: String,
    ) -> DegenerateControlPlane {
        Self::over(
            Registry::new(),
            MisrouteAudit::new(),
            cell_id,
            region,
            endpoint,
        )
    }

    pub fn with_pg(
        cell_id: CellId,
        region: Region,
        endpoint: String,
        placement: DurablePlacementBacking,
        audit: DurableMisrouteAuditBacking,
        rt: tokio::runtime::Handle,
    ) -> DegenerateControlPlane {
        let cp = Self::over(
            Registry::with_pg(placement, rt.clone()),
            MisrouteAudit::with_pg(audit, rt),
            cell_id,
            region,
            endpoint,
        );
        assert!(
            cp.registry.cell_count() >= 1,
            "self-host boot: the durable registry is EMPTY after registering the install's own \
             cell - refusing to boot a control plane with no routing authority (fail loud)"
        );
        let durable = cp.cell();
        assert!(
            durable.region == cp.region,
            "self-host boot: this install claims region '{}' but the durable cell row for '{}' is \
             pinned to region '{}' (region is immutable) - refusing to boot on a mismatched region \
             claim; fix the boot config (fail loud, never a silent divergence)",
            cp.region.0,
            cp.cell_id.0,
            durable.region.0
        );
        cp
    }

    fn over(
        mut registry: Registry,
        audit: MisrouteAudit,
        cell_id: CellId,
        region: Region,
        endpoint: String,
    ) -> DegenerateControlPlane {
        registry.insert_cell(Cell {
            cell_id: cell_id.clone(),
            region: region.clone(),
            status: CellStatus::Active,
            isolation_kind: IsolationKind::Pool,
            capacity: Capacity {
                tenants_max: 1_000,
                write_qps_max: 5_000,
                storage_bytes_max: 1 << 40,
            },
            utilisation: 0,
            version: 1,
            endpoint,
        });
        DegenerateControlPlane {
            registry,
            cell_id,
            region,
            audit,
        }
    }

    pub fn cell_id(&self) -> &CellId {
        &self.cell_id
    }

    pub fn region(&self) -> &Region {
        &self.region
    }

    pub fn cell(&self) -> Cell {
        self.registry
            .cell(&self.cell_id)
            .expect("the degenerate control plane always has its one cell")
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn registry_mut(&mut self) -> &mut Registry {
        &mut self.registry
    }

    pub fn place(
        &mut self,
        service: &PlacementService<CounterMinter>,
        requested_tier: IsolationKind,
        slug: &str,
    ) -> Result<PlacementAnswer, PlaceError> {
        let region = self.region.clone();
        service.place(&mut self.registry, &region, requested_tier, slug)
    }

    pub fn discover_cell(&self, tenant_id: &TenantId) -> Option<CellId> {
        use crate::discover::DiscoverKey;
        self.registry
            .discover(&DiscoverKey::TenantId(tenant_id.clone()), 30)
            .map(|route| route.cell_id)
    }

    pub fn placement_of(&self, tenant_id: &TenantId) -> Option<PlacementOf> {
        self.registry.placement_of(tenant_id)
    }

    pub fn gateway(&self) -> CellGateway {
        CellGateway::with_audit(self.cell_id.clone(), self.audit.clone())
    }

    pub fn route(&self, tenant_id: &TenantId) -> Result<PlacementOf, GatewayReject> {
        self.gateway().route(&self.registry, tenant_id)
    }

    pub fn four_layer(&self) -> FourLayerEnforcement<'_> {
        FourLayerEnforcement::new(&self.registry, self.gateway(), self.region.clone())
    }

    pub fn cp_d3_residency_pin_holds(
        &self,
        a_foreign_region: &Region,
    ) -> Result<(), ResidencyWriteRejected> {
        let four_layer = self.four_layer();
        four_layer.admit_write(&self.region)?;
        match four_layer.admit_write(a_foreign_region) {
            Err(_) => Ok(()),
            Ok(()) => Err(ResidencyWriteRejected {
                cell_region: self.region.clone(),
                row_region: a_foreign_region.clone(),
            }),
        }
    }

    pub fn assert_no_cross_region_query_path(
        &self,
        tenant_id: &TenantId,
        a_foreign_region: &Region,
    ) -> Result<(), CrossRegionPathError> {
        self.four_layer()
            .assert_no_cross_region_query_path(tenant_id, a_foreign_region)
    }

    pub fn residency_verify_own_data(
        &self,
        tenant_id: &TenantId,
        key: &ResidencySigningKey,
    ) -> Result<SignedAttestation, ResidencyMismatch> {
        let reports: Vec<StoreRegionReport> = ResidencyStoreClass::M1_SET
            .iter()
            .map(|class| StoreRegionReport::new(*class, self.region.clone()))
            .collect();
        residency_verify(tenant_id, &self.region, &reports, key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::place::PlacementService;
    use crate::placement_of::GatewayReject;
    use crate::schema::PlacementStatus;

    fn self_host() -> DegenerateControlPlane {
        DegenerateControlPlane::bootstrap(CellId::from_token("cell-self"), Region::new("fr-par"))
    }

    #[test]
    fn degenerate_control_plane_is_a_one_row_registry() {
        let sh = self_host();
        assert_eq!(
            sh.registry().cell_count(),
            1,
            "a self-host install is EXACTLY one cell"
        );
        let cell = sh.cell();
        assert_eq!(cell.cell_id.as_str(), "cell-self");
        assert_eq!(
            cell.region.as_str(),
            "fr-par",
            "pinned to the install's region"
        );
        assert_eq!(
            cell.status,
            CellStatus::Active,
            "the one cell serves traffic"
        );
        assert_eq!(
            cell.isolation_kind,
            IsolationKind::Pool,
            "self-host is the Pool v1 tier"
        );
    }

    #[test]
    fn place_runs_the_identical_code_path_to_this_cell() {
        let mut sh = self_host();
        let service = PlacementService::new(CounterMinter::new());
        let answer = sh
            .place(&service, IsolationKind::Pool, "acme")
            .expect("the one Active cell is eligible → placed");
        assert_eq!(answer.home_cell.as_str(), "cell-self");
        assert_eq!(answer.cell_endpoint, "cell.fr-par.cell-self.local");
        assert_eq!(answer.isolation_tier, IsolationKind::Pool);
        assert_eq!(service.signals().placement_count, 1);
        assert_eq!(sh.registry().placement_count(), 1);
    }

    #[test]
    fn discover_and_placement_of_return_this_cell() {
        let mut sh = self_host();
        let service = PlacementService::new(CounterMinter::new());
        let answer = sh
            .place(&service, IsolationKind::Pool, "acme")
            .expect("placed");
        let tenant = answer.tenant_id.clone();

        let discovered = sh
            .discover_cell(&tenant)
            .expect("a placed tenant discovers");
        assert_eq!(
            discovered.as_str(),
            "cell-self",
            "discover returns 'this cell'"
        );

        let placement = sh
            .placement_of(&tenant)
            .expect("a placed tenant has a placement_of answer");
        assert_eq!(placement.home_cell.as_str(), "cell-self");
        assert_eq!(placement.region.as_str(), "fr-par");
        assert_eq!(
            placement.member_cells.len(),
            1,
            "v1 single-element (multi-cell N/A for self-host)"
        );
        assert_eq!(placement.member_cells[0].as_str(), "cell-self");
        assert_eq!(
            placement.status,
            PlacementStatus::Pending,
            "place writes Pending (phase 2 pending)"
        );
    }

    #[test]
    fn the_one_cell_gateway_accepts_every_tenant() {
        let mut sh = self_host();
        let service = PlacementService::new(CounterMinter::new());
        let answer = sh
            .place(&service, IsolationKind::Pool, "acme")
            .expect("placed");
        let served = sh
            .route(&answer.tenant_id)
            .expect("the one cell homes (and serves) every tenant");
        assert_eq!(served.home_cell.as_str(), "cell-self");
        let gw = sh.gateway();
        let _ = gw.route(sh.registry(), &answer.tenant_id);
        assert_eq!(gw.misroute_count(), 0, "no misroute on a one-cell install");
        assert_eq!(
            gw.cross_tenant_reads(),
            0,
            "0 cross-tenant reads (the CP-D2 zero) on the degenerate cell"
        );
    }

    #[test]
    fn an_unplaced_tenant_is_rejected_the_same_way() {
        let sh = self_host();
        let ghost = TenantId::from_token("01J0GHOST");
        assert!(
            sh.discover_cell(&ghost).is_none(),
            "an unplaced tenant has no route"
        );
        assert!(sh.placement_of(&ghost).is_none());
        let reject = sh
            .route(&ghost)
            .expect_err("an unplaced tenant is rejected");
        assert!(matches!(reject, GatewayReject::NoSuchTenant { .. }));
    }

    #[test]
    fn cp_d3_residency_pin_holds_on_the_degenerate_cell() {
        let sh = self_host();
        sh.cp_d3_residency_pin_holds(&Region::new("eu-north"))
            .expect(
                "the residency-pin holds on the degenerate cell (out-of-region write rejected)",
            );
        let four_layer = sh.four_layer();
        four_layer
            .admit_write(&Region::new("fr-par"))
            .expect("the install-region write is admitted");
        four_layer.admit_write(&Region::new("us-east")).expect_err(
            "an out-of-region write is rejected at the boundary on the degenerate cell",
        );
    }

    #[test]
    fn no_cross_region_query_path_on_the_degenerate_cell() {
        let mut sh = self_host();
        let service = PlacementService::new(CounterMinter::new());
        let answer = sh
            .place(&service, IsolationKind::Pool, "acme")
            .expect("placed");
        sh.assert_no_cross_region_query_path(&answer.tenant_id, &Region::new("eu-north"))
            .expect("the one cell serves its tenant and that data stays in fr-par (no cross-region path)");
    }

    #[test]
    fn residency_verify_green_on_the_one_cell_install() {
        let mut sh = self_host();
        let service = PlacementService::new(CounterMinter::new());
        let answer = sh
            .place(&service, IsolationKind::Pool, "acme")
            .expect("placed");
        let key = ResidencySigningKey::from_bytes([13u8; 32]);
        let attestation = sh
            .residency_verify_own_data(&answer.tenant_id, &key)
            .expect("residency_verify is green on the self-host cell's own data");
        assert_eq!(
            attestation.region.as_str(),
            "fr-par",
            "every store reported the install's region"
        );
        assert_eq!(
            attestation.store_regions.len(),
            ResidencyStoreClass::M1_SET.len(),
            "every M1 store attested"
        );
        assert!(
            attestation.verify(&key),
            "the green attestation verifies (0 mismatches)"
        );
    }

    #[test]
    fn place_in_a_foreign_region_finds_no_eligible_cell() {
        let sh = self_host();
        let assigned = sh
            .registry()
            .assign_cell(&Region::new("eu-north"), IsolationKind::Pool);
        assert!(
            assigned.is_none(),
            "a one-cell install only places in its own region"
        );
    }

    #[test]
    fn managed_fleet_only_is_na_by_definition_not_a_gap() {
        let mut sh = self_host();
        let service = PlacementService::new(CounterMinter::new());
        let answer = sh
            .place(&service, IsolationKind::Pool, "acme")
            .expect("placed");
        let placement = sh.placement_of(&answer.tenant_id).expect("placed");
        assert_eq!(
            placement.member_cells,
            vec![CellId::from_token("cell-self")]
        );
        assert_eq!(sh.registry().cell_count(), 1);
    }

    #[test]
    fn cdc_degenerate_cell_configuration_provider_consumer() {
        struct SelfHostGateway {
            this_cell: CellId,
        }
        impl SelfHostGateway {
            fn serving_cell(&self, placement: &PlacementOf) -> CellId {
                placement.home_cell.clone()
            }
            fn this_cell_hosts(&self, placement: &PlacementOf) -> bool {
                placement.home_cell == self.this_cell
            }
        }

        let mut sh = self_host();
        let service = PlacementService::new(CounterMinter::new());
        let answer = sh
            .place(&service, IsolationKind::Pool, "acme")
            .expect("placed");

        let placement = sh.placement_of(&answer.tenant_id).expect("placed");
        let gw = SelfHostGateway {
            this_cell: sh.cell_id().clone(),
        };
        assert_eq!(
            gw.serving_cell(&placement).as_str(),
            "cell-self",
            "resolves to 'this cell'"
        );
        assert!(
            gw.this_cell_hosts(&placement),
            "the one cell hosts every tenant (off the routing answer)"
        );
    }

    #[test]
    fn no_self_host_fork_the_shared_api_runs() {
        let mut sh = self_host();
        let service = PlacementService::new(CounterMinter::new());
        let answer = sh
            .place(&service, IsolationKind::Pool, "acme")
            .expect("placed");
        let tenant = answer.tenant_id.clone();

        let registry = sh.registry();
        use crate::discover::DiscoverKey;
        let route = registry
            .discover(&DiscoverKey::TenantId(tenant.clone()), 30)
            .expect("shared discover resolves");
        assert_eq!(route.cell_id.as_str(), "cell-self");
        let placement = registry
            .placement_of(&tenant)
            .expect("shared placement_of resolves");
        assert_eq!(placement.home_cell.as_str(), "cell-self");
        let gw = CellGateway::new(sh.cell_id().clone());
        let served = gw.route(registry, &tenant).expect("shared gateway serves");
        assert_eq!(served.home_cell.as_str(), "cell-self");
        let key = ResidencySigningKey::from_bytes([13u8; 32]);
        let reports: Vec<StoreRegionReport> = ResidencyStoreClass::M1_SET
            .iter()
            .map(|class| StoreRegionReport::new(*class, sh.region().clone()))
            .collect();
        let att = residency_verify(&tenant, sh.region(), &reports, &key)
            .expect("shared residency_verify is green");
        assert!(att.verify(&key));
    }
}

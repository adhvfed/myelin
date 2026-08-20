pub mod bulkhead;
pub mod cp_outage;
pub mod cross_cell_bridge;
pub mod cross_cell_bridge_durable;
pub mod discover;
pub mod four_layer;
pub mod holder;
pub mod isolation;
pub mod migration;
pub mod mirror_allowed;
pub mod multi_cell;
pub mod place;
pub mod placement_of;
pub mod placement_of_repo;
pub mod provision;
pub mod registry;
mod registry_codec;
pub mod registry_durable;
pub mod residency_verify;
pub mod runner_claim_pin;
pub mod schema;
pub mod self_host;

pub use bulkhead::{CellAdmission, CellBulkhead, CellFleet, CellFleetReport, SURGE_MULTIPLIER};
pub use cp_outage::{
    cp_outage_bound, ControlPlane, CpOutageReport, DataPlane, DegradeScope, ServeFailure, Served,
    SignupDegraded, SignupPlane,
};
pub use cross_cell_bridge::{
    bridge_carried_fields, BridgeMode, BridgeProjection, BridgeResolution, BridgeTombstone,
    BridgeTombstoneReason, CellLocalResolver, CellResolverRegistry, CrossCellBridge,
    ResolverProjection, ViewerId,
};
pub use cross_cell_bridge_durable::{ProjectionError, ResolverFactory};
pub use discover::{DiscoverKey, DiscoveryCache, DiscoverySignals, RouteTuple};
pub use four_layer::{
    CrossRegionPathError, FourLayerEnforcement, ResidencyWriteBoundary, ResidencyWriteRejected,
};
pub use holder::{
    assert_no_personal_columns, control_plane_data_map, ColumnClassification, ControlPlaneHolder,
    CONTROL_PLANE_STORE,
};
pub use isolation::{partition_key, IsolationTier, PartitionKey, PoolStore};
pub use migration::{
    measured_hot_at, restore_verify_at_cell_scale, CellTenantCopy, LiveMigration, MigrationError,
    MigrationPlan, MigrationReceipt, MigrationTrigger, WF_DURABLE_PROVISION, WF_LIVE_MIGRATION,
    WF_REPO_RELOCATION,
};
pub use mirror_allowed::{
    MirrorAllowReason, MirrorDecision, MirrorDenyReason, MirrorGate, MirrorTarget, TransferPolicy,
};
pub use multi_cell::{
    resolve_across_member_cells, CellDsrReceipt, CellLocalEraser, CrossCellDsrFanOut,
    CrossCellZookieReader, MultiCellDsrReceiptSet, RebalanceReceipt, ZookieStaleness,
    ZOOKIE_STALENESS_BUDGET_SECS,
};
pub use place::{
    CounterMinter, PlaceError, PlacementAnswer, PlacementService, PlacementSignals, TokenMinter,
};
pub use placement_of::{
    CellGateway, GatewayReject, Misroute, MisrouteAudit, MisrouteAuditRecord, PlacementOf,
};
pub use placement_of_repo::{RepoPlacement, RepoPlacementError, StorageGroup};
pub use provision::{
    ProvisionFailure, ProvisionVerdict, ProvisioningGate, ProvisioningSignals, STEP_ACTIVATE,
    STEP_READINESS, STEP_RESTORE_VERIFY,
};
pub use registry::{PlacementError, Registry};
pub use residency_verify::{
    residency_verify, residency_verify_ci, residency_verify_over, RequiredStoreSet,
    ResidencyAttestationSignal, ResidencyMismatch, ResidencySigningKey, ResidencyStoreClass,
    SignedAttestation, StoreRegionReport,
};
pub use runner_claim_pin::{CiStoreWritePinError, OutOfRegionRunnerClaim, RunnerClaimPin};
pub use schema::{
    Capacity, Cell, CellProvisioning, CellStatus, IsolationKind, LocalTenant, PlacementStatus,
    ProvisioningOutcome, TenantPlacement,
};
pub use self_host::DegenerateControlPlane;

use myelin_substrate::{
    AppSpec, Config, Migration, Migrations, OutboxSpec, StoreKind, StoreManifest,
};

pub const SERVICE_NAME: &str = "control-plane";

pub fn control_plane_migrations() -> Migrations {
    Migrations::of([
        Migration::plain(
            "0001_cell",
            "CREATE TABLE cell (\
                 cell_id TEXT PRIMARY KEY, \
                 region TEXT NOT NULL, \
                 status TEXT NOT NULL, \
                 isolation_kind TEXT NOT NULL, \
                 capacity JSONB NOT NULL, \
                 utilisation SMALLINT NOT NULL, \
                 version INT NOT NULL, \
                 endpoint TEXT NOT NULL);",
        ),
        Migration::plain(
            "0002_tenant_placement",
            "CREATE TABLE tenant_placement (\
                 tenant_id TEXT PRIMARY KEY, \
                 region TEXT NOT NULL, \
                 home_cell TEXT NOT NULL REFERENCES cell(cell_id), \
                 isolation_tier TEXT NOT NULL, \
                 slug TEXT NOT NULL, \
                 status TEXT NOT NULL, \
                 member_cells TEXT[] NOT NULL);",
        ),
        Migration::plain(
            "0003_cell_provisioning",
            "CREATE TABLE cell_provisioning (\
                 id BIGSERIAL PRIMARY KEY, \
                 cell_id TEXT NOT NULL REFERENCES cell(cell_id), \
                 step TEXT NOT NULL, \
                 outcome TEXT NOT NULL);",
        ),
        Migration::plain(
            "0004_local_tenant",
            "CREATE TABLE local_tenant (\
                 cell_id TEXT NOT NULL, \
                 tenant_id TEXT NOT NULL, \
                 isolation_tier TEXT NOT NULL, \
                 active BOOLEAN NOT NULL, \
                 PRIMARY KEY (cell_id, tenant_id));",
        ),
        Migration::plain(
            "0005_placement_invariant",
            "CREATE FUNCTION assert_placement_single_region() RETURNS trigger AS $$ \
             BEGIN \
               IF EXISTS ( \
                 SELECT 1 FROM cell c \
                 WHERE c.cell_id = ANY (array_append(NEW.member_cells, NEW.home_cell)) \
                   AND c.region <> NEW.region \
               ) THEN \
                 RAISE EXCEPTION 'placement invariant: a cell is in a different region than the tenant (multi-cell is single-region by construction)'; \
               END IF; \
               RETURN NEW; \
             END; $$ LANGUAGE plpgsql; \
             CREATE TRIGGER tenant_placement_single_region \
               BEFORE INSERT OR UPDATE ON tenant_placement \
               FOR EACH ROW EXECUTE FUNCTION assert_placement_single_region();",
        ),
    ])
}

pub fn control_plane_store_manifest() -> StoreManifest {
    StoreManifest::of([myelin_substrate::DeclaredStore::new(
        StoreKind::Oltp,
        CONTROL_PLANE_STORE_NAME,
    )])
}

pub const CONTROL_PLANE_STORE_NAME: &str = "control_plane_registry";

pub fn control_plane_app_spec(config: Config, outbox: OutboxSpec) -> AppSpec {
    let mut spec = AppSpec::minimal(SERVICE_NAME, config, outbox);
    spec.migrations = control_plane_migrations();
    spec.stores = control_plane_store_manifest();
    spec
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellUtilisationSignal {
    pub cell_id: String,
    pub utilisation: u8,
}

pub fn cell_utilisation_signal(cell: &Cell) -> CellUtilisationSignal {
    CellUtilisationSignal {
        cell_id: cell.cell_id.as_str().to_string(),
        utilisation: cell.utilisation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_substrate::{is_destructive, HolderRegistry, HotTables};

    #[test]
    fn migrations_are_forward_only_and_pii_free() {
        let migrations = control_plane_migrations();
        assert_eq!(
            migrations.0.len(),
            5,
            "cell + placement + provisioning + directory + trigger"
        );
        for m in &migrations.0 {
            assert!(
                !is_destructive(m.ddl.as_ref()),
                "migration {} must be forward-only (no DROP)",
                m.id
            );
            let lower = m.ddl.to_ascii_lowercase();
            for pii in ["email", "full_name", " name ", "phone", "address", "body"] {
                assert!(
                    !lower.contains(pii),
                    "migration {} must carry no PII column (`{pii}`)",
                    m.id
                );
            }
        }
    }

    #[test]
    fn placement_invariant_trigger_is_installed() {
        let migrations = control_plane_migrations();
        let trigger = migrations
            .0
            .iter()
            .find(|m| m.id == "0005_placement_invariant")
            .expect("the placement-invariant trigger migration exists");
        assert!(trigger.ddl.contains("CREATE TRIGGER"));
        assert!(trigger
            .ddl
            .contains("BEFORE INSERT OR UPDATE ON tenant_placement"));
        assert!(trigger.ddl.contains("RAISE EXCEPTION"));
    }

    #[test]
    fn app_spec_carries_registry_and_store_manifest() {
        let spec = control_plane_app_spec(Config::default(), OutboxSpec::default_inproc());
        assert_eq!(spec.name, SERVICE_NAME);
        assert_eq!(spec.migrations.0.len(), 5);
        let ids = spec.stores.holder_ids();
        assert!(
            ids.contains("oltp:control_plane_registry"),
            "registry store declared: {ids:?}"
        );
        let mut runner = myelin_substrate::MigrationRunner::new();
        runner
            .run(&spec.migrations, &HotTables::none())
            .expect("registry migrations are admitted (forward-only, PII-free)");
    }

    #[test]
    fn registry_store_auto_registers_as_a_holder() {
        let manifest = control_plane_store_manifest();
        let mut registry = HolderRegistry::new();
        for store in manifest.stores() {
            registry.open(store.kind, store.name);
        }
        let violations = myelin_substrate::holder_registered(&manifest, &registry);
        assert!(
            violations.is_empty(),
            "every declared store auto-registers: {violations:?}"
        );
        assert!(registry.is_registered(StoreKind::Oltp, CONTROL_PLANE_STORE_NAME));
    }

    #[test]
    fn cell_utilisation_signal_is_aggregate_and_pii_free() {
        use myelin_tenancy::{CellId, Region};
        let cell = Cell {
            cell_id: CellId::from_token("cell-eu-west-1"),
            region: Region::new("eu-west"),
            status: CellStatus::Active,
            isolation_kind: IsolationKind::Pool,
            capacity: Capacity {
                tenants_max: 1000,
                write_qps_max: 5000,
                storage_bytes_max: 1 << 40,
            },
            utilisation: 73,
            version: 1,
            endpoint: "cell.eu-west.myelin.eu".into(),
        };
        let signal = cell_utilisation_signal(&cell);
        assert_eq!(
            signal,
            CellUtilisationSignal {
                cell_id: "cell-eu-west-1".into(),
                utilisation: 73,
            }
        );
    }

    #[test]
    fn cdc_12_3_registry_schema_provider_consumer() {
        use myelin_tenancy::{CellId, Region, TenantId};

        struct PlacementOfAnswer {
            region: Region,
            home_cell: CellId,
            member_cells: Vec<CellId>,
            isolation_tier: IsolationKind,
            status: PlacementStatus,
        }
        impl PlacementOfAnswer {
            fn from_row(row: &TenantPlacement) -> PlacementOfAnswer {
                PlacementOfAnswer {
                    region: row.region.clone(),
                    home_cell: row.home_cell.clone(),
                    member_cells: row.member_cells.clone(),
                    isolation_tier: row.isolation_tier,
                    status: row.status,
                }
            }
        }

        let mut registry = Registry::new();
        registry.insert_cell(Cell {
            cell_id: CellId::from_token("cell-w-1"),
            region: Region::new("eu-west"),
            status: CellStatus::Active,
            isolation_kind: IsolationKind::Pool,
            capacity: Capacity {
                tenants_max: 1000,
                write_qps_max: 5000,
                storage_bytes_max: 1 << 40,
            },
            utilisation: 5,
            version: 1,
            endpoint: "cell.eu-west.myelin.eu".into(),
        });
        let tenant = TenantId::from_token("01J0ACME");
        registry
            .place_tenant(TenantPlacement {
                tenant_id: tenant.clone(),
                region: Region::new("eu-west"),
                home_cell: CellId::from_token("cell-w-1"),
                isolation_tier: IsolationKind::Pool,
                slug: "acme".into(),
                status: PlacementStatus::Active,
                member_cells: vec![CellId::from_token("cell-w-1")],
            })
            .expect("the registry admits the single-region placement");

        let row = registry
            .placement(&tenant)
            .expect("the placement is stored");
        let answer = PlacementOfAnswer::from_row(&row);
        assert_eq!(answer.region.as_str(), "eu-west");
        assert_eq!(answer.home_cell.as_str(), "cell-w-1");
        assert_eq!(answer.member_cells.len(), 1);
        assert_eq!(answer.isolation_tier, IsolationKind::Pool);
        assert_eq!(answer.status, PlacementStatus::Active);
    }

    #[test]
    fn cdc_12_2_discover_tenant_grain_provider_consumer() {
        use myelin_tenancy::{CellId, Region, TenantId};

        struct GatewayRoute {
            target_endpoint: String,
            pinned_region: String,
            cache_ttl_secs: u64,
        }
        impl GatewayRoute {
            fn from_route(route: &RouteTuple) -> GatewayRoute {
                GatewayRoute {
                    target_endpoint: route.cell_endpoint.clone(),
                    pinned_region: route.region.as_str().to_string(),
                    cache_ttl_secs: route.ttl_seconds,
                }
            }
        }

        let mut registry = Registry::new();
        registry.insert_cell(Cell {
            cell_id: CellId::from_token("cell-w-1"),
            region: Region::new("eu-west"),
            status: CellStatus::Active,
            isolation_kind: IsolationKind::Pool,
            capacity: Capacity {
                tenants_max: 1000,
                write_qps_max: 5000,
                storage_bytes_max: 1 << 40,
            },
            utilisation: 5,
            version: 1,
            endpoint: "cell.eu-west.myelin.eu".into(),
        });
        registry
            .place_tenant(TenantPlacement {
                tenant_id: TenantId::from_token("01J0ACME"),
                region: Region::new("eu-west"),
                home_cell: CellId::from_token("cell-w-1"),
                isolation_tier: IsolationKind::Pool,
                slug: "acme".into(),
                status: PlacementStatus::Active,
                member_cells: vec![CellId::from_token("cell-w-1")],
            })
            .expect("the single-region placement is admitted");

        let route = registry
            .discover(&DiscoverKey::TenantId(TenantId::from_token("01J0ACME")), 30)
            .expect("the placed tenant resolves to a route");
        let gw = GatewayRoute::from_route(&route);
        assert_eq!(gw.target_endpoint, "cell.eu-west.myelin.eu");
        assert_eq!(gw.pinned_region, "eu-west");
        assert_eq!(gw.cache_ttl_secs, 30);

        let by_slug = registry
            .discover(&DiscoverKey::Slug("acme".into()), 30)
            .expect("the slug resolves to a route");
        assert_eq!(
            GatewayRoute::from_route(&by_slug).target_endpoint,
            "cell.eu-west.myelin.eu"
        );
    }

    #[test]
    fn cdc_12_3_placement_of_provider_consumer() {
        use myelin_tenancy::{CellId, Region, TenantId};

        struct GatewayHostsDecision {
            home_cell: CellId,
            region: Region,
            member_cells: Vec<CellId>,
            isolation_tier: IsolationKind,
            status: PlacementStatus,
        }
        impl GatewayHostsDecision {
            fn from_answer(a: &crate::placement_of::PlacementOf) -> GatewayHostsDecision {
                GatewayHostsDecision {
                    home_cell: a.home_cell.clone(),
                    region: a.region.clone(),
                    member_cells: a.member_cells.clone(),
                    isolation_tier: a.isolation_tier,
                    status: a.status,
                }
            }
            fn this_cell_hosts(&self, this_cell: &CellId) -> bool {
                &self.home_cell == this_cell
            }
        }

        let mut registry = Registry::new();
        registry.insert_cell(Cell {
            cell_id: CellId::from_token("cell-w-1"),
            region: Region::new("eu-west"),
            status: CellStatus::Active,
            isolation_kind: IsolationKind::Pool,
            capacity: Capacity {
                tenants_max: 1000,
                write_qps_max: 5000,
                storage_bytes_max: 1 << 40,
            },
            utilisation: 5,
            version: 1,
            endpoint: "cell.eu-west.cell-w-1.myelin.eu".into(),
        });
        registry
            .place_tenant(TenantPlacement {
                tenant_id: TenantId::from_token("01J0ACME"),
                region: Region::new("eu-west"),
                home_cell: CellId::from_token("cell-w-1"),
                isolation_tier: IsolationKind::Pool,
                slug: "acme".into(),
                status: PlacementStatus::Active,
                member_cells: vec![CellId::from_token("cell-w-1")],
            })
            .expect("the single-region placement is admitted");

        let answer = registry
            .placement_of(&TenantId::from_token("01J0ACME"))
            .expect("the placed tenant resolves to a placement_of answer");
        let decision = GatewayHostsDecision::from_answer(&answer);
        assert_eq!(decision.region.as_str(), "eu-west");
        assert_eq!(
            decision.member_cells.len(),
            1,
            "v1 member_cells single-element"
        );
        assert_eq!(decision.isolation_tier, IsolationKind::Pool);
        assert_eq!(decision.status, PlacementStatus::Active);
        assert!(
            decision.this_cell_hosts(&CellId::from_token("cell-w-1")),
            "the home cell hosts"
        );
        assert!(
            !decision.this_cell_hosts(&CellId::from_token("cell-w-2")),
            "a different cell misroutes"
        );
    }
}

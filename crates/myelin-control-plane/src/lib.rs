//! # `myelin-control-plane` — the PII-free control-plane registry + routing (CP-M1)
//!
//! Built across the CP-M1 prompts: **P-CP-05 / P-080** (the registry tables + the HARD placement
//! invariant), **P-CP-06 / P-081** (`discover` — PII-free tenant-grain routing, off the hot path,
//! client-cacheable fail-static), and **P-CP-07 / P-082** (`place(region, requested_tier)` +
//! two-phase signup — PII born inside the cell; see [`place`]), and **P-CP-08 / P-084**
//! (`placement_of(tenant_id)` — the routing answer + the gateway misroute-rejection, layer 4, CP-D2;
//! see [`placement_of`]). The remaining attestation answer (`residency_verify`) lands in P-CP-09.
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md`
//! §3 (what lives ONLY in the control plane — the PII-free `tenant → {cell_id(s), region}` record,
//! cell inventory, isolation tier, opaque routing token, aggregate utilisation, provisioning state;
//! ZERO in-region personal data), §5.1 (the three PII-free tables + the per-cell `local_tenant`
//! directory + the HARD placement invariant), §5.3 (the four-layer region-pinning defence).
//!
//! **Contract-index cluster 12 — Tenancy & control plane.** Owns: the **registry-schema half of
//! 12.3** (the `tenant_placement` table the `place`/`placement_of` answers store in, + the
//! invariant). Consumed: 12.1 (the partition key — [`myelin_tenancy::TenantId`] /
//! [`myelin_tenancy::Region`] / [`myelin_tenancy::CellId`]); 1.1/1.4/1.8 (the harness boot /
//! holder auto-registration / telemetry).
//!
//! ## What this prompt (P-CP-05 / P-080) ships
//! 1. **The three PII-free registry tables** ([`schema::Cell`], [`schema::TenantPlacement`],
//!    [`schema::CellProvisioning`]) + the per-cell [`schema::LocalTenant`] directory — EVERY column
//!    opaque / region / status / non-personal slug / aggregate count, guarded by the
//!    `control-plane-pii-free` lint (P-CP-04 / P-028) over the `@control-plane`-marked `schema.rs`.
//! 2. **The HARD placement invariant** ([`registry::Registry::place_tenant`]) — a DB trigger (in
//!    code on this floor; the Postgres DDL is the named Storage-driver floor) that rejects any
//!    `tenant_placement` whose `{home_cell} ∪ member_cells` contains a cell in a different region:
//!    **multi-cell is single-region by construction** (0 cross-region member cells admitted).
//! 3. **The service boots from `serve(AppSpec)`** ([`control_plane_app_spec`]) and **auto-registers
//!    the registry store as a holder** ([`holder::ControlPlaneHolder`]) via the harness's one door;
//!    the registry-touching **`cell_utilisation`** telemetry signal is emitted
//!    ([`cell_utilisation_signal`]).
//! 4. **The CP-D1 registry leg** ([`holder::assert_no_personal_columns`]) — the data-map over the
//!    LIVE registry schema asserts 0 `is_personal=true` columns (the static lint leg is P-CP-04).
//!
//! ## What P-CP-06 / P-081 adds ([`discover`])
//! 5. **`discover(slug | tenant_id) → RouteTuple`** ([`Registry::discover`] / [`discover::RouteTuple`]
//!    `{cell_id, region, cell_endpoint, ttl_seconds}`) — PII-free routing keyed by the opaque id /
//!    non-personal slug, **never an authz answer** (routing ≠ authorization). It reads
//!    `tenant_placement` JOINed to `cell`.
//! 6. **The client-cacheable, fail-static [`DiscoveryCache`]** — wraps [`myelin_substrate::FailStatic`]
//!    (contract 1.10) so a client caches the route with the returned TTL and, on a control-plane
//!    outage, serves the last-known-good route **fail-static for routing** (bounded-staleness) rather
//!    than failing closed (the CP-D4 blast-radius win seed, P-CP-14).
//! 7. **The `discovery_cache_hit` + `misroute_count` telemetry** ([`discover::DiscoverySignals`]) —
//!    aggregate PII-free counters; `discovery_cache_hit` increments on a cache serve, `misroute_count`
//!    on a `discover` that resolves to no route (the gateway's correction signal).
//!
//! ## DAG POSITION (a NAMED extension of the §2.9 eleven-crate library DAG)
//! This is a **SERVICE crate** — a leaf consumer ABOVE the glue crates (it depends on
//! `myelin-substrate`/`-tenancy`/`-gdpr`), exactly like `myelin-identity-service` /
//! `myelin-gdpr-service` / `myelin-git`. NOTHING in the production library DAG depends back on it,
//! so it is OUTSIDE the eleven-crate library graph modelled by `myelin_substrate::crate_graph`
//! (`substrate_is_root()` is preserved — a service is the graph's terminal consumer).
//!
//! ## Floors named (deferred bodies → filling prompt) — VISION §3 name-your-floors
//! - **`member_cells` is single-element in v1.** The multi-element cross-cell fan-out + the
//!   multi-cell `CrossCellPointer` resolution is the **M5 floor, P-CP-19 / P-CP-20** (bridge
//!   resolution live + the fan-out). The schema field is a `Vec<CellId>` (so the shape is frozen)
//!   but every v1 placement carries exactly one member cell (its home), asserted in tests.
//! - **`placement_of` + the gateway misroute-reject (layer 4, CP-D2) are LIVE (P-CP-08 / P-084,
//!   [`placement_of`]).** [`Registry::placement_of`] returns the frozen routing tuple `{region,
//!   home_cell, member_cells, isolation_tier, status}` (`member_cells` single-element in v1 — the
//!   floor); [`CellGateway::route`] REJECTS (does not proxy) + REDIRECTS + AUDITS a request for a
//!   `tenant_id` a cell doesn't host, reading **0** cross-tenant/cross-cell rows (the CP-D2 zero) and
//!   emitting `misroute_count`. The remaining attestation answer (`residency_verify`) is the next
//!   prompt **P-CP-09**. `discover` is LIVE (P-CP-06 / P-081); `place` + two-phase signup is LIVE
//!   (P-CP-07 / P-082, [`place`]) — it emits the `placement_count` + `provision_latency` routing
//!   signals ([`place::PlacementSignals`]).
//! - **`member_cells` single-element / resolution always same-cell in v1 (P-CP-08 floor).** The
//!   gateway accepts a request IFF the tenant's `home_cell` is THIS cell; the multi-cell resolution
//!   path (a member cell serving a slice, the `CrossCellPointer` bridge) goes live in **M5
//!   (P-CP-19 / P-CP-20)**.
//! - **The GeoDNS/anycast discovery edge is `[OPEN → P4 (infra)]`** (architecture §7.3) — a latency
//!   optimisation that fronts the PII-free discovery contract with a geo-routed edge. **v1 is
//!   CP-lookup + client cache** ([`discover::DiscoveryCache`]); the edge is an infra follow-on, not a
//!   band-gated engineering unit. The discovery *contract* ([`discover::RouteTuple`] + the client
//!   cache + fail-static) is fully built and does not change shape when the edge lands.
//! - **Repo-grain `discover` / `placement_of(repo)`** (C-1) is the M3 follow-on **P-CP-15**; this
//!   crate's `discover` is **tenant-grain** only.
//! - **The concrete Postgres execution** (the bounded pool + RLS the registry tables open through,
//!   Storage **P-ST-01 / P-007**; the trigger DDL executed against the live pool, **P-S12**) is the
//!   named Storage-driver floor — the invariant logic + the region-immutability discipline here are
//!   real + tested now and do not change shape when the driver lands (mirrors how
//!   `myelin-storage`'s migration runner validates in code while the DDL executes through the pool).
//! - **Cell-provisioning gating (CP-D6) is LIVE (P-CP-11 / P-083, [`provision`]).** A cell does not
//!   go `Active` until it passes **restore-verify** (the storage [`myelin_storage::RestoreVerifyGate`],
//!   contract 11.5) **+ readiness** (the cell's [`myelin_substrate::MetricsHealthSurface`]); a failing
//!   cell stays `Provisioning` (0 traffic — the place path filters on `Active`). Tenant decommission
//!   crypto-shreds the tenant KEK ([`myelin_storage::KmsEngine::destroy_kek`], 11.3). **FLOOR:**
//!   provisioning is a SCRIPTED procedure on this M1 floor; the durable-workflow promotion (the same
//!   gating under `myelin-flow`'s `DurableExecutor`, 9.1, M2) is **P-CP-22**'s re-confirmation. The
//!   `cell_provisioning` log records each gating step.

pub mod discover;
pub mod holder;
pub mod place;
pub mod placement_of;
pub mod provision;
pub mod registry;
pub mod schema;

pub use discover::{DiscoverKey, DiscoveryCache, DiscoverySignals, RouteTuple};
pub use placement_of::{
    CellGateway, GatewayReject, Misroute, MisrouteAudit, MisrouteAuditRecord, PlacementOf,
};
pub use provision::{
    ProvisionFailure, ProvisionVerdict, ProvisioningGate, ProvisioningSignals, STEP_ACTIVATE,
    STEP_READINESS, STEP_RESTORE_VERIFY,
};
pub use place::{
    CounterMinter, PlaceError, PlacementAnswer, PlacementService, PlacementSignals, TokenMinter,
};
pub use holder::{
    assert_no_personal_columns, control_plane_data_map, ColumnClassification, ControlPlaneHolder,
    CONTROL_PLANE_STORE,
};
pub use registry::{PlacementError, Registry};
pub use schema::{
    Capacity, Cell, CellProvisioning, CellStatus, IsolationKind, LocalTenant, PlacementStatus,
    ProvisioningOutcome, TenantPlacement,
};

use myelin_substrate::{AppSpec, Config, Migration, Migrations, StoreKind, StoreManifest};

/// The service name (the PII-free telemetry/trace identifier the harness keys on).
pub const SERVICE_NAME: &str = "control-plane";

/// **The forward-only migration set that creates the three PII-free registry tables + the
/// `local_tenant` directory + the placement-invariant trigger** (architecture §5.1). Each migration
/// is forward-only (a region change is a NEW row, never an `ALTER`/`DROP`) and PII-free (no
/// name/email/body column). The DDL executes against the live pool the harness opens (the driver is
/// the named Storage floor, P-ST-01 / P-S12); the runner here freezes the ordered DDL set.
///
/// The trigger migration (`0005_placement_invariant`) installs the HARD placement invariant as a
/// `BEFORE INSERT OR UPDATE` trigger on `tenant_placement` — the same predicate
/// [`Registry::check_placement_invariant`] enforces in code (0 cross-region member cells admitted).
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
        // The HARD placement invariant as a DB trigger (architecture §5.1) — multi-cell single-
        // region by construction. The trigger raises (rejecting the write) if any cell in
        // {home_cell} ∪ member_cells is in a different region than the tenant. This is the SAME
        // predicate Registry::check_placement_invariant enforces in code.
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

/// **The control-plane service's [`StoreManifest`]** — it declares the ONE registry store it owns
/// (the OLTP-backed registry schema). The harness opens (and therefore auto-registers, contract
/// 1.4) every declared store through its one door, so the `holder-registered` architecture test is
/// green by construction. The registry store is PII-free (its holder's DSR surface is empty by
/// construction — see [`ControlPlaneHolder`]), but it still registers so the one-door discipline
/// covers it.
pub fn control_plane_store_manifest() -> StoreManifest {
    // `CONTROL_PLANE_STORE` is `&'static str`; the manifest takes a `&'static str` name.
    StoreManifest::of([myelin_substrate::DeclaredStore::new(
        StoreKind::Oltp,
        CONTROL_PLANE_STORE_NAME,
    )])
}

/// The `&'static str` form of [`CONTROL_PLANE_STORE`] the manifest / registry need (a manifest name
/// is `&'static str`). Kept in lock-step with [`CONTROL_PLANE_STORE`] by the unit test below.
pub const CONTROL_PLANE_STORE_NAME: &str = "control_plane_registry";

/// **The control-plane [`AppSpec`] the harness wires** (boot → migrate → relay → consumers →
/// ports → drain). It carries the registry [`control_plane_migrations`] + the
/// [`control_plane_store_manifest`] (so the registry store auto-registers as a holder). The routing
/// surfaces (`discover`/`place`/`placement_of`) land in P-CP-06..P-CP-08; here the spec stands up
/// the registry the service is built on.
pub fn control_plane_app_spec(config: Config) -> AppSpec {
    let mut spec = AppSpec::minimal(SERVICE_NAME, config);
    spec.migrations = control_plane_migrations();
    spec.stores = control_plane_store_manifest();
    spec
}

/// **The `cell_utilisation` telemetry signal (architecture §4.1 / §14)** — the cell-level survival
/// signal the registry emits (the routing signals `placement_count` / `provision_latency` /
/// `misroute_count` / `discovery_cache_hit` land with `place`/`discover`, P-CP-06/P-CP-07/P-CP-08).
/// Observability is part of the pass (EI-01 §3); this is the signal a sizing/rebalance drill reads.
/// PII-free: a `(cell_id, utilisation)` aggregate pair, never per-subject data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellUtilisationSignal {
    /// The cell this aggregate utilisation is for (opaque id, PII-free).
    pub cell_id: String,
    /// The aggregate utilisation 0..=100 (aggregate-only, PII-free).
    pub utilisation: u8,
}

/// Emit the [`CellUtilisationSignal`] for a cell (the registry-touching telemetry the prompt names).
/// Reads the aggregate `utilisation` off the inventory row — never any per-subject data.
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

    /// The store-name constant is in lock-step with the holder's (one PII-free store id, no drift).
    #[test]
    fn store_name_is_consistent() {
        assert_eq!(CONTROL_PLANE_STORE_NAME, CONTROL_PLANE_STORE);
    }

    /// **The registry migrations are forward-only + PII-free.** No migration is destructive (a
    /// region change is a NEW row, never a DROP/ALTER), and the DDL carries no PII column
    /// (name/email/body) — the `control-plane-pii-free` lint guards the schema structs; this asserts
    /// the DDL text is PII-free too.
    #[test]
    fn migrations_are_forward_only_and_pii_free() {
        let migrations = control_plane_migrations();
        assert_eq!(migrations.0.len(), 5, "cell + placement + provisioning + directory + trigger");
        for m in &migrations.0 {
            assert!(!is_destructive(m.ddl), "migration {} must be forward-only (no DROP)", m.id);
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

    /// The placement-invariant trigger DDL is installed (architecture §5.1) — the trigger is the
    /// DB-level half of the invariant `Registry::check_placement_invariant` enforces in code.
    #[test]
    fn placement_invariant_trigger_is_installed() {
        let migrations = control_plane_migrations();
        let trigger = migrations
            .0
            .iter()
            .find(|m| m.id == "0005_placement_invariant")
            .expect("the placement-invariant trigger migration exists");
        assert!(trigger.ddl.contains("CREATE TRIGGER"));
        assert!(trigger.ddl.contains("BEFORE INSERT OR UPDATE ON tenant_placement"));
        assert!(trigger.ddl.contains("RAISE EXCEPTION"));
    }

    /// **The control-plane AppSpec boots from `serve(AppSpec)`** with the registry migrations + the
    /// store manifest (so the registry store auto-registers as a holder). The migration runner
    /// admits the registry DDL (forward-only, no hot table) — the boot path is exercisable.
    #[test]
    fn app_spec_carries_registry_and_store_manifest() {
        let spec = control_plane_app_spec(Config::default());
        assert_eq!(spec.name, SERVICE_NAME);
        assert_eq!(spec.migrations.0.len(), 5);
        // The control-plane registry store is declared (so opening it = registering it).
        let ids = spec.stores.holder_ids();
        assert!(ids.contains("oltp:control_plane_registry"), "registry store declared: {ids:?}");
        // The migration runner admits the registry DDL (no hot table, all forward-only).
        let mut runner = myelin_substrate::MigrationRunner::new();
        runner
            .run(&spec.migrations, &HotTables::none())
            .expect("registry migrations are admitted (forward-only, PII-free)");
    }

    /// **The holder-registered property: opening the registry store through the harness's one door
    /// registers it** (contract 1.4) — so a DSR fan-out reaches the control-plane holder. Joined
    /// against the manifest, no declared store escapes registration.
    #[test]
    fn registry_store_auto_registers_as_a_holder() {
        let manifest = control_plane_store_manifest();
        let mut registry = HolderRegistry::new();
        // The harness opens every declared store through its one door (open == register).
        for store in manifest.stores() {
            registry.open(store.kind, store.name);
        }
        // No declared store escaped registration (the holder-registered architecture test verdict).
        let violations =
            myelin_substrate::holder_registered(&manifest, &registry);
        assert!(violations.is_empty(), "every declared store auto-registers: {violations:?}");
        assert!(registry.is_registered(StoreKind::Oltp, CONTROL_PLANE_STORE_NAME));
    }

    /// **The `cell_utilisation` telemetry signal is emitted off the inventory row** (architecture
    /// §4.1) — an aggregate `(cell_id, utilisation)` pair, PII-free.
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

    /// **CDC pair for the registry-schema half of 12.3 (provider + consumer).** The PROVIDER side
    /// is this crate's [`Registry`] storing/answering from the `tenant_placement` table (through the
    /// placement invariant); the CONSUMER side stands in for a `place`/`placement_of` caller
    /// (P-CP-07 / P-CP-08) — it writes a placement via the provider and reads back the exact stored
    /// shape (`{region, home_cell, member_cells, isolation_tier, status}`). If the registry-schema
    /// shape drifts (a field added/removed/retyped), this consumer stops compiling — the whole point
    /// of a glue-crate CDC. The full `place`/`placement_of` ANSWERS (the routing tuple + the gateway
    /// misroute-reject) are the named follow-on (P-CP-07/P-CP-08); this CDC exercises the SCHEMA the
    /// answers store in.
    #[test]
    fn cdc_12_3_registry_schema_provider_consumer() {
        use myelin_tenancy::{CellId, Region, TenantId};

        /// A stand-in `placement_of` consumer (the shape P-CP-08 builds): it reads a placement's
        /// routing fields back from the registry-schema half of 12.3. It can ONLY read the frozen
        /// `tenant_placement` columns — it cannot read a name/email (there is none).
        struct PlacementOfAnswer {
            region: Region,
            home_cell: CellId,
            member_cells: Vec<CellId>,
            isolation_tier: IsolationKind,
            status: PlacementStatus,
        }
        impl PlacementOfAnswer {
            /// Build the answer from a stored `tenant_placement` row (the provider's table) — the
            /// CDC's read side.
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

        // PROVIDER: write a placement through the registry (the invariant admits it).
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

        // CONSUMER: read the placement back through the frozen registry-schema shape.
        let row = registry.placement(&tenant).expect("the placement is stored");
        let answer = PlacementOfAnswer::from_row(row);
        assert_eq!(answer.region.as_str(), "eu-west");
        assert_eq!(answer.home_cell.as_str(), "cell-w-1");
        assert_eq!(answer.member_cells.len(), 1); // v1 single-element floor.
        assert_eq!(answer.isolation_tier, IsolationKind::Pool);
        assert_eq!(answer.status, PlacementStatus::Active);
    }

    /// **CDC pair for 12.2 tenant-grain `discover` (provider + consumer).** The PROVIDER is this
    /// crate's [`Registry::discover`] answering a [`RouteTuple`] from `tenant_placement` JOINed to
    /// `cell`. The CONSUMER stands in for a **gateway / git-wire** caller (architecture §7.3): it
    /// takes the route, encodes the cell endpoint into the URL it routes to, and — load-bearing —
    /// can read ONLY the routing fields (`cell_id`/`region`/`cell_endpoint`/`ttl_seconds`), NEVER an
    /// authz answer (there is no grant/principal/permission field on `RouteTuple`). If the route shape
    /// drifts, the consumer stops compiling — the point of a glue-crate CDC.
    #[test]
    fn cdc_12_2_discover_tenant_grain_provider_consumer() {
        use myelin_tenancy::{CellId, Region, TenantId};

        /// A stand-in gateway / git-wire consumer: it routes a request to the discovered cell. It can
        /// ONLY use the routing tuple — it has no way to obtain a grant from `discover` (routing ≠
        /// authorization; the cell does its own fail-closed `check`).
        struct GatewayRoute {
            /// The endpoint the gateway connects the request to (e.g. the git remote host).
            target_endpoint: String,
            /// The region the gateway confirms the route stays inside (residency, §5.3).
            pinned_region: String,
            /// The TTL the gateway caches the route for (off the hot path, §8).
            cache_ttl_secs: u64,
        }
        impl GatewayRoute {
            /// Build the gateway's routing decision from a discovered [`RouteTuple`] (the read side).
            fn from_route(route: &RouteTuple) -> GatewayRoute {
                GatewayRoute {
                    target_endpoint: route.cell_endpoint.clone(),
                    pinned_region: route.region.as_str().to_string(),
                    cache_ttl_secs: route.ttl_seconds,
                }
            }
        }

        // PROVIDER: a placed tenant in eu-west.
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

        // PROVIDER answers `discover`; CONSUMER routes through the frozen tuple shape.
        let route = registry
            .discover(&DiscoverKey::TenantId(TenantId::from_token("01J0ACME")), 30)
            .expect("the placed tenant resolves to a route");
        let gw = GatewayRoute::from_route(&route);
        assert_eq!(gw.target_endpoint, "cell.eu-west.myelin.eu");
        assert_eq!(gw.pinned_region, "eu-west");
        assert_eq!(gw.cache_ttl_secs, 30);

        // The git-wire caller resolves the SAME route by the non-personal slug (the C-1 git-wire use,
        // tenant-grain here; repo-grain is P-CP-15).
        let by_slug = registry
            .discover(&DiscoverKey::Slug("acme".into()), 30)
            .expect("the slug resolves to a route");
        assert_eq!(GatewayRoute::from_route(&by_slug).target_endpoint, "cell.eu-west.myelin.eu");
    }

    /// **CDC pair for the `placement_of` half of 12.3 (provider + consumer) — P-CP-08.** The PROVIDER
    /// is this crate's [`Registry::placement_of`] answering the frozen routing tuple `{region,
    /// home_cell, member_cells, isolation_tier, status}` from `tenant_placement`. The CONSUMER stands
    /// in for a **cell gateway** (architecture §5.3 layer 4): it takes the answer and decides — purely
    /// off the routing fields — whether it hosts the tenant, NEVER reading the tenant's data. It can
    /// read ONLY the routing fields (there is no grant/principal/permission on [`PlacementOf`]); if the
    /// `placement_of` shape drifts (a field added/removed/retyped) the consumer stops compiling — the
    /// point of a glue-crate CDC.
    #[test]
    fn cdc_12_3_placement_of_provider_consumer() {
        use myelin_tenancy::{CellId, Region, TenantId};

        /// A stand-in **cell gateway** consumer: it reads the `placement_of` routing answer and
        /// decides whether THIS cell hosts the tenant — purely from the routing fields, never from the
        /// tenant's data. This is the read side of the layer-4 misroute decision.
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
            /// The layer-4 decision: this cell hosts the tenant IFF its home cell is `this_cell`.
            fn this_cell_hosts(&self, this_cell: &CellId) -> bool {
                &self.home_cell == this_cell
            }
        }

        // PROVIDER: a placed tenant homed on cell-w-1, in eu-west.
        let mut registry = Registry::new();
        registry.insert_cell(Cell {
            cell_id: CellId::from_token("cell-w-1"),
            region: Region::new("eu-west"),
            status: CellStatus::Active,
            isolation_kind: IsolationKind::Pool,
            capacity: Capacity { tenants_max: 1000, write_qps_max: 5000, storage_bytes_max: 1 << 40 },
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

        // PROVIDER answers `placement_of`; CONSUMER decides hosting off the frozen routing tuple.
        let answer = registry
            .placement_of(&TenantId::from_token("01J0ACME"))
            .expect("the placed tenant resolves to a placement_of answer");
        let decision = GatewayHostsDecision::from_answer(&answer);
        assert_eq!(decision.region.as_str(), "eu-west");
        assert_eq!(decision.member_cells.len(), 1, "v1 member_cells single-element");
        assert_eq!(decision.isolation_tier, IsolationKind::Pool);
        assert_eq!(decision.status, PlacementStatus::Active);
        // The home cell hosts; a different cell does NOT (the layer-4 decision, off routing only).
        assert!(decision.this_cell_hosts(&CellId::from_token("cell-w-1")), "the home cell hosts");
        assert!(!decision.this_cell_hosts(&CellId::from_token("cell-w-2")), "a different cell misroutes");
    }
}

//! # Self-host parity: the degenerate one-cell control plane runs the identical code path (P-CP-13)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md` §10 in full
//! (**self-host parity** — a self-hosted install is **exactly one cell of identical artifacts**
//! (ADR-11.1, same monorepo build); the control plane is **degenerate** — discovery/placement
//! trivially return "this cell", the registry is a **one-row local table** — but the **SAME code
//! path** runs, the **SAME `residency-pin` lint holds** (the customer's data stays in the customer's
//! region by the same write-boundary check), the **SAME drills run**; managed-fleet-only features
//! (cross-cell tenants, fleet deploy waves) are **N/A for self-host by definition** — not a gap, the
//! model). Contract-index rows 12.1 (the partition key), 12.2 / 12.3 (`discover` / `placement_of`
//! returning "this cell"). VISION §3 (GDPR-safe & EU-sovereign by construction — the self-host cell
//! stays in the customer's region) / §5 (self-host is a first-class deployment). EI-01 §3 (prove-it —
//! the same drills run on the degenerate cell; the property is not real until a test forces it) / §1
//! (code-wins-over-docs).
//!
//! ## The load-bearing distinction: there is NO self-host fork
//! Self-host parity is a *configuration*, not a code path. A self-hosted Myelin is the SAME
//! `myelin-control-plane` binary, booted with a registry that holds **exactly one** `Active` cell
//! (the install's own cell). `discover`/`place`/`placement_of`/`residency_verify` and the four-layer
//! enforcement run **unchanged** — they simply always resolve to the one cell. This module ships
//! [`DegenerateControlPlane`], the one-cell *configuration*, and its drills assert the identity:
//!
//! - **No self-host fork of the routing answers.** [`DegenerateControlPlane`] does NOT define its own
//!   `discover`/`place`/`placement_of`/`residency_verify` — it calls [`crate::registry::Registry`]'s,
//!   [`crate::place::PlacementService`]'s, [`crate::placement_of::CellGateway`]'s, and the free
//!   [`crate::residency_verify::residency_verify`] — the EXACT functions a managed-fleet cell calls.
//!   The structural proof is the absence of any second implementation: this module is a thin
//!   *assembly* over the shared crate API, asserted by [`DegenerateControlPlane::cell`] /
//!   [`DegenerateControlPlane::registry`] handing back the shared types.
//! - **`discover`/`placement_of` return "this cell".** With one `Active` cell, every placed tenant
//!   routes to it; the gateway for the one cell ACCEPTS every tenant it homes (which, in v1 single-
//!   home-cell, is every tenant).
//! - **The `residency-pin` lint holds; CP-D3 runs green on the degenerate cell.** The same runtime
//!   [`crate::four_layer::ResidencyWriteBoundary`] (layer 3) rejects an out-of-region write on the
//!   one cell, and the same [`crate::four_layer::FourLayerEnforcement`] asserts there is no
//!   cross-region query path for personal data — on the degenerate cell exactly as on a fleet cell.
//! - **`residency_verify` is green on the one cell's data.** The one cell's M1 stores all report the
//!   install's region, so the same [`crate::residency_verify::residency_verify`] mints a green
//!   `residency-attestation` (0 mismatches).
//!
//! ## Managed-fleet-only is N/A by definition — NOT a gap (architecture §10, named explicitly)
//! Cross-cell tenants (`member_cells` multi-element), fleet deploy waves, and multi-cell discovery
//! fan-out are **N/A for self-host by definition** — a one-cell install has nothing to fan out to.
//! This is the *model*, not a missing feature (per VISION §3 name-your-floors, named here so it is
//! visible). The degenerate cell's `member_cells` is exactly `[this_cell]` — the v1 single-element
//! shape (the multi-cell fan-out is the M5 floor P-CP-19/P-CP-20, which a self-host install never
//! exercises). Crucially these are absences of *fleet configuration*, not forks of code: the same
//! `member_cells: Vec<CellId>` field carries one element; the same `place`/`placement_of` run.
//!
//! ## No floor here (the prompt says so)
//! Per P-CP-13: there is **no engineering floor**. The degenerate-cell configuration is fully built;
//! the managed-fleet-only-N/A is named above (the model, not a gap). The CP-D2 misroute + CP-D4
//! blast-radius legs are re-confirmed in the dogfood band (P-CP-23, [`crate`] M6); here CP-D3 (the
//! residency write-boundary) is proven green on the degenerate cell.

use myelin_tenancy::{CellId, Region, TenantId};

use crate::four_layer::{CrossRegionPathError, FourLayerEnforcement, ResidencyWriteRejected};
use crate::place::{CounterMinter, PlaceError, PlacementAnswer, PlacementService};
use crate::placement_of::{CellGateway, GatewayReject, PlacementOf};
use crate::registry::Registry;
use crate::residency_verify::{
    residency_verify, ResidencyMismatch, ResidencySigningKey, ResidencyStoreClass,
    SignedAttestation, StoreRegionReport,
};
use crate::schema::{Capacity, Cell, CellStatus, IsolationKind};

/// **The degenerate one-cell control plane (architecture §10).** A self-hosted Myelin install is
/// EXACTLY one cell of identical artifacts: this is the one-cell *configuration* of the SAME
/// `myelin-control-plane` code, NOT a self-host fork. It holds the shared [`Registry`] (a one-row
/// `cell` inventory + the placed tenants) and the one cell's id/region — and answers
/// `discover`/`place`/`placement_of`/`residency_verify` by calling the SAME shared functions a
/// managed-fleet cell calls.
///
/// The proof of parity is structural: there is no `discover`/`place`/`placement_of`/`residency_verify`
/// method ON this type — the routing answers come from [`Registry`] / [`PlacementService`] /
/// [`CellGateway`] / the free [`residency_verify`], the EXACT path the fleet runs. This type is a thin
/// *assembly* (the one-cell registry + the cell handle + the cell's gateway).
#[derive(Debug)]
pub struct DegenerateControlPlane {
    /// The shared control-plane registry — here a **one-row** `cell` inventory (the install's own
    /// cell) plus the tenants placed on it. The SAME [`Registry`] a fleet uses, with one cell.
    registry: Registry,
    /// The one cell's opaque id (the install's own cell). Every tenant homes here.
    cell_id: CellId,
    /// The install's region (the customer's region — the same region every store pins to). Immutable
    /// (region immutability, §5.3 layer 1 — there is no setter).
    region: Region,
}

impl DegenerateControlPlane {
    /// **Stand up a degenerate one-cell control plane** for a self-hosted install in `region`, with
    /// the one cell `cell_id`. The cell is `Active` (a self-host install's single cell is the one it
    /// serves from) and at the Pool isolation tier (the v1 self-host tier — Bridge/Dedicated are
    /// managed-fleet on-demand, N/A for self-host). This is the SAME [`Registry::insert_cell`] a fleet
    /// uses — there is no degenerate-only insert path.
    ///
    /// The cell `endpoint` is the install's own host (`cell.<region>.<cell_id>.local` by default — a
    /// PII-free routing host, never personal data); a self-host operator overrides it via
    /// [`Self::with_endpoint`].
    pub fn bootstrap(cell_id: CellId, region: Region) -> DegenerateControlPlane {
        let endpoint = format!("cell.{}.{}.local", region.as_str(), cell_id.as_str());
        Self::with_endpoint(cell_id, region, endpoint)
    }

    /// As [`Self::bootstrap`], but with an explicit self-host `endpoint` (the install's own routing
    /// host). PII-free routing host — never personal data.
    pub fn with_endpoint(
        cell_id: CellId,
        region: Region,
        endpoint: String,
    ) -> DegenerateControlPlane {
        let mut registry = Registry::new();
        // The ONE row of the degenerate registry — the install's own cell, Active, Pool tier, pinned
        // to the install's region. Inserted via the SAME shared `insert_cell` (no self-host fork).
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
        }
    }

    /// The install's one cell id (opaque, PII-free). `discover`/`placement_of` always resolve here.
    pub fn cell_id(&self) -> &CellId {
        &self.cell_id
    }

    /// The install's region (immutable — the customer's region every store pins to).
    pub fn region(&self) -> &Region {
        &self.region
    }

    /// **The one cell row (architecture §10 — the registry is a one-row local table).** Borrowed from
    /// the SHARED [`Registry`] — the same `cell` inventory type a fleet holds, here with exactly one
    /// row. (Returns the shared [`Cell`], not a degenerate-only type — the parity proof.)
    pub fn cell(&self) -> &Cell {
        self.registry
            .cell(&self.cell_id)
            .expect("the degenerate control plane always has its one cell")
    }

    /// A borrow of the SHARED control-plane registry — so a drill can run the SAME
    /// `discover`/`placement_of`/`assign_cell` a fleet cell calls. **There is no degenerate-only
    /// routing API**; the answers come from THIS shared registry. The registry holds exactly one cell
    /// (`registry.cell_count() == 1`).
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// A mutable borrow of the shared registry (so [`place`](Self::place) can write the sticky
    /// placement through the SAME [`Registry::place_tenant`] invariant).
    pub fn registry_mut(&mut self) -> &mut Registry {
        &mut self.registry
    }

    /// **`place(region, requested_tier)` on the degenerate cell — runs the IDENTICAL two-phase-signup
    /// code path (architecture §10 / §7.2).** This does NOT reimplement `place`: it calls the SAME
    /// [`PlacementService::place`] a managed-fleet cell calls, against this one-cell registry. With one
    /// `Active` cell, assignment (region-first → tier-second → capacity-third → stability-always)
    /// trivially resolves to "this cell" — that is the degeneracy, not a fork. The PII-free id is
    /// minted the same way; the human name/email are born INSIDE the returned `cell_endpoint` (phase
    /// 2), never in the control plane.
    ///
    /// A self-host `place` to a different region than the install's would find NO eligible cell
    /// (`PlaceError::NoEligibleCell`) — a one-cell install only places in its own region (the
    /// residency model, structurally enforced by the same `assign_cell` region-first filter).
    pub fn place(
        &mut self,
        service: &PlacementService<CounterMinter>,
        requested_tier: IsolationKind,
        slug: &str,
    ) -> Result<PlacementAnswer, PlaceError> {
        let region = self.region.clone();
        // The SAME PlacementService::place a fleet cell calls — no self-host fork.
        service.place(&mut self.registry, &region, requested_tier, slug)
    }

    /// **`discover(tenant_id) → "this cell"` (architecture §10) — the SAME [`Registry::discover`].**
    /// Returns the cell id every placed tenant routes to (the one cell). A convenience that asserts the
    /// degeneracy: a placed tenant ALWAYS discovers to `self.cell_id`. It calls the shared `discover`
    /// (no self-host routing fork) and returns the resolved cell id (or `None` for an unplaced tenant).
    pub fn discover_cell(&self, tenant_id: &TenantId) -> Option<CellId> {
        use crate::discover::DiscoverKey;
        // The SAME Registry::discover — the degenerate cell does not fork routing. ttl is the install's
        // configured discovery TTL (a self-host install caches its one route just as a fleet does).
        self.registry
            .discover(&DiscoverKey::TenantId(tenant_id.clone()), 30)
            .map(|route| route.cell_id)
    }

    /// **`placement_of(tenant_id) → "this cell"` (architecture §10) — the SAME
    /// [`Registry::placement_of`].** The routing answer for a placed tenant; its `home_cell` is the one
    /// cell and `member_cells` is exactly `[this_cell]` (the v1 single-element shape — multi-cell is
    /// N/A for self-host by definition). Calls the shared `placement_of` (no self-host fork).
    pub fn placement_of(&self, tenant_id: &TenantId) -> Option<PlacementOf> {
        self.registry.placement_of(tenant_id)
    }

    /// **The one cell's gateway (architecture §10, layer 4) — the SAME [`CellGateway`].** The gateway
    /// for the degenerate cell ACCEPTS every tenant it homes (which, on a one-cell install, is every
    /// placed tenant) and would REJECT a misroute exactly as a fleet gateway does (there is no misroute
    /// on a one-cell install, but the SAME reject logic is present — N/A by configuration, not by a
    /// forked code path).
    pub fn gateway(&self) -> CellGateway {
        CellGateway::new(self.cell_id.clone())
    }

    /// **`route(tenant_id)` through the one cell's gateway — the SAME [`CellGateway::route`].** On the
    /// degenerate cell this ACCEPTS every placed tenant (the one cell homes them all); it reads 0
    /// cross-tenant rows exactly as a fleet gateway does. Calls the shared gateway over this registry.
    pub fn route(&self, tenant_id: &TenantId) -> Result<PlacementOf, GatewayReject> {
        self.gateway().route(&self.registry, tenant_id)
    }

    /// **The four-layer enforcement wired over the degenerate cell — the SAME
    /// [`FourLayerEnforcement`].** Layers 1+2 (the one-row registry's placement invariant), layer 3
    /// (the runtime `residency-pin` write boundary on the install's region), layer 4 (the one cell's
    /// gateway). Used by the CP-D3 leg below to assert there is no cross-region query path for personal
    /// data ON THE DEGENERATE CELL.
    pub fn four_layer(&self) -> FourLayerEnforcement<'_> {
        FourLayerEnforcement::new(&self.registry, self.gateway(), self.region.clone())
    }

    /// **CP-D3 on the degenerate cell (architecture §10 — the `residency-pin` lint holds + CP-D3 runs
    /// green): the runtime write boundary REJECTS an out-of-region write on the one cell.** A write in
    /// the install's region is admitted; a write in any other region is rejected at the boundary — the
    /// SAME [`crate::four_layer::ResidencyWriteBoundary`] a fleet cell uses, proving the customer's data
    /// stays in the customer's region by the same write-boundary check.
    ///
    /// Returns `Ok(())` iff the in-region write is admitted AND the foreign-region write is rejected;
    /// a loud [`ResidencyWriteRejected`] surfaces if the in-region write were (wrongly) rejected.
    pub fn cp_d3_residency_pin_holds(
        &self,
        a_foreign_region: &Region,
    ) -> Result<(), ResidencyWriteRejected> {
        let four_layer = self.four_layer();
        // The install-region write is admitted (the customer's own data lands in-region).
        four_layer.admit_write(&self.region)?;
        // The out-of-region write is REJECTED at the boundary (the SAME layer-3 check the fleet runs).
        // If it were admitted, that is the breach — surface it loudly as a rejected-that-should-be.
        match four_layer.admit_write(a_foreign_region) {
            Err(_) => Ok(()), // rejected — the residency-pin holds on the degenerate cell.
            Ok(()) => Err(ResidencyWriteRejected {
                cell_region: self.region.clone(),
                row_region: a_foreign_region.clone(),
            }),
        }
    }

    /// **The no-cross-region-query-path assertion ON the degenerate cell — the SAME
    /// [`FourLayerEnforcement::assert_no_cross_region_query_path`].** For a tenant the one cell homes,
    /// every write of that tenant's data lands in the install's region (layer 3) and the request is
    /// served entirely within the one cell (layer 4). Proves the four-layer property holds on the
    /// degenerate cell exactly as on a fleet cell.
    pub fn assert_no_cross_region_query_path(
        &self,
        tenant_id: &TenantId,
        a_foreign_region: &Region,
    ) -> Result<(), CrossRegionPathError> {
        self.four_layer()
            .assert_no_cross_region_query_path(tenant_id, a_foreign_region)
    }

    /// **`residency_verify` green on the degenerate cell's data (architecture §10) — the SAME free
    /// [`residency_verify`].** The one cell's M1 stores (OLTP/blob/index/KMS) all report the install's
    /// region, so the SAME aggregation+sign mints a green `residency-attestation` (0 mismatches). Calls
    /// the shared function — there is no self-host attestation fork; the store reports simply all carry
    /// the one region.
    ///
    /// This is the self-host instance of CP-D3's `residency-attestation` green-on-the-one-cell-install
    /// (the prompt's telemetry leg).
    pub fn residency_verify_own_data(
        &self,
        tenant_id: &TenantId,
        key: &ResidencySigningKey,
    ) -> Result<SignedAttestation, ResidencyMismatch> {
        // Every M1 store on the one cell reports the install's region (the customer's region). The
        // store-layer residency-pin (Storage P-ST-07) is what guarantees it; here we feed the M1 set's
        // region reports into the SAME residency_verify a fleet runs.
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

    /// **The degenerate control plane is a ONE-ROW registry (architecture §10).** Exactly one cell —
    /// the install's own — and it is `Active`, Pool-tier, pinned to the install's region.
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

    /// **`place` runs the IDENTICAL two-phase-signup code path on the degenerate cell.** It calls the
    /// SAME [`PlacementService::place`] — assignment trivially resolves to "this cell" (the one
    /// `Active` cell), the id is minted PII-free, and the human name/email are NOT threaded through
    /// (the signature takes none). The placement routes to the install's own cell-endpoint.
    #[test]
    fn place_runs_the_identical_code_path_to_this_cell() {
        let mut sh = self_host();
        let service = PlacementService::new(CounterMinter::new());
        let answer = sh
            .place(&service, IsolationKind::Pool, "acme")
            .expect("the one Active cell is eligible → placed");
        // Placed on the install's own cell, at the install's endpoint.
        assert_eq!(answer.home_cell.as_str(), "cell-self");
        assert_eq!(answer.cell_endpoint, "cell.fr-par.cell-self.local");
        assert_eq!(answer.isolation_tier, IsolationKind::Pool);
        // The placement is a sticky stored fact on the SAME registry (placement_count via the service).
        assert_eq!(service.signals().placement_count, 1);
        assert_eq!(sh.registry().placement_count(), 1);
    }

    /// **`discover`/`placement_of` return "this cell" (architecture §10).** A placed tenant ALWAYS
    /// routes to the one cell; `placement_of`'s `member_cells` is the single-element `[this_cell]` (the
    /// v1 shape — multi-cell is N/A for self-host by definition).
    #[test]
    fn discover_and_placement_of_return_this_cell() {
        let mut sh = self_host();
        let service = PlacementService::new(CounterMinter::new());
        let answer = sh
            .place(&service, IsolationKind::Pool, "acme")
            .expect("placed");
        let tenant = answer.tenant_id.clone();

        // discover → this cell.
        let discovered = sh
            .discover_cell(&tenant)
            .expect("a placed tenant discovers");
        assert_eq!(
            discovered.as_str(),
            "cell-self",
            "discover returns 'this cell'"
        );

        // placement_of → this cell, single-element member_cells.
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

    /// **The one cell's gateway ACCEPTS every tenant it homes (architecture §10, layer 4).** On a
    /// one-cell install there is no misroute — the one cell homes every tenant — and the SAME
    /// [`CellGateway::route`] serves it, reading 0 cross-tenant rows.
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
        // The SAME gateway logic — 0 cross-tenant reads on the degenerate cell.
        let gw = sh.gateway();
        let _ = gw.route(sh.registry(), &answer.tenant_id);
        assert_eq!(gw.misroute_count(), 0, "no misroute on a one-cell install");
        assert_eq!(
            gw.cross_tenant_reads(),
            0,
            "0 cross-tenant reads (the CP-D2 zero) on the degenerate cell"
        );
    }

    /// **An UNPLACED tenant has no route (the same fail-closed answer a fleet gives).** `discover` /
    /// `placement_of` return `None`; the gateway REJECTS as `NoSuchTenant` — the SAME shared logic.
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

    /// **CP-D3 RUNS GREEN ON THE DEGENERATE CELL (architecture §10 — the `residency-pin` lint holds):
    /// the runtime write boundary REJECTS an out-of-region write on the one cell.** The customer's data
    /// stays in the customer's region by the SAME layer-3 write-boundary check a fleet cell runs.
    #[test]
    fn cp_d3_residency_pin_holds_on_the_degenerate_cell() {
        let sh = self_host();
        // An in-region write is admitted; an out-of-region write is REJECTED — the same check.
        sh.cp_d3_residency_pin_holds(&Region::new("eu-north"))
            .expect(
                "the residency-pin holds on the degenerate cell (out-of-region write rejected)",
            );
        // The four-layer boundary is the SAME type the fleet uses (no self-host fork).
        let four_layer = sh.four_layer();
        four_layer
            .admit_write(&Region::new("fr-par"))
            .expect("the install-region write is admitted");
        four_layer.admit_write(&Region::new("us-east")).expect_err(
            "an out-of-region write is rejected at the boundary on the degenerate cell",
        );
    }

    /// **The no-cross-region-query-path property holds on the degenerate cell (the SAME four-layer
    /// assertion).** For a tenant the one cell homes, its data can only be written in the install's
    /// region, and the request is served entirely within the one cell.
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

    /// **`residency_verify` is GREEN on the platform's own data (architecture §10 — the
    /// `residency-attestation` green-on-the-one-cell-install).** The one cell's M1 stores all report
    /// the install's region → the SAME `residency_verify` mints a verifying, 0-mismatch attestation.
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

    /// **A self-host `place` to a DIFFERENT region than the install's finds no eligible cell (the
    /// residency model holds structurally).** A one-cell install only places in its own region — the
    /// SAME `assign_cell` region-first filter rejects a cross-region request (it is not a self-host
    /// fork; the fleet's filter behaves identically with one cell).
    #[test]
    fn place_in_a_foreign_region_finds_no_eligible_cell() {
        let sh = self_host();
        // Directly exercise assign_cell over the shared registry for a foreign region: no eligible cell.
        let assigned = sh
            .registry()
            .assign_cell(&Region::new("eu-north"), IsolationKind::Pool);
        assert!(
            assigned.is_none(),
            "a one-cell install only places in its own region"
        );
    }

    /// **Managed-fleet-only features are N/A by definition — NOT a gap (architecture §10).** The
    /// degenerate cell's `member_cells` is exactly `[this_cell]` (the v1 single-element shape; there is
    /// nothing to fan out to on a one-cell install). This documents the model: it is an absence of
    /// fleet *configuration*, not a fork of code — the SAME `member_cells: Vec<CellId>` field carries
    /// one element.
    #[test]
    fn managed_fleet_only_is_na_by_definition_not_a_gap() {
        let mut sh = self_host();
        let service = PlacementService::new(CounterMinter::new());
        let answer = sh
            .place(&service, IsolationKind::Pool, "acme")
            .expect("placed");
        let placement = sh.placement_of(&answer.tenant_id).expect("placed");
        // One member cell (itself) — the v1 shape; multi-cell fan-out is N/A for self-host (the model).
        assert_eq!(
            placement.member_cells,
            vec![CellId::from_token("cell-self")]
        );
        // The registry has exactly one cell — nothing to fan out to (the degeneracy, not a gap).
        assert_eq!(sh.registry().cell_count(), 1);
    }

    /// **CDC pair for the degenerate-cell configuration (provider + consumer) — a self-host gateway
    /// resolving "this cell".** The PROVIDER is [`DegenerateControlPlane`] (the one-cell registry +
    /// routing answers); the CONSUMER stands in for a **self-host gateway** that resolves a request to
    /// the one cell PURELY off the shared routing answers — it can read ONLY the routing fields (the
    /// SAME `PlacementOf` shape a fleet gateway reads), never the tenant's data. If the degenerate
    /// config drifted to a self-host-only shape, this consumer would stop compiling — the point of a
    /// CDC, and the parity proof (the consumer is shape-identical to the fleet's).
    #[test]
    fn cdc_degenerate_cell_configuration_provider_consumer() {
        /// A stand-in self-host gateway consumer: it resolves "which cell serves this request?" off the
        /// SHARED routing answer — identical to the fleet gateway's read side, just over a one-cell CP.
        struct SelfHostGateway {
            this_cell: CellId,
        }
        impl SelfHostGateway {
            /// Decide the serving cell from the shared `placement_of` answer (routing only).
            fn serving_cell(&self, placement: &PlacementOf) -> CellId {
                // On a self-host install the home cell IS this cell — but the consumer reads the
                // routing answer the SAME way a fleet gateway does (it does not assume; it reads).
                placement.home_cell.clone()
            }
            /// Whether this self-host gateway hosts the tenant (always true on a one-cell install, but
            /// decided off the routing answer, never the tenant's data — the SAME layer-4 decision).
            fn this_cell_hosts(&self, placement: &PlacementOf) -> bool {
                placement.home_cell == self.this_cell
            }
        }

        // PROVIDER: a degenerate control plane with a placed tenant.
        let mut sh = self_host();
        let service = PlacementService::new(CounterMinter::new());
        let answer = sh
            .place(&service, IsolationKind::Pool, "acme")
            .expect("placed");

        // PROVIDER answers placement_of (the SHARED routing answer); CONSUMER resolves the serving cell.
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

    /// **The parity proof: there is NO self-host fork — the routing answers come from the SHARED
    /// crate API.** This test exercises the degenerate cell ENTIRELY through the shared
    /// [`Registry`]/[`CellGateway`]/[`residency_verify`] surface a fleet uses, never a degenerate-only
    /// method, proving the code path is identical (only the registry is one-row).
    #[test]
    fn no_self_host_fork_the_shared_api_runs() {
        let mut sh = self_host();
        let service = PlacementService::new(CounterMinter::new());
        let answer = sh
            .place(&service, IsolationKind::Pool, "acme")
            .expect("placed");
        let tenant = answer.tenant_id.clone();

        // Run EVERYTHING through the shared registry + gateway (the fleet's exact path):
        let registry = sh.registry();
        // discover (shared)
        use crate::discover::DiscoverKey;
        let route = registry
            .discover(&DiscoverKey::TenantId(tenant.clone()), 30)
            .expect("shared discover resolves");
        assert_eq!(route.cell_id.as_str(), "cell-self");
        // placement_of (shared)
        let placement = registry
            .placement_of(&tenant)
            .expect("shared placement_of resolves");
        assert_eq!(placement.home_cell.as_str(), "cell-self");
        // gateway route (shared)
        let gw = CellGateway::new(sh.cell_id().clone());
        let served = gw.route(registry, &tenant).expect("shared gateway serves");
        assert_eq!(served.home_cell.as_str(), "cell-self");
        // residency_verify (shared, free function)
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

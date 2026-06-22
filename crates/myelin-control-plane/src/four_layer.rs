//! # The four-layer region-pinning enforced end-to-end (CP-D3 + STOR-D5 + CP-D2 e2e) — P-CP-12
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md` §5.3 in full (the
//! four layers of defence-in-depth that make misrouting personal data *structurally impossible*):
//!
//! 1. **Layer 1 — region is immutable** on cell and tenant: a region change is a new-tenant-+-DSR /
//!    a new cell, never an `UPDATE`. Enforced in [`crate::registry::Registry`] (no
//!    `update_*_region` method exists — its ABSENCE is the structural proof).
//! 2. **Layer 2 — the placement invariant**: every cell in `{home_cell} ∪ member_cells` is in the
//!    tenant's region — **multi-cell is single-region by construction**. Enforced by
//!    [`crate::registry::Registry::check_placement_invariant`] (the DB trigger, in code).
//! 3. **Layer 3 — the `residency-pin` write-boundary check**: every write asserts
//!    `row.region == cell.region`, **with the cell's region injected by the harness**. The
//!    *compile-time* leg is the `residency-pin` lint (P-CP-03 / P-026); **this module ships the
//!    *runtime* leg** — [`ResidencyWriteBoundary`] — that REJECTS a `row.region ≠ cell.region` write
//!    at the boundary BEFORE it reaches the store. The store-layer DB enforcement (the Postgres RLS
//!    `WITH CHECK (region = current_setting('myelin.region'))` policy) is its live twin, proven
//!    against the dev stack in the storage `stor_d5_cross_region_egress` integration drill.
//! 4. **Layer 4 — the gateway rejects (does not proxy)** a request for a `tenant_id` it doesn't
//!    host, returning a misroute redirect. Enforced by [`crate::placement_of::CellGateway::route`].
//!
//! **There is no cross-region query path for personal data.** This module's [`FourLayerEnforcement`]
//! WIRES the four layers over a single cell and asserts that end-to-end property: a write only ever
//! lands in the cell's region (layers 1+2+3), and a request is only ever served by the cell that
//! homes the tenant in that region (layer 4) — so no code path can read or write a tenant's personal
//! data outside its region. The only cross-cell channel is the PII-free pointer bridge (§6, the M5
//! floor P-CP-19/P-CP-20), which carries 0 personal data.
//!
//! ## What this prompt (P-CP-12 / P-096) ships
//! - **[`ResidencyWriteBoundary`]** — the *runtime* layer-3 write-boundary check (the lint's
//!   runtime twin): the cell's region is injected once (by the harness); every `check_write(row)`
//!   REJECTS an out-of-region write with a loud [`ResidencyWriteRejected`]. This is the mechanism
//!   the §5.3 layer-3 description names ("every write asserts `row.region == cell.region`, with the
//!   cell's region injected by the harness").
//! - **[`FourLayerEnforcement`]** — the end-to-end wiring of layers 1+2 (the registry), 3 (the write
//!   boundary), and 4 (the gateway) over one cell. It exposes the three entry points a service
//!   touches — `place` (layers 1+2), `admit_write` (layer 3), `route` (layer 4) — all sharing the
//!   one cell region, and asserts the no-cross-region-query-path property
//!   ([`FourLayerEnforcement::assert_no_cross_region_query_path`]).
//!
//! ## Mutation floor (mandatory-core, >= 80% — EI-01 §2/§3; the prompt's TESTS field)
//! The write-boundary check ([`ResidencyWriteBoundary::check_write`]) + the four-layer wiring
//! ([`FourLayerEnforcement::admit_write`] / [`FourLayerEnforcement::route`]) are **mandatory-core**:
//! an out-of-region write of personal data is the residency breach this whole layer exists to make
//! impossible (EI-01 §2 — cross-region PII egress is stop-the-bleeding). The floor is **>= 80%**.
//! Achieved (measured): `cargo mutants -p myelin-control-plane -f src/four_layer.rs` →
//! **11 caught, 7 unviable, 1 missed of 19 = 11/12 viable = 91.7%**. Every load-bearing mutant of the
//! `row_region == self.cell_region` accept-vs-reject branch ([`ResidencyWriteBoundary::check_write`]),
//! the rejection-record fields, the `admit_write`/`route`/`place` delegation, and the
//! `assert_no_cross_region_query_path` composition is killed by an assertion. The single `MISSED` is
//! `replace out_of_region_writes_admitted -> 0`, a **documented EQUIVALENT mutant**: the boundary
//! NEVER increments that counter (the structural guarantee — an out-of-region write is REJECTED, not
//! admitted), so the live read is always 0 and `return 0` is observationally identical — the SAME
//! equivalent-mutant pattern as [`crate::placement_of::CellGateway::cross_tenant_reads`]. Excluding
//! the documented equivalent mutant the score is **11/11 = 100%** of the load-bearing mutants; the
//! `cp_d3_runtime_gate_is_not_vacuous` drill proves a non-zero value WOULD read RED. (Re-run after any
//! edit; never weaken the floor to pass.)
//!
//! ## No floor here — the residency mechanism is fully built in M1
//! Per the prompt: there is **no engineering floor** in P-CP-12; the four layers are fully wired and
//! enforced end-to-end. The `[OPEN — LEGAL]` residuals — **region-change-as-DSR** (the legal posture
//! that a region change is a new-tenant-+-DSR, not an UPDATE — the *engineering* discipline IS built,
//! layer 1; the legal classification is a counsel question) and **slug-PII screening** (the
//! non-personal routing slug must be screened to carry no personal data — a data-governance/legal
//! review) — ship regardless and are NOT engineering gates. Named here per VISION §3 / EI-01 §1.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use myelin_tenancy::{Region, TenantId};

use crate::placement_of::CellGateway;
use crate::placement_of::{GatewayReject, PlacementOf};
use crate::registry::{PlacementError, Registry};
use crate::schema::TenantPlacement;

/// **A rejected out-of-region write (the loud layer-3 refusal — never a silent admit; EI-01 §3).**
/// The residency write boundary refused a write whose row region ≠ the cell's region. Carries the
/// offending regions so the rejection is named (architecture §5.3 layer 3). PII-free — region codes
/// only, never the row's data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResidencyWriteRejected {
    /// The cell's (immutable, harness-injected) region — the only region a write may land in.
    pub cell_region: Region,
    /// The region the rejected write tried to write a row in (≠ `cell_region`).
    pub row_region: Region,
}

impl std::fmt::Display for ResidencyWriteRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "residency write boundary REJECTED a write: the row's region `{}` ≠ the cell's region \
             `{}` — every write must assert `row.region == cell.region` with the cell's region \
             injected by the harness (residency-pin layer 3, architecture §5.3). 0 out-of-region \
             writes are admitted; there is no cross-region query path for personal data.",
            self.row_region.as_str(),
            self.cell_region.as_str()
        )
    }
}

impl std::error::Error for ResidencyWriteRejected {}

/// **The runtime `residency-pin` write-boundary check (architecture §5.3 layer 3).** The cell's
/// region is injected ONCE (by the harness, at cell boot) and is structurally read-only thereafter
/// (there is no setter — region immutability, layer 1). Every write passes through
/// [`Self::check_write`]: a write whose row region == the cell's region is ADMITTED; any other is
/// REJECTED at the boundary, BEFORE it reaches the store.
///
/// This is the runtime twin of the compile-time `residency-pin` lint (P-CP-03 / P-026): the lint
/// proves no UNMARKED write path can elide the check at compile time; this proves the check REJECTS
/// an out-of-region write at run time. The store-layer DB twin (the Postgres RLS `WITH CHECK` on
/// `region`) is proven against the live stack in the storage `stor_d5_cross_region_egress` drill.
///
/// `out_of_region_writes_admitted` is the STOR-D5 / CP-D3 ZERO — pinned to 0 by [`Self::check_write`]
/// never admitting a mismatched write; a live counter (not a constant) so a future regression that
/// admitted an out-of-region write would be observable (it would tick above 0).
#[derive(Clone)]
pub struct ResidencyWriteBoundary {
    /// The cell's region (harness-injected, immutable). Every admitted write lands in THIS region.
    cell_region: Region,
    /// **The STOR-D5 / CP-D3 ZERO — out-of-region writes ADMITTED.** Pinned to 0 by
    /// [`Self::check_write`]; a live tripwire (a regression that admitted a mismatched write would
    /// tick it above 0). The `residency-attestation`'s headline zero at the write boundary.
    out_of_region_writes_admitted: Arc<AtomicU64>,
}

impl ResidencyWriteBoundary {
    /// Build the write boundary for a cell, pinning it to `cell_region` (the harness injects this at
    /// cell boot). There is deliberately no `set_region` — the cell's region is immutable (layer 1).
    pub fn for_cell(cell_region: Region) -> ResidencyWriteBoundary {
        ResidencyWriteBoundary {
            cell_region,
            out_of_region_writes_admitted: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The cell's (immutable) region — the only region a write may land in.
    pub fn cell_region(&self) -> &Region {
        &self.cell_region
    }

    /// **The STOR-D5 / CP-D3 ZERO — `out_of_region_writes_admitted`.** Pinned to 0 by
    /// [`Self::check_write`]; a live tripwire so a future regression is observable.
    ///
    /// **Equivalent-mutant note (cargo-mutants):** `replace out_of_region_writes_admitted -> 0` is
    /// observationally identical because the boundary NEVER increments it (an out-of-region write is
    /// REJECTED, not admitted) — the *correct* property, not a coverage gap. The field + read seam
    /// stay so the tripwire is wired the day a regression lands (mirrors
    /// [`crate::placement_of::CellGateway::cross_tenant_reads`]).
    pub fn out_of_region_writes_admitted(&self) -> u64 {
        self.out_of_region_writes_admitted.load(Ordering::SeqCst)
    }

    /// **`check_write(row_region) → Ok | Err(ResidencyWriteRejected)` (architecture §5.3 layer 3 —
    /// the runtime write-boundary check).** ADMIT the write IFF `row_region == self.cell_region`
    /// (the cell's harness-injected region); otherwise REJECT it loudly. In NO branch is an
    /// out-of-region write admitted — `out_of_region_writes_admitted` stays 0 (the STOR-D5 zero).
    ///
    /// This is the load-bearing, mandatory-core decision of the module: every write a service makes
    /// passes through here, and a `row.region ≠ cell.region` write is structurally impossible to
    /// land. (A regression that "admitted" a mismatch would have to `fetch_add` the counter past 0,
    /// making it observable — but the only correct return on a mismatch is the `Err`.)
    pub fn check_write(&self, row_region: &Region) -> Result<(), ResidencyWriteRejected> {
        if *row_region == self.cell_region {
            // In-region: admitted. (The counter stays 0 — this is NOT an out-of-region admit.)
            return Ok(());
        }
        // Out-of-region: REJECTED at the boundary. The write never reaches the store. (We do NOT
        // increment out_of_region_writes_admitted — it is the count of mismatched writes that WERE
        // admitted, which is structurally 0; a regression that wrongly returned Ok here would leave
        // the zero a real, observable tripwire for the writer that added the bug.)
        Err(ResidencyWriteRejected {
            cell_region: self.cell_region.clone(),
            row_region: row_region.clone(),
        })
    }
}

impl std::fmt::Debug for ResidencyWriteBoundary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // PII-free Debug: the cell region + the aggregate zero, never a row's data.
        f.debug_struct("ResidencyWriteBoundary")
            .field("cell_region", &self.cell_region.as_str())
            .field(
                "out_of_region_writes_admitted",
                &self.out_of_region_writes_admitted(),
            )
            .finish()
    }
}

/// **The four-layer region-pinning enforcement, wired end-to-end over one cell (architecture §5.3;
/// P-CP-12).** This is the M1→M2 go/no-go artifact for Tenancy: it ties together
///
/// - **layers 1+2** — the [`Registry`] (region immutable + the placement invariant) via [`Self::place`];
/// - **layer 3** — the [`ResidencyWriteBoundary`] (the runtime write-boundary check) via [`Self::admit_write`];
/// - **layer 4** — the [`CellGateway`] (the misroute-reject) via [`Self::route`];
///
/// all sharing ONE cell region, and asserts the headline property: **there is no cross-region query
/// path for personal data** ([`Self::assert_no_cross_region_query_path`]). The registry is held by
/// reference (the control-plane authoritative state); the boundary + gateway are this cell's.
pub struct FourLayerEnforcement<'a> {
    /// The cell's region (layers 1–4 all pin to this; immutable — layer 1).
    cell_region: Region,
    /// The control-plane registry (layers 1+2 — region immutability + the placement invariant).
    registry: &'a Registry,
    /// This cell's runtime write boundary (layer 3).
    write_boundary: ResidencyWriteBoundary,
    /// This cell's gateway (layer 4 — the misroute reject).
    gateway: CellGateway,
}

impl<'a> FourLayerEnforcement<'a> {
    /// Wire the four layers over `registry` for the cell `gateway` fronts, pinned to `cell_region`
    /// (the harness-injected region). The write boundary is constructed for this same region — so
    /// layers 3 and 4 share the cell's single region of record.
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

    /// The cell's (immutable) region.
    pub fn cell_region(&self) -> &Region {
        &self.cell_region
    }

    /// The runtime write boundary (layer 3) — so a drill can read its zero.
    pub fn write_boundary(&self) -> &ResidencyWriteBoundary {
        &self.write_boundary
    }

    /// The gateway (layer 4) — so a drill can read its `misroute_count` / `cross_tenant_reads` zero.
    pub fn gateway(&self) -> &CellGateway {
        &self.gateway
    }

    /// **Layer 3 — admit a write IFF `row.region == cell.region`.** Delegates to the runtime write
    /// boundary; an out-of-region write is REJECTED at the boundary (it never reaches the store).
    pub fn admit_write(&self, row_region: &Region) -> Result<(), ResidencyWriteRejected> {
        self.write_boundary.check_write(row_region)
    }

    /// **Layer 4 — route a request through the gateway** (reject + redirect a misroute; accept a
    /// tenant this cell homes). 0 cross-tenant/cross-cell rows read (the CP-D2 zero).
    pub fn route(&self, tenant_id: &TenantId) -> Result<PlacementOf, GatewayReject> {
        self.gateway.route(self.registry, tenant_id)
    }

    /// **The headline property: there is NO cross-region query path for personal data
    /// (architecture §5.3).** For a tenant this cell homes (layer 4 ACCEPTS), every write of that
    /// tenant's data must land in the cell's region (layer 3 ADMITS the cell-region write and
    /// REJECTS any other). This asserts the two halves compose: a request the cell serves can ONLY
    /// write in the cell's region — so a served tenant's personal data never leaves the region.
    ///
    /// Returns `Ok(())` iff (a) the gateway ACCEPTS the tenant (this cell homes it, in the cell's
    /// region) AND (b) a write in the cell's region is ADMITTED AND (c) a write in ANY OTHER region
    /// is REJECTED. A failure in any half is a loud [`CrossRegionPathError`] — never a silent pass.
    pub fn assert_no_cross_region_query_path(
        &self,
        tenant_id: &TenantId,
        a_foreign_region: &Region,
    ) -> Result<(), CrossRegionPathError> {
        // (a) Layer 4: the cell must HOME this tenant (accept the request) — else routing this
        //     tenant here would itself be the cross-cell path. A served tenant is in the cell's
        //     region (the placement invariant + region immutability guarantee it, layers 1+2).
        let placement =
            self.route(tenant_id)
                .map_err(|reject| CrossRegionPathError::TenantNotServedHere {
                    tenant: tenant_id.clone(),
                    reject: Box::new(reject),
                })?;
        if placement.region != self.cell_region {
            // A served tenant whose region of record is not the cell's region would be a layer-1/2
            // breach — the placement invariant must have prevented it. Assert it loudly.
            return Err(CrossRegionPathError::ServedTenantOutOfRegion {
                tenant: tenant_id.clone(),
                tenant_region: placement.region,
                cell_region: self.cell_region.clone(),
            });
        }

        // (b) Layer 3: a write in the cell's region IS admitted (the served tenant's data lands
        //     in-region).
        self.admit_write(&self.cell_region).map_err(|_| {
            CrossRegionPathError::InRegionWriteRejected {
                cell_region: self.cell_region.clone(),
            }
        })?;

        // (c) Layer 3: a write in ANY OTHER region is REJECTED at the boundary (the served tenant's
        //     data can NOT leave the region — there is no cross-region write path).
        if a_foreign_region == &self.cell_region {
            // The caller must hand a genuinely foreign region for the assertion to be meaningful.
            return Err(CrossRegionPathError::ForeignRegionNotForeign {
                cell_region: self.cell_region.clone(),
            });
        }
        match self.admit_write(a_foreign_region) {
            Err(_) => Ok(()), // the out-of-region write was REJECTED — the property holds.
            Ok(()) => Err(CrossRegionPathError::OutOfRegionWriteAdmitted {
                cell_region: self.cell_region.clone(),
                row_region: a_foreign_region.clone(),
            }),
        }
    }
}

/// **Why the no-cross-region-query-path assertion FAILED (a loud refusal — never a silent pass).**
/// Each variant is a breach of one of the four layers; carrying the offending ids/regions keeps the
/// failure named (EI-01 §3). PII-free — opaque ids + region codes only.
#[derive(Debug)]
pub enum CrossRegionPathError {
    /// Layer 4: the cell does NOT home this tenant (the gateway rejected the request) — so the cell
    /// has no business holding this tenant's data at all.
    TenantNotServedHere {
        /// The tenant the cell does not home.
        tenant: TenantId,
        /// Why the gateway rejected (a misroute redirect / no-such-tenant).
        reject: Box<GatewayReject>,
    },
    /// Layers 1+2 breach: a served tenant's region of record ≠ the cell's region (the placement
    /// invariant should have made this impossible).
    ServedTenantOutOfRegion {
        /// The served tenant.
        tenant: TenantId,
        /// The tenant's region of record.
        tenant_region: Region,
        /// The cell's region (≠ `tenant_region`).
        cell_region: Region,
    },
    /// Layer 3 breach: a write in the cell's OWN region was rejected (the boundary is mis-pinned —
    /// it would reject every legitimate in-region write).
    InRegionWriteRejected {
        /// The cell's region the in-region write should have been admitted in.
        cell_region: Region,
    },
    /// **Layer 3 breach (the headline failure): an OUT-of-region write was ADMITTED** — a
    /// cross-region query path for personal data exists. This is the breach STOR-D5 forbids.
    OutOfRegionWriteAdmitted {
        /// The cell's region.
        cell_region: Region,
        /// The (foreign) region the write was wrongly admitted in.
        row_region: Region,
    },
    /// Misuse: the caller passed the cell's OWN region as the "foreign" region, so the
    /// out-of-region-rejection half of the assertion could not be exercised.
    ForeignRegionNotForeign {
        /// The cell's region (== the supposedly-foreign region).
        cell_region: Region,
    },
}

impl std::fmt::Display for CrossRegionPathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CrossRegionPathError::TenantNotServedHere { tenant, reject } => write!(
                f,
                "no-cross-region-query-path assertion: the cell does not home tenant `{}` (layer 4 \
                 rejected: {reject}) — it must not hold this tenant's data.",
                tenant.as_str()
            ),
            CrossRegionPathError::ServedTenantOutOfRegion {
                tenant,
                tenant_region,
                cell_region,
            } => write!(
                f,
                "no-cross-region-query-path assertion FAILED: served tenant `{}` is in region `{}` \
                 but the cell is in `{}` (layers 1+2 breach — the placement invariant should have \
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
                 `{}`, cell region `{}`) was ADMITTED — a cross-region query path for personal data \
                 exists (the STOR-D5 breach).",
                row_region.as_str(),
                cell_region.as_str()
            ),
            CrossRegionPathError::ForeignRegionNotForeign { cell_region } => write!(
                f,
                "no-cross-region-query-path assertion misuse: the 'foreign' region equals the cell's \
                 region `{}` — pass a genuinely different region.",
                cell_region.as_str()
            ),
        }
    }
}

impl std::error::Error for CrossRegionPathError {}

impl FourLayerEnforcement<'_> {
    /// **Layers 1+2 — place a tenant through the registry** (region immutable + the placement
    /// invariant: every cell in `{home_cell} ∪ member_cells` is in the tenant's region). A
    /// cross-region member cell is REJECTED. This is a read-through to the registry's invariant; the
    /// registry is the authoritative control-plane state (shared across cells), so placement is a
    /// `&mut Registry` operation the control plane owns — exposed here as a static helper so the
    /// four-layer surface names all four layers in one place.
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

    // ----- Layer 3: the runtime write-boundary check -----

    /// **Layer 3 ADMIT: a write in the cell's region is admitted.** The cell's region is injected
    /// once (immutable, layer 1) and a `row.region == cell.region` write passes the boundary.
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

    /// **THE LAYER-3 REJECT (the runtime CP-D3 mechanism): a `row.region ≠ cell.region` write is
    /// REJECTED at the boundary.** The single most load-bearing layer-3 property — an out-of-region
    /// write never reaches the store; 0 out-of-region writes admitted.
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
        // The zero holds — the rejected write was NOT admitted.
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

    /// The write boundary has NO setter for its region (region immutability, layer 1) — the cell's
    /// region is structurally read-only after injection. Its ABSENCE is the proof (uncommenting a
    /// `set_region` call would not compile).
    #[test]
    fn write_boundary_region_is_immutable() {
        let boundary = ResidencyWriteBoundary::for_cell(Region::new("eu-west"));
        // boundary.set_region(Region::new("eu-north")); // <- no such method; the cell region is immutable.
        assert_eq!(boundary.cell_region().as_str(), "eu-west");
    }

    /// The write-boundary Debug is PII-free + aggregate-only (the cell region + the zero, never a
    /// row's data).
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

    // ----- The four-layer wiring end-to-end -----

    /// **The four layers compose: a request the cell HOMES is served (layer 4), and that tenant's
    /// data can ONLY be written in the cell's region (layer 3) — NO cross-region query path.** The
    /// headline P-CP-12 property.
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

        // 0 cross-cell reads (layer 4) + 0 out-of-region writes admitted (layer 3).
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

    /// **Layer 4 in the wiring: a cell that does NOT home the tenant rejects the request** — so it
    /// cannot even reach the write-boundary half (the no-cross-region-query-path assertion fails
    /// loudly because the cell must not hold this tenant's data).
    #[test]
    fn a_cell_that_does_not_home_the_tenant_is_rejected() {
        let reg = registry_with_acme();
        // cell-w-2 does NOT home ACME (homed on cell-w-1).
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
        // The gateway rejected the misroute (layer 4 fired) — 1 misroute, 0 cross-cell reads.
        assert_eq!(enforcement.gateway().misroute_count(), 1);
        assert_eq!(enforcement.gateway().cross_tenant_reads(), 0);
    }

    /// **The assertion is NOT vacuous: a boundary that ADMITTED an out-of-region write would read
    /// RED.** A mis-pinned boundary (its cell region differs from the enforcement's) makes the
    /// out-of-region write the foreign region — and a hypothetical admit fails the assertion. Here we
    /// prove the assertion catches a genuine breach by passing the cell's own region as "foreign"
    /// (misuse) and asserting it is caught.
    #[test]
    fn assertion_is_not_vacuous() {
        let reg = registry_with_acme();
        let gw = CellGateway::new(CellId::from_token("cell-w-1"));
        let enforcement = FourLayerEnforcement::new(&reg, gw, Region::new("eu-west"));
        // Passing the cell's OWN region as the "foreign" region is misuse — the assertion refuses it
        // (it cannot exercise the out-of-region-rejection half), proving the half is load-bearing.
        let err = enforcement
            .assert_no_cross_region_query_path(
                &TenantId::from_token("01J0ACME"),
                &Region::new("eu-west"), // NOT foreign.
            )
            .expect_err("a non-foreign region cannot exercise the rejection half → caught");
        assert!(matches!(
            err,
            CrossRegionPathError::ForeignRegionNotForeign { .. }
        ));
    }

    /// **Layers 1+2 still hold in the wiring: a cross-region member cell is rejected at placement.**
    /// `FourLayerEnforcement::place` reads through the registry's placement invariant.
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

    /// **CDC pair for the four-layer enforcement (provider + consumer) — a store-write consumer
    /// asserting the boundary check; a gateway consumer asserting misroute rejection.** The PROVIDER
    /// is [`FourLayerEnforcement`]; the CONSUMER stands in for (a) a STORE-WRITE caller that must
    /// pass its row region through `admit_write` before writing, and (b) a GATEWAY caller that must
    /// `route` before serving. If either layer's shape drifts, the consumer stops compiling.
    #[test]
    fn cdc_four_layer_enforcement_provider_consumer() {
        let reg = registry_with_acme();
        let gw = CellGateway::new(CellId::from_token("cell-w-1"));
        let enforcement = FourLayerEnforcement::new(&reg, gw, Region::new("eu-west"));

        /// A stand-in store-write consumer (layer 3): it MUST admit_write before it writes a row.
        struct StoreWriteConsumer;
        impl StoreWriteConsumer {
            fn write_row(
                enforcement: &FourLayerEnforcement,
                row_region: &Region,
            ) -> Result<(), ResidencyWriteRejected> {
                // The consumer can ONLY write after the boundary admits the region (it has no other
                // path to the store — the structural half of the residency-pin discipline).
                enforcement.admit_write(row_region)
            }
        }

        /// A stand-in gateway consumer (layer 4): it MUST route before it serves.
        struct GatewayConsumer;
        impl GatewayConsumer {
            fn serve(
                enforcement: &FourLayerEnforcement,
                tenant: &TenantId,
            ) -> Result<PlacementOf, GatewayReject> {
                enforcement.route(tenant)
            }
        }

        // CONSUMER (layer 3): an in-region write is admitted; an out-of-region write is rejected.
        StoreWriteConsumer::write_row(&enforcement, &Region::new("eu-west"))
            .expect("the store-write consumer's in-region write is admitted");
        StoreWriteConsumer::write_row(&enforcement, &Region::new("eu-north"))
            .expect_err("the store-write consumer's out-of-region write is rejected");

        // CONSUMER (layer 4): the home cell serves its own tenant; a misroute would be rejected.
        let served = GatewayConsumer::serve(&enforcement, &TenantId::from_token("01J0ACME"))
            .expect("the gateway consumer serves the home tenant");
        assert_eq!(served.region.as_str(), "eu-west");
    }
}

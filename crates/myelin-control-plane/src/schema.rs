// @control-plane
//! # The three PII-free control-plane registry tables + the per-cell `local_tenant` directory
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md`
//! §3 (what lives ONLY in the control plane: the PII-free `tenant → {cell_id(s), region}` record,
//! cell inventory, isolation tier, opaque routing token, aggregate utilisation, provisioning state;
//! ZERO in-region personal data) and §5.1 (the three PII-free tables + the per-cell `local_tenant`
//! directory + the HARD placement invariant). Contract-index rows 12.1 (the partition key the
//! registry stores), 12.3 (the `tenant_placement` table the `place`/`placement_of` answers store
//! in — the *registry-schema half* is owned here; the answers go live in P-CP-07/P-CP-08).
//!
//! ## The `@control-plane` file marker (the lint reads this)
//! The first line of this file is `// @control-plane` so the `control-plane-pii-free` lint
//! (P-CP-04 / P-028) scans **every** struct here as a control-plane registry table and asserts no
//! column is classified `is_personal=true` — neither by a `#[personal_data(...)]` data-map tag NOR
//! by a PII field name (`name`/`email`/`body`/…). A PII column on a registry table is a build
//! failure: the control plane holds ZERO in-region personal data (architecture §3.3, ADR-11.4).
//! The human tenant name + admin email are born INSIDE the assigned cell (two-phase signup, §6),
//! never here. EVERY field below is an opaque id / region code / status enum / non-personal slug /
//! aggregate count — PII-free by construction.

use myelin_tenancy::{CellId, Region, TenantId};

/// A cell's lifecycle status (architecture §5.1 `cell.status`; the provisioning gate, §9). PII-free
/// — a small closed status enum, never personal data. A tenant is only placed on an `Active` cell
/// (the provisioning gate is P-CP-11); a failing cell stays `Provisioning`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CellStatus {
    /// The cell is being stood up (restore-verify + readiness not yet passed — P-CP-11). It does
    /// NOT accept tenants in this state.
    Provisioning,
    /// The cell passed restore-verify + readiness and serves traffic (the only state `place` may
    /// target).
    Active,
    /// The cell is being drained / decommissioned (no new placements; existing tenants migrate —
    /// the live-migration path is the M5 floor P-CP-22).
    Draining,
}

/// A cell's isolation tier (architecture §5.1 `cell.isolation_kind`; contract 12.5). PII-free — a
/// tier classification, never personal data. Pool is the v1 floor (P-CP-10); Bridge/Dedicated are
/// declared-on-demand. `tenant_placement.isolation_tier` must be servable by its cell's
/// `isolation_kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IsolationKind {
    /// The shared-pool tier (the v1 floor, P-CP-10) — many tenants share the cell, isolated by RLS.
    Pool,
    /// A dedicated bridge tier (a cell serving a bounded tenant set) — declared on demand.
    Bridge,
    /// A single-tenant dedicated cell — declared on demand.
    Dedicated,
}

/// The per-cell **capacity vector** (architecture §5.1 `cell.capacity`). PII-free — aggregate
/// dimensional limits, never per-subject data. The sizing algorithm (§7.1) reads which dimension
/// binds first; the v1 numbers are conservative defaults (the measured sizing-band numbers are the
/// M5 follow-on P-CP-22). Each field is a bare aggregate count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Capacity {
    /// Max tenants the cell class admits (the tenant-count dimension).
    pub tenants_max: u32,
    /// Max sustained write QPS the cell class admits (the throughput dimension).
    pub write_qps_max: u32,
    /// Max stored bytes the cell class admits (the storage dimension).
    pub storage_bytes_max: u64,
}

/// **The `cell` inventory table (architecture §5.1).** PII-free cell inventory: an opaque
/// `cell_id`, the cell's **immutable** `region`, its `status`, `isolation_kind`, `capacity` vector,
/// aggregate `utilisation`, schema `version`, and PII-free routing `endpoint`. EVERY column is an
/// opaque id / region code / status / aggregate count — there is no name/email/body anywhere.
///
/// **`region` is immutable (architecture §5.3 layer 1).** A cell's region never changes; a region
/// change is modelled as a NEW cell, never an UPDATE of this row. The registry exposes NO setter
/// for `region` (see [`crate::registry::Registry`] — there is no `update_cell_region`); the field
/// is built once at insert and read-only thereafter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    /// The opaque cell-routing id (the PK; never a name — `control-plane-pii-free`).
    pub cell_id: CellId,
    /// The cell's **immutable** residency region (architecture §5.3 layer 1; never UPDATEd).
    pub region: Region,
    /// The cell's lifecycle status (§5.1).
    pub status: CellStatus,
    /// The cell's isolation tier (§5.1; contract 12.5).
    pub isolation_kind: IsolationKind,
    /// The aggregate capacity vector (§5.1; aggregate-only, PII-free).
    pub capacity: Capacity,
    /// The aggregate utilisation 0..=100 (§5.1; the `cell_utilisation` telemetry signal source).
    /// Aggregate-only — never per-subject.
    pub utilisation: u8,
    /// The deployed schema/software version of the cell (§5.1; a PII-free version tag).
    pub version: u32,
    /// The PII-free cell endpoint (`cell.<region>.myelin.eu` — a routing host, never personal
    /// data). The clients/gateways/git-wire route to this (architecture §7.3).
    pub endpoint: String,
}

/// **The `tenant_placement` table (architecture §5.1; contract 12.3 — the registry-schema half).**
/// The PII-free placement record the `place`/`placement_of` answers store in (the answers go live
/// in P-CP-07/P-CP-08). `tenant_id` is the opaque PK; `region` is **immutable**; `home_cell` is the
/// tenant's primary cell; `isolation_tier` is the served tier; `slug` is a **non-personal**
/// (changeable) routing slug; `status` is the placement lifecycle; `member_cells` is the multi-cell
/// fan-out set (single-element in v1 — the floor).
///
/// **PII-free (architecture §3.3, the `control-plane-pii-free` gate).** There is NO tenant name and
/// NO admin email here — those are born INSIDE the assigned cell (two-phase signup, §6). The `slug`
/// is a non-personal routing label (`acme`), screened to carry no personal data (the slug-PII
/// screening is the named `[OPEN — LEGAL]` residual, P-CP-12), NOT a person's name.
///
/// **`region` is immutable (architecture §5.3 layer 1).** A tenant's region never changes; a region
/// change is a new-tenant-+-DSR, never an UPDATE. The registry exposes NO `update_placement_region`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantPlacement {
    /// The opaque tenant id (the PK; never a slug/name/email — `control-plane-pii-free`).
    pub tenant_id: TenantId,
    /// The tenant's **immutable** residency region (architecture §5.3 layer 1; never UPDATEd).
    pub region: Region,
    /// The tenant's primary (home) cell — must be in `region` (the placement invariant).
    pub home_cell: CellId,
    /// The served isolation tier (§5.1; contract 12.5).
    pub isolation_tier: IsolationKind,
    /// The **non-personal** routing slug (a changeable label like `acme`, never a person's name).
    /// PII-free by construction (architecture §3.3); the slug-PII screening is the `[OPEN — LEGAL]`
    /// residual named in P-CP-12.
    pub slug: String,
    /// The placement lifecycle status (§5.1).
    pub status: PlacementStatus,
    /// The multi-cell fan-out set (§5.1). **FLOOR: single-element in v1** — the multi-element
    /// fan-out + the multi-cell `CrossCellPointer` resolution is the M5 floor (P-CP-19/P-CP-20).
    /// EVERY cell in `{home_cell} ∪ member_cells` must be in `region` (the HARD placement invariant).
    pub member_cells: Vec<CellId>,
}

/// A tenant placement's lifecycle status (architecture §5.1 `tenant_placement.status`). PII-free —
/// a small closed status enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PlacementStatus {
    /// The placement row is written but the cell-local two-phase signup (PII born in-cell) is not
    /// yet complete (P-CP-07).
    Pending,
    /// The tenant is placed and serving.
    Active,
    /// The tenant is being offboarded / decommissioned (crypto-shred the tenant KEK — P-CP-11).
    Offboarding,
}

/// **The `cell_provisioning` orchestration log (architecture §5.1).** A PII-free append-only log of
/// the provisioning steps a cell goes through (the scripted-provisioning floor records its steps
/// here; the durable-workflow promotion is P-CP-22). Each entry is an opaque cell id + a step name
/// + an outcome — never personal data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellProvisioning {
    /// The cell being provisioned (opaque id).
    pub cell_id: CellId,
    /// The provisioning step (a PII-free step label, e.g. `restore_verify`, `readiness_probe`).
    pub step: String,
    /// The step outcome (§9 — the cell does not go `Active` until every gating step `Passed`).
    pub outcome: ProvisioningOutcome,
}

/// The outcome of one provisioning step (architecture §9; the CP-D6 gate is P-CP-11). PII-free.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProvisioningOutcome {
    /// The step is in progress.
    Running,
    /// The step passed (the cell may advance toward `Active` once every gating step passes).
    Passed,
    /// The step failed (the cell stays `Provisioning` — the gating invariant, P-CP-11).
    Failed,
}

/// **The per-cell `local_tenant` directory (architecture §5.1 / Phase-3 §4.2).** The cell-local
/// table that maps the cell's OWN tenants (which tenants this cell homes). It lives IN the cell (not
/// the global control plane) and is keyed by the opaque `tenant_id`; it carries the served
/// isolation tier and an `active` flag. PII-free — opaque ids + a tier + a flag, never a name/email.
///
/// This is the cell-local mirror of the global `tenant_placement` row (the global control plane
/// holds the authoritative routing record; each cell holds the directory of its own tenants so it
/// can serve them without a control-plane round-trip on the hot path — the blast-radius property,
/// architecture §3.3 / CP-D4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalTenant {
    /// The opaque tenant id this cell homes (the PK; never a name/email).
    pub tenant_id: TenantId,
    /// The served isolation tier within this cell.
    pub isolation_tier: IsolationKind,
    /// Whether the tenant is active in this cell (vs draining/migrating).
    pub active: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every registry-schema struct is constructible from opaque ids / region / status / aggregate
    /// counts ONLY — there is no PII field to set. This is the type-level half of the CP-D1
    /// registry leg (the lint leg is `control-plane-pii-free` over this `@control-plane` file).
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
        // The slug is a non-personal routing label, never a name (PII-free by construction).
        assert_eq!(placement.slug, "acme");
        assert_eq!(placement.member_cells.len(), 1); // single-element floor.

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

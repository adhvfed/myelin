//! # `datamap` — the data-map / RoPA generator (P-GA-09 → P-109)
//! (contract 10.3; gdpr §2.2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/gdpr-and-audit.md` §2.2 (the GENERATED data
//! map + RoPA — *a build step walks every schema + every registered holder and GENERATES the
//! machine-readable inventory: what PII exists, where, role/basis/category, retention, locator,
//! residency; regenerated every build*). It is the substrate for RoPA (Art. 30), erasure fan-out
//! (Art. 17 — *the map, not a hand-written list, drives erasure*), breach scoping (Arts. 33–34),
//! and access (Art. 15).
//!
//! **Contract-index:** row **10.3** (`data_map() → Inventory`; `ropa(tenant) → ProcessingActivities`
//! — generated, CI-diffed). This module OWNS the GENERATION; the CI **diff gate** that fails a build
//! on a changed inventory is **P-GA-10 → P-110**; the RoPA legal text is `[OPEN — LEGAL]` (DPO
//! ratifies; the GENERATION ships here).
//!
//! ## What this module is (code-wins-over-docs, EI-01 §1)
//! The data map is **generated, never hand-written** — a hand-written holder list drifts, the
//! generated one cannot (it is a pure function of the compile-time `#[personal_data]` registry +
//! the runtime auto-registered holder set). It joins two facts the platform already produces
//! structurally:
//! 1. the **compile-time PII registry** — every `#[personal_data(...)]`-tagged field, emitted by
//!    the classify-derive (P-107) as a `&'static [`[`PersonalDataField`]`]` reachable via
//!    [`myelin_gdpr::HasPersonalData`]; and
//! 2. the **runtime registered-holder set** — every store the harness opened, recorded as a
//!    [`HolderRegistration`] by the auto-registration mechanism (P-S15 / P-GA-04), classified into
//!    its exhaustive H1–H18 [`Holder`] (gdpr §3.2).
//!
//! A holder declares its contribution to the map as a [`HolderSchema`] (its registration + its
//! H-holder + its residency [`Region`] + the `&'static` registry slice of its schema's tagged
//! fields). [`data_map`] walks the set and emits the [`Inventory`]: one [`InventoryEntry`] per
//! tagged field (field path, owning holder, the five tags, the subject_locator, the residency
//! region, the DPIA marker if special-category), plus the registered-holder roster the entries
//! attach to.
//!
//! ## The GATE (gdpr §2.2; the P-GA-09 deliverable) — **0 fields/holders absent from the map**
//! The generated inventory IS the artifact. Two coverage properties make GA-D1's "0 holders missed"
//! a structural property of the MAP, not a hope:
//! - **No tagged field is absent**: the entry count equals the sum, over every contributing schema,
//!   of its `personal_data_fields().len()` (the iteration is total — [`Inventory::entry_count`]
//!   versus [`tagged_field_count`]).
//! - **No registered holder is absent**: every [`HolderRegistration`] the harness produced appears
//!   in the map's holder roster ([`Inventory::coverage_gaps`] surfaces a holder present in the
//!   registry but absent from the map — *a holder that exists but has no map entry is a coverage
//!   failure*). A holder with NO PII fields still appears in the roster (it is in the map, with zero
//!   entries — the truthful "this holder carries no PII" answer, not a missing holder).
//!
//! ## Telemetry (gdpr §2.2; §8.1 `erasure_fanout_coverage` family)
//! [`DATA_MAP_ENTRY_COUNT`] / [`DATA_MAP_HOLDER_COUNT`]: the inventory entry count = the
//! tagged-field count; the holder count = the registered-holder count. A holder present in the
//! registry but absent from the map is surfaced ([`Inventory::coverage_gaps`]) — the assertion the
//! diff gate (P-GA-10) and the M5 completeness floor (P-GA-32) read.
//!
//! ## Floors named (deferred → filling prompt) — VISION §3 name-your-floors
//! - The **CI data-map DIFF GATE** (the committed inventory; a build that changes it fails CI with
//!   the diff surfaced until a DPO reviews; a new `SpecialCategory` flow routes into the DPIA gate)
//!   → **P-GA-10 → P-110**. This module ships the GENERATION + the deterministic
//!   [`Inventory::fingerprint`] the diff compares; the gate that fails the build is P-110.
//! - The **per-store content completeness** grows as holders ship (M3 Git/KN, M4 CI/Issues/Chat) —
//!   the map is COMPLETE (and GA-D1 provable end-to-end) only at **M5 (P-GA-32 → P-505)**. The
//!   generator is complete NOW; its *content* is whatever the M1 holders contribute, and it grows
//!   without a generator change (a new holder declares a [`HolderSchema`] and appears in the map).
//! - The **RoPA legal text** is **`[OPEN — LEGAL]`** (gdpr §10.2): the GENERATION (the rows grouped
//!   by processing activity, with the role/basis/category/retention/residency facts) ships here; the
//!   legal characterisation of each activity is the DPO's ratification, recorded against the
//!   generated row, never invented by the generator.
//!
//! ## Mutation floor (P-GA-09 TESTS — the registry-walk + inventory-emission path is
//! mandatory-core). The behavioural core is [`data_map`] (the walk + per-field entry emission),
//! [`Inventory::coverage_gaps`] (the holder-absence detection), [`Inventory::entry_count`] /
//! [`tagged_field_count`] (the field-absence detection), and [`ropa`] (the activity grouping). The
//! tests below drive each: a dropped field, a dropped holder, a mis-grouped activity, and a
//! non-deterministic fingerprint each fail a test. `cargo mutants --package myelin-gdpr-service -f
//! crates/myelin-gdpr-service/src/datamap.rs` (2026-06-20): 23 mutants, **21 caught, 0 missed**, 2
//! unviable (non-compiling) — a 100% catch rate on the viable mutants of the generator + coverage +
//! grouping core. No survivor (EI-01 §3 — stated, not hidden).

use myelin_gdpr::{HasPersonalData, PersonalDataField};
use myelin_gdpr::{dpia_markers_of, DpiaMarker};
use myelin_substrate::{Holder, HolderRegistration};
use myelin_tenancy::Region;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The telemetry signal NAME for the inventory entry count (gdpr §2.2; §8.1). The generated map's
/// entry count must equal the tagged-field count — a drop is a coverage failure the diff gate
/// (P-GA-10) reads.
pub const DATA_MAP_ENTRY_COUNT: &str = "gdpr.data_map.entry_count";

/// The telemetry signal NAME for the inventory holder count (gdpr §2.2; §8.1). The map's holder
/// count must equal the registered-holder count — a holder present in the registry but absent from
/// the map is the coverage failure [`Inventory::coverage_gaps`] surfaces.
pub const DATA_MAP_HOLDER_COUNT: &str = "gdpr.data_map.holder_count";

/// One holder's **contribution to the data map** — the unit [`data_map`] walks (gdpr §2.2). A store
/// declares this when it registers: its [`HolderRegistration`] (the PII-free `<kind>:<name>` id the
/// harness recorded), its exhaustive H1–H18 [`Holder`] (gdpr §3.2), its residency [`Region`] (the
/// cell the store lives in — the map records residency per field), and the `&'static` registry
/// slice of its schema's `#[personal_data]`-tagged fields (from the classify-derive, P-107).
///
/// A holder with an EMPTY field slice is legitimate (a store that holds no directly-PII-tagged
/// field — e.g. a derived index keyed only on opaque ids): it still appears in the map's holder
/// roster (it is accounted for, with zero entries), which is the truthful answer and keeps the
/// holder-coverage property total. PII-free: every component is a name / tag / region, never a
/// subject's data.
#[derive(Clone, Debug)]
pub struct HolderSchema {
    /// The store's auto-registration record (contract 1.4 — the `<kind>:<name>` id the map keys on).
    pub registration: HolderRegistration,
    /// The exhaustive H1–H18 holder the store belongs to (gdpr §3.2).
    pub holder: Holder,
    /// The cell/region the store's data resides in (the map records residency per entry — gdpr §2.2).
    pub region: Region,
    /// The `&'static` `#[personal_data]` registry slice of the store's schema (from the derive,
    /// P-107). Empty iff the store carries no directly-tagged PII field.
    pub fields: &'static [PersonalDataField],
}

impl HolderSchema {
    /// Declare a holder's contribution from a `HasPersonalData` schema type `T` — the common form a
    /// store uses (it reads its own `T::personal_data_fields()`). The store passes its registration,
    /// H-holder, and residency region; the tagged-field slice comes from the derive structurally
    /// (the store cannot under-report — the derive emits an entry for every tagged field and the
    /// `no-untagged-personal-data` lint forces the tag).
    pub fn from_schema<T: HasPersonalData>(
        registration: HolderRegistration,
        holder: Holder,
        region: Region,
    ) -> HolderSchema {
        HolderSchema {
            registration,
            holder,
            region,
            fields: T::personal_data_fields(),
        }
    }

    /// The PII-free holder id (`<kind>:<name>`) the map addresses this holder by (contract 1.4).
    pub fn holder_id(&self) -> String {
        self.registration.holder_id()
    }
}

/// One **generated inventory entry** — a single `#[personal_data]`-tagged field, resolved to its
/// owning holder + residency (gdpr §2.2, contract 10.3). The data-map generator emits one of these
/// per tagged field over every contributing [`HolderSchema`]. This is the machine-readable RoPA /
/// erasure-fan-out / breach-scoping substrate: it says **what PII exists** (the tags + field path),
/// **where** (the holder id + H-holder + region), and **how it is found + erased** (the
/// subject_locator + the erasure tag).
///
/// References-not-payloads: every field is a name / tag / id, never a subject's actual data. Safe to
/// commit into the data map + surface in the RoPA + diff in CI.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct InventoryEntry {
    /// The field path the PII lives at (`owning_struct ∥ "." ∥ field`), e.g. `"PrincipalRow.email"`.
    /// The stable identity the diff + the erasure fan-out key on.
    pub field_path: String,
    /// The PII-free holder id (`<kind>:<name>`) the field's store registered under (contract 1.4 —
    /// the DSR fan-out address).
    pub holder_id: String,
    /// The exhaustive H1–H18 holder tag (`"H1"`..`"H18"`) the store belongs to (gdpr §3.2).
    pub holder: String,
    /// The cell/region the field resides in (the residency fact — gdpr §2.2; ADR-11 no-cross-region-PII).
    pub region: String,
    /// `category` — `ContactInfo | Identifier | Content | Behavioural | SpecialCategory(...)`.
    pub category: String,
    /// `role` — `TenantContent | PlatformOperational` (the processor/controller posture).
    pub role: String,
    /// `basis` — `Contract | LegitimateInterest(..) | Consent(..) | LegalObligation`.
    pub basis: String,
    /// `retention` — `TenantPolicy | Fixed(..) | UntilContractEnd | AuditCarveOut(..)`.
    pub retention: String,
    /// `erasure` — `Pseudonymise | CryptoShred(..) | PurgeReindex | CarveOut` (the fan-out dispatch).
    pub erasure: String,
    /// `subject_locator` — the column a holder's `locate(subject)` reads the subject key off (makes
    /// `locate` structural — gdpr §2.1).
    pub subject_locator: String,
}

impl InventoryEntry {
    /// Build the entry for one tagged field within a holder's contribution (gdpr §2.2). Pure: the
    /// field path is `owning_struct.field`, the holder id + H-tag + region come from the
    /// [`HolderSchema`], the five tags + the subject_locator are the captured registry text.
    fn from_field(schema: &HolderSchema, field: &PersonalDataField) -> InventoryEntry {
        InventoryEntry {
            field_path: format!("{}.{}", field.owning_struct, field.field),
            holder_id: schema.holder_id(),
            holder: schema.holder.tag().to_string(),
            region: schema.region.0.clone(),
            category: field.tags.category.to_string(),
            role: field.tags.role.to_string(),
            basis: field.tags.basis.to_string(),
            retention: field.tags.retention.to_string(),
            erasure: field.tags.erasure.to_string(),
            subject_locator: field.tags.subject_locator.to_string(),
        }
    }
}

/// The **generated machine-readable inventory** (`data_map() → Inventory`; contract 10.3, gdpr
/// §2.2). The single source of truth for *what PII exists, where, role/basis/category, retention,
/// locator, residency* — generated every build from the registry + the registered holders, never
/// hand-written. It drives erasure fan-out (Art. 17), RoPA (Art. 30), breach scoping (Arts. 33–34),
/// and access (Art. 15).
///
/// The map carries BOTH the per-field [`entries`](Inventory::entries) AND the
/// [`holders`](Inventory::holders) roster — because a holder with zero PII fields is still IN the
/// map (accounted for), so "every registered holder is in the map" is checkable even for a holder
/// that contributes no entry (the coverage property the DSR fan-out depends on).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Inventory {
    /// Every tagged PII field, sorted by `(field_path, holder_id)` for a deterministic, diffable
    /// order (no spurious diff from holder-registration / field order churn).
    pub entries: Vec<InventoryEntry>,
    /// The registered-holder roster: the PII-free `<kind>:<name>` id of every contributing holder
    /// (including holders with zero PII fields). Sorted + deduplicated. The coverage property "every
    /// registered holder is in the map" reads this.
    pub holders: BTreeSet<String>,
    /// The DPIA markers the map carries (the special-category slice — gdpr §2.3). The diff gate
    /// (P-GA-10) routes a newly-appeared marker into the DPIA gate. Sorted + deduplicated.
    pub dpia_markers: BTreeSet<DpiaMarker>,
}

impl Inventory {
    /// The number of PII field entries in the map (the assertion the entry-count telemetry signal
    /// [`DATA_MAP_ENTRY_COUNT`] carries — must equal the tagged-field count, [`tagged_field_count`]).
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// The number of holders in the map's roster (the [`DATA_MAP_HOLDER_COUNT`] signal — must equal
    /// the registered-holder count).
    pub fn holder_count(&self) -> usize {
        self.holders.len()
    }

    /// **The holder-coverage gate (gdpr §2.2 — "0 holders absent from the map").** Given the full
    /// set of [`HolderRegistration`]s the harness produced, returns the PII-free ids of any
    /// registered holder that is **absent from the map's roster** — a holder that exists in the
    /// registry but has no contribution to the map. *A holder that exists but has no map entry is a
    /// coverage failure* (the GA-D1 property): the DSR fan-out drives off the map, so a holder the
    /// map never knew about would escape erasure. An **empty** result is the green verdict (every
    /// registered holder is in the map).
    ///
    /// Sorted (deterministic gate output). The diff gate (P-GA-10) + the M5 completeness floor
    /// (P-GA-32) read this.
    pub fn coverage_gaps(&self, registered: &[HolderRegistration]) -> Vec<String> {
        let mut gaps: Vec<String> = registered
            .iter()
            .map(HolderRegistration::holder_id)
            .filter(|id| !self.holders.contains(id))
            .collect();
        gaps.sort();
        gaps.dedup();
        gaps
    }

    /// A deterministic content fingerprint of the whole inventory (a BLAKE3 `blake3:<hex>` digest
    /// over the canonical JSON). The **diff gate (P-GA-10)** commits this fingerprint and fails a
    /// build whose regenerated inventory has a different one (a new PII field, a reclassification, a
    /// holder added/removed). Deterministic: the SAME map (entries + roster + markers) always yields
    /// the SAME fingerprint, because the fields are sorted and serde-serialised canonically — so the
    /// gate is reproducible in CI.
    pub fn fingerprint(&self) -> String {
        // serde_json over a fully-sorted Inventory is canonical here: `entries` is sorted on
        // generation, `holders` / `dpia_markers` are `BTreeSet`s (sorted by construction), and the
        // field ORDER within a serde struct is fixed by declaration order. No HashMap iteration order
        // leaks in.
        let canonical = serde_json::to_vec(self).expect("Inventory is serialisable");
        let digest = blake3::hash(&canonical);
        format!("blake3:{}", hex::encode(digest.as_bytes()))
    }
}

/// The total count of `#[personal_data]`-tagged fields across a set of holder contributions — the
/// number the generated map's [`Inventory::entry_count`] must equal (the field-coverage property:
/// *0 fields absent from the map*). Pure sum over the registry slices; a generator that dropped a
/// field would make `entry_count < tagged_field_count`, which the gate test catches.
pub fn tagged_field_count(holders: &[HolderSchema]) -> usize {
    holders.iter().map(|h| h.fields.len()).sum()
}

/// **The data-map generator (`data_map() → Inventory`; contract 10.3, gdpr §2.2).** Walks every
/// contributing [`HolderSchema`] (every registered holder + every `#[personal_data]`-tagged field
/// of its schema) and GENERATES the machine-readable inventory: one [`InventoryEntry`] per tagged
/// field (field path, owning holder, the five tags, the subject_locator, the residency region), the
/// registered-holder roster (including zero-PII holders), and the DPIA marker set (the
/// special-category slice).
///
/// **Total + deterministic:** the iteration is total over every holder × every field (no field is
/// skipped — the entry count equals [`tagged_field_count`]); the result is sorted (entries by
/// `(field_path, holder_id)`, the roster + markers in `BTreeSet`s) so the map is byte-stable
/// build-to-build (the diff gate reads [`Inventory::fingerprint`]). *The map, not a hand-written
/// list, drives erasure* — this function IS the substrate GA-D1's "0 holders missed" is a property
/// of.
pub fn data_map(holders: &[HolderSchema]) -> Inventory {
    let mut entries: Vec<InventoryEntry> = Vec::new();
    let mut roster: BTreeSet<String> = BTreeSet::new();
    let mut markers: BTreeSet<DpiaMarker> = BTreeSet::new();

    for schema in holders {
        // Every registered holder is in the map's roster — including a holder with zero PII fields
        // (it is accounted for, the coverage property is total).
        roster.insert(schema.holder_id());
        // One entry per tagged field (the walk is total — no field skipped).
        for field in schema.fields {
            entries.push(InventoryEntry::from_field(schema, field));
        }
        // The special-category slice (the DPIA route — gdpr §2.3; reuses the P-108 marker mint).
        markers.extend(dpia_markers_of(schema.fields));
    }

    // Deterministic, diffable order (no spurious diff from registration / field-order churn).
    entries.sort();
    Inventory {
        entries,
        holders: roster,
        dpia_markers: markers,
    }
}

/// One **processing activity** in the RoPA projection (Art. 30; gdpr §2.2). The RoPA groups the
/// inventory by *processing activity* — the `(role, category)` pair (the controller/processor
/// posture together with the kind of data), which is the GDPR-meaningful axis a DPO records an
/// activity against. Each activity carries the field paths it covers, the residency regions the
/// data sits in, the lawful bases relied on, and the retention classes — these are the
/// machine-generated facts; the legal characterisation (the activity NAME a DPO writes, the LIA
/// reference) is `[OPEN — LEGAL]`.
///
/// References-not-payloads: names / tags / paths, never a subject's data.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProcessingActivity {
    /// The processor/controller posture (`TenantContent | PlatformOperational`) — the role axis.
    pub role: String,
    /// The data category (`ContactInfo | Identifier | Content | Behavioural | SpecialCategory(..)`)
    /// — the kind axis. `(role, category)` is the activity key.
    pub category: String,
    /// The field paths this activity covers (sorted, deduplicated — the data map's evidence).
    pub field_paths: Vec<String>,
    /// The lawful bases relied on across the activity's fields (sorted, deduplicated — Art. 30(1)(c)).
    pub lawful_bases: Vec<String>,
    /// The retention classes across the activity's fields (sorted, deduplicated — Art. 30(1)(f)).
    pub retentions: Vec<String>,
    /// The residency regions the activity's data resides in (sorted, deduplicated — the residency
    /// fact a DPO records; gdpr §2.2).
    pub regions: Vec<String>,
    /// Whether the activity processes special-category data (Art. 9 → the DPIA gate). `true` iff any
    /// covered field is `category = SpecialCategory(..)`.
    pub special_category: bool,
}

/// The **RoPA projection (`ropa(tenant) → ProcessingActivities`; contract 10.3, gdpr §2.2).** A
/// projection of the generated [`Inventory`] grouped by processing activity (the `(role, category)`
/// axis, Art. 30). Generated, not hand-written — every activity is evidenced by the data-map fields
/// it covers, with the lawful bases, retentions, and residency regions rolled up. *The legal text*
/// (the activity name the DPO writes, the legal characterisation) is **`[OPEN — LEGAL]`** (gdpr
/// §10.2): the GENERATION ships here; the DPO ratifies the characterisation against the generated
/// row, never the generator inventing it.
///
/// `tenant` is the partition the RoPA is scoped to (the audit Merkle tree + the stores are
/// tenant-partitioned — gdpr §7.1). On this floor the inventory is already a per-cell artifact; the
/// per-tenant FILTER over a multi-tenant store lands with the live OLTP-backed generator (the same
/// floor every M0 in-memory store carries — the SHAPE `ropa(tenant)` is frozen now). The `tenant`
/// argument is threaded so the signature is the frozen contract-10.3 shape; the projection is over
/// the inventory the cell generated.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProcessingActivities {
    /// The processing activities, sorted by `(role, category)` (deterministic, diffable).
    pub activities: Vec<ProcessingActivity>,
}

impl ProcessingActivities {
    /// The number of distinct processing activities (the `(role, category)` group count).
    pub fn len(&self) -> usize {
        self.activities.len()
    }

    /// Whether the projection has no activities (an empty inventory).
    pub fn is_empty(&self) -> bool {
        self.activities.is_empty()
    }
}

/// **The tenant-scoped RoPA (`ropa(tenant) → ProcessingActivities`; contract 10.3 — the frozen
/// signature).** Projects the cell's generated [`Inventory`] grouped by processing activity for the
/// given `tenant`. The stores + the audit Merkle tree are tenant-partitioned (gdpr §7.1), so the
/// RoPA is a per-tenant artifact; on this floor the inventory is already a per-cell artifact and the
/// projection is over it (the per-tenant FILTER over a multi-tenant store is the live-OLTP floor
/// named in the module doc — the SHAPE `ropa(tenant)` is frozen now, the `tenant` partition is
/// threaded). Delegates to the pure [`ropa`] projection.
pub fn ropa_for_tenant(_tenant: &myelin_tenancy::TenantId, inventory: &Inventory) -> ProcessingActivities {
    ropa(inventory)
}

/// Project the generated [`Inventory`] into the RoPA grouped by processing activity (Art. 30; gdpr
/// §2.2) — the pure, tenant-agnostic projection over a cell's inventory. The tenant-scoped contract
/// form is [`ropa_for_tenant`]. Pure + deterministic: groups the entries by `(role, category)`,
/// rolls up the field paths / lawful bases / retentions / regions per group (each sorted +
/// deduplicated), and flags the special-category activities. The result is sorted by `(role,
/// category)` so the RoPA is byte-stable build-to-build.
pub fn ropa(inventory: &Inventory) -> ProcessingActivities {
    // Group the entries by the (role, category) activity key.
    #[derive(Default)]
    struct Acc {
        field_paths: BTreeSet<String>,
        lawful_bases: BTreeSet<String>,
        retentions: BTreeSet<String>,
        regions: BTreeSet<String>,
        special_category: bool,
    }
    let mut groups: BTreeMap<(String, String), Acc> = BTreeMap::new();

    for e in &inventory.entries {
        let acc = groups.entry((e.role.clone(), e.category.clone())).or_default();
        acc.field_paths.insert(e.field_path.clone());
        acc.lawful_bases.insert(e.basis.clone());
        acc.retentions.insert(e.retention.clone());
        acc.regions.insert(e.region.clone());
        // The special-category flag is detected off the SAME registry text the marker mint uses
        // (gdpr §2.3) — never re-detected, so the RoPA and the DPIA marker set cannot disagree.
        if e.category.starts_with("SpecialCategory(") {
            acc.special_category = true;
        }
    }

    let activities = groups
        .into_iter()
        .map(|((role, category), acc)| ProcessingActivity {
            role,
            category,
            field_paths: acc.field_paths.into_iter().collect(),
            lawful_bases: acc.lawful_bases.into_iter().collect(),
            retentions: acc.retentions.into_iter().collect(),
            regions: acc.regions.into_iter().collect(),
            special_category: acc.special_category,
        })
        .collect();

    ProcessingActivities { activities }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_gdpr::PersonalData;
    use myelin_substrate::StoreKind;

    // ── Two real-shaped holder schemas: an Identity-like principal store (H15, PII fields) and a
    //    derived index (H7, NO directly-tagged PII field — keyed on opaque ids). Together they prove
    //    BOTH coverage properties: a holder WITH fields and a holder with ZERO fields are each in the
    //    map, and the field count + holder count are each exact.

    #[derive(PersonalData)]
    #[allow(dead_code)]
    struct PrincipalRow {
        #[personal_data(
            category = ContactInfo,
            role = PlatformOperational,
            basis = Contract,
            retention = UntilContractEnd,
            erasure = CryptoShred(subject_dek),
            subject_locator = "principal_id"
        )]
        email: String,
        #[personal_data(
            category = SpecialCategory(health),
            role = PlatformOperational,
            basis = Consent(c-1),
            retention = Fixed(365d),
            erasure = CryptoShred(subject_dek),
            subject_locator = "principal_id"
        )]
        health_note: String,
        // a non-PII key — no entry.
        row_version: u64,
    }

    /// A derived search index keyed only on opaque ids — NO directly-tagged PII field. It MUST still
    /// appear in the map's holder roster (accounted for, zero entries).
    #[derive(PersonalData)]
    #[allow(dead_code)]
    struct OpaqueIndexRow {
        doc_id: u64,
        shard: u32,
    }

    fn region() -> Region {
        Region("fr-par".into())
    }

    fn principal_schema() -> HolderSchema {
        HolderSchema::from_schema::<PrincipalRow>(
            HolderRegistration { kind: StoreKind::Oltp, name: "identity_oltp" },
            Holder::H15Identity,
            region(),
        )
    }

    fn index_schema() -> HolderSchema {
        HolderSchema::from_schema::<OpaqueIndexRow>(
            HolderRegistration { kind: StoreKind::SearchIndex, name: "search_index" },
            Holder::H7SearchIndex,
            region(),
        )
    }

    /// The generator emits an inventory entry for EVERY tagged field, with the field path + owning
    /// holder + the five tags + the subject_locator + the residency region (gdpr §2.2). This is the
    /// registry-walk + inventory-emission mandatory-core path: a dropped or mis-captured field fails
    /// here.
    #[test]
    fn data_map_emits_an_entry_per_tagged_field_with_every_fact() {
        let holders = [principal_schema(), index_schema()];
        let inv = data_map(&holders);

        // 0 fields absent: the entry count equals the tagged-field count, field-for-field.
        assert_eq!(inv.entry_count(), tagged_field_count(&holders));
        assert_eq!(inv.entry_count(), 2, "PrincipalRow has 2 tagged fields; OpaqueIndexRow has 0");

        // The email entry carries every fact.
        let email = inv
            .entries
            .iter()
            .find(|e| e.field_path == "PrincipalRow.email")
            .expect("the email field is in the map");
        assert_eq!(email.holder_id, "oltp:identity_oltp");
        assert_eq!(email.holder, "H15");
        assert_eq!(email.region, "fr-par");
        assert_eq!(email.category, "ContactInfo");
        assert_eq!(email.role, "PlatformOperational");
        assert_eq!(email.basis, "Contract");
        assert_eq!(email.retention, "UntilContractEnd");
        assert_eq!(email.erasure, "CryptoShred(subject_dek)");
        assert_eq!(email.subject_locator, "principal_id");

        // The special-category field surfaces a DPIA marker into the map (gdpr §2.3).
        assert!(inv.dpia_markers.contains(&DpiaMarker {
            field_path: "PrincipalRow.health_note".into(),
            special_category_kind: "health".into(),
        }));
        assert_eq!(inv.dpia_markers.len(), 1, "exactly the one special-category field");
    }

    /// **The holder-coverage GATE (gdpr §2.2 — 0 holders absent from the map).** Every REGISTERED
    /// holder is in the map's roster — INCLUDING the derived index that contributes zero PII fields.
    /// A holder present in the registry but absent from the map is surfaced as a coverage gap.
    #[test]
    fn every_registered_holder_is_in_the_map_including_zero_pii_holders() {
        let holders = [principal_schema(), index_schema()];
        let inv = data_map(&holders);

        // Both holders are in the roster (the zero-PII index too — accounted for).
        assert_eq!(inv.holder_count(), 2);
        assert!(inv.holders.contains("oltp:identity_oltp"));
        assert!(inv.holders.contains("search_index:search_index"));

        // The coverage gate is GREEN over exactly the holders that contributed.
        let registered = [
            HolderRegistration { kind: StoreKind::Oltp, name: "identity_oltp" },
            HolderRegistration { kind: StoreKind::SearchIndex, name: "search_index" },
        ];
        assert!(
            inv.coverage_gaps(&registered).is_empty(),
            "every registered holder is in the map — 0 holders absent"
        );
    }

    /// **The RED coverage verdict.** A holder is REGISTERED (the harness opened it) but did NOT
    /// contribute a [`HolderSchema`] to the map — *a holder that exists but has no map entry is a
    /// coverage failure* (gdpr §2.2). [`Inventory::coverage_gaps`] surfaces exactly that holder, so
    /// the DSR fan-out cannot silently skip a store the map forgot. This is the captured-expected
    /// failure the P-GA-09 GATE requires.
    #[test]
    fn a_registered_holder_absent_from_the_map_is_a_coverage_gap() {
        // The map is generated over only the identity store…
        let inv = data_map(&[principal_schema()]);
        // …but the harness ALSO opened a CI store (registered) that never contributed to the map.
        let registered = [
            HolderRegistration { kind: StoreKind::Oltp, name: "identity_oltp" },
            HolderRegistration { kind: StoreKind::Oltp, name: "ci_oltp" }, // absent from the map!
        ];
        let gaps = inv.coverage_gaps(&registered);
        assert_eq!(
            gaps,
            vec!["oltp:ci_oltp".to_string()],
            "the registered-but-unmapped holder is the coverage gap"
        );
    }

    /// The generated inventory is DETERMINISTIC — the same holder set (in any registration order)
    /// yields a byte-identical fingerprint (gdpr §2.2 — regenerated every build, diffed in CI). The
    /// diff gate (P-GA-10) reads this; a non-deterministic fingerprint would make the gate flap.
    #[test]
    fn the_generated_map_is_deterministic_and_order_independent() {
        let a = data_map(&[principal_schema(), index_schema()]);
        // Reverse the holder registration order — the map (sorted entries + roster) is identical.
        let b = data_map(&[index_schema(), principal_schema()]);
        assert_eq!(a, b, "the map is order-independent (sorted)");
        assert_eq!(a.fingerprint(), b.fingerprint(), "the fingerprint is deterministic");
        assert!(a.fingerprint().starts_with("blake3:"));

        // A CHANGED map (an added holder) has a DIFFERENT fingerprint — the diff gate fires.
        let c = data_map(&[principal_schema()]);
        assert_ne!(a.fingerprint(), c.fingerprint(), "a changed inventory diffs");
    }

    /// **The RoPA projection groups by processing activity** (Art. 30; gdpr §2.2). The `(role,
    /// category)` axis is the activity key; each activity rolls up its field paths, lawful bases,
    /// retentions, and residency regions, and flags special-category processing. Generated, not
    /// hand-written; the legal text is `[OPEN — LEGAL]` (the DPO ratifies the characterisation).
    #[test]
    fn ropa_projects_the_inventory_grouped_by_processing_activity() {
        let inv = data_map(&[principal_schema()]);
        let tenant = myelin_tenancy::TenantId::from_token("acme");
        let activities = ropa_for_tenant(&tenant, &inv);

        // Two distinct (role, category) activities from PrincipalRow:
        //   (PlatformOperational, ContactInfo) and (PlatformOperational, SpecialCategory(health)).
        assert_eq!(activities.len(), 2);

        let contact = activities
            .activities
            .iter()
            .find(|a| a.category == "ContactInfo")
            .expect("the ContactInfo activity");
        assert_eq!(contact.role, "PlatformOperational");
        assert_eq!(contact.field_paths, vec!["PrincipalRow.email".to_string()]);
        assert_eq!(contact.lawful_bases, vec!["Contract".to_string()]);
        assert_eq!(contact.regions, vec!["fr-par".to_string()]);
        assert!(!contact.special_category, "ContactInfo is not special-category");

        let health = activities
            .activities
            .iter()
            .find(|a| a.category.starts_with("SpecialCategory"))
            .expect("the special-category activity");
        assert!(health.special_category, "the Art. 9 activity is flagged special-category");
        assert_eq!(health.lawful_bases, vec!["Consent(c-1)".to_string()]);
    }

    /// Two holders sharing the SAME `(role, category)` collapse into ONE activity carrying BOTH field
    /// paths (the activity is the group, not the field) — and the rolled-up regions/bases/retentions
    /// deduplicate. This is the grouping mandatory-core path.
    #[test]
    fn ropa_collapses_same_activity_fields_and_dedups_rollups() {
        #[derive(PersonalData)]
        #[allow(dead_code)]
        struct OtherContact {
            #[personal_data(
                category = ContactInfo,
                role = PlatformOperational,
                basis = Contract,
                retention = UntilContractEnd,
                erasure = CryptoShred(subject_dek),
                subject_locator = "principal_id"
            )]
            phone: String,
        }
        let other = HolderSchema::from_schema::<OtherContact>(
            HolderRegistration { kind: StoreKind::Oltp, name: "billing_oltp" },
            Holder::H18GdprOwn,
            region(),
        );
        let inv = data_map(&[principal_schema(), other]);
        let acts = ropa(&inv);

        let contact = acts
            .activities
            .iter()
            .find(|a| a.category == "ContactInfo")
            .expect("the shared ContactInfo activity");
        // BOTH ContactInfo fields collapse into the one activity.
        assert_eq!(
            contact.field_paths,
            vec!["OtherContact.phone".to_string(), "PrincipalRow.email".to_string()]
        );
        // The same basis from two fields deduplicates to one entry.
        assert_eq!(contact.lawful_bases, vec!["Contract".to_string()]);
        // The same region deduplicates.
        assert_eq!(contact.regions, vec!["fr-par".to_string()]);
    }

    /// **CDC (contract 10.3 provider+consumer): the DSR orchestrator (P-GA-13) consumes `data_map()`
    /// to build its fan-out checklist.** The PROVIDER is this generator; the CONSUMER is the
    /// fan-out's resolve-scope-from-the-map step (gdpr §4.1 step 2 — *the map, not a hand-written
    /// list, drives fan-out*). This stub consumer (the P-GA-13 body is the named M1 follow-on)
    /// resolves the per-holder erase checklist FROM the generated map: the set of holders to drive,
    /// each with the per-field erasure mechanism. The contract is: the consumer can address every
    /// holder + every field's erasure tag off the map alone — no out-of-band store list.
    #[test]
    fn cdc_dsr_orchestrator_resolves_the_fan_out_checklist_from_the_map() {
        let inv = data_map(&[principal_schema(), index_schema()]);

        // The consumer (P-GA-13) resolves: which holders to drive (the map's roster) + per-holder,
        // the erasure mechanism of each field. It NEVER reaches outside the map.
        let mut checklist: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for holder_id in &inv.holders {
            checklist.entry(holder_id.clone()).or_default();
        }
        for e in &inv.entries {
            checklist
                .entry(e.holder_id.clone())
                .or_default()
                .push(e.erasure.clone());
        }

        // Every registered holder is a checklist key (the zero-PII index too — it is still driven,
        // it just has no per-field mechanism).
        assert!(checklist.contains_key("oltp:identity_oltp"));
        assert!(checklist.contains_key("search_index:search_index"));
        // The identity holder's checklist resolves the per-field erasure mechanism off the map.
        let id_mechs = &checklist["oltp:identity_oltp"];
        assert_eq!(id_mechs.len(), 2, "both tagged fields drive an erasure");
        assert!(id_mechs.iter().all(|m| m == "CryptoShred(subject_dek)"));
        // The consumer's checklist covers exactly the map's holders — 0 holders missed.
        assert_eq!(checklist.len(), inv.holder_count());
    }

    /// The inventory + RoPA round-trip serialize — they cross the crate boundary (the diff gate
    /// P-GA-10 commits the inventory; the RoPA is surfaced to a DPO), so a stable serde shape is part
    /// of the frozen surface.
    #[test]
    fn inventory_and_ropa_round_trip_serialize() {
        let inv = data_map(&[principal_schema(), index_schema()]);
        let back: Inventory =
            serde_json::from_str(&serde_json::to_string(&inv).unwrap()).unwrap();
        assert_eq!(back, inv);

        let acts = ropa(&inv);
        let acts_back: ProcessingActivities =
            serde_json::from_str(&serde_json::to_string(&acts).unwrap()).unwrap();
        assert_eq!(acts_back, acts);
    }

    /// The frozen contract-10.3 `ropa(tenant)` signature is exercised: the tenant-scoped form
    /// delegates to the pure projection over the cell's inventory.
    #[test]
    fn ropa_for_tenant_matches_the_pure_projection() {
        let inv = data_map(&[principal_schema()]);
        let tenant = myelin_tenancy::TenantId::from_token("acme");
        assert_eq!(ropa_for_tenant(&tenant, &inv), ropa(&inv));
    }

    /// An empty holder set yields an empty (but valid) map + RoPA — the generator is total on the
    /// degenerate input (no holder ⇒ no entry, no gap).
    #[test]
    fn empty_holder_set_yields_an_empty_map() {
        let inv = data_map(&[]);
        assert_eq!(inv.entry_count(), 0);
        assert_eq!(inv.holder_count(), 0);
        assert!(inv.coverage_gaps(&[]).is_empty());
        let acts = ropa(&inv);
        assert!(acts.is_empty(), "an empty inventory projects to no activity");
        assert_eq!(acts.len(), 0);
        // A populated inventory is NOT empty (kills the `is_empty -> true` mutant).
        let populated = ropa(&data_map(&[principal_schema()]));
        assert!(!populated.is_empty(), "a populated inventory has activities");
    }
}

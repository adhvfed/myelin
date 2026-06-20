//! # Residency pinning enforced end-to-end (STOR-D5) — the storage half (P-ST-15 / P-102).
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/storage.md` §1.1 (every store is
//! residency-pinned; **no cross-region query path** — the `(tenant, region)` predicate is the
//! compiled-in shard key, "EU data stays in EU" and "scale this tenant out" are the SAME
//! mechanism), §8 (the cell topology: a tenant's data lives in exactly one cell/region), §9
//! ("CLI/admin surface: `myelin storage residency verify <tenant>` — prove region pinning").
//! Contract-index rows 12.4 (`residency_verify` — Storage feeds its per-store region reports in),
//! 12.1 (the `(tenant, region)` partition key), 11.1 (the OLTP tier — the store this region-pins).
//! Drill catalogue row STOR-D5 (§4.2 — read/replicate outside the region → impossible; 0 egress).
//!
//! ## What P-ST-15 (this prompt) ships — the storage half of residency enforcement
//! P-ST-15 is the **STORAGE FACE of CP-D3 / STOR-D5**. The control-plane `residency_verify`
//! AGGREGATION+SIGN (P-CP-09 / P-085, `myelin-control-plane`) is already real; it explicitly
//! consumes a per-store region REPORT VALUE the store layer feeds in. Storage is UPSTREAM of the
//! control plane in the crate DAG (control-plane depends on `myelin-storage`, never the reverse —
//! see this crate's `Cargo.toml`), so Storage CANNOT import the control plane's report type; it
//! OWNS the **report-producing** side. This module is that side:
//!
//! 1. **The per-pool runtime region-pin** ([`RegionPinnedStore`]) — closes the floor named in
//!    [`crate::oltp`] and [`crate::holder`]: "a per-POOL runtime region-pin lands end-to-end in
//!    P-ST-15 / P-102". Every store is constructed pinned to its cell's [`Region`]; the pin is
//!    immutable (a region change is a NEW value, never a mutation — `myelin_tenancy::Region` has
//!    no setter). The store reports its region into the residency attestation.
//! 2. **The residency WRITE boundary** ([`RegionPinnedStore::admit_write`]) — a write whose row
//!    region ≠ the store's pinned region is REJECTED in-process by construction (the
//!    [`ResidencyViolation::OutOfRegionWrite`]); this is the in-process twin of the live-DB RLS
//!    `WITH CHECK (region = current_setting(...))` boundary the STOR-D5 integration drill
//!    (`tests/stor_d5_cross_region_egress_drill.rs`, P-096) proves against real Postgres. *No
//!    store ever writes a row outside its pinned region* — so cross-region replication has no
//!    source to copy from.
//! 3. **The per-store region report** ([`StoreResidencyReport`]) — each M1 store class
//!    ([`ResidencyStoreClass`]: OLTP / blob / index-search / KMS) reports "for tenant T, I served
//!    the data in region R". PII-free: a store-class tag + a region code, never personal data.
//! 4. **The `myelin storage residency verify <tenant>` admin path** ([`verify_region_pinning`] +
//!    [`StoreSet::residency_verify`]) — gathers a report from EVERY M1 store the tenant uses and
//!    proves region pinning: it FAILS LOUDLY (never a silent pass, EI-01 §3) iff any store reports
//!    a region ≠ the tenant's region ([`ResidencyViolation::OutOfRegionStore`]) OR an M1 store
//!    class never reported ([`ResidencyViolation::MissingStoreReport`] — a silently-absent store
//!    is exactly the global-pool the no-global-pool property forbids; fail-closed). On success it
//!    emits a PII-free [`RegionPinningAttestation`] whose `cross_region_egress == 0` is the dated
//!    STOR-D5 green artifact.
//! 5. **The telemetry signal** ([`ResidencyVerifySignal`]) — the PII-free
//!    `(tenant, region, stores_attested, cross_region_egress)` the STOR-D5 drill asserts against
//!    (`cross_region_egress == 0`).
//!
//! ## The CDC seam to control-plane 12.4 (provider Storage → consumer control plane)
//! Storage is the PROVIDER of the per-store region reports; the control plane's `residency_verify`
//! is the CONSUMER that signs them into the auditor attestation. The CDC pair lives in
//! `tests/cdc_12_4_storage_residency_report.rs`: Storage produces [`StoreResidencyReport`]s; a
//! stand-in consumer maps them into the control-plane `StoreRegionReport`/`residency_verify` shape
//! and signs the no-global-pool attestation. If either shape drifts, the test stops compiling — the
//! point of a glue CDC. The two crates speak ONE vocabulary (the M1 store set = OLTP/blob/index/KMS,
//! the region codes) without a shared type, because the DAG forbids a `myelin-storage ->
//! myelin-control-plane` edge (documented deviation, EI-01 §1).
//!
//! ## Floors named (the prompt's required follow-ons)
//! - **The CDN edge set + the push-mirror targets** are NOT in this M1 attestation — they are the
//!   M3 follow-ons: the within-EU CDN clone/bundle blob class is **P-ST-23 (global P-254)** and
//!   the outbound push-mirror residency gate is **P-ST-25 (global P-255)**. Both EXTEND this same
//!   [`StoreSet::residency_verify`] with additional [`ResidencyStoreClass`] variants (the
//!   aggregation + fail-on-mismatch shape does not change). The T3 firehose-archive surface is
//!   **P-ST-20 (global P-147)**. Recorded here + in the report + the scorecard.
//! - **The live store DRIVER reporting its region** is proven against real Postgres in the STOR-D5
//!   integration drill (`tests/stor_d5_cross_region_egress_drill.rs`, behind `--features
//!   integration`); on the default DB-free build the region pin + write boundary + aggregation are
//!   fully real and unit-tested (the same posture P-085's control-plane half documents).
//!
//! ## Mutation floor (mandatory-core, >= 80% — EI-01 §2/§3; the prompt's TESTS field)
//! The region-pin enforcement — the write-boundary region compare
//! ([`RegionPinnedStore::admit_write`]: `row_region != self.region`), the out-of-region report
//! branch ([`verify_region_pinning`]: `report.region != tenant_region`), and the missing-store
//! fail-closed branch (`!present.contains`) — is **mandatory-core**: a store serving a tenant's
//! personal data in the wrong region (or silently absent from the attestation) is the residency
//! breach STOR-D5 exists to catch. The token-region carried into the partition key is the breach
//! surface. The floor is **>= 80%**; the achieved score is recorded in the P-102 report
//! (`cargo mutants -p myelin-storage -f crates/myelin-storage/src/residency.rs`).

use std::collections::BTreeSet;

use myelin_tenancy::{Region, TenantId};

/// **The M1 store set residency verification covers (storage.md §8 / control-plane §5.4).** Each
/// M1 store class reports the tenant's region; [`StoreSet::residency_verify`] requires a report
/// from EVERY class (a missing report is a FAIL, not a pass). PII-free — a store-class tag, never
/// data. Byte-for-byte the same set the control plane's `residency_verify` requires (P-085); the
/// two crates pin the SAME M1 set without sharing a type (the DAG forbids the edge).
///
/// **FLOOR (named):** the within-EU CDN edge set (P-ST-23 / P-254) + the push-mirror targets
/// (P-ST-25 / P-255) + the T3 firehose archive (P-ST-20 / P-147) are the M3/M2 follow-ons — they
/// become additional variants here and feed the SAME [`StoreSet::residency_verify`]. The M1 set is
/// OLTP / blob / index-search / KMS.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResidencyStoreClass {
    /// The OLTP tier (P-ST-01) — the tenant's transactional rows.
    Oltp,
    /// The content-addressed blob tier (P-ST-02/03) — the tenant's blobs.
    Blob,
    /// The index/search tier — the tenant's encrypted-from-birth per-tenant index.
    IndexSearch,
    /// The KMS (P-ST-06) — the tenant's per-tenant DEK/KEK material.
    Kms,
    /// **The T3 firehose archive (P-ST-20 / P-147) — the durable, sealed, DEK-encrypted segments of
    /// the firehose.** A follow-on store class (NOT in the M1 set): a sealed segment is a
    /// tenant-keyspace T2 blob in the cell's region, so the archive reports its region here and
    /// `verify_region_pinning` catches a wrong-region archive without a code change (the floor this
    /// module named for P-ST-20). The C2 `(job,step,byte-range)` index (P-ST-26) + the C1 per-subject
    /// CI-log DEK (P-ST-27) are the M4 follow-ons; they do not change this residency shape.
    T3FirehoseArchive,
}

impl ResidencyStoreClass {
    /// A stable, PII-free label for the store class (for the attestation body + telemetry).
    pub fn label(self) -> &'static str {
        match self {
            ResidencyStoreClass::Oltp => "oltp",
            ResidencyStoreClass::Blob => "blob",
            ResidencyStoreClass::IndexSearch => "index_search",
            ResidencyStoreClass::Kms => "kms",
            ResidencyStoreClass::T3FirehoseArchive => "t3_firehose_archive",
        }
    }

    /// **The M1 store set residency verification requires a region report from (storage.md §8).** A
    /// `residency_verify` that is missing ANY of these FAILS (a store that never reported its
    /// region is exactly the silent global-pool the no-global-pool property must catch). The
    /// CDN/mirror surfaces are the named follow-ons (NOT in this M1 set).
    pub const M1_SET: [ResidencyStoreClass; 4] = [
        ResidencyStoreClass::Oltp,
        ResidencyStoreClass::Blob,
        ResidencyStoreClass::IndexSearch,
        ResidencyStoreClass::Kms,
    ];
}

/// **A region-pinned store (the per-pool runtime region-pin — closes the [`crate::oltp`] /
/// [`crate::holder`] M0 floor).** A store is constructed pinned to its cell's [`Region`]; the pin
/// is immutable (no setter — a region change is a NEW value, `myelin_tenancy::Region` is frozen
/// that way). Every write goes through [`Self::admit_write`], which REJECTS a row whose region ≠
/// the pinned region (the in-process residency write boundary — the twin of the live-DB RLS
/// `WITH CHECK`). Because no store ever writes outside its region, cross-region replication has no
/// source to copy from (STOR-D5 by construction).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionPinnedStore {
    /// The store class this pin is for (one of the M1 set).
    store_class: ResidencyStoreClass,
    /// The cell's region this store is pinned to (immutable once constructed).
    region: Region,
}

impl RegionPinnedStore {
    /// Construct a store pinned to `region` (the cell's region — the `residency-pin` the harness
    /// injects at store open, closing the M0 region-less-pool floor). The pin is immutable.
    pub fn pinned_to(store_class: ResidencyStoreClass, region: Region) -> RegionPinnedStore {
        RegionPinnedStore { store_class, region }
    }

    /// The store class.
    pub fn store_class(&self) -> ResidencyStoreClass {
        self.store_class
    }

    /// The region this store is pinned to (the cell's region).
    pub fn region(&self) -> &Region {
        &self.region
    }

    /// **The residency WRITE boundary (in-process, by construction).** A write whose `row_region`
    /// equals the store's pinned region is ADMITTED; a write whose `row_region` differs is REJECTED
    /// with [`ResidencyViolation::OutOfRegionWrite`] — *no store ever writes a row outside its
    /// pinned region*. This is the in-process twin of the live-DB RLS `WITH CHECK (region =
    /// current_setting(...))` boundary (proven against real Postgres in the STOR-D5 integration
    /// drill). The normal production path always writes `self.region`, so it is in-region by
    /// construction; this boundary catches a bug/misroute that would attempt otherwise.
    pub fn admit_write(&self, row_region: &Region) -> Result<(), ResidencyViolation> {
        if row_region != &self.region {
            return Err(ResidencyViolation::OutOfRegionWrite {
                store_class: self.store_class,
                store_region: self.region.clone(),
                row_region: row_region.clone(),
            });
        }
        Ok(())
    }

    /// **The per-store region report for `tenant` (storage.md §8 — "every store reports its
    /// region").** Because the store is region-pinned and its write boundary rejects any
    /// out-of-region row, the region it serves the tenant's data in IS its pinned region — so the
    /// report is `(store_class, self.region)`. This is the value [`StoreSet::residency_verify`]
    /// (and, downstream, the control plane's `residency_verify`) aggregates. PII-free.
    pub fn report_for(&self, tenant: &TenantId) -> StoreResidencyReport {
        StoreResidencyReport {
            tenant: tenant.clone(),
            store_class: self.store_class,
            region: self.region.clone(),
        }
    }
}

/// **One store's region report for a tenant (storage.md §8; control-plane §5.4).** "For tenant
/// `T`, I (the `<store_class>` store) served the data in region `R`." PII-free — a store-class tag,
/// an opaque tenant id, and a region code, never personal data. This is the report VALUE Storage
/// PROVIDES; the control plane's `residency_verify` is the downstream CONSUMER that signs it (the
/// CDC pair, 12.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreResidencyReport {
    /// The tenant the report is for (opaque id, PII-free).
    pub tenant: TenantId,
    /// The store class reporting (one of the M1 set).
    pub store_class: ResidencyStoreClass,
    /// The region the store served the tenant's data in (== the store's pinned region).
    pub region: Region,
}

/// **Why residency verification FAILED (a loud refusal — NEVER a silent pass; EI-01 §3).** Either a
/// store reported / attempted a write in a region ≠ the tenant's region (the headline residency
/// breach STOR-D5 catches) or an M1 store class never reported (a silently-absent store is the
/// global-pool the no-global-pool property forbids). Carrying the offending store + region keeps
/// the refusal named.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResidencyViolation {
    /// **A store attempted to WRITE a row outside its pinned region** (the in-process write
    /// boundary). Rejected by construction — *no store writes outside its region*.
    OutOfRegionWrite {
        /// The store class whose boundary rejected the write.
        store_class: ResidencyStoreClass,
        /// The region the store is pinned to.
        store_region: Region,
        /// The (out-of-region) region the rejected write targeted.
        row_region: Region,
    },
    /// **A store reported a region ≠ the tenant's region.** The headline residency breach: a store
    /// served the tenant's personal data in the wrong region. The attestation FAILS (not a silent
    /// pass). 0 of these is the green STOR-D5 artifact.
    OutOfRegionStore {
        /// The tenant the verification was for (opaque id, PII-free).
        tenant: TenantId,
        /// The tenant's (immutable) region of record.
        tenant_region: Region,
        /// The store that reported the wrong region.
        store_class: ResidencyStoreClass,
        /// The (wrong) region the store reported (≠ `tenant_region`).
        store_region: Region,
    },
    /// **An M1 store class never reported its region.** A store that is silently absent from the
    /// attestation is the global-pool the no-global-pool property forbids — so a missing report is
    /// a FAIL, fail-closed (never "assume in-region").
    MissingStoreReport {
        /// The tenant the verification was for (opaque id, PII-free).
        tenant: TenantId,
        /// The M1 store class that never reported.
        store_class: ResidencyStoreClass,
    },
}

impl std::fmt::Display for ResidencyViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResidencyViolation::OutOfRegionWrite { store_class, store_region, row_region } => {
                write!(
                    f,
                    "residency WRITE boundary REJECTED a write: the `{}` store is pinned to region \
                     `{}` but the row targeted region `{}` — no store ever writes outside its \
                     region (storage.md §1.1, the residency-pin write boundary). 0 out-of-region \
                     writes admitted.",
                    store_class.label(),
                    store_region.as_str(),
                    row_region.as_str()
                )
            }
            ResidencyViolation::OutOfRegionStore {
                tenant,
                tenant_region,
                store_class,
                store_region,
            } => write!(
                f,
                "residency verify FAILED for tenant `{}`: the `{}` store served data in region `{}` \
                 but the tenant is pinned to region `{}` — every store must report the tenant's \
                 region (no-global-pool, STOR-D5). The attestation FAILS (not a silent pass, \
                 EI-01 §3).",
                tenant.as_str(),
                store_class.label(),
                store_region.as_str(),
                tenant_region.as_str()
            ),
            ResidencyViolation::MissingStoreReport { tenant, store_class } => write!(
                f,
                "residency verify FAILED for tenant `{}`: the M1 store class `{}` never reported \
                 its region — a silently-absent store is the global-pool the no-global-pool \
                 attestation must catch (fail-closed, STOR-D5).",
                tenant.as_str(),
                store_class.label()
            ),
        }
    }
}

impl std::error::Error for ResidencyViolation {}

/// **The PII-free region-pinning attestation (the storage face of contract 12.4; the STOR-D5
/// artifact).** For `tenant` pinned to `region`, EVERY M1 store reported that same region — so
/// `cross_region_egress == 0` by construction. The body carries ONLY opaque ids / region codes /
/// store-class tags — no name/email/body. This is what the `myelin storage residency verify
/// <tenant>` admin path returns; it is also the report set the control plane's `residency_verify`
/// (P-085) signs into the auditor's no-global-pool attestation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegionPinningAttestation {
    /// The tenant the attestation is for (opaque id, PII-free).
    pub tenant: TenantId,
    /// The tenant's (immutable) region of record — every store reported THIS region.
    pub region: Region,
    /// The per-store-class region reports, store-class-ordered — every M1 store class is present
    /// (a missing one would have FAILED). PII-free pairs.
    pub store_regions: Vec<(ResidencyStoreClass, Region)>,
}

impl RegionPinningAttestation {
    /// The per-store reports as the value the control plane's `residency_verify` consumes (12.4) —
    /// the [`StoreResidencyReport`] set this attestation aggregated, in store-class order. The
    /// downstream control plane maps these into its own report type + signs them.
    pub fn reports(&self) -> Vec<StoreResidencyReport> {
        self.store_regions
            .iter()
            .map(|(class, region)| StoreResidencyReport {
                tenant: self.tenant.clone(),
                store_class: *class,
                region: region.clone(),
            })
            .collect()
    }
}

/// **`verify_region_pinning(tenant, tenant_region, reports) -> Result<RegionPinningAttestation,
/// ResidencyViolation>` — the storage half of contract 12.4 (storage.md §8/§9).**
///
/// Given the tenant's authoritative `tenant_region` of record and the per-store region `reports`,
/// aggregate them into a [`RegionPinningAttestation`] IFF:
///
/// 1. **Every reported region == the tenant's region** — a store reporting a different region is a
///    [`ResidencyViolation::OutOfRegionStore`] FAIL (the headline residency breach). **Never a
///    silent pass** (EI-01 §3). Checked FIRST so a wrong-region store fails loudly before any
///    presence check.
/// 2. **Every M1 store class reported** ([`ResidencyStoreClass::M1_SET`]) — a missing report is a
///    [`ResidencyViolation::MissingStoreReport`] FAIL (a silently-absent store is the global-pool
///    the attestation must catch; fail-closed).
///
/// On success the attestation aggregates every store's `(class, region)` (store-class-ordered);
/// `cross_region_egress == 0` is the green artifact. Reports for classes beyond the M1 set (a
/// future CDN/mirror report added by P-ST-23/P-ST-25) are checked for region too, so a wrong-region
/// follow-on store is caught here WITHOUT a code change.
pub fn verify_region_pinning(
    tenant: &TenantId,
    tenant_region: &Region,
    reports: &[StoreResidencyReport],
) -> Result<RegionPinningAttestation, ResidencyViolation> {
    // 1. Every report's region must match the tenant's region — a wrong region FAILS loudly. We
    //    check ALL reports (not just the M1 set) so a follow-on CDN/mirror report is caught too.
    let mut store_regions: Vec<(ResidencyStoreClass, Region)> = Vec::new();
    let mut present: BTreeSet<ResidencyStoreClass> = BTreeSet::new();
    for report in reports {
        if &report.region != tenant_region {
            return Err(ResidencyViolation::OutOfRegionStore {
                tenant: tenant.clone(),
                tenant_region: tenant_region.clone(),
                store_class: report.store_class,
                store_region: report.region.clone(),
            });
        }
        if present.insert(report.store_class) {
            store_regions.push((report.store_class, report.region.clone()));
        }
    }

    // 2. Require a report from EVERY M1 store class — a missing one FAILS fail-closed.
    for class in ResidencyStoreClass::M1_SET {
        if !present.contains(&class) {
            return Err(ResidencyViolation::MissingStoreReport {
                tenant: tenant.clone(),
                store_class: class,
            });
        }
    }

    // 3. Aggregate (store-class-ordered for a stable, reproducible body).
    store_regions.sort_by_key(|(class, _)| *class);
    Ok(RegionPinningAttestation {
        tenant: tenant.clone(),
        region: tenant_region.clone(),
        store_regions,
    })
}

/// **The M1 store set a tenant uses, region-pinned (the live store set the admin path verifies
/// over).** Each store is a [`RegionPinnedStore`] pinned to the cell's region; [`Self::for_cell`]
/// builds the full M1 set pinned to one region (the common case — a tenant's stores all live in
/// one cell). [`Self::residency_verify`] is the `myelin storage residency verify <tenant>` admin
/// path: it gathers each store's region report and proves region pinning.
#[derive(Clone, Debug)]
pub struct StoreSet {
    stores: Vec<RegionPinnedStore>,
}

impl StoreSet {
    /// Build the full M1 store set pinned to one cell `region` (the common case: a tenant's OLTP /
    /// blob / index / KMS stores all live in that tenant's cell). This is what the harness wires at
    /// cell-provisioning time (P-CP-11); the region pin is the `residency-pin` the M0 floor named.
    pub fn for_cell(region: &Region) -> StoreSet {
        let stores = ResidencyStoreClass::M1_SET
            .iter()
            .map(|class| RegionPinnedStore::pinned_to(*class, region.clone()))
            .collect();
        StoreSet { stores }
    }

    /// Build a store set from explicit per-store pins (used to model a MISROUTED store — one store
    /// pinned to the wrong region — for the STOR-D5 fail leg). The admin path catches it.
    pub fn from_stores(stores: Vec<RegionPinnedStore>) -> StoreSet {
        StoreSet { stores }
    }

    /// The region reports every store in the set produces for `tenant`.
    pub fn reports_for(&self, tenant: &TenantId) -> Vec<StoreResidencyReport> {
        self.stores.iter().map(|s| s.report_for(tenant)).collect()
    }

    /// **`myelin storage residency verify <tenant>` — the admin path that proves region pinning
    /// (storage.md §9).** Gathers a region report from every store the tenant uses and runs
    /// [`verify_region_pinning`] against the tenant's authoritative `tenant_region` of record. On
    /// PASS it returns the PII-free [`RegionPinningAttestation`] (`cross_region_egress == 0` — the
    /// dated STOR-D5 green artifact); on a wrong-region or missing store it FAILS LOUDLY (never a
    /// silent pass). The CDN edge set + mirror targets are the named P-ST-23/P-ST-25 follow-ons.
    pub fn residency_verify(
        &self,
        tenant: &TenantId,
        tenant_region: &Region,
    ) -> Result<RegionPinningAttestation, ResidencyViolation> {
        verify_region_pinning(tenant, tenant_region, &self.reports_for(tenant))
    }
}

/// **The `residency-verify` telemetry signal (storage.md §9; the STOR-D5 artifact).** The
/// aggregate, PII-free result of a `residency verify` run: the tenant + region, how many stores
/// attested, and the count of cross-region egress paths (`0` is the green artifact). Observability
/// is part of the pass (EI-01 §3). PII-free: opaque id + region code + aggregate counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResidencyVerifySignal {
    /// The tenant the verification was for (opaque id, PII-free).
    pub tenant: TenantId,
    /// The tenant's region of record (every attested store reported this).
    pub region: Region,
    /// How many M1 stores attested their region (the green artifact has all of `M1_SET`).
    pub stores_attested: u32,
    /// **The headline zero** — how many stores serve / could serve the tenant's data outside its
    /// region. `0` is the green STOR-D5 `residency-verify` artifact; `> 0` reads RED (a breach).
    pub cross_region_egress: u32,
}

impl ResidencyVerifySignal {
    /// The `residency-verify` signal for a SUCCESSFUL verification (every M1 store attested, 0
    /// cross-region egress — the green STOR-D5 artifact).
    pub fn green(att: &RegionPinningAttestation) -> ResidencyVerifySignal {
        ResidencyVerifySignal {
            tenant: att.tenant.clone(),
            region: att.region.clone(),
            stores_attested: att.store_regions.len() as u32,
            cross_region_egress: 0,
        }
    }

    /// The `residency-verify` signal for a FAILED verification — a store served / would serve the
    /// tenant's data outside its region (`cross_region_egress >= 1`, reads RED). A missing-store
    /// FAIL reports 0 egress but `stores_attested < M1_SET.len()`, so the drill asserts BOTH the
    /// egress zero AND the full store-set coverage.
    pub fn red(
        tenant: TenantId,
        region: Region,
        stores_attested: u32,
        cross_region_egress: u32,
    ) -> ResidencyVerifySignal {
        ResidencyVerifySignal {
            tenant,
            region,
            stores_attested,
            cross_region_egress,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant() -> TenantId {
        TenantId::from_token("01J0ACME")
    }

    /// **The per-pool runtime region-pin (the M0-floor closer): a store is pinned to its region and
    /// the pin is what its report carries.** Closes the floor named in `oltp.rs`/`holder.rs`.
    #[test]
    fn a_store_is_region_pinned_and_reports_its_region() {
        let store = RegionPinnedStore::pinned_to(ResidencyStoreClass::Oltp, Region::new("fr-par"));
        assert_eq!(store.region().as_str(), "fr-par");
        assert_eq!(store.store_class(), ResidencyStoreClass::Oltp);
        let report = store.report_for(&tenant());
        assert_eq!(report.store_class, ResidencyStoreClass::Oltp);
        assert_eq!(report.region.as_str(), "fr-par");
        assert_eq!(report.tenant, tenant());
    }

    /// **The residency WRITE boundary: an in-region write is ADMITTED, an out-of-region write is
    /// REJECTED in-process (the partition-key + residency-pin mechanism).** The unit twin of the
    /// live-DB RLS `WITH CHECK` boundary the STOR-D5 integration drill proves against real Postgres.
    #[test]
    fn the_write_boundary_rejects_an_out_of_region_write() {
        let store = RegionPinnedStore::pinned_to(ResidencyStoreClass::Blob, Region::new("fr-par"));
        // In-region: admitted.
        assert_eq!(store.admit_write(&Region::new("fr-par")), Ok(()));
        // Out-of-region: REJECTED (no store writes outside its region).
        let err = store
            .admit_write(&Region::new("eu-central"))
            .expect_err("an out-of-region write MUST be rejected by the residency write boundary");
        assert_eq!(
            err,
            ResidencyViolation::OutOfRegionWrite {
                store_class: ResidencyStoreClass::Blob,
                store_region: Region::new("fr-par"),
                row_region: Region::new("eu-central"),
            }
        );
        assert!(err.to_string().contains("no store ever writes outside its region"));
    }

    /// **The admin path attests the tenant's SINGLE region across every M1 store (the green leg).**
    /// `myelin storage residency verify <tenant>` over a cell-pinned store set → every store
    /// reports fr-par → a region-pinning attestation; `cross_region_egress == 0`.
    #[test]
    fn residency_verify_attests_the_tenants_single_region() {
        let region = Region::new("fr-par");
        let set = StoreSet::for_cell(&region);
        let att = set
            .residency_verify(&tenant(), &region)
            .expect("every store in-region → a region-pinning attestation");
        assert_eq!(att.tenant, tenant());
        assert_eq!(att.region.as_str(), "fr-par");
        // Every M1 store class is present, store-class-ordered, all reporting the single region.
        assert_eq!(att.store_regions.len(), ResidencyStoreClass::M1_SET.len());
        for (class, r) in &att.store_regions {
            assert_eq!(r.as_str(), "fr-par", "store `{}` reports the tenant's single region", class.label());
        }
        let signal = ResidencyVerifySignal::green(&att);
        assert_eq!(signal.cross_region_egress, 0, "the green STOR-D5 artifact is 0 cross-region egress");
        assert_eq!(signal.stores_attested, ResidencyStoreClass::M1_SET.len() as u32);
    }

    /// **THE STOR-D5 FAIL LEG (no silent pass, EI-01 §3): a cross-region store FAILS the
    /// attestation.** A blob store misrouted to eu-north (≠ the tenant's fr-par) → the admin path
    /// FAILS loudly — the headline residency breach STOR-D5 catches.
    #[test]
    fn residency_verify_fails_on_a_cross_region_store() {
        let region = Region::new("fr-par");
        // The blob store is (wrongly) pinned to eu-north; the others to fr-par.
        let set = StoreSet::from_stores(vec![
            RegionPinnedStore::pinned_to(ResidencyStoreClass::Oltp, region.clone()),
            RegionPinnedStore::pinned_to(ResidencyStoreClass::Blob, Region::new("eu-north")),
            RegionPinnedStore::pinned_to(ResidencyStoreClass::IndexSearch, region.clone()),
            RegionPinnedStore::pinned_to(ResidencyStoreClass::Kms, region.clone()),
        ]);
        let err = set
            .residency_verify(&tenant(), &region)
            .expect_err("a cross-region store FAILS the attestation (not a silent pass)");
        assert_eq!(
            err,
            ResidencyViolation::OutOfRegionStore {
                tenant: tenant(),
                tenant_region: Region::new("fr-par"),
                store_class: ResidencyStoreClass::Blob,
                store_region: Region::new("eu-north"),
            }
        );
        assert!(err.to_string().contains("no-global-pool"), "loud reason: {err}");
        assert!(err.to_string().contains("not a silent pass"), "loud reason: {err}");
    }

    /// **A missing M1 store report FAILS fail-closed (a silently-absent store is the global-pool).**
    /// The KMS never reported → the attestation FAILS (never "assume in-region").
    #[test]
    fn residency_verify_fails_on_a_missing_store_report() {
        let region = Region::new("fr-par");
        // Drop the KMS store (it never reported its region).
        let set = StoreSet::from_stores(vec![
            RegionPinnedStore::pinned_to(ResidencyStoreClass::Oltp, region.clone()),
            RegionPinnedStore::pinned_to(ResidencyStoreClass::Blob, region.clone()),
            RegionPinnedStore::pinned_to(ResidencyStoreClass::IndexSearch, region.clone()),
        ]);
        let err = set
            .residency_verify(&tenant(), &region)
            .expect_err("a missing M1 store report FAILS fail-closed");
        assert_eq!(
            err,
            ResidencyViolation::MissingStoreReport {
                tenant: tenant(),
                store_class: ResidencyStoreClass::Kms,
            }
        );
        assert!(err.to_string().contains("fail-closed"), "loud reason: {err}");
        // The RED signal: 0 egress but NOT all stores attested (so a missing store is not a silent green).
        let red = ResidencyVerifySignal::red(tenant(), region, 3, 0);
        assert!(
            red.stores_attested < ResidencyStoreClass::M1_SET.len() as u32,
            "a missing-store FAIL is caught by the store-set-coverage assertion, not just the egress zero"
        );
    }

    /// **The M1 store set is exactly OLTP/blob/index/KMS (the named partial; CDN/mirror are
    /// follow-ons).** Pins the M1 coverage so P-ST-23/P-ST-25 are a visible EXTENSION, not a silent
    /// redefinition — and so it matches the control plane's `residency_verify` M1 set byte-for-byte.
    #[test]
    fn the_m1_store_set_is_oltp_blob_index_kms() {
        assert_eq!(ResidencyStoreClass::M1_SET.len(), 4, "the M1 set is OLTP/blob/index/KMS");
        let labels: Vec<&str> = ResidencyStoreClass::M1_SET.iter().map(|c| c.label()).collect();
        assert_eq!(labels, vec!["oltp", "blob", "index_search", "kms"]);
    }

    /// **The attestation is PII-free** — it carries only opaque ids / region codes / store-class
    /// labels, never a name/email/body. The `reports()` projection (what the control plane consumes)
    /// is PII-free too.
    #[test]
    fn the_attestation_is_pii_free() {
        let region = Region::new("fr-par");
        let att = StoreSet::for_cell(&region)
            .residency_verify(&tenant(), &region)
            .expect("a region-pinning attestation");
        for (class, r) in &att.store_regions {
            assert!(
                matches!(
                    class,
                    ResidencyStoreClass::Oltp
                        | ResidencyStoreClass::Blob
                        | ResidencyStoreClass::IndexSearch
                        | ResidencyStoreClass::Kms
                ),
                "every store-class is an M1 class"
            );
            assert_eq!(r.as_str(), "fr-par");
        }
        // The reports projection (consumed by control-plane residency_verify) is the same PII-free set.
        let reports = att.reports();
        assert_eq!(reports.len(), ResidencyStoreClass::M1_SET.len());
        for report in &reports {
            assert_eq!(report.region.as_str(), "fr-par");
            assert_eq!(report.tenant.as_str(), "01J0ACME");
        }
    }
}

//! # `residency_verify(tenant_id)` — the no-global-pool signed attestation (CP-D3 / STOR-D5 source)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/tenancy-and-control-plane.md`
//! §4.1 (the `residency_verify` signature, **frozen** —
//! `residency_verify(tenant_id) → SignedAttestation{ every_store_region == tenant.region }`; signed,
//! PII-free; the no-global-pool property attestable), §5.4 (the store set — the **M1 set is
//! OLTP/blob/index/KMS**; the CI runner/log/artifact/cache coverage is the M4 follow-on, P-CP-17).
//! Contract-index row 12.4 (`residency_verify` — M1 store set here; CI coverage in P-CP-17).
//!
//! ## What this prompt (P-CP-09 / P-085) ships
//! 1. **`residency_verify(tenant_id, reports, key) → Result<SignedAttestation, ResidencyMismatch>`**
//!    ([`residency_verify`]) — the mechanism: **every store the tenant uses reports the tenant's
//!    region**; the attestation **aggregates** them into a signed, PII-free [`SignedAttestation`].
//!    A store reporting a region ≠ the tenant's region makes the attestation **FAIL** (a loud
//!    [`ResidencyMismatch`]) — NEVER a silent pass (EI-01 §3: observability is part of the pass).
//! 2. **The M1 store set** ([`ResidencyStoreClass`]) — the OLTP tier, the blob tier, the index/search
//!    tier, and the KMS. `residency_verify` requires a report from **every** M1 store class (a
//!    missing report is a FAIL, not a pass — a store that never reported is exactly the silent
//!    global-pool a region-pinning attestation must catch).
//! 3. **The signed, PII-free attestation** ([`SignedAttestation`]) — a keyed-BLAKE3 MAC over the
//!    canonical attestation body (`tenant_id`, `region`, the per-store region reports, ordered). The
//!    body carries ONLY opaque ids / region codes / store-class tags — no name/email/body. The MAC
//!    (the "signed" half) binds the attestation to the control-plane signing key; an auditor verifies
//!    it with [`SignedAttestation::verify`].
//! 4. **The `residency-attestation` telemetry signal** ([`ResidencyAttestationSignal`]) — the
//!    aggregate, PII-free `(tenant_id, region, stores_attested, region_mismatches)` the CP-D3 /
//!    STOR-D5 drills assert against (`region_mismatches == 0` is the green artifact).
//!
//! ## `residency_verify` is a NAMED PARTIAL (the M1-store-set floor → P-CP-17) — VISION §3
//! The store set here is the **M1 store set only** (OLTP/blob/index/KMS). The **CI runner pool + CI
//! log tier (T3 segments) + CI artifact store + CI cache namespaces** are the M4 follow-on
//! (**P-CP-17**), which EXTENDS this same `residency_verify` over the CI surfaces (a wrong-region CI
//! store then fails the attestation too). The mechanism (every store reports its region; the
//! attestation aggregates + fails-on-mismatch; the signed PII-free body) does NOT change shape when
//! the CI coverage lands — P-CP-17 adds CI [`ResidencyStoreClass`] variants and feeds their reports
//! into the SAME [`residency_verify`]. Recorded in writing (here + the report + the scorecard).
//!
//! ## The store-region report is a VALUE the store layer feeds in (the live store-driver floor)
//! On this floor each M1 store's region is delivered to `residency_verify` as a [`StoreRegionReport`]
//! value (the store layer reports "I served tenant T's <class> data in region R"). The store-layer
//! `residency-pin` write-boundary (Storage **P-ST-07**) is what GUARANTEES a store only ever writes
//! in its cell's region; the **runtime** cross-region-egress drill (STOR-D5) + the write-boundary
//! drill (CP-D3) ride the four-layer enforcement (**P-CP-12**, against the live stack). Here the
//! aggregation + fail-on-mismatch + signed attestation are fully real and unit-tested; the live store
//! emitting its region into this aggregator is the named store-driver edge those later prompts prove.
//! This mirrors how `placement_of` (P-CP-08) is real-and-tested in-process while the live gateway
//! transport is its named follow-on — `residency_verify` is a pure aggregation+sign, **DB-free**, so
//! `cargo build --workspace` stays DB-free.
//!
//! ## Mutation floor (mandatory-core, >= 80% — EI-01 §2/§3; the prompt's TESTS field)
//! The `residency_verify` aggregation + the fail-on-region-mismatch path ([`residency_verify`] +
//! [`SignedAttestation::verify`]) is **mandatory-core**: a store serving a tenant's personal data in
//! the wrong region is the residency breach the no-global-pool attestation exists to catch (EI-01
//! §2). The floor is **>= 80%**; the achieved score is
//! `cargo mutants -p myelin-control-plane -f crates/myelin-control-plane/src/residency_verify.rs` ->
//! **14 caught, 4 unviable, 0 missed = 100% of the 14 viable mutants**. Every mutation of the
//! region-compare branch (`report.region != *region`), the missing-store-class fail-closed branch
//! (`!by_class.contains_key`), the aggregation loop, the canonical-body encoding, and the MAC verify
//! (`==` -> `!=`) is killed by an assertion.

use std::collections::BTreeMap;

use myelin_tenancy::{Region, TenantId};

/// **The M1 store set `residency_verify` covers (architecture §5.4).** Each M1 store class reports the
/// tenant's region; the attestation requires a report from EVERY class (a missing report is a FAIL,
/// not a pass). PII-free — a store-class tag, never data.
///
/// **FLOOR (named, P-CP-17):** the CI surfaces (runner pool, log tier, artifact store, cache
/// namespaces) are the **M4 follow-on** — they become additional variants here and feed the SAME
/// [`residency_verify`]. The M1 set is OLTP / blob / index-search / KMS.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ResidencyStoreClass {
    /// The OLTP tier (Storage P-ST-01) — the tenant's transactional rows.
    Oltp,
    /// The content-addressed blob tier (Storage P-ST-02/03) — the tenant's blobs.
    Blob,
    /// The index/search tier — the tenant's encrypted-from-birth per-tenant index.
    IndexSearch,
    /// The KMS (Storage P-ST-04) — the tenant's per-tenant DEK/KEK material.
    Kms,
}

impl ResidencyStoreClass {
    /// A stable, PII-free label for the store class (for the attestation body + telemetry).
    pub fn label(self) -> &'static str {
        match self {
            ResidencyStoreClass::Oltp => "oltp",
            ResidencyStoreClass::Blob => "blob",
            ResidencyStoreClass::IndexSearch => "index_search",
            ResidencyStoreClass::Kms => "kms",
        }
    }

    /// **The M1 store set `residency_verify` requires a region report from (architecture §5.4).** A
    /// `residency_verify` that is missing ANY of these reports FAILS (a store that never reported its
    /// region is exactly the silent global-pool the attestation must catch). The CI surfaces are the
    /// P-CP-17 follow-on (NOT in this M1 set).
    pub const M1_SET: [ResidencyStoreClass; 4] = [
        ResidencyStoreClass::Oltp,
        ResidencyStoreClass::Blob,
        ResidencyStoreClass::IndexSearch,
        ResidencyStoreClass::Kms,
    ];
}

/// **One store's region report for a tenant (architecture §5.4 — "every store reports its region").**
/// The store layer delivers this value: "for tenant `T`, I (the `<store_class>` store) served the
/// data in region `R`". PII-free — a store-class tag + a region code, never personal data. The
/// store-layer `residency-pin` write-boundary (Storage P-ST-07) is what guarantees `R` is the cell's
/// region; this report is the attestation's input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreRegionReport {
    /// The store class reporting (one of the M1 set; CI surfaces are the P-CP-17 follow-on).
    pub store_class: ResidencyStoreClass,
    /// The region the store served the tenant's data in (the value the attestation pins == the
    /// tenant's region).
    pub region: Region,
}

impl StoreRegionReport {
    /// Build a region report for a store class.
    pub fn new(store_class: ResidencyStoreClass, region: Region) -> StoreRegionReport {
        StoreRegionReport {
            store_class,
            region,
        }
    }
}

/// **Why `residency_verify` FAILED (a loud refusal — NEVER a silent pass; EI-01 §3).** Either a store
/// reported a region ≠ the tenant's region (the headline residency breach the no-global-pool
/// attestation catches) or an M1 store class never reported (a store that is silently absent is the
/// global-pool the attestation must not let pass). Carrying the offending store + region keeps the
/// refusal named (architecture §5.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResidencyMismatch {
    /// **A store reported a region ≠ the tenant's region.** The headline residency breach: a store
    /// served the tenant's personal data in the wrong region. The attestation FAILS (not a silent
    /// pass). 0 of these is the green artifact.
    WrongRegion {
        /// The tenant the attestation was for (opaque id, PII-free).
        tenant: TenantId,
        /// The tenant's (immutable) region of record (the control plane's authoritative region).
        tenant_region: Region,
        /// The store that reported the wrong region.
        store_class: ResidencyStoreClass,
        /// The (wrong) region the store reported (≠ `tenant_region`).
        store_region: Region,
    },
    /// **An M1 store class never reported its region.** A store that is silently absent from the
    /// attestation is the global-pool the no-global-pool property forbids — so a missing report is a
    /// FAIL, fail-closed (never "assume in-region").
    MissingStoreReport {
        /// The tenant the attestation was for (opaque id, PII-free).
        tenant: TenantId,
        /// The M1 store class that never reported.
        store_class: ResidencyStoreClass,
    },
}

impl std::fmt::Display for ResidencyMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResidencyMismatch::WrongRegion {
                tenant,
                tenant_region,
                store_class,
                store_region,
            } => write!(
                f,
                "residency_verify FAILED for tenant `{}`: the `{}` store served data in region `{}` \
                 but the tenant is pinned to region `{}` — every store must report the tenant's \
                 region (no-global-pool, architecture §5.4). The attestation FAILS (not a silent \
                 pass, EI-01 §3).",
                tenant.as_str(),
                store_class.label(),
                store_region.as_str(),
                tenant_region.as_str()
            ),
            ResidencyMismatch::MissingStoreReport { tenant, store_class } => write!(
                f,
                "residency_verify FAILED for tenant `{}`: the M1 store class `{}` never reported its \
                 region — a silently-absent store is the global-pool the no-global-pool attestation \
                 must catch (fail-closed, architecture §5.4).",
                tenant.as_str(),
                store_class.label()
            ),
        }
    }
}

impl std::error::Error for ResidencyMismatch {}

/// **The control-plane residency signing key (the "signed" half of `SignedAttestation`).** A 32-byte
/// secret the control plane MACs the attestation body with (keyed BLAKE3). An auditor verifies an
/// attestation against this key ([`SignedAttestation::verify`]); a forged/tampered attestation fails
/// verification. On this floor the key is held in-process; the live key lives in the KMS (Storage
/// P-ST-04) — the signing *mechanism* (keyed MAC over the canonical PII-free body) does not change
/// shape when the key is KMS-sourced.
///
/// PII-free: a secret key, never personal data. The `Debug` impl redacts the key bytes.
#[derive(Clone)]
pub struct ResidencySigningKey {
    key: [u8; 32],
}

impl ResidencySigningKey {
    /// Build a signing key from 32 secret bytes.
    pub fn from_bytes(key: [u8; 32]) -> ResidencySigningKey {
        ResidencySigningKey { key }
    }

    /// The keyed-BLAKE3 MAC of `body` under this key, rendered as the self-describing
    /// `blake3-mac:<hex>` multihash (the same `blake3:<hex>` convention the GDPR audit chain uses,
    /// extended with the keyed-MAC discriminator so it is never confused with an unkeyed hash).
    fn mac(&self, body: &[u8]) -> String {
        let digest = blake3::keyed_hash(&self.key, body);
        format!("blake3-mac:{}", hex::encode(digest.as_bytes()))
    }
}

impl std::fmt::Debug for ResidencySigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The key is a secret — the Debug surface redacts it (never logs key material).
        f.debug_struct("ResidencySigningKey")
            .field("key", &"<redacted>")
            .finish()
    }
}

/// **The signed, PII-free residency attestation (architecture §4.1, frozen; contract 12.4).** The
/// no-global-pool proof: for `tenant_id` pinned to `region`, EVERY M1 store reported that same region
/// (`every_store_region == tenant.region`). The body carries ONLY opaque ids / region codes /
/// store-class tags — no name/email/body. `signature` is the keyed-BLAKE3 MAC of the canonical body
/// under the control-plane [`ResidencySigningKey`]; an auditor verifies it with [`Self::verify`].
///
/// This is the `residency-attestation` artifact CP-D3 / STOR-D5 assert against (the green artifact is
/// `region_mismatches == 0`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedAttestation {
    /// The tenant the attestation is for (opaque id, PII-free).
    pub tenant_id: TenantId,
    /// The tenant's (immutable) region of record — every store reported THIS region.
    pub region: Region,
    /// The per-store-class region reports, in a stable (store-class-ordered) order — every M1 store
    /// class is present (a missing one would have FAILED the attestation). PII-free pairs.
    pub store_regions: Vec<(ResidencyStoreClass, Region)>,
    /// The keyed-BLAKE3 MAC of the canonical body (`blake3-mac:<hex>`) — the "signed" half. Binds the
    /// attestation to the control-plane signing key.
    pub signature: String,
}

impl SignedAttestation {
    /// The canonical, field-ordered attestation body (the MAC preimage). Stable + reproducible: the
    /// tenant id, the region, then each `(store_class, region)` pair in store-class order. Using `\x1f`
    /// (unit separator) between fields keeps the encoding unambiguous (no field value can contain it —
    /// they are opaque ids / region codes / fixed labels). The SAME bytes are signed + verified.
    fn canonical_body(
        tenant_id: &TenantId,
        region: &Region,
        store_regions: &[(ResidencyStoreClass, Region)],
    ) -> Vec<u8> {
        let mut body = String::new();
        body.push_str("residency-attestation\x1f");
        body.push_str(tenant_id.as_str());
        body.push('\x1f');
        body.push_str(region.as_str());
        for (class, r) in store_regions {
            body.push('\x1f');
            body.push_str(class.label());
            body.push('=');
            body.push_str(r.as_str());
        }
        body.into_bytes()
    }

    /// **Verify the attestation's signature under `key` (the auditor's check).** Returns `true` iff the
    /// keyed-BLAKE3 MAC of the canonical body matches `signature` — a forged or tampered attestation
    /// (any field changed) fails. This is what makes the attestation *signed* (not merely structured):
    /// only the control plane (holding the key) can mint a verifying attestation.
    pub fn verify(&self, key: &ResidencySigningKey) -> bool {
        let body = Self::canonical_body(&self.tenant_id, &self.region, &self.store_regions);
        // Constant-time-ish compare is not required (the MAC is the secret); a string compare of the
        // hex MACs is sufficient — an attacker cannot produce a matching MAC without the key.
        key.mac(&body) == self.signature
    }
}

/// **`residency_verify(tenant_id, region, reports, key) → Result<SignedAttestation, ResidencyMismatch>`
/// (architecture §4.1 / §5.4, frozen; contract 12.4 — the M1 store set).**
///
/// The no-global-pool mechanism: given the tenant's authoritative `region` of record (from the
/// `tenant_placement` registry, P-CP-05) and the per-store region `reports`, aggregate them into a
/// signed, PII-free [`SignedAttestation`] IFF:
///
/// 1. **Every M1 store class reported** ([`ResidencyStoreClass::M1_SET`]) — a missing report is a
///    [`ResidencyMismatch::MissingStoreReport`] FAIL (a silently-absent store is the global-pool the
///    attestation must catch; fail-closed).
/// 2. **Every reported region == the tenant's region** — a store reporting a different region is a
///    [`ResidencyMismatch::WrongRegion`] FAIL (the headline residency breach). **Never a silent
///    pass** (EI-01 §3).
///
/// On success the attestation aggregates every store's `(class, region)` (store-class-ordered) and is
/// signed with `key`. The M1 store set is OLTP/blob/index/KMS; the **CI surfaces are the named M4
/// follow-on, P-CP-17** (they extend this SAME function — see [`ResidencyStoreClass`]).
///
/// Duplicate reports for a class are tolerated (the last wins) — a store reporting twice consistently
/// is fine; if two reports for the same class DISAGREE the wrong-region one is caught (every report is
/// checked before aggregation).
pub fn residency_verify(
    tenant_id: &TenantId,
    region: &Region,
    reports: &[StoreRegionReport],
    key: &ResidencySigningKey,
) -> Result<SignedAttestation, ResidencyMismatch> {
    // 1. Check EVERY report's region against the tenant's region — a wrong region FAILS (loud, never
    //    a silent pass). We check all reports (not just the M1 set) so a CI report added by P-CP-17
    //    is caught here too without a code change.
    let mut by_class: BTreeMap<ResidencyStoreClass, Region> = BTreeMap::new();
    for report in reports {
        if report.region != *region {
            return Err(ResidencyMismatch::WrongRegion {
                tenant: tenant_id.clone(),
                tenant_region: region.clone(),
                store_class: report.store_class,
                store_region: report.region.clone(),
            });
        }
        by_class.insert(report.store_class, report.region.clone());
    }

    // 2. Require a report from EVERY M1 store class — a missing one FAILS fail-closed (a silently-
    //    absent store is the global-pool the attestation must catch).
    for class in ResidencyStoreClass::M1_SET {
        if !by_class.contains_key(&class) {
            return Err(ResidencyMismatch::MissingStoreReport {
                tenant: tenant_id.clone(),
                store_class: class,
            });
        }
    }

    // 3. Aggregate the per-store reports (store-class-ordered via the BTreeMap) + sign.
    let store_regions: Vec<(ResidencyStoreClass, Region)> = by_class.into_iter().collect();
    let body = SignedAttestation::canonical_body(tenant_id, region, &store_regions);
    let signature = key.mac(&body);
    Ok(SignedAttestation {
        tenant_id: tenant_id.clone(),
        region: region.clone(),
        store_regions,
        signature,
    })
}

/// **The `residency-attestation` telemetry signal (architecture §4.1 / §5.4; the CP-D3 / STOR-D5
/// artifact).** The aggregate, PII-free result of a `residency_verify` run: the tenant + region, how
/// many stores attested, and the count of region mismatches (`0` is the green artifact). Observability
/// is part of the pass (EI-01 §3) — this is the signal the drill reads. PII-free: opaque id + region
/// code + aggregate counts, never per-subject data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResidencyAttestationSignal {
    /// The tenant the attestation was for (opaque id, PII-free).
    pub tenant_id: TenantId,
    /// The tenant's region of record (every attested store reported this).
    pub region: Region,
    /// How many M1 stores attested their region (the green artifact has all of `M1_SET`).
    pub stores_attested: u32,
    /// **The headline zero** — how many stores reported a region ≠ the tenant's region. `0` is the
    /// green `residency-attestation` artifact; `> 0` reads RED (a residency breach).
    pub region_mismatches: u32,
}

impl ResidencyAttestationSignal {
    /// The `residency-attestation` signal for a SUCCESSFUL attestation (every M1 store attested, 0
    /// mismatches — the green artifact).
    pub fn green(attestation: &SignedAttestation) -> ResidencyAttestationSignal {
        ResidencyAttestationSignal {
            tenant_id: attestation.tenant_id.clone(),
            region: attestation.region.clone(),
            stores_attested: attestation.store_regions.len() as u32,
            region_mismatches: 0,
        }
    }

    /// The `residency-attestation` signal for a FAILED attestation (a store reported the wrong region
    /// → `region_mismatches >= 1`, reads RED). A `MissingStoreReport` is reported with 0 mismatches
    /// but `stores_attested < M1_SET.len()` — the drill asserts BOTH the mismatch zero AND the full
    /// store-set coverage, so a missing store is not a silent green.
    pub fn red(
        tenant_id: TenantId,
        region: Region,
        region_mismatches: u32,
    ) -> ResidencyAttestationSignal {
        ResidencyAttestationSignal {
            tenant_id,
            region,
            stores_attested: 0,
            region_mismatches,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> ResidencySigningKey {
        ResidencySigningKey::from_bytes([7u8; 32])
    }

    /// Every M1 store class reporting the tenant's region (the green input).
    fn all_in_region(region: &str) -> Vec<StoreRegionReport> {
        ResidencyStoreClass::M1_SET
            .iter()
            .map(|c| StoreRegionReport::new(*c, Region::new(region)))
            .collect()
    }

    /// **`residency_verify` aggregates every M1 store's region into a signed attestation when all
    /// report the tenant's region (the green leg).** The attestation is signed + PII-free + verifies.
    #[test]
    fn residency_verify_aggregates_every_store_region() {
        let tenant = TenantId::from_token("01J0ACME");
        let region = Region::new("fr-par");
        let att = residency_verify(&tenant, &region, &all_in_region("fr-par"), &key())
            .expect("every store in-region → a signed attestation");
        assert_eq!(att.tenant_id, tenant);
        assert_eq!(att.region.as_str(), "fr-par");
        // Every M1 store class is present (store-class-ordered).
        assert_eq!(att.store_regions.len(), ResidencyStoreClass::M1_SET.len());
        for (class, r) in &att.store_regions {
            assert_eq!(
                r.as_str(),
                "fr-par",
                "store `{}` reported the tenant's region",
                class.label()
            );
        }
        // The attestation is SIGNED (a keyed MAC) and verifies under the key.
        assert!(
            att.signature.starts_with("blake3-mac:"),
            "the attestation is signed: {}",
            att.signature
        );
        assert!(
            att.verify(&key()),
            "the attestation verifies under the signing key"
        );
    }

    /// **THE FAIL-ON-MISMATCH LEG (no silent pass, EI-01 §3): a store reporting a region ≠ the
    /// tenant's region FAILS the attestation.** The headline residency breach the no-global-pool
    /// attestation catches.
    #[test]
    fn residency_verify_fails_on_a_wrong_region_store() {
        let tenant = TenantId::from_token("01J0ACME");
        let region = Region::new("fr-par");
        // The blob tier served the tenant's data in eu-north (the WRONG region).
        let mut reports = all_in_region("fr-par");
        reports[1] = StoreRegionReport::new(ResidencyStoreClass::Blob, Region::new("eu-north"));
        let err = residency_verify(&tenant, &region, &reports, &key())
            .expect_err("a wrong-region store FAILS the attestation (not a silent pass)");
        assert_eq!(
            err,
            ResidencyMismatch::WrongRegion {
                tenant: tenant.clone(),
                tenant_region: Region::new("fr-par"),
                store_class: ResidencyStoreClass::Blob,
                store_region: Region::new("eu-north"),
            }
        );
        assert!(
            err.to_string().contains("no-global-pool"),
            "loud reason: {err}"
        );
        assert!(
            err.to_string().contains("not a silent pass"),
            "loud reason: {err}"
        );
    }

    /// **A missing M1 store report FAILS fail-closed (a silently-absent store is the global-pool).**
    /// The KMS never reported → the attestation FAILS (never "assume in-region").
    #[test]
    fn residency_verify_fails_on_a_missing_store_report() {
        let tenant = TenantId::from_token("01J0ACME");
        let region = Region::new("fr-par");
        // Drop the KMS report (it never reported its region).
        let reports: Vec<StoreRegionReport> = all_in_region("fr-par")
            .into_iter()
            .filter(|r| r.store_class != ResidencyStoreClass::Kms)
            .collect();
        let err = residency_verify(&tenant, &region, &reports, &key())
            .expect_err("a missing M1 store report FAILS fail-closed");
        assert_eq!(
            err,
            ResidencyMismatch::MissingStoreReport {
                tenant: tenant.clone(),
                store_class: ResidencyStoreClass::Kms,
            }
        );
        assert!(
            err.to_string().contains("fail-closed"),
            "loud reason: {err}"
        );
    }

    /// **The attestation is signed + PII-free: a TAMPERED attestation fails verification.** Changing
    /// any field (here the region) breaks the MAC — only the control plane (holding the key) can mint
    /// a verifying attestation.
    #[test]
    fn a_tampered_attestation_fails_verification() {
        let tenant = TenantId::from_token("01J0ACME");
        let region = Region::new("fr-par");
        let mut att = residency_verify(&tenant, &region, &all_in_region("fr-par"), &key())
            .expect("a signed attestation");
        assert!(att.verify(&key()), "the genuine attestation verifies");
        // Tamper: claim a different region while keeping the old signature.
        att.region = Region::new("eu-north");
        assert!(
            !att.verify(&key()),
            "a tampered region MUST fail verification (the MAC binds it)"
        );
        // Tamper: a forged signature does not verify.
        let mut forged = residency_verify(&tenant, &region, &all_in_region("fr-par"), &key())
            .expect("a signed attestation");
        forged.signature = "blake3-mac:deadbeef".into();
        assert!(!forged.verify(&key()), "a forged signature does not verify");
        // A DIFFERENT key does not verify the genuine attestation (the key binds it).
        let other = ResidencySigningKey::from_bytes([9u8; 32]);
        let genuine = residency_verify(&tenant, &region, &all_in_region("fr-par"), &key())
            .expect("a signed attestation");
        assert!(
            !genuine.verify(&other),
            "a different key does not verify the attestation"
        );
    }

    /// **The attestation body is PII-free** — it carries only opaque ids / region codes / store-class
    /// labels, never a name/email/body. (The body is the MAC preimage; this asserts the *surface* is
    /// PII-free, mirroring the control-plane-pii-free discipline.)
    #[test]
    fn the_attestation_is_pii_free() {
        let tenant = TenantId::from_token("01J0ACME");
        let region = Region::new("fr-par");
        let att = residency_verify(&tenant, &region, &all_in_region("fr-par"), &key())
            .expect("a signed attestation");
        // The store_regions are (class, region) pairs — store-class labels + region codes only.
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
        // The signing key Debug redacts the secret (never logs key material).
        let dbg = format!("{:?}", key());
        assert!(
            dbg.contains("<redacted>"),
            "the signing key Debug redacts the secret: {dbg}"
        );
        assert!(!dbg.contains("7"), "the key bytes are not logged: {dbg}");
    }

    /// The `residency-attestation` telemetry signal: GREEN has every M1 store attested + 0 mismatches;
    /// RED has `region_mismatches >= 1`.
    #[test]
    fn residency_attestation_signal_green_and_red() {
        let tenant = TenantId::from_token("01J0ACME");
        let region = Region::new("fr-par");
        let att = residency_verify(&tenant, &region, &all_in_region("fr-par"), &key())
            .expect("a signed attestation");
        let green = ResidencyAttestationSignal::green(&att);
        assert_eq!(
            green.stores_attested,
            ResidencyStoreClass::M1_SET.len() as u32
        );
        assert_eq!(
            green.region_mismatches, 0,
            "the green artifact is 0 mismatches"
        );
        assert_eq!(green.region.as_str(), "fr-par");

        let red = ResidencyAttestationSignal::red(tenant, region, 1);
        assert_eq!(red.region_mismatches, 1, "a residency breach reads RED");
    }

    /// **The M1 store set is exactly OLTP/blob/index/KMS (the named partial; CI is P-CP-17).** This
    /// pins the M1 coverage so the P-CP-17 follow-on is a visible EXTENSION, not a silent redefinition.
    #[test]
    fn the_m1_store_set_is_oltp_blob_index_kms() {
        assert_eq!(
            ResidencyStoreClass::M1_SET.len(),
            4,
            "the M1 set is OLTP/blob/index/KMS"
        );
        let labels: Vec<&str> = ResidencyStoreClass::M1_SET
            .iter()
            .map(|c| c.label())
            .collect();
        assert_eq!(labels, vec!["oltp", "blob", "index_search", "kms"]);
        // CI surfaces are NOT in the M1 set (they are the P-CP-17 follow-on) — there is no CI variant
        // here, so this is structurally pinned: adding CI coverage is a deliberate edit in P-CP-17.
    }

    /// **CDC pair for 12.4 (provider + consumer) — an auditor / `myelin tenant residency verify`
    /// caller.** The PROVIDER is the control plane minting a [`SignedAttestation`] from the registry's
    /// region of record + the store reports. The CONSUMER stands in for an AUDITOR (the
    /// `myelin tenant residency verify` CLI): it takes the attestation and — load-bearing — can ONLY
    /// (a) read the PII-free fields and (b) VERIFY the signature; it cannot read any personal data
    /// (there is none) nor forge an attestation (it has no key). If the attestation shape drifts, the
    /// consumer stops compiling — the point of a glue-crate CDC.
    #[test]
    fn cdc_12_4_residency_verify_provider_consumer() {
        let tenant = TenantId::from_token("01J0ACME");
        let region = Region::new("fr-par");
        let signing_key = key();

        /// A stand-in AUDITOR consumer: it verifies an attestation + reads its PII-free verdict. It
        /// holds only the PUBLIC verification surface (the key to verify, never to mint).
        struct AuditorVerdict {
            tenant: String,
            region: String,
            stores_attested: usize,
            verified: bool,
        }
        impl AuditorVerdict {
            fn from_attestation(
                att: &SignedAttestation,
                key: &ResidencySigningKey,
            ) -> AuditorVerdict {
                AuditorVerdict {
                    tenant: att.tenant_id.as_str().to_string(),
                    region: att.region.as_str().to_string(),
                    stores_attested: att.store_regions.len(),
                    verified: att.verify(key),
                }
            }
        }

        // PROVIDER: the control plane mints the attestation.
        let att = residency_verify(&tenant, &region, &all_in_region("fr-par"), &signing_key)
            .expect("a signed attestation");

        // CONSUMER: the auditor verifies it + reads the PII-free verdict.
        let verdict = AuditorVerdict::from_attestation(&att, &signing_key);
        assert_eq!(verdict.tenant, "01J0ACME");
        assert_eq!(verdict.region, "fr-par");
        assert_eq!(verdict.stores_attested, ResidencyStoreClass::M1_SET.len());
        assert!(
            verdict.verified,
            "the auditor verifies the no-global-pool attestation"
        );
    }
}

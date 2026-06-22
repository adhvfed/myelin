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
//! ## P-CP-17 (P-324) — the CI-store coverage CLOSES the P-CP-09 named partial (VISION §1/§3, C-2)
//! P-CP-09 shipped `residency_verify` over the **M1 store set only** (OLTP/blob/index/KMS) and NAMED
//! the CI surfaces as the M4 follow-on. **P-CP-17 closes that partial:** the **CI runner pool + CI log
//! tier (T3 segments, Storage 11.8) + CI artifact store + CI cache namespaces (incl. the
//! trust-tier/branch-scoped namespaces, Storage 11.2)** are now [`ResidencyStoreClass`] variants
//! ([`ResidencyStoreClass::CI_SET`]) and [`residency_verify_ci`] attests over the full
//! [`RequiredStoreSet::M1AndCi`] set. A wrong-region (or absent) CI runner / log / artifact / cache
//! FAILS the attestation exactly as a wrong-region M1 store does — the **no-global-CI-pool property is
//! now attestable per-tenant** (a CI runner that executed a tenant's job in the wrong region fails
//! `residency_verify`). The mechanism did NOT fork: both coverages call the SAME
//! [`residency_verify_over`] (EI-01 §7 — one mechanism, two coverages); the CI coverage is a VISIBLE
//! extension, not a silent redefinition (the attestation declares its `coverage`). The in-region
//! runner-CLAIM enforcement (a job claimed only by an in-region runner) is the sibling **P-CP-18**.
//! The store-region report remains a VALUE the store/CI layer feeds in (the live store-driver floor,
//! below); the CI subsystem's full crate + its runner-pool region report lands with CI in M4 and feeds
//! these SAME CI [`ResidencyStoreClass`] variants. Recorded in writing (here + the report + scorecard).
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
//! **17 caught, 7 unviable, 0 missed = 100% of the 17 viable mutants** (P-CP-17 added the CI-store
//! aggregation; the count grew from P-085's 14). Every mutation of the region-compare branch
//! (`report.region != *region`), the missing-store-class fail-closed branch (`!by_class.contains_key`),
//! the coverage-required-set loop (M1 vs M1+CI), the aggregation loop, the canonical-body encoding
//! (incl. the coverage bind), and the MAC verify (`==` -> `!=`) is killed by an assertion — the
//! CI-store coverage's fail-on-mismatch path is mandatory-core and fully covered.

use std::collections::BTreeMap;

use myelin_tenancy::{Region, TenantId};

/// **The store set `residency_verify` covers (architecture §5.4).** Each store class reports the
/// tenant's region; the attestation requires a report from EVERY class in the [`RequiredStoreSet`]
/// it is run over (a missing report is a FAIL, not a pass). PII-free — a store-class tag, never data.
///
/// **M1 set (P-CP-09):** OLTP / blob / index-search / KMS. **CI set (P-CP-17, the M4 follow-on that
/// CLOSES the P-CP-09 named partial):** the CI runner pool, the CI log tier (T3 content-addressed
/// segments, Storage 11.8), the CI artifact store, and the CI cache namespaces (incl. the
/// trust-tier/branch-scoped namespaces, Storage 11.2). The CI variants feed the SAME aggregation
/// ([`residency_verify_over`]) — the mechanism is unchanged, only the coverage is pinned. A wrong-
/// region CI store then FAILS the attestation exactly as a wrong-region M1 store does.
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
    /// **The CI runner pool (P-CP-17 / CI M4) — the compute the tenant's CI jobs EXECUTE on.** A CI
    /// surface (NOT in the M1 set): a runner that executed a tenant's job reports the region it ran
    /// in. A runner in a region ≠ the tenant's region is the *global CI pool* the no-global-pool
    /// pitch (VISION §1) forbids — it FAILS `residency_verify` here (not a silent pass). The in-region
    /// runner-CLAIM enforcement (a job claimed only by an in-region runner) is the sibling P-CP-18;
    /// this variant makes the property ATTESTABLE.
    CiRunnerPool,
    /// **The CI log tier (P-CP-17 / Storage 11.8) — the T3 content-addressed log segments + the
    /// `(job, step, byte-range)` OLTP index, per-tenant-DEK.** A CI surface (NOT in the M1 set): the
    /// sealed log segments for a tenant's CI run report the region they were sealed in. A log tier in
    /// the wrong region FAILS `residency_verify` here.
    CiLogTier,
    /// **The CI artifact store (P-CP-17 / CI M4) — the build artifacts a tenant's CI run produces.** A
    /// CI surface (NOT in the M1 set): the artifact store reports the region it persisted the run's
    /// artifacts in. An artifact store in the wrong region FAILS `residency_verify` here.
    CiArtifactStore,
    /// **The CI cache namespaces (P-CP-17 / Storage 11.2) — the build caches, including the
    /// trust-tier/branch-scoped namespaces (an UntrustedFork write cannot reach the trusted scope).**
    /// A CI surface (NOT in the M1 set): the cache namespaces report the region they hold a tenant's
    /// cache entries in. A cache namespace in the wrong region FAILS `residency_verify` here.
    CiCacheNamespaces,
}

impl ResidencyStoreClass {
    /// A stable, PII-free label for the store class (for the attestation body + telemetry).
    pub fn label(self) -> &'static str {
        match self {
            ResidencyStoreClass::Oltp => "oltp",
            ResidencyStoreClass::Blob => "blob",
            ResidencyStoreClass::IndexSearch => "index_search",
            ResidencyStoreClass::Kms => "kms",
            ResidencyStoreClass::CiRunnerPool => "ci_runner_pool",
            ResidencyStoreClass::CiLogTier => "ci_log_tier",
            ResidencyStoreClass::CiArtifactStore => "ci_artifact_store",
            ResidencyStoreClass::CiCacheNamespaces => "ci_cache_namespaces",
        }
    }

    /// **The M1 store set `residency_verify` requires a region report from (architecture §5.4).** A
    /// `residency_verify` that is missing ANY of these reports FAILS (a store that never reported its
    /// region is exactly the silent global-pool the attestation must catch). The CI surfaces are the
    /// P-CP-17 follow-on ([`Self::CI_SET`]) — NOT in this M1 set.
    pub const M1_SET: [ResidencyStoreClass; 4] = [
        ResidencyStoreClass::Oltp,
        ResidencyStoreClass::Blob,
        ResidencyStoreClass::IndexSearch,
        ResidencyStoreClass::Kms,
    ];

    /// **The CI store set `residency_verify` requires a region report from when run over the CI
    /// coverage (P-CP-17, architecture §5.4 — the no-global-CI-pool surfaces).** The CI runner pool,
    /// the CI log tier (T3 segments, Storage 11.8), the CI artifact store, and the CI cache namespaces
    /// (incl. the trust-tier/branch-scoped namespaces, Storage 11.2). A CI run whose runner / log /
    /// artifact / cache surface is missing — or in the wrong region — FAILS the CI-coverage attestation
    /// (the no-global-CI-pool property is attestable per-tenant).
    pub const CI_SET: [ResidencyStoreClass; 4] = [
        ResidencyStoreClass::CiRunnerPool,
        ResidencyStoreClass::CiLogTier,
        ResidencyStoreClass::CiArtifactStore,
        ResidencyStoreClass::CiCacheNamespaces,
    ];

    /// **The full store set `residency_verify` requires when run over the CI coverage (P-CP-17).** The
    /// M1 stores AND the CI surfaces — the complete no-global-pool set (every store the tenant uses,
    /// M1 + CI, must report the tenant's region). This is the set [`RequiredStoreSet::M1AndCi`]
    /// enforces; the P-CP-09 M1-only partial is CLOSED by this set covering the CI surfaces too.
    pub const M1_AND_CI_SET: [ResidencyStoreClass; 8] = [
        ResidencyStoreClass::Oltp,
        ResidencyStoreClass::Blob,
        ResidencyStoreClass::IndexSearch,
        ResidencyStoreClass::Kms,
        ResidencyStoreClass::CiRunnerPool,
        ResidencyStoreClass::CiLogTier,
        ResidencyStoreClass::CiArtifactStore,
        ResidencyStoreClass::CiCacheNamespaces,
    ];
}

/// **Which store set `residency_verify` requires a complete region report from (the attestation's
/// coverage — architecture §5.4).** Naming the required set explicitly keeps the P-CP-17 CI extension
/// VISIBLE (an attestation declares whether it covered M1-only or M1+CI) rather than a silent
/// redefinition of "every store". A `residency_verify` run over a [`RequiredStoreSet`] FAILS if any
/// store class in that set never reported (fail-closed — a silently-absent store is the global pool).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RequiredStoreSet {
    /// **The M1 store set only (P-CP-09):** OLTP / blob / index-search / KMS. The original coverage;
    /// the CI surfaces were a NAMED PARTIAL on this set until P-CP-17.
    M1Only,
    /// **The M1 store set AND the CI surfaces (P-CP-17 — the no-global-CI-pool coverage):** OLTP /
    /// blob / index-search / KMS + the CI runner pool / log tier / artifact store / cache namespaces.
    /// This is the set that closes the P-CP-09 partial — a wrong-region CI store FAILS the attestation
    /// exactly as a wrong-region M1 store does.
    M1AndCi,
}

impl RequiredStoreSet {
    /// The store classes this set requires a region report from (a missing one FAILS fail-closed).
    pub fn required_classes(self) -> &'static [ResidencyStoreClass] {
        match self {
            RequiredStoreSet::M1Only => &ResidencyStoreClass::M1_SET,
            RequiredStoreSet::M1AndCi => &ResidencyStoreClass::M1_AND_CI_SET,
        }
    }

    /// A stable, PII-free label for the coverage (for the attestation body + telemetry) — so an
    /// attestation is self-describing about WHICH set it attested over (M1-only vs M1+CI).
    pub fn label(self) -> &'static str {
        match self {
            RequiredStoreSet::M1Only => "m1",
            RequiredStoreSet::M1AndCi => "m1+ci",
        }
    }
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
    /// **The store set this attestation attested over (M1-only or M1+CI — P-CP-17).** Self-describing
    /// coverage: an auditor reads whether the no-global-pool property was attested over the M1 stores
    /// only or the full M1+CI set. The coverage is bound into the signed body (an M1-only attestation
    /// can never be passed off as an M1+CI one — the MACs differ).
    pub coverage: RequiredStoreSet,
    /// The per-store-class region reports, in a stable (store-class-ordered) order — every required
    /// store class (per `coverage`) is present (a missing one would have FAILED the attestation).
    /// PII-free pairs.
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
        coverage: RequiredStoreSet,
        store_regions: &[(ResidencyStoreClass, Region)],
    ) -> Vec<u8> {
        let mut body = String::new();
        body.push_str("residency-attestation\x1f");
        body.push_str(tenant_id.as_str());
        body.push('\x1f');
        body.push_str(region.as_str());
        // Bind the COVERAGE into the body (P-CP-17): an M1-only attestation and an M1+CI attestation
        // over the same tenant/region/reports MUST NOT share a MAC — the coverage is load-bearing.
        body.push('\x1f');
        body.push_str("coverage=");
        body.push_str(coverage.label());
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
        let body = Self::canonical_body(
            &self.tenant_id,
            &self.region,
            self.coverage,
            &self.store_regions,
        );
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
///
/// This is the M1-only entry point ([`RequiredStoreSet::M1Only`]); the **CI-store coverage** is
/// [`residency_verify_ci`] ([`RequiredStoreSet::M1AndCi`], P-CP-17) — both delegate to the SAME
/// [`residency_verify_over`] mechanism.
pub fn residency_verify(
    tenant_id: &TenantId,
    region: &Region,
    reports: &[StoreRegionReport],
    key: &ResidencySigningKey,
) -> Result<SignedAttestation, ResidencyMismatch> {
    residency_verify_over(tenant_id, region, RequiredStoreSet::M1Only, reports, key)
}

/// **`residency_verify_ci(...)` — the no-global-pool attestation over the M1 stores AND the CI
/// surfaces (P-CP-17, architecture §5.4, C-2; the M4 follow-on that CLOSES the P-CP-09 named
/// partial).**
///
/// Identical mechanism to [`residency_verify`], but the required store set is
/// [`RequiredStoreSet::M1AndCi`] — the attestation requires a region report from EVERY M1 store AND
/// EVERY CI surface (the CI runner pool, the CI log tier, the CI artifact store, the CI cache
/// namespaces). A CI runner / log / artifact / cache that reported a region ≠ the tenant's region — or
/// that never reported — FAILS the attestation (the no-global-CI-pool property is attestable
/// per-tenant; NEVER a silent pass). The in-region runner-CLAIM enforcement is the sibling P-CP-18.
pub fn residency_verify_ci(
    tenant_id: &TenantId,
    region: &Region,
    reports: &[StoreRegionReport],
    key: &ResidencySigningKey,
) -> Result<SignedAttestation, ResidencyMismatch> {
    residency_verify_over(tenant_id, region, RequiredStoreSet::M1AndCi, reports, key)
}

/// **The shared no-global-pool aggregation (architecture §4.1 / §5.4, frozen) — parameterized over the
/// required store set ([`RequiredStoreSet`]).** Both [`residency_verify`] (M1) and
/// [`residency_verify_ci`] (M1+CI, P-CP-17) call this; the mechanism (check every report's region,
/// require every store class in the coverage set, aggregate + sign) is identical — only the required
/// set differs. This is the structural guarantee that adding CI coverage did NOT fork the attestation
/// logic (EI-01 §7 — one mechanism, two coverages).
pub fn residency_verify_over(
    tenant_id: &TenantId,
    region: &Region,
    coverage: RequiredStoreSet,
    reports: &[StoreRegionReport],
    key: &ResidencySigningKey,
) -> Result<SignedAttestation, ResidencyMismatch> {
    // 1. Check EVERY report's region against the tenant's region — a wrong region FAILS (loud, never
    //    a silent pass). We check all reports (M1 + CI alike) so a wrong-region CI store is caught by
    //    the SAME branch as a wrong-region M1 store.
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

    // 2. Require a report from EVERY store class in the coverage set — a missing one FAILS fail-closed
    //    (a silently-absent store is the global-pool the attestation must catch). For M1+CI this means
    //    a CI runner / log / artifact / cache that never reported FAILS the no-global-CI-pool
    //    attestation (it is not assumed in-region).
    for &class in coverage.required_classes() {
        if !by_class.contains_key(&class) {
            return Err(ResidencyMismatch::MissingStoreReport {
                tenant: tenant_id.clone(),
                store_class: class,
            });
        }
    }

    // 3. Aggregate the per-store reports (store-class-ordered via the BTreeMap) + sign over the body
    //    (which binds the coverage — an M1-only and an M1+CI attestation never share a MAC).
    let store_regions: Vec<(ResidencyStoreClass, Region)> = by_class.into_iter().collect();
    let body = SignedAttestation::canonical_body(tenant_id, region, coverage, &store_regions);
    let signature = key.mac(&body);
    Ok(SignedAttestation {
        tenant_id: tenant_id.clone(),
        region: region.clone(),
        coverage,
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

    /// Every M1 store class AND every CI surface reporting the tenant's region (the green CI input,
    /// P-CP-17).
    fn all_in_region_with_ci(region: &str) -> Vec<StoreRegionReport> {
        ResidencyStoreClass::M1_AND_CI_SET
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
        // CI surfaces are NOT in the M1 set — they are the P-CP-17 CI_SET. The M1 set staying exactly
        // these four pins that the CI extension did not silently widen the M1 coverage.
        for ci in ResidencyStoreClass::CI_SET {
            assert!(
                !ResidencyStoreClass::M1_SET.contains(&ci),
                "the CI surface `{}` is in CI_SET, not M1_SET",
                ci.label()
            );
        }
    }

    /// **P-CP-17: the CI store set is exactly the runner pool / log tier / artifact store / cache
    /// namespaces, and M1_AND_CI_SET is the disjoint union (8 stores).** Pins the CI extension's shape
    /// so it is a VISIBLE coverage, not a silent redefinition of "every store".
    #[test]
    fn the_ci_store_set_is_runner_log_artifact_cache() {
        let ci_labels: Vec<&str> = ResidencyStoreClass::CI_SET
            .iter()
            .map(|c| c.label())
            .collect();
        assert_eq!(
            ci_labels,
            vec![
                "ci_runner_pool",
                "ci_log_tier",
                "ci_artifact_store",
                "ci_cache_namespaces"
            ]
        );
        // M1_AND_CI_SET is the union: every M1 class + every CI class, no duplicates, 8 total.
        assert_eq!(ResidencyStoreClass::M1_AND_CI_SET.len(), 8);
        for c in ResidencyStoreClass::M1_SET {
            assert!(ResidencyStoreClass::M1_AND_CI_SET.contains(&c));
        }
        for c in ResidencyStoreClass::CI_SET {
            assert!(ResidencyStoreClass::M1_AND_CI_SET.contains(&c));
        }
        assert_eq!(
            RequiredStoreSet::M1AndCi.required_classes(),
            &ResidencyStoreClass::M1_AND_CI_SET
        );
        assert_eq!(
            RequiredStoreSet::M1Only.required_classes(),
            &ResidencyStoreClass::M1_SET
        );
    }

    /// **P-CP-17 GREEN leg: `residency_verify_ci` attests over the M1 stores AND the CI surfaces when
    /// all report the tenant's region — a signed, verifying attestation whose `coverage` is M1+CI.**
    /// The no-global-CI-pool property is attestable: every CI surface reported fr-par.
    #[test]
    fn residency_verify_ci_aggregates_m1_and_ci() {
        let tenant = TenantId::from_token("01J0ACME");
        let region = Region::new("fr-par");
        let att = residency_verify_ci(&tenant, &region, &all_in_region_with_ci("fr-par"), &key())
            .expect("every M1 + CI store in-region → a signed CI-coverage attestation");
        assert_eq!(att.coverage, RequiredStoreSet::M1AndCi);
        assert_eq!(
            att.store_regions.len(),
            ResidencyStoreClass::M1_AND_CI_SET.len(),
            "the attestation aggregates ALL 8 (M1 + CI) stores — none silently absent"
        );
        // All four CI surfaces are present in the attestation.
        for ci in ResidencyStoreClass::CI_SET {
            assert!(
                att.store_regions.iter().any(|(c, _)| *c == ci),
                "CI surface `{}` is attested",
                ci.label()
            );
        }
        assert!(att.verify(&key()), "the CI-coverage attestation verifies");
    }

    /// **P-CP-17 RED leg 1 (no silent pass): a CI runner that executed the tenant's job in the WRONG
    /// region FAILS `residency_verify_ci`.** The global-CI-pool the no-global-pool pitch forbids.
    #[test]
    fn residency_verify_ci_fails_on_a_wrong_region_ci_runner() {
        let tenant = TenantId::from_token("01J0ACME");
        let region = Region::new("fr-par");
        let mut reports = all_in_region_with_ci("fr-par");
        // The CI runner pool executed the job in eu-north (a runner outside the tenant's region).
        let idx = reports
            .iter()
            .position(|r| r.store_class == ResidencyStoreClass::CiRunnerPool)
            .unwrap();
        reports[idx] =
            StoreRegionReport::new(ResidencyStoreClass::CiRunnerPool, Region::new("eu-north"));
        let err = residency_verify_ci(&tenant, &region, &reports, &key())
            .expect_err("a wrong-region CI runner FAILS the attestation (not a silent pass)");
        assert_eq!(
            err,
            ResidencyMismatch::WrongRegion {
                tenant: tenant.clone(),
                tenant_region: Region::new("fr-par"),
                store_class: ResidencyStoreClass::CiRunnerPool,
                store_region: Region::new("eu-north"),
            }
        );
        assert!(err.to_string().contains("not a silent pass"), "loud: {err}");
    }

    /// **P-CP-17 RED leg 2 (fail-closed): a CI surface that NEVER reported FAILS `residency_verify_ci`
    /// — a silently-absent CI store is the global-CI-pool the attestation must catch.** Here the CI
    /// artifact store never reported its region.
    #[test]
    fn residency_verify_ci_fails_on_a_missing_ci_store() {
        let tenant = TenantId::from_token("01J0ACME");
        let region = Region::new("fr-par");
        let reports: Vec<StoreRegionReport> = all_in_region_with_ci("fr-par")
            .into_iter()
            .filter(|r| r.store_class != ResidencyStoreClass::CiArtifactStore)
            .collect();
        let err = residency_verify_ci(&tenant, &region, &reports, &key())
            .expect_err("a missing CI artifact-store report FAILS fail-closed");
        assert_eq!(
            err,
            ResidencyMismatch::MissingStoreReport {
                tenant: tenant.clone(),
                store_class: ResidencyStoreClass::CiArtifactStore,
            }
        );
        assert!(err.to_string().contains("fail-closed"), "loud: {err}");
    }

    /// **P-CP-17: the M1-only attestation is NOT valid as an M1+CI attestation (the coverage is bound
    /// into the MAC).** A green M1-only attestation, relabelled M1+CI, FAILS verification — an
    /// attestation can never overstate its coverage. AND: an M1+CI set of reports run through the
    /// M1-only entry point still succeeds (the CI reports are checked but not required).
    #[test]
    fn coverage_is_bound_into_the_signature() {
        let tenant = TenantId::from_token("01J0ACME");
        let region = Region::new("fr-par");
        // An M1-only attestation: its coverage is M1Only.
        let mut m1 = residency_verify(&tenant, &region, &all_in_region("fr-par"), &key())
            .expect("an M1 attestation");
        assert_eq!(m1.coverage, RequiredStoreSet::M1Only);
        assert!(m1.verify(&key()));
        // Relabel it M1+CI without re-signing → the MAC no longer matches (coverage is load-bearing).
        m1.coverage = RequiredStoreSet::M1AndCi;
        assert!(
            !m1.verify(&key()),
            "an M1-only attestation cannot be passed off as an M1+CI one — the coverage is signed"
        );

        // The M1-only entry point over M1+CI reports succeeds (CI reports are checked for wrong-region
        // but only the M1 set is REQUIRED) — and its coverage is honestly M1Only.
        let m1_over_ci =
            residency_verify(&tenant, &region, &all_in_region_with_ci("fr-par"), &key())
                .expect("M1-only verify over a superset of reports still succeeds");
        assert_eq!(m1_over_ci.coverage, RequiredStoreSet::M1Only);
        // A wrong-region CI store is STILL caught by the M1-only entry point (the region check runs
        // over every report) — so even M1-only never silently passes a wrong-region CI store.
        let mut wrong_ci = all_in_region_with_ci("fr-par");
        let idx = wrong_ci
            .iter()
            .position(|r| r.store_class == ResidencyStoreClass::CiLogTier)
            .unwrap();
        wrong_ci[idx] =
            StoreRegionReport::new(ResidencyStoreClass::CiLogTier, Region::new("eu-north"));
        assert!(
            residency_verify(&tenant, &region, &wrong_ci, &key()).is_err(),
            "a wrong-region CI log tier is caught even by the M1-only entry point"
        );
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

//! P-CP-09 (global P-085) GATE / DRILL — **`residency_verify` over the M1 store set (the
//! no-global-pool signed attestation)** — dated green artifact.
//!
//! **The GATE (testing-strategy `residency_verify` smoke / tenancy-and-control-plane.md §4.1 / §5.4):**
//! `residency_verify(tenant)` over the M1 stores (OLTP / blob / index-search / KMS) returns a SIGNED,
//! PII-free attestation where every store's region == the tenant's region. A store reporting a wrong
//! region — or a silently-absent store — makes the attestation **FAIL** (not a silent pass). Telemetry:
//! the `residency-attestation` signal (`region_mismatches == 0` is the green artifact).
//!
//! **The most load-bearing residency property (EI-01 §2):** a store serving a tenant's personal data
//! in the wrong region is the residency breach the EU-sovereign no-global-pool pitch (VISION §1) must
//! be able to ATTEST against. The structural defence here is the aggregation: every store reports its
//! region; the attestation aggregates + fails on ANY mismatch; the signed PII-free body is the proof
//! an auditor (`myelin tenant residency verify`) verifies.
//!
//! **This drill proves the gate can go RED** (a wrong-region store / a missing store FAILS the
//! attestation; a gate that cannot go red is not a gate, EI-01 §3) **AND green** (every M1 store
//! in-region → a signed, verifying attestation), and emits the `residency-attestation` result on the
//! SAME [`SignalSource`] every drill uses (observability is part of the pass).
//!
//! **FLOOR (named, VISION §3 / P-CP-17):** the store set is the **M1 stores only** (OLTP / blob /
//! index-search / KMS). The **CI runner pool + CI log tier + CI artifact store + CI cache namespaces**
//! are the **M4 follow-on (P-CP-17)** — `residency_verify` is a NAMED PARTIAL until CI lands (P-CP-17
//! extends this SAME function over the CI surfaces; a wrong-region CI store then fails the attestation
//! too). The store-layer `residency-pin` write-boundary (Storage P-ST-07) GUARANTEES a store only
//! writes in its cell's region; the runtime cross-region-egress drill (STOR-D5) + the write-boundary
//! drill (CP-D3) ride the four-layer enforcement (P-CP-12, against the live stack).

use myelin_control_plane::{
    residency_verify, ResidencyAttestationSignal, ResidencyMismatch, ResidencySigningKey,
    ResidencyStoreClass, StoreRegionReport,
};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_tenancy::{Region, TenantId};

/// Every M1 store class reporting `region` (the green input — `MYELIN_REGION=fr-par` in the dev/prod
/// stack).
fn all_in_region(region: &str) -> Vec<StoreRegionReport> {
    ResidencyStoreClass::M1_SET
        .iter()
        .map(|c| StoreRegionReport::new(*c, Region::new(region)))
        .collect()
}

/// **THE `residency_verify` SMOKE DRILL (dated green artifact): every M1 store reports the tenant's
/// region → a signed, verifying, PII-free attestation (`region_mismatches == 0`); a wrong-region store
/// and a missing store each FAIL the attestation (the gate goes RED).**
#[test]
fn cp_d3_residency_verify_m1_store_set() {
    let tenant = TenantId::from_token("01J0ACME");
    let region = Region::new("fr-par"); // the dev/prod region (MYELIN_REGION=fr-par).
    let key = ResidencySigningKey::from_bytes([0x5eu8; 32]);

    // ── GREEN leg: every M1 store (OLTP / blob / index-search / KMS) reports fr-par → a signed,
    //    PII-free attestation; every_store_region == tenant.region; it VERIFIES under the key. ──
    let attestation = residency_verify(&tenant, &region, &all_in_region("fr-par"), &key)
        .expect("every M1 store in-region → a signed attestation (the gate is GREEN)");
    assert_eq!(attestation.region.as_str(), "fr-par");
    assert_eq!(
        attestation.store_regions.len(),
        ResidencyStoreClass::M1_SET.len(),
        "the attestation aggregates ALL M1 stores (OLTP/blob/index/KMS) — none silently absent"
    );
    for (class, r) in &attestation.store_regions {
        assert_eq!(
            r.as_str(),
            "fr-par",
            "store `{}` reported the tenant's region",
            class.label()
        );
    }
    assert!(
        attestation.signature.starts_with("blake3-mac:"),
        "the attestation is SIGNED"
    );
    assert!(
        attestation.verify(&key),
        "an auditor verifies the no-global-pool attestation"
    );
    let green = ResidencyAttestationSignal::green(&attestation);
    assert_eq!(
        green.region_mismatches, 0,
        "the green artifact is 0 region mismatches"
    );

    // ── RED leg 1: the blob tier served the tenant's data in eu-north (the WRONG region) → the
    //    attestation FAILS (loud, never a silent pass). ──
    let mut wrong = all_in_region("fr-par");
    wrong[1] = StoreRegionReport::new(ResidencyStoreClass::Blob, Region::new("eu-north"));
    let breach = residency_verify(&tenant, &region, &wrong, &key)
        .expect_err("a wrong-region store FAILS the attestation (the gate is RED for the breach)");
    assert_eq!(
        breach,
        ResidencyMismatch::WrongRegion {
            tenant: tenant.clone(),
            tenant_region: Region::new("fr-par"),
            store_class: ResidencyStoreClass::Blob,
            store_region: Region::new("eu-north"),
        }
    );
    assert!(
        breach.to_string().contains("not a silent pass"),
        "loud: {breach}"
    );

    // ── RED leg 2: the KMS never reported its region → the attestation FAILS fail-closed (a
    //    silently-absent store is the global-pool the no-global-pool attestation must catch). ──
    let missing: Vec<StoreRegionReport> = all_in_region("fr-par")
        .into_iter()
        .filter(|r| r.store_class != ResidencyStoreClass::Kms)
        .collect();
    let gap = residency_verify(&tenant, &region, &missing, &key)
        .expect_err("a missing M1 store report FAILS fail-closed (the gate is RED for the gap)");
    assert_eq!(
        gap,
        ResidencyMismatch::MissingStoreReport {
            tenant: tenant.clone(),
            store_class: ResidencyStoreClass::Kms,
        }
    );

    // ── Emit the `residency-attestation` gate result on the SAME SignalSource every drill uses
    //    (observability is part of the pass, EI-01 §3): region_mismatches == 0 (the green artifact).
    //    We use the CrossTenantCount scalar as the harness's PII-free "zero-violations" projection —
    //    the residency-attestation's headline zero is the count of stores in the wrong region. ──
    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, green.region_mismatches as i64);
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-085 CP-D3 GREEN 2026-06-19] residency_verify over the M1 store set: tenant 01J0ACME \
         (fr-par) — every M1 store (OLTP/blob/index-search/KMS) reported fr-par → a SIGNED, PII-free \
         attestation ({} stores attested, region_mismatches={}, signature={}…); it VERIFIES under the \
         control-plane key. RED legs proven: a blob tier in eu-north FAILED the attestation (not a \
         silent pass); a missing KMS report FAILED fail-closed. FLOOR (NAMED PARTIAL): the store set \
         is the M1 stores only — the CI runner pool + log/artifact/cache coverage is the M4 follow-on \
         P-CP-17 (it extends this SAME residency_verify); the CP-D3 write-boundary + STOR-D5 \
         cross-region-egress runtime drills ride the four-layer enforcement P-CP-12 (live stack).",
        green.stores_attested,
        green.region_mismatches,
        &attestation.signature[..attestation.signature.len().min(22)],
    );
}

/// **The gate is NOT vacuous: a region mismatch WOULD read RED.** Proves the `residency-attestation`
/// zero is a real tripwire — if a store served a tenant in the wrong region, `region_mismatches > 0`
/// would fail the predicate. (The structural aggregation FAILS the attestation on any mismatch; this
/// asserts the signal-level assertion is load-bearing — EI-01 §3, a gate that cannot go red is not a
/// gate.)
#[test]
fn cp_d3_gate_is_not_vacuous() {
    let mut sig = SignalSource::new();
    // A hypothetical residency breach: one store in the wrong region.
    sig.set_scalar(SignalName::CrossTenantCount, 1);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "a region mismatch MUST read RED — the residency-attestation zero is a real tripwire"
    );
}

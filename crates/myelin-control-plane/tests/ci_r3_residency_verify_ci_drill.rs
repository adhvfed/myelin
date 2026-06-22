//! P-CP-17 (global P-324) GATE / DRILL — **CI-R3: `residency_verify` CI-store coverage (the
//! no-global-CI-pool signed attestation)** — dated green artifact.
//!
//! **The GATE (tenancy-and-control-plane.md §5.4 / §4.1, C-2):** an EU-resident tenant's CI run → the
//! CI runner pool, the CI log tier (T3 content-addressed segments, Storage 11.8), the CI artifact
//! store, and the CI cache namespaces (incl. the trust-tier/branch-scoped namespaces, Storage 11.2)
//! all report region; `residency_verify_ci(tenant)` attests over the FULL M1+CI store set and returns
//! a SIGNED, PII-free attestation where every store's region == the tenant's region. A CI runner / log
//! / artifact / cache reporting a WRONG region — or a silently-absent CI store — makes the attestation
//! **FAIL** (0 silent pass). Telemetry: the `residency-attestation` signal, now covering the CI stores
//! (`region_mismatches == 0` is the green artifact).
//!
//! **The no-global-pool property is now ATTESTABLE per-tenant (VISION §1):** the EU-sovereign pitch's
//! *no-global-CI-pool* claim is structurally checkable — a CI runner that executed a tenant's job in
//! the wrong region FAILS `residency_verify`, it does not silently pass (EI-01 §3). This drill proves
//! the gate can go RED (a wrong-region / missing CI store FAILS) AND green (every M1 + CI store
//! in-region → a signed, verifying attestation), and emits the `residency-attestation` result on the
//! SAME [`SignalSource`] every drill uses (observability is part of the pass).
//!
//! **FLOOR CLOSED:** this is the M4 follow-on that CLOSES the P-CP-09 (P-085) `residency_verify`-over-
//! M1-stores-only named partial — the CI surfaces are now covered. The in-region runner-CLAIM
//! enforcement (an EU tenant's CI run claimed ONLY by an in-region runner) is the sibling P-CP-18
//! (P-325); the CI subsystem's full crate + its live runner-pool region report land with CI in M4 and
//! feed these SAME CI `ResidencyStoreClass` variants (the live store-driver edge, EI-01 §1 — the
//! aggregation + fail-on-mismatch + signed attestation are fully real and tested here).

use myelin_control_plane::{
    residency_verify_ci, RequiredStoreSet, ResidencyAttestationSignal, ResidencyMismatch,
    ResidencySigningKey, ResidencyStoreClass, StoreRegionReport,
};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_tenancy::{Region, TenantId};

/// Every M1 store class AND every CI surface reporting `region` (the green input —
/// `MYELIN_REGION=fr-par` in the dev/prod stack).
fn all_in_region_with_ci(region: &str) -> Vec<StoreRegionReport> {
    ResidencyStoreClass::M1_AND_CI_SET
        .iter()
        .map(|c| StoreRegionReport::new(*c, Region::new(region)))
        .collect()
}

/// **THE CI-R3 ATTESTATION-LEG DRILL (dated green artifact): an EU-resident tenant's CI run → every
/// M1 + CI store reports the tenant's region → a signed, verifying, PII-free attestation
/// (`region_mismatches == 0`, coverage = M1+CI); a wrong-region CI runner / log / artifact / cache and
/// a missing CI store each FAIL the attestation (the gate goes RED, 0 silent pass).**
#[test]
fn ci_r3_residency_verify_ci_store_set() {
    let tenant = TenantId::from_token("01J0EUTENANT");
    let region = Region::new("fr-par"); // the EU tenant's region of record (MYELIN_REGION=fr-par).
    let key = ResidencySigningKey::from_bytes([0xc1u8; 32]);

    // ── GREEN leg: every M1 store + every CI surface (runner pool / log tier / artifact store / cache
    //    namespaces) reports fr-par → a signed, PII-free attestation; every_store_region ==
    //    tenant.region; coverage is M1+CI; it VERIFIES under the key. ──
    let attestation = residency_verify_ci(&tenant, &region, &all_in_region_with_ci("fr-par"), &key)
        .expect("every M1 + CI store in-region → a signed CI-coverage attestation (gate GREEN)");
    assert_eq!(attestation.region.as_str(), "fr-par");
    assert_eq!(
        attestation.coverage,
        RequiredStoreSet::M1AndCi,
        "the attestation declares it covered the M1 + CI store set (no-global-CI-pool)"
    );
    assert_eq!(
        attestation.store_regions.len(),
        ResidencyStoreClass::M1_AND_CI_SET.len(),
        "the attestation aggregates ALL 8 stores (M1 + CI) — none silently absent"
    );
    // Every CI surface is present + in-region (the no-global-CI-pool surfaces).
    for ci in ResidencyStoreClass::CI_SET {
        let (_, r) = attestation
            .store_regions
            .iter()
            .find(|(c, _)| *c == ci)
            .unwrap_or_else(|| panic!("CI surface `{}` is attested", ci.label()));
        assert_eq!(
            r.as_str(),
            "fr-par",
            "CI surface `{}` in-region",
            ci.label()
        );
    }
    assert!(
        attestation.signature.starts_with("blake3-mac:"),
        "the CI-coverage attestation is SIGNED"
    );
    assert!(
        attestation.verify(&key),
        "an auditor verifies the no-global-CI-pool attestation"
    );
    let green = ResidencyAttestationSignal::green(&attestation);
    assert_eq!(
        green.region_mismatches, 0,
        "the green artifact is 0 region mismatches over the M1 + CI store set"
    );
    assert_eq!(
        green.stores_attested,
        ResidencyStoreClass::M1_AND_CI_SET.len() as u32,
        "all 8 stores (M1 + CI) attested"
    );

    // ── RED leg 1: the CI runner pool executed the tenant's job in eu-north (the WRONG region — the
    //    global-CI-pool the no-global-pool pitch forbids) → the attestation FAILS (0 silent pass). ──
    let mut wrong_runner = all_in_region_with_ci("fr-par");
    let idx = wrong_runner
        .iter()
        .position(|r| r.store_class == ResidencyStoreClass::CiRunnerPool)
        .expect("the CI runner pool reports");
    wrong_runner[idx] =
        StoreRegionReport::new(ResidencyStoreClass::CiRunnerPool, Region::new("eu-north"));
    let runner_breach = residency_verify_ci(&tenant, &region, &wrong_runner, &key).expect_err(
        "a CI runner in the wrong region FAILS the attestation (gate RED, 0 silent pass)",
    );
    assert_eq!(
        runner_breach,
        ResidencyMismatch::WrongRegion {
            tenant: tenant.clone(),
            tenant_region: Region::new("fr-par"),
            store_class: ResidencyStoreClass::CiRunnerPool,
            store_region: Region::new("eu-north"),
        }
    );
    assert!(
        runner_breach.to_string().contains("not a silent pass"),
        "loud: {runner_breach}"
    );

    // ── RED leg 2: the CI log tier sealed the run's segments in eu-north (a wrong-region log tier,
    //    Storage 11.8) → the attestation FAILS. ──
    let mut wrong_log = all_in_region_with_ci("fr-par");
    let idx = wrong_log
        .iter()
        .position(|r| r.store_class == ResidencyStoreClass::CiLogTier)
        .expect("the CI log tier reports");
    wrong_log[idx] =
        StoreRegionReport::new(ResidencyStoreClass::CiLogTier, Region::new("eu-north"));
    let log_breach = residency_verify_ci(&tenant, &region, &wrong_log, &key)
        .expect_err("a CI log tier in the wrong region FAILS the attestation (gate RED)");
    assert!(
        matches!(
            log_breach,
            ResidencyMismatch::WrongRegion {
                store_class: ResidencyStoreClass::CiLogTier,
                ..
            }
        ),
        "the wrong-region CI log tier is the named breach: {log_breach}"
    );

    // ── RED leg 3: the CI cache namespaces never reported their region → the attestation FAILS
    //    fail-closed (a silently-absent CI store is the global-CI-pool the attestation must catch). ──
    let missing_cache: Vec<StoreRegionReport> = all_in_region_with_ci("fr-par")
        .into_iter()
        .filter(|r| r.store_class != ResidencyStoreClass::CiCacheNamespaces)
        .collect();
    let cache_gap = residency_verify_ci(&tenant, &region, &missing_cache, &key)
        .expect_err("a missing CI cache report FAILS fail-closed (gate RED)");
    assert_eq!(
        cache_gap,
        ResidencyMismatch::MissingStoreReport {
            tenant: tenant.clone(),
            store_class: ResidencyStoreClass::CiCacheNamespaces,
        }
    );
    assert!(
        cache_gap.to_string().contains("fail-closed"),
        "loud: {cache_gap}"
    );

    // ── Emit the `residency-attestation` gate result (now covering the CI stores) on the SAME
    //    SignalSource every drill uses (observability is part of the pass, EI-01 §3): the headline zero
    //    is the count of CI/M1 stores in the wrong region. ──
    let mut sig = SignalSource::new();
    sig.set_scalar(SignalName::CrossTenantCount, green.region_mismatches as i64);
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-324 CI-R3 GREEN 2026-06-22] residency_verify CI-store coverage: EU tenant 01J0EUTENANT \
         (fr-par) — every M1 store (OLTP/blob/index/KMS) AND every CI surface (runner pool / log tier \
         / artifact store / cache namespaces) reported fr-par → a SIGNED, PII-free attestation \
         (coverage=m1+ci, {} stores attested, region_mismatches={}, signature={}…); it VERIFIES under \
         the control-plane key. RED legs proven (0 silent pass): a CI runner in eu-north FAILED; a CI \
         log tier in eu-north FAILED; a missing CI cache report FAILED fail-closed. FLOOR CLOSED: the \
         P-CP-09 (P-085) residency_verify-over-M1-stores-only named partial is now CLOSED — the \
         no-global-CI-pool property is ATTESTABLE per-tenant. The in-region runner-CLAIM enforcement \
         is the sibling P-CP-18 (P-325); the live CI runner-pool region report rides CI's M4 crate.",
        green.stores_attested,
        green.region_mismatches,
        &attestation.signature[..attestation.signature.len().min(22)],
    );
}

/// **The CI-R3 gate is NOT vacuous: a wrong-region CI store WOULD read RED.** Proves the
/// `residency-attestation` zero (now over the CI stores) is a real tripwire — if a CI runner served a
/// tenant in the wrong region, `region_mismatches > 0` would fail the predicate. (EI-01 §3 — a gate
/// that cannot go red is not a gate.)
#[test]
fn ci_r3_gate_is_not_vacuous() {
    let mut sig = SignalSource::new();
    // A hypothetical CI residency breach: one CI store in the wrong region.
    sig.set_scalar(SignalName::CrossTenantCount, 1);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "a CI-store region mismatch MUST read RED — the residency-attestation zero is a real tripwire"
    );
}

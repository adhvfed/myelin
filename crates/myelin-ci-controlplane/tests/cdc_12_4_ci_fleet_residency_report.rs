//! # The CDC pair for contract 12.4 (CI fleet half) — the **fleet's `residency_verify` report**
//! (CI-P14 → P-357, M4)
//!
//! **Contract-index row 12.4** (`residency_verify(tenant_id) → SignedAttestation` — every store
//! REPORTS the tenant's region; the no-global-pool property is attestable; **SHARPENED** to cover the
//! CI runner pool region, recon §10). The AGGREGATION+SIGN half is the control plane's authority
//! (`residency_verify_ci`, P-CP-17 / P-324 — `myelin-control-plane`); THIS file is the **CI fleet
//! PROVIDER side**: the EU fleet autoscaler REPORTS its runner pool's region
//! ([`FleetResidencyReport`]) so the no-global-pool attestation can aggregate it, and a pool serving a
//! tenant in the WRONG region FAILs the per-report match-of-record check (the breach the attestation
//! catches).
//!
//! ## The name-agreement anchor (why this side does not import the control-plane crate)
//! `myelin-ci-controlplane` is a CI subsystem LEAF consumer; the §2.9 acyclic DAG forbids it depending
//! on the control-plane authority (`myelin-control-plane`). The CONSUMER (the control plane's
//! aggregation/sign) cannot be imported here, so the agreement is asserted against the architecture
//! invariant: the fleet's report carries the bare `(tenant, region)` pair the aggregator keys on, and
//! its `matches_region_of_record` predicate is the SAME per-report check the control-plane aggregation
//! applies (a `false` is the residency breach). The control-plane-side CDC
//! (`cdc_12_4_residency_verify_ci_coverage.rs`) proves the CONSUMER DEMANDS CI coverage + signs the
//! attestation; THIS one freezes the PROVIDER's report shape + the breach predicate. A rename/region
//! drift on either side is a CDC break, never a silent leak (EI-01 §7). The two CDCs together are the
//! row-12.4 CI-fleet slice: this one freezes what the fleet REPORTS, that one proves the control plane
//! AGGREGATES + signs it.

use myelin_ci_controlplane::{EuFleetProvider, FleetResidencyReport, GenericEuIaasAdapter};
use myelin_ci_sandbox::Region;

fn fr_par() -> Region {
    Region::new("fr-par")
}
fn eu_north() -> Region {
    Region::new("eu-north")
}

/// **PROVIDER → the fleet REPORTS its runner pool's `(tenant, region)` into `residency_verify`
/// (contract 12.4, consumed).** The report is the bare pair the no-global-pool aggregation keys on —
/// PII-free (an opaque tenant id + a region code, never personal data). A fleet pinned to fr-par
/// reports fr-par; the report's tenant is the pool's tenant.
#[test]
fn provider_the_fleet_reports_its_runner_pool_region() {
    let fleet = EuFleetProvider::new(GenericEuIaasAdapter, "01J0ACME", fr_par(), 100);
    let report = fleet.residency_report();
    assert_eq!(
        report.tenant_id, "01J0ACME",
        "the pool's tenant (opaque id)"
    );
    assert_eq!(
        report.region,
        fr_par(),
        "the pool's residency-pinned region"
    );
}

/// **CONSUMER → the per-report match-of-record check: a report matching the tenant's region of record
/// PASSES; a mismatch is the residency BREACH the no-global-pool attestation catches (contract
/// 12.4).** This is the SAME predicate the control-plane aggregation applies per-store — the fleet is
/// just another store the no-global-pool property covers.
#[test]
fn consumer_a_wrong_region_report_fails_the_match_of_record() {
    // The tenant's region of record is fr-par (the control plane's authoritative region).
    let region_of_record = fr_par();

    // GREEN: the fleet served the tenant in-region → the report agrees with the record.
    let in_region = FleetResidencyReport {
        tenant_id: "01J0ACME".into(),
        region: fr_par(),
    };
    assert!(
        in_region.matches_region_of_record(&region_of_record),
        "an in-region fleet report passes the no-global-pool attestation"
    );

    // RED: had the fleet served the tenant in eu-north, the report would DISAGREE — the breach the
    // aggregation catches (a runner pool in the wrong region FAILs residency_verify).
    let cross_region = FleetResidencyReport {
        tenant_id: "01J0ACME".into(),
        region: eu_north(),
    };
    assert!(
        !cross_region.matches_region_of_record(&region_of_record),
        "a wrong-region fleet report FAILs the attestation — the no-global-pool breach is caught"
    );
}

/// **PROVIDER ⇄ CONSUMER agree: the fleet's reported region IS the region the consumer checks against
/// the record — same `(tenant, region)` pair, no drift.** The fleet builds the report; the auditor
/// reads the SAME fields it keys the aggregation on. A region drift on either side breaks this CDC.
#[test]
fn provider_and_consumer_agree_on_the_report_pair() {
    let fleet = EuFleetProvider::new(GenericEuIaasAdapter, "01J0ACME", fr_par(), 100);
    let report = fleet.residency_report();

    // The consumer keys the aggregation on the report's (tenant, region); when the record matches the
    // pool's pin, the attestation is green — the two halves agree on the same pair.
    assert!(
        report.matches_region_of_record(fleet.cell_region()),
        "the fleet's report region IS its pinned cell region — the provider and the consumer agree"
    );
    assert_eq!(
        report.tenant_id,
        fleet.tenant_id(),
        "the report tenant IS the pool's tenant — no drift between provider and consumer"
    );
}

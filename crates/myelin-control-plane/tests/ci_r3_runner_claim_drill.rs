//! P-CP-18 (global P-325) GATE / DRILL — **CI-R3: residency-pinned runners (the in-region runner-claim
//! leg + the `residency-pin` leg on every CI-store write)** — dated green artifact.
//!
//! **The GATE (tenancy-and-control-plane.md §5.4, CI-R3 runner-claim leg):** an EU-resident tenant's CI
//! run is claimed ONLY by an in-region runner; logs/artifacts/caches never leave the region (within-EU
//! CDN); `residency-pin` passes on every write the CI run makes. Telemetry: the in-region runner-claim
//! decision + the `residency-pin` lint green on every CI-store write. Assert: an out-of-region runner
//! cannot claim an EU-resident tenant's run (**0 out-of-region claims**).
//!
//! **The split (who owns what):** the **CI subsystem owns the runner-claim MECHANISM**
//! (`myelin_ci_sandbox::JobLeaseStore::claim_for_labels`, whose `FOR UPDATE SKIP LOCKED` claim predicate
//! already SKIPS any job whose `region` ≠ the runner's cell region); **Tenancy owns the region-pin
//! ASSERTION** (`myelin_control_plane::RunnerClaimPin::admit_claim` — the control plane asserts the
//! runner's region == the tenant's region of record). This CROSS-OWNER drill proves the two halves
//! AGREE: an out-of-region runner is refused from BOTH sides (the CI claim skips it; the control-plane
//! pin rejects it), so the no-global-CI-pool property is enforced at claim time, not merely attestable.
//!
//! **This is the sibling of P-CP-17 (P-324, the attestation leg, `ci_r3_residency_verify_ci_drill.rs`):**
//! P-CP-17 made the no-global-CI-pool property ATTESTABLE (a wrong-region CI store FAILS
//! `residency_verify`); P-CP-18 makes it ENFORCED at claim time (an out-of-region runner cannot claim
//! the job in the first place) AND pins every CI-store write (logs/artifacts/caches never leave the
//! region). **No floor here** — this completes the CI residency posture begun in P-CP-17.

use myelin_ci_sandbox::{
    EgressPolicy, IdemToken, ImageRef, JobKind, JobLeaseStore, JobSpec, MeterTarget, QueuedJob,
    ResourceLimits, RunTokenRef, TrustTier, WorkspaceSpec,
};
use myelin_control_plane::ResidencyStoreClass;
use myelin_control_plane::{CiStoreWritePinError, OutOfRegionRunnerClaim, RunnerClaimPin};
use myelin_harness::{Predicate, SignalName, SignalSource};
use myelin_tenancy::{Region, TenantId};

/// A minimal digest-pinned CI [`JobSpec`] (the runner-claim drill only needs the spec to exist; the
/// sandbox launch is CI's own drill).
fn ci_spec(idem: &str) -> JobSpec {
    JobSpec::new(
        JobKind::Ci,
        ImageRef::pinned("registry.example/runner@sha256:0123456789abcdef000000000000000000000000000000000000000000000000").unwrap(),
        vec!["cargo".into(), "test".into()],
        vec![],
        vec![],
        EgressPolicy::deny_all(),
        ResourceLimits {
            cpu_millis: 1000,
            mem_bytes: 256 << 20,
            disk_bytes: 1 << 30,
            pids_max: 128,
            timeout_secs: 600,
        },
        WorkspaceSpec::default(),
        TrustTier::Trusted,
        RunTokenRef {
            jti: "jti-1".into(),
        },
        MeterTarget {
            reserve_id: "res-1".into(),
        },
        IdemToken(idem.into()),
    )
    .unwrap()
}

/// **THE CI-R3 RUNNER-CLAIM-LEG DRILL (dated green artifact): an EU-resident tenant's CI run is claimed
/// ONLY by an in-region runner; an out-of-region runner is REJECTED from BOTH the CI claim mechanism AND
/// the control-plane region-pin (0 out-of-region claims); every CI-store write passes the `residency-pin`
/// leg (logs/artifacts/caches never leave the region).**
#[test]
fn ci_r3_residency_pinned_runners() {
    let tenant = TenantId::from_token("01J0EUTENANT");
    let region = Region::new("fr-par"); // the EU tenant's region of record (MYELIN_REGION=fr-par).
    let pin = RunnerClaimPin::for_tenant(tenant.clone(), region.clone());

    // ── Tenancy's region-pin ASSERTION leg (the control-plane half) ──
    // GREEN: an in-region runner (fr-par) claims the EU tenant's CI run.
    pin.admit_claim(&Region::new("fr-par"))
        .expect("an in-region runner claims the EU tenant's CI run (gate GREEN)");
    // RED: an out-of-region runner (eu-north) cannot claim it (0 out-of-region claims, 0 silent pass).
    let refused: OutOfRegionRunnerClaim = pin
        .admit_claim(&Region::new("eu-north"))
        .expect_err("an out-of-region runner cannot claim the EU tenant's CI run (gate RED)");
    assert_eq!(refused.tenant_region.as_str(), "fr-par");
    assert_eq!(refused.runner_region.as_str(), "eu-north");
    assert!(
        refused.to_string().contains("ONLY by an in-region runner"),
        "loud: {refused}"
    );
    assert_eq!(
        pin.out_of_region_claims_admitted(),
        0,
        "0 out-of-region claims admitted"
    );

    // ── CROSS-OWNER agreement: the CI claim MECHANISM agrees with the control-plane pin ──
    // The CI subsystem's `JobLeaseStore::claim_for_labels` is the live runner-claim. Seed the EU
    // tenant's job into the queue (region = fr-par) and prove:
    //   (a) an IN-REGION runner (fr-par) claims it — AND the control-plane pin admits the same runner;
    //   (b) an OUT-OF-REGION runner (eu-north) claims NOTHING — the claim predicate skips it — AND the
    //       control-plane pin rejects the same runner. The two halves agree: no global CI pool.
    let q = JobLeaseStore::new();
    q.enqueue(QueuedJob::new(
        tenant.clone(),
        region.clone(),
        "run-1",
        "job-1",
        vec!["linux".into()],
        ci_spec("idem-1"),
    ));
    let tiers = [TrustTier::Trusted];

    // (b) the out-of-region runner: the CI claim mechanism finds NO in-region job for it.
    let out_of_region_claim = q.claim_for_labels(
        "runner-eu-north",
        &["linux".into()],
        &tiers,
        &Region::new("eu-north"), // the runner's cell region.
        1000,
        30,
    );
    assert!(
        out_of_region_claim.is_none(),
        "the CI claim mechanism SKIPS the EU tenant's fr-par job for an eu-north runner (no global pool)"
    );
    // …and the control-plane pin would reject the same out-of-region runner (the assertion agrees).
    assert!(
        pin.admit_claim(&Region::new("eu-north")).is_err(),
        "the control-plane pin REJECTS the eu-north runner — the two halves agree (0 out-of-region claims)"
    );

    // (a) the in-region runner: the CI claim mechanism claims it, AND the control-plane pin admits it.
    let in_region_claim = q
        .claim_for_labels(
            "runner-fr-par",
            &["linux".into()],
            &tiers,
            &Region::new("fr-par"), // the runner's cell region == the tenant's region.
            1000,
            30,
        )
        .expect("an in-region (fr-par) runner claims the EU tenant's job");
    assert_eq!(in_region_claim.job_id, "job-1");
    assert_eq!(in_region_claim.region.as_str(), "fr-par");
    pin.admit_claim(&in_region_claim.region)
        .expect("the control-plane pin admits the in-region runner — the two halves agree");

    // ── the `residency-pin` leg on every CI-store write (logs/artifacts/caches never leave region) ──
    // The run writes its log tier (Storage 11.8), artifact store, and cache namespaces (Storage 11.2).
    // Each write passes the residency-pin leg in-region and is REJECTED out of region.
    for surface in ResidencyStoreClass::CI_SET {
        pin.pin_ci_store_write(surface, &Region::new("fr-par"))
            .unwrap_or_else(|e| {
                panic!("in-region CI write to `{}` admitted: {e}", surface.label())
            });
        let leak: CiStoreWritePinError = pin
            .pin_ci_store_write(surface, &Region::new("eu-north"))
            .expect_err("an out-of-region CI write is REJECTED (it never leaves the region)");
        assert!(
            matches!(leak, CiStoreWritePinError::OutOfRegion { .. }),
            "the out-of-region CI write is the named breach: {leak}"
        );
    }
    assert_eq!(
        pin.out_of_region_ci_writes_admitted(),
        0,
        "0 out-of-region CI-store writes admitted (logs/artifacts/caches stay in region)"
    );

    // ── Emit the CI-R3 gate result (the out-of-region runner-claim count) on the SAME SignalSource
    //    every drill uses (observability is part of the pass, EI-01 §3): the headline zero is the count
    //    of out-of-region claims admitted. ──
    let mut sig = SignalSource::new();
    sig.set_scalar(
        SignalName::CrossTenantCount,
        pin.out_of_region_claims_admitted() as i64,
    );
    sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
        .expect_green();

    println!(
        "[P-325 CI-R3 GREEN 2026-06-22] residency-pinned runners: EU tenant 01J0EUTENANT (fr-par) — an \
         in-region (fr-par) runner claimed the CI run; an out-of-region (eu-north) runner was REJECTED \
         from BOTH the CI claim mechanism (the claim predicate skipped the job) AND the control-plane \
         region-pin assertion (out_of_region_claims_admitted={}). Every CI-store write (log tier / \
         artifact store / cache namespaces) passed the residency-pin leg in-region and was REJECTED out \
         of region (out_of_region_ci_writes_admitted={}) — logs/artifacts/caches never leave the region. \
         The CI subsystem owns the runner-claim MECHANISM; Tenancy owns the region-pin ASSERTION; the \
         two halves AGREE. This completes the CI residency posture begun in P-CP-17 (P-324, the \
         attestation leg) — NO floor here.",
        pin.out_of_region_claims_admitted(),
        pin.out_of_region_ci_writes_admitted(),
    );
}

/// **The CI-R3 runner-claim gate is NOT vacuous: an out-of-region claim WOULD read RED.** Proves the
/// `out_of_region_claims_admitted` zero is a real tripwire — if an out-of-region runner ever claimed an
/// EU tenant's run, the count would be > 0 and fail the predicate. (EI-01 §3 — a gate that cannot go red
/// is not a gate.)
#[test]
fn ci_r3_runner_claim_gate_is_not_vacuous() {
    let mut sig = SignalSource::new();
    // A hypothetical residency breach: one out-of-region runner claimed the run.
    sig.set_scalar(SignalName::CrossTenantCount, 1);
    assert!(
        !sig.assert_signal(SignalName::CrossTenantCount, Predicate::Eq(0))
            .is_green(),
        "an out-of-region runner claim MUST read RED — the 0-out-of-region-claims zero is a real tripwire"
    );
}

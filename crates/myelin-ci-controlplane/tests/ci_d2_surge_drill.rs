//! # CI-D2 — the 30× CI surge family: the interactive lane holds, the batch/CI lane sheds, the tuned
//! DRR/shed-budget numbers + the pre-warm buffer sizing + the measured per-`fair_key` starvation signal
//! (CI-P30 / P-490, M5).
//!
//! **Drill catalogue:** `01-whole-system-e2e-and-drill-catalogue.md` row **CI-D2** (F6 surge family):
//! *"30× CI surge one tenant → interactive holds, batch sheds 429+Retry-After, others unaffected,
//! reserve refuses over-budget, killed-runner jobs re-queue within lease TTL 0 orphans"*, signals
//! `shed-counts/lane` + the per-`fair_key` wait-time histogram (contract 1.8), cadence `SCHED`. This is
//! the **CI slice** of the master M5 surge family (sibling to SUB-D3 / BUS-D7 / ID-D9 / FLOW-D8 / AG-D6 /
//! REF-D10 / SRCH-D6). **Architecture:** continuous-integration §2.2 (the DRR floor + the per-`fair_key`
//! starvation histogram), §2.4 (the 30× surge sheds batch/CI, holds interactive, others unaffected),
//! §5.4 (the pre-warm buffer sizing). **Contract-index:** row **1.11** (the shed order + the CI
//! per-surface budget — CONSUMED), row **1.8** (the telemetry survival signals — the per-`fair_key`
//! wait-time histogram + the per-lane shed count). **Doctrine:** EI-01 §3 (prove-it under 1×/10×/30×;
//! the multiplier is read from the FROZEN thresholds file, never hardcoded; never weaken a threshold to
//! pass), §2 (the protected lane; per-tenant blast radius); EI-04 §5 (the hierarchical scheduler is
//! promoted ONLY on a MEASURED starvation signal, never predicted).
//!
//! ## What this drill proves (the CI-D2 properties under the 30× CI surge)
//! Under a 30× CI storm (a 10k-job matrix push) by ONE tenant the CI surge gate + the DRR scheduler +
//! the dead-runner reaper:
//! 1. **ABSORB the storm by SHEDDING** the batch/CI lane (`429 + Retry-After`), never by growing
//!    dispatch latency unboundedly — `surging_batch_shed_count > 0`, every shed carries the surface's
//!    Retry-After (`myelin ci` honours it);
//! 2. **HOLD the protected interactive lane** — the surging tenant's OWN interactive PR-check is
//!    admitted (a PR-check never queues behind a batch matrix) AND an unrelated co-tenant's interactive
//!    dispatch is admitted within budget;
//! 3. keep **cross-tenant impact at 0** — the storm fills only the surging tenant's per-tenant bounded
//!    run-queue; the quiet co-tenant's lanes are untouched;
//! 4. a **KILLED runner's jobs re-queue within the lease TTL with 0 ORPHANS** (the dead-runner reaper);
//! 5. the **per-`fair_key` wait-time p99 stays WITHIN the starvation trigger** (flat DRR fairly
//!    interleaves the surging tenant — no starvation), so the hierarchical scheduler (CI-P29) stays a
//!    **named floor** (measured-not-predicted, open question 07#1).
//!
//! ## The load is REAL (derived from the P-S02 generator), the multiplier is from the FILE (EI-01 §3)
//! The storm-op count is DERIVED from a real `myelin_harness::LoadGenerator` run at the surge multiplier
//! (the CI-surge storm profile, the agent/CI-skewed mix) spread on the surging tenant — never a
//! hand-typed number. The surge multiplier is read from the workspace-root `thresholds.toml` `[surge]`
//! row (the versioned source of truth) and asserted to equal the documented default-to-beat
//! [`CI_SURGE_MULTIPLIER`] — a divergence is a LOUD failure, never a silent weakening.
//!
//! ## The CI-surge controls: the tuned DRR/shed-budget/pre-warm numbers (the CI-P30 DoD)
//! The tuned numbers (the per-tenant cap, the DRR quantum/ceiling, the starvation trigger, the pre-warm
//! sizing) are read from the FROZEN thresholds-file `[ci_surge]` row via
//! [`myelin_ci_controlplane::CiSurgeControls`]; the drill asserts the tuned cap EQUALS the `CiDispatch`
//! shed-budget cap (one number, not two). The numbers are MEASURED sufficient here — never a number
//! chosen to make the drill pass (a regression past the starvation trigger is the hierarchical-scheduler
//! promotion signal, never a lowered bar — EI-01 §3).
//!
//! ## Floors named (the prompt's honesty register)
//! - **The 30× world-scale FLEET-hardware load is the ONE legitimate remaining floor** (real fleet).
//!   Here the load is the P-S02 generator at 30× across the surging tenant; the fairness + shed-order +
//!   cross-tenant-0 + 0-orphan-reaper PROPERTIES are complete + testable now and do not change shape when
//!   the real cell carries the load.
//! - **The flat-DRR → hierarchical scheduler** (CI-P29) is promoted ONLY if the measured starvation
//!   signal fires — it did NOT here, so it stays a named floor (this drill MEASURES that it does not).
//!
//! Permanent-gate posture: re-run on every CI-surge-surface-touching change; contributes to the master
//! M5→M6 boundary (the F6 surge family green on the CI-dispatch surface).

use myelin_ci_controlplane::{
    drive_ci_d2_surge, CiSurgeControls, CiSurgeGate, CI_SURGE_MULTIPLIER,
};
use myelin_harness::load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, RecordingSink, StormProfile,
};
use myelin_substrate::shed::{RunClass, SurfaceBudget};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

/// Read the `[surge] multiplier` from the workspace-root `thresholds.toml` through the typed
/// [`Thresholds`] loader (the versioned source of truth) — the SAME loader every other surge drill uses.
/// A missing/unreadable file is a LOUD failure (EI-01 §3).
fn surge_multiplier_from_thresholds() -> u32 {
    let t = Thresholds::load_canonical().expect("the versioned thresholds file must load");
    let m = t.surge.multiplier;
    assert!(m > 0, "the surge multiplier must be a positive factor");
    m
}

/// Drive the P-S02 load generator at the surge multiplier (CI-surge storm profile, agent/CI-skewed mix)
/// on `surging` and return the number of **CI** (batch) dispatch ops the storm issues — the REAL derived
/// storm-op count (never hand-typed). CI dispatches project onto the batch/CI lane.
fn derived_ci_storm_ops(surging: &TenantId, base_requests: u64, multiplier: u32) -> u64 {
    let m = Multiplier::custom(multiplier).expect("a positive surge multiplier");
    let gen = LoadGenerator::new(
        base_requests,
        m,
        PrincipalMix::agent_skewed(), // agent/CI-skewed mix: mostly machine traffic, a thin human lane.
        StormProfile::ci_surge(),
        vec![surging.clone()],
    )
    .expect("a non-empty tenant list");
    let mut sink = RecordingSink::default();
    gen.drive(&mut sink);
    let ci_ops = sink
        .received
        .iter()
        .filter(|r| r.load_kind == LoadPrincipalKind::Ci)
        .count() as u64;
    assert!(
        ci_ops > 0,
        "the agent/CI-skewed surge mix must issue CI dispatch ops (the storm the batch lane sheds)"
    );
    ci_ops
}

/// **THE CI-D2 SURGE PROOF (the dated green artifact the DoD names).** A 30× CI storm by one tenant (the
/// storm-op count derived from a real generator run; the multiplier read from the FILE): the batch/CI
/// lane sheds (absorbed, not unbounded — `myelin ci` honours the Retry-After), the interactive lane
/// HOLDS (surging tenant's own + the quiet tenant's), cross-tenant impact 0, a killed runner's jobs
/// re-queue within the lease TTL with 0 orphans, and the per-`fair_key` wait p99 stays within the
/// starvation trigger (flat DRR holds — the hierarchical scheduler stays a named floor).
#[test]
fn ci_d2_surge_interactive_holds_batch_sheds_cross_tenant_zero_reaper_zero_orphans() {
    let multiplier = surge_multiplier_from_thresholds();
    assert_eq!(
        multiplier, CI_SURGE_MULTIPLIER,
        "the thresholds-file surge multiplier must match the documented CI default-to-beat \
         (a divergence is a LOUD failure, never a silent weakening — EI-01 §3)"
    );

    let surging = TenantId("noisy-ci-tenant".into());
    let quiet = TenantId("quiet-co-tenant".into());

    // The CI-surge controls are read FROM THE FROZEN thresholds file (the tuned DRR/cap/starvation/
    // pre-warm numbers). Construction asserts the tuned cap == the CiDispatch shed-budget cap (one
    // number, not two) — a divergence is a loud error.
    let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
    let controls =
        CiSurgeControls::from_thresholds(&thresholds).expect("the CI-surge controls from the file");
    assert_eq!(
        controls.multiplier(),
        multiplier,
        "the controls read the same 30× multiplier"
    );

    // The storm-op count is DERIVED from a real generator run at the surge multiplier (base 16 → with
    // the agent/CI-skewed mix's 20% CI weight that is ~96 CI ops at 30×, well past the per-tenant cap of
    // 64, so the storm MUST shed).
    let storm_ops = derived_ci_storm_ops(&surging, 16, multiplier) as u32;
    assert!(
        storm_ops > controls.per_tenant_in_flight_cap(),
        "the derived storm must exceed the per-tenant cap so the lane genuinely sheds"
    );

    let report = drive_ci_d2_surge(&controls, storm_ops, &surging, &quiet, "fr-par");

    assert!(
        report.is_ci_d2_green(),
        "CI-D2 must be GREEN: {}",
        report.summary()
    );
    assert!(
        report.surging_batch_shed_count > 0,
        "the CI storm MUST be absorbed by SHEDDING (429+Retry-After), not unbounded latency"
    );
    assert!(
        report.batch_shed_retry_after_secs > 0,
        "every batch/CI-lane shed carries a Retry-After (myelin ci honours it — no retry storm)"
    );
    assert_eq!(
        report.surging_interactive_shed_count, 0,
        "the protected INTERACTIVE lane HELD on the surging tenant (a PR-check never queues behind a matrix)"
    );
    assert!(
        report.surging_interactive_admitted,
        "the surging tenant's OWN interactive PR-check was admitted (held last)"
    );
    assert!(
        report.quiet_interactive_admitted,
        "the quiet co-tenant's interactive dispatch was admitted within budget (untouched)"
    );
    assert_eq!(
        report.cross_tenant_shed_count, 0,
        "cross-tenant impact is 0 — the storm is contained to the surging tenant's bounded run-queue"
    );
    assert_eq!(
        report.orphan_count, 0,
        "0 ORPHANS — every killed-runner lease re-queued within the lease TTL (the headline zero)"
    );
    assert!(
        report.requeued_count > 0,
        "the killed runner's jobs re-queued (claimable again) within the lease TTL (the reaper recovered them)"
    );

    // The MEASURED per-`fair_key` starvation signal stayed WITHIN the trigger → flat DRR holds; the
    // hierarchical scheduler (CI-P29) stays a NAMED FLOOR (measured-not-predicted, open question 07#1).
    assert!(
        report.fair_key_wait_p99_ticks <= report.starvation_trigger_ticks,
        "the per-fair_key wait p99 ({}t) stayed within the starvation trigger ({}t) — flat DRR holds",
        report.fair_key_wait_p99_ticks,
        report.starvation_trigger_ticks
    );
    assert!(
        !report.hierarchical_scheduler_owed,
        "the measured starvation signal did NOT fire → the hierarchical scheduler stays a named floor (CI-P29)"
    );
    assert!(
        !thresholds.ci_surge.hierarchical_scheduler_promotion_owed,
        "the FROZEN file records the hierarchical scheduler as a named floor (promotion not owed)"
    );

    println!(
        "[P-490 CI-D2 GREEN 2026-06-25] {} (storm_ops={storm_ops} derived from the P-S02 generator at \
         {multiplier}× CI-surge; tuned cap={} == CiDispatch shed budget)",
        report.summary(),
        controls.per_tenant_in_flight_cap()
    );
}

/// **MANDATORY: the cross-tenant-0 property is REAL — the quiet tenant's interactive dispatch is admitted
/// DURING the surge, never starved.** Saturate the surging tenant's batch lane completely, then prove the
/// quiet tenant's dispatch is STILL admitted within budget (the per-tenant bounded run-queue is the
/// blast-radius boundary).
#[test]
fn ci_d2_cross_tenant_isolation_quiet_tenant_admitted_during_the_storm() {
    let mut gate = CiSurgeGate::with_budget(SurfaceBudget {
        per_tenant_in_flight_cap: 8,
        human_lane_reservation: 0,
        retry_after_secs: 5,
    });
    let surging = TenantId("noisy".into());
    let quiet = TenantId("quiet".into());

    // Saturate the surging tenant's batch lane (a storm well over its graded ceiling → it sheds).
    for _ in 0..64 {
        let _ = gate.admit(&surging, RunClass::BatchCi);
    }
    assert!(
        gate.shed_count(RunClass::BatchCi) > 0,
        "the surging tenant's batch lane is saturated and sheds"
    );

    // The quiet co-tenant's interactive AND batch dispatches are STILL admitted (its in-flight is 0 —
    // the storm never bled into its per-tenant budget).
    assert!(
        gate.admit(&quiet, RunClass::Human).is_ok(),
        "the quiet tenant's interactive dispatch is admitted DURING the surge (per-tenant blast radius)"
    );
    assert!(
        gate.admit(&quiet, RunClass::BatchCi).is_ok(),
        "the quiet tenant's batch dispatch is admitted DURING the surge (the storm is contained)"
    );
    assert_eq!(
        gate.in_flight(&quiet),
        2,
        "the quiet tenant's in-flight is exactly its OWN two ops — 0 cross-tenant bleed"
    );

    println!(
        "[P-490 CI-D2 cross-tenant 2026-06-25] surging batch_shed={} quiet admitted (in_flight={}) → \
         the per-tenant bounded run-queue contains the storm",
        gate.shed_count(RunClass::BatchCi),
        gate.in_flight(&quiet)
    );
}

/// **The counter-case proves the green is EARNED (EI-01 §3).** An UNBOUNDED lane (a storm that never
/// exceeds the cap) shows 0 shed — the report is NOT green. The CI-D2 green is only reachable when the
/// bound actually fires.
#[test]
fn ci_d2_unbounded_lane_is_not_green() {
    let thresholds = Thresholds::load_canonical().expect("load thresholds.toml");
    let controls = CiSurgeControls::from_thresholds(&thresholds).expect("controls");
    // A single batch op — well under the cap, never sheds → the surge property is not exercised.
    let report = drive_ci_d2_surge(
        &controls,
        1,
        &TenantId("s".into()),
        &TenantId("q".into()),
        "fr-par",
    );
    assert_eq!(
        report.surging_batch_shed_count, 0,
        "a sub-cap storm never sheds"
    );
    assert!(
        !report.is_ci_d2_green(),
        "with 0 shed the surge property is not exercised → NOT green (the green must be earned)"
    );
}

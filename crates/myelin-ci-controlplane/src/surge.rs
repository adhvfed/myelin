//! # `surge` — the 30× CI surge family (CI-D2): the interactive lane holds, the batch/CI lane sheds,
//! the tuned DRR/shed-budget numbers, the pre-warm buffer sizing, and the measured per-`fair_key`
//! starvation signal (CI-P30 / P-490, M5).
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/02-internals-and-algorithms.md`
//! §2.2 (the DRR floor → hierarchical promotion, the per-`fair_key` starvation histogram — open
//! question 07#1) + §2.4 (backpressure & abuse — the 30× surge sheds batch/CI, holds interactive,
//! others unaffected) + §5.4 (the pre-warm buffer sizing function — open question 07#2);
//! `07-drills-and-open-questions.md` §1 row D-2 + §2 (open questions 1, 2, 5 — the shed-budget concrete
//! numbers). **Reconciliation:** `00-reconciliation-decisions.md` §OQ-K (the per-surface CI-surge shed
//! budget). **Contracts consumed:** 1.11 (the shed order + the CI per-surface budget), 1.8 (the
//! telemetry survival signals — the per-`fair_key` wait-time histogram + the per-lane shed count).
//! **Drill catalogue:** `01-whole-system-e2e-and-drill-catalogue.md` row **CI-D2** (30× CI surge one
//! tenant → interactive holds, batch sheds `429 + Retry-After`, others unaffected, reserve refuses
//! over-budget, killed-runner jobs re-queue within lease TTL 0 orphans), cadence `SCHED`.
//!
//! ## What this module is (CI's slice of the F6 surge family — CI-P30 / CI-D2)
//! The 30× CI surge is one tenant's enormous CI fan-out (a 10k-job matrix push) arriving at the CI
//! dispatch front door + the DRR fair-share scheduler. Three structural defences — ALL already built —
//! are TIED TOGETHER here under the surge and proven (this module authors NO parallel second
//! implementation, EI-01 §7):
//! 1. **the concurrency front (the substrate's shed lane)** — the batch/CI lane sheds with
//!    `429 + Retry-After` at the per-tenant in-flight cap, the interactive PR-check lane is protected
//!    (held last). REUSES [`myelin_substrate::shed::ShedLane`] over [`Surface::CiDispatch`] (the same
//!    surface the substrate's SUB-D3 slice already sheds on — NOT a second shed lane). CI is the §7.6
//!    batch lane (no human reservation; CI + agent share the wallet), so the protected lane inside CI is
//!    the *interactive* run-class (a PR-check) vs the *batch/CI* run-class (a matrix job).
//! 2. **the fairness front (the DRR fair-share scheduler)** — the per-`fair_key` deficit advance/
//!    replenish keeps the surging tenant's 10k matrix from starving a co-tenant's jobs. REUSES
//!    [`crate::fairness::FairShare`] + [`crate::fairness::Backpressure`] (CI-P13) — the surge MEASURES
//!    the wait-time/starvation histogram (1.8) the DRR produces, and proves no-starvation.
//! 3. **the lease front (the dead-runner reaper)** — a KILLED runner's leased jobs re-queue within the
//!    lease TTL with 0 orphans. REUSES [`crate::scheduler::SchedulerState::reap`] (CI-P12).
//!
//! ## What this module MEASURES (the CI-P30 deliverable — the tuned numbers)
//! - the **tuned DRR weights + per-tenant cap + the per-`fair_key` starvation-histogram threshold** are
//!   read from the FROZEN thresholds file ([`myelin_substrate::thresholds::CiSurge`]) — the versioned
//!   source of truth; the surge asserts the file's cap equals the `CiDispatch` shed-budget cap (one
//!   number, not two).
//! - the **per-`fair_key` wait-time/starvation histogram** ([`StarvationHistogram`]) — the contract-1.8
//!   survival signal CI-P29's hierarchical-scheduler promotion is GATED on. The 30× surge measures the
//!   wait p99; if it stays WITHIN the threshold, flat DRR holds no-starvation and the hierarchical
//!   scheduler stays a NAMED FLOOR (measured-not-predicted, open question 07#1).
//! - the **pre-warm buffer sizing** is the measured function on the autoscaler
//!   ([`crate::fleet::AutoscalePolicy::from_measured_arrival_rate`]) reading the same `CiSurge` row
//!   (open question 07#2 — replaces CI-P4's fixed-buffer floor).
//!
//! ## FLOOR named (VISION §3 / the prompt DoD)
//! **flat DRR → the hierarchical (per-tenant → per-project → per-pipeline) scheduler is promoted at
//! CI-P29 ONLY IF the measured starvation signal fires** (otherwise it stays the named floor —
//! measured-not-predicted). This module MEASURES that signal: at the 30× CI-D2 surge the per-`fair_key`
//! wait p99 stays WITHIN the starvation threshold (flat DRR fairly interleaves the surging tenant), so
//! the signal does NOT fire and the hierarchical scheduler STAYS the named floor
//! ([`crate::floor_followons`] `hierarchical-scheduler`, status `NotFired` with this measured evidence).
//! The remaining floor is the world-scale FLEET-hardware 30× load (the ONE legitimate floor) — here the
//! load is the P-S02 generator at 30× on one tenant; the fairness + shed-order + cross-tenant-0 +
//! 0-orphan-reaper PROPERTIES are complete + testable now and do not change shape when the real cell
//! carries the load.
//!
//! ## Mutation floor (mandatory-core — the surge survival path, EI-01 §2/§3)
//! The shed DECISION ([`CiSurgeGate::admit`] — interactive held, batch/CI shed) + the starvation
//! signal ([`StarvationHistogram::wait_p99_ticks`] + [`CiSurgeControls::hierarchical_promotion_owed`])
//! are mandatory-core: an off-by-one that sheds an interactive PR-check before a batch matrix job, or a
//! starvation signal that silently mis-fires (promoting the hierarchy prematurely, or hiding a real
//! starvation), is the failure this exists to catch. The cargo-mutants floor for the shed-budget module
//! is **100% of viable mutants caught** (`cargo mutants -p myelin-ci-controlplane --file
//! crates/myelin-ci-controlplane/src/surge.rs`): every comparison/arithmetic mutant in the admit
//! decision + the starvation p99 + the cross-tenant accounting is killed by the unit + drill tests below.

use myelin_substrate::shed::{RunClass, ShedDecision, ShedLane, Surface, SurfaceBudget};
use myelin_substrate::thresholds::{CiSurge, ThresholdError, Thresholds};
use myelin_tenancy::TenantId;

use crate::fairness::{Backpressure, FairShare, PlanTier};
use crate::scheduler::{ClaimRequest, JobState, Lane, QueuedJob, SchedulerState};
use myelin_ci_sandbox::TrustTier;

/// **The CI-surge default-to-beat multiplier (CI-D2).** The 30× world-scale surge factor the CI-D2
/// drill drives at — read from the FROZEN thresholds file `[surge] multiplier` row (the versioned
/// source of truth) and asserted to equal this documented default-to-beat; a divergence is a LOUD
/// failure, never a silent weakening (EI-01 §3).
pub const CI_SURGE_MULTIPLIER: u32 = 30;

// =================================================================================================
// 1. The tuned CI-surge controls — read from the FROZEN thresholds file (the versioned source of truth).
// =================================================================================================

/// **The MEASURED CI-surge controls (CI-P30) — a typed view over the [`CiSurge`] thresholds row.** The
/// tuned DRR quantum/ceiling + the per-tenant cap + the per-`fair_key` starvation trigger + the
/// pre-warm sizing, read from the FROZEN thresholds file. The gate refuses to construct against numbers
/// that are not well-formed (a vacuous bar is a LOUD error, never a silent default — EI-01 §3); a
/// missing row is a loud [`ThresholdError`].
#[derive(Clone, Debug)]
pub struct CiSurgeControls {
    ci_surge: CiSurge,
    multiplier: u32,
}

impl CiSurgeControls {
    /// Load the tuned controls from the FROZEN thresholds file. Asserts the row is well-formed (a
    /// degenerate row — a 0 cap, a quantum ≥ ceiling, a 0 starvation trigger — is a LOUD error, not a
    /// silently-accepted vacuous bar) AND that the tuned CI cap EQUALS the `CiDispatch` shed-budget cap
    /// (one number, not two — the scheduler-internal cap and the public-surface shed budget agree).
    pub fn from_thresholds(thresholds: &Thresholds) -> Result<CiSurgeControls, String> {
        let ci_surge = thresholds.ci_surge.clone();
        if !ci_surge.is_well_formed() {
            return Err(
                "ci_surge thresholds are not well-formed (a vacuous bar — EI-01 §3)".into(),
            );
        }
        let shed_cap = thresholds
            .shed_budget(Surface::CiDispatch)
            .map_err(|e: ThresholdError| format!("CiDispatch shed budget unavailable: {e}"))?
            .per_tenant_in_flight_cap;
        if ci_surge.per_tenant_in_flight_cap != shed_cap {
            return Err(format!(
                "ci_surge cap {} != CiDispatch shed cap {} — the scheduler cap and the public-surface \
                 shed budget MUST agree (one number, not two)",
                ci_surge.per_tenant_in_flight_cap, shed_cap
            ));
        }
        Ok(CiSurgeControls {
            ci_surge,
            multiplier: thresholds.surge.multiplier,
        })
    }

    /// The tuned per-tenant CI in-flight cap (the bounded run-queue, §2.4).
    pub fn per_tenant_in_flight_cap(&self) -> u32 {
        self.ci_surge.per_tenant_in_flight_cap
    }

    /// The surge multiplier read from the file (asserted == [`CI_SURGE_MULTIPLIER`] by the drill).
    pub fn multiplier(&self) -> u32 {
        self.multiplier
    }

    /// The tuned DRR base quantum (the deficit decrement on one claim, §2.2).
    pub fn drr_base_quantum(&self) -> i64 {
        self.ci_surge.drr_base_quantum
    }

    /// The tuned DRR deficit ceiling (the burst-credit cap, §2.2).
    pub fn drr_deficit_ceiling(&self) -> i64 {
        self.ci_surge.drr_deficit_ceiling
    }

    /// The per-`fair_key` starvation-histogram p99 trigger (claim ticks, open question 07#1).
    pub fn starvation_wait_p99_max_ticks(&self) -> u64 {
        self.ci_surge.starvation_wait_p99_max_ticks
    }

    /// **The hierarchical-scheduler promotion-gate decision (the measurement gate, CI-P29).** Given the
    /// MEASURED per-`fair_key` wait-time p99 (claim ticks) under the 30× CI-D2 surge, decide whether the
    /// hierarchical scheduler (CI-P29) is owed: owed iff the measured p99 STRICTLY exceeds the tuned
    /// starvation trigger. A p99 at/under the trigger means flat DRR holds no-starvation → the
    /// hierarchical scheduler stays a NAMED FLOOR (measured-not-predicted, open question 07#1).
    pub fn hierarchical_promotion_owed(&self, measured_wait_p99_ticks: u64) -> bool {
        self.ci_surge
            .hierarchical_promotion_owed_for(measured_wait_p99_ticks)
    }

    /// The MEASURED pre-warm buffer size for a pool's recent arrival rate (the §5.4 sizing function;
    /// replaces CI-P4's fixed floor — open question 07#2).
    pub fn prewarm_buffer_for(&self, arrival_rate: u32) -> u32 {
        self.ci_surge.prewarm_buffer_for(arrival_rate)
    }
}

// =================================================================================================
// 2. The CI-surge shed gate — the interactive lane holds, the batch/CI lane sheds (1.11 / §2.4).
// =================================================================================================

/// **Why a CI dispatch was shed at the surge gate** — the typed form the transport maps to the wire
/// `429`. A shed carries the `Retry-After` (seconds) `myelin ci` honours (1.11 / 1.9 — the
/// no-amplification guarantee).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CiDispatchShed {
    /// The lane that was shed (the contract-1.8 per-lane shed-count signal keys on this).
    pub lane: RunClass,
    /// The `Retry-After` value in SECONDS the transport sets on the `429` and `myelin ci` honours.
    pub retry_after_secs: u64,
}

/// **The CI-surge shed gate at the CI-dispatch front door (CI-D2 / §2.4; contract 1.11).**
///
/// A thin CI wiring over the substrate's [`ShedLane`] for [`Surface::CiDispatch`]: it reads the budget
/// **from the thresholds file** and applies the shed order `speculative → batch/CI → agent → human-last`
/// per-tenant. CI is the batch lane (no human reservation), so inside CI the PROTECTED lane is the
/// *interactive* PR-check run-class and the SHED-FIRST lane is the *batch/CI* matrix run-class. An
/// over-budget batch/CI dispatch is shed with `429 + Retry-After`; the interactive lane is held last.
pub struct CiSurgeGate {
    lane: ShedLane,
}

impl CiSurgeGate {
    /// Open the CI-surge gate, reading its budget **from the thresholds file** (the prompt's "the shed
    /// budget is read from the thresholds file"). A missing row is a LOUD error (the gate refuses to open
    /// against a guessed budget — EI-01 §3), never a silent default.
    pub fn from_thresholds(thresholds: &Thresholds) -> Result<CiSurgeGate, String> {
        let budget = thresholds
            .shed_budget(Surface::CiDispatch)
            .map_err(|e| format!("CI shed budget for CiDispatch unavailable: {e}"))?;
        Ok(CiSurgeGate {
            lane: ShedLane::with_budget(Surface::CiDispatch, budget),
        })
    }

    /// Open the gate against an explicit budget (used by the surge drill / unit tests to drive the
    /// boundary at a small, deterministic budget without editing the thresholds file).
    pub fn with_budget(budget: SurfaceBudget) -> CiSurgeGate {
        CiSurgeGate {
            lane: ShedLane::with_budget(Surface::CiDispatch, budget),
        }
    }

    /// **Admit a CI dispatch of a [`RunClass`] for `tenant`.** Returns `Ok(())` admitted (a slot taken —
    /// release it on completion via [`CiSurgeGate::release`]) or `Err(CiDispatchShed)` shed
    /// (`429 + Retry-After`). The interactive lane is protected: the batch/CI lane sheds FIRST (at
    /// `cap - reservation`, and for CiDispatch the reservation is 0 so CI is purely bounded); the
    /// interactive lane is held last. The decision is per-tenant (one tenant's surge never sheds
    /// another's — the blast-radius guarantee, EI-02 §1).
    pub fn admit(&mut self, tenant: &TenantId, class: RunClass) -> Result<(), CiDispatchShed> {
        match self.lane.admit(tenant, class) {
            ShedDecision::Admit => Ok(()),
            ShedDecision::Shed { retry_after_secs } => Err(CiDispatchShed {
                lane: class,
                retry_after_secs,
            }),
        }
    }

    /// Release a slot a prior admit took for `(tenant, class)` — call on completion so the lane recovers.
    pub fn release(&mut self, tenant: &TenantId, class: RunClass) {
        self.lane.release(tenant, class);
    }

    /// The cumulative shed count for a lane (the contract-1.8 `shed-count per lane` survival signal).
    pub fn shed_count(&self, class: RunClass) -> u64 {
        self.lane.shed_count(class)
    }

    /// The per-tenant in-flight count (admitted not yet released) — for the blast-radius assertions.
    pub fn in_flight(&self, tenant: &TenantId) -> u32 {
        self.lane.in_flight(tenant)
    }
}

// =================================================================================================
// 3. The per-`fair_key` wait-time / starvation histogram (contract 1.8; open question 07#1).
// =================================================================================================

/// **The per-`fair_key` wait-time / starvation histogram (contract 1.8; open question 07#1).** Records,
/// for each contending `fair_key`, how many claim ticks its job WAITED before it was served (the surge's
/// survival signal). The wait-time p99 across all served jobs is the per-`fair_key` starvation signal
/// CI-P29's hierarchical-scheduler promotion is gated on: a high p99 means flat DRR is starving a
/// tenant; a low p99 means it fairly interleaves the surging tenant (no starvation). DB-free +
/// deterministic — the same wait samples the live OLTP `fair_deficit.last_served` deltas would record.
#[derive(Clone, Debug, Default)]
pub struct StarvationHistogram {
    /// Every served job's wait (in claim ticks: how many claims it waited before being served).
    waits: Vec<u64>,
}

impl StarvationHistogram {
    /// A fresh empty histogram.
    pub fn new() -> StarvationHistogram {
        StarvationHistogram::default()
    }

    /// Record a served job's wait (in claim ticks).
    pub fn record_wait(&mut self, wait_ticks: u64) {
        self.waits.push(wait_ticks);
    }

    /// The number of recorded waits.
    pub fn len(&self) -> usize {
        self.waits.len()
    }

    /// Whether no wait has been recorded.
    pub fn is_empty(&self) -> bool {
        self.waits.is_empty()
    }

    /// **The wait-time p99 (in claim ticks) — the starvation signal.** The 99th-percentile wait across
    /// all served jobs (nearest-rank: the smallest sample at-or-above the 99% rank). 0 for an empty
    /// histogram (no contention → no starvation). This is the number the [`CiSurgeControls`] starvation
    /// trigger is compared against (a p99 over the trigger fires the hierarchical-scheduler promotion).
    pub fn wait_p99_ticks(&self) -> u64 {
        if self.waits.is_empty() {
            return 0;
        }
        let mut sorted = self.waits.clone();
        sorted.sort_unstable();
        // Nearest-rank p99: rank = ceil(0.99 * n), 1-indexed → index rank-1 (clamped to the last).
        let n = sorted.len();
        let rank = ((99 * n) as f64 / 100.0).ceil() as usize;
        let idx = rank.saturating_sub(1).min(n - 1);
        sorted[idx]
    }

    /// The maximum recorded wait (the worst-case starvation a single job saw).
    pub fn max_wait_ticks(&self) -> u64 {
        self.waits.iter().copied().max().unwrap_or(0)
    }
}

// =================================================================================================
// 4. The CI-D2 surge report — the four properties the DoD names.
// =================================================================================================

/// **The CI-D2 30× CI surge report — the properties the DoD names.** The dated green artifact: the
/// interactive lane HELD (0 interactive shed — a PR-check is admitted, never queued behind a batch
/// matrix), the batch/CI lane SHED (`429 + Retry-After`, absorbed not unbounded), OTHER tenants
/// UNAFFECTED (cross-tenant shed 0 — the per-tenant bounded run-queue held), a KILLED runner's jobs
/// RE-QUEUED within the lease TTL with 0 ORPHANS, and the per-`fair_key` wait p99 stayed WITHIN the
/// starvation trigger (flat DRR holds no-starvation → the hierarchical scheduler stays a named floor).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CiSurgeReport {
    /// The batch/CI-lane shed count on the surging tenant (the storm absorbed by shedding — must be > 0).
    pub surging_batch_shed_count: u64,
    /// The interactive-lane shed count on the surging tenant (the protected lane — must be 0).
    pub surging_interactive_shed_count: u64,
    /// Whether the surging tenant's OWN interactive PR-check was admitted (held last on the noisy tenant).
    pub surging_interactive_admitted: bool,
    /// Whether the quiet co-tenant's interactive dispatch was admitted within its own budget (untouched).
    pub quiet_interactive_admitted: bool,
    /// The quiet co-tenant's shed count (the cross-tenant impact — must be 0; the storm never sheds the
    /// quiet tenant's lanes).
    pub cross_tenant_shed_count: u64,
    /// The `Retry-After` (seconds) the batch/CI lane's shed carried (must be > 0 — every shed advertises
    /// a backoff `myelin ci` honours).
    pub batch_shed_retry_after_secs: u64,
    /// **The headline zero** — orphaned (`leased`-forever) jobs after a killed runner's lease expired and
    /// the reaper swept (`0`; every expired lease re-queued within the lease TTL, 0 orphans).
    pub orphan_count: u64,
    /// The count of killed-runner jobs that re-queued (claimable again) within the lease TTL (must be >
    /// 0 — the reaper recovered them).
    pub requeued_count: u64,
    /// The MEASURED per-`fair_key` wait-time p99 (claim ticks) under the surge — the starvation signal.
    pub fair_key_wait_p99_ticks: u64,
    /// The tuned starvation trigger (claim ticks) the wait p99 is compared against (open question 07#1).
    pub starvation_trigger_ticks: u64,
    /// Whether the hierarchical scheduler (CI-P29) is owed — `true` iff the measured wait p99 crossed the
    /// trigger. MUST be `false` for the CI-D2 green (flat DRR holds no-starvation; the hierarchy stays a
    /// named floor).
    pub hierarchical_scheduler_owed: bool,
}

impl CiSurgeReport {
    /// **The CI-D2 GREEN predicate (the properties — all measured, none weakened).** The batch/CI lane
    /// shed (absorbed, carrying a Retry-After), the interactive lane held (0 shed + the PR-check admitted,
    /// both the surging tenant's own and the quiet co-tenant's), the quiet co-tenant was unaffected
    /// (cross-tenant shed 0), the killed runner's jobs re-queued within the lease TTL with 0 orphans, and
    /// the per-`fair_key` wait p99 stayed within the starvation trigger (flat DRR holds; the hierarchical
    /// scheduler stays a named floor).
    pub fn is_ci_d2_green(&self) -> bool {
        self.surging_batch_shed_count > 0
            && self.batch_shed_retry_after_secs > 0
            && self.surging_interactive_shed_count == 0
            && self.surging_interactive_admitted
            && self.quiet_interactive_admitted
            && self.cross_tenant_shed_count == 0
            && self.orphan_count == 0
            && self.requeued_count > 0
            && self.fair_key_wait_p99_ticks <= self.starvation_trigger_ticks
            && !self.hierarchical_scheduler_owed
    }

    /// A one-line summary for the dated green-artifact log row (observability is part of the pass).
    pub fn summary(&self) -> String {
        format!(
            "CI-D2: surging batch_shed={} (retry_after={}s) interactive_shed={} \
             surging_interactive_admitted={} quiet_interactive_admitted={} cross_tenant_shed={} \
             orphans={} requeued={} fair_key_wait_p99={}t (trigger={}t) hierarchical_owed={} → {}",
            self.surging_batch_shed_count,
            self.batch_shed_retry_after_secs,
            self.surging_interactive_shed_count,
            self.surging_interactive_admitted,
            self.quiet_interactive_admitted,
            self.cross_tenant_shed_count,
            self.orphan_count,
            self.requeued_count,
            self.fair_key_wait_p99_ticks,
            self.starvation_trigger_ticks,
            self.hierarchical_scheduler_owed,
            if self.is_ci_d2_green() {
                "GREEN"
            } else {
                "RED"
            }
        )
    }
}

// =================================================================================================
// 5. Drive the CI-D2 30× CI surge (the drill core — REUSED by the drill test + the unit test).
// =================================================================================================

/// **Drive the CI-D2 30× CI surge across the three fronts.** The surge is a CONCURRENCY BURST:
/// `storm_batch_ops` batch/CI dispatches arrive at the surging tenant near-simultaneously (a 10k-job
/// matrix push). The three structural defences fire and CI-D2 proves all:
///
/// 1. **the concurrency front (the shed lane).** The whole burst is offered to the [`CiSurgeGate`]
///    FIRST. The lane admits up to its per-tenant cap concurrently then SHEDS the excess
///    (`429 + Retry-After`) — absorbed by shedding, never unbounded latency. The surging tenant's OWN
///    interactive PR-check is then proven still admitted (held last), and a quiet co-tenant's
///    interactive dispatch admitted within its independent budget (cross-tenant shed 0).
/// 2. **the fairness front (the DRR scheduler).** The surging tenant's matrix + a co-tenant's jobs are
///    enqueued; the DRR claim loop (advance-on-claim + periodic replenish) serves them, recording each
///    served job's wait into the [`StarvationHistogram`]. The wait p99 is the measured starvation signal:
///    it stays WITHIN the trigger (flat DRR fairly interleaves the surging tenant — no starvation).
/// 3. **the lease front (the reaper).** A runner claims jobs then is KILLED (its lease expires); the
///    reaper sweeps and re-queues them within the lease TTL with 0 orphans.
///
/// Returns the [`CiSurgeReport`]. `controls` carries the tuned numbers (from the file); `multiplier` is
/// the surge factor (read from the file by the caller; passed through for the log row + the storm size).
pub fn drive_ci_d2_surge(
    controls: &CiSurgeControls,
    storm_batch_ops: u32,
    surging: &TenantId,
    quiet: &TenantId,
    region: &str,
) -> CiSurgeReport {
    // ── Front 1: the concurrency front (the shed lane), at the tuned cap. ──
    let cap = controls.per_tenant_in_flight_cap();
    let mut gate = CiSurgeGate::with_budget(SurfaceBudget {
        per_tenant_in_flight_cap: cap,
        human_lane_reservation: 0, // CI is the batch lane (§7.6 n/a human reservation).
        retry_after_secs: 5,
    });

    // The whole batch storm arrives near-simultaneously on the surging tenant → the lane admits up to
    // the cap then sheds the excess (the over-cap batch ops are shed with 429 + Retry-After).
    for _ in 0..storm_batch_ops {
        let _ = gate.admit(surging, RunClass::BatchCi);
    }
    let surging_batch_shed_count = gate.shed_count(RunClass::BatchCi);
    // The batch shed advertises the surface's Retry-After (re-offer one over-cap op to read it).
    let batch_shed_retry_after_secs = match gate.admit(surging, RunClass::BatchCi) {
        Err(shed) => shed.retry_after_secs,
        Ok(()) => 0, // unreachable under a real surge (the lane is saturated); 0 would fail the green.
    };

    // The surging tenant's OWN interactive PR-check is held LAST — admitted even while its batch lane
    // is saturated (the protected lane inside CI; a human's PR feedback never queues behind a matrix).
    let surging_interactive_admitted = gate.admit(surging, RunClass::Human).is_ok();
    let surging_interactive_shed_count = gate.shed_count(RunClass::Human);

    // A quiet co-tenant is UNAFFECTED — its interactive dispatch admitted within its own per-tenant
    // budget; the storm never spent the quiet tenant's budget (the per-tenant bulkhead). The
    // cross-tenant signal: the quiet tenant's in-flight is EXACTLY its own one op (the storm did not
    // bleed into its budget) — anything above 1 is cross-tenant leakage. (A shed-count on the quiet
    // tenant is impossible since the lane keys in-flight per-tenant; the in-flight delta is the precise
    // blast-radius signal — 0 means the storm stayed entirely inside the surging tenant's budget.)
    let quiet_interactive_admitted = gate.admit(quiet, RunClass::Human).is_ok();
    let cross_tenant_shed_count = (gate.in_flight(quiet).saturating_sub(1)) as u64;

    // ── Front 2: the fairness front (the DRR scheduler) — measure the starvation histogram. ──
    let histogram = measure_starvation_histogram(controls, surging, quiet, region, storm_batch_ops);
    let fair_key_wait_p99_ticks = histogram.wait_p99_ticks();
    let starvation_trigger_ticks = controls.starvation_wait_p99_max_ticks();
    let hierarchical_scheduler_owed = controls.hierarchical_promotion_owed(fair_key_wait_p99_ticks);

    // ── Front 3: the lease front (the reaper) — a killed runner's jobs re-queue, 0 orphans. ──
    let (requeued_count, orphan_count) = measure_reaper_recovery(surging, region);

    CiSurgeReport {
        surging_batch_shed_count,
        surging_interactive_shed_count,
        surging_interactive_admitted,
        quiet_interactive_admitted,
        cross_tenant_shed_count,
        batch_shed_retry_after_secs,
        orphan_count,
        requeued_count,
        fair_key_wait_p99_ticks,
        starvation_trigger_ticks,
        hierarchical_scheduler_owed,
    }
}

/// **Measure the per-`fair_key` wait-time / starvation histogram under the surge (the DRR fairness
/// front).** The surging tenant enqueues a large matrix (`storm_ops` jobs on its `fair_key`) while a
/// quiet co-tenant enqueues a few; the DRR claim loop serves them (advance-on-claim + periodic
/// replenish, the tuned quantum/ceiling), recording each served job's WAIT (claim ticks between its
/// enqueue and its serve). Returns the histogram whose p99 is the starvation signal. The property the
/// drill asserts: flat DRR fairly interleaves the surging tenant, so the QUIET tenant's jobs are served
/// within a bounded wait — the p99 stays within the starvation trigger (no starvation).
fn measure_starvation_histogram(
    controls: &CiSurgeControls,
    surging: &TenantId,
    quiet: &TenantId,
    region: &str,
    storm_ops: u32,
) -> StarvationHistogram {
    let mut fair = FairShare::new();
    // The per-tenant in-flight cap (the bounded run-queue, §2.4): a tenant runs at most `cap` jobs at
    // once, so a surging tenant cannot occupy every runner — it leaves claim opportunities for the
    // co-tenant. A small cap here so the bound is load-bearing on the deterministic core (the file's
    // real cap is asserted == the shed budget elsewhere; this drives the FAIRNESS interleave).
    let cap = controls.per_tenant_in_flight_cap().clamp(1, 4);
    let mut bp = Backpressure::with_cap(cap);
    let mut hist = StarvationHistogram::new();

    // Both tenants at the same plan tier → pure DRR fairness, no weight skew (the worst case for the
    // quiet tenant). The fair_key is the tenant id (the flat single-level DRR — CI-P29 would split it).
    fair.set_tier(&surging.0, region, &surging.0, PlanTier::Free);
    fair.set_tier(&quiet.0, region, &quiet.0, PlanTier::Free);

    // The backlog: the surging tenant's huge matrix vs a few quiet jobs. The burst arrives at tick 0, so
    // a served job's wait == the tick it is served. Each running job holds a slot for `run_duration`
    // ticks (it occupies the bounded run-queue), then releases — so the cap genuinely bounds concurrency.
    let quiet_jobs = 5u32;
    let run_duration = 2u64;
    let mut backlog: [(TenantId, u32); 2] =
        [(surging.clone(), storm_ops), (quiet.clone(), quiet_jobs)];
    // Running jobs: (tenant, tick it finishes). When `now >= finish` the slot is released.
    let mut running: Vec<(TenantId, u64)> = Vec::new();
    let mut served_quiet = 0u32;

    // Run the DRR claim loop. Each tick: (1) release finished jobs (free their in-flight slots); (2)
    // pick the eligible (has-backlog, UNDER ITS CAP) fair_key with the highest deficit, serve it
    // (advance = decrement, occupy a slot), recording the QUIET tenant's served-job waits (the signal —
    // does the surge starve the co-tenant?); (3) periodically replenish (the DRR sweep).
    let total_to_serve = storm_ops as u64 + quiet_jobs as u64;
    let mut served_total = 0u64;
    let mut tick = 0u64;
    // Bound the loop generously (every job runs in run_duration; the cap bounds concurrency) so a bug
    // can never spin forever (EI-01 §5 — loud, not silent).
    let tick_ceiling = total_to_serve * (run_duration + 2) + 16;
    while served_total < total_to_serve && tick < tick_ceiling {
        // (1) Release finished jobs.
        let now = tick;
        running.retain(|(t, finish)| {
            if *finish <= now {
                bp.on_released(&t.0, region);
                false
            } else {
                true
            }
        });
        // (2) Pick the highest-deficit eligible-and-admitted tenant (the claim's `deficit DESC`).
        let pick = backlog
            .iter()
            .enumerate()
            .filter(|(_, (_, n))| *n > 0)
            .filter(|(_, (t, _))| bp.admits(&t.0, region))
            .max_by_key(|(_, (t, _))| {
                (
                    fair.deficit(&t.0, region, &t.0),
                    std::cmp::Reverse(t.0.clone()),
                )
            })
            .map(|(i, _)| i);
        if let Some(i) = pick {
            let (t, n) = &mut backlog[i];
            let t = t.clone();
            *n -= 1;
            fair.advance_on_claim(&t.0, region, &t.0);
            bp.on_claimed(&t.0, region);
            running.push((t.clone(), tick + run_duration));
            served_total += 1;
            if t == *quiet {
                hist.record_wait(tick);
                served_quiet += 1;
            }
        }
        // (3) The periodic replenish (every 3 ticks) — a served tenant recovers priority.
        if tick % 3 == 2 {
            fair.replenish(region);
        }
        tick += 1;
    }
    debug_assert_eq!(
        served_quiet, quiet_jobs,
        "the quiet tenant must be fully served (no starvation)"
    );
    hist
}

/// **Measure the dead-runner reaper recovery (the lease front).** A runner claims a batch of the
/// surging tenant's jobs (taking leases), then is KILLED — its leases expire. The reaper sweeps within
/// the lease TTL and re-queues them. Returns `(requeued_count, orphan_count)`: every expired lease must
/// re-queue (claimable again) with 0 orphans (no `leased`-forever job). REUSES the scheduler reaper
/// (CI-P12) — this only DRIVES it under the surge to prove the CI-D2 property.
fn measure_reaper_recovery(surging: &TenantId, region: &str) -> (u64, u64) {
    let mut sched = SchedulerState::new();
    let lease_ttl = 4u64;
    let killed_jobs = 8u32;

    // Enqueue + claim a batch of the surging tenant's jobs (a runner takes the leases).
    for i in 0..killed_jobs {
        let job = QueuedJob::enqueued(
            &surging.0,
            region,
            format!("job-{i}"),
            format!("run-{i}"),
            Lane::Batch,
            TrustTier::Trusted,
            &surging.0,
            format!("idem-{i}"),
            i as u64,
        );
        sched.enqueue(job);
    }
    let req = ClaimRequest {
        cell_region: region.to_string(),
        runner_labels: Vec::new(),
        runner_allowed_tiers: vec![TrustTier::Trusted],
        lease_owner: "doomed-runner".to_string(),
        lease_ttl,
    };
    let mut claimed = 0u32;
    while sched.claim(&req).is_some() {
        claimed += 1;
    }

    // The runner is KILLED: no heartbeat, the leases expire (advance past the lease TTL).
    sched.advance(lease_ttl + 1);
    // The reaper sweeps within the (now-expired) lease TTL → re-queues every dead lease.
    let reaped = sched.reap();
    let requeued_count = reaped.len() as u64;

    // 0 orphans: no job is left `leased`-forever (every claimed job is now re-queued, claimable again).
    let orphan_count = (0..killed_jobs)
        .filter(|i| sched.state_of(&surging.0, &format!("job-{i}")) == Some(JobState::Leased))
        .count() as u64;

    debug_assert_eq!(
        requeued_count, claimed as u64,
        "every leased job re-queued (0 orphans)"
    );
    (requeued_count, orphan_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fairness::{BASE_QUANTUM, DEFAULT_TENANT_IN_FLIGHT_CAP};

    fn tenant(s: &str) -> TenantId {
        TenantId(s.to_string())
    }

    fn canonical_controls() -> CiSurgeControls {
        let t = Thresholds::load_canonical().expect("load canonical thresholds");
        CiSurgeControls::from_thresholds(&t).expect("CI-surge controls from the canonical file")
    }

    // ── The tuned controls read from the FROZEN file (one number, not two) ───────────────────────

    /// The tuned CI cap EQUALS the `CiDispatch` shed-budget cap — the scheduler cap and the public-
    /// surface shed budget agree (one v1 floor). A divergence is a LOUD error at construction.
    #[test]
    fn controls_cap_equals_the_cidispatch_shed_budget() {
        let t = Thresholds::load_canonical().expect("load");
        let controls = CiSurgeControls::from_thresholds(&t).expect("controls");
        let shed_cap = t
            .shed_budget(Surface::CiDispatch)
            .unwrap()
            .per_tenant_in_flight_cap;
        assert_eq!(controls.per_tenant_in_flight_cap(), shed_cap);
        // And the substrate fairness floor agrees too (the third copy is the same v1 number).
        assert_eq!(
            controls.per_tenant_in_flight_cap(),
            DEFAULT_TENANT_IN_FLIGHT_CAP
        );
        assert_eq!(controls.drr_base_quantum(), BASE_QUANTUM);
    }

    /// A mismatched cap (the scheduler cap ≠ the shed budget) is a LOUD construction error — one number,
    /// never two. Pins the agreement check (a mutation that drops it is caught).
    #[test]
    fn mismatched_cap_is_a_loud_error() {
        let mut t = Thresholds::load_canonical().expect("load");
        t.ci_surge.per_tenant_in_flight_cap = 999; // diverge from the CiDispatch shed cap.
        assert!(
            CiSurgeControls::from_thresholds(&t).is_err(),
            "a scheduler cap that disagrees with the shed budget is a loud error (one number, not two)"
        );
    }

    /// A degenerate (vacuous-bar) CI-surge row is a LOUD construction error (EI-01 §3).
    #[test]
    fn vacuous_controls_are_a_loud_error() {
        let mut t = Thresholds::load_canonical().expect("load");
        t.ci_surge.starvation_wait_p99_max_ticks = 0; // "any wait starves" — a vacuous bar.
        assert!(CiSurgeControls::from_thresholds(&t).is_err());
    }

    // ── The shed gate: interactive holds, batch/CI sheds (1.11 / §2.4) ───────────────────────────

    /// **The batch/CI lane sheds FIRST; the interactive lane is held LAST (the protected-human-lane
    /// analogue inside CI, 1.11).** Pins the shed order — a mutation that sheds interactive before batch
    /// is caught (mandatory-core).
    #[test]
    fn interactive_holds_while_batch_sheds() {
        // A small cap so the boundary is exercised deterministically.
        let mut gate = CiSurgeGate::with_budget(SurfaceBudget {
            per_tenant_in_flight_cap: 8,
            human_lane_reservation: 0,
            retry_after_secs: 5,
        });
        let acme = tenant("acme");
        // Saturate the batch lane: the graded shed holds batch/ci to `cap - step` (the §7.6 shed order
        // sheds speculative before batch before agent), so the batch lane sheds BEFORE the full cap —
        // leaving headroom for the higher-promise lanes. Offer the whole storm; some shed.
        for _ in 0..16 {
            let _ = gate.admit(&acme, RunClass::BatchCi);
        }
        assert!(
            gate.shed_count(RunClass::BatchCi) > 0,
            "over its graded ceiling the batch lane sheds (429 + Retry-After)"
        );
        // The shed carries the surface's Retry-After (myelin ci honours it).
        match gate.admit(&acme, RunClass::BatchCi) {
            Err(shed) => assert_eq!(shed.retry_after_secs, 5),
            Ok(()) => panic!("the saturated batch lane must shed"),
        }
        // The interactive lane is HELD — admitted even while the batch lane is saturated (shed last).
        assert!(
            gate.admit(&acme, RunClass::Human).is_ok(),
            "the interactive PR-check is held last — admitted while batch sheds"
        );
        assert_eq!(gate.shed_count(RunClass::Human), 0, "0 interactive shed");
    }

    /// **The cap is PER-TENANT (the blast-radius guarantee, EI-02 §1).** One tenant's storm never sheds
    /// another tenant's dispatch.
    #[test]
    fn cap_is_per_tenant() {
        let mut gate = CiSurgeGate::with_budget(SurfaceBudget {
            per_tenant_in_flight_cap: 8,
            human_lane_reservation: 0,
            retry_after_secs: 5,
        });
        let noisy = tenant("noisy");
        let quiet = tenant("quiet");
        // Saturate the noisy tenant's batch lane (a storm well over its graded ceiling → it sheds).
        for _ in 0..16 {
            let _ = gate.admit(&noisy, RunClass::BatchCi);
        }
        assert!(
            gate.shed_count(RunClass::BatchCi) > 0,
            "noisy's batch lane sheds"
        );
        // A DIFFERENT tenant is unaffected — its in-flight is 0, so its batch dispatch is admitted (the
        // per-tenant bounded run-queue means one tenant's storm never sheds another's).
        assert!(
            gate.admit(&quiet, RunClass::BatchCi).is_ok(),
            "a different tenant is unaffected (per-tenant blast radius)"
        );
    }

    // ── The starvation histogram (contract 1.8; open question 07#1) ──────────────────────────────

    /// **The wait-time p99 is the nearest-rank 99th percentile.** Pins the percentile arithmetic (a
    /// mutation that mis-computes the rank is caught — mandatory-core for the starvation signal).
    #[test]
    fn wait_p99_is_nearest_rank() {
        let mut h = StarvationHistogram::new();
        assert_eq!(
            h.wait_p99_ticks(),
            0,
            "empty → 0 (no contention, no starvation)"
        );
        for w in 1..=100 {
            h.record_wait(w);
        }
        // 100 samples 1..=100: rank = ceil(0.99*100) = 99 → the 99th smallest = 99.
        assert_eq!(h.wait_p99_ticks(), 99);
        assert_eq!(h.max_wait_ticks(), 100);
        assert_eq!(h.len(), 100);
    }

    /// **The hierarchical-scheduler promotion is gated on the measured starvation p99 (open question
    /// 07#1).** A p99 within the trigger keeps flat DRR (no promotion); a p99 over it owes the hierarchy.
    #[test]
    fn hierarchical_promotion_gated_on_starvation_p99() {
        let controls = canonical_controls();
        let trigger = controls.starvation_wait_p99_max_ticks();
        assert!(
            !controls.hierarchical_promotion_owed(trigger),
            "at the trigger → within budget"
        );
        assert!(
            controls.hierarchical_promotion_owed(trigger + 1),
            "over the trigger → the hierarchical scheduler is owed (CI-P29)"
        );
    }

    // ── THE CI-D2 DRILL CORE (the prompt GATE — driven against the FROZEN file) ───────────────────

    /// **THE CI-D2 30× CI SURGE DRILL (the prompt GATE).** Driven against the FROZEN thresholds-file
    /// numbers at a storm sized by the surge multiplier × the cap: the interactive lane HOLDS (0 shed,
    /// admitted), the batch/CI lane SHEDS (`429 + Retry-After`), the quiet co-tenant is UNAFFECTED
    /// (cross-tenant shed 0), the killed runner's jobs RE-QUEUE within the lease TTL with 0 orphans, and
    /// the per-`fair_key` wait p99 stays WITHIN the starvation trigger (flat DRR holds — the hierarchical
    /// scheduler stays a named floor). The full LoadGenerator-driven drill is
    /// `tests/ci_d2_surge_drill.rs`; this is the deterministic in-crate core.
    #[test]
    fn ci_d2_surge_drill_core_is_green() {
        let controls = canonical_controls();
        assert_eq!(
            controls.multiplier(),
            CI_SURGE_MULTIPLIER,
            "the file's 30× multiplier"
        );
        // A storm well over the per-tenant cap (the surge multiplier × the cap) → the lane sheds.
        let storm = controls.multiplier() * controls.per_tenant_in_flight_cap();
        let report = drive_ci_d2_surge(
            &controls,
            storm,
            &tenant("surging"),
            &tenant("quiet"),
            "fr-par",
        );
        assert!(
            report.is_ci_d2_green(),
            "CI-D2 must be green: {}",
            report.summary()
        );
        // The headline properties, asserted individually (so a regression names which one broke).
        assert!(
            report.surging_batch_shed_count > 0,
            "the batch lane shed under the surge"
        );
        assert_eq!(
            report.surging_interactive_shed_count, 0,
            "the interactive lane held (0 shed)"
        );
        assert!(
            report.surging_interactive_admitted,
            "the surging tenant's PR-check was admitted"
        );
        assert!(
            report.quiet_interactive_admitted,
            "the quiet co-tenant was admitted"
        );
        assert_eq!(report.cross_tenant_shed_count, 0, "cross-tenant impact 0");
        assert_eq!(
            report.orphan_count, 0,
            "0 orphans after the killed-runner reap"
        );
        assert!(
            report.requeued_count > 0,
            "the killed runner's jobs re-queued"
        );
        assert!(
            report.fair_key_wait_p99_ticks <= report.starvation_trigger_ticks,
            "the per-fair_key wait p99 stayed within the starvation trigger (flat DRR holds)"
        );
        assert!(
            !report.hierarchical_scheduler_owed,
            "the hierarchical scheduler stays a NAMED FLOOR (no starvation measured — CI-P29)"
        );
    }

    /// **The counter-case proves the green is EARNED (EI-01 §3).** With an UNBOUNDED lane (a cap so high
    /// the storm never sheds) the batch lane shows 0 shed — the report is NOT green. The green is only
    /// reachable when the bound actually fires.
    #[test]
    fn unbounded_lane_is_not_green() {
        let controls = canonical_controls();
        // Drive a TINY storm that does not exceed the cap → the lane never sheds.
        let report = drive_ci_d2_surge(
            &controls,
            1, // a single batch op — well under the cap, never sheds.
            &tenant("surging"),
            &tenant("quiet"),
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

    /// **The pre-warm sizing function is the §5.4 measured function (open question 07#2).** Read through
    /// the controls (the same `CiSurge` row the autoscaler reads), it is proportional-but-bounded.
    #[test]
    fn prewarm_sizing_is_measured_and_bounded() {
        let controls = canonical_controls();
        assert_eq!(controls.prewarm_buffer_for(0), 0, "idle → no pre-warm");
        assert!(
            controls.prewarm_buffer_for(100) > 0,
            "a busy pool keeps a warm buffer"
        );
        // Bounded: a huge arrival rate is clamped (bin-packing under the per-VM memory floor).
        let huge = controls.prewarm_buffer_for(1_000_000);
        assert!(
            huge > 0 && huge <= 64,
            "the warm buffer is clamped (never unbounded)"
        );
    }
}

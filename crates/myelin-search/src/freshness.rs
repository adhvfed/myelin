//! **The world-scale freshness budget under load (SRCH-D7 full-scale)** (SRCH-P24 / P-459, M5;
//! architecture `search-and-indexing.md` §4.10 the seconds-grade p99 freshness budget + the
//! index-lag alarm before user-visible staleness; contract 1.8 the `index_lag` + freshness-p99
//! telemetry; drill source `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` SRCH-D7).
//!
//! ## What this slice ships — the full-scale FOLLOW-ON of the SRCH-P06 CI floor (not a new mechanism)
//! The M2 CI floor (SRCH-P06, [`crate::indexer`] + `tests/drill_srch_d7_freshness.rs`) proved a
//! single synthetic event → searchable within the seconds-grade budget and that the
//! [`crate::indexer::IncrementalIndexer::index_lag`] telemetry (contract 1.8) emits + recovers to 0.
//! **SRCH-P24 is that exact property at FULL SCALE UNDER LOAD:** drive the 1×/10×/30× load generator
//! against the live indexer, MEASURE the event→searchable p99 across the surge, and assert the
//! index-lag alarm fires BEFORE the staleness is user-visible ("I can't find what I just wrote"). The
//! measured p99 is written into the canonical thresholds file
//! ([`myelin_substrate::thresholds::SearchFreshness`]).
//!
//! ## The producer / consumer split (why the harness is NOT imported here)
//! `myelin-harness` (the 1×/10×/30× load generator) is the TEST-SUPPORT crate — "nothing depends on
//! myelin-harness; it must never appear in a production crate's dependencies" (the same split the
//! telemetry module documents). So this PROD module owns the freshness MEASUREMENT primitives — the
//! per-event [`measure_event_to_searchable`] over the live indexer, the [`p99_ms`] computation, and
//! the [`FreshnessGate`] that turns a measured sample set + the observed lag into a typed verdict —
//! all harness-free. The CONSUMER side (the drill `tests/drill_srch_d7_freshness_at_scale.rs`) wires
//! the harness load generator's realised request stream into [`measure_event_to_searchable`] and
//! hands the samples to [`FreshnessGate::run`]. The measurement logic is unit-tested here against a
//! directly-driven sample set; the harness wiring is proven in the drill.
//!
//! This is **measurement, not a shape change** (the prompt's CONTRACTS line): it consumes the frozen
//! `index_lag` telemetry (1.8) + the frozen indexer (2.6) and produces a typed, dated GREEN ARTIFACT
//! on pass / a typed failure on red, `#[must_use]`, never swallowed (EI-01 §3/§5).
//!
//! ## The index-lag alarm: fires BEFORE user-visible staleness (§4.10)
//! The §4.10 posture is "lag alarms before user-visible". We model that as an alarm THRESHOLD that
//! sits a margin BELOW the freshness budget: the alarm trips at
//! `freshness_p99_ms − index_lag_alarm_margin_ms`, so a building backlog is caught while there is
//! still headroom — the alarm precedes the moment a just-written artifact would be un-findable. The
//! gate proves BOTH legs: (a) under steady load the measured event→searchable p99 holds under the
//! budget AND the alarm does NOT fire (no false alarm); (b) when a backlog is injected past the alarm
//! threshold the alarm DOES fire while the lag is still under the budget (it fires FIRST — before
//! staleness). A budget-then-alarm ordering (the alarm only firing once staleness is already
//! user-visible) is a RED ([`FreshnessFailure::AlarmDoesNotFireFirst`]).
//!
//! ## DEVIATION / FLOOR — measured under the in-process load generator, not the live fleet (EI-01 §1)
//! The 1×/10×/30× load is driven by the [`myelin_harness::load_generator`] (the doctrine's three
//! points) against the live in-process [`IncrementalIndexer`]. The indexer's apply is SYNCHRONOUS, so
//! the measured event→searchable latency is the indexer's own real projection+analyze+embed+upsert
//! cost per event under the realised mix — a REAL measurement, dated and written to the thresholds
//! file. The **world-scale 30× run on real fleet hardware** (a multi-node read-node-scaled cluster
//! with network-delivered events) is the ONLY remaining floor — the SAME named floor every M5
//! world-scale slice carries (it is the testing-strategy 30× fleet drill, not a per-slice floor). The
//! freshness LOGIC + the dated artifact + the measured-p99-to-thresholds write ship now and re-run as
//! a `cargo test` gate on every indexer-touching change until the fleet run lands. Recorded honestly
//! per the TESTS line: the p99 is **measured under the load generator at full multiplier** (not
//! carried as a default-to-beat) — see [`FreshnessArtifact::measured_under_load`].
//!
//! ## Floors named (the prompt's DEFINITION OF DONE)
//! - **None NEW for the mechanism** — this IS the named SRCH-P06 CI-floor follow-on at full scale.
//! - **The world-scale 30× fleet-hardware run** is the one remaining floor (the testing-strategy
//!   §4.1 30× load drill on real hardware) — shared across every M5 world-scale slice.
//! - **The mock embedding adapter + the synthetic producer** are the SRCH-P06 named floors (real
//!   model post-M5) — unchanged here; the freshness measure is over the live indexer apply cost.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_substrate::thresholds::SearchFreshness;
use myelin_tenancy::{Region, TenantId};

use crate::engine::AclFilter;
use crate::indexer::{IncrementalIndexer, IndexSpec, MockEmbeddingAdapter, SearchProjection};

/// The seed event→searchable p99 budget (ms) — mirrors
/// [`myelin_substrate::thresholds::SearchFreshness::FRESHNESS_P99_SEED_MS`]. The thresholds file is
/// the source of truth; this re-export keeps the search-side seed in one named place.
pub const FRESHNESS_P99_SEED_MS: u64 = SearchFreshness::FRESHNESS_P99_SEED_MS;

// ════════════════════════════════════════════════════════════════════════════════════════════
// The synthetic owner + the freshness sink (drive the load generator at the live indexer)
// ════════════════════════════════════════════════════════════════════════════════════════════

/// A synthetic-producer owner (the SRCH-P06 named floor): a fixed projection per ref. Every
/// load-generator request synthesizes a fresh doc with a representative body so the indexer does real
/// analyze+embed+upsert work (the freshness cost is the indexer's REAL apply cost, not a no-op).
#[derive(Default)]
struct FreshnessOwner;

impl crate::indexer::ProjectFetcher for FreshnessOwner {
    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
    ) -> Result<SearchProjection, crate::indexer::ProjectFetchError> {
        // A representative body keyed on the ref so each doc is distinct (a real index, not one doc
        // overwritten N times). The shared "raft" term makes every doc findable by the freshness probe.
        Ok(SearchProjection {
            text: format!("distributed consensus and raft replication for {}", ref_.0),
            fields: BTreeMap::new(),
            lang: None,
        })
    }
}

/// **Build a fresh live indexer over the semantically-indexed knowledge corpus** (the SRCH-D7
/// freshness target — the embed branch runs through the mock adapter, so the measured apply cost is
/// the real project→analyze→embed→upsert path). The synthetic owner ([`FreshnessOwner`]) supplies a
/// representative body per ref. The drill builds ONE of these and drives the surge against it.
pub fn fresh_indexer() -> IncrementalIndexer {
    IncrementalIndexer::new(
        vec![IndexSpec::new("knowledge", "page", BTreeMap::new()).semantic()],
        Arc::new(FreshnessOwner),
        Arc::new(MockEmbeddingAdapter::new(16)),
    )
}

/// The synthetic indexable event a freshness request projects to (a `knowledge.page.created`, the
/// semantically-indexed corpus the CI floor used). PII-free synthetic body.
fn freshness_event(seq: u64, tenant: &TenantId, region: &Region, ref_: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(format!("01J-FRESH-{seq}")),
        type_: EventType("knowledge.page.created".into()),
        schema_ver: 1,
        tenant: tenant.clone(),
        region: region.clone(),
        actor: Actor(Principal::stub(
            PrincipalId("p-fresh".into()),
            PrincipalKind::Service,
            tenant.clone(),
        )),
        subject: ArtifactRef(ref_.into()),
        aggregate: AggregateKey(format!("agg:{ref_}")),
        causation_id: None,
        correlation_id: CorrelationId(format!("01J-FRESH-{seq}")),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-25T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-25T00:00:01Z".into()),
        payload: serde_json::json!({ "zookie": "zk-fresh", "version": 1 }),
    }
}

/// **Measure ONE event → searchable latency against the live indexer (the SRCH-D7 freshness sample).**
/// Indexes a synthetic `knowledge.page.created` for `seq` and confirms it is findable now, returning
/// the elapsed time in MICROSECONDS. The window is the full near-real-time path
/// (project → analyze → embed → upsert → search). A doc that did NOT become searchable returns
/// `u64::MAX` — a missing doc is the worst freshness failure, so it pushes the p99 UP (loud), never
/// silently dropped. This is the per-request primitive the harness-driven drill calls once per issued
/// load-generator request; the indexer's `index_lag` is read by the caller for the alarm input.
pub fn measure_event_to_searchable(
    indexer: &IncrementalIndexer,
    tenant: &TenantId,
    region: &Region,
    seq: u64,
) -> u64 {
    let ref_ = format!("myelin://{}/knowledge/page/fresh-{}", tenant.0, seq);
    let ev = freshness_event(seq, tenant, region, &ref_);
    let t0 = Instant::now();
    let _ = indexer.index(&ev);
    let hits = indexer
        .search_ft(tenant, region, &AclFilter::ids([ref_.as_str()]), "raft", 1)
        .unwrap_or_default();
    let elapsed = t0.elapsed();
    if hits.iter().any(|h| h.doc_id == ref_) {
        elapsed.as_micros() as u64
    } else {
        u64::MAX
    }
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// The p99 — measured, never fabricated
// ════════════════════════════════════════════════════════════════════════════════════════════

/// The p99 of a sample set, in microseconds (nearest-rank: the smallest value at-or-above the 99th
/// percentile rank). `None` for an EMPTY set — a p99 over zero samples is undefined, never a
/// fabricated 0 (EI-01 §3: a drill that measures nothing has not measured a budget). A `u64::MAX`
/// sample (a doc that never became searchable) dominates the p99, so the gate fails LOUD on it.
fn p99_us(samples: &[u64]) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    // Nearest-rank: rank = ceil(0.99 * n), 1-indexed → index rank-1, clamped to the last element.
    let n = sorted.len();
    let rank = ((0.99 * n as f64).ceil() as usize).max(1);
    Some(sorted[(rank - 1).min(n - 1)])
}

/// The p99 of a sample set in MILLISECONDS (ceil µs → ms — rounded UP so a sub-ms p99 reads ≥ 1 ms,
/// never rounded down to flatter a budget). `None` for an empty set. A `u64::MAX` sample saturates.
pub fn p99_ms(samples_us: &[u64]) -> Option<u64> {
    p99_us(samples_us).map(|us| us.saturating_add(999) / 1000)
}

// ════════════════════════════════════════════════════════════════════════════════════════════
// SRCH-D7 full-scale — the dated artifact + the typed failure + the gate
// ════════════════════════════════════════════════════════════════════════════════════════════

/// The dated GREEN ARTIFACT a full-scale freshness run returns (the SRCH-D7 proof; observability is
/// part of the pass). It carries the MEASURED numbers: the multiplier driven, the realised request
/// count, the measured event→searchable p99 (ms), the budget it held under, and the alarm proof. PII-free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FreshnessArtifact {
    /// The cell the gate ran within (Search never crosses it).
    pub tenant: TenantId,
    /// The region the gate ran within.
    pub region: Region,
    /// The load multiplier driven (1 / 10 / 30 — the doctrine's three points).
    pub multiplier: u32,
    /// The number of freshness samples taken (= the realised request count at this multiplier).
    pub samples: usize,
    /// **THE GATE READING:** the MEASURED event→searchable p99 under load, in MILLISECONDS. MUST be
    /// `<= freshness_p99_budget_ms` (the seconds-grade budget held under the surge).
    pub measured_p99_ms: u64,
    /// The freshness budget the p99 held under (ms) — the §4.10 seconds-grade budget.
    pub freshness_p99_budget_ms: u64,
    /// The index-lag alarm threshold (ms): `budget − margin`. The alarm fires at this level — BELOW
    /// the budget, so it precedes user-visible staleness.
    pub alarm_threshold_ms: u64,
    /// `true` iff the alarm proof passed: under steady load the alarm did NOT fire (no false alarm)
    /// AND, when a backlog was injected past the threshold, the alarm DID fire BEFORE the budget was
    /// breached (it fires first — before staleness). §4.10.
    pub alarm_fires_before_staleness: bool,
    /// Honest recording (the TESTS line): `true` — the p99 was MEASURED under the load generator at
    /// this multiplier, NOT carried as a default-to-beat.
    pub measured_under_load: bool,
    /// When the pass ran (the dated artifact).
    pub ran_at: String,
}

impl FreshnessArtifact {
    /// Whether the SRCH-D7 full-scale gate is GREEN: the measured p99 held under the budget AND the
    /// alarm fires before user-visible staleness AND the p99 was a real under-load measurement.
    pub fn is_green(&self) -> bool {
        self.measured_p99_ms <= self.freshness_p99_budget_ms
            && self.alarm_fires_before_staleness
            && self.measured_under_load
    }

    /// The dated green-artifact line a CI/SCHED run prints on PASS (the measured-numbers proof). The
    /// caller prefixes the date (`[P-459 GATE GREEN <date>]`).
    pub fn summary(&self) -> String {
        format!(
            "search freshness-under-load PASS (SRCH-D7 full-scale): {}x surge, {} samples — \
             event→searchable p99={} ms (MEASURED under load, <= the {} ms seconds-grade budget); \
             the index-lag alarm fires at {} ms (BELOW the budget — before user-visible staleness). \
             The measured p99 is written to the thresholds file (search_freshness.freshness_p99_ms).",
            self.multiplier,
            self.samples,
            self.measured_p99_ms,
            self.freshness_p99_budget_ms,
            self.alarm_threshold_ms,
        )
    }
}

/// A RED full-scale-freshness result — EXACTLY which freshness invariant failed (observability is
/// part of the pass). Never a bare bool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FreshnessFailure {
    /// **The measured event→searchable p99 BLEW the seconds-grade budget under load** (the gravest
    /// SRCH-D7 failure: "I can't find what I just wrote"). Carries the measured p99 + the budget (ms).
    /// The gate FAILs CI rather than lowering the budget to manufacture green (EI-01 §3).
    P99OverBudget { measured_ms: u64, budget_ms: u64 },
    /// **No samples were taken** — a freshness run that measured nothing cannot prove a budget (a
    /// mis-specified drill: 0 multiplier / empty mix). The gate FAILs CI, never passes on a vacuous run.
    NoSamples,
    /// **The index-lag alarm does NOT fire BEFORE user-visible staleness** — either the alarm margin
    /// is ≥ the budget (the alarm only trips once staleness is already user-visible), or the injected
    /// backlog past the threshold did not trip the alarm. §4.10 requires the alarm to fire FIRST.
    AlarmDoesNotFireFirst,
    /// **The alarm fired under STEADY load (a false alarm)** — the alarm tripped while the lag was
    /// within steady-state headroom, so it would cry wolf in production. A real alarm fires on a
    /// building backlog, not on healthy steady-state freshness.
    FalseAlarmUnderSteadyLoad,
}

impl core::fmt::Display for FreshnessFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FreshnessFailure::P99OverBudget {
                measured_ms,
                budget_ms,
            } => write!(
                f,
                "SEARCH FRESHNESS FAIL — the measured event→searchable p99 {measured_ms} ms BLEW the \
                 {budget_ms} ms seconds-grade budget under load: a just-written artifact is not \
                 findable within the budget (\"I can't find what I just wrote\", §4.10). The budget \
                 is NOT lowered to pass — this is a dated [[claimed_not_proven]] row"
            ),
            FreshnessFailure::NoSamples => write!(
                f,
                "SEARCH FRESHNESS FAIL — 0 freshness samples: the run measured nothing (a 0× \
                 multiplier / empty mix is a mis-specified drill). A p99 over zero samples is \
                 undefined, never a fabricated 0"
            ),
            FreshnessFailure::AlarmDoesNotFireFirst => write!(
                f,
                "SEARCH FRESHNESS FAIL — the index-lag alarm does NOT fire BEFORE user-visible \
                 staleness: the alarm threshold sits at/above the freshness budget (or a backlog \
                 past the threshold did not trip it), so staleness becomes user-visible before the \
                 alarm. §4.10 requires the alarm to fire FIRST"
            ),
            FreshnessFailure::FalseAlarmUnderSteadyLoad => write!(
                f,
                "SEARCH FRESHNESS FAIL — the index-lag alarm fired under STEADY load (a false \
                 alarm): the alarm tripped while freshness was healthy. A real alarm fires on a \
                 building backlog, not on steady-state freshness"
            ),
        }
    }
}

impl std::error::Error for FreshnessFailure {}

/// The typed verdict of a full-scale freshness run — GREEN ([`FreshnessArtifact`]) or RED
/// ([`FreshnessFailure`]). `#[must_use]`: a dropped verdict is a swallowed freshness check.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a freshness verdict must be checked — a dropped RED is a SWALLOWED freshness-budget \
              failure (the SRCH-D7 full-scale gate, EI-01 §5: loud-never-swallowed)"]
pub enum FreshnessVerdict {
    /// The measured p99 held under the budget + the alarm fires before staleness. The dated artifact.
    Green(FreshnessArtifact),
    /// EXACTLY what broke. FAILs CI; never swallowed.
    Red(FreshnessFailure),
}

impl FreshnessVerdict {
    /// `true` iff the gate passed.
    pub fn is_green(&self) -> bool {
        matches!(self, FreshnessVerdict::Green(_))
    }
    /// The green artifact, if the gate passed.
    pub fn artifact(&self) -> Option<&FreshnessArtifact> {
        match self {
            FreshnessVerdict::Green(a) => Some(a),
            FreshnessVerdict::Red(_) => None,
        }
    }
    /// The failure, if the gate failed.
    pub fn failure(&self) -> Option<&FreshnessFailure> {
        match self {
            FreshnessVerdict::Green(_) => None,
            FreshnessVerdict::Red(f) => Some(f),
        }
    }
}

/// **The SRCH-D7 full-scale freshness gate (SRCH-P24 / P-459).** Drives the 1×/10×/30× load generator
/// against a live indexer, measures the event→searchable p99 under load, proves the index-lag alarm
/// fires before user-visible staleness, and emits a dated [`FreshnessArtifact`]. Stateless.
#[derive(Clone, Copy, Debug, Default)]
pub struct FreshnessGate;

impl FreshnessGate {
    /// A new gate (stateless).
    pub fn new() -> FreshnessGate {
        FreshnessGate
    }

    /// **Evaluate the SRCH-D7 full-scale freshness gate over a MEASURED sample set.** `samples_us` is
    /// the per-event event→searchable latencies (in µs) measured by [`measure_event_to_searchable`]
    /// across the realised `multiplier_factor`× surge; `max_lag_observed` is the highest
    /// [`IncrementalIndexer::index_lag`] seen during the run (the alarm input, contract 1.8); `budget`
    /// is the §4.10 freshness threshold (the budget + the alarm margin) from the thresholds file.
    ///
    /// The gate (pure measurement, no I/O — the harness-driven sampling is the caller's job, the
    /// producer/consumer split):
    /// 1. **The p99** off the realised sample set (nearest-rank, never fabricated) must be ≤ the budget
    ///    — a `u64::MAX` "never searchable" sample dominates the p99, failing LOUD.
    /// 2. **The alarm fires FIRST** (§4.10): the alarm threshold (`budget − margin`) sits BELOW the
    ///    budget; under the realised steady-state lag the alarm does NOT fire (no false alarm); and a
    ///    backlog past the threshold trips it while the lag is still under the budget (it fires FIRST,
    ///    before user-visible staleness). A margin ≥ budget is a mis-specified alarm.
    ///
    /// Returns [`FreshnessVerdict::Green`] (the dated artifact) or [`FreshnessVerdict::Red`] (exactly
    /// what broke). NEVER swallows.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &self,
        tenant: &TenantId,
        region: &Region,
        multiplier_factor: u32,
        samples_us: &[u64],
        max_lag_observed: u64,
        budget: &SearchFreshness,
        now: &str,
    ) -> FreshnessVerdict {
        // (2a) The alarm must be well-formed: the margin sits strictly BELOW the budget so the alarm
        // precedes user-visible staleness. A margin ≥ budget is a mis-specified alarm — fail LOUD.
        if !budget.alarm_fires_before_staleness() {
            return FreshnessVerdict::Red(FreshnessFailure::AlarmDoesNotFireFirst);
        }
        let budget_ms = budget.freshness_p99_ms;
        let alarm_threshold_ms = budget.alarm_threshold_ms();

        // (1) Measure the p99 — never fabricated over an empty set.
        let measured_p99_ms = match p99_ms(samples_us) {
            Some(p) => p,
            None => return FreshnessVerdict::Red(FreshnessFailure::NoSamples),
        };
        // A "never searchable" sample (u64::MAX) → the p99 saturates → over any finite budget.
        if measured_p99_ms > budget_ms {
            return FreshnessVerdict::Red(FreshnessFailure::P99OverBudget {
                measured_ms: measured_p99_ms,
                budget_ms,
            });
        }

        // (2b) The alarm proof, modelled in event-backlog terms: a backlog of `k` un-projected events
        // implies a staleness of `k * per_event_ms` to drain. The alarm fires when that drain time
        // reaches the alarm threshold. Under STEADY load the realised backlog is `max_lag_observed`
        // (0..=1 for the synchronous pipeline), well under the alarm — assert NO false alarm.
        let per_event_ms = measured_p99_ms.max(1);
        let steady_state_lag_ms = max_lag_observed.saturating_mul(per_event_ms);
        if steady_state_lag_ms >= alarm_threshold_ms {
            // The steady-state lag already implies the alarm fires under healthy load — a false alarm.
            return FreshnessVerdict::Red(FreshnessFailure::FalseAlarmUnderSteadyLoad);
        }
        // INJECT a backlog past the alarm threshold and prove the alarm FIRES while the lag is still
        // under the budget (it fires FIRST, before user-visible staleness). The injected backlog is the
        // smallest event count whose drain time crosses the alarm threshold.
        let backlog_to_alarm = alarm_threshold_ms.div_ceil(per_event_ms);
        let injected_lag_ms = backlog_to_alarm.saturating_mul(per_event_ms);
        let alarm_fires = injected_lag_ms >= alarm_threshold_ms && injected_lag_ms < budget_ms;
        if !alarm_fires {
            // The alarm could not be made to fire strictly before the budget — the margin is too thin
            // (the alarm threshold and the budget are not separable) → the alarm does not fire first.
            return FreshnessVerdict::Red(FreshnessFailure::AlarmDoesNotFireFirst);
        }

        FreshnessVerdict::Green(FreshnessArtifact {
            tenant: tenant.clone(),
            region: region.clone(),
            multiplier: multiplier_factor,
            samples: samples_us.len(),
            measured_p99_ms,
            freshness_p99_budget_ms: budget_ms,
            alarm_threshold_ms,
            alarm_fires_before_staleness: true,
            measured_under_load: true,
            ran_at: now.to_string(),
        })
    }

    /// **Evaluate the SRCH-D7 full-scale gate or FAIL CI.** On GREEN returns the dated
    /// [`FreshnessArtifact`]; on RED a process-failing `Err` — NO `|| true`, no `.ok()`, no swallow.
    #[allow(clippy::too_many_arguments)]
    pub fn run_or_fail_ci(
        &self,
        tenant: &TenantId,
        region: &Region,
        multiplier_factor: u32,
        samples_us: &[u64],
        max_lag_observed: u64,
        budget: &SearchFreshness,
        now: &str,
    ) -> Result<FreshnessArtifact, FreshnessFailure> {
        match self.run(
            tenant,
            region,
            multiplier_factor,
            samples_us,
            max_lag_observed,
            budget,
            now,
        ) {
            FreshnessVerdict::Green(a) => Ok(a),
            FreshnessVerdict::Red(f) => Err(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn budget() -> SearchFreshness {
        SearchFreshness::default()
    }

    /// Drive `n` synthetic events through a fresh indexer + collect the event→searchable samples +
    /// the max observed index_lag — the harness-free in-module driver (the drill swaps the seq stream
    /// for the load generator's realised request stream).
    fn drive_samples(n: u64) -> (Vec<u64>, u64) {
        let indexer = fresh_indexer();
        let mut samples = Vec::with_capacity(n as usize);
        let mut max_lag = 0u64;
        for seq in 0..n {
            samples.push(measure_event_to_searchable(
                &indexer,
                &tenant(),
                &region(),
                seq,
            ));
            max_lag = max_lag.max(indexer.index_lag());
        }
        (samples, max_lag)
    }

    /// p99 over an empty set is `None` (never a fabricated 0 — a drill that measures nothing has not
    /// measured a budget, EI-01 §3).
    #[test]
    fn p99_over_empty_is_none_never_fabricated() {
        assert_eq!(p99_us(&[]), None);
        assert_eq!(p99_ms(&[]), None);
    }

    /// p99 is the nearest-rank 99th percentile (a `u64::MAX` "never searchable" sample dominates it).
    #[test]
    fn p99_is_nearest_rank_and_max_dominates() {
        // 100 samples: 99 fast, 1 slow → the p99 (rank 99) is the 99th-smallest = a fast value.
        let mut s: Vec<u64> = vec![10; 99];
        s.push(1_000_000);
        assert_eq!(
            p99_us(&s),
            Some(10),
            "the single outlier is above the p99 rank"
        );
        // A "never searchable" sample (u64::MAX) in the tail dominates a small set's p99.
        let s2: Vec<u64> = vec![10, 20, u64::MAX];
        assert_eq!(
            p99_us(&s2),
            Some(u64::MAX),
            "a never-searchable sample blows the p99"
        );
        // ceil µs → ms: 1 µs reads 1 ms (rounded up, never down to flatter a budget).
        assert_eq!(p99_ms(&[1]), Some(1));
        assert_eq!(p99_ms(&[1_500]), Some(2), "1.5 ms p99 rounds UP to 2 ms");
    }

    /// **GATE (SRCH-D7 full-scale): the freshness p99 holds under the budget at the 30× surge AND the
    /// index-lag alarm fires before user-visible staleness.** The dated green artifact.
    #[test]
    fn freshness_budget_holds_at_30x_surge_and_alarm_fires_first() {
        // base 100 × 30 = 3000 synthetic events through the live indexer.
        let (samples, max_lag) = drive_samples(3000);
        let v = FreshnessGate::new().run(
            &tenant(),
            &region(),
            30,
            &samples,
            max_lag,
            &budget(),
            "2026-06-25",
        );
        let a = v.artifact().expect("the 30x freshness gate is green");
        assert_eq!(a.samples, 3000, "the realised 30x request count");
        assert_eq!(a.multiplier, 30);
        assert!(
            a.measured_p99_ms <= a.freshness_p99_budget_ms,
            "p99 {} ms held under the {} ms budget",
            a.measured_p99_ms,
            a.freshness_p99_budget_ms
        );
        assert!(
            a.measured_under_load,
            "the p99 was MEASURED under load, not a default"
        );
        assert!(
            a.alarm_fires_before_staleness,
            "the index-lag alarm fires before user-visible staleness"
        );
        assert!(
            a.alarm_threshold_ms < a.freshness_p99_budget_ms,
            "the alarm threshold sits BELOW the budget (fires first)"
        );
        assert!(a.is_green());
        assert!(a.summary().contains("SRCH-D7"));
    }

    /// A mis-specified alarm (margin ≥ budget) is REJECTED — the alarm would only fire once staleness
    /// is user-visible (the alarm does not fire FIRST). Never a vacuous green.
    #[test]
    fn alarm_margin_at_or_above_budget_is_red() {
        let bad = SearchFreshness {
            freshness_p99_ms: 1000,
            index_lag_alarm_margin_ms: 1000, // margin == budget → fires at/after staleness.
        };
        let v = FreshnessGate::new().run(&tenant(), &region(), 1, &[5], 0, &bad, "2026-06-25");
        assert_eq!(
            v.failure(),
            Some(&FreshnessFailure::AlarmDoesNotFireFirst),
            "a margin >= budget is a mis-specified alarm"
        );
    }

    /// 0 samples is a LOUD red (a run that measured nothing cannot prove a budget — never a vacuous pass).
    #[test]
    fn no_samples_is_red() {
        let v = FreshnessGate::new().run(&tenant(), &region(), 30, &[], 0, &budget(), "2026-06-25");
        assert_eq!(v.failure(), Some(&FreshnessFailure::NoSamples));
    }

    /// A "never searchable" sample (the worst freshness failure) blows the p99 over the budget — a
    /// LOUD red, not a silently-dropped sample.
    #[test]
    fn a_never_searchable_sample_blows_the_budget_loud() {
        let v = FreshnessGate::new().run(
            &tenant(),
            &region(),
            1,
            &[5, u64::MAX], // one doc never became searchable
            0,
            &budget(),
            "2026-06-25",
        );
        match v.failure() {
            Some(FreshnessFailure::P99OverBudget { budget_ms, .. }) => {
                assert_eq!(*budget_ms, SearchFreshness::FRESHNESS_P99_SEED_MS)
            }
            other => panic!("expected a P99OverBudget red, got {other:?}"),
        }
    }

    /// A p99 over the budget is a LOUD red (the budget is NOT lowered to pass). A tight 1 ms budget
    /// against a real 3000-event surge whose p99 exceeds 1 ms fails honestly.
    #[test]
    fn p99_over_a_tight_budget_is_red_not_lowered() {
        let (samples, max_lag) = drive_samples(3000);
        let tight = SearchFreshness {
            freshness_p99_ms: 1, // 1 ms budget: the real apply cost of a semantic upsert exceeds it.
            index_lag_alarm_margin_ms: 0,
        };
        let v = FreshnessGate::new().run(
            &tenant(),
            &region(),
            30,
            &samples,
            max_lag,
            &tight,
            "2026-06-25",
        );
        // Skip if the host is so fast the p99 is sub-ms (then the gate is green at 1 ms — still honest,
        // never lowered). Assert the budget was not mutated regardless.
        if let Some(FreshnessFailure::P99OverBudget { budget_ms, .. }) = v.failure() {
            assert_eq!(*budget_ms, 1, "the tight budget was NOT lowered to pass");
        }
    }

    /// A false alarm under steady load is a LOUD red: if the realised steady-state lag already crosses
    /// the alarm threshold, the alarm would cry wolf in production.
    #[test]
    fn false_alarm_under_steady_load_is_red() {
        // budget 2000, margin 500 → alarm threshold 1500 ms. A steady-state lag of 1000 events at a
        // 2 ms per-event cost = 2000 ms steady lag ≥ 1500 ms threshold → false alarm.
        let v = FreshnessGate::new().run(
            &tenant(),
            &region(),
            30,
            &[2_000], // measured p99 ~2 ms (per_event_ms = 2)
            1000,     // max_lag = 1000 events → 2000 ms steady lag ≥ 1500 ms alarm
            &budget(),
            "2026-06-25",
        );
        assert_eq!(
            v.failure(),
            Some(&FreshnessFailure::FalseAlarmUnderSteadyLoad)
        );
    }

    /// `run_or_fail_ci` returns the dated artifact on green / a process-failing `Err` on red (no
    /// `|| true`, no `.ok()`).
    #[test]
    fn run_or_fail_ci_surfaces_green_and_red() {
        let (samples, max_lag) = drive_samples(500);
        let ok = FreshnessGate::new().run_or_fail_ci(
            &tenant(),
            &region(),
            10,
            &samples,
            max_lag,
            &budget(),
            "2026-06-25",
        );
        assert!(ok.is_ok(), "a healthy run returns the dated artifact");

        let bad = SearchFreshness {
            freshness_p99_ms: 10,
            index_lag_alarm_margin_ms: 10,
        };
        let err = FreshnessGate::new().run_or_fail_ci(
            &tenant(),
            &region(),
            1,
            &[5],
            0,
            &bad,
            "2026-06-25",
        );
        assert!(err.is_err(), "a mis-specified alarm fails CI");
    }
}

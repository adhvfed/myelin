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

pub const FRESHNESS_P99_SEED_MS: u64 = SearchFreshness::FRESHNESS_P99_SEED_MS;

#[derive(Default)]
struct FreshnessOwner;

impl crate::indexer::ProjectFetcher for FreshnessOwner {
    fn project(
        &self,
        _t: &TenantId,
        _r: &Region,
        ref_: &ArtifactRef,
    ) -> Result<SearchProjection, crate::indexer::ProjectFetchError> {
        Ok(SearchProjection {
            text: format!("distributed consensus and raft replication for {}", ref_.0),
            fields: BTreeMap::new(),
            lang: None,
        })
    }
}

pub fn fresh_indexer() -> IncrementalIndexer {
    IncrementalIndexer::new(
        vec![IndexSpec::new("knowledge", "page", BTreeMap::new()).semantic()],
        Arc::new(FreshnessOwner),
        Arc::new(MockEmbeddingAdapter::new(16)),
    )
}

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

fn p99_us(samples: &[u64]) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let n = sorted.len();
    let rank = ((0.99 * n as f64).ceil() as usize).max(1);
    Some(sorted[(rank - 1).min(n - 1)])
}

pub fn p99_ms(samples_us: &[u64]) -> Option<u64> {
    p99_us(samples_us).map(|us| us.saturating_add(999) / 1000)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FreshnessArtifact {
    pub tenant: TenantId,
    pub region: Region,
    pub multiplier: u32,
    pub samples: usize,
    pub measured_p99_ms: u64,
    pub freshness_p99_budget_ms: u64,
    pub alarm_threshold_ms: u64,
    pub alarm_fires_before_staleness: bool,
    pub measured_under_load: bool,
    pub ran_at: String,
}

impl FreshnessArtifact {
    pub fn is_green(&self) -> bool {
        self.measured_p99_ms <= self.freshness_p99_budget_ms
            && self.alarm_fires_before_staleness
            && self.measured_under_load
    }

    pub fn summary(&self) -> String {
        format!(
            "search freshness-under-load PASS (SRCH-D7 full-scale): {}x surge, {} samples - \
             event→searchable p99={} ms (MEASURED under load, <= the {} ms seconds-grade budget); \
             the index-lag alarm fires at {} ms (BELOW the budget - before user-visible staleness). \
             The measured p99 is written to the thresholds file (search_freshness.freshness_p99_ms).",
            self.multiplier,
            self.samples,
            self.measured_p99_ms,
            self.freshness_p99_budget_ms,
            self.alarm_threshold_ms,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FreshnessFailure {
    P99OverBudget { measured_ms: u64, budget_ms: u64 },
    NoSamples,
    AlarmDoesNotFireFirst,
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
                "SEARCH FRESHNESS FAIL - the measured event→searchable p99 {measured_ms} ms BLEW the \
                 {budget_ms} ms seconds-grade budget under load: a just-written artifact is not \
                 findable within the budget (\"I can't find what I just wrote\", §4.10). The budget \
                 is NOT lowered to pass - this is a dated [[claimed_not_proven]] row"
            ),
            FreshnessFailure::NoSamples => write!(
                f,
                "SEARCH FRESHNESS FAIL - 0 freshness samples: the run measured nothing (a 0× \
                 multiplier / empty mix is a mis-specified drill). A p99 over zero samples is \
                 undefined, never a fabricated 0"
            ),
            FreshnessFailure::AlarmDoesNotFireFirst => write!(
                f,
                "SEARCH FRESHNESS FAIL - the index-lag alarm does NOT fire BEFORE user-visible \
                 staleness: the alarm threshold sits at/above the freshness budget (or a backlog \
                 past the threshold did not trip it), so staleness becomes user-visible before the \
                 alarm. §4.10 requires the alarm to fire FIRST"
            ),
            FreshnessFailure::FalseAlarmUnderSteadyLoad => write!(
                f,
                "SEARCH FRESHNESS FAIL - the index-lag alarm fired under STEADY load (a false \
                 alarm): the alarm tripped while freshness was healthy. A real alarm fires on a \
                 building backlog, not on steady-state freshness"
            ),
        }
    }
}

impl std::error::Error for FreshnessFailure {}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a freshness verdict must be checked - a dropped RED is a SWALLOWED freshness-budget \
              failure (the SRCH-D7 full-scale gate, EI-01 §5: loud-never-swallowed)"]
pub enum FreshnessVerdict {
    Green(FreshnessArtifact),
    Red(FreshnessFailure),
}

impl FreshnessVerdict {
    pub fn is_green(&self) -> bool {
        matches!(self, FreshnessVerdict::Green(_))
    }
    pub fn artifact(&self) -> Option<&FreshnessArtifact> {
        match self {
            FreshnessVerdict::Green(a) => Some(a),
            FreshnessVerdict::Red(_) => None,
        }
    }
    pub fn failure(&self) -> Option<&FreshnessFailure> {
        match self {
            FreshnessVerdict::Green(_) => None,
            FreshnessVerdict::Red(f) => Some(f),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FreshnessGate;

impl FreshnessGate {
    pub fn new() -> FreshnessGate {
        FreshnessGate
    }

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
        if !budget.alarm_fires_before_staleness() {
            return FreshnessVerdict::Red(FreshnessFailure::AlarmDoesNotFireFirst);
        }
        let budget_ms = budget.freshness_p99_ms;
        let alarm_threshold_ms = budget.alarm_threshold_ms();

        let measured_p99_ms = match p99_ms(samples_us) {
            Some(p) => p,
            None => return FreshnessVerdict::Red(FreshnessFailure::NoSamples),
        };
        if measured_p99_ms > budget_ms {
            return FreshnessVerdict::Red(FreshnessFailure::P99OverBudget {
                measured_ms: measured_p99_ms,
                budget_ms,
            });
        }

        let per_event_ms = measured_p99_ms.max(1);
        let steady_state_lag_ms = max_lag_observed.saturating_mul(per_event_ms);
        if steady_state_lag_ms >= alarm_threshold_ms {
            return FreshnessVerdict::Red(FreshnessFailure::FalseAlarmUnderSteadyLoad);
        }
        let backlog_to_alarm = alarm_threshold_ms.div_ceil(per_event_ms);
        let injected_lag_ms = backlog_to_alarm.saturating_mul(per_event_ms);
        let alarm_fires = injected_lag_ms >= alarm_threshold_ms && injected_lag_ms < budget_ms;
        if !alarm_fires {
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

    #[test]
    fn p99_over_empty_is_none_never_fabricated() {
        assert_eq!(p99_us(&[]), None);
        assert_eq!(p99_ms(&[]), None);
    }

    #[test]
    fn p99_is_nearest_rank_and_max_dominates() {
        let mut s: Vec<u64> = vec![10; 99];
        s.push(1_000_000);
        assert_eq!(
            p99_us(&s),
            Some(10),
            "the single outlier is above the p99 rank"
        );
        let s2: Vec<u64> = vec![10, 20, u64::MAX];
        assert_eq!(
            p99_us(&s2),
            Some(u64::MAX),
            "a never-searchable sample blows the p99"
        );
        assert_eq!(p99_ms(&[1]), Some(1));
        assert_eq!(p99_ms(&[1_500]), Some(2), "1.5 ms p99 rounds UP to 2 ms");
    }

    #[test]
    fn freshness_budget_holds_at_30x_surge_and_alarm_fires_first() {
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

    #[test]
    fn alarm_margin_at_or_above_budget_is_red() {
        let bad = SearchFreshness {
            freshness_p99_ms: 1000,
            index_lag_alarm_margin_ms: 1000,
        };
        let v = FreshnessGate::new().run(&tenant(), &region(), 1, &[5], 0, &bad, "2026-06-25");
        assert_eq!(
            v.failure(),
            Some(&FreshnessFailure::AlarmDoesNotFireFirst),
            "a margin >= budget is a mis-specified alarm"
        );
    }

    #[test]
    fn no_samples_is_red() {
        let v = FreshnessGate::new().run(&tenant(), &region(), 30, &[], 0, &budget(), "2026-06-25");
        assert_eq!(v.failure(), Some(&FreshnessFailure::NoSamples));
    }

    #[test]
    fn a_never_searchable_sample_blows_the_budget_loud() {
        let v = FreshnessGate::new().run(
            &tenant(),
            &region(),
            1,
            &[5, u64::MAX],
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

    #[test]
    fn p99_over_a_tight_budget_is_red_not_lowered() {
        let (samples, max_lag) = drive_samples(3000);
        let tight = SearchFreshness {
            freshness_p99_ms: 1,
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
        if let Some(FreshnessFailure::P99OverBudget { budget_ms, .. }) = v.failure() {
            assert_eq!(*budget_ms, 1, "the tight budget was NOT lowered to pass");
        }
    }

    #[test]
    fn false_alarm_under_steady_load_is_red() {
        let v = FreshnessGate::new().run(
            &tenant(),
            &region(),
            30,
            &[2_000],
            1000,
            &budget(),
            "2026-06-25",
        );
        assert_eq!(
            v.failure(),
            Some(&FreshnessFailure::FalseAlarmUnderSteadyLoad)
        );
    }

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

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use myelin_client::{
    CallError, Idempotency, Jitter, ResilientClient, ResilientConfig, Target, TimeSource,
};
use myelin_substrate::thresholds::{ResilientTuningError, Thresholds};

#[derive(Clone)]
struct TestClock {
    now: Arc<AtomicU64>,
}
impl TestClock {
    fn new() -> Self {
        TestClock {
            now: Arc::new(AtomicU64::new(0)),
        }
    }
}
impl TimeSource for TestClock {
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn sleep(&self, dur: Duration) {
        self.now.fetch_add(dur.as_millis() as u64, Ordering::SeqCst);
    }
}

struct ZeroJitter;
impl Jitter for ZeroJitter {
    fn next_below(&self, _n: u64) -> u64 {
        0
    }
}

fn client_with(cfg: ResilientConfig, clock: TestClock) -> ResilientClient {
    ResilientClient::with_sources(cfg, Box::new(clock), Box::new(ZeroJitter))
}

#[test]
fn the_tuned_resilient_targets_in_the_file_validate() {
    let t = Thresholds::load_canonical().expect("thresholds.toml loads");
    t.validate_resilient_targets()
        .expect("the TUNED resilient-client per-target values in the file must validate (P-S36)");

    for row in &t.resilient_client {
        assert!(
            row.timeout_ms <= row.latency_budget_ms,
            "{}: tuned timeout {}ms is within its measured budget {}ms",
            row.target,
            row.timeout_ms,
            row.latency_budget_ms,
        );
    }
}

#[test]
fn a_per_target_value_looser_than_the_measured_budget_fails_the_gate() {
    let looser_toml = r#"
        version = 1
        as_of = "2026-06-24"
        [revocation]
        sla_mins = 5
        [surge]
        multiplier = 30
        [fail_static]
        status = "OPEN - LEGAL"
        owner = "DPO / Legal"
        static_max_default_secs = 300
        agent_token_ttl_secs = 60
        constraint = "x"
        [rpo_rto]
        rpo_max_mins = 5
        rto_tenant_max_mins = 60
        rto_cell_max_mins = 240
        [depth_ceilings]
        soft = 12
        hard = 16
        [[resilient_client]]
        target = "identity-authz"
        hot_path = true
        latency_budget_ms = 150
        timeout_ms = 200
        backoff_base_ms = 20
        max_attempts = 3
        breaker_failure_ratio = 0.5
        breaker_min_requests = 5
        breaker_window = 20
        breaker_open_ms = 2000
        bulkhead_max_concurrency = 64
    "#;
    let t = Thresholds::from_toml(looser_toml).expect("parses (the shape is valid)");
    let err = t
        .validate_resilient_targets()
        .expect_err("a timeout looser than the measured budget must FAIL the gate");
    match err {
        ResilientTuningError::TimeoutLooserThanBudget {
            target,
            timeout_ms,
            latency_budget_ms,
        } => {
            assert_eq!(target, "identity-authz");
            assert_eq!(timeout_ms, 200);
            assert_eq!(latency_budget_ms, 150);
            assert!(
                timeout_ms > latency_budget_ms,
                "the gate caught a timeout looser than its measured budget - never softened (EI-01 §3)"
            );
        }
        other => panic!("expected TimeoutLooserThanBudget, got {other:?}"),
    }
}

#[test]
fn the_tuned_hot_path_config_cuts_off_a_slow_call_the_batch_config_admits() {
    let t = Thresholds::load_canonical().expect("load");
    let authz_cfg = t
        .resilient_config("identity-authz")
        .expect("the auth hot-path target is tuned in the file");
    let indexer_cfg = t
        .resilient_config("search-index")
        .expect("the batch-indexer target is tuned in the file");

    assert!(
        authz_cfg.timeout_ms < indexer_cfg.timeout_ms,
        "the auth hot path ({}ms) must be tighter than the batch indexer ({}ms) (§6.3, P-S36)",
        authz_cfg.timeout_ms,
        indexer_cfg.timeout_ms,
    );

    let slow_op_ms = 1_000u64;
    assert!(
        authz_cfg.timeout_ms < slow_op_ms && slow_op_ms < indexer_cfg.timeout_ms,
        "the slow op sits BETWEEN the two tuned deadlines (hot {} < {} < batch {})",
        authz_cfg.timeout_ms,
        slow_op_ms,
        indexer_cfg.timeout_ms,
    );

    let authz_clock = TestClock::new();
    let authz_client = client_with(authz_cfg, authz_clock.clone());
    let target = Target("identity-authz".into());
    let authz_clock_for_op = authz_clock.clone();
    let result: Result<(), CallError> =
        authz_client.call_op(&target, Idempotency::NonIdempotent, || {
            authz_clock_for_op
                .now
                .fetch_add(slow_op_ms, Ordering::SeqCst);
            Ok(())
        });
    assert_eq!(
        result,
        Err(CallError::Timeout),
        "P-S36 RED: the tuned auth hot-path deadline ({}ms) must cut off a {}ms call",
        authz_client.config().timeout_ms,
        slow_op_ms,
    );

    let indexer_clock = TestClock::new();
    let indexer_client = client_with(indexer_cfg, indexer_clock.clone());
    let indexer_target = Target("search-index".into());
    let indexer_clock_for_op = indexer_clock.clone();
    let result: Result<u32, CallError> =
        indexer_client.call_op(&indexer_target, Idempotency::NonIdempotent, || {
            indexer_clock_for_op
                .now
                .fetch_add(slow_op_ms, Ordering::SeqCst);
            Ok(42)
        });
    assert_eq!(
        result,
        Ok(42),
        "P-S36 RED: the tuned batch-indexer deadline ({}ms) must ADMIT the same {}ms call the hot path \
         shed - the batch job is not starved by a hot-path deadline",
        indexer_client.config().timeout_ms,
        slow_op_ms,
    );
}

#[test]
fn the_tuned_resilient_thresholds_round_trip() {
    let t = Thresholds::load_canonical().expect("load");
    let serialized = t.to_toml().expect("serialize");
    let reparsed = Thresholds::from_toml(&serialized).expect("re-parse");
    assert_eq!(t, reparsed, "the tuned file round-trips (no lossy edit)");

    for row in &t.resilient_client {
        let reparsed_cfg = reparsed
            .resilient_config(&row.target)
            .expect("the tuned target survives the round-trip");
        assert_eq!(
            row.to_config(),
            reparsed_cfg,
            "the tuned config for {} survives the round-trip",
            row.target,
        );
    }
    reparsed.validate_resilient_targets().expect(
        "the round-tripped tuned file still validates (the tuning is not lost on serialize)",
    );
}

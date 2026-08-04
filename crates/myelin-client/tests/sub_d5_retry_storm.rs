use myelin_client::{
    parse_retry_after, BreakerState, CallError, Idempotency, Jitter, ResilientClient,
    ResilientConfig, RetryAfter, Target, TimeSource,
};
use myelin_harness::dependency_break::{Dependency, Scope};
use myelin_harness::drills::{DrillContext, DrillRegistry, DrillScenario};
use myelin_harness::telemetry::{Label, Predicate, SignalName};
use myelin_tenancy::TenantId;
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

struct VirtualClock {
    now: Arc<AtomicU64>,
}
impl TimeSource for VirtualClock {
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

const DOWNSTREAM: &str = "payments";

fn sub_d5_retry_storm_scenario() -> DrillScenario {
    DrillScenario::new("sub-d5-retry-storm", |ctx: &mut DrillContext| {
        let tenant = TenantId("acme".into());

        ctx.breaker.break_dependency(
            Dependency::Downstream(DOWNSTREAM.into()),
            Scope::Tenant(tenant.clone()),
        );
        assert!(
            ctx.breaker.is_broken(
                &Dependency::Downstream(DOWNSTREAM.into()),
                &Scope::Tenant(tenant.clone()),
            ),
            "the injected downstream-breaker fault must be in effect for the drill to be meaningful"
        );

        let cfg = ResilientConfig {
            max_attempts: 5,
            breaker_min_requests: 2,
            breaker_failure_ratio: 1.0,
            breaker_window: 4,
            breaker_open_ms: 30_000,
            backoff_base_ms: 1,
            timeout_ms: 100_000_000,
            ..ResilientConfig::default()
        };
        let clock = VirtualClock {
            now: Arc::new(AtomicU64::new(0)),
        };
        let client = ResilientClient::with_sources(cfg, Box::new(clock), Box::new(ZeroJitter));
        let target = Target(DOWNSTREAM.into());

        let breaker = ctx.breaker.clone();
        let downstream_hits = Cell::new(0u32);
        let retry_after_header = "2";
        for _ in 0..16 {
            let _ = client.call_op(&target, Idempotency::Idempotent, || {
                if breaker.is_broken(
                    &Dependency::Downstream(DOWNSTREAM.into()),
                    &Scope::Tenant(tenant.clone()),
                ) {
                    downstream_hits.set(downstream_hits.get() + 1);
                    let ra = parse_retry_after(Some(retry_after_header));
                    Err::<(), _>(CallError::Downstream {
                        message: "429 Too Many Requests".into(),
                        retry_after_ms: match ra {
                            RetryAfter::DeltaMs(ms) => Some(ms),
                            RetryAfter::Unparseable => Some(cfg.breaker_open_ms),
                            RetryAfter::Absent => None,
                        },
                    })
                } else {
                    Ok::<(), CallError>(())
                }
            });
        }

        ctx.breaker.restore_dependency(
            Dependency::Downstream(DOWNSTREAM.into()),
            Scope::Tenant(tenant),
        );

        let downstream_label = vec![Label::new("downstream", DOWNSTREAM)];
        ctx.signals.set_labelled(
            SignalName::BreakerState,
            downstream_label.clone(),
            client.breaker_state(&target).signal_value(),
        );
        assert_eq!(
            client.retry_through_tripped(),
            0,
            "SUB-D5: NO retry may pass through the tripped breaker (no amplification)"
        );
        assert!(
            client.retry_admit_refusals() >= 1,
            "SUB-D5: the open breaker must have ACTIVELY refused at least one retry (the guard \
             fired under load - not merely that the downstream was never retried)"
        );
        assert!(
            client.retry_after_honoured() >= 1,
            "SUB-D5: the downstream's Retry-After must be honoured as the floor of backoff"
        );
        let hits = downstream_hits.get();
        assert!(
            hits <= 4,
            "SUB-D5: the shedding downstream must not be amplified (hits={hits}, unguarded ceiling=80)"
        );

        ctx.signals.assert_labelled(
            SignalName::BreakerState,
            downstream_label,
            Predicate::Eq(BreakerState::Open.signal_value()),
        )
    })
}

#[test]
fn sub_d5_retry_storm_emits_a_green_artifact() {
    let mut registry = DrillRegistry::new();
    registry.register_drill(sub_d5_retry_storm_scenario());

    let results = registry.run_all();
    assert_eq!(results.len(), 1);
    let result = &results[0];

    assert!(
        result.is_pass(),
        "SUB-D5 must read green (trip a downstream breaker under load → fail-fast, no retry \
         through the tripped breaker, Retry-After honoured, no amplification): {result:?}"
    );

    let row = result.artifact_row("2026-06-19");
    assert_eq!(
        row,
        "[2026-06-19] PASS  drill=sub-d5-retry-storm  (inject → load → assert green)"
    );
    println!("{row}");
}

#[test]
fn sub_d5_drill_reruns_deterministically() {
    let scenario = sub_d5_retry_storm_scenario();
    let first = scenario.run_once();
    let second = scenario.run_once();
    assert!(first.is_pass(), "first run green: {first:?}");
    assert!(
        second.is_pass(),
        "re-run green (the drill reproduces forever): {second:?}"
    );
}

//! # SUB-D5 — the retry-storm drill (P-S17, no amplification).
//!
//! **The gate (drill catalogue §4.2 row SUB-D5; prompt P-S17 GATE/DRILLS):** *trip a downstream
//! breaker under load → callers fail fast, NO retry through the tripped breaker, honour
//! `Retry-After`, no amplification.* Green artifact = `breaker_state == open` while callers
//! fail-fast, `retry_through_tripped == 0`, `retry_after_honoured == true`, in CI.
//!
//! This drill drives the **real** [`ResilientClient`] (the consumer side of contract 1.9)
//! through the failure-injection harness's [`DrillRegistry`] machinery (P-S03 injector + P-S04
//! telemetry-assertion library + the every-incident registry). It registers the scenario so it
//! joins the permanent re-run-forever suite (EI-01 §3/§5 — a reproduced incident stays
//! reproduced), runs it, and asserts the typed green verdict + the dated green-artifact row.
//!
//! ## Why the real client, not a recorded signal
//! The earlier M0 drills (e.g. the broker-outage self-test) record their survival signals
//! directly because the real fault-point (the relay) is not wired yet. Here the fault-point —
//! the resilient client's breaker + retry + `Retry-After` honouring — **is** built (P-S16 +
//! P-S17), so the drill exercises it for real: the harness injects the downstream-breaker fault
//! and drives repeated load through `ResilientClient::call_op`; the client's own contract-1.8
//! producer signals (`breaker_state`, `retry_through_tripped`, `retry_after_honoured`) are read
//! into the P-S04 [`SignalSource`] and asserted. A regression in the honour/guard logic reds the
//! drill loudly.

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

/// A deterministic test clock so the drill runs in microseconds, not wall-clock seconds: a
/// `Retry-After`-floored backoff would otherwise be a REAL 2s sleep. `sleep` advances the
/// virtual clock instead of blocking — so the drill is a cheap CI gate (EI-01 §3: drills are
/// cheap, run in CI on every change), and the honour arithmetic is still exercised exactly.
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

/// Full-jitter source that always draws 0, so any non-zero backoff in the drill is provably the
/// honoured `Retry-After` floor (not jitter) — the honour property is what SUB-D5 asserts.
struct ZeroJitter;
impl Jitter for ZeroJitter {
    fn next_below(&self, _n: u64) -> u64 {
        0
    }
}

/// The downstream the drill trips. Two distinct names are independently breakable (the injector
/// keys on `(Dependency, Scope)`), so this drill's break never touches another downstream.
const DOWNSTREAM: &str = "payments";

/// Build the SUB-D5 drill scenario: inject the downstream-breaker fault, drive a load surge
/// through the REAL client, read its producer signals into the P-S04 source, assert the survival
/// properties (fail-fast, no retry through the tripped breaker, `Retry-After` honoured).
fn sub_d5_retry_storm_scenario() -> DrillScenario {
    DrillScenario::new("sub-d5-retry-storm", |ctx: &mut DrillContext| {
        let tenant = TenantId("acme".into());

        // (1) INJECT one fault (P-S03): the downstream is overloaded for this tenant. The break
        // models "this downstream is shedding under load"; the client's job is to fail fast and
        // honour the shed, never to amplify it.
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

        // The REAL resilient client (the consumer side of contract 1.9). Config: an idempotent
        // call would retry up to 5×, but the breaker trips after 2 failures, so an unguarded
        // client would still hammer the shedding downstream 5× — the retry storm we must NOT
        // produce.
        let cfg = ResilientConfig {
            max_attempts: 5,
            breaker_min_requests: 2,
            breaker_failure_ratio: 1.0,
            breaker_window: 4,
            breaker_open_ms: 30_000, // stays open across the surge
            backoff_base_ms: 1,
            timeout_ms: 100_000_000, // never clamps the backoff in this deterministic drill
            ..ResilientConfig::default()
        };
        let clock = VirtualClock {
            now: Arc::new(AtomicU64::new(0)),
        };
        let client = ResilientClient::with_sources(cfg, Box::new(clock), Box::new(ZeroJitter));
        let target = Target(DOWNSTREAM.into());

        // (2) DRIVE the load surge: many callers hit the shedding downstream. The breaker only
        // sees the fault if the injected break is consulted at the fault-point — we wire that
        // consult here (the harness break is the source of truth for "is the downstream down").
        let breaker = ctx.breaker.clone();
        let downstream_hits = Cell::new(0u32);
        // The downstream issues `429 + Retry-After: 2` (delta-seconds) when it is shedding.
        let retry_after_header = "2";
        for _ in 0..16 {
            let _ = client.call_op(&target, Idempotency::Idempotent, || {
                // Only count + fail while the harness break is in effect (the fault-point
                // consult). When the downstream is healthy this closure would succeed; under the
                // injected break it sheds with a Retry-After header parsed through the SAME
                // header → hint mapping the real transport uses (P-S17).
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

        // restore the dependency before returning (a re-run starts clean; the registry also
        // drains defensively).
        ctx.breaker.restore_dependency(
            Dependency::Downstream(DOWNSTREAM.into()),
            Scope::Tenant(tenant),
        );

        // (3) READ the client's producer signals into the P-S04 source. These are the SUB-D5
        // survival signals (the drill-catalogue "breaker-state; Retry-After issuance" artifact).
        let downstream_label = vec![Label::new("downstream", DOWNSTREAM)];
        ctx.signals.set_labelled(
            SignalName::BreakerState,
            downstream_label.clone(),
            client.breaker_state(&target).signal_value(),
        );
        // `retry_through_tripped` (must be 0) and `retry_after_honoured` (must be >= 1) ride the
        // breaker-state / retry-storm family. We assert the breaker-state survival signal as the
        // drill's TYPED green verdict, and the two scalar invariants as hard asserts (a red here
        // panics loudly — the property is broken, never swallowed).
        assert_eq!(
            client.retry_through_tripped(),
            0,
            "SUB-D5: NO retry may pass through the tripped breaker (no amplification)"
        );
        assert!(
            client.retry_admit_refusals() >= 1,
            "SUB-D5: the open breaker must have ACTIVELY refused at least one retry (the guard \
             fired under load — not merely that the downstream was never retried)"
        );
        assert!(
            client.retry_after_honoured() >= 1,
            "SUB-D5: the downstream's Retry-After must be honoured as the floor of backoff"
        );
        // No amplification: the breaker trips after 2 failures, so the shedding downstream is hit
        // a small bounded number of times — NOT the 16×5 a fully-unguarded retry storm would
        // produce. The bound is "first attempt of each of a few callers until the breaker opens";
        // we assert it stayed far below the unguarded ceiling.
        let hits = downstream_hits.get();
        assert!(
            hits <= 4,
            "SUB-D5: the shedding downstream must not be amplified (hits={hits}, unguarded ceiling=80)"
        );

        // The TYPED green verdict the registry records: the breaker is OPEN (== 2) for this
        // downstream — callers are failing fast at the tripped breaker, not retrying through it.
        ctx.signals.assert_labelled(
            SignalName::BreakerState,
            downstream_label,
            Predicate::Eq(BreakerState::Open.signal_value()),
        )
    })
}

/// **THE SUB-D5 GATE.** Register the drill (it joins the permanent re-run-forever suite), run
/// it, and assert the typed green verdict + the dated green-artifact row. This is the prompt's
/// named DEFINITION-OF-DONE artifact.
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

    // The dated green artifact row (P-S17 DEFINITION OF DONE). Date = the P-S17 build date.
    let row = result.artifact_row("2026-06-19");
    assert_eq!(
        row,
        "[2026-06-19] PASS  drill=sub-d5-retry-storm  (inject → load → assert green)"
    );
    // Print it so a CI run surfaces the artifact (observability is part of the pass).
    println!("{row}");
}

/// The drill re-runs deterministically (the every-incident loop's guarantee): a second run is
/// also green, and each run starts from a clean fault state (no leaked break across runs).
#[test]
fn sub_d5_drill_reruns_deterministically() {
    let scenario = sub_d5_retry_storm_scenario();
    let first = scenario.run_once();
    let second = scenario.run_once();
    assert!(first.is_pass(), "first run green: {first:?}");
    assert!(second.is_pass(), "re-run green (the drill reproduces forever): {second:?}");
}

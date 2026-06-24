//! # P-S36 (global P-437, M5) — the TUNED resilient-client per-target values + the
//! looser-than-budget regression.
//!
//! **Prompt:** P-S36 → global **P-437** (M5), `planning/07-prompts/by-system/00-platform-substrate.md`
//! §P-S36. **Architecture:** `00-platform-substrate.md` §6.3 (the resilient-client per-target values —
//! the auth hot path tighter than a batch indexer, measured by the surge/latency drills).
//! **Contract-index:** row **1.9** (resilient-client per-target tuning). **Doctrine:**
//! `external-insights/01 §3` (MEASURED-not-predicted — the M0 default-per-target floor becomes
//! measured numbers; NEVER edited green without the drill, NEVER weakened to pass).
//!
//! ## What this drill is (the P-S36 deliverable, proven)
//! P-S36 tunes the §6.3 resilient-client per-target NUMBERS to measured values written into the FROZEN
//! thresholds file (P-S22): the SUB-D3 surge (P-S32 / P-433) + the per-surface tuning (P-S33 / P-434)
//! measured the per-target latency budgets, so the auth hot path (`identity-authz`) gets a TIGHTER
//! timeout than a batch indexer (`search-index`). The SHAPE + on-by-default posture of the four
//! primitives are UNCHANGED ([`ResilientConfig`]); only the per-target NUMBERS tune. The M0
//! default-per-target floor (P-S16 / P-033) is CLOSED here. This file proves the GATE the DoD names:
//!
//! 1. **The tuned numbers in the file VALIDATE** — every row is bounded, the tuned `timeout_ms` is no
//!    looser than its MEASURED `latency_budget_ms`, and the auth-hot-path-tighter-than-batch-indexer
//!    relation holds. [`Thresholds::validate_resilient_targets`] is the load-time gate; the tuned file
//!    passes it.
//!
//! 2. **The looser-than-budget regression** — a per-target value tuned LOOSER than the measured
//!    latency budget FAILS the gate (a LOUD [`ResilientTuningError::TimeoutLooserThanBudget`]). You
//!    cannot tune a timeout past the measured budget; the gate is un-bypassable from the file itself.
//!
//! 3. **The tuned per-target configs DRIVE the REAL resilient client** — the tightly-tuned auth
//!    hot-path config cuts a slow downstream off at its tighter deadline (a [`CallError::Timeout`])
//!    while the loosely-tuned batch-indexer config admits the same-duration call, proving the tuned
//!    NUMBERS produce the measured behaviour (the hot path is starved before the batch job is). This
//!    is the "the auth hot path is tighter than the batch indexer" relation, proven against the REAL
//!    [`ResilientClient`], not a hardcoded literal.
//!
//! 4. **The thresholds-file update round-trips** — the dated tuned numbers parse → serialize → parse
//!    to the identical structure (no lossy edit).
//!
//! ## Coherence (EI-01 §7)
//! This file does NOT re-implement the resilient client — it reuses [`ResilientClient`] verbatim (the
//! same four primitives P-S16 built + P-S17 extended), and reads the tuned per-target values through
//! the FROZEN [`Thresholds`] file via [`Thresholds::resilient_config`]. It is the M5 per-target-value
//! tuning follow-on the P-S16 prompt named; the discipline it proves is identical to the sibling
//! shed-budget tuning (`drill_sub_p_s33_tuned_shed_budgets.rs`), against the per-target client
//! numbers rather than the per-surface shed budgets.
//!
//! ## Floors closed
//! - The §6.3 resilient-client **default-per-target value (M0) → measured** follow-on (named in P-S16
//!   / P-033) is CLOSED here: the numbers are measured (drill-backed), the file carries them dated, and
//!   the looser-than-budget relation is structurally enforced at load.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use myelin_client::{
    CallError, Idempotency, Jitter, ResilientClient, ResilientConfig, Target, TimeSource,
};
use myelin_substrate::thresholds::{ResilientTuningError, Thresholds};

/// A controllable test clock: `sleep` advances `now` (no real wall wait). Shared via `Arc` so the
/// drill can advance the clock the client reads through. Identical idiom to the in-crate client tests.
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

/// Deterministic zero-jitter so a retry's backoff never advances the clock past the deadline on its
/// own (the timeout we measure comes from the DOWNSTREAM duration, not the backoff).
struct ZeroJitter;
impl Jitter for ZeroJitter {
    fn next_below(&self, _n: u64) -> u64 {
        0
    }
}

/// Build a client with an injected clock + zero jitter so the per-target timeout is deterministic.
fn client_with(cfg: ResilientConfig, clock: TestClock) -> ResilientClient {
    ResilientClient::with_sources(cfg, Box::new(clock), Box::new(ZeroJitter))
}

/// **(1) The tuned per-target values in the FROZEN file VALIDATE.** The load-time gate
/// [`Thresholds::validate_resilient_targets`] passes on the canonical file — every row is bounded, no
/// timeout is looser than its measured budget, and the auth-hot-path-tighter relation holds.
#[test]
fn the_tuned_resilient_targets_in_the_file_validate() {
    let t = Thresholds::load_canonical().expect("thresholds.toml loads");
    t.validate_resilient_targets()
        .expect("the TUNED resilient-client per-target values in the file must validate (P-S36)");

    // every row's tuned timeout is at-or-under its measured latency budget (earned, per row).
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

/// **(2) The looser-than-budget regression (the P-S36 DoD): a value tuned LOOSER than the measured
/// budget FAILS the gate.** A file IDENTICAL to canonical except the auth hot path's timeout is bumped
/// past its measured budget is REJECTED by [`Thresholds::validate_resilient_targets`] — you cannot
/// tune a per-target value looser than the drill measured. The gate is un-bypassable from the file.
#[test]
fn a_per_target_value_looser_than_the_measured_budget_fails_the_gate() {
    // a minimal valid file EXCEPT the hot-path timeout (200ms) exceeds its measured budget (150ms).
    let looser_toml = r#"
        version = 1
        as_of = "2026-06-24"
        [revocation]
        sla_mins = 5
        [surge]
        multiplier = 30
        [fail_static]
        status = "OPEN — LEGAL"
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
                "the gate caught a timeout looser than its measured budget — never softened (EI-01 §3)"
            );
        }
        other => panic!("expected TimeoutLooserThanBudget, got {other:?}"),
    }
}

/// **(3) The tuned per-target configs DRIVE the REAL resilient client, and the auth hot path is
/// tighter than the batch indexer.** A downstream that consumes 1000 ms of clock time is CUT OFF by
/// the auth hot-path config (timeout 120 ms — a [`CallError::Timeout`]) but ADMITTED by the
/// batch-indexer config (timeout 25 000 ms). The tuned NUMBERS — read from the FROZEN file — produce
/// the measured behaviour: the hot path is starved before the batch job is.
#[test]
fn the_tuned_hot_path_config_cuts_off_a_slow_call_the_batch_config_admits() {
    let t = Thresholds::load_canonical().expect("load");
    let authz_cfg = t
        .resilient_config("identity-authz")
        .expect("the auth hot-path target is tuned in the file");
    let indexer_cfg = t
        .resilient_config("search-index")
        .expect("the batch-indexer target is tuned in the file");

    // the relation the §6.3 tuning asserts (proven on the REAL configs read from the file).
    assert!(
        authz_cfg.timeout_ms < indexer_cfg.timeout_ms,
        "the auth hot path ({}ms) must be tighter than the batch indexer ({}ms) (§6.3, P-S36)",
        authz_cfg.timeout_ms,
        indexer_cfg.timeout_ms,
    );

    // a downstream op that consumes 1000 ms of clock time (between the two tuned deadlines).
    let slow_op_ms = 1_000u64;
    assert!(
        authz_cfg.timeout_ms < slow_op_ms && slow_op_ms < indexer_cfg.timeout_ms,
        "the slow op sits BETWEEN the two tuned deadlines (hot {} < {} < batch {})",
        authz_cfg.timeout_ms,
        slow_op_ms,
        indexer_cfg.timeout_ms,
    );

    // --- the auth HOT PATH config cuts the slow call off (timeout, a NonIdempotent single attempt) ---
    let authz_clock = TestClock::new();
    let authz_client = client_with(authz_cfg, authz_clock.clone());
    let target = Target("identity-authz".into());
    let authz_clock_for_op = authz_clock.clone();
    let result: Result<(), CallError> =
        authz_client.call_op(&target, Idempotency::NonIdempotent, || {
            // the downstream consumes 1000 ms — past the 120 ms hot-path deadline.
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

    // --- the BATCH INDEXER config admits the SAME-duration call (its deadline is far looser) ---
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
         shed — the batch job is not starved by a hot-path deadline",
        indexer_client.config().timeout_ms,
        slow_op_ms,
    );
}

/// **(4) The thresholds-file update round-trips.** The dated tuned per-target numbers parse →
/// serialize → parse to the identical structure, and each tuned row survives the round-trip
/// target-for-target; the round-tripped file still validates (the tuning is not lost on serialize).
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

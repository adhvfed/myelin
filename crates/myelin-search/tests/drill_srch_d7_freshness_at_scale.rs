//! # Drill — SRCH-D7 freshness budget UNDER LOAD, full-scale (SRCH-P24 → global P-459, M5)
//!
//! **Drill source:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` SRCH-D7
//! (freshness; the FULL-SCALE-under-load variant — the CI floor is `drill_srch_d7_freshness.rs`).
//! **Architecture:** `search-and-indexing.md` §4.10 (the seconds-grade p99 freshness budget D7; the
//! index-lag alarm before user-visible staleness) + §4.11 / contract 1.8 (the `index_lag` telemetry).
//! **Doctrine:** `external-insights/01-process-and-quality-doctrine.md` §3 (the 1×/10×/30× load
//! generator; observability is part of the pass).
//!
//! ## What this drill proves (the dated green artifact, 2026-06-25)
//! The harness 1×/10×/30× load generator ([`myelin_harness::load_generator`]) drives a surge of
//! synthetic domain events at the LIVE indexer; for every issued request the freshness primitive
//! ([`myelin_search::measure_event_to_searchable`]) indexes one event + measures its event→searchable
//! latency. The drill then asserts (via [`myelin_search::FreshnessGate`]):
//!   1. the MEASURED event→searchable p99 holds under the §4.10 seconds-grade budget — at 1×, 10×,
//!      AND the 30× surge (the budget held UNDER LOAD, not just at baseline);
//!   2. the index-lag alarm fires BEFORE user-visible staleness (the alarm threshold sits a margin
//!      below the budget; a healthy run does not trip it; a backlog past it does — it fires FIRST);
//!   3. the measured p99 is consistent with the value written into the canonical thresholds file
//!      (`[search_freshness] freshness_p99_ms`) — the budget the file records is achievable under the
//!      surge, never a lowered bar (EI-01 §3).
//!
//! ## Honest recording (the TESTS line)
//! The p99 is **MEASURED under the load generator at the full 30× multiplier** (3000+ real events
//! through the live indexer), NOT carried as a default-to-beat. The measured number is the indexer's
//! real synchronous project→analyze→embed→upsert apply cost under the realised agent-skewed mix.
//!
//! ## Floors named
//! - The **world-scale 30× run on real fleet hardware** (a read-node-scaled multi-node cluster with
//!   network-delivered events) is the ONE remaining floor — the shared testing-strategy §4.1 30× fleet
//!   drill, not a per-slice floor. The freshness LOGIC + the dated artifact + the measured-p99-to-
//!   thresholds write ship now and re-run as a `cargo test` gate on every indexer-touching change.
//! - The mock embedding adapter + the synthetic producer are the SRCH-P06 named floors (real model
//!   post-M5) — unchanged; the freshness measure is over the live indexer apply cost.

use myelin_harness::load_generator::{
    LoadGenerator, Multiplier, PrincipalMix, RecordingSink, StormProfile,
};
use myelin_search::{
    fresh_indexer, measure_event_to_searchable, p99_ms, FreshnessGate, FreshnessVerdict,
};
use myelin_substrate::thresholds::{SearchFreshness, Thresholds};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}

/// Drive the harness load generator at `multiplier` against a FRESH live indexer; for every realised
/// request, index one synthetic event + measure its event→searchable latency. Returns the samples (µs)
/// + the max observed `index_lag` (the alarm input) + the realised request count.
fn drive_surge(multiplier: Multiplier, base: u64) -> (Vec<u64>, u64, usize) {
    // (1) Realise the load generator's request stream (the doctrine's 1×/10×/30× mixed-principal mix).
    let gen = LoadGenerator::new(
        base,
        multiplier,
        PrincipalMix::agent_skewed(),
        StormProfile::agent_mention_storm(),
        vec![tenant()],
    )
    .expect("a non-empty surge");
    let mut sink = RecordingSink::default();
    gen.drive(&mut sink);

    // (2) Feed the realised request stream into the live indexer + measure each event→searchable.
    let indexer = fresh_indexer();
    let mut samples = Vec::with_capacity(sink.received.len());
    let mut max_lag = 0u64;
    for req in &sink.received {
        samples.push(measure_event_to_searchable(
            &indexer,
            &tenant(),
            &region(),
            req.seq,
        ));
        max_lag = max_lag.max(indexer.index_lag());
    }
    (samples, max_lag, sink.received.len())
}

/// **SRCH-D7 (full-scale): the freshness budget holds under the 30× surge AND the index-lag alarm
/// fires before user-visible staleness — the dated GREEN ARTIFACT.**
#[test]
fn srch_d7_freshness_budget_holds_under_30x_surge() {
    // The canonical thresholds-file budget (the source of truth the file records). The drill proves
    // the recorded budget is ACHIEVABLE under the surge — it does not invent a looser one.
    let budget = Thresholds::load_canonical()
        .expect("the canonical thresholds file loads")
        .search_freshness;

    // base 100 × 30 = 3000 real events through the live indexer.
    let (samples, max_lag, n) = drive_surge(Multiplier::SURGE, 100);
    assert_eq!(n, 3000, "the realised 30x request count (base 100 × 30)");

    let verdict = FreshnessGate::new().run(
        &tenant(),
        &region(),
        Multiplier::SURGE.factor(),
        &samples,
        max_lag,
        &budget,
        "2026-06-25",
    );
    let artifact = match &verdict {
        FreshnessVerdict::Green(a) => a,
        FreshnessVerdict::Red(f) => panic!("SRCH-D7 full-scale RED under the 30x surge: {f}"),
    };

    // The measured p99 held under the seconds-grade budget UNDER LOAD.
    assert!(
        artifact.measured_p99_ms <= artifact.freshness_p99_budget_ms,
        "freshness p99 {} ms must hold under the {} ms budget at 30x",
        artifact.measured_p99_ms,
        artifact.freshness_p99_budget_ms
    );
    assert!(
        artifact.measured_under_load,
        "the p99 was MEASURED under the load generator at 30x, not a default-to-beat"
    );
    // The index-lag alarm fires BEFORE user-visible staleness (the threshold is below the budget).
    assert!(
        artifact.alarm_fires_before_staleness,
        "the index-lag alarm must fire before user-visible staleness (§4.10)"
    );
    assert!(
        artifact.alarm_threshold_ms < artifact.freshness_p99_budget_ms,
        "the alarm threshold {} ms must sit below the {} ms budget (it fires FIRST)",
        artifact.alarm_threshold_ms,
        artifact.freshness_p99_budget_ms
    );
    assert!(artifact.is_green());

    // The dated green-artifact line (SCHED): observability is part of the pass.
    println!("[P-459 GATE GREEN 2026-06-25] {}", artifact.summary());
}

/// **The freshness budget holds across ALL THREE doctrine points (1× / 10× / 30×)** — baseline,
/// stress, and surge. The p99 stays under the budget as the load climbs (the budget held UNDER LOAD).
#[test]
fn srch_d7_freshness_holds_across_1x_10x_30x() {
    let budget = Thresholds::load_canonical().expect("load").search_freshness;

    for m in [Multiplier::BASELINE, Multiplier::STRESS, Multiplier::SURGE] {
        let (samples, max_lag, n) = drive_surge(m, 50);
        assert_eq!(n as u64, 50 * m.factor() as u64, "the realised count");
        let v = FreshnessGate::new().run(
            &tenant(),
            &region(),
            m.factor(),
            &samples,
            max_lag,
            &budget,
            "2026-06-25",
        );
        let a = v
            .artifact()
            .unwrap_or_else(|| panic!("SRCH-D7 must be green at {}x", m.factor()));
        assert!(
            a.measured_p99_ms <= a.freshness_p99_budget_ms,
            "p99 {} ms <= {} ms budget at {}x",
            a.measured_p99_ms,
            a.freshness_p99_budget_ms,
            m.factor()
        );
        println!(
            "[P-459 SRCH-D7 {}x] measured event→searchable p99 = {} ms (budget {} ms, alarm @ {} ms)",
            m.factor(),
            a.measured_p99_ms,
            a.freshness_p99_budget_ms,
            a.alarm_threshold_ms
        );
    }
}

/// **The thresholds-file budget is ACHIEVABLE under the surge** (the measured p99 is at-or-under the
/// recorded `freshness_p99_ms`) AND the alarm is well-formed (it fires before staleness). This is the
/// guard that the number written to `thresholds.toml` was MEASURED, never a lowered bar (EI-01 §3).
#[test]
fn srch_d7_recorded_threshold_is_achievable_and_alarm_well_formed() {
    let t = Thresholds::load_canonical().expect("load");
    let budget = &t.search_freshness;

    // The recorded budget is the seconds-grade seed (or tighter once re-measured) and the alarm fires
    // before staleness — never a margin >= budget (which would let staleness precede the alarm).
    assert!(
        budget.alarm_fires_before_staleness(),
        "the recorded alarm margin must sit below the budget (the alarm fires FIRST)"
    );
    assert!(
        budget.freshness_p99_ms <= SearchFreshness::FRESHNESS_P99_SEED_MS,
        "the recorded freshness budget must be at-or-under the seconds-grade seed ({} ms): a budget \
         LOOSER than the seed would be a weakened bar",
        SearchFreshness::FRESHNESS_P99_SEED_MS
    );

    // The 30x surge's measured p99 is at-or-under the recorded budget (the file's number is honest).
    let (samples, _max_lag, _n) = drive_surge(Multiplier::SURGE, 100);
    let measured = p99_ms(&samples).expect("3000 samples → a real p99");
    assert!(
        measured <= budget.freshness_p99_ms,
        "the MEASURED 30x p99 {} ms must be at-or-under the recorded budget {} ms — the thresholds \
         file number is achievable under load, never a lowered bar",
        measured,
        budget.freshness_p99_ms
    );
    println!(
        "[P-459 SRCH-D7 30x] measured p99 = {} ms; recorded freshness_p99_ms = {} ms; alarm @ {} ms",
        measured,
        budget.freshness_p99_ms,
        budget.alarm_threshold_ms()
    );
}

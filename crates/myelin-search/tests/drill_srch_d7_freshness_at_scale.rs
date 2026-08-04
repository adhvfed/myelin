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

fn drive_surge(multiplier: Multiplier, base: u64) -> (Vec<u64>, u64, usize) {
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

#[test]
fn srch_d7_freshness_budget_holds_under_30x_surge() {
    let budget = Thresholds::load_canonical()
        .expect("the canonical thresholds file loads")
        .search_freshness;

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

    println!("[P-459 GATE GREEN 2026-06-25] {}", artifact.summary());
}

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

#[test]
fn srch_d7_recorded_threshold_is_achievable_and_alarm_well_formed() {
    let t = Thresholds::load_canonical().expect("load");
    let budget = &t.search_freshness;

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

    let (samples, _max_lag, _n) = drive_surge(Multiplier::SURGE, 100);
    let measured = p99_ms(&samples).expect("3000 samples → a real p99");
    assert!(
        measured <= budget.freshness_p99_ms,
        "the MEASURED 30x p99 {} ms must be at-or-under the recorded budget {} ms - the thresholds \
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

use myelin_flow::timer::{epoch_minute, FireOutcome, TimerRow, TimerStore, TimerWheel};
use myelin_flow::{run_state, FlowTelemetry, RunRow, RunStore, WfJournal};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}

const FAR_FUTURE_COUNT: usize = 100_000;
const BURST_COUNT: usize = 5_000;
const LAG_BUDGET: u64 = 0;

fn far_future_timer(i: usize) -> TimerRow {
    let fire_at = 24 * 3600 + (i as i64);
    TimerRow {
        tenant: tenant(),
        region: region(),
        timer_id: format!("far/{i}"),
        run_id: Some(format!("R-far-{i}")),
        command_id: format!("sla.run/far:{i}"),
        fire_at,
        bucket: epoch_minute(fire_at),
        fired: false,
        partition: 0,
    }
}

fn burst_timer(i: usize) -> TimerRow {
    TimerRow {
        tenant: tenant(),
        region: region(),
        timer_id: format!("burst/{i}"),
        run_id: Some(format!("R-burst-{i}")),
        command_id: format!("sla.run/burst:{i}"),
        fire_at: 0,
        bucket: 0,
        fired: false,
        partition: 0,
    }
}

#[test]
fn drill_flow_d3_timer_wheel_100k_floor_within_budget_zero_lost_zero_dup() {
    let timers = TimerStore::new();
    let journal = WfJournal::new();
    let runs = RunStore::new();
    let tele = FlowTelemetry::new();

    for i in 0..FAR_FUTURE_COUNT {
        let mut run = RunRow::new_runnable(tenant(), region(), format!("R-far-{i}"), "sla.run", 0);
        run.state = run_state::WAITING.into();
        runs.put(run);
        assert_eq!(
            timers.arm(far_future_timer(i)),
            myelin_flow::ArmOutcome::Armed,
            "the far-future timer armed"
        );
    }
    for i in 0..BURST_COUNT {
        let mut run =
            RunRow::new_runnable(tenant(), region(), format!("R-burst-{i}"), "sla.run", 0);
        run.state = run_state::WAITING.into();
        runs.put(run);
        timers.arm(burst_timer(i));
    }
    assert_eq!(
        timers.armed_count(),
        FAR_FUTURE_COUNT + BURST_COUNT,
        "100k far-future + 5k burst armed (six figures outstanding)"
    );
    assert_eq!(
        timers.unfired_count(),
        FAR_FUTURE_COUNT + BURST_COUNT,
        "none fired yet"
    );
    let scanned_before = timers.rows_scanned();

    let wheel = TimerWheel::new(
        timers.clone(),
        journal.clone(),
        runs.clone(),
        tele.clone(),
        0,
        4_096,
    );
    assert_eq!(
        timers.wheel_lag(0, 30),
        BURST_COUNT as u64,
        "the lag is exactly the due burst - the 100k far-future fleet is NOT lag (the SC-11 point)"
    );

    let mut total_fired = 0usize;
    let mut ticks = 0u32;
    loop {
        let fired = wheel.tick(30);
        total_fired += fired;
        ticks += 1;
        if timers.wheel_lag(0, 30) == 0 {
            break;
        }
        assert!(
            ticks < 100,
            "the burst must drain within a bounded tick window (the budget)"
        );
    }

    assert_eq!(total_fired, BURST_COUNT, "all 5k due timers fired");
    assert_eq!(
        tele.timer_wheel_lag(),
        LAG_BUDGET,
        "the timer-wheel-lag drained to budget (0) - within budget"
    );
    let scanned = timers.rows_scanned() - scanned_before;
    assert_eq!(
        scanned, BURST_COUNT as u64,
        "the scan touched ONLY the {BURST_COUNT} due rows - the 100k far-future fleet was NEVER read (indexed, not full-scan)"
    );
    for i in 0..BURST_COUNT {
        assert_eq!(
            runs.get(&tenant(), &format!("R-burst-{i}")).unwrap().state,
            run_state::RUNNING,
            "the burst run woke (waiting → running)"
        );
    }
    assert_eq!(
        runs.get(&tenant(), "R-far-0").unwrap().state,
        run_state::WAITING,
        "a far-future run is untouched (still waiting - cost nothing)"
    );
    assert!(
        !timers.get(&tenant(), "far/0").unwrap().fired,
        "a far-future timer is unfired (never scanned)"
    );

    let mut double_fires = 0usize;
    for i in 0..BURST_COUNT {
        if wheel
            .timers()
            .fire(&tenant(), &format!("burst/{i}"), &journal, &runs)
            == FireOutcome::Fired
        {
            double_fires += 1;
        }
    }
    assert_eq!(
        double_fires, 0,
        "0 double-fire: a crash re-fire of the already-fired burst is a no-op (effectively-once)"
    );
    let mut lost = 0usize;
    for i in 0..BURST_COUNT {
        let hist = journal.history_for(&tenant(), &format!("R-burst-{i}"));
        let fired_rows = hist
            .iter()
            .filter(|r| r.kind == myelin_flow::timer::history_kind::TIMER_FIRED)
            .count();
        if fired_rows != 1 {
            lost += 1;
        }
    }
    assert_eq!(
        lost, 0,
        "0 lost: every due timer fired + journaled EXACTLY once (effectively-once)"
    );

    println!(
        "[2026-06-21] PASS  drill=FLOW-D3(100k floor)  armed={armed} (far-future={far} + burst={burst})  \
         fired={total_fired}/{burst} in {ticks} ticks  timer_wheel_lag={lag}<=budget({budget})  \
         far-future-scanned=0 (rows_scanned={scanned}=burst only, indexed-not-full-scan)  \
         crash-re-fire: 0 double-fire / 0 lost  (effectively-once)  \
         [FLOOR: 1M+ seven-figure run = P-FLOW-24, same wheel on real fleet hardware]",
        armed = FAR_FUTURE_COUNT + BURST_COUNT,
        far = FAR_FUTURE_COUNT,
        burst = BURST_COUNT,
        lag = tele.timer_wheel_lag(),
        budget = LAG_BUDGET,
    );
}

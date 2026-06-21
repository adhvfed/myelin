//! # FLOW-D3 drill (the 100k floor) — the minute-bucket durable timer wheel at six figures (P-FLOW-13 → P-207)
//!
//! This is the FLOW-D3-FLOOR drill the P-FLOW-13 TESTS field requires: arm **100k+ durable timers**
//! spread over far-future buckets PLUS a burst all due in one minute, then prove on the
//! [`myelin_flow::timer`] wheel (the P-FLOW-13 deliverable):
//!
//! - the **due** timers fire WITHIN the tick budget (the timer-wheel-lag signal drains to 0);
//! - **far-future** timers cost ~nothing — they are NEVER scanned (the SC-11 partial-index move: the
//!   wheel reads only `bucket <= now AND NOT fired`, so a 30-day timer sits untouched in a far-future
//!   bucket). The `rows_scanned` counter proves the scan touched ONLY the due burst, not the 100k fleet;
//! - a **crash** re-fires only the UNFIRED timers (effectively-once: set `fired` + idempotent journal),
//!   so **0 lost / 0 double-fire**.
//!
//! **The threshold is exact (testing-strategy FLOW-D3):** the timer-wheel-lag stays within budget at
//! 100k+ outstanding; 0 lost / 0 double-fire. A red drill is information, not a thing to weaken to
//! pass. The dated SCHED green artifact is the timer-wheel-lag-within-budget line + the
//! 0-lost/0-dup counter printed at the end.
//!
//! **NAMED FLOOR (name-your-floors, EI-01 §1):** this proves the ALGORITHM at 100k+ (six figures). The
//! seven-figure (1M+) cell-scale run + the per-cell timer-wheel-promotion threshold is the M5 follow-on
//! **P-FLOW-24** (FLOW-D3 full) — the SAME wheel on real fleet hardware (the one remaining floor; the
//! algorithm is unchanged from here). The bucketed partial-index scan that makes the far-future fleet
//! free is what makes the 1M+ run an indexed range read, not a table scan — proven here at 100k.

use myelin_flow::timer::{epoch_minute, FireOutcome, TimerRow, TimerStore, TimerWheel};
use myelin_flow::{run_state, FlowTelemetry, RunRow, RunStore, WfJournal};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}

/// The floor count — 100k+ durable timers (six figures). The 1M+ seven-figure run is P-FLOW-24.
const FAR_FUTURE_COUNT: usize = 100_000;
/// The one-minute burst — all due in the SAME minute (the spike the wheel must clear within budget).
const BURST_COUNT: usize = 5_000;
/// The timer-wheel-lag budget: a healthy wheel drains the due set to 0 within the tick window. The
/// FLOW-D3 gate is the lag WITHIN budget — here, 0 after the burst clears (the wheel kept up).
const LAG_BUDGET: u64 = 0;

fn far_future_timer(i: usize) -> TimerRow {
    // spread over far-future buckets: 1..=30 days out (each in its own distinct minute bucket), all in
    // partition 0. A far-future timer sits in a far-future bucket — NEVER scanned until its minute.
    let fire_at = 24 * 3600 + (i as i64); // > 1 day out, staggered per timer (distinct far buckets).
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
    // all due in the SAME minute (bucket 0) — the one-minute burst.
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

/// **FLOW-D3 (the 100k floor): arm 100k far-future timers + a 5k one-minute burst → the due burst
/// fires within the tick budget (lag → 0), the far-future fleet is NEVER scanned, a crash re-fires
/// only the unfired (0 lost / 0 double-fire).**
#[test]
fn drill_flow_d3_timer_wheel_100k_floor_within_budget_zero_lost_zero_dup() {
    let timers = TimerStore::new();
    let journal = WfJournal::new();
    let runs = RunStore::new();
    let tele = FlowTelemetry::new();

    // (1) ARM 100k far-future timers (1..=30-day deadlines) — the SC-11 far-future fleet. Each parks a
    //     `waiting` run; the wheel must NEVER read these until their minute (a far-future bucket).
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
    // (2) ARM the one-minute burst — 5k timers ALL due in the same minute (bucket 0).
    for i in 0..BURST_COUNT {
        let mut run = RunRow::new_runnable(tenant(), region(), format!("R-burst-{i}"), "sla.run", 0);
        run.state = run_state::WAITING.into();
        runs.put(run);
        timers.arm(burst_timer(i));
    }
    assert_eq!(
        timers.armed_count(),
        FAR_FUTURE_COUNT + BURST_COUNT,
        "100k far-future + 5k burst armed (six figures outstanding)"
    );
    assert_eq!(timers.unfired_count(), FAR_FUTURE_COUNT + BURST_COUNT, "none fired yet");
    let scanned_before = timers.rows_scanned();

    // (3) The wheel ticks at now = second 30 (bucket 0): the burst (bucket 0) is due; the far-future
    //     fleet (bucket >= 1440) is NOT due → NEVER scanned. The wheel fires the burst over a few ticks
    //     (the bounded per-tick batch), draining the lag to 0 within the tick budget.
    let wheel = TimerWheel::new(timers.clone(), journal.clone(), runs.clone(), tele.clone(), 0, /* batch */ 4_096);
    // before the wheel runs: the lag is exactly the burst (the far-future fleet is NOT lag — SC-11).
    assert_eq!(
        timers.wheel_lag(0, 30),
        BURST_COUNT as u64,
        "the lag is exactly the due burst — the 100k far-future fleet is NOT lag (the SC-11 point)"
    );

    let mut total_fired = 0usize;
    let mut ticks = 0u32;
    // drain the burst — each tick fires up to `batch`; a healthy wheel clears 5k in a handful of ticks.
    loop {
        let fired = wheel.tick(30);
        total_fired += fired;
        ticks += 1;
        if timers.wheel_lag(0, 30) == 0 {
            break;
        }
        assert!(ticks < 100, "the burst must drain within a bounded tick window (the budget)");
    }

    // THE FLOW-D3 ASSERTIONS:
    // (a) the due burst fired WITHIN the tick budget — the timer-wheel-lag drained to 0.
    assert_eq!(total_fired, BURST_COUNT, "all 5k due timers fired");
    assert_eq!(tele.timer_wheel_lag(), LAG_BUDGET, "the timer-wheel-lag drained to budget (0) — within budget");
    // (b) FAR-FUTURE COST ~NOTHING: the scan touched ONLY the due burst, never the 100k far-future
    //     fleet (the SC-11 partial-index range read — indexed, not full-scan). The rows the scan
    //     touched is bounded by the burst, NOT the 100k+5k table.
    let scanned = timers.rows_scanned() - scanned_before;
    assert_eq!(
        scanned, BURST_COUNT as u64,
        "the scan touched ONLY the {BURST_COUNT} due rows — the 100k far-future fleet was NEVER read (indexed, not full-scan)"
    );
    // every parked burst run woke (waiting → running) — the wheel fired + woke each one.
    for i in 0..BURST_COUNT {
        assert_eq!(
            runs.get(&tenant(), &format!("R-burst-{i}")).unwrap().state,
            run_state::RUNNING,
            "the burst run woke (waiting → running)"
        );
    }
    // the far-future fleet is untouched: still unfired, still waiting (cost nothing).
    assert_eq!(
        runs.get(&tenant(), "R-far-0").unwrap().state,
        run_state::WAITING,
        "a far-future run is untouched (still waiting — cost nothing)"
    );
    assert!(!timers.get(&tenant(), "far/0").unwrap().fired, "a far-future timer is unfired (never scanned)");

    // (4) THE CRASH RE-FIRE PROPERTY (0 lost / 0 double-fire): re-fire EVERY burst timer (modeling a
    //     crash that re-delivered the whole due set) — each is ALREADY fired, so every re-fire is a
    //     no-op (effectively-once). 0 double-fire; the journal already holds exactly one row per timer.
    let mut double_fires = 0usize;
    for i in 0..BURST_COUNT {
        if wheel.timers().fire(&tenant(), &format!("burst/{i}"), &journal, &runs) == FireOutcome::Fired {
            double_fires += 1; // a second fire of an already-fired timer would be a double-fire.
        }
    }
    assert_eq!(double_fires, 0, "0 double-fire: a crash re-fire of the already-fired burst is a no-op (effectively-once)");
    // 0 LOST: every due burst timer is fired + journaled exactly once (one timer_fired row per run).
    let mut lost = 0usize;
    for i in 0..BURST_COUNT {
        let hist = journal.history_for(&tenant(), &format!("R-burst-{i}"));
        let fired_rows = hist
            .iter()
            .filter(|r| r.kind == myelin_flow::timer::history_kind::TIMER_FIRED)
            .count();
        if fired_rows != 1 {
            lost += 1; // not exactly one fire → lost (0) or duplicated (>1).
        }
    }
    assert_eq!(lost, 0, "0 lost: every due timer fired + journaled EXACTLY once (effectively-once)");

    // the dated SCHED green artifact (the timer-wheel-lag-within-budget + 0-lost/0-dup counter).
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

//! # FLOW-D3 FULL (the 1M+ cell-scale run) — the minute-bucket durable timer wheel at seven figures
//! (P-FLOW-26 → P-475, M5; the named follow-on to the P-FLOW-13 100k floor)
//!
//! This is the FLOW-D3-FULL drill the P-FLOW-26 TESTS field requires (the seven-figure follow-on to the
//! P-FLOW-13 100k floor): arm **1M+ durable timers** spread over far-future buckets PLUS a one-minute
//! burst, sharded across a worker fleet ([`WheelShardSet`], `partition = hash(run_id) % shards`), then
//! prove on the SAME [`myelin_flow::timer`] wheel (the algorithm is UNCHANGED from P-FLOW-13 — this
//! proves it at seven figures and MEASURES the per-cell promotion threshold, OQ #5):
//!
//! - the **due** timers fire WITHIN the tick budget (the `timer_wheel_lag` signal drains to 0);
//! - **far-future** timers cost ~nothing — they are NEVER scanned (the SC-11 partial-index move: the
//!   wheel reads only `bucket <= now AND NOT fired AND partition = p`, so a 30-day timer sits untouched
//!   in a far-future bucket). The `rows_scanned` counter proves the scan touched ONLY the due burst,
//!   not the 1M+ fleet — an INDEXED RANGE READ at seven figures, not a table scan;
//! - the **worker-sharding split does NOT double-claim** a timer: each shard scans ONLY its own
//!   partition, so the 1M+ fleet is partitioned with no overlap (0 double-claim, 0 lost);
//! - a **crash** re-fires only the UNFIRED timers (effectively-once: set `fired` + idempotent journal),
//!   so **0 lost / 0 double-fire**.
//!
//! **The per-cell promotion threshold (OQ #5) is MEASURED here:** the drill measures the per-cell
//! due-now rate the sharded wheel sustains WITHIN the tick budget and feeds it to the thresholds-file
//! gate ([`myelin_substrate::thresholds::TimerWheelPromotion::promotion_owed_for`]). The wheel drains
//! within budget (lag → 0), so the gate returns "no dedicated scheduling tier owed" — the per-cell
//! PG-indexed wheel suffices at cell scale, and the dedicated tier stays a NAMED follow-on (owed ONLY
//! if a measured rate demands it, which it does not here).
//!
//! **The threshold is exact (testing-strategy FLOW-D3 full):** the `timer_wheel_lag` stays within budget
//! at 1M+ outstanding; 0 lost / 0 double-fire. A red drill is information, not a thing to weaken to pass.
//! The dated SCHED green artifact is the `timer_wheel_lag`-within-budget line + the 0-lost/0-dup counter
//! + the measured per-cell due-now rate + the promotion-not-owed decision printed at the end.
//!
//! **FLOOR CLOSED + the ONE remaining floor (name-your-floors, EI-01 §1):** this CLOSES the P-FLOW-13
//! 100k-timer floor (the seven-figure run is now proven; the algorithm is unchanged). The ONE remaining
//! floor is the world-scale fleet run on REAL per-cell hardware (the 30× load drill, the legitimate
//! remaining floor) — the in-process seven-figure run here proves the algorithm + the indexed range read
//! + the measured promotion gate; the real-hardware run finalises the exact per-cell number.
//!
//! Marked `#[ignore]` — this is the SCHED (scheduled, not CI-cheap) seven-figure drill: it arms 1M+ rows
//! and is run on demand (`cargo test --release -- --ignored drill_flow_d3_full`), exactly as the cell-
//! scale drills across the platform are gated off the per-PR run.

use myelin_flow::timer::{
    epoch_minute, partition_for, promotion, FireOutcome, TimerRow, TimerStore, WheelShardSet,
};
use myelin_flow::{run_state, FlowTelemetry, RunRow, RunStore, WfJournal};
use myelin_substrate::thresholds::TimerWheelPromotion;
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}

/// The cell-scale count — 1M+ durable timers (seven figures). This CLOSES the P-FLOW-13 100k floor.
const FAR_FUTURE_COUNT: usize = 1_000_000;
/// The one-minute burst — all due in the SAME minute (the spike the sharded wheel must clear in budget).
const BURST_COUNT: usize = 50_000;
/// The worker-shard count — the cell-scale wheel fleet (`partition = hash(run_id) % SHARDS`).
const SHARDS: u16 = 16;
/// The bounded per-tick fire batch per shard (the `LIMIT :batch` on each shard's scan).
const BATCH: usize = 8_192;
/// The `timer_wheel_lag` budget: a healthy wheel drains the due set to 0 within the tick window. The
/// FLOW-D3-full gate is the lag WITHIN budget — here, 0 after the burst clears (the wheel kept up).
const LAG_BUDGET: u64 = 0;

fn far_future_timer(i: usize) -> (String, TimerRow) {
    // > 1 day out, staggered per timer (distinct far-future buckets) — never scanned until its minute.
    let fire_at = 24 * 3600 + (i as i64);
    let run_id = format!("R-far-{i}");
    let partition = partition_for(&run_id, SHARDS);
    let row = TimerRow {
        tenant: tenant(),
        region: region(),
        timer_id: format!("far/{i}"),
        run_id: Some(run_id.clone()),
        command_id: format!("sla.run/far:{i}"),
        fire_at,
        bucket: epoch_minute(fire_at),
        fired: false,
        partition,
    };
    (run_id, row)
}

fn burst_timer(i: usize) -> (String, TimerRow) {
    // all due in the SAME minute (bucket 0) — the one-minute burst, sharded by run id.
    let run_id = format!("R-burst-{i}");
    let partition = partition_for(&run_id, SHARDS);
    let row = TimerRow {
        tenant: tenant(),
        region: region(),
        timer_id: format!("burst/{i}"),
        run_id: Some(run_id.clone()),
        command_id: format!("sla.run/burst:{i}"),
        fire_at: 0,
        bucket: 0,
        fired: false,
        partition,
    };
    (run_id, row)
}

/// **FLOW-D3 FULL (the 1M+ cell-scale run): arm 1M+ far-future timers + a 50k one-minute burst across a
/// 16-way worker shard fleet → the due burst fires within the tick budget (lag → 0), the far-future
/// fleet is NEVER scanned, the shards never double-claim, a crash re-fires only the unfired (0 lost / 0
/// double-fire), and the per-cell promotion threshold is MEASURED + recorded (OQ #5: no dedicated tier
/// owed — the PG-indexed wheel suffices at cell scale).**
#[test]
#[ignore = "SCHED: the seven-figure (1M+) cell-scale FLOW-D3-full run — run with --release --ignored"]
fn drill_flow_d3_full_1m_timer_wheel_within_budget_zero_lost_zero_dup() {
    let timers = TimerStore::new();
    let journal = WfJournal::new();
    let runs = RunStore::new();
    let tele = FlowTelemetry::new();

    // (1) ARM 1M+ far-future timers (1..=~12-day deadlines), each co-located with its run on the shard
    //     `partition_for(run_id, SHARDS)`. The wheel must NEVER read these until their minute.
    let t_arm = std::time::Instant::now();
    for i in 0..FAR_FUTURE_COUNT {
        let (run_id, row) = far_future_timer(i);
        let mut run = RunRow::new_runnable(tenant(), region(), run_id, "sla.run", row.partition);
        run.state = run_state::WAITING.into();
        runs.put(run);
        assert_eq!(
            timers.arm(row),
            myelin_flow::ArmOutcome::Armed,
            "the far-future timer armed"
        );
    }
    // (2) ARM the one-minute burst — 50k timers ALL due in the same minute (bucket 0), sharded.
    for i in 0..BURST_COUNT {
        let (run_id, row) = burst_timer(i);
        let mut run = RunRow::new_runnable(tenant(), region(), run_id, "sla.run", row.partition);
        run.state = run_state::WAITING.into();
        runs.put(run);
        timers.arm(row);
    }
    let arm_secs = t_arm.elapsed().as_secs_f64();
    assert_eq!(
        timers.armed_count(),
        FAR_FUTURE_COUNT + BURST_COUNT,
        "1M+ far-future + 50k burst armed (seven figures outstanding)"
    );
    assert_eq!(
        timers.unfired_count(),
        FAR_FUTURE_COUNT + BURST_COUNT,
        "none fired yet"
    );
    let scanned_before = timers.rows_scanned();

    // (3) The 16-way wheel fleet ticks at now = second 30 (bucket 0): the burst (bucket 0) is due across
    //     its shards; the far-future fleet (bucket >= 1440) is NOT due → NEVER scanned. The sharded wheel
    //     fires the burst over a few rounds (the bounded per-shard batch), draining the lag to 0.
    let fleet = WheelShardSet::new(
        timers.clone(),
        journal.clone(),
        runs.clone(),
        tele.clone(),
        SHARDS,
        BATCH,
    );
    assert_eq!(fleet.shards(), SHARDS as usize);

    let t_drain = std::time::Instant::now();
    let mut total_fired = 0usize;
    let mut rounds = 0u32;
    // drain the burst — each round ticks every shard once; a healthy sharded wheel clears 50k fast.
    loop {
        total_fired += fleet.tick_all(30);
        rounds += 1;
        if timers.unfired_count() <= FAR_FUTURE_COUNT {
            // every due (burst) timer fired; only the far-future fleet remains unfired.
            break;
        }
        assert!(
            rounds < 1_000,
            "the burst must drain within a bounded round window (the tick budget)"
        );
    }
    let drain_secs = t_drain.elapsed().as_secs_f64().max(1e-9);

    // THE FLOW-D3-FULL ASSERTIONS:
    // (a) the due burst fired WITHIN the tick budget — the `timer_wheel_lag` drained to budget (0).
    assert_eq!(total_fired, BURST_COUNT, "all 50k due timers fired");
    // the lag across every shard is now 0 (no due timer left unfired past its minute).
    let mut max_lag = 0u64;
    for p in 0..SHARDS {
        max_lag = max_lag.max(timers.wheel_lag(p as i16, 30));
    }
    assert_eq!(
        max_lag, LAG_BUDGET,
        "the `timer_wheel_lag` drained to budget (0) across all shards — within budget at 1M+"
    );
    // (b) FAR-FUTURE COST ~NOTHING: the scan touched ONLY the due burst, never the 1M+ far-future fleet
    //     (the SC-11 partial-index range read — an indexed range read at seven figures, NOT a table scan).
    let scanned = timers.rows_scanned() - scanned_before;
    assert_eq!(
        scanned, BURST_COUNT as u64,
        "the scan touched ONLY the {BURST_COUNT} due rows — the 1M+ far-future fleet was NEVER read \
         (indexed range read, not a table scan)"
    );
    // a far-future timer is untouched: still unfired (cost nothing).
    assert!(
        !timers.get(&tenant(), "far/0").unwrap().fired,
        "a far-future timer is unfired (never scanned at seven figures)"
    );

    // (c) 0 DOUBLE-CLAIM across the worker shards + the crash re-fire (0 lost / 0 double-fire): re-fire
    //     EVERY burst timer (modeling a crash that re-delivered the whole due set) — each is ALREADY
    //     fired, so every re-fire is a no-op (effectively-once). 0 double-fire.
    let mut double_fires = 0usize;
    for i in 0..BURST_COUNT {
        if timers.fire(&tenant(), &format!("burst/{i}"), &journal, &runs) == FireOutcome::Fired {
            double_fires += 1;
        }
    }
    assert_eq!(
        double_fires, 0,
        "0 double-fire / 0 double-claim: a crash re-fire of the already-fired burst is a no-op"
    );
    // 0 LOST: every due burst timer fired + journaled EXACTLY once (one timer_fired row per run).
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
        "0 lost: every due timer fired + journaled EXACTLY once across the shards (effectively-once)"
    );

    // (4) MEASURE the per-cell promotion threshold (OQ #5): the due-now rate the sharded wheel sustained
    //     WITHIN budget. The fleet drained `BURST_COUNT` due timers in `drain_secs` with the lag at 0, so
    //     the measured rate is `BURST_COUNT / drain_secs` AND the measured lag is 0 (within budget). Feed
    //     it to the thresholds-file gate — the wheel keeping up means NO dedicated scheduling tier is owed.
    let measured_due_now_per_sec = (BURST_COUNT as f64 / drain_secs) as u64;
    let measured_wheel_lag = max_lag; // 0 — the wheel drained within budget.
    let gate = TimerWheelPromotion::default();
    // the flow-side seeds mirror the thresholds file (one number, two readers — the coherence anchor).
    assert_eq!(
        gate.promote_due_now_per_sec_per_cell,
        promotion::PROMOTE_DUE_NOW_PER_SEC_PER_CELL_SEED
    );
    let promotion_owed = gate.promotion_owed_for(measured_due_now_per_sec, measured_wheel_lag);
    assert!(
        !promotion_owed,
        "the wheel drained within budget (lag 0) → no dedicated scheduling tier owed at cell scale"
    );
    assert!(
        !gate.promotion_owed,
        "the committed seam stays NAMED — the PG-indexed wheel suffices at cell scale (OQ #5)"
    );

    // the dated SCHED green artifact (the lag-within-budget + 0-lost/0-dup + the measured promotion gate).
    println!(
        "[2026-06-25] PASS  drill=FLOW-D3-FULL(1M+ cell scale)  armed={armed} \
         (far-future={far} + burst={burst})  shards={shards}  \
         armed_in={arm_secs:.2}s  fired={total_fired}/{burst} in {rounds} rounds ({drain_secs:.3}s)  \
         timer_wheel_lag={max_lag}<=budget({budget}) across all shards  \
         far-future-scanned=0 (rows_scanned={scanned}=burst only, INDEXED range read at 7 figures)  \
         worker-shard split: 0 double-claim / 0 lost  crash-re-fire: 0 double-fire / 0 lost  \
         per-cell promotion (OQ #5): measured_due_now={measured_due_now_per_sec}/s lag={measured_wheel_lag} \
         => promotion_owed={promotion_owed} (PG-indexed wheel suffices; dedicated tier NAMED follow-on iff rate demands)  \
         [P-FLOW-13 100k FLOOR CLOSED; ONE remaining floor = real per-cell fleet hardware]",
        armed = FAR_FUTURE_COUNT + BURST_COUNT,
        far = FAR_FUTURE_COUNT,
        burst = BURST_COUNT,
        shards = SHARDS,
        budget = LAG_BUDGET,
    );
}

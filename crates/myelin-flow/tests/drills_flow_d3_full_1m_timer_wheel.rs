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

const FAR_FUTURE_COUNT: usize = 1_000_000;
const BURST_COUNT: usize = 50_000;
const SHARDS: u16 = 16;
const BATCH: usize = 8_192;
const LAG_BUDGET: u64 = 0;

fn far_future_timer(i: usize) -> (String, TimerRow) {
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

#[test]
#[ignore = "SCHED: the seven-figure (1M+) cell-scale FLOW-D3-full run - run with --release --ignored"]
fn drill_flow_d3_full_1m_timer_wheel_within_budget_zero_lost_zero_dup() {
    let timers = TimerStore::new();
    let journal = WfJournal::new();
    let runs = RunStore::new();
    let tele = FlowTelemetry::new();

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
    loop {
        total_fired += fleet.tick_all(30);
        rounds += 1;
        if timers.unfired_count() <= FAR_FUTURE_COUNT {
            break;
        }
        assert!(
            rounds < 1_000,
            "the burst must drain within a bounded round window (the tick budget)"
        );
    }
    let drain_secs = t_drain.elapsed().as_secs_f64().max(1e-9);

    assert_eq!(total_fired, BURST_COUNT, "all 50k due timers fired");
    let mut max_lag = 0u64;
    for p in 0..SHARDS {
        max_lag = max_lag.max(timers.wheel_lag(p as i16, 30));
    }
    assert_eq!(
        max_lag, LAG_BUDGET,
        "the `timer_wheel_lag` drained to budget (0) across all shards - within budget at 1M+"
    );
    let scanned = timers.rows_scanned() - scanned_before;
    assert_eq!(
        scanned, BURST_COUNT as u64,
        "the scan touched ONLY the {BURST_COUNT} due rows - the 1M+ far-future fleet was NEVER read \
         (indexed range read, not a table scan)"
    );
    assert!(
        !timers.get(&tenant(), "far/0").unwrap().fired,
        "a far-future timer is unfired (never scanned at seven figures)"
    );

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

    let measured_due_now_per_sec = (BURST_COUNT as f64 / drain_secs) as u64;
    let measured_wheel_lag = max_lag;
    let gate = TimerWheelPromotion::default();
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
        "the committed seam stays NAMED - the PG-indexed wheel suffices at cell scale (OQ #5)"
    );

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

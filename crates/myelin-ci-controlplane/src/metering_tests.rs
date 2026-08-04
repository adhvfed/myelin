use super::*;
use myelin_flow::{BudgetGate, Wallet};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId::from_token("01J0ACME")
}

#[test]
fn meter_taxonomy_is_the_frozen_cost_event_set() {
    assert_eq!(
        Meter::ALL.len(),
        5,
        "exactly five resource-second dimensions"
    );
    assert_eq!(Meter::CpuSeconds.token(), "cpu_seconds");
    assert_eq!(Meter::MemGbSeconds.token(), "mem_gb_seconds");
    assert_eq!(Meter::GpuSeconds.token(), "gpu_seconds");
    assert_eq!(Meter::StorageGbHours.token(), "storage_gb_hours");
    assert_eq!(Meter::EgressGb.token(), "egress_gb");
    for m in Meter::ALL {
        assert_eq!(Meter::from_token(m.token()), Some(m), "round-trips");
    }
    assert_eq!(
        Meter::from_token("disk_iops"),
        None,
        "an unknown meter token is rejected (the CHECK constraint set is frozen), never coerced"
    );
}

#[test]
fn cost_kind_tokens_are_ci_and_agent() {
    assert_eq!(CostKind::Ci.token(), "ci");
    assert_eq!(CostKind::Agent.token(), "agent");
}

#[test]
fn cost_event_rows_carry_distinct_wholesale_and_markup_columns() {
    let markup = FlatBpsMarkup::new(2_000);
    let samples = [
        (Meter::CpuSeconds, 120u64, MicroUsd(240)),
        (Meter::MemGbSeconds, 64, MicroUsd(80)),
        (Meter::EgressGb, 2, MicroUsd(50)),
    ];
    let rows = meter_resource_seconds(
        &tenant(),
        "ci/run/7",
        "ci/job/build",
        CostKind::Ci,
        &samples,
        &markup,
    );

    assert_eq!(rows.len(), 3, "one cost_event per metered unit");

    let cpu = &rows[0];
    assert_eq!(cpu.meter, Meter::CpuSeconds);
    assert_eq!(
        cpu.amount, 120,
        "the integer resource-second quantity (never a float)"
    );
    assert_eq!(
        cpu.wholesale,
        MicroUsd(240),
        "the honest wholesale column (CI's)"
    );
    assert_eq!(
        cpu.markup,
        MicroUsd(48),
        "the markup column (Commercial's, R-2 seam)"
    );
    assert_ne!(
        cpu.wholesale, cpu.markup,
        "wholesale ≠ markup - two distinct columns"
    );
    assert_eq!(
        cpu.billed(),
        Some(MicroUsd(288)),
        "billed = wholesale + markup"
    );
    assert_eq!(cpu.kind, CostKind::Ci);

    let agent_rows = meter_resource_seconds(
        &tenant(),
        "agent/run/9",
        "agent/job/compute",
        CostKind::Agent,
        &samples,
        &markup,
    );
    assert_eq!(agent_rows[0].kind, CostKind::Agent);
    assert_eq!(
        agent_rows[0].wholesale, cpu.wholesale,
        "the SAME wholesale meter fronts CI + agent (directly comparable, X-6)"
    );
}

#[test]
fn flat_bps_markup_is_integer_and_overflow_safe() {
    let m = FlatBpsMarkup::new(2_000);
    assert_eq!(
        m.markup_for(Meter::CpuSeconds, 1, MicroUsd(100)),
        MicroUsd(20)
    );
    assert_eq!(
        m.markup_for(Meter::CpuSeconds, 1, MicroUsd(99)),
        MicroUsd(19)
    );
    let big = m.markup_for(Meter::CpuSeconds, 1, MicroUsd(u64::MAX));
    assert!(
        big.0 > 0,
        "a large wholesale marks up without overflowing to 0"
    );
}

#[test]
fn ci_meter_reserves_and_settles_resource_seconds() {
    let gate = BudgetGate::new(Wallet::new(MicroUsd(1_000)));
    let meter = CiMeter::new(&gate, FlatBpsMarkup::new(2_000));
    let run = myelin_storage::reserve_settle::RunId::new("ci/run/1");

    meter
        .reserve_budget(&tenant(), &run, MicroUsd(400))
        .expect("a funded reserve admits + begins (in-flight)");
    assert_eq!(meter.balance(), MicroUsd(600), "reserved 400");

    let rows = meter
        .settle_budget(
            &tenant(),
            &run,
            "ci/run/1",
            "ci/job/build",
            CostKind::Ci,
            &[(Meter::CpuSeconds, 100, MicroUsd(200))],
        )
        .expect("a settle records the cost events + refunds the over-reservation");
    assert_eq!(rows.len(), 1, "one cost_event per metered unit");
    assert_eq!(rows[0].billed(), Some(MicroUsd(240)));
    assert_eq!(
        meter.balance(),
        MicroUsd(760),
        "only the billed 240 is drawn"
    );
    assert_eq!(
        meter.inflight_interrupt_count(),
        0,
        "0 interrupts (never interrupt in flight)"
    );
}

#[test]
fn reserve_against_exhausted_wallet_refuses_the_start() {
    let gate = BudgetGate::new(Wallet::new(MicroUsd::ZERO));
    let meter = CiMeter::new(&gate, FlatBpsMarkup::new(2_000));
    let run = myelin_storage::reserve_settle::RunId::new("ci/run/broke");
    let err = meter
        .reserve_budget(&tenant(), &run, MicroUsd(100))
        .expect_err("an exhausted wallet refuses the start");
    assert!(
        matches!(err, myelin_flow::BudgetError::Refused { .. }),
        "the refusal is the loud no-balance → no-start floor, got {err:?}"
    );
}

#[test]
fn in_flight_run_is_never_interrupted_by_exhaustion() {
    let gate = BudgetGate::new(Wallet::new(MicroUsd(100)));
    let meter = CiMeter::new(&gate, FlatBpsMarkup::new(0));
    let run_a = myelin_storage::reserve_settle::RunId::new("ci/run/a");
    let run_b = myelin_storage::reserve_settle::RunId::new("ci/run/b");

    meter
        .reserve_budget(&tenant(), &run_a, MicroUsd(100))
        .unwrap();
    assert!(matches!(
        meter.reserve_budget(&tenant(), &run_b, MicroUsd(50)),
        Err(myelin_flow::BudgetError::Refused { .. })
    ));
    meter
        .settle_budget(
            &tenant(),
            &run_a,
            "ci/run/a",
            "ci/job/a",
            CostKind::Ci,
            &[(Meter::CpuSeconds, 100, MicroUsd(100))],
        )
        .expect("the in-flight run settles normally");
    assert_eq!(
        meter.inflight_interrupt_count(),
        0,
        "the headline zero: 0 in-flight interrupts"
    );
}

#[test]
fn metered_units_carry_the_split_through_unchanged() {
    let rows = meter_resource_seconds(
        &tenant(),
        "r",
        "j",
        CostKind::Ci,
        &[(Meter::CpuSeconds, 10, MicroUsd(100))],
        &FlatBpsMarkup::new(5_000),
    );
    let units = metered_units_for(&rows);
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].unit, "cpu_seconds");
    assert_eq!(units[0].wholesale, MicroUsd(100));
    assert_eq!(units[0].markup, MicroUsd(50));
}

#[test]
fn metered_resource_converts_and_bills() {
    let r = MeteredResource {
        meter: Meter::GpuSeconds,
        amount: 7,
        wholesale: MicroUsd(120),
        markup: MicroUsd(30),
    };
    let unit = r.to_metered_unit();
    assert_eq!(unit.unit, "gpu_seconds");
    assert_eq!(unit.wholesale, MicroUsd(120));
    assert_eq!(unit.markup, MicroUsd(30));
    assert_eq!(
        r.billed(),
        Some(MicroUsd(150)),
        "billed = wholesale + markup"
    );
    let overflow = MeteredResource {
        meter: Meter::CpuSeconds,
        amount: 1,
        wholesale: MicroUsd(u64::MAX),
        markup: MicroUsd(1),
    };
    assert_eq!(overflow.billed(), None);
}

#[test]
fn ci_d5_reserve_settle_parity_drill_is_green() {
    let samples = [
        (Meter::CpuSeconds, 120u64, MicroUsd(200)),
        (Meter::MemGbSeconds, 64, MicroUsd(40)),
    ];
    let before = FlatBpsMarkup::new(2_000);
    let after = FlatBpsMarkup::new(3_500);

    let signal =
        reserve_settle_parity_drill(&tenant(), MicroUsd(300), 4, &samples, &before, &after);

    assert!(
        signal.is_green(),
        "the CI-D5 drill must be GREEN: {signal:?}"
    );
    assert!(
        signal.ci_refused_when_exhausted,
        "the CI run refused-start past exhaustion"
    );
    assert!(
        signal.agent_refused_when_exhausted,
        "the agent run refused-start past exhaustion"
    );
    assert_eq!(signal.starts_past_exhaustion, 0, "0 over-exhaustion starts");
    assert_eq!(signal.inflight_interrupt_count, 0, "0 in-flight interrupts");
    assert_eq!(signal.cost_events_recorded, 8);
    assert_eq!(signal.metered_units, 8);
    assert_eq!(
        signal.ci_cost_events, 4,
        "CI runs metered into the shared path"
    );
    assert_eq!(
        signal.agent_cost_events, 4,
        "agent runs metered into the SAME path"
    );
    assert_eq!(
        signal.ci_cost_events + signal.agent_cost_events,
        signal.cost_events_recorded
    );
    assert_ne!(
        signal.wholesale_total, signal.markup_total_before,
        "wholesale ≠ markup"
    );
    assert_ne!(
        signal.markup_total_before, signal.markup_total_after,
        "the pricing change re-prices the markup column"
    );
    assert_eq!(
        signal.wholesale_total,
        MicroUsd(960),
        "the wholesale column is stable"
    );
    assert_eq!(signal.markup_total_before, MicroUsd(192));
    assert_eq!(signal.markup_total_after, MicroUsd(336));
}

#[test]
fn run_kind_alternates_ci_even_agent_odd() {
    assert_eq!(super::run_kind(0), CostKind::Ci);
    assert_eq!(super::run_kind(1), CostKind::Agent);
    assert_eq!(super::run_kind(2), CostKind::Ci);
    assert_eq!(super::run_kind(3), CostKind::Agent);
}

#[test]
fn over_exhaustion_starts_counts_each_non_refusal() {
    assert_eq!(
        super::count_over_exhaustion_starts(true, true),
        0,
        "both refused = 0 starts (green)"
    );
    assert_eq!(
        super::count_over_exhaustion_starts(false, true),
        1,
        "the CI run started past exhaustion"
    );
    assert_eq!(
        super::count_over_exhaustion_starts(true, false),
        1,
        "the agent run started past exhaustion"
    );
    assert_eq!(
        super::count_over_exhaustion_starts(false, false),
        2,
        "both started past exhaustion"
    );
}

#[test]
fn a_red_parity_signal_is_not_green() {
    let base = ReserveSettleParitySignal {
        ci_refused_when_exhausted: true,
        agent_refused_when_exhausted: true,
        starts_past_exhaustion: 0,
        inflight_interrupt_count: 0,
        cost_events_recorded: 8,
        ci_cost_events: 4,
        agent_cost_events: 4,
        metered_units: 8,
        wholesale_total: MicroUsd(960),
        markup_total_before: MicroUsd(192),
        markup_total_after: MicroUsd(336),
    };
    assert!(base.is_green(), "the baseline is green");

    let no_ci = ReserveSettleParitySignal {
        ci_cost_events: 0,
        agent_cost_events: 8,
        ..base.clone()
    };
    assert!(!no_ci.is_green(), "no CI run in the shared path reads RED");
    let no_agent = ReserveSettleParitySignal {
        ci_cost_events: 8,
        agent_cost_events: 0,
        ..base.clone()
    };
    assert!(
        !no_agent.is_green(),
        "no agent run in the shared path reads RED"
    );

    let agent_started = ReserveSettleParitySignal {
        agent_refused_when_exhausted: false,
        starts_past_exhaustion: 1,
        ..base.clone()
    };
    assert!(
        !agent_started.is_green(),
        "an agent start past exhaustion reads RED"
    );

    let mismatch = ReserveSettleParitySignal {
        cost_events_recorded: 7,
        ..base.clone()
    };
    assert!(!mismatch.is_green(), "a cost-event mismatch reads RED");

    let conflated = ReserveSettleParitySignal {
        markup_total_before: MicroUsd(960),
        ..base.clone()
    };
    assert!(!conflated.is_green(), "wholesale == markup reads RED");
}

#[test]
fn insert_cost_event_query_matches_the_cost_event_row_model() {
    let q = INSERT_COST_EVENT_QUERY;
    assert!(
        q.contains("INSERT INTO ci_cost_event"),
        "writes the ci_cost_event table (CT-004m: CI-namespaced, distinct from Storage's cost_event)"
    );
    for col in [
        "tenant_id",
        "region",
        "cost_id",
        "run_id",
        "job_id",
        "meter",
        "amount",
        "wholesale_minor_units",
        "markup_minor_units",
        "kind",
    ] {
        assert!(
            q.contains(col),
            "the durable settle writes the {col} column"
        );
    }
    assert!(
        q.contains("wholesale_minor_units") && q.contains("markup_minor_units"),
        "wholesale ≠ markup: the two cost columns are distinct in the durable write"
    );
    assert!(
        q.contains("ON CONFLICT (tenant_id, cost_id) DO NOTHING"),
        "the settle is idempotent on (tenant_id, cost_id) - exactly-once cost recording"
    );
    assert!(
        SELECT_COST_EVENTS_FOR_RUN_QUERY.contains("WHERE tenant_id = $1 AND run_id = $2"),
        "the read-back attributes every metered unit to its (tenant, run)"
    );
}

//! Unit tests for the CI metering module (CI-P17 → P-360, M4): the reserve/settle bookends
//! (refuse-start-on-exhaustion, settle-on-job.done, never interrupt in flight), the `cost_event`
//! integer-minor-units + wholesale ≠ markup invariant, the resource-second meter taxonomy, and the
//! CI-D5 reserve/settle-parity drill (the GATE).

use super::*;
use myelin_flow::{BudgetGate, Wallet};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId::from_token("01J0ACME")
}

/// The frozen meter token set is EXACTLY the `cost_event.meter` CHECK constraint set — no sixth meter,
/// round-trips through the token, parses back, and rejects an unknown token (a corrupt write).
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

/// The kind tokens are exactly the `cost_event.kind` CHECK set — `ci` | `agent` (UNIFY / X-6).
#[test]
fn cost_kind_tokens_are_ci_and_agent() {
    assert_eq!(CostKind::Ci.token(), "ci");
    assert_eq!(CostKind::Agent.token(), "agent");
}

/// **`cost_event` rows record wholesale + markup as DISTINCT integer-minor-units columns** — one row
/// per metered unit, the billed total is `wholesale + markup`, and a float cost is unrepresentable.
#[test]
fn cost_event_rows_carry_distinct_wholesale_and_markup_columns() {
    // 20% flat markup (the test stand-in for Commercial's R-2 pricing table).
    let markup = FlatBpsMarkup::new(2_000);
    let samples = [
        (Meter::CpuSeconds, 120u64, MinorUnits(240)),
        (Meter::MemGbSeconds, 64, MinorUnits(80)),
        (Meter::EgressGb, 2, MinorUnits(50)),
    ];
    let rows = meter_resource_seconds(
        &tenant(),
        "ci/run/7",
        "ci/job/build",
        CostKind::Ci,
        &samples,
        &markup,
    );

    // ONE row per metered unit (the cost_events_per_unit == 1 invariant).
    assert_eq!(rows.len(), 3, "one cost_event per metered unit");

    // The cpu row: wholesale 240, markup 20% of 240 = 48 — DISTINCT columns, never conflated.
    let cpu = &rows[0];
    assert_eq!(cpu.meter, Meter::CpuSeconds);
    assert_eq!(
        cpu.amount, 120,
        "the integer resource-second quantity (never a float)"
    );
    assert_eq!(
        cpu.wholesale,
        MinorUnits(240),
        "the honest wholesale column (CI's)"
    );
    assert_eq!(
        cpu.markup,
        MinorUnits(48),
        "the markup column (Commercial's, R-2 seam)"
    );
    assert_ne!(
        cpu.wholesale, cpu.markup,
        "wholesale ≠ markup — two distinct columns"
    );
    assert_eq!(
        cpu.billed(),
        Some(MinorUnits(288)),
        "billed = wholesale + markup"
    );
    assert_eq!(cpu.kind, CostKind::Ci);

    // The agent kind fronts the SAME schema (UNIFY / X-6).
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

/// The flat-bps markup is an integer floor of `wholesale * bps / 10_000` — never a fractional cost,
/// and a huge wholesale does not overflow before the divide (u128 widening).
#[test]
fn flat_bps_markup_is_integer_and_overflow_safe() {
    let m = FlatBpsMarkup::new(2_000); // 20%
    assert_eq!(
        m.markup_for(Meter::CpuSeconds, 1, MinorUnits(100)),
        MinorUnits(20)
    );
    // Integer floor: 99 * 20% = 19.8 → 19.
    assert_eq!(
        m.markup_for(Meter::CpuSeconds, 1, MinorUnits(99)),
        MinorUnits(19)
    );
    // A huge wholesale * bps would overflow u64 if not widened — proves the u128 path.
    let big = m.markup_for(Meter::CpuSeconds, 1, MinorUnits(u64::MAX));
    assert!(
        big.0 > 0,
        "a large wholesale marks up without overflowing to 0"
    );
}

/// **The reserve/settle bookend at the resource-second grain (refuse-start, settle-on-job.done).** A
/// funded CI run reserves, settles its resource-seconds (recording cost_event rows), and the wallet is
/// debited only the billed amount.
#[test]
fn ci_meter_reserves_and_settles_resource_seconds() {
    let gate = BudgetGate::new(Wallet::new(MinorUnits(1_000)));
    let meter = CiMeter::new(&gate, FlatBpsMarkup::new(2_000));
    let run = myelin_storage::reserve_settle::RunId::new("ci/run/1");

    // reserve_budget: reserve 400 (the resource-second upper bound) → wallet 600, in-flight.
    meter
        .reserve_budget(&tenant(), &run, MinorUnits(400))
        .expect("a funded reserve admits + begins (in-flight)");
    assert_eq!(meter.balance(), MinorUnits(600), "reserved 400");

    // settle_budget: bill cpu 200 wholesale + 40 markup = 240 → refund 400 − 240 = 160.
    let rows = meter
        .settle_budget(
            &tenant(),
            &run,
            "ci/run/1",
            "ci/job/build",
            CostKind::Ci,
            &[(Meter::CpuSeconds, 100, MinorUnits(200))],
        )
        .expect("a settle records the cost events + refunds the over-reservation");
    assert_eq!(rows.len(), 1, "one cost_event per metered unit");
    assert_eq!(rows[0].billed(), Some(MinorUnits(240)));
    // wallet: 600 + refund(400 − 240 = 160) = 760.
    assert_eq!(
        meter.balance(),
        MinorUnits(760),
        "only the billed 240 is drawn"
    );
    assert_eq!(
        meter.inflight_interrupt_count(),
        0,
        "0 interrupts (never interrupt in flight)"
    );
}

/// **refuse-to-start on exhaustion — the run NEVER starts (arch §6, the runaway self-limiter).** A
/// reserve against an empty wallet is REFUSED loudly; nothing is reserved.
#[test]
fn reserve_against_exhausted_wallet_refuses_the_start() {
    let gate = BudgetGate::new(Wallet::new(MinorUnits::ZERO)); // exhausted.
    let meter = CiMeter::new(&gate, FlatBpsMarkup::new(2_000));
    let run = myelin_storage::reserve_settle::RunId::new("ci/run/broke");
    let err = meter
        .reserve_budget(&tenant(), &run, MinorUnits(100))
        .expect_err("an exhausted wallet refuses the start");
    assert!(
        matches!(err, myelin_flow::BudgetError::Refused { .. }),
        "the refusal is the loud no-balance → no-start floor, got {err:?}"
    );
}

/// **Never interrupt in flight — a depleting wallet refuses the NEXT run but never tears down the
/// running one.** Run A reserves the whole wallet (in-flight); run B is refused; A still settles.
#[test]
fn in_flight_run_is_never_interrupted_by_exhaustion() {
    let gate = BudgetGate::new(Wallet::new(MinorUnits(100)));
    let meter = CiMeter::new(&gate, FlatBpsMarkup::new(0));
    let run_a = myelin_storage::reserve_settle::RunId::new("ci/run/a");
    let run_b = myelin_storage::reserve_settle::RunId::new("ci/run/b");

    // A reserves the whole wallet (in-flight, NEVER interrupted).
    meter
        .reserve_budget(&tenant(), &run_a, MinorUnits(100))
        .unwrap();
    // B is refused — the wallet is exhausted.
    assert!(matches!(
        meter.reserve_budget(&tenant(), &run_b, MinorUnits(50)),
        Err(myelin_flow::BudgetError::Refused { .. })
    ));
    // A still settles (it was never torn down) — bill 100, no refund.
    meter
        .settle_budget(
            &tenant(),
            &run_a,
            "ci/run/a",
            "ci/job/a",
            CostKind::Ci,
            &[(Meter::CpuSeconds, 100, MinorUnits(100))],
        )
        .expect("the in-flight run settles normally");
    assert_eq!(
        meter.inflight_interrupt_count(),
        0,
        "the headline zero: 0 in-flight interrupts"
    );
}

/// `metered_units_for` carries the wholesale + markup split THROUGH unchanged so the engine ledger
/// records the same two distinct columns.
#[test]
fn metered_units_carry_the_split_through_unchanged() {
    let rows = meter_resource_seconds(
        &tenant(),
        "r",
        "j",
        CostKind::Ci,
        &[(Meter::CpuSeconds, 10, MinorUnits(100))],
        &FlatBpsMarkup::new(5_000), // 50%
    );
    let units = metered_units_for(&rows);
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].unit, "cpu_seconds");
    assert_eq!(units[0].wholesale, MinorUnits(100));
    assert_eq!(units[0].markup, MinorUnits(50));
}

/// [`MeteredResource`] converts to the engine [`MeteredUnit`] carrying the meter token + the split,
/// and `billed` sums wholesale + markup (checked) — exercises the public `MeteredResource` API so it
/// is not a silent dead value.
#[test]
fn metered_resource_converts_and_bills() {
    let r = MeteredResource {
        meter: Meter::GpuSeconds,
        amount: 7,
        wholesale: MinorUnits(120),
        markup: MinorUnits(30),
    };
    let unit = r.to_metered_unit();
    assert_eq!(unit.unit, "gpu_seconds");
    assert_eq!(unit.wholesale, MinorUnits(120));
    assert_eq!(unit.markup, MinorUnits(30));
    assert_eq!(
        r.billed(),
        Some(MinorUnits(150)),
        "billed = wholesale + markup"
    );
    // An overflowing billed is a loud None (integer minor-units, never a silent wrap).
    let overflow = MeteredResource {
        meter: Meter::CpuSeconds,
        amount: 1,
        wholesale: MinorUnits(u64::MAX),
        markup: MinorUnits(1),
    };
    assert_eq!(overflow.billed(), None);
}

/// **THE CI-D5 GATE: reserve/settle parity CI ↔ agent.** Exhaust ONE wallet; both a CI run AND an
/// agent run refuse-start (the parity); 0 starts past exhaustion; 0 in-flight interrupts; one
/// cost_event per metered unit; and a pricing change re-prices the markup column while the wholesale
/// column + the 0-over-exhaustion property are STABLE.
#[test]
fn ci_d5_reserve_settle_parity_drill_is_green() {
    // The wallet affords exactly 4 runs of 300; samples are 2 dimensions so wholesale ≠ markup is
    // exercised across meters.
    let samples = [
        (Meter::CpuSeconds, 120u64, MinorUnits(200)),
        (Meter::MemGbSeconds, 64, MinorUnits(40)),
    ];
    let before = FlatBpsMarkup::new(2_000); // 20% markup
    let after = FlatBpsMarkup::new(3_500); // a PRICING CHANGE — 35% markup

    let signal =
        reserve_settle_parity_drill(&tenant(), MinorUnits(300), 4, &samples, &before, &after);

    assert!(
        signal.is_green(),
        "the CI-D5 drill must be GREEN: {signal:?}"
    );
    // The parity: BOTH kinds refuse-start when exhausted.
    assert!(
        signal.ci_refused_when_exhausted,
        "the CI run refused-start past exhaustion"
    );
    assert!(
        signal.agent_refused_when_exhausted,
        "the agent run refused-start past exhaustion"
    );
    // 0 starts past exhaustion (the headline number).
    assert_eq!(signal.starts_past_exhaustion, 0, "0 over-exhaustion starts");
    assert_eq!(signal.inflight_interrupt_count, 0, "0 in-flight interrupts");
    // One cost_event per metered unit: 4 runs * 2 dimensions = 8.
    assert_eq!(signal.cost_events_recorded, 8);
    assert_eq!(signal.metered_units, 8);
    // BOTH kinds participated in the SAME metering path (the unified meter — runs 0,2 = ci; 1,3 =
    // agent → 2 runs each * 2 dimensions = 4 events each).
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
    // wholesale ≠ markup, and the pricing change moves the markup but NOT the wholesale.
    assert_ne!(
        signal.wholesale_total, signal.markup_total_before,
        "wholesale ≠ markup"
    );
    assert_ne!(
        signal.markup_total_before, signal.markup_total_after,
        "the pricing change re-prices the markup column"
    );
    // wholesale is the SAME basis under both pricings (the pricing change touches ONLY markup):
    // 4 runs * (200 + 40) = 960 wholesale.
    assert_eq!(
        signal.wholesale_total,
        MinorUnits(960),
        "the wholesale column is stable"
    );
    // markup before: 4 * (200*20% + 40*20%) = 4 * (40 + 8) = 192.
    assert_eq!(signal.markup_total_before, MinorUnits(192));
    // markup after: 4 * (200*35% + 40*35%) = 4 * (70 + 14) = 336.
    assert_eq!(signal.markup_total_after, MinorUnits(336));
}

/// The drill's `run_kind` alternates CI (even) / agent (odd) so both kinds meter into the same path
/// (the unified-meter property). Pins the alternation so a `%`→`/` or `==`→`!=` regression (which
/// would make every run the SAME kind — breaking the parity coverage) is caught.
#[test]
fn run_kind_alternates_ci_even_agent_odd() {
    assert_eq!(super::run_kind(0), CostKind::Ci);
    assert_eq!(super::run_kind(1), CostKind::Agent);
    assert_eq!(super::run_kind(2), CostKind::Ci);
    assert_eq!(super::run_kind(3), CostKind::Agent);
}

/// `count_over_exhaustion_starts` counts the kinds that did NOT refuse-start (the RED metric): both
/// refused → 0 (green), one started → 1, both started → 2. Pins the `+` (not `-`/`*`) so a regression
/// that under-counts a parity breach is caught.
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

/// A RED parity signal is correctly classified NOT green — proving `is_green` is not vacuously true
/// (a start past exhaustion, or one kind not refusing, must read RED).
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
        wholesale_total: MinorUnits(960),
        markup_total_before: MinorUnits(192),
        markup_total_after: MinorUnits(336),
    };
    assert!(base.is_green(), "the baseline is green");

    // No CI run participated (only agent metered) → NOT the parity, reads RED.
    let no_ci = ReserveSettleParitySignal {
        ci_cost_events: 0,
        agent_cost_events: 8,
        ..base.clone()
    };
    assert!(!no_ci.is_green(), "no CI run in the shared path reads RED");
    // No agent run participated → reads RED.
    let no_agent = ReserveSettleParitySignal {
        ci_cost_events: 8,
        agent_cost_events: 0,
        ..base.clone()
    };
    assert!(
        !no_agent.is_green(),
        "no agent run in the shared path reads RED"
    );

    // The agent kind did NOT refuse — a parity breach (an over-exhaustion agent start).
    let agent_started = ReserveSettleParitySignal {
        agent_refused_when_exhausted: false,
        starts_past_exhaustion: 1,
        ..base.clone()
    };
    assert!(
        !agent_started.is_green(),
        "an agent start past exhaustion reads RED"
    );

    // A cost-event mismatch (a metered unit recorded no event) reads RED.
    let mismatch = ReserveSettleParitySignal {
        cost_events_recorded: 7,
        ..base.clone()
    };
    assert!(!mismatch.is_green(), "a cost-event mismatch reads RED");

    // wholesale == markup (the two columns conflated) reads RED.
    let conflated = ReserveSettleParitySignal {
        markup_total_before: MinorUnits(960),
        ..base.clone()
    };
    assert!(!conflated.is_green(), "wholesale == markup reads RED");
}

/// **Model ↔ SQL drift guard for the durable `cost_event` settle (CT-004).** The durable
/// [`INSERT_COST_EVENT_QUERY`] MUST write exactly the columns the in-memory [`CostEventRow`] model
/// mirrors, keep wholesale + markup as TWO distinct columns (the §8 invariant — never one conflated
/// number), and be exactly-once on `(tenant_id, cost_id)` (`ON CONFLICT … DO NOTHING`). If the model
/// row gains/loses a billed dimension the constant must move in lockstep — this fails loud otherwise.
#[test]
fn insert_cost_event_query_matches_the_cost_event_row_model() {
    let q = INSERT_COST_EVENT_QUERY;
    assert!(
        q.contains("INSERT INTO cost_event"),
        "writes the cost_event table"
    );
    // Every CostEventRow field has its column (run/job attribution + the two distinct cost columns).
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
    // wholesale + markup are SEPARATE columns (never conflated — the arch 02 §8 invariant).
    assert!(
        q.contains("wholesale_minor_units") && q.contains("markup_minor_units"),
        "wholesale ≠ markup: the two cost columns are distinct in the durable write"
    );
    // Exactly-once settle: a re-delivered job.done records the same cost_id ONCE (double-effect = 0).
    assert!(
        q.contains("ON CONFLICT (tenant_id, cost_id) DO NOTHING"),
        "the settle is idempotent on (tenant_id, cost_id) — exactly-once cost recording"
    );
    // The read-back attributes to the producing run.
    assert!(
        SELECT_COST_EVENTS_FOR_RUN_QUERY.contains("WHERE tenant_id = $1 AND run_id = $2"),
        "the read-back attributes every metered unit to its (tenant, run)"
    );
}

use myelin_flow::{
    epoch_minute, run_state, sla_timer_id, trigger_stale_timer_id, ArmOutcome, DisarmOutcome,
    FireOutcome, FlowTelemetry, ReArmOutcome, RunRow, RunStore, SlaTimerCall, TimerRow, TimerStore,
    TimerWheel, WfJournal,
};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}

fn sla_timer(timer_id: &str, run_id: &str, fire_at: i64) -> TimerRow {
    TimerRow {
        tenant: tenant(),
        region: region(),
        timer_id: timer_id.into(),
        run_id: Some(run_id.into()),
        command_id: format!("issues.sla:{timer_id}"),
        fire_at,
        bucket: epoch_minute(fire_at),
        fired: false,
        partition: 0,
    }
}

#[test]
fn provider_re_arm_is_one_row_update_of_fire_at_and_bucket() {
    let store = TimerStore::new();
    assert_eq!(
        store.arm(sla_timer("sla/issue-7", "R-sla-7", 14_400)),
        ArmOutcome::Armed
    );
    assert_eq!(store.get(&tenant(), "sla/issue-7").unwrap().bucket, 240);

    assert_eq!(
        store.re_arm(&tenant(), "sla/issue-7", 28_800),
        ReArmOutcome::ReArmed
    );
    let row = store.get(&tenant(), "sla/issue-7").unwrap();
    assert_eq!(
        row.fire_at, 28_800,
        "PROVIDER: fire_at slid forward (the row update)"
    );
    assert_eq!(
        row.bucket,
        epoch_minute(28_800),
        "PROVIDER: the derived bucket was recomputed"
    );
    assert_eq!(row.bucket, 480);
    assert!(!row.fired, "PROVIDER: the re-arm re-opened the timer");
    assert_eq!(
        store.armed_count(),
        1,
        "PROVIDER: STILL one row - a re-arm is an UPDATE, no wheel pollution"
    );
}

#[test]
fn provider_disarm_makes_the_timer_never_fire() {
    let store = TimerStore::new();
    let journal = WfJournal::new();
    let runs = RunStore::new();
    let mut run = RunRow::new_runnable(tenant(), region(), "R-sla-7", "issues.sla", 0);
    run.state = run_state::WAITING.into();
    runs.put(run);
    store.arm(sla_timer("sla/issue-7", "R-sla-7", 0));

    assert_eq!(
        store.disarm(&tenant(), "sla/issue-7"),
        DisarmOutcome::Disarmed,
        "PROVIDER: disarm sets fired"
    );
    assert!(
        store.get(&tenant(), "sla/issue-7").unwrap().fired,
        "PROVIDER: the partial-index pivot is set"
    );

    let wheel = TimerWheel::new(
        store.clone(),
        journal.clone(),
        runs.clone(),
        FlowTelemetry::new(),
        0,
        100,
    );
    assert_eq!(
        wheel.tick(60),
        0,
        "PROVIDER: the wheel fires NOTHING for a disarmed timer"
    );
    assert!(
        journal.history_for(&tenant(), "R-sla-7").is_empty(),
        "PROVIDER: no timer_fired - the breach was cancelled"
    );
}

#[test]
fn consumer_issues_sla_slides_the_deadline_then_disarms_on_resolution() {
    let store = TimerStore::new();
    let journal = WfJournal::new();
    let runs = RunStore::new();
    let mut run = RunRow::new_runnable(tenant(), region(), "R-sla-issue-7", "issues.sla", 0);
    run.state = run_state::WAITING.into();
    runs.put(run);

    let timer_id = sla_timer_id("issue-7");
    assert_eq!(
        store.arm(sla_timer(&timer_id, "R-sla-issue-7", 14_400)),
        ArmOutcome::Armed
    );

    let call = SlaTimerCall::new(&store, tenant(), timer_id.clone());
    for (touch, new_fire_at) in [(1, 21_600i64), (2, 28_800), (3, 36_000)] {
        assert_eq!(
            call.re_arm(new_fire_at),
            ReArmOutcome::ReArmed,
            "CONSUMER touch {touch}: the SLA deadline slid forward by a cheap call-site re-arm"
        );
        assert_eq!(
            store.armed_count(),
            1,
            "CONSUMER: STILL one timer on the wheel (re-arm is a row update)"
        );
        assert_eq!(
            store.rows_scanned(),
            0,
            "CONSUMER: the call-site re-arm never scanned the wheel"
        );
    }
    let row = store.get(&tenant(), &timer_id).unwrap();
    assert_eq!(row.fire_at, 36_000);
    assert_eq!(row.bucket, 600);

    let wheel = TimerWheel::new(
        store.clone(),
        journal.clone(),
        runs.clone(),
        FlowTelemetry::new(),
        0,
        100,
    );
    assert_eq!(
        wheel.tick(18_000),
        0,
        "CONSUMER: the breach has not fired (the deadline keeps sliding forward)"
    );

    assert_eq!(
        call.disarm(),
        DisarmOutcome::Disarmed,
        "CONSUMER: resolution disarms the breach"
    );

    assert_eq!(
        wheel.tick(40_000),
        0,
        "CONSUMER: the disarmed breach never fires (the SLA was satisfied)"
    );
    assert!(
        journal.history_for(&tenant(), "R-sla-issue-7").is_empty(),
        "CONSUMER: no breach event - the SLA was met before the deadline"
    );
    assert_eq!(
        runs.get(&tenant(), "R-sla-issue-7").unwrap().state,
        run_state::WAITING,
        "CONSUMER: the SLA workflow was never woken (the breach was disarmed)"
    );
}

#[test]
fn consumer_trigger_stale_after_resets_via_the_same_re_arm_path() {
    let store = TimerStore::new();

    let trig_id = trigger_stale_timer_id("u-42", "issue/acme/proj#7");
    assert_eq!(
        store.arm(sla_timer(&trig_id, "R-trigger", 2_592_000)),
        ArmOutcome::Armed
    );

    let call = SlaTimerCall::new(&store, tenant(), trig_id.clone());
    assert_eq!(
        call.timer_id(),
        "trigger/u-42/issue/acme/proj#7",
        "the Trigger call site keys on owner/subject"
    );

    for new_fire_at in [2_600_000i64, 2_700_000, 2_800_000] {
        assert_eq!(
            call.re_arm(new_fire_at),
            ReArmOutcome::ReArmed,
            "stale_after reset is the same re-arm path"
        );
        assert_eq!(
            store.armed_count(),
            1,
            "STILL one row (the reset is an UPDATE, no wheel pollution)"
        );
    }
    assert_eq!(
        store.rows_scanned(),
        0,
        "no stale_after reset ever scanned the wheel (row-update cost)"
    );
    assert_eq!(
        store.get(&tenant(), &trig_id).unwrap().fire_at,
        2_800_000,
        "the latest stale_after deadline"
    );

    assert_eq!(
        call.disarm(),
        DisarmOutcome::Disarmed,
        "resolution disarms the stale_after timer"
    );
    assert!(
        store.get(&tenant(), &trig_id).unwrap().fired,
        "the disarmed stale_after's partial-index pivot is set"
    );
}

#[test]
fn consumer_an_untouched_sla_fires_its_breach() {
    let store = TimerStore::new();
    let journal = WfJournal::new();
    let runs = RunStore::new();
    let mut run = RunRow::new_runnable(tenant(), region(), "R-sla-stale", "issues.sla", 0);
    run.state = run_state::WAITING.into();
    runs.put(run);
    store.arm(sla_timer("sla/issue-stale", "R-sla-stale", 0));

    let wheel = TimerWheel::new(
        store.clone(),
        journal.clone(),
        runs.clone(),
        FlowTelemetry::new(),
        0,
        100,
    );
    assert_eq!(
        wheel.tick(60),
        1,
        "CONSUMER: the untouched SLA breaches (the breach timer fires)"
    );
    let hist = journal.history_for(&tenant(), "R-sla-stale");
    assert_eq!(
        hist.len(),
        1,
        "CONSUMER: one timer_fired (the breach event)"
    );
    assert_eq!(
        runs.get(&tenant(), "R-sla-stale").unwrap().state,
        run_state::RUNNING,
        "CONSUMER: the SLA workflow woke to handle the breach"
    );
    assert_eq!(
        store.fire(&tenant(), "sla/issue-stale", &journal, &runs),
        FireOutcome::AlreadyFired
    );
}

//! # The CDC pair for the cheap SLA-timer disarm/re-arm — contract 9.3 (PROVIDER half) (P-FLOW-14 → P-210, M2)
//!
//! **Contracts:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 9.3
//! (the durable timer wheel — the cheap disarm/re-arm of a precomputed `fire_at` WITHOUT calendar
//! logic; the disarm/re-arm half is OWNED HERE by P-FLOW-14). Owning architecture:
//! `durable-workflow.md` §6.6 (a re-arm is a row update of `fire_at` + `bucket`; a disarm sets
//! `fired = true` or deletes — millions re-arm at row-update cost, no wheel pollution) + §4.2 (the
//! wheel only ever scans `bucket <= now AND NOT fired`).
//!
//! ## What this pair pins (the PROVIDER ↔ CONSUMER agreement of 9.3's disarm/re-arm half)
//!
//! **9.3 PROVIDER (the workflow timer store) — the agreement the timer wheel guarantees:**
//! - a RE-ARM of a precomputed `fire_at` is a SINGLE row UPDATE ([`TimerStore::re_arm`]): it rewrites
//!   `wf_timer.fire_at` + its derived `bucket = epoch_minute(fire_at)` and re-opens the timer
//!   (`fired = false`). NO new row, NO calendar logic on the wheel — re-arming N timers is N row
//!   updates (the SC-11 row-update cost, not a wheel rescan);
//! - a DISARM ([`TimerStore::disarm`]) sets `fired = true` (the partial-index pivot — the wheel's
//!   `WHERE NOT fired` scan never reads it again), so the timer NEVER fires;
//! - the deterministic `timer_id` is the stable handle the re-arm/disarm targets (no duplicate row).
//!
//! **9.3 CONSUMER (Issues / Trigger — the `stale_after` / SLA-deadline producer) — what it relies on:**
//! - it arms an SLA timer once (`arm`), then SLIDES the deadline forward on every issue touch by a
//!   cheap `re_arm` (NOT a fresh arm — no second row on the wheel), and CANCELS the breach timer on
//!   resolution by a cheap `disarm` (the SLA was met — the timer must never fire). The CONSUMER never
//!   touches the wheel scan; it relies on the provider's promise that a re-arm/disarm is a row op.
//!
//! This pins the provider's promise NOW (the disarm/re-arm row-op surface); the Issues/Trigger CALL
//! SITES are confirmed-and-tested under their real producers in **M3, P-FLOW-17**. The fixture here is
//! the consumer's modeled usage of the promise.

use myelin_flow::{
    epoch_minute, run_state, ArmOutcome, DisarmOutcome, FireOutcome, FlowTelemetry, ReArmOutcome,
    RunRow, RunStore, TimerRow, TimerStore, TimerWheel, WfJournal,
};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}

/// An SLA timer keyed by the deterministic `timer_id` (the stable handle the re-arm/disarm targets).
/// `run_id` is the Issues SLA workflow the breach wakes; `command_id` is the position it journals.
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

/// **PROVIDER side of 9.3 (the re-arm is a single row update of `fire_at` + `bucket`).** The provider's
/// promise: a re-arm rewrites the SAME row's `fire_at` + derived `bucket` (and re-opens it) — no new
/// row, no calendar logic. The wheel depth is unchanged; the bucket is `epoch_minute(new_fire_at)`.
#[test]
fn provider_re_arm_is_one_row_update_of_fire_at_and_bucket() {
    let store = TimerStore::new();
    // arm an SLA timer due in 4 hours (fire_at = 14_400s, bucket 240).
    assert_eq!(store.arm(sla_timer("sla/issue-7", "R-sla-7", 14_400)), ArmOutcome::Armed);
    assert_eq!(store.get(&tenant(), "sla/issue-7").unwrap().bucket, 240);

    // re-arm forward to 8 hours (fire_at = 28_800s, bucket 480) — a SINGLE row update.
    assert_eq!(store.re_arm(&tenant(), "sla/issue-7", 28_800), ReArmOutcome::ReArmed);
    let row = store.get(&tenant(), "sla/issue-7").unwrap();
    assert_eq!(row.fire_at, 28_800, "PROVIDER: fire_at slid forward (the row update)");
    assert_eq!(row.bucket, epoch_minute(28_800), "PROVIDER: the derived bucket was recomputed");
    assert_eq!(row.bucket, 480);
    assert!(!row.fired, "PROVIDER: the re-arm re-opened the timer");
    assert_eq!(store.armed_count(), 1, "PROVIDER: STILL one row — a re-arm is an UPDATE, no wheel pollution");
}

/// **PROVIDER side of 9.3 (a disarm sets `fired = true` — the timer never fires).** The provider's
/// promise: a disarm flips the partial-index pivot so the wheel's `WHERE NOT fired` scan excludes it
/// forever. The wheel fires NOTHING for a disarmed timer.
#[test]
fn provider_disarm_makes_the_timer_never_fire() {
    let store = TimerStore::new();
    let journal = WfJournal::new();
    let runs = RunStore::new();
    let mut run = RunRow::new_runnable(tenant(), region(), "R-sla-7", "issues.sla", 0);
    run.state = run_state::WAITING.into();
    runs.put(run);
    store.arm(sla_timer("sla/issue-7", "R-sla-7", 0)); // due now.

    assert_eq!(store.disarm(&tenant(), "sla/issue-7"), DisarmOutcome::Disarmed, "PROVIDER: disarm sets fired");
    assert!(store.get(&tenant(), "sla/issue-7").unwrap().fired, "PROVIDER: the partial-index pivot is set");

    // the wheel never fires it (WHERE NOT fired excludes the disarmed row).
    let wheel = TimerWheel::new(store.clone(), journal.clone(), runs.clone(), FlowTelemetry::new(), 0, 100);
    assert_eq!(wheel.tick(60), 0, "PROVIDER: the wheel fires NOTHING for a disarmed timer");
    assert!(journal.history_for(&tenant(), "R-sla-7").is_empty(), "PROVIDER: no timer_fired — the breach was cancelled");
}

/// **CONSUMER side of 9.3 (Issues slides an SLA deadline forward by cheap re-arms, then cancels on
/// resolution).** The Issues `stale_after` producer fixture: it arms the breach timer ONCE, then every
/// time the issue is touched it re-arms the SAME timer forward (a row update — NOT a fresh arm, so the
/// wheel never grows a second row), and on resolution it disarms (the SLA was met → the timer must
/// never fire). The consumer relies ONLY on the provider's row-op promise; it never scans the wheel.
#[test]
fn consumer_issues_sla_slides_the_deadline_then_disarms_on_resolution() {
    let store = TimerStore::new();
    let journal = WfJournal::new();
    let runs = RunStore::new();
    let mut run = RunRow::new_runnable(tenant(), region(), "R-sla-issue-7", "issues.sla", 0);
    run.state = run_state::WAITING.into();
    runs.put(run);

    // CONSUMER: arm the SLA breach timer ONCE (the issue opened; breach due in 4h = 14_400s).
    let timer_id = "sla/issue-7";
    assert_eq!(store.arm(sla_timer(timer_id, "R-sla-issue-7", 14_400)), ArmOutcome::Armed);

    // CONSUMER: the issue is touched 3 times (a comment, a reassignment, a label) — each SLIDES the
    // breach deadline forward by a cheap re-arm (the `stale_after` reset). NONE adds a wheel row.
    for (touch, new_fire_at) in [(1, 21_600i64), (2, 28_800), (3, 36_000)] {
        assert_eq!(
            store.re_arm(&tenant(), timer_id, new_fire_at),
            ReArmOutcome::ReArmed,
            "CONSUMER touch {touch}: the SLA deadline slid forward by a cheap re-arm"
        );
        assert_eq!(store.armed_count(), 1, "CONSUMER: STILL one timer on the wheel (re-arm is a row update)");
    }
    // the breach timer now sits at its latest deadline (10h = 36_000s, bucket 600) — far-future.
    let row = store.get(&tenant(), timer_id).unwrap();
    assert_eq!(row.fire_at, 36_000);
    assert_eq!(row.bucket, 600);

    // the wheel at hour-5 (18_000s) finds NOTHING due — the deadline slid past it (the SC-11 move:
    // the breach timer is in a far-future bucket, never scanned until its minute).
    let wheel = TimerWheel::new(store.clone(), journal.clone(), runs.clone(), FlowTelemetry::new(), 0, 100);
    assert_eq!(wheel.tick(18_000), 0, "CONSUMER: the breach has not fired (the deadline keeps sliding forward)");

    // CONSUMER: the issue is RESOLVED — disarm the breach timer (the SLA was met → never fire).
    assert_eq!(store.disarm(&tenant(), timer_id), DisarmOutcome::Disarmed, "CONSUMER: resolution disarms the breach");

    // even when the (now-disarmed) deadline arrives, the wheel fires NOTHING (the breach was met).
    assert_eq!(wheel.tick(40_000), 0, "CONSUMER: the disarmed breach never fires (the SLA was satisfied)");
    assert!(
        journal.history_for(&tenant(), "R-sla-issue-7").is_empty(),
        "CONSUMER: no breach event — the SLA was met before the deadline"
    );
    assert_eq!(
        runs.get(&tenant(), "R-sla-issue-7").unwrap().state,
        run_state::WAITING,
        "CONSUMER: the SLA workflow was never woken (the breach was disarmed)"
    );
}

/// **CONSUMER negative — when the SLA is NOT touched, the breach DOES fire (the re-arm is the only
/// thing keeping it alive).** Arm a breach timer; do NOT re-arm; the wheel fires it at the deadline
/// (the SLA was breached). This proves the re-arm is load-bearing (a disarm/re-arm bug would either
/// leak fires or suppress real breaches).
#[test]
fn consumer_an_untouched_sla_fires_its_breach() {
    let store = TimerStore::new();
    let journal = WfJournal::new();
    let runs = RunStore::new();
    let mut run = RunRow::new_runnable(tenant(), region(), "R-sla-stale", "issues.sla", 0);
    run.state = run_state::WAITING.into();
    runs.put(run);
    store.arm(sla_timer("sla/issue-stale", "R-sla-stale", 0)); // due now, never re-armed.

    let wheel = TimerWheel::new(store.clone(), journal.clone(), runs.clone(), FlowTelemetry::new(), 0, 100);
    assert_eq!(wheel.tick(60), 1, "CONSUMER: the untouched SLA breaches (the breach timer fires)");
    let hist = journal.history_for(&tenant(), "R-sla-stale");
    assert_eq!(hist.len(), 1, "CONSUMER: one timer_fired (the breach event)");
    assert_eq!(
        runs.get(&tenant(), "R-sla-stale").unwrap().state,
        run_state::RUNNING,
        "CONSUMER: the SLA workflow woke to handle the breach"
    );
    // and a re-fire is effectively-once (the firing path is unchanged by P-FLOW-14).
    assert_eq!(store.fire(&tenant(), "sla/issue-stale", &journal, &runs), FireOutcome::AlreadyFired);
}

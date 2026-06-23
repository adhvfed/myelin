//! Unit tests for the business-calendar SLA arithmetic engine (ISS-P26 / P-393).
//!
//! The corpus the ISS-D6 drill names: DST spring-forward + fall-back, multi-day spans, holiday
//! boundaries, and mid-window pause/resume → `fire_at` correct TO THE SECOND. Plus the across-restart
//! EXACTLY-ONCE breach (snapshot/restore re-arms the SAME `fire_at`; a re-fire is a no-op) and the
//! escalation-chain start on breach. A mis-fired SLA is a governance failure (the mandatory-core
//! mutation floor), so these assert exact instants, not approximations.

use super::*;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}

/// Epoch seconds for a UTC civil datetime (the test's clock — the SAME civil arithmetic the engine's
/// encoder inverts). Days-from-civil (Howard Hinnant).
fn utc(year: i64, month: i64, day: i64, hour: i64, min: i64, sec: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400 + hour * 3600 + min * 60 + sec
}

/// A Europe/Paris-style calendar: UTC+1 standard, UTC+2 DST. Spring-forward 2024-03-31 01:00 UTC
/// (local 02:00 → 03:00); fall-back 2024-10-27 01:00 UTC (local 03:00 → 02:00). Mon–Fri 09:00–17:00
/// local. `holidays` are LOCAL day numbers.
fn paris_calendar(holidays: Vec<i64>) -> Calendar {
    Calendar::new(
        "europe-paris",
        vec![
            Weekday::Mon,
            Weekday::Tue,
            Weekday::Wed,
            Weekday::Thu,
            Weekday::Fri,
        ],
        9 * 60,
        17 * 60,
        holidays,
        vec![
            // base offset UTC+1 from the dawn of time.
            OffsetTransition {
                at_utc: i64::MIN / 2,
                offset_secs: 3600,
            },
            // spring forward → UTC+2 at 2024-03-31 01:00 UTC.
            OffsetTransition {
                at_utc: utc(2024, 3, 31, 1, 0, 0),
                offset_secs: 7200,
            },
            // fall back → UTC+1 at 2024-10-27 01:00 UTC.
            OffsetTransition {
                at_utc: utc(2024, 10, 27, 1, 0, 0),
                offset_secs: 3600,
            },
        ],
    )
    .expect("paris calendar is well-formed")
}

/// The LOCAL day number for a civil date at UTC+1 (a holiday handle; holidays are LOCAL day numbers).
fn local_day_utc_plus_1(year: i64, month: i64, day: i64) -> i64 {
    (utc(year, month, day, 12, 0, 0) + 3600).div_euclid(86_400)
}

// --- the calendar primitives ---

/// **A degenerate working window is REJECTED at construction (a mis-config fails loudly, not silently).**
#[test]
fn degenerate_calendar_is_rejected() {
    assert!(Calendar::new(
        "bad",
        vec![Weekday::Mon],
        1020,
        540,
        vec![],
        vec![OffsetTransition {
            at_utc: 0,
            offset_secs: 0
        }]
    )
    .is_err());
    assert!(Calendar::new(
        "bad",
        vec![],
        540,
        1020,
        vec![],
        vec![OffsetTransition {
            at_utc: 0,
            offset_secs: 0
        }]
    )
    .is_err());
    assert!(Calendar::new("bad", vec![Weekday::Mon], 540, 1020, vec![], vec![]).is_err());
}

/// **The weekday of a local day matches the civil calendar (1970-01-01 was a Thursday).**
#[test]
fn weekday_of_local_day_is_correct() {
    assert_eq!(
        Weekday::of_local_day(0),
        Weekday::Thu,
        "1970-01-01 is a Thursday"
    );
    // 2024-06-21 is a Friday.
    let fri = (utc(2024, 6, 21, 12, 0, 0) + 3600).div_euclid(86_400);
    assert_eq!(Weekday::of_local_day(fri), Weekday::Fri);
}

/// **The offset lookup returns the DST rule in effect at an instant (the VTIMEZONE predecessor search).**
#[test]
fn offset_at_resolves_the_dst_rule() {
    let cal = paris_calendar(vec![]);
    // before spring-forward: UTC+1.
    assert_eq!(cal.offset_at(utc(2024, 3, 1, 0, 0, 0)), 3600);
    // after spring-forward, before fall-back: UTC+2.
    assert_eq!(cal.offset_at(utc(2024, 6, 1, 0, 0, 0)), 7200);
    // after fall-back: UTC+1.
    assert_eq!(cal.offset_at(utc(2024, 11, 1, 0, 0, 0)), 3600);
}

// --- business_fire_at: the genuinely-owned algorithm, to-the-second ---

/// **A budget within ONE working day fires inside that day (to-the-second).** Start Mon 2024-06-03
/// 09:00 local (07:00 UTC at UTC+2); a 2h budget fires at 11:00 local = 09:00 UTC.
#[test]
fn budget_within_one_day_fires_to_the_second() {
    let cal = paris_calendar(vec![]);
    // Monday 2024-06-03, 09:00 local at UTC+2 = 07:00 UTC.
    let start = utc(2024, 6, 3, 7, 0, 0);
    let fire = business_fire_at(start, 2 * 3600, &cal).unwrap();
    // 2 business hours later = 11:00 local = 09:00 UTC.
    assert_eq!(
        fire,
        utc(2024, 6, 3, 9, 0, 0),
        "2h fires at 11:00 local to-the-second"
    );
}

/// **A budget started BEFORE the working window starts at the window open (the SLA armed at night is
/// due the next working second).** Start Mon 03:00 local; a 1h budget fires at 10:00 local.
#[test]
fn budget_before_window_starts_at_window_open() {
    let cal = paris_calendar(vec![]);
    // Monday 2024-06-03, 03:00 local (UTC+2) = 01:00 UTC — before the 09:00 window.
    let start = utc(2024, 6, 3, 1, 0, 0);
    let fire = business_fire_at(start, 3600, &cal).unwrap();
    // the clock starts at 09:00 local; 1h later = 10:00 local = 08:00 UTC.
    assert_eq!(fire, utc(2024, 6, 3, 8, 0, 0));
}

/// **A multi-day budget spans nights + weekends correctly (the multi-day corpus).** Start Fri 15:00
/// local; a 12h budget = 2h Fri + 8h Mon + 2h Tue → fires Tue 11:00 local.
#[test]
fn multi_day_budget_spans_weekend_to_the_second() {
    let cal = paris_calendar(vec![]);
    // Friday 2024-06-07, 15:00 local (UTC+2) = 13:00 UTC. 2h left in Fri's window (15:00→17:00).
    let start = utc(2024, 6, 7, 13, 0, 0);
    // 12h budget: 2h Fri + 8h Mon (full day) + 2h Tue → fires Tue 11:00 local.
    let fire = business_fire_at(start, 12 * 3600, &cal).unwrap();
    // Tuesday 2024-06-11, 11:00 local (UTC+2) = 09:00 UTC.
    assert_eq!(
        fire,
        utc(2024, 6, 11, 9, 0, 0),
        "weekend skipped, multi-day to-the-second"
    );
}

/// **A holiday is skipped (the holiday corpus).** Make Mon 2024-06-03 a holiday; a budget started Fri
/// afternoon that would land Mon instead lands Tue.
#[test]
fn holiday_is_skipped_to_the_second() {
    // Mon 2024-06-03 is a local-day holiday.
    let holiday = local_day_utc_plus_1(2024, 6, 3);
    let cal = paris_calendar(vec![holiday]);
    // Friday 2024-05-31, 16:00 local (UTC+2) = 14:00 UTC; 1h left Fri.
    let start = utc(2024, 5, 31, 14, 0, 0);
    // 2h budget: 1h Fri + (Mon is a holiday → skipped) + 1h Tue → fires Tue 10:00 local.
    let fire = business_fire_at(start, 2 * 3600, &cal).unwrap();
    // Tuesday 2024-06-04, 10:00 local (UTC+2) = 08:00 UTC.
    assert_eq!(
        fire,
        utc(2024, 6, 4, 8, 0, 0),
        "the holiday Monday is skipped"
    );
}

/// **A working window that straddles the spring-forward DST change is computed to-the-second.** On
/// 2024-03-31 (a Sunday — not a working day) the clock jumps; we use the Friday before + the Monday
/// after to prove the offset switch lands the fire_at on the right wall-clock instant. A budget over
/// the DST weekend lands using the NEW (UTC+2) offset on Monday.
#[test]
fn dst_spring_forward_weekend_to_the_second() {
    let cal = paris_calendar(vec![]);
    // Friday 2024-03-29, 16:00 local at UTC+1 (pre-DST) = 15:00 UTC; 1h left Fri.
    let start = utc(2024, 3, 29, 15, 0, 0);
    // 2h budget: 1h Fri (UTC+1) + (Sat/Sun skipped, DST flips Sunday) + 1h Mon (UTC+2) → Mon 10:00 local.
    let fire = business_fire_at(start, 2 * 3600, &cal).unwrap();
    // Monday 2024-04-01, 10:00 local at UTC+2 = 08:00 UTC.
    assert_eq!(
        fire,
        utc(2024, 4, 1, 8, 0, 0),
        "the Monday window uses the post-DST UTC+2 offset"
    );
}

/// **A working window that straddles the fall-back DST change is computed to-the-second.** A budget over
/// the fall-back weekend lands using the NEW (UTC+1) offset on Monday.
#[test]
fn dst_fall_back_weekend_to_the_second() {
    let cal = paris_calendar(vec![]);
    // Friday 2024-10-25, 16:00 local at UTC+2 (pre-fall-back) = 14:00 UTC; 1h left Fri.
    let start = utc(2024, 10, 25, 14, 0, 0);
    // 2h budget: 1h Fri (UTC+2) + (weekend + fall-back) + 1h Mon (UTC+1) → Mon 10:00 local.
    let fire = business_fire_at(start, 2 * 3600, &cal).unwrap();
    // Monday 2024-10-28, 10:00 local at UTC+1 = 09:00 UTC.
    assert_eq!(
        fire,
        utc(2024, 10, 28, 9, 0, 0),
        "the Monday window uses the post-fall-back UTC+1 offset"
    );
}

/// **business_seconds_between is the inverse of business_fire_at over an interval (the pause-consumed
/// computation).** Working time between Mon 09:00 and Mon 11:00 local = exactly 2h; over a weekend = 0
/// outside windows; a multi-day interval sums the windows.
#[test]
fn business_seconds_between_sums_working_time() {
    let cal = paris_calendar(vec![]);
    // Mon 09:00 → 11:00 local (UTC+2) = 07:00 → 09:00 UTC = 2h working.
    let a =
        business_seconds_between(utc(2024, 6, 3, 7, 0, 0), utc(2024, 6, 3, 9, 0, 0), &cal).unwrap();
    assert_eq!(a, 2 * 3600);
    // a weekend (Sat 00:00 → Sun 23:00) has 0 working seconds.
    let weekend =
        business_seconds_between(utc(2024, 6, 8, 0, 0, 0), utc(2024, 6, 9, 23, 0, 0), &cal)
            .unwrap();
    assert_eq!(weekend, 0, "no working seconds over the weekend");
    // Fri 15:00 → Mon 11:00 local = 2h Fri (15:00→17:00) + 2h Mon (09:00→11:00) = 4h working
    // (the weekend nights contribute nothing). Mon 11:00 local at UTC+2 = 09:00 UTC.
    let span = business_seconds_between(utc(2024, 6, 7, 13, 0, 0), utc(2024, 6, 10, 9, 0, 0), &cal)
        .unwrap();
    assert_eq!(span, 4 * 3600, "2h Fri + 2h Mon, weekend nights excluded");
}

// --- the engine: arm / pause / resume / breach / met + across-restart ---

/// **arm precomputes fire_at + at_risk_fire_at, arms TWO timers, emits started.**
#[test]
fn arm_precomputes_two_fire_ats_and_arms_two_timers() {
    let cal = paris_calendar(vec![]);
    let mut eng = SlaEngine::new(tenant(), region());
    // start Mon 2024-06-03 09:00 local = 07:00 UTC; 8h (one working day) budget.
    let start = utc(2024, 6, 3, 7, 0, 0);
    let run = eng
        .arm("ENG#7", "sla-resp-8h", cal, 8 * 3600, start)
        .unwrap();
    // breach at 17:00 local = 15:00 UTC.
    assert_eq!(
        run.fire_at,
        utc(2024, 6, 3, 15, 0, 0),
        "breach to-the-second"
    );
    // at-risk at 80% = 6.4h → 6h24m after 09:00 = 15:24 local = 13:24 UTC.
    assert_eq!(
        run.at_risk_fire_at,
        utc(2024, 6, 3, 13, 24, 0),
        "80% at-risk to-the-second"
    );
    assert!(
        run.at_risk_fire_at < run.fire_at,
        "the nudge precedes the breach"
    );
    assert_eq!(
        eng.armed_timer_count(),
        2,
        "two timers armed (breach + at-risk)"
    );
    assert_eq!(eng.emitted().len(), 1);
    assert_eq!(eng.emitted()[0].event_type(), crate::events::SLA_STARTED);
}

/// **pause disarms both timers + stores remaining; resume recomputes + re-arms; the total budget is
/// preserved across a mid-window pause (the pause/resume corpus, to-the-second).**
#[test]
fn pause_resume_preserves_budget_to_the_second() {
    let cal = paris_calendar(vec![]);
    let mut eng = SlaEngine::new(tenant(), region());
    // start Mon 09:00 local (07:00 UTC), 8h budget → breach Mon 17:00 local.
    let start = utc(2024, 6, 3, 7, 0, 0);
    eng.arm("ENG#9", "sla-8h", cal, 8 * 3600, start).unwrap();

    // pause at Mon 11:00 local (09:00 UTC) — 2h consumed, 6h remaining.
    eng.pause("ENG#9", utc(2024, 6, 3, 9, 0, 0)).unwrap();
    let paused = eng.run("ENG#9").unwrap();
    assert_eq!(paused.state, SlaState::Paused);
    assert_eq!(
        paused.remaining_business_secs,
        6 * 3600,
        "6h remaining after a 2h run"
    );
    assert_eq!(
        eng.armed_timer_count(),
        0,
        "both timers disarmed on pause (no wheel timer while paused)"
    );

    // resume the NEXT day at Tue 09:00 local (07:00 UTC) — 6h remaining → breach Tue 15:00 local.
    eng.resume("ENG#9", utc(2024, 6, 4, 7, 0, 0)).unwrap();
    let resumed = eng.run("ENG#9").unwrap();
    assert_eq!(resumed.state, SlaState::Running);
    assert_eq!(
        resumed.fire_at,
        utc(2024, 6, 4, 13, 0, 0),
        "6h from Tue 09:00 = Tue 15:00 local, to-the-second"
    );
    assert_eq!(eng.armed_timer_count(), 2, "both timers re-armed on resume");
    // the emitted stream: started, paused, resumed.
    let kinds: Vec<&str> = eng.emitted().iter().map(|e| e.event_type()).collect();
    assert_eq!(
        kinds,
        vec![
            crate::events::SLA_STARTED,
            crate::events::SLA_PAUSED,
            crate::events::SLA_RESUMED
        ]
    );
}

/// **The breach fires EXACTLY ONCE → starts the FROZEN escalation chain; a re-fire is a no-op.**
#[test]
fn breach_fires_exactly_once_and_starts_the_chain() {
    let cal = paris_calendar(vec![]);
    let mut eng = SlaEngine::new(tenant(), region());
    eng.arm("ENG#11", "sla-8h", cal, 8 * 3600, utc(2024, 6, 3, 7, 0, 0))
        .unwrap();

    // the breach timer fires.
    assert!(
        eng.on_breach_timer("ENG#11", 15, 1),
        "the breach fires once"
    );
    assert_eq!(eng.run("ENG#11").unwrap().state, SlaState::Breached);
    // a re-fire (the wheel re-delivers) is a no-op — 0 double-fire.
    assert!(
        !eng.on_breach_timer("ENG#11", 15, 1),
        "the re-fire is a no-op (exactly-once)"
    );

    // exactly ONE breached event, naming the FROZEN escalation chain policy id.
    let breaches: Vec<&SlaOutcomeEvent> = eng
        .emitted()
        .iter()
        .filter(|e| e.event_type() == crate::events::SLA_BREACHED)
        .collect();
    assert_eq!(breaches.len(), 1, "exactly one breach event");
    match breaches[0] {
        SlaOutcomeEvent::Breached {
            escalation_policy_id,
            ..
        } => assert_eq!(
            escalation_policy_id,
            crate::sla_escalation::SLA_ESCALATION_POLICY_ID,
            "the breach started the FROZEN Issues chain (not a parallel calc)"
        ),
        other => panic!("expected a breach, got {other:?}"),
    }
}

/// **meet disarms the timers + counts toward compliance; a met SLA never breaches.**
#[test]
fn meet_disarms_and_blocks_a_later_breach() {
    let cal = paris_calendar(vec![]);
    let mut eng = SlaEngine::new(tenant(), region());
    eng.arm("ENG#13", "sla-8h", cal, 8 * 3600, utc(2024, 6, 3, 7, 0, 0))
        .unwrap();
    assert!(eng.meet("ENG#13"), "the SLA is met (closed within target)");
    assert_eq!(eng.run("ENG#13").unwrap().state, SlaState::Met);
    assert_eq!(
        eng.armed_timer_count(),
        0,
        "both timers disarmed on met (the breach never fires)"
    );
    // a breach after a met is a no-op — a closed issue does not breach.
    assert!(
        !eng.on_breach_timer("ENG#13", 15, 1),
        "a met SLA does not breach"
    );
    let kinds: Vec<&str> = eng.emitted().iter().map(|e| e.event_type()).collect();
    assert_eq!(
        kinds,
        vec![crate::events::SLA_STARTED, crate::events::SLA_MET]
    );
}

/// **The at-risk nudge fires once while running, and not after a terminal/paused state.**
#[test]
fn at_risk_nudge_fires_only_while_running() {
    let cal = paris_calendar(vec![]);
    let mut eng = SlaEngine::new(tenant(), region());
    eng.arm("ENG#15", "sla-8h", cal, 8 * 3600, utc(2024, 6, 3, 7, 0, 0))
        .unwrap();
    assert!(
        eng.on_at_risk_timer("ENG#15"),
        "the nudge fires while running"
    );
    eng.meet("ENG#15");
    assert!(!eng.on_at_risk_timer("ENG#15"), "no nudge after met");
}

/// **ISS-D6 across-restart: the breach fires to-the-second AFTER a restart, and EXACTLY ONCE.** Snapshot
/// the running engine, build a FRESH engine from it (the restart), and assert the restored run carries
/// the SAME fire_at + re-armed its two timers; the breach fires once post-restart and a pre-restart
/// re-fire on the fresh engine is the only fire.
#[test]
fn across_restart_breach_fires_to_the_second_exactly_once() {
    let cal = paris_calendar(vec![]);
    let mut eng = SlaEngine::new(tenant(), region());
    let start = utc(2024, 6, 3, 7, 0, 0);
    let armed = eng.arm("ENG#21", "sla-8h", cal, 8 * 3600, start).unwrap();
    let fire_at = armed.fire_at;

    // --- the process restarts: snapshot → restore a fresh engine ---
    let snap = eng.snapshot();
    drop(eng);
    let mut restored = SlaEngine::restore(tenant(), region(), snap).unwrap();

    // the restored run carries the SAME precomputed fire_at (to-the-second across the restart).
    let r = restored.run("ENG#21").unwrap();
    assert_eq!(
        r.fire_at, fire_at,
        "the fire_at survives the restart to-the-second"
    );
    assert_eq!(r.state, SlaState::Running);
    assert_eq!(
        restored.armed_timer_count(),
        2,
        "both timers RE-ARMED after the restart"
    );
    // restore did NOT re-emit started (the restart is not a new start — exactly-once preserved).
    assert!(restored.emitted().is_empty(), "no re-emit on restore");

    // the breach fires once after the restart; a wheel re-delivery is a no-op.
    assert!(
        restored.on_breach_timer("ENG#21", 15, 1),
        "the breach fires post-restart"
    );
    assert!(
        !restored.on_breach_timer("ENG#21", 15, 1),
        "a re-delivery is a no-op (exactly-once)"
    );
    let breaches = restored
        .emitted()
        .iter()
        .filter(|e| e.event_type() == crate::events::SLA_BREACHED)
        .count();
    assert_eq!(breaches, 1, "EXACTLY ONE breach across the restart");
}

/// **A paused SLA is restored WITHOUT a live timer (a paused clock has no wheel timer).**
#[test]
fn across_restart_a_paused_sla_has_no_live_timer() {
    let cal = paris_calendar(vec![]);
    let mut eng = SlaEngine::new(tenant(), region());
    eng.arm("ENG#23", "sla-8h", cal, 8 * 3600, utc(2024, 6, 3, 7, 0, 0))
        .unwrap();
    eng.pause("ENG#23", utc(2024, 6, 3, 9, 0, 0)).unwrap();
    let snap = eng.snapshot();
    let restored = SlaEngine::restore(tenant(), region(), snap).unwrap();
    assert_eq!(restored.run("ENG#23").unwrap().state, SlaState::Paused);
    assert_eq!(
        restored.armed_timer_count(),
        0,
        "a paused SLA re-arms NO timer (it resumes to re-arm)"
    );
}

/// **The pause_conditions type IS the one QueryAst grammar (one grammar, never a second SLA DSL).**
#[test]
fn pause_conditions_is_the_one_query_ast() {
    // PauseConditions is a type alias for myelin_query::Predicate — building one compiles, proving the
    // SLA policy's pause_conditions is the SAME grammar the matcher/trigger use.
    let _pc: PauseConditions = Predicate::True;
}

/// **The floors are NAMED (the ledger audits them) — none is claimed green.**
#[test]
fn floors_are_named() {
    assert!(SlaCalendarFloors::HISTORY_COMPACTION.contains("R-11"));
    assert!(SlaCalendarFloors::TZ_DATABASE.contains("tz database"));
    assert!(SlaCalendarFloors::LIVE_WHEEL.contains("9.3"));
}

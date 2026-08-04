use super::*;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}

fn utc(year: i64, month: i64, day: i64, hour: i64, min: i64, sec: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400 + hour * 3600 + min * 60 + sec
}

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
            OffsetTransition {
                at_utc: i64::MIN / 2,
                offset_secs: 3600,
            },
            OffsetTransition {
                at_utc: utc(2024, 3, 31, 1, 0, 0),
                offset_secs: 7200,
            },
            OffsetTransition {
                at_utc: utc(2024, 10, 27, 1, 0, 0),
                offset_secs: 3600,
            },
        ],
    )
    .expect("paris calendar is well-formed")
}

fn local_day_utc_plus_1(year: i64, month: i64, day: i64) -> i64 {
    (utc(year, month, day, 12, 0, 0) + 3600).div_euclid(86_400)
}

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

#[test]
fn weekday_of_local_day_is_correct() {
    assert_eq!(
        Weekday::of_local_day(0),
        Weekday::Thu,
        "1970-01-01 is a Thursday"
    );
    let fri = (utc(2024, 6, 21, 12, 0, 0) + 3600).div_euclid(86_400);
    assert_eq!(Weekday::of_local_day(fri), Weekday::Fri);
}

#[test]
fn offset_at_resolves_the_dst_rule() {
    let cal = paris_calendar(vec![]);
    assert_eq!(cal.offset_at(utc(2024, 3, 1, 0, 0, 0)), 3600);
    assert_eq!(cal.offset_at(utc(2024, 6, 1, 0, 0, 0)), 7200);
    assert_eq!(cal.offset_at(utc(2024, 11, 1, 0, 0, 0)), 3600);
}

#[test]
fn budget_within_one_day_fires_to_the_second() {
    let cal = paris_calendar(vec![]);
    let start = utc(2024, 6, 3, 7, 0, 0);
    let fire = business_fire_at(start, 2 * 3600, &cal).unwrap();
    assert_eq!(
        fire,
        utc(2024, 6, 3, 9, 0, 0),
        "2h fires at 11:00 local to-the-second"
    );
}

#[test]
fn budget_before_window_starts_at_window_open() {
    let cal = paris_calendar(vec![]);
    let start = utc(2024, 6, 3, 1, 0, 0);
    let fire = business_fire_at(start, 3600, &cal).unwrap();
    assert_eq!(fire, utc(2024, 6, 3, 8, 0, 0));
}

#[test]
fn multi_day_budget_spans_weekend_to_the_second() {
    let cal = paris_calendar(vec![]);
    let start = utc(2024, 6, 7, 13, 0, 0);
    let fire = business_fire_at(start, 12 * 3600, &cal).unwrap();
    assert_eq!(
        fire,
        utc(2024, 6, 11, 9, 0, 0),
        "weekend skipped, multi-day to-the-second"
    );
}

#[test]
fn holiday_is_skipped_to_the_second() {
    let holiday = local_day_utc_plus_1(2024, 6, 3);
    let cal = paris_calendar(vec![holiday]);
    let start = utc(2024, 5, 31, 14, 0, 0);
    let fire = business_fire_at(start, 2 * 3600, &cal).unwrap();
    assert_eq!(
        fire,
        utc(2024, 6, 4, 8, 0, 0),
        "the holiday Monday is skipped"
    );
}

#[test]
fn dst_spring_forward_weekend_to_the_second() {
    let cal = paris_calendar(vec![]);
    let start = utc(2024, 3, 29, 15, 0, 0);
    let fire = business_fire_at(start, 2 * 3600, &cal).unwrap();
    assert_eq!(
        fire,
        utc(2024, 4, 1, 8, 0, 0),
        "the Monday window uses the post-DST UTC+2 offset"
    );
}

#[test]
fn dst_fall_back_weekend_to_the_second() {
    let cal = paris_calendar(vec![]);
    let start = utc(2024, 10, 25, 14, 0, 0);
    let fire = business_fire_at(start, 2 * 3600, &cal).unwrap();
    assert_eq!(
        fire,
        utc(2024, 10, 28, 9, 0, 0),
        "the Monday window uses the post-fall-back UTC+1 offset"
    );
}

#[test]
fn business_seconds_between_sums_working_time() {
    let cal = paris_calendar(vec![]);
    let a =
        business_seconds_between(utc(2024, 6, 3, 7, 0, 0), utc(2024, 6, 3, 9, 0, 0), &cal).unwrap();
    assert_eq!(a, 2 * 3600);
    let weekend =
        business_seconds_between(utc(2024, 6, 8, 0, 0, 0), utc(2024, 6, 9, 23, 0, 0), &cal)
            .unwrap();
    assert_eq!(weekend, 0, "no working seconds over the weekend");
    let span = business_seconds_between(utc(2024, 6, 7, 13, 0, 0), utc(2024, 6, 10, 9, 0, 0), &cal)
        .unwrap();
    assert_eq!(span, 4 * 3600, "2h Fri + 2h Mon, weekend nights excluded");
}

#[test]
fn arm_precomputes_two_fire_ats_and_arms_two_timers() {
    let cal = paris_calendar(vec![]);
    let mut eng = SlaEngine::new(tenant(), region());
    let start = utc(2024, 6, 3, 7, 0, 0);
    let run = eng
        .arm("ENG#7", "sla-resp-8h", cal, 8 * 3600, start)
        .unwrap();
    assert_eq!(
        run.fire_at,
        utc(2024, 6, 3, 15, 0, 0),
        "breach to-the-second"
    );
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

#[test]
fn pause_resume_preserves_budget_to_the_second() {
    let cal = paris_calendar(vec![]);
    let mut eng = SlaEngine::new(tenant(), region());
    let start = utc(2024, 6, 3, 7, 0, 0);
    eng.arm("ENG#9", "sla-8h", cal, 8 * 3600, start).unwrap();

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

    eng.resume("ENG#9", utc(2024, 6, 4, 7, 0, 0)).unwrap();
    let resumed = eng.run("ENG#9").unwrap();
    assert_eq!(resumed.state, SlaState::Running);
    assert_eq!(
        resumed.fire_at,
        utc(2024, 6, 4, 13, 0, 0),
        "6h from Tue 09:00 = Tue 15:00 local, to-the-second"
    );
    assert_eq!(eng.armed_timer_count(), 2, "both timers re-armed on resume");
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

#[test]
fn breach_fires_exactly_once_and_starts_the_chain() {
    let cal = paris_calendar(vec![]);
    let mut eng = SlaEngine::new(tenant(), region());
    eng.arm("ENG#11", "sla-8h", cal, 8 * 3600, utc(2024, 6, 3, 7, 0, 0))
        .unwrap();

    assert!(
        eng.on_breach_timer("ENG#11", 15, 1),
        "the breach fires once"
    );
    assert_eq!(eng.run("ENG#11").unwrap().state, SlaState::Breached);
    assert!(
        !eng.on_breach_timer("ENG#11", 15, 1),
        "the re-fire is a no-op (exactly-once)"
    );

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

#[test]
fn across_restart_breach_fires_to_the_second_exactly_once() {
    let cal = paris_calendar(vec![]);
    let mut eng = SlaEngine::new(tenant(), region());
    let start = utc(2024, 6, 3, 7, 0, 0);
    let armed = eng.arm("ENG#21", "sla-8h", cal, 8 * 3600, start).unwrap();
    let fire_at = armed.fire_at;

    let snap = eng.snapshot();
    drop(eng);
    let mut restored = SlaEngine::restore(tenant(), region(), snap).unwrap();

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
    assert!(restored.emitted().is_empty(), "no re-emit on restore");

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

#[test]
fn pause_conditions_is_the_one_query_ast() {
    let _pc: PauseConditions = Predicate::True;
}

#[test]
fn floors_are_named() {
    assert!(SlaCalendarFloors::HISTORY_COMPACTION.contains("R-11"));
    assert!(SlaCalendarFloors::TZ_DATABASE.contains("tz database"));
    assert!(SlaCalendarFloors::LIVE_WHEEL.contains("9.3"));
}

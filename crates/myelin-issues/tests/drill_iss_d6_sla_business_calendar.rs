use myelin_issues::events::SLA_BREACHED;
use myelin_issues::sla_calendar::{
    business_fire_at, Calendar, OffsetTransition, SlaEngine, SlaOutcomeEvent, SlaState, Weekday,
};
use myelin_issues::sla_escalation::SLA_ESCALATION_POLICY_ID;
use myelin_tenancy::{Region, TenantId};

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
    .expect("paris calendar")
}

fn local_day(year: i64, month: i64, day: i64, offset: i64) -> i64 {
    (utc(year, month, day, 12, 0, 0) + offset).div_euclid(86_400)
}

struct CorpusCase {
    name: &'static str,
    cal: Calendar,
    start: i64,
    budget_secs: i64,
    expected_fire_at: i64,
}

#[test]
fn iss_d6_b_business_calendar_fire_at_to_the_second() {
    let corpus = vec![
        CorpusCase {
            name: "within-one-day",
            cal: paris_calendar(vec![]),
            start: utc(2024, 6, 3, 7, 0, 0),
            budget_secs: 2 * 3600,
            expected_fire_at: utc(2024, 6, 3, 9, 0, 0),
        },
        CorpusCase {
            name: "multi-day-weekend",
            cal: paris_calendar(vec![]),
            start: utc(2024, 6, 7, 13, 0, 0),
            budget_secs: 12 * 3600,
            expected_fire_at: utc(2024, 6, 11, 9, 0, 0),
        },
        CorpusCase {
            name: "holiday-skip",
            cal: paris_calendar(vec![local_day(2024, 6, 3, 7200)]),
            start: utc(2024, 5, 31, 14, 0, 0),
            budget_secs: 2 * 3600,
            expected_fire_at: utc(2024, 6, 4, 8, 0, 0),
        },
        CorpusCase {
            name: "dst-spring-forward",
            cal: paris_calendar(vec![]),
            start: utc(2024, 3, 29, 15, 0, 0),
            budget_secs: 2 * 3600,
            expected_fire_at: utc(2024, 4, 1, 8, 0, 0),
        },
        CorpusCase {
            name: "dst-fall-back",
            cal: paris_calendar(vec![]),
            start: utc(2024, 10, 25, 14, 0, 0),
            budget_secs: 2 * 3600,
            expected_fire_at: utc(2024, 10, 28, 9, 0, 0),
        },
    ];

    let mut max_error_secs = 0i64;
    for case in &corpus {
        let computed = business_fire_at(case.start, case.budget_secs, &case.cal).unwrap();
        let error = (computed - case.expected_fire_at).abs();
        max_error_secs = max_error_secs.max(error);
        assert_eq!(
            computed, case.expected_fire_at,
            "ISS-D6 (b): case '{}' fire_at is NOT to-the-second (error {error}s)",
            case.name
        );
    }
    assert_eq!(
        max_error_secs, 0,
        "ISS-D6 (b) GREEN: 0-second fire_at error across the DST/multi-day/holiday corpus"
    );
}

#[test]
fn iss_d6_b_pause_resume_preserves_budget_to_the_second() {
    let cal = paris_calendar(vec![]);
    let mut eng = SlaEngine::new(tenant(), region());
    eng.arm("ENG#6b", "sla-8h", cal, 8 * 3600, utc(2024, 6, 3, 7, 0, 0))
        .unwrap();
    eng.pause("ENG#6b", utc(2024, 6, 3, 9, 0, 0)).unwrap();
    assert_eq!(eng.run("ENG#6b").unwrap().remaining_business_secs, 6 * 3600);
    eng.resume("ENG#6b", utc(2024, 6, 4, 7, 0, 0)).unwrap();
    assert_eq!(
        eng.run("ENG#6b").unwrap().fire_at,
        utc(2024, 6, 4, 13, 0, 0),
        "ISS-D6 (b) GREEN: pause/resume preserves the budget to-the-second"
    );
}

#[test]
fn iss_d6_a_breach_after_restart_exactly_once_and_c_starts_chain() {
    let cal = paris_calendar(vec![]);
    let mut eng = SlaEngine::new(tenant(), region());
    let start = utc(2024, 6, 3, 7, 0, 0);
    let armed = eng.arm("ENG#6a", "sla-8h", cal, 8 * 3600, start).unwrap();
    let fire_at = armed.fire_at;

    let snap = eng.snapshot();
    drop(eng);
    let mut restored = SlaEngine::restore(tenant(), region(), snap).unwrap();
    assert_eq!(
        restored.run("ENG#6a").unwrap().fire_at,
        fire_at,
        "ISS-D6 (a): the fire_at survived the restart to-the-second"
    );
    assert_eq!(
        restored.armed_timer_count(),
        2,
        "ISS-D6 (a): both timers re-armed after the restart"
    );

    let first = restored.on_breach_timer("ENG#6a", 15, 1);
    let second = restored.on_breach_timer("ENG#6a", 15, 1);
    assert!(first, "ISS-D6 (a): the breach fires post-restart");
    assert!(!second, "ISS-D6 (a): a wheel re-delivery is a no-op");
    assert_eq!(restored.run("ENG#6a").unwrap().state, SlaState::Breached);

    let breaches: Vec<&SlaOutcomeEvent> = restored
        .emitted()
        .iter()
        .filter(|e| e.event_type() == SLA_BREACHED)
        .collect();
    assert_eq!(
        breaches.len(),
        1,
        "ISS-D6 (a) GREEN: EXACTLY ONE breach across the restart (0 missed, 0 duplicate)"
    );

    match breaches[0] {
        SlaOutcomeEvent::Breached {
            escalation_policy_id,
            ..
        } => assert_eq!(
            escalation_policy_id, SLA_ESCALATION_POLICY_ID,
            "ISS-D6 (c) GREEN: the breach starts the FROZEN Issues escalation chain (not a parallel calc)"
        ),
        other => panic!("expected a breach, got {other:?}"),
    }
}

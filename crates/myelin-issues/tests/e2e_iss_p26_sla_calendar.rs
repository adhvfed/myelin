use myelin_issues::events::{SLA_BREACHED, SLA_PAUSED, SLA_RESUMED, SLA_STARTED};
use myelin_issues::sla_calendar::{
    business_fire_at, Calendar, OffsetTransition, SlaEngine, SlaEngineSnapshot, SlaOutcomeEvent,
    SlaState, Weekday,
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

#[test]
fn sla_calendar_chained_mutation_arm_pause_resume_restart_breach() {
    let holiday = (utc(2024, 6, 5, 12, 0, 0) + 7200).div_euclid(86_400);
    let cal = paris_calendar(vec![holiday]);
    let mut eng = SlaEngine::new(tenant(), region());

    let start = utc(2024, 6, 3, 7, 0, 0);
    let armed = eng
        .arm("ENG#42", "sla-resp-24h", cal.clone(), 24 * 3600, start)
        .unwrap();
    let expected_breach = utc(2024, 6, 6, 15, 0, 0);
    assert_eq!(
        armed.fire_at, expected_breach,
        "the breach fire_at is to-the-second over the DST+holiday calendar"
    );
    assert_eq!(
        business_fire_at(start, 24 * 3600, &cal).unwrap(),
        expected_breach,
        "the engine used business_fire_at (no parallel calc)"
    );
    assert_eq!(eng.armed_timer_count(), 2, "breach + at-risk armed");

    eng.pause("ENG#42", utc(2024, 6, 4, 9, 0, 0)).unwrap();
    assert_eq!(
        eng.run("ENG#42").unwrap().remaining_business_secs,
        14 * 3600
    );
    assert_eq!(eng.armed_timer_count(), 0, "timers disarmed while paused");

    eng.resume("ENG#42", utc(2024, 6, 6, 7, 0, 0)).unwrap();
    let resumed_breach = utc(2024, 6, 7, 13, 0, 0);
    assert_eq!(
        eng.run("ENG#42").unwrap().fire_at,
        resumed_breach,
        "the resume recomputed fire_at to-the-second over the remaining budget"
    );
    assert_eq!(eng.armed_timer_count(), 2, "re-armed on resume");

    let snap: SlaEngineSnapshot = eng.snapshot();
    drop(eng);
    let mut restored = SlaEngine::restore(tenant(), region(), snap).unwrap();
    assert_eq!(
        restored.run("ENG#42").unwrap().fire_at,
        resumed_breach,
        "the fire_at survived the restart to-the-second"
    );
    assert_eq!(
        restored.armed_timer_count(),
        2,
        "both timers RE-ARMED after the restart"
    );

    assert!(
        restored.on_breach_timer("ENG#42", 15, 1),
        "the breach fires post-restart"
    );
    assert!(
        !restored.on_breach_timer("ENG#42", 15, 1),
        "a wheel re-delivery is a no-op (exactly-once)"
    );
    assert_eq!(restored.run("ENG#42").unwrap().state, SlaState::Breached);

    let breaches: Vec<&SlaOutcomeEvent> = restored
        .emitted()
        .iter()
        .filter(|e| e.event_type() == SLA_BREACHED)
        .collect();
    assert_eq!(breaches.len(), 1, "EXACTLY ONE breach across the restart");
    match breaches[0] {
        SlaOutcomeEvent::Breached {
            escalation_policy_id,
            ..
        } => assert_eq!(escalation_policy_id, SLA_ESCALATION_POLICY_ID),
        other => panic!("expected a breach, got {other:?}"),
    }

    let pre_restart_kinds = [SLA_STARTED, SLA_PAUSED, SLA_RESUMED];
    for k in pre_restart_kinds {
        assert!(
            !restored.emitted().iter().any(|e| e.event_type() == k),
            "the restored engine does NOT re-emit {k} (exactly-once across restart)"
        );
    }
}

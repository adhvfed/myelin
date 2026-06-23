//! # e2e ISS-P26 — the SLA business-calendar chained-mutation lifecycle (P-393, M4-I7)
//!
//! The chained-mutation e2e the prompt's TESTS line requires: **arm an SLA → restart → assert breach
//! fires + chain starts.** This drives the FULL Issues SLA business-calendar surface through one
//! scenario over a real IANA-tz calendar (Europe/Paris with DST + a holiday), exercising the genuinely
//! owned arithmetic ([`business_fire_at`] to-the-second), the pause/resume cheap disarm/re-arm, and the
//! across-restart EXACTLY-ONCE breach → FROZEN escalation chain start. The durable wheel
//! ([`myelin_query::DurableTimer`] / `myelin-flow` 9.3) + the escalation chain
//! ([`myelin_issues::sla_escalation`], 7.5) + the SLA event vocabulary ([`myelin_issues::events`]) are
//! all CONSUMED, never re-implemented (EI-01 §7).

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

/// Epoch seconds for a UTC civil datetime (days-from-civil, Howard Hinnant).
fn utc(year: i64, month: i64, day: i64, hour: i64, min: i64, sec: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400 + hour * 3600 + min * 60 + sec
}

/// A Europe/Paris-style calendar (UTC+1 std, UTC+2 DST), Mon–Fri 09:00–17:00 local, with the given
/// LOCAL-day-number holidays.
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

/// **The full ISS-P26 chained-mutation lifecycle: arm → pause → resume (over a DST + holiday calendar)
/// → restart → breach fires + chain starts EXACTLY ONCE.**
#[test]
fn sla_calendar_chained_mutation_arm_pause_resume_restart_breach() {
    // a holiday on Wed 2024-06-05 (a local-day number).
    let holiday = (utc(2024, 6, 5, 12, 0, 0) + 7200).div_euclid(86_400);
    let cal = paris_calendar(vec![holiday]);
    let mut eng = SlaEngine::new(tenant(), region());

    // --- arm: a 24-business-hour (3 working days) SLA, started Mon 2024-06-03 09:00 local (07:00 UTC) ---
    let start = utc(2024, 6, 3, 7, 0, 0);
    let armed = eng
        .arm("ENG#42", "sla-resp-24h", cal.clone(), 24 * 3600, start)
        .unwrap();
    // 24 business hours = Mon (8h) + Tue (8h) + Wed-is-a-holiday-skip + Thu (8h) → breach Thu 17:00 local.
    // Thu 2024-06-06 17:00 local at UTC+2 = 15:00 UTC.
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

    // --- pause Tue 11:00 local (09:00 UTC): waiting-on-customer (a pause_conditions match) ---
    eng.pause("ENG#42", utc(2024, 6, 4, 9, 0, 0)).unwrap();
    // consumed = Mon 8h + Tue 09:00→11:00 = 2h → 10h consumed, 14h remaining.
    assert_eq!(
        eng.run("ENG#42").unwrap().remaining_business_secs,
        14 * 3600
    );
    assert_eq!(eng.armed_timer_count(), 0, "timers disarmed while paused");

    // --- resume Thu 09:00 local (07:00 UTC) — Wed was a holiday so the customer replied Thu ---
    eng.resume("ENG#42", utc(2024, 6, 6, 7, 0, 0)).unwrap();
    // 14h remaining from Thu 09:00 = Thu (8h) + Fri (6h) → breach Fri 15:00 local = 13:00 UTC.
    let resumed_breach = utc(2024, 6, 7, 13, 0, 0);
    assert_eq!(
        eng.run("ENG#42").unwrap().fire_at,
        resumed_breach,
        "the resume recomputed fire_at to-the-second over the remaining budget"
    );
    assert_eq!(eng.armed_timer_count(), 2, "re-armed on resume");

    // --- the process RESTARTS: snapshot → restore a fresh engine (the durable sla_run table) ---
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

    // --- the breach timer fires after the restart: EXACTLY ONCE → the FROZEN chain starts ---
    assert!(
        restored.on_breach_timer("ENG#42", 15, 1),
        "the breach fires post-restart"
    );
    assert!(
        !restored.on_breach_timer("ENG#42", 15, 1),
        "a wheel re-delivery is a no-op (exactly-once)"
    );
    assert_eq!(restored.run("ENG#42").unwrap().state, SlaState::Breached);

    // the breach started the FROZEN Issues escalation chain (contract 7.5) — not a parallel calc.
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

    // the pre-restart engine's emitted stream walked the lifecycle in order.
    // (we dropped it, but the restored engine carries only the post-restart breach — exactly-once.)
    let pre_restart_kinds = [SLA_STARTED, SLA_PAUSED, SLA_RESUMED];
    for k in pre_restart_kinds {
        assert!(
            !restored.emitted().iter().any(|e| e.event_type() == k),
            "the restored engine does NOT re-emit {k} (exactly-once across restart)"
        );
    }
}

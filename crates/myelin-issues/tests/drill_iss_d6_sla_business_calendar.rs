//! # ISS-D6 — SLA durability: the business-calendar `fire_at` to-the-second + breach-after-restart
//! (ISS-P26 / P-393, M4-I7)
//!
//! **Drill source:**
//! `planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! row **ISS-D6** ("(a) breach fires after a process restart; (b) a business-calendar corpus — DST,
//! multi-day, holiday, pause/resume → computed `fire_at` matches wall-clock TO THE SECOND; (c) breach
//! starts the escalation chain" — artifact: fire-at accuracy (to-the-second) + chain-start, CI).
//! Architecture `issue-tracker/architecture/02-internals-and-algorithms.md` §6.2 (the business-calendar
//! SLA arithmetic — the genuinely-owned hard part).
//!
//! This is the **ISS-P26 (a)+(b) half** of ISS-D6 (the business-calendar `fire_at` accuracy + the
//! breach-after-restart over the durable timer seam). The **(c) chain-start** integration with Notif's
//! frozen [`myelin_notif::escalation::EscalationEngine`] on the live wheel is the NOTIF-P21 half
//! (`drill_iss_d6_sla_escalation.rs`); here (c) is proven AT THE ISSUES SEAM — the breach STARTS the
//! FROZEN Issues escalation chain ([`myelin_issues::sla_escalation`], contract 7.5), which the NOTIF-P21
//! drill then walks on the durable wheel.
//!
//! **The dated GREEN artifact (2026-06-23).** Over [`myelin_issues::sla_calendar::SlaEngine`] (the
//! business-calendar arithmetic + the CONSUMED `myelin-flow` durable-timer seam 9.3), the drill measures
//! + asserts, with NO threshold weakened:
//!
//! 1. **(b) the business-calendar corpus — to-the-second.** A deterministic corpus (DST spring-forward,
//!    DST fall-back, a multi-day weekend span, a holiday boundary, a mid-window pause/resume) → the
//!    computed `fire_at` matches the hand-computed wall-clock instant EXACTLY (0-second error).
//! 2. **(a) breach fires after a process restart.** An armed SLA is snapshotted; the engine is KILLED
//!    (dropped); a NEW engine restores from the durable `sla_run` rows + re-arms the SAME `fire_at`; the
//!    breach timer fires post-restart → EXACTLY ONE breach (a re-delivery fires nothing — the state
//!    guard). 0 lost, 0 double-fire.
//! 3. **(c) breach starts the escalation chain.** The winning breach emits `issue.sla.breached` naming
//!    the FROZEN Issues escalation policy id (team → project → org) — the chain Notif walks (NOTIF-P21).
//!
//! Threshold: 0-second `fire_at` error across the corpus; 1 breach across a restart (0 missed, 0
//! duplicate); chain-start named.

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

/// One business-calendar corpus case: a (start, budget) over a calendar → an expected wall-clock UTC
/// `fire_at`. The drill asserts the computed value matches TO THE SECOND.
struct CorpusCase {
    name: &'static str,
    cal: Calendar,
    start: i64,
    budget_secs: i64,
    expected_fire_at: i64,
}

/// **(b) the business-calendar corpus — DST / multi-day / holiday → `fire_at` to-the-second.** Each case
/// is hand-computed; the drill asserts a 0-second error. This is the genuinely-owned arithmetic
/// (`business_fire_at`) under the exact corpus the ISS-D6 row names.
#[test]
fn iss_d6_b_business_calendar_fire_at_to_the_second() {
    let corpus = vec![
        // a budget within one working day (2h from Mon 09:00 local UTC+2 → 11:00 local = 09:00 UTC).
        CorpusCase {
            name: "within-one-day",
            cal: paris_calendar(vec![]),
            start: utc(2024, 6, 3, 7, 0, 0),
            budget_secs: 2 * 3600,
            expected_fire_at: utc(2024, 6, 3, 9, 0, 0),
        },
        // a multi-day weekend span (12h from Fri 15:00 local → 2h Fri + 8h Mon + 2h Tue = Tue 11:00 local).
        CorpusCase {
            name: "multi-day-weekend",
            cal: paris_calendar(vec![]),
            start: utc(2024, 6, 7, 13, 0, 0),
            budget_secs: 12 * 3600,
            expected_fire_at: utc(2024, 6, 11, 9, 0, 0),
        },
        // a holiday boundary (Mon is a holiday → a Fri-afternoon budget lands Tue, not Mon).
        CorpusCase {
            name: "holiday-skip",
            cal: paris_calendar(vec![local_day(2024, 6, 3, 7200)]),
            start: utc(2024, 5, 31, 14, 0, 0),
            budget_secs: 2 * 3600,
            expected_fire_at: utc(2024, 6, 4, 8, 0, 0),
        },
        // DST spring-forward weekend (the Monday window uses the post-DST UTC+2 offset).
        CorpusCase {
            name: "dst-spring-forward",
            cal: paris_calendar(vec![]),
            start: utc(2024, 3, 29, 15, 0, 0),
            budget_secs: 2 * 3600,
            expected_fire_at: utc(2024, 4, 1, 8, 0, 0),
        },
        // DST fall-back weekend (the Monday window uses the post-fall-back UTC+1 offset).
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
    // the dated green artifact: the fire-at accuracy is 0-second across the whole corpus.
    assert_eq!(
        max_error_secs, 0,
        "ISS-D6 (b) GREEN: 0-second fire_at error across the DST/multi-day/holiday corpus"
    );
}

/// **(b) pause/resume mid-window → the total budget is preserved to-the-second.** A pause stores the
/// remaining business seconds; the resume recomputes `fire_at` over the remaining budget — the sum of
/// run + remaining equals the original target, to-the-second.
#[test]
fn iss_d6_b_pause_resume_preserves_budget_to_the_second() {
    let cal = paris_calendar(vec![]);
    let mut eng = SlaEngine::new(tenant(), region());
    // an 8h SLA started Mon 09:00 local (07:00 UTC).
    eng.arm("ENG#6b", "sla-8h", cal, 8 * 3600, utc(2024, 6, 3, 7, 0, 0))
        .unwrap();
    // pause Mon 11:00 local (09:00 UTC) — 2h run, 6h remaining.
    eng.pause("ENG#6b", utc(2024, 6, 3, 9, 0, 0)).unwrap();
    assert_eq!(eng.run("ENG#6b").unwrap().remaining_business_secs, 6 * 3600);
    // resume Tue 09:00 local — 6h remaining → breach Tue 15:00 local = 13:00 UTC, to-the-second.
    eng.resume("ENG#6b", utc(2024, 6, 4, 7, 0, 0)).unwrap();
    assert_eq!(
        eng.run("ENG#6b").unwrap().fire_at,
        utc(2024, 6, 4, 13, 0, 0),
        "ISS-D6 (b) GREEN: pause/resume preserves the budget to-the-second"
    );
}

/// **(a) the breach fires after a process restart + (c) starts the escalation chain — EXACTLY ONCE.**
#[test]
fn iss_d6_a_breach_after_restart_exactly_once_and_c_starts_chain() {
    let cal = paris_calendar(vec![]);
    let mut eng = SlaEngine::new(tenant(), region());
    let start = utc(2024, 6, 3, 7, 0, 0);
    let armed = eng.arm("ENG#6a", "sla-8h", cal, 8 * 3600, start).unwrap();
    let fire_at = armed.fire_at;

    // --- KILL the engine: snapshot the durable sla_run rows, drop the engine, restore a fresh one ---
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

    // --- (a) the breach fires after the restart; a re-delivery is a no-op (exactly-once) ---
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
    // the dated green artifact: 1 breach across the restart (0 missed, 0 duplicate).
    assert_eq!(
        breaches.len(),
        1,
        "ISS-D6 (a) GREEN: EXACTLY ONE breach across the restart (0 missed, 0 duplicate)"
    );

    // --- (c) the breach started the FROZEN Issues escalation chain ---
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

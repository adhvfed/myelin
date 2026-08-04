use myelin_events::OutboxStore;
use myelin_identity::PrincipalId;
use myelin_issues::events::SLA_BREACHED;
use myelin_issues::sla_calendar::{
    Calendar, OffsetTransition, SlaEngine, SlaOutcomeEvent, Weekday,
};
use myelin_issues::sla_escalation::{
    issue_sla_escalation_policy, SLA_ESCALATION_POLICY_ID, SLA_TEAM_ONCALL_SCHEDULE,
};
use myelin_notif::escalation::{EscalationEngine, InMemoryWheel, OncallSchedule, RotationWindow};
use myelin_notif::prefs::QuietHours;
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn pid(s: &str) -> PrincipalId {
    PrincipalId(s.into())
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

fn business_calendar() -> Calendar {
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
        vec![],
        vec![
            OffsetTransition {
                at_utc: i64::MIN / 2,
                offset_secs: 3600,
            },
            OffsetTransition {
                at_utc: utc(2024, 3, 31, 1, 0, 0),
                offset_secs: 7200,
            },
        ],
    )
    .expect("calendar")
}

fn team_schedule() -> OncallSchedule {
    OncallSchedule {
        schedule_id: SLA_TEAM_ONCALL_SCHEDULE.into(),
        rotation: vec![RotationWindow {
            from_minute: 0,
            to_minute: 1440,
            principal: pid("psn:team-lead"),
        }],
    }
}

#[test]
fn producer_sla_engine_breach_starts_the_frozen_chain() {
    let mut eng = SlaEngine::new(tenant(), region());
    eng.arm(
        "ENG#7",
        "sla-8h",
        business_calendar(),
        8 * 3600,
        utc(2024, 6, 3, 7, 0, 0),
    )
    .unwrap();
    assert!(eng.on_breach_timer("ENG#7", 15, 1), "the breach fires");

    let breach = eng
        .emitted()
        .iter()
        .find(|e| e.event_type() == SLA_BREACHED)
        .expect("a breach was emitted");
    match breach {
        SlaOutcomeEvent::Breached {
            escalation_policy_id,
            ..
        } => assert_eq!(
            escalation_policy_id, SLA_ESCALATION_POLICY_ID,
            "the SLA engine starts the FROZEN Issues chain"
        ),
        other => panic!("expected a breach, got {other:?}"),
    }
}

#[test]
fn consumer_notif_pages_the_chain_the_engine_starts() {
    let mut eng = SlaEngine::new(tenant(), region());
    eng.arm(
        "ENG#9",
        "sla-8h",
        business_calendar(),
        8 * 3600,
        utc(2024, 6, 3, 7, 0, 0),
    )
    .unwrap();
    eng.on_breach_timer("ENG#9", 15, 1);
    let policy_id = match eng
        .emitted()
        .iter()
        .find(|e| e.event_type() == SLA_BREACHED)
    {
        Some(SlaOutcomeEvent::Breached {
            escalation_policy_id,
            ..
        }) => escalation_policy_id.clone(),
        _ => panic!("no breach"),
    };

    let chain = issue_sla_escalation_policy(15, 1);
    assert_eq!(
        chain.policy_id, policy_id,
        "the consumer walks the chain the engine started"
    );

    let wheel = InMemoryWheel::new();
    let outbox = OutboxStore::new();
    let engine = EscalationEngine::new(wheel, outbox);
    let breach_subject = ArtifactRef("myelin://acme/issue/issue/ENG-9".into());
    let never_quiet = QuietHours::default();
    let (_run, first) = engine
        .page(
            tenant(),
            region(),
            "esc-iss-9".into(),
            chain,
            breach_subject,
            Some(&team_schedule()),
            600,
            &never_quiet,
            false,
        )
        .expect("the consumer starts the engine's chain");
    assert_eq!(
        first.principal,
        pid("psn:team-lead"),
        "tier 1 (the team on-call) is paged at fire time - the chain the SLA engine started"
    );
}

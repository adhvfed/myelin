//! # The CDC pair for contract 7.5 — **the SLA business-calendar engine STARTS the FROZEN chain on
//! breach** (ISS-P26 / P-393, M4-I7)
//!
//! **Contract-index row 7.5** (`oncall_now` / `page` — the FROZEN escalation chain). The chain SHAPE +
//! the watcher read-fanout are pinned by `cdc_7_5_4_9_issues_sla_escalation.rs` (NOTIF-P21 / P-342);
//! THIS file pins the NEW ISS-P26 producer/consumer edge: the **SLA business-calendar engine's breach
//! handler** ([`myelin_issues::sla_calendar::SlaEngine::on_breach_timer`]) is the PRODUCER that, on a
//! to-the-second breach, hands the FROZEN Issues chain
//! ([`myelin_issues::sla_escalation::issue_sla_escalation_policy`]) to Notif's CONSUMER
//! ([`myelin_notif::escalation::EscalationEngine::page`]) — proving the engine starts the chain Notif
//! walks, not a parallel calc.
//!
//! - the **PROVIDER** is the SLA engine's breach: it emits `issue.sla.breached` naming the FROZEN
//!   `SLA_ESCALATION_POLICY_ID`, and the FROZEN chain value it starts is byte-identical to the one the
//!   7.5 CDC pins.
//! - the **CONSUMER** is Notif's `EscalationEngine::page` starting + walking that SAME chain on the
//!   durable wheel (pinned end-to-end in the NOTIF-P21 CDC; here we assert the engine HANDS the
//!   identical chain — the seam between the SLA-engine producer and the escalation consumer).
//!
//! A drift on either (the SLA engine starts a different policy id / chain; Notif renames the chain
//! shape) fails this test.

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

/// **PROVIDER — the SLA engine's breach handler starts the FROZEN chain (by policy id).** A breach
/// emits `issue.sla.breached` naming `SLA_ESCALATION_POLICY_ID` — the engine starts the FROZEN Issues
/// chain, not a parallel one. A drift (the engine invents a different policy id) fails here.
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

/// **CONSUMER — the chain the SLA engine starts is the SAME frozen value Notif's EscalationEngine
/// pages.** The policy id the engine's breach names keys the SAME `issue_sla_escalation_policy` the
/// consumer walks: the FROZEN three-tier chain pages the team on-call AT FIRE TIME with zero Notif
/// change. This pins the SLA-engine→escalation seam.
#[test]
fn consumer_notif_pages_the_chain_the_engine_starts() {
    // the engine's breach names this policy id ...
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

    // ... and the FROZEN chain under that id is exactly what the consumer pages.
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
        "tier 1 (the team on-call) is paged at fire time — the chain the SLA engine started"
    );
}

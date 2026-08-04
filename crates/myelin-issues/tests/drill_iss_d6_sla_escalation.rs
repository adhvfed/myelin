use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::PrincipalId;
use myelin_issues::events::SLA_BREACHED;
use myelin_issues::sla_escalation::{
    issue_sla_escalation_policy, SLA_PROJECT_ONCALL_SCHEDULE, SLA_TEAM_ONCALL_SCHEDULE,
};
use myelin_notif::escalation::{
    DurableWheel, EscalationEngine, EscalationRun, InMemoryWheel, OncallSchedule, RotationWindow,
    RunState,
};
use myelin_notif::prefs::QuietHours;
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

fn pid(s: &str) -> PrincipalId {
    PrincipalId(s.into())
}

fn schedules() -> Vec<OncallSchedule> {
    vec![
        OncallSchedule {
            schedule_id: SLA_TEAM_ONCALL_SCHEDULE.into(),
            rotation: vec![RotationWindow {
                from_minute: 0,
                to_minute: 1440,
                principal: pid("psn:team-lead"),
            }],
        },
        OncallSchedule {
            schedule_id: SLA_PROJECT_ONCALL_SCHEDULE.into(),
            rotation: vec![RotationWindow {
                from_minute: 0,
                to_minute: 1440,
                principal: pid("psn:project-lead"),
            }],
        },
    ]
}

fn sched_for(id: &str) -> OncallSchedule {
    schedules()
        .into_iter()
        .find(|s| s.schedule_id == id)
        .unwrap()
}

#[test]
fn iss_d6_sla_breach_starts_real_chain_walks_exactly_once_then_ack_halts() {
    let wheel = InMemoryWheel::new();
    let outbox = OutboxStore::new();
    let tenant = TenantId("acme".into());
    let region = Region("fr-par".into());

    let breach = ArtifactRef(format!("myelin://acme/issue/{SLA_BREACHED}/ENG-1421"));
    let policy = issue_sla_escalation_policy(15, 1);
    let never_quiet = QuietHours::default();

    let eng_a = EscalationEngine::new(wheel.clone(), outbox.clone());
    let (run_id, first) = eng_a
        .page(
            tenant.clone(),
            region.clone(),
            "esc-iss-d6".into(),
            policy.clone(),
            breach.clone(),
            Some(&sched_for(SLA_TEAM_ONCALL_SCHEDULE)),
            600,
            &never_quiet,
            false,
        )
        .expect("the SLA breach STARTS the chain (ISS-D6 chain start)");
    assert_eq!(
        first.principal,
        pid("psn:team-lead"),
        "tier 1 = the team on-call"
    );
    assert_eq!(first.walk, 0);
    assert!(
        !first.channels.is_empty(),
        "the critical page delivers (pierces quiet-hours)"
    );
    assert!(
        wheel.has_timer(&run_id),
        "the ack_window durable timer is armed"
    );

    drop(eng_a);

    let eng_b = EscalationEngine::new(wheel.clone(), outbox.clone());
    eng_b.resume_for_test(EscalationRun {
        tenant: tenant.clone(),
        region: region.clone(),
        run_id: run_id.clone(),
        policy,
        trigger_event: breach,
        walk: 0,
        state: RunState::Active,
        acked_by: None,
        pages: vec![(0, pid("psn:team-lead"))],
    });

    let next = eng_b
        .advance(
            &run_id,
            Some(&sched_for(SLA_PROJECT_ONCALL_SCHEDULE)),
            600,
            &never_quiet,
            false,
        )
        .expect("advance ok")
        .expect("the resumed chain pages the next tier (NOT zero)");
    assert_eq!(
        next.principal,
        pid("psn:project-lead"),
        "tier 2 = the project lead"
    );
    assert_eq!(next.walk, 1);

    let run = eng_b.run(&run_id).expect("run present");
    assert_eq!(
        run.pages.len(),
        2,
        "exactly two pages across the kill/resume - 0 missed, 0 duplicate"
    );

    let acked = Timestamp("2026-06-23T12:15:00Z".into());
    assert!(
        eng_b
            .ack(&run_id, pid("psn:project-lead"), acked.clone())
            .expect("ack ok"),
        "the ack halts"
    );
    let run = eng_b.run(&run_id).expect("run present");
    assert_eq!(run.state, RunState::Acked, "the chain HALTED on the ack");
    assert!(
        !eng_b.wheel().has_timer(&run_id),
        "the ack cancelled the durable timer"
    );

    let all_scheds: Vec<OncallSchedule> = schedules();
    let after_ack = eng_b
        .advance(&run_id, all_scheds.first(), 600, &never_quiet, false)
        .expect("advance ok");
    assert_eq!(
        after_ack, None,
        "no page after the ack - the chain is stopped"
    );

    assert!(
        !eng_b.ack(&run_id, pid("psn:other"), acked).expect("ack ok"),
        "the re-ack is a no-op"
    );
    assert_eq!(
        outbox.committed_count(),
        1,
        "exactly one notif.escalation.acked event committed via the outbox (the ONLY emit path)"
    );

}

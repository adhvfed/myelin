use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::PrincipalId;
use myelin_notif::escalation::{
    DurableWheel, EscalationEngine, EscalationPolicy, EscalationRun, InMemoryWheel, OncallSchedule,
    RotationWindow, RunState,
};
use myelin_notif::prefs::QuietHours;
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

fn pid(s: &str) -> PrincipalId {
    PrincipalId(s.into())
}

fn schedule() -> OncallSchedule {
    OncallSchedule {
        schedule_id: "platform-oncall".into(),
        rotation: vec![RotationWindow {
            from_minute: 0,
            to_minute: 1440,
            principal: pid("psn:alice"),
        }],
    }
}

#[test]
fn notif_d7_kill_mid_ack_window_pages_next_step_exactly_once_then_ack_halts() {
    let wheel = InMemoryWheel::new();
    let outbox = OutboxStore::new();
    let tenant = TenantId("acme".into());
    let region = Region("fr-par".into());
    let trigger = ArtifactRef("myelin://acme/issues/issue/42".into());
    let policy = EscalationPolicy::test_chain(15, pid("psn:lead"));
    let never_quiet = QuietHours::default();

    let eng_a = EscalationEngine::new(wheel.clone(), outbox.clone());
    let (run_id, first) = eng_a
        .page(
            tenant.clone(),
            region.clone(),
            "esc-run-1".into(),
            policy.clone(),
            trigger.clone(),
            Some(&schedule()),
            600,
            &never_quiet,
            false,
        )
        .expect("page starts the chain");
    assert_eq!(
        first.principal,
        pid("psn:alice"),
        "the first page reaches the on-call AT FIRE TIME"
    );
    assert_eq!(first.walk, 0);
    assert!(
        wheel.has_timer(&run_id),
        "the ack_window DURABLE timer is armed"
    );

    drop(eng_a);

    let eng_b = EscalationEngine::new(wheel.clone(), outbox.clone());
    eng_b.resume_for_test(EscalationRun {
        tenant: tenant.clone(),
        region: region.clone(),
        run_id: run_id.clone(),
        policy,
        trigger_event: trigger,
        walk: 0,
        state: RunState::Active,
        acked_by: None,
        pages: vec![(0, pid("psn:alice"))],
    });

    let next = eng_b
        .advance(&run_id, Some(&schedule()), 600, &never_quiet, false)
        .expect("advance ok")
        .expect("the resumed chain pages the next step (NOT zero)");
    assert_eq!(
        next.principal,
        pid("psn:lead"),
        "the next step (the secondary lead)"
    );
    assert_eq!(next.walk, 1);

    let replay = eng_b
        .advance(&run_id, Some(&schedule()), 600, &never_quiet, false)
        .expect("advance ok");
    assert_eq!(
        replay, None,
        "a replayed timer fire is a no-op - 0 duplicate page (NOTIF-D7)"
    );

    let run = eng_b.run(&run_id).expect("run present");
    assert_eq!(
        run.pages.len(),
        2,
        "exactly two pages across the kill/resume - 0 missed, 0 duplicate"
    );

    let acked = Timestamp("2026-06-20T12:15:00Z".into());
    assert!(
        eng_b
            .ack(&run_id, pid("psn:lead"), acked.clone())
            .expect("ack ok"),
        "the ack halts"
    );
    let run = eng_b.run(&run_id).expect("run present");
    assert_eq!(run.state, RunState::Acked, "the chain HALTED on the ack");
    assert!(
        !eng_b.wheel().has_timer(&run_id),
        "the ack cancelled the durable timer"
    );

    let after_ack = eng_b
        .advance(&run_id, Some(&schedule()), 600, &never_quiet, false)
        .expect("advance ok");
    assert_eq!(
        after_ack, None,
        "no page after the ack - the chain is stopped"
    );
    assert_eq!(
        eng_b.run(&run_id).unwrap().pages.len(),
        2,
        "still exactly two pages (ack-halt)"
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

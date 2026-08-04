use super::*;
use myelin_events::Timestamp;

fn ts() -> Timestamp {
    Timestamp("2026-06-20T12:00:00Z".into())
}

fn pid(s: &str) -> PrincipalId {
    PrincipalId(s.into())
}

fn tenant() -> TenantId {
    TenantId("acme".into())
}

fn region() -> Region {
    Region("fr-par".into())
}

fn schedule() -> OncallSchedule {
    OncallSchedule {
        schedule_id: "platform-oncall".into(),
        rotation: vec![
            RotationWindow {
                from_minute: 0,
                to_minute: 720,
                principal: pid("psn:alice"),
            },
            RotationWindow {
                from_minute: 720,
                to_minute: 1440,
                principal: pid("psn:bob"),
            },
        ],
    }
}

fn never_quiet() -> QuietHours {
    QuietHours::default()
}

#[test]
fn oncall_now_resolves_the_window_covering_the_instant() {
    let s = schedule();
    assert_eq!(oncall_now(&s, 600), Some(pid("psn:alice")));
    assert_eq!(oncall_now(&s, 780), Some(pid("psn:bob")));
}

#[test]
fn oncall_now_first_covering_window_wins_override_semantics() {
    let s = OncallSchedule {
        schedule_id: "ovr".into(),
        rotation: vec![
            RotationWindow {
                from_minute: 480,
                to_minute: 540,
                principal: pid("psn:carol"),
            },
            RotationWindow {
                from_minute: 0,
                to_minute: 1440,
                principal: pid("psn:dave"),
            },
        ],
    };
    assert_eq!(
        oncall_now(&s, 500),
        Some(pid("psn:carol")),
        "the override window wins inside it"
    );
    assert_eq!(
        oncall_now(&s, 600),
        Some(pid("psn:dave")),
        "the base window outside the override"
    );
}

#[test]
fn oncall_now_uncovered_instant_is_none_not_silent_drop() {
    let s = OncallSchedule {
        schedule_id: "gap".into(),
        rotation: vec![RotationWindow {
            from_minute: 0,
            to_minute: 60,
            principal: pid("psn:eve"),
        }],
    };
    assert_eq!(oncall_now(&s, 600), None);
}

#[test]
fn oncall_now_boundary_is_half_open() {
    let s = schedule();
    assert_eq!(oncall_now(&s, 0), Some(pid("psn:alice")));
    assert_eq!(oncall_now(&s, 720), Some(pid("psn:bob")));
    assert_eq!(oncall_now(&s, 719), Some(pid("psn:alice")));
}

#[test]
fn step_at_walks_the_steps_in_order() {
    let p = EscalationPolicy::test_chain(15, pid("psn:lead"));
    assert_eq!(
        p.step_at(0).unwrap().target,
        EscalationTarget::Schedule("platform-oncall".into())
    );
    assert_eq!(
        p.step_at(1).unwrap().target,
        EscalationTarget::Principal(pid("psn:lead"))
    );
}

#[test]
fn step_at_exhausts_after_the_last_step_when_repeat_one() {
    let p = EscalationPolicy::test_chain(15, pid("psn:lead"));
    assert!(p.step_at(0).is_some());
    assert!(p.step_at(1).is_some());
    assert!(
        p.step_at(2).is_none(),
        "walk 2 is exhausted (gave up after the chain walked once)"
    );
}

#[test]
fn step_at_repeat_wraps_the_chain() {
    let mut p = EscalationPolicy::test_chain(15, pid("psn:lead"));
    p.repeat = 2;
    assert_eq!(
        p.step_at(2).unwrap().target,
        EscalationTarget::Schedule("platform-oncall".into())
    );
    assert_eq!(
        p.step_at(3).unwrap().target,
        EscalationTarget::Principal(pid("psn:lead"))
    );
    assert!(
        p.step_at(4).is_none(),
        "walk 4 is exhausted (both loops done)"
    );
}

#[test]
fn step_at_empty_policy_is_none() {
    let p = EscalationPolicy {
        policy_id: "x".into(),
        steps: vec![],
        repeat: 3,
    };
    assert!(p.step_at(0).is_none());
}

#[test]
fn step_at_zero_repeat_is_treated_as_one_loop() {
    let mut p = EscalationPolicy::test_chain(15, pid("psn:lead"));
    p.repeat = 0;
    assert!(p.step_at(0).is_some());
    assert!(p.step_at(1).is_some());
    assert!(p.step_at(2).is_none());
}

#[test]
fn notify_for_critical_pierces_quiet_hours_on_all_channels() {
    let chans = vec![Channel::InApp, Channel::WebPush];
    let out = notify_for(&chans, Class::Critical, &never_quiet(), true);
    assert_eq!(
        out, chans,
        "critical pierces - you cannot silence an on-call page"
    );
}

#[test]
fn notify_for_noncritical_in_quiet_window_is_in_app_only() {
    let chans = vec![Channel::InApp, Channel::WebPush];
    let out = notify_for(&chans, Class::Watching, &never_quiet(), true);
    assert_eq!(out, vec![Channel::InApp]);
}

#[test]
fn notify_for_critical_pierces_even_if_pierce_classes_omits_critical() {
    let chans = vec![Channel::InApp, Channel::WebPush];
    let no_pierce = QuietHours {
        tz: crate::prefs::Tz::UTC,
        windows: vec![],
        pierce_classes: vec![],
    };
    let out = notify_for(
        &chans,
        Class::Critical,
        &no_pierce,
         true,
    );
    assert_eq!(
        out, chans,
        "critical pages all channels regardless of pierce_classes config"
    );
}

#[test]
fn notify_for_noncritical_outside_quiet_window_pages_all() {
    let chans = vec![Channel::InApp, Channel::WebPush];
    let out = notify_for(&chans, Class::Watching, &never_quiet(), false);
    assert_eq!(out, chans, "not in a quiet window → all channels");
}

#[test]
fn wheel_fire_due_is_effectively_once() {
    let w = InMemoryWheel::new();
    w.schedule_timer("run-1", 15);
    assert!(w.has_timer("run-1"));
    assert!(w.fire_due("run-1"), "the FIRST fire does work");
    assert!(
        !w.fire_due("run-1"),
        "a re-fire (restart replay) is a no-op - no double page"
    );
    assert!(!w.has_timer("run-1"), "a fired timer is no longer live");
}

#[test]
fn wheel_cancel_disarms() {
    let w = InMemoryWheel::new();
    w.schedule_timer("run-1", 15);
    w.cancel_timer("run-1");
    assert!(!w.has_timer("run-1"));
    assert!(!w.fire_due("run-1"), "a cancelled timer never fires");
}

#[test]
fn wheel_reschedule_replaces_the_handle() {
    let w = InMemoryWheel::new();
    w.schedule_timer("run-1", 15);
    assert!(w.fire_due("run-1"));
    w.schedule_timer("run-1", 30);
    assert!(w.has_timer("run-1"));
    assert!(w.fire_due("run-1"), "the re-armed timer fires once");
}

fn engine() -> EscalationEngine<InMemoryWheel> {
    EscalationEngine::new(InMemoryWheel::new(), OutboxStore::new())
}

#[test]
fn page_starts_the_chain_and_pages_the_first_on_call_at_fire_time() {
    let eng = engine();
    let policy = EscalationPolicy::test_chain(15, pid("psn:lead"));
    let (run_id, outcome) = eng
        .page(
            tenant(),
            region(),
            "run-1".into(),
            policy,
            ArtifactRef("myelin://acme/issues/issue/7".into()),
            Some(&schedule()),
            780,
            &never_quiet(),
            true,
        )
        .expect("page starts");
    assert_eq!(
        outcome.principal,
        pid("psn:bob"),
        "the first page reaches the on-call AT FIRE TIME"
    );
    assert_eq!(
        outcome.channels,
        vec![Channel::InApp, Channel::WebPush],
        "critical pierces all channels"
    );
    assert_eq!(outcome.walk, 0);
    let run = eng.run(&run_id).unwrap();
    assert_eq!(run.state, RunState::Active);
    assert_eq!(run.pages.len(), 1, "exactly one page so far");
    assert!(
        eng.wheel().has_timer(&run_id),
        "the ack_window durable timer is armed"
    );
}

#[test]
fn empty_policy_surfaces_a_config_error_not_a_silent_drop() {
    let eng = engine();
    let policy = EscalationPolicy {
        policy_id: "x".into(),
        steps: vec![],
        repeat: 1,
    };
    let err = eng
        .page(
            tenant(),
            region(),
            "run-x".into(),
            policy,
            ArtifactRef("myelin://acme/x".into()),
            Some(&schedule()),
            600,
            &never_quiet(),
            false,
        )
        .unwrap_err();
    assert_eq!(err, EscalationError::EmptyPolicy);
}

#[test]
fn page_surfaces_no_one_on_call_when_the_schedule_does_not_cover() {
    let eng = engine();
    let policy = EscalationPolicy::test_chain(15, pid("psn:lead"));
    let gap = OncallSchedule {
        schedule_id: "platform-oncall".into(),
        rotation: vec![RotationWindow {
            from_minute: 0,
            to_minute: 60,
            principal: pid("psn:eve"),
        }],
    };
    let err = eng
        .page(
            tenant(),
            region(),
            "run-g".into(),
            policy,
            ArtifactRef("myelin://acme/x".into()),
            Some(&gap),
            600,
            &never_quiet(),
            false,
        )
        .unwrap_err();
    assert_eq!(err, EscalationError::NoOneOnCall("platform-oncall".into()));
}

#[test]
fn advance_walks_to_the_next_step_exactly_once() {
    let eng = engine();
    let policy = EscalationPolicy::test_chain(15, pid("psn:lead"));
    let (run_id, _) = eng
        .page(
            tenant(),
            region(),
            "run-1".into(),
            policy,
            ArtifactRef("myelin://acme/x".into()),
            Some(&schedule()),
            600,
            &never_quiet(),
            false,
        )
        .unwrap();
    let next = eng
        .advance(&run_id, Some(&schedule()), 600, &never_quiet(), false)
        .unwrap()
        .expect("a page to the next step");
    assert_eq!(next.principal, pid("psn:lead"));
    assert_eq!(next.walk, 1);
    let run = eng.run(&run_id).unwrap();
    assert_eq!(
        run.pages.len(),
        2,
        "exactly two pages total - never zero, never three"
    );
}

#[test]
fn advance_is_effectively_once_a_replayed_fire_does_not_double_page() {
    let eng = engine();
    let policy = EscalationPolicy::test_chain(15, pid("psn:lead"));
    let (run_id, _) = eng
        .page(
            tenant(),
            region(),
            "run-1".into(),
            policy,
            ArtifactRef("myelin://acme/x".into()),
            Some(&schedule()),
            600,
            &never_quiet(),
            false,
        )
        .unwrap();
    let _ = eng
        .advance(&run_id, Some(&schedule()), 600, &never_quiet(), false)
        .unwrap();
    let replay = eng
        .advance(&run_id, Some(&schedule()), 600, &never_quiet(), false)
        .unwrap();
    assert_eq!(
        replay, None,
        "a replayed fire is a no-op - no double page (NOTIF-D7)"
    );
    assert_eq!(
        eng.run(&run_id).unwrap().pages.len(),
        2,
        "still exactly two pages"
    );
}

#[test]
fn advance_past_the_last_step_exhausts_the_chain() {
    let eng = engine();
    let policy = EscalationPolicy::test_chain(15, pid("psn:lead"));
    let (run_id, _) = eng
        .page(
            tenant(),
            region(),
            "run-1".into(),
            policy,
            ArtifactRef("myelin://acme/x".into()),
            Some(&schedule()),
            600,
            &never_quiet(),
            false,
        )
        .unwrap();
    let _ = eng
        .advance(&run_id, Some(&schedule()), 600, &never_quiet(), false)
        .unwrap();
    let exhausted = eng
        .advance(&run_id, Some(&schedule()), 600, &never_quiet(), false)
        .unwrap();
    assert_eq!(exhausted, None);
    assert_eq!(eng.run(&run_id).unwrap().state, RunState::Exhausted);
}

#[test]
fn ack_halts_the_chain_and_emits_the_outbox_event() {
    let outbox = OutboxStore::new();
    let eng = EscalationEngine::new(InMemoryWheel::new(), outbox.clone());
    let policy = EscalationPolicy::test_chain(15, pid("psn:lead"));
    let (run_id, _) = eng
        .page(
            tenant(),
            region(),
            "run-1".into(),
            policy,
            ArtifactRef("myelin://acme/x".into()),
            Some(&schedule()),
            600,
            &never_quiet(),
            false,
        )
        .unwrap();
    let halted = eng.ack(&run_id, pid("psn:alice"), ts()).unwrap();
    assert!(halted, "the ack halted the chain");
    let run = eng.run(&run_id).unwrap();
    assert_eq!(run.state, RunState::Acked);
    assert_eq!(run.acked_by, Some(pid("psn:alice")));
    assert!(
        !eng.wheel().has_timer(&run_id),
        "the ack cancelled the durable timer"
    );
    assert_eq!(
        outbox.committed_count(),
        1,
        "notif.escalation.acked committed via the outbox"
    );
}

#[test]
fn ack_is_idempotent_a_double_ack_acks_once() {
    let outbox = OutboxStore::new();
    let eng = EscalationEngine::new(InMemoryWheel::new(), outbox.clone());
    let policy = EscalationPolicy::test_chain(15, pid("psn:lead"));
    let (run_id, _) = eng
        .page(
            tenant(),
            region(),
            "run-1".into(),
            policy,
            ArtifactRef("myelin://acme/x".into()),
            Some(&schedule()),
            600,
            &never_quiet(),
            false,
        )
        .unwrap();
    assert!(
        eng.ack(&run_id, pid("psn:alice"), ts()).unwrap(),
        "first ack halts"
    );
    assert!(
        !eng.ack(&run_id, pid("psn:bob"), ts()).unwrap(),
        "second ack is a no-op"
    );
    assert_eq!(outbox.committed_count(), 1, "the re-ack did NOT re-emit");
    assert_eq!(
        eng.run(&run_id).unwrap().acked_by,
        Some(pid("psn:alice")),
        "the first acker stands"
    );
}

#[test]
fn ack_after_the_window_fired_still_halts_but_a_fired_timer_does_not_page() {
    let eng = engine();
    let policy = EscalationPolicy::test_chain(15, pid("psn:lead"));
    let (run_id, _) = eng
        .page(
            tenant(),
            region(),
            "run-1".into(),
            policy,
            ArtifactRef("myelin://acme/x".into()),
            Some(&schedule()),
            600,
            &never_quiet(),
            false,
        )
        .unwrap();
    assert!(eng.ack(&run_id, pid("psn:alice"), ts()).unwrap());
    let late = eng
        .advance(&run_id, Some(&schedule()), 600, &never_quiet(), false)
        .unwrap();
    assert_eq!(late, None, "no page after an ack - the chain is halted");
    assert_eq!(
        eng.run(&run_id).unwrap().pages.len(),
        1,
        "still exactly one page (acked at step 0)"
    );
}

#[test]
fn ack_unknown_run_surfaces_an_error() {
    let eng = engine();
    let err = eng.ack("nope", pid("psn:alice"), ts()).unwrap_err();
    assert_eq!(err, EscalationError::UnknownRun("nope".into()));
}

#[test]
fn render_oncall_shows_who_is_on_call_now_and_the_windows() {
    let out = render_oncall(&schedule(), 600);
    assert!(
        out.contains("now on call: psn:alice"),
        "shows the current on-call (opaque pseudonym)"
    );
    assert!(
        out.contains("00:00–12:00) → psn:alice"),
        "shows the rotation windows"
    );
    assert!(out.contains("12:00–24:00) → psn:bob"));
}

#[test]
fn render_oncall_handles_an_uncovered_window() {
    let s = OncallSchedule {
        schedule_id: "gap".into(),
        rotation: vec![RotationWindow {
            from_minute: 0,
            to_minute: 60,
            principal: pid("psn:eve"),
        }],
    };
    let out = render_oncall(&s, 600);
    assert!(
        out.contains("none - uncovered window"),
        "surfaces no-one-on-call, never a silent gap"
    );
}

#[test]
fn render_page_shows_the_paged_principal_and_channels() {
    let outcome = PageOutcome {
        principal: pid("psn:alice"),
        channels: vec![Channel::InApp, Channel::WebPush],
        walk: 0,
    };
    let out = render_page(&outcome);
    assert!(out.contains("paged psn:alice"));
    assert!(
        out.contains("in_app, web_push"),
        "shows the pierce-result channels"
    );
    assert!(out.contains("step 0"));
    assert!(out.contains("pierces quiet-hours"));
}

#[test]
fn chained_kill_mid_ack_window_resumes_and_pages_next_step_exactly_once() {
    let wheel = InMemoryWheel::new();
    let outbox = OutboxStore::new();

    let eng_a = EscalationEngine::new(wheel.clone(), outbox.clone());
    let policy = EscalationPolicy::test_chain(15, pid("psn:lead"));
    let (run_id, first) = eng_a
        .page(
            tenant(),
            region(),
            "run-1".into(),
            policy.clone(),
            ArtifactRef("myelin://acme/issues/issue/7".into()),
            Some(&schedule()),
            600,
            &never_quiet(),
            false,
        )
        .unwrap();
    assert_eq!(
        first.principal,
        pid("psn:alice"),
        "first page to the on-call (minute 600 → alice)"
    );
    assert!(wheel.has_timer(&run_id), "the ack_window timer is armed");

    drop(eng_a);
    let eng_b = EscalationEngine::new(wheel.clone(), outbox.clone());
    eng_b.resume_for_test(EscalationRun {
        tenant: tenant(),
        region: region(),
        run_id: run_id.clone(),
        policy,
        trigger_event: ArtifactRef("myelin://acme/issues/issue/7".into()),
        walk: 0,
        state: RunState::Active,
        acked_by: None,
        pages: vec![(0, pid("psn:alice"))],
    });

    let next = eng_b
        .advance(&run_id, Some(&schedule()), 600, &never_quiet(), false)
        .unwrap()
        .expect("the resumed chain pages the next step");
    assert_eq!(
        next.principal,
        pid("psn:lead"),
        "the next step (the fixed secondary lead)"
    );
    assert_eq!(next.walk, 1);

    assert_eq!(
        eng_b
            .advance(&run_id, Some(&schedule()), 600, &never_quiet(), false)
            .unwrap(),
        None,
        "no double page across the kill/resume - exactly once (NOTIF-D7)"
    );

    assert!(
        eng_b.ack(&run_id, pid("psn:lead"), ts()).unwrap(),
        "the ack halts the chain"
    );
    assert_eq!(eng_b.run(&run_id).unwrap().state, RunState::Acked);
    assert_eq!(eng_b.run(&run_id).unwrap().pages.len(), 2);
    assert_eq!(outbox.committed_count(), 1, "one ack event committed");
}

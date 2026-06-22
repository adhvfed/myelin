//! Unit tests for the escalation chain-walk POLICY (NOTIF-P14 / P-192): step ordering + repeat
//! wraparound + exhaustion ([`EscalationPolicy::step_at`]), rotation resolution at fire time
//! ([`oncall_now`]), the pierce decision ([`notify_for`] — critical ALWAYS pierces), the chain-walk
//! state machine (one page per step), and the idempotent ack-halt. The chained durability test
//! (start → kill mid-`ack_window` → resume → page next step EXACTLY ONCE → ack halts) lives here and
//! in the drill harness. The mandatory-core decision logic is exercised to the ≥80% mutation floor.

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
            // 00:00–12:00 → alice; 12:00–24:00 → bob.
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

// ---- oncall_now: rotation resolution AT FIRE TIME (the POLICY) -----------------------------------

#[test]
fn oncall_now_resolves_the_window_covering_the_instant() {
    let s = schedule();
    // 10:00 (minute 600) is alice's window.
    assert_eq!(oncall_now(&s, 600), Some(pid("psn:alice")));
    // 13:00 (minute 780) is bob's window — resolved AT FIRE TIME, not policy-author time.
    assert_eq!(oncall_now(&s, 780), Some(pid("psn:bob")));
}

#[test]
fn oncall_now_first_covering_window_wins_override_semantics() {
    let s = OncallSchedule {
        schedule_id: "ovr".into(),
        rotation: vec![
            // An override window first (08:00–09:00 → carol), then the base all-day → dave.
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
    // minute 600 is uncovered → None (no one on call — surfaced, never silently dropped).
    assert_eq!(oncall_now(&s, 600), None);
}

#[test]
fn oncall_now_boundary_is_half_open() {
    let s = schedule();
    // [0,720): minute 0 is alice (inclusive start), minute 720 is bob (exclusive end of alice's).
    assert_eq!(oncall_now(&s, 0), Some(pid("psn:alice")));
    assert_eq!(oncall_now(&s, 720), Some(pid("psn:bob")));
    assert_eq!(oncall_now(&s, 719), Some(pid("psn:alice")));
}

// ---- step_at: step ordering + repeat wraparound + exhaustion (the POLICY) ------------------------

#[test]
fn step_at_walks_the_steps_in_order() {
    let p = EscalationPolicy::test_chain(15, pid("psn:lead"));
    // walk 0 → step 0 (schedule); walk 1 → step 1 (the fixed secondary lead).
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
    let p = EscalationPolicy::test_chain(15, pid("psn:lead")); // 2 steps, repeat 1 → walks 0,1 live.
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
    p.repeat = 2; // 2 steps × 2 loops → walks 0..4 live, mapped 0→s0, 1→s1, 2→s0, 3→s1.
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
    p.repeat = 0; // a degenerate config → at least one loop (repeat.max(1)).
    assert!(p.step_at(0).is_some());
    assert!(p.step_at(1).is_some());
    assert!(p.step_at(2).is_none());
}

// ---- notify_for: the pierce decision — critical ALWAYS pierces (NOTIF-D8) ------------------------

#[test]
fn notify_for_critical_pierces_quiet_hours_on_all_channels() {
    let chans = vec![Channel::InApp, Channel::WebPush];
    // Even WITH the recipient in a quiet window, a CRITICAL escalation pages every channel (§2.4).
    let out = notify_for(&chans, Class::Critical, &never_quiet(), true);
    assert_eq!(
        out, chans,
        "critical pierces — you cannot silence an on-call page"
    );
}

#[test]
fn notify_for_noncritical_in_quiet_window_is_in_app_only() {
    let chans = vec![Channel::InApp, Channel::WebPush];
    // A non-piercing (watching) class inside a quiet window → in-app only (off-cell push silenced).
    let out = notify_for(&chans, Class::Watching, &never_quiet(), true);
    assert_eq!(out, vec![Channel::InApp]);
}

#[test]
fn notify_for_critical_pierces_even_if_pierce_classes_omits_critical() {
    // The HARD invariant: a CRITICAL escalation pierces quiet-hours UNCONDITIONALLY — even a
    // (misconfigured) quiet-hours whose pierce_classes does NOT list Critical cannot silence an
    // on-call page. `notify_for` checks `class == Critical` FIRST, independent of pierce_classes.
    let chans = vec![Channel::InApp, Channel::WebPush];
    let no_pierce = QuietHours {
        tz: crate::prefs::Tz::UTC,
        windows: vec![],
        pierce_classes: vec![], // deliberately empty — Critical is NOT in the pierce set.
    };
    let out = notify_for(
        &chans,
        Class::Critical,
        &no_pierce,
        /* in_quiet */ true,
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

// ---- the InMemoryWheel: effectively-once fire (the no-double-page anchor) ------------------------

#[test]
fn wheel_fire_due_is_effectively_once() {
    let w = InMemoryWheel::new();
    w.schedule_timer("run-1", 15);
    assert!(w.has_timer("run-1"));
    assert!(w.fire_due("run-1"), "the FIRST fire does work");
    assert!(
        !w.fire_due("run-1"),
        "a re-fire (restart replay) is a no-op — no double page"
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
    // Re-arm (the guarded UPDATE for the next step) → a fresh live handle.
    w.schedule_timer("run-1", 30);
    assert!(w.has_timer("run-1"));
    assert!(w.fire_due("run-1"), "the re-armed timer fires once");
}

// ---- the engine: page → advance → ack chain-walk (one page per step) -----------------------------

fn engine() -> EscalationEngine<InMemoryWheel> {
    EscalationEngine::new(InMemoryWheel::new(), OutboxStore::new())
}

#[test]
fn page_starts_the_chain_and_pages_the_first_on_call_at_fire_time() {
    let eng = engine();
    let policy = EscalationPolicy::test_chain(15, pid("psn:lead"));
    // Fire at 13:00 (minute 780) → bob is on call (resolved at fire time, not author time).
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
    // The ack_window timer fires (unacked) → walk to step 1 (the fixed lead), page exactly once.
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
        "exactly two pages total — never zero, never three"
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
    // A restart replays the SAME timer fire → the wheel's fire_due returns false → NO second page.
    let replay = eng
        .advance(&run_id, Some(&schedule()), 600, &never_quiet(), false)
        .unwrap();
    assert_eq!(
        replay, None,
        "a replayed fire is a no-op — no double page (NOTIF-D7)"
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
    let policy = EscalationPolicy::test_chain(15, pid("psn:lead")); // 2 steps, repeat 1.
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
    // walk 0 → 1 (page lead); the next fire walks past the last step → exhausted, no page.
    let _ = eng
        .advance(&run_id, Some(&schedule()), 600, &never_quiet(), false)
        .unwrap();
    let exhausted = eng
        .advance(&run_id, Some(&schedule()), 600, &never_quiet(), false)
        .unwrap();
    assert_eq!(exhausted, None);
    assert_eq!(eng.run(&run_id).unwrap().state, RunState::Exhausted);
}

// ---- ack-as-event: HALT the chain idempotently (the outbox emit) ---------------------------------

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
    // The ack EVENT rode the outbox (the ONLY emit path) — exactly one committed row.
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
    // Idempotent: only ONE ack event ever emitted (no double-resolve of the signal-wait).
    assert_eq!(outbox.committed_count(), 1, "the re-ack did NOT re-emit");
    assert_eq!(
        eng.run(&run_id).unwrap().acked_by,
        Some(pid("psn:alice")),
        "the first acker stands"
    );
}

#[test]
fn ack_after_the_window_fired_still_halts_but_a_fired_timer_does_not_page() {
    // The ack-halt vs fire race: if an ack arrives AFTER the timer already fired and walked, the run
    // is no longer at the same step — but a fresh fire on an ACKED run is a no-op (the ack wins).
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
    // A late timer fire after the ack: the wheel was cancelled, so fire_due is false → no page.
    let late = eng
        .advance(&run_id, Some(&schedule()), 600, &never_quiet(), false)
        .unwrap();
    assert_eq!(late, None, "no page after an ack — the chain is halted");
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

// ---- CLI: myelin oncall show | page (the operator surface, PII-minimised) ------------------------

#[test]
fn render_oncall_shows_who_is_on_call_now_and_the_windows() {
    let out = render_oncall(&schedule(), 600); // minute 600 → alice on call.
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
    let out = render_oncall(&s, 600); // uncovered.
    assert!(
        out.contains("none — uncovered window"),
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

// ---- THE CHAINED DURABILITY TEST (EI-01 §4) — start → kill mid-ack_window → resume → exactly once -

/// **The NOTIF-D7 chained durability property in a unit test (EI-01 §4):** start an escalation →
/// "kill" Notif mid-`ack_window` (drop the engine, keep the durable wheel + the persisted run
/// handle) → resume on a NEW engine over the SAME durable state → the timer fires → page the next
/// step EXACTLY ONCE (not zero, not two) → deliver the ack → the chain HALTS. This is the
/// kill-and-resume that proves the durable handle resumes the chain without missing or double-paging.
#[test]
fn chained_kill_mid_ack_window_resumes_and_pages_next_step_exactly_once() {
    // The DURABLE state that survives a Notif restart: the wheel (the timer) + the outbox.
    let wheel = InMemoryWheel::new();
    let outbox = OutboxStore::new();

    // --- before the kill: start the chain on engine A ---
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

    // --- THE KILL: drop engine A mid-ack_window (before any ack). The durable wheel + outbox stay. ---
    // A real restart loses the in-process EscalationEngine.runs map; here we model resume by
    // rebuilding the run handle from the persisted escalation_run row (run_id, policy, walk=0) onto
    // a FRESH engine over the SAME durable wheel + outbox.
    drop(eng_a);
    let eng_b = EscalationEngine::new(wheel.clone(), outbox.clone());
    // Re-hydrate the persisted run handle (the escalation_run row a restart reads).
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

    // --- after the kill: the ack_window timer fires on the resumed engine → page next step ONCE ---
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

    // A replayed fire (a SECOND restart over the same fired timer) does NOT double-page.
    assert_eq!(
        eng_b
            .advance(&run_id, Some(&schedule()), 600, &never_quiet(), false)
            .unwrap(),
        None,
        "no double page across the kill/resume — exactly once (NOTIF-D7)"
    );

    // --- the ack halts the chain ---
    assert!(
        eng_b.ack(&run_id, pid("psn:lead"), ts()).unwrap(),
        "the ack halts the chain"
    );
    assert_eq!(eng_b.run(&run_id).unwrap().state, RunState::Acked);
    // Exactly two pages total across the whole kill/resume (step 0 before, step 1 after) — 0 missed,
    // 0 duplicate; and exactly one ack event committed to the outbox.
    assert_eq!(eng_b.run(&run_id).unwrap().pages.len(), 2);
    assert_eq!(outbox.committed_count(), 1, "one ack event committed");
}

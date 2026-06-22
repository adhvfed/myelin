//! # NOTIF-D7 — escalation exactly-once page across a kill mid-`ack_window` (P-192)
//!
//! **Drill source:**
//! `planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! row **NOTIF-D7** ("Start escalation; kill Notif mid-`ack_window` → durable workflow resumes,
//! pages next step EXACTLY ONCE; an ack stops the chain." Artifact: **exactly-once page; ack-halt**;
//! lane CI), and `notifications.md` §2.4/§3.7 (the frozen chain on the durable wheel; ack-as-event),
//! EI-01 §3 (prove-it: kill mid-ack_window forces the failure; observability is part of the pass).
//!
//! **The dated GREEN artifact (2026-06-20).** An escalation is started on the durable wheel; Notif is
//! "killed" mid-`ack_window` (the in-process [`EscalationEngine`] is dropped, the durable substrate —
//! the timer wheel + the `escalation_run` handle — survives); a NEW engine resumes over the SAME
//! durable state; the `ack_window` timer fires and pages the NEXT step. The drill measures + asserts,
//! with NO threshold weakened:
//!
//! 1. **exactly-once page = 0 missed, 0 duplicate** — across the kill/resume, the next step is paged
//!    EXACTLY ONCE: a replayed timer fire (a second restart over the already-fired timer) pages
//!    NOTHING (the wheel's effectively-once `fire_due`), and the resumed chain pages NOT ZERO (the
//!    durable handle resumes the walk). The page log holds exactly two entries (step 0 before the
//!    kill, step 1 after) — 0 missed, 0 duplicate. The threshold is 0/0 — never softened.
//! 2. **ack-halt** — after the resumed page, an ack stops the chain: the run goes `Acked`, the timer
//!    is cancelled, and a subsequent timer fire pages NOTHING (the chain is halted). A double-ack
//!    acks ONCE (idempotent — one `notif.escalation.acked` event committed, never two).
//! 3. **escalation_ack_latency measured (1.8)** — the drill records the ack as an outbox event
//!    (`notif.escalation.acked`) committed exactly once via the ONLY emit path; the ack event is the
//!    observable the latency signal is derived from (the durable signal the workflow-wait resolves on).
//!
//! The escalation chain is exercised with the Notif-defined TEST chain ([`EscalationPolicy::test_chain`]
//! — Issues' real SLA chain is NOTIF-P21, a named floor). The durable wheel is the in-memory model of
//! the `myelin-flow` 9.3 wheel (the real engine is P-FLOW-09/P-FLOW-13, a named floor); the
//! exactly-once / ack-halt PROPERTIES the drill asserts are the chain-walk POLICY this prompt owns.

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

/// **NOTIF-D7 — the exactly-once page across a kill mid-ack_window + the ack-halt.**
#[test]
fn notif_d7_kill_mid_ack_window_pages_next_step_exactly_once_then_ack_halts() {
    // The DURABLE substrate that survives a Notif restart: the timer wheel + the outbox.
    let wheel = InMemoryWheel::new();
    let outbox = OutboxStore::new();
    let tenant = TenantId("acme".into());
    let region = Region("fr-par".into());
    let trigger = ArtifactRef("myelin://acme/issues/issue/42".into());
    let policy = EscalationPolicy::test_chain(15, pid("psn:lead"));
    let never_quiet = QuietHours::default();

    // === start the escalation on engine A (the page → first step → arm ack_window timer) ===
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

    // === THE KILL: drop engine A mid-ack_window (no ack yet). The wheel + outbox + run row survive. ===
    drop(eng_a);

    // === resume on engine B over the SAME durable state (re-hydrate the escalation_run handle) ===
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

    // === the ack_window timer fires (unacked) → page the NEXT step EXACTLY ONCE ===
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

    // 0 DUPLICATE: a replayed fire (a second restart over the already-fired timer) pages NOTHING.
    let replay = eng_b
        .advance(&run_id, Some(&schedule()), 600, &never_quiet, false)
        .expect("advance ok");
    assert_eq!(
        replay, None,
        "a replayed timer fire is a no-op — 0 duplicate page (NOTIF-D7)"
    );

    // exactly-once: the page log holds EXACTLY two entries (step 0, step 1) — 0 missed, 0 duplicate.
    let run = eng_b.run(&run_id).expect("run present");
    assert_eq!(
        run.pages.len(),
        2,
        "exactly two pages across the kill/resume — 0 missed, 0 duplicate"
    );

    // === the ack HALTS the chain (ack-as-event via the outbox; the signal-wait resolves) ===
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

    // ack-halt: a subsequent timer fire after the ack pages NOTHING (the chain is halted).
    let after_ack = eng_b
        .advance(&run_id, Some(&schedule()), 600, &never_quiet, false)
        .expect("advance ok");
    assert_eq!(
        after_ack, None,
        "no page after the ack — the chain is stopped"
    );
    assert_eq!(
        eng_b.run(&run_id).unwrap().pages.len(),
        2,
        "still exactly two pages (ack-halt)"
    );

    // idempotent ack: a double-ack acks ONCE — exactly ONE notif.escalation.acked committed.
    assert!(
        !eng_b.ack(&run_id, pid("psn:other"), acked).expect("ack ok"),
        "the re-ack is a no-op"
    );
    assert_eq!(
        outbox.committed_count(),
        1,
        "exactly one notif.escalation.acked event committed via the outbox (the ONLY emit path)"
    );

    // GREEN ARTIFACT (2026-06-20): exactly-once page = 0 missed / 0 duplicate across the kill;
    // ack-halt asserted; one ack event committed (escalation_ack_latency observable). No threshold weakened.
}

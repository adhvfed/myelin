//! # P-GA-21 → P-148 — The DSR deadline durable timer + nearing-deadline warning Signal (GA-D4)
//!
//! **DATED GREEN ARTIFACT (2026-06-20).** This integration drill is the dated green artifact the
//! P-GA-21 GATE (GA-D4) requires (as with the other GDPR drills, the test IS the artifact — there
//! is no GDPR scorecard binary). It proves, end-to-end, the GA-D4 row of the drill catalogue:
//!
//! > **GA-D4** — *Open a DSR → the durable timer fires a warning Signal before the 1-month deadline;
//! > the certificate seals on completion. 0 silent misses.* Plus the restart-resilience leg: kill
//! > the orchestrator between arm and fire → on restart the timer STILL fires (the wheel state
//! > survives).
//!
//! ## The scenario (chained end-to-end over the ALREADY-SHIPPED machinery + the new durable timer)
//! 1. **Open a DSR** through the real [`DsrOrchestrator`] (P-GA-11) — `dsr_submit` records the
//!    request + the coarse `deadline_secs` field; the durable [`DsrDeadlineTimer`] (P-GA-21, the
//!    NEW deliverable) arms a `sleep_until(deadline − warning_margin)` on the minute-bucket wheel,
//!    REPLACING the P-GA-14 coarse-tracking floor.
//! 2. **A tick a month out fires NOTHING** (the deadline is not near — no spurious warning).
//! 3. **A tick at the nearing-deadline point FIRES the warning Signal BEFORE the deadline** — a
//!    PII-free [`DsrDeadlineWarning`] carrying the opaque `dsr_id`, the tenant token, and the
//!    `dsr_deadline_margin` (the seconds of margin remaining, positive). **0 silent misses.**
//! 4. **The certificate seals on completion** — the orchestrator runs the state machine to
//!    `Completed` and `dsr_certificate` yields the content-addressed bundle (the Merkle seal rides
//!    P-GA-20). On completion the (now-moot) warning timer is disarmed.
//! 5. **The restart-resilience leg** — a SECOND DSR is armed, the orchestrator is KILLED between
//!    arm and fire (modelled by snapshotting the durable wheel rows + dropping the timer), and a
//!    FRESH timer restores the wheel. The warning STILL fires at the nearing-deadline point — the
//!    wheel state is durable, not in-process state.
//! 6. **The Art. 12(3) extension re-arms** — a complex request extends the deadline to 3 months
//!    with a recorded reason; the warning re-arms later and the old warning point no longer fires.
//!
//! ## What this proves vs what it reuses (EI-01 §7 coherence)
//! The DSR spine + state machine + coarse deadline ([`DsrOrchestrator`], P-GA-11) is REUSED
//! unchanged; the durable timer ([`DsrDeadlineTimer`] / [`DsrTimerWheel`], P-GA-21) is the new
//! deliverable. The timer models the §9.3 `myelin-flow` minute-bucket wheel deterministically (the
//! real engine is the named floor P-FLOW-13 → P-207; gdpr-service is UPSTREAM of `myelin-flow`, so
//! it carries its own model with byte-for-byte the §9.3 semantics — a config swap when the wheel
//! lands, not a code change).
//!
//! ## Telemetry (observability is part of the pass — EI-01 §3)
//! The warning fires on the `gdpr.dsr_deadline_margin` signal (the GA-D4 telemetry). The
//! `DSR-timer fire` (the warning Signal) + the `sealed cert` (the certificate) are the green
//! artifacts — both are asserted below with measured numbers.

use myelin_gdpr::{EraseScope, SubjectRef};
use myelin_gdpr_service::{
    DsrDeadlineTimer, DsrKind, DsrOrchestrator, DsrState, DsrTimerWheel, Initiator, Posture,
    DSR_DEADLINE_MARGIN,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_substrate::{DsrDeadline, TestClock};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

fn subject(id: &str) -> SubjectRef {
    SubjectRef::new(Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant()))
}

fn subject_scope(s: &str) -> EraseScope {
    EraseScope::Subject { subject: subject(s), tenant: tenant() }
}

#[test]
fn ga_d4_open_dsr_warning_fires_before_deadline_certificate_seals_and_restart_still_fires() {
    let t0 = 1_700_000_000;
    let thr = DsrDeadline::default();

    // The orchestrator (P-GA-11) over the same clock the timer arms against.
    let orch = DsrOrchestrator::new(TestClock::at(t0));
    // The NEW durable deadline timer (P-GA-21) — the deliverable under drill.
    let mut timer = DsrDeadlineTimer::new(TestClock::at(t0), thr.clone());

    // ─────────── 1. open a DSR → arm the durable timer ───────────
    let id = orch.dsr_submit(
        DsrKind::Erasure,
        tenant(),
        subject("p1"),
        subject_scope("p1"),
        Posture::Controller, // platform-operational — Myelin is the controller
        Initiator::Myelin,
    );
    // the orchestrator's coarse deadline field (P-GA-14) ...
    let coarse_deadline = orch.dsr_status(&id).unwrap().deadline_secs;
    // ... is now BACKED by the durable timer (P-GA-21) — the same `now + 1 month`.
    let durable_deadline = timer.arm_deadline(id.clone(), tenant(), t0).unwrap();
    assert_eq!(
        durable_deadline, coarse_deadline,
        "the durable timer's deadline matches the orchestrator's coarse field (the field shape is \
         unchanged — the timer REPLACES the tracking, not the field)"
    );
    assert!(timer.wheel().is_armed(&id), "the durable timer is armed on submit");

    // ─────────── 2. a tick a month out fires NOTHING ───────────
    assert!(timer.tick().is_empty(), "no spurious warning fires a month out");
    assert!(timer.wheel().is_armed(&id), "the timer stays armed");

    // ─────────── 3. the warning fires at the nearing-deadline point, BEFORE the deadline ───────────
    let warning_at = durable_deadline - thr.warning_margin_secs;
    assert!(warning_at < durable_deadline, "the warning point is BEFORE the deadline");
    let fired = timer.tick_at(warning_at);
    // MEASURED: exactly 1 warning Signal fires.
    assert_eq!(fired.len(), 1, "GA-D4: the durable timer fires the warning Signal");
    let w = &fired[0];
    assert_eq!(w.dsr_id, id, "the warning carries the opaque DSR id (PII-free)");
    assert_eq!(w.tenant, tenant(), "the warning carries the PII-free tenant token");
    assert_eq!(w.deadline_secs, durable_deadline);
    // MEASURED: the margin is positive (0 silent misses — the warning fired BEFORE the deadline).
    assert_eq!(w.margin_remaining_secs, thr.warning_margin_secs);
    assert!(w.margin_remaining_secs > 0, "GA-D4: 0 silent misses — the warning fired in time");
    // the telemetry signal the warning fires on (§1.8 — observability is part of the pass).
    assert_eq!(DSR_DEADLINE_MARGIN, ("gdpr.dsr_deadline_margin", "secs"));
    // fire-once: the warning timer is disarmed after firing.
    assert!(!timer.wheel().is_armed(&id), "the warning fired once and is disarmed");

    // ─────────── 4. the certificate seals on completion ───────────
    // run the orchestrator state machine to Completed (the §4.1 happy path).
    orch.validate(&id).unwrap();
    orch.fan_out(&id, &Default::default()).unwrap();
    orch.verify(&id, vec!["receipt-1".into()]).unwrap();
    orch.complete(&id).unwrap();
    assert_eq!(orch.state_of(&id).unwrap(), DsrState::Completed);
    let cert = orch.dsr_certificate(&id).unwrap();
    // MEASURED: the certificate seals (the content-addressed bundle; the Merkle seal rides P-GA-20).
    assert_eq!(cert.dsr_id, id, "GA-D4: the certificate seals on completion");
    assert!(cert.bundle_digest.starts_with("blake3:"));

    // ─────────── 5. the restart-resilience leg: kill between arm and fire → STILL fires ───────────
    let id2 = orch.dsr_submit(
        DsrKind::Access,
        tenant(),
        subject("p2"),
        subject_scope("p2"),
        Posture::Controller,
        Initiator::Myelin,
    );
    let deadline2 = timer.arm_deadline(id2.clone(), tenant(), t0).unwrap();
    // CRASH: snapshot the durable wheel rows, then drop the timer entirely (a process kill).
    let durable_rows = timer.wheel().snapshot();
    assert!(
        durable_rows.iter().any(|r| r.dsr_id == id2),
        "the armed timer is DURABLE state (a row), not in-process state"
    );
    drop(timer);
    // RESTART: a fresh timer (a fresh clock) restores the wheel from the durable rows.
    let warning_at2 = deadline2 - thr.warning_margin_secs;
    let mut restarted = DsrDeadlineTimer::new(TestClock::at(warning_at2), thr.clone());
    restarted.restore_wheel(DsrTimerWheel::restore(durable_rows));
    assert!(restarted.wheel().is_armed(&id2), "the timer survived the restart");
    let fired2 = restarted.tick();
    // MEASURED: the restored timer STILL fires (0 silent misses across a restart).
    assert_eq!(fired2.len(), 1, "GA-D4 restart leg: the timer fires after a kill-and-restart");
    assert_eq!(fired2[0].dsr_id, id2);

    // ─────────── 6. the Art. 12(3) extension re-arms with a recorded reason ───────────
    let id3 = orch.dsr_submit(
        DsrKind::Erasure,
        tenant(),
        subject("p3"),
        subject_scope("p3"),
        Posture::Controller,
        Initiator::Myelin,
    );
    let mut timer3 = DsrDeadlineTimer::new(TestClock::at(t0), thr.clone());
    let one_month = timer3.arm_deadline(id3.clone(), tenant(), t0).unwrap();
    let one_month_warning = timer3.wheel().fire_at_for(&id3).unwrap();
    let reason = "complex: cross-cell member iteration (Art. 12(3))".to_string();
    let three_months = timer3.extend_deadline(&id3, t0, reason.clone()).unwrap();
    assert_eq!(three_months, t0 + thr.extension_total_secs, "extended to the 3-month total");
    assert!(three_months > one_month);
    // the reason is recorded (Art. 12(3)).
    assert_eq!(timer3.wheel().extension_reason_for(&id3), Some(reason));
    // the OLD (1-month) warning point no longer fires after the extension.
    let mut at_old = DsrDeadlineTimer::new(TestClock::at(one_month_warning), thr);
    at_old.restore_wheel(DsrTimerWheel::restore(timer3.wheel().snapshot()));
    assert!(
        at_old.tick().is_empty(),
        "the extension re-armed later — the old warning point is disarmed (no double-fire)"
    );
}

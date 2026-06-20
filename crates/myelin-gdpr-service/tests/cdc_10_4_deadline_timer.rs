//! # CDC 10.4 (the deadline-timer leg) — the DSR deadline durable timer (P-GA-21 → P-148)
//!
//! **Contract:** index row 10.4 (the deadline-timer leg — *"track the deadline on a durable timer
//! (contract 9.3, the same `myelin-flow` timer wheel); a nearing-deadline emits a warning Signal"*,
//! gdpr §4.1 step 6). This is the consumer-driven contract test the coverage scanner (P-S21) reads
//! both halves of, for the deadline-timer leg that COMPLETES the P-GA-14 coarse-deadline floor:
//!
//! - **provider** = the durable deadline timer ([`DsrDeadlineTimer`]) backed by the minute-bucket
//!   wheel ([`DsrTimerWheel`]) — it arms a `sleep_until(deadline − warning_margin)` on `dsr_submit`,
//!   fires a [`DsrDeadlineWarning`] Signal at the nearing-deadline point, survives a restart (the
//!   wheel state is durable), and re-arms on the Art. 12(3) extension with a recorded reason.
//! - **consumer** = (a) the **DSR orchestrator** that arms the timer on submit + disarms it on
//!   completion (the certificate sealed in time); (b) a **warning-Signal consumer** (the dispatch
//!   tier / an ops surface) that routes the fired [`DsrDeadlineWarning`] — reading the opaque
//!   `dsr_id` + the tenant token + the `dsr_deadline_margin` (PII-free), never a subject.
//!
//! The dated green artifact: a DSR is armed on submit; a tick at the nearing-deadline point fires
//! exactly one PII-free warning carrying the margin; a restart between arm and fire still fires
//! (0 silent misses); an extension re-arms later with a recorded reason. If 10.4's deadline-timer
//! leg drifts (the warning fires after the deadline, a restart loses the timer, the extension fails
//! to record a reason), this stops compiling/passing — that is the contract.

use myelin_gdpr_service::{
    DsrDeadlineTimer, DsrDeadlineWarning, DsrId, DsrTimerWheel, DSR_DEADLINE_MARGIN,
};
use myelin_substrate::{DsrDeadline, TestClock};
use myelin_tenancy::TenantId;

fn tenant() -> TenantId {
    TenantId::from_token("acme")
}

fn dsr(n: u64) -> DsrId {
    DsrId(format!("dsr:{n}"))
}

/// The deadline-timer leg of 10.4: the orchestrator (consumer) arms the durable timer on submit;
/// the timer (provider) fires the warning Signal at the nearing-deadline point; the warning-Signal
/// consumer reads the PII-free margin. This is the provider+consumer seam the scanner reads.
#[test]
fn cdc_10_4_deadline_timer_arm_fire_warning_signal() {
    let t0 = 1_700_000_000;
    let thr = DsrDeadline::default();

    // ── provider: the durable timer. consumer (the orchestrator): arms it on dsr_submit. ──
    let mut timer = DsrDeadlineTimer::new(TestClock::at(t0), thr.clone());
    let deadline = timer.arm_deadline(dsr(0), tenant(), t0).unwrap();
    // the statutory deadline is the §4.1 `now + 1 month` (unchanged shape — the coarse field the
    // P-GA-14 floor tracked is now backed by a durable timer).
    assert_eq!(deadline, t0 + thr.deadline_secs);

    // a tick a month out fires nothing (the deadline is not near).
    assert!(timer.tick().is_empty());

    // ── provider fires the warning Signal at the nearing-deadline point. ──
    let warning_at = deadline - thr.warning_margin_secs;
    let fired: Vec<DsrDeadlineWarning> = timer.tick_at(warning_at);
    assert_eq!(fired.len(), 1, "the warning Signal fires at the nearing-deadline point");

    // ── consumer: the warning-Signal consumer reads the PII-free fields. ──
    let w = &fired[0];
    assert_eq!(w.dsr_id, dsr(0), "the opaque DSR id (the consumer resolves the subject behind it)");
    assert_eq!(w.tenant, tenant(), "the PII-free tenant token");
    assert_eq!(w.deadline_secs, deadline, "the deadline the warning is racing");
    // the `dsr_deadline_margin` (§1.8) — positive: the warning fires BEFORE the deadline.
    assert_eq!(w.margin_remaining_secs, thr.warning_margin_secs);
    assert!(w.margin_remaining_secs > 0, "0 silent misses: the warning fires before the deadline");

    // the signal NAME + UNIT are the frozen §1.8 anchor (the consumer keys telemetry off it).
    assert_eq!(DSR_DEADLINE_MARGIN, ("gdpr.dsr_deadline_margin", "secs"));
}

/// The restart-survival leg of the contract: the wheel state is the durable truth (the §9.3 rows),
/// so an orchestrator killed BETWEEN arm and fire restores the wheel and the timer STILL fires.
/// The provider (the wheel) snapshots/restores; the consumer (the restarted orchestrator) ticks.
#[test]
fn cdc_10_4_deadline_timer_survives_a_restart() {
    let t0 = 1_700_000_000;
    let thr = DsrDeadline::default();
    let mut timer = DsrDeadlineTimer::new(TestClock::at(t0), thr.clone());
    let deadline = timer.arm_deadline(dsr(0), tenant(), t0).unwrap();

    // CRASH: snapshot the durable rows, drop the orchestrator.
    let rows = timer.wheel().snapshot();
    drop(timer);

    // RESTART: a fresh orchestrator restores the wheel + ticks at the warning point.
    let warning_at = deadline - thr.warning_margin_secs;
    let mut restarted = DsrDeadlineTimer::new(TestClock::at(warning_at), thr);
    restarted.restore_wheel(DsrTimerWheel::restore(rows));
    let fired = restarted.tick();
    assert_eq!(fired.len(), 1, "the restored timer fires — the restart lost nothing");
    assert_eq!(fired[0].dsr_id, dsr(0));
}

/// The Art. 12(3) extension leg: the provider re-arms the timer to the 3-month point with a
/// recorded reason; the consumer (an auditor) reads the recorded reason off the entry.
#[test]
fn cdc_10_4_deadline_timer_extension_records_a_reason() {
    let t0 = 1_700_000_000;
    let thr = DsrDeadline::default();
    let mut timer = DsrDeadlineTimer::new(TestClock::at(t0), thr.clone());
    timer.arm_deadline(dsr(0), tenant(), t0).unwrap();

    let reason = "complex: multi-jurisdiction member iteration".to_string();
    let extended = timer.extend_deadline(&dsr(0), t0, reason.clone()).unwrap();
    assert_eq!(extended, t0 + thr.extension_total_secs, "extended to the 3-month total");
    // the recorded reason is on the entry (Art. 12(3) — extension reasons are recorded).
    assert_eq!(timer.wheel().extension_reason_for(&dsr(0)), Some(reason));
}

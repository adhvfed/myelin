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
    SubjectRef::new(Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        tenant(),
    ))
}

fn subject_scope(s: &str) -> EraseScope {
    EraseScope::Subject {
        subject: subject(s),
        tenant: tenant(),
    }
}

#[test]
fn ga_d4_open_dsr_warning_fires_before_deadline_certificate_seals_and_restart_still_fires() {
    let t0 = 1_700_000_000;
    let thr = DsrDeadline::default();

    let orch = DsrOrchestrator::new(TestClock::at(t0));
    let mut timer = DsrDeadlineTimer::new(TestClock::at(t0), thr.clone());

    let id = orch.dsr_submit(
        DsrKind::Erasure,
        tenant(),
        subject("p1"),
        subject_scope("p1"),
        Posture::Controller,
        Initiator::Myelin,
    );
    let coarse_deadline = orch.dsr_status(&id).unwrap().deadline_secs;
    let durable_deadline = timer.arm_deadline(id.clone(), tenant(), t0).unwrap();
    assert_eq!(
        durable_deadline, coarse_deadline,
        "the durable timer's deadline matches the orchestrator's coarse field (the field shape is \
         unchanged - the timer REPLACES the tracking, not the field)"
    );
    assert!(
        timer.wheel().is_armed(&id),
        "the durable timer is armed on submit"
    );

    assert!(
        timer.tick().is_empty(),
        "no spurious warning fires a month out"
    );
    assert!(timer.wheel().is_armed(&id), "the timer stays armed");

    let warning_at = durable_deadline - thr.warning_margin_secs;
    assert!(
        warning_at < durable_deadline,
        "the warning point is BEFORE the deadline"
    );
    let fired = timer.tick_at(warning_at);
    assert_eq!(
        fired.len(),
        1,
        "GA-D4: the durable timer fires the warning Signal"
    );
    let w = &fired[0];
    assert_eq!(
        w.dsr_id, id,
        "the warning carries the opaque DSR id (PII-free)"
    );
    assert_eq!(
        w.tenant,
        tenant(),
        "the warning carries the PII-free tenant token"
    );
    assert_eq!(w.deadline_secs, durable_deadline);
    assert_eq!(w.margin_remaining_secs, thr.warning_margin_secs);
    assert!(
        w.margin_remaining_secs > 0,
        "GA-D4: 0 silent misses - the warning fired in time"
    );
    assert_eq!(DSR_DEADLINE_MARGIN, ("gdpr.dsr_deadline_margin", "secs"));
    assert!(
        !timer.wheel().is_armed(&id),
        "the warning fired once and is disarmed"
    );

    orch.validate(&id).unwrap();
    orch.fan_out(&id, &Default::default()).unwrap();
    orch.verify(&id, vec!["receipt-1".into()]).unwrap();
    orch.complete(&id).unwrap();
    assert_eq!(orch.state_of(&id).unwrap(), DsrState::Completed);
    let cert = orch.dsr_certificate(&id).unwrap();
    assert_eq!(
        cert.dsr_id, id,
        "GA-D4: the certificate seals on completion"
    );
    assert!(cert.bundle_digest.starts_with("blake3:"));

    let id2 = orch.dsr_submit(
        DsrKind::Access,
        tenant(),
        subject("p2"),
        subject_scope("p2"),
        Posture::Controller,
        Initiator::Myelin,
    );
    let deadline2 = timer.arm_deadline(id2.clone(), tenant(), t0).unwrap();
    let durable_rows = timer.wheel().snapshot();
    assert!(
        durable_rows.iter().any(|r| r.dsr_id == id2),
        "the armed timer is DURABLE state (a row), not in-process state"
    );
    drop(timer);
    let warning_at2 = deadline2 - thr.warning_margin_secs;
    let mut restarted = DsrDeadlineTimer::new(TestClock::at(warning_at2), thr.clone());
    restarted.restore_wheel(DsrTimerWheel::restore(durable_rows));
    assert!(
        restarted.wheel().is_armed(&id2),
        "the timer survived the restart"
    );
    let fired2 = restarted.tick();
    assert_eq!(
        fired2.len(),
        1,
        "GA-D4 restart leg: the timer fires after a kill-and-restart"
    );
    assert_eq!(fired2[0].dsr_id, id2);

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
    assert_eq!(
        three_months,
        t0 + thr.extension_total_secs,
        "extended to the 3-month total"
    );
    assert!(three_months > one_month);
    assert_eq!(timer3.wheel().extension_reason_for(&id3), Some(reason));
    let mut at_old = DsrDeadlineTimer::new(TestClock::at(one_month_warning), thr);
    at_old.restore_wheel(DsrTimerWheel::restore(timer3.wheel().snapshot()));
    assert!(
        at_old.tick().is_empty(),
        "the extension re-armed later - the old warning point is disarmed (no double-fire)"
    );
}

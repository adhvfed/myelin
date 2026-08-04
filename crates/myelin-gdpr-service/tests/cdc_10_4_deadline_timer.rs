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

#[test]
fn cdc_10_4_deadline_timer_arm_fire_warning_signal() {
    let t0 = 1_700_000_000;
    let thr = DsrDeadline::default();

    let mut timer = DsrDeadlineTimer::new(TestClock::at(t0), thr.clone());
    let deadline = timer.arm_deadline(dsr(0), tenant(), t0).unwrap();
    assert_eq!(deadline, t0 + thr.deadline_secs);

    assert!(timer.tick().is_empty());

    let warning_at = deadline - thr.warning_margin_secs;
    let fired: Vec<DsrDeadlineWarning> = timer.tick_at(warning_at);
    assert_eq!(
        fired.len(),
        1,
        "the warning Signal fires at the nearing-deadline point"
    );

    let w = &fired[0];
    assert_eq!(
        w.dsr_id,
        dsr(0),
        "the opaque DSR id (the consumer resolves the subject behind it)"
    );
    assert_eq!(w.tenant, tenant(), "the PII-free tenant token");
    assert_eq!(
        w.deadline_secs, deadline,
        "the deadline the warning is racing"
    );
    assert_eq!(w.margin_remaining_secs, thr.warning_margin_secs);
    assert!(
        w.margin_remaining_secs > 0,
        "0 silent misses: the warning fires before the deadline"
    );

    assert_eq!(DSR_DEADLINE_MARGIN, ("gdpr.dsr_deadline_margin", "secs"));
}

#[test]
fn cdc_10_4_deadline_timer_survives_a_restart() {
    let t0 = 1_700_000_000;
    let thr = DsrDeadline::default();
    let mut timer = DsrDeadlineTimer::new(TestClock::at(t0), thr.clone());
    let deadline = timer.arm_deadline(dsr(0), tenant(), t0).unwrap();

    let rows = timer.wheel().snapshot();
    drop(timer);

    let warning_at = deadline - thr.warning_margin_secs;
    let mut restarted = DsrDeadlineTimer::new(TestClock::at(warning_at), thr);
    restarted.restore_wheel(DsrTimerWheel::restore(rows));
    let fired = restarted.tick();
    assert_eq!(
        fired.len(),
        1,
        "the restored timer fires - the restart lost nothing"
    );
    assert_eq!(fired[0].dsr_id, dsr(0));
}

#[test]
fn cdc_10_4_deadline_timer_extension_records_a_reason() {
    let t0 = 1_700_000_000;
    let thr = DsrDeadline::default();
    let mut timer = DsrDeadlineTimer::new(TestClock::at(t0), thr.clone());
    timer.arm_deadline(dsr(0), tenant(), t0).unwrap();

    let reason = "complex: multi-jurisdiction member iteration".to_string();
    let extended = timer.extend_deadline(&dsr(0), t0, reason.clone()).unwrap();
    assert_eq!(
        extended,
        t0 + thr.extension_total_secs,
        "extended to the 3-month total"
    );
    assert_eq!(timer.wheel().extension_reason_for(&dsr(0)), Some(reason));
}

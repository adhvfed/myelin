use std::collections::BTreeMap;

use myelin_substrate::{Clock, DsrDeadline};
use myelin_tenancy::TenantId;

use crate::dsr::DsrId;

pub const DSR_DEADLINE_MARGIN: (&str, &str) = ("gdpr.dsr_deadline_margin", "secs");

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DsrDeadlineWarning {
    pub dsr_id: DsrId,
    pub tenant: TenantId,
    pub deadline_secs: u64,
    pub margin_remaining_secs: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TimerEntry {
    dsr_id: DsrId,
    tenant: TenantId,
    fire_at_secs: u64,
    deadline_secs: u64,
    extension_reason: Option<String>,
}

impl TimerEntry {
    fn minute_bucket(&self) -> u64 {
        self.fire_at_secs / 60
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimerEntrySnapshot {
    pub dsr_id: DsrId,
    pub tenant: TenantId,
    pub fire_at_secs: u64,
    pub deadline_secs: u64,
    pub extension_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimerError {
    AlreadyArmed(DsrId),
    NotArmed(DsrId),
    ExtensionNotLater {
        current_secs: u64,
        requested_secs: u64,
    },
}

impl std::fmt::Display for TimerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TimerError::AlreadyArmed(id) => write!(
                f,
                "DSR `{}` already has an armed deadline timer (a re-arm is `rearm_extension`, not a \
                 second `arm`)",
                id.0
            ),
            TimerError::NotArmed(id) => {
                write!(f, "DSR `{}` has no armed deadline timer (a stale id / an already-fired timer)", id.0)
            }
            TimerError::ExtensionNotLater { current_secs, requested_secs } => write!(
                f,
                "extension deadline {requested_secs}s is not later than the current {current_secs}s \
                 (Art. 12(3) extends the deadline, never shortens it)"
            ),
        }
    }
}

impl std::error::Error for TimerError {}

#[derive(Default)]
pub struct DsrTimerWheel {
    armed: BTreeMap<DsrId, TimerEntry>,
}

impl DsrTimerWheel {
    pub fn new() -> DsrTimerWheel {
        DsrTimerWheel {
            armed: BTreeMap::new(),
        }
    }

    fn arm(
        &mut self,
        dsr_id: DsrId,
        tenant: TenantId,
        fire_at_secs: u64,
        deadline_secs: u64,
        extension_reason: Option<String>,
    ) -> Result<(), TimerError> {
        if self.armed.contains_key(&dsr_id) {
            return Err(TimerError::AlreadyArmed(dsr_id));
        }
        self.armed.insert(
            dsr_id.clone(),
            TimerEntry {
                dsr_id,
                tenant,
                fire_at_secs,
                deadline_secs,
                extension_reason,
            },
        );
        Ok(())
    }

    pub fn tick(&mut self, now_secs: u64) -> Vec<DsrDeadlineWarning> {
        let now_bucket = now_secs / 60;
        let due: Vec<DsrId> = self
            .armed
            .values()
            .filter(|e| e.minute_bucket() <= now_bucket)
            .map(|e| e.dsr_id.clone())
            .collect();
        let mut fired = Vec::with_capacity(due.len());
        for id in due {
            let e = self
                .armed
                .remove(&id)
                .expect("due entry was just observed present");
            let margin_remaining_secs = e.deadline_secs.saturating_sub(now_secs);
            fired.push(DsrDeadlineWarning {
                dsr_id: e.dsr_id,
                tenant: e.tenant,
                deadline_secs: e.deadline_secs,
                margin_remaining_secs,
            });
        }
        fired.sort();
        fired
    }

    pub fn rearm_extension(
        &mut self,
        dsr_id: &DsrId,
        new_fire_at_secs: u64,
        new_deadline_secs: u64,
        reason: String,
    ) -> Result<(), TimerError> {
        let current = self
            .armed
            .get(dsr_id)
            .ok_or_else(|| TimerError::NotArmed(dsr_id.clone()))?;
        if new_deadline_secs <= current.deadline_secs {
            return Err(TimerError::ExtensionNotLater {
                current_secs: current.deadline_secs,
                requested_secs: new_deadline_secs,
            });
        }
        let tenant = current.tenant.clone();
        self.armed.remove(dsr_id);
        self.arm(
            dsr_id.clone(),
            tenant,
            new_fire_at_secs,
            new_deadline_secs,
            Some(reason),
        )
        .expect("just disarmed - cannot be already-armed");
        Ok(())
    }

    pub fn disarm(&mut self, dsr_id: &DsrId) -> Result<(), TimerError> {
        self.armed
            .remove(dsr_id)
            .map(|_| ())
            .ok_or_else(|| TimerError::NotArmed(dsr_id.clone()))
    }

    pub fn armed_count(&self) -> usize {
        self.armed.len()
    }

    pub fn is_armed(&self, dsr_id: &DsrId) -> bool {
        self.armed.contains_key(dsr_id)
    }

    pub fn fire_at_for(&self, dsr_id: &DsrId) -> Option<u64> {
        self.armed.get(dsr_id).map(|e| e.fire_at_secs)
    }

    pub fn extension_reason_for(&self, dsr_id: &DsrId) -> Option<String> {
        self.armed
            .get(dsr_id)
            .and_then(|e| e.extension_reason.clone())
    }

    pub fn snapshot(&self) -> Vec<TimerEntrySnapshot> {
        self.armed
            .values()
            .map(|e| TimerEntrySnapshot {
                dsr_id: e.dsr_id.clone(),
                tenant: e.tenant.clone(),
                fire_at_secs: e.fire_at_secs,
                deadline_secs: e.deadline_secs,
                extension_reason: e.extension_reason.clone(),
            })
            .collect()
    }

    pub fn restore(rows: Vec<TimerEntrySnapshot>) -> DsrTimerWheel {
        let mut armed = BTreeMap::new();
        for r in rows {
            armed.insert(
                r.dsr_id.clone(),
                TimerEntry {
                    dsr_id: r.dsr_id,
                    tenant: r.tenant,
                    fire_at_secs: r.fire_at_secs,
                    deadline_secs: r.deadline_secs,
                    extension_reason: r.extension_reason,
                },
            );
        }
        DsrTimerWheel { armed }
    }
}

pub struct DsrDeadlineTimer<C: Clock> {
    clock: C,
    thresholds: DsrDeadline,
    wheel: DsrTimerWheel,
}

impl<C: Clock> DsrDeadlineTimer<C> {
    pub fn new(clock: C, thresholds: DsrDeadline) -> DsrDeadlineTimer<C> {
        DsrDeadlineTimer {
            clock,
            thresholds,
            wheel: DsrTimerWheel::new(),
        }
    }

    pub fn arm_deadline(
        &mut self,
        dsr_id: DsrId,
        tenant: TenantId,
        submitted_at_secs: u64,
    ) -> Result<u64, TimerError> {
        let deadline_secs = submitted_at_secs + self.thresholds.deadline_secs;
        let warning_at_secs = deadline_secs
            .saturating_sub(self.thresholds.warning_margin_secs)
            .max(submitted_at_secs);
        self.wheel
            .arm(dsr_id, tenant, warning_at_secs, deadline_secs, None)?;
        Ok(deadline_secs)
    }

    pub fn extend_deadline(
        &mut self,
        dsr_id: &DsrId,
        submitted_at_secs: u64,
        reason: String,
    ) -> Result<u64, TimerError> {
        let new_deadline_secs = submitted_at_secs + self.thresholds.extension_total_secs;
        let new_warning_at_secs = new_deadline_secs
            .saturating_sub(self.thresholds.warning_margin_secs)
            .max(submitted_at_secs);
        self.wheel
            .rearm_extension(dsr_id, new_warning_at_secs, new_deadline_secs, reason)?;
        Ok(new_deadline_secs)
    }

    pub fn disarm(&mut self, dsr_id: &DsrId) -> Result<(), TimerError> {
        self.wheel.disarm(dsr_id)
    }

    pub fn tick(&mut self) -> Vec<DsrDeadlineWarning> {
        self.wheel.tick(self.clock.now_secs())
    }

    pub fn tick_at(&mut self, now_secs: u64) -> Vec<DsrDeadlineWarning> {
        self.wheel.tick(now_secs)
    }

    pub fn wheel(&self) -> &DsrTimerWheel {
        &self.wheel
    }

    pub fn restore_wheel(&mut self, wheel: DsrTimerWheel) {
        self.wheel = wheel;
    }
}

#[cfg(test)]
impl DsrDeadlineTimer<myelin_substrate::TestClock> {
    fn advance_for_test(&self, secs: u64) {
        self.clock.advance(secs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_substrate::TestClock;

    fn tenant() -> TenantId {
        TenantId::from_token("acme")
    }

    fn dsr(n: u64) -> DsrId {
        DsrId(format!("dsr:{n}"))
    }

    fn thresholds() -> DsrDeadline {
        DsrDeadline::default()
    }

    #[test]
    fn arming_a_dsr_schedules_a_wheel_entry_at_now_plus_one_month_minus_margin() {
        let t0 = 1_700_000_000;
        let mut timer = DsrDeadlineTimer::new(TestClock::at(t0), thresholds());
        let deadline = timer.arm_deadline(dsr(0), tenant(), t0).unwrap();
        assert_eq!(deadline, t0 + 30 * 24 * 60 * 60);
        let expected_warning = deadline - 7 * 24 * 60 * 60;
        assert_eq!(timer.wheel().fire_at_for(&dsr(0)), Some(expected_warning));
        assert!(timer.wheel().is_armed(&dsr(0)));
        assert_eq!(timer.wheel().armed_count(), 1);
    }

    #[test]
    fn the_nearing_deadline_margin_fires_the_warning_signal_before_the_deadline() {
        let t0 = 1_700_000_000;
        let clock = TestClock::at(t0);
        let mut timer = DsrDeadlineTimer::new(clock, thresholds());
        let deadline = timer.arm_deadline(dsr(0), tenant(), t0).unwrap();

        let early = timer.tick();
        assert!(early.is_empty(), "no warning fires a month out");
        assert!(timer.wheel().is_armed(&dsr(0)), "the timer stays armed");

        let warning_at = deadline - 7 * 24 * 60 * 60;
        let to_advance = warning_at - t0;
        timer.advance_for_test(to_advance);
        let fired = timer.tick();
        assert_eq!(
            fired.len(),
            1,
            "the warning fires at the nearing-deadline point"
        );
        let w = &fired[0];
        assert_eq!(w.dsr_id, dsr(0));
        assert_eq!(w.tenant, tenant());
        assert_eq!(w.deadline_secs, deadline);
        assert!(
            w.margin_remaining_secs > 0,
            "the warning fires before the deadline"
        );
        assert_eq!(w.margin_remaining_secs, 7 * 24 * 60 * 60);
        assert!(
            !timer.wheel().is_armed(&dsr(0)),
            "the warning fired once and is disarmed"
        );
        assert_eq!(timer.wheel().armed_count(), 0);
    }

    #[test]
    fn a_restart_between_arm_and_fire_still_fires_the_warning() {
        let t0 = 1_700_000_000;
        let mut timer = DsrDeadlineTimer::new(TestClock::at(t0), thresholds());
        let deadline = timer.arm_deadline(dsr(0), tenant(), t0).unwrap();

        let durable_rows = timer.wheel().snapshot();
        assert_eq!(
            durable_rows.len(),
            1,
            "the armed timer is durable state, not in-process state"
        );
        drop(timer);

        let warning_at = deadline - 7 * 24 * 60 * 60;
        let mut restarted = DsrDeadlineTimer::new(TestClock::at(warning_at), thresholds());
        restarted.restore_wheel(DsrTimerWheel::restore(durable_rows));
        assert!(
            restarted.wheel().is_armed(&dsr(0)),
            "the timer survived the restart"
        );

        let fired = restarted.tick();
        assert_eq!(
            fired.len(),
            1,
            "the restored timer fires - the restart did not lose it"
        );
        assert_eq!(fired[0].dsr_id, dsr(0));
    }

    #[test]
    fn the_extension_to_three_months_rearms_with_a_recorded_reason() {
        let t0 = 1_700_000_000;
        let mut timer = DsrDeadlineTimer::new(TestClock::at(t0), thresholds());
        let one_month = timer.arm_deadline(dsr(0), tenant(), t0).unwrap();
        let one_month_warning = timer.wheel().fire_at_for(&dsr(0)).unwrap();

        let reason = "complex: cross-cell member iteration".to_string();
        let three_months = timer.extend_deadline(&dsr(0), t0, reason.clone()).unwrap();
        assert_eq!(three_months, t0 + 90 * 24 * 60 * 60);
        assert!(three_months > one_month);
        let three_month_warning = timer.wheel().fire_at_for(&dsr(0)).unwrap();
        assert!(
            three_month_warning > one_month_warning,
            "the warning re-armed later"
        );
        assert_eq!(three_month_warning, three_months - 7 * 24 * 60 * 60);
        assert_eq!(timer.wheel().extension_reason_for(&dsr(0)), Some(reason));
        assert_eq!(timer.wheel().armed_count(), 1);

        let mut at_old_warning =
            DsrDeadlineTimer::new(TestClock::at(one_month_warning), thresholds());
        at_old_warning.restore_wheel(DsrTimerWheel::restore(timer.wheel().snapshot()));
        assert!(
            at_old_warning.tick().is_empty(),
            "the old warning point is disarmed by the extension"
        );
    }

    #[test]
    fn an_extension_that_does_not_extend_is_a_loud_error() {
        let t0 = 1_700_000_000;
        let mut wheel = DsrTimerWheel::new();
        wheel
            .arm(dsr(0), tenant(), t0 + 100, t0 + 200, None)
            .unwrap();
        let err = wheel
            .rearm_extension(&dsr(0), t0 + 50, t0 + 150, "x".into())
            .unwrap_err();
        assert_eq!(
            err,
            TimerError::ExtensionNotLater {
                current_secs: t0 + 200,
                requested_secs: t0 + 150
            }
        );
        assert_eq!(wheel.fire_at_for(&dsr(0)), Some(t0 + 100));
    }

    #[test]
    fn arming_the_same_dsr_twice_is_a_loud_error() {
        let mut wheel = DsrTimerWheel::new();
        wheel.arm(dsr(0), tenant(), 100, 200, None).unwrap();
        let err = wheel.arm(dsr(0), tenant(), 300, 400, None).unwrap_err();
        assert_eq!(err, TimerError::AlreadyArmed(dsr(0)));
    }

    #[test]
    fn rearming_or_disarming_an_unarmed_dsr_is_a_loud_error() {
        let mut wheel = DsrTimerWheel::new();
        assert_eq!(
            wheel
                .rearm_extension(&dsr(9), 100, 200, "x".into())
                .unwrap_err(),
            TimerError::NotArmed(dsr(9))
        );
        assert_eq!(
            wheel.disarm(&dsr(9)).unwrap_err(),
            TimerError::NotArmed(dsr(9))
        );
    }

    #[test]
    fn the_minute_bucket_fire_boundary_is_inclusive() {
        let mut wheel = DsrTimerWheel::new();
        wheel.arm(dsr(0), tenant(), 600, 660, None).unwrap();
        assert!(
            wheel.tick(599).is_empty(),
            "minute bucket 9 < 10: not yet due"
        );
        assert!(wheel.is_armed(&dsr(0)), "still armed");
        let fired = wheel.tick(600);
        assert_eq!(
            fired.len(),
            1,
            "minute bucket 10 == 10: due (inclusive boundary)"
        );
        assert_eq!(fired[0].dsr_id, dsr(0));
    }

    #[test]
    fn a_tick_past_the_deadline_reports_zero_margin_not_an_underflow() {
        let mut wheel = DsrTimerWheel::new();
        wheel.arm(dsr(0), tenant(), 600, 660, None).unwrap();
        let fired = wheel.tick(1000);
        assert_eq!(fired.len(), 1);
        assert_eq!(
            fired[0].margin_remaining_secs, 0,
            "past-deadline margin is 0, never an underflow"
        );
    }

    #[test]
    fn disarm_on_completion_removes_the_armed_warning() {
        let t0 = 1_700_000_000;
        let mut timer = DsrDeadlineTimer::new(TestClock::at(t0), thresholds());
        timer.arm_deadline(dsr(0), tenant(), t0).unwrap();
        assert!(timer.wheel().is_armed(&dsr(0)));
        timer.disarm(&dsr(0)).unwrap();
        assert!(!timer.wheel().is_armed(&dsr(0)));
        timer.advance_for_test(40 * 24 * 60 * 60);
        assert!(timer.tick().is_empty(), "a completed DSR fires no warning");
    }

    #[test]
    fn multiple_due_warnings_fire_in_deterministic_id_order() {
        let mut wheel = DsrTimerWheel::new();
        wheel.arm(dsr(2), tenant(), 600, 660, None).unwrap();
        wheel.arm(dsr(0), tenant(), 600, 660, None).unwrap();
        wheel.arm(dsr(1), tenant(), 600, 660, None).unwrap();
        let fired = wheel.tick(600);
        let ids: Vec<&str> = fired.iter().map(|w| w.dsr_id.0.as_str()).collect();
        assert_eq!(
            ids,
            vec!["dsr:0", "dsr:1", "dsr:2"],
            "deterministic id order"
        );
    }
}

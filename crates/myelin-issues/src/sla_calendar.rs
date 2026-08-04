use std::collections::BTreeMap;

use myelin_query::{DurableTimer, InMemoryTimer, Predicate, StaleAfter};
use myelin_tenancy::{Region, TenantId};
use serde::{Deserialize, Serialize};

use crate::sla_escalation::issue_sla_escalation_policy;

pub struct SlaCalendarFloors;

impl SlaCalendarFloors {
    pub const HISTORY_COMPACTION: &'static str =
        "R-11 (M5+): time_to_resolution history-compaction via myelin-flow continue-as-new";
    pub const TZ_DATABASE: &'static str =
        "prod: load IANA-zone offset transitions from the system tz database (chrono-tz/VTIMEZONE)";
    pub const LIVE_WHEEL: &'static str =
        "app boot: arm on myelin_flow::timer (9.3) + start the chain via Notif EscalationEngine::page";
}

pub const DEFAULT_AT_RISK_BPS: u32 = 8_000;

const BPS_DENOM: i64 = 10_000;

const SECS_PER_DAY: i64 = 86_400;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Weekday {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl Weekday {
    fn of_local_day(local_day: i64) -> Weekday {
        let idx = (local_day + 3).rem_euclid(7);
        match idx {
            0 => Weekday::Mon,
            1 => Weekday::Tue,
            2 => Weekday::Wed,
            3 => Weekday::Thu,
            4 => Weekday::Fri,
            5 => Weekday::Sat,
            _ => Weekday::Sun,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OffsetTransition {
    pub at_utc: i64,
    pub offset_secs: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Calendar {
    pub calendar_id: String,
    pub working_weekdays: Vec<Weekday>,
    pub work_start_min: i64,
    pub work_end_min: i64,
    pub holidays: Vec<i64>,
    pub offset_transitions: Vec<OffsetTransition>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkingWindow {
    pub start: i64,
    pub end: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarError(pub String);

impl Calendar {
    pub fn new(
        calendar_id: impl Into<String>,
        working_weekdays: Vec<Weekday>,
        work_start_min: i64,
        work_end_min: i64,
        holidays: Vec<i64>,
        mut offset_transitions: Vec<OffsetTransition>,
    ) -> Result<Calendar, CalendarError> {
        if work_end_min <= work_start_min {
            return Err(CalendarError(format!(
                "degenerate working window: end {work_end_min} <= start {work_start_min} (no available business seconds)"
            )));
        }
        if working_weekdays.is_empty() {
            return Err(CalendarError(
                "no working weekdays: the budget loop would never terminate".into(),
            ));
        }
        if offset_transitions.is_empty() {
            return Err(CalendarError(
                "no offset transitions: cannot resolve a local offset".into(),
            ));
        }
        offset_transitions.sort_by_key(|t| t.at_utc);
        Ok(Calendar {
            calendar_id: calendar_id.into(),
            working_weekdays,
            work_start_min,
            work_end_min,
            holidays,
            offset_transitions,
        })
    }

    pub fn business_hours_fixed(calendar_id: impl Into<String>, offset_secs: i64) -> Calendar {
        Calendar::new(
            calendar_id,
            vec![
                Weekday::Mon,
                Weekday::Tue,
                Weekday::Wed,
                Weekday::Thu,
                Weekday::Fri,
            ],
            9 * 60,
            17 * 60,
            Vec::new(),
            vec![OffsetTransition {
                at_utc: i64::MIN / 2,
                offset_secs,
            }],
        )
        .expect("the fixed business-hours calendar is well-formed")
    }

    pub fn offset_at(&self, utc: i64) -> i64 {
        let mut offset = self.offset_transitions[0].offset_secs;
        for t in &self.offset_transitions {
            if t.at_utc <= utc {
                offset = t.offset_secs;
            } else {
                break;
            }
        }
        offset
    }

    fn is_working_day(&self, local_day: i64) -> bool {
        let wd = Weekday::of_local_day(local_day);
        self.working_weekdays.contains(&wd) && !self.holidays.contains(&local_day)
    }

    pub fn next_working_window(&self, from: i64) -> Result<WorkingWindow, CalendarError> {
        let local_from = from + self.offset_at(from);
        let mut local_day = local_from.div_euclid(SECS_PER_DAY);
        let scan_limit = local_day + 366;
        while local_day <= scan_limit {
            if self.is_working_day(local_day) {
                let local_day_start = local_day * SECS_PER_DAY;
                let local_noon = local_day_start + 12 * 3600;
                let approx_utc_noon = local_noon - self.offset_transitions[0].offset_secs;
                let offset = self.offset_at(approx_utc_noon);
                let local_win_start = local_day_start + self.work_start_min * 60;
                let local_win_end = local_day_start + self.work_end_min * 60;
                let utc_start = local_win_start - offset;
                let utc_end = local_win_end - offset;
                if utc_end > from {
                    return Ok(WorkingWindow {
                        start: utc_start,
                        end: utc_end,
                    });
                }
            }
            local_day += 1;
        }
        Err(CalendarError(format!(
            "no working day within 366 days of {from} - calendar {} is misconfigured (no working day)",
            self.calendar_id
        )))
    }
}

pub fn business_fire_at(
    start: i64,
    budget_secs: i64,
    cal: &Calendar,
) -> Result<i64, CalendarError> {
    let mut cursor = start;
    let mut remaining = budget_secs.max(0);
    for _ in 0..100_000 {
        let win = cal.next_working_window(cursor)?;
        let effective_start = cursor.max(win.start);
        let avail = win.end - effective_start;
        if remaining <= avail {
            return Ok(effective_start + remaining);
        }
        remaining -= avail;
        cursor = win.end;
    }
    Err(CalendarError(format!(
        "budget {budget_secs}s exceeds 100k working windows for calendar {} - misconfigured SLA",
        cal.calendar_id
    )))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlaState {
    Running,
    Paused,
    Breached,
    Met,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlaRun {
    pub issue_key: String,
    pub policy_id: String,
    pub calendar: Calendar,
    pub target_business_secs: i64,
    pub at_risk_bps: u32,
    pub fire_at: i64,
    pub at_risk_fire_at: i64,
    pub remaining_business_secs: i64,
    pub running_since: i64,
    pub state: SlaState,
}

impl SlaRun {
    pub fn breach_timer_id(&self) -> String {
        format!("sla/{}", self.issue_key)
    }

    pub fn at_risk_timer_id(&self) -> String {
        format!("sla-at-risk/{}", self.issue_key)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlaOutcomeEvent {
    Started {
        issue_key: String,
        target_seconds: i64,
        fire_at: i64,
    },
    Paused {
        issue_key: String,
        remaining_seconds: i64,
    },
    Resumed {
        issue_key: String,
        fire_at: i64,
    },
    AtRisk {
        issue_key: String,
    },
    Breached {
        issue_key: String,
        escalation_policy_id: String,
    },
    Met {
        issue_key: String,
    },
}

impl SlaOutcomeEvent {
    pub fn event_type(&self) -> &'static str {
        match self {
            SlaOutcomeEvent::Started { .. } => crate::events::SLA_STARTED,
            SlaOutcomeEvent::Paused { .. } => crate::events::SLA_PAUSED,
            SlaOutcomeEvent::Resumed { .. } => crate::events::SLA_RESUMED,
            SlaOutcomeEvent::AtRisk { .. } => crate::events::SLA_AT_RISK,
            SlaOutcomeEvent::Breached { .. } => crate::events::SLA_BREACHED,
            SlaOutcomeEvent::Met { .. } => crate::events::SLA_MET,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlaEngineSnapshot {
    pub runs: Vec<SlaRun>,
}

pub struct SlaEngine {
    tenant: TenantId,
    region: Region,
    timer: InMemoryTimer,
    runs: BTreeMap<String, SlaRun>,
    armings: BTreeMap<String, myelin_query::ArmingId>,
    emitted: Vec<SlaOutcomeEvent>,
}

impl SlaEngine {
    pub fn new(tenant: TenantId, region: Region) -> SlaEngine {
        SlaEngine {
            tenant,
            region,
            timer: InMemoryTimer::new(),
            runs: BTreeMap::new(),
            armings: BTreeMap::new(),
            emitted: Vec::new(),
        }
    }

    pub fn partition(&self) -> (&TenantId, &Region) {
        (&self.tenant, &self.region)
    }

    pub fn emitted(&self) -> &[SlaOutcomeEvent] {
        &self.emitted
    }

    pub fn run(&self, issue_key: &str) -> Option<&SlaRun> {
        self.runs.get(issue_key)
    }

    pub fn armed_timer_count(&self) -> usize {
        self.timer.armed_count()
    }

    fn arming_for(timer_id: &str) -> myelin_query::ArmingId {
        myelin_query::ArmingId(format!("sla:{timer_id}"))
    }

    pub fn arm(
        &mut self,
        issue_key: impl Into<String>,
        policy_id: impl Into<String>,
        calendar: Calendar,
        target_business_secs: i64,
        now: i64,
    ) -> Result<SlaRun, CalendarError> {
        self.arm_with_bps(
            issue_key,
            policy_id,
            calendar,
            target_business_secs,
            DEFAULT_AT_RISK_BPS,
            now,
        )
    }

    pub fn arm_with_bps(
        &mut self,
        issue_key: impl Into<String>,
        policy_id: impl Into<String>,
        calendar: Calendar,
        target_business_secs: i64,
        at_risk_bps: u32,
        now: i64,
    ) -> Result<SlaRun, CalendarError> {
        let issue_key = issue_key.into();
        let fire_at = business_fire_at(now, target_business_secs, &calendar)?;
        let at_risk_budget = at_risk_budget(target_business_secs, at_risk_bps);
        let at_risk_fire_at = business_fire_at(now, at_risk_budget, &calendar)?;
        let run = SlaRun {
            issue_key: issue_key.clone(),
            policy_id: policy_id.into(),
            calendar,
            target_business_secs,
            at_risk_bps,
            fire_at,
            at_risk_fire_at,
            remaining_business_secs: target_business_secs,
            running_since: now,
            state: SlaState::Running,
        };
        self.arm_timers(&run)?;
        self.emit(SlaOutcomeEvent::Started {
            issue_key: issue_key.clone(),
            target_seconds: target_business_secs,
            fire_at,
        });
        self.runs.insert(issue_key, run.clone());
        Ok(run)
    }

    fn arm_timers(&mut self, run: &SlaRun) -> Result<(), CalendarError> {
        let breach_id = run.breach_timer_id();
        let at_risk_id = run.at_risk_timer_id();
        let breach_arming = Self::arming_for(&breach_id);
        let at_risk_arming = Self::arming_for(&at_risk_id);
        self.timer
            .arm(&breach_arming, &fire_at_to_stale_after(run.fire_at))
            .map_err(|e| CalendarError(format!("arm breach timer: {}", e.0)))?;
        self.timer
            .arm(
                &at_risk_arming,
                &fire_at_to_stale_after(run.at_risk_fire_at),
            )
            .map_err(|e| CalendarError(format!("arm at-risk timer: {}", e.0)))?;
        self.armings.insert(breach_id, breach_arming);
        self.armings.insert(at_risk_id, at_risk_arming);
        Ok(())
    }

    fn disarm_timers(&mut self, run: &SlaRun) {
        let breach_id = run.breach_timer_id();
        let at_risk_id = run.at_risk_timer_id();
        let _ = self.timer.disarm(&Self::arming_for(&breach_id));
        let _ = self.timer.disarm(&Self::arming_for(&at_risk_id));
        self.armings.remove(&breach_id);
        self.armings.remove(&at_risk_id);
    }

    pub fn pause(&mut self, issue_key: &str, now: i64) -> Result<(), CalendarError> {
        let Some(run) = self.runs.get(issue_key).cloned() else {
            return Ok(());
        };
        if run.state != SlaState::Running {
            return Ok(());
        }
        let consumed = business_seconds_between(run.running_since, now, &run.calendar)?;
        let remaining = (run.remaining_business_secs - consumed).max(0);
        let mut updated = run.clone();
        updated.remaining_business_secs = remaining;
        updated.state = SlaState::Paused;
        self.disarm_timers(&run);
        self.emit(SlaOutcomeEvent::Paused {
            issue_key: issue_key.to_string(),
            remaining_seconds: remaining,
        });
        self.runs.insert(issue_key.to_string(), updated);
        Ok(())
    }

    pub fn resume(&mut self, issue_key: &str, now: i64) -> Result<(), CalendarError> {
        let Some(run) = self.runs.get(issue_key).cloned() else {
            return Ok(());
        };
        if run.state != SlaState::Paused {
            return Ok(());
        }
        let fire_at = business_fire_at(now, run.remaining_business_secs, &run.calendar)?;
        let at_risk_budget = at_risk_budget(run.remaining_business_secs, run.at_risk_bps);
        let at_risk_fire_at = business_fire_at(now, at_risk_budget, &run.calendar)?;
        let mut updated = run.clone();
        updated.fire_at = fire_at;
        updated.at_risk_fire_at = at_risk_fire_at;
        updated.running_since = now;
        updated.state = SlaState::Running;
        self.arm_timers(&updated)?;
        self.emit(SlaOutcomeEvent::Resumed {
            issue_key: issue_key.to_string(),
            fire_at,
        });
        self.runs.insert(issue_key.to_string(), updated);
        Ok(())
    }

    pub fn on_at_risk_timer(&mut self, issue_key: &str) -> bool {
        let Some(run) = self.runs.get(issue_key) else {
            return false;
        };
        if run.state != SlaState::Running {
            return false;
        }
        self.emit(SlaOutcomeEvent::AtRisk {
            issue_key: issue_key.to_string(),
        });
        true
    }

    pub fn on_breach_timer(
        &mut self,
        issue_key: &str,
        ack_window_minutes: u32,
        repeat: u32,
    ) -> bool {
        let Some(run) = self.runs.get_mut(issue_key) else {
            return false;
        };
        if run.state != SlaState::Running {
            return false;
        }
        run.state = SlaState::Breached;
        let policy = issue_sla_escalation_policy(ack_window_minutes, repeat);
        self.emit(SlaOutcomeEvent::Breached {
            issue_key: issue_key.to_string(),
            escalation_policy_id: policy.policy_id,
        });
        true
    }

    pub fn meet(&mut self, issue_key: &str) -> bool {
        let Some(run) = self.runs.get(issue_key).cloned() else {
            return false;
        };
        if matches!(run.state, SlaState::Breached | SlaState::Met) {
            return false;
        }
        self.disarm_timers(&run);
        let mut updated = run;
        updated.state = SlaState::Met;
        self.emit(SlaOutcomeEvent::Met {
            issue_key: issue_key.to_string(),
        });
        self.runs.insert(issue_key.to_string(), updated);
        true
    }

    pub fn snapshot(&self) -> SlaEngineSnapshot {
        SlaEngineSnapshot {
            runs: self.runs.values().cloned().collect(),
        }
    }

    pub fn restore(
        tenant: TenantId,
        region: Region,
        snapshot: SlaEngineSnapshot,
    ) -> Result<SlaEngine, CalendarError> {
        let mut engine = SlaEngine::new(tenant, region);
        for run in snapshot.runs {
            if run.state == SlaState::Running {
                engine.arm_timers(&run)?;
            }
            engine.runs.insert(run.issue_key.clone(), run);
        }
        Ok(engine)
    }

    fn emit(&mut self, e: SlaOutcomeEvent) {
        self.emitted.push(e);
    }
}

fn at_risk_budget(target_business_secs: i64, at_risk_bps: u32) -> i64 {
    target_business_secs * at_risk_bps as i64 / BPS_DENOM
}

pub fn business_seconds_between(from: i64, to: i64, cal: &Calendar) -> Result<i64, CalendarError> {
    if to <= from {
        return Ok(0);
    }
    let mut cursor = from;
    let mut consumed = 0i64;
    for _ in 0..100_000 {
        let win = cal.next_working_window(cursor)?;
        if win.start >= to {
            break;
        }
        let overlap_start = cursor.max(win.start);
        let overlap_end = win.end.min(to);
        if overlap_end > overlap_start {
            consumed += overlap_end - overlap_start;
        }
        if win.end >= to {
            break;
        }
        cursor = win.end;
    }
    Ok(consumed)
}

fn fire_at_to_stale_after(fire_at: i64) -> StaleAfter {
    StaleAfter(epoch_secs_to_rfc3339(fire_at))
}

fn epoch_secs_to_rfc3339(epoch_secs: i64) -> String {
    let days = epoch_secs.div_euclid(86_400);
    let secs_of_day = epoch_secs.rem_euclid(86_400);
    let (h, m, s) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

pub type PauseConditions = Predicate;

#[cfg(test)]
#[path = "sla_calendar/tests.rs"]
mod tests;

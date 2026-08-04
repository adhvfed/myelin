use crate::prefs::{Channel, QuietHours};
use crate::{Class, Reason};
use myelin_events::{
    AggregateKey, DataRole, EmitContextBase, EventDraft, EventType, IdMinter, MonotonicMinter,
    OutboxStore, OutboxTx, Visibility,
};
use myelin_identity::PrincipalId;
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::router::NOTIF_ESCALATION_ACKED;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EscalationTarget {
    Schedule(String),
    Principal(PrincipalId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscalationStep {
    pub target: EscalationTarget,
    pub channels: Vec<Channel>,
    pub ack_window_minutes: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscalationPolicy {
    pub policy_id: String,
    pub steps: Vec<EscalationStep>,
    pub repeat: u32,
}

impl EscalationPolicy {
    pub fn test_chain(ack_window_minutes: u32, secondary: PrincipalId) -> EscalationPolicy {
        EscalationPolicy {
            policy_id: "esc-test-chain".into(),
            steps: vec![
                EscalationStep {
                    target: EscalationTarget::Schedule("platform-oncall".into()),
                    channels: vec![Channel::InApp, Channel::WebPush],
                    ack_window_minutes,
                },
                EscalationStep {
                    target: EscalationTarget::Principal(secondary),
                    channels: vec![Channel::InApp, Channel::WebPush],
                    ack_window_minutes,
                },
            ],
            repeat: 1,
        }
    }

    pub fn step_at(&self, walk: u32) -> Option<&EscalationStep> {
        if self.steps.is_empty() {
            return None;
        }
        let total = (self.steps.len() as u32).saturating_mul(self.repeat.max(1));
        if walk >= total {
            return None;
        }
        let idx = (walk as usize) % self.steps.len();
        self.steps.get(idx)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RotationWindow {
    pub from_minute: i32,
    pub to_minute: i32,
    pub principal: PrincipalId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OncallSchedule {
    pub schedule_id: String,
    pub rotation: Vec<RotationWindow>,
}

pub fn oncall_now(schedule: &OncallSchedule, minute_of_day: i32) -> Option<PrincipalId> {
    schedule
        .rotation
        .iter()
        .find(|w| minute_of_day >= w.from_minute && minute_of_day < w.to_minute)
        .map(|w| w.principal.clone())
}

pub fn notify_for(
    step_channels: &[Channel],
    class: Class,
    quiet: &QuietHours,
    recipient_in_quiet: bool,
) -> Vec<Channel> {
    if class == Class::Critical || quiet.pierces(class) || !recipient_in_quiet {
        step_channels.to_vec()
    } else {
        step_channels
            .iter()
            .copied()
            .filter(|c| *c == Channel::InApp)
            .collect()
    }
}

pub trait DurableWheel {
    fn schedule_timer(&self, run_id: &str, ack_window_minutes: u32);
    fn cancel_timer(&self, run_id: &str);
    fn has_timer(&self, run_id: &str) -> bool;
    fn fire_due(&self, run_id: &str) -> bool;
}

#[derive(Clone, Default)]
pub struct InMemoryWheel {
    inner: Arc<Mutex<WheelInner>>,
}

#[derive(Default)]
struct WheelInner {
    timers: BTreeMap<String, TimerHandle>,
}

#[derive(Clone)]
struct TimerHandle {
    fired: bool,
}

impl InMemoryWheel {
    pub fn new() -> InMemoryWheel {
        InMemoryWheel::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, WheelInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl DurableWheel for InMemoryWheel {
    fn schedule_timer(&self, run_id: &str, _ack_window_minutes: u32) {
        self.lock()
            .timers
            .insert(run_id.to_string(), TimerHandle { fired: false });
    }

    fn cancel_timer(&self, run_id: &str) {
        self.lock().timers.remove(run_id);
    }

    fn has_timer(&self, run_id: &str) -> bool {
        self.lock().timers.get(run_id).is_some_and(|t| !t.fired)
    }

    fn fire_due(&self, run_id: &str) -> bool {
        let mut inner = self.lock();
        match inner.timers.get_mut(run_id) {
            Some(h) if !h.fired => {
                h.fired = true;
                true
            }
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    Active,
    Acked,
    Exhausted,
}

#[derive(Clone, Debug)]
pub struct EscalationRun {
    pub tenant: TenantId,
    pub region: Region,
    pub run_id: String,
    pub policy: EscalationPolicy,
    pub trigger_event: ArtifactRef,
    pub walk: u32,
    pub state: RunState,
    pub acked_by: Option<PrincipalId>,
    pub pages: Vec<(u32, PrincipalId)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageOutcome {
    pub principal: PrincipalId,
    pub channels: Vec<Channel>,
    pub walk: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EscalationError {
    EmptyPolicy,
    NoOneOnCall(String),
    AckEmitFailed(String),
    UnknownRun(String),
}

pub struct EscalationEngine<W: DurableWheel> {
    wheel: W,
    outbox: OutboxStore,
    minter: Arc<dyn IdMinter>,
    runs: Arc<Mutex<BTreeMap<String, EscalationRun>>>,
}

impl<W: DurableWheel> EscalationEngine<W> {
    pub fn new(wheel: W, outbox: OutboxStore) -> EscalationEngine<W> {
        EscalationEngine {
            wheel,
            outbox,
            minter: Arc::new(MonotonicMinter::new()),
            runs: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, EscalationRun>> {
        self.runs.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn wheel(&self) -> &W {
        &self.wheel
    }

    pub fn run(&self, run_id: &str) -> Option<EscalationRun> {
        self.lock().get(run_id).cloned()
    }

    pub fn resume_for_test(&self, run: EscalationRun) {
        self.lock().insert(run.run_id.clone(), run);
    }

    fn resolve_target(
        &self,
        target: &EscalationTarget,
        schedule: Option<&OncallSchedule>,
        minute_of_day: i32,
    ) -> Result<PrincipalId, EscalationError> {
        match target {
            EscalationTarget::Principal(p) => Ok(p.clone()),
            EscalationTarget::Schedule(sched_id) => {
                let sched = schedule
                    .filter(|s| &s.schedule_id == sched_id)
                    .ok_or_else(|| EscalationError::NoOneOnCall(sched_id.clone()))?;
                oncall_now(sched, minute_of_day)
                    .ok_or_else(|| EscalationError::NoOneOnCall(sched_id.clone()))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn page(
        &self,
        tenant: TenantId,
        region: Region,
        run_id: String,
        policy: EscalationPolicy,
        trigger_event: ArtifactRef,
        schedule: Option<&OncallSchedule>,
        minute_of_day: i32,
        recipient_quiet: &QuietHours,
        recipient_in_quiet: bool,
    ) -> Result<(String, PageOutcome), EscalationError> {
        let first = policy
            .step_at(0)
            .ok_or(EscalationError::EmptyPolicy)?
            .clone();
        let principal = self.resolve_target(&first.target, schedule, minute_of_day)?;
        let channels = notify_for(
            &first.channels,
            Class::Critical,
            recipient_quiet,
            recipient_in_quiet,
        );
        let outcome = PageOutcome {
            principal: principal.clone(),
            channels,
            walk: 0,
        };

        let run = EscalationRun {
            tenant,
            region,
            run_id: run_id.clone(),
            policy,
            trigger_event,
            walk: 0,
            state: RunState::Active,
            acked_by: None,
            pages: vec![(0, principal)],
        };
        self.lock().insert(run_id.clone(), run);
        self.wheel.schedule_timer(&run_id, first.ack_window_minutes);
        Ok((run_id, outcome))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn advance(
        &self,
        run_id: &str,
        schedule: Option<&OncallSchedule>,
        minute_of_day: i32,
        recipient_quiet: &QuietHours,
        recipient_in_quiet: bool,
    ) -> Result<Option<PageOutcome>, EscalationError> {
        if !self.wheel.fire_due(run_id) {
            return Ok(None);
        }
        let mut runs = self.lock();
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| EscalationError::UnknownRun(run_id.into()))?;
        if run.state != RunState::Active {
            return Ok(None);
        }
        let next_walk = run.walk + 1;
        let Some(step) = run.policy.step_at(next_walk).cloned() else {
            run.state = RunState::Exhausted;
            return Ok(None);
        };
        let principal = self.resolve_target(&step.target, schedule, minute_of_day)?;
        let channels = notify_for(
            &step.channels,
            Class::Critical,
            recipient_quiet,
            recipient_in_quiet,
        );
        run.walk = next_walk;
        run.pages.push((next_walk, principal.clone()));
        let outcome = PageOutcome {
            principal,
            channels,
            walk: next_walk,
        };
        drop(runs);
        self.wheel.schedule_timer(run_id, step.ack_window_minutes);
        Ok(Some(outcome))
    }

    pub fn ack(
        &self,
        run_id: &str,
        acked_by: PrincipalId,
        occurred_at: myelin_events::Timestamp,
    ) -> Result<bool, EscalationError> {
        let (tenant, region, trigger_event, already_acked) = {
            let runs = self.lock();
            let run = runs
                .get(run_id)
                .ok_or_else(|| EscalationError::UnknownRun(run_id.into()))?;
            (
                run.tenant.clone(),
                run.region.clone(),
                run.trigger_event.clone(),
                run.state == RunState::Acked,
            )
        };
        if already_acked {
            return Ok(false);
        }
        let actor = myelin_events::Actor(myelin_identity::Principal::new(
            tenant.clone(),
            region.clone(),
            acked_by.clone(),
            myelin_identity::PrincipalKind::Human,
            myelin_identity::DataRole::Controller,
            myelin_identity::PrincipalStatus::Active,
        ));
        let base = EmitContextBase {
            tenant: tenant.clone(),
            region: region.clone(),
            actor,
            schema_ver: 1,
            occurred_at: occurred_at.clone(),
            recorded_at: occurred_at,
            caused_by: None,
        };
        let mut tx = self.outbox.begin(self.minter.clone(), base);
        tx.stage_state_change(format!(
            "UPDATE notif_escalation_run SET state='acked' WHERE run_id={run_id}"
        ));
        let draft = EventDraft {
            type_: EventType(NOTIF_ESCALATION_ACKED.into()),
            subject: trigger_event,
            aggregate: AggregateKey(format!("notif-escalation:{run_id}")),
            payload: serde_json::json!({
                "run_id": run_id,
                "acked_by": acked_by.0,
            }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        };
        tx.emit(draft, None)
            .map_err(|e| EscalationError::AckEmitFailed(format!("{e:?}")))?;
        tx.commit()
            .map_err(|e| EscalationError::AckEmitFailed(format!("{e:?}")))?;

        self.wheel.cancel_timer(run_id);
        let mut runs = self.lock();
        if let Some(run) = runs.get_mut(run_id) {
            run.state = RunState::Acked;
            run.acked_by = Some(acked_by);
        }
        Ok(true)
    }
}

pub const ESCALATION_REASON: Reason = Reason::Escalated;

pub fn render_oncall(schedule: &OncallSchedule, minute_of_day: i32) -> String {
    let mut out = format!("on-call schedule {}\n", schedule.schedule_id);
    match oncall_now(schedule, minute_of_day) {
        Some(p) => out.push_str(&format!("  now on call: {}\n", p.0)),
        None => out.push_str("  now on call: (none - uncovered window)\n"),
    }
    for w in &schedule.rotation {
        out.push_str(&format!(
            "  [{:02}:{:02}–{:02}:{:02}) → {}\n",
            w.from_minute / 60,
            w.from_minute % 60,
            w.to_minute / 60,
            w.to_minute % 60,
            w.principal.0
        ));
    }
    out
}

pub fn render_page(outcome: &PageOutcome) -> String {
    let chans: Vec<&str> = outcome.channels.iter().map(|c| c.token()).collect();
    format!(
        "paged {} on [{}] (escalation step {}, class=critical pierces quiet-hours)",
        outcome.principal.0,
        chans.join(", "),
        outcome.walk
    )
}

#[cfg(test)]
mod tests;

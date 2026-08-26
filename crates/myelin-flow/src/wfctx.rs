use crate::schema::{WfActivityAttemptRow, WfHistoryRow};
use myelin_events::{
    EmitContextBase, EventDraft, EventEnvelope, EventId, IdMinter, OutboxStore, OutboxTransaction,
    OutboxTx, Result as EmitResult,
};
use myelin_tenancy::{Region, TenantId};
use std::sync::{Arc, Mutex};

pub mod history_kind {
    pub const ACTIVITY_SCHEDULED: &str = "activity_scheduled";
    pub const ACTIVITY_COMPLETED: &str = "activity_completed";
    pub const ACTIVITY_FAILED: &str = "activity_failed";
    pub const SIDE_MARKER: &str = "side_marker";
    pub const SIGNAL_WAITED: &str = "signal_waited";
    pub const SIGNAL_RECEIVED: &str = "signal_received";
}

pub mod attempt_state {
    pub const SUCCEEDED: &str = "succeeded";
    pub const RETRYING: &str = "retrying";
    pub const FAILED: &str = "failed";
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActivityError {
    Retryable(String),
    Permanent(String),
}

impl ActivityError {
    pub fn retryable(detail: impl Into<String>) -> Self {
        Self::Retryable(detail.into())
    }

    pub fn permanent(detail: impl Into<String>) -> Self {
        Self::Permanent(detail.into())
    }

    pub fn detail(&self) -> &str {
        match self {
            Self::Retryable(detail) | Self::Permanent(detail) => detail,
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Retryable(_))
    }
}

impl core::fmt::Display for ActivityError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.detail())
    }
}

impl std::error::Error for ActivityError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WfError {
    ActivityExhausted(ActivityError),
    CoCommit(String),
    Nondeterministic(String),
}

impl WfError {
    pub fn is_nondeterministic(&self) -> bool {
        matches!(self, WfError::Nondeterministic(_))
    }
}

pub type WfResult<T> = core::result::Result<T, WfError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaitOutcome {
    Signalled {
        idem_key: String,
        payload: Vec<myelin_refs::ArtifactRef>,
        payload_key_ref: Option<String>,
    },
    Parked,
    TimedOut,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParkCondition {
    Signal {
        name: String,
        idem_key: Option<String>,
    },
    Timer {
        timer_id: String,
    },
}

pub(crate) const WAIT_IDEM_PREFIX: &str = "myelin://flow/signal-idem/";
const LEGACY_WAIT_IDEM_PREFIX: &str = "wait:idem:";
pub(crate) const WAIT_KEYREF_PREFIX: &str = "myelin://flow/signal-key-ref/";
const LEGACY_WAIT_KEYREF_PREFIX: &str = "wait:keyref:";
const WAIT_DEADLINE_PREFIX: &str = "wait:deadline:";
pub(crate) const WAIT_EXPECTED_IDEM_PREFIX: &str = "myelin://flow/wait-idem/";
pub(crate) const WAIT_EXPECTED_NAME_PREFIX: &str = "myelin://flow/wait-name/";
pub(crate) const WAIT_SIGNAL_NAME_PREFIX: &str = "myelin://flow/signal-name/";
const WAIT_TIMEOUT_MARKER: &str = "wait:timeout";

fn decode_received(result: &Option<Vec<myelin_refs::ArtifactRef>>) -> WaitOutcome {
    let refs = match result {
        Some(r) => r,
        None => {
            return WaitOutcome::Signalled {
                idem_key: String::new(),
                payload: vec![],
                payload_key_ref: None,
            }
        }
    };
    let mut idem_key = String::new();
    let mut payload_key_ref: Option<String> = None;
    let mut payload = Vec::new();
    for r in refs {
        if let Some(k) =
            r.0.strip_prefix(WAIT_IDEM_PREFIX)
                .or_else(|| r.0.strip_prefix(LEGACY_WAIT_IDEM_PREFIX))
        {
            idem_key = k.to_string();
        } else if r.0.starts_with(WAIT_SIGNAL_NAME_PREFIX) {
        } else if let Some(kr) =
            r.0.strip_prefix(WAIT_KEYREF_PREFIX)
                .or_else(|| r.0.strip_prefix(LEGACY_WAIT_KEYREF_PREFIX))
        {
            payload_key_ref = Some(kr.to_string());
        } else {
            payload.push(r.clone());
        }
    }
    if idem_key == WAIT_TIMEOUT_MARKER {
        return WaitOutcome::TimedOut;
    }
    WaitOutcome::Signalled {
        idem_key,
        payload,
        payload_key_ref,
    }
}

fn decode_received_signal_name(result: &Option<Vec<myelin_refs::ArtifactRef>>) -> Option<String> {
    result.as_ref()?.iter().find_map(|artifact| {
        artifact
            .0
            .strip_prefix(WAIT_SIGNAL_NAME_PREFIX)
            .map(ToOwned::to_owned)
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_attempts: u32,
}

impl RetryPolicy {
    pub const fn default_policy() -> Self {
        Self { max_attempts: 3 }
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::default_policy()
    }
}

#[derive(Clone, Default)]
pub struct WfJournal {
    inner: Arc<Mutex<JournalInner>>,
}

#[derive(Default)]
struct JournalInner {
    history: Vec<WfHistoryRow>,
    attempts: Vec<WfActivityAttemptRow>,
    journaled_commands: std::collections::HashSet<(String, String, String)>,
}

impl WfJournal {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, JournalInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn history_len(&self) -> usize {
        self.lock().history.len()
    }

    pub fn attempt_len(&self) -> usize {
        self.lock().attempts.len()
    }

    pub fn history_for(&self, tenant: &TenantId, run_id: &str) -> Vec<WfHistoryRow> {
        self.lock()
            .history
            .iter()
            .filter(|r| &r.tenant == tenant && r.run_id == run_id)
            .cloned()
            .collect()
    }

    pub fn history_in_tenant(&self, tenant: &TenantId) -> Vec<WfHistoryRow> {
        self.lock()
            .history
            .iter()
            .filter(|r| &r.tenant == tenant)
            .cloned()
            .collect()
    }

    pub fn all_history_in_seq_order(&self) -> Vec<WfHistoryRow> {
        self.lock().history.clone()
    }

    #[doc(hidden)]
    pub fn append_history_for_test(&self, row: WfHistoryRow) {
        self.commit_rows(vec![row], Vec::new());
    }

    pub fn attempts_for(&self, tenant: &TenantId, run_id: &str) -> Vec<WfActivityAttemptRow> {
        self.lock()
            .attempts
            .iter()
            .filter(|r| &r.tenant == tenant && r.run_id == run_id)
            .cloned()
            .collect()
    }

    pub fn is_journaled(&self, tenant: &TenantId, run_id: &str, command_id: &str) -> bool {
        self.lock().journaled_commands.contains(&(
            tenant.0.clone(),
            run_id.to_string(),
            command_id.to_string(),
        ))
    }

    fn commit_rows(&self, history: Vec<WfHistoryRow>, attempts: Vec<WfActivityAttemptRow>) {
        let mut inner = self.lock();
        for row in history {
            let key = (
                row.tenant.0.clone(),
                row.run_id.clone(),
                row.command_id.clone(),
            );
            if inner.journaled_commands.insert(key) {
                inner.history.push(row);
            } else if row.kind == crate::wfctx::history_kind::SIGNAL_RECEIVED {
                if let Some(existing) = inner.history.iter_mut().find(|h| {
                    h.tenant.0 == row.tenant.0
                        && h.run_id == row.run_id
                        && h.command_id == row.command_id
                        && h.kind == crate::wfctx::history_kind::SIGNAL_WAITED
                }) {
                    existing.kind = row.kind;
                    existing.result = row.result;
                    existing.result_key_ref = row.result_key_ref;
                }
            }
        }
        inner.attempts.extend(attempts);
    }
}

pub struct WfCtx {
    tenant: TenantId,
    region: Region,
    run_id: String,
    wf_type: String,
    tx: OutboxTransaction,
    journal: WfJournal,
    staged_history: Vec<WfHistoryRow>,
    staged_attempts: Vec<WfActivityAttemptRow>,
    command_seq: u64,
    history_seq: i64,
    rand_state: u64,
    now_clock: String,
    replay_history: std::collections::HashMap<String, ReplayedCommand>,
    side_effects_executed: u64,
    double_effects: u64,
    divergence: Option<String>,
    pinned_wf_version: Option<i32>,
    timers: Option<TimerContext>,
    parked_on_timer: bool,
    signals: Option<crate::engine::SignalStore>,
    parked_on_signal: bool,
    park_condition: Option<ParkCondition>,
    consumed_signals: Vec<(String, String)>,
    consumed_signal_commands: Vec<ConsumedSignalCommand>,
    pub(crate) budget: Option<crate::budget::BudgetGate>,
    pub(crate) run_identity: Option<crate::remint::RunTokenLease>,
    pub(crate) reminted_tokens: u64,
    pub(crate) job_dispatches: std::collections::HashMap<String, (String, Option<i64>, String)>,
    pub(crate) joined_job_dispatches: std::collections::HashSet<String>,
    pub(crate) disarmed_timer_ids: Vec<String>,
}

#[derive(Clone)]
struct TimerContext {
    store: crate::timer::TimerStore,
    partition: i16,
    now_unix_secs: i64,
}

#[derive(Clone, Debug)]
struct ReplayedCommand {
    seq: i64,
    kind: String,
    result: Option<Vec<myelin_refs::ArtifactRef>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsumedSignalCommand {
    pub command_id: String,
    pub signal_name: String,
    pub idem_key: String,
}

#[derive(Clone, Debug)]
pub struct StagedWfDrive {
    pub history: Vec<WfHistoryRow>,
    pub attempts: Vec<WfActivityAttemptRow>,
    pub timers: Vec<crate::timer::TimerRow>,
    pub outbox: Vec<myelin_events::OutboxRow>,
    pub consumed_signals: Vec<ConsumedSignalCommand>,
    pub disarmed_timer_ids: Vec<String>,
    pub park: Option<ParkCondition>,
}

impl WfCtx {
    #[allow(clippy::too_many_arguments)]
    pub fn begin(
        outbox: &OutboxStore,
        minter: Arc<dyn IdMinter>,
        journal: WfJournal,
        ctx_base: EmitContextBase,
        run_id: impl Into<String>,
        wf_type: impl Into<String>,
        now_clock: impl Into<String>,
        rand_seed: u64,
    ) -> Self {
        let tenant = ctx_base.tenant.clone();
        let region = ctx_base.region.clone();
        let tx = outbox.begin(minter, ctx_base);
        Self::begin_with_tx(
            tx, journal, tenant, region, run_id, wf_type, now_clock, rand_seed,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn begin_with_tx(
        tx: OutboxTransaction,
        journal: WfJournal,
        tenant: TenantId,
        region: Region,
        run_id: impl Into<String>,
        wf_type: impl Into<String>,
        now_clock: impl Into<String>,
        rand_seed: u64,
    ) -> Self {
        Self {
            tenant,
            region,
            run_id: run_id.into(),
            wf_type: wf_type.into(),
            tx,
            journal,
            staged_history: Vec::new(),
            staged_attempts: Vec::new(),
            command_seq: 0,
            history_seq: 0,
            rand_state: rand_seed,
            now_clock: now_clock.into(),
            replay_history: std::collections::HashMap::new(),
            side_effects_executed: 0,
            double_effects: 0,
            divergence: None,
            pinned_wf_version: None,
            timers: None,
            parked_on_timer: false,
            signals: None,
            parked_on_signal: false,
            park_condition: None,
            consumed_signals: Vec::new(),
            consumed_signal_commands: Vec::new(),
            budget: None,
            run_identity: None,
            reminted_tokens: 0,
            job_dispatches: std::collections::HashMap::new(),
            joined_job_dispatches: std::collections::HashSet::new(),
            disarmed_timer_ids: Vec::new(),
        }
    }

    pub fn with_timers(
        mut self,
        timers: crate::timer::TimerStore,
        partition: i16,
        now_secs: i64,
    ) -> Self {
        self.timers = Some(TimerContext {
            store: timers,
            partition,
            now_unix_secs: now_secs,
        });
        self
    }

    pub fn with_signals(mut self, signals: crate::engine::SignalStore) -> Self {
        self.signals = Some(signals);
        self
    }

    #[allow(clippy::too_many_arguments)]
    pub fn resume(
        outbox: &OutboxStore,
        minter: Arc<dyn IdMinter>,
        journal: WfJournal,
        ctx_base: EmitContextBase,
        run_id: impl Into<String>,
        wf_type: impl Into<String>,
        now_clock: impl Into<String>,
        rand_seed: u64,
        history: Vec<WfHistoryRow>,
    ) -> Self {
        let mut ctx = Self::begin(
            outbox, minter, journal, ctx_base, run_id, wf_type, now_clock, rand_seed,
        );
        ctx.history_seq = history
            .iter()
            .map(|r| r.seq)
            .max()
            .map(|m| m + 1)
            .unwrap_or(0);
        for row in history {
            ctx.replay_history.insert(
                row.command_id,
                ReplayedCommand {
                    seq: row.seq,
                    kind: row.kind,
                    result: row.result,
                },
            );
        }
        ctx
    }

    #[allow(clippy::too_many_arguments)]
    pub fn resume_versioned(
        outbox: &OutboxStore,
        minter: Arc<dyn IdMinter>,
        journal: WfJournal,
        ctx_base: EmitContextBase,
        run_id: impl Into<String>,
        wf_type: impl Into<String>,
        now_clock: impl Into<String>,
        rand_seed: u64,
        history: Vec<WfHistoryRow>,
        run_version: i32,
        replay_version: i32,
    ) -> Self {
        let mut ctx = Self::resume(
            outbox, minter, journal, ctx_base, run_id, wf_type, now_clock, rand_seed, history,
        );
        ctx.pinned_wf_version = Some(run_version);
        if run_version != replay_version {
            ctx.latch_divergence(format!(
                "wf_version pin mismatch: run pinned to v{run_version} but replayed with v{replay_version} \
                 (a deploy diverged an in-flight run, §4.6)"
            ));
        }
        ctx
    }

    #[allow(clippy::too_many_arguments)]
    pub fn resume_staged_versioned(
        minter: Arc<dyn IdMinter>,
        ctx_base: EmitContextBase,
        run_id: impl Into<String>,
        wf_type: impl Into<String>,
        now_clock: impl Into<String>,
        rand_seed: u64,
        history: Vec<WfHistoryRow>,
        run_version: i32,
        replay_version: i32,
    ) -> Self {
        let tenant = ctx_base.tenant.clone();
        let region = ctx_base.region.clone();
        let tx = OutboxTransaction::detached(minter, ctx_base);
        let mut ctx = Self::begin_with_tx(
            tx,
            WfJournal::new(),
            tenant,
            region,
            run_id,
            wf_type,
            now_clock,
            rand_seed,
        );
        ctx.history_seq = history
            .iter()
            .map(|row| row.seq)
            .max()
            .map(|seq| seq + 1)
            .unwrap_or(0);
        for row in history {
            ctx.replay_history.insert(
                row.command_id,
                ReplayedCommand {
                    seq: row.seq,
                    kind: row.kind,
                    result: row.result,
                },
            );
        }
        ctx.pinned_wf_version = Some(run_version);
        if run_version != replay_version {
            ctx.latch_divergence(format!(
                "wf_version pin mismatch: run pinned to v{run_version} but replayed with v{replay_version} \
                 (a deploy diverged an in-flight run, §4.6)"
            ));
        }
        ctx
    }

    pub fn divergence(&self) -> Option<&str> {
        self.divergence.as_deref()
    }

    pub fn is_divergent(&self) -> bool {
        self.divergence.is_some()
    }

    pub(crate) fn latch_divergence(&mut self, reason: String) {
        if self.divergence.is_none() {
            self.divergence = Some(reason);
        }
    }

    pub(crate) fn diverge(&mut self, reason: String) -> WfError {
        self.latch_divergence(reason.clone());
        WfError::Nondeterministic(reason)
    }

    fn halt_if_diverged(&self) -> WfResult<()> {
        match self.divergence.clone() {
            Some(r) => Err(WfError::Nondeterministic(r)),
            None => Ok(()),
        }
    }

    pub fn side_effects_executed(&self) -> u64 {
        self.side_effects_executed
    }

    pub fn double_effects(&self) -> u64 {
        self.double_effects
    }

    fn next_command_id(&mut self) -> String {
        let id = format!("{}:{}", self.wf_type, self.command_seq);
        self.command_seq += 1;
        id
    }

    pub(crate) fn peek_next_command_id(&self) -> String {
        format!("{}:{}", self.wf_type, self.command_seq)
    }

    pub(crate) fn is_replaying_command(&self, command_id: &str) -> bool {
        self.replay_history.contains_key(command_id)
    }

    fn next_history_seq(&mut self) -> i64 {
        let s = self.history_seq;
        self.history_seq += 1;
        s
    }

    fn stage_history(
        &mut self,
        kind: &str,
        command_id: String,
        result: Option<Vec<myelin_refs::ArtifactRef>>,
    ) {
        let seq = self.next_history_seq();
        self.staged_history.push(WfHistoryRow {
            tenant: self.tenant.clone(),
            region: self.region.clone(),
            run_id: self.run_id.clone(),
            seq,
            kind: kind.to_string(),
            command_id,
            result,
            result_key_ref: None,
        });
    }

    fn stage_history_at(
        &mut self,
        seq: i64,
        kind: &str,
        command_id: String,
        result: Option<Vec<myelin_refs::ArtifactRef>>,
    ) {
        self.staged_history.push(WfHistoryRow {
            tenant: self.tenant.clone(),
            region: self.region.clone(),
            run_id: self.run_id.clone(),
            seq,
            kind: kind.to_string(),
            command_id,
            result,
            result_key_ref: None,
        });
    }

    pub fn activity<F>(
        &mut self,
        policy: RetryPolicy,
        run: F,
    ) -> WfResult<Vec<myelin_refs::ArtifactRef>>
    where
        F: Fn(&str, u32) -> Result<Vec<myelin_refs::ArtifactRef>, ActivityError>,
    {
        self.halt_if_diverged()?;
        let command_id = self.next_command_id();
        if let Some(replayed) = self.replay_history.get(&command_id) {
            match replayed.kind.as_str() {
                history_kind::ACTIVITY_COMPLETED => {
                    return Ok(replayed.result.clone().unwrap_or_default());
                }
                history_kind::ACTIVITY_FAILED => {
                    return Err(WfError::ActivityExhausted(ActivityError::permanent(
                        "replayed activity_failed",
                    )));
                }
                other => {
                    return Err(self.diverge(format!(
                        "replay divergence at {command_id}: body issued `activity` but the journal \
                         records kind `{other}` (the workflow body diverged from its journal)"
                    )));
                }
            }
        }
        self.side_effects_executed += 1;
        let idem_token = format!("{}/{}/{}", self.run_id, command_id, "act");
        let max = policy.max_attempts.max(1);
        for attempt in 1..=max {
            match run(&idem_token, attempt) {
                Ok(result) => {
                    self.stage_history(
                        history_kind::ACTIVITY_COMPLETED,
                        command_id.clone(),
                        Some(result.clone()),
                    );
                    self.staged_attempts.push(self.attempt_row(
                        &command_id,
                        attempt,
                        &idem_token,
                        attempt_state::SUCCEEDED,
                        None,
                    ));
                    return Ok(result);
                }
                Err(e) => {
                    let final_attempt = attempt == max || !e.is_retryable();
                    self.staged_attempts.push(self.attempt_row(
                        &command_id,
                        attempt,
                        &idem_token,
                        if final_attempt {
                            attempt_state::FAILED
                        } else {
                            attempt_state::RETRYING
                        },
                        Some(e.detail().to_owned()),
                    ));
                    if final_attempt {
                        self.stage_history(history_kind::ACTIVITY_FAILED, command_id, None);
                        return Err(WfError::ActivityExhausted(e));
                    }
                }
            }
        }
        unreachable!("an activity attempt either succeeds or returns its final error")
    }

    fn attempt_row(
        &self,
        command_id: &str,
        attempt: u32,
        idem_token: &str,
        state: &str,
        error: Option<String>,
    ) -> WfActivityAttemptRow {
        WfActivityAttemptRow {
            tenant: self.tenant.clone(),
            region: self.region.clone(),
            run_id: self.run_id.clone(),
            command_id: command_id.to_string(),
            attempt: attempt as i32,
            idem_token: idem_token.to_string(),
            state: state.to_string(),
            error,
            started_at: Some(self.now_clock.clone()),
            ended_at: Some(self.now_clock.clone()),
        }
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub(crate) fn tenant_id(&self) -> &TenantId {
        &self.tenant
    }

    pub fn now(&mut self) -> String {
        if self.is_divergent() {
            return self.now_clock.clone();
        }
        let command_id = self.next_command_id();
        if self.divergent_marker(&command_id) {
            return self.now_clock.clone();
        }
        if let Some(value) = self.replayed_marker_value(&command_id) {
            return value;
        }
        let value = self.now_clock.clone();
        self.stage_marker_value(command_id, &value);
        value
    }

    pub fn rand(&mut self) -> u64 {
        if self.is_divergent() {
            return 0;
        }
        let command_id = self.next_command_id();
        if self.divergent_marker(&command_id) {
            return 0;
        }
        if let Some(value) = self.replayed_marker_value(&command_id) {
            return value.parse::<u64>().unwrap_or(0);
        }
        self.rand_state = self.rand_state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.rand_state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        let value = z ^ (z >> 31);
        self.stage_marker_value(command_id, &value.to_string());
        value
    }

    fn divergent_marker(&mut self, command_id: &str) -> bool {
        let Some(replayed) = self.replay_history.get(command_id) else {
            return false;
        };
        if replayed.kind == history_kind::SIDE_MARKER {
            return false;
        }
        let kind = replayed.kind.clone();
        self.latch_divergence(format!(
            "replay divergence at {command_id}: body issued a side-marker (`now`/`rand`) but the \
             journal records kind `{kind}` (the workflow body diverged from its journal)"
        ));
        true
    }

    fn replayed_marker_value(&self, command_id: &str) -> Option<String> {
        let replayed = self.replay_history.get(command_id)?;
        if replayed.kind != history_kind::SIDE_MARKER {
            return None;
        }
        replayed
            .result
            .as_ref()
            .and_then(|refs| refs.first())
            .map(|r| r.0.clone())
    }

    fn stage_marker_value(&mut self, command_id: String, value: &str) {
        self.stage_history(
            history_kind::SIDE_MARKER,
            command_id,
            Some(vec![myelin_refs::ArtifactRef(value.to_string())]),
        );
    }

    pub fn sleep_until(&mut self, fire_at_secs: i64) -> WfResult<()> {
        self.halt_if_diverged()?;
        let command_id = self.next_command_id();
        if let Some(replayed) = self.replay_history.get(&command_id) {
            match replayed.kind.as_str() {
                crate::timer::history_kind::TIMER_SET => return Ok(()),
                other => {
                    return Err(self.diverge(format!(
                        "replay divergence at {command_id}: body issued `sleep` but the journal records \
                         kind `{other}` (the workflow body diverged from its journal)"
                    )));
                }
            }
        }
        let timer = self.timers.clone().ok_or_else(|| {
            WfError::CoCommit("sleep_until requires a timer wheel (WfCtx::with_timers)".into())
        })?;
        let timer_id = format!("{}/{}", self.run_id, command_id);
        let bucket = crate::timer::epoch_minute(fire_at_secs);
        timer.store.arm(crate::timer::TimerRow {
            tenant: self.tenant.clone(),
            region: self.region.clone(),
            timer_id: timer_id.clone(),
            run_id: Some(self.run_id.clone()),
            command_id: command_id.clone(),
            fire_at: fire_at_secs,
            bucket,
            fired: false,
            partition: timer.partition,
        });
        self.stage_history(crate::timer::history_kind::TIMER_SET, command_id, None);
        if fire_at_secs > timer.now_unix_secs {
            self.parked_on_timer = true;
            self.park_condition = Some(ParkCondition::Timer { timer_id });
        }
        Ok(())
    }

    pub fn sleep_for(&mut self, duration_secs: i64) -> WfResult<()> {
        let fire_at = self.timer_deadline_after(duration_secs.max(0), "sleep_for")?;
        self.sleep_until(fire_at)
    }

    pub(crate) fn timer_now_unix_secs(&self, operation: &str) -> WfResult<i64> {
        self.timers
            .as_ref()
            .map(|timer| timer.now_unix_secs)
            .ok_or_else(|| {
                WfError::CoCommit(format!(
                    "{operation} requires a durable timer wheel (WfCtx::with_timers)"
                ))
            })
    }

    pub(crate) fn timer_deadline_after(
        &self,
        duration_secs: i64,
        operation: &str,
    ) -> WfResult<i64> {
        self.timer_now_unix_secs(operation)?
            .checked_add(duration_secs)
            .ok_or_else(|| {
                WfError::CoCommit(format!(
                    "{operation} deadline is outside the Unix time range"
                ))
            })
    }

    pub fn wait_for_signal(
        &mut self,
        name: &str,
        timeout_secs: Option<i64>,
    ) -> WfResult<WaitOutcome> {
        self.wait_for_signal_inner(name, None, timeout_secs, None, true)
    }

    pub fn wait_for_signal_exact(
        &mut self,
        name: &str,
        idem_key: &str,
        timeout_secs: Option<i64>,
    ) -> WfResult<WaitOutcome> {
        self.wait_for_signal_inner(name, Some(idem_key), timeout_secs, None, true)
    }

    pub fn wait_for_signal_exact_until(
        &mut self,
        name: &str,
        idem_key: &str,
        deadline_unix_secs: Option<i64>,
    ) -> WfResult<WaitOutcome> {
        self.wait_for_signal_inner(name, Some(idem_key), None, deadline_unix_secs, true)
    }

    pub(crate) fn wait_for_signal_exact_until_prearmed(
        &mut self,
        name: &str,
        idem_key: &str,
        deadline_unix_secs: Option<i64>,
    ) -> WfResult<WaitOutcome> {
        self.wait_for_signal_inner(name, Some(idem_key), None, deadline_unix_secs, false)
    }

    fn wait_for_signal_inner(
        &mut self,
        name: &str,
        expected_idem_key: Option<&str>,
        timeout_secs: Option<i64>,
        absolute_deadline: Option<i64>,
        arm_timeout: bool,
    ) -> WfResult<WaitOutcome> {
        self.halt_if_diverged()?;
        let command_id = self.next_command_id();

        if let Some(replayed) = self.replay_history.get(&command_id).cloned() {
            match replayed.kind.as_str() {
                history_kind::SIGNAL_RECEIVED => {
                    let outcome = decode_received(&replayed.result);
                    if let (Some(expected), WaitOutcome::Signalled { idem_key, .. }) =
                        (expected_idem_key, &outcome)
                    {
                        if idem_key != expected {
                            return Err(self.diverge(format!(
                                "replay divergence at {command_id}: exact wait expected idem_key \
                                 `{expected}` but the journal records `{idem_key}`"
                            )));
                        }
                    }
                    if expected_idem_key.is_some() {
                        if let Some(recorded_name) = decode_received_signal_name(&replayed.result) {
                            if recorded_name != name {
                                return Err(self.diverge(format!(
                                    "replay divergence at {command_id}: exact wait expected signal \
                                     name `{name}` but the journal records `{recorded_name}`"
                                )));
                            }
                        }
                    }
                    return Ok(outcome);
                }
                crate::wfctx::history_kind::SIGNAL_WAITED => {}
                other => {
                    return Err(self.diverge(format!(
                        "replay divergence at {command_id}: body issued `wait_for_signal` but the \
                         journal records kind `{other}` (the workflow body diverged from its journal)"
                    )));
                }
            }
        }

        let signals = self.signals.clone().ok_or_else(|| {
            WfError::CoCommit(
                "wait_for_signal requires a signal store (WfCtx::with_signals)".into(),
            )
        })?;

        let already_waited = self
            .replay_history
            .get(&command_id)
            .map(|r| r.kind == crate::wfctx::history_kind::SIGNAL_WAITED)
            .unwrap_or(false);

        if already_waited {
            if let Some(recorded) = self.replayed_wait_expected_idem(&command_id) {
                if Some(recorded.as_str()) != expected_idem_key {
                    return Err(self.diverge(format!(
                        "replay divergence at {command_id}: exact wait expected idem_key {:?} but \
                         the journaled wait expects `{recorded}`",
                        expected_idem_key
                    )));
                }
            }
            if expected_idem_key.is_some() {
                if let Some(recorded_name) = self.replayed_wait_expected_name(&command_id) {
                    if recorded_name != name {
                        return Err(self.diverge(format!(
                            "replay divergence at {command_id}: exact wait expected signal name \
                             `{name}` but the journaled wait expects `{recorded_name}`"
                        )));
                    }
                }
            }
        }

        let timer = if timeout_secs.is_some() || absolute_deadline.is_some() {
            Some(self.timers.clone().ok_or_else(|| {
                WfError::CoCommit(
                    "a timed signal wait requires a durable timer wheel (WfCtx::with_timers)"
                        .into(),
                )
            })?)
        } else {
            None
        };
        let requested_timed_wait = timer.is_some();
        let effective_deadline = if already_waited {
            let recorded_deadline = match self.replayed_wait_deadline(&command_id) {
                Ok(deadline) => deadline,
                Err(detail) => return Err(self.diverge(detail)),
            };
            match (requested_timed_wait, recorded_deadline) {
                (true, Some(deadline)) => Some(deadline),
                (false, None) => None,
                (true, None) => {
                    return Err(self.diverge(format!(
                        "replay divergence at {command_id}: body issued a timed signal wait but \
                         the journal records an untimed wait"
                    )));
                }
                (false, Some(_)) => {
                    return Err(self.diverge(format!(
                        "replay divergence at {command_id}: body issued an untimed signal wait but \
                         the journal records a timed wait"
                    )));
                }
            }
        } else {
            match (absolute_deadline, timeout_secs) {
                (Some(deadline), _) => Some(deadline),
                (None, Some(timeout)) => {
                    Some(self.timer_deadline_after(timeout, "timed signal wait")?)
                }
                (None, None) => None,
            }
        };

        let candidate = match expected_idem_key {
            Some(idem_key) => signals
                .unconsumed_for_exact(&self.tenant, &self.run_id, name, idem_key)
                .map(|row| (idem_key.to_string(), row)),
            None => signals.first_unconsumed_for(&self.tenant, &self.run_id, name),
        };
        if let Some((idem_key, row)) = candidate {
            if effective_deadline.is_some_and(|deadline| {
                i128::from(row.received_unix_ms) > i128::from(deadline) * 1_000
            }) {
                self.stage_received(
                    command_id,
                    Some(name),
                    WAIT_TIMEOUT_MARKER,
                    &[],
                    Some(WAIT_TIMEOUT_MARKER),
                );
                if already_waited {
                    self.remint_if_resuming()?;
                }
                return Ok(WaitOutcome::TimedOut);
            }
            let received_seq = self
                .replay_history
                .get(&command_id)
                .filter(|row| row.kind == history_kind::SIGNAL_WAITED)
                .map(|row| row.seq)
                .unwrap_or(self.history_seq);
            signals.consume(&self.tenant, &self.run_id, name, &idem_key, received_seq);
            self.consumed_signals
                .push((name.to_string(), idem_key.clone()));
            self.consumed_signal_commands.push(ConsumedSignalCommand {
                command_id: command_id.clone(),
                signal_name: name.to_string(),
                idem_key: idem_key.clone(),
            });
            self.stage_received(
                command_id,
                Some(name),
                &idem_key,
                &row.payload,
                row.payload_key_ref.as_deref(),
            );
            if already_waited {
                self.remint_if_resuming()?;
            }
            return Ok(WaitOutcome::Signalled {
                idem_key,
                payload: row.payload,
                payload_key_ref: row.payload_key_ref,
            });
        }

        if let Some(deadline) = effective_deadline {
            let timer = timer
                .as_ref()
                .ok_or_else(|| WfError::CoCommit("timed signal wait lost its timer".into()))?;
            if timer.now_unix_secs >= deadline {
                self.stage_received(
                    command_id,
                    Some(name),
                    WAIT_TIMEOUT_MARKER,
                    &[],
                    Some(WAIT_TIMEOUT_MARKER),
                );
                if already_waited {
                    self.remint_if_resuming()?;
                }
                return Ok(WaitOutcome::TimedOut);
            }
            if arm_timeout {
                let timer_id = format!("{}/{}/timeout", self.run_id, command_id);
                let bucket = crate::timer::epoch_minute(deadline);
                timer.store.arm(crate::timer::TimerRow {
                    tenant: self.tenant.clone(),
                    region: self.region.clone(),
                    timer_id,
                    run_id: Some(self.run_id.clone()),
                    command_id: command_id.clone(),
                    fire_at: deadline,
                    bucket,
                    fired: false,
                    partition: timer.partition,
                });
            }
            if !already_waited {
                self.stage_waited(
                    command_id,
                    Some(deadline),
                    expected_idem_key.map(|_| name),
                    expected_idem_key,
                );
            }
        } else if !already_waited {
            self.stage_waited(
                command_id,
                None,
                expected_idem_key.map(|_| name),
                expected_idem_key,
            );
        }
        self.parked_on_signal = true;
        self.park_condition = Some(ParkCondition::Signal {
            name: name.to_string(),
            idem_key: expected_idem_key.map(str::to_string),
        });
        Ok(WaitOutcome::Parked)
    }

    pub(crate) fn arm_job_deadline(
        &mut self,
        dispatch_command_id: &str,
        deadline: i64,
    ) -> WfResult<()> {
        let timer = self.timers.clone().ok_or_else(|| {
            WfError::CoCommit(
                "a timed job dispatch requires a durable timer wheel (WfCtx::with_timers)".into(),
            )
        })?;
        let command_id = format!("{dispatch_command_id}/job-timeout");
        timer.store.arm(crate::timer::TimerRow {
            tenant: self.tenant.clone(),
            region: self.region.clone(),
            timer_id: format!("{}/{command_id}", self.run_id),
            run_id: Some(self.run_id.clone()),
            command_id,
            fire_at: deadline,
            bucket: crate::timer::epoch_minute(deadline),
            fired: false,
            partition: timer.partition,
        });
        Ok(())
    }

    pub(crate) fn disarm_job_deadline(&mut self, dispatch_command_id: &str) -> WfResult<()> {
        let timer = self.timers.clone().ok_or_else(|| {
            WfError::CoCommit(
                "a timed job join requires a durable timer wheel (WfCtx::with_timers)".into(),
            )
        })?;
        let timer_id = format!("{}/{dispatch_command_id}/job-timeout", self.run_id);
        timer.store.disarm(&self.tenant, &timer_id);
        if !self.disarmed_timer_ids.contains(&timer_id) {
            self.disarmed_timer_ids.push(timer_id);
        }
        Ok(())
    }

    fn replayed_wait_deadline(&self, command_id: &str) -> Result<Option<i64>, String> {
        let Some(replayed) = self.replay_history.get(command_id) else {
            return Ok(None);
        };
        if replayed.kind != crate::wfctx::history_kind::SIGNAL_WAITED {
            return Ok(None);
        }
        let Some(encoded) = replayed.result.as_ref().and_then(|refs| {
            refs.iter()
                .find_map(|r| r.0.strip_prefix(WAIT_DEADLINE_PREFIX))
        }) else {
            return Ok(None);
        };
        encoded.parse::<i64>().map(Some).map_err(|_| {
            format!(
                "replay divergence at {command_id}: journaled signal deadline `{encoded}` is not \
                 a Unix timestamp"
            )
        })
    }

    fn replayed_wait_expected_idem(&self, command_id: &str) -> Option<String> {
        let replayed = self.replay_history.get(command_id)?;
        if replayed.kind != crate::wfctx::history_kind::SIGNAL_WAITED {
            return None;
        }
        replayed.result.as_ref()?.iter().find_map(|artifact| {
            artifact
                .0
                .strip_prefix(WAIT_EXPECTED_IDEM_PREFIX)
                .map(ToOwned::to_owned)
        })
    }

    fn replayed_wait_expected_name(&self, command_id: &str) -> Option<String> {
        let replayed = self.replay_history.get(command_id)?;
        if replayed.kind != crate::wfctx::history_kind::SIGNAL_WAITED {
            return None;
        }
        replayed.result.as_ref()?.iter().find_map(|artifact| {
            artifact
                .0
                .strip_prefix(WAIT_EXPECTED_NAME_PREFIX)
                .map(ToOwned::to_owned)
        })
    }

    fn stage_waited(
        &mut self,
        command_id: String,
        deadline: Option<i64>,
        expected_signal_name: Option<&str>,
        expected_idem_key: Option<&str>,
    ) {
        let mut markers = Vec::new();
        if let Some(deadline) = deadline {
            markers.push(myelin_refs::ArtifactRef(format!(
                "{WAIT_DEADLINE_PREFIX}{deadline}"
            )));
        }
        if let Some(idem_key) = expected_idem_key {
            markers.push(myelin_refs::ArtifactRef(format!(
                "{WAIT_EXPECTED_IDEM_PREFIX}{idem_key}"
            )));
        }
        if let Some(signal_name) = expected_signal_name {
            markers.push(myelin_refs::ArtifactRef(format!(
                "{WAIT_EXPECTED_NAME_PREFIX}{signal_name}"
            )));
        }
        let result = (!markers.is_empty()).then_some(markers);
        self.stage_history(
            crate::wfctx::history_kind::SIGNAL_WAITED,
            command_id,
            result,
        );
    }

    fn stage_received(
        &mut self,
        command_id: String,
        signal_name: Option<&str>,
        idem_key: &str,
        payload: &[myelin_refs::ArtifactRef],
        payload_key_ref: Option<&str>,
    ) {
        let mut result = vec![myelin_refs::ArtifactRef(format!(
            "{WAIT_IDEM_PREFIX}{idem_key}"
        ))];
        if let Some(signal_name) = signal_name {
            result.push(myelin_refs::ArtifactRef(format!(
                "{WAIT_SIGNAL_NAME_PREFIX}{signal_name}"
            )));
        }
        if let Some(kr) = payload_key_ref {
            result.push(myelin_refs::ArtifactRef(format!(
                "{WAIT_KEYREF_PREFIX}{kr}"
            )));
        }
        result.extend(payload.iter().cloned());
        let replayed_wait_seq = self
            .replay_history
            .get(&command_id)
            .filter(|row| row.kind == history_kind::SIGNAL_WAITED)
            .map(|row| row.seq);
        if let Some(seq) = replayed_wait_seq {
            self.stage_history_at(
                seq,
                crate::wfctx::history_kind::SIGNAL_RECEIVED,
                command_id,
                Some(result),
            );
        } else {
            self.stage_history(
                crate::wfctx::history_kind::SIGNAL_RECEIVED,
                command_id,
                Some(result),
            );
        }
    }

    pub fn parked_on_timer(&self) -> bool {
        self.parked_on_timer
    }

    pub fn parked_on_signal(&self) -> bool {
        self.parked_on_signal
    }

    pub fn parked(&self) -> bool {
        self.parked_on_timer || self.parked_on_signal
    }

    pub fn park_condition(&self) -> Option<&ParkCondition> {
        self.park_condition.as_ref()
    }

    pub fn consumed_signals(&self) -> &[(String, String)] {
        &self.consumed_signals
    }

    pub fn emit(&mut self, draft: EventDraft, cause: Option<&EventEnvelope>) -> WfResult<EventId> {
        self.emit_inner(draft, cause)
            .map_err(|e| WfError::CoCommit(e.0))
    }

    fn emit_inner(
        &mut self,
        draft: EventDraft,
        cause: Option<&EventEnvelope>,
    ) -> EmitResult<EventId> {
        self.tx.emit(draft, cause)
    }

    pub fn into_staged_drive(self) -> WfResult<StagedWfDrive> {
        let timers = self
            .timers
            .as_ref()
            .map(|timer| {
                timer
                    .store
                    .rows_for_run(&self.tenant, &self.region, &self.run_id)
            })
            .unwrap_or_default();
        let WfCtx {
            tx,
            staged_history,
            staged_attempts,
            consumed_signal_commands,
            disarmed_timer_ids,
            park_condition,
            ..
        } = self;
        let outbox = tx
            .into_staged_rows()
            .map_err(|error| WfError::CoCommit(error.0))?;
        Ok(StagedWfDrive {
            history: staged_history,
            attempts: staged_attempts,
            timers,
            outbox,
            consumed_signals: consumed_signal_commands,
            disarmed_timer_ids,
            park: park_condition,
        })
    }

    pub fn commit(self) -> WfResult<()> {
        let WfCtx {
            tx,
            journal,
            staged_history,
            staged_attempts,
            ..
        } = self;
        tx.commit().map_err(|e| WfError::CoCommit(e.0))?;
        journal.commit_rows(staged_history, staged_attempts);
        Ok(())
    }

    pub fn staged_history_len(&self) -> usize {
        self.staged_history.len()
    }

    pub fn staged_emit_len(&self) -> usize {
        self.tx.staged_len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        Actor, AggregateKey, ArtifactRef as EvArtifactRef, DataRole, EventType, MonotonicMinter,
        Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use myelin_refs::ArtifactRef;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn principal() -> Principal {
        Principal::stub(PrincipalId("p".into()), PrincipalKind::Human, tenant())
    }
    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: tenant(),
            region: region(),
            actor: Actor(principal()),
            schema_ver: 1,
            occurred_at: myelin_events::Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: myelin_events::Timestamp("2026-06-21T00:00:01Z".into()),
            caused_by: None,
        }
    }
    fn minter() -> Arc<dyn IdMinter> {
        Arc::new(MonotonicMinter::new()) as Arc<dyn IdMinter>
    }
    fn draft(type_: &str) -> EventDraft {
        EventDraft {
            type_: EventType(type_.into()),
            subject: EvArtifactRef("myelin://acme/agent/run/R1".into()),
            aggregate: AggregateKey("run:R1".into()),
            payload: serde_json::json!({ "ref": "R1" }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        }
    }
    fn begin(outbox: &OutboxStore, journal: WfJournal) -> WfCtx {
        WfCtx::begin(
            outbox,
            minter(),
            journal,
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
        )
    }

    #[test]
    fn activity_journals_exactly_one_history_row_under_its_command_id() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let mut ctx = begin(&outbox, journal.clone());
        let out = ctx
            .activity(RetryPolicy::default_policy(), |_idem, _attempt| {
                Ok(vec![ArtifactRef("myelin://acme/agent/effect/e1".into())])
            })
            .expect("the activity succeeds");
        assert_eq!(out.len(), 1, "the activity returned its result refs");
        assert_eq!(ctx.staged_history_len(), 1, "one history row staged");
        assert_eq!(journal.history_len(), 0, "nothing durable before commit");
        ctx.commit().expect("co-commit");
        let hist = journal.history_for(&tenant(), "R1");
        assert_eq!(
            hist.len(),
            1,
            "exactly one history row journaled for the command"
        );
        assert_eq!(hist[0].kind, history_kind::ACTIVITY_COMPLETED);
        assert_eq!(
            hist[0].command_id, "agent.run:0",
            "deterministic command_id from position"
        );
        assert_eq!(hist[0].seq, 0, "the per-run replay-order seq starts at 0");
        let attempts = journal.attempts_for(&tenant(), "R1");
        assert_eq!(attempts.len(), 1, "one attempt ledger row");
        assert_eq!(attempts[0].state, attempt_state::SUCCEEDED);
    }

    #[test]
    fn emit_and_journal_share_one_txn_co_commit() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let mut ctx = begin(&outbox, journal.clone());
        ctx.activity(RetryPolicy::default_policy(), |_idem, _attempt| {
            Ok(vec![ArtifactRef("myelin://acme/agent/effect/e1".into())])
        })
        .expect("activity");
        ctx.emit(draft("agent.run.step"), None)
            .expect("emit buffers into the txn");
        assert_eq!(journal.history_len(), 0, "no journal row before commit");
        assert_eq!(outbox.outbox_depth(), 0, "no outbox row before commit");
        assert_eq!(ctx.staged_history_len(), 1, "history staged");
        assert_eq!(ctx.staged_emit_len(), 1, "emit staged");
        ctx.commit().expect("co-commit");
        assert_eq!(journal.history_len(), 1, "journal row durable after commit");
        assert_eq!(outbox.outbox_depth(), 1, "outbox row durable after commit");
    }

    #[test]
    fn flow_d5_crash_between_journal_and_emit_is_atomic_neither() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        {
            let mut ctx = begin(&outbox, journal.clone());
            ctx.activity(RetryPolicy::default_policy(), |_idem, _attempt| {
                Ok(vec![ArtifactRef("myelin://acme/agent/effect/e1".into())])
            })
            .expect("activity");
            ctx.emit(draft("agent.run.step"), None)
                .expect("emit buffers");
            assert_eq!(ctx.staged_history_len(), 1, "journaled-but-not-committed");
            assert_eq!(ctx.staged_emit_len(), 1, "emitted-but-not-committed");
        }
        assert_eq!(
            journal.history_len(),
            0,
            "0 lost: an aborted step journals nothing"
        );
        assert_eq!(
            journal.attempt_len(),
            0,
            "0 lost: the attempt ledger row is not written either"
        );
        assert_eq!(
            outbox.outbox_depth(),
            0,
            "0 ghost: an aborted step emits nothing"
        );
        assert_eq!(
            outbox.committed_count(),
            0,
            "no committed outbox row from an abort"
        );
    }

    #[test]
    fn retried_activity_reuses_its_idem_token_no_duplicate_effect() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let mut ctx = begin(&outbox, journal.clone());
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen2 = seen.clone();
        let out = ctx
            .activity(RetryPolicy { max_attempts: 3 }, move |idem, attempt| {
                seen2.lock().unwrap().push(idem.to_string());
                if attempt < 3 {
                    Err(ActivityError::retryable(format!(
                        "transient failure on attempt {attempt}"
                    )))
                } else {
                    Ok(vec![ArtifactRef("myelin://acme/agent/effect/e1".into())])
                }
            })
            .expect("the activity succeeds on the third attempt");
        assert_eq!(out.len(), 1);
        let tokens = seen.lock().unwrap().clone();
        assert_eq!(tokens.len(), 3, "three attempts ran");
        assert!(
            tokens.iter().all(|t| t == &tokens[0]),
            "every attempt reused the SAME idem_token (the BUS-2 dedup anchor): {tokens:?}"
        );
        ctx.commit().expect("co-commit");
        let hist = journal.history_for(&tenant(), "R1");
        let completed: Vec<_> = hist
            .iter()
            .filter(|r| r.kind == history_kind::ACTIVITY_COMPLETED)
            .collect();
        assert_eq!(
            completed.len(),
            1,
            "exactly one activity_completed row (no duplicate effect)"
        );
        let attempts = journal.attempts_for(&tenant(), "R1");
        assert_eq!(attempts.len(), 3, "three attempt ledger rows");
        assert!(
            attempts
                .iter()
                .all(|a| a.idem_token == attempts[0].idem_token),
            "all attempts share one idem_token"
        );
        assert_eq!(
            attempts[2].state,
            attempt_state::SUCCEEDED,
            "the third attempt succeeded"
        );
    }

    #[test]
    fn exhausted_activity_journals_failed_and_returns_error() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let mut ctx = begin(&outbox, journal.clone());
        let err = ctx
            .activity(RetryPolicy { max_attempts: 2 }, |_idem, attempt| {
                Err(ActivityError::retryable(format!("hard failure {attempt}")))
            })
            .expect_err("the activity exhausts its retries");
        match err {
            WfError::ActivityExhausted(error) => {
                assert_eq!(
                    error.detail(),
                    "hard failure 2",
                    "the LAST attempt's error surfaces"
                )
            }
            other => panic!("expected ActivityExhausted, got {other:?}"),
        }
        ctx.commit().expect("co-commit");
        let hist = journal.history_for(&tenant(), "R1");
        let failed: Vec<_> = hist
            .iter()
            .filter(|r| r.kind == history_kind::ACTIVITY_FAILED)
            .collect();
        assert_eq!(failed.len(), 1, "exactly one activity_failed history row");
        assert!(
            !hist
                .iter()
                .any(|r| r.kind == history_kind::ACTIVITY_COMPLETED),
            "no completed row for a fully-failed activity"
        );
        let attempts = journal.attempts_for(&tenant(), "R1");
        assert_eq!(attempts.len(), 2, "both attempts in the ledger");
        assert_eq!(
            attempts[1].state,
            attempt_state::FAILED,
            "the last attempt is FAILED"
        );
    }

    #[test]
    fn permanent_activity_failure_is_not_retried() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let mut ctx = begin(&outbox, journal.clone());
        let attempts_run = Arc::new(Mutex::new(0_u32));
        let observed = attempts_run.clone();

        let error = ctx
            .activity(RetryPolicy { max_attempts: 9 }, move |_idem, _attempt| {
                *observed.lock().unwrap() += 1;
                Err(ActivityError::permanent(
                    "privacy erasure cannot become retryable",
                ))
            })
            .expect_err("a permanent failure ends the activity immediately");

        assert_eq!(*attempts_run.lock().unwrap(), 1);
        assert!(matches!(
            error,
            WfError::ActivityExhausted(ActivityError::Permanent(_))
        ));
        ctx.commit().expect("co-commit the permanent failure");
        let attempts = journal.attempts_for(&tenant(), "R1");
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].state, attempt_state::FAILED);
        assert_eq!(
            attempts[0].error.as_deref(),
            Some("privacy erasure cannot become retryable")
        );
    }

    #[test]
    fn now_and_rand_are_journaled_deterministic_side_markers() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let mut ctx = begin(&outbox, journal.clone());
        let t1 = ctx.now();
        let r1 = ctx.rand();
        let r2 = ctx.rand();
        assert_eq!(
            t1, "2026-06-21T00:00:00Z",
            "now() returns the deterministic clock"
        );
        assert_ne!(r1, r2, "rand() advances (a sequence, not a constant)");
        assert_eq!(
            r1, 13_679_457_532_755_275_413,
            "rand() draw 1 is the frozen splitmix64(42) value"
        );
        assert_eq!(
            r2, 2_949_826_092_126_892_291,
            "rand() draw 2 is the frozen splitmix64 value"
        );
        assert_eq!(
            ctx.staged_history_len(),
            3,
            "now/rand each journal a side-marker"
        );
        ctx.commit().expect("co-commit");
        let markers: Vec<_> = journal
            .history_for(&tenant(), "R1")
            .into_iter()
            .filter(|r| r.kind == history_kind::SIDE_MARKER)
            .collect();
        assert_eq!(markers.len(), 3, "three side-marker rows journaled");

        let outbox2 = OutboxStore::new();
        let mut ctx2 = begin(&outbox2, WfJournal::new());
        assert_eq!(ctx2.now(), t1, "now() is replay-stable");
        assert_eq!(ctx2.rand(), r1, "rand() draw 1 is replay-stable");
        assert_eq!(ctx2.rand(), r2, "rand() draw 2 is replay-stable");
    }

    #[test]
    fn journaling_is_idempotent_on_command_id() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let mut ctx = begin(&outbox, journal.clone());
        ctx.activity(RetryPolicy::default_policy(), |_i, _a| {
            Ok(vec![ArtifactRef("myelin://acme/agent/effect/e1".into())])
        })
        .expect("activity");
        ctx.commit().expect("co-commit");
        assert_eq!(journal.history_len(), 1);
        assert!(journal.is_journaled(&tenant(), "R1", "agent.run:0"));
        let outbox2 = OutboxStore::new();
        let mut ctx2 = begin(&outbox2, journal.clone());
        ctx2.activity(RetryPolicy::default_policy(), |_i, _a| {
            Ok(vec![ArtifactRef("myelin://acme/agent/effect/e1".into())])
        })
        .expect("activity");
        ctx2.commit().expect("co-commit");
        assert_eq!(
            journal.history_len(),
            1,
            "the re-journal of agent.run:0 is a no-op (UNIQUE(tenant, run_id, command_id))"
        );
        assert!(
            !journal.is_journaled(&tenant(), "R1", "agent.run:99"),
            "an un-journaled command_id reads false (the idempotency check is real, not vacuous)"
        );
        assert!(
            !journal.is_journaled(&tenant(), "R-other", "agent.run:0"),
            "is_journaled is keyed on the run too (a different run's same command is not journaled)"
        );
    }

    #[test]
    fn journal_reads_are_per_run_and_seq_is_monotonic() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let minter_shared = minter();
        let mut c1 = WfCtx::begin(
            &outbox,
            minter_shared.clone(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
        );
        let _ = c1.now();
        c1.activity(RetryPolicy::default_policy(), |_i, _a| {
            Ok(vec![ArtifactRef("myelin://acme/agent/effect/e1".into())])
        })
        .expect("activity");
        let _ = c1.rand();
        c1.commit().expect("co-commit R1");

        let mut c2 = WfCtx::begin(
            &outbox,
            minter_shared,
            journal.clone(),
            ctx_base(),
            "R2",
            "agent.run",
            "2026-06-21T00:00:00Z",
            7,
        );
        c2.activity(RetryPolicy::default_policy(), |_i, _a| {
            Ok(vec![ArtifactRef("myelin://acme/agent/effect/e2".into())])
        })
        .expect("activity");
        c2.commit().expect("co-commit R2");

        let h1 = journal.history_for(&tenant(), "R1");
        assert_eq!(
            h1.len(),
            3,
            "R1 has exactly its three history rows (now+activity+rand)"
        );
        assert!(
            h1.iter().all(|r| r.run_id == "R1"),
            "no R2 row leaked into R1's history (AND-filter)"
        );
        assert_eq!(
            h1.iter().map(|r| r.seq).collect::<Vec<_>>(),
            vec![0, 1, 2],
            "the per-run history seq is monotonic 0,1,2 (the replay-order PK §3.2)"
        );
        let a1 = journal.attempts_for(&tenant(), "R1");
        assert_eq!(a1.len(), 1, "R1 has exactly one attempt row");
        assert!(
            a1.iter().all(|r| r.run_id == "R1"),
            "no R2 attempt leaked into R1's (AND-filter)"
        );
        assert_eq!(
            journal.history_for(&tenant(), "R2").len(),
            1,
            "R2 has exactly its one row"
        );
        assert_eq!(
            journal.attempts_for(&tenant(), "R2").len(),
            1,
            "R2 has exactly its one attempt"
        );
        assert!(
            journal
                .history_for(&TenantId("other".into()), "R1")
                .is_empty(),
            "a different tenant sees none of acme's rows (the tenant half of the AND-filter)"
        );
    }

    #[test]
    fn resume_short_circuits_a_journaled_activity_zero_re_execution() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let mut c1 = begin(&outbox, journal.clone());
        c1.activity(RetryPolicy::default_policy(), |_i, _a| {
            Ok(vec![ArtifactRef("myelin://acme/agent/effect/e1".into())])
        })
        .expect("activity");
        c1.commit().expect("co-commit");
        let history = journal.history_for(&tenant(), "R1");
        assert_eq!(history.len(), 1, "one journaled command");

        let ran = Arc::new(Mutex::new(false));
        let ran2 = ran.clone();
        let mut c2 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
            history,
        );
        let out = c2
            .activity(RetryPolicy::default_policy(), move |_i, _a| {
                *ran2.lock().unwrap() = true;
                Ok(vec![ArtifactRef(
                    "myelin://acme/agent/effect/SHOULD-NOT-APPEAR".into(),
                )])
            })
            .expect("the activity replays");
        assert!(
            !*ran.lock().unwrap(),
            "the closure was NOT re-executed (replay short-circuit)"
        );
        assert_eq!(
            out[0].0, "myelin://acme/agent/effect/e1",
            "the JOURNALED result is returned, not the re-run closure's"
        );
        assert_eq!(
            c2.side_effects_executed(),
            0,
            "0 side effects executed on a pure replay"
        );
        assert_eq!(
            c2.double_effects(),
            0,
            "0 double-effect (the FLOW-D1 floor)"
        );
    }

    #[test]
    fn resume_short_circuits_a_journaled_activity_failed() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let mut c1 = begin(&outbox, journal.clone());
        c1.activity(RetryPolicy { max_attempts: 1 }, |_i, _a| {
            Err(ActivityError::retryable("hard failure"))
        })
        .expect_err("the activity exhausts");
        c1.commit().expect("co-commit the failure");
        let history = journal.history_for(&tenant(), "R1");
        assert!(
            history
                .iter()
                .any(|r| r.kind == history_kind::ACTIVITY_FAILED),
            "an activity_failed row is journaled"
        );

        let ran = Arc::new(Mutex::new(false));
        let ran2 = ran.clone();
        let mut c2 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
            history,
        );
        let err = c2
            .activity(RetryPolicy { max_attempts: 5 }, move |_i, _a| {
                *ran2.lock().unwrap() = true;
                Ok(vec![ArtifactRef("myelin://acme/SHOULD-NOT-RUN".into())])
            })
            .expect_err("the journaled failure replays to a failure");
        assert!(
            matches!(err, WfError::ActivityExhausted(_)),
            "replays to ActivityExhausted"
        );
        assert!(
            !*ran.lock().unwrap(),
            "the closure was NOT re-executed (failed-replay short-circuit)"
        );
        assert_eq!(
            c2.side_effects_executed(),
            0,
            "0 side effects on a failed-replay short-circuit"
        );
        assert_eq!(
            c2.double_effects(),
            0,
            "0 double-effect on the failed-replay path"
        );
    }

    #[test]
    fn kind_mismatch_on_replay_halts_nondeterministic_not_re_execute() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let mut c1 = begin(&outbox, journal.clone());
        let _ = c1.now();
        c1.commit().expect("co-commit the marker");
        let history = journal.history_for(&tenant(), "R1");

        let ran = Arc::new(Mutex::new(false));
        let ran2 = ran.clone();
        let mut c2 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
            history,
        );
        let err = c2
            .activity(RetryPolicy::default_policy(), move |_i, _a| {
                *ran2.lock().unwrap() = true;
                Ok(vec![ArtifactRef(
                    "myelin://acme/agent/effect/SHOULD-NOT-RUN".into(),
                )])
            })
            .expect_err("the kind-mismatch halts as nondeterministic");
        assert!(
            matches!(err, WfError::Nondeterministic(_)),
            "the verdict is Nondeterministic, got {err:?}"
        );
        assert!(
            err.is_nondeterministic(),
            "is_nondeterministic predicate is true"
        );
        assert!(
            !*ran.lock().unwrap(),
            "the divergent activity did NOT run live (the guard halted it)"
        );
        assert_eq!(
            c2.side_effects_executed(),
            0,
            "0 side effects - the guard halted before live exec"
        );
        assert_eq!(
            c2.double_effects(),
            0,
            "0 double-effect - the divergence is a halt, not a re-execution"
        );
        assert!(
            c2.is_divergent(),
            "the divergence latch is set (the engine dead-letters the run)"
        );
        assert!(
            c2.divergence().unwrap().contains("agent.run:0"),
            "the divergence reason names the diverging position: {:?}",
            c2.divergence()
        );
    }

    #[test]
    fn is_nondeterministic_is_true_only_for_the_divergence_verdict() {
        assert!(
            WfError::Nondeterministic("diverged".into()).is_nondeterministic(),
            "the divergence verdict reads true"
        );
        assert!(
            !WfError::ActivityExhausted(ActivityError::retryable("x")).is_nondeterministic(),
            "an activity failure is not a replay divergence"
        );
        assert!(
            !WfError::CoCommit("y".into()).is_nondeterministic(),
            "a co-commit failure is NOT a divergence (the predicate is not a constant true)"
        );
    }

    #[test]
    fn reverse_kind_mismatch_now_at_activity_position_halts() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let mut c1 = begin(&outbox, journal.clone());
        c1.activity(RetryPolicy::default_policy(), |_i, _a| {
            Ok(vec![ArtifactRef("myelin://acme/agent/effect/e0".into())])
        })
        .expect("activity");
        c1.commit().expect("co-commit");
        let history = journal.history_for(&tenant(), "R1");

        let mut c2 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
            history,
        );
        let _ = c2.now();
        assert!(
            c2.is_divergent(),
            "a now() at an activity position latches the divergence"
        );
        assert!(
            c2.divergence().unwrap().contains("side-marker"),
            "the reason names the side-marker divergence: {:?}",
            c2.divergence()
        );
        let history2 = journal.history_for(&tenant(), "R1");
        let mut c3 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
            history2,
        );
        let _ = c3.rand();
        assert!(
            c3.is_divergent(),
            "a rand() at an activity position also latches the divergence"
        );
    }

    #[test]
    fn resume_now_and_rand_return_captured_values_not_recomputed() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let mut c1 = begin(&outbox, journal.clone());
        let t1 = c1.now();
        let r1 = c1.rand();
        c1.commit().expect("co-commit");
        let history = journal.history_for(&tenant(), "R1");

        let mut c2 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2099-01-01T00:00:00Z",
            999_999,
            history,
        );
        assert_eq!(
            c2.now(),
            t1,
            "now() replays its captured clock (not the resume-time clock)"
        );
        assert_eq!(
            c2.rand(),
            r1,
            "rand() replays its captured draw (not a re-seeded draw)"
        );
    }

    #[test]
    fn staged_emit_len_tracks_the_open_transaction_buffer() {
        let outbox = OutboxStore::new();
        let mut ctx = begin(&outbox, WfJournal::new());
        assert_eq!(ctx.staged_emit_len(), 0, "nothing emitted yet");
        ctx.emit(draft("a.b.c"), None).expect("emit 1");
        assert_eq!(ctx.staged_emit_len(), 1, "one emit staged");
        ctx.emit(draft("a.b.d"), None).expect("emit 2");
        assert_eq!(
            ctx.staged_emit_len(),
            2,
            "two emits staged (not a constant)"
        );
        ctx.commit().expect("co-commit");
        assert_eq!(
            outbox.outbox_depth(),
            2,
            "both emits durable after the co-commit"
        );
    }

    #[test]
    fn sleep_until_arms_a_durable_timer_journals_timer_set_and_parks() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let timers = crate::timer::TimerStore::new();
        let mut ctx = begin(&outbox, journal.clone()).with_timers(timers.clone(), 3, 1000);
        ctx.sleep_until(1600).expect("the sleep arms + journals");
        assert!(
            ctx.parked_on_timer(),
            "a future-deadline sleep parks the run (waiting, no runtime)"
        );
        let timer = timers
            .get(&tenant(), "R1/agent.run:0")
            .expect("the armed timer");
        assert_eq!(timer.fire_at, 1600, "the absolute deadline");
        assert_eq!(
            timer.bucket, 26,
            "the minute bucket = epoch_minute(1600) = 26"
        );
        assert_eq!(
            timer.partition, 3,
            "the timer rides the run's partition (co-located dispatch)"
        );
        assert!(!timer.fired, "armed-not-fired (the partial-index pivot)");
        assert_eq!(ctx.staged_history_len(), 1, "one timer_set marker staged");
        ctx.commit().expect("co-commit");
        let hist = journal.history_for(&tenant(), "R1");
        assert_eq!(hist.len(), 1, "the timer_set marker is journaled");
        assert_eq!(hist[0].kind, crate::timer::history_kind::TIMER_SET);
        assert_eq!(
            hist[0].command_id, "agent.run:0",
            "under the deterministic command position"
        );
    }

    #[test]
    fn a_past_deadline_sleep_does_not_park() {
        let outbox = OutboxStore::new();
        let timers = crate::timer::TimerStore::new();
        let mut ctx = begin(&outbox, WfJournal::new()).with_timers(timers.clone(), 0, 1000);
        ctx.sleep_until(500).expect("the sleep arms");
        assert!(
            !ctx.parked_on_timer(),
            "a past-deadline sleep is a no-wait continuation (no park)"
        );
        assert!(
            timers.get(&tenant(), "R1/agent.run:0").is_some(),
            "the immediately-due timer is armed"
        );
    }

    #[test]
    fn sleep_for_arms_a_relative_timer_and_parks() {
        let outbox = OutboxStore::new();
        let timers = crate::timer::TimerStore::new();
        let mut ctx = begin(&outbox, WfJournal::new()).with_timers(timers.clone(), 0, 1000);
        ctx.sleep_for(30 * 24 * 3600)
            .expect("the relative sleep arms");
        assert!(ctx.parked_on_timer(), "a 30-day sleep parks the run");
        let timer = timers
            .get(&tenant(), "R1/agent.run:0")
            .expect("the armed timer");
        assert_eq!(
            timer.fire_at,
            1000 + 30 * 24 * 3600,
            "the deadline is now + duration"
        );
        assert_eq!(
            timer.bucket,
            crate::timer::epoch_minute(1000 + 30 * 24 * 3600)
        );
    }

    #[test]
    fn sleep_for_refuses_a_deadline_outside_unix_time() {
        let outbox = OutboxStore::new();
        let timers = crate::timer::TimerStore::new();
        let mut ctx = begin(&outbox, WfJournal::new()).with_timers(timers.clone(), 0, i64::MAX);

        let error = ctx
            .sleep_for(1)
            .expect_err("an overflowing relative sleep must be refused");

        assert!(matches!(error, WfError::CoCommit(_)));
        assert_eq!(timers.armed_count(), 0);
        assert_eq!(ctx.staged_history_len(), 0);
    }

    #[test]
    fn sleep_with_no_timer_wheel_errors_loudly() {
        let outbox = OutboxStore::new();
        let mut ctx = begin(&outbox, WfJournal::new());
        let err = ctx
            .sleep_until(2000)
            .expect_err("a sleep with no wheel is a loud error");
        match err {
            WfError::CoCommit(msg) => assert!(
                msg.contains("timer wheel"),
                "the error names the missing wheel: {msg}"
            ),
            other => panic!("expected CoCommit naming the missing wheel, got {other:?}"),
        }
        assert!(
            !ctx.parked_on_timer(),
            "a failed sleep did not park (it errored)"
        );
    }

    #[test]
    fn resume_sleep_replays_without_re_arming() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let timers = crate::timer::TimerStore::new();
        let mut c1 = begin(&outbox, journal.clone()).with_timers(timers.clone(), 0, 1000);
        c1.sleep_until(1600).expect("arm");
        c1.commit().expect("co-commit the marker");
        assert_eq!(
            timers.armed_count(),
            1,
            "one timer armed on the first drive"
        );
        let history = journal.history_for(&tenant(), "R1");

        let mut c2 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
            history,
        )
        .with_timers(timers.clone(), 0, 1700);
        c2.sleep_until(1600).expect("the sleep replays");
        assert_eq!(
            timers.armed_count(),
            1,
            "no SECOND timer armed (the replay short-circuited)"
        );
        assert!(
            !c2.parked_on_timer(),
            "the resumed sleep does not re-park (the run already waited)"
        );
    }

    #[test]
    fn sleep_at_an_activity_position_halts_nondeterministic() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let timers = crate::timer::TimerStore::new();
        let mut c1 = begin(&outbox, journal.clone());
        c1.activity(RetryPolicy::default_policy(), |_i, _a| {
            Ok(vec![ArtifactRef("myelin://acme/agent/effect/e0".into())])
        })
        .expect("activity");
        c1.commit().expect("co-commit");
        let history = journal.history_for(&tenant(), "R1");

        let mut c2 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
            history,
        )
        .with_timers(timers.clone(), 0, 1000);
        let err = c2
            .sleep_until(2000)
            .expect_err("the sleep-at-activity-position diverges");
        assert!(
            err.is_nondeterministic(),
            "the verdict is Nondeterministic, got {err:?}"
        );
        assert!(
            c2.is_divergent(),
            "the divergence latch is set (the engine dead-letters the run)"
        );
        assert_eq!(
            timers.armed_count(),
            0,
            "no timer armed against the journaled activity position"
        );
    }

    use crate::engine::{SignalRow, SignalStore};

    fn buffer_signal(signals: &SignalStore, name: &str, idem: &str, payload: Vec<ArtifactRef>) {
        signals.deliver(SignalRow {
            tenant: tenant(),
            region: region(),
            run_id: "R1".into(),
            signal_name: name.into(),
            idem_key: idem.into(),
            payload,
            payload_key_ref: None,
            received_unix_ms: 0,
            consumed_seq: None,
        });
    }

    #[test]
    fn wait_on_absent_signal_parks_holding_no_runtime() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let mut ctx = begin(&outbox, journal.clone()).with_signals(signals.clone());
        let out = ctx.wait_for_signal("approval:call-1", None).expect("wait");
        assert_eq!(out, WaitOutcome::Parked, "an absent signal parks the run");
        assert!(
            ctx.parked_on_signal(),
            "the run is parked on the signal (state=waiting holds no runtime)"
        );
        assert!(
            ctx.parked(),
            "the unified park predicate sees the signal park"
        );
        assert_eq!(
            ctx.consumed_signals().len(),
            0,
            "nothing consumed - the signal has not arrived"
        );
        ctx.commit().expect("co-commit the park marker");
        let hist = journal.history_for(&tenant(), "R1");
        assert_eq!(hist.len(), 1, "one signal_waited marker journaled");
        assert_eq!(hist[0].kind, history_kind::SIGNAL_WAITED);
    }

    #[test]
    fn exact_wait_consumes_only_its_key_and_leaves_a_sibling_buffered() {
        let outbox = OutboxStore::new();
        let signals = SignalStore::new();
        buffer_signal(
            &signals,
            "job.done",
            "job-b",
            vec![ArtifactRef("myelin://acme/ci/result/b".into())],
        );
        buffer_signal(
            &signals,
            "job.done",
            "job-a",
            vec![ArtifactRef("myelin://acme/ci/result/a".into())],
        );
        let mut ctx = begin(&outbox, WfJournal::new()).with_signals(signals.clone());

        let outcome = ctx
            .wait_for_signal_exact("job.done", "job-b", None)
            .unwrap();
        assert!(matches!(
            outcome,
            WaitOutcome::Signalled { idem_key, .. } if idem_key == "job-b"
        ));
        assert_eq!(signals.buffered_depth(), 1);
        assert!(signals
            .unconsumed_for_exact(&tenant(), "R1", "job.done", "job-a")
            .is_some());
    }

    #[test]
    fn exact_wait_key_change_is_a_replay_divergence() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let mut first = begin(&outbox, journal.clone()).with_signals(signals.clone());
        assert_eq!(
            first
                .wait_for_signal_exact("job.done", "job-a", None)
                .unwrap(),
            WaitOutcome::Parked
        );
        first.commit().unwrap();

        let mut replay = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
            journal.history_for(&tenant(), "R1"),
        )
        .with_signals(signals);
        let error = replay
            .wait_for_signal_exact("job.done", "job-b", None)
            .unwrap_err();
        assert!(error.is_nondeterministic());
    }

    #[test]
    fn exact_wait_name_change_is_a_replay_divergence() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let mut first = begin(&outbox, journal.clone()).with_signals(signals.clone());
        assert_eq!(
            first
                .wait_for_signal_exact("job.done", "job-a", None)
                .unwrap(),
            WaitOutcome::Parked
        );
        first.commit().unwrap();

        let mut replay = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
            journal.history_for(&tenant(), "R1"),
        )
        .with_signals(signals);
        let error = replay
            .wait_for_signal_exact("ci.result", "job-a", None)
            .unwrap_err();
        assert!(error.is_nondeterministic());
    }

    #[test]
    fn consumed_exact_wait_binds_signal_name_on_replay() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        buffer_signal(
            &signals,
            "job.done",
            "job-a",
            vec![ArtifactRef("myelin://acme/ci/result/a".into())],
        );
        let mut first = begin(&outbox, journal.clone()).with_signals(signals.clone());
        assert!(matches!(
            first
                .wait_for_signal_exact("job.done", "job-a", None)
                .unwrap(),
            WaitOutcome::Signalled { .. }
        ));
        first.commit().unwrap();

        let mut replay = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
            journal.history_for(&tenant(), "R1"),
        );
        let error = replay
            .wait_for_signal_exact("ci.result", "job-a", None)
            .unwrap_err();
        assert!(error.is_nondeterministic());
    }

    #[test]
    fn exact_wait_accepts_legacy_rows_without_signal_name_binding() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        journal.append_history_for_test(WfHistoryRow {
            tenant: tenant(),
            region: region(),
            run_id: "R1".into(),
            seq: 0,
            kind: history_kind::SIGNAL_WAITED.into(),
            command_id: "agent.run:0".into(),
            result: Some(vec![ArtifactRef(format!(
                "{WAIT_EXPECTED_IDEM_PREFIX}job-a"
            ))]),
            result_key_ref: None,
        });
        let signals = SignalStore::new();
        let mut replay = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
            journal.history_for(&tenant(), "R1"),
        )
        .with_signals(signals);
        assert_eq!(
            replay
                .wait_for_signal_exact("job.done", "job-a", None)
                .unwrap(),
            WaitOutcome::Parked
        );
    }

    #[test]
    fn exact_receipt_accepts_legacy_rows_without_signal_name_binding() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        journal.append_history_for_test(WfHistoryRow {
            tenant: tenant(),
            region: region(),
            run_id: "R1".into(),
            seq: 0,
            kind: history_kind::SIGNAL_RECEIVED.into(),
            command_id: "agent.run:0".into(),
            result: Some(vec![
                ArtifactRef(format!("{WAIT_IDEM_PREFIX}job-a")),
                ArtifactRef("myelin://acme/ci/result/a".into()),
            ]),
            result_key_ref: None,
        });
        let mut replay = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
            journal.history_for(&tenant(), "R1"),
        );
        assert!(matches!(
            replay
                .wait_for_signal_exact("job.done", "job-a", None)
                .unwrap(),
            WaitOutcome::Signalled { idem_key, .. } if idem_key == "job-a"
        ));
    }

    #[test]
    fn replay_decoder_accepts_legacy_signal_receipt_markers() {
        let outcome = decode_received(&Some(vec![
            ArtifactRef("wait:idem:job-a".into()),
            ArtifactRef("wait:keyref:kms://acme/key-1".into()),
            ArtifactRef("myelin://acme/ci/result/a".into()),
        ]));
        assert_eq!(
            outcome,
            WaitOutcome::Signalled {
                idem_key: "job-a".into(),
                payload: vec![ArtifactRef("myelin://acme/ci/result/a".into())],
                payload_key_ref: Some("kms://acme/key-1".into()),
            }
        );
    }

    #[test]
    fn receipt_at_deadline_is_accepted_even_when_the_wheel_runs_late() {
        let outbox = OutboxStore::new();
        let signals = SignalStore::new();
        signals.deliver(SignalRow {
            tenant: tenant(),
            region: region(),
            run_id: "R1".into(),
            signal_name: "job.done".into(),
            idem_key: "job-a".into(),
            payload: vec![ArtifactRef("myelin://acme/ci/result/a".into())],
            payload_key_ref: None,
            received_unix_ms: 110_000,
            consumed_seq: None,
        });
        let mut ctx = begin(&outbox, WfJournal::new())
            .with_signals(signals.clone())
            .with_timers(crate::timer::TimerStore::new(), 0, 120);
        assert!(matches!(
            ctx.wait_for_signal_exact_until("job.done", "job-a", Some(110))
                .unwrap(),
            WaitOutcome::Signalled { .. }
        ));
        assert_eq!(signals.buffered_depth(), 0);
    }

    #[test]
    fn receipt_after_deadline_times_out_even_when_timer_processing_lags() {
        let outbox = OutboxStore::new();
        let signals = SignalStore::new();
        signals.deliver(SignalRow {
            tenant: tenant(),
            region: region(),
            run_id: "R1".into(),
            signal_name: "job.done".into(),
            idem_key: "job-a".into(),
            payload: vec![ArtifactRef("myelin://acme/ci/result/a".into())],
            payload_key_ref: None,
            received_unix_ms: 110_001,
            consumed_seq: None,
        });
        let mut ctx = begin(&outbox, WfJournal::new())
            .with_signals(signals.clone())
            .with_timers(crate::timer::TimerStore::new(), 0, 120);
        assert_eq!(
            ctx.wait_for_signal_exact_until("job.done", "job-a", Some(110))
                .unwrap(),
            WaitOutcome::TimedOut
        );
        assert_eq!(
            signals.buffered_depth(),
            1,
            "the losing late result remains unconsumed for audit/cleanup"
        );
    }

    #[test]
    fn buffered_signal_resumes_and_consumes_exactly_once() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        buffer_signal(
            &signals,
            "approval:call-1",
            "card-7",
            vec![ArtifactRef("myelin://acme/agent/decision/approve".into())],
        );
        assert_eq!(signals.buffered_depth(), 1, "the approval is buffered");

        let mut ctx = begin(&outbox, journal.clone()).with_signals(signals.clone());
        let out = ctx.wait_for_signal("approval:call-1", None).expect("wait");
        match out {
            WaitOutcome::Signalled {
                idem_key, payload, ..
            } => {
                assert_eq!(idem_key, "card-7", "the consumed signal's per-effect key");
                assert_eq!(
                    payload,
                    vec![ArtifactRef("myelin://acme/agent/decision/approve".into())],
                    "the references-not-payloads decision body"
                );
            }
            other => panic!("expected Signalled, got {other:?}"),
        }
        assert!(!ctx.parked_on_signal(), "a consumed wait does NOT park");
        assert_eq!(
            ctx.consumed_signals(),
            &[("approval:call-1".to_string(), "card-7".to_string())],
            "exactly ONE signal consumed (FLOW-D4: 1 consume)"
        );
        assert_eq!(
            signals.buffered_depth(),
            0,
            "the consumed signal no longer counts (1 consume)"
        );
        ctx.commit().expect("co-commit the signal_received marker");
        let hist = journal.history_for(&tenant(), "R1");
        assert_eq!(
            hist[0].kind,
            history_kind::SIGNAL_RECEIVED,
            "the consume is journaled"
        );
    }

    #[test]
    fn park_then_days_later_signal_resumes_and_consumes_once() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();

        let mut c1 = begin(&outbox, journal.clone()).with_signals(signals.clone());
        assert_eq!(
            c1.wait_for_signal("approval:call-1", None).unwrap(),
            WaitOutcome::Parked
        );
        assert!(c1.parked_on_signal());
        c1.commit().expect("co-commit the park");
        let history = journal.history_for(&tenant(), "R1");

        buffer_signal(
            &signals,
            "approval:call-1",
            "card-7",
            vec![ArtifactRef("myelin://acme/agent/decision/approve".into())],
        );

        let mut c2 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
            history,
        )
        .with_signals(signals.clone());
        let out = c2
            .wait_for_signal("approval:call-1", None)
            .expect("the days-later resume");
        assert!(
            matches!(out, WaitOutcome::Signalled { .. }),
            "the days-later signal resumes, got {out:?}"
        );
        assert_eq!(
            c2.consumed_signals().len(),
            1,
            "exactly ONE consume across the restart (FLOW-D4)"
        );
        assert!(!c2.parked_on_signal(), "the resumed run no longer parks");
        c2.commit().expect("co-commit the consume");
        assert_eq!(
            signals.buffered_depth(),
            0,
            "the signal is consumed once (buffered depth 0)"
        );
    }

    #[test]
    fn replay_returns_the_journaled_signal_without_reconsuming() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        buffer_signal(
            &signals,
            "approval:call-1",
            "card-7",
            vec![ArtifactRef("myelin://acme/agent/decision/approve".into())],
        );
        let mut c1 = begin(&outbox, journal.clone()).with_signals(signals.clone());
        c1.wait_for_signal("approval:call-1", None)
            .expect("consume");
        c1.commit().expect("co-commit");
        let history = journal.history_for(&tenant(), "R1");

        buffer_signal(
            &signals,
            "approval:call-1",
            "card-99",
            vec![ArtifactRef("myelin://acme/agent/decision/other".into())],
        );
        let depth_before = signals.buffered_depth();

        let mut c2 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
            history,
        )
        .with_signals(signals.clone());
        let out = c2
            .wait_for_signal("approval:call-1", None)
            .expect("replay the consume");
        match out {
            WaitOutcome::Signalled { idem_key, .. } => assert_eq!(
                idem_key, "card-7",
                "replay returns the SAME journaled signal (card-7), never re-scans to card-99"
            ),
            other => panic!("expected the journaled Signalled, got {other:?}"),
        }
        assert_eq!(
            c2.consumed_signals().len(),
            0,
            "replay consumed NOTHING new (the journal is the truth)"
        );
        assert_eq!(
            signals.buffered_depth(),
            depth_before,
            "the second signal (card-99) was NOT consumed on replay"
        );
    }

    #[test]
    fn wait_times_out_when_deadline_passes_without_a_signal() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = crate::timer::TimerStore::new();

        let mut c1 = begin(&outbox, journal.clone())
            .with_signals(signals.clone())
            .with_timers(timers.clone(), 0, 1000);
        assert_eq!(
            c1.wait_for_signal("approval:call-1", Some(100)).unwrap(),
            WaitOutcome::Parked
        );
        c1.commit().expect("co-commit the park + the timeout-timer");
        assert_eq!(timers.armed_count(), 1, "a durable timeout-timer was armed");
        let history = journal.history_for(&tenant(), "R1");

        let mut c2 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
            history,
        )
        .with_signals(signals.clone())
        .with_timers(timers.clone(), 0, 2000);
        let out = c2
            .wait_for_signal("approval:call-1", Some(100))
            .expect("the timeout drive");
        assert_eq!(
            out,
            WaitOutcome::TimedOut,
            "the deadline passed without a signal → TimedOut (auto-deny)"
        );
        assert_eq!(
            c2.consumed_signals().len(),
            0,
            "a timeout consumes no signal (0 mutation, AG-8)"
        );
    }

    #[test]
    fn wait_without_a_signal_store_is_a_loud_error() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let mut ctx = begin(&outbox, journal);
        let err = ctx
            .wait_for_signal("approval:call-1", None)
            .expect_err("a wait with no store errors");
        assert!(
            matches!(err, WfError::CoCommit(ref m) if m.contains("signal store")),
            "the missing-store wait is a loud CoCommit error, got {err:?}"
        );
    }

    #[test]
    fn a_timed_wait_without_a_timer_refuses_instead_of_parking_forever() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let mut ctx = begin(&outbox, journal.clone()).with_signals(SignalStore::new());

        let error = ctx
            .wait_for_signal("approval:call-1", Some(60))
            .expect_err("a timed wait needs a durable way to wake itself");

        assert!(matches!(error, WfError::CoCommit(_)));
        assert!(journal.history_for(&tenant(), "R1").is_empty());
        assert_eq!(ctx.park_condition(), None);
    }

    #[test]
    fn an_unrepresentable_signal_deadline_is_refused_before_it_is_journaled() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let timers = crate::timer::TimerStore::new();
        let mut ctx = begin(&outbox, journal.clone())
            .with_signals(SignalStore::new())
            .with_timers(timers.clone(), 0, i64::MAX);

        let error = ctx
            .wait_for_signal("approval:call-1", Some(1))
            .expect_err("a deadline beyond Unix time must be refused");

        assert!(matches!(error, WfError::CoCommit(_)));
        assert!(journal.history_for(&tenant(), "R1").is_empty());
        assert_eq!(timers.armed_count(), 0);
    }

    #[test]
    fn a_timed_wait_cannot_replay_as_an_untimed_wait() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let mut first = begin(&outbox, journal.clone())
            .with_signals(signals.clone())
            .with_timers(crate::timer::TimerStore::new(), 0, 1_000);
        assert_eq!(
            first.wait_for_signal("approval:call-1", Some(60)).unwrap(),
            WaitOutcome::Parked
        );
        first.commit().unwrap();

        let mut replay = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
            journal.history_for(&tenant(), "R1"),
        )
        .with_signals(signals);

        let error = replay
            .wait_for_signal("approval:call-1", None)
            .expect_err("changing a durable wait's timing is nondeterministic");
        assert!(error.is_nondeterministic());
    }

    #[test]
    fn an_untimed_wait_cannot_gain_a_timeout_during_replay() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let mut first = begin(&outbox, journal.clone()).with_signals(signals.clone());
        assert_eq!(
            first.wait_for_signal("approval:call-1", None).unwrap(),
            WaitOutcome::Parked
        );
        first.commit().unwrap();

        let mut replay = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
            journal.history_for(&tenant(), "R1"),
        )
        .with_signals(signals)
        .with_timers(crate::timer::TimerStore::new(), 0, 1_000);

        let error = replay
            .wait_for_signal("approval:call-1", Some(60))
            .expect_err("adding a timeout during replay is nondeterministic");
        assert!(error.is_nondeterministic());
    }

    #[test]
    fn wait_at_an_activity_position_halts_nondeterministic() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let mut c1 = begin(&outbox, journal.clone());
        c1.activity(RetryPolicy::default_policy(), |_i, _a| {
            Ok(vec![ArtifactRef("myelin://acme/agent/effect/e0".into())])
        })
        .expect("activity");
        c1.commit().expect("co-commit");
        let history = journal.history_for(&tenant(), "R1");

        let mut c2 = WfCtx::resume(
            &outbox,
            minter(),
            journal.clone(),
            ctx_base(),
            "R1",
            "agent.run",
            "2026-06-21T00:00:00Z",
            42,
            history,
        )
        .with_signals(signals.clone());
        let err = c2
            .wait_for_signal("approval:call-1", None)
            .expect_err("wait-at-activity diverges");
        assert!(
            err.is_nondeterministic(),
            "the verdict is Nondeterministic, got {err:?}"
        );
        assert!(c2.is_divergent(), "the divergence latch is set");
    }
}

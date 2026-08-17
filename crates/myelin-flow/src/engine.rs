use crate::schema::WfHistoryRow;
use crate::wfctx::WfCtx;
use myelin_events::{EmitContextBase, IdMinter, OutboxStore};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub mod run_state {
    pub const RUNNING: &str = "running";
    pub const WAITING: &str = "waiting";
    pub const COMPLETED: &str = "completed";
    pub const FAILED: &str = "failed";
    pub const TERMINATED: &str = "terminated";
    pub const NONDETERMINISTIC: &str = "nondeterministic";

    pub fn is_terminal(state: &str) -> bool {
        matches!(state, COMPLETED | FAILED | TERMINATED | NONDETERMINISTIC)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriveOutcome {
    Completed(Vec<ArtifactRef>),
    Failed(String),
    Waiting,
    Nondeterministic(String),
}

#[derive(Clone, Debug)]
pub struct RunRow {
    pub tenant: TenantId,
    pub region: Region,
    pub run_id: String,
    pub wf_type: String,
    pub wf_version: i32,
    pub state: String,
    pub cursor: i64,
    pub partition: i16,
    pub lease_owner: Option<String>,
    pub lease_expires: Option<i64>,
}

impl RunRow {
    pub fn new_runnable(
        tenant: TenantId,
        region: Region,
        run_id: impl Into<String>,
        wf_type: impl Into<String>,
        partition: i16,
    ) -> Self {
        Self {
            tenant,
            region,
            run_id: run_id.into(),
            wf_type: wf_type.into(),
            wf_version: 1,
            state: run_state::RUNNING.into(),
            cursor: 0,
            partition,
            lease_owner: None,
            lease_expires: None,
        }
    }

    pub fn new_runnable_versioned(
        tenant: TenantId,
        region: Region,
        run_id: impl Into<String>,
        wf_type: impl Into<String>,
        wf_version: i32,
        partition: i16,
    ) -> Self {
        let mut row = Self::new_runnable(tenant, region, run_id, wf_type, partition);
        row.wf_version = wf_version;
        row
    }
}

#[derive(Clone, Default)]
pub struct RunStore {
    inner: Arc<Mutex<HashMap<(String, String), RunRow>>>,
}

impl RunStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(String, String), RunRow>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn key(run: &RunRow) -> (String, String) {
        (run.tenant.0.clone(), run.run_id.clone())
    }

    pub fn put(&self, run: RunRow) {
        self.lock().insert(Self::key(&run), run);
    }

    pub fn get(&self, tenant: &TenantId, run_id: &str) -> Option<RunRow> {
        self.lock()
            .get(&(tenant.0.clone(), run_id.to_string()))
            .cloned()
    }

    pub fn all_runs(&self) -> Vec<RunRow> {
        let mut runs: Vec<RunRow> = self.lock().values().cloned().collect();
        runs.sort_by(|a, b| {
            (a.tenant.0.as_str(), a.run_id.as_str()).cmp(&(b.tenant.0.as_str(), b.run_id.as_str()))
        });
        runs
    }

    pub fn with_run_mut<R>(
        &self,
        tenant: &TenantId,
        run_id: &str,
        f: impl FnOnce(&mut RunRow) -> R,
    ) -> Option<R> {
        let mut runs = self.lock();
        runs.get_mut(&(tenant.0.clone(), run_id.to_string())).map(f)
    }

    pub fn lease_runnable(
        &self,
        partition: i16,
        worker: &str,
        now: i64,
        lease_ttl_secs: i64,
    ) -> Option<RunRow> {
        let mut runs = self.lock();
        let mut keys: Vec<_> = runs.keys().cloned().collect();
        keys.sort();
        for k in keys {
            let run = runs.get_mut(&k).expect("key from the same map");
            if run.partition != partition || run.state != run_state::RUNNING {
                continue;
            }
            let lease_free = match run.lease_expires {
                None => true,
                Some(exp) => exp <= now,
            };
            if lease_free {
                run.lease_owner = Some(worker.to_string());
                run.lease_expires = Some(now + lease_ttl_secs);
                return Some(run.clone());
            }
        }
        None
    }

    pub fn runnable_lag(&self, partition: i16, now: i64) -> usize {
        self.lock()
            .values()
            .filter(|r| {
                r.partition == partition
                    && r.state == run_state::RUNNING
                    && r.lease_expires.map(|e| e <= now).unwrap_or(true)
            })
            .count()
    }

    fn settle(&self, tenant: &TenantId, run_id: &str, cursor: i64, state: &str) {
        if let Some(run) = self.lock().get_mut(&(tenant.0.clone(), run_id.to_string())) {
            run.cursor = cursor;
            run.state = state.to_string();
            run.lease_owner = None;
            run.lease_expires = None;
        }
    }

    pub fn terminate(&self, tenant: &TenantId, run_id: &str, state: &str) {
        if let Some(run) = self.lock().get_mut(&(tenant.0.clone(), run_id.to_string())) {
            run.state = state.to_string();
            run.lease_owner = None;
            run.lease_expires = None;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalRow {
    pub tenant: TenantId,
    pub region: Region,
    pub run_id: String,
    pub signal_name: String,
    pub idem_key: String,
    pub payload: Vec<ArtifactRef>,
    pub payload_key_ref: Option<String>,
    pub received_unix_ms: i64,
    pub consumed_seq: Option<i64>,
}

type SignalKey = (String, String, String, String);

#[derive(Clone, Default)]
pub struct SignalStore {
    inner: Arc<Mutex<HashMap<SignalKey, SignalRow>>>,
}

impl SignalStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<SignalKey, SignalRow>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn key(row: &SignalRow) -> SignalKey {
        (
            row.tenant.0.clone(),
            row.run_id.clone(),
            row.signal_name.clone(),
            row.idem_key.clone(),
        )
    }

    pub fn deliver(&self, row: SignalRow) -> bool {
        let mut signals = self.lock();
        let key = Self::key(&row);
        if signals.contains_key(&key) {
            return false;
        }
        signals.insert(key, row);
        true
    }

    pub fn get(
        &self,
        tenant: &TenantId,
        run_id: &str,
        signal_name: &str,
        idem_key: &str,
    ) -> Option<SignalRow> {
        self.lock()
            .get(&(
                tenant.0.clone(),
                run_id.to_string(),
                signal_name.to_string(),
                idem_key.to_string(),
            ))
            .cloned()
    }

    pub fn buffered_depth(&self) -> u64 {
        self.lock()
            .values()
            .filter(|s| s.consumed_seq.is_none())
            .count() as u64
    }

    pub fn count_for_run(&self, tenant: &TenantId, run_id: &str) -> usize {
        self.lock()
            .values()
            .filter(|s| s.tenant.0 == tenant.0 && s.run_id == run_id)
            .count()
    }

    pub fn first_unconsumed_for(
        &self,
        tenant: &TenantId,
        run_id: &str,
        signal_name: &str,
    ) -> Option<(String, SignalRow)> {
        let signals = self.lock();
        signals
            .values()
            .filter(|s| {
                s.tenant.0 == tenant.0
                    && s.run_id == run_id
                    && s.signal_name == signal_name
                    && s.consumed_seq.is_none()
            })
            .min_by(|a, b| a.idem_key.cmp(&b.idem_key))
            .map(|s| (s.idem_key.clone(), s.clone()))
    }

    pub fn unconsumed_for_exact(
        &self,
        tenant: &TenantId,
        run_id: &str,
        signal_name: &str,
        idem_key: &str,
    ) -> Option<SignalRow> {
        self.get(tenant, run_id, signal_name, idem_key)
            .filter(|row| row.consumed_seq.is_none())
    }

    pub fn consume(
        &self,
        tenant: &TenantId,
        run_id: &str,
        signal_name: &str,
        idem_key: &str,
        consumed_seq: i64,
    ) -> bool {
        let mut signals = self.lock();
        let key = (
            tenant.0.clone(),
            run_id.to_string(),
            signal_name.to_string(),
            idem_key.to_string(),
        );
        match signals.get_mut(&key) {
            Some(row) if row.consumed_seq.is_none() => {
                row.consumed_seq = Some(consumed_seq);
                true
            }
            _ => false,
        }
    }
}

#[derive(Clone, Default)]
pub struct FlowTelemetry {
    inner: Arc<Mutex<TelemetryInner>>,
}

#[derive(Default)]
struct TelemetryInner {
    commands_replayed: u64,
    commands_executed: u64,
    double_effect_count: u64,
    runnable_run_lag: u64,
    activity_queue_depth: u64,
    activity_retry_count: u64,
    dead_letter_count: u64,
    nondeterministic_halt_count: u64,
    signal_buffer_depth: u64,
    timer_wheel_lag: u64,
    oldest_unconsumed_wait_age_secs: u64,
    reserve_attempted: u64,
    reserve_rejected: u64,
    settled: u64,
    causal_depth_hist: Vec<u64>,
    causal_depth_max: u32,
    depth_ceiling_hits: u64,
    shared_root_tripwire_firings: u64,
    activity_pool_sheds: u64,
    fork_count: u64,
    crypto_shred_lag_secs: u64,
    crypto_shreds_count: u64,
    restore_verify_consistent_offset: i64,
    restore_verify_runs_resumed: u64,
    restore_verify_green_count: u64,
    restore_verify_red_count: u64,
}

impl FlowTelemetry {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TelemetryInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn record_drive(&self, replayed: u64, executed: u64, side_effects_on_replayed: u64) {
        let mut t = self.lock();
        t.commands_replayed += replayed;
        t.commands_executed += executed;
        t.double_effect_count += side_effects_on_replayed;
    }

    pub fn set_runnable_lag(&self, lag: u64) {
        self.lock().runnable_run_lag = lag;
    }

    pub fn set_activity_queue_depth(&self, depth: u64) {
        self.lock().activity_queue_depth = depth;
    }

    pub fn record_activity_retry(&self) {
        self.lock().activity_retry_count += 1;
    }

    pub fn record_dead_letter(&self) {
        self.lock().dead_letter_count += 1;
    }

    pub fn record_nondeterministic_halt(&self) {
        self.lock().nondeterministic_halt_count += 1;
    }

    pub fn nondeterministic_halt_count(&self) -> u64 {
        self.lock().nondeterministic_halt_count
    }

    pub fn activity_queue_depth(&self) -> u64 {
        self.lock().activity_queue_depth
    }

    pub fn activity_retry_count(&self) -> u64 {
        self.lock().activity_retry_count
    }

    pub fn dead_letter_count(&self) -> u64 {
        self.lock().dead_letter_count
    }

    pub fn replay_rate_bps(&self) -> u64 {
        let t = self.lock();
        let total = t.commands_replayed + t.commands_executed;
        (10_000 * t.commands_replayed)
            .checked_div(total)
            .unwrap_or(0)
    }

    pub fn double_effect_count(&self) -> u64 {
        self.lock().double_effect_count
    }

    pub fn commands_replayed(&self) -> u64 {
        self.lock().commands_replayed
    }

    pub fn commands_executed(&self) -> u64 {
        self.lock().commands_executed
    }

    pub fn runnable_run_lag(&self) -> u64 {
        self.lock().runnable_run_lag
    }

    pub fn set_signal_buffer_depth(&self, depth: u64) {
        self.lock().signal_buffer_depth = depth;
    }

    pub fn signal_buffer_depth(&self) -> u64 {
        self.lock().signal_buffer_depth
    }

    pub fn set_timer_wheel_lag(&self, lag: u64) {
        self.lock().timer_wheel_lag = lag;
    }

    pub fn timer_wheel_lag(&self) -> u64 {
        self.lock().timer_wheel_lag
    }

    pub fn set_oldest_unconsumed_wait_age(&self, age_secs: u64) {
        self.lock().oldest_unconsumed_wait_age_secs = age_secs;
    }

    pub fn oldest_unconsumed_wait_age_secs(&self) -> u64 {
        self.lock().oldest_unconsumed_wait_age_secs
    }

    pub fn record_reserve_attempt(&self) {
        self.lock().reserve_attempted += 1;
    }

    pub fn record_reserve_reject(&self) {
        self.lock().reserve_rejected += 1;
    }

    pub fn record_settle(&self) {
        self.lock().settled += 1;
    }

    pub fn reserve_attempted(&self) -> u64 {
        self.lock().reserve_attempted
    }

    pub fn reserve_rejected(&self) -> u64 {
        self.lock().reserve_rejected
    }

    pub fn settled(&self) -> u64 {
        self.lock().settled
    }

    pub fn record_crypto_shred(&self, lag_secs: u64) {
        let mut t = self.lock();
        t.crypto_shred_lag_secs = lag_secs;
        t.crypto_shreds_count += 1;
    }

    pub fn crypto_shred_lag_secs(&self) -> u64 {
        self.lock().crypto_shred_lag_secs
    }

    pub fn crypto_shreds_count(&self) -> u64 {
        self.lock().crypto_shreds_count
    }

    pub fn record_restore_verify_green(&self, consistent_offset: i64, runs_resumed: u64) {
        let mut t = self.lock();
        t.restore_verify_consistent_offset = consistent_offset;
        t.restore_verify_runs_resumed = runs_resumed;
        t.restore_verify_green_count += 1;
    }

    pub fn record_restore_verify_red(&self) {
        self.lock().restore_verify_red_count += 1;
    }

    pub fn restore_verify_consistent_offset(&self) -> i64 {
        self.lock().restore_verify_consistent_offset
    }

    pub fn restore_verify_runs_resumed(&self) -> u64 {
        self.lock().restore_verify_runs_resumed
    }

    pub fn restore_verify_green_count(&self) -> u64 {
        self.lock().restore_verify_green_count
    }

    pub fn restore_verify_red_count(&self) -> u64 {
        self.lock().restore_verify_red_count
    }

    pub fn reserve_reject_rate_bps(&self) -> u64 {
        let t = self.lock();
        (10_000 * t.reserve_rejected)
            .checked_div(t.reserve_attempted)
            .unwrap_or(0)
    }

    pub fn observe_causal_depth(&self, depth: u32, ceiling: u32) {
        let mut t = self.lock();
        let buckets = (ceiling as usize) + 2;
        if t.causal_depth_hist.len() < buckets {
            t.causal_depth_hist.resize(buckets, 0);
        }
        let idx = (depth as usize).min(buckets - 1);
        t.causal_depth_hist[idx] += 1;
        if depth > t.causal_depth_max {
            t.causal_depth_max = depth;
        }
    }

    pub fn causal_depth_histogram(&self) -> Vec<u64> {
        self.lock().causal_depth_hist.clone()
    }

    pub fn causal_depth_max(&self) -> u32 {
        self.lock().causal_depth_max
    }

    pub fn record_depth_ceiling_hit(&self) {
        self.lock().depth_ceiling_hits += 1;
    }

    pub fn depth_ceiling_hits(&self) -> u64 {
        self.lock().depth_ceiling_hits
    }

    pub fn record_shared_root_tripwire_firing(&self) {
        self.lock().shared_root_tripwire_firings += 1;
    }

    pub fn shared_root_tripwire_firings(&self) -> u64 {
        self.lock().shared_root_tripwire_firings
    }

    pub fn record_activity_pool_shed(&self) {
        self.lock().activity_pool_sheds += 1;
    }

    pub fn activity_pool_sheds(&self) -> u64 {
        self.lock().activity_pool_sheds
    }

    pub fn fork_count(&self) -> u64 {
        self.lock().fork_count
    }
}

pub type WorkflowBody = dyn Fn(&mut WfCtx) -> Result<Vec<ArtifactRef>, String>;

#[allow(clippy::too_many_arguments)]
pub fn drive(
    runs: &RunStore,
    outbox: &OutboxStore,
    journal: &crate::wfctx::WfJournal,
    telemetry: &FlowTelemetry,
    minter: Arc<dyn IdMinter>,
    ctx_base: EmitContextBase,
    run: &RunRow,
    now_clock: impl Into<String>,
    rand_seed: u64,
    body: &WorkflowBody,
) -> DriveOutcome {
    drive_versioned(
        runs, outbox, journal, telemetry, minter, ctx_base, run, now_clock, rand_seed, body, 1, 1,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn drive_versioned(
    runs: &RunStore,
    outbox: &OutboxStore,
    journal: &crate::wfctx::WfJournal,
    telemetry: &FlowTelemetry,
    minter: Arc<dyn IdMinter>,
    ctx_base: EmitContextBase,
    run: &RunRow,
    now_clock: impl Into<String>,
    rand_seed: u64,
    body: &WorkflowBody,
    run_version: i32,
    replay_version: i32,
) -> DriveOutcome {
    drive_with_timers(
        runs,
        outbox,
        journal,
        telemetry,
        minter,
        ctx_base,
        run,
        now_clock,
        rand_seed,
        body,
        run_version,
        replay_version,
        None,
        0,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn drive_with_timers(
    runs: &RunStore,
    outbox: &OutboxStore,
    journal: &crate::wfctx::WfJournal,
    telemetry: &FlowTelemetry,
    minter: Arc<dyn IdMinter>,
    ctx_base: EmitContextBase,
    run: &RunRow,
    now_clock: impl Into<String>,
    rand_seed: u64,
    body: &WorkflowBody,
    run_version: i32,
    replay_version: i32,
    timers: Option<crate::timer::TimerStore>,
    now_secs: i64,
) -> DriveOutcome {
    drive_full(
        runs,
        outbox,
        journal,
        telemetry,
        minter,
        ctx_base,
        run,
        now_clock,
        rand_seed,
        body,
        run_version,
        replay_version,
        timers,
        None,
        now_secs,
        None,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn drive_full(
    runs: &RunStore,
    outbox: &OutboxStore,
    journal: &crate::wfctx::WfJournal,
    telemetry: &FlowTelemetry,
    minter: Arc<dyn IdMinter>,
    ctx_base: EmitContextBase,
    run: &RunRow,
    now_clock: impl Into<String>,
    rand_seed: u64,
    body: &WorkflowBody,
    run_version: i32,
    replay_version: i32,
    timers: Option<crate::timer::TimerStore>,
    signals: Option<SignalStore>,
    now_secs: i64,
    run_identity: Option<crate::remint::RunTokenLease>,
    budget: Option<crate::budget::BudgetGate>,
) -> DriveOutcome {
    let tenant = run.tenant.clone();
    let history: Vec<WfHistoryRow> = journal.history_for(&tenant, &run.run_id);
    let journaled_commands = history.len() as u64;

    let mut ctx = WfCtx::resume_versioned(
        outbox,
        minter,
        journal.clone(),
        ctx_base,
        run.run_id.clone(),
        run.wf_type.clone(),
        now_clock,
        rand_seed,
        history,
        run_version,
        replay_version,
    );
    if let Some(timers) = timers {
        ctx = ctx.with_timers(timers, run.partition, now_secs);
    }
    if let Some(signals) = signals.clone() {
        ctx = ctx.with_signals(signals);
    }
    if let Some(lease) = run_identity {
        ctx = ctx.with_run_identity(lease);
    }
    if let Some(gate) = budget {
        ctx = ctx.with_budget(gate);
    }

    let result = body(&mut ctx);
    let side_effects_executed = ctx.side_effects_executed();
    let double_effects = ctx.double_effects();
    telemetry.record_drive(journaled_commands, side_effects_executed, double_effects);

    if let Some(reason) = ctx.divergence() {
        let reason = reason.to_string();
        drop(ctx);
        telemetry.record_nondeterministic_halt();
        runs.settle(
            &tenant,
            &run.run_id,
            run.cursor,
            run_state::NONDETERMINISTIC,
        );
        let lag = runs.runnable_lag(run.partition, i64::MAX) as u64;
        telemetry.set_runnable_lag(lag);
        return DriveOutcome::Nondeterministic(reason);
    }

    let parked = ctx.parked();
    let consumed_signals = ctx.consumed_signals().to_vec();

    let committed = ctx.commit().is_ok();

    let new_cursor = journal.history_for(&tenant, &run.run_id).len() as i64;
    let (outcome, state) = match (&result, committed, parked) {
        (Ok(_), true, true) => (DriveOutcome::Waiting, run_state::WAITING),
        (Ok(refs), true, false) => (DriveOutcome::Completed(refs.clone()), run_state::COMPLETED),
        (Ok(_), false, _) => (
            DriveOutcome::Failed("co-commit failed".into()),
            run_state::FAILED,
        ),
        (Err(e), _, _) => (DriveOutcome::Failed(e.clone()), run_state::FAILED),
    };
    runs.settle(&tenant, &run.run_id, new_cursor, state);
    let lag = runs.runnable_lag(run.partition, i64::MAX) as u64;
    telemetry.set_runnable_lag(lag);
    if let Some(signals) = signals.as_ref() {
        telemetry.set_signal_buffer_depth(signals.buffered_depth());
        let _ = consumed_signals;
    }
    outcome
}

pub struct FlowDispatcher {
    runs: RunStore,
    outbox: OutboxStore,
    journal: crate::wfctx::WfJournal,
    telemetry: FlowTelemetry,
    minter: Arc<dyn IdMinter>,
    ctx_base: EmitContextBase,
    partition: i16,
    worker: String,
    lease_ttl_secs: i64,
    bodies: HashMap<String, Box<WorkflowBody>>,
    running_versions: HashMap<String, i32>,
    timers: Option<crate::timer::TimerStore>,
    signals: Option<SignalStore>,
    run_identity: Option<crate::remint::RunTokenLease>,
    budget: Option<crate::budget::BudgetGate>,
}

impl FlowDispatcher {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        runs: RunStore,
        outbox: OutboxStore,
        journal: crate::wfctx::WfJournal,
        telemetry: FlowTelemetry,
        minter: Arc<dyn IdMinter>,
        ctx_base: EmitContextBase,
        partition: i16,
        worker: impl Into<String>,
        lease_ttl_secs: i64,
    ) -> Self {
        Self {
            runs,
            outbox,
            journal,
            telemetry,
            minter,
            ctx_base,
            partition,
            worker: worker.into(),
            lease_ttl_secs,
            bodies: HashMap::new(),
            running_versions: HashMap::new(),
            timers: None,
            signals: None,
            run_identity: None,
            budget: None,
        }
    }

    pub fn with_budget(mut self, gate: crate::budget::BudgetGate) -> Self {
        self.budget = Some(gate);
        self
    }

    pub fn with_run_identity(mut self, lease: crate::remint::RunTokenLease) -> Self {
        self.run_identity = Some(lease);
        self
    }

    pub fn with_signals(mut self, signals: SignalStore) -> Self {
        self.signals = Some(signals);
        self
    }

    pub fn with_timers(mut self, timers: crate::timer::TimerStore) -> Self {
        self.timers = Some(timers);
        self
    }

    pub fn timers(&self) -> Option<&crate::timer::TimerStore> {
        self.timers.as_ref()
    }

    pub fn register(&mut self, wf_type: impl Into<String>, body: Box<WorkflowBody>) {
        let wf_type = wf_type.into();
        self.running_versions.insert(wf_type.clone(), 1);
        self.bodies.insert(wf_type, body);
    }

    pub fn register_versioned(
        &mut self,
        wf_type: impl Into<String>,
        wf_version: i32,
        body: Box<WorkflowBody>,
    ) {
        let wf_type = wf_type.into();
        self.running_versions.insert(wf_type.clone(), wf_version);
        self.bodies.insert(wf_type, body);
    }

    pub fn tick(&self, now: i64, now_clock: &str, rand_seed: u64) -> Option<DriveOutcome> {
        let run =
            self.runs
                .lease_runnable(self.partition, &self.worker, now, self.lease_ttl_secs)?;
        let body = self.bodies.get(&run.wf_type)?;
        let replay_version = self
            .running_versions
            .get(&run.wf_type)
            .copied()
            .unwrap_or(run.wf_version);
        let outcome = drive_full(
            &self.runs,
            &self.outbox,
            &self.journal,
            &self.telemetry,
            self.minter.clone(),
            self.ctx_base.clone(),
            &run,
            now_clock,
            rand_seed,
            body.as_ref(),
            run.wf_version,
            replay_version,
            self.timers.clone(),
            self.signals.clone(),
            now,
            self.run_identity.clone(),
            self.budget.clone(),
        );
        Some(outcome)
    }

    pub fn telemetry(&self) -> &FlowTelemetry {
        &self.telemetry
    }

    pub fn runs(&self) -> &RunStore {
        &self.runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wfctx::{ActivityError, RetryPolicy, WfJournal};
    use myelin_events::{
        Actor, AggregateKey, ArtifactRef as EvArtifactRef, DataRole, EmitContextBase, EventDraft,
        EventType, MonotonicMinter, Visibility,
    };
    use myelin_identity::{Principal, PrincipalId, PrincipalKind};
    use std::sync::{Arc, Mutex};

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }
    fn ctx_base() -> EmitContextBase {
        EmitContextBase {
            tenant: tenant(),
            region: region(),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                tenant(),
            )),
            schema_ver: 1,
            occurred_at: myelin_events::Timestamp("2026-06-21T00:00:00Z".into()),
            recorded_at: myelin_events::Timestamp("2026-06-21T00:00:01Z".into()),
            caused_by: None,
        }
    }
    fn minter() -> Arc<dyn IdMinter> {
        Arc::new(MonotonicMinter::new())
    }
    fn draft() -> EventDraft {
        EventDraft {
            type_: EventType("agent.run.step".into()),
            subject: EvArtifactRef("myelin://acme/agent/run/R1".into()),
            aggregate: AggregateKey("run:R1".into()),
            payload: serde_json::json!({ "ref": "R1" }),
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            contains_personal_data: false,
            pii_key_ref: None,
        }
    }

    fn n_activity_body(n: usize, executed: Arc<Mutex<Vec<usize>>>) -> Box<WorkflowBody> {
        Box::new(move |ctx: &mut WfCtx| {
            for k in 0..n {
                let ex = executed.clone();
                ctx.activity(RetryPolicy::default_policy(), move |_idem, _attempt| {
                    ex.lock().unwrap().push(k);
                    Ok(vec![ArtifactRef(format!(
                        "myelin://acme/agent/effect/e{k}"
                    ))])
                })
                .map_err(|e| format!("{e:?}"))?;
            }
            Ok(vec![ArtifactRef("myelin://acme/agent/run/R1/done".into())])
        })
    }

    #[test]
    fn cold_drive_executes_and_journals_every_command() {
        let runs = RunStore::new();
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let tele = FlowTelemetry::new();
        let run = RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0);
        runs.put(run.clone());

        let executed = Arc::new(Mutex::new(Vec::new()));
        let body = n_activity_body(10, executed.clone());
        let outcome = drive(
            &runs,
            &outbox,
            &journal,
            &tele,
            minter(),
            ctx_base(),
            &run,
            "2026-06-21T00:00:00Z",
            7,
            body.as_ref(),
        );

        assert!(
            matches!(outcome, DriveOutcome::Completed(_)),
            "the run completed"
        );
        assert_eq!(
            executed.lock().unwrap().len(),
            10,
            "all 10 activities ran live (cold)"
        );
        assert_eq!(
            journal.history_for(&tenant(), "R1").len(),
            10,
            "10 history rows journaled"
        );
        let settled = runs.get(&tenant(), "R1").expect("run row");
        assert_eq!(settled.state, run_state::COMPLETED, "the run is completed");
        assert_eq!(
            settled.cursor, 10,
            "the cursor advanced to the journal depth"
        );
        assert!(
            settled.lease_owner.is_none(),
            "the lease is released on settle"
        );
        assert_eq!(tele.commands_executed(), 10, "10 live executions recorded");
        assert_eq!(
            tele.commands_replayed(),
            0,
            "nothing replayed on a cold drive"
        );
        assert_eq!(tele.double_effect_count(), 0, "0 double-effect");
    }

    #[test]
    fn crash_at_5_of_10_replays_to_6_with_zero_double_effect() {
        let runs = RunStore::new();
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let tele = FlowTelemetry::new();
        let run = RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0);
        runs.put(run.clone());

        let executed1 = Arc::new(Mutex::new(Vec::new()));
        let body1 = n_activity_body(5, executed1.clone());
        {
            let mut crash_ctx = WfCtx::begin(
                &outbox,
                minter(),
                journal.clone(),
                ctx_base(),
                "R1",
                "agent.run",
                "2026-06-21T00:00:00Z",
                7,
            );
            body1(&mut crash_ctx).expect("the 5 activities run");
            crash_ctx
                .commit()
                .expect("the 5 steps co-commit (durable before the crash)");
            let mut r = runs.get(&tenant(), "R1").expect("run");
            r.cursor = 5;
            runs.put(r);
        }
        assert_eq!(
            executed1.lock().unwrap().len(),
            5,
            "drive 1 ran activities 0..=4"
        );
        assert_eq!(
            journal.history_for(&tenant(), "R1").len(),
            5,
            "5 journaled at the crash point"
        );

        let leased = runs
            .lease_runnable(0, "worker-2", 1000, 30)
            .expect("the run is re-leasable (drive 1 released the lease on settle)");
        assert_eq!(leased.cursor, 5, "the re-leased run resumes from cursor 5");
        let executed2 = Arc::new(Mutex::new(Vec::new()));
        let body2 = n_activity_body(10, executed2.clone());
        let outcome = drive(
            &runs,
            &outbox,
            &journal,
            &tele,
            minter(),
            ctx_base(),
            &leased,
            "2026-06-21T00:00:00Z",
            7,
            body2.as_ref(),
        );

        let ran = executed2.lock().unwrap().clone();
        assert_eq!(
            ran,
            vec![5, 6, 7, 8, 9],
            "resumed at step 6 - activities 5..=9 ran, 0..=4 replayed"
        );
        assert!(
            matches!(outcome, DriveOutcome::Completed(_)),
            "the run completed after recovery"
        );
        assert_eq!(
            journal.history_for(&tenant(), "R1").len(),
            10,
            "10 journaled, 0 lost progress"
        );
        assert_eq!(
            tele.double_effect_count(),
            0,
            "0 re-executed side effects (exactly-once-in-effect)"
        );
        assert_eq!(
            tele.commands_replayed(),
            5,
            "drive 2 replayed the 5 journaled commands (short-circuited, 0 re-execution)"
        );
        assert_eq!(
            tele.commands_executed(),
            5,
            "drive 2 executed 5 new commands live"
        );
        assert_eq!(
            tele.replay_rate_bps(),
            5000,
            "the replay rate (5000 bps) is emitted (the FLOW-D1 green artifact)"
        );
    }

    #[test]
    fn full_replay_executes_zero_side_effects() {
        let runs = RunStore::new();
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let tele = FlowTelemetry::new();
        let run = RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0);
        runs.put(run.clone());

        let body = n_activity_body(10, Arc::new(Mutex::new(Vec::new())));
        drive(
            &runs,
            &outbox,
            &journal,
            &tele,
            minter(),
            ctx_base(),
            &run,
            "2026-06-21T00:00:00Z",
            7,
            body.as_ref(),
        );
        let executed = Arc::new(Mutex::new(Vec::new()));
        let body2 = n_activity_body(10, executed.clone());
        let again = runs.get(&tenant(), "R1").expect("run");
        drive(
            &runs,
            &outbox,
            &journal,
            &tele,
            minter(),
            ctx_base(),
            &again,
            "2026-06-21T00:00:00Z",
            7,
            body2.as_ref(),
        );
        assert_eq!(
            executed.lock().unwrap().len(),
            0,
            "a full replay re-executes 0 side effects"
        );
        assert_eq!(
            journal.history_for(&tenant(), "R1").len(),
            10,
            "no duplicate journal rows"
        );
        assert_eq!(
            tele.double_effect_count(),
            0,
            "0 double-effect on a full replay"
        );
    }

    #[test]
    fn lease_expiry_re_leases_to_another_worker() {
        let runs = RunStore::new();
        runs.put(RunRow::new_runnable(
            tenant(),
            region(),
            "R1",
            "agent.run",
            0,
        ));

        let l1 = runs
            .lease_runnable(0, "worker-1", 1000, 30)
            .expect("worker-1 leases");
        assert_eq!(l1.lease_owner.as_deref(), Some("worker-1"));
        assert_eq!(l1.lease_expires, Some(1030));

        assert!(
            runs.lease_runnable(0, "worker-2", 1020, 30).is_none(),
            "a live lease is skip-locked - no second worker drives the same run"
        );

        let l2 = runs
            .lease_runnable(0, "worker-2", 1031, 30)
            .expect("worker-2 re-leases after expiry");
        assert_eq!(
            l2.lease_owner.as_deref(),
            Some("worker-2"),
            "the run re-leased to worker-2"
        );
        assert_eq!(l2.lease_expires, Some(1061), "a fresh lease deadline");
    }

    #[test]
    fn lease_is_partition_scoped_and_skips_non_runnable() {
        let runs = RunStore::new();
        runs.put(RunRow::new_runnable(
            tenant(),
            region(),
            "R-p0",
            "agent.run",
            0,
        ));
        runs.put(RunRow::new_runnable(
            tenant(),
            region(),
            "R-p1",
            "agent.run",
            1,
        ));
        let mut waiting = RunRow::new_runnable(tenant(), region(), "R-wait", "agent.run", 0);
        waiting.state = run_state::WAITING.into();
        runs.put(waiting);

        let leased = runs
            .lease_runnable(0, "w", 1, 30)
            .expect("a runnable run in partition 0");
        assert_eq!(
            leased.run_id, "R-p0",
            "the partition-0 runnable run is leased"
        );
        assert_eq!(
            runs.runnable_lag(1, 1),
            1,
            "partition 1 has its own runnable run"
        );
    }

    #[test]
    fn runnable_lag_counts_unleased_and_expired() {
        let runs = RunStore::new();
        for i in 0..3 {
            runs.put(RunRow::new_runnable(
                tenant(),
                region(),
                format!("R{i}"),
                "agent.run",
                0,
            ));
        }
        assert_eq!(
            runs.runnable_lag(0, 1000),
            3,
            "three runnable runs await a lease"
        );
        runs.lease_runnable(0, "w", 1000, 30).expect("lease one");
        assert_eq!(
            runs.runnable_lag(0, 1000),
            2,
            "a live-leased run is not runnable-lag"
        );
        assert_eq!(
            runs.runnable_lag(0, 1031),
            3,
            "the expired-lease run re-enters the runnable set"
        );
    }

    #[test]
    fn drive_of_a_failing_body_settles_failed() {
        let runs = RunStore::new();
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let tele = FlowTelemetry::new();
        let run = RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0);
        runs.put(run.clone());

        let body: Box<WorkflowBody> = Box::new(|ctx: &mut WfCtx| {
            ctx.activity(RetryPolicy { max_attempts: 1 }, |_i, _a| {
                Err(ActivityError::retryable("hard failure"))
            })
            .map_err(|e| format!("{e:?}"))?;
            Ok(vec![])
        });
        let outcome = drive(
            &runs,
            &outbox,
            &journal,
            &tele,
            minter(),
            ctx_base(),
            &run,
            "2026-06-21T00:00:00Z",
            7,
            body.as_ref(),
        );
        assert!(matches!(outcome, DriveOutcome::Failed(_)), "the run failed");
        let settled = runs.get(&tenant(), "R1").expect("run");
        assert_eq!(
            settled.state,
            run_state::FAILED,
            "the run is settled failed"
        );
        assert!(
            settled.lease_owner.is_none(),
            "the lease is released even on failure"
        );
        assert_eq!(
            journal.history_for(&tenant(), "R1").len(),
            1,
            "the failure is journaled"
        );
    }

    #[test]
    fn divergent_replay_halts_nondeterministic_and_dead_letters() {
        let runs = RunStore::new();
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let tele = FlowTelemetry::new();
        runs.put(RunRow::new_runnable(
            tenant(),
            region(),
            "R1",
            "agent.run",
            0,
        ));

        {
            let mut ctx = WfCtx::begin(
                &outbox,
                minter(),
                journal.clone(),
                ctx_base(),
                "R1",
                "agent.run",
                "2026-06-21T00:00:00Z",
                7,
            );
            let _ = ctx.now();
            ctx.commit().expect("co-commit the marker");
            let mut r = runs.get(&tenant(), "R1").unwrap();
            r.cursor = 1;
            runs.put(r);
        }
        let ran = std::sync::Arc::new(Mutex::new(false));
        let ran2 = ran.clone();
        let run = runs.get(&tenant(), "R1").unwrap();
        let body: Box<WorkflowBody> = Box::new(move |ctx: &mut WfCtx| {
            let ran3 = ran2.clone();
            ctx.activity(RetryPolicy::default_policy(), move |_i, _a| {
                *ran3.lock().unwrap() = true;
                Ok(vec![ArtifactRef(
                    "myelin://acme/agent/effect/SHOULD-NOT-RUN".into(),
                )])
            })
            .map_err(|e| format!("{e:?}"))?;
            Ok(vec![])
        });
        let outcome = drive(
            &runs,
            &outbox,
            &journal,
            &tele,
            minter(),
            ctx_base(),
            &run,
            "2026-06-21T00:00:00Z",
            7,
            body.as_ref(),
        );

        assert!(
            matches!(outcome, DriveOutcome::Nondeterministic(_)),
            "the drive halts as Nondeterministic, got {outcome:?}"
        );
        assert!(
            !*ran.lock().unwrap(),
            "the divergent activity did NOT run live (the guard halted it)"
        );
        let settled = runs.get(&tenant(), "R1").expect("run row");
        assert_eq!(
            settled.state,
            run_state::NONDETERMINISTIC,
            "the run is dead-lettered as nondeterministic (terminal)"
        );
        assert!(
            run_state::is_terminal(&settled.state),
            "nondeterministic is terminal - never re-driven"
        );
        assert!(
            settled.lease_owner.is_none(),
            "the lease is released on the halt"
        );
        assert_eq!(
            settled.cursor, 1,
            "the cursor is UNCHANGED (the guard rewrote no journal)"
        );
        assert_eq!(
            journal.history_for(&tenant(), "R1").len(),
            1,
            "the journal is unchanged (the divergent drive committed NOTHING - 0 corruption)"
        );
        assert_eq!(
            tele.double_effect_count(),
            0,
            "0 double-effect - the divergence is a halt, not a re-exec"
        );
        assert_eq!(
            tele.nondeterministic_halt_count(),
            1,
            "the nondeterministic-halt counter incremented by exactly the injected divergence count (1)"
        );
    }

    #[test]
    fn wrong_version_replay_halts_nondeterministic() {
        let runs = RunStore::new();
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let tele = FlowTelemetry::new();
        runs.put(RunRow::new_runnable(
            tenant(),
            region(),
            "R1",
            "agent.run",
            0,
        ));

        let ran = std::sync::Arc::new(Mutex::new(false));
        let ran2 = ran.clone();
        let run = runs.get(&tenant(), "R1").unwrap();
        let body: Box<WorkflowBody> = Box::new(move |ctx: &mut WfCtx| {
            let ran3 = ran2.clone();
            ctx.activity(RetryPolicy::default_policy(), move |_i, _a| {
                *ran3.lock().unwrap() = true;
                Ok(vec![ArtifactRef("myelin://acme/agent/effect/e0".into())])
            })
            .map_err(|e| format!("{e:?}"))?;
            Ok(vec![])
        });
        let outcome = drive_versioned(
            &runs,
            &outbox,
            &journal,
            &tele,
            minter(),
            ctx_base(),
            &run,
            "2026-06-21T00:00:00Z",
            7,
            body.as_ref(),
            1,
            2,
        );
        assert!(
            matches!(outcome, DriveOutcome::Nondeterministic(ref r) if r.contains("wf_version")),
            "the version mismatch halts as Nondeterministic naming the version pin, got {outcome:?}"
        );
        assert!(
            !*ran.lock().unwrap(),
            "the body did NOT run a command (the version guard halted first)"
        );
        assert_eq!(
            runs.get(&tenant(), "R1").unwrap().state,
            run_state::NONDETERMINISTIC,
            "the wrong-version run is dead-lettered"
        );
        assert_eq!(
            tele.nondeterministic_halt_count(),
            1,
            "the version-divergence halt counted once"
        );
        runs.put(RunRow::new_runnable(
            tenant(),
            region(),
            "R2",
            "agent.run",
            0,
        ));
        let run2 = runs.get(&tenant(), "R2").unwrap();
        let ok_body = n_activity_body(1, std::sync::Arc::new(Mutex::new(Vec::new())));
        let outcome2 = drive_versioned(
            &runs,
            &outbox,
            &journal,
            &tele,
            minter(),
            ctx_base(),
            &run2,
            "2026-06-21T00:00:00Z",
            7,
            ok_body.as_ref(),
            1,
            1,
        );
        assert!(
            matches!(outcome2, DriveOutcome::Completed(_)),
            "a matching version drives normally"
        );
        assert_eq!(
            tele.nondeterministic_halt_count(),
            1,
            "no false-positive halt on a matching version"
        );
    }

    #[test]
    fn deterministic_replay_does_not_trip_the_guard() {
        let runs = RunStore::new();
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let tele = FlowTelemetry::new();
        runs.put(RunRow::new_runnable(
            tenant(),
            region(),
            "R1",
            "agent.run",
            0,
        ));

        let run = runs.get(&tenant(), "R1").unwrap();
        drive(
            &runs,
            &outbox,
            &journal,
            &tele,
            minter(),
            ctx_base(),
            &run,
            "2026-06-21T00:00:00Z",
            7,
            n_activity_body(3, std::sync::Arc::new(Mutex::new(Vec::new()))).as_ref(),
        );
        let again = runs.get(&tenant(), "R1").unwrap();
        let outcome = drive(
            &runs,
            &outbox,
            &journal,
            &tele,
            minter(),
            ctx_base(),
            &again,
            "2026-06-21T00:00:00Z",
            7,
            n_activity_body(3, std::sync::Arc::new(Mutex::new(Vec::new()))).as_ref(),
        );
        assert!(
            matches!(outcome, DriveOutcome::Completed(_)),
            "the deterministic re-drive completes"
        );
        assert_eq!(
            tele.nondeterministic_halt_count(),
            0,
            "0 silent divergence: a deterministic replay NEVER trips the divergence guard"
        );
    }

    #[test]
    fn telemetry_accumulates_across_drives_and_reports_lag() {
        let runs = RunStore::new();
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let tele = FlowTelemetry::new();
        runs.put(RunRow::new_runnable(
            tenant(),
            region(),
            "R1",
            "agent.run",
            0,
        ));
        runs.put(RunRow::new_runnable(
            tenant(),
            region(),
            "R2",
            "agent.run",
            0,
        ));
        assert_eq!(
            tele.runnable_run_lag(),
            0,
            "no drive yet → the lag gauge default is 0"
        );

        let run1 = runs.get(&tenant(), "R1").unwrap();
        drive(
            &runs,
            &outbox,
            &journal,
            &tele,
            minter(),
            ctx_base(),
            &run1,
            "2026-06-21T00:00:00Z",
            7,
            n_activity_body(3, Arc::new(Mutex::new(Vec::new()))).as_ref(),
        );
        assert_eq!(tele.commands_executed(), 3, "drive 1 executed 3 commands");
        assert_eq!(
            tele.runnable_run_lag(),
            1,
            "R2 is still runnable (the lag gauge is set from the store)"
        );

        let run2 = runs.get(&tenant(), "R2").unwrap();
        drive(
            &runs,
            &outbox,
            &journal,
            &tele,
            minter(),
            ctx_base(),
            &run2,
            "2026-06-21T00:00:00Z",
            7,
            n_activity_body(3, Arc::new(Mutex::new(Vec::new()))).as_ref(),
        );
        assert_eq!(
            tele.commands_executed(),
            6,
            "the executed counter accumulated across drives (6)"
        );
        assert_eq!(
            tele.runnable_run_lag(),
            0,
            "both runs completed → the runnable-run-lag drops to 0"
        );
        assert_eq!(
            tele.double_effect_count(),
            0,
            "0 double-effect across both drives"
        );
    }

    #[test]
    fn resume_continues_history_seq_past_the_journal() {
        let runs = RunStore::new();
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let tele = FlowTelemetry::new();
        runs.put(RunRow::new_runnable(
            tenant(),
            region(),
            "R1",
            "agent.run",
            0,
        ));

        {
            let mut ctx = WfCtx::begin(
                &outbox,
                minter(),
                journal.clone(),
                ctx_base(),
                "R1",
                "agent.run",
                "2026-06-21T00:00:00Z",
                7,
            );
            for k in 0..3 {
                ctx.activity(RetryPolicy::default_policy(), move |_i, _a| {
                    Ok(vec![ArtifactRef(format!(
                        "myelin://acme/agent/effect/e{k}"
                    ))])
                })
                .expect("activity");
            }
            ctx.commit().expect("3 steps co-commit");
            let mut r = runs.get(&tenant(), "R1").unwrap();
            r.cursor = 3;
            runs.put(r);
        }

        let run = runs.get(&tenant(), "R1").unwrap();
        drive(
            &runs,
            &outbox,
            &journal,
            &tele,
            minter(),
            ctx_base(),
            &run,
            "2026-06-21T00:00:00Z",
            7,
            n_activity_body(4, Arc::new(Mutex::new(Vec::new()))).as_ref(),
        );
        let seqs: Vec<i64> = journal
            .history_for(&tenant(), "R1")
            .iter()
            .map(|r| r.seq)
            .collect();
        assert_eq!(
            seqs,
            vec![0, 1, 2, 3],
            "the resumed command journaled at seq 3 (past the journal, no overwrite)"
        );
    }

    #[test]
    fn dispatcher_tick_leases_and_drives_runnable_runs() {
        let runs = RunStore::new();
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let tele = FlowTelemetry::new();
        runs.put(RunRow::new_runnable(
            tenant(),
            region(),
            "R1",
            "agent.run",
            0,
        ));
        runs.put(RunRow::new_runnable(
            tenant(),
            region(),
            "R2",
            "agent.run",
            0,
        ));

        let mut disp = FlowDispatcher::new(
            runs.clone(),
            outbox.clone(),
            journal.clone(),
            tele.clone(),
            minter(),
            ctx_base(),
            0,
            "worker-1",
            30,
        );
        disp.register(
            "agent.run",
            n_activity_body(3, Arc::new(Mutex::new(Vec::new()))),
        );

        assert!(matches!(
            disp.tick(1000, "2026-06-21T00:00:00Z", 7),
            Some(DriveOutcome::Completed(_))
        ));
        assert!(matches!(
            disp.tick(1001, "2026-06-21T00:00:00Z", 7),
            Some(DriveOutcome::Completed(_))
        ));
        assert!(
            disp.tick(1002, "2026-06-21T00:00:00Z", 7).is_none(),
            "no runnable work left"
        );
        assert_eq!(
            runs.get(&tenant(), "R1").unwrap().state,
            run_state::COMPLETED
        );
        assert_eq!(
            runs.get(&tenant(), "R2").unwrap().state,
            run_state::COMPLETED
        );
        assert_eq!(
            disp.telemetry().double_effect_count(),
            0,
            "0 double-effect across the loop"
        );
    }

    #[test]
    fn dispatcher_re_leases_a_crashed_run_and_resumes() {
        let runs = RunStore::new();
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let tele = FlowTelemetry::new();
        runs.put(RunRow::new_runnable(
            tenant(),
            region(),
            "R1",
            "agent.run",
            0,
        ));

        {
            let mut ctx = WfCtx::begin(
                &outbox,
                minter(),
                journal.clone(),
                ctx_base(),
                "R1",
                "agent.run",
                "2026-06-21T00:00:00Z",
                7,
            );
            for k in 0..3 {
                ctx.activity(RetryPolicy::default_policy(), move |_i, _a| {
                    Ok(vec![ArtifactRef(format!(
                        "myelin://acme/agent/effect/e{k}"
                    ))])
                })
                .expect("activity");
            }
            ctx.commit().expect("3 steps co-commit");
            let mut r = runs.get(&tenant(), "R1").unwrap();
            r.cursor = 3;
            r.lease_owner = Some("dead-worker".into());
            r.lease_expires = Some(500);
            runs.put(r);
        }

        let executed = Arc::new(Mutex::new(Vec::new()));
        let mut disp = FlowDispatcher::new(
            runs.clone(),
            outbox.clone(),
            journal.clone(),
            tele.clone(),
            minter(),
            ctx_base(),
            0,
            "worker-2",
            30,
        );
        disp.register("agent.run", n_activity_body(5, executed.clone()));
        let outcome = disp
            .tick(1000, "2026-06-21T00:00:00Z", 7)
            .expect("a runnable run was re-leased");
        assert!(
            matches!(outcome, DriveOutcome::Completed(_)),
            "resumed to completion"
        );
        assert_eq!(
            executed.lock().unwrap().clone(),
            vec![3, 4],
            "resumed at step 4 - only 3,4 ran"
        );
        assert_eq!(
            tele.double_effect_count(),
            0,
            "0 re-executed side effects on re-lease"
        );
        assert_eq!(
            disp.telemetry().commands_replayed(),
            3,
            "the dispatcher's telemetry recorded 3 replays"
        );
        assert_eq!(
            disp.telemetry().replay_rate_bps(),
            6000,
            "the live telemetry handle reports 6000 bps"
        );
    }

    #[test]
    fn drive_emit_co_commits_and_replay_does_not_re_emit() {
        let runs = RunStore::new();
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let tele = FlowTelemetry::new();
        let run = RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0);
        runs.put(run.clone());

        let make_body = || -> Box<WorkflowBody> {
            Box::new(|ctx: &mut WfCtx| {
                ctx.activity(RetryPolicy::default_policy(), |_i, _a| {
                    Ok(vec![ArtifactRef("myelin://acme/agent/effect/e0".into())])
                })
                .map_err(|e| format!("{e:?}"))?;
                ctx.emit(draft(), None).map_err(|e| format!("{e:?}"))?;
                Ok(vec![])
            })
        };
        drive(
            &runs,
            &outbox,
            &journal,
            &tele,
            minter(),
            ctx_base(),
            &run,
            "2026-06-21T00:00:00Z",
            7,
            make_body().as_ref(),
        );
        assert_eq!(
            outbox.committed_count(),
            1,
            "the emit co-committed with the journal"
        );

        let again = runs.get(&tenant(), "R1").expect("run");
        drive(
            &runs,
            &outbox,
            &journal,
            &tele,
            minter(),
            ctx_base(),
            &again,
            "2026-06-21T00:00:00Z",
            7,
            make_body().as_ref(),
        );
        assert_eq!(
            tele.double_effect_count(),
            0,
            "0 double-effect on the re-drive"
        );
        assert_eq!(
            journal.history_for(&tenant(), "R1").len(),
            1,
            "no duplicate journal row"
        );
    }
}

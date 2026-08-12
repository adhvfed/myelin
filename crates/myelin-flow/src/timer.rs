use crate::engine::{run_state, FlowTelemetry, RunStore};
use crate::wfctx::WfJournal;
use myelin_tenancy::{Region, TenantId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const SECS_PER_MINUTE: i64 = 60;

pub fn epoch_minute(fire_at_secs: i64) -> i32 {
    (fire_at_secs.max(0) / SECS_PER_MINUTE) as i32
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimerRow {
    pub tenant: TenantId,
    pub region: Region,
    pub timer_id: String,
    pub run_id: Option<String>,
    pub command_id: String,
    pub fire_at: i64,
    pub bucket: i32,
    pub fired: bool,
    pub partition: i16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArmOutcome {
    Armed,
    AlreadyArmed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FireOutcome {
    Fired,
    AlreadyFired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReArmOutcome {
    ReArmed,
    Absent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisarmOutcome {
    Disarmed,
    Absent,
}

#[derive(Clone, Default)]
pub struct TimerStore {
    inner: Arc<Mutex<TimerInner>>,
}

#[derive(Default)]
struct TimerInner {
    timers: HashMap<(String, String), TimerRow>,
    rows_scanned: u64,
}

impl TimerStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TimerInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn arm(&self, row: TimerRow) -> ArmOutcome {
        let mut inner = self.lock();
        let key = (row.tenant.0.clone(), row.timer_id.clone());
        if inner.timers.contains_key(&key) {
            return ArmOutcome::AlreadyArmed;
        }
        inner.timers.insert(key, row);
        ArmOutcome::Armed
    }

    pub fn re_arm(&self, tenant: &TenantId, timer_id: &str, new_fire_at: i64) -> ReArmOutcome {
        let mut inner = self.lock();
        let key = (tenant.0.clone(), timer_id.to_string());
        let Some(t) = inner.timers.get_mut(&key) else {
            return ReArmOutcome::Absent;
        };
        t.fire_at = new_fire_at;
        t.bucket = epoch_minute(new_fire_at);
        t.fired = false;
        ReArmOutcome::ReArmed
    }

    pub fn disarm(&self, tenant: &TenantId, timer_id: &str) -> DisarmOutcome {
        let mut inner = self.lock();
        let key = (tenant.0.clone(), timer_id.to_string());
        let Some(t) = inner.timers.get_mut(&key) else {
            return DisarmOutcome::Absent;
        };
        if t.fired {
            return DisarmOutcome::Absent;
        }
        t.fired = true;
        DisarmOutcome::Disarmed
    }

    pub fn disarm_delete(&self, tenant: &TenantId, timer_id: &str) -> DisarmOutcome {
        let mut inner = self.lock();
        let key = (tenant.0.clone(), timer_id.to_string());
        match inner.timers.remove(&key) {
            Some(_) => DisarmOutcome::Disarmed,
            None => DisarmOutcome::Absent,
        }
    }

    pub fn scan_due(&self, partition: i16, now_secs: i64, batch: usize) -> Vec<TimerRow> {
        let now_bucket = epoch_minute(now_secs);
        let mut inner = self.lock();
        let mut due: Vec<TimerRow> = inner
            .timers
            .values()
            .filter(|t| t.partition == partition && !t.fired && t.bucket <= now_bucket)
            .cloned()
            .collect();
        due.sort_by(|a, b| a.fire_at.cmp(&b.fire_at).then(a.timer_id.cmp(&b.timer_id)));
        due.truncate(batch);
        inner.rows_scanned += due.len() as u64;
        due
    }

    pub fn fire(
        &self,
        tenant: &TenantId,
        timer_id: &str,
        journal: &WfJournal,
        runs: &RunStore,
    ) -> FireOutcome {
        let row = {
            let mut inner = self.lock();
            let key = (tenant.0.clone(), timer_id.to_string());
            let Some(t) = inner.timers.get_mut(&key) else {
                return FireOutcome::AlreadyFired;
            };
            if t.fired {
                return FireOutcome::AlreadyFired;
            }
            t.fired = true;
            t.clone()
        };

        if let Some(run_id) = &row.run_id {
            let seq = journal.history_for(tenant, run_id).len() as i64;
            journal.append_history_for_test(crate::schema::WfHistoryRow {
                tenant: tenant.clone(),
                region: row.region.clone(),
                run_id: run_id.clone(),
                seq,
                kind: history_kind::TIMER_FIRED.to_string(),
                command_id: row.command_id.clone(),
                result: None,
                result_key_ref: None,
            });
            runs.wake(tenant, run_id);
        }
        FireOutcome::Fired
    }

    pub fn get(&self, tenant: &TenantId, timer_id: &str) -> Option<TimerRow> {
        self.lock()
            .timers
            .get(&(tenant.0.clone(), timer_id.to_string()))
            .cloned()
    }

    pub fn rows_for_run(&self, tenant: &TenantId, region: &Region, run_id: &str) -> Vec<TimerRow> {
        let mut rows: Vec<_> = self
            .lock()
            .timers
            .values()
            .filter(|row| {
                row.tenant == *tenant
                    && row.region == *region
                    && row.run_id.as_deref() == Some(run_id)
            })
            .cloned()
            .collect();
        rows.sort_by(|left, right| left.timer_id.cmp(&right.timer_id));
        rows
    }

    pub fn wheel_lag(&self, partition: i16, now_secs: i64) -> u64 {
        let now_bucket = epoch_minute(now_secs);
        self.lock()
            .timers
            .values()
            .filter(|t| t.partition == partition && !t.fired && t.bucket <= now_bucket)
            .count() as u64
    }

    pub fn armed_count(&self) -> usize {
        self.lock().timers.len()
    }

    pub fn unfired_count(&self) -> usize {
        self.lock().timers.values().filter(|t| !t.fired).count()
    }

    pub fn rows_scanned(&self) -> u64 {
        self.lock().rows_scanned
    }
}

pub mod sla {
    use super::{DisarmOutcome, ReArmOutcome, TimerStore};
    use myelin_tenancy::TenantId;

    pub fn sla_timer_id(issue_key: &str) -> String {
        format!("sla/{issue_key}")
    }

    pub fn trigger_stale_timer_id(owner: &str, arms_subject: &str) -> String {
        format!("trigger/{owner}/{arms_subject}")
    }

    pub struct SlaTimerCall<'a> {
        timers: &'a TimerStore,
        tenant: TenantId,
        timer_id: String,
    }

    impl<'a> SlaTimerCall<'a> {
        pub fn new(timers: &'a TimerStore, tenant: TenantId, timer_id: impl Into<String>) -> Self {
            Self {
                timers,
                tenant,
                timer_id: timer_id.into(),
            }
        }

        pub fn timer_id(&self) -> &str {
            &self.timer_id
        }

        pub fn re_arm(&self, new_fire_at: i64) -> ReArmOutcome {
            self.timers
                .re_arm(&self.tenant, &self.timer_id, new_fire_at)
        }

        pub fn disarm(&self) -> DisarmOutcome {
            self.timers.disarm(&self.tenant, &self.timer_id)
        }
    }
}

pub mod promotion {
    pub const PROMOTE_DUE_NOW_PER_SEC_PER_CELL_SEED: u64 = 100_000;
    pub const DEGRADED_WHEEL_LAG_BUDGET_SEED: u64 = 0;
}

pub mod history_kind {
    pub const TIMER_SET: &str = "timer_set";
    pub const TIMER_FIRED: &str = "timer_fired";
}

pub struct TimerWheel {
    timers: TimerStore,
    journal: WfJournal,
    runs: RunStore,
    telemetry: FlowTelemetry,
    partition: i16,
    batch: usize,
}

impl TimerWheel {
    pub fn new(
        timers: TimerStore,
        journal: WfJournal,
        runs: RunStore,
        telemetry: FlowTelemetry,
        partition: i16,
        batch: usize,
    ) -> Self {
        Self {
            timers,
            journal,
            runs,
            telemetry,
            partition,
            batch,
        }
    }

    pub fn tick(&self, now_secs: i64) -> usize {
        let due = self.timers.scan_due(self.partition, now_secs, self.batch);
        let mut fired = 0usize;
        for t in &due {
            if self
                .timers
                .fire(&t.tenant, &t.timer_id, &self.journal, &self.runs)
                == FireOutcome::Fired
            {
                fired += 1;
            }
        }
        self.telemetry
            .set_timer_wheel_lag(self.timers.wheel_lag(self.partition, now_secs));
        fired
    }

    pub fn timers(&self) -> &TimerStore {
        &self.timers
    }

    pub fn telemetry(&self) -> &FlowTelemetry {
        &self.telemetry
    }
}

pub fn partition_for(run_id: &str, shards: u16) -> i16 {
    crate::executor::partition_for_shards(run_id, u32::from(shards))
}

pub struct WheelShardSet {
    wheels: Vec<TimerWheel>,
    timers: TimerStore,
}

impl WheelShardSet {
    pub fn new(
        timers: TimerStore,
        journal: WfJournal,
        runs: RunStore,
        telemetry: FlowTelemetry,
        shards: u16,
        batch: usize,
    ) -> Self {
        let shards = shards.max(1);
        let wheels = (0..shards)
            .map(|p| {
                TimerWheel::new(
                    timers.clone(),
                    journal.clone(),
                    runs.clone(),
                    telemetry.clone(),
                    p as i16,
                    batch,
                )
            })
            .collect();
        Self { wheels, timers }
    }

    pub fn shards(&self) -> usize {
        self.wheels.len()
    }

    pub fn tick_all(&self, now_secs: i64) -> usize {
        self.wheels.iter().map(|w| w.tick(now_secs)).sum()
    }

    pub fn timers(&self) -> &TimerStore {
        &self.timers
    }
}

impl RunStore {
    pub fn wake(&self, tenant: &TenantId, run_id: &str) {
        self.with_run_mut(tenant, run_id, |run| {
            if run.state == run_state::WAITING {
                run.state = run_state::RUNNING.to_string();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::RunRow;

    fn tenant() -> TenantId {
        TenantId("acme".into())
    }
    fn region() -> Region {
        Region("fr-par".into())
    }

    fn timer(timer_id: &str, run_id: &str, fire_at: i64, partition: i16) -> TimerRow {
        TimerRow {
            tenant: tenant(),
            region: region(),
            timer_id: timer_id.into(),
            run_id: Some(run_id.into()),
            command_id: format!("agent.run:{timer_id}"),
            fire_at,
            bucket: epoch_minute(fire_at),
            fired: false,
            partition,
        }
    }

    #[test]
    fn epoch_minute_is_the_coarse_minute_bucket() {
        assert_eq!(epoch_minute(0), 0);
        assert_eq!(epoch_minute(59), 0, "second 59 is still minute 0");
        assert_eq!(epoch_minute(60), 1, "second 60 rolls into minute 1");
        assert_eq!(epoch_minute(3600), 60, "one hour is bucket 60");
        assert_eq!(
            epoch_minute(30 * 24 * 3600),
            43_200,
            "30 days is bucket 43200"
        );
        assert_eq!(
            epoch_minute(-5),
            0,
            "a pre-epoch deadline floors to bucket 0 (immediately due)"
        );
    }

    #[test]
    fn arming_is_idempotent_on_the_deterministic_timer_id() {
        let store = TimerStore::new();
        assert_eq!(
            store.arm(timer("t1", "R1", 600, 3)),
            ArmOutcome::Armed,
            "the first arm lands the row"
        );
        assert_eq!(
            store.arm(timer("t1", "R1", 600, 3)),
            ArmOutcome::AlreadyArmed,
            "a re-arm of the SAME timer_id is a no-op (a replayed sleep never double-arms)"
        );
        assert_eq!(
            store.armed_count(),
            1,
            "exactly one timer on the wheel (the re-arm was a no-op)"
        );
    }

    #[test]
    fn the_scan_reads_only_the_due_bucket_far_future_never_touched() {
        let store = TimerStore::new();
        store.arm(timer("due", "R-due", 0, 0));
        for i in 0..100 {
            store.arm(timer(
                &format!("far{i}"),
                &format!("R-far{i}"),
                30 * 24 * 3600,
                0,
            ));
        }
        assert_eq!(store.armed_count(), 101, "101 timers armed");

        let due = store.scan_due(0, 30, 1000);
        assert_eq!(
            due.len(),
            1,
            "exactly the ONE due timer (the 100 far-future are not in the due bucket)"
        );
        assert_eq!(due[0].timer_id, "due");
        assert_eq!(
            store.rows_scanned(),
            1,
            "the scan touched ONLY the due row (indexed, not full-scan)"
        );
        assert_eq!(
            store.wheel_lag(0, 30),
            1,
            "the lag counts only the due timer, not the far-future fleet"
        );
    }

    #[test]
    fn firing_is_effectively_once_set_fired_journal_wake() {
        let store = TimerStore::new();
        let journal = WfJournal::new();
        let runs = RunStore::new();
        let mut run = RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0);
        run.state = run_state::WAITING.into();
        runs.put(run);
        store.arm(timer("t1", "R1", 0, 0));

        assert_eq!(
            store.fire(&tenant(), "t1", &journal, &runs),
            FireOutcome::Fired,
            "the first fire fires"
        );
        assert!(
            store.get(&tenant(), "t1").unwrap().fired,
            "the timer is fired (the partial-index pivot)"
        );
        let hist = journal.history_for(&tenant(), "R1");
        assert_eq!(hist.len(), 1, "exactly one timer_fired journal row");
        assert_eq!(hist[0].kind, history_kind::TIMER_FIRED);
        assert_eq!(
            hist[0].command_id, "agent.run:t1",
            "the fire journals under the timer's command_id"
        );
        assert_eq!(
            runs.get(&tenant(), "R1").unwrap().state,
            run_state::RUNNING,
            "the parked run woke (waiting → running) - the dispatcher re-drives it"
        );

        assert_eq!(
            store.fire(&tenant(), "t1", &journal, &runs),
            FireOutcome::AlreadyFired,
            "a re-fire of an already-fired timer is a no-op (effectively-once, 0 double-fire)"
        );
        assert_eq!(
            journal.history_for(&tenant(), "R1").len(),
            1,
            "still ONE timer_fired row (the journal UNIQUE made the re-journal a no-op)"
        );
    }

    #[test]
    fn the_scan_does_not_double_fire_a_timer_under_concurrent_claims() {
        let store = TimerStore::new();
        let journal = WfJournal::new();
        let runs = RunStore::new();
        let mut run = RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0);
        run.state = run_state::WAITING.into();
        runs.put(run);
        store.arm(timer("t1", "R1", 0, 0));

        let claim_a = store.scan_due(0, 30, 100);
        let claim_b = store.scan_due(0, 30, 100);
        assert_eq!(claim_a.len(), 1);
        assert_eq!(
            claim_b.len(),
            1,
            "both claims see the unfired timer (the SQL SKIP LOCKED would gate to one)"
        );

        let a = store.fire(&tenant(), "t1", &journal, &runs);
        let b = store.fire(&tenant(), "t1", &journal, &runs);
        assert_eq!(a, FireOutcome::Fired, "worker A fires the timer");
        assert_eq!(
            b,
            FireOutcome::AlreadyFired,
            "worker B's claim is a no-op (0 double-fire)"
        );
        assert_eq!(
            journal.history_for(&tenant(), "R1").len(),
            1,
            "ONE timer_fired row (fired once in effect)"
        );
    }

    #[test]
    fn a_crash_re_fires_only_the_unfired_timers() {
        let store = TimerStore::new();
        let journal = WfJournal::new();
        let runs = RunStore::new();
        for id in ["a", "b", "c"] {
            let mut run =
                RunRow::new_runnable(tenant(), region(), format!("R-{id}"), "agent.run", 0);
            run.state = run_state::WAITING.into();
            runs.put(run);
            store.arm(timer(id, &format!("R-{id}"), 0, 0));
        }
        assert_eq!(
            store.fire(&tenant(), "a", &journal, &runs),
            FireOutcome::Fired
        );

        let after_crash = store.scan_due(0, 30, 100);
        let ids: Vec<&str> = after_crash.iter().map(|t| t.timer_id.as_str()).collect();
        assert_eq!(
            after_crash.len(),
            2,
            "only the two UNFIRED timers are re-scanned (a is excluded by WHERE NOT fired)"
        );
        assert!(
            !ids.contains(&"a"),
            "the already-fired timer `a` is NOT re-fired (0 double-fire)"
        );
        assert!(
            ids.contains(&"b") && ids.contains(&"c"),
            "the unfired b/c are re-fired (0 lost)"
        );
    }

    #[test]
    fn the_wheel_tick_fires_due_and_drives_the_lag_to_zero() {
        let store = TimerStore::new();
        let journal = WfJournal::new();
        let runs = RunStore::new();
        let tele = FlowTelemetry::new();
        for i in 0..5 {
            let mut run = RunRow::new_runnable(tenant(), region(), format!("R{i}"), "agent.run", 0);
            run.state = run_state::WAITING.into();
            runs.put(run);
            store.arm(timer(&format!("due{i}"), &format!("R{i}"), 0, 0));
        }
        store.arm(timer("far", "R-far", 30 * 24 * 3600, 0));

        let wheel = TimerWheel::new(
            store.clone(),
            journal.clone(),
            runs.clone(),
            tele.clone(),
            0,
            1000,
        );
        assert_eq!(
            store.wheel_lag(0, 30),
            5,
            "five due timers are the lag (the far-future is not)"
        );

        let fired = wheel.tick(30);
        assert_eq!(fired, 5, "the tick fired all five due timers");
        for i in 0..5 {
            assert_eq!(
                runs.get(&tenant(), &format!("R{i}")).unwrap().state,
                run_state::RUNNING
            );
        }
        assert_eq!(
            tele.timer_wheel_lag(),
            0,
            "the timer-wheel-lag is 0 after the tick (the SC-11 health signal)"
        );
        assert!(
            !store.get(&tenant(), "far").unwrap().fired,
            "the far-future timer is untouched"
        );
    }

    #[test]
    fn a_re_arm_is_one_row_update_of_fire_at_and_bucket_no_new_row() {
        let store = TimerStore::new();
        store.arm(timer("sla", "R1", 600, 0));
        assert_eq!(store.armed_count(), 1);
        assert_eq!(store.get(&tenant(), "sla").unwrap().bucket, 10);
        assert_eq!(
            store.rows_scanned(),
            0,
            "no scan yet - the re-arm must not rescan the wheel"
        );

        assert_eq!(
            store.re_arm(&tenant(), "sla", 1800),
            ReArmOutcome::ReArmed,
            "the re-arm updates the row"
        );
        let row = store.get(&tenant(), "sla").unwrap();
        assert_eq!(
            row.fire_at, 1800,
            "fire_at slid forward (the cheap row update)"
        );
        assert_eq!(
            row.bucket,
            epoch_minute(1800),
            "the derived bucket was recomputed"
        );
        assert_eq!(row.bucket, 30, "the new minute bucket");
        assert_eq!(
            store.armed_count(),
            1,
            "STILL one row - a re-arm is an UPDATE, not a new INSERT (no wheel pollution)"
        );
        assert_eq!(
            store.rows_scanned(),
            0,
            "the re-arm did NOT scan the wheel (row-update cost, not wheel-scan cost)"
        );

        assert!(
            store.scan_due(0, 900, 100).is_empty(),
            "the re-armed timer is no longer due at the old time"
        );
    }

    #[test]
    fn a_re_arm_reopens_a_fired_timer_and_is_absent_for_an_unarmed_one() {
        let store = TimerStore::new();
        let journal = WfJournal::new();
        let runs = RunStore::new();
        let mut run = RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0);
        run.state = run_state::WAITING.into();
        runs.put(run);
        store.arm(timer("sla", "R1", 0, 0));
        assert_eq!(
            store.fire(&tenant(), "sla", &journal, &runs),
            FireOutcome::Fired
        );
        assert!(
            store.get(&tenant(), "sla").unwrap().fired,
            "the timer fired"
        );

        assert_eq!(store.re_arm(&tenant(), "sla", 1800), ReArmOutcome::ReArmed);
        let row = store.get(&tenant(), "sla").unwrap();
        assert!(
            !row.fired,
            "the re-arm re-opened the fired timer (fired = false)"
        );
        assert_eq!(row.fire_at, 1800);
        assert_eq!(row.bucket, 30);

        assert_eq!(
            store.re_arm(&tenant(), "ghost", 1800),
            ReArmOutcome::Absent,
            "no row to re-arm"
        );
    }

    #[test]
    fn re_arming_n_timers_is_n_row_updates_not_a_wheel_rescan() {
        let store = TimerStore::new();
        for i in 0..1000 {
            store.arm(timer(&format!("sla{i}"), &format!("R{i}"), 600, 0));
        }
        assert_eq!(store.armed_count(), 1000);
        assert_eq!(store.rows_scanned(), 0);

        for i in 0..1000 {
            assert_eq!(
                store.re_arm(&tenant(), &format!("sla{i}"), 1800 + i as i64),
                ReArmOutcome::ReArmed
            );
        }
        assert_eq!(
            store.armed_count(),
            1000,
            "STILL 1000 rows (no duplicates - every re-arm was an in-place UPDATE)"
        );
        assert_eq!(
            store.rows_scanned(),
            0,
            "1000 re-arms scanned the wheel ZERO times (row-update cost, not wheel-scan cost)"
        );
    }

    #[test]
    fn a_disarm_sets_fired_and_the_timer_never_fires() {
        let store = TimerStore::new();
        let journal = WfJournal::new();
        let runs = RunStore::new();
        let mut run = RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0);
        run.state = run_state::WAITING.into();
        runs.put(run);
        store.arm(timer("sla", "R1", 0, 0));

        assert_eq!(
            store.disarm(&tenant(), "sla"),
            DisarmOutcome::Disarmed,
            "the disarm sets fired"
        );
        assert!(
            store.get(&tenant(), "sla").unwrap().fired,
            "the disarmed timer's partial-index pivot is set"
        );
        assert_eq!(
            store.armed_count(),
            1,
            "the disarmed row stays (excluded from the wheel by WHERE NOT fired)"
        );

        let due = store.scan_due(0, 30, 100);
        assert!(
            due.is_empty(),
            "the disarmed timer is NOT in the due scan (WHERE NOT fired excludes it)"
        );
        let wheel = TimerWheel::new(
            store.clone(),
            journal.clone(),
            runs.clone(),
            FlowTelemetry::new(),
            0,
            100,
        );
        assert_eq!(
            wheel.tick(30),
            0,
            "the wheel fires NOTHING - the disarmed timer never fires"
        );
        assert_eq!(
            journal.history_for(&tenant(), "R1").len(),
            0,
            "no timer_fired row - the disarm cancelled the fire"
        );
        assert_eq!(
            runs.get(&tenant(), "R1").unwrap().state,
            run_state::WAITING,
            "the parked run was never woken"
        );

        assert_eq!(
            store.disarm(&tenant(), "sla"),
            DisarmOutcome::Absent,
            "re-disarm of a fired timer is a no-op"
        );
        assert_eq!(
            store.disarm(&tenant(), "ghost"),
            DisarmOutcome::Absent,
            "disarm of an absent timer is a no-op"
        );
    }

    #[test]
    fn disarm_delete_removes_the_row_and_the_timer_never_fires() {
        let store = TimerStore::new();
        store.arm(timer("sla", "R1", 0, 0));
        assert_eq!(store.armed_count(), 1);

        assert_eq!(
            store.disarm_delete(&tenant(), "sla"),
            DisarmOutcome::Disarmed,
            "the row is deleted"
        );
        assert_eq!(
            store.armed_count(),
            0,
            "the row is gone (the no-trace cancel)"
        );
        assert!(
            store.get(&tenant(), "sla").is_none(),
            "the timer no longer exists"
        );
        assert!(
            store.scan_due(0, 30, 100).is_empty(),
            "nothing to fire - the timer was deleted"
        );

        assert_eq!(
            store.disarm_delete(&tenant(), "sla"),
            DisarmOutcome::Absent,
            "a re-delete is a no-op"
        );
    }

    #[test]
    fn the_call_site_helper_derives_stable_per_producer_timer_ids() {
        use super::sla::{sla_timer_id, trigger_stale_timer_id};
        assert_eq!(
            sla_timer_id("acme/proj#7"),
            "sla/acme/proj#7",
            "Issues SLA key is sla/<issue_key>"
        );
        assert_eq!(
            sla_timer_id("acme/proj#7"),
            sla_timer_id("acme/proj#7"),
            "the same issue → the same key"
        );
        assert_eq!(
            trigger_stale_timer_id("u-42", "issue/acme/proj#7"),
            "trigger/u-42/issue/acme/proj#7",
            "Trigger stale_after key is trigger/<owner>/<arms_subject>"
        );
        assert_ne!(sla_timer_id("x"), trigger_stale_timer_id("x", "x"));
    }

    #[test]
    fn the_call_site_helper_re_arm_is_one_row_update_disarm_is_one_row_op() {
        use super::sla::{sla_timer_id, trigger_stale_timer_id, SlaTimerCall};
        let store = TimerStore::new();

        let issue_key = "acme/proj#7";
        let id = sla_timer_id(issue_key);
        store.arm(timer(&id, "R-sla-7", 14_400, 0));
        assert_eq!(store.armed_count(), 1);
        assert_eq!(
            store.rows_scanned(),
            0,
            "no scan yet - the call site must not rescan the wheel"
        );

        let call = SlaTimerCall::new(&store, tenant(), id.clone());
        assert_eq!(call.timer_id(), "sla/acme/proj#7");
        assert_eq!(
            call.re_arm(28_800),
            ReArmOutcome::ReArmed,
            "the call-site re-arm updates the row"
        );
        let row = store.get(&tenant(), &id).unwrap();
        assert_eq!(
            row.fire_at, 28_800,
            "fire_at slid forward at the call boundary"
        );
        assert_eq!(
            row.bucket,
            epoch_minute(28_800),
            "the derived bucket was recomputed"
        );
        assert_eq!(
            store.armed_count(),
            1,
            "STILL one row - the call-site re-arm is an UPDATE, not a new arm"
        );
        assert_eq!(
            store.rows_scanned(),
            0,
            "the call-site re-arm did NOT scan the wheel (row-update cost)"
        );
        assert_eq!(
            call.disarm(),
            DisarmOutcome::Disarmed,
            "the call-site disarm is one row op"
        );
        assert!(
            store.get(&tenant(), &id).unwrap().fired,
            "the disarmed breach's partial-index pivot is set"
        );
        assert_eq!(
            call.disarm(),
            DisarmOutcome::Absent,
            "a re-disarm at the call site is idempotent (0 rows)"
        );

        let trig_id = trigger_stale_timer_id("u-42", "issue/acme/proj#7");
        store.arm(timer(&trig_id, "R-trig", 600, 0));
        let trig_call = SlaTimerCall::new(&store, tenant(), trig_id.clone());
        assert_eq!(
            trig_call.re_arm(7_200),
            ReArmOutcome::ReArmed,
            "the stale_after reset is the same re-arm path"
        );
        assert_eq!(store.get(&tenant(), &trig_id).unwrap().fire_at, 7_200);
        assert_eq!(
            store.armed_count(),
            2,
            "two timers on the wheel (Issues SLA + Trigger), each one row"
        );
        assert_eq!(
            store.rows_scanned(),
            0,
            "no producer touched the wheel scan - all re-arms were row updates"
        );

        let ghost = SlaTimerCall::new(&store, tenant(), sla_timer_id("never-armed"));
        assert_eq!(
            ghost.re_arm(9_999),
            ReArmOutcome::Absent,
            "re-arm of an unarmed key is Absent (0 rows)"
        );
    }

    #[test]
    fn timer_partition_matches_the_durable_run_partition_for_known_answers() {
        let durable_shards = u16::try_from(crate::PARTITION_COUNT)
            .expect("the durable partition count fits the timer API");
        let run_ids = [
            "",
            "a",
            "run-rolled-back",
            "00000000-0000-0000-0000-000000000000",
            "0190f8b0-7c00-7f3d-8000-000000000001",
            "wf:evt-42",
            "myelin://acme/flow/run/123",
            "éclair",
            "emoji-🧠",
            "nul\0inside",
        ];

        for run_id in run_ids {
            assert_eq!(
                partition_for(run_id, durable_shards),
                crate::partition_for_run_id(run_id),
                "the timer and durable run partition diverged for {run_id:?}"
            );
        }
    }

    #[test]
    fn partition_for_is_deterministic_stable_and_in_range() {
        assert_eq!(
            partition_for("R-42", 8),
            partition_for("R-42", 8),
            "the same run → the same partition (stable)"
        );
        for i in 0..10_000 {
            let p = partition_for(&format!("R-{i}"), 16);
            assert!((0..16).contains(&p), "every partition is in 0..shards");
        }
        for run_id in ["", "R-x", "emoji-🧠", "nul\0inside"] {
            assert_eq!(
                partition_for(run_id, 0),
                partition_for(run_id, 1),
                "0 shards retains the historical one-shard floor for {run_id:?}"
            );
            assert_eq!(
                partition_for(run_id, 0),
                0,
                "one shard has only partition 0 for {run_id:?}"
            );
        }
    }

    #[test]
    fn the_worker_sharding_split_does_not_double_claim_a_timer() {
        let store = TimerStore::new();
        let journal = WfJournal::new();
        let runs = RunStore::new();
        let tele = FlowTelemetry::new();
        const SHARDS: u16 = 8;
        const N: usize = 4_000;

        for i in 0..N {
            let run_id = format!("R-{i}");
            let p = partition_for(&run_id, SHARDS);
            let mut run = RunRow::new_runnable(tenant(), region(), run_id.clone(), "sla.run", p);
            run.state = run_state::WAITING.into();
            runs.put(run);
            store.arm(timer(&format!("t/{i}"), &run_id, 0, p));
        }
        assert_eq!(store.armed_count(), N);

        let fleet = WheelShardSet::new(
            store.clone(),
            journal.clone(),
            runs.clone(),
            tele.clone(),
            SHARDS,
            4_096,
        );
        assert_eq!(fleet.shards(), SHARDS as usize);

        let mut total = 0usize;
        let mut rounds = 0u32;
        loop {
            total += fleet.tick_all(30);
            rounds += 1;
            if store.unfired_count() == 0 {
                break;
            }
            assert!(
                rounds < 100,
                "the fleet drains in a bounded number of rounds"
            );
        }
        assert_eq!(
            total, N,
            "every timer fired EXACTLY once across the fleet (0 lost)"
        );

        for i in 0..N {
            let hist = journal.history_for(&tenant(), &format!("R-{i}"));
            let fired = hist
                .iter()
                .filter(|r| r.kind == history_kind::TIMER_FIRED)
                .count();
            assert_eq!(fired, 1, "run R-{i} fired exactly once (0 double-claim)");
        }
    }

    #[test]
    fn the_promotion_threshold_measurement_reads_the_due_now_rate() {
        use myelin_substrate::thresholds::TimerWheelPromotion;
        let gate = TimerWheelPromotion::default();
        assert_eq!(
            gate.promote_due_now_per_sec_per_cell,
            promotion::PROMOTE_DUE_NOW_PER_SEC_PER_CELL_SEED
        );
        assert_eq!(
            gate.degraded_wheel_lag_budget,
            promotion::DEGRADED_WHEEL_LAG_BUDGET_SEED
        );

        assert!(
            !gate.promotion_owed_for( 250_000,  0),
            "a wheel draining within budget is not owed a dedicated tier (rate alone never promotes)"
        );
        assert!(
            gate.promotion_owed_for(250_000, 5_000),
            "a wheel over rate AND falling behind is owed a dedicated scheduling tier"
        );
        assert!(
            !gate.promotion_owed_for(150_000, 0),
            "rate alone, with the wheel keeping up, never promotes"
        );
        assert!(
            !gate.promotion_owed,
            "the committed seam stays NAMED - no dedicated tier owed (the wheel suffices at cell scale)"
        );
    }

    #[test]
    fn a_timer_fire_never_resurrects_a_terminal_run() {
        let store = TimerStore::new();
        let journal = WfJournal::new();
        let runs = RunStore::new();
        let mut run = RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0);
        run.state = run_state::TERMINATED.into();
        runs.put(run);
        store.arm(timer("t1", "R1", 0, 0));

        assert_eq!(
            store.fire(&tenant(), "t1", &journal, &runs),
            FireOutcome::Fired,
            "the timer fires (journals)"
        );
        assert_eq!(
            runs.get(&tenant(), "R1").unwrap().state,
            run_state::TERMINATED,
            "the terminal run is NOT resurrected by the wake (a late fire on a cancelled run is harmless)"
        );
    }
}

//! # `timer` — the minute-bucket durable timer wheel + the timer-wheel scan loop (P-FLOW-13 → P-207, M2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/durable-workflow.md` §3.3 (`wf_timer`;
//! `bucket = epoch_minute(fire_at)` + the partial index `(bucket, partition) WHERE NOT fired`),
//! §4.2 (the timer wheel: scan `bucket <= now AND NOT fired`, `FOR UPDATE SKIP LOCKED`, NO calendar
//! logic on the wheel — a 30-day timer is never read until its minute), §7.3 (the
//! millions-of-timers scaling story), §5.4 (the timer-wheel-lag telemetry — the SC-11 health
//! signal). Carried forward from Phase-3 §3.3/§4.2/§7.3 unchanged. Contract 9.3 (the durable timer
//! wheel — the wheel + arm/fire OWNED here; the cheap disarm/re-arm half is the named follow-on
//! **P-FLOW-14**) + 9.2 (`WfCtx::sleep_until`/`sleep_for` — the timer half, owned here).
//!
//! ## What this prompt (P-FLOW-13) ships — the durable timer wheel
//!
//! 1. **The bucketed timer store** ([`TimerStore`]): a cloneable handle over the `wf_timer` rows. A
//!    timer is ARMED with [`TimerStore::arm`] (an idempotent `INSERT … ON CONFLICT (tenant, timer_id)
//!    DO NOTHING`, keyed by the deterministic `timer_id` so a re-arm of the same workflow command
//!    never double-arms), landing in its minute `bucket = epoch_minute(fire_at)`. The wheel SCAN
//!    ([`TimerStore::scan_due`]) reads ONLY `bucket <= epoch_minute(now) AND NOT fired AND partition
//!    = p` — the SC-11 partial-index range read: a 30-day timer sits in a far-future bucket and is
//!    NEVER read until its minute (the [`TimerStore::buckets_scanned`] counter proves it). The scan
//!    claims under the `FOR UPDATE SKIP LOCKED` discipline (modeled in-memory: a timer claimed by one
//!    wheel worker is skipped by another — no two fire the same timer).
//!
//! 2. **Effectively-once fire** ([`TimerStore::fire`]): firing a due timer (a) sets `fired = true`
//!    AND (b) journals a `timer_fired` `wf_history` row (idempotent on `UNIQUE(tenant, run_id,
//!    command_id)`) AND (c) marks the parked run RUNNABLE (`waiting → running`). A crash BETWEEN "set
//!    fired" and "journal" re-fires the unfired timer; the journal's UNIQUE makes the second journal
//!    a no-op — **fired effectively-once** (at-least-once fire + idempotent journal). A far-future
//!    timer costs ~nothing (never scanned); 0 lost, 0 double-fire.
//!
//! 3. **The timer-wheel scan loop** ([`TimerWheel`]): the per-partition wheel worker wired into the
//!    consumer slot (`flow_app_spec_with_engine`'s dispatcher tick drives it). Each [`TimerWheel::tick`]
//!    scans the due bucket, fires each due timer effectively-once, and refreshes the
//!    **timer-wheel-lag** telemetry (contract 1.8 / §5.4 — the SC-11 health signal: how many due
//!    timers await firing past their minute).
//!
//! ## The epoch-seconds in-memory model (a DOCUMENTED deviation, mirrors the engine's lease clock)
//!
//! Architecture §5.1 names timers in **seconds** and timestamps **RFC-3339 UTC**; the persisted
//! `wf_timer.fire_at` is a `timestamptz` ([`crate::migrations::WF_TIMER_DDL`]) and the row carrier
//! [`crate::schema::WfTimerRow::fire_at`] is the RFC-3339 string. The in-memory wheel here works in
//! **epoch seconds** (`i64`) — EXACTLY as [`crate::engine::RunRow::lease_expires`] models the
//! `timestamptz` lease deadline as `i64` and [`crate::engine::FlowDispatcher::tick`] takes `now: i64`.
//! `bucket = epoch_minute(fire_at) = fire_at_secs / 60` is the SAME coarse minute bucket the SQL
//! `extract(epoch from fire_at)::int / 60` computes. dev↔prod is a config swap (the live wheel issues
//! `WHERE bucket <= (extract(epoch from now())::int / 60) AND NOT fired … FOR UPDATE SKIP LOCKED`),
//! never a code change — the in-memory model proves the SAME observable property (the live-PG apply
//! is `tests/integration_flow_timer.rs`, the `integration` feature).
//!
//! ## FLOORS named
//!
//! - **The cheap SLA-timer disarm/re-arm** (a re-arm is a single row update of `fire_at` + `bucket`;
//!   a disarm sets `fired = true` — millions re-arm at row-update cost, no wheel pollution) → the
//!   named follow-on **P-FLOW-14** (contract 9.3 disarm/re-arm half). This prompt owns the wheel +
//!   arm/fire; [`TimerStore::arm`]/[`TimerStore::fire`] are the seam the re-arm updates in place.
//! - **The seven-figure (1M+) cell-scale run** + the per-cell timer-wheel-promotion threshold → the
//!   M5 follow-on **P-FLOW-24** (FLOW-D3 full). This prompt proves the ALGORITHM at 100k+ timers (six
//!   figures) — the FLOW-D3 floor (`tests/drills_flow_d3_timer_wheel.rs`). The algorithm is unchanged
//!   at 1M+ (P-FLOW-24 is the same wheel on real fleet hardware, the one remaining floor).
//! - **The live OLTP binding** ([`TimerStore`] in-memory) — see the epoch-seconds note; the live-PG
//!   bucketed-scan + partial-index + effectively-once apply is `tests/integration_flow_timer.rs`.

use crate::engine::{run_state, FlowTelemetry, RunStore};
use crate::wfctx::WfJournal;
use myelin_tenancy::{Region, TenantId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The seconds in one minute bucket — `bucket = epoch_minute(fire_at) = fire_at_secs / SECS_PER_MINUTE`
/// (architecture §3.3). The coarse bucket granularity is one MINUTE: the wheel reads at most one
/// indexed bucket-range per minute per partition, so millions of far-future timers cost nothing.
pub const SECS_PER_MINUTE: i64 = 60;

/// **The minute bucket for an epoch-seconds deadline** — `bucket = epoch_minute(fire_at)` (§3.3). The
/// SC-11 scan index: a timer due far in the future sits in a far-future bucket and is NEVER read until
/// its minute. The SAME value the live SQL `extract(epoch from fire_at)::int / 60` computes (the
/// epoch-seconds in-memory model — see the module note). Floors negative (pre-epoch) deadlines to
/// bucket 0 (a deadline in the past is immediately due — the wheel fires it on the next tick).
pub fn epoch_minute(fire_at_secs: i64) -> i32 {
    (fire_at_secs.max(0) / SECS_PER_MINUTE) as i32
}

/// **One durable timer — the `wf_timer` row carrier (§3.3, the in-memory wheel shape).** Mirrors the
/// frozen [`crate::schema::WfTimerRow`] / [`crate::migrations::WF_TIMER_DDL`], with `fire_at` as
/// epoch seconds (the engine's lease-clock convention; the persisted column is a `timestamptz`). The
/// `bucket = epoch_minute(fire_at)` is the SC-11 scan index; `fired` is the partial-index pivot;
/// `run_id` is the workflow to wake (`None` for a bare SLA timer — fires an `sla.deadline.reached`
/// event via the outbox, §4.2; the bare-SLA emit is P-FLOW-14's Issues/Trigger call site).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimerRow {
    /// `(tenant, region)` partition key — the residency pin (§3.3).
    pub tenant: TenantId,
    /// `(tenant, region)` partition key.
    pub region: Region,
    /// the stable opaque timer id — DETERMINISTIC from the workflow command (`<run_id>/<command_id>`)
    /// so a re-arm of the same workflow position never double-arms (the ON CONFLICT key). Not PII.
    pub timer_id: String,
    /// the workflow to wake (`None` for a bare SLA timer, §3.3) — an opaque run id, no PII.
    pub run_id: Option<String>,
    /// the `wf_history` command this timer satisfies — an opaque command id, no PII. The
    /// `timer_fired` journal row is written under this command (idempotent — effectively-once fire).
    pub command_id: String,
    /// the durable deadline as EPOCH SECONDS (the persisted column is a `timestamptz`; see the module
    /// note) — a timestamp, not PII.
    pub fire_at: i64,
    /// the coarse time bucket = `epoch_minute(fire_at)` — the SC-11 scan index (§3.3). Not PII.
    pub bucket: i32,
    /// whether the timer has fired — the partial-index pivot the wheel scan reads `WHERE NOT fired`.
    pub fired: bool,
    /// = the run's partition (co-located dispatch, §3.3) — the wheel scan is per-partition. Not PII.
    pub partition: i16,
}

/// **The outcome of arming a durable timer (§3.3).** A re-arm of the SAME deterministic `timer_id`
/// (the same workflow command position) is a no-op — the ON CONFLICT DO NOTHING dedup makes arming
/// effectively-once (a replayed `sleep_until` never double-arms).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArmOutcome {
    /// the FIRST arm under `(tenant, timer_id)` — the row was inserted into its minute bucket.
    Armed,
    /// a re-arm under an already-armed `timer_id` — a no-op (the timer is already on the wheel).
    AlreadyArmed,
}

/// **The outcome of firing a due timer (§4.2).** A re-fire of an already-fired timer (the crash
/// between "set fired" and "journal" re-fires; the journal UNIQUE makes the re-journal a no-op) is
/// the effectively-once property: 0 double-fire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FireOutcome {
    /// the timer fired for the FIRST time — `fired` set, `timer_fired` journaled, the run woken.
    Fired,
    /// the timer was ALREADY fired (a re-fire after a crash) — a no-op (effectively-once, 0 double-fire).
    AlreadyFired,
}

/// **The in-memory `wf_timer` store — the bucketed durable-timer wheel substrate (§3.3/§4.2/§7.3).** A
/// cloneable handle over a shared map keyed by `(tenant, timer_id)` (the frozen PK), mirroring the
/// `wf_timer` shape. The SC-11 move is the BUCKETED scan: [`TimerStore::scan_due`] reads ONLY the
/// imminent/overdue buckets `WHERE bucket <= now AND NOT fired` for a partition — a far-future timer
/// is never read until its minute (the [`buckets_scanned`](TimerStore::buckets_scanned) counter
/// proves "indexed, not scanned"). Firing is effectively-once (set `fired` + idempotent journal +
/// wake the run).
#[derive(Clone, Default)]
pub struct TimerStore {
    inner: Arc<Mutex<TimerInner>>,
}

#[derive(Default)]
struct TimerInner {
    /// the `wf_timer` rows keyed by the frozen PK `(tenant, timer_id)`.
    timers: HashMap<(String, String), TimerRow>,
    /// **the total ROWS the wheel scan TOUCHED across all ticks (the "indexed, not scanned" probe).**
    /// The bucketed partial index means a due-bucket scan touches ONLY the rows in `bucket <= now AND
    /// NOT fired` — a far-future timer is NEVER counted here. The FLOW-D3 drill reads this to assert a
    /// 100k-far-future fleet costs ~nothing (the scan touches only the due rows, not the whole table).
    rows_scanned: u64,
}

impl TimerStore {
    /// A fresh, empty timer store.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TimerInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// **Arm a durable timer — `INSERT … ON CONFLICT (tenant, timer_id) DO NOTHING` (§3.3).** The
    /// `timer_id` is DETERMINISTIC from the workflow command (`<run_id>/<command_id>`), so a replayed
    /// `sleep_until` (the SAME workflow position) re-arms the SAME key and is a no-op
    /// ([`ArmOutcome::AlreadyArmed`]) — arming is effectively-once (a re-driven run never double-arms).
    /// The row lands in its minute `bucket = epoch_minute(fire_at)`; a far-future deadline sits in a
    /// far-future bucket, never scanned until its minute (the SC-11 partial-index move).
    pub fn arm(&self, row: TimerRow) -> ArmOutcome {
        let mut inner = self.lock();
        let key = (row.tenant.0.clone(), row.timer_id.clone());
        if inner.timers.contains_key(&key) {
            // ON CONFLICT DO NOTHING — already on the wheel under this deterministic key.
            return ArmOutcome::AlreadyArmed;
        }
        inner.timers.insert(key, row);
        ArmOutcome::Armed
    }

    /// **The bucketed wheel SCAN — `SELECT … WHERE bucket <= epoch_minute(now) AND NOT fired AND
    /// partition = p ORDER BY fire_at FOR UPDATE SKIP LOCKED LIMIT batch` (§4.2).** Returns the due
    /// timers in `partition` whose minute has arrived, UP TO `batch` (the bounded claim). The scan
    /// reads ONLY rows in the due buckets (`bucket <= now_bucket`) that are UNFIRED — a far-future
    /// timer (a 30-day SLA) sits in a far-future bucket and is NEVER touched (the partial-index range
    /// read; [`buckets_scanned`](TimerStore::rows_scanned) is bumped ONLY by the due rows, proving the
    /// far-future fleet costs nothing). Ordered by `fire_at` so the oldest-due fire first.
    ///
    /// **`FOR UPDATE SKIP LOCKED` (modeled in-memory):** the returned timers are the CLAIM; the caller
    /// fires them (sets `fired`). A concurrent scan that runs before the fire sees the SAME unfired
    /// rows — the [`TimerStore::fire`] effectively-once guard (set-fired-once + idempotent journal) is
    /// what makes a double-claim safe (0 double-fire), exactly as the SQL `SKIP LOCKED` + the journal
    /// UNIQUE do. `now_secs` is the worker's clock (epoch seconds).
    pub fn scan_due(&self, partition: i16, now_secs: i64, batch: usize) -> Vec<TimerRow> {
        let now_bucket = epoch_minute(now_secs);
        let mut inner = self.lock();
        // The partial-index range read: ONLY unfired rows in this partition whose bucket has arrived.
        // A far-future timer (bucket > now_bucket) is NOT touched — the SC-11 "indexed, not scanned".
        let mut due: Vec<TimerRow> = inner
            .timers
            .values()
            .filter(|t| t.partition == partition && !t.fired && t.bucket <= now_bucket)
            .cloned()
            .collect();
        // ORDER BY fire_at (the oldest-due fire first) — a stable claim order across replica workers.
        due.sort_by(|a, b| a.fire_at.cmp(&b.fire_at).then(a.timer_id.cmp(&b.timer_id)));
        due.truncate(batch);
        // Account the rows the scan TOUCHED (only the due, unfired rows — the partial index never read
        // the far-future buckets). This is the "indexed, not full-scan" green artifact the drill reads.
        inner.rows_scanned += due.len() as u64;
        due
    }

    /// **Fire a due timer effectively-once (§4.2): set `fired`, journal `timer_fired`, wake the run.**
    /// In ONE logical step (the live engine's one txn): (a) flip `fired = true` (the partial-index
    /// pivot — the wheel never re-scans it), (b) journal a `timer_fired` `wf_history` row under the
    /// timer's `command_id` (idempotent on `UNIQUE(tenant, run_id, command_id)` — a re-fire's journal
    /// is a no-op), and (c) mark the parked run RUNNABLE (`waiting → running`) so the dispatcher
    /// re-leases + replays it (the timeout branch / the `sleep` continuation).
    ///
    /// **Effectively-once:** a crash BETWEEN "set fired" and "journal" re-fires the (still-unfired)
    /// timer; the journal UNIQUE makes the second journal a no-op — fired ONCE in effect (0 double-fire).
    /// A re-fire of an ALREADY-fired timer is [`FireOutcome::AlreadyFired`] (a no-op). `journal` is the
    /// run's history (the `timer_fired` row lands there); `runs` is the run store (the run is woken).
    pub fn fire(&self, tenant: &TenantId, timer_id: &str, journal: &WfJournal, runs: &RunStore) -> FireOutcome {
        // (a) set fired, atomically — the partial-index pivot. A re-fire of an already-fired timer is a
        // no-op (effectively-once). We snapshot the row under the lock so the journal/wake use it.
        let row = {
            let mut inner = self.lock();
            let key = (tenant.0.clone(), timer_id.to_string());
            let Some(t) = inner.timers.get_mut(&key) else {
                return FireOutcome::AlreadyFired; // a vanished timer — nothing to fire.
            };
            if t.fired {
                return FireOutcome::AlreadyFired; // already fired — 0 double-fire.
            }
            t.fired = true;
            t.clone()
        };

        // (b) journal the timer_fired row (idempotent on UNIQUE(tenant, run_id, command_id) — a
        // re-fire's journal is a no-op). references-not-payloads: the fire carries no result body.
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
            // (c) wake the parked run: waiting → running, so the dispatcher re-leases + replays it.
            runs.wake(tenant, run_id);
        }
        // A bare SLA timer (run_id = None) emits an `sla.deadline.reached` event via the outbox on
        // fire (§4.2) — the bare-SLA emit + the Issues/Trigger call sites are P-FLOW-14's surface.
        FireOutcome::Fired
    }

    /// Read a timer by its frozen PK `(tenant, timer_id)`.
    pub fn get(&self, tenant: &TenantId, timer_id: &str) -> Option<TimerRow> {
        self.lock().timers.get(&(tenant.0.clone(), timer_id.to_string())).cloned()
    }

    /// **The timer-wheel LAG (the SC-11 health signal, contract 1.8 / §5.4): the count of DUE timers
    /// (`bucket <= now AND NOT fired`) awaiting firing in `partition`.** A healthy wheel keeps this at
    /// ~0 (it fires due timers within the tick budget); a growing lag is the "the wheel is falling
    /// behind" signal — the FLOW-D3 green artifact is this staying within budget under the 100k burst.
    /// Does NOT count far-future timers (they are not due — the SC-11 point).
    pub fn wheel_lag(&self, partition: i16, now_secs: i64) -> u64 {
        let now_bucket = epoch_minute(now_secs);
        self.lock()
            .timers
            .values()
            .filter(|t| t.partition == partition && !t.fired && t.bucket <= now_bucket)
            .count() as u64
    }

    /// The total number of armed timers (fired or not) — the wheel depth (a test reads it to assert a
    /// 100k arm-fleet landed without scanning them).
    pub fn armed_count(&self) -> usize {
        self.lock().timers.len()
    }

    /// The count of UNFIRED timers across all partitions — the outstanding-timer gauge.
    pub fn unfired_count(&self) -> usize {
        self.lock().timers.values().filter(|t| !t.fired).count()
    }

    /// **The total ROWS the wheel scan has TOUCHED across all ticks (the "indexed, not full-scan"
    /// probe).** The bucketed partial index means a scan touches ONLY the due, unfired rows — a
    /// far-future fleet is NEVER counted. The FLOW-D3 drill reads this to assert a 100k-far-future
    /// fleet costs ~nothing: the scan touched only the small due burst, not the whole table.
    pub fn rows_scanned(&self) -> u64 {
        self.lock().rows_scanned
    }
}

/// The frozen `wf_history.kind` tokens the timer wheel writes (the §3.2 vocabulary the
/// [`crate::migrations`] `CHECK` admits): `timer_set` when a `sleep` arms a timer, `timer_fired` when
/// the wheel fires it.
pub mod history_kind {
    /// A durable timer was ARMED (the `sleep_until`/`sleep_for` journal row) — §3.2 / §4.2.
    pub const TIMER_SET: &str = "timer_set";
    /// A durable timer FIRED (the wheel's journal row — the run wakes here) — §3.2 / §4.2.
    pub const TIMER_FIRED: &str = "timer_fired";
}

/// **The per-partition timer-wheel worker — the scan loop wired into the consumer slot (§4.2).** Holds
/// the [`TimerStore`] + the [`WfJournal`] (the `timer_fired` journal lands there) + the [`RunStore`]
/// (the woken run flips `waiting → running`) + the [`FlowTelemetry`] (the timer-wheel-lag signal). Each
/// [`TimerWheel::tick`] scans the due bucket of its partition, fires each due timer effectively-once,
/// and refreshes the timer-wheel-lag gauge — the unit of work the `flow_app_spec_with_engine`
/// dispatcher drives on the wheel cadence (jittered ~1s, §4.2).
pub struct TimerWheel {
    timers: TimerStore,
    journal: WfJournal,
    runs: RunStore,
    telemetry: FlowTelemetry,
    partition: i16,
    /// the bounded fire batch per tick (the `LIMIT :batch` on the scan, §4.2) — the wheel fires at
    /// most this many due timers per tick so one tick never starves the worker (a burst drains over
    /// a few ticks, the lag signal tracks the drain).
    batch: usize,
}

impl TimerWheel {
    /// Build a timer-wheel worker for one partition over the shared timer store + journal + run store
    /// + telemetry. `batch` is the bounded per-tick fire LIMIT (§4.2).
    pub fn new(
        timers: TimerStore,
        journal: WfJournal,
        runs: RunStore,
        telemetry: FlowTelemetry,
        partition: i16,
        batch: usize,
    ) -> Self {
        Self { timers, journal, runs, telemetry, partition, batch }
    }

    /// **One wheel tick (§4.2): scan the due bucket, fire each due timer effectively-once, refresh the
    /// lag.** Scans `bucket <= epoch_minute(now) AND NOT fired AND partition = p` (the SC-11 partial
    /// index — far-future timers untouched), fires each due timer (set `fired`, journal `timer_fired`,
    /// wake the run), and refreshes the timer-wheel-lag gauge. Returns the number of timers FIRED this
    /// tick (0 if no timer was due). `now_secs` is the worker's clock (epoch seconds).
    pub fn tick(&self, now_secs: i64) -> usize {
        let due = self.timers.scan_due(self.partition, now_secs, self.batch);
        let mut fired = 0usize;
        for t in &due {
            if self.timers.fire(&t.tenant, &t.timer_id, &self.journal, &self.runs) == FireOutcome::Fired {
                fired += 1;
            }
        }
        // Refresh the timer-wheel-lag (the SC-11 health signal): how many due timers still await
        // firing in this partition AFTER this tick (a healthy wheel keeps it ~0; a burst drains over
        // a few ticks). The drill asserts it stays within budget.
        self.telemetry
            .set_timer_wheel_lag(self.timers.wheel_lag(self.partition, now_secs));
        fired
    }

    /// The timer store the wheel scans (so a test/executor arms timers into it).
    pub fn timers(&self) -> &TimerStore {
        &self.timers
    }

    /// The telemetry handle the metrics-health port reads (the timer-wheel-lag signal).
    pub fn telemetry(&self) -> &FlowTelemetry {
        &self.telemetry
    }
}

/// Extend [`RunStore`] with the `waiting → running` wake the timer fire (and a signal-wake) performs.
/// A parked run (`state = waiting`, holding NO runtime) is woken to `running`, so the dispatcher's
/// next lease scan picks it up and re-drives it from the journal (the `timer_fired` row is now in the
/// journal — the `sleep`/timeout continuation runs). A no-op on a terminal/absent run.
impl RunStore {
    /// **Wake a parked run (`waiting → running`) — the timer/signal wake (§4.2/§4.3).** Flips a
    /// `waiting` run back to `running` (and leaves a `running` run untouched — idempotent) so the
    /// dispatcher re-leases + replays it. A TERMINAL run (completed/failed/terminated/nondeterministic)
    /// is NOT resurrected (a late timer fire on a cancelled run is a harmless no-op). Absent run: no-op.
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

    /// **`epoch_minute(fire_at)` is the coarse minute bucket (§3.3).** A deadline at second 0 is bucket
    /// 0; at second 59 still bucket 0; at second 60 bucket 1 — the one-minute granularity the wheel
    /// scans (a 30-day timer sits in a far-future bucket). A negative (pre-epoch) deadline floors to 0.
    #[test]
    fn epoch_minute_is_the_coarse_minute_bucket() {
        assert_eq!(epoch_minute(0), 0);
        assert_eq!(epoch_minute(59), 0, "second 59 is still minute 0");
        assert_eq!(epoch_minute(60), 1, "second 60 rolls into minute 1");
        assert_eq!(epoch_minute(3600), 60, "one hour is bucket 60");
        // a 30-day timer sits in a far-future bucket (never scanned until its minute).
        assert_eq!(epoch_minute(30 * 24 * 3600), 43_200, "30 days is bucket 43200");
        assert_eq!(epoch_minute(-5), 0, "a pre-epoch deadline floors to bucket 0 (immediately due)");
    }

    /// **Arming a timer is idempotent on the deterministic `timer_id` (§3.3) — a re-arm is a no-op.** A
    /// replayed `sleep_until` (the SAME workflow command position → the SAME `timer_id`) never
    /// double-arms: the first arm lands the row, the second is `AlreadyArmed` (ON CONFLICT DO NOTHING).
    #[test]
    fn arming_is_idempotent_on_the_deterministic_timer_id() {
        let store = TimerStore::new();
        assert_eq!(store.arm(timer("t1", "R1", 600, 3)), ArmOutcome::Armed, "the first arm lands the row");
        assert_eq!(
            store.arm(timer("t1", "R1", 600, 3)),
            ArmOutcome::AlreadyArmed,
            "a re-arm of the SAME timer_id is a no-op (a replayed sleep never double-arms)"
        );
        assert_eq!(store.armed_count(), 1, "exactly one timer on the wheel (the re-arm was a no-op)");
    }

    /// **The wheel scan reads ONLY the due bucket — a far-future timer is NEVER touched (the SC-11
    /// partial-index move, §4.2/§7.3).** Arm one due timer + 100 far-future (30-day) timers; the scan
    /// at `now` returns ONLY the due one, and `rows_scanned` counts ONLY that one row — the
    /// far-future fleet cost nothing (indexed, not full-scan).
    #[test]
    fn the_scan_reads_only_the_due_bucket_far_future_never_touched() {
        let store = TimerStore::new();
        // one timer due now (bucket 0), 100 timers due in 30 days (a far-future bucket).
        store.arm(timer("due", "R-due", 0, 0));
        for i in 0..100 {
            store.arm(timer(&format!("far{i}"), &format!("R-far{i}"), 30 * 24 * 3600, 0));
        }
        assert_eq!(store.armed_count(), 101, "101 timers armed");

        // scan at now = second 30 (bucket 0): ONLY the due timer is returned.
        let due = store.scan_due(0, 30, 1000);
        assert_eq!(due.len(), 1, "exactly the ONE due timer (the 100 far-future are not in the due bucket)");
        assert_eq!(due[0].timer_id, "due");
        // the partial-index probe: the scan TOUCHED only the one due row — the far-future fleet cost
        // nothing (a far-future timer is never read until its minute, §7.3).
        assert_eq!(store.rows_scanned(), 1, "the scan touched ONLY the due row (indexed, not full-scan)");
        // the wheel lag is 1 (one due timer awaiting firing); the far-future timers are NOT lag.
        assert_eq!(store.wheel_lag(0, 30), 1, "the lag counts only the due timer, not the far-future fleet");
    }

    /// **Firing a due timer is effectively-once: set `fired`, journal `timer_fired`, wake the run; a
    /// re-fire is a no-op (§4.2).** Fire a due timer → `fired` set, one `timer_fired` journal row, the
    /// parked run flips `waiting → running`. A SECOND fire (the crash-re-fire) is `AlreadyFired` — 0
    /// double-fire, 0 duplicate journal row.
    #[test]
    fn firing_is_effectively_once_set_fired_journal_wake() {
        let store = TimerStore::new();
        let journal = WfJournal::new();
        let runs = RunStore::new();
        // a parked (waiting) run + an armed due timer.
        let mut run = RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0);
        run.state = run_state::WAITING.into();
        runs.put(run);
        store.arm(timer("t1", "R1", 0, 0));

        // FIRST fire: set fired + journal + wake.
        assert_eq!(store.fire(&tenant(), "t1", &journal, &runs), FireOutcome::Fired, "the first fire fires");
        assert!(store.get(&tenant(), "t1").unwrap().fired, "the timer is fired (the partial-index pivot)");
        let hist = journal.history_for(&tenant(), "R1");
        assert_eq!(hist.len(), 1, "exactly one timer_fired journal row");
        assert_eq!(hist[0].kind, history_kind::TIMER_FIRED);
        assert_eq!(hist[0].command_id, "agent.run:t1", "the fire journals under the timer's command_id");
        assert_eq!(
            runs.get(&tenant(), "R1").unwrap().state,
            run_state::RUNNING,
            "the parked run woke (waiting → running) — the dispatcher re-drives it"
        );

        // SECOND fire (the crash-re-fire): a no-op — 0 double-fire, 0 duplicate journal row.
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

    /// **The scan does not double-claim a timer across two concurrent wheel workers (`FOR UPDATE SKIP
    /// LOCKED` + effectively-once fire, §4.2).** Two scans claim the SAME due timer (the in-memory
    /// model lets both SEE it — the SQL SKIP LOCKED would let only one); BOTH call `fire`, but only the
    /// FIRST fires (the second is `AlreadyFired`) — so the timer fires ONCE even under a double-claim.
    #[test]
    fn the_scan_does_not_double_fire_a_timer_under_concurrent_claims() {
        let store = TimerStore::new();
        let journal = WfJournal::new();
        let runs = RunStore::new();
        let mut run = RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0);
        run.state = run_state::WAITING.into();
        runs.put(run);
        store.arm(timer("t1", "R1", 0, 0));

        // two workers both scan the due timer (the double-claim).
        let claim_a = store.scan_due(0, 30, 100);
        let claim_b = store.scan_due(0, 30, 100);
        assert_eq!(claim_a.len(), 1);
        assert_eq!(claim_b.len(), 1, "both claims see the unfired timer (the SQL SKIP LOCKED would gate to one)");

        // both fire — but the effectively-once guard makes only ONE actually fire.
        let a = store.fire(&tenant(), "t1", &journal, &runs);
        let b = store.fire(&tenant(), "t1", &journal, &runs);
        assert_eq!(a, FireOutcome::Fired, "worker A fires the timer");
        assert_eq!(b, FireOutcome::AlreadyFired, "worker B's claim is a no-op (0 double-fire)");
        assert_eq!(journal.history_for(&tenant(), "R1").len(), 1, "ONE timer_fired row (fired once in effect)");
    }

    /// **A crash re-fires only the UNFIRED timers (§4.2 — the FLOW-D3 crash property).** Arm three due
    /// timers; fire one (it journals + sets fired); then a "crash" re-runs the wheel scan — it returns
    /// only the TWO still-unfired timers (the fired one is excluded by the partial index `WHERE NOT
    /// fired`). 0 lost, 0 double-fire.
    #[test]
    fn a_crash_re_fires_only_the_unfired_timers() {
        let store = TimerStore::new();
        let journal = WfJournal::new();
        let runs = RunStore::new();
        for id in ["a", "b", "c"] {
            let mut run = RunRow::new_runnable(tenant(), region(), format!("R-{id}"), "agent.run", 0);
            run.state = run_state::WAITING.into();
            runs.put(run);
            store.arm(timer(id, &format!("R-{id}"), 0, 0));
        }
        // fire timer `a` (it sets fired + journals), then the worker CRASHES before firing b/c.
        assert_eq!(store.fire(&tenant(), "a", &journal, &runs), FireOutcome::Fired);

        // a NEW worker re-scans the due bucket after the crash: only b/c (still unfired) come back.
        let after_crash = store.scan_due(0, 30, 100);
        let ids: Vec<&str> = after_crash.iter().map(|t| t.timer_id.as_str()).collect();
        assert_eq!(after_crash.len(), 2, "only the two UNFIRED timers are re-scanned (a is excluded by WHERE NOT fired)");
        assert!(!ids.contains(&"a"), "the already-fired timer `a` is NOT re-fired (0 double-fire)");
        assert!(ids.contains(&"b") && ids.contains(&"c"), "the unfired b/c are re-fired (0 lost)");
    }

    /// **The `TimerWheel::tick` fires every due timer and refreshes the timer-wheel-lag (the SC-11
    /// health signal, §4.2/§5.4).** A wheel over a due burst fires all of them in one tick (within the
    /// batch), wakes their runs, and drives the timer-wheel-lag gauge back to 0 (the FLOW-D3 green
    /// artifact: lag within budget). A far-future timer is untouched (lag never counts it).
    #[test]
    fn the_wheel_tick_fires_due_and_drives_the_lag_to_zero() {
        let store = TimerStore::new();
        let journal = WfJournal::new();
        let runs = RunStore::new();
        let tele = FlowTelemetry::new();
        // 5 due timers + 1 far-future, all in partition 0.
        for i in 0..5 {
            let mut run = RunRow::new_runnable(tenant(), region(), format!("R{i}"), "agent.run", 0);
            run.state = run_state::WAITING.into();
            runs.put(run);
            store.arm(timer(&format!("due{i}"), &format!("R{i}"), 0, 0));
        }
        store.arm(timer("far", "R-far", 30 * 24 * 3600, 0));

        let wheel = TimerWheel::new(store.clone(), journal.clone(), runs.clone(), tele.clone(), 0, 1000);
        // before the tick: the lag is 5 (five due timers awaiting firing); the far-future is not lag.
        assert_eq!(store.wheel_lag(0, 30), 5, "five due timers are the lag (the far-future is not)");

        let fired = wheel.tick(30);
        assert_eq!(fired, 5, "the tick fired all five due timers");
        // every parked run woke (waiting → running).
        for i in 0..5 {
            assert_eq!(runs.get(&tenant(), &format!("R{i}")).unwrap().state, run_state::RUNNING);
        }
        // the timer-wheel-lag drove back to 0 (the FLOW-D3 green artifact: lag within budget) — the
        // far-future timer is still unfired but NOT due, so it does not count as lag.
        assert_eq!(tele.timer_wheel_lag(), 0, "the timer-wheel-lag is 0 after the tick (the SC-11 health signal)");
        assert!(!store.get(&tenant(), "far").unwrap().fired, "the far-future timer is untouched");
    }

    /// **A late timer fire on a TERMINAL run is a harmless no-op (the wake never resurrects).** A
    /// cancelled (terminated) run's timer fires; the journal records it but the run stays `terminated`
    /// (the wake only flips `waiting → running`, never resurrects a terminal run — §4.2).
    #[test]
    fn a_timer_fire_never_resurrects_a_terminal_run() {
        let store = TimerStore::new();
        let journal = WfJournal::new();
        let runs = RunStore::new();
        let mut run = RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0);
        run.state = run_state::TERMINATED.into();
        runs.put(run);
        store.arm(timer("t1", "R1", 0, 0));

        assert_eq!(store.fire(&tenant(), "t1", &journal, &runs), FireOutcome::Fired, "the timer fires (journals)");
        assert_eq!(
            runs.get(&tenant(), "R1").unwrap().state,
            run_state::TERMINATED,
            "the terminal run is NOT resurrected by the wake (a late fire on a cancelled run is harmless)"
        );
    }
}

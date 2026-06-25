//! # `timer` — the minute-bucket durable timer wheel + the cheap disarm/re-arm (P-FLOW-13/14 → P-207/P-210, M2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/durable-workflow.md` §3.3 (`wf_timer`;
//! `bucket = epoch_minute(fire_at)` + the partial index `(bucket, partition) WHERE NOT fired`),
//! §4.2 (the timer wheel: scan `bucket <= now AND NOT fired`, `FOR UPDATE SKIP LOCKED`, NO calendar
//! logic on the wheel — a 30-day timer is never read until its minute), §6.6 (the cheap SLA-timer
//! disarm/re-arm: a re-arm is a row update of `fire_at` + `bucket`, a disarm sets `fired = true` or
//! deletes — millions re-arm at row-update cost, no wheel pollution), §7.3 (the
//! millions-of-timers scaling story), §5.4 (the timer-wheel-lag telemetry — the SC-11 health
//! signal). Carried forward from Phase-3 §3.3/§4.2/§7.3 unchanged. Contract 9.3 (the durable timer
//! wheel — the wheel + arm/fire from **P-FLOW-13/P-207**; the cheap disarm/re-arm half is **P-FLOW-14
//! / P-210**, OWNED here: [`TimerStore::re_arm`] / [`TimerStore::disarm`] / [`TimerStore::disarm_delete`])
//! + 9.2 (`WfCtx::sleep_until`/`sleep_for` — the timer half, owned by P-FLOW-13).
//!
//! ## What P-FLOW-14 (P-210) adds — the cheap SLA-timer disarm/re-arm (§6.6)
//!
//! A re-arm of a precomputed `fire_at` is a SINGLE row UPDATE ([`TimerStore::re_arm`]): rewrite
//! `wf_timer.fire_at` and its derived `bucket = epoch_minute(fire_at)`, re-open the timer
//! (`fired = false`). No new row, no calendar logic on the wheel (the wheel still only scans
//! `bucket <= now AND NOT fired`, §4.2) — re-arming N timers is N row updates, so millions of SLA
//! timers re-arm at row-update cost, not wheel-scan cost (the SC-11 property holds under churn). A
//! disarm ([`TimerStore::disarm`]/[`TimerStore::disarm_delete`]) sets `fired = true` (the partial-index
//! pivot — the wheel never reads it again) or deletes the row, making the timer never fire. These are
//! the documented Issues "stale_after" / SLA-deadline re-arm + SLA-cancel helper calls; the
//! Issues/Trigger CALL SITES are confirmed-and-tested under their producers in **M3, P-FLOW-17**.
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
//! - **The cheap SLA-timer disarm/re-arm** — NOW OWNED here (P-FLOW-14 / P-210): [`TimerStore::re_arm`]
//!   (a single row update of `fire_at` + `bucket`), [`TimerStore::disarm`] (`fired = true`), and
//!   [`TimerStore::disarm_delete`] (delete the row). Millions re-arm at row-update cost, no wheel
//!   pollution. The Issues/Trigger CALL SITES (the `stale_after` / SLA-deadline producers) are the
//!   named follow-on **P-FLOW-17** (M3) — confirmed-and-tested under their producers there.
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

/// **The outcome of a cheap SLA-timer RE-ARM (§6.6, contract 9.3 disarm/re-arm half — P-FLOW-14).** A
/// re-arm of a precomputed `fire_at` is a SINGLE row UPDATE of `wf_timer.fire_at` + its derived
/// `bucket` — **no calendar logic ever pollutes the wheel** (the wheel only scans
/// `bucket <= now AND NOT fired`, §4.2). Millions of SLA timers re-arm at row-update cost, not
/// wheel-scan cost — the SC-11 property holds under churn (the Issues "stale_after" / SLA-deadline
/// re-arm). A re-arm RE-OPENS a fired timer (`fired = false`) and re-buckets it, so a re-armed timer
/// that had already fired is live again at its NEW deadline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReArmOutcome {
    /// the timer's `fire_at` + `bucket` were UPDATED in place (one row touched) — the cheap re-arm.
    ReArmed,
    /// no timer under `(tenant, timer_id)` — the re-arm touched 0 rows (the timer was never armed, or
    /// was disarmed-by-delete). A no-op; the caller re-arms by [`TimerStore::arm`] if it wants the row.
    Absent,
}

/// **The outcome of a cheap SLA-timer DISARM (§6.6, contract 9.3 disarm/re-arm half — P-FLOW-14).** A
/// disarm makes the timer NEVER fire: it sets `fired = true` (the partial-index pivot — the wheel's
/// `WHERE NOT fired` scan never reads it again) OR deletes the row. Either is a single cheap row op —
/// no wheel scan, no calendar logic. The Issues "the SLA was satisfied, cancel the breach timer" call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisarmOutcome {
    /// the timer was DISARMED — `fired = true` set (or the row deleted); it will never fire. One row touched.
    Disarmed,
    /// no timer under `(tenant, timer_id)`, or it had already fired — a no-op (0 rows; nothing to disarm).
    Absent,
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

    /// **Cheap SLA-timer RE-ARM — `UPDATE wf_timer SET fire_at = $new, bucket = epoch_minute($new),
    /// fired = false WHERE (tenant, timer_id) = …` (§6.6, contract 9.3 disarm/re-arm half).** A re-arm
    /// of a precomputed `fire_at` is a SINGLE row UPDATE — it slides the timer's deadline forward (or
    /// back) by rewriting `fire_at` and its DERIVED `bucket = epoch_minute(fire_at)`, and re-opens it
    /// (`fired = false`) so a timer that already fired is live again at its NEW deadline. **No calendar
    /// logic ever pollutes the wheel** — the wheel still only scans `bucket <= now AND NOT fired`
    /// (§4.2). The cost is ONE row update, NOT a wheel rescan: re-arming N timers is N row updates, so
    /// millions of SLA timers re-arm at row-update cost (the SC-11 property holds under churn). The
    /// deterministic `timer_id` is unchanged (the re-arm targets the SAME row — no new row, no
    /// duplicate on the wheel). This is the Issues "stale_after" / SLA-deadline re-arm helper call (the
    /// Issues/Trigger call sites are confirmed-and-tested under their producers in M3, P-FLOW-17).
    pub fn re_arm(&self, tenant: &TenantId, timer_id: &str, new_fire_at: i64) -> ReArmOutcome {
        let mut inner = self.lock();
        let key = (tenant.0.clone(), timer_id.to_string());
        let Some(t) = inner.timers.get_mut(&key) else {
            return ReArmOutcome::Absent; // no row to re-arm — UPDATE … touched 0 rows.
        };
        // The cheap row update: rewrite fire_at + its derived bucket, re-open the timer. NO new row,
        // NO calendar scan — a single field write on the existing row (the SC-11 row-update cost).
        t.fire_at = new_fire_at;
        t.bucket = epoch_minute(new_fire_at);
        t.fired = false;
        ReArmOutcome::ReArmed
    }

    /// **Cheap SLA-timer DISARM — `UPDATE wf_timer SET fired = true WHERE (tenant, timer_id) = … AND
    /// NOT fired` (§6.6, contract 9.3 disarm/re-arm half).** A disarm makes the timer NEVER fire by
    /// setting the partial-index pivot `fired = true` — the wheel's `WHERE NOT fired` scan never reads
    /// it again (the SLA was satisfied, cancel the breach timer). A single cheap row op — no wheel
    /// scan, no calendar logic. A disarm of an already-fired/absent timer is a no-op
    /// ([`DisarmOutcome::Absent`], 0 rows). The disarmed row stays on the table (excluded from the
    /// wheel by the partial index) — [`TimerStore::disarm_delete`] is the delete variant when the row
    /// itself should go. The Issues/Trigger SLA-cancel call site is confirmed under its producer in M3
    /// (P-FLOW-17).
    pub fn disarm(&self, tenant: &TenantId, timer_id: &str) -> DisarmOutcome {
        let mut inner = self.lock();
        let key = (tenant.0.clone(), timer_id.to_string());
        let Some(t) = inner.timers.get_mut(&key) else {
            return DisarmOutcome::Absent; // no row — UPDATE … touched 0 rows.
        };
        if t.fired {
            return DisarmOutcome::Absent; // already fired — nothing to disarm (the wheel never reads it).
        }
        // Set the partial-index pivot: the wheel's `WHERE NOT fired` scan excludes it forever. One row.
        t.fired = true;
        DisarmOutcome::Disarmed
    }

    /// **The DELETE variant of disarm — `DELETE FROM wf_timer WHERE (tenant, timer_id) = …` (§6.6).** A
    /// disarm may delete the row instead of flipping `fired` (the architecture admits either: "sets
    /// `fired = true` or deletes the row"). Use this when the timer should leave no trace (a cancelled
    /// SLA whose run is also gone); use [`TimerStore::disarm`] when the audit row should remain
    /// (excluded from the wheel by the partial index). A single cheap row op — no wheel scan.
    pub fn disarm_delete(&self, tenant: &TenantId, timer_id: &str) -> DisarmOutcome {
        let mut inner = self.lock();
        let key = (tenant.0.clone(), timer_id.to_string());
        match inner.timers.remove(&key) {
            Some(_) => DisarmOutcome::Disarmed,
            None => DisarmOutcome::Absent,
        }
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
    pub fn fire(
        &self,
        tenant: &TenantId,
        timer_id: &str,
        journal: &WfJournal,
        runs: &RunStore,
    ) -> FireOutcome {
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
        self.lock()
            .timers
            .get(&(tenant.0.clone(), timer_id.to_string()))
            .cloned()
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

/// **The cheap SLA-timer disarm/re-arm CALL-SITE helper — the documented Issues (SLA timers) +
/// Trigger (`stale_after`) entry point (§6.6, contract 9.3; P-FLOW-21 M3 confirmation).**
///
/// P-FLOW-14 owns the row-op primitive ([`TimerStore::re_arm`] / [`TimerStore::disarm`]); this module
/// is the M3 CONFIRMATION that those primitives hold UNDER THEIR REAL CALL SITES. Before this, the two
/// producers (Issues' SLA-breach deadline + the Event-Bus stateful Trigger's `stale_after`) constructed
/// the deterministic `timer_id` AD-HOC at the call site (`format!("sla/{issue_key}")` and friends),
/// which is the kind of drift the coherence doctrine (EI-01 §7) warns against — two producers inventing
/// two key schemes for the SAME wheel. This module is the ONE documented helper both call:
///
/// - [`sla_timer_id`] — the deterministic `timer_id` the Issues SLA-breach producer keys on (stable per
///   `(issue_key)` so a re-arm targets the SAME row, never a second one);
/// - [`trigger_stale_timer_id`] — the deterministic `timer_id` the Trigger `stale_after` producer keys
///   on (stable per `(trigger_owner, arms_subject)`, contract 3.3 — the stateful per-person promise);
/// - [`SlaTimerCall::re_arm`] — the documented re-arm helper: a re-arm of a precomputed `fire_at` at the
///   call boundary is a SINGLE row update ([`ReArmOutcome::ReArmed`]); the call site NEVER touches the
///   wheel scan, NEVER adds a row;
/// - [`SlaTimerCall::disarm`] — the documented cancel helper: the SLA was met / the trigger resolved →
///   one row op ([`DisarmOutcome::Disarmed`]), the timer never fires.
///
/// **Both call sites take the SAME `re_arm` path (§6.6):** the Trigger `stale_after` reset and the
/// Issues SLA-deadline slide are the IDENTICAL row update of `fire_at` + derived `bucket` — there is no
/// second code path, no calendar logic on the wheel. The M3 gate is the call-site test proving a re-arm
/// is a single row update at the Issues/Trigger boundary (`tests/cdc_9_3_disarm_rearm.rs` exercises this
/// helper, not ad-hoc keys); the merge-queue holds-no-runtime re-green is the P-FLOW-19 drill re-run
/// (`tests/drills_flow_merge_queue.rs`).
pub mod sla {
    use super::{DisarmOutcome, ReArmOutcome, TimerStore};
    use myelin_tenancy::TenantId;

    /// **The deterministic `timer_id` the Issues SLA-breach producer keys on (§6.6, contract 9.3).** A
    /// stable, PII-free handle per issue — `sla/<issue_key>` — so the FIRST `arm` lands the breach
    /// timer and every later `re_arm` (a comment/label/reassign that slides the deadline) targets the
    /// SAME row (no second wheel row). `issue_key` is the opaque issue ref (e.g. `acme/proj#7`), not PII.
    /// The Issues SLA workflow calls this once at the call boundary — it never builds the key by hand.
    pub fn sla_timer_id(issue_key: &str) -> String {
        format!("sla/{issue_key}")
    }

    /// **The deterministic `timer_id` the Event-Bus stateful Trigger's `stale_after` keys on (§6.6,
    /// contract 3.3).** A stable, PII-free handle per stateful-promise — `trigger/<owner>/<arms_subject>`
    /// — so a Trigger armed for `owner` over `arms_subject` re-arms the SAME `stale_after` row every time
    /// the promise is touched (the `Trigger{owner, condition, arms_subject, on_resolve, stale_after}`
    /// reset), and disarms it on resolve/disarm. `owner`/`arms_subject` are opaque refs, not PII.
    pub fn trigger_stale_timer_id(owner: &str, arms_subject: &str) -> String {
        format!("trigger/{owner}/{arms_subject}")
    }

    /// **The documented SLA-timer call-site helper both Issues (SLA) and Trigger (`stale_after`) call
    /// (§6.6, contract 9.3 — P-FLOW-21 M3 confirmation).** A thin, INTENTIONALLY-trivial wrapper over
    /// the P-FLOW-14 row-op primitives ([`TimerStore::re_arm`] / [`TimerStore::disarm`]) that binds a
    /// `(tenant, timer_id)` so a producer's re-arm/disarm at the call boundary is a SINGLE documented
    /// path — no ad-hoc key construction, no second code path per producer. The whole point is that this
    /// is NOT a new mechanism: it is the M3 confirmation that the existing cheap row op is what the call
    /// sites hit. Hold one per armed SLA/`stale_after` timer; call [`SlaTimerCall::re_arm`] to slide the
    /// deadline (a row update) and [`SlaTimerCall::disarm`] to cancel it (the SLA met / promise resolved).
    pub struct SlaTimerCall<'a> {
        timers: &'a TimerStore,
        tenant: TenantId,
        timer_id: String,
    }

    impl<'a> SlaTimerCall<'a> {
        /// Bind the call helper to the timer store + the `(tenant, timer_id)` the producer keys on
        /// (derive `timer_id` via [`sla_timer_id`] / [`trigger_stale_timer_id`] — never by hand).
        pub fn new(timers: &'a TimerStore, tenant: TenantId, timer_id: impl Into<String>) -> Self {
            Self {
                timers,
                tenant,
                timer_id: timer_id.into(),
            }
        }

        /// The `timer_id` this call helper targets (the stable handle — read by a test/audit).
        pub fn timer_id(&self) -> &str {
            &self.timer_id
        }

        /// **Re-arm the SLA/`stale_after` deadline — a SINGLE row UPDATE at the call boundary (§6.6).**
        /// The Issues SLA-deadline slide + the Trigger `stale_after` reset are the IDENTICAL row op:
        /// rewrite `fire_at` + the derived `bucket`, re-open the timer — NO new row, NO calendar logic on
        /// the wheel. Returns [`ReArmOutcome::ReArmed`] when the row was updated (one row touched),
        /// [`ReArmOutcome::Absent`] when no timer is armed under the key (the producer arms first). The
        /// merge-queue/wheel scan is NEVER touched — re-arming N timers is N row updates (the SC-11
        /// churn property holds under the real call sites).
        pub fn re_arm(&self, new_fire_at: i64) -> ReArmOutcome {
            self.timers
                .re_arm(&self.tenant, &self.timer_id, new_fire_at)
        }

        /// **Disarm the SLA/`stale_after` timer — one cheap row op, the timer never fires (§6.6).** The
        /// Issues "the SLA was satisfied, cancel the breach" + the Trigger "resolved/disarmed" call:
        /// sets the partial-index pivot `fired = true` so the wheel's `WHERE NOT fired` scan never reads
        /// it again. Returns [`DisarmOutcome::Disarmed`] (one row) or [`DisarmOutcome::Absent`] (already
        /// fired / never armed — idempotent, no double-cancel).
        pub fn disarm(&self) -> DisarmOutcome {
            self.timers.disarm(&self.tenant, &self.timer_id)
        }
    }
}

/// **The per-cell timer-wheel-promotion seed constants (§7.3 / OQ #5 — the FLOW-D3-full measurement
/// gate, P-FLOW-26).** The canonical, versioned numbers live in the thresholds file
/// (`myelin_substrate::thresholds::TimerWheelPromotion`); these mirror its seeds so the flow crate that
/// OWNS the wheel carries the same default-to-beat in code (the coherence anchor — one number, two
/// readers). The per-cell promotion threshold is the sustained DUE-NOW rate (timers crossing
/// `bucket <= now` and firing per second) above which the PG-indexed minute-bucket wheel yields to a
/// dedicated scheduling tier; the wheel is "degraded" (the second half of the trigger) when the
/// `timer_wheel_lag` exceeds its budget at that rate. The 1M+ FLOW-D3-full run measures the rate and
/// proves the wheel drains within budget — so the dedicated tier is a NAMED follow-on, owed ONLY if a
/// measured rate demands it (it does not at this commit).
pub mod promotion {
    /// The seed per-cell sustained due-now fire rate (timers/sec) above which the wheel is a promotion
    /// CANDIDATE — mirrors `thresholds::TimerWheelPromotion::promote_due_now_per_sec_per_cell`.
    pub const PROMOTE_DUE_NOW_PER_SEC_PER_CELL_SEED: u64 = 100_000;
    /// The seed `timer_wheel_lag` budget above which the wheel is judged degraded at the candidate rate —
    /// mirrors `thresholds::TimerWheelPromotion::degraded_wheel_lag_budget` (0: any past-minute timer).
    pub const DEGRADED_WHEEL_LAG_BUDGET_SEED: u64 = 0;
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
        Self {
            timers,
            journal,
            runs,
            telemetry,
            partition,
            batch,
        }
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
            if self
                .timers
                .fire(&t.tenant, &t.timer_id, &self.journal, &self.runs)
                == FireOutcome::Fired
            {
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

/// **The worker-shard partition for a run id — `partition = hash(run_id) % shards` (§7.2).** The
/// SAME `partition = hash(run_id) % N` the engine's lease scan uses ([`crate::engine::RunRow::partition`]),
/// so a timer is co-located with its run on ONE shard: the per-partition wheel scan
/// ([`TimerStore::scan_due`] filters `partition = p`) means shard `p` reads ONLY its own timers, and no
/// two shards ever scan the SAME timer (the structural half of "0 double-claim at cell scale", §7.3).
/// Deterministic + stable (the FNV-1a hash) so a run's partition never drifts across restarts. `shards`
/// must be ≥ 1 (a 0-shard fleet is a config error — floored to 1).
pub fn partition_for(run_id: &str, shards: u16) -> i16 {
    let n = shards.max(1) as u64;
    // FNV-1a 64-bit — a stable, well-distributed hash (no std Hasher seed drift across processes).
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in run_id.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (h % n) as i16
}

/// **The per-cell worker-sharding wheel fleet — N per-partition [`TimerWheel`] workers over ONE shared
/// store (§7.2/§7.3, the cell-scale FLOW-D3-full substrate).** The 1M+ cell-scale run shards the wheel
/// by `partition = hash(run_id) % shards`: each [`TimerWheel`] owns ONE partition and scans ONLY its own
/// timers (`scan_due` filters `partition = p`), so the seven-figure outstanding fleet is split across
/// shards and **no two shards ever scan — let alone double-claim — the same timer** (the per-partition
/// scan is the structural guard; the effectively-once [`TimerStore::fire`] is the belt-and-braces guard
/// for the in-process double-claim model). The algorithm is UNCHANGED from P-FLOW-13 — this is the same
/// bucketed wheel, sharded for the cell-scale worker fleet (the 1M+ proof is the SAME indexed range read
/// as the 100k floor).
pub struct WheelShardSet {
    wheels: Vec<TimerWheel>,
    timers: TimerStore,
}

impl WheelShardSet {
    /// Build a `shards`-way wheel fleet over a shared `(timers, journal, runs, telemetry)` — one
    /// [`TimerWheel`] per partition `0..shards`, each with the bounded per-tick fire `batch`. `shards`
    /// must be ≥ 1 (floored to 1). Every shard shares the SAME store so a timer armed under
    /// `partition_for(run_id, shards)` lands on exactly one shard's scan.
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

    /// The number of shards (worker partitions) in this fleet.
    pub fn shards(&self) -> usize {
        self.wheels.len()
    }

    /// **Tick EVERY shard once at `now_secs` — returns the total timers fired across all partitions.**
    /// Each shard scans ONLY its own partition's due bucket (`scan_due` filters `partition = p`), so the
    /// shards partition the work with no overlap: a timer is scanned by exactly the one shard
    /// `partition_for(run_id, shards)` it was armed under. The per-shard fires sum to the total drained
    /// this round; a burst spread across shards drains in parallel (each shard within its own batch).
    pub fn tick_all(&self, now_secs: i64) -> usize {
        self.wheels.iter().map(|w| w.tick(now_secs)).sum()
    }

    /// The shared timer store all shards scan (so a test/executor arms timers into it under
    /// [`partition_for`]).
    pub fn timers(&self) -> &TimerStore {
        &self.timers
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

    /// **Arming a timer is idempotent on the deterministic `timer_id` (§3.3) — a re-arm is a no-op.** A
    /// replayed `sleep_until` (the SAME workflow command position → the SAME `timer_id`) never
    /// double-arms: the first arm lands the row, the second is `AlreadyArmed` (ON CONFLICT DO NOTHING).
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
            store.arm(timer(
                &format!("far{i}"),
                &format!("R-far{i}"),
                30 * 24 * 3600,
                0,
            ));
        }
        assert_eq!(store.armed_count(), 101, "101 timers armed");

        // scan at now = second 30 (bucket 0): ONLY the due timer is returned.
        let due = store.scan_due(0, 30, 1000);
        assert_eq!(
            due.len(),
            1,
            "exactly the ONE due timer (the 100 far-future are not in the due bucket)"
        );
        assert_eq!(due[0].timer_id, "due");
        // the partial-index probe: the scan TOUCHED only the one due row — the far-future fleet cost
        // nothing (a far-future timer is never read until its minute, §7.3).
        assert_eq!(
            store.rows_scanned(),
            1,
            "the scan touched ONLY the due row (indexed, not full-scan)"
        );
        // the wheel lag is 1 (one due timer awaiting firing); the far-future timers are NOT lag.
        assert_eq!(
            store.wheel_lag(0, 30),
            1,
            "the lag counts only the due timer, not the far-future fleet"
        );
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
        assert_eq!(
            claim_b.len(),
            1,
            "both claims see the unfired timer (the SQL SKIP LOCKED would gate to one)"
        );

        // both fire — but the effectively-once guard makes only ONE actually fire.
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
            let mut run =
                RunRow::new_runnable(tenant(), region(), format!("R-{id}"), "agent.run", 0);
            run.state = run_state::WAITING.into();
            runs.put(run);
            store.arm(timer(id, &format!("R-{id}"), 0, 0));
        }
        // fire timer `a` (it sets fired + journals), then the worker CRASHES before firing b/c.
        assert_eq!(
            store.fire(&tenant(), "a", &journal, &runs),
            FireOutcome::Fired
        );

        // a NEW worker re-scans the due bucket after the crash: only b/c (still unfired) come back.
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

        let wheel = TimerWheel::new(
            store.clone(),
            journal.clone(),
            runs.clone(),
            tele.clone(),
            0,
            1000,
        );
        // before the tick: the lag is 5 (five due timers awaiting firing); the far-future is not lag.
        assert_eq!(
            store.wheel_lag(0, 30),
            5,
            "five due timers are the lag (the far-future is not)"
        );

        let fired = wheel.tick(30);
        assert_eq!(fired, 5, "the tick fired all five due timers");
        // every parked run woke (waiting → running).
        for i in 0..5 {
            assert_eq!(
                runs.get(&tenant(), &format!("R{i}")).unwrap().state,
                run_state::RUNNING
            );
        }
        // the timer-wheel-lag drove back to 0 (the FLOW-D3 green artifact: lag within budget) — the
        // far-future timer is still unfired but NOT due, so it does not count as lag.
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

    /// **A re-arm is a SINGLE row UPDATE of `fire_at` + `bucket` — no new row, no wheel rescan
    /// (§6.6, P-FLOW-14).** Arm a timer due in 10 minutes, then re-arm it to 30 minutes: the SAME row
    /// is updated in place (its `fire_at` + derived `bucket` change), the wheel depth stays 1 (no
    /// duplicate row), and the new bucket is `epoch_minute(new_fire_at)` — the cheap row-update cost,
    /// not a wheel scan. A scan at the OLD due time now returns nothing (the row moved forward).
    #[test]
    fn a_re_arm_is_one_row_update_of_fire_at_and_bucket_no_new_row() {
        let store = TimerStore::new();
        // armed due at minute 10 (fire_at = 600s, bucket 10).
        store.arm(timer("sla", "R1", 600, 0));
        assert_eq!(store.armed_count(), 1);
        assert_eq!(store.get(&tenant(), "sla").unwrap().bucket, 10);
        assert_eq!(
            store.rows_scanned(),
            0,
            "no scan yet — the re-arm must not rescan the wheel"
        );

        // re-arm forward to minute 30 (fire_at = 1800s, bucket 30) — a SINGLE row update.
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
            "STILL one row — a re-arm is an UPDATE, not a new INSERT (no wheel pollution)"
        );
        assert_eq!(
            store.rows_scanned(),
            0,
            "the re-arm did NOT scan the wheel (row-update cost, not wheel-scan cost)"
        );

        // a scan at minute 15 (now = 900s) finds NOTHING — the timer moved to minute 30 (far-future).
        assert!(
            store.scan_due(0, 900, 100).is_empty(),
            "the re-armed timer is no longer due at the old time"
        );
    }

    /// **A re-arm RE-OPENS a fired timer at its NEW deadline (§6.6).** A timer that already fired is
    /// live again after a re-arm (`fired = false`), bucketed at the new `fire_at` — the SLA "the issue
    /// re-opened, re-arm the breach timer" case. Re-arming an ABSENT timer is `Absent` (0 rows).
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

        // re-arm re-opens it at a new (future) deadline.
        assert_eq!(store.re_arm(&tenant(), "sla", 1800), ReArmOutcome::ReArmed);
        let row = store.get(&tenant(), "sla").unwrap();
        assert!(
            !row.fired,
            "the re-arm re-opened the fired timer (fired = false)"
        );
        assert_eq!(row.fire_at, 1800);
        assert_eq!(row.bucket, 30);

        // re-arming a timer that was never armed is Absent (UPDATE … touched 0 rows).
        assert_eq!(
            store.re_arm(&tenant(), "ghost", 1800),
            ReArmOutcome::Absent,
            "no row to re-arm"
        );
    }

    /// **Re-arming N timers is N row updates, NOT a wheel rescan (§6.6 — the SC-11 churn property).**
    /// Arm 1000 timers, then re-arm every one: the wheel depth stays 1000 (no new rows), and the scan
    /// counter is UNTOUCHED (re-arm never scans the wheel) — millions re-arm at row-update cost.
    #[test]
    fn re_arming_n_timers_is_n_row_updates_not_a_wheel_rescan() {
        let store = TimerStore::new();
        for i in 0..1000 {
            store.arm(timer(&format!("sla{i}"), &format!("R{i}"), 600, 0));
        }
        assert_eq!(store.armed_count(), 1000);
        assert_eq!(store.rows_scanned(), 0);

        // re-arm all 1000 forward — each is a single row update.
        for i in 0..1000 {
            assert_eq!(
                store.re_arm(&tenant(), &format!("sla{i}"), 1800 + i as i64),
                ReArmOutcome::ReArmed
            );
        }
        assert_eq!(
            store.armed_count(),
            1000,
            "STILL 1000 rows (no duplicates — every re-arm was an in-place UPDATE)"
        );
        assert_eq!(
            store.rows_scanned(),
            0,
            "1000 re-arms scanned the wheel ZERO times (row-update cost, not wheel-scan cost)"
        );
    }

    /// **A disarm (`fired = true`) makes the timer NEVER fire — excluded by the partial index (§6.6).**
    /// Arm a due timer, disarm it, then run the wheel: the disarmed timer is NOT in the due scan
    /// (`WHERE NOT fired` excludes it) and never fires. A disarm of an absent/already-fired timer is a
    /// no-op (`Absent`). The disarmed row remains (the audit trail) but is invisible to the wheel.
    #[test]
    fn a_disarm_sets_fired_and_the_timer_never_fires() {
        let store = TimerStore::new();
        let journal = WfJournal::new();
        let runs = RunStore::new();
        let mut run = RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0);
        run.state = run_state::WAITING.into();
        runs.put(run);
        store.arm(timer("sla", "R1", 0, 0)); // due now.

        // disarm: a single row op — sets fired = true.
        assert_eq!(
            store.disarm(&tenant(), "sla"),
            DisarmOutcome::Disarmed,
            "the disarm sets fired"
        );
        assert!(
            store.get(&tenant(), "sla").unwrap().fired,
            "the disarmed timer's partial-index pivot is set"
        );
        // the row REMAINS on the table (the audit trail) but is invisible to the wheel.
        assert_eq!(
            store.armed_count(),
            1,
            "the disarmed row stays (excluded from the wheel by WHERE NOT fired)"
        );

        // the wheel never sees it: the due-scan excludes the disarmed (fired) timer.
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
            "the wheel fires NOTHING — the disarmed timer never fires"
        );
        assert_eq!(
            journal.history_for(&tenant(), "R1").len(),
            0,
            "no timer_fired row — the disarm cancelled the fire"
        );
        assert_eq!(
            runs.get(&tenant(), "R1").unwrap().state,
            run_state::WAITING,
            "the parked run was never woken"
        );

        // a disarm of an already-disarmed (fired) timer is a no-op; an absent timer too.
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

    /// **The DELETE variant of disarm removes the row entirely (§6.6).** `disarm_delete` deletes the
    /// `wf_timer` row (the no-trace cancel) — the wheel depth drops, the timer never fires, and a
    /// re-delete is `Absent`.
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
            "nothing to fire — the timer was deleted"
        );

        assert_eq!(
            store.disarm_delete(&tenant(), "sla"),
            DisarmOutcome::Absent,
            "a re-delete is a no-op"
        );
    }

    /// **The SLA/`stale_after` call-site helper derives a STABLE deterministic `timer_id` per producer
    /// (§6.6, contract 9.3 — P-FLOW-21).** The Issues SLA-breach key is `sla/<issue_key>`; the Trigger
    /// `stale_after` key is `trigger/<owner>/<arms_subject>`. The SAME issue always yields the SAME key
    /// (so a re-arm targets the SAME row, never a second), and the two producers never collide (distinct
    /// prefixes). No PII in the key (opaque refs only).
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
        // the two producers never collide on the same wheel (distinct prefixes).
        assert_ne!(sla_timer_id("x"), trigger_stale_timer_id("x", "x"));
    }

    /// **The documented call-site helper makes a re-arm a SINGLE row update at the Issues/Trigger
    /// boundary (§6.6, contract 9.3 — the P-FLOW-21 M3 confirmation gate).** A producer binds a
    /// [`SlaTimerCall`] to the wheel + its deterministic key, then re-arms forward via the ONE documented
    /// path: the row is updated in place (its `fire_at` + derived `bucket` slide), the wheel depth stays
    /// 1 (no second row), and the wheel scan is NEVER touched (row-update cost, not wheel-scan cost). A
    /// disarm at the call boundary is one row op (the SLA met → never fire). This is the M3 confirmation
    /// that the existing cheap row op is what the REAL call sites hit — no ad-hoc key, no second path.
    #[test]
    fn the_call_site_helper_re_arm_is_one_row_update_disarm_is_one_row_op() {
        use super::sla::{sla_timer_id, trigger_stale_timer_id, SlaTimerCall};
        let store = TimerStore::new();

        // --- Issues SLA-breach call site ---
        let issue_key = "acme/proj#7";
        let id = sla_timer_id(issue_key);
        // the producer arms the breach ONCE (issue opened; due in 4h = 14_400s, bucket 240).
        store.arm(timer(&id, "R-sla-7", 14_400, 0));
        assert_eq!(store.armed_count(), 1);
        assert_eq!(
            store.rows_scanned(),
            0,
            "no scan yet — the call site must not rescan the wheel"
        );

        let call = SlaTimerCall::new(&store, tenant(), id.clone());
        assert_eq!(call.timer_id(), "sla/acme/proj#7");
        // the issue is touched → the call site SLIDES the deadline forward (one row update).
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
            "STILL one row — the call-site re-arm is an UPDATE, not a new arm"
        );
        assert_eq!(
            store.rows_scanned(),
            0,
            "the call-site re-arm did NOT scan the wheel (row-update cost)"
        );
        // resolution → the call site disarms the breach (one row op, the timer never fires).
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

        // --- Trigger stale_after call site (the SAME documented path) ---
        let trig_id = trigger_stale_timer_id("u-42", "issue/acme/proj#7");
        store.arm(timer(&trig_id, "R-trig", 600, 0));
        let trig_call = SlaTimerCall::new(&store, tenant(), trig_id.clone());
        // the trigger is touched → stale_after RESET (the IDENTICAL re-arm row op as the SLA slide).
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
            "no producer touched the wheel scan — all re-arms were row updates"
        );

        // re-arming an ABSENT key at the call site is Absent (the producer arms first).
        let ghost = SlaTimerCall::new(&store, tenant(), sla_timer_id("never-armed"));
        assert_eq!(
            ghost.re_arm(9_999),
            ReArmOutcome::Absent,
            "re-arm of an unarmed key is Absent (0 rows)"
        );
    }

    /// **`partition_for` is deterministic, stable, and bounded to `0..shards` (§7.2).** The SAME run id
    /// always hashes to the SAME partition (so a run's timers never drift across shards/restarts), every
    /// partition is in range, and a 0-shard fleet floors to a single partition 0 (a config error, never a
    /// panic / out-of-range partition).
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
        // a 0-shard fleet floors to one partition (no divide-by-zero, no out-of-range).
        assert_eq!(partition_for("R-x", 0), 0, "0 shards floors to partition 0");
    }

    /// **The worker-sharding split does NOT double-claim a timer at scale (§7.2/§7.3 — the FLOW-D3-full
    /// unit guard).** Arm a fleet of due timers, each under `partition_for(run_id, shards)`; run the
    /// `WheelShardSet` (every shard scans ONLY its own partition). EVERY timer fires EXACTLY once across
    /// the whole fleet (0 lost), and NO timer fires twice (0 double-claim) — the per-partition scan
    /// partitions the work, and the effectively-once fire is the belt-and-braces guard.
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
            store.arm(timer(&format!("t/{i}"), &run_id, 0, p)); // all due now (bucket 0).
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

        // tick every shard until the whole fleet drains (each shard scans only its own partition).
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

        // 0 double-claim: each run has EXACTLY one timer_fired row (no shard fired another's timer twice).
        for i in 0..N {
            let hist = journal.history_for(&tenant(), &format!("R-{i}"));
            let fired = hist
                .iter()
                .filter(|r| r.kind == history_kind::TIMER_FIRED)
                .count();
            assert_eq!(fired, 1, "run R-{i} fired exactly once (0 double-claim)");
        }
    }

    /// **The promotion-threshold measurement reads the due-now rate (OQ #5 — the FLOW-D3-full gate).** The
    /// per-cell promotion decision is the SAME measurement-gate shape as the column-store seam: given a
    /// MEASURED due-now rate and a MEASURED `timer_wheel_lag`, the gate says whether a dedicated scheduling
    /// tier is owed. A wheel draining within budget (lag 0) at any rate is NOT owed a tier; a wheel falling
    /// behind (lag over budget) AT a rate over the threshold IS. This mirrors
    /// `thresholds::TimerWheelPromotion::promotion_owed_for`; the flow-side seeds match the thresholds file.
    #[test]
    fn the_promotion_threshold_measurement_reads_the_due_now_rate() {
        use myelin_substrate::thresholds::TimerWheelPromotion;
        let gate = TimerWheelPromotion::default();
        // the flow crate's seed mirrors the thresholds file (one number, two readers — the coherence anchor).
        assert_eq!(
            gate.promote_due_now_per_sec_per_cell,
            promotion::PROMOTE_DUE_NOW_PER_SEC_PER_CELL_SEED
        );
        assert_eq!(
            gate.degraded_wheel_lag_budget,
            promotion::DEGRADED_WHEEL_LAG_BUDGET_SEED
        );

        // the 1M+-run posture: a HIGH due-now rate but lag drained to 0 (within budget) → NOT owed a tier.
        assert!(
            !gate.promotion_owed_for(/*rate*/ 250_000, /*lag*/ 0),
            "a wheel draining within budget is not owed a dedicated tier (rate alone never promotes)"
        );
        // a rate OVER the threshold AND a lag OVER budget (the wheel measurably falling behind) → owed.
        assert!(
            gate.promotion_owed_for(/*rate*/ 250_000, /*lag*/ 5_000),
            "a wheel over rate AND falling behind is owed a dedicated scheduling tier"
        );
        // rate over but lag within budget → not owed (the second-half degraded criterion gates it).
        assert!(
            !gate.promotion_owed_for(150_000, 0),
            "rate alone, with the wheel keeping up, never promotes"
        );
        // the committed posture: no production rate has crossed both → promotion not owed.
        assert!(
            !gate.promotion_owed,
            "the committed seam stays NAMED — no dedicated tier owed (the wheel suffices at cell scale)"
        );
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

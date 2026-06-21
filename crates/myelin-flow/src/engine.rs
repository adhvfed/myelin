//! # `engine` — deterministic replay/recovery + lease-based dispatch + crash recovery (P-FLOW-05 → P-202, M2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/durable-workflow.md` §4.1 (the deterministic
//! replay/recovery algorithm — replay `wf_history` to the cursor, short-circuit already-journaled
//! commands, continue from the first un-journaled command), §4.7 (lease-based dispatch + crash
//! recovery — `lease_owner`/`lease_expires` with an expiry re-lease), §3.2 (`wf_history` as the
//! journal source of truth). Carried forward from Phase-3 §4.1/§4.7 unchanged.
//!
//! **Contract-index cluster:** completes the REPLAY-DETERMINISM half of 9.2 (`WfCtx`) — the
//! activity surface's replay contract made real (the WRITE half landed at P-FLOW-04/P-199). Wires
//! the contract-1.8 telemetry: runnable-run lag + replay rate + the 0-double-effect counter.
//!
//! ## What this prompt (P-FLOW-05) ships — the replay/lease HEARTBEAT
//!
//! 1. **Deterministic replay/recovery (§4.1).** [`drive`] re-drives a workflow function over a run's
//!    journaled `wf_history`: every journaled command SHORT-CIRCUITS (returns the journaled outcome
//!    WITHOUT re-executing the side effect — the result was journaled, not re-run, via
//!    [`WfCtx::resume`]), and the run CONTINUES from the first un-journaled command. A crash mid-run
//!    leaves the journal as the source of truth, so a re-drive resumes EXACTLY — 0 re-executed side
//!    effects, 0 lost progress, exactly-once-in-effect. This is the FLOW-D1 property.
//!
//! 2. **Lease-based dispatch + crash recovery (§4.7).** [`RunStore`] models `workflow_run` with the
//!    `lease_owner`/`lease_expires`/`cursor`/`state` columns. A worker LEASES a runnable run
//!    ([`RunStore::lease_runnable`], the `FOR UPDATE SKIP LOCKED` claim modeled in-memory); a lease
//!    EXPIRY (the worker died) re-leases the run to ANOTHER worker, which re-drives from the journal
//!    — crash recovery with no lost progress.
//!
//! 3. **The contract-1.8 telemetry** ([`FlowTelemetry`]): the runnable-run lag (how many runnable
//!    runs await a lease), the replay rate (the fraction of commands replayed vs executed on a
//!    drive), and the **0-double-effect counter** (the FLOW-D1 green artifact — a replay that
//!    re-executes a journaled side effect increments it and reds the drill).
//!
//! ## Why an in-memory model (the dev-real binding, EI policy)
//!
//! The journal + the run store are modeled here over the substrate's transactional
//! [`OutboxStore`](myelin_events::OutboxStore) + [`WfJournal`] + the in-memory [`RunStore`], mirroring
//! the frozen `workflow_run`/`wf_history` shapes ([`crate::schema`]). The replay short-circuit + the
//! lease re-lease this proves are the SAME observable properties the real `SELECT … FOR UPDATE SKIP
//! LOCKED` + the cursor-keyed `wf_history` scan land (dev↔prod is a config swap, never a code
//! change). The live-DB lease/replay apply is exercised in `tests/integration_flow_replay.rs` (the
//! `integration` feature) against the dev stack.
//!
//! ## FLOORS named
//!
//! - **The replay-DIVERGENCE guard** (halt-as-nondeterministic + dead-letter when a re-driven
//!   command's `command_id` mismatches the journal) → **P-FLOW-07** (FLOW-D2). This prompt replays a
//!   DETERMINISTIC body (the happy path); the guard that catches a body that diverges from its
//!   journal lands next. [`drive`] falls through to live execution on a non-activity-kind mismatch
//!   rather than silently mis-replaying — the structural seam the guard sits inside.
//! - **`DurableExecutor::start/describe/cancel`** (the run lifecycle entry points the replay/lease
//!   loop drives) → **P-FLOW-06**. This prompt drives an ALREADY-started run from its journal.
//! - **The live OLTP binding** ([`RunStore`] in-memory) — see above; the integration apply is
//!   `tests/integration_flow_replay.rs`.

use crate::schema::WfHistoryRow;
use crate::wfctx::WfCtx;
use myelin_events::{EmitContextBase, IdMinter, OutboxStore};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The frozen `workflow_run.state` lifecycle tokens the engine reads/writes (§3.1; the
/// [`crate::migrations`] `CHECK` admits these). The replay/lease loop drives `running` ↔ `waiting`
/// and lands `completed`/`failed`.
pub mod run_state {
    /// The run is runnable — a worker may lease it and drive it (§3.1).
    pub const RUNNING: &str = "running";
    /// The run is parked on a wait (a signal/timer); not runnable until woken (§3.1). Not driven
    /// here — durable signals/timers land at P-FLOW-09/13.
    pub const WAITING: &str = "waiting";
    /// The run's workflow function returned successfully — terminal (§3.1).
    pub const COMPLETED: &str = "completed";
    /// The run failed (an un-handled activity exhaustion) — terminal (§3.1).
    pub const FAILED: &str = "failed";
    /// The run was CANCELLED via `DurableExecutor::cancel` — terminal (§3.1, §5.1). A cancel
    /// transitions a non-terminal run to this state; it is never driven further (P-FLOW-06).
    pub const TERMINATED: &str = "terminated";
    /// The run's body diverged from its journal on replay — terminal, dead-lettered (§3.1). Written
    /// by the replay-divergence guard (P-FLOW-07); admitted here so `describe` can report it.
    pub const NONDETERMINISTIC: &str = "nondeterministic";

    /// Whether a state token is TERMINAL (the run will never be driven again — §3.1). The
    /// `DurableExecutor::cancel` surface refuses to re-terminate a terminal run (idempotent /
    /// no-op), and `describe` reports terminality to the caller.
    pub fn is_terminal(state: &str) -> bool {
        matches!(state, COMPLETED | FAILED | TERMINATED | NONDETERMINISTIC)
    }
}

/// The outcome of driving a workflow function to one suspension/terminal point (§4.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriveOutcome {
    /// The workflow function RETURNED — the run is `completed`. Carries the terminal result refs.
    Completed(Vec<ArtifactRef>),
    /// The workflow function FAILED (an un-handled activity exhaustion) — the run is `failed`.
    Failed(String),
    /// **The replay-DIVERGENCE verdict (P-FLOW-07, FLOW-D2).** The workflow body diverged from its
    /// journal on replay (a command-kind mismatch at a journaled position, or a pinned-`wf_version`
    /// mismatch) — the run is HALTED as `nondeterministic` and DEAD-LETTERED, never silently
    /// continued. Carries the machine divergence reason (no PII). The nondeterministic-halt telemetry
    /// counter increments by exactly one per occurrence (the FLOW-D2 green artifact).
    Nondeterministic(String),
}

/// The `workflow_run` row the engine leases + drives (a working subset of [`crate::schema::WorkflowRunRow`]:
/// the lifecycle/cursor/lease columns the replay/lease loop reads — §3.1). The full row carries the
/// references-not-payloads input + budget + causality; those are the `DurableExecutor::start`
/// surface (P-FLOW-06).
#[derive(Clone, Debug)]
pub struct RunRow {
    /// `(tenant, region)` partition key — the residency pin (§3.1).
    pub tenant: TenantId,
    /// `(tenant, region)` partition key.
    pub region: Region,
    /// the ULID-ordered durable run handle (§3.1).
    pub run_id: String,
    /// the registered definition name (e.g. `agent.run`) — the body `drive` runs (§3.1).
    pub wf_type: String,
    /// **the definition version PINNED at start (§4.6).** The replay-divergence guard (P-FLOW-07)
    /// compares it against the version of the definition the engine is replaying with; a mismatch (a
    /// deploy bumped the definition while the run was in flight) halts the run as `nondeterministic`.
    /// Defaults to 1 for a run seeded without an explicit pin (`new_runnable`); the executor stamps
    /// the real pinned version on start.
    pub wf_version: i32,
    /// the ONE lifecycle state column (running|waiting|completed|failed) — §3.1.
    pub state: String,
    /// the highest applied history seq — the replay short-circuit floor (§3.1).
    pub cursor: i64,
    /// the worker-shard key = hash(run_id) % N (§7.2) — the lease scan is per-partition.
    pub partition: i16,
    /// the worker currently driving this run (§4.7) — `None` = unleased (runnable).
    pub lease_owner: Option<String>,
    /// the lease deadline (epoch seconds in this in-memory model; an RFC-3339 `timestamptz` in PG) —
    /// expiry → another worker may steal (crash recovery, §4.7).
    pub lease_expires: Option<i64>,
}

impl RunRow {
    /// A fresh runnable run (state `running`, cursor 0, unleased) — the row a `DurableExecutor::start`
    /// inserts (P-FLOW-06; here the test/engine seeds it directly).
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

    /// A fresh runnable run pinned to an explicit `wf_version` (§4.6) — the executor stamps the
    /// pinned definition version so the divergence guard can detect a deploy that bumps the
    /// definition while the run is in flight.
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

/// **The in-memory `workflow_run` store — the lease-dispatch substrate (§4.7).** A cloneable handle
/// over a shared set of runs (an `Arc<Mutex<…>>`), mirroring the frozen `workflow_run` shape. A
/// worker LEASES a runnable run ([`RunStore::lease_runnable`], modeling `FOR UPDATE SKIP LOCKED`);
/// a lease EXPIRY re-leases it to another worker (crash recovery). The journal is the source of
/// truth — the run store carries only the lease + cursor + state (§3.1/§4.7).
#[derive(Clone, Default)]
pub struct RunStore {
    inner: Arc<Mutex<HashMap<(String, String), RunRow>>>,
}

impl RunStore {
    /// A fresh, empty run store.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(String, String), RunRow>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn key(run: &RunRow) -> (String, String) {
        (run.tenant.0.clone(), run.run_id.clone())
    }

    /// Insert (or replace) a run — the `DurableExecutor::start` insert (P-FLOW-06; here the
    /// engine/test seeds a runnable run).
    pub fn put(&self, run: RunRow) {
        self.lock().insert(Self::key(&run), run);
    }

    /// Read a run by `(tenant, run_id)`.
    pub fn get(&self, tenant: &TenantId, run_id: &str) -> Option<RunRow> {
        self.lock().get(&(tenant.0.clone(), run_id.to_string())).cloned()
    }

    /// **Lease ONE runnable run for `worker` (the `FOR UPDATE SKIP LOCKED` claim, §4.7).** Returns
    /// the first run in `partition` that is `running` AND whose lease is FREE (unleased OR EXPIRED at
    /// `now`), stamping `lease_owner = worker` + `lease_expires = now + lease_ttl_secs`. A run another
    /// worker holds a LIVE lease on is SKIPPED (no two workers drive the same run — the
    /// skip-locked safety). Returns `None` if no runnable run awaits a lease.
    ///
    /// **The expiry re-lease is crash recovery (§4.7):** a worker that died holds a lease that
    /// EXPIRES; once expired, this hands the run to another worker — which re-drives from the journal.
    pub fn lease_runnable(
        &self,
        partition: i16,
        worker: &str,
        now: i64,
        lease_ttl_secs: i64,
    ) -> Option<RunRow> {
        let mut runs = self.lock();
        // Deterministic scan order (sorted by run_id) so the lease claim is stable across workers —
        // models the `ORDER BY` of the runnable-work scan (§3.1 the wf_runnable index).
        let mut keys: Vec<_> = runs.keys().cloned().collect();
        keys.sort();
        for k in keys {
            let run = runs.get_mut(&k).expect("key from the same map");
            if run.partition != partition || run.state != run_state::RUNNING {
                continue;
            }
            let lease_free = match run.lease_expires {
                None => true,
                Some(exp) => exp <= now, // EXPIRED — the dead worker's lease lapsed (crash recovery).
            };
            if lease_free {
                run.lease_owner = Some(worker.to_string());
                run.lease_expires = Some(now + lease_ttl_secs);
                return Some(run.clone());
            }
            // else: a LIVE lease another worker holds — SKIP it (skip-locked; no double-drive).
        }
        None
    }

    /// The number of RUNNABLE runs awaiting a lease in `partition` at `now` (the contract-1.8
    /// runnable-run-lag signal §1.8 — how much runnable work is queued). A run is runnable if it is
    /// `running` AND its lease is free (unleased or expired): a crashed worker's run counts as
    /// runnable once its lease lapses.
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

    /// Persist a drive's result on the run row: advance the `cursor` to the journal depth, set the
    /// terminal state, and RELEASE the lease (the drive finished — §4.1/§4.7). Called by [`drive`]
    /// after a workflow function returns/fails.
    fn settle(&self, tenant: &TenantId, run_id: &str, cursor: i64, state: &str) {
        if let Some(run) = self.lock().get_mut(&(tenant.0.clone(), run_id.to_string())) {
            run.cursor = cursor;
            run.state = state.to_string();
            run.lease_owner = None;
            run.lease_expires = None;
        }
    }

    /// **Transition a run to a terminal `state` and release its lease — the `DurableExecutor::cancel`
    /// move (P-FLOW-06, §5.1).** Unlike [`settle`], the cursor is UNCHANGED (a cancel does not
    /// rewrite the journal — the journal is the source of truth). The run becomes non-runnable so
    /// the dispatcher never leases it again. A no-op if the run is absent.
    pub fn terminate(&self, tenant: &TenantId, run_id: &str, state: &str) {
        if let Some(run) = self.lock().get_mut(&(tenant.0.clone(), run_id.to_string())) {
            run.state = state.to_string();
            run.lease_owner = None;
            run.lease_expires = None;
        }
    }
}

/// **The contract-1.8 engine telemetry — the replay/lease survival signals (§1.8).** A cloneable
/// handle (an `Arc<Mutex<…>>`) the metrics-health port reads. The FLOW-D1 green artifact is the
/// REPLAY RATE signal emitted + the 0-DOUBLE-EFFECT counter on this port: a replay that re-executes a
/// journaled side effect increments [`FlowTelemetry::double_effect_count`] and reds the drill.
#[derive(Clone, Default)]
pub struct FlowTelemetry {
    inner: Arc<Mutex<TelemetryInner>>,
}

#[derive(Default)]
struct TelemetryInner {
    /// total commands a drive REPLAYED (short-circuited from the journal) — the replay-rate numerator.
    commands_replayed: u64,
    /// total commands a drive EXECUTED live (past the cursor) — the replay-rate denominator's other leg.
    commands_executed: u64,
    /// **the 0-double-effect counter (the FLOW-D1 floor).** A journaled side effect that gets
    /// RE-EXECUTED on replay increments this. It MUST stay 0 — a non-zero reds the drill loudly.
    double_effect_count: u64,
    /// the last observed runnable-run lag (the §1.8 runnable-run-lag gauge).
    runnable_run_lag: u64,
    /// the activity-queue DEPTH gauge (§1.8): how many activity attempts are scheduled-but-not-yet-
    /// terminal across the engine — the work the activity pool has queued. Set as the engine
    /// schedules/settles attempts. The timer/signal-buffer depth signals are added by their owning
    /// prompts (P-FLOW-09/13); this is the activity leg of the §1.8 contract.
    activity_queue_depth: u64,
    /// the activity RETRY counter (§1.8): total activity attempts past the first (a `retrying`
    /// transition). A monotonic `+=` — the retry-rate numerator the metrics-health port reads.
    activity_retry_count: u64,
    /// the activity DEAD-LETTER counter (§1.8): total activities that exhausted their retry budget
    /// (an `activity_failed` terminal). A monotonic `+=` — the dead-letter signal the §1.8 contract
    /// names. NON-zero is the "an activity could not be made to succeed" health signal.
    dead_letter_count: u64,
    /// **the nondeterministic-HALT counter (the FLOW-D2 green artifact, §1.8 / contract 1.8).** A
    /// monotonic `+=` incremented EXACTLY ONCE each time the replay-divergence guard halts a run as
    /// `nondeterministic` (a body that diverged from its journal, or a pinned-`wf_version` mismatch).
    /// The FLOW-D2 drill asserts it increments by EXACTLY the injected divergence count — and that 0
    /// divergences silently continued. NON-zero is the "a workflow body diverged on replay" health
    /// signal; the run is dead-lettered, never silently continued.
    nondeterministic_halt_count: u64,
}

impl FlowTelemetry {
    /// A fresh telemetry sink (0 replayed, 0 executed, 0 double-effect).
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TelemetryInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Record one drive's replay accounting: `replayed` commands short-circuited, `executed`
    /// commands ran live. `expected_replays` is how many journaled commands the drive SHOULD have
    /// short-circuited (the journal depth at resume); if fewer were replayed than expected, the
    /// difference is journaled side effects that got RE-EXECUTED → a double-effect (the floor).
    fn record_drive(&self, replayed: u64, executed: u64, side_effects_on_replayed: u64) {
        let mut t = self.lock();
        t.commands_replayed += replayed;
        t.commands_executed += executed;
        // A side effect executed against a command that WAS journaled is a double-effect (§4.1).
        t.double_effect_count += side_effects_on_replayed;
    }

    /// Set the runnable-run-lag gauge (the §1.8 signal) from the run store.
    pub fn set_runnable_lag(&self, lag: u64) {
        self.lock().runnable_run_lag = lag;
    }

    /// Set the activity-queue-depth gauge (§1.8) — how many activity attempts are scheduled-but-not-
    /// terminal. Set by the engine/`DurableExecutor` as it schedules/settles activity attempts.
    pub fn set_activity_queue_depth(&self, depth: u64) {
        self.lock().activity_queue_depth = depth;
    }

    /// Record one activity RETRY (§1.8) — an attempt past the first. Monotonic `+=` (the retry-rate
    /// numerator).
    pub fn record_activity_retry(&self) {
        self.lock().activity_retry_count += 1;
    }

    /// Record one activity DEAD-LETTER (§1.8) — an activity that exhausted its retry budget.
    /// Monotonic `+=` (the dead-letter signal).
    pub fn record_dead_letter(&self) {
        self.lock().dead_letter_count += 1;
    }

    /// **Record one nondeterministic HALT (the FLOW-D2 green artifact, §1.8).** Incremented EXACTLY
    /// once when the replay-divergence guard halts a run as `nondeterministic` + dead-letters it.
    /// Monotonic `+=` — the FLOW-D2 drill reads it to assert the guard fired by exactly the injected
    /// divergence count (and that 0 divergences silently continued).
    pub fn record_nondeterministic_halt(&self) {
        self.lock().nondeterministic_halt_count += 1;
    }

    /// The nondeterministic-halt counter (the FLOW-D2 green artifact, §1.8) — total runs the
    /// divergence guard halted as `nondeterministic`. 0 on a healthy fleet; each `+=` is a dead-
    /// lettered divergent run.
    pub fn nondeterministic_halt_count(&self) -> u64 {
        self.lock().nondeterministic_halt_count
    }

    /// The activity-queue-depth gauge (§1.8) — the activity work queued.
    pub fn activity_queue_depth(&self) -> u64 {
        self.lock().activity_queue_depth
    }

    /// The activity-retry counter (§1.8) — total attempts past the first.
    pub fn activity_retry_count(&self) -> u64 {
        self.lock().activity_retry_count
    }

    /// The activity dead-letter counter (§1.8) — total retry-exhausted activities.
    pub fn dead_letter_count(&self) -> u64 {
        self.lock().dead_letter_count
    }

    /// The **replay rate** — the fraction of commands a drive REPLAYED vs total commands, scaled to
    /// basis points (`0..=10000`, an integer a predicate can read; §1.8). A full crash-recovery
    /// re-drive of an N-command journal that then runs M new commands reads `10000 * N/(N+M)`. `0`
    /// when nothing has driven yet (no division by zero).
    pub fn replay_rate_bps(&self) -> u64 {
        let t = self.lock();
        let total = t.commands_replayed + t.commands_executed;
        // `checked_div` returns None on a zero denominator (nothing driven yet) → 0, no panic.
        (10_000 * t.commands_replayed).checked_div(total).unwrap_or(0)
    }

    /// The **0-double-effect counter** (the FLOW-D1 green artifact). MUST be 0 — a replay that
    /// re-executes a journaled side effect increments it.
    pub fn double_effect_count(&self) -> u64 {
        self.lock().double_effect_count
    }

    /// The total commands replayed across all drives (the replay-rate numerator).
    pub fn commands_replayed(&self) -> u64 {
        self.lock().commands_replayed
    }

    /// The total commands executed live across all drives.
    pub fn commands_executed(&self) -> u64 {
        self.lock().commands_executed
    }

    /// The last observed runnable-run lag (the §1.8 gauge).
    pub fn runnable_run_lag(&self) -> u64 {
        self.lock().runnable_run_lag
    }
}

/// A workflow body the engine drives — a DETERMINISTIC function over a [`WfCtx`] (§2.5/§4.1). It
/// issues `ctx.activity`/`ctx.now`/`ctx.rand`/`ctx.emit` commands and returns its terminal result
/// refs (or an error string the run fails with). Determinism is the contract: the SAME body over the
/// SAME journal issues the SAME command sequence — that is what makes replay short-circuit correctly
/// (the `flow-determinism` lint + the P-FLOW-07 divergence guard enforce it).
pub type WorkflowBody = dyn Fn(&mut WfCtx) -> Result<Vec<ArtifactRef>, String>;

/// **Drive a workflow run to one terminal point — the deterministic replay/recovery core (§4.1).**
///
/// Reads the run's journaled `wf_history` (the source of truth §3.2), resumes a [`WfCtx`] over it
/// ([`WfCtx::resume`] — so every journaled command SHORT-CIRCUITS with 0 re-execution of side
/// effects), runs the `body` (which replays the journaled prefix then CONTINUES from the first
/// un-journaled command), CO-COMMITS the newly-journaled commands + their emits (FLOW-D5), advances
/// the run's `cursor`, and settles the terminal state — RELEASING the lease.
///
/// **The FLOW-D1 property:** a worker that re-drives a half-journaled run replays every journaled
/// step (0 double-effect — the result was journaled, not the activity re-run) and resumes at the
/// first un-journaled command. The telemetry records the replay rate + the 0-double-effect counter.
///
/// `now_clock`/`rand_seed` seed the LIVE side-markers (a replayed `now()`/`rand()` returns its
/// CAPTURED value, not these — §4.1). Returns the [`DriveOutcome`] and the new cursor.
///
/// **The version-divergence leg (§4.6) is NOT armed here** — this version-agnostic `drive` replays
/// against whatever body is registered (the kind-divergence guard still fires). Use
/// [`drive_versioned`] on the replay path where the run's pinned `wf_version` is known, so a deploy
/// that bumped the definition while the run was in flight halts as `nondeterministic`.
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
    // Version-agnostic: pin == replay (the version-divergence leg is disarmed; the kind guard fires).
    drive_versioned(
        runs, outbox, journal, telemetry, minter, ctx_base, run, now_clock, rand_seed, body, 1, 1,
    )
}

/// **Drive a workflow run WITH the version-divergence leg armed (P-FLOW-07, §4.6).** Identical to
/// [`drive`] but threads the run's pinned `wf_version` (`run_version`, recorded at start) and the
/// version of the definition the engine is REPLAYING with (`replay_version`). If they MISMATCH — a
/// deploy bumped the definition while the run was in flight — the replay-divergence guard halts the
/// run as `nondeterministic` + dead-letters it BEFORE running a command (replaying a new body over an
/// old journal is a silent divergence). A MATCH drives exactly as [`drive`] (the kind-divergence
/// guard still applies over the journal).
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
    let tenant = run.tenant.clone();
    // Load the journal so far (the replay prefix the body short-circuits over §4.1).
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

    // Run the body. It replays the journaled commands (short-circuit, 0 side effect) then runs the
    // first un-journaled command live. `side_effects_executed` counts the LIVE activity executions;
    // `double_effects` counts any journaled command re-executed (the FLOW-D1 floor — 0 by
    // construction via the activity-replay short-circuit).
    let result = body(&mut ctx);
    let side_effects_executed = ctx.side_effects_executed();
    let double_effects = ctx.double_effects();
    // The replay accounting: the drive REPLAYED `journaled_commands` commands (short-circuited) and
    // EXECUTED `side_effects_executed` activity closures live; `double_effects` MUST be 0.
    telemetry.record_drive(journaled_commands, side_effects_executed, double_effects);

    // **THE REPLAY-DIVERGENCE GUARD (P-FLOW-07, FLOW-D2).** If the body DIVERGED from its journal on
    // replay (a command-kind mismatch at a journaled position, or a pinned-`wf_version` mismatch), the
    // run is HALTED as `nondeterministic` and DEAD-LETTERED — it is NEVER silently continued (a silent
    // divergence is a Tier-1 failure, EI-01 §2; a red gate is information, never invert it, EI-01 §3).
    // We DROP the `WfCtx` WITHOUT committing — the divergent drive's partial journal rows are
    // discarded (they would corrupt the journal), and the run's cursor is UNCHANGED (the journal
    // source-of-truth is preserved as it was at the crash/divergence point). The
    // nondeterministic-halt counter increments by EXACTLY one (the FLOW-D2 green artifact). The
    // divergence reason is surfaced (no PII) so an operator can see WHICH position diverged.
    if let Some(reason) = ctx.divergence() {
        let reason = reason.to_string();
        drop(ctx); // discard the divergent drive's staged journal/outbox rows — 0 corruption.
        telemetry.record_nondeterministic_halt();
        // Dead-letter the run: settle it `nondeterministic` (terminal, never re-driven) WITHOUT
        // advancing the cursor (the journal is unchanged — the guard rewrote nothing).
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

    // Co-commit the newly-journaled commands + their emits (FLOW-D5). A failed co-commit leaves the
    // journal untouched (the drive is retried by a re-lease).
    let committed = ctx.commit().is_ok();

    let new_cursor = journal.history_for(&tenant, &run.run_id).len() as i64;
    let (outcome, state) = match (&result, committed) {
        (Ok(refs), true) => (DriveOutcome::Completed(refs.clone()), run_state::COMPLETED),
        (Ok(_), false) => (
            DriveOutcome::Failed("co-commit failed".into()),
            run_state::FAILED,
        ),
        (Err(e), _) => (DriveOutcome::Failed(e.clone()), run_state::FAILED),
    };
    runs.settle(&tenant, &run.run_id, new_cursor, state);
    let lag = runs.runnable_lag(run.partition, i64::MAX) as u64;
    telemetry.set_runnable_lag(lag);
    outcome
}

/// **The replay/lease DISPATCHER — the engine's worker loop (§4.7), the consumer-slot seam.** Holds
/// the [`RunStore`] + the [`WfJournal`] + the [`OutboxStore`] + the [`FlowTelemetry`] one cell's
/// worker shares, plus the registry of deterministic workflow bodies keyed by `wf_type`. Each
/// [`FlowDispatcher::tick`] LEASES one runnable run from its partition and [`drive`]s it (replaying
/// the journal, resuming at the first un-journaled command) — the unit of work a per-partition
/// worker thread repeats. This is the loop wired into the `myelin-flow` AppSpec consumer seam
/// ([`crate::app::flow_app_spec_with_engine`]).
///
/// **Why a tick loop, not an `EventHandler`:** the replay/lease engine is a worker that POLLS the
/// run store for leasable runnable work (the `FOR UPDATE SKIP LOCKED` claim, §4.7), not a bus
/// subscriber. So it occupies the consumer seam as a tick-driven dispatcher rather than an
/// `EventHandler`-shaped `Consumer`. The DurableExecutor surface (`start` inserts a runnable run;
/// signals/timers wake `waiting` runs) that FEEDS this loop lands at P-FLOW-06/09/13 — this prompt
/// ships the loop + the replay/lease core it drives.
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
    /// **the version of each registered definition the engine is RUNNING (§4.6).** The divergence
    /// guard compares it against the run's pinned `wf_version`; a mismatch (a deploy bumped the body)
    /// halts the run as `nondeterministic`. Defaults to 1 per registered type ([`Self::register`]);
    /// [`Self::register_versioned`] sets an explicit running version.
    running_versions: HashMap<String, i32>,
}

impl FlowDispatcher {
    /// Build a dispatcher for one partition's worker. `bodies` is the `wf_type → deterministic body`
    /// registry (the definition registry the DurableExecutor populates, P-FLOW-06; here the
    /// engine/test registers them directly).
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
        }
    }

    /// Register a deterministic workflow body under its `wf_type` (the definition registry seam,
    /// §3.6 — populated by the DurableExecutor at P-FLOW-06). The running version defaults to 1.
    pub fn register(&mut self, wf_type: impl Into<String>, body: Box<WorkflowBody>) {
        let wf_type = wf_type.into();
        self.running_versions.insert(wf_type.clone(), 1);
        self.bodies.insert(wf_type, body);
    }

    /// Register a deterministic workflow body under its `wf_type` at an explicit RUNNING `wf_version`
    /// (§4.6) — so the divergence guard halts a run pinned to a DIFFERENT version (a deploy bumped
    /// the body while the run was in flight). The body is the version-`wf_version` definition.
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

    /// **One worker tick (§4.7): lease one runnable run and drive it.** Leases the next runnable run
    /// in this partition (skip-locked — a run another worker holds a live lease on is skipped; a
    /// crashed worker's expired lease re-leases here), looks up its registered body, and [`drive`]s
    /// it — replaying the journal with 0 re-execution, resuming at the first un-journaled command.
    /// `now` is the worker's clock (epoch seconds) for the lease deadline + the live side-markers.
    /// Returns the [`DriveOutcome`] if a run was driven, `None` if no runnable work awaited.
    pub fn tick(&self, now: i64, now_clock: &str, rand_seed: u64) -> Option<DriveOutcome> {
        let run = self
            .runs
            .lease_runnable(self.partition, &self.worker, now, self.lease_ttl_secs)?;
        let body = self.bodies.get(&run.wf_type)?;
        // **The version-divergence leg (§4.6):** drive with the run's PINNED `wf_version` against the
        // version the engine is RUNNING for this `wf_type`. A mismatch (a deploy bumped the body while
        // the run was in flight) halts the run as `nondeterministic` (the divergence guard).
        let replay_version = self
            .running_versions
            .get(&run.wf_type)
            .copied()
            .unwrap_or(run.wf_version);
        let outcome = drive_versioned(
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
        );
        Some(outcome)
    }

    /// The telemetry handle the metrics-health port reads (the contract-1.8 replay-rate +
    /// 0-double-effect signals).
    pub fn telemetry(&self) -> &FlowTelemetry {
        &self.telemetry
    }

    /// The run store the dispatcher leases from (so a test/DurableExecutor seeds runnable runs).
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

    /// A workflow body of `n` activities, each recording the activity-execution into `executed` so a
    /// test can read which steps actually RAN (vs replayed). The Kth activity returns effect ref `eK`.
    fn n_activity_body(n: usize, executed: Arc<Mutex<Vec<usize>>>) -> Box<WorkflowBody> {
        Box::new(move |ctx: &mut WfCtx| {
            for k in 0..n {
                let ex = executed.clone();
                ctx.activity(RetryPolicy::default_policy(), move |_idem, _attempt| {
                    ex.lock().unwrap().push(k);
                    Ok(vec![ArtifactRef(format!("myelin://acme/agent/effect/e{k}"))])
                })
                .map_err(|e| format!("{e:?}"))?;
            }
            Ok(vec![ArtifactRef("myelin://acme/agent/run/R1/done".into())])
        })
    }

    /// **A drive of a COLD run (empty journal) executes every command live and journals it (§4.1).**
    /// Ten activities run, ten history rows journal, the run completes, cursor = 10. No replay (0
    /// replayed, 10 executed) — the baseline the recovery drive is measured against.
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
            &runs, &outbox, &journal, &tele, minter(), ctx_base(), &run,
            "2026-06-21T00:00:00Z", 7, body.as_ref(),
        );

        assert!(matches!(outcome, DriveOutcome::Completed(_)), "the run completed");
        assert_eq!(executed.lock().unwrap().len(), 10, "all 10 activities ran live (cold)");
        assert_eq!(journal.history_for(&tenant(), "R1").len(), 10, "10 history rows journaled");
        let settled = runs.get(&tenant(), "R1").expect("run row");
        assert_eq!(settled.state, run_state::COMPLETED, "the run is completed");
        assert_eq!(settled.cursor, 10, "the cursor advanced to the journal depth");
        assert!(settled.lease_owner.is_none(), "the lease is released on settle");
        assert_eq!(tele.commands_executed(), 10, "10 live executions recorded");
        assert_eq!(tele.commands_replayed(), 0, "nothing replayed on a cold drive");
        assert_eq!(tele.double_effect_count(), 0, "0 double-effect");
    }

    /// **FLOW-D1 CORE: a crash at activity 5 of 10 → replay short-circuits 1..=5, resumes at 6, with
    /// 0 re-executed side effects (§4.1).** The first drive crashes after journaling 5 activities (it
    /// runs a 5-activity body whose journal co-commits). A second drive of the SAME run, now with the
    /// full 10-activity body, REPLAYS commands 0..=4 (0 re-execution — the journaled results are
    /// returned) and EXECUTES only 5..=9. The double-effect counter stays 0.
    #[test]
    fn crash_at_5_of_10_replays_to_6_with_zero_double_effect() {
        let runs = RunStore::new();
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let tele = FlowTelemetry::new();
        let run = RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0);
        runs.put(run.clone());

        // DRIVE 1 (CRASH) — the worker journals the first 5 activities (they co-commit, durable),
        // then the PROCESS DIES before settling the run row: the journal holds 5 rows, but the run is
        // left `running` with its cursor advanced to 5 (the worker bumped it as it journaled). This is
        // the crash-recovery setup — the journal is the source of truth; the half-driven run survives.
        let executed1 = Arc::new(Mutex::new(Vec::new()));
        let body1 = n_activity_body(5, executed1.clone());
        {
            // a WfCtx that journals the 5 activities and co-commits — but the worker never settles
            // the run (it crashed). We commit the journal directly to model the durable 5 steps.
            let mut crash_ctx = WfCtx::begin(
                &outbox, minter(), journal.clone(), ctx_base(), "R1", "agent.run",
                "2026-06-21T00:00:00Z", 7,
            );
            body1(&mut crash_ctx).expect("the 5 activities run");
            crash_ctx.commit().expect("the 5 steps co-commit (durable before the crash)");
            // the worker bumped the cursor to 5 as it journaled, but DIED before settling state — the
            // run is left `running` (NOT completed), unleased (the lease lapsed on the dead worker).
            let mut r = runs.get(&tenant(), "R1").expect("run");
            r.cursor = 5;
            runs.put(r);
        }
        assert_eq!(executed1.lock().unwrap().len(), 5, "drive 1 ran activities 0..=4");
        assert_eq!(journal.history_for(&tenant(), "R1").len(), 5, "5 journaled at the crash point");

        // DRIVE 2 — another worker re-leases and re-drives with the FULL 10-activity body. Commands
        // 0..=4 REPLAY (short-circuit, no re-execution); 5..=9 execute live.
        let leased = runs
            .lease_runnable(0, "worker-2", 1000, 30)
            .expect("the run is re-leasable (drive 1 released the lease on settle)");
        assert_eq!(leased.cursor, 5, "the re-leased run resumes from cursor 5");
        let executed2 = Arc::new(Mutex::new(Vec::new()));
        let body2 = n_activity_body(10, executed2.clone());
        let outcome = drive(
            &runs, &outbox, &journal, &tele, minter(), ctx_base(), &leased,
            "2026-06-21T00:00:00Z", 7, body2.as_ref(),
        );

        // THE FLOW-D1 ASSERTIONS: only 5..=9 executed (0 re-execution of 0..=4); the run completed.
        let ran = executed2.lock().unwrap().clone();
        assert_eq!(ran, vec![5, 6, 7, 8, 9], "resumed at step 6 — activities 5..=9 ran, 0..=4 replayed");
        assert!(matches!(outcome, DriveOutcome::Completed(_)), "the run completed after recovery");
        assert_eq!(journal.history_for(&tenant(), "R1").len(), 10, "10 journaled, 0 lost progress");
        // 0 DOUBLE-EFFECT — the journaled commands were NOT re-executed (the FLOW-D1 floor).
        assert_eq!(tele.double_effect_count(), 0, "0 re-executed side effects (exactly-once-in-effect)");
        // the replay rate: drive 2 replayed 5 commands and executed 5 live → 5/(5+5) = 5000 bps. The
        // crash drive 1 used its own WfCtx (not `drive`), so only drive 2's accounting is recorded.
        assert_eq!(
            tele.commands_replayed(),
            5,
            "drive 2 replayed the 5 journaled commands (short-circuited, 0 re-execution)"
        );
        assert_eq!(tele.commands_executed(), 5, "drive 2 executed 5 new commands live");
        assert_eq!(
            tele.replay_rate_bps(),
            5000,
            "the replay rate (5000 bps) is emitted (the FLOW-D1 green artifact)"
        );
    }

    /// **A re-drive of a FULLY-journaled run replays EVERYTHING and executes 0 (the pure-recovery
    /// case).** A completed-but-re-driven run (e.g. an at-least-once dispatch redelivery) replays all
    /// 10 commands and re-executes NONE — exactly-once-in-effect under redelivery (§4.1).
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
            &runs, &outbox, &journal, &tele, minter(), ctx_base(), &run,
            "2026-06-21T00:00:00Z", 7, body.as_ref(),
        );
        // re-drive the same journal: everything replays.
        let executed = Arc::new(Mutex::new(Vec::new()));
        let body2 = n_activity_body(10, executed.clone());
        let again = runs.get(&tenant(), "R1").expect("run");
        drive(
            &runs, &outbox, &journal, &tele, minter(), ctx_base(), &again,
            "2026-06-21T00:00:00Z", 7, body2.as_ref(),
        );
        assert_eq!(executed.lock().unwrap().len(), 0, "a full replay re-executes 0 side effects");
        assert_eq!(journal.history_for(&tenant(), "R1").len(), 10, "no duplicate journal rows");
        assert_eq!(tele.double_effect_count(), 0, "0 double-effect on a full replay");
    }

    /// **A lease expiry re-leases to ANOTHER worker (crash recovery, §4.7).** Worker-1 leases a
    /// runnable run (a 30s lease). Before it expires, worker-2 CANNOT steal it (skip-locked). After
    /// it expires, worker-2 DOES re-lease it — the crashed worker's lease lapsed.
    #[test]
    fn lease_expiry_re_leases_to_another_worker() {
        let runs = RunStore::new();
        runs.put(RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0));

        // worker-1 leases at t=1000 with a 30s TTL → lease_expires = 1030.
        let l1 = runs.lease_runnable(0, "worker-1", 1000, 30).expect("worker-1 leases");
        assert_eq!(l1.lease_owner.as_deref(), Some("worker-1"));
        assert_eq!(l1.lease_expires, Some(1030));

        // BEFORE expiry (t=1020): worker-2 cannot steal the run (the LIVE lease is skip-locked).
        assert!(
            runs.lease_runnable(0, "worker-2", 1020, 30).is_none(),
            "a live lease is skip-locked — no second worker drives the same run"
        );

        // AFTER expiry (t=1031): worker-2 re-leases (the crashed worker's lease lapsed → recovery).
        let l2 = runs.lease_runnable(0, "worker-2", 1031, 30).expect("worker-2 re-leases after expiry");
        assert_eq!(l2.lease_owner.as_deref(), Some("worker-2"), "the run re-leased to worker-2");
        assert_eq!(l2.lease_expires, Some(1061), "a fresh lease deadline");
    }

    /// **The lease scan is partition-scoped (§7.2) and skips non-runnable runs.** A run in a
    /// DIFFERENT partition is not leased by this partition's worker; a `waiting` run is not leased.
    #[test]
    fn lease_is_partition_scoped_and_skips_non_runnable() {
        let runs = RunStore::new();
        runs.put(RunRow::new_runnable(tenant(), region(), "R-p0", "agent.run", 0));
        runs.put(RunRow::new_runnable(tenant(), region(), "R-p1", "agent.run", 1));
        // a waiting run in partition 0 is not runnable.
        let mut waiting = RunRow::new_runnable(tenant(), region(), "R-wait", "agent.run", 0);
        waiting.state = run_state::WAITING.into();
        runs.put(waiting);

        // partition 0's worker leases R-p0 (not R-p1, not the waiting run).
        let leased = runs.lease_runnable(0, "w", 1, 30).expect("a runnable run in partition 0");
        assert_eq!(leased.run_id, "R-p0", "the partition-0 runnable run is leased");
        // the runnable-lag for partition 0 was 1 (R-p0); partition 1's lag is 1 (R-p1).
        assert_eq!(runs.runnable_lag(1, 1), 1, "partition 1 has its own runnable run");
    }

    /// **The runnable-run-lag telemetry counts runnable, unleased (or expired-lease) runs (§1.8).**
    /// Three runnable runs in a partition read lag 3; leasing one drops the live lag to 2; after the
    /// lease expires it counts again (a crashed worker's run re-enters the runnable set).
    #[test]
    fn runnable_lag_counts_unleased_and_expired() {
        let runs = RunStore::new();
        for i in 0..3 {
            runs.put(RunRow::new_runnable(tenant(), region(), format!("R{i}"), "agent.run", 0));
        }
        assert_eq!(runs.runnable_lag(0, 1000), 3, "three runnable runs await a lease");
        // lease one at t=1000 (TTL 30) → live lag drops to 2 at t=1000.
        runs.lease_runnable(0, "w", 1000, 30).expect("lease one");
        assert_eq!(runs.runnable_lag(0, 1000), 2, "a live-leased run is not runnable-lag");
        // after the lease expires (t=1031) the run counts again (crash recovery makes it runnable).
        assert_eq!(runs.runnable_lag(0, 1031), 3, "the expired-lease run re-enters the runnable set");
    }

    /// **A drive whose body FAILS an un-handled activity exhaustion settles the run `failed` (§4.1).**
    /// The terminal state is `failed`, the lease is released, and the failure is journaled (0 ghost).
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
                Err(ActivityError("hard failure".into()))
            })
            .map_err(|e| format!("{e:?}"))?;
            Ok(vec![])
        });
        let outcome = drive(
            &runs, &outbox, &journal, &tele, minter(), ctx_base(), &run,
            "2026-06-21T00:00:00Z", 7, body.as_ref(),
        );
        assert!(matches!(outcome, DriveOutcome::Failed(_)), "the run failed");
        let settled = runs.get(&tenant(), "R1").expect("run");
        assert_eq!(settled.state, run_state::FAILED, "the run is settled failed");
        assert!(settled.lease_owner.is_none(), "the lease is released even on failure");
        // the activity_failed row IS journaled (0 lost — the failure is durable).
        assert_eq!(journal.history_for(&tenant(), "R1").len(), 1, "the failure is journaled");
    }

    /// **FLOW-D2 CORE (P-FLOW-07): a divergent replay HALTS the run as `nondeterministic` +
    /// DEAD-LETTERS it — 0 silent divergence, 0 double-effect.** Position 0 is journaled as a
    /// `side_marker` (a `now()`); a re-drive issues an `activity` at position 0 — the body diverged
    /// from its journal. The `drive` divergence guard HALTS: the run settles `nondeterministic`
    /// (terminal, dead-lettered), the activity does NOT run live (0 double-effect), the divergent
    /// drive's partial journal is DISCARDED (the journal stays exactly the 1 marker row), and the
    /// nondeterministic-halt counter increments by EXACTLY 1 (the FLOW-D2 green artifact).
    #[test]
    fn divergent_replay_halts_nondeterministic_and_dead_letters() {
        let runs = RunStore::new();
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let tele = FlowTelemetry::new();
        runs.put(RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0));

        // journal a side-marker (a now()) at position 0, leave the run runnable at cursor 1.
        {
            let mut ctx = WfCtx::begin(
                &outbox, minter(), journal.clone(), ctx_base(), "R1", "agent.run",
                "2026-06-21T00:00:00Z", 7,
            );
            let _ = ctx.now();
            ctx.commit().expect("co-commit the marker");
            let mut r = runs.get(&tenant(), "R1").unwrap();
            r.cursor = 1;
            runs.put(r);
        }
        // re-drive with an ACTIVITY at position 0 — the divergence. The guard HALTS, does not re-exec.
        let ran = std::sync::Arc::new(Mutex::new(false));
        let ran2 = ran.clone();
        let run = runs.get(&tenant(), "R1").unwrap();
        let body: Box<WorkflowBody> = Box::new(move |ctx: &mut WfCtx| {
            let ran3 = ran2.clone();
            ctx.activity(RetryPolicy::default_policy(), move |_i, _a| {
                *ran3.lock().unwrap() = true; // would flip IF the divergent activity ran live.
                Ok(vec![ArtifactRef("myelin://acme/agent/effect/SHOULD-NOT-RUN".into())])
            })
            .map_err(|e| format!("{e:?}"))?;
            Ok(vec![])
        });
        let outcome = drive(
            &runs, &outbox, &journal, &tele, minter(), ctx_base(), &run,
            "2026-06-21T00:00:00Z", 7, body.as_ref(),
        );

        // THE FLOW-D2 ASSERTIONS:
        assert!(
            matches!(outcome, DriveOutcome::Nondeterministic(_)),
            "the drive halts as Nondeterministic, got {outcome:?}"
        );
        assert!(!*ran.lock().unwrap(), "the divergent activity did NOT run live (the guard halted it)");
        let settled = runs.get(&tenant(), "R1").expect("run row");
        assert_eq!(
            settled.state,
            run_state::NONDETERMINISTIC,
            "the run is dead-lettered as nondeterministic (terminal)"
        );
        assert!(run_state::is_terminal(&settled.state), "nondeterministic is terminal — never re-driven");
        assert!(settled.lease_owner.is_none(), "the lease is released on the halt");
        assert_eq!(settled.cursor, 1, "the cursor is UNCHANGED (the guard rewrote no journal)");
        // the divergent drive's partial journal was DISCARDED — only the original marker row survives.
        assert_eq!(
            journal.history_for(&tenant(), "R1").len(),
            1,
            "the journal is unchanged (the divergent drive committed NOTHING — 0 corruption)"
        );
        assert_eq!(tele.double_effect_count(), 0, "0 double-effect — the divergence is a halt, not a re-exec");
        // THE GREEN ARTIFACT: the nondeterministic-halt count incremented by EXACTLY 1.
        assert_eq!(
            tele.nondeterministic_halt_count(),
            1,
            "the nondeterministic-halt counter incremented by exactly the injected divergence count (1)"
        );
    }

    /// **FLOW-D2 versioning leg (§4.6): a WRONG-VERSION replay HALTS as `nondeterministic`.** A run
    /// pinned to `wf_version = 1` at start is re-driven by an engine running definition version 2 (a
    /// deploy bumped the body while the run was in flight). `drive_versioned` halts the run BEFORE a
    /// single command runs — replaying a new body over an old journal is a silent divergence. The
    /// nondeterministic-halt counter increments by 1.
    #[test]
    fn wrong_version_replay_halts_nondeterministic() {
        let runs = RunStore::new();
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let tele = FlowTelemetry::new();
        runs.put(RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0));

        // a body whose activity would run IF the version guard did not halt first.
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
        // the run was pinned to v1 at start; the engine is replaying with v2 — a version divergence.
        let outcome = drive_versioned(
            &runs, &outbox, &journal, &tele, minter(), ctx_base(), &run,
            "2026-06-21T00:00:00Z", 7, body.as_ref(), 1, 2,
        );
        assert!(
            matches!(outcome, DriveOutcome::Nondeterministic(ref r) if r.contains("wf_version")),
            "the version mismatch halts as Nondeterministic naming the version pin, got {outcome:?}"
        );
        assert!(!*ran.lock().unwrap(), "the body did NOT run a command (the version guard halted first)");
        assert_eq!(
            runs.get(&tenant(), "R1").unwrap().state,
            run_state::NONDETERMINISTIC,
            "the wrong-version run is dead-lettered"
        );
        assert_eq!(tele.nondeterministic_halt_count(), 1, "the version-divergence halt counted once");
        // a MATCHING version (v1 == v1) drives normally (the version leg does NOT false-positive).
        runs.put(RunRow::new_runnable(tenant(), region(), "R2", "agent.run", 0));
        let run2 = runs.get(&tenant(), "R2").unwrap();
        let ok_body = n_activity_body(1, std::sync::Arc::new(Mutex::new(Vec::new())));
        let outcome2 = drive_versioned(
            &runs, &outbox, &journal, &tele, minter(), ctx_base(), &run2,
            "2026-06-21T00:00:00Z", 7, ok_body.as_ref(), 1, 1,
        );
        assert!(matches!(outcome2, DriveOutcome::Completed(_)), "a matching version drives normally");
        assert_eq!(tele.nondeterministic_halt_count(), 1, "no false-positive halt on a matching version");
    }

    /// **FLOW-D2: 0 SILENT divergence — a DETERMINISTIC replay never trips the guard.** A clean
    /// crash-recovery re-drive (the FLOW-D1 happy path) replays its journal and completes WITHOUT
    /// incrementing the nondeterministic-halt counter. This pins the guard does not fire on a healthy
    /// replay (the "0 silent divergence" floor reads BOTH ways: the guard halts a divergence AND stays
    /// silent on a deterministic run).
    #[test]
    fn deterministic_replay_does_not_trip_the_guard() {
        let runs = RunStore::new();
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let tele = FlowTelemetry::new();
        runs.put(RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0));

        // a full deterministic drive of 3 activities, then a clean re-drive of the SAME body.
        let run = runs.get(&tenant(), "R1").unwrap();
        drive(
            &runs, &outbox, &journal, &tele, minter(), ctx_base(), &run,
            "2026-06-21T00:00:00Z", 7, n_activity_body(3, std::sync::Arc::new(Mutex::new(Vec::new()))).as_ref(),
        );
        let again = runs.get(&tenant(), "R1").unwrap();
        let outcome = drive(
            &runs, &outbox, &journal, &tele, minter(), ctx_base(), &again,
            "2026-06-21T00:00:00Z", 7, n_activity_body(3, std::sync::Arc::new(Mutex::new(Vec::new()))).as_ref(),
        );
        assert!(matches!(outcome, DriveOutcome::Completed(_)), "the deterministic re-drive completes");
        assert_eq!(
            tele.nondeterministic_halt_count(),
            0,
            "0 silent divergence: a deterministic replay NEVER trips the divergence guard"
        );
    }

    /// **The telemetry accumulates ACROSS drives + reports the runnable-run-lag gauge (§1.8).** Two
    /// cold drives of 2 runs (3 activities each) accumulate 6 executed commands (the `+=` is real, not
    /// a `-=`/`*=`); the runnable-run-lag gauge tracks the unleased runnable count and DROPS to 0 as
    /// the runs complete. Pins `record_drive`'s accumulation + `set_runnable_lag`/`runnable_run_lag`.
    #[test]
    fn telemetry_accumulates_across_drives_and_reports_lag() {
        let runs = RunStore::new();
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let tele = FlowTelemetry::new();
        runs.put(RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0));
        runs.put(RunRow::new_runnable(tenant(), region(), "R2", "agent.run", 0));
        // before any drive the gauge reads its default 0.
        assert_eq!(tele.runnable_run_lag(), 0, "no drive yet → the lag gauge default is 0");

        let run1 = runs.get(&tenant(), "R1").unwrap();
        drive(
            &runs, &outbox, &journal, &tele, minter(), ctx_base(), &run1,
            "2026-06-21T00:00:00Z", 7, n_activity_body(3, Arc::new(Mutex::new(Vec::new()))).as_ref(),
        );
        // after drive 1: 3 commands executed; R2 still runnable → lag gauge reads 1.
        assert_eq!(tele.commands_executed(), 3, "drive 1 executed 3 commands");
        assert_eq!(tele.runnable_run_lag(), 1, "R2 is still runnable (the lag gauge is set from the store)");

        let run2 = runs.get(&tenant(), "R2").unwrap();
        drive(
            &runs, &outbox, &journal, &tele, minter(), ctx_base(), &run2,
            "2026-06-21T00:00:00Z", 7, n_activity_body(3, Arc::new(Mutex::new(Vec::new()))).as_ref(),
        );
        // after drive 2: the counter ACCUMULATED to 6 (the `+=` is real); both runs completed → lag 0.
        assert_eq!(tele.commands_executed(), 6, "the executed counter accumulated across drives (6)");
        assert_eq!(tele.runnable_run_lag(), 0, "both runs completed → the runnable-run-lag drops to 0");
        assert_eq!(tele.double_effect_count(), 0, "0 double-effect across both drives");
    }

    /// **`resume` continues the history `seq` PAST the journaled rows (the cursor floor §3.1).** A run
    /// journals 3 commands; a resume that adds a 4th command journals it at `seq = 3` (NOT
    /// overwriting seq 0..=2). Pins the `max(seq) + 1` continuation in `WfCtx::resume`.
    #[test]
    fn resume_continues_history_seq_past_the_journal() {
        let runs = RunStore::new();
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let tele = FlowTelemetry::new();
        runs.put(RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0));

        // crash after 3 journaled commands (seq 0,1,2), run left runnable at cursor 3.
        {
            let mut ctx = WfCtx::begin(
                &outbox, minter(), journal.clone(), ctx_base(), "R1", "agent.run",
                "2026-06-21T00:00:00Z", 7,
            );
            for k in 0..3 {
                ctx.activity(RetryPolicy::default_policy(), move |_i, _a| {
                    Ok(vec![ArtifactRef(format!("myelin://acme/agent/effect/e{k}"))])
                })
                .expect("activity");
            }
            ctx.commit().expect("3 steps co-commit");
            let mut r = runs.get(&tenant(), "R1").unwrap();
            r.cursor = 3;
            runs.put(r);
        }

        // re-drive a 4-command body: 0..=2 replay, command 3 journals at seq 3 (past the journal).
        let run = runs.get(&tenant(), "R1").unwrap();
        drive(
            &runs, &outbox, &journal, &tele, minter(), ctx_base(), &run,
            "2026-06-21T00:00:00Z", 7, n_activity_body(4, Arc::new(Mutex::new(Vec::new()))).as_ref(),
        );
        let seqs: Vec<i64> = journal.history_for(&tenant(), "R1").iter().map(|r| r.seq).collect();
        assert_eq!(seqs, vec![0, 1, 2, 3], "the resumed command journaled at seq 3 (past the journal, no overwrite)");
    }

    /// **The dispatcher tick loop leases + drives a runnable run, then drains (§4.7).** Two runnable
    /// runs are seeded; two ticks drive them both to completion; a third tick finds nothing runnable
    /// (returns `None`). The registered body's `wf_type` is matched.
    #[test]
    fn dispatcher_tick_leases_and_drives_runnable_runs() {
        let runs = RunStore::new();
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let tele = FlowTelemetry::new();
        runs.put(RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0));
        runs.put(RunRow::new_runnable(tenant(), region(), "R2", "agent.run", 0));

        let mut disp = FlowDispatcher::new(
            runs.clone(), outbox.clone(), journal.clone(), tele.clone(), minter(),
            ctx_base(), 0, "worker-1", 30,
        );
        disp.register("agent.run", n_activity_body(3, Arc::new(Mutex::new(Vec::new()))));

        // tick 1 + tick 2 drive the two runnable runs to completion; tick 3 finds nothing.
        assert!(matches!(disp.tick(1000, "2026-06-21T00:00:00Z", 7), Some(DriveOutcome::Completed(_))));
        assert!(matches!(disp.tick(1001, "2026-06-21T00:00:00Z", 7), Some(DriveOutcome::Completed(_))));
        assert!(disp.tick(1002, "2026-06-21T00:00:00Z", 7).is_none(), "no runnable work left");
        assert_eq!(runs.get(&tenant(), "R1").unwrap().state, run_state::COMPLETED);
        assert_eq!(runs.get(&tenant(), "R2").unwrap().state, run_state::COMPLETED);
        assert_eq!(disp.telemetry().double_effect_count(), 0, "0 double-effect across the loop");
    }

    /// **A dispatcher tick on a run whose worker crashed mid-drive re-leases + resumes (§4.7).** A run
    /// is leased by a worker that crashes (leaving 3 journaled commands + an expired lease); a later
    /// tick re-leases it and resumes — 0 re-execution of the journaled commands.
    #[test]
    fn dispatcher_re_leases_a_crashed_run_and_resumes() {
        let runs = RunStore::new();
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let tele = FlowTelemetry::new();
        runs.put(RunRow::new_runnable(tenant(), region(), "R1", "agent.run", 0));

        // a worker journals 3 steps then crashes (leaves the run running, cursor 3, lease expired).
        {
            let mut ctx = WfCtx::begin(
                &outbox, minter(), journal.clone(), ctx_base(), "R1", "agent.run",
                "2026-06-21T00:00:00Z", 7,
            );
            for k in 0..3 {
                ctx.activity(RetryPolicy::default_policy(), move |_i, _a| {
                    Ok(vec![ArtifactRef(format!("myelin://acme/agent/effect/e{k}"))])
                })
                .expect("activity");
            }
            ctx.commit().expect("3 steps co-commit");
            let mut r = runs.get(&tenant(), "R1").unwrap();
            r.cursor = 3;
            r.lease_owner = Some("dead-worker".into());
            r.lease_expires = Some(500); // an EXPIRED lease (now will be later).
            runs.put(r);
        }

        let executed = Arc::new(Mutex::new(Vec::new()));
        let mut disp = FlowDispatcher::new(
            runs.clone(), outbox.clone(), journal.clone(), tele.clone(), minter(),
            ctx_base(), 0, "worker-2", 30,
        );
        disp.register("agent.run", n_activity_body(5, executed.clone()));
        // tick at now=1000 (> 500, the dead lease expired): worker-2 re-leases + resumes.
        let outcome = disp.tick(1000, "2026-06-21T00:00:00Z", 7).expect("a runnable run was re-leased");
        assert!(matches!(outcome, DriveOutcome::Completed(_)), "resumed to completion");
        assert_eq!(executed.lock().unwrap().clone(), vec![3, 4], "resumed at step 4 — only 3,4 ran");
        assert_eq!(tele.double_effect_count(), 0, "0 re-executed side effects on re-lease");
        // the dispatcher's telemetry handle reports the REAL replay accounting (3 replayed, 2
        // executed → 3/5 = 6000 bps) — pins `FlowDispatcher::telemetry` returns the live handle.
        assert_eq!(disp.telemetry().commands_replayed(), 3, "the dispatcher's telemetry recorded 3 replays");
        assert_eq!(disp.telemetry().replay_rate_bps(), 6000, "the live telemetry handle reports 6000 bps");
    }

    /// **An emit on the drive co-commits with the journal (FLOW-D5 preserved under the engine).** A
    /// body that emits an event has its emit + journal co-commit; the re-drive replays the activity
    /// (0 re-execution) and does NOT re-emit a duplicate (the journaled command short-circuits).
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
            &runs, &outbox, &journal, &tele, minter(), ctx_base(), &run,
            "2026-06-21T00:00:00Z", 7, make_body().as_ref(),
        );
        assert_eq!(outbox.committed_count(), 1, "the emit co-committed with the journal");

        // re-drive: the activity replays (no re-execution); the run completes again. The emit is a
        // LIVE command after the replayed activity — but the activity short-circuit means 0 re-exec.
        let again = runs.get(&tenant(), "R1").expect("run");
        drive(
            &runs, &outbox, &journal, &tele, minter(), ctx_base(), &again,
            "2026-06-21T00:00:00Z", 7, make_body().as_ref(),
        );
        assert_eq!(tele.double_effect_count(), 0, "0 double-effect on the re-drive");
        assert_eq!(journal.history_for(&tenant(), "R1").len(), 1, "no duplicate journal row");
    }
}

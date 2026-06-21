//! # `wfctx` — the `WfCtx` core surface + the journal/outbox co-commit (P-FLOW-04 → P-199, M2)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/durable-workflow.md` §5.1 (the `WfCtx` trait
//! surface: `activity`, `now`, `rand`, `emit`), §4.4 (activity execution + retry — the idempotent
//! at-least-once primitive), §4.5 (the outbox seam — NO second emit path), §3.2 (`wf_history` as
//! the journal source of truth, `UNIQUE(tenant, run_id, command_id)`). Carried forward from
//! Phase-3 §4.1/§4.4/§4.5/§5.1 unchanged.
//!
//! **Contract-index cluster:** owns the WRITE HALF of 9.2 (`WfCtx`: `activity`/`now`/`rand`/`emit`)
//! — the deterministic surface. Consumes 2.2 `OutboxTx::emit` (the ONLY emit path). The
//! replay/wait/timer halves of 9.2 land in later prompts (P-FLOW-05/09/13).
//!
//! ## What this prompt (P-FLOW-04) ships — the SINGLE-TXN write discipline
//!
//! The heart of this prompt is the **journal/outbox co-commit (FLOW-D5)**: a workflow step
//! journals its effect to `wf_history` (under its deterministic `command_id`) AND emits its event
//! via the outbox **in ONE transaction**. There is no second emit path — `emit` goes through
//! [`myelin_events::OutboxTx`], and the journal row is staged into the SAME
//! [`OutboxTransaction`](myelin_events::OutboxTransaction) the outbox row commits with. So a crash
//! between "journal the activity's DB write" and "emit its event" is impossible-by-construction: a
//! step is **either fully journaled-and-emitted or neither** (0 ghost, 0 lost). This is
//! `myelin-flow`'s face of the Tier-1 silent-data-loss floor (BUS-D4-equivalent for the workflow
//! journal — EI-01 §2).
//!
//! The surface:
//! - [`WfCtx::activity`] — runs the activity closure, journals EXACTLY ONE `wf_history` row under
//!   its deterministic `command_id` (idempotent on `UNIQUE(tenant, run_id, command_id)`, §3.2),
//!   records the BUS-2 dedup `idem_token` in `wf_activity_attempt`, and RETRIES on failure reusing
//!   the SAME `idem_token` (§4.4 — a retried activity produces no duplicate effect).
//! - [`WfCtx::now`] / [`WfCtx::rand`] — journaled SIDE-MARKERS (`kind = side_marker`): the
//!   non-deterministic reads a workflow makes are captured in the journal so replay (P-FLOW-05)
//!   returns the SAME value, making the workflow body deterministic.
//! - [`WfCtx::emit`] — emits an [`EventDraft`](myelin_events::EventDraft) via the outbox; the
//!   journal row and the outbox row co-commit in the one transaction (no raw publish).
//! - [`WfCtx::commit`] — the atomic co-commit (the staged journal rows + the staged outbox rows
//!   become durable TOGETHER, or — if the `WfCtx` is dropped without `commit` — NEITHER).
//!
//! ## FLOORS named (this prompt ships the WRITE half only)
//!
//! - **The replay short-circuit** that READS these journal rows back (deterministic recovery +
//!   lease dispatch + crash recovery) → **P-FLOW-05** (FLOW-D1). This prompt WRITES the journal;
//!   the replay that re-derives a run from it lands next. An activity here always EXECUTES (there
//!   is no cursor-skip yet); the journal it writes is what P-FLOW-05's replay reads.
//! - **`DurableExecutor::start/describe/cancel`** (the run lifecycle entry points) → **P-FLOW-06**.
//! - **`sleep_until`/`sleep_for`** (durable timers) → **P-FLOW-13**; **`wait_for_signal`** (durable
//!   signals) → **P-FLOW-09/11**. The `WfCtx` surface here is the `activity`/`now`/`rand`/`emit`
//!   quartet only (§5.1's first four methods).
//! - **The live OLTP binding.** The journal + the outbox are modeled here over the substrate's
//!   in-memory transactional [`OutboxStore`](myelin_events::OutboxStore) + an in-memory
//!   [`WfJournal`] that mirrors the frozen `wf_history`/`wf_activity_attempt` shapes
//!   ([`crate::schema`]/[`crate::migrations`]). The co-commit ATOMICITY this proves is the same
//!   observable property the real `INSERT … RETURNING` inside the caller's PG transaction lands
//!   (dev↔prod is a config swap, never a code change). The live-DB co-commit apply is exercised in
//!   `tests/integration_flow_cocommit.rs` (the `integration` feature) against the dev stack.

use crate::schema::{WfActivityAttemptRow, WfHistoryRow};
use myelin_events::{
    EmitContextBase, EventDraft, EventEnvelope, EventId, IdMinter, OutboxStore, OutboxTransaction,
    OutboxTx, Result as EmitResult,
};
use myelin_tenancy::{Region, TenantId};
use std::sync::{Arc, Mutex};

/// The frozen `wf_history.kind` tokens this prompt writes (a subset of the §3.2 vocabulary the
/// [`crate::migrations`] `CHECK` constraint admits). The WfCtx core writes activity scheduling /
/// completion / failure rows + the `side_marker` rows for `now`/`rand`; the timer/signal kinds are
/// written by the later prompts (P-FLOW-09/13).
pub mod history_kind {
    /// An activity was scheduled (the journal row written before the closure runs) — §3.2.
    pub const ACTIVITY_SCHEDULED: &str = "activity_scheduled";
    /// An activity completed successfully (the journal row carrying its result refs) — §3.2.
    pub const ACTIVITY_COMPLETED: &str = "activity_completed";
    /// An activity failed (all retries exhausted) — §3.2.
    pub const ACTIVITY_FAILED: &str = "activity_failed";
    /// A `now()`/`rand()` non-deterministic read, captured for deterministic replay — §3.2.
    pub const SIDE_MARKER: &str = "side_marker";
}

/// The `wf_activity_attempt.state` tokens this prompt writes (the §3.5 ledger lifecycle; mirrors
/// the [`crate::migrations`] `CHECK`).
pub mod attempt_state {
    /// The attempt succeeded — its result is journaled, its `idem_token` is spent.
    pub const SUCCEEDED: &str = "succeeded";
    /// The attempt failed and a retry will follow (reusing the same `idem_token`).
    pub const RETRYING: &str = "retrying";
    /// The attempt failed and no retry remains (the activity is failed).
    pub const FAILED: &str = "failed";
}

/// An activity's error — a machine error string (NO subject data; the activity's PII stays in its
/// own erasable store, references-not-payloads §3.5). Carried so the retry/dead-letter policy can
/// inspect it and the failure is journaled without leaking PII.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityError(pub String);

/// A `WfCtx` operation error (a co-commit / journal failure, or an activity that exhausted its
/// retries). Distinct from [`ActivityError`] (a single attempt's failure) — this is the durable
/// step's verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WfError {
    /// An activity exhausted its retry budget (§4.4) — the durable step failed. Carries the last
    /// attempt's error (the journaled `activity_failed` reason).
    ActivityExhausted(ActivityError),
    /// The co-commit / emit path failed (the outbox emit returned an error). Surfaces the
    /// underlying [`myelin_events::OutboxError`] message verbatim — never swallowed.
    CoCommit(String),
}

/// The result type for the durable `WfCtx` surface.
pub type WfResult<T> = core::result::Result<T, WfError>;

/// The retry policy for [`WfCtx::activity`] (§4.4) — the bounded attempt count. The `idem_token`
/// is REUSED across attempts (a retried activity produces no duplicate downstream effect), and the
/// failure is journaled only after the budget is exhausted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    /// The maximum number of attempts (>= 1). Attempt 1 is the first try; attempts 2..=`max`
    /// are retries. A `max` of 1 means "no retry".
    pub max_attempts: u32,
}

impl RetryPolicy {
    /// The default activity retry policy — three attempts (one try + two retries), the §4.4 floor
    /// the engine's bounded-retry survival signal (contract 1.8 retry rate) is read against.
    pub const fn default_policy() -> Self {
        Self { max_attempts: 3 }
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::default_policy()
    }
}

/// An in-memory model of the `wf_history` + `wf_activity_attempt` journal (the frozen
/// [`crate::schema`] shapes), behind an `Arc<Mutex<…>>` so it is a cloneable handle the run + a
/// telemetry/replay reader share one truth over (exactly as [`OutboxStore`] is for the outbox).
///
/// **The journal is the SOURCE OF TRUTH (§3.2).** Rows enter it ONLY through
/// [`WfCtx::commit`], which appends the WfCtx's staged journal rows in the SAME atomic step the
/// outbox transaction commits its rows — so a dropped `WfCtx` (an abort / a crash between staging
/// and commit) writes NOTHING to the journal either: there is no journaled effect without its
/// emitted event, and (because they share the one commit) no emitted event without its journal
/// row. This is the FLOW-D5 co-commit, correct-by-construction.
#[derive(Clone, Default)]
pub struct WfJournal {
    inner: Arc<Mutex<JournalInner>>,
}

#[derive(Default)]
struct JournalInner {
    /// The append-only `wf_history` rows, in commit order (the replay-order source of truth).
    history: Vec<WfHistoryRow>,
    /// The `wf_activity_attempt` idempotency ledger rows, in commit order.
    attempts: Vec<WfActivityAttemptRow>,
    /// The set of `(tenant, run_id, command_id)` already journaled — models the
    /// `UNIQUE(tenant, run_id, command_id)` (§3.2): a second journal of the same command is a
    /// no-op (idempotent journaling — the silent-data-loss floor under replay).
    journaled_commands: std::collections::HashSet<(String, String, String)>,
}

impl WfJournal {
    /// A fresh, empty journal (no history, no attempts).
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, JournalInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The number of `wf_history` rows durably journaled (the replay-order journal depth).
    pub fn history_len(&self) -> usize {
        self.lock().history.len()
    }

    /// The number of `wf_activity_attempt` ledger rows durably journaled.
    pub fn attempt_len(&self) -> usize {
        self.lock().attempts.len()
    }

    /// The `wf_history` rows for a run, in replay order (the journal a replay reads back — the
    /// floor the P-FLOW-05 replay short-circuit consumes).
    pub fn history_for(&self, tenant: &TenantId, run_id: &str) -> Vec<WfHistoryRow> {
        self.lock()
            .history
            .iter()
            .filter(|r| &r.tenant == tenant && r.run_id == run_id)
            .cloned()
            .collect()
    }

    /// The `wf_activity_attempt` ledger rows for a run, in attempt order.
    pub fn attempts_for(&self, tenant: &TenantId, run_id: &str) -> Vec<WfActivityAttemptRow> {
        self.lock()
            .attempts
            .iter()
            .filter(|r| &r.tenant == tenant && r.run_id == run_id)
            .cloned()
            .collect()
    }

    /// Whether `(tenant, run_id, command_id)` is already journaled (the idempotency check —
    /// models the `UNIQUE(tenant, run_id, command_id)` §3.2).
    pub fn is_journaled(&self, tenant: &TenantId, run_id: &str, command_id: &str) -> bool {
        self.lock().journaled_commands.contains(&(
            tenant.0.clone(),
            run_id.to_string(),
            command_id.to_string(),
        ))
    }

    /// Atomically append the staged journal rows. Called ONLY from [`WfCtx::commit`] (the one
    /// write path) so the journal mirrors the outbox's emit-iff-committed discipline. Journaling
    /// is idempotent on `(tenant, run_id, command_id)`: a row whose command is already journaled
    /// is skipped (a replayed commit is a no-op, never a duplicate journal row).
    fn commit_rows(&self, history: Vec<WfHistoryRow>, attempts: Vec<WfActivityAttemptRow>) {
        let mut inner = self.lock();
        for row in history {
            let key = (row.tenant.0.clone(), row.run_id.clone(), row.command_id.clone());
            if inner.journaled_commands.insert(key) {
                inner.history.push(row);
            }
            // else: UNIQUE(tenant, run_id, command_id) — already journaled, this is a no-op
            // (idempotent journaling; the replay-safe property §3.2).
        }
        inner.attempts.extend(attempts);
    }
}

/// **The `WfCtx` core surface (contract 9.2 write half) — the deterministic workflow step
/// surface, journaled with single-txn co-commit (FLOW-D5).**
///
/// A `WfCtx` is created for a run with [`WfCtx::begin`]; it opens ONE
/// [`OutboxTransaction`](myelin_events::OutboxTransaction) and stages the run's `wf_history` /
/// `wf_activity_attempt` journal rows alongside the outbox rows. [`WfCtx::commit`] makes them
/// durable TOGETHER; dropping the `WfCtx` without `commit` writes NOTHING (FLOW-D5, the
/// silent-data-loss floor).
///
/// `command_id` is DETERMINISTIC from the workflow position (`<wf_type>:<n>`, n the per-run command
/// counter) — the replay-match key (§3.2). `now`/`rand` are journaled side-markers so replay is
/// deterministic (P-FLOW-05). `emit` goes through the outbox — the no-raw-publish lint forbids a
/// second emit path (§4.5).
pub struct WfCtx {
    tenant: TenantId,
    region: Region,
    run_id: String,
    wf_type: String,
    /// The open outbox transaction — `emit` buffers into it; `commit` makes its rows durable.
    tx: OutboxTransaction,
    /// The journal store the staged history/attempt rows commit into (the source of truth §3.2).
    journal: WfJournal,
    /// Staged `wf_history` rows — durable iff [`WfCtx::commit`] is called (FLOW-D5).
    staged_history: Vec<WfHistoryRow>,
    /// Staged `wf_activity_attempt` ledger rows — durable iff [`WfCtx::commit`] is called.
    staged_attempts: Vec<WfActivityAttemptRow>,
    /// The per-run deterministic command counter (the `command_id` position, §3.2).
    command_seq: u64,
    /// The per-run monotonic history `seq` (the replay-order PK, §3.2).
    history_seq: i64,
    /// The deterministic `rand()` seed state (a journaled, replay-stable sequence — NOT a real RNG;
    /// the value is captured in the side-marker so replay returns the same number, §5.1).
    rand_state: u64,
    /// The deterministic `now()` clock the side-marker captures (RFC-3339 UTC; §5.1). In the live
    /// engine this is the worker's wall-clock at first execution, journaled so replay returns it.
    now_clock: String,
}

impl WfCtx {
    /// **Begin a `WfCtx` for a run** — opens the one co-commit transaction the run's journal +
    /// outbox rows share. `outbox` is the service-owned [`OutboxStore`]; `minter` supplies the
    /// stable ULID for emitted events; `journal` is the `wf_history`/`wf_activity_attempt` store.
    /// `now_clock` is the deterministic RFC-3339 UTC `now()` returns (journaled as a side-marker so
    /// replay is deterministic); `rand_seed` seeds the journaled `rand()` sequence.
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
        }
    }

    /// The deterministic `command_id` for the NEXT command (`<wf_type>:<n>`) — the replay-match key
    /// (§3.2). Increments the per-run command counter. Producer and consumer agree on the key
    /// WITHOUT coordination because it is derived purely from the workflow position.
    fn next_command_id(&mut self) -> String {
        let id = format!("{}:{}", self.wf_type, self.command_seq);
        self.command_seq += 1;
        id
    }

    /// The next per-run monotonic history `seq` (the replay-order PK, §3.2).
    fn next_history_seq(&mut self) -> i64 {
        let s = self.history_seq;
        self.history_seq += 1;
        s
    }

    /// Stage one `wf_history` row (idempotent on `command_id` at commit). NOT durable until
    /// [`WfCtx::commit`].
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
            // references-not-payloads: the WfCtx core writes NO inline-PII result (the rare
            // crypto-shred case is a later, explicit opt-in). The key ref stays None by default.
            result_key_ref: None,
        });
    }

    /// **`activity<I, O>` (contract 9.2, §4.4) — the at-least-once + idempotent activity
    /// primitive.** Runs the activity closure under a deterministic `command_id`, journals EXACTLY
    /// ONE `wf_history` row for the outcome, records the BUS-2 dedup `idem_token` in
    /// `wf_activity_attempt`, and RETRIES on failure (up to `policy.max_attempts`) REUSING the same
    /// `idem_token` — so a retried activity that already produced its downstream effect is
    /// broker-deduped, never duplicated (§3.5/§4.4).
    ///
    /// The journal row + (if the closure emits via this same `WfCtx`) the outbox rows co-commit on
    /// [`WfCtx::commit`] — one transaction (FLOW-D5). On success, one `activity_completed` history
    /// row is staged (carrying the result refs); on exhausted retries, one `activity_failed` row
    /// is staged and [`WfError::ActivityExhausted`] is returned.
    ///
    /// `run` is the activity body: it takes the BUS-2 `idem_token` (so its OWN downstream write/
    /// emit is dedup-keyed, §3.5) and the attempt number, and returns the result refs or an
    /// [`ActivityError`].
    pub fn activity<F>(
        &mut self,
        policy: RetryPolicy,
        run: F,
    ) -> WfResult<Vec<myelin_refs::ArtifactRef>>
    where
        F: Fn(&str, u32) -> Result<Vec<myelin_refs::ArtifactRef>, ActivityError>,
    {
        let command_id = self.next_command_id();
        // The idem_token is DETERMINISTIC from the command position (so producer and consumer agree
        // WITHOUT coordination, §3.5) and is the SAME across every attempt — the retry-dedup anchor.
        let idem_token = format!("{}/{}/{}", self.run_id, command_id, "act");
        let max = policy.max_attempts.max(1);
        let mut last_err: Option<ActivityError> = None;

        for attempt in 1..=max {
            match run(&idem_token, attempt) {
                Ok(result) => {
                    // Journal the success EXACTLY ONCE under the deterministic command_id (§3.2),
                    // and record the spent idem_token in the attempt ledger (§3.5). Both are STAGED
                    // — durable iff commit (FLOW-D5).
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
                    let final_attempt = attempt == max;
                    self.staged_attempts.push(self.attempt_row(
                        &command_id,
                        attempt,
                        &idem_token,
                        if final_attempt {
                            attempt_state::FAILED
                        } else {
                            attempt_state::RETRYING
                        },
                        Some(e.0.clone()),
                    ));
                    last_err = Some(e);
                }
            }
        }

        // Retries exhausted: journal the failure EXACTLY ONCE (§3.2) and surface it (§4.4).
        let err = last_err.expect("a failing loop produced at least one error");
        self.stage_history(history_kind::ACTIVITY_FAILED, command_id, None);
        Err(WfError::ActivityExhausted(err))
    }

    /// Build a `wf_activity_attempt` ledger row (the §3.5 idempotency ledger; mirrors the frozen
    /// [`crate::schema::WfActivityAttemptRow`]).
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
            // The wall-clock stamps are the worker's; in the in-memory model the deterministic
            // now_clock stands in (journaled side-markers carry the real clock at replay).
            started_at: Some(self.now_clock.clone()),
            ended_at: Some(self.now_clock.clone()),
        }
    }

    /// **`now()` (contract 9.2, §5.1) — a journaled SIDE-MARKER.** Returns the deterministic
    /// RFC-3339 UTC clock for this run AND journals a `side_marker` `wf_history` row capturing it,
    /// so on replay (P-FLOW-05) the workflow body reads back the SAME timestamp — making a
    /// `now()`-dependent workflow deterministic. The marker is STAGED (durable iff commit).
    pub fn now(&mut self) -> String {
        let command_id = self.next_command_id();
        self.stage_history(history_kind::SIDE_MARKER, command_id, None);
        self.now_clock.clone()
    }

    /// **`rand()` (contract 9.2, §5.1) — a journaled SIDE-MARKER.** Returns the next value of a
    /// deterministic, replay-stable sequence (a splitmix64 step over the seeded state — NOT a
    /// source of entropy; it is replay-stable BY DESIGN) AND journals a `side_marker` row capturing
    /// the draw, so replay returns the SAME number. The marker is STAGED (durable iff commit).
    pub fn rand(&mut self) -> u64 {
        // splitmix64 — a deterministic, well-mixed sequence (replay-stable; the journaled marker is
        // what makes it correct under replay, the sequence itself is just reproducible).
        self.rand_state = self.rand_state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.rand_state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        let value = z ^ (z >> 31);
        let command_id = self.next_command_id();
        self.stage_history(history_kind::SIDE_MARKER, command_id, None);
        value
    }

    /// **`emit(EventDraft)` (contract 9.2, §4.5; consumes 2.2) — the ONLY emit path.** Buffers the
    /// event into the SAME [`OutboxTransaction`](myelin_events::OutboxTransaction) the run's journal
    /// rows commit with, so the journal row and the outbox row co-commit in ONE transaction (no raw
    /// publish — the no-raw-publish lint, §4.5/contract 2.2 forbids a second emit path). `cause` is
    /// the parent envelope this emit reacts to (causality is derived correct-by-construction by
    /// [`OutboxTx::emit`](myelin_events::OutboxTx::emit), BUS-5); pass `None` for a root.
    ///
    /// The emit is NOT durable until [`WfCtx::commit`] — it is staged in the open transaction
    /// exactly like the journal rows (FLOW-D5: a `WfCtx` dropped without commit emits NOTHING).
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

    /// **`commit()` — the atomic journal/outbox co-commit (FLOW-D5).** The staged `wf_history` +
    /// `wf_activity_attempt` rows AND the staged outbox rows become durable TOGETHER, in one
    /// transaction. After this, the journal reflects the run's effects AND the outbox reflects its
    /// emits — exactly, with 0 ghost / 0 lost.
    ///
    /// **The order matters for the floor.** The outbox transaction commits FIRST (it is the
    /// substrate's atomic primitive — emit-iff-committed, BUS-D4). If it fails (a `UNIQUE(event_id)`
    /// violation — a programming error on the happy path), NOTHING is journaled either: the journal
    /// rows are still only staged on `self`, and a returned `Err` leaves them unwritten. If the
    /// outbox commit succeeds, the journal rows are appended in the SAME logical step (modeling the
    /// one PG transaction that holds both inserts). A `WfCtx` DROPPED before `commit` writes neither
    /// (no `Drop` flush — staging is purely on `self`).
    pub fn commit(self) -> WfResult<()> {
        let WfCtx {
            tx,
            journal,
            staged_history,
            staged_attempts,
            ..
        } = self;
        // The outbox transaction is the atomic boundary (the substrate primitive). It commits its
        // staged rows (emit-iff-committed); a failure here writes nothing and the journal rows —
        // still only staged in the moved-out `staged_history`/`staged_attempts` — are dropped
        // unwritten. So an aborted co-commit leaves NEITHER (FLOW-D5).
        tx.commit().map_err(|e| WfError::CoCommit(e.0))?;
        // The outbox rows are durable; the journal rows join them in the same logical transaction
        // (the real engine's INSERTs share one PG txn). Idempotent on (tenant, run_id, command_id)
        // — a replayed commit is a no-op, never a duplicate journal row.
        journal.commit_rows(staged_history, staged_attempts);
        Ok(())
    }

    /// The number of `wf_history` rows currently STAGED (not yet committed) — for the co-commit
    /// assertions (a test reads it to prove "buffered, not durable" before commit).
    pub fn staged_history_len(&self) -> usize {
        self.staged_history.len()
    }

    /// The number of outbox rows currently STAGED in the open transaction (emitted, not committed).
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

    /// **An activity journals EXACTLY ONE `wf_history` row under its deterministic `command_id`.**
    /// The success path stages one `activity_completed` row (carrying the result refs) + one
    /// succeeded `wf_activity_attempt` ledger row; after commit the journal holds exactly one
    /// history row for the command, keyed by the deterministic `<wf_type>:0` command_id (§3.2).
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
        // staged, not durable yet (FLOW-D5).
        assert_eq!(ctx.staged_history_len(), 1, "one history row staged");
        assert_eq!(journal.history_len(), 0, "nothing durable before commit");
        ctx.commit().expect("co-commit");
        let hist = journal.history_for(&tenant(), "R1");
        assert_eq!(hist.len(), 1, "exactly one history row journaled for the command");
        assert_eq!(hist[0].kind, history_kind::ACTIVITY_COMPLETED);
        assert_eq!(hist[0].command_id, "agent.run:0", "deterministic command_id from position");
        assert_eq!(hist[0].seq, 0, "the per-run replay-order seq starts at 0");
        let attempts = journal.attempts_for(&tenant(), "R1");
        assert_eq!(attempts.len(), 1, "one attempt ledger row");
        assert_eq!(attempts[0].state, attempt_state::SUCCEEDED);
    }

    /// **emit and journal share ONE transaction (FLOW-D5) — the co-commit happy path.** An
    /// activity journals its row AND emits an event on the same `WfCtx`; before commit NEITHER is
    /// durable; after commit BOTH are. One transaction, atomic.
    #[test]
    fn emit_and_journal_share_one_txn_co_commit() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let mut ctx = begin(&outbox, journal.clone());
        ctx.activity(RetryPolicy::default_policy(), |_idem, _attempt| {
            Ok(vec![ArtifactRef("myelin://acme/agent/effect/e1".into())])
        })
        .expect("activity");
        ctx.emit(draft("agent.run.step"), None).expect("emit buffers into the txn");
        // before commit: NEITHER the journal row NOR the outbox row is durable (one transaction).
        assert_eq!(journal.history_len(), 0, "no journal row before commit");
        assert_eq!(outbox.outbox_depth(), 0, "no outbox row before commit");
        assert_eq!(ctx.staged_history_len(), 1, "history staged");
        assert_eq!(ctx.staged_emit_len(), 1, "emit staged");
        ctx.commit().expect("co-commit");
        // after commit: BOTH durable, together.
        assert_eq!(journal.history_len(), 1, "journal row durable after commit");
        assert_eq!(outbox.outbox_depth(), 1, "outbox row durable after commit");
    }

    /// **FLOW-D5: inject a failure between journal and emit → atomic (NEITHER or BOTH).** A `WfCtx`
    /// that journals an activity AND emits, then is DROPPED without commit (the crash point between
    /// "journal the DB write" and "emit the event durably") writes NOTHING: 0 journal rows, 0
    /// outbox rows — 0 ghost, 0 lost. This is the silent-data-loss floor, correct-by-construction.
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
            ctx.emit(draft("agent.run.step"), None).expect("emit buffers");
            assert_eq!(ctx.staged_history_len(), 1, "journaled-but-not-committed");
            assert_eq!(ctx.staged_emit_len(), 1, "emitted-but-not-committed");
            // ctx dropped HERE without commit — the crash between journal and emit.
        }
        // NEITHER: 0 journal rows, 0 outbox rows — 0 ghost, 0 lost (FLOW-D5).
        assert_eq!(journal.history_len(), 0, "0 lost: an aborted step journals nothing");
        assert_eq!(journal.attempt_len(), 0, "0 lost: the attempt ledger row is not written either");
        assert_eq!(outbox.outbox_depth(), 0, "0 ghost: an aborted step emits nothing");
        assert_eq!(outbox.committed_count(), 0, "no committed outbox row from an abort");
    }

    /// **A retried activity REUSES its `idem_token` (no duplicate effect, §3.5/§4.4).** An activity
    /// that fails twice then succeeds runs three attempts; every attempt sees the SAME `idem_token`
    /// (the BUS-2 dedup anchor — a retried emit is broker-deduped, never duplicated), and exactly
    /// ONE `activity_completed` history row is journaled (the failures journal no history row, only
    /// retrying/failed attempt-ledger rows).
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
                    Err(ActivityError(format!("transient failure on attempt {attempt}")))
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
        // exactly ONE completed history row (the failures journal no history row).
        let hist = journal.history_for(&tenant(), "R1");
        let completed: Vec<_> = hist
            .iter()
            .filter(|r| r.kind == history_kind::ACTIVITY_COMPLETED)
            .collect();
        assert_eq!(completed.len(), 1, "exactly one activity_completed row (no duplicate effect)");
        // the attempt ledger records all three attempts, all on the same idem_token.
        let attempts = journal.attempts_for(&tenant(), "R1");
        assert_eq!(attempts.len(), 3, "three attempt ledger rows");
        assert!(
            attempts.iter().all(|a| a.idem_token == attempts[0].idem_token),
            "all attempts share one idem_token"
        );
        assert_eq!(attempts[2].state, attempt_state::SUCCEEDED, "the third attempt succeeded");
    }

    /// **An exhausted activity journals exactly one `activity_failed` row and returns
    /// `ActivityExhausted` (§4.4).** All retries fail → the last error surfaces, one failure
    /// history row is journaled, and the run can take its error branch.
    #[test]
    fn exhausted_activity_journals_failed_and_returns_error() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let mut ctx = begin(&outbox, journal.clone());
        let err = ctx
            .activity(RetryPolicy { max_attempts: 2 }, |_idem, attempt| {
                Err(ActivityError(format!("hard failure {attempt}")))
            })
            .expect_err("the activity exhausts its retries");
        match err {
            WfError::ActivityExhausted(ActivityError(msg)) => {
                assert_eq!(msg, "hard failure 2", "the LAST attempt's error surfaces")
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
            !hist.iter().any(|r| r.kind == history_kind::ACTIVITY_COMPLETED),
            "no completed row for a fully-failed activity"
        );
        let attempts = journal.attempts_for(&tenant(), "R1");
        assert_eq!(attempts.len(), 2, "both attempts in the ledger");
        assert_eq!(attempts[1].state, attempt_state::FAILED, "the last attempt is FAILED");
    }

    /// **`now()` / `rand()` are journaled SIDE-MARKERS and are deterministic (§5.1).** Two `WfCtx`
    /// runs over the SAME run with the same clock/seed produce the SAME `now`/`rand` values
    /// (replay-stable), and each draw journals a `side_marker` history row so replay (P-FLOW-05)
    /// reads the value back.
    #[test]
    fn now_and_rand_are_journaled_deterministic_side_markers() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let mut ctx = begin(&outbox, journal.clone());
        let t1 = ctx.now();
        let r1 = ctx.rand();
        let r2 = ctx.rand();
        assert_eq!(t1, "2026-06-21T00:00:00Z", "now() returns the deterministic clock");
        assert_ne!(r1, r2, "rand() advances (a sequence, not a constant)");
        // The EXACT splitmix64 draws for seed 42 — pinned so a regression in the mixing function
        // (a flipped ^/>>) is caught: replay-stability requires the value derivation be frozen.
        assert_eq!(r1, 13_679_457_532_755_275_413, "rand() draw 1 is the frozen splitmix64(42) value");
        assert_eq!(r2, 2_949_826_092_126_892_291, "rand() draw 2 is the frozen splitmix64 value");
        // three side-marker history rows staged (now + two rands).
        assert_eq!(ctx.staged_history_len(), 3, "now/rand each journal a side-marker");
        ctx.commit().expect("co-commit");
        let markers: Vec<_> = journal
            .history_for(&tenant(), "R1")
            .into_iter()
            .filter(|r| r.kind == history_kind::SIDE_MARKER)
            .collect();
        assert_eq!(markers.len(), 3, "three side-marker rows journaled");

        // determinism: a second run with the same seed/clock reproduces the same draws.
        let outbox2 = OutboxStore::new();
        let mut ctx2 = begin(&outbox2, WfJournal::new());
        assert_eq!(ctx2.now(), t1, "now() is replay-stable");
        assert_eq!(ctx2.rand(), r1, "rand() draw 1 is replay-stable");
        assert_eq!(ctx2.rand(), r2, "rand() draw 2 is replay-stable");
    }

    /// **Journaling is idempotent on `(tenant, run_id, command_id)` (§3.2) — the replay-safe
    /// property.** Committing the same command twice (modeling a replayed commit) writes the
    /// history row ONCE: the `UNIQUE(tenant, run_id, command_id)` makes a re-journal a no-op (0
    /// duplicate journal rows — the silent-data-loss floor under replay).
    #[test]
    fn journaling_is_idempotent_on_command_id() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        // First run journals agent.run:0.
        let mut ctx = begin(&outbox, journal.clone());
        ctx.activity(RetryPolicy::default_policy(), |_i, _a| {
            Ok(vec![ArtifactRef("myelin://acme/agent/effect/e1".into())])
        })
        .expect("activity");
        ctx.commit().expect("co-commit");
        assert_eq!(journal.history_len(), 1);
        assert!(journal.is_journaled(&tenant(), "R1", "agent.run:0"));
        // A replayed run re-journals the SAME command_id — the UNIQUE makes it a no-op.
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
        // is_journaled is precise: a command NOT journaled reads false (kills the always-true
        // mutant — the idempotency check must actually consult the journaled-commands set).
        assert!(
            !journal.is_journaled(&tenant(), "R1", "agent.run:99"),
            "an un-journaled command_id reads false (the idempotency check is real, not vacuous)"
        );
        assert!(
            !journal.is_journaled(&tenant(), "R-other", "agent.run:0"),
            "is_journaled is keyed on the run too (a different run's same command is not journaled)"
        );
    }

    /// **The journal read-filters are precise: a query for one run returns ONLY that run's rows
    /// (tenant AND run_id must match — §3.2 isolation).** Two runs journal into the SAME journal;
    /// `history_for`/`attempts_for` for run R1 return exactly R1's rows (not R2's, not both),
    /// proving the AND-filter is real (a tenant-OR-run filter would leak cross-run rows). Also pins
    /// the per-run monotonic history `seq` (the replay-order PK §3.2) increments 0, 1, 2.
    #[test]
    fn journal_reads_are_per_run_and_seq_is_monotonic() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let minter_shared = minter();
        // Run R1: now + activity + rand → three history rows, seq 0,1,2; one attempt row.
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

        // Run R2 (a DIFFERENT run): one activity → one history row, one attempt row.
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

        // history_for(R1) returns EXACTLY R1's three rows (the AND-filter is real — not R2's).
        let h1 = journal.history_for(&tenant(), "R1");
        assert_eq!(h1.len(), 3, "R1 has exactly its three history rows (now+activity+rand)");
        assert!(h1.iter().all(|r| r.run_id == "R1"), "no R2 row leaked into R1's history (AND-filter)");
        // the per-run monotonic replay-order seq is 0, 1, 2 (kills the next_history_seq mutant).
        assert_eq!(
            h1.iter().map(|r| r.seq).collect::<Vec<_>>(),
            vec![0, 1, 2],
            "the per-run history seq is monotonic 0,1,2 (the replay-order PK §3.2)"
        );
        // attempts_for(R1) returns exactly R1's one attempt (not R2's).
        let a1 = journal.attempts_for(&tenant(), "R1");
        assert_eq!(a1.len(), 1, "R1 has exactly one attempt row");
        assert!(a1.iter().all(|r| r.run_id == "R1"), "no R2 attempt leaked into R1's (AND-filter)");
        // R2 is isolated too.
        assert_eq!(journal.history_for(&tenant(), "R2").len(), 1, "R2 has exactly its one row");
        assert_eq!(journal.attempts_for(&tenant(), "R2").len(), 1, "R2 has exactly its one attempt");
        // a wrong tenant returns nothing (the tenant half of the AND-filter).
        assert!(
            journal.history_for(&TenantId("other".into()), "R1").is_empty(),
            "a different tenant sees none of acme's rows (the tenant half of the AND-filter)"
        );
    }

    /// **`staged_emit_len` reflects the OPEN transaction's buffered emits precisely.** Two emits on
    /// one `WfCtx` stage two rows (not a constant); after commit they are durable. Pins the
    /// co-commit staging count (kills the `staged_emit_len -> 1` constant mutant).
    #[test]
    fn staged_emit_len_tracks_the_open_transaction_buffer() {
        let outbox = OutboxStore::new();
        let mut ctx = begin(&outbox, WfJournal::new());
        assert_eq!(ctx.staged_emit_len(), 0, "nothing emitted yet");
        ctx.emit(draft("a.b.c"), None).expect("emit 1");
        assert_eq!(ctx.staged_emit_len(), 1, "one emit staged");
        ctx.emit(draft("a.b.d"), None).expect("emit 2");
        assert_eq!(ctx.staged_emit_len(), 2, "two emits staged (not a constant)");
        ctx.commit().expect("co-commit");
        assert_eq!(outbox.outbox_depth(), 2, "both emits durable after the co-commit");
    }
}

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
    /// **The run PARKED on a `wait_for_signal` (the multi-day HITL wait, P-FLOW-11, §4.3).** Journaled
    /// the first time the body reaches a `wait_for_signal` whose named signal is not yet buffered — the
    /// run goes `waiting` holding no runtime. On replay this short-circuits: the body re-issues the wait
    /// and the journal says "we are/were parked here", so the wait re-checks the buffer (a signal that
    /// arrived in the meantime resumes; an absent one re-parks). Admitted by the migrations `CHECK`.
    pub const SIGNAL_WAITED: &str = "signal_waited";
    /// **A `wait_for_signal` CONSUMED its signal (P-FLOW-11, §4.3).** Journaled when the buffered signal
    /// arrives and the wait resumes — it carries the consumed signal's payload refs (references-not-
    /// payloads). On replay this short-circuits to the SAME consumed payload (the wait returns the
    /// journaled signal, never re-consuming a second buffered row). This is the consume-exactly-once
    /// anchor: the journal records WHICH signal woke the run. Admitted by the migrations `CHECK`.
    pub const SIGNAL_RECEIVED: &str = "signal_received";
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
    /// **The replay-DIVERGENCE verdict (P-FLOW-07, FLOW-D2).** The workflow body, on replay, issued
    /// a command that does NOT match the journal at that position — either a KIND mismatch (the body
    /// issues an `activity` where the journal records a `side_marker`, or vice versa) or a pinned
    /// `wf_version` mismatch (the replay ran a DIFFERENT definition version than the run was pinned to
    /// at start, §4.6). This is a determinism violation: the body is NOT a deterministic function of
    /// its journal. The engine HALTS the run as `nondeterministic` and DEAD-LETTERS it — it NEVER
    /// silently continues (a silent divergence is a Tier-1 failure, EI-01 §2). Carries a machine
    /// reason (no PII) describing the position + the expected-vs-issued shapes.
    Nondeterministic(String),
}

impl WfError {
    /// Whether this error is the replay-DIVERGENCE verdict (P-FLOW-07). The engine reads it to settle
    /// the run as `nondeterministic` + dead-letter it (vs `failed` for an activity exhaustion). A
    /// divergence is NOT a normal failure — it means the run can never make deterministic progress,
    /// so it is parked for a human, not retried.
    pub fn is_nondeterministic(&self) -> bool {
        matches!(self, WfError::Nondeterministic(_))
    }
}

/// The result type for the durable `WfCtx` surface.
pub type WfResult<T> = core::result::Result<T, WfError>;

/// **The outcome of a [`WfCtx::wait_for_signal`] (contract 9.2/9.4, §4.3).** A wait either RESUMES with
/// the consumed signal's references-not-payloads body, PARKS (the named signal has not arrived — the run
/// is `waiting`, holding no runtime, until `DurableExecutor::signal` delivers it), or TIMES OUT (the wait
/// carried a timeout and the durable timeout-timer fired before the signal arrived — the timeout branch,
/// §6.3). The HITL approval round-trip maps these to resume-and-run (`Signalled` approve) / withhold
/// (`Signalled` decline → 0 mutation, AG-8) / timeout (auto-deny).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaitOutcome {
    /// **The named signal arrived and was CONSUMED exactly once (§4.3).** Carries the signal's
    /// references-not-payloads `payload` (`ArtifactRef`s — e.g. the approval decision's effect refs, the
    /// job's result refs) and the `idem_key` the producer chose (`card_id` / the job's `idem_token`),
    /// so the body can branch (approve vs decline — read off the payload / the per-effect rule §6.4). On
    /// replay this returns the SAME journaled signal (consume-exactly-once across a re-drive).
    Signalled {
        /// the per-effect `idem_key` of the consumed signal (the producer's choice — `card_id` single,
        /// `card_id:idx` multi, a job's `idem_token` for a long-park).
        idem_key: String,
        /// the consumed signal's body as `ArtifactRef`s (references-not-payloads, §3.4) — never a PII
        /// body.
        payload: Vec<myelin_refs::ArtifactRef>,
        /// the crypto-shred key ref IF the consumed signal carried inline PII (the rare §3.4 case).
        payload_key_ref: Option<String>,
    },
    /// **The wait PARKED — the named signal has not arrived (§4.3).** The run is `waiting`, holding NO
    /// runtime, until `DurableExecutor::signal` delivers the named signal (which may be DAYS later,
    /// across restarts + deploys — the durability is the point, FLOW-D4). The engine settles the run
    /// `waiting`; the dispatcher re-drives it when the signal is delivered (the body re-issues the wait
    /// and finds the now-buffered signal). The body should RETURN promptly on a `Parked` (it made no
    /// progress past the wait).
    Parked,
    /// **The wait TIMED OUT — the durable timeout-timer fired before the signal arrived (§4.3/§6.3).**
    /// The wait carried a `timeout` and the durable `wf_timer` armed for it fired first. The body takes
    /// the timeout branch (the HITL round-trip's auto-deny → 0 mutation, AG-8). On replay this returns
    /// the SAME journaled timeout (the timeout is a deterministic outcome once journaled).
    TimedOut,
}

/// **The condition a parked drive is AWAITING (the park descriptor, P-FLOW signal/park race).** When a
/// drive settles the run `waiting`, this names WHAT the run is parked on so the durable commit can
/// close the signal/park race: a [`ParkCondition::Signal`] that ALREADY has a matching buffered signal
/// (one that landed while the drive was mid-flight) settles the run RUNNABLE instead of stranding it
/// `waiting` behind an unobserved signal. A [`ParkCondition::Timer`] is woken by the timer wheel, so the
/// commit does not re-check signals for it. Derived from the wait/sleep site (never from stored rows),
/// so a wait's exact `(signal_name, idem_key)` is threaded through the drive instead of discarded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParkCondition {
    /// **Parked on a `wait_for_signal` (§4.3).** `name` is the awaited signal name; `idem_key` is the
    /// EXACT per-effect key IFF the wait was keyed (`wait_for_signal_exact`, e.g. a CI stage's
    /// `job.done` keyed on the dispatch `idem_token`), or `None` for a name-only wait (the first
    /// unconsumed signal of that name resumes it). The durable commit re-checks the buffer under the
    /// run-row lock for a PENDING signal matching this exact descriptor.
    Signal {
        /// the awaited signal name.
        name: String,
        /// the exact awaited per-effect key (a keyed wait), or `None` for a name-only wait.
        idem_key: Option<String>,
    },
    /// **Parked on a durable timer (a `sleep` into the future, §4.2).** Woken by the timer wheel
    /// (`fire_due_timer`), never by a signal — the commit does not re-check the signal buffer for it.
    Timer {
        /// the deterministic `wf_timer.timer_id` the run parked on (`<run_id>/<command_id>`).
        timer_id: String,
    },
}

/// The marker prefix the `signal_waited`/`signal_received` rows encode the consumed signal's `idem_key`
/// under (so replay reconstructs the [`WaitOutcome::Signalled`] idem_key) — a machine token, no PII.
pub(crate) const WAIT_IDEM_PREFIX: &str = "myelin://flow/signal-idem/";
const LEGACY_WAIT_IDEM_PREFIX: &str = "wait:idem:";
/// The marker prefix a journaled `signal_received` encodes the consumed signal's `payload_key_ref`
/// under (the rare inline-PII crypto-shred ref) — a machine token, no PII.
pub(crate) const WAIT_KEYREF_PREFIX: &str = "myelin://flow/signal-key-ref/";
const LEGACY_WAIT_KEYREF_PREFIX: &str = "wait:keyref:";
/// The marker prefix a journaled `signal_waited` encodes the stable timeout DEADLINE under (so a resume
/// reads the SAME deadline across re-drives) — a machine token, no PII.
const WAIT_DEADLINE_PREFIX: &str = "wait:deadline:";
/// The exact idempotency key a keyed wait expects. New exact waits journal this alongside their
/// deadline so changing a DAG join key between drives is a replay divergence, not a chance match.
pub(crate) const WAIT_EXPECTED_IDEM_PREFIX: &str = "myelin://flow/wait-idem/";
pub(crate) const WAIT_EXPECTED_NAME_PREFIX: &str = "myelin://flow/wait-name/";
pub(crate) const WAIT_SIGNAL_NAME_PREFIX: &str = "myelin://flow/signal-name/";
/// The marker a TIMED-OUT wait journals as its `signal_received` idem_key + key_ref (so replay returns
/// [`WaitOutcome::TimedOut`] deterministically) — a machine token, no PII.
const WAIT_TIMEOUT_MARKER: &str = "wait:timeout";

/// Decode a journaled `signal_received` row's `result` back into a [`WaitOutcome`] (the replay short-
/// circuit, §4.1). A row whose idem-marker is [`WAIT_TIMEOUT_MARKER`] decodes to [`WaitOutcome::
/// TimedOut`]; otherwise it is a [`WaitOutcome::Signalled`] carrying the consumed signal's idem_key +
/// payload refs (+ the rare crypto-shred key ref). The encoding mirrors [`WfCtx::stage_received`].
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
            // Binding metadata, not workflow payload.
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

    /// All `wf_history` rows for a tenant, in commit order (the tenant-scoped journal scan the
    /// `PersonalDataHolder` `locate`/`export` over `wf_history` consumes — P-FLOW-03). Tenant-first:
    /// the DSR fan-out is per (subject, tenant), so the holder scopes its scan to ONE tenant's rows.
    pub fn history_in_tenant(&self, tenant: &TenantId) -> Vec<WfHistoryRow> {
        self.lock()
            .history
            .iter()
            .filter(|r| &r.tenant == tenant)
            .cloned()
            .collect()
    }

    /// **All `wf_history` rows across every tenant/run, in commit (append) order — the whole-journal scan
    /// the restore-verify (FLOW-D10 / P-FLOW-25, [`crate::restore_verify`]) takes the consistent-point cut
    /// over (§3.2).** Restore truncates the journal at the event-log offset by retaining the rows with
    /// `seq <= T` from THIS scan; the gate then asserts no retained row points at a vanished result. The
    /// rows are in the append order the journal committed them (the `seq` ordering the cut respects).
    pub fn all_history_in_seq_order(&self) -> Vec<WfHistoryRow> {
        self.lock().history.clone()
    }

    /// **Test/holder seam: append a `wf_history` row directly** (bypassing the [`WfCtx`] co-commit),
    /// so a unit/CDC test of a journal CONSUMER (the P-FLOW-03 holder's `locate`/`erase` over a
    /// populated journal — [`crate::holder`]) can seed the journal with a known set of refs-stored
    /// rows, then assert the structural-erase 0-mutation property, without standing up a whole run.
    /// Routes through the SAME private `commit_rows` (one write path, idempotent on
    /// `(tenant, run_id, command_id)` §3.2) — NO second store. NOT a production write path: the ONE
    /// production write path is [`WfCtx::commit`] (the FLOW-D5 co-commit). Mirrors Notif's
    /// `upsert_for_test` holder seam.
    #[doc(hidden)]
    pub fn append_history_for_test(&self, row: WfHistoryRow) {
        self.commit_rows(vec![row], Vec::new());
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
            let key = (
                row.tenant.0.clone(),
                row.run_id.clone(),
                row.command_id.clone(),
            );
            if inner.journaled_commands.insert(key) {
                inner.history.push(row);
            } else if row.kind == crate::wfctx::history_kind::SIGNAL_RECEIVED {
                // **A `signal_received` UPGRADES a journaled `signal_waited` at the SAME command_id
                // (§4.3).** A `wait_for_signal` that PARKED journals a provisional `signal_waited` row;
                // when a later drive RESUMES + consumes the named signal, the wait resolves to a
                // `signal_received` carrying the CONSUMED signal's idem_key + payload refs. The two are
                // the SAME command position (the `UNIQUE(tenant, run_id, command_id)` row), so the
                // receipt UPGRADES the park IN PLACE rather than appending a second row. This records
                // WHICH idem_key the wait consumed, so a subsequent replay short-circuits to the SAME
                // signal (consume-exactly-once) — essential when a body re-uses ONE signal NAME across
                // several waits (a CI pipeline's per-stage `job.done`, §4.9 item 6): without the
                // upgrade, a re-driven earlier wait would re-scan the buffer and wrongly consume a LATER
                // stage's still-buffered signal (the multi-signal-name reuse divergence). The upgrade is
                // idempotent: a re-driven receipt of an already-`signal_received` row is a no-op (the
                // first receipt's captured idem_key stays). NEVER downgrades a `signal_received` back to
                // a `signal_waited`.
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
            // else: UNIQUE(tenant, run_id, command_id) — already journaled (and not a park→receipt
            // upgrade), this is a no-op (idempotent journaling; the replay-safe property §3.2).
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
    /// **The loaded journal a replay short-circuits against (P-FLOW-05, §4.1).** Keyed by the
    /// deterministic `command_id`, it holds the prior run's journaled `wf_history` outcome rows so a
    /// re-driven run RETURNS the journaled result (an activity result, a `now()`/`rand()` marker's
    /// captured value) WITHOUT re-executing the side effect — the whole point of journaled-result
    /// replay (§4.1: "do NOT re-execute the side effect"). Empty for a cold (never-driven) run.
    replay_history: std::collections::HashMap<String, ReplayedCommand>,
    /// **The count of activity closures actually EXECUTED on this drive** (the double-effect probe).
    /// A pure replay of an N-command journal executes ZERO activity closures (every one
    /// short-circuits); only commands past the cursor execute. The FLOW-D1 drill reads this to assert
    /// "0 re-executed side effects" — a replay that re-runs a journaled activity increments this and
    /// fails the drill loudly (§4.1, the silent-double-effect floor).
    side_effects_executed: u64,
    /// **The count of journaled commands that got RE-EXECUTED on this drive (the 0-double-effect
    /// counter, §4.1).** It MUST stay 0; a non-zero would be a silent double-effect the FLOW-D1 drill
    /// reds on. The activity-replay short-circuit makes a re-execution of a MATCHING journaled command
    /// impossible, and the P-FLOW-07 divergence guard now HALTS a kind-MISMATCH (an `activity` issued
    /// at a `side_marker` position) as `nondeterministic` BEFORE any live execution — so this counter
    /// is 0 by construction (the divergence that once double-effected is now a halt). Retained as the
    /// direct, in-`WfCtx` 0-floor probe (a regression that re-runs a journaled command would trip it).
    double_effects: u64,
    /// **The replay-DIVERGENCE latch (P-FLOW-07, FLOW-D2).** Set to the FIRST detected divergence
    /// reason (a KIND mismatch at a journaled command position, or a pinned-`wf_version` mismatch) so
    /// the engine HALTS the run as `nondeterministic` instead of silently continuing. Once latched,
    /// the run is dead-lettered; the latch is sticky (a later command never CLEARS a divergence).
    /// `None` on a deterministic drive. This is the surface-level enforcement of the determinism
    /// contract (9.2): a body that diverges from its journal cannot be replayed, so it halts.
    divergence: Option<String>,
    /// **The `wf_version` the run is PINNED to (§4.6).** Set on [`WfCtx::resume_versioned`] to the
    /// version recorded on the run at start; the divergence guard compares it against the version of
    /// the definition the engine is REPLAYING with — a mismatch (a deploy bumped the definition while
    /// the run was in flight) is a divergence (the body shape may differ), so the run halts as
    /// `nondeterministic` rather than replaying a different definition over an old journal. `None`
    /// when no version pin is supplied (the version-divergence leg is not armed).
    pinned_wf_version: Option<i32>,
    /// **The durable-timer wheel a `sleep_until`/`sleep_for` arms into (P-FLOW-13, §4.2).** `None`
    /// until [`WfCtx::with_timers`] supplies the engine's [`crate::timer::TimerStore`] + the run's
    /// partition — a `WfCtx` built WITHOUT it (the pure activity/now/rand surface) cannot `sleep` (the
    /// arming target is absent). The dispatcher supplies it so a workflow body's `sleep` arms a
    /// durable `wf_timer` row the wheel then fires; the run parks (`waiting`) holding no runtime. The
    /// tuple carries `(store, partition, now_secs)` — the engine's live EPOCH-SECONDS clock so a
    /// `sleep_until(deadline)` parks iff `deadline > now_secs` and `sleep_for(d)` arms `now_secs + d`
    /// (the deterministic `now()` side-marker is the BODY's RFC-3339 clock; this is the engine's
    /// lease/wheel clock — [`crate::engine::FlowDispatcher::tick`]'s `now: i64`).
    timers: Option<(crate::timer::TimerStore, i16, i64)>,
    /// **Whether this drive PARKED on a durable timer — a `sleep` that armed a not-yet-due timer
    /// (§4.2).** Set by [`WfCtx::sleep_until`] when the deadline is in the future (the run must wait):
    /// the engine reads it to settle the run `waiting` (holding NO runtime) instead of `completed`.
    /// `false` on a drive with no live sleep (or a `sleep` whose deadline already passed — it returns
    /// immediately, no park).
    parked_on_timer: bool,
    /// **The durably-buffered `wf_signal` store a `wait_for_signal` consumes from (P-FLOW-11, §4.3).**
    /// `None` until [`WfCtx::with_signals`] supplies the engine's [`crate::engine::SignalStore`] — a
    /// `WfCtx` built WITHOUT it (the pure activity/now/rand/sleep surface) cannot `wait_for_signal`
    /// (the buffer to consume from is absent), so the wait returns a loud [`WfError::CoCommit`] rather
    /// than silently no-op-ing. The dispatcher supplies it so a body's `wait_for_signal` consumes the
    /// signal `DurableExecutor::signal` buffered (P-FLOW-09) and parks the run (`waiting`) when the
    /// signal has not arrived yet.
    signals: Option<crate::engine::SignalStore>,
    /// **Whether this drive PARKED on a `wait_for_signal` — a wait whose named signal is not yet
    /// buffered (§4.3).** Set by [`WfCtx::wait_for_signal`] when no buffered signal is found (the run
    /// must wait, holding NO runtime, until `DurableExecutor::signal` delivers it). The engine reads it
    /// to settle the run `waiting` instead of `completed`. `false` on a drive whose waits all found a
    /// buffered signal (or that issued no wait). This is the multi-day-HITL state=waiting holds no
    /// runtime property (FLOW-D4).
    parked_on_signal: bool,
    /// **The park descriptor — WHAT this drive parked on (the signal/park race fix).** `Some` iff this
    /// drive settled `waiting`: a [`ParkCondition::Signal`] naming the awaited signal (+ exact
    /// `idem_key` for a keyed wait) OR a [`ParkCondition::Timer`] naming the awaited timer. Threaded
    /// into the durable [`crate::DriveCommit`] so the commit can settle a run RUNNABLE (instead of
    /// stranded `waiting`) when a matching signal already landed mid-drive. Last-park-wins (a drive
    /// returns promptly after the park that leaves the run waiting).
    park_condition: Option<ParkCondition>,
    /// **The per-effect `idem_key`s a `wait_for_signal` CONSUMED on this drive (P-FLOW-11, §4.3).** Each
    /// entry is `(signal_name, idem_key)` — the buffered `wf_signal` row a wait consumed (stamped
    /// `consumed_seq`). The engine reads it to refresh the signal-buffer-depth telemetry after a drive
    /// (a consumed signal drops the buffered depth). The FLOW-D4 drill asserts EXACTLY ONE consume per
    /// delivered approval (a double-click delivers one buffered row → one consume).
    consumed_signals: Vec<(String, String)>,
    /// Exact command-to-signal bindings exported by the PostgreSQL drive adapter. The in-memory
    /// dispatcher only needs the pair above; the durable commit must additionally name the
    /// journal command whose `signal_waited` row is upgraded under the same transaction.
    consumed_signal_commands: Vec<ConsumedSignalCommand>,
    /// **The reserve/settle bookend the spend-bearing dispatches reserve/settle against (P-FLOW-16,
    /// contract 9.5/§4.9).** `None` on an UN-METERED `WfCtx` (the pure activity/now/rand/sleep/signal
    /// surface) — a `metered_activity`/`metered_schedule_and_run_job` then runs WITHOUT a reserve (the
    /// loop-cap depth is still the runaway bound, AG-6). Supplied via [`WfCtx::with_budget`] from the
    /// run's [`crate::RunBudget`] so a body's spend-bearing dispatch reserves-at-dispatch (no balance →
    /// no dispatch) + settles-on-completion into the SAME wallet, never interrupting in-flight.
    pub(crate) budget: Option<crate::budget::BudgetGate>,
    /// **The per-run mint context a resume re-mints a fresh token against (P-FLOW-17, contract 4.7,
    /// §6.2).** `None` on a `WfCtx` with no run-identity wired (a body that never crosses a multi-day
    /// wait, or a unit test of the pure activity surface) — [`WfCtx::remint_on_resume`] then returns a
    /// loud [`WfError::CoCommit`] rather than silently running a resumed activity under no/expired
    /// token. Supplied via [`WfCtx::with_run_identity`] so the resume legs of [`WfCtx::wait_for_signal`]
    /// and [`WfCtx::schedule_and_run_job`] re-mint a short-lived attenuated per-run token (token life
    /// equals activity life, NOT the days-long workflow life — the workflow holds no long-lived
    /// privileged token across a wait).
    pub(crate) run_identity: Option<crate::remint::RunTokenLease>,
    /// **The count of fresh per-run tokens re-minted on this drive (the §6.2 re-mint probe,
    /// P-FLOW-17).** Each resume across a multi-day wait re-mints exactly one fresh short-lived token;
    /// the gate reads it to assert a resume DID re-mint. `0` on a drive that never resumed (a cold first
    /// drive that parked, or a pure non-waiting body).
    pub(crate) reminted_tokens: u64,
    /// Dispatch identities reconstructed from journaled activity results during this deterministic
    /// drive. Job joins validate opaque handles against this registry and enforce earliest-deadline
    /// ordering, so caller material cannot select a sibling or extend its SLA.
    pub(crate) job_dispatches: std::collections::HashMap<String, (String, Option<i64>, String)>,
    pub(crate) joined_job_dispatches: std::collections::HashSet<String>,
    pub(crate) disarmed_timer_ids: Vec<String>,
}

/// One journaled command outcome a replay reads back (§4.1) — the result an `activity` returns on
/// replay, or the captured value a `now()`/`rand()` side-marker journaled. Carried so a re-driven
/// `WfCtx` returns the SAME value the original drive produced, WITHOUT re-executing the side effect.
#[derive(Clone, Debug)]
struct ReplayedCommand {
    /// The existing journal sequence. A `signal_waited` -> `signal_received` upgrade rewrites this
    /// exact row; allocating a new sequence would violate the PostgreSQL journal fence.
    seq: i64,
    /// The `wf_history.kind` of the journaled row (`activity_completed`/`activity_failed`/
    /// `side_marker`) — the replay branch selector.
    kind: String,
    /// The journaled activity result refs (for `activity_completed`) — returned verbatim on replay.
    result: Option<Vec<myelin_refs::ArtifactRef>>,
}

/// Exact signal consumed by one staged workflow command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsumedSignalCommand {
    pub command_id: String,
    pub signal_name: String,
    pub idem_key: String,
}

/// Pure output of a detached workflow drive. Nothing in this value is durable until a caller
/// moves every row into one caller-owned transaction (the PostgreSQL drive store does that).
#[derive(Clone, Debug)]
pub struct StagedWfDrive {
    pub history: Vec<WfHistoryRow>,
    pub attempts: Vec<WfActivityAttemptRow>,
    pub timers: Vec<crate::timer::TimerRow>,
    pub outbox: Vec<myelin_events::OutboxRow>,
    pub consumed_signals: Vec<ConsumedSignalCommand>,
    pub disarmed_timer_ids: Vec<String>,
    /// **The park descriptor this drive settled on (the signal/park race fix).** `Some` iff the drive
    /// parked (`waiting`); threaded into the durable [`crate::DriveCommit`] so the commit can settle a
    /// run runnable when a matching signal already landed mid-drive. `None` on a non-parking drive.
    pub park: Option<ParkCondition>,
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

    /// **Supply the durable-timer wheel + the run's partition so this `WfCtx` can `sleep` (P-FLOW-13,
    /// §4.2).** A `WfCtx` built without this (the pure `activity`/`now`/`rand`/`emit` surface) cannot
    /// arm a durable timer — [`WfCtx::sleep_until`]/[`WfCtx::sleep_for`] return a [`WfError::CoCommit`]
    /// naming the missing wheel rather than silently no-op-ing (a sleep that did nothing would be a
    /// silent correctness bug, EI-01 §2). The dispatcher calls this when it builds the drive's `WfCtx`
    /// so a workflow body's `sleep` arms a `wf_timer` row the wheel fires. `now_secs` is the engine's
    /// live epoch-seconds clock (the lease/wheel clock) the park decision + `sleep_for`'s relative base
    /// read. Chainable on `begin`/`resume`.
    pub fn with_timers(
        mut self,
        timers: crate::timer::TimerStore,
        partition: i16,
        now_secs: i64,
    ) -> Self {
        self.timers = Some((timers, partition, now_secs));
        self
    }

    /// **Supply the durably-buffered `wf_signal` store so this `WfCtx` can `wait_for_signal` (P-FLOW-11,
    /// §4.3).** A `WfCtx` built without this (the pure activity/now/rand/sleep surface) cannot consume a
    /// durable signal — [`WfCtx::wait_for_signal`] returns a [`WfError::CoCommit`] naming the missing
    /// store rather than silently no-op-ing (a wait that did nothing would be a silent correctness bug,
    /// EI-01 §2). The dispatcher calls this when it builds the drive's `WfCtx` so a body's
    /// `wait_for_signal` consumes the signal `DurableExecutor::signal` buffered (P-FLOW-09) and parks
    /// the run (`waiting`) when no signal has arrived. Chainable on `begin`/`resume`.
    pub fn with_signals(mut self, signals: crate::engine::SignalStore) -> Self {
        self.signals = Some(signals);
        self
    }

    /// **Resume (re-drive) a `WfCtx` from its journaled `wf_history` — the deterministic
    /// replay/recovery entry point (P-FLOW-05, §4.1).** Identical to [`WfCtx::begin`] but LOADS the
    /// run's prior journal so each subsequent `activity`/`now`/`rand` SHORT-CIRCUITS its journaled
    /// command (returning the journaled outcome WITHOUT re-executing the side effect) and the run
    /// CONTINUES from the first un-journaled command. The `history` is the rows read back from the
    /// journal (`journal.history_for(tenant, run_id)`), in replay order.
    ///
    /// This is what makes crash recovery work (§4.7): a worker that re-leases a half-driven run calls
    /// `resume`, replays every journaled step (0 re-execution of side effects — the result was
    /// journaled, not re-run), and resumes "as if nothing happened". The per-run command counter +
    /// `seq` advance past the journaled rows so a newly-journaled command lands AFTER the replayed
    /// ones (the cursor floor, §3.1).
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
        // The replay-order seq continues past the journaled rows (the cursor floor §3.1): a
        // newly-journaled command must land AFTER everything replayed, never overwrite it.
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

    /// **Resume a `WfCtx` from its journal WITH the version pin armed (P-FLOW-07, §4.6).** Identical
    /// to [`WfCtx::resume`] but records the run's pinned `wf_version` (`run_version`) AND the version
    /// the engine is REPLAYING with (`replay_version`). If they MISMATCH — a deploy bumped the
    /// definition while the run was in flight — the divergence guard immediately latches a
    /// `nondeterministic` halt (the body shape may differ; replaying a new definition over an old
    /// journal is a silent divergence, §4.6). A MATCH arms the per-command kind-divergence guard
    /// over the journal (a body that issues a command whose kind differs from the journaled command at
    /// that position halts). Use this on the replay/recovery path where the run's pinned version is
    /// known; [`WfCtx::resume`] is the version-agnostic form (the kind guard still applies).
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
        // **Version-divergence guard (§4.6):** the run was pinned to `run_version` at start; if the
        // engine is replaying a DIFFERENT definition version, halt as nondeterministic BEFORE running
        // a single command — replaying a new body over an old journal is a silent divergence. The
        // latch is set eagerly so even an empty body (no commands) halts.
        if run_version != replay_version {
            ctx.latch_divergence(format!(
                "wf_version pin mismatch: run pinned to v{run_version} but replayed with v{replay_version} \
                 (a deploy diverged an in-flight run, §4.6)"
            ));
        }
        ctx
    }

    /// Resume into a pure staging buffer for a larger PostgreSQL drive transaction. This is not a
    /// second persistence implementation: emitted rows remain unallocated and unpublished, and
    /// [`WfCtx::into_staged_drive`] is the only successful terminal operation.
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

    /// **The replay-DIVERGENCE verdict (P-FLOW-07, FLOW-D2).** `Some(reason)` if this drive detected a
    /// divergence (a kind mismatch at a journaled position, or a pinned-`wf_version` mismatch) — the
    /// engine reads it to HALT the run as `nondeterministic` + dead-letter it, never silently continue.
    /// `None` on a deterministic drive. The reason is a machine string (no PII).
    pub fn divergence(&self) -> Option<&str> {
        self.divergence.as_deref()
    }

    /// Whether this drive diverged (P-FLOW-07) — the engine's halt predicate.
    pub fn is_divergent(&self) -> bool {
        self.divergence.is_some()
    }

    /// Latch the FIRST divergence reason (sticky — a later command never clears it). Idempotent on a
    /// re-latch: only the first reason is kept (the position the body first diverged).
    pub(crate) fn latch_divergence(&mut self, reason: String) {
        if self.divergence.is_none() {
            self.divergence = Some(reason);
        }
    }

    /// Declare a replay divergence: latch it sticky, then produce the halt error. One spelling for
    /// "the run's history no longer matches — halt non-deterministically."
    pub(crate) fn diverge(&mut self, reason: String) -> WfError {
        self.latch_divergence(reason.clone());
        WfError::Nondeterministic(reason)
    }

    /// Halt early if a divergence was already latched (the fn-top precheck).
    fn halt_if_diverged(&self) -> WfResult<()> {
        match self.divergence.clone() {
            Some(r) => Err(WfError::Nondeterministic(r)),
            None => Ok(()),
        }
    }

    /// The number of activity closures actually EXECUTED on this drive (the FLOW-D1 double-effect
    /// probe). A pure replay of a fully-journaled run reads `0` (every command short-circuited); a
    /// regression that re-executes a journaled activity reads `> 0` and reds the drill.
    pub fn side_effects_executed(&self) -> u64 {
        self.side_effects_executed
    }

    /// **The 0-double-effect counter (the FLOW-D1 floor, §4.1).** The number of JOURNALED commands
    /// that nonetheless reached LIVE execution on this drive (a re-executed side effect). The
    /// activity-replay short-circuit makes this 0 by construction; a regression that re-runs a
    /// journaled activity increments it and the drill reds. MUST be 0.
    pub fn double_effects(&self) -> u64 {
        self.double_effects
    }

    /// The deterministic `command_id` for the NEXT command (`<wf_type>:<n>`) — the replay-match key
    /// (§3.2). Increments the per-run command counter. Producer and consumer agree on the key
    /// WITHOUT coordination because it is derived purely from the workflow position.
    fn next_command_id(&mut self) -> String {
        let id = format!("{}:{}", self.wf_type, self.command_seq);
        self.command_seq += 1;
        id
    }

    /// **PEEK the `command_id` the NEXT command will use — WITHOUT consuming the counter (§3.2).**
    /// The [`SCHEDULE_AND_RUN_JOB`](WfCtx::schedule_and_run_job) idiom (P-FLOW-15, §4.9) needs the
    /// dispatch position's `command_id` to mint its DETERMINISTIC `idem_token` BEFORE the dispatch
    /// `activity` consumes the counter — so the wait (the next command) and a re-drive both reconstruct
    /// the SAME token. This returns the SAME value [`WfCtx::next_command_id`] would, but leaves the
    /// counter untouched (the `activity` that follows consumes it for real).
    pub(crate) fn peek_next_command_id(&self) -> String {
        format!("{}:{}", self.wf_type, self.command_seq)
    }

    /// Whether the deterministic command position is already present in the loaded durable
    /// journal. Side resources co-committed with that row (such as a dispatch-time SLA timer) must
    /// not be recreated during replay.
    pub(crate) fn is_replaying_command(&self, command_id: &str) -> bool {
        self.replay_history.contains_key(command_id)
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
        // **DIVERGENCE HALT (P-FLOW-07).** Once a divergence is latched (a prior kind mismatch, or a
        // pinned-`wf_version` mismatch armed at resume), NO further command executes — the body is not
        // a deterministic function of its journal, so the run halts as `nondeterministic` rather than
        // making more (possibly double-effecting) progress. Surface the latched reason.
        self.halt_if_diverged()?;
        let command_id = self.next_command_id();
        // **REPLAY SHORT-CIRCUIT (§4.1).** If this command is already journaled, RETURN the journaled
        // outcome WITHOUT re-executing the closure — the side effect is NOT re-run (the result was
        // journaled, not the activity re-executed). This is the heart of crash recovery: a re-driven
        // run replays every journaled step with 0 double-effect, then continues from the first
        // un-journaled command. The closure body never runs, so `side_effects_executed` is NOT bumped.
        if let Some(replayed) = self.replay_history.get(&command_id) {
            match replayed.kind.as_str() {
                history_kind::ACTIVITY_COMPLETED => {
                    return Ok(replayed.result.clone().unwrap_or_default());
                }
                history_kind::ACTIVITY_FAILED => {
                    // A journaled failure replays to the same failure (the run takes its error
                    // branch deterministically) — still 0 re-execution of the activity.
                    return Err(WfError::ActivityExhausted(ActivityError(
                        "replayed activity_failed".into(),
                    )));
                }
                // **THE REPLAY-DIVERGENCE GUARD (P-FLOW-07, FLOW-D2).** This command_id journaled a
                // NON-activity kind (e.g. a `side_marker`) but the body is now issuing an `activity` at
                // the same position — the body DIVERGED from its journal. This is a determinism
                // violation: the body is not the deterministic function of its journal the replay
                // contract (9.2) requires. We HALT — latch `nondeterministic` and return the verdict —
                // rather than execute the activity LIVE against a journaled position (which would be a
                // silent double-effect). 0 silent divergence: the guard halts, never silent-continues
                // (EI-01 §3 — a red gate is information; never invert an assertion).
                other => {
                    return Err(self.diverge(format!(
                        "replay divergence at {command_id}: body issued `activity` but the journal \
                         records kind `{other}` (the workflow body diverged from its journal)"
                    )));
                }
            }
        }
        // LIVE: this command is past the cursor — execute it for real (and count the side effect).
        self.side_effects_executed += 1;
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

    /// **`run_id()` — the run this body is executing (read-only).** A pure accessor (NOT journaled —
    /// the run id is structural, fixed for the run's lifetime), so a body can build per-run keys (e.g.
    /// the §6.4 per-effect approval card's `run_id` the gated loop reads its buffered signals under).
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// The run's [`TenantId`] — the partition key the reserve/settle bookend (P-FLOW-16) keys its
    /// ledger reservations under (there is no cross-tenant ledger path; §1.1). Fixed for the run's
    /// lifetime.
    pub(crate) fn tenant_id(&self) -> &TenantId {
        &self.tenant
    }

    /// **`now()` (contract 9.2, §5.1) — a journaled SIDE-MARKER.** Returns the deterministic
    /// RFC-3339 UTC clock for this run AND journals a `side_marker` `wf_history` row capturing it,
    /// so on replay (P-FLOW-05) the workflow body reads back the SAME timestamp — making a
    /// `now()`-dependent workflow deterministic. The marker is STAGED (durable iff commit).
    pub fn now(&mut self) -> String {
        // DIVERGENCE HALT (P-FLOW-07): a latched divergence freezes the side-marker too — the captured
        // clock is meaningless once the body has diverged. Return the run's clock unchanged (the engine
        // already halts the run via the latch; a `now()` mid-divergence never makes durable progress).
        if self.is_divergent() {
            return self.now_clock.clone();
        }
        let command_id = self.next_command_id();
        // **THE REPLAY-DIVERGENCE GUARD (P-FLOW-07, FLOW-D2):** if this position IS journaled but NOT
        // as a `side_marker` (the body issues `now()` where the journal records an activity), the body
        // diverged from its journal — latch `nondeterministic` and return the live clock (the engine
        // halts the run on the latch). Never silently recompute against a journaled activity position.
        if self.divergent_marker(&command_id) {
            return self.now_clock.clone();
        }
        // REPLAY SHORT-CIRCUIT (§4.1): a journaled `now()` side-marker returns the CAPTURED clock so
        // replay reads back the SAME timestamp (the worker's wall-clock at FIRST execution), making a
        // `now()`-dependent workflow deterministic even though the replay-time clock differs.
        if let Some(value) = self.replayed_marker_value(&command_id) {
            return value;
        }
        // LIVE: capture the deterministic clock INTO the side-marker (so the value, not the
        // resume-time clock, drives replay).
        let value = self.now_clock.clone();
        self.stage_marker_value(command_id, &value);
        value
    }

    /// **`rand()` (contract 9.2, §5.1) — a journaled SIDE-MARKER.** Returns the next value of a
    /// deterministic, replay-stable sequence (a splitmix64 step over the seeded state — NOT a
    /// source of entropy; it is replay-stable BY DESIGN) AND journals a `side_marker` row capturing
    /// the draw, so replay returns the SAME number. The marker is STAGED (durable iff commit).
    pub fn rand(&mut self) -> u64 {
        // DIVERGENCE HALT (P-FLOW-07): a latched divergence freezes the draw (the body already halted).
        if self.is_divergent() {
            return 0;
        }
        let command_id = self.next_command_id();
        // **THE REPLAY-DIVERGENCE GUARD (P-FLOW-07, FLOW-D2):** a `rand()` at a position journaled as a
        // NON-side-marker (an activity) is a body that diverged from its journal — latch and halt.
        if self.divergent_marker(&command_id) {
            return 0;
        }
        // REPLAY SHORT-CIRCUIT (§4.1): a journaled `rand()` side-marker returns the CAPTURED draw so
        // replay reads back the SAME number (even if the resume seed differs) — the draw is a side
        // effect, captured-not-recomputed.
        if let Some(value) = self.replayed_marker_value(&command_id) {
            return value.parse::<u64>().unwrap_or(0);
        }
        // splitmix64 — a deterministic, well-mixed sequence (replay-stable; the journaled marker is
        // what makes it correct under replay, the sequence itself is just reproducible).
        self.rand_state = self.rand_state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.rand_state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        let value = z ^ (z >> 31);
        // LIVE: capture the drawn value INTO the side-marker (the value drives replay, not the seed).
        self.stage_marker_value(command_id, &value.to_string());
        value
    }

    /// If the side-marker `command_id` is already journaled, return its CAPTURED value (the string
    /// the original draw stored in the marker's `result` ref) — the replay short-circuit for
    /// `now()`/`rand()` (§4.1). `None` for a live (un-journaled) command.
    /// **The side-marker DIVERGENCE check (P-FLOW-07, FLOW-D2).** Returns `true` (AND latches the
    /// divergence) IFF `command_id` IS journaled but as a NON-`side_marker` kind — i.e. the body
    /// issued a `now()`/`rand()` at a position the journal records as an activity. That is a body
    /// that diverged from its journal (a determinism violation); the caller returns a halt-time
    /// value and the engine halts the run as `nondeterministic` via the latch. Returns `false` for a
    /// matching `side_marker` (the normal replay) or an un-journaled (live) position.
    fn divergent_marker(&mut self, command_id: &str) -> bool {
        let Some(replayed) = self.replay_history.get(command_id) else {
            return false; // un-journaled — a live side-marker, no divergence.
        };
        if replayed.kind == history_kind::SIDE_MARKER {
            return false; // the expected kind — a normal marker replay.
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

    /// Stage a `side_marker` `wf_history` row CARRYING its captured value (encoded as a single
    /// [`ArtifactRef`] in the row's `result`) so replay returns the captured value, not a recomputed
    /// one — the determinism levers `now()`/`rand()` depend on (§4.1/§5.1).
    fn stage_marker_value(&mut self, command_id: String, value: &str) {
        self.stage_history(
            history_kind::SIDE_MARKER,
            command_id,
            Some(vec![myelin_refs::ArtifactRef(value.to_string())]),
        );
    }

    /// **`sleep_until(fire_at_secs)` (contract 9.2, §4.2/§9.2) — arm a durable timer + park.** Arms a
    /// durable `wf_timer` row in its minute `bucket = epoch_minute(fire_at)` (idempotent on the
    /// deterministic `timer_id = <run_id>/<command_id>`, so a replayed `sleep` never double-arms),
    /// journals a `timer_set` `wf_history` side row under the command position (the replay short-circuit
    /// reads it back so the body issues NO second arm on re-drive), and PARKS the run: the workflow
    /// holds NO runtime while it waits — it is a `wf_timer` row + a `waiting` run, not a thread (the
    /// SC-11 substrate). The wheel ([`crate::timer::TimerWheel`]) fires it at its minute, wakes the run
    /// (`waiting → running`), and the dispatcher re-drives past the `sleep`. A crash re-fires only the
    /// unfired timer (effectively-once).
    ///
    /// **Replay (§4.1):** on re-drive, the `timer_set` command short-circuits — the `sleep` returns
    /// WITHOUT re-arming (the timer is already on the wheel; the journaled command is replayed, not
    /// re-issued). The arming is the live (first-drive) effect; the journal makes it replay-safe.
    ///
    /// **Returns:** `Ok(())` once the timer is armed/journaled. If the deadline is already PAST
    /// (`fire_at <= now`) on the live drive, the timer is armed in bucket 0 (immediately due) and the
    /// run does NOT park (the next wheel tick fires it) — a `sleep` into the past is a no-wait
    /// continuation. A [`WfError::CoCommit`] is returned if no timer wheel was supplied
    /// ([`WfCtx::with_timers`]) — a `sleep` with no wheel is a loud error, never a silent no-op.
    /// `fire_at_secs` is the absolute deadline in epoch seconds (the §5.1 units convention the engine's
    /// in-memory clock uses); the live drive clock the park decision reads is the engine's `now_secs`
    /// (supplied via [`WfCtx::with_timers`]).
    pub fn sleep_until(&mut self, fire_at_secs: i64) -> WfResult<()> {
        // DIVERGENCE HALT (P-FLOW-07): a latched divergence freezes the sleep too (the run already halts).
        self.halt_if_diverged()?;
        let command_id = self.next_command_id();
        // REPLAY SHORT-CIRCUIT (§4.1): a journaled `timer_set` returns WITHOUT re-arming — the timer is
        // already on the wheel (armed on the first drive). The body issues no second arm on re-drive.
        if let Some(replayed) = self.replay_history.get(&command_id) {
            match replayed.kind.as_str() {
                crate::timer::history_kind::TIMER_SET => return Ok(()),
                // **DIVERGENCE GUARD (P-FLOW-07, FLOW-D2):** a `sleep` at a position journaled as a
                // NON-`timer_set` kind (an activity / a side-marker) is a body that diverged from its
                // journal — latch the divergence + halt (never re-arm against a journaled activity).
                other => {
                    return Err(self.diverge(format!(
                        "replay divergence at {command_id}: body issued `sleep` but the journal records \
                         kind `{other}` (the workflow body diverged from its journal)"
                    )));
                }
            }
        }
        // LIVE: arm the durable timer + journal the timer_set marker.
        let (timers, partition, now_secs) = self.timers.clone().ok_or_else(|| {
            WfError::CoCommit("sleep_until requires a timer wheel (WfCtx::with_timers)".into())
        })?;
        // The deterministic timer_id = <run_id>/<command_id> (so a replayed sleep re-arms the SAME key
        // → ON CONFLICT DO NOTHING; producer + consumer agree without coordination, §3.3).
        let timer_id = format!("{}/{}", self.run_id, command_id);
        let bucket = crate::timer::epoch_minute(fire_at_secs);
        timers.arm(crate::timer::TimerRow {
            tenant: self.tenant.clone(),
            region: self.region.clone(),
            timer_id: timer_id.clone(),
            run_id: Some(self.run_id.clone()),
            command_id: command_id.clone(),
            fire_at: fire_at_secs,
            bucket,
            fired: false,
            partition,
        });
        // Journal the timer_set side row (the replay short-circuit reads it back) — references-not-
        // payloads (the marker carries no result body). STAGED — durable iff commit (FLOW-D5).
        self.stage_history(crate::timer::history_kind::TIMER_SET, command_id, None);
        // PARK iff the deadline is in the FUTURE: the run waits (holding no runtime) until the wheel
        // fires the timer. A deadline already past arms immediately-due (bucket 0) and does NOT park —
        // the next wheel tick fires it (a sleep into the past is a no-wait continuation, §4.2).
        if fire_at_secs > now_secs {
            self.parked_on_timer = true;
            // Record the timer park descriptor (the wheel wakes it — the commit does not re-check the
            // signal buffer for a timer park).
            self.park_condition = Some(ParkCondition::Timer { timer_id });
        }
        Ok(())
    }

    /// **`sleep_for(duration_secs)` (contract 9.2, §4.2/§9.2) — sleep a RELATIVE duration.** Computes
    /// the absolute deadline `now + duration` and arms a durable timer via [`WfCtx::sleep_until`]. The
    /// SAME park/replay/effectively-once semantics — a relative `sleep_for(30 days)` arms a far-future
    /// timer that costs nothing until its minute (the SC-11 substrate). `duration_secs` is in seconds
    /// (the §5.1 units convention); a non-positive duration is a no-wait continuation (the deadline is
    /// now/past). The relative base is the engine's live `now_secs` (supplied via [`WfCtx::with_timers`]).
    pub fn sleep_for(&mut self, duration_secs: i64) -> WfResult<()> {
        let now_secs = self.timers.as_ref().map(|(_, _, n)| *n).unwrap_or(0);
        let fire_at = now_secs.saturating_add(duration_secs.max(0));
        self.sleep_until(fire_at)
    }

    /// The drive clock used for durable timer deadlines. Job dispatch records deadlines against
    /// this clock before a parallel branch is joined.
    pub(crate) fn drive_now_unix_secs(&self) -> i64 {
        self.timers.as_ref().map(|(_, _, now)| *now).unwrap_or(0)
    }

    /// **`wait_for_signal(name, timeout)` (contract 9.2/9.4, §4.3 — the multi-day HITL wait).** PARKS
    /// the run on a named durable signal (`approval` / `cancel` / `ci.result` / `job.done`, §4.3): the
    /// workflow is `state=waiting` holding NO runtime until `DurableExecutor::signal` delivers the named
    /// signal — which may be DAYS later, across worker restarts + deploys (the durability is the point,
    /// FLOW-D4). When the signal arrives, the wait CONSUMES it EXACTLY ONCE (stamps `consumed_seq`,
    /// journals a `signal_received` row carrying the consumed payload) and RESUMES with [`WaitOutcome::
    /// Signalled`]. An optional `timeout` (seconds) arms a durable timeout-timer (P-FLOW-13); if it
    /// fires before the signal arrives, the wait returns [`WaitOutcome::TimedOut`] (the §6.3 auto-deny
    /// branch).
    ///
    /// **The round-trip (§6.3):** a gated tool calls `wait_for_signal("approval:<call>", timeout)`; a
    /// human clicks Approve/Deny days later; Chat calls `DurableExecutor::signal(run, "approval:<call>",
    /// {decision}, idem_key)`; the buffered signal wakes the run; the wait consumes it and the body runs
    /// (approve) / withholds (decline → 0 mutation, AG-8) / takes the timeout path. The CARD UX/visual is
    /// Chat+Agent-Fabric product work (OQ #1) — NOT this engine; this engine owns the wait + the
    /// consume-once + the park.
    ///
    /// **Replay (§4.1):** a `signal_received` command short-circuits to the SAME journaled signal (the
    /// wait returns the journaled payload, NEVER re-consuming a second buffered row — consume-exactly-
    /// once across a re-drive). A `signal_waited` command re-checks the live buffer: a signal that
    /// arrived since the park resumes (consumes + journals `signal_received` at the NEXT seq); an absent
    /// one re-parks (the run stays `waiting`); a fired timeout returns `TimedOut`.
    ///
    /// **Returns** [`WaitOutcome::Signalled`] (consumed), [`WaitOutcome::Parked`] (the run waits — the
    /// body should return promptly), or [`WaitOutcome::TimedOut`]. A [`WfError::CoCommit`] is returned if
    /// no signal store was supplied ([`WfCtx::with_signals`]) — a wait with no buffer is a loud error,
    /// never a silent no-op. A [`WfError::Nondeterministic`] is latched if the journal records a
    /// different command kind at this position (the divergence guard, P-FLOW-07).
    pub fn wait_for_signal(
        &mut self,
        name: &str,
        timeout_secs: Option<i64>,
    ) -> WfResult<WaitOutcome> {
        self.wait_for_signal_inner(name, None, timeout_secs, None, true)
    }

    /// Wait for one exact `(signal_name, idem_key)` pair. This is the join primitive for parallel
    /// workflow branches: a buffered completion for a sibling branch remains buffered until the
    /// sibling's own exact wait consumes it.
    pub fn wait_for_signal_exact(
        &mut self,
        name: &str,
        idem_key: &str,
        timeout_secs: Option<i64>,
    ) -> WfResult<WaitOutcome> {
        self.wait_for_signal_inner(name, Some(idem_key), timeout_secs, None, true)
    }

    /// Wait for an exact signal with an absolute deadline fixed by an earlier dispatch. The
    /// deadline is not rebased when the workflow eventually reaches this join, so queued DAG work
    /// cannot silently extend a runner SLA.
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
        // DIVERGENCE HALT (P-FLOW-07): a latched divergence freezes the wait (the run already halts).
        self.halt_if_diverged()?;
        let command_id = self.next_command_id();

        // **REPLAY SHORT-CIRCUIT (§4.1).** A `signal_received` at this position returns the SAME
        // journaled signal (consume-exactly-once) — NEVER re-scans the buffer (a second buffered row
        // under a different key would otherwise be wrongly consumed on re-drive). A `signal_waited`
        // re-checks the live buffer below (a signal may have arrived since the park). Any OTHER
        // journaled kind is a body that diverged from its journal → latch + halt.
        if let Some(replayed) = self.replay_history.get(&command_id).cloned() {
            match replayed.kind.as_str() {
                history_kind::SIGNAL_RECEIVED => {
                    // The journaled consumed signal: idem_key + payload were captured in the row's
                    // result (the first ref is the idem_key marker, the rest are the payload refs).
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
                // a journaled `signal_waited` — fall through to the live re-check (the resume path).
                crate::wfctx::history_kind::SIGNAL_WAITED => {}
                other => {
                    return Err(self.diverge(format!(
                        "replay divergence at {command_id}: body issued `wait_for_signal` but the \
                         journal records kind `{other}` (the workflow body diverged from its journal)"
                    )));
                }
            }
        }

        // LIVE (or the resume re-check of a journaled `signal_waited`): scan the durable buffer for the
        // first unconsumed signal under (tenant, run, name). The signal store is REQUIRED — a wait with
        // no buffer is a loud error (never a silent no-op).
        let signals = self.signals.clone().ok_or_else(|| {
            WfError::CoCommit(
                "wait_for_signal requires a signal store (WfCtx::with_signals)".into(),
            )
        })?;

        // Whether this command was already journaled as a `signal_waited` (a resume re-check) — so we do
        // NOT journal a SECOND `signal_waited` for the same park (idempotent on the command position).
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

        let now_secs = self.timers.as_ref().map(|(_, _, n)| *n).unwrap_or(0);
        let effective_deadline =
            (timeout_secs.is_some() || absolute_deadline.is_some()).then(|| {
                if already_waited {
                    self.replayed_wait_deadline(&command_id).unwrap_or_else(|| {
                        absolute_deadline.unwrap_or_else(|| {
                            now_secs.saturating_add(timeout_secs.unwrap_or_default())
                        })
                    })
                } else {
                    absolute_deadline.unwrap_or_else(|| {
                        now_secs.saturating_add(timeout_secs.unwrap_or_default())
                    })
                }
            });

        let candidate = match expected_idem_key {
            Some(idem_key) => signals
                .unconsumed_for_exact(&self.tenant, &self.run_id, name, idem_key)
                .map(|row| (idem_key.to_string(), row)),
            None => signals.first_unconsumed_for(&self.tenant, &self.run_id, name),
        };
        if let Some((idem_key, row)) = candidate {
            // A result received exactly at the deadline is accepted; one received even a
            // millisecond later loses to the timeout. Receipt time, not timer-wheel processing
            // order, decides this race.
            if effective_deadline
                .is_some_and(|deadline| row.received_unix_ms > deadline.saturating_mul(1000))
            {
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
            // **THE SIGNAL ARRIVED — consume it EXACTLY ONCE (§4.3).** Stamp its `consumed_seq` (the
            // history seq the `signal_received` row will land at) so the signal-buffer-depth drops and a
            // re-scan never re-consumes it. The consume is idempotent on `consumed_seq IS NULL` (a re-
            // drive races to the same NULL guard) — if a concurrent drive already consumed it we would
            // see it as consumed (None from the scan); here the scan returned it unconsumed so we win.
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
            // Journal the `signal_received` row carrying the consumed signal (idem_key + payload refs)
            // so replay returns the SAME signal (consume-exactly-once). references-not-payloads.
            self.stage_received(
                command_id,
                Some(name),
                &idem_key,
                &row.payload,
                row.payload_key_ref.as_deref(),
            );
            // **MID-WORKFLOW TOKEN RE-MINT ON RESUME (P-FLOW-17, contract 4.7, §6.2).** This wait had
            // PARKED (a journaled `signal_waited`) and is now RESUMING — the resumed body is about to run
            // (the consumed approval/job.done leads back into live activity), so the workflow's per-run
            // token (expired during the days-long wait) is re-minted FRESH + short-lived + attenuated
            // (token life == activity life, NOT the days-long workflow life). Only fires on a TRUE resume
            // (a prior `signal_waited`); a fast first-drive consume (no park) does not re-mint. A wired
            // lease whose mint fails surfaces LOUD (the resumed activity must not run under a stale token).
            if already_waited {
                self.remint_if_resuming()?;
            }
            return Ok(WaitOutcome::Signalled {
                idem_key,
                payload: row.payload,
                payload_key_ref: row.payload_key_ref,
            });
        }

        // **NO SIGNAL YET.** If a timeout was set and the timeout-timer's deadline has PASSED relative to
        // the engine's live clock, the wait TIMES OUT (the §6.3 auto-deny branch) — a deterministic
        // outcome the next re-drive reads off the same clock. Otherwise the run PARKS (`waiting`).
        if let Some(deadline) = effective_deadline {
            // The absolute timeout deadline: on the FIRST park it is now + timeout; on a resume it is the
            // deadline captured in the `signal_waited` marker (so the deadline is stable across re-drives).
            if now_secs >= deadline {
                // the timeout fired before the signal arrived — journal a `signal_received` carrying the
                // TIMEOUT marker (so replay returns TimedOut deterministically) and take the timeout path.
                self.stage_received(
                    command_id,
                    Some(name),
                    WAIT_TIMEOUT_MARKER,
                    &[],
                    Some(WAIT_TIMEOUT_MARKER),
                );
                // **RE-MINT ON RESUME (P-FLOW-17, §6.2).** A timed-out wait that had PARKED is ALSO a
                // resume — the body takes its timeout branch (the §6.3 auto-deny), which runs live (it may
                // compensate / withhold), so it too runs under a FRESH short-lived per-run token. Only on a
                // true resume (a prior `signal_waited`); a first-drive immediate timeout did not park.
                if already_waited {
                    self.remint_if_resuming()?;
                }
                return Ok(WaitOutcome::TimedOut);
            }
            // not yet due — arm/keep the durable timeout-timer (idempotent on the deterministic timer_id)
            // so the wheel wakes the run at the deadline even if the signal never arrives. Best-effort: a
            // wait with no timer wheel still parks (the signal delivery wakes it; the timeout is the
            // wheel's job when present).
            if arm_timeout {
                if let Some((timers, partition, _)) = self.timers.clone() {
                    let timer_id = format!("{}/{}/timeout", self.run_id, command_id);
                    let bucket = crate::timer::epoch_minute(deadline);
                    timers.arm(crate::timer::TimerRow {
                        tenant: self.tenant.clone(),
                        region: self.region.clone(),
                        timer_id,
                        run_id: Some(self.run_id.clone()),
                        command_id: command_id.clone(),
                        fire_at: deadline,
                        bucket,
                        fired: false,
                        partition,
                    });
                }
            }
            // park, recording the deadline so the resume reads a STABLE deadline.
            if !already_waited {
                self.stage_waited(
                    command_id,
                    Some(deadline),
                    expected_idem_key.map(|_| name),
                    expected_idem_key,
                );
            }
        } else if !already_waited {
            // an unbounded wait — journal `signal_waited` (no deadline) on the first park only.
            self.stage_waited(
                command_id,
                None,
                expected_idem_key.map(|_| name),
                expected_idem_key,
            );
        }
        self.parked_on_signal = true;
        // Record the park descriptor: WHAT this run is awaiting. The exact `(name, idem_key)` the wait
        // knows here is threaded into the durable commit so a signal that landed mid-drive settles the
        // run runnable instead of stranding it behind the buffer. A keyed wait carries its exact
        // idem_key; a name-only wait carries `None` (any first unconsumed signal of that name resumes).
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
        let (timers, partition, _) = self.timers.clone().ok_or_else(|| {
            WfError::CoCommit(
                "a timed job dispatch requires a durable timer wheel (WfCtx::with_timers)".into(),
            )
        })?;
        let command_id = format!("{dispatch_command_id}/job-timeout");
        timers.arm(crate::timer::TimerRow {
            tenant: self.tenant.clone(),
            region: self.region.clone(),
            timer_id: format!("{}/{command_id}", self.run_id),
            run_id: Some(self.run_id.clone()),
            command_id,
            fire_at: deadline,
            bucket: crate::timer::epoch_minute(deadline),
            fired: false,
            partition,
        });
        Ok(())
    }

    pub(crate) fn disarm_job_deadline(&mut self, dispatch_command_id: &str) -> WfResult<()> {
        let (timers, _, _) = self.timers.clone().ok_or_else(|| {
            WfError::CoCommit(
                "a timed job join requires a durable timer wheel (WfCtx::with_timers)".into(),
            )
        })?;
        let timer_id = format!("{}/{dispatch_command_id}/job-timeout", self.run_id);
        timers.disarm(&self.tenant, &timer_id);
        if !self.disarmed_timer_ids.contains(&timer_id) {
            self.disarmed_timer_ids.push(timer_id);
        }
        Ok(())
    }

    /// The deadline captured in a journaled `signal_waited` marker (the stable timeout deadline a resume
    /// reads, §4.3). `None` if the marker carried no deadline (an unbounded wait) or is absent.
    fn replayed_wait_deadline(&self, command_id: &str) -> Option<i64> {
        let replayed = self.replay_history.get(command_id)?;
        if replayed.kind != crate::wfctx::history_kind::SIGNAL_WAITED {
            return None;
        }
        replayed
            .result
            .as_ref()
            .and_then(|refs| {
                refs.iter()
                    .find_map(|r| r.0.strip_prefix(WAIT_DEADLINE_PREFIX))
            })
            .and_then(|s| s.parse::<i64>().ok())
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

    /// Stage a `signal_waited` row (the park marker, §4.3) carrying the optional timeout deadline so a
    /// resume reads a STABLE deadline. references-not-payloads (no PII body). STAGED — durable iff
    /// commit (FLOW-D5).
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

    /// Stage a `signal_received` row (the consume marker, §4.3) carrying the consumed signal's idem_key
    /// and payload refs so replay returns the SAME signal (consume-exactly-once). references-not-
    /// payloads. The first ref encodes the idem_key (prefixed), the rest are the payload refs. STAGED —
    /// durable iff commit (FLOW-D5).
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

    /// **Whether this drive PARKED on a durable timer (a `sleep` into the future, §4.2).** The engine
    /// reads it to settle the run `waiting` (holding NO runtime) rather than `completed` — the run
    /// wakes when the wheel fires the timer. `false` if no live `sleep` parked (or a `sleep` whose
    /// deadline already passed — a no-wait continuation).
    pub fn parked_on_timer(&self) -> bool {
        self.parked_on_timer
    }

    /// **Whether this drive PARKED on a `wait_for_signal` (a wait whose named signal is not yet
    /// buffered, §4.3).** The engine reads it (alongside [`WfCtx::parked_on_timer`]) to settle the run
    /// `waiting` (holding NO runtime) rather than `completed` — the run wakes when
    /// `DurableExecutor::signal` delivers the named signal. `false` if every wait found a buffered
    /// signal (or no wait was issued). This is the multi-day-HITL `state=waiting` holds-no-runtime
    /// property (FLOW-D4).
    pub fn parked_on_signal(&self) -> bool {
        self.parked_on_signal
    }

    /// **Whether this drive PARKED on ANY durable wait — a timer OR a signal (§4.2/§4.3).** The engine's
    /// single park predicate: a body that armed a not-yet-due `sleep` OR reached a `wait_for_signal`
    /// whose signal has not arrived settles `waiting`, not terminal.
    pub fn parked(&self) -> bool {
        self.parked_on_timer || self.parked_on_signal
    }

    /// **The park descriptor — WHAT this drive parked on (the signal/park race fix).** `Some` iff this
    /// drive parked (settled `waiting`). The durable commit reads it to close the signal/park race: a
    /// [`ParkCondition::Signal`] whose exact `(name, idem_key)` already has a buffered signal (one that
    /// landed while the drive was mid-flight) settles the run RUNNABLE instead of stranding it. `None`
    /// on a drive that ran to completion / failure / a non-parking continuation.
    pub fn park_condition(&self) -> Option<&ParkCondition> {
        self.park_condition.as_ref()
    }

    /// **The `(signal_name, idem_key)` pairs a `wait_for_signal` CONSUMED on this drive (P-FLOW-11).**
    /// The engine reads it after a drive to refresh the signal-buffer-depth telemetry (a consumed
    /// signal drops the buffered depth). Exactly one entry per signal a wait woke on; the FLOW-D4 drill
    /// asserts ONE consume per delivered approval (a double-click delivers one buffered row → one
    /// consume).
    pub fn consumed_signals(&self) -> &[(String, String)] {
        &self.consumed_signals
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

    /// Consume a detached drive and export every staged side effect without persisting any of it.
    /// The returned rows must be committed together; splitting them would violate FLOW-D5.
    pub fn into_staged_drive(self) -> WfResult<StagedWfDrive> {
        let timers = self
            .timers
            .as_ref()
            .map(|(store, _, _)| store.rows_for_run(&self.tenant, &self.region, &self.run_id))
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
        ctx.emit(draft("agent.run.step"), None)
            .expect("emit buffers into the txn");
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
            ctx.emit(draft("agent.run.step"), None)
                .expect("emit buffers");
            assert_eq!(ctx.staged_history_len(), 1, "journaled-but-not-committed");
            assert_eq!(ctx.staged_emit_len(), 1, "emitted-but-not-committed");
            // ctx dropped HERE without commit — the crash between journal and emit.
        }
        // NEITHER: 0 journal rows, 0 outbox rows — 0 ghost, 0 lost (FLOW-D5).
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
                    Err(ActivityError(format!(
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
        // exactly ONE completed history row (the failures journal no history row).
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
        // the attempt ledger records all three attempts, all on the same idem_token.
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
        assert_eq!(
            t1, "2026-06-21T00:00:00Z",
            "now() returns the deterministic clock"
        );
        assert_ne!(r1, r2, "rand() advances (a sequence, not a constant)");
        // The EXACT splitmix64 draws for seed 42 — pinned so a regression in the mixing function
        // (a flipped ^/>>) is caught: replay-stability requires the value derivation be frozen.
        assert_eq!(
            r1, 13_679_457_532_755_275_413,
            "rand() draw 1 is the frozen splitmix64(42) value"
        );
        assert_eq!(
            r2, 2_949_826_092_126_892_291,
            "rand() draw 2 is the frozen splitmix64 value"
        );
        // three side-marker history rows staged (now + two rands).
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
        assert_eq!(
            h1.len(),
            3,
            "R1 has exactly its three history rows (now+activity+rand)"
        );
        assert!(
            h1.iter().all(|r| r.run_id == "R1"),
            "no R2 row leaked into R1's history (AND-filter)"
        );
        // the per-run monotonic replay-order seq is 0, 1, 2 (kills the next_history_seq mutant).
        assert_eq!(
            h1.iter().map(|r| r.seq).collect::<Vec<_>>(),
            vec![0, 1, 2],
            "the per-run history seq is monotonic 0,1,2 (the replay-order PK §3.2)"
        );
        // attempts_for(R1) returns exactly R1's one attempt (not R2's).
        let a1 = journal.attempts_for(&tenant(), "R1");
        assert_eq!(a1.len(), 1, "R1 has exactly one attempt row");
        assert!(
            a1.iter().all(|r| r.run_id == "R1"),
            "no R2 attempt leaked into R1's (AND-filter)"
        );
        // R2 is isolated too.
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
        // a wrong tenant returns nothing (the tenant half of the AND-filter).
        assert!(
            journal
                .history_for(&TenantId("other".into()), "R1")
                .is_empty(),
            "a different tenant sees none of acme's rows (the tenant half of the AND-filter)"
        );
    }

    /// **REPLAY: a resumed `WfCtx` short-circuits a journaled activity — 0 re-execution (§4.1).** A
    /// run journals one activity, commits, then a SECOND `WfCtx` resumes from that journal: the same
    /// activity's closure is NOT re-run (the journaled result is returned), `side_effects_executed`
    /// stays 0, and `double_effects` stays 0 (the FLOW-D1 floor in the WfCtx).
    #[test]
    fn resume_short_circuits_a_journaled_activity_zero_re_execution() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        // first drive: run + journal one activity.
        let mut c1 = begin(&outbox, journal.clone());
        c1.activity(RetryPolicy::default_policy(), |_i, _a| {
            Ok(vec![ArtifactRef("myelin://acme/agent/effect/e1".into())])
        })
        .expect("activity");
        c1.commit().expect("co-commit");
        let history = journal.history_for(&tenant(), "R1");
        assert_eq!(history.len(), 1, "one journaled command");

        // resume: re-drive the SAME activity from the journal — it short-circuits.
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
                *ran2.lock().unwrap() = true; // would flip true IF the closure re-ran.
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

    /// **REPLAY: a journaled `activity_failed` command replays to the SAME failure — 0 re-execution
    /// (§4.1).** A run journals an exhausted (failed) activity, commits, then a resume re-drives the
    /// same activity: the closure is NOT re-run, the failure is returned deterministically (the run
    /// takes its error branch on replay), and `side_effects_executed`/`double_effects` stay 0.
    #[test]
    fn resume_short_circuits_a_journaled_activity_failed() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        // first drive: an activity that exhausts its retries → an activity_failed history row.
        let mut c1 = begin(&outbox, journal.clone());
        c1.activity(RetryPolicy { max_attempts: 1 }, |_i, _a| {
            Err(ActivityError("hard failure".into()))
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

        // resume: the failed activity replays to the same failure WITHOUT re-running the closure.
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
                *ran2.lock().unwrap() = true; // would flip IF re-run.
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

    /// **P-FLOW-07: a KIND-MISMATCH on replay HALTS as `nondeterministic` — it does NOT re-execute
    /// (FLOW-D2, the divergence guard).** A position journaled as a `side_marker` (a `now()` in the
    /// original drive) is re-driven as an `activity`: the body diverged from its journal. The guard
    /// HALTS — the activity does NOT run live (0 side effect, 0 double-effect), `activity()` returns
    /// [`WfError::Nondeterministic`], and the divergence latch is set so the engine dead-letters the
    /// run. This is the reconciled successor of the former double-effect probe: the divergence that
    /// once silently double-effected is now a loud halt (0 silent divergence, EI-01 §2/§3).
    #[test]
    fn kind_mismatch_on_replay_halts_nondeterministic_not_re_execute() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        // first drive journals a side-marker at position 0 (a now()).
        let mut c1 = begin(&outbox, journal.clone());
        let _ = c1.now();
        c1.commit().expect("co-commit the marker");
        let history = journal.history_for(&tenant(), "R1");

        // resume but the body now issues an ACTIVITY at position 0 (the divergence): the marker-kind
        // journal does NOT match an `activity` — the guard HALTS rather than re-executing live.
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
                *ran2.lock().unwrap() = true; // would flip IF the divergent activity ran live.
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
            "0 side effects — the guard halted before live exec"
        );
        assert_eq!(
            c2.double_effects(),
            0,
            "0 double-effect — the divergence is a halt, not a re-execution"
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

    /// **`WfError::is_nondeterministic` discriminates the divergence verdict from the others.** Only
    /// the `Nondeterministic` variant reads true; an `ActivityExhausted`/`CoCommit` reads FALSE — so
    /// the engine settles a normal failure as `failed` (retryable) and ONLY a divergence as
    /// `nondeterministic` (dead-lettered). Pins the predicate is real, not a constant `true` (a
    /// vacuous-true would dead-letter every failure, mis-parking retryable work).
    #[test]
    fn is_nondeterministic_is_true_only_for_the_divergence_verdict() {
        assert!(
            WfError::Nondeterministic("diverged".into()).is_nondeterministic(),
            "the divergence verdict reads true"
        );
        assert!(
            !WfError::ActivityExhausted(ActivityError("x".into())).is_nondeterministic(),
            "an activity exhaustion is NOT a divergence (it is a retryable failure, not a dead-letter)"
        );
        assert!(
            !WfError::CoCommit("y".into()).is_nondeterministic(),
            "a co-commit failure is NOT a divergence (the predicate is not a constant true)"
        );
    }

    /// **P-FLOW-07: the REVERSE kind-mismatch (a `now()`/`rand()` issued at a journaled ACTIVITY
    /// position) also HALTS (FLOW-D2).** Position 0 journals an `activity`; a resume issues a `now()`
    /// at position 0 — the side-marker guard latches the divergence so the engine halts the run.
    #[test]
    fn reverse_kind_mismatch_now_at_activity_position_halts() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        // first drive journals an activity at position 0.
        let mut c1 = begin(&outbox, journal.clone());
        c1.activity(RetryPolicy::default_policy(), |_i, _a| {
            Ok(vec![ArtifactRef("myelin://acme/agent/effect/e0".into())])
        })
        .expect("activity");
        c1.commit().expect("co-commit");
        let history = journal.history_for(&tenant(), "R1");

        // resume but issue a now() at position 0 (the reverse divergence).
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
        let _ = c2.now(); // the body reads now() where the journal records an activity.
        assert!(
            c2.is_divergent(),
            "a now() at an activity position latches the divergence"
        );
        assert!(
            c2.divergence().unwrap().contains("side-marker"),
            "the reason names the side-marker divergence: {:?}",
            c2.divergence()
        );
        // a rand() at a journaled activity position likewise diverges (resume fresh to test rand).
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

    /// **REPLAY: a resumed `now()`/`rand()` returns its CAPTURED value, not a recomputed one (§4.1).**
    /// The first drive captures the clock + the draw into side-markers; a resume with a DIFFERENT
    /// clock + seed still returns the original captured values — `now()`/`rand()` are replay-stable
    /// because the value, not the seed, is journaled.
    #[test]
    fn resume_now_and_rand_return_captured_values_not_recomputed() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let mut c1 = begin(&outbox, journal.clone());
        let t1 = c1.now();
        let r1 = c1.rand();
        c1.commit().expect("co-commit");
        let history = journal.history_for(&tenant(), "R1");

        // resume with a DIFFERENT clock + seed — the captured values must still come back.
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

    /// **`sleep_until` arms a durable `wf_timer` row + journals `timer_set` + PARKS (P-FLOW-13,
    /// §4.2).** A future deadline: the timer lands on the wheel in its minute bucket, one `timer_set`
    /// history row is staged, the deterministic `timer_id = <run_id>/<command_id>`, and `parked_on_timer`
    /// is true (the run waits, holding no runtime). After commit the journal holds the marker.
    #[test]
    fn sleep_until_arms_a_durable_timer_journals_timer_set_and_parks() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let timers = crate::timer::TimerStore::new();
        // now = 1000s; sleep until 1600s (future → parks). partition 3.
        let mut ctx = begin(&outbox, journal.clone()).with_timers(timers.clone(), 3, 1000);
        ctx.sleep_until(1600).expect("the sleep arms + journals");
        assert!(
            ctx.parked_on_timer(),
            "a future-deadline sleep parks the run (waiting, no runtime)"
        );
        // the timer is armed on the wheel in its minute bucket (1600/60 = 26), deterministic id.
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
        // one timer_set side row staged + journaled.
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

    /// **A `sleep_until` whose deadline already PASSED arms immediately-due and does NOT park (§4.2).**
    /// A deadline `<= now` is a no-wait continuation: the timer is armed in bucket 0 (the next wheel
    /// tick fires it) but the run does not park — the body continues.
    #[test]
    fn a_past_deadline_sleep_does_not_park() {
        let outbox = OutboxStore::new();
        let timers = crate::timer::TimerStore::new();
        // now = 1000s; sleep until 500s (already past).
        let mut ctx = begin(&outbox, WfJournal::new()).with_timers(timers.clone(), 0, 1000);
        ctx.sleep_until(500).expect("the sleep arms");
        assert!(
            !ctx.parked_on_timer(),
            "a past-deadline sleep is a no-wait continuation (no park)"
        );
        // the timer is still armed (immediately due — the next wheel tick fires it).
        assert!(
            timers.get(&tenant(), "R1/agent.run:0").is_some(),
            "the immediately-due timer is armed"
        );
    }

    /// **`sleep_for` arms a RELATIVE timer (now + duration) and parks (§4.2).** `sleep_for(30 days)`
    /// over now=1000s arms a far-future timer (bucket far in the future, never scanned until its
    /// minute — the SC-11 substrate) and parks. A non-positive duration is a no-wait continuation.
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
        // a far-future bucket — the SC-11 partial index never reads it until its minute.
        assert_eq!(
            timer.bucket,
            crate::timer::epoch_minute(1000 + 30 * 24 * 3600)
        );
    }

    /// **A `sleep` on a `WfCtx` with NO timer wheel errors LOUDLY — never a silent no-op (EI-01 §2).**
    /// A `WfCtx` built without `with_timers` cannot arm a durable timer; `sleep_until` returns a
    /// `CoCommit` error naming the missing wheel rather than silently doing nothing (a silent sleep
    /// that did not park would be a correctness bug — the run would "complete" without waiting).
    #[test]
    fn sleep_with_no_timer_wheel_errors_loudly() {
        let outbox = OutboxStore::new();
        let mut ctx = begin(&outbox, WfJournal::new()); // NO with_timers.
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

    /// **REPLAY: a resumed `sleep_until` returns WITHOUT re-arming the timer (§4.1).** The first drive
    /// arms the timer + journals `timer_set`; a resume re-issues the `sleep` at the same command
    /// position — it short-circuits (the journaled `timer_set` replays), arms NO second timer, and does
    /// NOT re-park (the run already waited; the re-drive continues past the sleep). The wheel holds one
    /// timer, not two.
    #[test]
    fn resume_sleep_replays_without_re_arming() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let timers = crate::timer::TimerStore::new();
        // first drive: arm + journal the timer_set marker.
        let mut c1 = begin(&outbox, journal.clone()).with_timers(timers.clone(), 0, 1000);
        c1.sleep_until(1600).expect("arm");
        c1.commit().expect("co-commit the marker");
        assert_eq!(
            timers.armed_count(),
            1,
            "one timer armed on the first drive"
        );
        let history = journal.history_for(&tenant(), "R1");

        // resume: re-issue the sleep at the same position — it replays (no re-arm, no second park).
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

    /// **REVERSE divergence: a `sleep` at a position journaled as an ACTIVITY halts (P-FLOW-07,
    /// FLOW-D2).** Position 0 journals an activity; a resume issues a `sleep` at position 0 — the body
    /// diverged from its journal. The sleep latches the divergence + returns `Nondeterministic`
    /// (the engine halts + dead-letters the run), never re-arms against the journaled activity.
    #[test]
    fn sleep_at_an_activity_position_halts_nondeterministic() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let timers = crate::timer::TimerStore::new();
        // first drive journals an activity at position 0.
        let mut c1 = begin(&outbox, journal.clone());
        c1.activity(RetryPolicy::default_policy(), |_i, _a| {
            Ok(vec![ArtifactRef("myelin://acme/agent/effect/e0".into())])
        })
        .expect("activity");
        c1.commit().expect("co-commit");
        let history = journal.history_for(&tenant(), "R1");

        // resume but issue a sleep at position 0 (the divergence).
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

    // ---- wait_for_signal (P-FLOW-11, §4.3) -------------------------------------------------------

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

    /// **A `wait_for_signal` on an ABSENT signal PARKS — state=waiting holds no runtime (FLOW-D4).** The
    /// body reaches the wait, no signal is buffered, so the run parks (`parked_on_signal`) — the engine
    /// settles it `waiting`. The park journals a `signal_waited` marker (the resume short-circuit reads
    /// it back). NO signal was consumed.
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
            "nothing consumed — the signal has not arrived"
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

    /// **A buffered signal RESUMES the wait, consuming it EXACTLY ONCE (FLOW-D4 — 1 consume).** The
    /// approval arrives (buffered), the body's wait finds it, consumes it (stamps `consumed_seq` — the
    /// buffered depth drops to 0), and returns `Signalled` carrying the references-not-payloads payload.
    /// A `signal_received` marker is journaled (the replay short-circuit).
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
        // the consume stamped consumed_seq → buffered depth dropped to 0 (the §4.3 consume-once).
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

    /// **The multi-day round-trip: park, then a days-later signal resumes + consumes ONCE (FLOW-D4).**
    /// Drive 1 parks (no signal). The approval arrives DAYS later. Drive 2 (a re-lease / re-drive) re-
    /// issues the wait, finds the now-buffered signal, consumes it EXACTLY once, and resumes. This is the
    /// state=waiting-holds-no-runtime → signal-arrives → resume bridge across a restart.
    #[test]
    fn park_then_days_later_signal_resumes_and_consumes_once() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();

        // DRIVE 1: the body waits; no signal is buffered → park (state=waiting).
        let mut c1 = begin(&outbox, journal.clone()).with_signals(signals.clone());
        assert_eq!(
            c1.wait_for_signal("approval:call-1", None).unwrap(),
            WaitOutcome::Parked
        );
        assert!(c1.parked_on_signal());
        c1.commit().expect("co-commit the park");
        let history = journal.history_for(&tenant(), "R1");

        // DAYS LATER: a human clicks Approve → the signal is buffered (DurableExecutor::signal).
        buffer_signal(
            &signals,
            "approval:call-1",
            "card-7",
            vec![ArtifactRef("myelin://acme/agent/decision/approve".into())],
        );

        // DRIVE 2 (re-lease / re-drive): the wait replays the journaled `signal_waited` then re-checks
        // the buffer — the now-present signal resumes the run, consuming it EXACTLY once.
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

    /// **Replay returns the SAME consumed signal — consume-exactly-once across a re-drive (§4.1).** A
    /// run that consumed a signal, re-driven AGAIN (a third drive, e.g. a later step crashed), replays
    /// the `signal_received` marker → returns the SAME journaled signal WITHOUT re-scanning the buffer
    /// (so a SECOND buffered signal under a different key is NOT wrongly consumed).
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
        // drive 1 consumes the signal + journals signal_received.
        let mut c1 = begin(&outbox, journal.clone()).with_signals(signals.clone());
        c1.wait_for_signal("approval:call-1", None)
            .expect("consume");
        c1.commit().expect("co-commit");
        let history = journal.history_for(&tenant(), "R1");

        // a SECOND, distinct signal is buffered (a different key) — replay must NOT consume it.
        buffer_signal(
            &signals,
            "approval:call-1",
            "card-99",
            vec![ArtifactRef("myelin://acme/agent/decision/other".into())],
        );
        let depth_before = signals.buffered_depth();

        // drive 2: re-issue the wait — it short-circuits to the journaled signal (card-7), not card-99.
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

    /// **A wait with a TIMEOUT whose deadline PASSED returns `TimedOut` (the §6.3 auto-deny branch).** No
    /// signal arrives; the engine clock advances past the timeout deadline → the wait times out (the
    /// body takes the auto-deny path → 0 mutation, AG-8).
    #[test]
    fn wait_times_out_when_deadline_passes_without_a_signal() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let signals = SignalStore::new();
        let timers = crate::timer::TimerStore::new();

        // DRIVE 1 at clock=1000 with a 100s timeout → parks (deadline 1100 not yet reached).
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

        // DRIVE 2 at clock=2000 (past the deadline 1100), STILL no signal → TimedOut.
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

    /// **A `wait_for_signal` with NO signal store is a LOUD error (never a silent no-op, EI-01 §2).** A
    /// `WfCtx` built without `with_signals` cannot consume a durable signal — the wait returns a
    /// CoCommit error naming the missing store rather than silently doing nothing.
    #[test]
    fn wait_without_a_signal_store_is_a_loud_error() {
        let outbox = OutboxStore::new();
        let journal = WfJournal::new();
        let mut ctx = begin(&outbox, journal); // NO with_signals.
        let err = ctx
            .wait_for_signal("approval:call-1", None)
            .expect_err("a wait with no store errors");
        assert!(
            matches!(err, WfError::CoCommit(ref m) if m.contains("signal store")),
            "the missing-store wait is a loud CoCommit error, got {err:?}"
        );
    }

    /// **A `wait_for_signal` at a position journaled as an ACTIVITY halts nondeterministic (P-FLOW-07).**
    /// Position 0 journals an activity; a resume issues a `wait_for_signal` at position 0 — the body
    /// diverged from its journal. The wait latches the divergence + returns `Nondeterministic`.
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

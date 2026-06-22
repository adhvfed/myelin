//! # Escalation on the durable wheel — the frozen chain shape; ack-as-event (NOTIF-P14 / P-192, M2)
//!
//! **Owning architecture doc:** `notifications.md` §2.4 (the escalation-chain config shape FROZEN:
//! `page → oncall_now → notify(class=critical, pierces quiet-hours) → escalate-after-timer
//! (ack_window) → if !acked next-step / if acked stop`; Issues passes the chain definition; **Notif
//! owns POLICY evaluation, the workflow engine owns DURABILITY**; the timers are `myelin-flow`
//! durable timers not in-process sleeps; ack is an event `notif.escalation.acked` via outbox, the
//! workflow signal-wait resolves on it; on-call cannot be silenced, `pierce_classes` default
//! critical), §3.7 (escalation on the durable-workflow substrate).
//!
//! **Contracts:** **7.5** `oncall_now(schedule) → principal` + `page(target, reason)` starts an
//! escalation durable workflow (owned; the frozen chain shape). **Consumed:** 9.1 `DurableExecutor`,
//! 9.3 the durable timer wheel (millions of timers as an indexed range read, effectively-once), 9.4
//! the durable signal (state=waiting holds no runtime; an ack/cancel arrives later; idempotent),
//! 2.2 `OutboxTx::emit` (the ack event). **Drills:** NOTIF-D7 (kill mid-`ack_window` → resume →
//! page next step EXACTLY ONCE; ack stops the chain), NOTIF-D8 (critical pierces quiet-hours;
//! non-crit suppressed).
//!
//! ## What this prompt (NOTIF-P14) ships
//!
//! 1. **`page(target, reason)`** ([`page`]) — starts an escalation DURABLE WORKFLOW on the durable
//!    substrate walking the frozen chain shape: `page → oncall_now(schedule)` resolves the rotation
//!    AT FIRE TIME → `notify(principal, channels, class=critical)` which PIERCES quiet-hours →
//!    `escalate-after-timer(ack_window)` a DURABLE TIMER that survives a Notif restart and fires
//!    effectively-once → `if !acked` walk to the next step; `if acked` stop. Notif owns the POLICY
//!    (which step, which target, which channels, the pierce decision); the engine owns the DURABILITY.
//! 2. **`oncall_now(schedule) → principal`** ([`oncall_now`]) — resolves the on-call rotation roster
//!    at the supplied instant (the target resolution AT FIRE TIME, not at policy-author time).
//! 3. **Ack-as-event** ([`EscalationEngine::ack`]) — an ack emits `notif.escalation.acked` via
//!    [`OutboxTx::emit`] (contract 2.2) and resolves the workflow's signal-wait (9.4), HALTING the
//!    chain. Idempotent on the run id (a double-ack acks once, never double-resolves, never re-pages).
//! 4. **The `escalation_run` row holds the durable handle** — a restart resumes the chain from the
//!    persisted current-step + the live durable timer; it never misses a step nor double-pages.
//!
//! ## The durable substrate — the [`DurableWheel`] seam (a DOCUMENTED DEVIATION)
//!
//! The prompt's prose consumes the `myelin-flow` `DurableExecutor` plus the durable timer wheel plus
//! the durable signal (contracts 9.1/9.3/9.4). The `myelin-flow` crate does not exist yet: it is
//! built by the workflow prompts (the P-FLOW series = global P-197 onward), which the ledger places
//! AFTER this prompt (P-192). The run-table's resolved DEPENDS-ON for P-192 is `P-181, P-188` (the
//! router plus prefs) and does NOT include the workflow crate. To avoid building on a crate that
//! does not exist while still implementing the deliverable HONESTLY, the durable substrate is
//! modelled as the [`DurableWheel`] trait seam (`schedule_timer` / `cancel_timer` / `fire_due` /
//! `has_timer`) that `myelin-flow` backs at P-FLOW-13 (the minute-bucket wheel) and P-FLOW-09 (the
//! durable signal). The in-memory [`InMemoryWheel`] models the effectively-once timer plus the
//! idempotent signal so the chain-walk POLICY (the thing THIS prompt owns) is proven today, and the
//! kill-and-resume drill (NOTIF-D7) is exercised against a persisted handle. This is the SAME
//! in-memory-now / Postgres-or-flow-later seam pattern the rest of this crate uses
//! ([`PrefStore`](crate::prefs::PrefStore), [`InboxProjection`](crate::router::InboxProjection)).
//! The real durable wheel/signal is the named floor `myelin-flow` (P-FLOW-09/P-FLOW-13); wiring the
//! escalation chain onto the real engine is the snooze/SLA one-substrate-three-uses reconcile at
//! NOTIF-P18 (snooze re-surface, same wheel). The drill harness here proves the POLICY plus the
//! exactly-once / pierce properties against the seam.
//!
//! ## FLOORS named
//!
//! - **Issues' real SLA escalation chain** is passed in N-M4 (**NOTIF-P21**); here the frozen chain
//!   shape is exercised with a Notif-defined TEST chain ([`EscalationPolicy::test_chain`]).
//! - **Snooze re-surfacing on the SAME durable wheel** is **NOTIF-P18** (one substrate, three uses:
//!   escalation timers here, snooze re-surface there, SLA timers).
//! - **The real `myelin-flow` durable executor/timer/signal** is P-FLOW-09/P-FLOW-13 (see above).
//!
//! ## Mutation floor (the escalation module — mandatory-core)
//!
//! Escalation is mandatory-core (an exactly-once page is a correctness/safety seam: a missed page is
//! a silenced on-call, a double page is alert fatigue). The mutation-tested core is the POLICY:
//! [`EscalationPolicy::step_at`] (step ordering + repeat wraparound + exhaustion), [`oncall_now`]
//! (rotation resolution at fire time), the pierce decision ([`notify_for`] — critical ALWAYS pierces,
//! you cannot silence an on-call page), the chain-walk state machine ([`EscalationEngine::advance`] —
//! one page per step, never zero, never two), and the idempotent ack-halt ([`EscalationEngine::ack`]).
//! **Floor: ≥ 80% line/branch mutation score on `escalation.rs`** (measured with `cargo mutants`;
//! reported in the P-192 commit body).

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

// ===========================================================================================
//  THE FROZEN CHAIN SHAPE (architecture §2.4 / contract 7.5) — the config Issues passes
// ===========================================================================================

/// A **target selector** for one escalation step — resolved to a concrete [`PrincipalId`] AT FIRE
/// TIME (not at policy-author time), so the page reaches whoever is on call WHEN the step fires
/// (architecture §2.4 "resolve the rotation at fire time"). The on-call PII (who is on call) lives
/// in the rotation roster; a selector is an opaque routing key (no PII).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EscalationTarget {
    /// Resolve `oncall_now(schedule)` → the principal on call at fire time (the rotation roster).
    Schedule(String),
    /// A fixed principal (a named on-call lead) — already an opaque pseudonym, no resolution needed.
    Principal(PrincipalId),
}

/// One **step** of the frozen escalation chain (architecture §2.4): WHO to page (resolved at fire
/// time), on WHICH channels, and the `ack_window` to wait before walking to the next step. The
/// `class` is always [`Class::Critical`] for an escalation step — an on-call page cannot be silenced
/// (`pierce_classes` default critical, §2.4). A step config carries no PII.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscalationStep {
    /// The target selector (resolved at fire time via [`oncall_now`] for a [`EscalationTarget::Schedule`]).
    pub target: EscalationTarget,
    /// The channels to page on (in-app always; off-cell channels are EU-preferring at delivery).
    pub channels: Vec<Channel>,
    /// The ack window: wait this many minutes for an ack before escalating to the next step. The
    /// durable timer the chain arms is a `myelin-flow` durable timer (9.3), NOT an in-process sleep.
    pub ack_window_minutes: u32,
}

/// The **escalation policy** — the ordered chain config (the frozen C3 shape an SLA/on-call producer,
/// e.g. Issues, passes to Notif; architecture §2.4 / contract 7.5). Notif owns the POLICY EVALUATION
/// over this; the DURABILITY is the engine's. `repeat` loops the whole chain N times before giving up
/// (exhaustion). A policy carries no PII (targets are selectors, channels are channel kinds).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscalationPolicy {
    /// The opaque policy id (FK target for an `escalation_run`).
    pub policy_id: String,
    /// The ordered chain steps (the frozen §2.4 shape). Must be non-empty for a startable chain.
    pub steps: Vec<EscalationStep>,
    /// Loop the whole chain this many times before exhaustion (≥ 1; `repeat = 1` walks once).
    pub repeat: u32,
}

impl EscalationPolicy {
    /// **The Notif-defined TEST chain (the FLOOR — Issues' real SLA chain is NOTIF-P21).** A
    /// two-step chain: page the on-call schedule (`platform-oncall`), then escalate to a fixed
    /// secondary lead if unacked. Used by the drill harness to exercise the FROZEN chain shape until
    /// Issues passes its real SLA policy in N-M4.
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

    /// **The step at logical chain position `walk` (the POLICY — step ordering + repeat + exhaustion).**
    /// `walk` is the count of escalate-after-timer fires so far (0 = the FIRST page). With `S` steps
    /// and `repeat` loops, positions `0 .. S*repeat` are live (each maps to `steps[walk % S]`); a
    /// `walk >= S*repeat` is EXHAUSTED (the chain gave up — returns `None`). This is the single source
    /// of "which step do we page next", so the chain-walk cannot drift from the config.
    pub fn step_at(&self, walk: u32) -> Option<&EscalationStep> {
        if self.steps.is_empty() {
            return None;
        }
        let total = (self.steps.len() as u32).saturating_mul(self.repeat.max(1));
        if walk >= total {
            return None; // exhausted — the chain walked every step `repeat` times unacked.
        }
        let idx = (walk as usize) % self.steps.len();
        self.steps.get(idx)
    }
}

// ===========================================================================================
//  oncall_now (contract 7.5) — rotation resolution AT FIRE TIME
// ===========================================================================================

/// One **rotation window** of an on-call schedule — `[from, to)` minute-of-day in the schedule's tz,
/// mapping to the principal on call during that window. The roster (who is on call when) is the
/// on-call PII (a pseudonym); a window carries the opaque principal pseudonym + the minute bounds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RotationWindow {
    /// Inclusive start, minute-of-day `0..1440`.
    pub from_minute: i32,
    /// Exclusive end, minute-of-day `0..=1440` (a window may equal 1440 = end-of-day).
    pub to_minute: i32,
    /// The principal on call during `[from, to)` (an opaque pseudonym, 4.8).
    pub principal: PrincipalId,
}

/// An **on-call schedule** — a rotation roster resolved by [`oncall_now`] at fire time (architecture
/// §2.4, the `oncall_schedule` row). The roster is a layered set of [`RotationWindow`]s; the FIRST
/// window covering the instant wins (later windows are overrides). An empty roster / an uncovered
/// instant resolves to `None` (no one on call — the engine must surface this, never silently drop).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OncallSchedule {
    /// The opaque schedule id (`platform-oncall`).
    pub schedule_id: String,
    /// The layered rotation windows (the on-call PII roster).
    pub rotation: Vec<RotationWindow>,
}

/// **`oncall_now(schedule) → principal` (contract 7.5) — resolve the rotation roster AT FIRE TIME.**
/// Returns the principal on call at `minute_of_day` (the FIRST covering window wins, so a later
/// override-window shadows an earlier base-window). Returns `None` if no window covers the instant
/// (no one on call) — the engine surfaces this rather than silently dropping a page. Resolving at
/// FIRE TIME (not policy-author time) is the §2.4 invariant: the page reaches whoever is on call
/// WHEN the step fires, not who was on call when the policy was written.
pub fn oncall_now(schedule: &OncallSchedule, minute_of_day: i32) -> Option<PrincipalId> {
    schedule
        .rotation
        .iter()
        .find(|w| minute_of_day >= w.from_minute && minute_of_day < w.to_minute)
        .map(|w| w.principal.clone())
}

// ===========================================================================================
//  THE notify(principal, channels, class=critical) PIERCE DECISION (NOTIF-D8)
// ===========================================================================================

/// **The `notify` delivery decision for an escalation step (the PIERCE POLICY — NOTIF-D8).** An
/// escalation page is ALWAYS [`Class::Critical`], which PIERCES quiet-hours by default
/// (`pierce_classes ⊇ {critical}`, §2.4) — **you cannot silence an on-call page**. So a critical
/// escalation delivers on EVERY step channel regardless of the recipient's quiet window
/// (`recipient_in_quiet`). A NON-critical (watching) item that lands in a quiet window is suppressed
/// off-cell (the [`route`](crate::prefs::route) path) — but escalation steps are never non-critical,
/// so this function pages on all channels for `Class::Critical`, and for any lower class only when
/// the recipient is NOT in a quiet window or the recipient's prefs pierce it.
///
/// Returns the channels the page actually pushes on. For a critical escalation this is ALWAYS the
/// full step channel set (the pierce); the empty return is only reachable for a non-critical class
/// inside a quiet window with no in-app channel — which an escalation step never produces.
pub fn notify_for(
    step_channels: &[Channel],
    class: Class,
    quiet: &QuietHours,
    recipient_in_quiet: bool,
) -> Vec<Channel> {
    if class == Class::Critical || quiet.pierces(class) || !recipient_in_quiet {
        // The pierce: an on-call page is NEVER silenced by a quiet window.
        step_channels.to_vec()
    } else {
        // A non-critical class inside a quiet window: in-cell in-app only (the off-cell push is
        // silenced — but the inbox row is never suppressed; that is the router's job, not here).
        step_channels
            .iter()
            .copied()
            .filter(|c| *c == Channel::InApp)
            .collect()
    }
}

// ===========================================================================================
//  THE DURABLE SUBSTRATE SEAM (the myelin-flow durable timer + signal — FLOOR P-FLOW-09/13)
// ===========================================================================================

/// **The durable-substrate seam (contracts 9.1/9.3/9.4) — backed by `myelin-flow` at
/// P-FLOW-09/P-FLOW-13 (a NAMED FLOOR).** The escalation chain arms a `myelin-flow` DURABLE TIMER
/// (9.3) for the `ack_window` and a DURABLE SIGNAL-WAIT (9.4) for the ack; this trait is the seam
/// the policy drives, so the chain-walk can be proven today against [`InMemoryWheel`] and wired to
/// the real engine when `myelin-flow` lands (the run-table places it after this prompt). The
/// IMPORTANT properties the seam guarantees (and the drill asserts): a timer fires **effectively
/// once** (a Notif restart re-arms from the persisted handle, never double-fires), and a signal is
/// **idempotent** (a duplicate ack delivers once).
pub trait DurableWheel {
    /// Arm a durable timer for `run_id` to fire after `ack_window_minutes` (a `myelin-flow` 9.3
    /// timer — survives a Notif restart). Re-arming the same `run_id` REPLACES the prior timer (the
    /// guarded UPDATE — disarm/re-arm at row-update cost, no wheel pollution; FLOW-D3).
    fn schedule_timer(&self, run_id: &str, ack_window_minutes: u32);
    /// Cancel `run_id`'s timer (an ack/cancel disarms it so the chain does not walk after an ack).
    fn cancel_timer(&self, run_id: &str);
    /// Whether `run_id` has a live (un-fired, un-cancelled) timer — the persisted durable handle a
    /// restart resumes from.
    fn has_timer(&self, run_id: &str) -> bool;
    /// Fire `run_id`'s due timer EXACTLY ONCE (effectively-once, 9.3): returns `true` the first time,
    /// `false` on any re-fire (a restart-replayed fire is idempotent — the no-double-page anchor).
    fn fire_due(&self, run_id: &str) -> bool;
}

/// **The in-memory durable wheel (models `myelin-flow` 9.3/9.4 for the chain-walk drill).** A timer
/// is a persisted handle keyed by `run_id`; `fire_due` is effectively-once (it consumes the handle,
/// so a replayed fire returns `false` — no double page). This is the seam the real engine replaces
/// at P-FLOW-13; the POLICY (chain-walk) is proven against it. Cloneable (an `Arc` inner) so the
/// engine and the drill share the same wheel.
#[derive(Clone, Default)]
pub struct InMemoryWheel {
    inner: Arc<Mutex<WheelInner>>,
}

#[derive(Default)]
struct WheelInner {
    /// run_id → (ack_window_minutes, fired?). A live handle is `fired = false`.
    timers: BTreeMap<String, TimerHandle>,
}

#[derive(Clone)]
struct TimerHandle {
    fired: bool,
}

impl InMemoryWheel {
    /// A fresh empty wheel.
    pub fn new() -> InMemoryWheel {
        InMemoryWheel::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, WheelInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl DurableWheel for InMemoryWheel {
    fn schedule_timer(&self, run_id: &str, _ack_window_minutes: u32) {
        // Re-arming REPLACES (the guarded UPDATE) — a fresh live handle.
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
            // Live handle → fire EXACTLY once; mark fired so a replay is a no-op (no double page).
            Some(h) if !h.fired => {
                h.fired = true;
                true
            }
            // Already fired (a restart replay) or no handle → idempotent no-op.
            _ => false,
        }
    }
}

// ===========================================================================================
//  THE escalation_run STATE (the durable handle the restart resumes from)
// ===========================================================================================

/// The run state of a live escalation (architecture §2.4 `escalation_run.state`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    /// The chain is walking (a page is out, waiting for ack or the ack_window timer).
    Active,
    /// An ack arrived — the chain HALTED (the terminal happy path).
    Acked,
    /// The chain walked every step `repeat` times unacked — gave up (the terminal sad path).
    Exhausted,
}

/// **A live escalation run — the durable handle the `escalation_run` row holds (architecture §2.4).**
/// A restart resumes the chain from `walk` (the persisted step cursor) + the live durable timer, so
/// it never misses a step nor double-pages. The `pages` log records WHO was paged at each step (the
/// exactly-once drill reads it: one entry per fired step, never zero, never two).
#[derive(Clone, Debug)]
pub struct EscalationRun {
    /// `(tenant, region)`-first partition (the residency pin).
    pub tenant: TenantId,
    pub region: Region,
    /// The opaque run id (the durable handle key).
    pub run_id: String,
    /// The policy this run executes.
    pub policy: EscalationPolicy,
    /// The originating event (the SLA breach / agent escalation) — an opaque event ref.
    pub trigger_event: ArtifactRef,
    /// The chain-walk cursor: how many escalate-after-timer fires have happened (0 = first page out).
    pub walk: u32,
    /// The run state.
    pub state: RunState,
    /// WHO acked (an opaque pseudonym), set once on ack (idempotent).
    pub acked_by: Option<PrincipalId>,
    /// The page log: `(walk, principal)` — one entry per page actually sent (the exactly-once anchor).
    pub pages: Vec<(u32, PrincipalId)>,
}

// ===========================================================================================
//  THE ENGINE — page / advance / ack (the chain-walk POLICY over the durable seam)
// ===========================================================================================

/// A page that was actually delivered (the output of [`EscalationEngine::page`] /
/// [`EscalationEngine::advance`]) — WHO was paged, on which channels, at which chain position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageOutcome {
    /// The principal paged (resolved at fire time for a schedule target).
    pub principal: PrincipalId,
    /// The channels the page pushed on (the pierce result — critical pages all channels).
    pub channels: Vec<Channel>,
    /// The chain position this page is for (`0` = the first page).
    pub walk: u32,
}

/// Errors the escalation engine surfaces (loudly — never a silent dropped page).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EscalationError {
    /// The policy has no steps — nothing to page (a config error, surfaced not swallowed).
    EmptyPolicy,
    /// No one is on call for a schedule target at fire time (the roster does not cover the instant).
    NoOneOnCall(String),
    /// The outbox emit/commit of the ack event failed transiently (retry).
    AckEmitFailed(String),
    /// The run id is unknown (an ack/advance against a run that does not exist).
    UnknownRun(String),
}

/// **The escalation engine — owns the chain-walk POLICY over the [`DurableWheel`] seam.** `page`
/// starts a run (resolves the first step at fire time, pages, arms the durable timer); `advance` is
/// the escalate-after-timer fire (walk to the next step, page exactly once); `ack` halts the chain
/// (emit `notif.escalation.acked` via the outbox, cancel the timer, idempotent). The runs are held
/// in an in-memory store (the `escalation_run` table model — the durable handle a restart resumes
/// from); the live OLTP store is the named integration floor.
pub struct EscalationEngine<W: DurableWheel> {
    wheel: W,
    outbox: OutboxStore,
    minter: Arc<dyn IdMinter>,
    runs: Arc<Mutex<BTreeMap<String, EscalationRun>>>,
}

impl<W: DurableWheel> EscalationEngine<W> {
    /// Build the engine over a durable wheel + the shared outbox (the ack-event emit path).
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

    /// The durable wheel (for the drill to fire a due timer / assert a live handle).
    pub fn wheel(&self) -> &W {
        &self.wheel
    }

    /// Read a run's current state (the durable handle the restart resumes from).
    pub fn run(&self, run_id: &str) -> Option<EscalationRun> {
        self.lock().get(run_id).cloned()
    }

    /// **Re-hydrate a persisted `escalation_run` handle onto this engine (the RESTART resume path).**
    /// A real Notif restart loses the in-process `runs` map; the durable substrate (the wheel + the
    /// `escalation_run` row) survives, so resume reads the row back and rebuilds the run handle, then
    /// the live durable timer (still armed on the wheel) fires through [`advance`](Self::advance).
    /// Named `_for_test` because the live OLTP `escalation_run` read-back is the named integration
    /// floor (the in-memory model proves the chain-walk resumes from the persisted cursor).
    pub fn resume_for_test(&self, run: EscalationRun) {
        self.lock().insert(run.run_id.clone(), run);
    }

    /// **Resolve the target of a step AT FIRE TIME (the POLICY).** A [`EscalationTarget::Schedule`]
    /// resolves through [`oncall_now`] against the supplied schedule at `minute_of_day`; a
    /// [`EscalationTarget::Principal`] is already concrete. Returns [`EscalationError::NoOneOnCall`]
    /// if the schedule does not cover the instant (surfaced, never a silent drop).
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

    /// **`page(target, reason)` (contract 7.5) — start an escalation DURABLE WORKFLOW.** Resolves the
    /// FIRST step's target at fire time (`oncall_now` for a schedule), pages it (critical pierces
    /// quiet-hours), arms the `ack_window` DURABLE TIMER (9.3), and persists the `escalation_run`
    /// handle so a restart resumes. `reason` is always [`Reason::Escalated`] (class critical). Returns
    /// the started run id + the first [`PageOutcome`]. An empty policy is surfaced as a config error.
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
        // notify(principal, channels, class=critical) — critical ALWAYS pierces quiet-hours (§2.4).
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
        // Arm the ack_window DURABLE TIMER (9.3) — survives a Notif restart, fires effectively-once.
        self.wheel.schedule_timer(&run_id, first.ack_window_minutes);
        Ok((run_id, outcome))
    }

    /// **The escalate-after-timer fire (contract 9.3) — walk to the next step, page EXACTLY ONCE.**
    /// Called when `run_id`'s `ack_window` durable timer fires. Because the wheel's `fire_due` is
    /// effectively-once, a restart-replayed fire is a NO-OP (returns `Ok(None)` — no double page).
    /// The first genuine fire advances `walk`, resolves the next step at fire time, pages it once,
    /// and re-arms the timer (or marks the run `Exhausted` if the chain gave up). An ALREADY-ACKED
    /// run is a no-op (the ack halted the chain — `Ok(None)`).
    #[allow(clippy::too_many_arguments)]
    pub fn advance(
        &self,
        run_id: &str,
        schedule: Option<&OncallSchedule>,
        minute_of_day: i32,
        recipient_quiet: &QuietHours,
        recipient_in_quiet: bool,
    ) -> Result<Option<PageOutcome>, EscalationError> {
        // Effectively-once: only the FIRST fire of this timer does work (a restart-replay → no-op).
        if !self.wheel.fire_due(run_id) {
            return Ok(None);
        }
        let mut runs = self.lock();
        let run = runs
            .get_mut(run_id)
            .ok_or_else(|| EscalationError::UnknownRun(run_id.into()))?;
        // An ack halted the chain before the timer fired — do not page (ack-halt wins the race).
        if run.state != RunState::Active {
            return Ok(None);
        }
        let next_walk = run.walk + 1;
        let Some(step) = run.policy.step_at(next_walk).cloned() else {
            // The chain walked every step `repeat` times unacked → give up.
            run.state = RunState::Exhausted;
            return Ok(None);
        };
        // Resolve the next step at fire time (re-resolve the rotation — who is on call NOW).
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
        // Re-arm the ack_window timer for the new step (the guarded UPDATE — replaces the fired one).
        self.wheel.schedule_timer(run_id, step.ack_window_minutes);
        Ok(Some(outcome))
    }

    /// **Ack-as-event (contract 7.5 / 9.4 / 2.2) — HALT the chain idempotently.** Emits
    /// `notif.escalation.acked` via [`OutboxTx::emit`] (the ONLY emit path — no `publish_now`), cancels
    /// the durable timer (so [`advance`](Self::advance) will not page after the ack), and marks the run
    /// `Acked`. **Idempotent on the run id:** a second ack of the same run acks ONCE (it does not
    /// re-emit, does not re-resolve a signal-wait, does not re-page) — `Ok(false)` on the redundant
    /// ack, `Ok(true)` on the one that halted the chain. The ack is the durable SIGNAL the workflow's
    /// signal-wait (9.4) resolves on. `occurred_at` is when the on-call acknowledged (the frozen
    /// RFC-3339 unit, §2.10); it clocks the emitted ack event.
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
        // Idempotent: a re-ack of an already-acked run halts once (no re-emit, no re-page).
        if already_acked {
            return Ok(false);
        }
        // Emit notif.escalation.acked via the outbox (2.2) — the durable signal the wait resolves on.
        // The actor is the on-call principal who acked (attribution = WHO acknowledged), partitioned
        // to the run's (tenant, region) so the ack event lands in the SAME residency as the run.
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
        // `cause = None`: the ack is a ROOT human action (the on-call acknowledging), not a child of
        // the page event — it is the distinct human-action that resolves the signal-wait.
        tx.emit(draft, None)
            .map_err(|e| EscalationError::AckEmitFailed(format!("{e:?}")))?;
        tx.commit()
            .map_err(|e| EscalationError::AckEmitFailed(format!("{e:?}")))?;

        // Resolve the signal-wait + halt the chain: cancel the timer, mark acked. After this,
        // a fired timer (advance) is a no-op (the run is no longer Active).
        self.wheel.cancel_timer(run_id);
        let mut runs = self.lock();
        if let Some(run) = runs.get_mut(run_id) {
            run.state = RunState::Acked;
            run.acked_by = Some(acked_by);
        }
        Ok(true)
    }
}

/// The [`Reason`] every escalation page carries (architecture §2.4: an escalation is `Reason::Escalated`,
/// `Class::Critical`). A named constant so the router/ranking agree the escalation reason is fixed.
pub const ESCALATION_REASON: Reason = Reason::Escalated;

// ===========================================================================================
//  THE CLI: `myelin oncall show | page` (the deliverable's operator surface)
// ===========================================================================================

/// **`myelin oncall show` — render the on-call rotation roster at an instant (PII-minimised).** Shows
/// WHO is on call now plus the upcoming windows. The principal is an opaque pseudonym (4.8), never a
/// name — references-not-payloads (the operator resolves the display name per-viewer separately). A
/// pure renderer (no I/O); the caller supplies the schedule + the instant.
pub fn render_oncall(schedule: &OncallSchedule, minute_of_day: i32) -> String {
    let mut out = format!("on-call schedule {}\n", schedule.schedule_id);
    match oncall_now(schedule, minute_of_day) {
        Some(p) => out.push_str(&format!("  now on call: {}\n", p.0)),
        None => out.push_str("  now on call: (none — uncovered window)\n"),
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

/// **`myelin oncall page` — render a started-escalation page outcome (the operator confirmation).**
/// Shows WHO was paged, on which channels (the pierce result), and the chain position — the receipt
/// the operator sees when `page` starts an escalation run. PII-minimised (opaque pseudonym).
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

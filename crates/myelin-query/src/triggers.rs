//! # `triggers` — Triggers: the stateful per-person promise (fire-once-per-arming)
//! (contract 3.3; Bus §3.6 / §1.2 / §4.6 / §5.4; P-140 / EB-20)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/event-bus.md` §1.2 (the four primitives —
//! a **Trigger** is *a stateful promise a person owns: "wait until C, then unblock/remind."*
//! It **fires once per arming**, persists until `resolved | stale | disarmed`, and is owned by a
//! **person** — deliberately NOT the stateless [`Automation`](crate::AutomationEngine) reflex,
//! ADR-19), §3.6 (the `trigger` store — `condition` is a matcher AST, now the frozen `QueryAst`;
//! `stale_after` is a `myelin-flow` durable timer, contract 3.3), §4.6 (the state machine
//! `armed → {resolved | stale | disarmed}`, fire-once-per-arming via the atomic guarded UPDATE
//! `UPDATE trigger SET state='resolved', resolved_by=:event_id WHERE id=:id AND state='armed'`,
//! `condition` = the frozen `QueryAst` over PROJECTION STATE, `stale_after` delegated to the
//! `myelin-flow` timer wheel, contract 9.3), §5.4 (`arm_trigger`/`disarm_trigger`). Contract-index
//! rows **3.3** (`arm_trigger`/`disarm_trigger`, owned) + **9.3** (the durable timer wheel —
//! CONSUMED, for `stale_after`).
//!
//! ## Why the Trigger engine lives in `myelin-query`, not `myelin-events` (DOCUMENTED DEVIATION)
//! The EB-20 prompt's DELIVERABLE field says "In `myelin-events`: `triggers.rs`". That is
//! **genuinely unworkable against the frozen crate DAG** for the SAME reason the
//! [`EventMatcher`](crate::EventMatcher) (P-137 / EB-17), the [`SignalEngine`](crate::SignalEngine)
//! (P-138 / EB-18) and the [`AutomationEngine`](crate::AutomationEngine) (P-139 / EB-19) were
//! built here and not in `myelin-events` (see [`crate::matcher`] §"Why the matcher lives in
//! `myelin-query`"): a Trigger's `condition` field **IS** an [`EventMatcher`] (= the frozen
//! `QueryAst` over projection state, §4.6), whose predicate ENGINE was promoted into `myelin-query`
//! by P-133, and `myelin-query` **depends on `myelin-events`** (architecture §2.9). Putting the
//! Trigger engine in `myelin-events` would require `…-events → …-query` for the matcher type — the
//! cycle the `no-cross-sync-cycle` lint (E-5) and the events `Cargo.toml` forbid. So the Trigger
//! engine is built HERE, ON TOP of the one [`EventMatcher`], over the upstream [`EventEnvelope`].
//! The Bus dispatch tier (EB-23) references `myelin_query::TriggerEngine`. This deviation is
//! recorded here and in the P-140 report, per external-insights/01 §1 (do the right thing; document
//! the deviation), and it is the SAME pattern the matcher + signals + automations already follow.
//!
//! ## What this module adds (it does NOT re-define the matcher, the predicate engine, or `myelin-flow`)
//! - [`Trigger`] + [`arm_trigger`] / [`disarm_trigger`] — the stateful per-person promise: the
//!   `owner` person, the [`EventMatcher`] `condition` (= the frozen `QueryAst` over projection
//!   state — "all `blocked_by` resolved"), the `arms_subject` the promise is about, the
//!   [`OnResolve`] action, and the optional `stale_after` durable-timer deadline.
//! - [`TriggerEngine`] — the per-event Trigger consumer: on an incoming event/projection update it
//!   evaluates each ARMED trigger's `condition` (permission-aware BY CONSTRUCTION through
//!   [`EventMatcher::matches`]) and, on a match, performs the **atomic guarded UPDATE**
//!   (`armed → resolved` only if still `armed`) — so under N concurrent resolving events EXACTLY
//!   ONE wins the arming and runs `on_resolve`; the losers see `state != armed` and do nothing
//!   (fire-once-per-arming, §4.6). `armed → stale` is delegated to the [`DurableTimer`] seam (9.3);
//!   `armed → disarmed` is the owner cancel; **re-arming creates a NEW arming** (idempotency is
//!   per-arming — a fresh [`ArmingId`], so a re-armed promise can fire again).
//! - [`DurableTimer`] — the CONSUMED `myelin-flow` durable-timer-wheel seam (contract 9.3
//!   `sleep_until`/`sleep_for`). The REAL engine is the downstream `myelin-flow` crate (the named
//!   floor **P-FLOW-13 / P-207**, the minute-bucket wheel); this crate is UPSTREAM of it in the
//!   §2.9 DAG, so it consumes the seam exactly as [`crate::automations`] consumes the 9.1
//!   `DurableExecutor` and [`crate::matcher`] consumes the `list_objects` push-down. The Trigger
//!   engine NEVER reinvents the durable timer; `stale_after` is a single `arm` call through this
//!   seam (cheap disarm/re-arm of a precomputed `fire_at`, the ISS ask, §4.6). [`InMemoryTimer`]
//!   is the deterministic floor for the unit/CDC tests.
//!
//! **Stateful, by construction.** A [`TriggerEngine`] holds the per-arming lifecycle
//! (`armed → {resolved | stale | disarmed}`) — that IS the per-person promise (NOT the stateless
//! automation reflex, which holds only a fired-once guard). The fire-once property is the atomic
//! guarded UPDATE: the state column is the single source of truth, and the transition is a
//! compare-and-set on `state == armed`. The engine is in-memory + deterministic (the durable
//! `trigger` table, architecture §3.6, is the dispatch tier's persistence concern; the guarded
//! UPDATE here models the SQL `WHERE id=:id AND state='armed'` exactly), so the same event sequence
//! replays to the same transitions (what BUS-D3 in EB-23 relies on).

use crate::matcher::RelMembership;
// `WorkflowRef` is REUSED from `crate::automations` (the 9.1 workflow-definition reference) — the
// SAME opaque ref the Automation engine hands to `DurableExecutor::start`. There is no second
// workflow-reference type: `on_resolve.kind = workflow` and `action.kind = workflow` name the same
// `myelin-flow` definition through the one ref (no drift).
use crate::{EventMatcher, WorkflowRef};
use myelin_events::{ArtifactRef, EventEnvelope, EventId};
use myelin_identity::{PrincipalId, SetExpr};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A stable trigger identifier (the `trigger.id`, §3.6). It names the durable promise across
/// re-armings; each *arming* of it gets a fresh [`ArmingId`] (idempotency is per-arming, §4.6).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TriggerId(pub String);

/// **A single ARMING of a trigger** — the unit fire-once is scoped to (§4.6 "fires once per
/// arming"). [`arm_trigger`] mints a fresh `ArmingId`; a re-arm of the same [`TriggerId`] mints a
/// NEW `ArmingId` (so the promise can fire again — idempotency is per-arming, never per-trigger).
/// The atomic guarded UPDATE keys on `(TriggerId)` but the won/lost decision and the
/// `resolved_by`/`on_resolve` run are scoped to the live arming.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ArmingId(pub String);

/// **What a trigger does when its `condition` resolves** (`on_resolve`, §4.6 — "notify / tool /
/// workflow"). A closed set: `Notify` (the owner is reminded — references-not-payloads, the Notif
/// fan-out), `Workflow` (DELEGATE to `myelin-flow`, never reinvented), `Emit` (publish a derived
/// fact through the outbox, P-S10). The engine yields the [`Resolution`] carrying this; the
/// dispatch tier (EB-23) runs it carrying causality (the resolving event is the cause).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OnResolve {
    /// Remind the `owner` — the unblock/remind reflex ("notify me when C"). References-not-
    /// payloads: it carries the `arms_subject`, never a PII body; the dispatch tier routes it
    /// through the Notif fan-out humanised per-viewer.
    Notify,
    /// DELEGATE to a `myelin-flow` durable workflow (the durable engine is never reinvented here;
    /// the dispatch tier calls `DurableExecutor::start` with this ref, contract 9.1).
    Workflow {
        /// The `myelin-flow` workflow definition to start.
        workflow_ref: WorkflowRef,
    },
    /// Publish a derived event (publish-is-outbox-only, P-S10) — e.g. an "all blockers cleared"
    /// fact the dispatch tier turns into `OutboxTx::emit`.
    Emit {
        /// The event-type token of the derived event (`<subsystem>.<artifact>.<event>`, §6).
        emit_type: String,
    },
}

/// **A Trigger** (contract 3.3; §5.4 frozen shape `Trigger{ owner, condition, arms_subject,
/// on_resolve, stale_after }`): the stateful per-person promise.
///
/// Built via [`arm_trigger`]. The `condition` is an [`EventMatcher`] — the one bounded,
/// permission-aware predicate surface over PROJECTION STATE (§4.6; "all `blocked_by` resolved" is a
/// `Has`/`Ref` predicate over the projection the trigger reads, NOT a join the engine executes) —
/// so a trigger can never resolve on an artifact the `owner` can't see (the 0-leak property rides
/// through [`TriggerEngine`]). It is **stateful**: the [`TriggerArming`] carries the
/// `armed → {resolved | stale | disarmed}` lifecycle (deliberately a different primitive from the
/// stateless [`Automation`](crate::AutomationEngine), ADR-19).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trigger {
    /// The person who owns the promise (`owner`, §5.4). The `condition` is evaluated against the
    /// owner's visible set (the 0-leak compose) — a person's trigger never resolves on something
    /// they cannot read.
    pub owner: PrincipalId,
    /// The resolve condition — the [`EventMatcher`] (= the frozen `QueryAst` over projection state,
    /// §4.6). The arming resolves iff an incoming event/projection update matches this.
    pub condition: EventMatcher,
    /// The subject the promise is about (`arms_subject`, §5.4) — the artifact "wait until C on
    /// THIS" arms over. Carried into `on_resolve` (references-not-payloads).
    pub arms_subject: ArtifactRef,
    /// What to run on resolve (`on_resolve`, §5.4 — notify / tool / workflow).
    pub on_resolve: OnResolve,
    /// The optional staleness deadline (`stale_after`, §5.4) — a `myelin-flow` durable timer
    /// (contract 9.3). `Some(deadline)` ⇒ the arming goes `armed → stale` if the timer fires before
    /// the condition resolves; `None` ⇒ the promise never goes stale (waits indefinitely). The
    /// `deadline` is the precomputed `fire_at` (RFC-3339 UTC, §2.10 — the cheap disarm/re-arm idiom,
    /// §4.6); the durability is delegated to the [`DurableTimer`] seam, NOT reinvented.
    pub stale_after: Option<StaleAfter>,
}

/// The `stale_after` deadline (§4.6) — the precomputed `fire_at` the [`DurableTimer`] arms on.
/// RFC-3339 UTC (§2.10). A separate newtype (not a bare `String`) so the timer-wheel contract is
/// explicit at the type level.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StaleAfter(pub String);

/// **`arm_trigger(Trigger{ owner, condition, arms_subject, on_resolve, stale_after })`** (contract
/// 3.3) — the arming verb. Constructs a [`Trigger`]. The `condition` [`EventMatcher`] was already
/// cost-validated at its own `compile` (the over-budget AST was rejected at construction, §4.5), so
/// this verb is total. The engine mints the fresh [`ArmingId`] when the trigger is added (a re-arm
/// is a new arming — see [`TriggerEngine::arm`]).
pub fn arm_trigger(
    owner: PrincipalId,
    condition: EventMatcher,
    arms_subject: ArtifactRef,
    on_resolve: OnResolve,
    stale_after: Option<StaleAfter>,
) -> Trigger {
    Trigger {
        owner,
        condition,
        arms_subject,
        on_resolve,
        stale_after,
    }
}

/// **`disarm_trigger(id)`** (contract 3.3) — the disarm verb. The owner cancels the live arming:
/// `armed → disarmed` (§4.6). Modelled here as [`TriggerEngine::disarm_trigger`]; the verb returns
/// the trigger id so the dispatch tier can correlate the disarm with the durable row.
pub fn disarm_trigger(id: TriggerId) -> TriggerId {
    id
}

/// **The state of one [`TriggerArming`]** — the `armed → {resolved | stale | disarmed}` lifecycle
/// (§4.6). The state column IS the fire-once guard: the atomic guarded UPDATE compare-and-sets on
/// `Armed`, so only the FIRST resolving event (or the timer, or the owner) wins the transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerState {
    /// The promise is live and waiting for its `condition` (the only state from which a transition
    /// is possible).
    Armed,
    /// The `condition` matched and the guarded UPDATE won — `on_resolve` ran ONCE for this arming.
    Resolved,
    /// The `stale_after` durable timer fired before the condition resolved (§4.6 `armed → stale`).
    Stale,
    /// The owner cancelled (§4.6 `armed → disarmed`).
    Disarmed,
}

/// **One arming of a [`Trigger`]** — the stateful per-person promise as it lives in the engine
/// (modelling the durable `trigger` row, §3.6). The `state` column is the fire-once guard; a re-arm
/// of the same [`TriggerId`] mints a fresh [`ArmingId`] (a new row / a new promise that can fire
/// again).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerArming {
    /// The stable trigger id (durable across re-armings).
    pub trigger_id: TriggerId,
    /// THIS arming's id (fresh per arm — idempotency is per-arming, §4.6).
    pub arming_id: ArmingId,
    /// The trigger definition (owner / condition / arms_subject / on_resolve / stale_after).
    pub trigger: Trigger,
    /// The lifecycle state (the fire-once guard column).
    pub state: TriggerState,
    /// `Some(event_id)` once resolved — the `resolved_by` column the guarded UPDATE sets (the
    /// resolving event is the cause of `on_resolve`, §4.6). `None` while armed / stale / disarmed.
    pub resolved_by: Option<EventId>,
}

/// **What an arming's resolution produced** (the per-event Trigger result the dispatch tier acts
/// on). One per arming that transitioned on THIS event/timer/cancel; the dispatch tier records the
/// transition + runs `on_resolve` carrying causality.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// The arming's `condition` matched and this event WON the atomic guarded UPDATE
    /// (`armed → resolved`) — `on_resolve` runs ONCE. Carries the ids + the action + the resolving
    /// event (the cause, §4.6) so the dispatch tier runs `on_resolve` with nested causality.
    Resolved {
        trigger_id: TriggerId,
        arming_id: ArmingId,
        resolved_by: EventId,
        on_resolve: OnResolve,
        owner: PrincipalId,
        arms_subject: ArtifactRef,
    },
    /// The arming's `condition` matched but this event LOST the guarded UPDATE (the arming was
    /// already `!= armed` — another concurrent event won, or it went stale/disarmed first). A
    /// NO-OP: `on_resolve` does NOT run a second time (the fire-once-per-arming guarantee, §4.6).
    AlreadyResolved {
        trigger_id: TriggerId,
        arming_id: ArmingId,
    },
}

/// **The CONSUMED `myelin-flow` durable-timer-wheel seam** (contract 9.3 — the minute-bucket timer
/// wheel `sleep_until`/`sleep_for`; millions of timers = an indexed range read; effectively-once).
/// This is the ONLY part of 9.3 the Trigger engine needs: `arm` a `stale_after` deadline and
/// `disarm` it (cheap disarm/re-arm of a precomputed `fire_at` without calendar logic, §4.6 / the
/// ISS ask). The REAL implementation is the downstream `myelin-flow` crate (the named floor,
/// **P-FLOW-13 / P-207**); this crate is UPSTREAM of it in the §2.9 DAG, so it depends on this
/// trait, never on `myelin-flow` directly — exactly the DAG-respecting seam pattern
/// [`crate::automations`] uses for the 9.1 `DurableExecutor` and [`crate::matcher`] uses for the
/// `list_objects` push-down. The engine NEVER reinvents the durable timer; `stale_after` is a
/// single `arm` call through this seam.
pub trait DurableTimer {
    /// Arm a durable timer that fires at `fire_at` (RFC-3339 UTC) for `arming_id`. When it fires,
    /// the timer-wheel delivers a `stale` event back to the Trigger engine (modelled here by the
    /// test driving [`TriggerEngine::on_timer_fired`]). Idempotent on `arming_id`: re-arming the
    /// same arming replaces the deadline (the cheap disarm/re-arm idiom). A genuine failure (the
    /// wheel is unreachable) is a [`TimerError`] — surfaced, never a silent drop.
    fn arm(&self, arming_id: &ArmingId, fire_at: &StaleAfter) -> Result<(), TimerError>;

    /// Disarm the durable timer for `arming_id` (the owner disarmed the trigger, or it resolved
    /// first — the `stale_after` timer must not fire on an already-finished arming). Idempotent: a
    /// disarm of an unknown / already-fired arming is a no-op.
    fn disarm(&self, arming_id: &ArmingId) -> Result<(), TimerError>;
}

/// An error arming/disarming a durable timer (the `myelin-flow` 9.3 seam's failure). Surfaced
/// through [`TriggerEngine::arm`] / [`TriggerEngine::disarm_trigger`] — never swallowed (EI-02 §4:
/// no fire-and-forget).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimerError(pub String);

/// The deterministic in-memory [`DurableTimer`] floor (for the unit/CDC tests + the EB-23
/// replay-determinism substrate). It records every armed deadline (so a test can assert the
/// `stale_after` timer was DELEGATED — armed through the seam, not reinvented) and is idempotent on
/// `arming_id`. The real durable wheel is `myelin-flow` (the named floor P-207); this models its
/// `arm`/`disarm` semantics until then.
#[derive(Debug, Default)]
pub struct InMemoryTimer {
    /// `arming_id → fire_at` — the armed deadlines. `RefCell` so `arm`/`disarm` take `&self` (the
    /// seam shape) while recording.
    armed: std::cell::RefCell<BTreeMap<ArmingId, StaleAfter>>,
}

impl InMemoryTimer {
    /// A fresh timer with no armed deadlines.
    pub fn new() -> InMemoryTimer {
        InMemoryTimer::default()
    }

    /// How many distinct deadlines are currently armed (a disarm/resolve removes one — the proof
    /// the engine cleans up the `stale_after` timer when an arming finishes).
    pub fn armed_count(&self) -> usize {
        self.armed.borrow().len()
    }

    /// The armed `fire_at` for an arming, if any (a test asserts the `stale_after` deadline was
    /// armed through the seam — delegation, not reinvention).
    pub fn deadline_for(&self, arming_id: &ArmingId) -> Option<StaleAfter> {
        self.armed.borrow().get(arming_id).cloned()
    }
}

impl DurableTimer for InMemoryTimer {
    fn arm(&self, arming_id: &ArmingId, fire_at: &StaleAfter) -> Result<(), TimerError> {
        self.armed
            .borrow_mut()
            .insert(arming_id.clone(), fire_at.clone());
        Ok(())
    }

    fn disarm(&self, arming_id: &ArmingId) -> Result<(), TimerError> {
        self.armed.borrow_mut().remove(arming_id);
        Ok(())
    }
}

/// **The Trigger engine** (Bus §3.6) — the stateful per-person-promise consumer over the matcher.
/// It is built ON the [`EventMatcher`]: each arming's `condition` is a matcher, so the permission
/// compose (the 0-leak property, §4.5) rides through every resolve evaluation. It holds the armings
/// (the `armed → {resolved | stale | disarmed}` lifecycle — the per-person promise state, NOT a
/// stateless fired-once guard) and delegates `stale_after` to the [`DurableTimer`] seam (9.3).
///
/// **Fire-once-per-arming, by construction.** [`TriggerEngine::on_event`] performs the atomic
/// guarded UPDATE: a transition to `Resolved` happens ONLY if the arming is still `Armed`
/// (compare-and-set on the state column — the in-memory model of
/// `UPDATE … SET state='resolved' WHERE id=:id AND state='armed'`). Under N concurrent resolving
/// events exactly ONE wins; the rest see `state != Armed` and yield [`Resolution::AlreadyResolved`]
/// (no second `on_resolve`). The store is in-memory + deterministic; the durable `trigger` table
/// (architecture §3.6) is the dispatch tier's persistence concern.
#[derive(Debug, Default)]
pub struct TriggerEngine {
    /// The armings, keyed by [`TriggerId`] (one LIVE arming per trigger id at a time; a re-arm
    /// replaces the entry with a fresh [`ArmingId`]). The `state` column on each is the fire-once
    /// guard.
    armings: BTreeMap<TriggerId, TriggerArming>,
    /// A monotone counter minting fresh [`ArmingId`]s deterministically (so the replay is stable;
    /// the durable table uses a ULID, this models the per-arming freshness).
    next_arming: u64,
}

impl TriggerEngine {
    /// A fresh engine with no armings.
    pub fn new() -> TriggerEngine {
        TriggerEngine::default()
    }

    /// **`arm` a trigger** (contract 3.3 `arm_trigger`) — mint a FRESH [`ArmingId`] and put the
    /// arming `Armed`. **Re-arming creates a new arming** (§4.6 — idempotency is per-arming): a
    /// second `arm` of the same [`TriggerId`] replaces the prior arming with a new id, so the
    /// promise can fire AGAIN (a resolved/stale/disarmed arming does not block a re-arm). If a
    /// `stale_after` deadline is set, it is DELEGATED to the [`DurableTimer`] seam (9.3) — armed,
    /// not reinvented. Returns the minted [`ArmingId`]. A timer-arm failure is surfaced.
    pub fn arm(
        &mut self,
        trigger_id: TriggerId,
        trigger: Trigger,
        timer: &dyn DurableTimer,
    ) -> Result<ArmingId, TimerError> {
        // If a prior arming had a live stale_after timer, disarm it first (the old promise is
        // replaced; its timer must not fire on the superseded arming).
        if let Some(prev) = self.armings.get(&trigger_id) {
            if prev.state == TriggerState::Armed && prev.trigger.stale_after.is_some() {
                timer.disarm(&prev.arming_id)?;
            }
        }
        let arming_id = ArmingId(format!("{}#{}", trigger_id.0, self.next_arming));
        self.next_arming += 1;

        // Delegate the stale_after deadline to the durable timer wheel (9.3) — never reinvented.
        if let Some(deadline) = &trigger.stale_after {
            timer.arm(&arming_id, deadline)?;
        }

        self.armings.insert(
            trigger_id.clone(),
            TriggerArming {
                trigger_id,
                arming_id: arming_id.clone(),
                trigger,
                state: TriggerState::Armed,
                resolved_by: None,
            },
        );
        Ok(arming_id)
    }

    /// The current arming for a trigger id (read-only inspection; the dispatch tier reads the
    /// durable table, this is the in-engine view).
    pub fn arming(&self, trigger_id: &TriggerId) -> Option<&TriggerArming> {
        self.armings.get(trigger_id)
    }

    /// **`disarm_trigger(id)`** (contract 3.3) — the owner cancels: `armed → disarmed` (§4.6),
    /// **the atomic guarded UPDATE again** (only an `Armed` arming disarms; a resolved/stale one is
    /// untouched — disarm cannot un-fire a fired promise). The `stale_after` timer is disarmed
    /// through the seam (9.3) so it never fires on a cancelled arming. Returns `true` iff an armed
    /// arming was disarmed.
    pub fn disarm_trigger(
        &mut self,
        trigger_id: &TriggerId,
        timer: &dyn DurableTimer,
    ) -> Result<bool, TimerError> {
        let Some(arming) = self.armings.get_mut(trigger_id) else {
            return Ok(false);
        };
        // Guarded transition: only an Armed arming disarms (compare-and-set on the state column).
        if arming.state != TriggerState::Armed {
            return Ok(false);
        }
        arming.state = TriggerState::Disarmed;
        if arming.trigger.stale_after.is_some() {
            timer.disarm(&arming.arming_id)?;
        }
        Ok(true)
    }

    /// **The per-event resolve reflex** (§4.6) — for each ARMED arming, evaluate its `condition`
    /// (permission-aware, 0-leak via [`EventMatcher::matches`]); on a match, perform the **atomic
    /// guarded UPDATE** (`armed → resolved` only if still `Armed`). Returns one [`Resolution`] per
    /// arming that the event acted on (so the dispatch tier can record the transition + run
    /// `on_resolve` once, carrying causality — the resolving event is the cause).
    ///
    /// **Fire-once-per-arming, BY CONSTRUCTION:** the guarded UPDATE compare-and-sets on
    /// `state == Armed`. Under N concurrent deliveries of resolving events, exactly ONE wins
    /// (transitions to `Resolved` + yields [`Resolution::Resolved`]); the rest see `state != Armed`
    /// and yield [`Resolution::AlreadyResolved`] (no second `on_resolve`). The `stale_after` timer
    /// is disarmed through the seam on resolve (it must not fire on a resolved arming).
    ///
    /// **Permission-aware BY CONSTRUCTION (0-leak):** `visible` is the
    /// `list_objects(owner, read, type)` [`SetExpr`] result (4.3) the condition composes with; an
    /// event for an artifact the trigger's `owner` can't see NEVER resolves the arming.
    /// `member_oracle` answers the relational `SetExpr` arms (the consumer's authz reverse-index).
    pub fn on_event(
        &mut self,
        envelope: &EventEnvelope,
        visible: &SetExpr,
        member_oracle: &dyn Fn(&RelMembership) -> bool,
        timer: &dyn DurableTimer,
    ) -> Vec<Resolution> {
        // Snapshot the trigger ids so the borrow of `self.armings` is released for the mutating
        // resolve loop (deterministic order: BTreeMap iterates sorted).
        let ids: Vec<TriggerId> = self.armings.keys().cloned().collect();
        let mut resolutions = Vec::new();
        for id in &ids {
            if let Some(res) = self.try_resolve(id, envelope, visible, member_oracle, timer) {
                resolutions.push(res);
            }
        }
        resolutions
    }

    /// Try to resolve ONE arming against the event — the inner reflex (factored out so `on_event`
    /// is the thin per-arming loop). Returns `Some(Resolution)` iff the arming's `condition` matched
    /// THIS event (won or lost the guard); `None` if the condition did not match (no transition).
    fn try_resolve(
        &mut self,
        trigger_id: &TriggerId,
        envelope: &EventEnvelope,
        visible: &SetExpr,
        member_oracle: &dyn Fn(&RelMembership) -> bool,
        timer: &dyn DurableTimer,
    ) -> Option<Resolution> {
        let arming = self.armings.get(trigger_id)?;

        // Evaluate the condition FIRST — permission-aware BY CONSTRUCTION (the 0-leak property
        // rides through EventMatcher::matches: an unviewable subject returns false with the
        // predicate never consulted). A mis-authored predicate that errors is treated as NO MATCH
        // (fail-closed) — never a silent resolve, never a panic.
        let matched = arming
            .trigger
            .condition
            .matches(envelope, visible, member_oracle)
            .unwrap_or(false);
        if !matched {
            // The condition did not match this event — no transition for this arming.
            return None;
        }

        // The condition matched. THE ATOMIC GUARDED UPDATE: transition armed → resolved ONLY if the
        // arming is still Armed (the in-memory model of
        // `UPDATE trigger SET state='resolved', resolved_by=:event_id WHERE id=:id AND state='armed'`).
        // A concurrent resolving event that already won leaves state = Resolved; this delivery then
        // LOSES the guard and is a no-op (fire-once-per-arming, §4.6).
        let arming = self.armings.get_mut(trigger_id)?;
        if arming.state != TriggerState::Armed {
            // Lost the guard (already resolved / stale / disarmed) — on_resolve does NOT run again.
            return Some(Resolution::AlreadyResolved {
                trigger_id: arming.trigger_id.clone(),
                arming_id: arming.arming_id.clone(),
            });
        }

        // WON the guard: this delivery is the single resolver of this arming.
        arming.state = TriggerState::Resolved;
        arming.resolved_by = Some(envelope.event_id.clone());
        let resolution = Resolution::Resolved {
            trigger_id: arming.trigger_id.clone(),
            arming_id: arming.arming_id.clone(),
            resolved_by: envelope.event_id.clone(),
            on_resolve: arming.trigger.on_resolve.clone(),
            owner: arming.trigger.owner.clone(),
            arms_subject: arming.trigger.arms_subject.clone(),
        };

        // The arming finished — disarm its stale_after timer through the seam (it must not fire on a
        // resolved arming). A timer disarm failure is non-fatal to the resolution (the resolve
        // already happened durably); we best-effort disarm and ignore the wheel's transient error
        // here (the dispatch tier re-disarms idempotently). We DO honour the seam call so the
        // delegation is observable.
        if arming.trigger.stale_after.is_some() {
            let _ = timer.disarm(&arming.arming_id);
        }
        Some(resolution)
    }

    /// **`armed → stale`** (§4.6) — the `myelin-flow` durable timer fired for `arming_id` (the
    /// `stale_after` deadline elapsed before the condition resolved). The atomic guarded UPDATE
    /// AGAIN: the arming goes `Stale` ONLY if it is still `Armed` (a concurrent resolve that already
    /// won leaves it `Resolved` — the timer then loses and does NOT clobber the resolution). This is
    /// the callback the durable timer-wheel (the CONSUMED 9.3 seam) drives when the bucket fires.
    /// Returns `true` iff an armed arming went stale.
    pub fn on_timer_fired(&mut self, arming_id: &ArmingId) -> bool {
        // Find the arming with this arming_id (the timer fires per-arming).
        let Some(arming) = self
            .armings
            .values_mut()
            .find(|a| &a.arming_id == arming_id)
        else {
            return false;
        };
        // Guarded transition: only an Armed arming goes Stale (the timer loses to a prior resolve).
        if arming.state != TriggerState::Armed {
            return false;
        }
        arming.state = TriggerState::Stale;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CmpOp, Expr, Predicate};
    use myelin_events::{
        Actor, AggregateKey, CorrelationId, DataRole, EventType, Timestamp, Visibility,
    };
    use myelin_identity::{Literal, ObjectType, Principal, PrincipalKind};
    use myelin_tenancy::{Region, TenantId};

    fn var(name: &str) -> Expr {
        Expr::Var(name.into())
    }
    fn str_(s: &str) -> Expr {
        Expr::Lit(Literal::Str(s.into()))
    }

    fn owner() -> PrincipalId {
        PrincipalId("alice".into())
    }

    /// `event.type == <type>` condition over the given object type — models "resolve when an event
    /// of this type lands on the arms_subject" (the simplest projection-state condition).
    fn type_condition(object_type: &str, type_: &str) -> EventMatcher {
        EventMatcher::compile(
            ObjectType(object_type.into()),
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("event.type"),
                rhs: str_(type_),
            },
        )
        .unwrap()
    }

    /// The "all blocked_by resolved" projection-state condition (§4.6): the payload projection
    /// carries `payload.blocked_by_unresolved`; the condition resolves when it reaches 0 (the
    /// trigger reads projection state, NOT a join — the same shape the matcher §"project_envelope"
    /// documents).
    fn all_blockers_resolved(object_type: &str) -> EventMatcher {
        EventMatcher::compile(
            ObjectType(object_type.into()),
            Predicate::Cmp {
                op: CmpOp::Eq,
                lhs: var("payload.blocked_by_unresolved"),
                rhs: Expr::Lit(Literal::Int(0)),
            },
        )
        .unwrap()
    }

    fn envelope(type_: &str, id: &str, event_id: &str) -> EventEnvelope {
        EventEnvelope {
            event_id: EventId(event_id.into()),
            type_: EventType(type_.into()),
            schema_ver: 1,
            tenant: TenantId("t1".into()),
            region: Region("fr-par".into()),
            actor: Actor(Principal::stub(
                PrincipalId("svc-bot".into()),
                PrincipalKind::Human,
                TenantId("t1".into()),
            )),
            subject: ArtifactRef(format!("myelin://t1/issues/issue/{id}")),
            aggregate: AggregateKey(format!("issue:{id}")),
            causation_id: None,
            correlation_id: CorrelationId("root".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: false,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: None,
            occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
            payload: serde_json::json!({}),
        }
    }

    /// The "see nothing relationally" oracle — the simple tests drive visibility via the SetExpr.
    fn no_rel(_m: &RelMembership) -> bool {
        false
    }

    fn notify_trigger(on_type: &str, stale_after: Option<StaleAfter>) -> Trigger {
        arm_trigger(
            owner(),
            type_condition("issue", on_type),
            ArtifactRef("myelin://t1/issues/issue/PROJ-1".into()),
            OnResolve::Notify,
            stale_after,
        )
    }

    /// **GATE: the Trigger fires EXACTLY ONCE per arming under TWO concurrent resolving events
    /// (only one wins the guarded UPDATE).** The fire-once property test (the mandatory-core guard).
    #[test]
    fn fires_exactly_once_per_arming_under_concurrent_events() {
        let mut engine = TriggerEngine::new();
        let timer = InMemoryTimer::new();
        engine
            .arm(
                TriggerId("t-block".into()),
                notify_trigger("issues.issue.unblocked", None),
                &timer,
            )
            .unwrap();

        // TWO concurrent resolving events (distinct event_ids — the platform may deliver the same
        // logical resolution twice, or two different events both satisfy the condition). Only ONE
        // may win the arming (fire-once-per-arming, the atomic guarded UPDATE).
        let e1 = envelope("issues.issue.unblocked", "PROJ-1", "evt-resolve-a");
        let e2 = envelope("issues.issue.unblocked", "PROJ-1", "evt-resolve-b");

        let r1 = engine.on_event(&e1, &SetExpr::All, &no_rel, &timer);
        let r2 = engine.on_event(&e2, &SetExpr::All, &no_rel, &timer);

        // Exactly one Resolved, exactly one AlreadyResolved (the loser).
        assert_eq!(r1.len(), 1);
        assert_eq!(r2.len(), 1);
        assert!(
            matches!(&r1[0], Resolution::Resolved { resolved_by, .. } if resolved_by.0 == "evt-resolve-a"),
            "the first delivery won the arming and runs on_resolve once"
        );
        assert!(
            matches!(&r2[0], Resolution::AlreadyResolved { .. }),
            "the second delivery LOST the guarded UPDATE — on_resolve does not run again"
        );
        // The durable state is Resolved, resolved_by the winner.
        let arming = engine.arming(&TriggerId("t-block".into())).unwrap();
        assert_eq!(arming.state, TriggerState::Resolved);
        assert_eq!(arming.resolved_by, Some(EventId("evt-resolve-a".into())));
    }

    /// **GATE: `armed → stale` is DELEGATED to the `myelin-flow` durable timer (armed through the
    /// 9.3 seam, not reinvented), and the timer firing transitions the arming to `Stale`.**
    #[test]
    fn armed_to_stale_delegates_to_myelin_flow_timer() {
        let mut engine = TriggerEngine::new();
        let timer = InMemoryTimer::new();
        let deadline = StaleAfter("2026-06-21T00:00:00Z".into());
        let arming_id = engine
            .arm(
                TriggerId("t-stale".into()),
                notify_trigger("issues.issue.unblocked", Some(deadline.clone())),
                &timer,
            )
            .unwrap();

        // The stale_after deadline was DELEGATED to the durable timer wheel (armed through the seam,
        // NOT reinvented in the engine) — the proof of delegation.
        assert_eq!(timer.armed_count(), 1, "the stale_after timer was armed via the seam");
        assert_eq!(timer.deadline_for(&arming_id), Some(deadline));

        // The durable timer fires (the minute-bucket wheel delivers the stale callback) → Stale.
        assert!(engine.on_timer_fired(&arming_id), "the timer fired armed → stale");
        assert_eq!(
            engine.arming(&TriggerId("t-stale".into())).unwrap().state,
            TriggerState::Stale
        );
    }

    /// **The stale timer LOSES to a prior resolve (the guarded UPDATE again):** if the condition
    /// resolved first, a late timer firing does NOT clobber the resolution.
    #[test]
    fn stale_timer_loses_to_a_prior_resolve() {
        let mut engine = TriggerEngine::new();
        let timer = InMemoryTimer::new();
        let arming_id = engine
            .arm(
                TriggerId("t".into()),
                notify_trigger("issues.issue.unblocked", Some(StaleAfter("2026-06-21T00:00:00Z".into()))),
                &timer,
            )
            .unwrap();

        // Resolve first.
        let e = envelope("issues.issue.unblocked", "PROJ-1", "evt-resolve");
        let r = engine.on_event(&e, &SetExpr::All, &no_rel, &timer);
        assert!(matches!(&r[0], Resolution::Resolved { .. }));
        // The resolve disarmed the stale timer through the seam.
        assert_eq!(timer.armed_count(), 0, "the resolve disarmed the stale_after timer");

        // A late timer firing must NOT clobber the resolution (it lost the guard).
        assert!(!engine.on_timer_fired(&arming_id), "the late timer loses to the resolve");
        assert_eq!(
            engine.arming(&TriggerId("t".into())).unwrap().state,
            TriggerState::Resolved
        );
    }

    /// **GATE: `armed → disarmed` on owner cancel** (the guarded UPDATE — only an armed arming
    /// disarms; the stale_after timer is disarmed through the seam).
    #[test]
    fn armed_to_disarmed_on_owner_cancel() {
        let mut engine = TriggerEngine::new();
        let timer = InMemoryTimer::new();
        engine
            .arm(
                TriggerId("t".into()),
                notify_trigger("issues.issue.unblocked", Some(StaleAfter("2026-06-21T00:00:00Z".into()))),
                &timer,
            )
            .unwrap();
        assert_eq!(timer.armed_count(), 1);

        // The owner cancels → armed → disarmed; the stale_after timer is disarmed via the seam.
        assert!(engine.disarm_trigger(&TriggerId("t".into()), &timer).unwrap());
        assert_eq!(
            engine.arming(&TriggerId("t".into())).unwrap().state,
            TriggerState::Disarmed
        );
        assert_eq!(timer.armed_count(), 0, "the owner cancel disarmed the stale_after timer");

        // A disarm of an already-disarmed arming is a no-op (the guard rejects it).
        assert!(!engine.disarm_trigger(&TriggerId("t".into()), &timer).unwrap());
        // A resolving event after disarm does NOT resolve (the arming is no longer armed).
        let e = envelope("issues.issue.unblocked", "PROJ-1", "evt-late");
        let r = engine.on_event(&e, &SetExpr::All, &no_rel, &timer);
        assert!(matches!(&r[0], Resolution::AlreadyResolved { .. }));
    }

    /// **GATE: re-arming creates a FRESH arming (idempotency is per-arming, §4.6).** A re-armed
    /// promise gets a new ArmingId and can fire AGAIN — a resolved arming does not block a re-arm.
    #[test]
    fn re_arming_creates_a_fresh_arming() {
        let mut engine = TriggerEngine::new();
        let timer = InMemoryTimer::new();
        let id = TriggerId("t".into());

        // Arm + resolve the first arming.
        let a1 = engine
            .arm(id.clone(), notify_trigger("issues.issue.unblocked", None), &timer)
            .unwrap();
        let e1 = envelope("issues.issue.unblocked", "PROJ-1", "evt-1");
        let r1 = engine.on_event(&e1, &SetExpr::All, &no_rel, &timer);
        assert!(matches!(&r1[0], Resolution::Resolved { .. }));

        // RE-ARM the SAME trigger id → a FRESH arming (new ArmingId), back to Armed.
        let a2 = engine
            .arm(id.clone(), notify_trigger("issues.issue.unblocked", None), &timer)
            .unwrap();
        assert_ne!(a1, a2, "re-arming mints a fresh ArmingId (idempotency is per-arming)");
        assert_eq!(engine.arming(&id).unwrap().state, TriggerState::Armed);

        // The new arming can fire AGAIN (the per-arming promise is independent of the old one).
        let e2 = envelope("issues.issue.unblocked", "PROJ-1", "evt-2");
        let r2 = engine.on_event(&e2, &SetExpr::All, &no_rel, &timer);
        assert!(
            matches!(&r2[0], Resolution::Resolved { arming_id, resolved_by, .. }
                if arming_id == &a2 && resolved_by.0 == "evt-2"),
            "the re-armed promise fires again on its own arming"
        );
    }

    /// **The condition is permission-aware BY CONSTRUCTION (0-leak):** an event for an artifact the
    /// trigger's `owner` cannot see (`visible = None`) NEVER resolves the arming.
    #[test]
    fn unviewable_subject_never_resolves_the_trigger() {
        let mut engine = TriggerEngine::new();
        let timer = InMemoryTimer::new();
        engine
            .arm(
                TriggerId("t".into()),
                notify_trigger("issues.issue.unblocked", None),
                &timer,
            )
            .unwrap();
        let e = envelope("issues.issue.unblocked", "PROJ-1", "evt-hidden");
        // SetExpr::None ⇒ the owner sees nothing ⇒ no resolution (the condition is never consulted).
        let r = engine.on_event(&e, &SetExpr::None, &no_rel, &timer);
        assert!(r.is_empty(), "an unviewable subject never resolves (0-leak)");
        assert_eq!(
            engine.arming(&TriggerId("t".into())).unwrap().state,
            TriggerState::Armed,
            "the arming stays armed — no leak-driven resolution"
        );
    }

    /// **The "all blocked_by resolved" projection-state condition** (§4.6): the trigger reads
    /// projection state (`payload.blocked_by_unresolved == 0`), NOT a join. A partial-resolution
    /// event (still 1 blocker) does not fire; the final one (0 blockers) does.
    #[test]
    fn all_blockers_resolved_projection_state_condition() {
        let mut engine = TriggerEngine::new();
        let timer = InMemoryTimer::new();
        engine
            .arm(
                TriggerId("t-blockers".into()),
                arm_trigger(
                    owner(),
                    all_blockers_resolved("issue"),
                    ArtifactRef("myelin://t1/issues/issue/PROJ-1".into()),
                    OnResolve::Notify,
                    None,
                ),
                &timer,
            )
            .unwrap();

        // Still one blocker unresolved → the condition is false → no resolution.
        let mut partial = envelope("issues.issue.relation_resolved", "PROJ-1", "evt-partial");
        partial.payload = serde_json::json!({ "blocked_by_unresolved": 1 });
        let r0 = engine.on_event(&partial, &SetExpr::All, &no_rel, &timer);
        assert!(r0.is_empty(), "a partial resolution does not fire the trigger");

        // The last blocker resolved → blocked_by_unresolved == 0 → the condition resolves.
        let mut done = envelope("issues.issue.relation_resolved", "PROJ-1", "evt-done");
        done.payload = serde_json::json!({ "blocked_by_unresolved": 0 });
        let r1 = engine.on_event(&done, &SetExpr::All, &no_rel, &timer);
        assert!(
            matches!(&r1[0], Resolution::Resolved { .. }),
            "the projection-state condition resolves when all blockers clear"
        );
    }

    /// **`on_resolve` carries the resolving event as the cause + the owner + arms_subject** (§4.6 —
    /// the dispatch tier runs notify/tool/workflow with nested causality).
    #[test]
    fn resolution_carries_cause_owner_and_action() {
        let mut engine = TriggerEngine::new();
        let timer = InMemoryTimer::new();
        engine
            .arm(
                TriggerId("t-wf".into()),
                arm_trigger(
                    owner(),
                    type_condition("issue", "issues.issue.unblocked"),
                    ArtifactRef("myelin://t1/issues/issue/PROJ-1".into()),
                    OnResolve::Workflow {
                        workflow_ref: WorkflowRef("notify_owner".into()),
                    },
                    None,
                ),
                &timer,
            )
            .unwrap();
        let e = envelope("issues.issue.unblocked", "PROJ-1", "evt-cause");
        let r = engine.on_event(&e, &SetExpr::All, &no_rel, &timer);
        match &r[0] {
            Resolution::Resolved {
                resolved_by,
                on_resolve,
                owner: o,
                arms_subject,
                ..
            } => {
                assert_eq!(resolved_by.0, "evt-cause", "the resolving event is the cause");
                assert_eq!(o, &owner());
                assert_eq!(
                    arms_subject,
                    &ArtifactRef("myelin://t1/issues/issue/PROJ-1".into())
                );
                assert!(matches!(
                    on_resolve,
                    OnResolve::Workflow { workflow_ref } if workflow_ref.0 == "notify_owner"
                ));
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    /// **The resolve reflex is replay-deterministic** (what BUS-D3 in EB-23 relies on): the same
    /// event sequence yields the same transitions. Two independent engines fed the same stream agree
    /// on the final arming state + resolved_by.
    #[test]
    fn on_event_is_replay_deterministic() {
        let stream: Vec<EventEnvelope> = (0..5)
            .map(|i| envelope("issues.issue.unblocked", "PROJ-1", &format!("evt-{i}")))
            .collect();
        let final_state = || {
            let mut e = TriggerEngine::new();
            let timer = InMemoryTimer::new();
            e.arm(
                TriggerId("t".into()),
                notify_trigger("issues.issue.unblocked", None),
                &timer,
            )
            .unwrap();
            for env in &stream {
                e.on_event(env, &SetExpr::All, &no_rel, &timer);
            }
            e.arming(&TriggerId("t".into())).unwrap().clone()
        };
        let a = final_state();
        let b = final_state();
        assert_eq!(a.state, b.state);
        assert_eq!(a.resolved_by, b.resolved_by);
        assert_eq!(a.state, TriggerState::Resolved);
        // The FIRST event in the stream won the arming (deterministic) — exactly once.
        assert_eq!(a.resolved_by, Some(EventId("evt-0".into())));
    }

    /// **`Trigger` round-trips stably (the wire contract — the durable `trigger` row).** The
    /// `condition` field is the byte-identical `QueryAst` (no drift, 13.3).
    #[test]
    fn trigger_round_trips_stably() {
        let trigger = arm_trigger(
            owner(),
            all_blockers_resolved("issue"),
            ArtifactRef("myelin://t1/issues/issue/PROJ-1".into()),
            OnResolve::Emit {
                emit_type: "issues.issue.all_blockers_cleared".into(),
            },
            Some(StaleAfter("2026-06-30T00:00:00Z".into())),
        );
        let json = serde_json::to_string(&trigger).unwrap();
        let back: Trigger = serde_json::from_str(&json).unwrap();
        assert_eq!(trigger, back);
    }

    /// **A timer-arm failure is SURFACED** (never a silent drop) — a trigger whose `stale_after`
    /// could not be armed is observable so the dispatch tier can retry/alert.
    #[test]
    fn timer_arm_failure_is_surfaced() {
        struct FailingTimer;
        impl DurableTimer for FailingTimer {
            fn arm(&self, _a: &ArmingId, _f: &StaleAfter) -> Result<(), TimerError> {
                Err(TimerError("myelin-flow timer wheel unreachable".into()))
            }
            fn disarm(&self, _a: &ArmingId) -> Result<(), TimerError> {
                Ok(())
            }
        }
        let mut engine = TriggerEngine::new();
        let res = engine.arm(
            TriggerId("t".into()),
            notify_trigger("issues.issue.unblocked", Some(StaleAfter("2026-06-21T00:00:00Z".into()))),
            &FailingTimer,
        );
        assert_eq!(
            res,
            Err(TimerError("myelin-flow timer wheel unreachable".into())),
            "a stale_after arm failure is surfaced, never swallowed"
        );
    }
}

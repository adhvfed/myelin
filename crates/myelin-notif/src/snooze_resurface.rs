//! # Snooze re-surfacing on the SAME durable timer wheel — one substrate, three uses (NOTIF-P18 / P-196, M2)
//!
//! **Owning architecture doc:** `notifications.md` §3.7 (snooze re-surfacing and SLA timers ride the
//! SAME minute-bucket wheel as escalation — **one substrate, three uses**), §2.4 (the durable timer
//! wheel is `myelin-flow`'s, **not** in-process sleeps — it survives a Notif restart and fires
//! effectively-once), §2.1 (`snooze(item, until)` records the `snooze_until` and PARKS the item; the
//! re-surface flips it back to `unread` so it re-enters the active inbox at the `until`).
//!
//! **Contracts (CONSUMED — none owned here).** **9.3** the durable timer wheel (millions of timers,
//! effectively-once) — consumed via the SAME [`DurableWheel`](crate::escalation::DurableWheel) seam
//! escalation (NOTIF-P14) uses; **7.2** `snooze(item_id, until)` — owned in NOTIF-P6
//! ([`crate::read_state::snooze`]), the `until` it records is what this module's timer re-surfaces.
//! Frozen signatures only.
//!
//! ## What this prompt (NOTIF-P18) ships — snooze re-surfacing, nothing else
//!
//! NOTIF-P6 ([`crate::read_state`]) made `snooze(item, until)` RECORD the `snooze_until` and PARK the
//! item (suppressed from the active inbox) — but it named the durable re-surface TIMER as the floor
//! "NOTIF-P14 / NOTIF-P18". This prompt fills that floor: a snoozed item re-surfaces at its `until`
//! via a `myelin-flow` DURABLE TIMER that
//!
//! 1. **survives a Notif restart** — the timer is a durable handle on the wheel (the same persisted
//!    substrate escalation arms), not an in-process `sleep`. A worker kill before the `until` does NOT
//!    lose the re-surface; a fresh worker resumes from the persisted handle.
//! 2. **fires effectively-once** — the wheel's [`fire_due`](crate::escalation::DurableWheel::fire_due)
//!    is effectively-once (it consumes the handle), so a restart-replayed fire is a NO-OP. The item
//!    re-surfaces EXACTLY ONCE: never zero (the durable handle survives), never twice (the consumed
//!    handle).
//! 3. **clears the snoozed state** — at the `until` the item's `state` flips `snoozed → unread` and
//!    the `snooze_until` is cleared, so it re-enters the ACTIVE inbox ([`crate::read_state::active_inbox`]
//!    now shows it again).
//!
//! ## ONE substrate, three uses (the load-bearing reconcile — NOT a second timer mechanism)
//!
//! The durability is the SAME [`DurableWheel`](crate::escalation::DurableWheel) trait the escalation
//! chain (NOTIF-P14) arms its `ack_window` timers on. There is **no second timer substrate** and **no
//! in-process sleep** anywhere in this module — `arm_snooze_timer` calls
//! [`DurableWheel::schedule_timer`], the re-surface fire is gated on
//! [`DurableWheel::fire_due`], and a cancel (a manual un-snooze before the `until`) calls
//! [`DurableWheel::cancel_timer`]. Three uses of one wheel: **escalation** ack-window timers
//! (NOTIF-P14), **snooze** re-surface timers (here), **SLA** timers (the third use — NOTIF-P21, a
//! named FLOOR below). The [`SNOOZE_TIMER_NS`] namespace prefix keys snooze timers distinctly from
//! escalation runs on the shared wheel (one wheel, distinct keys — no collision).
//!
//! ## FLOORS named
//!
//! - **SLA timers (the THIRD use of the wheel)** are driven by Issues' real SLA policy in N-M4 —
//!   **NOTIF-P21**. Here only escalation (NOTIF-P14) + snooze (this prompt) ride the wheel; the SLA
//!   timer is named, not built.
//! - **The real `myelin-flow` durable executor/timer** is **P-FLOW-09 / P-FLOW-13** (the minute-bucket
//!   wheel + the durable signal). The [`InMemoryWheel`](crate::escalation::InMemoryWheel) models the
//!   effectively-once timer so the re-surface POLICY (the thing THIS prompt owns) + the kill/resume
//!   durability drill are proven today against the seam, and wired to the real engine when
//!   `myelin-flow` lands. This is the SAME in-memory-now / flow-later seam escalation uses.
//! - **The live OLTP `notif_inbox_item` re-surface UPDATE** (`SET state='unread', snooze_until=NULL
//!   WHERE … AND state='snoozed'`) is modelled by [`crate::router::InboxProjection::mutate_state`];
//!   the live-Postgres binding is the named integration floor the data model (NOTIF-P2) carries.
//!
//! ## Mutation floor (the snooze-timer module — mandatory-core)
//!
//! Snooze re-surfacing is mandatory-core (a missed re-surface is a notification lost forever — the
//! item never returns; a double re-surface is a duplicate buzz). The mutation-tested core is the
//! POLICY: [`snooze_timer_key`] (the timer key namespacing — a snooze key never collides with an
//! escalation run), the effectively-once re-surface ([`SnoozeResurfacer::resurface_due`] — flips
//! `snoozed → unread` exactly once, never on a replay, never on a non-snoozed row), and the
//! arm/cancel pair ([`SnoozeResurfacer::arm`] / [`SnoozeResurfacer::cancel`] — a manual un-snooze
//! disarms the timer so it does not re-surface a row the user already actioned).
//! **Floor: ≥ 80% line/branch mutation score on `snooze_resurface.rs`** (measured with
//! `cargo mutants`; reported in the P-196 commit body).
//!
//! **Measured (P-196):** `cargo mutants --file crates/myelin-notif/src/snooze_resurface.rs` — see the
//! commit body for the caught/missed/unviable counts (≥ 80% floor MET).

use myelin_identity::Principal;
use myelin_tenancy::TenantId;

use crate::escalation::DurableWheel;
use crate::read_state::ReadState;
use crate::router::InboxProjection;

/// The **timer-key namespace prefix for a snooze re-surface timer** on the SHARED durable wheel.
/// Escalation runs key their `ack_window` timers on the raw `run_id`; snooze re-surface timers key on
/// `snooze:{tenant}:{recipient}:{item_id}` — so the ONE wheel serves both uses with NO key collision
/// (a snooze timer and an escalation run with the same opaque id never alias). The third use (SLA,
/// NOTIF-P21) gets its own prefix when it lands.
pub const SNOOZE_TIMER_NS: &str = "snooze:";

/// **The durable-timer key for a snooze re-surface (the SHARED-wheel namespacing).** A snoozed item
/// is keyed `snooze:{tenant}:{recipient}:{item_id}` so its re-surface timer lives on the SAME wheel as
/// escalation without colliding (one substrate, distinct keys). The key is deterministic from the
/// row's identity, so arming/cancelling/firing the same item always hits the same wheel handle (a
/// re-snooze REPLACES the prior handle via the wheel's guarded UPDATE).
pub fn snooze_timer_key(tenant: &TenantId, recipient: &str, item_id: &str) -> String {
    format!("{SNOOZE_TIMER_NS}{}:{}:{}", tenant.0, recipient, item_id)
}

/// The outcome of a snooze re-surface attempt (the observable the durability drill asserts on).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResurfaceOutcome {
    /// The item re-surfaced: its `state` flipped `snoozed → unread`, `snooze_until` cleared. It is
    /// back in the active inbox. This is the EXACTLY-ONCE fire (the durable handle was consumed).
    Resurfaced,
    /// The timer fire was a NO-OP: either a restart-replayed fire (the effectively-once handle was
    /// already consumed) OR the row was no longer snoozed (the user manually un-snoozed first). Either
    /// way the item is NOT re-surfaced a second time (0 duplicate).
    NoOp,
}

/// **The snooze re-surfacer — owns the re-surface POLICY over the SHARED [`DurableWheel`] seam.**
/// `arm` schedules a durable re-surface timer for a freshly-snoozed item (the same wheel escalation
/// uses); `resurface_due` is the timer fire (flip `snoozed → unread` effectively-once); `cancel`
/// disarms a re-surface timer when the user manually un-snoozes before the `until`. The wheel is the
/// `myelin-flow` durable substrate (9.3) — a kill before the `until` does not lose the re-surface, a
/// restart-replayed fire re-surfaces NOTHING.
///
/// **One substrate, three uses:** this holds the SAME `W: DurableWheel` escalation arms its timers on
/// — there is no second timer mechanism and no in-process sleep. The [`InboxProjection`] is the model
/// of the `notif_inbox_item` table the live OLTP re-surface UPDATE hits.
pub struct SnoozeResurfacer<W: DurableWheel> {
    wheel: W,
}

impl<W: DurableWheel> SnoozeResurfacer<W> {
    /// Build the re-surfacer over a durable wheel — the SAME wheel the escalation engine arms its
    /// `ack_window` timers on (one substrate, three uses). Pass the cell's shared
    /// [`InMemoryWheel`](crate::escalation::InMemoryWheel) (or, when it lands, the `myelin-flow`
    /// durable wheel) so snooze + escalation + SLA all ride one substrate.
    pub fn new(wheel: W) -> SnoozeResurfacer<W> {
        SnoozeResurfacer { wheel }
    }

    /// The durable wheel (so a drill can fire a due timer / assert a live handle / count timers).
    pub fn wheel(&self) -> &W {
        &self.wheel
    }

    /// **Arm the durable re-surface timer for a freshly-snoozed item (contract 9.3).** Called right
    /// after [`crate::read_state::snooze`] records the `until` and parks the item: this schedules a
    /// `myelin-flow` DURABLE TIMER (NOT an in-process sleep) on the SHARED wheel, keyed
    /// [`snooze_timer_key`], to fire at the `until`. The timer survives a Notif restart (it is a
    /// persisted handle); a re-snooze of the same item REPLACES the prior handle (the wheel's guarded
    /// UPDATE). `until_minutes` is the delay until the re-surface (the wheel's bucket unit — the real
    /// wheel computes it from the `snooze_until` RFC-3339 instant; the seam takes the delay directly so
    /// the POLICY is testable).
    pub fn arm(&self, tenant: &TenantId, recipient: &str, item_id: &str, until_minutes: u32) {
        let key = snooze_timer_key(tenant, recipient, item_id);
        // The SAME `schedule_timer` escalation arms its ack_window timer with — one substrate. No
        // in-process sleep: the delay lives on the durable wheel, surviving a restart.
        self.wheel.schedule_timer(&key, until_minutes);
    }

    /// **Whether a snoozed item has a live (un-fired, un-cancelled) durable re-surface timer.** The
    /// persisted handle a restart resumes from — used by the durability drill to assert the timer
    /// survived the kill.
    pub fn has_timer(&self, tenant: &TenantId, recipient: &str, item_id: &str) -> bool {
        self.wheel
            .has_timer(&snooze_timer_key(tenant, recipient, item_id))
    }

    /// **The re-surface fire (contract 9.3) — flip `snoozed → unread` EXACTLY ONCE.** Called when the
    /// item's re-surface durable timer fires (at the `until`). Because the wheel's `fire_due` is
    /// effectively-once, a restart-replayed fire is a NO-OP ([`ResurfaceOutcome::NoOp`] — 0 duplicate
    /// re-surface). The first genuine fire flips the row `snoozed → unread` and clears `snooze_until`
    /// (so the item re-enters the active inbox) — but ONLY if the row is still `snoozed` (a row the
    /// user manually un-snoozed first is left as-is: no re-surface of an already-actioned item). The
    /// row is matched by `(tenant, recipient, item_id)` (recipient-scoped — it never re-surfaces
    /// another principal's item).
    pub fn resurface_due(
        &self,
        inbox: &InboxProjection,
        tenant: &TenantId,
        recipient: &str,
        item_id: &str,
    ) -> ResurfaceOutcome {
        let key = snooze_timer_key(tenant, recipient, item_id);
        // Effectively-once: only the FIRST fire of this timer does work (a restart-replay → no-op).
        // This consumes the durable handle, so a replayed fire returns `false` (no double re-surface).
        if !self.wheel.fire_due(&key) {
            return ResurfaceOutcome::NoOp;
        }
        // Flip the row `snoozed → unread` and clear the `until` — but ONLY if it is STILL snoozed. A
        // user who manually marked/un-snoozed the item before the `until` already actioned it; the
        // timer fire must not resurrect an already-handled item. `mutate_state` is recipient-scoped.
        let mut flipped = false;
        let found = inbox.mutate_state(tenant, recipient, item_id, |row| {
            if row.state == ReadState::Snoozed.token() {
                row.state = ReadState::Unread.token().to_string();
                row.snooze_until = None;
                flipped = true;
            }
        });
        if found && flipped {
            ResurfaceOutcome::Resurfaced
        } else {
            // The row vanished (erased/aged out) OR was no longer snoozed (manually actioned) — the
            // re-surface is a no-op (the durable handle was already consumed above; this fire is spent).
            ResurfaceOutcome::NoOp
        }
    }

    /// **Cancel a snooze re-surface timer (a manual un-snooze before the `until`).** When the user
    /// marks/un-snoozes a snoozed item before its `until` ([`crate::read_state::mark`]), the pending
    /// re-surface timer must be disarmed so it does not later flip the (now actioned) item back to
    /// `unread`. Idempotent (cancelling a non-existent timer is a no-op). Uses the SHARED wheel's
    /// `cancel_timer` (one substrate).
    pub fn cancel(&self, tenant: &TenantId, recipient: &str, item_id: &str) {
        self.wheel
            .cancel_timer(&snooze_timer_key(tenant, recipient, item_id));
    }
}

/// **`snooze_and_arm` — the integrated snooze path: record the `until` (7.2) AND arm the durable
/// re-surface timer (9.3) atomically from the caller's view.** NOTIF-P6's [`crate::read_state::snooze`]
/// records the `until` + parks the item; this wraps it so the durable re-surface timer is armed in the
/// SAME call (the `until` recorded in the row and the timer on the wheel never drift). Returns the
/// snooze result; a not-for-me / missing item snoozes NOTHING and arms NOTHING (the error surfaces).
///
/// `until` is the persisted RFC-3339 instant (the `snooze_until` column); `until_minutes` is the
/// derived delay the wheel arms (the real wheel computes it from `until`; the seam takes it directly).
pub fn snooze_and_arm<W: DurableWheel>(
    inbox: &InboxProjection,
    resurfacer: &SnoozeResurfacer<W>,
    principal: &Principal,
    item_id: &str,
    until: &str,
    until_minutes: u32,
) -> Result<(), crate::read_state::ReadStateError> {
    // Record the until + park the item (the NOTIF-P6 read-state truth — the ONE store).
    crate::read_state::snooze(inbox, principal, item_id, until)?;
    // Arm the durable re-surface timer on the SHARED wheel (only if the snooze applied — a
    // not-for-me item returned Err above and never reaches here, so no orphan timer is armed).
    resurfacer.arm(
        &principal.tenant,
        principal.principal_id.0.as_str(),
        item_id,
        until_minutes,
    );
    Ok(())
}

#[cfg(test)]
mod tests;

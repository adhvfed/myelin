//! Unit + chained-durability tests for snooze re-surfacing on the SHARED durable wheel (NOTIF-P18).
//!
//! The mandatory-core mutation surface (≥ 80% floor): [`snooze_timer_key`] (the namespacing — no
//! collision with escalation), [`SnoozeResurfacer::resurface_due`] (effectively-once flip
//! `snoozed → unread`, never on a replay, never on a non-snoozed row), [`SnoozeResurfacer::arm`] /
//! [`SnoozeResurfacer::cancel`] (the manual-un-snooze disarm). Every transition is asserted; the
//! chained durability test kills the worker before the `until` and asserts EXACTLY ONE re-surface.

use super::*;
use crate::escalation::{DurableWheel, InMemoryWheel};
use crate::read_state::{active_inbox, mark, ReadState};
use crate::router::{InboxProjection, RoutedInboxItem};
use crate::{Class, Reason};
use myelin_events::ArtifactRef;
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn principal(id: &str) -> Principal {
    Principal::stub(PrincipalId(id.into()), PrincipalKind::Human, tenant())
}

fn item(recipient: &str, item_id: &str, subject: &str) -> RoutedInboxItem {
    RoutedInboxItem {
        tenant: tenant(),
        region: Region("fr-par".into()),
        item_id: item_id.into(),
        recipient: recipient.into(),
        subject: ArtifactRef(subject.into()),
        reason: Reason::Assigned,
        class: Class::Direct,
        origin_event: ArtifactRef(format!("myelin://acme/bus/event/{item_id}")),
        dedup_key: item_id.into(),
        coalesce_count: 1,
        state: "unread".into(),
        snooze_until: None,
    }
}

fn seeded(me: &str) -> InboxProjection {
    let inbox = InboxProjection::new();
    inbox.upsert_for_test(item(me, "iss-1", "myelin://acme/issue/issue/PROJ-1"));
    inbox.upsert_for_test(item(me, "iss-2", "myelin://acme/issue/issue/PROJ-2"));
    inbox
}

fn state_of(inbox: &InboxProjection, recipient: &str, item_id: &str) -> Option<String> {
    inbox
        .snapshot_for_tenant(&tenant())
        .into_iter()
        .find(|r| r.recipient == recipient && r.item_id == item_id)
        .map(|r| r.state)
}

// --- the timer-key namespacing (one substrate, no collision with escalation) ---

/// **A snooze timer key is namespaced under `snooze:` and is deterministic from the row identity.**
/// So a snooze timer never aliases an escalation run on the SHARED wheel (one substrate, distinct
/// keys). A mutant that drops the namespace prefix or mis-builds the key is caught.
#[test]
fn snooze_timer_key_is_namespaced_and_deterministic() {
    let k = snooze_timer_key(&tenant(), "u1", "iss-1");
    assert!(
        k.starts_with(SNOOZE_TIMER_NS),
        "the key is namespaced under `snooze:`"
    );
    assert_eq!(
        k, "snooze:acme:u1:iss-1",
        "deterministic from (tenant, recipient, item_id)"
    );
    // Distinct rows → distinct keys (no collision on the shared wheel).
    assert_ne!(
        snooze_timer_key(&tenant(), "u1", "iss-1"),
        snooze_timer_key(&tenant(), "u1", "iss-2")
    );
    assert_ne!(
        snooze_timer_key(&tenant(), "u1", "iss-1"),
        snooze_timer_key(&tenant(), "u2", "iss-1")
    );
    // It does NOT collide with a raw escalation run_id of the same shape (escalation keys are raw).
    assert_ne!(
        snooze_timer_key(&tenant(), "u1", "iss-1"),
        "iss-1".to_string()
    );
}

// --- arm: the durable re-surface timer is armed on the SHARED wheel (no in-process sleep) ---

/// **`arm` schedules a durable re-surface timer on the SHARED wheel.** After arming, the wheel holds
/// a live handle for the snoozed item (the persisted handle a restart resumes from) — a durable
/// timer, not an in-process sleep.
#[test]
fn arm_schedules_a_durable_timer_on_the_shared_wheel() {
    let wheel = InMemoryWheel::new();
    let r = SnoozeResurfacer::new(wheel);
    assert!(
        !r.has_timer(&tenant(), "u1", "iss-1"),
        "no timer before arming"
    );
    r.arm(&tenant(), "u1", "iss-1", 30);
    assert!(
        r.has_timer(&tenant(), "u1", "iss-1"),
        "the durable re-surface timer is armed"
    );
    // A DIFFERENT item has no timer (arming is per-item).
    assert!(!r.has_timer(&tenant(), "u1", "iss-2"));
}

/// **A re-snooze REPLACES the prior timer handle (the wheel's guarded UPDATE) — no wheel pollution.**
/// Arm twice for the same item; there is still exactly one live handle (re-arm replaced, not stacked).
#[test]
fn re_arm_replaces_the_prior_handle() {
    let wheel = InMemoryWheel::new();
    let r = SnoozeResurfacer::new(wheel);
    r.arm(&tenant(), "u1", "iss-1", 30);
    r.arm(&tenant(), "u1", "iss-1", 60); // re-snooze → replace.
    assert!(
        r.has_timer(&tenant(), "u1", "iss-1"),
        "still exactly one live handle"
    );
    // Firing it once consumes the single handle; a second fire is a no-op.
    let key = snooze_timer_key(&tenant(), "u1", "iss-1");
    assert!(r.wheel().fire_due(&key), "the one handle fires once");
    assert!(
        !r.wheel().fire_due(&key),
        "no second handle (re-arm replaced, did not stack)"
    );
}

// --- resurface_due: effectively-once flip snoozed → unread ---

/// **`resurface_due` flips a snoozed item `snoozed → unread` and clears `snooze_until` — EXACTLY
/// ONCE.** The first fire re-surfaces (Resurfaced); a replayed fire (a restart over the consumed
/// handle) re-surfaces NOTHING (NoOp) — 0 duplicate re-surface. The item is back in the active inbox.
#[test]
fn resurface_due_flips_snoozed_to_unread_exactly_once() {
    let inbox = seeded("u1");
    let p = principal("u1");
    let wheel = InMemoryWheel::new();
    let r = SnoozeResurfacer::new(wheel);

    // snooze + arm the durable timer.
    snooze_and_arm(&inbox, &r, &p, "iss-1", "2026-06-25T09:00:00Z", 30).unwrap();
    assert_eq!(
        state_of(&inbox, "u1", "iss-1").unwrap(),
        "snoozed",
        "parked after snooze"
    );

    // suppressed from the active inbox while snoozed.
    let active = active_inbox(inbox.snapshot_for_tenant(&tenant()));
    assert!(
        !active.iter().any(|x| x.item_id == "iss-1"),
        "snoozed item absent from active inbox"
    );

    // the timer fires at the until → re-surface EXACTLY ONCE.
    assert_eq!(
        r.resurface_due(&inbox, &tenant(), "u1", "iss-1"),
        ResurfaceOutcome::Resurfaced,
        "the first fire re-surfaces the item"
    );
    let row = inbox
        .snapshot_for_tenant(&tenant())
        .into_iter()
        .find(|x| x.item_id == "iss-1")
        .unwrap();
    assert_eq!(row.state, "unread", "snoozed → unread");
    assert!(
        row.snooze_until.is_none(),
        "the snooze_until is cleared on re-surface"
    );

    // back in the active inbox.
    let active = active_inbox(inbox.snapshot_for_tenant(&tenant()));
    assert!(
        active.iter().any(|x| x.item_id == "iss-1"),
        "the re-surfaced item is back in the active inbox"
    );

    // 0 DUPLICATE: a replayed fire (the effectively-once handle is consumed) re-surfaces NOTHING.
    assert_eq!(
        r.resurface_due(&inbox, &tenant(), "u1", "iss-1"),
        ResurfaceOutcome::NoOp,
        "a replayed timer fire is a no-op — 0 duplicate re-surface"
    );
}

/// **`resurface_due` does NOT re-surface a row the user manually un-snoozed first.** If the user
/// marks the snoozed item read before the `until`, the timer fire (if it still fires) must leave the
/// row alone — no resurrection of an already-actioned item.
#[test]
fn resurface_due_leaves_a_manually_unsnoozed_row_alone() {
    let inbox = seeded("u1");
    let p = principal("u1");
    let wheel = InMemoryWheel::new();
    let r = SnoozeResurfacer::new(wheel);

    snooze_and_arm(&inbox, &r, &p, "iss-1", "2026-06-25T09:00:00Z", 30).unwrap();
    // the user manually marks it read before the until — AND the integrated path cancels the timer.
    mark(&inbox, &p, "iss-1", ReadState::Read).unwrap();
    r.cancel(&tenant(), "u1", "iss-1");

    // even if a stale fire is attempted, it must not flip a non-snoozed row.
    assert_eq!(
        r.resurface_due(&inbox, &tenant(), "u1", "iss-1"),
        ResurfaceOutcome::NoOp,
        "a cancelled timer never fires (and a non-snoozed row is never flipped)"
    );
    assert_eq!(
        state_of(&inbox, "u1", "iss-1").unwrap(),
        "read",
        "the user's read state is preserved"
    );
}

/// **The guard inside `resurface_due` (state == snoozed) protects a non-snoozed row even if the
/// timer were NOT cancelled.** Snooze, arm, then mark read WITHOUT cancelling the timer (simulate a
/// race); the timer fire consumes the handle but the state guard leaves the read row alone.
#[test]
fn resurface_due_state_guard_protects_a_read_row_without_cancel() {
    let inbox = seeded("u1");
    let p = principal("u1");
    let wheel = InMemoryWheel::new();
    let r = SnoozeResurfacer::new(wheel);

    snooze_and_arm(&inbox, &r, &p, "iss-1", "2026-06-25T09:00:00Z", 30).unwrap();
    mark(&inbox, &p, "iss-1", ReadState::Read).unwrap(); // marked read, timer NOT cancelled.

    // the timer fires (handle live) but the state guard prevents resurrecting the read row.
    assert_eq!(
        r.resurface_due(&inbox, &tenant(), "u1", "iss-1"),
        ResurfaceOutcome::NoOp,
        "the state guard leaves a no-longer-snoozed row alone (no resurrection)"
    );
    assert_eq!(
        state_of(&inbox, "u1", "iss-1").unwrap(),
        "read",
        "the read state is preserved"
    );
}

/// **`resurface_due` is recipient-scoped: it never re-surfaces another principal's item.** u2's fire
/// against u1's item re-surfaces nothing (the wheel key + the row match are both per-recipient).
#[test]
fn resurface_due_is_recipient_scoped() {
    let inbox = InboxProjection::new();
    inbox.upsert_for_test(item("u1", "iss-1", "myelin://acme/issue/issue/P1"));
    let p1 = principal("u1");
    let wheel = InMemoryWheel::new();
    let r = SnoozeResurfacer::new(wheel);
    snooze_and_arm(&inbox, &r, &p1, "iss-1", "2026-06-25T09:00:00Z", 30).unwrap();

    // u2 has no timer for u1's item (the key is per-recipient) → no-op, u1's row untouched.
    assert_eq!(
        r.resurface_due(&inbox, &tenant(), "u2", "iss-1"),
        ResurfaceOutcome::NoOp
    );
    assert_eq!(
        state_of(&inbox, "u1", "iss-1").unwrap(),
        "snoozed",
        "u1's snooze is untouched by u2"
    );
}

// --- cancel: a manual un-snooze disarms the timer ---

/// **`cancel` disarms a pending re-surface timer — the disarmed timer never fires.** After cancel
/// the wheel holds no live handle, and a fire attempt is a no-op (the manual un-snooze wins).
#[test]
fn cancel_disarms_the_resurface_timer() {
    let inbox = seeded("u1");
    let p = principal("u1");
    let wheel = InMemoryWheel::new();
    let r = SnoozeResurfacer::new(wheel);
    snooze_and_arm(&inbox, &r, &p, "iss-1", "2026-06-25T09:00:00Z", 30).unwrap();
    assert!(r.has_timer(&tenant(), "u1", "iss-1"), "armed");

    r.cancel(&tenant(), "u1", "iss-1");
    assert!(
        !r.has_timer(&tenant(), "u1", "iss-1"),
        "cancel disarmed the timer"
    );
    // cancel is idempotent (cancelling again is a no-op).
    r.cancel(&tenant(), "u1", "iss-1");
    assert_eq!(
        r.resurface_due(&inbox, &tenant(), "u1", "iss-1"),
        ResurfaceOutcome::NoOp,
        "a cancelled timer fires nothing"
    );
}

// --- snooze_and_arm: a not-for-me item snoozes nothing and arms no orphan timer ---

/// **`snooze_and_arm` of a not-for-me / missing item snoozes NOTHING and arms NO orphan timer.** u2
/// cannot snooze u1's item (NotFound); no timer is armed for it (the error surfaces before arm).
#[test]
fn snooze_and_arm_not_for_me_arms_no_orphan_timer() {
    let inbox = seeded("u1");
    let wheel = InMemoryWheel::new();
    let r = SnoozeResurfacer::new(wheel);
    let res = snooze_and_arm(
        &inbox,
        &r,
        &principal("u2"),
        "iss-1",
        "2026-06-25T09:00:00Z",
        30,
    );
    assert!(res.is_err(), "u2 cannot snooze u1's item");
    assert!(
        !r.has_timer(&tenant(), "u2", "iss-1"),
        "no orphan timer armed for the refused snooze"
    );
    assert_eq!(
        state_of(&inbox, "u1", "iss-1").unwrap(),
        "unread",
        "u1's row untouched"
    );
}

// --- THE CHAINED DURABILITY TEST (EI-01 §4): kill before the until → exactly one re-surface ---

/// **THE CHAINED DURABILITY PROPERTY (EI-01 §4 / the GATE): snooze → kill the worker before the
/// `until` → resume → assert EXACTLY ONE re-surface at the `until` (not zero, not two).** The durable
/// substrate (the wheel + the inbox row) survives the kill; a NEW resurfacer resumes over the SAME
/// wheel; the re-surface timer fires effectively-once. 0 missed re-surface, 0 duplicate re-surface.
#[test]
fn chained_snooze_kill_before_until_resurfaces_exactly_once() {
    // The DURABLE substrate that survives a Notif restart: the wheel + the inbox projection.
    let inbox = seeded("u1");
    let p = principal("u1");
    let wheel = InMemoryWheel::new();

    // === snooze on worker A: record the until + arm the durable re-surface timer ===
    let worker_a = SnoozeResurfacer::new(wheel.clone());
    snooze_and_arm(&inbox, &worker_a, &p, "iss-1", "2026-06-25T09:00:00Z", 30).unwrap();
    assert!(
        worker_a.has_timer(&tenant(), "u1", "iss-1"),
        "the durable re-surface timer is armed"
    );
    assert_eq!(
        state_of(&inbox, "u1", "iss-1").unwrap(),
        "snoozed",
        "parked"
    );

    // === THE KILL: drop worker A BEFORE the until. The wheel + the inbox row survive. ===
    drop(worker_a);

    // === resume on worker B over the SAME durable wheel (re-hydrate from the persisted handle) ===
    let worker_b = SnoozeResurfacer::new(wheel.clone());
    // 0 MISSED: the durable timer survived the kill — the handle is still live on the shared wheel.
    assert!(
        worker_b.has_timer(&tenant(), "u1", "iss-1"),
        "the timer SURVIVED the kill (0 missed)"
    );

    // === the re-surface timer fires at the until → re-surface EXACTLY ONCE ===
    assert_eq!(
        worker_b.resurface_due(&inbox, &tenant(), "u1", "iss-1"),
        ResurfaceOutcome::Resurfaced,
        "the resumed worker re-surfaces the item (NOT zero — the durable handle resumed)"
    );
    assert_eq!(
        state_of(&inbox, "u1", "iss-1").unwrap(),
        "unread",
        "the item re-surfaced (snoozed → unread)"
    );

    // 0 DUPLICATE: a second restart over the already-fired timer re-surfaces NOTHING.
    let worker_c = SnoozeResurfacer::new(wheel.clone());
    assert_eq!(
        worker_c.resurface_due(&inbox, &tenant(), "u1", "iss-1"),
        ResurfaceOutcome::NoOp,
        "a replayed fire after the re-surface is a no-op (0 duplicate)"
    );
    assert_eq!(
        state_of(&inbox, "u1", "iss-1").unwrap(),
        "unread",
        "still exactly one re-surface"
    );

    // GREEN: 0 missed re-surface, 0 duplicate re-surface across the kill. No threshold weakened.
    let _ = &wheel;
}

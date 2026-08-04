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
    assert_ne!(
        snooze_timer_key(&tenant(), "u1", "iss-1"),
        snooze_timer_key(&tenant(), "u1", "iss-2")
    );
    assert_ne!(
        snooze_timer_key(&tenant(), "u1", "iss-1"),
        snooze_timer_key(&tenant(), "u2", "iss-1")
    );
    assert_ne!(
        snooze_timer_key(&tenant(), "u1", "iss-1"),
        "iss-1".to_string()
    );
}

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
    assert!(!r.has_timer(&tenant(), "u1", "iss-2"));
}

#[test]
fn re_arm_replaces_the_prior_handle() {
    let wheel = InMemoryWheel::new();
    let r = SnoozeResurfacer::new(wheel);
    r.arm(&tenant(), "u1", "iss-1", 30);
    r.arm(&tenant(), "u1", "iss-1", 60);
    assert!(
        r.has_timer(&tenant(), "u1", "iss-1"),
        "still exactly one live handle"
    );
    let key = snooze_timer_key(&tenant(), "u1", "iss-1");
    assert!(r.wheel().fire_due(&key), "the one handle fires once");
    assert!(
        !r.wheel().fire_due(&key),
        "no second handle (re-arm replaced, did not stack)"
    );
}

#[test]
fn resurface_due_flips_snoozed_to_unread_exactly_once() {
    let inbox = seeded("u1");
    let p = principal("u1");
    let wheel = InMemoryWheel::new();
    let r = SnoozeResurfacer::new(wheel);

    snooze_and_arm(&inbox, &r, &p, "iss-1", "2026-06-25T09:00:00Z", 30).unwrap();
    assert_eq!(
        state_of(&inbox, "u1", "iss-1").unwrap(),
        "snoozed",
        "parked after snooze"
    );

    let active = active_inbox(inbox.snapshot_for_tenant(&tenant()));
    assert!(
        !active.iter().any(|x| x.item_id == "iss-1"),
        "snoozed item absent from active inbox"
    );

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

    let active = active_inbox(inbox.snapshot_for_tenant(&tenant()));
    assert!(
        active.iter().any(|x| x.item_id == "iss-1"),
        "the re-surfaced item is back in the active inbox"
    );

    assert_eq!(
        r.resurface_due(&inbox, &tenant(), "u1", "iss-1"),
        ResurfaceOutcome::NoOp,
        "a replayed timer fire is a no-op - 0 duplicate re-surface"
    );
}

#[test]
fn resurface_due_leaves_a_manually_unsnoozed_row_alone() {
    let inbox = seeded("u1");
    let p = principal("u1");
    let wheel = InMemoryWheel::new();
    let r = SnoozeResurfacer::new(wheel);

    snooze_and_arm(&inbox, &r, &p, "iss-1", "2026-06-25T09:00:00Z", 30).unwrap();
    mark(&inbox, &p, "iss-1", ReadState::Read).unwrap();
    r.cancel(&tenant(), "u1", "iss-1");

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

#[test]
fn resurface_due_state_guard_protects_a_read_row_without_cancel() {
    let inbox = seeded("u1");
    let p = principal("u1");
    let wheel = InMemoryWheel::new();
    let r = SnoozeResurfacer::new(wheel);

    snooze_and_arm(&inbox, &r, &p, "iss-1", "2026-06-25T09:00:00Z", 30).unwrap();
    mark(&inbox, &p, "iss-1", ReadState::Read).unwrap();

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

#[test]
fn resurface_due_is_recipient_scoped() {
    let inbox = InboxProjection::new();
    inbox.upsert_for_test(item("u1", "iss-1", "myelin://acme/issue/issue/P1"));
    let p1 = principal("u1");
    let wheel = InMemoryWheel::new();
    let r = SnoozeResurfacer::new(wheel);
    snooze_and_arm(&inbox, &r, &p1, "iss-1", "2026-06-25T09:00:00Z", 30).unwrap();

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
    r.cancel(&tenant(), "u1", "iss-1");
    assert_eq!(
        r.resurface_due(&inbox, &tenant(), "u1", "iss-1"),
        ResurfaceOutcome::NoOp,
        "a cancelled timer fires nothing"
    );
}

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

#[test]
fn chained_snooze_kill_before_until_resurfaces_exactly_once() {
    let inbox = seeded("u1");
    let p = principal("u1");
    let wheel = InMemoryWheel::new();

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

    drop(worker_a);

    let worker_b = SnoozeResurfacer::new(wheel.clone());
    assert!(
        worker_b.has_timer(&tenant(), "u1", "iss-1"),
        "the timer SURVIVED the kill (0 missed)"
    );

    assert_eq!(
        worker_b.resurface_due(&inbox, &tenant(), "u1", "iss-1"),
        ResurfaceOutcome::Resurfaced,
        "the resumed worker re-surfaces the item (NOT zero - the durable handle resumed)"
    );
    assert_eq!(
        state_of(&inbox, "u1", "iss-1").unwrap(),
        "unread",
        "the item re-surfaced (snoozed → unread)"
    );

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

    let _ = &wheel;
}

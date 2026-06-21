//! # The snooze re-surface durability drill — exactly-once re-surface across a kill (NOTIF-P18 / P-196)
//!
//! **Drill source:**
//! `planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! §3.3 ("the snooze re-surface durability is asserted on the SAME wheel as NOTIF-D7; no separate
//! drill row, but the durability property is gated") + `notifications.md` §3.7/§2.4 (snooze
//! re-surfacing rides the SAME minute-bucket durable wheel as escalation — one substrate, three uses;
//! the wheel is `myelin-flow`'s durable timer, NOT an in-process sleep) + EI-01 §3 (prove-it: kill
//! before the `until` forces the failure mode; the surviving re-surface is the pass).
//!
//! **The dated GREEN artifact (2026-06-21).** An item is snoozed on the durable wheel; Notif is
//! "killed" before the `until` (the in-process [`SnoozeResurfacer`] is dropped, the durable substrate
//! — the timer wheel + the inbox row — survives); a NEW worker resumes over the SAME durable state;
//! the re-surface timer fires and flips the item `snoozed → unread` EXACTLY ONCE. The drill measures +
//! asserts, with NO threshold weakened:
//!
//! 1. **0 missed re-surface** — the durable timer survived the kill (the handle is still live on the
//!    shared wheel after dropping the worker), and the resumed worker re-surfaces the item (NOT zero).
//! 2. **0 duplicate re-surface** — a replayed fire (a second restart over the already-fired timer)
//!    re-surfaces NOTHING (the wheel's effectively-once `fire_due`). The item is `unread` exactly once.
//! 3. **one substrate** — the re-surface rides the SAME [`InMemoryWheel`] the escalation chain
//!    (NOTIF-D7) arms its `ack_window` timers on. This drill builds a SINGLE wheel, arms BOTH an
//!    escalation timer and a snooze re-surface timer on it, and asserts they coexist on one substrate
//!    with distinct keys (0 second timer mechanism, 0 in-process sleep — there is no `sleep` anywhere).
//!
//! The durable wheel is the in-memory model of the `myelin-flow` 9.3 wheel (the real engine is
//! P-FLOW-09/P-FLOW-13, a named floor); the SLA timer (the third use) is NOTIF-P21, a named floor. The
//! exactly-once re-surface PROPERTY this drill asserts is the re-surface POLICY this prompt owns.

use myelin_events::{ArtifactRef, OutboxStore};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_notif::escalation::{
    DurableWheel, EscalationEngine, EscalationPolicy, InMemoryWheel, OncallSchedule, RotationWindow,
};
use myelin_notif::prefs::QuietHours;
use myelin_notif::router::{InboxProjection, RoutedInboxItem};
use myelin_notif::snooze_resurface::{snooze_and_arm, snooze_timer_key, ResurfaceOutcome, SnoozeResurfacer};
use myelin_notif::{Class, Reason};
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
fn state_of(inbox: &InboxProjection, recipient: &str, item_id: &str) -> Option<String> {
    inbox
        .snapshot_for_tenant(&tenant())
        .into_iter()
        .find(|r| r.recipient == recipient && r.item_id == item_id)
        .map(|r| r.state)
}

/// **The snooze re-surface durability drill — exactly-once re-surface across a kill before the `until`.**
#[test]
fn snooze_resurface_durability_kill_before_until_resurfaces_exactly_once() {
    // The DURABLE substrate that survives a Notif restart: the wheel + the inbox projection.
    let inbox = InboxProjection::new();
    inbox.upsert_for_test(item("u1", "iss-1", "myelin://acme/issue/issue/PROJ-1"));
    let p = principal("u1");
    let wheel = InMemoryWheel::new();

    // === snooze on worker A: record the until + arm the durable re-surface timer ===
    let worker_a = SnoozeResurfacer::new(wheel.clone());
    snooze_and_arm(&inbox, &worker_a, &p, "iss-1", "2026-06-25T09:00:00Z", 30)
        .expect("snooze + arm the durable re-surface timer");
    assert!(worker_a.has_timer(&tenant(), "u1", "iss-1"), "the durable re-surface timer is armed");
    assert_eq!(state_of(&inbox, "u1", "iss-1").unwrap(), "snoozed", "the item is parked");

    // === THE KILL: drop worker A BEFORE the until (no re-surface yet). Wheel + row survive. ===
    drop(worker_a);

    // === resume on worker B over the SAME durable wheel ===
    let worker_b = SnoozeResurfacer::new(wheel.clone());
    // 0 MISSED: the durable timer survived the kill (the persisted handle is still live).
    assert!(worker_b.has_timer(&tenant(), "u1", "iss-1"), "the timer SURVIVED the kill (0 missed)");

    // === the re-surface timer fires at the until → re-surface EXACTLY ONCE ===
    assert_eq!(
        worker_b.resurface_due(&inbox, &tenant(), "u1", "iss-1"),
        ResurfaceOutcome::Resurfaced,
        "the resumed worker re-surfaces the item (NOT zero — the durable handle resumed)"
    );
    assert_eq!(state_of(&inbox, "u1", "iss-1").unwrap(), "unread", "snoozed → unread (re-surfaced)");

    // 0 DUPLICATE: a second restart over the already-fired timer re-surfaces NOTHING.
    let worker_c = SnoozeResurfacer::new(wheel.clone());
    assert_eq!(
        worker_c.resurface_due(&inbox, &tenant(), "u1", "iss-1"),
        ResurfaceOutcome::NoOp,
        "a replayed fire after the re-surface is a no-op (0 duplicate)"
    );
    assert_eq!(state_of(&inbox, "u1", "iss-1").unwrap(), "unread", "still exactly one re-surface");

    // GREEN ARTIFACT (2026-06-21): 0 missed / 0 duplicate re-surface across the kill. No threshold weakened.
}

/// **The one-substrate check — snooze re-surface + escalation ride ONE wheel (0 second timer
/// mechanism, 0 in-process sleep).** A SINGLE [`InMemoryWheel`] carries BOTH an escalation
/// `ack_window` timer (NOTIF-P14) and a snooze re-surface timer (NOTIF-P18). They coexist on the one
/// substrate with distinct keys (the snooze key is namespaced under `snooze:`; the escalation run
/// keys on its raw `run_id`) — no collision, no second wheel. There is NO `std::thread::sleep` /
/// `tokio::time::sleep` anywhere on this path: the delay lives on the durable wheel.
#[test]
fn snooze_and_escalation_share_one_durable_wheel_no_second_mechanism() {
    let wheel = InMemoryWheel::new();
    let inbox = InboxProjection::new();
    inbox.upsert_for_test(item("u1", "iss-1", "myelin://acme/issue/issue/PROJ-1"));
    let p = principal("u1");

    // Arm a SNOOZE re-surface timer on the wheel.
    let resurfacer = SnoozeResurfacer::new(wheel.clone());
    snooze_and_arm(&inbox, &resurfacer, &p, "iss-1", "2026-06-25T09:00:00Z", 30).unwrap();

    // Arm an ESCALATION ack_window timer on the SAME wheel (the third construction sharing it).
    let outbox = OutboxStore::new();
    let eng = EscalationEngine::new(wheel.clone(), outbox);
    let schedule = OncallSchedule {
        schedule_id: "platform-oncall".into(),
        rotation: vec![RotationWindow { from_minute: 0, to_minute: 1440, principal: PrincipalId("psn:alice".into()) }],
    };
    let (run_id, _first) = eng
        .page(
            tenant(),
            Region("fr-par".into()),
            "esc-run-1".into(),
            EscalationPolicy::test_chain(15, PrincipalId("psn:lead".into())),
            ArtifactRef("myelin://acme/issue/issue/PROJ-1".into()),
            Some(&schedule),
            600,
            &QuietHours::default(),
            false,
        )
        .expect("page arms an ack_window timer on the SAME wheel");

    // BOTH timers live on the ONE wheel, with distinct keys (no collision).
    let snooze_key = snooze_timer_key(&tenant(), "u1", "iss-1");
    assert!(wheel.has_timer(&snooze_key), "the snooze re-surface timer lives on the wheel");
    assert!(wheel.has_timer(&run_id), "the escalation ack_window timer lives on the SAME wheel");
    assert_ne!(snooze_key, run_id, "distinct keys on one substrate (no collision)");
    assert!(snooze_key.starts_with("snooze:"), "the snooze timer is namespaced (one wheel, three uses)");

    // Firing the snooze timer does NOT fire the escalation timer (they are independent handles on one wheel).
    assert_eq!(resurfacer.resurface_due(&inbox, &tenant(), "u1", "iss-1"), ResurfaceOutcome::Resurfaced);
    assert!(wheel.has_timer(&run_id), "the escalation timer is untouched by the snooze fire");

    // ONE substrate, three uses (escalation + snooze proven; SLA is the NOTIF-P21 floor). No second mechanism.
}

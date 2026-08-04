use myelin_events::{ArtifactRef, OutboxStore};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_notif::escalation::{
    DurableWheel, EscalationEngine, EscalationPolicy, InMemoryWheel, OncallSchedule, RotationWindow,
};
use myelin_notif::prefs::QuietHours;
use myelin_notif::router::{InboxProjection, RoutedInboxItem};
use myelin_notif::snooze_resurface::{
    snooze_and_arm, snooze_timer_key, ResurfaceOutcome, SnoozeResurfacer,
};
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

#[test]
fn snooze_resurface_durability_kill_before_until_resurfaces_exactly_once() {
    let inbox = InboxProjection::new();
    inbox.upsert_for_test(item("u1", "iss-1", "myelin://acme/issue/issue/PROJ-1"));
    let p = principal("u1");
    let wheel = InMemoryWheel::new();

    let worker_a = SnoozeResurfacer::new(wheel.clone());
    snooze_and_arm(&inbox, &worker_a, &p, "iss-1", "2026-06-25T09:00:00Z", 30)
        .expect("snooze + arm the durable re-surface timer");
    assert!(
        worker_a.has_timer(&tenant(), "u1", "iss-1"),
        "the durable re-surface timer is armed"
    );
    assert_eq!(
        state_of(&inbox, "u1", "iss-1").unwrap(),
        "snoozed",
        "the item is parked"
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
        "snoozed → unread (re-surfaced)"
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

}

#[test]
fn snooze_and_escalation_share_one_durable_wheel_no_second_mechanism() {
    let wheel = InMemoryWheel::new();
    let inbox = InboxProjection::new();
    inbox.upsert_for_test(item("u1", "iss-1", "myelin://acme/issue/issue/PROJ-1"));
    let p = principal("u1");

    let resurfacer = SnoozeResurfacer::new(wheel.clone());
    snooze_and_arm(&inbox, &resurfacer, &p, "iss-1", "2026-06-25T09:00:00Z", 30).unwrap();

    let outbox = OutboxStore::new();
    let eng = EscalationEngine::new(wheel.clone(), outbox);
    let schedule = OncallSchedule {
        schedule_id: "platform-oncall".into(),
        rotation: vec![RotationWindow {
            from_minute: 0,
            to_minute: 1440,
            principal: PrincipalId("psn:alice".into()),
        }],
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

    let snooze_key = snooze_timer_key(&tenant(), "u1", "iss-1");
    assert!(
        wheel.has_timer(&snooze_key),
        "the snooze re-surface timer lives on the wheel"
    );
    assert!(
        wheel.has_timer(&run_id),
        "the escalation ack_window timer lives on the SAME wheel"
    );
    assert_ne!(
        snooze_key, run_id,
        "distinct keys on one substrate (no collision)"
    );
    assert!(
        snooze_key.starts_with("snooze:"),
        "the snooze timer is namespaced (one wheel, three uses)"
    );

    assert_eq!(
        resurfacer.resurface_due(&inbox, &tenant(), "u1", "iss-1"),
        ResurfaceOutcome::Resurfaced
    );
    assert!(
        wheel.has_timer(&run_id),
        "the escalation timer is untouched by the snooze fire"
    );

}

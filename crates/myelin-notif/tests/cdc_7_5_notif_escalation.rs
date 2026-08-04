use myelin_events::{OutboxStore, Timestamp};
use myelin_identity::PrincipalId;
use myelin_notif::escalation::{
    oncall_now, DurableWheel, EscalationEngine, EscalationPolicy, EscalationStep, EscalationTarget,
    InMemoryWheel, OncallSchedule, RotationWindow, RunState, ESCALATION_REASON,
};
use myelin_notif::prefs::{Channel, QuietHours};
use myelin_notif::{Reason, NOTIF_ESCALATION_ACKED};
use myelin_refs::ArtifactRef;
use myelin_tenancy::{Region, TenantId};

fn pid(s: &str) -> PrincipalId {
    PrincipalId(s.into())
}

fn schedule() -> OncallSchedule {
    OncallSchedule {
        schedule_id: "platform-oncall".into(),
        rotation: vec![RotationWindow {
            from_minute: 0,
            to_minute: 1440,
            principal: pid("psn:alice"),
        }],
    }
}

#[test]
fn provider_page_walks_the_frozen_chain() {
    let outbox = OutboxStore::new();
    let eng = EscalationEngine::new(InMemoryWheel::new(), outbox.clone());
    let policy = EscalationPolicy {
        policy_id: "sla-chain".into(),
        steps: vec![
            EscalationStep {
                target: EscalationTarget::Schedule("platform-oncall".into()),
                channels: vec![Channel::InApp, Channel::WebPush],
                ack_window_minutes: 10,
            },
            EscalationStep {
                target: EscalationTarget::Principal(pid("psn:lead")),
                channels: vec![Channel::InApp],
                ack_window_minutes: 10,
            },
        ],
        repeat: 1,
    };
    let never_quiet = QuietHours::default();
    let (run_id, first) = eng
        .page(
            TenantId("acme".into()),
            Region("fr-par".into()),
            "run-cdc".into(),
            policy,
            ArtifactRef("myelin://acme/issues/issue/9".into()),
            Some(&schedule()),
            600,
            &never_quiet,
            false,
        )
        .expect("page starts");
    assert_eq!(first.principal, pid("psn:alice"));
    assert_eq!(first.channels, vec![Channel::InApp, Channel::WebPush]);
    assert!(
        eng.wheel().has_timer(&run_id),
        "the ack_window durable timer is armed"
    );

    let next = eng
        .advance(&run_id, Some(&schedule()), 600, &never_quiet, false)
        .unwrap()
        .expect("the timer fire pages the next step");
    assert_eq!(next.principal, pid("psn:lead"));
    assert_eq!(next.walk, 1);
}

#[test]
fn provider_oncall_now_resolves_the_rotation() {
    let s = schedule();
    assert_eq!(
        oncall_now(&s, 600),
        Some(pid("psn:alice")),
        "the on-call at fire time"
    );
    let gap = OncallSchedule {
        schedule_id: "g".into(),
        rotation: vec![RotationWindow {
            from_minute: 0,
            to_minute: 60,
            principal: pid("psn:x"),
        }],
    };
    assert_eq!(oncall_now(&gap, 600), None);
}

#[test]
fn consumer_ack_emits_the_frozen_token_via_the_outbox() {
    let outbox = OutboxStore::new();
    let eng = EscalationEngine::new(InMemoryWheel::new(), outbox.clone());
    let policy = EscalationPolicy::test_chain(10, pid("psn:lead"));
    let never_quiet = QuietHours::default();
    let (run_id, _) = eng
        .page(
            TenantId("acme".into()),
            Region("fr-par".into()),
            "run-ack".into(),
            policy,
            ArtifactRef("myelin://acme/issues/issue/9".into()),
            Some(&schedule()),
            600,
            &never_quiet,
            false,
        )
        .unwrap();
    assert!(eng
        .ack(
            &run_id,
            pid("psn:alice"),
            Timestamp("2026-06-20T12:00:00Z".into())
        )
        .unwrap());
    assert_eq!(
        eng.run(&run_id).unwrap().state,
        RunState::Acked,
        "the ack halted the chain"
    );
    assert_eq!(
        outbox.committed_count(),
        1,
        "exactly one notif.escalation.acked committed via the outbox"
    );
    assert_eq!(NOTIF_ESCALATION_ACKED, "notif.escalation.acked");
}

#[test]
fn the_escalation_reason_is_frozen() {
    assert_eq!(ESCALATION_REASON, Reason::Escalated);
}

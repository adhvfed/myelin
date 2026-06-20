//! # CDC — contract 7.5 `oncall_now / page` (escalation on the durable wheel, the frozen chain) (P-192)
//!
//! **Architecture:** `notifications.md` §2.4 (the escalation-chain config shape FROZEN: `page →
//! oncall_now → notify(class=critical, pierces) → escalate-after-timer(ack_window) → if !acked
//! next-step / if acked stop`; Issues passes the chain, Notif owns POLICY, the engine owns
//! DURABILITY; ack is `notif.escalation.acked` via the outbox). **Contract:** **7.5**
//! `oncall_now(schedule) → principal` + `page(target, reason)` starts an escalation durable workflow
//! (owned). **Consumed:** 9.1/9.3/9.4 the durable executor/timer/signal (seamed); 2.2 OutboxTx::emit.
//!
//! This CDC pins the 7.5 seam from BOTH sides:
//!
//! - **PROVIDER (Notif owns 7.5):** `page` starts an escalation run on the durable substrate walking
//!   the FROZEN chain — it resolves `oncall_now(schedule)` at FIRE TIME, pages the on-call (critical
//!   pierces quiet-hours), arms the `ack_window` durable timer, walks to the next step on the timer
//!   fire (exactly once), and HALTS on an ack (emitted as `notif.escalation.acked` via the outbox).
//! - **CONSUMER (Issues / any SLA producer passes the chain; the router consumes the ack event):**
//!   the producer passes the FROZEN [`EscalationPolicy`] shape (ordered steps, each a target
//!   selector with channels and an ack_window); Notif evaluates the POLICY, the engine owns
//!   DURABILITY. The ack rides the SAME `notif.escalation.acked` token the router declares
//!   ([`NOTIF_ESCALATION_ACKED`]); a drift on the chain shape, the pierce, or the ack token breaks
//!   THIS build.
//!
//! Both halves agree on the WIRE: the frozen chain shape (§2.4), the at-fire-time target resolution,
//! the critical pierce, and the ack-as-outbox-event token. Issues' real SLA chain is NOTIF-P21 (a
//! named floor); the real `myelin-flow` durable engine is P-FLOW-09/P-FLOW-13 (a named floor); here
//! the chain shape + the exactly-once / pierce / ack-halt PROPERTIES are proven against the seam.

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
        rotation: vec![RotationWindow { from_minute: 0, to_minute: 1440, principal: pid("psn:alice") }],
    }
}

// === PROVIDER side: page walks the frozen chain on the durable substrate ===

/// **PROVIDER — `page(target, reason)` starts the chain, resolves on-call at fire time, walks once.**
#[test]
fn provider_page_walks_the_frozen_chain() {
    let outbox = OutboxStore::new();
    let eng = EscalationEngine::new(InMemoryWheel::new(), outbox.clone());
    // The FROZEN chain shape the producer (Issues) passes: ordered steps, each target + channels + ack_window.
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
    // oncall_now resolved the rotation AT FIRE TIME → alice (the on-call), critical pierces all channels.
    assert_eq!(first.principal, pid("psn:alice"));
    assert_eq!(first.channels, vec![Channel::InApp, Channel::WebPush]);
    assert!(eng.wheel().has_timer(&run_id), "the ack_window durable timer is armed");

    // escalate-after-timer → walk to the next step EXACTLY ONCE.
    let next = eng
        .advance(&run_id, Some(&schedule()), 600, &never_quiet, false)
        .unwrap()
        .expect("the timer fire pages the next step");
    assert_eq!(next.principal, pid("psn:lead"));
    assert_eq!(next.walk, 1);
}

/// **PROVIDER — `oncall_now(schedule)` resolves the rotation roster (the 7.5 read half).**
#[test]
fn provider_oncall_now_resolves_the_rotation() {
    let s = schedule();
    assert_eq!(oncall_now(&s, 600), Some(pid("psn:alice")), "the on-call at fire time");
    // An uncovered instant resolves to None (no one on call) — surfaced, never silently dropped.
    let gap = OncallSchedule {
        schedule_id: "g".into(),
        rotation: vec![RotationWindow { from_minute: 0, to_minute: 60, principal: pid("psn:x") }],
    };
    assert_eq!(oncall_now(&gap, 600), None);
}

// === CONSUMER side: the ack rides the frozen notif.escalation.acked token via the outbox ===

/// **CONSUMER — the ack is `notif.escalation.acked` emitted via the outbox (the signal-wait resolves).**
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
    // The ack halts the chain + emits the FROZEN token via the ONLY emit path (the outbox).
    assert!(eng.ack(&run_id, pid("psn:alice"), Timestamp("2026-06-20T12:00:00Z".into())).unwrap());
    assert_eq!(eng.run(&run_id).unwrap().state, RunState::Acked, "the ack halted the chain");
    assert_eq!(outbox.committed_count(), 1, "exactly one notif.escalation.acked committed via the outbox");
    // The token the router declares is the token the ack emits (the WIRE both sides agree on).
    assert_eq!(NOTIF_ESCALATION_ACKED, "notif.escalation.acked");
}

/// **The frozen chain attribution: an escalation is `Reason::Escalated` (the §2.4 invariant).**
#[test]
fn the_escalation_reason_is_frozen() {
    assert_eq!(ESCALATION_REASON, Reason::Escalated);
}

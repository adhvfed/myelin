//! # ISS-D7 — the stateful Trigger flagship: exactly-once across a restart + stale-once (ISS-P25 / P-392)
//!
//! **Drill source:**
//! `planning/05-refined-shared-systems-architecture/testing-strategy/01-whole-system-e2e-and-drill-catalogue.md`
//! row **ISS-D7** ("Arm 'remind me when unblocked' (`QueryAst`); resolve last blocker across a restart
//! → fires **exactly once** into the one inbox; after `stale_after`, stale nudge fires once, trigger
//! goes stale." — artifact: **1 fire; stale-once**, CI). Architecture
//! `issue-tracker/architecture/03-events-contracts-and-glue.md` §10 (the stateful Trigger — the
//! armable-condition catalogue, the one inbox for `on_resolve`, the `stale_after` nudge + stale).
//!
//! **The dated GREEN artifact (2026-06-23).** Over `myelin_issues::IssueTriggerEngine` (the Issues-side
//! stateful Trigger over the CONSUMED bus arm/disarm primitive 3.3/3.4 + the CONSUMED `myelin-flow`
//! `stale_after` wheel 9.3 + the ONE Notif inbox 7.1), the drill measures + asserts, with NO threshold
//! weakened:
//!
//! 1. **fire-once across a restart** — arm "remind me when unblocked" over an issue with two open
//!    `blocked_by` blockers; snapshot the durable `trigger` rows; KILL the engine (drop it); a NEW
//!    engine restores from the durable rows; the LAST blocker clears after the restart → **exactly one**
//!    inbox item fires into the one inbox. A re-delivered resolving event (across a SECOND restart) fires
//!    NOTHING (the durable `Resolved` state is the fire-once guard).
//! 2. **stale-once** — a SEPARATE armed promise that never resolves goes stale: after `stale_after` the
//!    `myelin-flow` wheel fires → **exactly one** stale nudge fires into the one inbox and the arming
//!    goes `stale`; a re-fire of the wheel (a second restart over the already-stale arming) fires
//!    NOTHING. No silent forever-armed promise.
//!
//! Threshold: 1 fire + stale-once (0 missed, 0 duplicate), across a restart.

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, SetExpr};
use myelin_issues::{
    default_stale_after, ArmRequest, ArmableCondition, IssueTriggerEngine, TriggerInboxKind,
};
use myelin_notif::router::InboxProjection;
use myelin_query::{RelMembership, TriggerId, TriggerState};
use myelin_tenancy::{Region, TenantId};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn owner() -> PrincipalId {
    PrincipalId("alice".into())
}
fn subject(key: &str) -> ArtifactRef {
    ArtifactRef(format!("myelin://acme/issues/issue/{key}"))
}

fn no_rel(_m: &RelMembership) -> bool {
    false
}

/// An `issue.relation.removed` event carrying the `issue_relation` projection-state count
/// `payload.blocked_by_unresolved` — the trigger reads PROJECTION STATE, not a join (§10).
fn unblock_event(key: &str, event_id: &str, blockers_open: i64) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(event_id.into()),
        type_: EventType("issue.relation.removed".into()),
        schema_ver: 1,
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(
            PrincipalId("svc-bot".into()),
            PrincipalKind::Human,
            tenant(),
        )),
        subject: subject(key),
        aggregate: AggregateKey(format!("issue:{key}")),
        causation_id: None,
        correlation_id: CorrelationId("root".into()),
        caused_by: None,
        depth: 0,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-23T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-23T00:00:00Z".into()),
        payload: serde_json::json!({ "blocked_by_unresolved": blockers_open }),
    }
}

fn arm_unblock(key: &str) -> ArmRequest {
    ArmRequest {
        trigger_id: TriggerId(format!("t-unblock/{key}")),
        condition: ArmableCondition::RemindWhenUnblocked,
        owner: owner(),
        arms_subject: subject(key),
        stale_after: Some(default_stale_after(0)),
    }
}

/// Model one process restart: snapshot the durable trigger rows + their catalogue meta, drop the
/// engine, and restore a fresh engine over the SAME durable Notif inbox.
fn restart(engine: IssueTriggerEngine, inbox: &InboxProjection) -> IssueTriggerEngine {
    let snap = engine.snapshot();
    let meta = engine.meta_for_snapshot();
    drop(engine);
    let mut restored = IssueTriggerEngine::restore(tenant(), region(), inbox.clone(), snap);
    for (tid, cond, ow, subj) in meta {
        restored.restore_meta(tid, cond, ow, subj);
    }
    restored
}

/// **ISS-D7 (1): the flagship fires EXACTLY ONCE when the last blocker clears ACROSS A RESTART.**
#[test]
fn iss_d7_fire_once_across_a_restart() {
    let inbox = InboxProjection::new();
    let id = TriggerId("t-unblock/PROJ-7".into());

    // Arm "remind me when unblocked" over PROJ-7 (two open blockers).
    let mut engine = IssueTriggerEngine::with_inbox(tenant(), region(), inbox.clone());
    engine.arm(arm_unblock("PROJ-7")).unwrap();

    // One blocker clears (still one open) — no fire.
    assert_eq!(
        engine.on_event(&unblock_event("PROJ-7", "rm-1", 1), &SetExpr::All, &no_rel),
        0,
        "a partial resolution does not fire"
    );

    // === RESTART (the engine process is killed; the durable trigger table survives) ===
    let mut engine = restart(engine, &inbox);
    assert_eq!(
        engine.arming(&id).unwrap().state,
        TriggerState::Armed,
        "the promise survives the restart still armed"
    );

    // The LAST blocker clears AFTER the restart → fires exactly once into the one inbox.
    assert_eq!(
        engine.on_event(&unblock_event("PROJ-7", "rm-2", 0), &SetExpr::All, &no_rel),
        1,
        "the last blocker clearing fires exactly once after the restart"
    );
    assert_eq!(
        engine.delivered().len(),
        1,
        "exactly one inbox item delivered"
    );
    assert_eq!(engine.delivered()[0].kind, TriggerInboxKind::Resolved);
    assert_eq!(
        inbox.snapshot_for_tenant(&tenant()).len(),
        1,
        "the ONE Notif inbox carries exactly one row (7.1 — no second store)"
    );

    // === SECOND RESTART + a re-delivered resolving event ===
    let mut engine = restart(engine, &inbox);
    assert_eq!(
        engine.arming(&id).unwrap().state,
        TriggerState::Resolved,
        "the durable state is Resolved (the fire-once guard survives the restart)"
    );
    assert_eq!(
        engine.on_event(
            &unblock_event("PROJ-7", "rm-replay", 0),
            &SetExpr::All,
            &no_rel
        ),
        0,
        "a re-delivered resolving event after a restart does NOT re-fire (0 duplicate)"
    );
    assert_eq!(
        engine.delivered().len(),
        0,
        "the restored engine fires nothing for an already-resolved arming"
    );
    // The one inbox STILL holds exactly one row (0 duplicate across two restarts).
    assert_eq!(
        inbox.snapshot_for_tenant(&tenant()).len(),
        1,
        "0 duplicate — exactly one inbox row after two restarts"
    );
}

/// **ISS-D7 (2): the stale nudge fires EXACTLY ONCE after `stale_after`, then the trigger goes stale
/// (stale-once across a restart).**
#[test]
fn iss_d7_stale_once_across_a_restart() {
    let inbox = InboxProjection::new();
    let id = TriggerId("t-unblock/PROJ-9".into());

    // Arm a promise that will never resolve (no blocker ever clears).
    let mut engine = IssueTriggerEngine::with_inbox(tenant(), region(), inbox.clone());
    engine.arm(arm_unblock("PROJ-9")).unwrap();

    // === RESTART before staleness ===
    let mut engine = restart(engine, &inbox);
    assert_eq!(engine.arming(&id).unwrap().state, TriggerState::Armed);

    // The stale_after deadline elapses → the myelin-flow wheel fires → exactly one stale nudge.
    assert!(
        engine.on_stale_timer(&id),
        "the stale nudge fires once after stale_after"
    );
    assert_eq!(engine.delivered().len(), 1);
    assert_eq!(engine.delivered()[0].kind, TriggerInboxKind::StaleNudge);
    assert_eq!(
        engine.arming(&id).unwrap().state,
        TriggerState::Stale,
        "the trigger goes stale — no silent forever-armed promise"
    );
    assert_eq!(
        inbox.snapshot_for_tenant(&tenant()).len(),
        1,
        "exactly one stale-nudge row in the one inbox"
    );

    // === SECOND RESTART + a re-fire of the wheel over the already-stale arming ===
    let mut engine = restart(engine, &inbox);
    assert_eq!(engine.arming(&id).unwrap().state, TriggerState::Stale);
    assert!(
        !engine.on_stale_timer(&id),
        "a re-fire of the wheel after stale is a no-op (stale-once, 0 duplicate)"
    );
    assert_eq!(
        engine.delivered().len(),
        0,
        "the restored engine fires no second nudge"
    );
    assert_eq!(
        inbox.snapshot_for_tenant(&tenant()).len(),
        1,
        "still exactly one stale-nudge row after two restarts"
    );
}

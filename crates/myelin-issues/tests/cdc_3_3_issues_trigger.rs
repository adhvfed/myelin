//! # The CDC pair for contract 3.3 — the **stateful Trigger** (the Issues slice) (ISS-P25 / P-392)
//!
//! **Contract-index row 3.3** (`arm_trigger/disarm_trigger(Trigger{owner, condition, arms_subject,
//! on_resolve, stale_after})` — stateful per-person promise; `armed → {resolved | stale | disarmed}`;
//! `stale_after` is a `myelin-flow` timer; `condition` is a `QueryAst` over projection state) +
//! row 3.4 (`EventMatcher = QueryAst`). The Bus seam + the fire-once-per-arming engine is OWNED by the
//! event-bus and frozen at EB-20 (`crates/myelin-query/src/triggers.rs`); THIS file pins the **Issues
//! slice** — the stateful Trigger ISS-P25 ships (the armable-condition catalogue + the one inbox +
//! stale-once).
//!
//! - the **PRODUCER** (provider side) is **Issues arming a catalogue condition** through
//!   [`myelin_issues::IssueTriggerEngine`] — each armable condition
//!   ([`myelin_issues::ArmableCondition`]) compiles to a frozen [`myelin_query::EventMatcher`] (= the
//!   `QueryAst` core, 3.4) over `issue.*` events + `issue_relation` projection state, built through the
//!   frozen bus [`myelin_query::arm_trigger`] verb. The producer's promise: it arms exactly the §10
//!   catalogue conditions through the ONE bus primitive (no second trigger engine, no per-subsystem
//!   DSL, EI-01 §7), and a resolve delivers ONE inbox item (7.1) while a `stale_after` elapse fires ONE
//!   stale nudge.
//! - the **CONSUMER** is the **bus [`myelin_query::TriggerEngine`]'s atomic guarded UPDATE** — it
//!   admits the Issues-armed [`myelin_query::Trigger`] and fires it ONCE per arming (`armed → resolved`
//!   only if still `armed`), delegates `stale_after` to the `myelin-flow` 9.3 wheel seam, and honours
//!   the `disarm_trigger` cancel.
//!
//! The two sides are pinned here so a drift on either (Issues changes a catalogue predicate; the bus
//! renames a `Trigger` field or weakens the fire-once guard) fails this test in the same CI job.

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CorrelationId, DataRole, EventEnvelope, EventId, EventType,
    Timestamp, Visibility,
};
use myelin_identity::{Literal, Principal, PrincipalId, PrincipalKind, SetExpr};
use myelin_issues::{
    default_stale_after, ArmRequest, ArmableCondition, IssueTriggerEngine, TriggerInboxKind,
    VAR_BLOCKED_BY_UNRESOLVED,
};
use myelin_notif::Reason;
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
fn subject() -> ArtifactRef {
    ArtifactRef("myelin://acme/issues/issue/PROJ-7".into())
}
fn no_rel(_m: &RelMembership) -> bool {
    false
}

fn unblock_event(event_id: &str, blockers_open: i64) -> EventEnvelope {
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
        subject: subject(),
        aggregate: AggregateKey("issue:PROJ-7".into()),
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

/// **PRODUCER side — the armable-condition catalogue compiles to a frozen EventMatcher (= QueryAst,
/// 3.4) over issue_relation projection state.** The flagship "remind me when unblocked" reads the
/// `blocked_by_unresolved == 0` projection-state predicate (NOT a join, §10). Pins the predicate shape.
#[test]
fn producer_catalogue_compiles_to_the_frozen_querast_matcher() {
    let m = ArmableCondition::RemindWhenUnblocked.to_matcher(&subject());
    // The matcher reads the issue_relation projection-state variable (the named freeze).
    assert_eq!(VAR_BLOCKED_BY_UNRESOLVED, "payload.blocked_by_unresolved");
    // Resolves only when all blockers cleared (the projection-state condition).
    assert!(m
        .matches(&unblock_event("e", 0), &SetExpr::All, &no_rel)
        .unwrap());
    assert!(!m
        .matches(&unblock_event("e", 2), &SetExpr::All, &no_rel)
        .unwrap());

    // The full §10 catalogue is a closed set of pre-authored predicates (no user DSL). Each compiles.
    let _ = ArmableCondition::PingWhenLeavesState {
        x: "unstarted".into(),
    }
    .to_matcher(&subject());
    let _ = ArmableCondition::NotifyWhenAssignedToMe { me: "alice".into() }.to_matcher(&subject());
    let _ = ArmableCondition::TellWhenSlaAtRisk.to_matcher(&subject());
    let _ = ArmableCondition::TellWhenInitiativeAtRisk.to_matcher(&subject());
    // The catalogue maps each condition to its frozen Notif reason (7.6 reconciled).
    assert_eq!(
        ArmableCondition::RemindWhenUnblocked.resolve_reason(),
        Reason::Unblocked
    );
    assert_eq!(
        ArmableCondition::TellWhenSlaAtRisk.resolve_reason(),
        Reason::Sla
    );
}

/// **CONSUMER side — the bus TriggerEngine admits the Issues-armed Trigger and fires it ONCE per
/// arming (the atomic guarded UPDATE), delivering ONE inbox item; a re-delivery is a no-op.**
#[test]
fn consumer_bus_engine_fires_once_per_arming_and_delivers_one_inbox_item() {
    let mut eng = IssueTriggerEngine::new(tenant(), region());
    eng.arm(ArmRequest {
        trigger_id: TriggerId("t".into()),
        condition: ArmableCondition::RemindWhenUnblocked,
        owner: owner(),
        arms_subject: subject(),
        stale_after: Some(default_stale_after(0)),
    })
    .unwrap();

    // The condition resolves → fires ONCE into the one inbox.
    assert_eq!(
        eng.on_event(&unblock_event("e1", 0), &SetExpr::All, &no_rel),
        1
    );
    assert_eq!(eng.delivered().len(), 1);
    assert_eq!(eng.delivered()[0].kind, TriggerInboxKind::Resolved);
    assert_eq!(
        eng.arming(&TriggerId("t".into())).unwrap().state,
        TriggerState::Resolved
    );
    // A re-delivery LOSES the guarded UPDATE — on_resolve does not run again (fire-once-per-arming).
    assert_eq!(
        eng.on_event(&unblock_event("e2", 0), &SetExpr::All, &no_rel),
        0
    );
    assert_eq!(eng.delivered().len(), 1, "still exactly one fire");
}

/// **CONSUMER side — `disarm_trigger` cancels: an armed promise disarms (the guarded UPDATE), a later
/// resolving event does not fire.**
#[test]
fn consumer_disarm_trigger_cancels_the_promise() {
    let mut eng = IssueTriggerEngine::new(tenant(), region());
    let id = TriggerId("t".into());
    eng.arm(ArmRequest {
        trigger_id: id.clone(),
        condition: ArmableCondition::RemindWhenUnblocked,
        owner: owner(),
        arms_subject: subject(),
        stale_after: Some(default_stale_after(0)),
    })
    .unwrap();
    assert!(eng.disarm(&id).unwrap(), "the owner cancel disarms");
    assert_eq!(eng.arming(&id).unwrap().state, TriggerState::Disarmed);
    assert_eq!(
        eng.on_event(&unblock_event("e", 0), &SetExpr::All, &no_rel),
        0
    );
    assert!(eng.delivered().is_empty());
}

/// **The condition is a real QueryAst over projection state — a typed-literal mismatch never silently
/// fires (3.4 fail-closed).** A payload whose `blocked_by_unresolved` is a STRING (not the Int 0 the
/// predicate compares) does NOT resolve (the bounded interpreter treats the type mismatch as no-match,
/// never a leak-driven resolve).
#[test]
fn condition_fails_closed_on_a_type_mismatch() {
    let mut eng = IssueTriggerEngine::new(tenant(), region());
    eng.arm(ArmRequest {
        trigger_id: TriggerId("t".into()),
        condition: ArmableCondition::RemindWhenUnblocked,
        owner: owner(),
        arms_subject: subject(),
        stale_after: None,
    })
    .unwrap();
    let mut e = unblock_event("e", 0);
    e.payload = serde_json::json!({ "blocked_by_unresolved": "zero" });
    let _ = Literal::Int(0); // the predicate's rhs literal type (pinned).
    assert_eq!(
        eng.on_event(&e, &SetExpr::All, &no_rel),
        0,
        "a type-mismatched projection value does not fire (fail-closed, 3.4)"
    );
}

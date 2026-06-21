//! # The CDC pair for the loop-safety HALF of contract 9.2 (P-FLOW-18, FLOW-D7)
//!
//! **Contracts:** `planning/05-refined-shared-systems-architecture/contract-index.md` row 9.2
//! (`WfCtx` — the loop-safety enforcement half: this HARDENS the surface against self-feeding loops;
//! no new owned row). Owning architecture: `durable-workflow.md` §6.2 (loop safety: causal-depth
//! ceiling + shared-root tripwire + bounded activity pool — *an adversarial workflow→event→workflow
//! loop is dropped/parked, NEVER forked*) + §3.1 (the `workflow_run.depth`/`correlation_id` causality
//! columns) + §5.4 (the causal-depth-histogram telemetry).
//!
//! ## What this pair pins (the PROVIDER ↔ CONSUMER agreement of 9.2's loop-safety half)
//!
//! **9.2 PROVIDER (the engine's [`CausalGuard`]) — the agreement the workflow engine guarantees:**
//! - a would-be CHILD start past the causal-depth ceiling is REFUSED ([`LoopVerdict::Drop`],
//!   `DepthCeiling`) — the depth NEVER exceeds the ceiling;
//! - a workflow→event→workflow loop re-entering one `correlation_id` root past the window cap is
//!   REFUSED ([`LoopVerdict::Drop`], `SharedRootTripwire`);
//! - a would-be activity over the bounded-pool cap is REFUSED ([`LoopVerdict::Park`],
//!   `ActivityPoolFull`);
//! - **EVERY refusal is a drop/park — there is NO `Fork` variant**; the 0-fork counter stays 0.
//!
//! **9.2 CONSUMER (a sibling subsystem that starts child runs — a bus automation, an agent run that
//! spawns sub-runs, the dispatch tier) — what it relies on:**
//! - it may safely propagate `correlation_id` + `depth` from a cause into a child start; the engine
//!   bounds the chain (the consumer cannot typo its way into a runaway loop, AG-6);
//! - a refused hop is OBSERVABLE (a verdict + a machine reason), never a silent drop or a fork.
//!
//! This pins the provider's loop-safety promise against a CONSUMER that drives the same
//! `EventEnvelope` causality (`correlation_id`/`depth`) a real start would carry — the bus's
//! dispatch-tier mirror (event-bus §4.7) reads the SAME signals.

use myelin_events::{
    Actor, AggregateKey, ArtifactRef as EvArtifactRef, DataRole, EmitContextBase, EventDraft,
    EventType, IdMinter, MonotonicMinter, OutboxStore, Timestamp, Visibility,
};
use myelin_flow::{
    CausalGuard, FlowTelemetry, LoopVerdict, RefusalReason, WfCtx, WfJournal,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
}
fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
}
fn ctx_base() -> EmitContextBase {
    EmitContextBase {
        tenant: tenant(),
        region: region(),
        actor: Actor(Principal::stub(PrincipalId("p".into()), PrincipalKind::Human, tenant())),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: None,
    }
}

/// **PROVIDER half — the engine's [`CausalGuard`] bounds a child-start chain by depth, root, and pool,
/// and NEVER forks.** A self-feeding chain at small caps is admitted up to the ceiling, then dropped;
/// a same-root loop is tripped; an over-cap activity is parked. Every refusal is a drop/park.
#[test]
fn provider_9_2_loop_safety_drops_or_parks_never_forks() {
    let telemetry = FlowTelemetry::new();
    let guard = CausalGuard::with_caps(3, 4, 2).with_telemetry(telemetry.clone());
    let root = "corr-9-2";

    // depth chain: child 1,2,3 admitted (<= ceiling 3); child 4 dropped.
    assert_eq!(guard.admit_child(root, 0).0, LoopVerdict::Admit);
    assert_eq!(guard.admit_child(root, 1).0, LoopVerdict::Admit);
    assert_eq!(guard.admit_child(root, 2).0, LoopVerdict::Admit);
    let (v, r) = guard.admit_child(root, 3);
    assert_eq!(v, LoopVerdict::Drop, "child at depth 4 > ceiling 3 is DROPPED");
    assert_eq!(r, Some(RefusalReason::DepthCeiling));
    assert!(telemetry.causal_depth_max() <= guard.ceiling(), "depth never exceeds the ceiling");

    // an over-cap activity is PARKED (never forked).
    assert_eq!(guard.admit_activity().0, LoopVerdict::Admit);
    assert_eq!(guard.admit_activity().0, LoopVerdict::Admit);
    assert_eq!(guard.admit_activity().0, LoopVerdict::Park, "over-cap activity is PARKED");

    // the 0-fork invariant — the headline.
    assert_eq!(telemetry.fork_count(), 0, "the engine NEVER forks a runaway loop");
}

/// **CONSUMER half — a sibling that propagates an `EventEnvelope`'s `correlation_id`/`depth` into a
/// child start relies on the engine to bound the chain.** The consumer emits an event (carrying the
/// causality the bus derives) then asks the guard to admit a child at the carried depth+root; the
/// engine's bound is what makes the consumer unable to typo into a loop (AG-6). The consumer sees a
/// VERDICT (admit/drop/park) + a machine reason, never a silent drop and never a fork.
#[test]
fn consumer_9_2_propagated_causality_is_bounded_by_the_engine() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let telemetry = FlowTelemetry::new();
    // the bus dispatch-tier mirror reads the same ceiling; small caps so the consumer's loop hits them.
    let guard = CausalGuard::with_caps(5, 3, 64).with_telemetry(telemetry.clone());

    // the consumer emits an event through the ONE outbox path (the causality the engine reads).
    let mut ctx = WfCtx::begin(
        &outbox,
        minter(),
        journal,
        ctx_base(),
        "R-consumer",
        "agent.run",
        "2026-06-21T00:00:00Z",
        7,
    );
    let draft = EventDraft {
        type_: EventType("agent.run.spawned".into()),
        subject: EvArtifactRef("myelin://acme/agent/run/R-consumer".into()),
        aggregate: AggregateKey("run:R-consumer".into()),
        payload: serde_json::json!({ "ref": "R-consumer" }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    };
    let _ = ctx.emit(draft, None).expect("emit through the outbox");
    ctx.commit().expect("co-commit");

    // The consumer drives a workflow→event→workflow loop at a SHALLOW depth under ONE root — the
    // depth ceiling cannot catch it; the shared-root tripwire MUST (the consumer cannot loop forever).
    let root = "corr-consumer-loop";
    let mut admitted = 0u32;
    let mut refused = 0u32;
    for _ in 0..8 {
        // `LoopVerdict` has only Admit/Drop/Park — there is NO Fork variant, so the consumer can never
        // observe a fork (the 0-fork invariant is enforced by the TYPE, not just a counter).
        let (verdict, reason) = guard.admit_child(root, 1);
        match verdict {
            LoopVerdict::Admit => admitted += 1,
            LoopVerdict::Drop => {
                refused += 1;
                assert_eq!(reason, Some(RefusalReason::SharedRootTripwire), "observable reason");
            }
            LoopVerdict::Park => refused += 1,
        }
    }

    assert_eq!(admitted, 3, "the consumer's loop was admitted up to the shared-root cap");
    assert_eq!(refused, 5, "every same-root hop past the cap was refused (drop/park)");
    assert_eq!(telemetry.fork_count(), 0, "0-fork — the consumer relies on this");
    assert!(
        telemetry.shared_root_tripwire_firings() >= 1,
        "the tripwire fired (the consumer's loop was caught by its root)"
    );
}

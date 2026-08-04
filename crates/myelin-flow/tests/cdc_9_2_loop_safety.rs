use myelin_events::{
    Actor, AggregateKey, ArtifactRef as EvArtifactRef, DataRole, EmitContextBase, EventDraft,
    EventType, IdMinter, MonotonicMinter, OutboxStore, Timestamp, Visibility,
};
use myelin_flow::{CausalGuard, FlowTelemetry, LoopVerdict, RefusalReason, WfCtx, WfJournal};
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
        actor: Actor(Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            tenant(),
        )),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-21T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-21T00:00:01Z".into()),
        caused_by: None,
    }
}

#[test]
fn provider_9_2_loop_safety_drops_or_parks_never_forks() {
    let telemetry = FlowTelemetry::new();
    let guard = CausalGuard::with_caps(3, 4, 2).with_telemetry(telemetry.clone());
    let root = "corr-9-2";

    assert_eq!(guard.admit_child(root, 0).0, LoopVerdict::Admit);
    assert_eq!(guard.admit_child(root, 1).0, LoopVerdict::Admit);
    assert_eq!(guard.admit_child(root, 2).0, LoopVerdict::Admit);
    let (v, r) = guard.admit_child(root, 3);
    assert_eq!(
        v,
        LoopVerdict::Drop,
        "child at depth 4 > ceiling 3 is DROPPED"
    );
    assert_eq!(r, Some(RefusalReason::DepthCeiling));
    assert!(
        telemetry.causal_depth_max() <= guard.ceiling(),
        "depth never exceeds the ceiling"
    );

    assert_eq!(guard.admit_activity().0, LoopVerdict::Admit);
    assert_eq!(guard.admit_activity().0, LoopVerdict::Admit);
    assert_eq!(
        guard.admit_activity().0,
        LoopVerdict::Park,
        "over-cap activity is PARKED"
    );

    assert_eq!(
        telemetry.fork_count(),
        0,
        "the engine NEVER forks a runaway loop"
    );
}

#[test]
fn consumer_9_2_propagated_causality_is_bounded_by_the_engine() {
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();
    let telemetry = FlowTelemetry::new();
    let guard = CausalGuard::with_caps(5, 3, 64).with_telemetry(telemetry.clone());

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

    let root = "corr-consumer-loop";
    let mut admitted = 0u32;
    let mut refused = 0u32;
    for _ in 0..8 {
        let (verdict, reason) = guard.admit_child(root, 1);
        match verdict {
            LoopVerdict::Admit => admitted += 1,
            LoopVerdict::Drop => {
                refused += 1;
                assert_eq!(
                    reason,
                    Some(RefusalReason::SharedRootTripwire),
                    "observable reason"
                );
            }
            LoopVerdict::Park => refused += 1,
        }
    }

    assert_eq!(
        admitted, 3,
        "the consumer's loop was admitted up to the shared-root cap"
    );
    assert_eq!(
        refused, 5,
        "every same-root hop past the cap was refused (drop/park)"
    );
    assert_eq!(
        telemetry.fork_count(),
        0,
        "0-fork - the consumer relies on this"
    );
    assert!(
        telemetry.shared_root_tripwire_firings() >= 1,
        "the tripwire fired (the consumer's loop was caught by its root)"
    );
}

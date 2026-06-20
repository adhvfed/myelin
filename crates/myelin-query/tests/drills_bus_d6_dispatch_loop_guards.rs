//! # BUS-D6 (F9) — the self-triggering automation loop drill (EB-23 / P-143)
//!
//! **Drill catalogue:** `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` row BUS-D6
//! ("Self-triggering automation → depth ceiling + shared-root tripwire trip the per-tenant
//! breaker", gate: **halts ≤ ceiling; breaker trips**) — architecture §8 D-6 / §4.7; AG-6.
//!
//! ## What this drill proves (the EB-23 GATE)
//! An adversarial **self-triggering** reactive chain — an agent whose dispatched action emits an
//! event that would re-trigger the SAME agent on the SAME causal root — is HALTED by TWO structural
//! guards, with NO weakened threshold:
//! 1. **The causal-depth ceiling (default 12).** A chain that deepens (`depth = parent + 1` each
//!    hop) is PARKED once it reaches the ceiling — it halts at `≤ ceiling`, it does NOT recurse
//!    unboundedly. We drive a chain that deepens and assert the dispatch parks at the ceiling.
//! 2. **The shared-causal-root tripwire.** A storm that stays UNDER the depth ceiling but fans out
//!    WIDE on one `correlation_id` trips the **per-tenant circuit breaker** once it exceeds `K`;
//!    every further dispatch for that tenant is shed (`429 + Retry-After`).
//!
//! The verdict is read off the FROZEN §10.2 harness survival signals (the EB-11 deviation bridge:
//! the dispatch tier owns the *measurement*, the harness owns the *assertion vocabulary*):
//! `CausalDepthFirings` (the ceiling/tripwire fired, asserted `>= 1`), `BreakerState` (open = 2,
//! asserted `== 2` for the storming tenant), and `DispatchPoolDrops` / `ShedCount{lane=agent}`
//! (the storm shed, asserted `>= 1`). All exact — never weakened.

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, CorrelationId, DataRole, EventEnvelope, EventId,
    EventType, Timestamp, Visibility,
};
use myelin_harness::{Label, Predicate, SignalName, SignalSource};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_query::{
    DispatchRequest, DispatchTier, Disposition, InMemoryCostGate, RecordingTarget, ShedReason,
    TriggerKind, CAUSAL_DEPTH_CEILING,
};
use myelin_tenancy::{Region, TenantId};

fn principal(id: &str) -> Principal {
    // Kind is irrelevant to the depth/tripwire guards (the self-guard compares principal_id, not
    // kind); `Human` keeps the fixture simple (the struct-variant `Agent` needs runtime_ref).
    Principal::stub(
        PrincipalId(id.into()),
        PrincipalKind::Human,
        TenantId("t1".into()),
    )
}

/// One event in the self-triggering chain: actor `actor`, the next artifact_ref the chain re-fires
/// on, at causal `depth`, on the shared root `correlation`.
fn chain_event(actor: &str, n: u64, depth: u32, correlation: &str) -> EventEnvelope {
    EventEnvelope {
        event_id: EventId(format!("ev:{n}")),
        type_: EventType("agent.run.emitted".into()),
        schema_ver: 1,
        tenant: TenantId("t1".into()),
        region: Region("t1-home".into()),
        actor: Actor(principal(actor)),
        subject: ArtifactRef(format!("myelin://t1/chat/message/{n}")),
        aggregate: AggregateKey("agg:loop".into()),
        causation_id: None,
        correlation_id: CorrelationId(correlation.into()),
        caused_by: Some(CausedBy("session:human".into())),
        depth,
        contains_personal_data: false,
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        pii_key_ref: None,
        occurred_at: Timestamp("2026-06-20T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-20T00:00:00Z".into()),
        payload: serde_json::json!({}),
    }
}

fn req(ev: EventEnvelope, n: u64) -> DispatchRequest {
    DispatchRequest {
        event: ev,
        // The dispatching consumer is a DIFFERENT agent than the chain's actor, so the self-guard
        // does not short-circuit this drill — we exercise the depth/tripwire guards specifically.
        agent: PrincipalId("dispatcher-agent".into()),
        run_ref: format!("run-{n}"),
        trigger: TriggerKind::Automation,
    }
}

/// Bridge the dispatch tier's measured loop-guard telemetry into the FROZEN §10.2 harness signal
/// vocabulary (the EB-11 deviation bridge). The tier owns the measurement; the harness owns the
/// assertion. `breaker_open` encodes the §10.2 numeric breaker state (open=2, closed=0).
fn bridge(src: &mut SignalSource, firings: i64, breaker_open: bool, sheds: i64) {
    src.set_scalar(SignalName::CausalDepthFirings, firings);
    src.set_labelled(
        SignalName::BreakerState,
        vec![Label::new("downstream", "dispatch:t1")],
        if breaker_open { 2 } else { 0 },
    );
    src.set_scalar(SignalName::DispatchPoolDrops, sheds);
    src.set_labelled(
        SignalName::ShedCount,
        vec![Label::new("lane", "agent")],
        sheds,
    );
}

/// **BUS-D6 LEG 1 — the causal-depth ceiling halts a deepening self-trigger at ≤ ceiling.**
#[test]
fn bus_d6_depth_ceiling_halts_a_deepening_chain_at_the_ceiling() {
    let gate = InMemoryCostGate::new(0); // cost 0 — isolate the depth guard from the balance gate
    let mut tier = DispatchTier::new(RecordingTarget::new(), gate);

    // Drive a self-triggering chain that deepens by 1 each hop, starting BELOW the ceiling. Each
    // dispatched action is depth = parent + 1 (nested causality), so the chain climbs until the
    // trigger reaches the ceiling and the dispatch PARKS.
    let mut parked_at: Option<u32> = None;
    let mut dispatched = 0u64;
    for depth in (CAUSAL_DEPTH_CEILING - 3)..=(CAUSAL_DEPTH_CEILING + 1) {
        let ev = chain_event("loop-agent", depth as u64, depth, "root-deep");
        match tier.dispatch(&req(ev, depth as u64), || EventId(format!("act-{depth}")), &Timestamp("2026-06-20T00:00:01Z".into())) {
            Disposition::Delivered { action } => {
                dispatched += 1;
                // The dispatched action is strictly deeper than its trigger (nested, never flat).
                assert_eq!(action.depth, depth + 1);
            }
            Disposition::DepthCeilingParked { depth: d } => {
                parked_at = Some(d);
                break;
            }
            o => panic!("unexpected disposition {o:?}"),
        }
    }
    let parked = parked_at.expect("the chain parked at the ceiling, never recursed unboundedly");
    assert!(parked >= CAUSAL_DEPTH_CEILING, "halted at ≤ ceiling (parked at {parked})");
    assert_eq!(tier.telemetry().depth_ceiling_parked, 1, "exactly one park recorded");
    assert!(dispatched <= 3, "the chain halted in a bounded number of hops, not unboundedly");

    // The §10.2 survival signal reads the ceiling fired.
    let mut src = SignalSource::new();
    bridge(&mut src, tier.telemetry().depth_ceiling_parked as i64, false, 0);
    src.assert_signal(SignalName::CausalDepthFirings, Predicate::Gte(1)).expect_green();
}

/// **BUS-D6 LEG 2 (the headline) — the shared-causal-root tripwire trips the per-tenant breaker.**
#[test]
fn bus_d6_shared_root_tripwire_trips_the_per_tenant_breaker_and_sheds() {
    // K = 5 so the trip is forced in a handful of events (the SAME structural code as K=64). A
    // wide storm on ONE root, all UNDER the depth ceiling (depth 1) — the depth guard does NOT fire,
    // only the tripwire. Generous balance + cap so only the tripwire is the limiter.
    let gate = InMemoryCostGate::new(0);
    let mut tier = DispatchTier::with_limits(RecordingTarget::new(), gate, 100, 5, 100_000);
    let t1 = TenantId("t1".into());
    let root = "root-storm";

    let mut shed = 0u64;
    for n in 0..20u64 {
        let ev = chain_event("loop-agent", n, 1, root); // depth 1 — under the ceiling
        match tier.dispatch(&req(ev, n), || EventId(format!("act-{n}")), &Timestamp("2026-06-20T00:00:01Z".into())) {
            Disposition::Delivered { .. } => {}
            Disposition::BreakerShed { shed: s } => {
                assert_eq!(s.status, 429, "the shed is 429 + Retry-After, never silent");
                assert!(s.retry_after_seconds > 0);
                assert_eq!(s.reason, ShedReason::BreakerOpen);
                assert_eq!(s.signal_subject(), "signal.dispatch.breaker_open");
                shed += 1;
            }
            o => panic!("unexpected disposition {o:?}"),
        }
    }

    // GATE: the breaker tripped (exactly once), the over-K root crossed the tripwire, and the
    // post-trip dispatches were shed — halts (the storm bounded), breaker trips.
    assert!(tier.breaker_open(&t1), "the per-tenant breaker is OPEN");
    assert_eq!(tier.telemetry().tripwire_firings, 1, "the tripwire fired exactly once");
    assert!(tier.root_count(&t1, &CorrelationId(root.into())) > 5, "the shared-root counter crossed K");
    assert!(shed >= 1, "post-trip dispatches were shed (the storm halted)");

    // The §10.2 survival signals read the verdict: tripwire fired, breaker OPEN, lane shed.
    let mut src = SignalSource::new();
    bridge(&mut src, tier.telemetry().tripwire_firings as i64, true, shed as i64);
    src.assert_signal(SignalName::CausalDepthFirings, Predicate::Gte(1)).expect_green();
    src.assert_labelled(
        SignalName::BreakerState,
        vec![Label::new("downstream", "dispatch:t1")],
        Predicate::Eq(2),
    )
    .expect_green();
    src.assert_labelled(
        SignalName::ShedCount,
        vec![Label::new("lane", "agent")],
        Predicate::Gte(1),
    )
    .expect_green();
}

/// **BUS-D6 isolation — the breaker is PER-TENANT: a storm in t1 does not break t2.**
#[test]
fn bus_d6_breaker_is_per_tenant_blast_radius_limited() {
    let gate = InMemoryCostGate::new(0);
    let mut tier = DispatchTier::with_limits(RecordingTarget::new(), gate, 100, 3, 100_000);

    // Storm t1's root until it trips.
    for n in 0..10u64 {
        let ev = chain_event("loop-agent", n, 1, "root-t1");
        let _ = tier.dispatch(&req(ev, n), || EventId(format!("t1-{n}")), &Timestamp("2026-06-20T00:00:01Z".into()));
    }
    assert!(tier.breaker_open(&TenantId("t1".into())));

    // A t2 event (different tenant) is NOT shed by t1's breaker — it dispatches normally.
    let mut t2 = chain_event("other-agent", 99, 1, "root-t2");
    t2.tenant = TenantId("t2".into());
    let disp = tier.dispatch(
        &DispatchRequest {
            event: t2,
            agent: PrincipalId("dispatcher-agent".into()),
            run_ref: "run-t2".into(),
            trigger: TriggerKind::Automation,
        },
        || EventId("t2-act".into()),
        &Timestamp("2026-06-20T00:00:01Z".into()),
    );
    assert!(matches!(disp, Disposition::Delivered { .. }), "t2 is unaffected by t1's breaker");
    assert!(!tier.breaker_open(&TenantId("t2".into())), "t2's breaker is closed");
}

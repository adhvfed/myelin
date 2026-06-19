//! # SUB-D1 + BUS-D4 drill scenarios (P-S07 → P-008, the silent-data-loss floor)
//!
//! These are the failure-injection-harness drills the P-S07 TESTS field requires: each rides
//! the **P-S03 scoped-reversible dependency-break injector** (`Dependency::Broker`) to inject
//! the "kill between commit and publish" / "broker down" fault, drives committed events through
//! the **outbox + relay** (the P-S07 deliverable), and reads the **P-S04 telemetry-assertion
//! library** survival signals (`outbox_depth`, `dead_letter_count`) — a typed green/red that is
//! never a swallowed pass (EI-01 §3).
//!
//! Both thresholds are **0** (0 ghost, 0 lost). A red drill is information: it is NOT weakened
//! to pass — it becomes a dated "claimed, not proven" thresholds-file row (P-S22). **This is a
//! PERMANENT gate (re-run on every emit-path change).**
//!
//! The injector's `Dependency::Broker` is the SAME seam the P-S03 module names ("severing it
//! mid-stream drives SUB-D2 / BUS-D1 … wired at P-S07 (relay)"): here the drill holds the
//! injector handle and, when the broker is broken for the drill's tenant, severs the
//! [`InProcessBus`] so the relay's `put` fails exactly as the real severance would. The
//! inject → load → assert SHAPE is the harness's frozen unit-of-proof.

use myelin_events::{
    Actor, AggregateKey, ArtifactRef, CausedBy, DataRole, EmitContextBase, EventDraft, EventType,
    IdMinter, InProcessBus, MonotonicMinter, OutboxStore, OutboxTx, Relay, Timestamp, Visibility,
};
use myelin_harness::{
    Dependency, DependencyBreaker, Predicate, Scope, SignalName, SignalSource,
};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};
use std::sync::Arc;

fn clock() -> Timestamp {
    Timestamp("2026-06-19T00:00:02Z".into())
}

fn ctx_base(tenant: &str) -> EmitContextBase {
    EmitContextBase {
        tenant: TenantId(tenant.into()),
        region: Region("eu-west".into()),
        actor: Actor(Principal::stub(PrincipalId("p".into()), PrincipalKind::Human, TenantId(tenant.into()))),
        schema_ver: 1,
        occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
        recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
        caused_by: Some(CausedBy("session:abc".into())),
    }
}

fn draft(i: usize, aggregate: &str) -> EventDraft {
    EventDraft {
        type_: EventType(format!("issues.issue.e{i}")),
        subject: ArtifactRef(format!("myelin://acme/issues/issue/{aggregate}")),
        aggregate: AggregateKey(aggregate.into()),
        payload: serde_json::json!({ "ref": aggregate }),
        data_role: DataRole::Controller,
        visibility: Visibility::Internal,
        contains_personal_data: false,
        pii_key_ref: None,
    }
}

/// Commit `n` events for one aggregate (the "state-commit" half), returning their ids.
fn commit_n(store: &OutboxStore, minter: Arc<dyn IdMinter>, n: usize, aggregate: &str) -> Vec<myelin_events::EventId> {
    let mut tx = store.begin(minter, ctx_base("acme"));
    tx.stage_state_change("issue created");
    let mut ids = Vec::new();
    for i in 0..n {
        ids.push(tx.emit(draft(i, aggregate), None).unwrap());
    }
    tx.commit().unwrap();
    ids
}

/// **SUB-D1 — kill service between commit & publish → outbox delivers every committed event
/// exactly-once-in-effect (0 ghost, 0 lost); outbox-depth drains.**
///
/// Rides the P-S03 injector (`Dependency::Broker`, tenant-scoped) + the P-S04 assertion library
/// (`outbox_depth → 0`). The fault is injected between the commit and the publish.
#[test]
fn drill_sub_d1_kill_between_commit_and_publish() {
    let tenant = TenantId("acme".into());
    let scope = Scope::Tenant(tenant.clone());

    // The P-S03 injector — the shared T-3 seam the relay's fault-point consults.
    let breaker = DependencyBreaker::new();

    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
    // STATE-COMMIT: 6 events committed (durable in the outbox, unsent).
    let ids = commit_n(&store, minter, 6, "issue:PROJ-1");

    let bus = InProcessBus::new();
    let relay = Relay::new(store.clone(), bus.clone(), clock);

    // (1) INJECT the fault: break the broker for this tenant (kill between commit and publish).
    //     The relay's fault-point consults the injector and severs the transport accordingly.
    breaker.break_dependency(Dependency::Broker, scope.clone());
    if breaker.is_broken(&Dependency::Broker, &scope) {
        bus.sever();
    }
    relay.drain_to_empty();

    // (2) READ the survival signals while severed: 0 LOST — every committed event is parked in
    //     the outbox (depth == the committed count), and 0 ghost (nothing delivered).
    let mut signals = SignalSource::new();
    signals.set_scalar(SignalName::OutboxDepth, store.outbox_depth() as i64);
    signals.set_scalar(SignalName::DeadLetterCount, store.dead_letter_count() as i64);
    // depth is exactly the committed set: nothing lost, nothing ghosted away.
    signals
        .assert_signal(SignalName::OutboxDepth, Predicate::Eq(6))
        .expect_green();
    signals
        .assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(bus.delivered_count(), 0, "severed broker delivered nothing (0 ghost)");

    // (3) RESTORE the dependency + drain: the outbox-depth drains to 0 and every committed event
    //     is delivered EXACTLY ONCE (0 ghost, 0 lost).
    breaker.restore_dependency(Dependency::Broker, scope.clone());
    if !breaker.is_broken(&Dependency::Broker, &scope) {
        bus.heal();
    }
    relay.drain_to_empty();

    let mut after = SignalSource::new();
    after.set_scalar(SignalName::OutboxDepth, store.outbox_depth() as i64);
    after.set_scalar(SignalName::DeadLetterCount, store.dead_letter_count() as i64);
    // outbox-depth drains → 0 (SUB-D1).
    after
        .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        .expect_green();
    // 0 lost / 0 ghost: the delivered set equals exactly the committed set.
    after
        .assert_signal(SignalName::DeadLetterCount, Predicate::Eq(0))
        .expect_green();
    assert_eq!(bus.delivered_count(), 6, "exactly-once: 6 committed → 6 delivered");
    assert_eq!(
        bus.delivered_ids(),
        ids.into_iter().collect(),
        "the delivered set == the committed set (0 ghost, 0 lost)"
    );

    // teardown: no leaked break.
    assert_eq!(breaker.broken_count(), 0);
    println!("[2026-06-19] PASS  drill=SUB-D1  outbox_depth→0, ghost=0, lost=0  (inject → load → assert green)");
}

/// **BUS-D4 — crash producer between state-commit and publish → event still delivered (outbox),
/// never without state; outbox emit-iff-committed.**
///
/// The crash is modeled as the producer's transaction being DROPPED without commit (the crash
/// point between state-commit and publish). The committed transaction's events ARE delivered;
/// the crashed (un-committed) transaction's events are NOT — and crucially, no event exists
/// without its state. Reads the P-S04 assertion library (`outbox_depth`).
#[test]
fn drill_bus_d4_emit_iff_committed() {
    let store = OutboxStore::new();
    let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());

    // (A) A COMMITTED producer transaction: state + 3 events commit together.
    let committed_ids = commit_n(&store, minter.clone(), 3, "issue:PROJ-1");

    // (B) A CRASHED producer transaction: state staged + 2 events emitted, then the transaction
    //     is dropped WITHOUT commit (the crash between state-commit and publish). emit-iff-
    //     committed: this writes NOTHING — no event without its state.
    {
        let mut tx = store.begin(minter, ctx_base("acme"));
        tx.stage_state_change("issue PROJ-2 (will crash before commit)");
        tx.emit(draft(0, "issue:PROJ-2"), None).unwrap();
        tx.emit(draft(1, "issue:PROJ-2"), None).unwrap();
        // crash: tx dropped here without commit.
    }

    // The crashed transaction left no rows: depth == exactly the committed 3 (not 5).
    let mut signals = SignalSource::new();
    signals.set_scalar(SignalName::OutboxDepth, store.outbox_depth() as i64);
    signals
        .assert_signal(SignalName::OutboxDepth, Predicate::Eq(3))
        .expect_green();
    assert_eq!(
        store.committed_count(),
        3,
        "emit-iff-committed: the crashed (un-committed) transaction wrote NO event"
    );

    // The relay delivers exactly the committed events — never the crashed ones (no event
    // without its state).
    let bus = InProcessBus::new();
    let relay = Relay::new(store.clone(), bus.clone(), clock);
    relay.drain_to_empty();

    let mut after = SignalSource::new();
    after.set_scalar(SignalName::OutboxDepth, store.outbox_depth() as i64);
    after
        .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
        .expect_green();
    assert_eq!(bus.delivered_count(), 3, "only the committed events are delivered");
    assert_eq!(
        bus.delivered_ids(),
        committed_ids.into_iter().collect(),
        "emit-iff-committed: delivered set == committed set, the crashed events never appear"
    );
    println!("[2026-06-19] PASS  drill=BUS-D4  emit_iff_committed=true  (inject → load → assert green)");
}

/// The drills also REGISTER into the P-S04 every-incident-adds-a-drill registry so they re-run
/// forever (EI-01 §3/§5) — a regression on the emit path re-reds them loudly. This wires the
/// SUB-D1 scenario into the permanent suite the harness self-test seeded.
#[test]
fn sub_d1_registers_into_the_permanent_drill_suite() {
    use myelin_harness::{DrillRegistry, DrillScenario};

    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new("SUB-D1-outbox-relay", |ctx| {
        let tenant = TenantId("acme".into());
        let scope = Scope::Tenant(tenant.clone());

        let store = OutboxStore::new();
        let minter: Arc<dyn IdMinter> = Arc::new(MonotonicMinter::new());
        commit_n(&store, minter, 4, "issue:PROJ-1");

        let bus = InProcessBus::new();
        let relay = Relay::new(store.clone(), bus.clone(), clock);

        // inject via the scenario's own breaker (the harness drains it on teardown).
        ctx.breaker.break_dependency(Dependency::Broker, scope.clone());
        if ctx.breaker.is_broken(&Dependency::Broker, &scope) {
            bus.sever();
        }
        relay.drain_to_empty();
        // restore + drain → depth 0.
        ctx.breaker.restore_dependency(Dependency::Broker, scope);
        bus.heal();
        relay.drain_to_empty();

        ctx.signals.set_scalar(SignalName::OutboxDepth, store.outbox_depth() as i64);
        // the asserted survival signal: the outbox drained (0 lost), exactly 4 delivered.
        assert_eq!(bus.delivered_count(), 4);
        ctx.signals
            .assert_signal(SignalName::OutboxDepth, Predicate::Eq(0))
    }));

    let results = registry.run_all();
    assert!(results[0].is_pass(), "SUB-D1 drill must read green: {:?}", results[0]);
    // re-runs forever (a regression re-reds it).
    assert!(registry.all_green());
    println!("{}", results[0].artifact_row("2026-06-19"));
}

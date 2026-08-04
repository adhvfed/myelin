use myelin_events::{
    Actor, CausedBy, EmitContextBase, IdMinter, MonotonicMinter, OutboxStore, Timestamp,
};
use myelin_flow::maintenance::{CacheNamespace, MaintenanceOp, MaintenancePerformer};
use myelin_flow::wfctx::ActivityError;
use myelin_flow::{WfCtx, WfJournal};
use myelin_harness::{Dependency, DependencyBreaker, Scope};
use myelin_identity::{Principal, PrincipalId, PrincipalKind};
use myelin_tenancy::{Region, TenantId};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn tenant() -> TenantId {
    TenantId("acme".into())
}
fn region() -> Region {
    Region("fr-par".into())
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
        caused_by: Some(CausedBy("session:abc".into())),
    }
}
fn minter() -> Arc<dyn IdMinter> {
    Arc::new(MonotonicMinter::new())
}

#[derive(Default)]
struct RecordingPerformer {
    steps: Mutex<Vec<usize>>,
    namespaces: Mutex<Vec<CacheNamespace>>,
    step_calls: AtomicUsize,
    ns_calls: AtomicUsize,
}
impl MaintenancePerformer for RecordingPerformer {
    fn perform_step(
        &self,
        _op: MaintenanceOp,
        step_index: usize,
        _idem: &str,
    ) -> Result<(), ActivityError> {
        self.step_calls.fetch_add(1, Ordering::SeqCst);
        self.steps.lock().unwrap().push(step_index);
        Ok(())
    }
    fn invalidate_namespace(
        &self,
        namespace: CacheNamespace,
        _idem: &str,
    ) -> Result<(), ActivityError> {
        self.ns_calls.fetch_add(1, Ordering::SeqCst);
        self.namespaces.lock().unwrap().push(namespace);
        Ok(())
    }
}

fn begin(outbox: &OutboxStore, journal: WfJournal) -> WfCtx {
    WfCtx::begin(
        outbox,
        minter(),
        journal,
        ctx_base(),
        "R1",
        "git.maintenance",
        "2026-06-21T00:00:00Z",
        7,
    )
}

fn journal_repack_prefix(outbox: &OutboxStore, journal: &WfJournal, up_to: usize) {
    let performer = RecordingPerformer::default();
    let mut ctx = begin(outbox, journal.clone());
    let ran = ctx
        .run_maintenance(MaintenanceOp::Repack, up_to, &performer)
        .expect("the prefix runs");
    assert_eq!(
        ran, up_to,
        "the prefix journaled {up_to} steps before the crash"
    );
    ctx.commit()
        .expect("the prefix co-commits (durable before the crash)");
}

#[test]
fn drill_crash_mid_repack_resumes_with_no_side_effect() {
    let scope = Scope::Tenant(tenant());
    let breaker = DependencyBreaker::new();
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();

    breaker.break_dependency(Dependency::Broker, scope.clone());
    assert!(
        breaker.is_broken(&Dependency::Broker, &scope),
        "the worker-crash fault is injected"
    );
    journal_repack_prefix(&outbox, &journal, 3);
    let history = journal.history_for(&tenant(), "R1");
    assert_eq!(history.len(), 3, "3 journaled at the crash point");

    breaker.restore_dependency(Dependency::Broker, scope.clone());
    assert!(
        !breaker.is_broken(&Dependency::Broker, &scope),
        "the fault is restored"
    );

    let performer = RecordingPerformer::default();
    let mut ctx = WfCtx::resume(
        &outbox,
        minter(),
        journal.clone(),
        ctx_base(),
        "R1",
        "git.maintenance",
        "2026-06-21T00:00:00Z",
        7,
        history,
    );
    let ran = ctx
        .run_maintenance(MaintenanceOp::Repack, 8, &performer)
        .expect("the resume drive");

    assert_eq!(ran, 5, "resumed at step 3 - only steps 3..=7 ran live");
    assert_eq!(
        *performer.steps.lock().unwrap(),
        vec![3, 4, 5, 6, 7],
        "steps 0..=2 replayed (0 re-execution) - replay to the un-journaled step"
    );
    assert_eq!(
        performer.step_calls.load(Ordering::SeqCst),
        5,
        "0 re-executed side effect - the journaled prefix's pack rewrite was NEVER re-run"
    );
    ctx.commit().expect("co-commit the resumed tail");
    assert_eq!(
        journal.history_for(&tenant(), "R1").len(),
        8,
        "8 journaled, 0 lost, 0 duplicate"
    );
    assert_eq!(breaker.broken_count(), 0, "no leaked dependency break");

    println!(
        "[2026-06-21] PASS  drill=FLOW-maintenance  crash@repack-step-3/8 resume@3  re_executed=0 lost=0  (inject \u{2192} re-lease \u{2192} replay-to-un-journaled-step \u{2192} 0-side-effect)"
    );
}

#[test]
fn drill_invalidation_fan_out_replays_from_last_step() {
    let scope = Scope::Tenant(tenant());
    let breaker = DependencyBreaker::new();
    let outbox = OutboxStore::new();
    let journal = WfJournal::new();

    breaker.break_dependency(Dependency::Broker, scope.clone());
    let performer1 = RecordingPerformer::default();
    let mut c1 = begin(&outbox, journal.clone());
    let n1 = c1
        .run_history_rewrite_invalidation(&[CacheNamespace::Fork], &performer1)
        .expect("the Fork invalidation");
    assert_eq!(n1, 1, "Fork invalidated before the crash");
    c1.commit().expect("the Fork invalidation co-commits");
    let history = journal.history_for(&tenant(), "R1");
    assert_eq!(history.len(), 1, "1 journaled at the crash point");

    breaker.restore_dependency(Dependency::Broker, scope);
    let performer2 = RecordingPerformer::default();
    let mut c2 = WfCtx::resume(
        &outbox,
        minter(),
        journal.clone(),
        ctx_base(),
        "R1",
        "git.maintenance",
        "2026-06-21T00:00:00Z",
        7,
        history,
    );
    let n2 = c2
        .run_history_rewrite_invalidation(&CacheNamespace::FANOUT_ORDER, &performer2)
        .expect("the resume drive");

    assert_eq!(
        n2, 2,
        "resumed from Mirror - only Mirror + CloneBundle ran live"
    );
    assert_eq!(
        *performer2.namespaces.lock().unwrap(),
        vec![CacheNamespace::Mirror, CacheNamespace::CloneBundle],
        "Fork replayed (0 re-invalidation) - the fan-out resumed from the last journaled step"
    );
    assert_eq!(
        performer2.ns_calls.load(Ordering::SeqCst),
        2,
        "0 re-invalidation of the already-purged Fork trust scope"
    );
    c2.commit().expect("co-commit the resumed fan-out tail");
    assert_eq!(
        journal.history_for(&tenant(), "R1").len(),
        3,
        "3 journaled, 0 duplicate"
    );

    println!(
        "[2026-06-21] PASS  drill=FLOW-maintenance  fan-out-replays-from-last-step  re_invalidated=0  (Fork short-circuited, resumed from Mirror)"
    );
}

#[test]
fn maintenance_drill_registers_into_the_permanent_suite() {
    use myelin_harness::{DrillRegistry, DrillScenario, Predicate, SignalName};

    let mut registry = DrillRegistry::new();
    registry.register_drill(DrillScenario::new(
        "FLOW-maintenance-crash-mid-repack",
        |ctx| {
            let scope = Scope::Tenant(tenant());
            let outbox = OutboxStore::new();
            let journal = WfJournal::new();

            ctx.breaker
                .break_dependency(Dependency::Broker, scope.clone());
            journal_repack_prefix(&outbox, &journal, 3);
            let history = journal.history_for(&tenant(), "R1");
            ctx.breaker.restore_dependency(Dependency::Broker, scope);

            let performer = RecordingPerformer::default();
            let mut wf = WfCtx::resume(
                &outbox,
                minter(),
                journal.clone(),
                ctx_base(),
                "R1",
                "git.maintenance",
                "2026-06-21T00:00:00Z",
                7,
                history,
            );
            wf.run_maintenance(MaintenanceOp::Repack, 8, &performer)
                .expect("the resume drive");
            assert_eq!(
                *performer.steps.lock().unwrap(),
                vec![3, 4, 5, 6, 7],
                "resumed at 3"
            );

            ctx.signals.set_scalar(
                SignalName::OutboxDepth,
                performer.step_calls.load(Ordering::SeqCst) as i64,
            );
            ctx.signals
                .assert_signal(SignalName::OutboxDepth, Predicate::Eq(5))
        },
    ));

    let results = registry.run_all();
    assert!(
        results[0].is_pass(),
        "the maintenance drill must read green: {:?}",
        results[0]
    );
    assert!(
        registry.all_green(),
        "the permanent suite re-runs the maintenance drill green forever"
    );
    println!("{}", results[0].artifact_row("2026-06-21"));
}

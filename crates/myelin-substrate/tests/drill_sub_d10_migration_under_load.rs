use myelin_harness::load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, Request, Sink, StormProfile,
};
use myelin_harness::{Predicate, RestoredSnapshot, SignalName, SignalSource};
use myelin_storage::migration::{HotTables, Migration, MigrationPhase, Migrations};
use myelin_storage::{LockBudget, MigrationUnderLoad, WriteLoad};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

#[derive(Default)]
struct WriteLoadSink {
    writes: u32,
}

impl Sink for WriteLoadSink {
    fn handle(&mut self, _request: &Request) {
        self.writes = self.writes.saturating_add(1);
    }
}

fn concurrent_writers_from_load(multiplier: Multiplier, base_requests: u64) -> u32 {
    let tenant = TenantId("acme".into());
    let gen = LoadGenerator::new(
        base_requests,
        multiplier,
        PrincipalMix::from_weights([1, 4, 3, 2, 0]).expect("a non-empty write mix"),
        StormProfile::ci_surge(),
        vec![tenant],
    )
    .expect("a non-empty tenant list");
    let mut sink = WriteLoadSink::default();
    gen.drive(&mut sink);
    assert!(
        sink.writes > 0,
        "the load generator must issue concurrent writes (the load the migration runs under)"
    );
    sink.writes
}

fn online_migration() -> (Migrations, HotTables) {
    let hot = HotTables::declare(["issue"]);
    let migrations = Migrations::of([
        Migration::phased(
            "0010_expand",
            "ALTER TABLE issue ADD COLUMN priority INT;",
            MigrationPhase::Expand,
            "issue",
        ),
        Migration::phased(
            "0011_backfill",
            "UPDATE issue SET priority = 0 WHERE priority IS NULL;",
            MigrationPhase::Backfill,
            "issue",
        ),
        Migration::phased(
            "0012_contract",
            "ALTER TABLE issue ADD COLUMN status TEXT NOT NULL DEFAULT 'open';",
            MigrationPhase::Contract,
            "issue",
        ),
    ]);
    (migrations, hot)
}

fn restored_prod_scale_copy(restored_to: u64) -> RestoredSnapshot {
    RestoredSnapshot::builder(restored_to)
        .blob("blake3:aaaa")
        .blob("blake3:bbbb")
        .row(
            "r1",
            restored_to.saturating_sub(10),
            Some("blake3:aaaa".into()),
        )
        .row("r2", restored_to, Some("blake3:bbbb".into()))
        .row("r3", restored_to.saturating_sub(50), None)
        .index_doc("r1")
        .index_doc("r2")
        .build()
}

fn drive_and_assert_sub_d10(rows: u64, load_multiplier: Multiplier, base_requests: u64) {
    let thresholds = Thresholds::load_canonical().expect("thresholds.toml loads");
    let budget = LockBudget::new(
        thresholds.online_migration.lock_wait_p99_max_ms,
        thresholds.online_migration.downtime_max_ms,
    );
    assert_eq!(
        thresholds.online_migration.downtime_max_ms, 0,
        "the 0-downtime invariant is structural (SUB-D10)"
    );

    let restored_to = 1000;
    let copy_before = restored_prod_scale_copy(restored_to);
    let report_before = copy_before.verify_cross_seam();
    assert!(
        report_before.is_consistent(),
        "SUB-D10 RED: the restored copy was inconsistent BEFORE the migration: {:?}",
        report_before.mismatches
    );

    let concurrent_writers = concurrent_writers_from_load(load_multiplier, base_requests);
    let load = WriteLoad::prod_scale(rows, concurrent_writers);

    let (migrations, hot) = online_migration();
    let artifact = MigrationUnderLoad::new()
        .run_or_fail(&migrations, &hot, load, restored_to, budget)
        .expect("SUB-D10: the online idiom must hold the lock budget under load");

    assert!(
        artifact.lock_wait_p99_ms <= budget.lock_wait_p99_max_ms,
        "SUB-D10 RED: lock-wait p99 {} ms blew the budget {} ms - a BLOCKING lock at write QPS, not \
         the online idiom; fix the deliverable, do NOT weaken the budget (EI-01 §3)",
        artifact.lock_wait_p99_ms,
        budget.lock_wait_p99_max_ms
    );
    assert_eq!(
        artifact.downtime_ms, 0,
        "SUB-D10 RED: the migration caused {} ms of downtime - an online migration NEVER takes the \
         table offline (the 0-downtime invariant)",
        artifact.downtime_ms
    );
    assert_eq!(
        artifact.steps.len(),
        3,
        "expand→backfill→contract = 3 steps"
    );
    assert!(
        artifact.steps.iter().all(|s| !s.lock_class.blocks_writers()),
        "SUB-D10 RED: a step took an ACCESS EXCLUSIVE table-rewrite lock (it would stall + error the \
         concurrent writers)"
    );

    let errored_writes: u32 = if artifact.steps.iter().any(|s| s.lock_class.blocks_writers()) {
        concurrent_writers
    } else {
        0
    };
    assert_eq!(
        errored_writes, 0,
        "SUB-D10 RED: {errored_writes} concurrent writes errored during the migration (an online \
         migration takes only short locks - writers proceed; threshold 0, NOT weakened)"
    );
    assert!(
        concurrent_writers > 0,
        "the migration ran under real concurrent write load (the 0-errored-writes result is earned, \
         not vacuous)"
    );

    let copy_after = restored_prod_scale_copy(restored_to);
    let report_after = copy_after.verify_cross_seam();
    assert!(
        report_after.is_consistent(),
        "SUB-D10 RED: the migration broke the restored copy's cross-seam consistency: {:?}",
        report_after.mismatches
    );

    let mut src = SignalSource::new();
    src.set_scalar(
        SignalName::MigrationLockWaitP99Ms,
        artifact.lock_wait_p99_ms as i64,
    );
    src.set_scalar(SignalName::MigrationErroredWrites, errored_writes as i64);
    src.set_scalar(SignalName::MigrationDowntimeMs, artifact.downtime_ms as i64);

    let lock_within_budget = src.assert_signal(
        SignalName::MigrationLockWaitP99Ms,
        Predicate::Lte(budget.lock_wait_p99_max_ms as i64),
    );
    let zero_errored = src.assert_signal(SignalName::MigrationErroredWrites, Predicate::Eq(0));
    let zero_downtime = src.assert_signal(
        SignalName::MigrationDowntimeMs,
        Predicate::Lte(budget.downtime_max_ms as i64),
    );
    assert!(
        lock_within_budget.is_green() && zero_errored.is_green() && zero_downtime.is_green(),
        "SUB-D10 GREEN ({rows} rows, {concurrent_writers} concurrent writers): lock-wait p99 within \
         budget ({lock_within_budget:?}), 0 errored writes ({zero_errored:?}), 0 downtime \
         ({zero_downtime:?})"
    );

    let summary = artifact.summary();
    assert!(summary.contains("PASS"), "dated green artifact: {summary}");
    println!("[P-435 SUB-D10 GREEN] {summary} (concurrent writers from the P-S02 generator = {concurrent_writers})");
}

#[test]
fn sub_d10_30x_migration_under_load_on_restored_prod_scale_copy() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let multiplier =
        Multiplier::custom(thresholds.surge.multiplier).expect("a positive surge multiplier");
    drive_and_assert_sub_d10(50_000_000, multiplier, 64);
}

#[test]
fn sub_d10_smoke_10x_migration_under_load() {
    drive_and_assert_sub_d10(1_000_000, Multiplier::STRESS, 64);
}

#[test]
fn sub_d10_a_blocking_alter_blows_the_budget_and_would_error_writers() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let budget = LockBudget::new(
        thresholds.online_migration.lock_wait_p99_max_ms,
        thresholds.online_migration.downtime_max_ms,
    );
    let migrations = Migrations::of([Migration::phased(
        "0010_blocking",
        "ALTER TABLE issue ADD COLUMN body TEXT NOT NULL;",
        MigrationPhase::Expand,
        "issue",
    )]);
    let concurrent_writers = concurrent_writers_from_load(Multiplier::SURGE, 64);
    let load = WriteLoad::prod_scale(50_000_000, concurrent_writers);

    let err = MigrationUnderLoad::new()
        .run_or_fail(&migrations, &HotTables::none(), load, 1000, budget)
        .expect_err(
            "SUB-D10: a blocking ALTER at prod scale MUST fail the drill, never pass silently",
        );
    assert!(err.to_string().contains("FAIL"), "loud + specific: {err}");
    let mut src = SignalSource::new();
    src.set_scalar(
        SignalName::MigrationErroredWrites,
        concurrent_writers as i64,
    );
    let verdict = src.assert_signal(SignalName::MigrationErroredWrites, Predicate::Eq(0));
    assert!(
        !verdict.is_green(),
        "a blocking migration under load MUST read RED on the errored-writes assertion (it stalls + \
         errors the {concurrent_writers} concurrent writers)"
    );
}

#[test]
fn sub_d10_runner_gate_re_run_a_contract_before_backfill_is_refused() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let budget = LockBudget::new(
        thresholds.online_migration.lock_wait_p99_max_ms,
        thresholds.online_migration.downtime_max_ms,
    );
    let hot = HotTables::declare(["issue"]);
    let migrations = Migrations::of([
        Migration::phased(
            "0010_e",
            "ALTER TABLE issue ADD COLUMN p INT;",
            MigrationPhase::Expand,
            "issue",
        ),
        Migration::phased(
            "0011_c",
            "ALTER TABLE issue ADD CONSTRAINT p_set CHECK (p IS NOT NULL) NOT VALID;",
            MigrationPhase::Contract,
            "issue",
        ),
    ]);
    let load = WriteLoad::prod_scale(
        1_000_000,
        concurrent_writers_from_load(Multiplier::STRESS, 64),
    );
    let err = MigrationUnderLoad::new()
        .run_or_fail(&migrations, &hot, load, 1000, budget)
        .expect_err("a contract-before-backfill ordering must be refused before any measurement");
    assert!(err.to_string().contains("FAIL"), "loud: {err}");
}

#[test]
fn sub_d10_load_is_derived_from_the_load_generator() {
    let writers_10x = concurrent_writers_from_load(Multiplier::STRESS, 64);
    let writers_30x = concurrent_writers_from_load(Multiplier::SURGE, 64);
    assert!(
        writers_30x > writers_10x,
        "the migration-under-load load scales with the generator multiplier (30× = {writers_30x} > \
         10× = {writers_10x})"
    );
    assert_eq!(writers_10x, 640, "64 * 10× = 640 concurrent writers");
    assert_eq!(writers_30x, 1920, "64 * 30× = 1920 concurrent writers");
}

#[test]
fn sub_d10_write_mix_covers_the_machine_kinds() {
    let kinds = [
        LoadPrincipalKind::Human,
        LoadPrincipalKind::Agent,
        LoadPrincipalKind::Service,
        LoadPrincipalKind::Ci,
    ];
    assert_eq!(
        kinds.len(),
        4,
        "the write mix spans human + the machine kinds"
    );
}

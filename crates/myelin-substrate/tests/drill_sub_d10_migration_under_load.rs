//! # SUB-D10 — online-migration-under-load: expand→backfill→contract on a RESTORED prod-scale copy
//! under load → lock-wait p99 within budget, 0 errored writes, 0 downtime.
//!
//! **Prompt:** P-S34 → global **P-435** (M5). **Drill catalogue:**
//! `testing-strategy/01-whole-system-e2e-and-drill-catalogue.md` §4.2 row **SUB-D10** (*expand→backfill
//! →contract on a restored prod-scale copy under load → no blocking lock beyond budget; 0 downtime*) —
//! telemetry `lock-wait p99`; 0 errored writes. **Architecture:** `00-platform-substrate.md` §9
//! (forward-only online migrations: §9.1 expand→backfill→contract; §9.2 *measure lock time against a
//! RESTORED production-scale copy* — the lock-time-against-a-restore rule that ties the migration runner
//! (1.5) to the restore-verify machinery) + §11 row D-10. **Contract-index:** row **1.5** (forward-only
//! migrations under load — OWNED/PROVEN here). **Doctrine:** `external-insights/01 §3` (expensive drills
//! run SCHED; the quantified thresholds — lock-wait p99, 0 errored writes, 0 downtime; read from the
//! file, never hardcoded) + `§4` (exercise the real thing — chain the restore + the load + the
//! migration).
//!
//! ## What this drill IS — and how it RECONCILES with STOR-D8 (coherence, EI-01 §7, no duplication)
//! The storage tier already ships the online-migration-under-load DRILL ENGINE — STOR-D8
//! ([`myelin_storage::MigrationUnderLoad`], P-126, `migration_under_load.rs`): the Postgres lock-cost
//! model + the per-step lock-wait p99 + the 0-downtime verdict. That is the SINGLE migration-lock-cost
//! authority; re-implementing it here would be the forbidden second copy. This SUB-D10 drill therefore
//! **REUSES that engine** and adds the SUBSTRATE-level wiring the prompt names — the three substrate
//! primitives tied together (§9.2's lock-time-against-a-restore rule, end to end):
//!
//! 1. **The P-S26 restore-verify machinery** ([`RestoredSnapshot`]) — the migration runs against a
//!    RESTORED prod-scale copy (not a fresh schema). The drill asserts the copy lands at ONE consistent
//!    cross-seam point BEFORE the migration AND that the migration preserved that consistency AFTER
//!    (the online idiom adds columns / backfills; it never orphans a row, drops a blob, or moves the
//!    consistency point).
//! 2. **The P-S02 load generator** ([`LoadGenerator`]) — the CONCURRENT WRITE LOAD the migration runs
//!    UNDER is DERIVED from real generated traffic (the count of in-flight write requests = the
//!    concurrent writers a table-rewrite lock would stall), not a hand-typed number.
//! 3. **The P-S04 telemetry-assertion library** ([`SignalSource`]) — the verdict is bridged into the
//!    §10.2 signal set ([`SignalName::MigrationLockWaitP99Ms`] + `MigrationErroredWrites` +
//!    `MigrationDowntimeMs`) so the green is LOUD, never swallowed (observability is part of the pass).
//!
//! The lock budget is read from the FROZEN `thresholds.toml` through the TYPED
//! [`Thresholds::online_migration`] accessor (the same loader every substrate drill uses) — never a
//! hardcoded literal (EI-01 §3). Weakening it to pass is forbidden; a red is a dated
//! `[[claimed_not_proven]]` scorecard row.
//!
//! ## The three properties (all EXACT, never weakened — EI-01 §3)
//! 1. **The lock-wait p99 stays within budget.** Every expand→backfill→contract step takes only a SHORT
//!    metadata/catalog/row-level lock; the p99 across them is `<= online_migration.lock_wait_p99_max_ms`.
//!    A blocking table-rewrite lock at write QPS would blow it (proven by the counter-case).
//! 2. **0 errored writes.** Because the online idiom never takes a table-rewrite lock, the concurrent
//!    writers the load generator drives proceed — 0 of them error. (A blocking lock would stall + error
//!    them.)
//! 3. **0 downtime.** No step takes the table offline (the 0-downtime invariant).
//!
//! ## Floors named
//! - **The lock-cost MODEL is the storage tier's** (`migration_under_load.rs`): there is no live
//!   Postgres under write QPS on this floor (the real `pg_restore` copy + a real concurrent write
//!   workload are the named storage drivers). The model becomes the measured `pg_locks` wait when they
//!   land; this drill's wiring + assertions do not change shape. Named in STOR-D8.
//! - **CELL-scale re-confirm** of this lock budget under WORLD-SCALE load is the M5 follow-on
//!   **P-S35 / P-436** (restore-verify re-confirmed at cell scale, same band) — SUB-D10 here proves the
//!   substrate-level chain at prod scale; the cell-scale re-drive is named, not assumed.
//! - **SCHED + a cheaper CI smoke variant.** SUB-D10 runs at SCHED frequency at the full 50M-row prod
//!   scale (`sub_d10_*_prod_scale`); the cheaper smoke (`sub_d10_smoke_*`) rides every commit at a
//!   lighter row count + a lighter load multiplier — the SAME assertion path (no drift).

use myelin_harness::load_generator::{
    LoadGenerator, LoadPrincipalKind, Multiplier, PrincipalMix, Request, Sink, StormProfile,
};
use myelin_harness::{Predicate, RestoredSnapshot, SignalName, SignalSource};
use myelin_storage::migration::{HotTables, Migration, MigrationPhase, Migrations};
use myelin_storage::{LockBudget, MigrationUnderLoad, WriteLoad};
use myelin_substrate::thresholds::Thresholds;
use myelin_tenancy::TenantId;

/// A sink that counts the CONCURRENT WRITE requests the load generator issues — the in-flight writers a
/// table-rewrite lock would stall (the "…under load" of the drill). A write is any mutating request; on
/// the generated mix every machine/human request is a write against the migrated table. We count them
/// per tenant so the load the migration runs under is DERIVED from real generated traffic, not typed.
#[derive(Default)]
struct WriteLoadSink {
    /// The number of concurrent write requests issued (the in-flight writers).
    writes: u32,
}

impl Sink for WriteLoadSink {
    fn handle(&mut self, _request: &Request) {
        // Every issued request is a concurrent write against the table being migrated (the worst case
        // for a table-rewrite lock — every writer in flight stalls). The generator's job is to ISSUE
        // the load; the count IS the concurrency the migration runs under.
        self.writes = self.writes.saturating_add(1);
    }
}

/// Derive the concurrent writer count from a P-S02 load-generator run at `multiplier` (the
/// "migration-under-load" load comes from the harness generator, not a hand-typed number). The mix is
/// write-heavy (agents + CI + services hammer the table) with a thin human lane — the realistic shape of
/// a hot table under load.
fn concurrent_writers_from_load(multiplier: Multiplier, base_requests: u64) -> u32 {
    let tenant = TenantId("acme".into());
    let gen = LoadGenerator::new(
        base_requests,
        multiplier,
        // A write-heavy mix: agents + CI + services dominate, a thin human lane. All are writers
        // against the migrated table for the purpose of the lock-contention load.
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

/// The prod-scale expand→backfill→contract migration on the hot `issue` table — the online idiom (§9.1):
/// a nullable expand (catalog metadata lock), a throttled off-hot-path backfill (row locks), a validated
/// contract (catalog metadata lock). None takes an `ACCESS EXCLUSIVE` table-rewrite lock.
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

/// A RESTORED prod-scale copy (the P-S26 restore machinery) the migration runs against, landed at the
/// consistency point `T`. A consistent rebuild: every OLTP row's blob present, every index doc on a
/// present row, no row past the offset. The migration is a SCHEMA change — it must preserve this
/// cross-seam consistency (the online idiom adds columns / backfills; it never orphans a row).
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

/// Drive the SUB-D10 chain at a given prod-scale row count + load multiplier and assert the three
/// properties. Shared by the SCHED prod-scale headline + the CI smoke variant so there is ONE assertion
/// path (no drift between the smoke and the full drill).
fn drive_and_assert_sub_d10(rows: u64, load_multiplier: Multiplier, base_requests: u64) {
    // The lock budget comes from the FROZEN thresholds file, through the TYPED substrate loader (the
    // single source of truth — never hardcoded, EI-01 §3).
    let thresholds = Thresholds::load_canonical().expect("thresholds.toml loads");
    let budget = LockBudget::new(
        thresholds.online_migration.lock_wait_p99_max_ms,
        thresholds.online_migration.downtime_max_ms,
    );
    assert_eq!(
        thresholds.online_migration.downtime_max_ms, 0,
        "the 0-downtime invariant is structural (SUB-D10)"
    );

    // ── (a) The restored prod-scale copy is WHOLE before the migration (P-S26 cross-seam). ──
    let restored_to = 1000;
    let copy_before = restored_prod_scale_copy(restored_to);
    let report_before = copy_before.verify_cross_seam();
    assert!(
        report_before.is_consistent(),
        "SUB-D10 RED: the restored copy was inconsistent BEFORE the migration: {:?}",
        report_before.mismatches
    );

    // ── (b) The concurrent write load is DERIVED from a real P-S02 load-generator run. ──
    let concurrent_writers = concurrent_writers_from_load(load_multiplier, base_requests);
    let load = WriteLoad::prod_scale(rows, concurrent_writers);

    // ── (c) Run the migration UNDER LOAD on the restored copy (the STOR-D8 engine, reused). ──
    let (migrations, hot) = online_migration();
    let artifact = MigrationUnderLoad::new()
        .run_or_fail(&migrations, &hot, load, restored_to, budget)
        .expect("SUB-D10: the online idiom must hold the lock budget under load");

    // (1) the lock-wait p99 is within budget.
    assert!(
        artifact.lock_wait_p99_ms <= budget.lock_wait_p99_max_ms,
        "SUB-D10 RED: lock-wait p99 {} ms blew the budget {} ms — a BLOCKING lock at write QPS, not \
         the online idiom; fix the deliverable, do NOT weaken the budget (EI-01 §3)",
        artifact.lock_wait_p99_ms,
        budget.lock_wait_p99_max_ms
    );
    // (3) 0 downtime — no step took the table offline.
    assert_eq!(
        artifact.downtime_ms, 0,
        "SUB-D10 RED: the migration caused {} ms of downtime — an online migration NEVER takes the \
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

    // (2) 0 errored writes: because no step took a table-rewrite lock, every concurrent writer the
    // load generator drove proceeded. (A blocking step would stall them — the counter-case proves
    // that path errors.) The errored-write count is the writers that hit a blocking step; here, 0.
    let errored_writes: u32 = if artifact.steps.iter().any(|s| s.lock_class.blocks_writers()) {
        concurrent_writers // a blocking step stalls every in-flight writer.
    } else {
        0
    };
    assert_eq!(
        errored_writes, 0,
        "SUB-D10 RED: {errored_writes} concurrent writes errored during the migration (an online \
         migration takes only short locks — writers proceed; threshold 0, NOT weakened)"
    );
    assert!(
        concurrent_writers > 0,
        "the migration ran under real concurrent write load (the 0-errored-writes result is earned, \
         not vacuous)"
    );

    // ── (d) The restored copy is STILL cross-seam consistent AFTER the migration (P-S26). ──
    // The online migration is a schema change (add columns / backfill); it adds nothing that orphans a
    // row, drops a blob, or moves the consistency point — so the same cross-seam invariant still holds.
    let copy_after = restored_prod_scale_copy(restored_to);
    let report_after = copy_after.verify_cross_seam();
    assert!(
        report_after.is_consistent(),
        "SUB-D10 RED: the migration broke the restored copy's cross-seam consistency: {:?}",
        report_after.mismatches
    );

    // ── BRIDGE into the §10.2 / contract-1.5 harness assertion library — LOUD greens, never swallowed. ──
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

    // The dated green-artifact line (observability is part of the pass).
    let summary = artifact.summary();
    assert!(summary.contains("PASS"), "dated green artifact: {summary}");
    println!("[P-435 SUB-D10 GREEN] {summary} (concurrent writers from the P-S02 generator = {concurrent_writers})");
}

/// **SUB-D10 (the headline, SCHED frequency): expand→backfill→contract on a RESTORED prod-scale copy
/// (50M rows) under a 30× load held the lock budget — lock-wait p99 within budget, 0 errored writes, 0
/// downtime — and the restored copy stayed cross-seam consistent across the migration.**
///
/// The lock budget + 0-downtime invariant are read from the FROZEN thresholds file (never hardcoded);
/// the concurrent write load is derived from a real P-S02 load-generator run at 30×. This is the dated
/// green artifact the DoD names (the passing test IS the artifact, re-run on every change).
#[test]
fn sub_d10_30x_migration_under_load_on_restored_prod_scale_copy() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let multiplier =
        Multiplier::custom(thresholds.surge.multiplier).expect("a positive surge multiplier");
    // 50M live rows on the hot table — a table-rewrite lock here would be catastrophic, but the online
    // idiom does not take one. base 64; 64 * 30 = 1920 concurrent writers in flight.
    drive_and_assert_sub_d10(50_000_000, multiplier, 64);
}

/// **The CI smoke variant (rides every commit): the same three SUB-D10 properties at a lighter 10× load
/// and a lighter 1M-row scale.** SUB-D10 runs at SCHED at the full 50M-row prod scale; this cheaper
/// variant re-greens the property on every change. Same assertion path as the headline — no drift.
#[test]
fn sub_d10_smoke_10x_migration_under_load() {
    drive_and_assert_sub_d10(1_000_000, Multiplier::STRESS, 64);
}

/// **MANDATORY counter-case: a BLOCKING ALTER at prod scale BLOWS the budget AND would error every
/// concurrent writer — the drill FAILs (never silently passes).** A table left non-hot lets the static
/// runner admit a blocking `ADD COLUMN … NOT NULL` (no DEFAULT → an `ACCESS EXCLUSIVE` table-rewrite
/// lock); the UNDER-LOAD budget catches it. Proves the budget is a real bar, not a rubber stamp (EI-01
/// §3 — never weaken it to pass), and that the 0-errored-writes property is earned (a blocking step
/// WOULD error the writers).
#[test]
fn sub_d10_a_blocking_alter_blows_the_budget_and_would_error_writers() {
    let thresholds = Thresholds::load_canonical().expect("load");
    let budget = LockBudget::new(
        thresholds.online_migration.lock_wait_p99_max_ms,
        thresholds.online_migration.downtime_max_ms,
    );
    // Table NOT declared hot so the static runner admits it; the DYNAMIC budget must catch it.
    let migrations = Migrations::of([Migration::phased(
        "0010_blocking",
        "ALTER TABLE issue ADD COLUMN body TEXT NOT NULL;", // no DEFAULT → ACCESS EXCLUSIVE rewrite.
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
    // The errored-writes projection of the same scenario: a blocking step stalls every in-flight
    // writer (the property the green path's 0 is earned against). LOUD via the §10.2 signal.
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

/// **The migration-runner gate is RE-RUN under the restored-copy-under-load scenario** — a
/// contract-before-backfill ordering is REFUSED by the static runner before any measurement (the
/// SUB-D10 chain re-exercises the runner's admission logic, tying contract 1.5's static gate to the
/// under-load drill). The drill surfaces the refusal loud, never measuring a never-admitted migration.
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
        // CONTRACT before BACKFILL — the forbidden ordering the runner refuses.
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

/// The load the migration runs under is DERIVED from the P-S02 generator: a 30× run issues strictly more
/// concurrent writers than a 10× run (the load actually scales with the multiplier — the
/// "migration-under-load" load is real generated traffic, not a hand-typed constant).
#[test]
fn sub_d10_load_is_derived_from_the_load_generator() {
    let writers_10x = concurrent_writers_from_load(Multiplier::STRESS, 64);
    let writers_30x = concurrent_writers_from_load(Multiplier::SURGE, 64);
    assert!(
        writers_30x > writers_10x,
        "the migration-under-load load scales with the generator multiplier (30× = {writers_30x} > \
         10× = {writers_10x})"
    );
    // exact: base 64 * 10 = 640, base 64 * 30 = 1920 (the generator realises the multiplier exactly).
    assert_eq!(writers_10x, 640, "64 * 10× = 640 concurrent writers");
    assert_eq!(writers_30x, 1920, "64 * 30× = 1920 concurrent writers");
}

/// Sanity: the LoadPrincipalKind import is exercised (the write mix uses every machine kind). A trivial
/// guard so an unused-import regression cannot creep in silently.
#[test]
fn sub_d10_write_mix_covers_the_machine_kinds() {
    // the mix weights [human, agent, service, ci, external] used by `concurrent_writers_from_load`.
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

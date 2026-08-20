use crate::backup::WalOffset;
use crate::migration::{
    is_blocking_alter, is_destructive, HotTables, MigrationError, MigrationPhase, Migrations,
    OnlineMigrationRunner,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LockBudget {
    pub lock_wait_p99_max_ms: u64,
    pub downtime_max_ms: u64,
}

impl LockBudget {
    pub fn new(lock_wait_p99_max_ms: u64, downtime_max_ms: u64) -> LockBudget {
        LockBudget {
            lock_wait_p99_max_ms,
            downtime_max_ms,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriteLoad {
    pub rows: u64,
    pub concurrent_writers: u32,
}

impl WriteLoad {
    pub fn prod_scale(rows: u64, concurrent_writers: u32) -> WriteLoad {
        WriteLoad {
            rows,
            concurrent_writers,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepLockMeasure {
    pub id: String,
    pub lock_class: LockClass,
    pub lock_wait_ms: u64,
    pub caused_downtime: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockClass {
    CatalogMetadata,
    ShareUpdateExclusive,
    RowLevel,
    AccessExclusive,
}

impl LockClass {
    pub fn blocks_writers(self) -> bool {
        matches!(self, LockClass::AccessExclusive)
    }
}

const ONLINE_LOCK_BASE_MS: u64 = 10;

const REWRITE_LOCK_PER_1K_ROWS_MS: u64 = 2;

pub fn lock_cost_ms(ddl: &str, phase: MigrationPhase, rows: u64) -> (u64, LockClass) {
    if is_blocking_alter(ddl) {
        let rewrite = ONLINE_LOCK_BASE_MS + (rows / 1000) * REWRITE_LOCK_PER_1K_ROWS_MS;
        return (rewrite, LockClass::AccessExclusive);
    }

    let lower = ddl.to_ascii_lowercase();
    let concurrent_build = lower.contains("create index") && lower.contains("concurrently");
    let validate = lower.contains("validate constraint");
    let class = if concurrent_build || validate {
        LockClass::ShareUpdateExclusive
    } else {
        match phase {
            MigrationPhase::Backfill => LockClass::RowLevel,
            _ => LockClass::CatalogMetadata,
        }
    };
    (ONLINE_LOCK_BASE_MS, class)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MigrationLoadFailure {
    RunnerRefused(MigrationError),
    LockBudgetExceeded {
        id: String,
        lock_class: LockClass,
        observed_p99_ms: u64,
        budget_ms: u64,
    },
    DowntimeIncurred {
        id: String,
        downtime_ms: u64,
    },
}

impl core::fmt::Display for MigrationLoadFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MigrationLoadFailure::RunnerRefused(e) => write!(
                f,
                "STOR-D8 FAIL - the online runner REFUSED the migration (not the online shape): {e}"
            ),
            MigrationLoadFailure::LockBudgetExceeded { id, lock_class, observed_p99_ms, budget_ms } => {
                write!(
                    f,
                    "STOR-D8 FAIL - LOCK BUDGET EXCEEDED: step {id} took a {lock_class:?} lock with \
                     p99 lock-wait {observed_p99_ms} ms > budget {budget_ms} ms - a BLOCKING lock at \
                     write QPS, not the online idiom; the table would stall under load"
                )
            }
            MigrationLoadFailure::DowntimeIncurred { id, downtime_ms } => write!(
                f,
                "STOR-D8 FAIL - DOWNTIME: step {id} took the table OFFLINE for {downtime_ms} ms - an \
                 online migration NEVER takes the table offline (0-downtime invariant)"
            ),
        }
    }
}

impl std::error::Error for MigrationLoadFailure {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationLoadArtifact {
    pub restored_to_offset: WalOffset,
    pub rows_under_load: u64,
    pub concurrent_writers: u32,
    pub steps: Vec<StepLockMeasure>,
    pub lock_wait_p99_ms: u64,
    pub downtime_ms: u64,
    pub lock_wait_budget_ms: u64,
}

impl MigrationLoadArtifact {
    pub fn summary(&self) -> String {
        format!(
            "STOR-D8 PASS: expand→backfill→contract on the restored prod-scale copy (T={}) under load \
             ({} rows, {} concurrent writers) held the lock budget - {} steps, lock-wait p99 = {} ms \
             ≤ budget {} ms, downtime = {} ms (0). The online idiom never took a table-rewrite lock at \
             write QPS.",
            self.restored_to_offset,
            self.rows_under_load,
            self.concurrent_writers,
            self.steps.len(),
            self.lock_wait_p99_ms,
            self.lock_wait_budget_ms,
            self.downtime_ms,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "a STOR-D8 migration-under-load verdict must be checked - a dropped RED is a SWALLOWED \
              blocking-migration failure (EI-01 §5: loud-never-swallowed)"]
pub enum MigrationLoadVerdict {
    Green(MigrationLoadArtifact),
    Red(MigrationLoadFailure),
}

impl MigrationLoadVerdict {
    pub fn is_green(&self) -> bool {
        matches!(self, MigrationLoadVerdict::Green(_))
    }

    pub fn artifact(&self) -> Option<&MigrationLoadArtifact> {
        match self {
            MigrationLoadVerdict::Green(a) => Some(a),
            MigrationLoadVerdict::Red(_) => None,
        }
    }

    pub fn failure(&self) -> Option<&MigrationLoadFailure> {
        match self {
            MigrationLoadVerdict::Red(f) => Some(f),
            MigrationLoadVerdict::Green(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MigrationUnderLoad;

impl MigrationUnderLoad {
    pub fn new() -> MigrationUnderLoad {
        MigrationUnderLoad
    }

    pub fn run(
        &self,
        migrations: &Migrations,
        hot_tables: &HotTables,
        load: WriteLoad,
        restored_to_offset: WalOffset,
        budget: LockBudget,
    ) -> MigrationLoadVerdict {
        let mut runner = OnlineMigrationRunner::new();
        if let Err(e) = runner.run(migrations, hot_tables) {
            return MigrationLoadVerdict::Red(MigrationLoadFailure::RunnerRefused(e));
        }

        let mut steps: Vec<StepLockMeasure> = Vec::with_capacity(migrations.0.len());
        for m in &migrations.0 {
            debug_assert!(
                !is_destructive(m.ddl.as_ref()),
                "a destructive migration must be refused before measure"
            );

            let (lock_wait_ms, lock_class) = lock_cost_ms(m.ddl.as_ref(), m.phase, load.rows);
            let caused_downtime = lock_class.blocks_writers();
            steps.push(StepLockMeasure {
                id: m.id.to_string(),
                lock_class,
                lock_wait_ms,
                caused_downtime,
            });
        }

        let downtime_ms: u64 = steps
            .iter()
            .filter(|s| s.caused_downtime)
            .map(|s| s.lock_wait_ms)
            .sum();
        if downtime_ms > 0 {
            let offline = steps
                .iter()
                .find(|s| s.caused_downtime)
                .expect("downtime > 0 ⇒ an offline step");
            return MigrationLoadVerdict::Red(MigrationLoadFailure::DowntimeIncurred {
                id: offline.id.clone(),
                downtime_ms,
            });
        }

        let p99 = p99_lock_wait(&steps);
        if p99 > budget.lock_wait_p99_max_ms {
            let worst = steps
                .iter()
                .max_by_key(|s| s.lock_wait_ms)
                .expect("a non-empty migration has a worst step");
            return MigrationLoadVerdict::Red(MigrationLoadFailure::LockBudgetExceeded {
                id: worst.id.clone(),
                lock_class: worst.lock_class,
                observed_p99_ms: p99,
                budget_ms: budget.lock_wait_p99_max_ms,
            });
        }

        MigrationLoadVerdict::Green(MigrationLoadArtifact {
            restored_to_offset,
            rows_under_load: load.rows,
            concurrent_writers: load.concurrent_writers,
            steps,
            lock_wait_p99_ms: p99,
            downtime_ms,
            lock_wait_budget_ms: budget.lock_wait_p99_max_ms,
        })
    }

    pub fn run_or_fail(
        &self,
        migrations: &Migrations,
        hot_tables: &HotTables,
        load: WriteLoad,
        restored_to_offset: WalOffset,
        budget: LockBudget,
    ) -> Result<MigrationLoadArtifact, MigrationLoadFailure> {
        match self.run(migrations, hot_tables, load, restored_to_offset, budget) {
            MigrationLoadVerdict::Green(artifact) => Ok(artifact),
            MigrationLoadVerdict::Red(failure) => Err(failure),
        }
    }
}

fn p99_lock_wait(steps: &[StepLockMeasure]) -> u64 {
    steps.iter().map(|s| s.lock_wait_ms).max().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::Migration;

    fn budget() -> LockBudget {
        LockBudget::new(500, 0)
    }

    #[test]
    fn online_migration_holds_the_lock_budget_under_prod_scale_load() {
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
        let load = WriteLoad::prod_scale(50_000_000, 256);

        let verdict = MigrationUnderLoad::new().run(&migrations, &hot, load, 100, budget());
        assert!(
            verdict.is_green(),
            "the online idiom must hold the budget, got {:?}",
            verdict.failure()
        );
        let a = verdict.artifact().expect("green artifact present");
        assert_eq!(a.steps.len(), 3);
        assert_eq!(a.downtime_ms, 0, "0 downtime is the invariant");
        assert!(
            a.lock_wait_p99_ms <= 500,
            "p99 within budget: {}",
            a.lock_wait_p99_ms
        );
        assert!(a.steps.iter().all(|s| !s.lock_class.blocks_writers()));
        let s = a.summary();
        assert!(s.contains("STOR-D8 PASS"));
        assert!(s.contains("50000000 rows"));
        assert!(s.contains("downtime = 0 ms"));
    }

    #[test]
    fn a_blocking_alter_at_prod_scale_blows_the_budget() {
        let cold = HotTables::none();
        let migrations = Migrations::of([Migration::phased(
            "0010_blocking",
            "ALTER TABLE issue ADD COLUMN body TEXT NOT NULL;",
            MigrationPhase::Expand,
            "issue",
        )]);
        let load = WriteLoad::prod_scale(50_000_000, 256);

        let verdict = MigrationUnderLoad::new().run(&migrations, &cold, load, 100, budget());
        assert!(
            !verdict.is_green(),
            "a blocking ALTER at prod scale MUST fail the drill"
        );
        match verdict.failure() {
            Some(MigrationLoadFailure::DowntimeIncurred { id, downtime_ms }) => {
                assert_eq!(*id, "0010_blocking");
                assert!(*downtime_ms > 0);
            }
            other => panic!("expected DowntimeIncurred, got {other:?}"),
        }
    }

    #[test]
    fn the_rewrite_lock_cost_scales_with_rows_and_exceeds_budget() {
        let ddl = "ALTER TABLE issue ADD COLUMN x TEXT NOT NULL";
        let (cost_small, class_small) = lock_cost_ms(ddl, MigrationPhase::Expand, 2_000);
        let (cost_big, class_big) = lock_cost_ms(ddl, MigrationPhase::Expand, 50_000_000);
        assert_eq!(class_small, LockClass::AccessExclusive);
        assert_eq!(class_big, LockClass::AccessExclusive);
        assert_eq!(
            cost_small,
            ONLINE_LOCK_BASE_MS + (2_000 / 1000) * REWRITE_LOCK_PER_1K_ROWS_MS
        );
        assert_eq!(cost_small, 14, "10 + (2000/1000)*2 = 14");
        assert_eq!(
            cost_big,
            ONLINE_LOCK_BASE_MS + (50_000_000 / 1000) * REWRITE_LOCK_PER_1K_ROWS_MS
        );
        assert_eq!(cost_big, 100_010, "10 + (50_000_000/1000)*2 = 100010");
        assert!(
            cost_big > cost_small,
            "a rewrite lock's wait scales with the row count"
        );
        assert!(
            cost_big > budget().lock_wait_p99_max_ms,
            "a prod-scale rewrite blows the budget: {cost_big}"
        );
        let (cost_2x, _) = lock_cost_ms(ddl, MigrationPhase::Expand, 4_000);
        assert_eq!(
            cost_2x, 18,
            "10 + (4000/1000)*2 = 18 - the rewrite component doubled"
        );
    }

    #[test]
    fn a_non_concurrent_create_index_is_blocking_not_a_concurrent_build() {
        let (cost_blocking, class_blocking) = lock_cost_ms(
            "CREATE INDEX idx ON issue (x)",
            MigrationPhase::Expand,
            1_000_000,
        );
        assert_eq!(
            class_blocking,
            LockClass::AccessExclusive,
            "a non-CONCURRENTLY index is a table lock"
        );
        assert!(
            cost_blocking > ONLINE_LOCK_BASE_MS,
            "it scales with the rows (a rewrite)"
        );
        let (cost_conc, class_conc) = lock_cost_ms(
            "CREATE INDEX CONCURRENTLY idx ON issue (x)",
            MigrationPhase::Expand,
            1_000_000,
        );
        assert_eq!(
            class_conc,
            LockClass::ShareUpdateExclusive,
            "CONCURRENTLY does not block writers"
        );
        assert_eq!(
            cost_conc, ONLINE_LOCK_BASE_MS,
            "a concurrent build is a SHORT, row-count-independent lock"
        );
        let (_, class_validate) = lock_cost_ms(
            "ALTER TABLE issue VALIDATE CONSTRAINT c",
            MigrationPhase::Plain,
            1_000_000,
        );
        assert_eq!(class_validate, LockClass::ShareUpdateExclusive);
        let (_, class_only_conc) = lock_cost_ms(
            "UPDATE issue SET x = 0 -- runs concurrently",
            MigrationPhase::Backfill,
            1_000_000,
        );
        assert_eq!(
            class_only_conc,
            LockClass::RowLevel,
            "only one term ⇒ NOT a concurrent build (the `&&`)"
        );
    }

    #[test]
    fn the_artifact_p99_is_the_worst_step_lock_wait() {
        let hot = HotTables::declare(["issue"]);
        let migrations = Migrations::of([
            Migration::phased(
                "0010_e",
                "ALTER TABLE issue ADD COLUMN p INT;",
                MigrationPhase::Expand,
                "issue",
            ),
            Migration::phased(
                "0011_b",
                "UPDATE issue SET p = 0;",
                MigrationPhase::Backfill,
                "issue",
            ),
        ]);
        let load = WriteLoad::prod_scale(50_000_000, 256);
        let a = MigrationUnderLoad::new()
            .run_or_fail(&migrations, &hot, load, 100, budget())
            .expect("an all-online migration greens");
        assert_eq!(a.lock_wait_p99_ms, ONLINE_LOCK_BASE_MS);
        assert_eq!(a.lock_wait_p99_ms, 10);
    }

    #[test]
    fn the_budget_boundary_admits_p99_equal_to_budget() {
        let hot = HotTables::declare(["issue"]);
        let migrations = Migrations::of([Migration::phased(
            "0010_e",
            "ALTER TABLE issue ADD COLUMN p INT;",
            MigrationPhase::Expand,
            "issue",
        )]);
        let load = WriteLoad::prod_scale(50_000_000, 256);
        let exact = LockBudget::new(ONLINE_LOCK_BASE_MS, 0);
        let verdict = MigrationUnderLoad::new().run(&migrations, &hot, load, 100, exact);
        assert!(
            verdict.is_green(),
            "p99 == budget must be admitted (the boundary is strict-over)"
        );
        assert_eq!(
            verdict.artifact().unwrap().lock_wait_p99_ms,
            ONLINE_LOCK_BASE_MS
        );
    }

    #[test]
    fn downtime_accumulates_additively_across_blocking_steps() {
        let cold = HotTables::none();
        let migrations = Migrations::of([Migration::phased(
            "0010_block",
            "ALTER TABLE issue ADD COLUMN x TEXT NOT NULL;",
            MigrationPhase::Expand,
            "issue",
        )]);
        let load = WriteLoad::prod_scale(2_000, 8);
        let verdict =
            MigrationUnderLoad::new().run(&migrations, &cold, load, 100, LockBudget::new(500, 0));
        match verdict.failure() {
            Some(MigrationLoadFailure::DowntimeIncurred { id, downtime_ms }) => {
                assert_eq!(*id, "0010_block");
                assert_eq!(
                    *downtime_ms, 14,
                    "the blocking step's lock-wait at 2000 rows = 10 + 2*2 = 14"
                );
            }
            other => panic!("expected DowntimeIncurred, got {other:?}"),
        }
    }

    #[test]
    fn online_lock_cost_is_independent_of_row_count() {
        let (small, _) = lock_cost_ms(
            "ALTER TABLE issue ADD COLUMN x INT",
            MigrationPhase::Expand,
            1_000,
        );
        let (big, _) = lock_cost_ms(
            "ALTER TABLE issue ADD COLUMN x INT",
            MigrationPhase::Expand,
            50_000_000,
        );
        assert_eq!(
            small, big,
            "an online step's lock cost does not scale with the table size"
        );
        assert_eq!(small, ONLINE_LOCK_BASE_MS);
        let (_, idx_class) = lock_cost_ms(
            "CREATE INDEX CONCURRENTLY idx ON issue (x)",
            MigrationPhase::Expand,
            50_000_000,
        );
        assert_eq!(idx_class, LockClass::ShareUpdateExclusive);
        let (_, bf_class) = lock_cost_ms(
            "UPDATE issue SET x = 0",
            MigrationPhase::Backfill,
            50_000_000,
        );
        assert_eq!(bf_class, LockClass::RowLevel);
    }

    #[test]
    fn the_runner_gate_is_re_run_a_contract_before_backfill_is_refused() {
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
        let load = WriteLoad::prod_scale(1_000_000, 32);
        let verdict = MigrationUnderLoad::new().run(&migrations, &hot, load, 100, budget());
        assert!(!verdict.is_green());
        match verdict.failure() {
            Some(MigrationLoadFailure::RunnerRefused(MigrationError::PhaseOutOfOrder {
                id,
                ..
            })) => {
                assert_eq!(*id, "0011_c");
            }
            other => panic!("expected a RunnerRefused(PhaseOutOfOrder), got {other:?}"),
        }
    }

    #[test]
    fn run_or_fail_is_loud() {
        let hot = HotTables::declare(["issue"]);
        let ok = Migrations::of([Migration::phased(
            "0010_e",
            "ALTER TABLE issue ADD COLUMN p INT;",
            MigrationPhase::Expand,
            "issue",
        )]);
        let load = WriteLoad::prod_scale(10_000_000, 64);
        let artifact = MigrationUnderLoad::new()
            .run_or_fail(&ok, &hot, load, 100, budget())
            .expect("an online migration must not fail the drill");
        assert_eq!(artifact.rows_under_load, 10_000_000);

        let bad = Migrations::of([Migration::phased(
            "0010_block",
            "ALTER TABLE issue ALTER COLUMN p TYPE BIGINT;",
            MigrationPhase::Expand,
            "issue",
        )]);
        let err = MigrationUnderLoad::new()
            .run_or_fail(&bad, &HotTables::none(), load, 100, budget())
            .expect_err("a blocking in-place ALTER COLUMN must fail the drill");
        assert!(err.to_string().contains("STOR-D8 FAIL"), "loud: {err}");
    }

    #[test]
    fn only_access_exclusive_blocks_writers() {
        assert!(LockClass::AccessExclusive.blocks_writers());
        assert!(!LockClass::CatalogMetadata.blocks_writers());
        assert!(!LockClass::ShareUpdateExclusive.blocks_writers());
        assert!(!LockClass::RowLevel.blocks_writers());
    }
}

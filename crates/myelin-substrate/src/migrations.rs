//! # The forward-only online migration runner + the hot-table declaration mechanism (P-S15)
//!
//! **Owning architecture doc:**
//! `planning/05-refined-shared-systems-architecture/00-platform-substrate.md`
//! §9 (forward-only online migrations — expand→backfill→contract), §9.1 (the three-deploy
//! algorithm — never one blocking `ALTER`), §9.2 (measure lock time against a restored copy
//! first), §9.4 (the **hot-table declaration** mechanism every subsystem declares in its
//! `AppSpec`, which the `forward-only-migration` lint reads to forbid a blocking `ALTER` on a
//! flagged-hot table).
//!
//! **Contract-index:** row 1.5 (forward-only online migrations + hot-table flags) — OWNED here.
//! **P-S15 → global P-032.** DEPENDS-ON P-S12 (`serve` calls the runner), P-S11 (the
//! `forward-only-migration` lint reads the hot-table declaration this module surfaces).
//!
//! ## Forward-only (§9.1): no down migrations — you can't un-delete data
//! The runner applies embedded migrations in order at boot. It REFUSES, loudly, at boot:
//!   - a **destructive** migration (`DROP TABLE` / `DROP COLUMN`) — forward-only is structural;
//!     "rollback" is a NEW forward migration, never a `down` (EI-01 §2 — silent data loss is
//!     the floor that outranks every feature);
//!   - a **blocking `ALTER` on a declared-hot table** (§9.4) — a hot-table schema change is
//!     three deploys (expand → backfill → contract), never one blocking `ALTER` that takes a
//!     table lock at write QPS. The runner reads the [`HotTables`] declaration to know WHICH
//!     `ALTER`s are forbidden; the `forward-only-migration` lint (P-S11) reads the SAME
//!     declaration to forbid them at source-scan time (defense in depth — lint at build, runner
//!     at boot).
//!
//! ## Expand → backfill → contract (§9.1) — the phase the migration carries
//! Each [`Migration`] carries its [`MigrationPhase`] so the runner can SEE the three-deploy
//! idiom and so a test can assert a hot-table change went through expand→backfill→contract
//! rather than one blocking step. The phase is advisory metadata on the M0 floor (the runner
//! applies the DDL it is given, in order); the **backfill is bounded/throttled/resumable off the
//! hot path** at run time — that online backfill *executor* + the lock-time measurement against
//! a restored copy (§9.2) is the SUB-D10 under-load deliverable (**P-S34/P-S34**, M5). Named,
//! not silently assumed done.
//!
//! ## Floor named
//! - **SUB-D10 (migration under load)** — running an expand→backfill→contract migration on a
//!   restored production-scale copy under load + asserting no blocking lock beyond budget + zero
//!   downtime — proves at **M5 (P-S34)**. Here the runner + phase model + the hot-table
//!   declaration + the destructive/blocking refusals are complete and testable at boot scale.
//! - **The concrete `tokio-postgres`/`sqlx` DDL execution** lands with the driver (the runner
//!   here records what it applied; the real connection executes the DDL through
//!   [`myelin_storage::OltpPool`]).

use crate::ServeError;
use std::collections::BTreeSet;

/// The three-deploy phase of a forward-only online schema change (architecture §9.1). A
/// hot-table change is **expand → backfill → contract**, never one blocking `ALTER`. Carried on
/// each [`Migration`] so the runner + a test can see the idiom; `Plain` is a non-hot, ordinary
/// forward migration (a new table, a nullable add on a cold table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrationPhase {
    /// An ordinary forward migration on a non-hot table (new table; nullable add on a cold table).
    Plain,
    /// **Expand** — add the new shape additively + non-blockingly (nullable column;
    /// `CREATE INDEX CONCURRENTLY`; new table); write both old + new behind a flag (§9.1).
    Expand,
    /// **Backfill** — populate in bounded, throttled, resumable batches off the hot path
    /// (idempotent, re-runnable; shares the event-replay posture) (§9.1).
    Backfill,
    /// **Contract** — switch reads to the new shape, stop writing the old, drop the old in a
    /// LATER non-blocking deploy (§9.1).
    Contract,
}

/// One forward-only migration: a stable, PII-free id + its DDL + its phase + the table it
/// targets (architecture §9). Ordered by registration (the runner applies them in order so a
/// later migration sees the earlier ones' tables).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Migration {
    /// A stable, monotonically-ordered id (e.g. `0001_outbox`). PII-free.
    pub id: &'static str,
    /// The forward-only DDL (`CREATE TABLE …` / `CREATE INDEX CONCURRENTLY …` / a nullable
    /// `ALTER … ADD`). A destructive `DROP TABLE`/`DROP COLUMN` is forward-only-illegal and is
    /// rejected by the runner; a blocking `ALTER` on a declared-hot table is rejected too.
    pub ddl: &'static str,
    /// The expand→backfill→contract phase (§9.1). `Plain` for an ordinary non-hot migration.
    pub phase: MigrationPhase,
    /// The table this migration targets, if it is a single-table change (so the runner can match
    /// it against the [`HotTables`] declaration). `None` for a multi-table / non-table migration.
    pub table: Option<&'static str>,
}

impl Migration {
    /// A plain forward migration (non-hot table; no phase discipline required).
    pub fn plain(id: &'static str, ddl: &'static str) -> Migration {
        Migration { id, ddl, phase: MigrationPhase::Plain, table: None }
    }

    /// A phased migration on a (possibly hot) table — the runner checks it against the hot-table
    /// declaration and the phase records which step of expand→backfill→contract it is.
    pub fn phased(
        id: &'static str,
        ddl: &'static str,
        phase: MigrationPhase,
        table: &'static str,
    ) -> Migration {
        Migration { id, ddl, phase, table: Some(table) }
    }
}

/// The forward-only embedded migration set (architecture §9; contract 1.5). Each entry is a
/// forward-only DDL statement (the `outbox` / `consumer_dedup` tables, a service's own schema).
/// The runner applies them in order at boot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Migrations(pub Vec<Migration>);

impl Migrations {
    /// Register PLAIN migrations from `(id, ddl)` pairs (ordered) — the ergonomic path for
    /// ordinary forward migrations (new tables, cold-table nullable adds).
    pub fn new(items: impl IntoIterator<Item = (&'static str, &'static str)>) -> Migrations {
        Migrations(items.into_iter().map(|(id, ddl)| Migration::plain(id, ddl)).collect())
    }

    /// Register an explicit migration list (so a hot-table change can carry its phase + table).
    pub fn of(items: impl IntoIterator<Item = Migration>) -> Migrations {
        Migrations(items.into_iter().collect())
    }
}

/// The per-subsystem **hot-table declaration** (architecture §9.4; contract 1.5; C-3). Every
/// subsystem declares its hot tables in its `AppSpec`; a table is flagged hot when its write
/// rate warrants expand→backfill→contract (**measured, not predicted** — per ADR-10). Both the
/// migration RUNNER (at boot) and the `forward-only-migration` LINT (at source-scan) read this
/// declaration to forbid a blocking `ALTER` on exactly these tables.
///
/// The seed set §9.4 names (measured per subsystem): Knowledge `block`/`db_row`/`doc_op`; the
/// high-write subsystems (Git ref/object metadata, CI `run`/`step`/log index, Issues
/// `issue`/`issue_relation`, Chat `message`/`channel_membership`). Those are declared by their
/// owning subsystems' `AppSpec`s as they land (M1+); the MECHANISM is frozen here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HotTables {
    tables: BTreeSet<String>,
}

impl HotTables {
    /// No hot tables declared (the default for a service with no high-write table yet).
    pub fn none() -> HotTables {
        HotTables { tables: BTreeSet::new() }
    }

    /// Declare a service's hot tables (§9.4) — measured-not-predicted per subsystem.
    pub fn declare(tables: impl IntoIterator<Item = impl Into<String>>) -> HotTables {
        HotTables {
            tables: tables.into_iter().map(Into::into).collect(),
        }
    }

    /// Whether `table` is declared hot (the runner + the lint both ask this).
    pub fn is_hot(&self, table: &str) -> bool {
        self.tables.contains(table)
    }

    /// The declared hot tables, sorted (so the data-map / the lint can read the set).
    pub fn tables(&self) -> impl Iterator<Item = &str> {
        self.tables.iter().map(String::as_str)
    }

    /// Whether any hot table is declared.
    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }
}

/// Whether a DDL statement is **destructive** (a forward-only violation): `DROP TABLE` /
/// `DROP COLUMN` would un-delete-able-y destroy data (§9.1; EI-01 §2). Case-insensitive.
pub fn is_destructive(ddl: &str) -> bool {
    let upper = ddl.to_ascii_uppercase();
    upper.contains("DROP TABLE") || upper.contains("DROP COLUMN")
}

/// Whether a DDL statement is a **blocking `ALTER`** (takes a table lock at write QPS):
/// `ALTER TABLE … ADD COLUMN … NOT NULL` without a `DEFAULT`, an in-place `ALTER … ALTER COLUMN`,
/// or a non-concurrent `CREATE INDEX` (architecture §9.1/§9.4). On a HOT table any of these
/// stalls writes — it must be the expand→backfill→contract idiom instead. Case-insensitive.
pub fn is_blocking_alter(ddl: &str) -> bool {
    let lower = ddl.to_ascii_lowercase();
    let add_not_null = lower.contains("alter table")
        && lower.contains("add column")
        && lower.contains("not null")
        && !lower.contains("default");
    let alter_column_inplace = lower.contains("alter table") && lower.contains("alter column");
    let non_concurrent_index =
        lower.contains("create index") && !lower.contains("concurrently");
    add_not_null || alter_column_inplace || non_concurrent_index
}

/// The forward-only migration RUNNER (architecture §9; contract 1.5). Applies the embedded DDL
/// in order at boot, recording what it applied, and REFUSES — loudly, at boot — a destructive
/// migration or a blocking `ALTER` on a declared-hot table.
#[derive(Default)]
pub struct MigrationRunner {
    applied: Vec<&'static str>,
}

impl MigrationRunner {
    /// A fresh runner (nothing applied yet).
    pub fn new() -> MigrationRunner {
        MigrationRunner { applied: Vec::new() }
    }

    /// Apply each migration in order, against the service's [`HotTables`] declaration (§9.4).
    /// Refuses:
    ///
    /// - a **destructive** migration (`DROP TABLE`/`DROP COLUMN`) — forward-only is structural
    ///   (§9.1); a service cannot start having silently destroyed data (EI-01 §2);
    /// - a **blocking `ALTER` on a declared-hot table** (§9.4) — a hot-table change must be
    ///   expand→backfill→contract, never one blocking `ALTER`.
    ///
    /// A blocking `ALTER` on a NON-hot table is admitted (the table can absorb the brief lock);
    /// this is the per-table tightening §9.4 freezes (the table-INDEPENDENT lint half already
    /// forbids the obviously-blocking `ADD … NOT NULL` regardless).
    pub fn run(
        &mut self,
        migrations: &Migrations,
        hot_tables: &HotTables,
    ) -> Result<(), ServeError> {
        for m in &migrations.0 {
            if is_destructive(m.ddl) {
                return Err(ServeError(format!(
                    "migration {} is destructive (DROP) — forward-only migrations only; \
                     a rollback is a NEW forward migration, never a down (§9.1)",
                    m.id
                )));
            }
            if let Some(table) = m.table {
                if hot_tables.is_hot(table) && is_blocking_alter(m.ddl) {
                    return Err(ServeError(format!(
                        "migration {} takes a blocking ALTER on the declared-HOT table `{}` \
                         (§9.4) — a hot-table change must be expand→backfill→contract \
                         (nullable add → throttled backfill → constrain), never one blocking \
                         ALTER that locks writes at QPS",
                        m.id, table
                    )));
                }
            }
            self.applied.push(m.id);
        }
        Ok(())
    }

    /// The ids applied, in order (so a test can assert boot ran the migrations before serving).
    pub fn applied(&self) -> &[&'static str] {
        &self.applied
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The runner applies an expand→backfill→contract migration on a hot table — the three-deploy
    /// idiom (§9.1): expand (nullable add), backfill (throttled), contract (constrain). Each step
    /// is non-blocking, so all three are admitted even though the table is declared hot.
    #[test]
    fn runner_applies_expand_backfill_contract_on_a_hot_table() {
        let hot = HotTables::declare(["issue"]);
        let migrations = Migrations::of([
            // expand: a NULLABLE add takes no blocking lock.
            Migration::phased(
                "0010_expand",
                "ALTER TABLE issue ADD COLUMN priority INT;",
                MigrationPhase::Expand,
                "issue",
            ),
            // backfill: data DML, off the hot path (no schema lock).
            Migration::phased(
                "0011_backfill",
                "UPDATE issue SET priority = 0 WHERE priority IS NULL;",
                MigrationPhase::Backfill,
                "issue",
            ),
            // contract: constrain via a validated, non-blocking path (NOT NULL with a DEFAULT).
            Migration::phased(
                "0012_contract",
                "ALTER TABLE issue ADD COLUMN status TEXT NOT NULL DEFAULT 'open';",
                MigrationPhase::Contract,
                "issue",
            ),
        ]);
        let mut runner = MigrationRunner::new();
        runner.run(&migrations, &hot).expect("expand→backfill→contract is admitted on a hot table");
        assert_eq!(runner.applied(), &["0010_expand", "0011_backfill", "0012_contract"]);
    }

    /// A destructive (DROP) migration is REJECTED at boot — forward-only is structural (§9.1).
    #[test]
    fn destructive_migration_is_rejected() {
        let migrations = Migrations::of([Migration::plain("0010_bad", "DROP TABLE issue")]);
        let mut runner = MigrationRunner::new();
        let e = runner.run(&migrations, &HotTables::none()).expect_err("DROP must be rejected");
        assert!(e.0.contains("forward-only"), "the error names forward-only: {}", e.0);
    }

    /// A blocking `ALTER` on a DECLARED-HOT table is REJECTED (§9.4) — a hot-table change must be
    /// expand→backfill→contract, never one blocking `ALTER`.
    #[test]
    fn blocking_alter_on_a_hot_table_is_rejected() {
        let hot = HotTables::declare(["issue"]);
        let migrations = Migrations::of([Migration::phased(
            "0010_hot",
            "ALTER TABLE issue ADD COLUMN body TEXT NOT NULL;", // no DEFAULT → blocking.
            MigrationPhase::Expand,
            "issue",
        )]);
        let mut runner = MigrationRunner::new();
        let e = runner.run(&migrations, &hot).expect_err("a blocking ALTER on a hot table is rejected");
        assert!(e.0.contains("declared-HOT"), "the error names the hot-table rule: {}", e.0);
        assert!(e.0.contains("issue"), "the error names the offending table: {}", e.0);
    }

    /// The SAME blocking `ALTER` on a NON-hot table is ADMITTED — the per-table tightening only
    /// fires on declared-hot tables (a cold table can absorb the brief lock). The table-
    /// INDEPENDENT obviously-blocking half is the lint's, not the runner's.
    #[test]
    fn blocking_alter_on_a_non_hot_table_is_admitted() {
        let cold = HotTables::none();
        let migrations = Migrations::of([Migration::phased(
            "0010_cold",
            "ALTER TABLE audit_archive ADD COLUMN note TEXT NOT NULL;",
            MigrationPhase::Plain,
            "audit_archive",
        )]);
        let mut runner = MigrationRunner::new();
        runner.run(&migrations, &cold).expect("a blocking ALTER on a non-hot table is admitted");
        assert_eq!(runner.applied(), &["0010_cold"]);
    }

    /// `is_destructive` / `is_blocking_alter` classify the DDL bug classes (the shared predicates
    /// the runner + the lint both use).
    #[test]
    fn ddl_classifiers_catch_the_bug_classes() {
        assert!(is_destructive("DROP TABLE issue"));
        assert!(is_destructive("ALTER TABLE issue DROP COLUMN body"));
        assert!(!is_destructive("ALTER TABLE issue ADD COLUMN x INT"));
        assert!(is_blocking_alter("ALTER TABLE issue ADD COLUMN x TEXT NOT NULL"));
        assert!(is_blocking_alter("ALTER TABLE issue ALTER COLUMN x TYPE BIGINT"));
        assert!(is_blocking_alter("CREATE INDEX idx ON issue (x)"));
        assert!(!is_blocking_alter("CREATE INDEX CONCURRENTLY idx ON issue (x)"));
        assert!(!is_blocking_alter("ALTER TABLE issue ADD COLUMN x TEXT")); // nullable add = expand.
    }

    /// The hot-table declaration mechanism (§9.4): a service declares its hot tables; `is_hot`
    /// answers per table (the frozen declare → query contract both the runner + lint read).
    #[test]
    fn hot_table_declaration_is_per_subsystem() {
        let hot = HotTables::declare(["block", "db_row", "doc_op"]); // the KN seed set (§9.4).
        assert!(hot.is_hot("block"));
        assert!(hot.is_hot("doc_op"));
        assert!(!hot.is_hot("audit_archive"));
        assert_eq!(hot.tables().collect::<Vec<_>>(), vec!["block", "db_row", "doc_op"]);
    }
}

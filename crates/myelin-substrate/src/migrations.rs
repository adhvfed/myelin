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
//! **Contract-index:** row 1.5 (forward-only online migrations + hot-table flags) — the RUNNER
//! is owned here; the contract VOCABULARY is owned by `myelin-storage` and RE-EXPORTED here.
//! **P-S15 → global P-032.** DEPENDS-ON P-S12 (`serve` calls the runner), P-S11 (the
//! `forward-only-migration` lint reads the hot-table declaration this module surfaces).
//!
//! ## SINGLE MIGRATION-CONTRACT AUTHORITY (de-dup, P-233 hardening)
//! **`myelin-storage` is the single migration-contract authority; substrate re-exports it.** The
//! contract vocabulary — [`Migration`], [`Migrations`], [`MigrationPhase`], [`HotTables`],
//! [`is_destructive`], [`is_blocking_alter`] — used to be DUPLICATED here (a structurally identical
//! second copy, kept in sync by hand). It is now defined ONCE in
//! [`myelin_storage::migration`] and **re-exported** below. The substrate→storage edge already
//! exists in the crate DAG (root-last; the harness depends on the tier client it wires); the reverse
//! `myelin-storage → myelin-substrate` edge is forbidden by the DAG, so storage is the canonical
//! home and substrate is the re-exporter. Consequently `myelin_substrate::migrations::Migration` and
//! `myelin_storage::migration::Migration` are now the SAME type — every existing importer (via either
//! path) keeps compiling unchanged. Substrate keeps its OWN [`MigrationRunner`] (this file): the
//! general forward-only **boot-time** validator that returns [`ServeError`], operating on the
//! re-exported types. (Storage additionally owns the ordering-enforcing
//! [`OnlineMigrationRunner`](myelin_storage::migration::OnlineMigrationRunner) — the
//! expand→backfill→contract gate — and, behind `--features integration`, the race-safe live
//! `PgMigrator` driver; those are storage-tier concerns, not re-exported here.)
//!
//! ## Forward-only (§9.1): no down migrations — you can't un-delete data
//! The [`MigrationRunner`] applies embedded migrations in order at boot. It REFUSES, loudly, at boot:
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
//! - **The concrete `tokio-postgres`/`sqlx` DDL execution** lands with the driver — now the
//!   race-safe [`PgMigrator`](myelin_storage::pg_migrator) in `myelin-storage` (behind
//!   `--features integration`): an advisory lock + an applied-migration version table serialise
//!   concurrent migrate() and record what was applied. The boot-time runner here records what it
//!   admitted; the live driver executes the admitted DDL.

use crate::ServeError;

// === The single migration-contract authority is `myelin-storage` (de-dup, P-233). ===
// These six items are DEFINED in `myelin_storage::migration` and RE-EXPORTED here; the substrate
// does NOT duplicate them. Re-exporting keeps every existing `myelin_substrate::…` importer
// compiling unchanged while collapsing the two former copies into ONE definition.
pub use myelin_storage::migration::{
    is_blocking_alter, is_destructive, HotTables, Migration, MigrationPhase, Migrations,
};

/// The forward-only migration RUNNER (architecture §9; contract 1.5). Applies the embedded DDL
/// in order at boot, recording what it applied, and REFUSES — loudly, at boot — a destructive
/// migration or a blocking `ALTER` on a declared-hot table.
///
/// This is substrate's OWN boot-time validator (it returns [`ServeError`], the harness error type);
/// it operates on the re-exported [`Migration`] / [`Migrations`] / [`HotTables`] types defined in
/// [`myelin_storage::migration`]. The ordering-enforcing
/// [`OnlineMigrationRunner`](myelin_storage::migration::OnlineMigrationRunner) and the race-safe live
/// `PgMigrator` driver are owned by the storage tier (not re-exported here).
#[derive(Default)]
pub struct MigrationRunner {
    applied: Vec<&'static str>,
}

impl MigrationRunner {
    /// A fresh runner (nothing applied yet).
    pub fn new() -> MigrationRunner {
        MigrationRunner {
            applied: Vec::new(),
        }
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
        runner
            .run(&migrations, &hot)
            .expect("expand→backfill→contract is admitted on a hot table");
        assert_eq!(
            runner.applied(),
            &["0010_expand", "0011_backfill", "0012_contract"]
        );
    }

    /// A destructive (DROP) migration is REJECTED at boot — forward-only is structural (§9.1).
    #[test]
    fn destructive_migration_is_rejected() {
        let migrations = Migrations::of([Migration::plain("0010_bad", "DROP TABLE issue")]);
        let mut runner = MigrationRunner::new();
        let e = runner
            .run(&migrations, &HotTables::none())
            .expect_err("DROP must be rejected");
        assert!(
            e.0.contains("forward-only"),
            "the error names forward-only: {}",
            e.0
        );
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
        let e = runner
            .run(&migrations, &hot)
            .expect_err("a blocking ALTER on a hot table is rejected");
        assert!(
            e.0.contains("declared-HOT"),
            "the error names the hot-table rule: {}",
            e.0
        );
        assert!(
            e.0.contains("issue"),
            "the error names the offending table: {}",
            e.0
        );
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
        runner
            .run(&migrations, &cold)
            .expect("a blocking ALTER on a non-hot table is admitted");
        assert_eq!(runner.applied(), &["0010_cold"]);
    }

    /// `is_destructive` / `is_blocking_alter` classify the DDL bug classes (the shared predicates
    /// the runner + the lint both use). These are the re-exported storage predicates.
    #[test]
    fn ddl_classifiers_catch_the_bug_classes() {
        assert!(is_destructive("DROP TABLE issue"));
        assert!(is_destructive("ALTER TABLE issue DROP COLUMN body"));
        assert!(!is_destructive("ALTER TABLE issue ADD COLUMN x INT"));
        assert!(is_blocking_alter(
            "ALTER TABLE issue ADD COLUMN x TEXT NOT NULL"
        ));
        assert!(is_blocking_alter(
            "ALTER TABLE issue ALTER COLUMN x TYPE BIGINT"
        ));
        assert!(!is_blocking_alter(
            "ALTER TABLE issue ALTER COLUMN reporter DROP NOT NULL"
        ));
        assert!(is_blocking_alter(
            "ALTER TABLE issue ALTER COLUMN reporter DROP NOT NULL, ALTER COLUMN x TYPE BIGINT"
        ));
        assert!(is_blocking_alter("CREATE INDEX idx ON issue (x)"));
        assert!(!is_blocking_alter(
            "CREATE INDEX CONCURRENTLY idx ON issue (x)"
        ));
        assert!(!is_blocking_alter("ALTER TABLE issue ADD COLUMN x TEXT")); // nullable add = expand.
    }

    /// The hot-table declaration mechanism (§9.4): a service declares its hot tables; `is_hot`
    /// answers per table (the frozen declare → query contract both the runner + lint read). This
    /// exercises the re-exported storage [`HotTables`].
    #[test]
    fn hot_table_declaration_is_per_subsystem() {
        let hot = HotTables::declare(["block", "db_row", "doc_op"]); // the KN seed set (§9.4).
        assert!(hot.is_hot("block"));
        assert!(hot.is_hot("doc_op"));
        assert!(!hot.is_hot("audit_archive"));
        assert_eq!(
            hot.tables().collect::<Vec<_>>(),
            vec!["block", "db_row", "doc_op"]
        );
    }

    /// The `(id, ddl)`-pair ergonomic constructor still works through the re-exported
    /// [`Migrations`] (it is now `myelin_storage::migration::Migrations::new`, additively added so
    /// substrate's pair-callers keep compiling).
    #[test]
    fn new_builds_plain_migrations_from_id_ddl_pairs() {
        let migrations =
            Migrations::new([("0010_hello", "CREATE TABLE IF NOT EXISTS hello (id TEXT)")]);
        assert_eq!(migrations.0.len(), 1);
        assert_eq!(migrations.0[0].id, "0010_hello");
        assert_eq!(migrations.0[0].phase, MigrationPhase::Plain);
    }
}

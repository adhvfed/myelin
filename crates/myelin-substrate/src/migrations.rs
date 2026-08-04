use crate::ServeError;

pub use myelin_storage::migration::{
    is_blocking_alter, is_destructive, HotTables, Migration, MigrationPhase, Migrations,
};

#[derive(Default)]
pub struct MigrationRunner {
    applied: Vec<&'static str>,
}

impl MigrationRunner {
    pub fn new() -> MigrationRunner {
        MigrationRunner {
            applied: Vec::new(),
        }
    }

    pub fn run(
        &mut self,
        migrations: &Migrations,
        hot_tables: &HotTables,
    ) -> Result<(), ServeError> {
        for m in &migrations.0 {
            if is_destructive(m.ddl) {
                return Err(ServeError(format!(
                    "migration {} is destructive (DROP) - forward-only migrations only; \
                     a rollback is a NEW forward migration, never a down (§9.1)",
                    m.id
                )));
            }
            if let Some(table) = m.table {
                if hot_tables.is_hot(table) && is_blocking_alter(m.ddl) {
                    return Err(ServeError(format!(
                        "migration {} takes a blocking ALTER on the declared-HOT table `{}` \
                         (§9.4) - a hot-table change must be expand→backfill→contract \
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

    pub fn applied(&self) -> &[&'static str] {
        &self.applied
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_applies_expand_backfill_contract_on_a_hot_table() {
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
        let mut runner = MigrationRunner::new();
        runner
            .run(&migrations, &hot)
            .expect("expand→backfill→contract is admitted on a hot table");
        assert_eq!(
            runner.applied(),
            &["0010_expand", "0011_backfill", "0012_contract"]
        );
    }

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

    #[test]
    fn blocking_alter_on_a_hot_table_is_rejected() {
        let hot = HotTables::declare(["issue"]);
        let migrations = Migrations::of([Migration::phased(
            "0010_hot",
            "ALTER TABLE issue ADD COLUMN body TEXT NOT NULL;",
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
        assert!(!is_blocking_alter("ALTER TABLE issue ADD COLUMN x TEXT"));
    }

    #[test]
    fn hot_table_declaration_is_per_subsystem() {
        let hot = HotTables::declare(["block", "db_row", "doc_op"]);
        assert!(hot.is_hot("block"));
        assert!(hot.is_hot("doc_op"));
        assert!(!hot.is_hot("audit_archive"));
        assert_eq!(
            hot.tables().collect::<Vec<_>>(),
            vec!["block", "db_row", "doc_op"]
        );
    }

    #[test]
    fn new_builds_plain_migrations_from_id_ddl_pairs() {
        let migrations =
            Migrations::new([("0010_hello", "CREATE TABLE IF NOT EXISTS hello (id TEXT)")]);
        assert_eq!(migrations.0.len(), 1);
        assert_eq!(migrations.0[0].id, "0010_hello");
        assert_eq!(migrations.0[0].phase, MigrationPhase::Plain);
    }
}

use myelin_substrate::{Migration, Migrations};

pub const CONSUMER_DEDUP_TABLE: &str = "consumer_dedup";

pub const CONSUMER_DEDUP_MIGRATION_ID: &str = "ci_dispatch_0001_consumer_dedup";

pub const CREATE_CONSUMER_DEDUP_DDL: &str = myelin_events::CONSUMER_DEDUP_MIGRATION;

pub fn dispatch_migrations() -> Migrations {
    Migrations::of([Migration::plain_on(
        CONSUMER_DEDUP_MIGRATION_ID,
        CREATE_CONSUMER_DEDUP_DDL,
        CONSUMER_DEDUP_TABLE,
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dedup_ledger_is_the_shared_foundation_shape() {
        let ddl = CREATE_CONSUMER_DEDUP_DDL;
        assert_eq!(ddl, myelin_events::CONSUMER_DEDUP_MIGRATION);
        for col in ["consumer", "event_id", "recorded_at"] {
            assert!(ddl.contains(col), "the §3.8 column `{col}` is declared");
        }
        assert!(
            ddl.contains("PRIMARY KEY (consumer, event_id)"),
            "the exactly-once dedup key is (consumer, event_id) - the platform consumer template"
        );
        assert!(!ddl.contains("tenant_id"));
        assert!(!ddl.contains("myelin_make_tenant_scoped"));
    }

    #[test]
    fn the_migration_is_forward_only_and_byte_identical_to_foundation() {
        let migrations = dispatch_migrations();
        assert_eq!(
            migrations.0.len(),
            1,
            "one forward migration: the dedup ledger"
        );
        let m = &migrations.0[0];
        assert_eq!(m.id, CONSUMER_DEDUP_MIGRATION_ID);
        assert_eq!(m.table, Some(CONSUMER_DEDUP_TABLE));
        assert!(
            !myelin_substrate::is_destructive(m.ddl),
            "the dedup migration is forward-only (no DROP)"
        );
        assert!(
            !m.ddl.to_ascii_uppercase().contains("DROP"),
            "no DROP in the dedup migration"
        );
        assert_eq!(m.ddl, myelin_events::CONSUMER_DEDUP_MIGRATION);
    }

    #[test]
    fn the_runner_admits_the_migration_and_refuses_a_drop() {
        use myelin_substrate::{HotTables, MigrationRunner};
        let mut runner = MigrationRunner::new();
        runner
            .run(&dispatch_migrations(), &HotTables::none())
            .expect("the dedup ledger migration applies forward-only");
        assert_eq!(runner.applied(), &[CONSUMER_DEDUP_MIGRATION_ID]);

        let bad = Migrations::of([Migration::plain(
            "ci_dispatch_9999_drop",
            "DROP TABLE consumer_dedup",
        )]);
        let mut runner2 = MigrationRunner::new();
        let e = runner2
            .run(&bad, &HotTables::none())
            .expect_err("a DROP must be refused");
        assert!(
            e.0.contains("forward-only"),
            "the refusal names forward-only: {}",
            e.0
        );
    }
}

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MigrationPhase {
    Plain,
    Expand,
    Backfill,
    Contract,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Migration {
    pub id: Cow<'static, str>,
    pub ddl: Cow<'static, str>,
    pub phase: MigrationPhase,
    pub table: Option<Cow<'static, str>>,
}

impl Migration {
    pub fn plain(id: impl Into<Cow<'static, str>>, ddl: impl Into<Cow<'static, str>>) -> Migration {
        Migration {
            id: id.into(),
            ddl: ddl.into(),
            phase: MigrationPhase::Plain,
            table: None,
        }
    }

    pub fn plain_on(
        id: impl Into<Cow<'static, str>>,
        ddl: impl Into<Cow<'static, str>>,
        table: impl Into<Cow<'static, str>>,
    ) -> Migration {
        Migration {
            id: id.into(),
            ddl: ddl.into(),
            phase: MigrationPhase::Plain,
            table: Some(table.into()),
        }
    }

    pub fn phased(
        id: impl Into<Cow<'static, str>>,
        ddl: impl Into<Cow<'static, str>>,
        phase: MigrationPhase,
        table: impl Into<Cow<'static, str>>,
    ) -> Migration {
        Migration {
            id: id.into(),
            ddl: ddl.into(),
            phase,
            table: Some(table.into()),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Migrations(pub Vec<Migration>);

impl Migrations {
    pub fn new(items: impl IntoIterator<Item = (&'static str, &'static str)>) -> Migrations {
        Migrations(
            items
                .into_iter()
                .map(|(id, ddl)| Migration::plain(id, ddl))
                .collect(),
        )
    }

    pub fn of(items: impl IntoIterator<Item = Migration>) -> Migrations {
        Migrations(items.into_iter().collect())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HotTables {
    tables: BTreeSet<String>,
}

impl HotTables {
    pub fn none() -> HotTables {
        HotTables {
            tables: BTreeSet::new(),
        }
    }

    pub fn declare(tables: impl IntoIterator<Item = impl Into<String>>) -> HotTables {
        HotTables {
            tables: tables.into_iter().map(Into::into).collect(),
        }
    }

    pub fn is_hot(&self, table: &str) -> bool {
        self.tables.contains(table)
    }

    pub fn tables(&self) -> impl Iterator<Item = &str> {
        self.tables.iter().map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }
}

pub fn is_destructive(ddl: &str) -> bool {
    let upper = ddl.to_ascii_uppercase();
    upper.contains("DROP TABLE") || upper.contains("DROP COLUMN")
}

pub fn is_blocking_alter(ddl: &str) -> bool {
    let lower = ddl.to_ascii_lowercase();
    let add_not_null = lower.contains("alter table")
        && lower.contains("add column")
        && lower.contains("not null")
        && !lower.contains("default");
    let alter_column_inplace = lower.split(';').any(|statement| {
        let normalized = statement.split_whitespace().collect::<Vec<_>>().join(" ");
        let is_alter_column =
            normalized.contains("alter table") && normalized.contains("alter column");
        let metadata_only_drop_not_null = normalized.matches("alter column").count() == 1
            && normalized.contains(" drop not null")
            && !normalized.contains(',');
        is_alter_column && !metadata_only_drop_not_null
    });
    let non_concurrent_index = lower.contains("create index") && !lower.contains("concurrently");
    add_not_null || alter_column_inplace || non_concurrent_index
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MigrationError {
    Destructive {
        id: String,
    },
    BlockingAlterOnHotTable {
        id: String,
        table: String,
    },
    HotTableNotOnline {
        id: String,
        table: String,
    },
    PhaseOutOfOrder {
        id: String,
        table: String,
        phase: MigrationPhase,
        after: PhaseProgress,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhaseProgress {
    None,
    Expanded,
    Backfilled,
    Contracted,
}

impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MigrationError::Destructive { id } => write!(
                f,
                "migration {id} is destructive (DROP) - forward-only migrations only; a rollback \
                 is a NEW forward migration, never a down (storage §3.1)"
            ),
            MigrationError::BlockingAlterOnHotTable { id, table } => write!(
                f,
                "migration {id} takes a blocking ALTER on the declared-HOT table `{table}` - a \
                 hot-table change must be expand→backfill→contract, never one blocking ALTER that \
                 locks writes at QPS (storage §3.1)"
            ),
            MigrationError::HotTableNotOnline { id, table } => write!(
                f,
                "migration {id} touches the declared-HOT table `{table}` as a Plain migration - a \
                 migration touching a declared hot table MUST use the online path \
                 (expand→backfill→contract), never a plain change (storage §3.1; P-ST-05)"
            ),
            MigrationError::PhaseOutOfOrder {
                id,
                table,
                phase,
                after,
            } => write!(
                f,
                "migration {id} on `{table}` carries phase {phase:?} out of order - \
                 expand→backfill→contract must run in order (the table is currently {after:?}); a \
                 contract-before-backfill ordering is rejected (storage §3.1; P-ST-05)"
            ),
        }
    }
}

impl std::error::Error for MigrationError {}

#[derive(Default)]
pub struct OnlineMigrationRunner {
    applied: Vec<String>,
}

impl OnlineMigrationRunner {
    pub fn new() -> OnlineMigrationRunner {
        OnlineMigrationRunner {
            applied: Vec::new(),
        }
    }

    pub fn run(
        &mut self,
        migrations: &Migrations,
        hot_tables: &HotTables,
    ) -> Result<(), MigrationError> {
        use std::collections::BTreeMap;
        let mut progress: BTreeMap<&str, PhaseProgress> = BTreeMap::new();

        for m in &migrations.0 {
            if is_destructive(m.ddl.as_ref()) {
                return Err(MigrationError::Destructive {
                    id: m.id.to_string(),
                });
            }

            if let Some(table) = m.table.as_deref() {
                let hot = hot_tables.is_hot(table);

                if hot && is_blocking_alter(m.ddl.as_ref()) {
                    return Err(MigrationError::BlockingAlterOnHotTable {
                        id: m.id.to_string(),
                        table: table.to_string(),
                    });
                }

                if hot && m.phase == MigrationPhase::Plain {
                    return Err(MigrationError::HotTableNotOnline {
                        id: m.id.to_string(),
                        table: table.to_string(),
                    });
                }

                if m.phase != MigrationPhase::Plain {
                    let current = *progress.get(table).unwrap_or(&PhaseProgress::None);
                    match next_progress(current, m.phase) {
                        Some(next) => {
                            progress.insert(table, next);
                        }
                        None => {
                            return Err(MigrationError::PhaseOutOfOrder {
                                id: m.id.to_string(),
                                table: table.to_string(),
                                phase: m.phase,
                                after: current,
                            });
                        }
                    }
                }
            }

            self.applied.push(m.id.to_string());
        }
        Ok(())
    }

    pub fn applied(&self) -> &[String] {
        &self.applied
    }
}

fn next_progress(current: PhaseProgress, phase: MigrationPhase) -> Option<PhaseProgress> {
    match (current, phase) {
        (PhaseProgress::None, MigrationPhase::Expand) => Some(PhaseProgress::Expanded),
        (PhaseProgress::Contracted, MigrationPhase::Expand) => Some(PhaseProgress::Expanded),
        (PhaseProgress::Expanded, MigrationPhase::Backfill) => Some(PhaseProgress::Backfilled),
        (PhaseProgress::Backfilled, MigrationPhase::Contract) => Some(PhaseProgress::Contracted),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_composed_migrations_own_their_descriptor_text() {
        let table = String::from("runtime_table");
        let migration = Migration::phased(
            format!("create_{table}"),
            format!("CREATE TABLE {table} (id bigint PRIMARY KEY)"),
            MigrationPhase::Plain,
            table,
        );

        assert!(matches!(&migration.id, Cow::Owned(_)));
        assert!(matches!(&migration.ddl, Cow::Owned(_)));
        assert!(matches!(&migration.table, Some(Cow::Owned(_))));
    }

    #[test]
    fn admits_expand_backfill_contract_on_a_hot_table() {
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
        let mut runner = OnlineMigrationRunner::new();
        runner
            .run(&migrations, &hot)
            .expect("expand→backfill→contract is admitted on a hot table");
        assert_eq!(
            runner.applied(),
            &["0010_expand", "0011_backfill", "0012_contract"]
        );
    }

    #[test]
    fn rejects_contract_before_backfill() {
        let hot = HotTables::declare(["issue"]);
        let migrations = Migrations::of([
            Migration::phased(
                "0010_expand",
                "ALTER TABLE issue ADD COLUMN priority INT;",
                MigrationPhase::Expand,
                "issue",
            ),
            Migration::phased(
                "0011_contract",
                "ALTER TABLE issue ADD CONSTRAINT priority_set CHECK (priority IS NOT NULL) NOT VALID;",
                MigrationPhase::Contract,
                "issue",
            ),
        ]);
        let mut runner = OnlineMigrationRunner::new();
        let e = runner
            .run(&migrations, &hot)
            .expect_err("a contract-before-backfill ordering is rejected");
        assert_eq!(
            e,
            MigrationError::PhaseOutOfOrder {
                id: "0011_contract".into(),
                table: "issue".into(),
                phase: MigrationPhase::Contract,
                after: PhaseProgress::Expanded,
            }
        );
        assert_eq!(runner.applied(), &["0010_expand"]);
        assert!(
            e.to_string().contains("contract-before-backfill"),
            "loud reason: {e}"
        );
    }

    #[test]
    fn rejects_backfill_before_expand() {
        let hot = HotTables::declare(["issue"]);
        let migrations = Migrations::of([Migration::phased(
            "0010_backfill",
            "UPDATE issue SET priority = 0;",
            MigrationPhase::Backfill,
            "issue",
        )]);
        let mut runner = OnlineMigrationRunner::new();
        let e = runner
            .run(&migrations, &hot)
            .expect_err("backfill-before-expand is rejected");
        assert_eq!(
            e,
            MigrationError::PhaseOutOfOrder {
                id: "0010_backfill".into(),
                table: "issue".into(),
                phase: MigrationPhase::Backfill,
                after: PhaseProgress::None,
            }
        );
    }

    #[test]
    fn hot_table_touched_plain_must_use_online_path() {
        let hot = HotTables::declare(["issue"]);
        let migrations = Migrations::of([Migration::plain_on(
            "0010_plain_hot",
            "ALTER TABLE issue ADD COLUMN note TEXT;",
            "issue",
        )]);
        let mut runner = OnlineMigrationRunner::new();
        let e = runner
            .run(&migrations, &hot)
            .expect_err("a hot table demands the online path");
        assert_eq!(
            e,
            MigrationError::HotTableNotOnline {
                id: "0010_plain_hot".into(),
                table: "issue".into()
            }
        );
        assert!(e.to_string().contains("online path"), "loud reason: {e}");
    }

    #[test]
    fn destructive_migration_is_rejected() {
        let migrations = Migrations::of([Migration::plain("0010_bad", "DROP TABLE issue")]);
        let mut runner = OnlineMigrationRunner::new();
        let e = runner
            .run(&migrations, &HotTables::none())
            .expect_err("a DROP must be rejected");
        assert_eq!(
            e,
            MigrationError::Destructive {
                id: "0010_bad".into()
            }
        );
        assert!(e.to_string().contains("forward-only"), "loud reason: {e}");
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
        let mut runner = OnlineMigrationRunner::new();
        let e = runner
            .run(&migrations, &hot)
            .expect_err("blocking ALTER on hot table is rejected");
        assert_eq!(
            e,
            MigrationError::BlockingAlterOnHotTable {
                id: "0010_hot".into(),
                table: "issue".into()
            }
        );
    }

    #[test]
    fn plain_migration_on_a_cold_table_is_admitted() {
        let hot = HotTables::declare(["issue"]);
        let migrations = Migrations::of([
            Migration::plain(
                "0010_new",
                "CREATE TABLE audit_archive (id BIGINT PRIMARY KEY);",
            ),
            Migration::plain_on(
                "0011_add",
                "ALTER TABLE audit_archive ADD COLUMN note TEXT;",
                "audit_archive",
            ),
        ]);
        let mut runner = OnlineMigrationRunner::new();
        runner
            .run(&migrations, &hot)
            .expect("a plain migration on a cold table is admitted");
        assert_eq!(runner.applied(), &["0010_new", "0011_add"]);
    }

    #[test]
    fn per_table_ordering_is_independent() {
        let hot = HotTables::declare(["issue", "message"]);
        let migrations = Migrations::of([
            Migration::phased(
                "0010_e",
                "ALTER TABLE issue ADD COLUMN a INT;",
                MigrationPhase::Expand,
                "issue",
            ),
            Migration::phased(
                "0011_e",
                "ALTER TABLE message ADD COLUMN b INT;",
                MigrationPhase::Expand,
                "message",
            ),
            Migration::phased(
                "0012_b",
                "UPDATE issue SET a = 0;",
                MigrationPhase::Backfill,
                "issue",
            ),
            Migration::phased(
                "0013_b",
                "UPDATE message SET b = 0;",
                MigrationPhase::Backfill,
                "message",
            ),
            Migration::phased(
                "0014_c",
                "ALTER TABLE issue ADD COLUMN a2 INT DEFAULT 0 NOT NULL;",
                MigrationPhase::Contract,
                "issue",
            ),
            Migration::phased(
                "0015_c",
                "ALTER TABLE message ADD COLUMN b2 INT DEFAULT 0 NOT NULL;",
                MigrationPhase::Contract,
                "message",
            ),
        ]);
        let mut runner = OnlineMigrationRunner::new();
        runner
            .run(&migrations, &hot)
            .expect("two interleaved online cycles are admitted");
        assert_eq!(runner.applied().len(), 6);
    }

    #[test]
    fn a_table_can_start_a_second_online_cycle() {
        assert_eq!(
            next_progress(PhaseProgress::Contracted, MigrationPhase::Expand),
            Some(PhaseProgress::Expanded)
        );
        assert_eq!(
            next_progress(PhaseProgress::Contracted, MigrationPhase::Contract),
            None
        );
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
        assert!(!hot.is_hot("audit_archive"));
        assert_eq!(
            hot.tables().collect::<Vec<_>>(),
            vec!["block", "db_row", "doc_op"]
        );
        assert!(!hot.is_empty());
        assert!(HotTables::none().is_empty());
        assert_eq!(HotTables::none(), HotTables::default());
        assert_eq!(HotTables::none().tables().count(), 0);
    }
}

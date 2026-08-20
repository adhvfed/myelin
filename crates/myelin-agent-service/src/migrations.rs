pub use myelin_storage::migration::Migration;
use myelin_storage::migration::{HotTables, MigrationError, Migrations, OnlineMigrationRunner};

pub const TABLES: [&str; 5] = [
    "agent_run",
    "agent_tool_def",
    "agent_proposed_effect",
    "agent_hitl_gate",
    "agent_trace",
];

pub const RUN_DDL: &str = "\
CREATE TABLE agent_run (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  run_id numeric NOT NULL, \
  agent_principal text NOT NULL, \
  on_behalf_of text NOT NULL, \
  binding_id numeric NOT NULL, \
  trigger_event text NOT NULL, \
  correlation_id text NOT NULL, \
  causation_id text NOT NULL, \
  depth integer NOT NULL, \
  runtime_ref text NOT NULL, \
  state text NOT NULL, \
  reservation_id text NOT NULL, \
  budget bigint NOT NULL, \
  trace_ref text, \
  PRIMARY KEY (tenant_id, region, run_id))";

pub const TOOL_DEF_DDL: &str = "\
CREATE TABLE agent_tool_def (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  name text NOT NULL, \
  subsystem text NOT NULL, \
  version integer NOT NULL, \
  input_schema text NOT NULL, \
  required_caps text[] NOT NULL, \
  effect_kind text NOT NULL, \
  side_effecting boolean NOT NULL, \
  requires_approval boolean NOT NULL, \
  exposed_over_mcp boolean NOT NULL, \
  PRIMARY KEY (tenant_id, region, subsystem, name, version))";

pub const PROPOSED_EFFECT_DDL: &str = "\
CREATE TABLE agent_proposed_effect (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  effect_id numeric NOT NULL, \
  run_id numeric NOT NULL, \
  tool_name text NOT NULL, \
  verdict text NOT NULL, \
  input_payload bytea, \
  PRIMARY KEY (tenant_id, region, effect_id))";

pub const HITL_GATE_DDL: &str = "\
CREATE TABLE agent_hitl_gate (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  gate_id text NOT NULL, \
  run_id text NOT NULL, \
  effect_id text NOT NULL, \
  risk_summary bytea, \
  cost_estimate bigint NOT NULL, \
  approver_filter text[] NOT NULL, \
  state text NOT NULL, \
  card_ref text, \
  PRIMARY KEY (tenant_id, region, gate_id))";

pub const TRACE_DDL: &str = "\
CREATE TABLE agent_trace (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  artifact_ref text NOT NULL, \
  run_id numeric NOT NULL, \
  trace_body bytea, \
  PRIMARY KEY (tenant_id, region, artifact_ref))";

pub fn migrations() -> Migrations {
    Migrations::of([
        Migration::plain_on("0001_create_agent_run", RUN_DDL, "agent_run"),
        Migration::plain_on("0002_create_agent_tool_def", TOOL_DEF_DDL, "agent_tool_def"),
        Migration::plain_on(
            "0003_create_agent_proposed_effect",
            PROPOSED_EFFECT_DDL,
            "agent_proposed_effect",
        ),
        Migration::plain_on(
            "0004_create_agent_hitl_gate",
            HITL_GATE_DDL,
            "agent_hitl_gate",
        ),
        Migration::plain_on("0005_create_agent_trace", TRACE_DDL, "agent_trace"),
    ])
}

pub fn hot_tables() -> HotTables {
    HotTables::none()
}

pub fn future_hot_tables() -> HotTables {
    HotTables::declare(["agent_run"])
}

pub fn runner() -> Result<OnlineMigrationRunner, MigrationError> {
    let mut runner = OnlineMigrationRunner::new();
    runner.run(&migrations(), &hot_tables())?;
    Ok(runner)
}

pub fn rls_scope_sql(table: &str) -> String {
    format!("SELECT myelin_make_tenant_scoped('{table}')")
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_storage::migration::MigrationPhase;

    #[test]
    fn the_five_migrations_apply_forward_only_in_order() {
        let runner = runner().expect("the five Agent-Fabric migrations apply forward-only");
        assert_eq!(
            runner.applied(),
            &[
                "0001_create_agent_run",
                "0002_create_agent_tool_def",
                "0003_create_agent_proposed_effect",
                "0004_create_agent_hitl_gate",
                "0005_create_agent_trace",
            ]
        );
    }

    #[test]
    fn every_table_is_tenant_region_first_with_a_tenant_first_pk() {
        let ddls = [
            RUN_DDL,
            TOOL_DEF_DDL,
            PROPOSED_EFFECT_DDL,
            HITL_GATE_DDL,
            TRACE_DDL,
        ];
        for ddl in ddls {
            let cols = ddl.split('(').nth(1).expect("a column list");
            let first_two: Vec<&str> = cols.split(',').take(2).map(str::trim).collect();
            assert!(
                first_two[0].starts_with("tenant_id text"),
                "first column must be tenant_id: {ddl}"
            );
            assert!(
                first_two[1].starts_with("region text"),
                "second column must be region: {ddl}"
            );
            assert!(
                ddl.contains("PRIMARY KEY (tenant_id, region"),
                "the primary key must lead with (tenant_id, region): {ddl}"
            );
        }
    }

    #[test]
    fn pii_columns_are_encrypted_byte_carriers() {
        assert!(PROPOSED_EFFECT_DDL.contains("input_payload bytea"));
        assert!(HITL_GATE_DDL.contains("risk_summary bytea"));
        assert!(TRACE_DDL.contains("trace_body bytea"));
    }

    #[test]
    fn each_table_gets_the_rls_scope_call() {
        for t in TABLES {
            assert_eq!(
                rls_scope_sql(t),
                format!("SELECT myelin_make_tenant_scoped('{t}')")
            );
        }
        assert_eq!(TABLES.len(), 5, "the five-table data model");
    }

    #[test]
    fn hitl_gate_model_ddl_is_a_subset_of_the_executed_boot_ddl() {
        let boot = myelin_storage::hitl_gate_durable::AGENT_HITL_GATE_MIGRATION;
        let boot_norm = boot.split_whitespace().collect::<Vec<_>>().join(" ");
        let model_cols = HITL_GATE_DDL
            .split('(')
            .nth(1)
            .expect("a column list")
            .rsplit_once(')')
            .map(|(cols, _)| cols)
            .unwrap_or_default();
        for col in model_cols.split(", ") {
            let col = col.trim();
            if col.starts_with("PRIMARY KEY") {
                continue;
            }
            let name_type = col.replace(" NOT NULL", "");
            assert!(
                boot_norm.contains(&name_type),
                "boot DDL (0054) must carry the §4.4 column `{name_type}` - model/boot drift"
            );
        }
        assert!(
            boot_norm.contains("PRIMARY KEY (tenant_id, region, gate_id)"),
            "both declarations share the (tenant, region, gate_id) PK"
        );
        assert!(boot_norm.contains("requested_by text NOT NULL"));
        assert!(boot_norm.contains("decided_by text"));
    }

    #[test]
    fn a_destructive_fabric_migration_is_refused() {
        let bad = Migrations::of([Migration::plain("0006_drop_run", "DROP TABLE agent_run")]);
        let mut runner = OnlineMigrationRunner::new();
        let e = runner
            .run(&bad, &hot_tables())
            .expect_err("a DROP must be refused");
        assert_eq!(
            e,
            MigrationError::Destructive {
                id: "0006_drop_run".into()
            }
        );
        assert!(e.to_string().contains("forward-only"), "loud reason: {e}");
    }

    #[test]
    fn a_blocking_alter_on_the_hot_run_table_is_refused() {
        let bad = Migrations::of([Migration::phased(
            "0006_run_add_notnull",
            "ALTER TABLE agent_run ADD COLUMN label text NOT NULL",
            MigrationPhase::Expand,
            "agent_run",
        )]);
        let mut runner = OnlineMigrationRunner::new();
        let e = runner
            .run(&bad, &future_hot_tables())
            .expect_err("a blocking ALTER on a hot table is refused");
        assert_eq!(
            e,
            MigrationError::BlockingAlterOnHotTable {
                id: "0006_run_add_notnull".into(),
                table: "agent_run".into()
            }
        );
    }
}

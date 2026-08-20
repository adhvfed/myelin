use myelin_substrate::{Migration, MigrationPhase, Migrations};

pub const TABLES: [&str; 6] = [
    "workflow_run",
    "wf_history",
    "wf_timer",
    "wf_signal",
    "wf_activity_attempt",
    "wf_definition",
];

pub const TENANT_SCOPED_TABLES: [&str; 5] = [
    "workflow_run",
    "wf_history",
    "wf_timer",
    "wf_signal",
    "wf_activity_attempt",
];

pub const WORKFLOW_RUN_DDL: &str = "\
CREATE TABLE workflow_run (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  run_id text NOT NULL, \
  wf_type text NOT NULL, \
  wf_version integer NOT NULL, \
  input jsonb NOT NULL, \
  state text NOT NULL CHECK (state IN ('running','waiting','completed','failed','nondeterministic','terminated')), \
  cursor bigint NOT NULL DEFAULT 0, \
  budget jsonb, \
  correlation_id text NOT NULL, \
  causation_id text, \
  caused_by text, \
  depth integer NOT NULL, \
  partition smallint NOT NULL, \
  lease_owner text, \
  lease_expires timestamptz, \
  created_at timestamptz NOT NULL DEFAULT now(), \
  updated_at timestamptz NOT NULL DEFAULT now(), \
  PRIMARY KEY (tenant_id, region, run_id))";

pub const WORKFLOW_RUN_RUNNABLE_IDX: &str =
    "CREATE INDEX wf_runnable ON workflow_run (partition, lease_expires) WHERE state IN ('running')";

pub const WF_HISTORY_DDL: &str = "\
CREATE TABLE wf_history (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  run_id text NOT NULL, \
  seq bigint NOT NULL, \
  kind text NOT NULL CHECK (kind IN ('wf_started','wf_completed','activity_scheduled','activity_completed','activity_failed','timer_set','timer_fired','signal_waited','signal_received','side_marker')), \
  command_id text NOT NULL, \
  result jsonb, \
  result_key_ref text, \
  occurred_at timestamptz NOT NULL DEFAULT now(), \
  PRIMARY KEY (tenant_id, region, run_id, seq), \
  UNIQUE (tenant_id, run_id, command_id))";

pub const WF_TIMER_DDL: &str = "\
CREATE TABLE wf_timer (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  timer_id text NOT NULL, \
  run_id text, \
  command_id text NOT NULL, \
  fire_at timestamptz NOT NULL, \
  bucket integer NOT NULL, \
  fired boolean NOT NULL DEFAULT false, \
  partition smallint NOT NULL, \
  PRIMARY KEY (tenant_id, region, timer_id))";

pub const WF_TIMER_DUE_IDX: &str =
    "CREATE INDEX wf_timer_due ON wf_timer (bucket, partition) WHERE NOT fired";

pub const WF_SIGNAL_DDL: &str = "\
CREATE TABLE wf_signal (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  run_id text NOT NULL, \
  signal_name text NOT NULL, \
  idem_key text NOT NULL, \
  payload jsonb NOT NULL, \
  payload_key_ref text, \
  received_at timestamptz NOT NULL DEFAULT now(), \
  consumed_seq bigint, \
  PRIMARY KEY (tenant_id, run_id, signal_name, idem_key))";

pub const WF_SIGNAL_PENDING_IDX: &str =
    "CREATE INDEX wf_signal_pending ON wf_signal (tenant_id, region, run_id) WHERE consumed_seq IS NULL";

pub const WF_ACTIVITY_ATTEMPT_DDL: &str = "\
CREATE TABLE wf_activity_attempt (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  run_id text NOT NULL, \
  command_id text NOT NULL, \
  attempt integer NOT NULL, \
  idem_token text NOT NULL, \
  state text NOT NULL CHECK (state IN ('scheduled','running','succeeded','failed','retrying')), \
  error text, \
  started_at timestamptz, \
  ended_at timestamptz, \
  PRIMARY KEY (tenant_id, region, run_id, command_id, attempt))";

pub const WF_DEFINITION_DDL: &str = "\
CREATE TABLE wf_definition (\
  wf_type text NOT NULL, \
  version integer NOT NULL, \
  code_hash text NOT NULL, \
  status text NOT NULL CHECK (status IN ('active','draining','retired')), \
  registered_at timestamptz NOT NULL DEFAULT now(), \
  PRIMARY KEY (wf_type, version))";

pub const WORKFLOW_RUN_CONTROL_EXPAND_DDL: &str = "\
ALTER TABLE workflow_run ADD COLUMN IF NOT EXISTS idem_key text;
ALTER TABLE workflow_run ADD COLUMN IF NOT EXISTS cancel_reason text";

pub const WORKFLOW_RUN_IDEM_INDEX_DDL: &str = "\
CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS wf_run_idem \
ON workflow_run (tenant_id, region, idem_key) WHERE idem_key IS NOT NULL";

pub const WORKFLOW_RUN_DRIVE_EXPAND_DDL: &str = "\
ALTER TABLE workflow_run ADD COLUMN IF NOT EXISTS lease_epoch bigint NOT NULL DEFAULT 0;
ALTER TABLE workflow_run ADD COLUMN IF NOT EXISTS last_drive_id text;
ALTER TABLE workflow_run ADD COLUMN IF NOT EXISTS last_drive_fingerprint text";

pub const WORKFLOW_RUN_SCOPED_RUNNABLE_INDEX_DDL: &str = "\
CREATE INDEX CONCURRENTLY IF NOT EXISTS wf_runnable_scoped \
ON workflow_run (tenant_id, region, partition, lease_expires, updated_at, run_id) \
WHERE state = 'running'";

pub const WORKFLOW_RUN_WAITING_REPAIR_INDEX_DDL: &str = "\
CREATE INDEX CONCURRENTLY IF NOT EXISTS wf_waiting_repair \
ON workflow_run (tenant_id, region, partition, lease_expires, updated_at, run_id) \
WHERE state = 'waiting'";

pub const VALIDATE_WORKFLOW_RUN_WAITING_REPAIR_INDEX_DDL: &str = "\
DO $myelin$
BEGIN
  IF NOT EXISTS (
    SELECT 1
      FROM pg_catalog.pg_index AS index_state
      JOIN pg_catalog.pg_class AS index_relation
        ON index_relation.oid = index_state.indexrelid
      JOIN pg_catalog.pg_class AS table_relation
        ON table_relation.oid = index_state.indrelid
      JOIN pg_catalog.pg_namespace AS relation_namespace
        ON relation_namespace.oid = table_relation.relnamespace
     WHERE relation_namespace.nspname = current_schema()
       AND table_relation.relname = 'workflow_run'
       AND table_relation.relkind = 'r'
       AND index_relation.relnamespace = relation_namespace.oid
       AND index_relation.relname = 'wf_waiting_repair'
       AND index_relation.relkind = 'i'
       AND index_state.indisvalid
       AND index_state.indisready
  ) THEN
    RAISE EXCEPTION 'wf_waiting_repair on %.workflow_run is missing, invalid, or not ready; verify the index exists, then repair it with REINDEX INDEX CONCURRENTLY %.wf_waiting_repair before restarting', current_schema(), current_schema();
  END IF;
END
$myelin$";

const TABLE_DDLS: &[(&str, &str, &str, bool)] = &[
    (
        "flow_0001_workflow_run",
        WORKFLOW_RUN_DDL,
        "workflow_run",
        true,
    ),
    ("flow_0002_wf_history", WF_HISTORY_DDL, "wf_history", true),
    ("flow_0003_wf_timer", WF_TIMER_DDL, "wf_timer", true),
    ("flow_0004_wf_signal", WF_SIGNAL_DDL, "wf_signal", true),
    (
        "flow_0005_wf_activity_attempt",
        WF_ACTIVITY_ATTEMPT_DDL,
        "wf_activity_attempt",
        true,
    ),
    (
        "flow_0006_wf_definition",
        WF_DEFINITION_DDL,
        "wf_definition",
        false,
    ),
];

const TABLE_INDEXES: &[(&str, &str)] = &[
    ("workflow_run", WORKFLOW_RUN_RUNNABLE_IDX),
    ("wf_timer", WF_TIMER_DUE_IDX),
    ("wf_signal", WF_SIGNAL_PENDING_IDX),
];

pub fn rls_scope_sql(table: &str) -> String {
    format!("SELECT myelin_make_tenant_scoped('{table}')")
}

pub fn migrations() -> Migrations {
    let mut items = TABLE_DDLS
        .iter()
        .copied()
        .map(|(id, create_ddl, table, rls_scoped)| {
            let mut ddl = String::new();
            ddl.push_str(create_ddl);
            ddl.push(';');
            for &(idx_table, idx_ddl) in TABLE_INDEXES {
                if idx_table == table {
                    ddl.push('\n');
                    ddl.push_str(idx_ddl);
                    ddl.push(';');
                }
            }
            if rls_scoped {
                ddl.push('\n');
                ddl.push_str(&rls_scope_sql(table));
                ddl.push(';');
            }
            if table == "workflow_run" {
                Migration::plain(id, ddl)
            } else {
                Migration::phased(id, ddl, MigrationPhase::Plain, table)
            }
        })
        .collect::<Vec<_>>();
    items.push(Migration::phased(
        "flow_0007_workflow_run_control_expand",
        WORKFLOW_RUN_CONTROL_EXPAND_DDL,
        MigrationPhase::Expand,
        "workflow_run",
    ));
    items.push(Migration::phased(
        "flow_0008_workflow_run_idem_index",
        WORKFLOW_RUN_IDEM_INDEX_DDL,
        MigrationPhase::Expand,
        "workflow_run",
    ));
    items.push(Migration::phased(
        "flow_0009_workflow_run_drive_expand",
        WORKFLOW_RUN_DRIVE_EXPAND_DDL,
        MigrationPhase::Expand,
        "workflow_run",
    ));
    items.push(Migration::phased(
        "flow_0010_workflow_run_scoped_runnable_index",
        WORKFLOW_RUN_SCOPED_RUNNABLE_INDEX_DDL,
        MigrationPhase::Expand,
        "workflow_run",
    ));
    items.push(Migration::phased(
        "flow_0011_workflow_run_waiting_repair_index",
        WORKFLOW_RUN_WAITING_REPAIR_INDEX_DDL,
        MigrationPhase::Expand,
        "workflow_run",
    ));
    items.push(Migration::phased(
        "flow_0012_validate_workflow_run_waiting_repair_index",
        VALIDATE_WORKFLOW_RUN_WAITING_REPAIR_INDEX_DDL,
        MigrationPhase::Expand,
        "workflow_run",
    ));
    Migrations::of(items)
}

pub fn ddl_is_forward_only(ddl: &str) -> bool {
    !myelin_substrate::is_destructive(ddl)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_substrate::{HotTables, MigrationRunner};

    #[test]
    fn the_schema_and_control_migrations_apply_forward_only_in_order() {
        let migrations = migrations();
        assert_eq!(
            migrations.0.len(),
            12,
            "six-table data model plus six online control/drive/repair expansions (incl. the \
             concurrent-index validation)"
        );
        let mut runner = MigrationRunner::new();
        runner
            .run(&migrations, &HotTables::declare(["workflow_run"]))
            .expect("the flow migrations apply forward-only");
        assert_eq!(
            runner.applied(),
            &[
                "flow_0001_workflow_run",
                "flow_0002_wf_history",
                "flow_0003_wf_timer",
                "flow_0004_wf_signal",
                "flow_0005_wf_activity_attempt",
                "flow_0006_wf_definition",
                "flow_0007_workflow_run_control_expand",
                "flow_0008_workflow_run_idem_index",
                "flow_0009_workflow_run_drive_expand",
                "flow_0010_workflow_run_scoped_runnable_index",
                "flow_0011_workflow_run_waiting_repair_index",
                "flow_0012_validate_workflow_run_waiting_repair_index",
            ],
            "tables then online control expands, in order - 0 backward migration"
        );
    }

    #[test]
    fn durable_control_idempotency_is_an_online_database_constraint() {
        assert!(WORKFLOW_RUN_CONTROL_EXPAND_DDL.contains("idem_key text"));
        assert!(WORKFLOW_RUN_CONTROL_EXPAND_DDL.contains("cancel_reason text"));
        assert!(WORKFLOW_RUN_IDEM_INDEX_DDL.contains("UNIQUE INDEX CONCURRENTLY"));
        assert!(WORKFLOW_RUN_IDEM_INDEX_DDL.contains("tenant_id, region, idem_key"));
        assert!(WORKFLOW_RUN_IDEM_INDEX_DDL.contains("WHERE idem_key IS NOT NULL"));
    }

    #[test]
    fn durable_drive_claim_is_fenced_and_tenant_region_indexed() {
        assert!(WORKFLOW_RUN_DRIVE_EXPAND_DDL.contains("lease_epoch bigint NOT NULL DEFAULT 0"));
        assert!(WORKFLOW_RUN_DRIVE_EXPAND_DDL.contains("last_drive_fingerprint text"));
        assert!(WORKFLOW_RUN_SCOPED_RUNNABLE_INDEX_DDL.contains("INDEX CONCURRENTLY"));
        assert!(WORKFLOW_RUN_SCOPED_RUNNABLE_INDEX_DDL
            .contains("tenant_id, region, partition, lease_expires, updated_at, run_id"));
    }

    #[test]
    fn every_tenant_table_is_tenant_region_first_with_a_tenant_first_pk() {
        for (_id, ddl, table, rls_scoped) in TABLE_DDLS {
            if !rls_scoped {
                assert_eq!(
                    *table, "wf_definition",
                    "the only non-tenant table is wf_definition (§3.6)"
                );
                assert!(
                    ddl.contains("PRIMARY KEY (wf_type, version)"),
                    "wf_definition is keyed by (wf_type, version) - definitions are code, not tenant data"
                );
                assert!(
                    !ddl.contains("tenant_id"),
                    "wf_definition carries NO tenant column (§3.6)"
                );
                continue;
            }
            let cols = ddl.split('(').nth(1).expect("a column list");
            let first_two: Vec<&str> = cols.split(',').take(2).map(str::trim).collect();
            assert!(
                first_two[0].starts_with("tenant_id text"),
                "first column must be tenant_id ({table}): {ddl}"
            );
            assert!(
                first_two[1].starts_with("region text"),
                "second column must be region ({table}): {ddl}"
            );
            assert!(
                ddl.contains("PRIMARY KEY (tenant_id,"),
                "the primary key must LEAD with tenant_id ({table}): {ddl}"
            );
            if *table == "wf_signal" {
                assert!(
                    ddl.contains("PRIMARY KEY (tenant_id, run_id, signal_name, idem_key)"),
                    "wf_signal keys on the per-effect idempotency anchor (§3.4): {ddl}"
                );
            } else {
                assert!(
                    ddl.contains("PRIMARY KEY (tenant_id, region"),
                    "the primary key must lead with (tenant_id, region) ({table}): {ddl}"
                );
            }
        }
    }

    #[test]
    fn wf_history_journal_is_idempotent_by_construction() {
        assert!(
            WF_HISTORY_DDL.contains("UNIQUE (tenant_id, run_id, command_id)"),
            "wf_history journaling is idempotent on (tenant, run_id, command_id) (§3.2)"
        );
        assert!(
            WF_HISTORY_DDL.contains("command_id text NOT NULL"),
            "the deterministic replay-match key"
        );
        assert!(
            WF_HISTORY_DDL.contains("PRIMARY KEY (tenant_id, region, run_id, seq)"),
            "the per-run monotonic seq is the replay order (§3.2)"
        );
        assert!(
            WF_HISTORY_DDL.contains("result_key_ref text"),
            "the inline-PII crypto-shred key ref (§4.8)"
        );
    }

    #[test]
    fn wf_signal_pk_is_idem_key_idempotent() {
        assert!(
            WF_SIGNAL_DDL.contains("PRIMARY KEY (tenant_id, run_id, signal_name, idem_key)"),
            "the signal PK dedups a re-delivered signal by construction (§3.4 / §6.4 / §4.9)"
        );
        assert!(
            WF_SIGNAL_DDL.contains("payload_key_ref text"),
            "the inline-PII crypto-shred key ref (§3.4)"
        );
        assert!(
            WF_SIGNAL_DDL.contains("payload jsonb NOT NULL"),
            "the signal payload is refs-not-payloads (§3.4)"
        );
    }

    #[test]
    fn wf_timer_partial_index_covers_bucket_partition_where_not_fired() {
        let m = migrations();
        let timer =
            m.0.iter()
                .find(|m| m.table.as_deref() == Some("wf_timer"))
                .expect("the wf_timer migration");
        assert!(
            timer.ddl.contains(
                "CREATE INDEX wf_timer_due ON wf_timer (bucket, partition) WHERE NOT fired"
            ),
            "the SC-11 partial index on (bucket, partition) WHERE NOT fired (§3.3): {}",
            timer.ddl
        );
        assert!(
            WF_TIMER_DDL.contains("bucket integer NOT NULL"),
            "the epoch_minute(fire_at) coarse bucket (§3.3)"
        );
        assert!(
            WF_TIMER_DDL.contains("fired boolean NOT NULL DEFAULT false"),
            "the unfired flag the partial index pivots on"
        );
    }

    #[test]
    fn wf_activity_attempt_carries_the_bus2_idem_token() {
        assert!(
            WF_ACTIVITY_ATTEMPT_DDL.contains("idem_token text NOT NULL"),
            "the BUS-2 dedup bridge token (§3.5)"
        );
        assert!(
            WF_ACTIVITY_ATTEMPT_DDL
                .contains("PRIMARY KEY (tenant_id, region, run_id, command_id, attempt)"),
            "the per-attempt ledger key (§3.5)"
        );
    }

    #[test]
    fn workflow_run_carries_the_3_1_lifecycle_invariants() {
        let ddl = WORKFLOW_RUN_DDL;
        for s in [
            "running",
            "waiting",
            "completed",
            "failed",
            "nondeterministic",
            "terminated",
        ] {
            assert!(
                ddl.contains(s),
                "the lifecycle state `{s}` is in the CHECK (§3.1)"
            );
        }
        assert!(
            ddl.contains("cursor bigint NOT NULL DEFAULT 0"),
            "the replay short-circuit cursor (§3.1)"
        );
        assert!(
            ddl.contains("budget jsonb"),
            "the owned RunBudget (§3.1; minor-units, not floats)"
        );
        for c in [
            "correlation_id text",
            "causation_id text",
            "caused_by text",
            "depth integer",
        ] {
            assert!(
                ddl.contains(c),
                "the causality column `{c}` (§3.1, BUS-5/AG-6)"
            );
        }
        for c in [
            "partition smallint",
            "lease_owner text",
            "lease_expires timestamptz",
        ] {
            assert!(
                ddl.contains(c),
                "the lease-dispatch column `{c}` (§3.1 / §4.7)"
            );
        }
        assert!(
            ddl.contains("input jsonb NOT NULL"),
            "the input is refs-not-payloads (§3.1)"
        );
    }

    #[test]
    fn wf_definition_is_the_global_versioned_registry() {
        assert!(
            WF_DEFINITION_DDL.contains("code_hash text NOT NULL"),
            "the code-hash drift-detector (§3.6)"
        );
        assert!(
            WF_DEFINITION_DDL.contains("PRIMARY KEY (wf_type, version)"),
            "the (wf_type, version) registry key (§3.6)"
        );
        for s in ["active", "draining", "retired"] {
            assert!(
                WF_DEFINITION_DDL.contains(s),
                "the definition status `{s}` (§3.6)"
            );
        }
        assert!(
            !WF_DEFINITION_DDL.contains("tenant_id"),
            "the global registry carries NO tenant column (§3.6)"
        );
    }

    #[test]
    fn each_tenant_table_gets_the_rls_scope_call_and_the_registry_does_not() {
        let migrations = migrations();
        for (i, (_id, _ddl, table, rls_scoped)) in TABLE_DDLS.iter().enumerate() {
            let m = &migrations.0[i];
            assert!(
                m.ddl.contains("CREATE TABLE"),
                "migration `{}` carries the create-table",
                m.id
            );
            if *rls_scoped {
                assert_eq!(
                    rls_scope_sql(table),
                    format!("SELECT myelin_make_tenant_scoped('{table}')")
                );
                assert!(
                    m.ddl.contains(&rls_scope_sql(table)),
                    "migration `{}` carries the RLS scoping for `{table}`",
                    m.id
                );
            } else {
                assert!(
                    !m.ddl.contains("myelin_make_tenant_scoped"),
                    "the global registry `{table}` carries NO RLS-scope call (no tenant column)"
                );
            }
        }
        assert_eq!(TABLES.len(), 6, "the six-table data model");
        assert_eq!(
            TENANT_SCOPED_TABLES.len(),
            5,
            "five tenant-scoped tables + the global registry"
        );
    }

    #[test]
    fn a_destructive_flow_migration_is_refused() {
        let bad = Migrations::of([Migration::plain(
            "flow_9999_drop",
            "DROP TABLE workflow_run",
        )]);
        let mut runner = MigrationRunner::new();
        let e = runner
            .run(&bad, &HotTables::none())
            .expect_err("a DROP must be refused");
        assert!(
            e.0.contains("forward-only"),
            "the refusal names forward-only: {}",
            e.0
        );
        for (_id, ddl, _table, _rls) in TABLE_DDLS {
            assert!(ddl_is_forward_only(ddl), "the real DDL is forward-only");
            assert!(
                !ddl.to_ascii_uppercase().contains("DROP"),
                "no DROP in the data-model DDL"
            );
        }
    }
}

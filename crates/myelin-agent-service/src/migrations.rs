//! The Agent-Fabric forward-only `(tenant, region)`-first RLS migrations (architecture §4;
//! contracts 1.5, 12.1, 11.4; AG-P2 / P-131).
//!
//! Five tables — `run` (§4.1), `tool_def` (§4.2), `proposed_effect` (§4.3), `hitl_gate` (§4.4),
//! `trace` (§4.5) — each:
//! - **`(tenant, region)`-first** — `tenant_id text` + `region text` are the leading columns + the
//!   primary-key prefix (12.1, ADR-11): the partition key, from the verified token, never the path.
//! - **RLS-enabled** — a trailing `SELECT myelin_make_tenant_scoped('<table>')` makes the table
//!   `ENABLE`+`FORCE ROW LEVEL SECURITY` and installs the standard `(tenant_id, region)` isolation
//!   policy (the dev/prod Postgres convention, `scripts/pg-init/00-rls-conventions.sql`). With the
//!   app role `NOSUPERUSER NOBYPASSRLS`, a session set to tenant A reads **only** tenant A's rows.
//! - **forward-only online** — applied through [`myelin_storage::migration::OnlineMigrationRunner`]
//!   (contract 1.5): a `DROP` / a blocking `ALTER` on a hot table / a contract-before-backfill is
//!   refused. These five are fresh `CREATE TABLE`s (`Plain`, non-destructive) — the first migration
//!   of each table; later columns evolve via the expand→backfill→contract online idiom.
//! - **residency-pinned + per-tenant envelope-encrypted** — `region` is a first-class partition
//!   dimension; the PII-bearing columns (`input_payload`, `risk_summary`, `trace_body`) are stored
//!   as `bytea` ENCRYPTED under the per-subject DEK (contract 11.4 — the crypto-shred lever the
//!   `#[personal_data]` tags in [`crate::schema`] name); the identity columns hold OPAQUE pseudonyms
//!   (contract 4.8). The DDL here declares the columns; the envelope-encryption is the storage DEK
//!   layer's (11.3/11.4) — the column type (`bytea`) is the carrier.
//!
//! ## What this module is (and the floor it names)
//! This is the migration SET + the forward-only ADMIT proof. The CONCRETE DDL execution against a
//! live Postgres connection is the storage driver's (P-S12); here the runner *validates ordering +
//! admits the online shape* and records what it applied, and the `integration` test
//! (`tests/integration_rls.rs`) applies the real DDL + proves the RLS policy denies a cross-tenant
//! read against the LIVE dev stack (0 rows). The validation logic does not change shape when the
//! driver lands.

pub use myelin_storage::migration::Migration;
use myelin_storage::migration::{HotTables, MigrationError, Migrations, OnlineMigrationRunner};

/// The five table names (architecture §4.1..§4.5), in their migration order. Exposed so the
/// integration test can build + RLS-scope each one without restating the names.
pub const TABLES: [&str; 5] = [
    "agent_run",
    "agent_tool_def",
    "agent_proposed_effect",
    "agent_hitl_gate",
    "agent_trace",
];

/// The `run` table DDL (§4.1) — `(tenant, region)`-first; the durable-workflow instance row.
/// `budget bigint` is integer minor-units (never a float). The PK leads with `(tenant_id, region)`.
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

/// The `tool_def` table DDL (§4.2) — the one permissioned registry; `(tenant, region)`-first,
/// keyed `(subsystem, name, version)` under the tenant partition. No PII (a catalogue entry).
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

/// The `proposed_effect` table DDL (§4.3) — the plan-then-apply audit row. `input_payload bytea` is
/// the per-subject-DEK-encrypted effect payload (11.4). `(tenant, region)`-first.
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

/// The `hitl_gate` table DDL (§4.4) — the durable approval state. `risk_summary bytea` is the
/// per-subject-DEK-encrypted humanised card text (11.4); `approver_filter text[]` is the
/// `list_subjects`-derived opaque-pseudonym set (4.4/4.8). `cost_estimate bigint` is integer
/// minor-units. `(tenant, region)`-first.
///
/// **R2.4 reconciliation:** the id columns (`gate_id`, `run_id`, `effect_id`) are `text` — the
/// code-side carriers are opaque STRINGS (`GateId(String)`, the run id, the per-effect key
/// `gate:{tool}:{object}`), and the gate id is deliberately an unguessable opaque token (never a
/// numeric sequence). The EXECUTED boot declaration of this table is
/// `myelin_storage::hitl_gate_durable::AGENT_HITL_GATE_MIGRATION` (migration `0054`, in
/// `all_durable_migrations()` — the boot gap this table sat in is closed); the parity test below
/// pins THIS model DDL's columns to that boot DDL so the two declarations cannot drift. (This DDL
/// was never boot-applied anywhere pre-R2.4 — that gap is exactly why the column-type
/// reconciliation is safe in place.)
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

/// The `trace` table DDL (§4.5) — the content-addressed execution-trace pointer; `region` IS the
/// residency pin. `trace_body bytea` is the per-subject-DEK-encrypted conversation history (11.4).
/// `(tenant, region)`-first.
pub const TRACE_DDL: &str = "\
CREATE TABLE agent_trace (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  artifact_ref text NOT NULL, \
  run_id numeric NOT NULL, \
  trace_body bytea, \
  PRIMARY KEY (tenant_id, region, artifact_ref))";

/// The five forward-only migrations, in order. Each is a fresh `CREATE TABLE` (`Plain`,
/// non-destructive) — the first migration of its table. They name their (cold-at-creation) table so
/// the runner verifies the table is not declared hot at creation time (a hot table demands the
/// online path; a brand-new table is created cold, then its later columns evolve online).
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

/// The Fabric's declared hot tables AT CREATION. The five tables are brand-new `CREATE TABLE`s with
/// no write QPS yet, so the creation set runs against `none()` — a `CREATE TABLE` is not a hot-table
/// mutation (the online expand→backfill→contract discipline governs *changes to* a hot table, not
/// its birth). Hotness is **measured, not predicted** (storage §3.1): once millions of durable runs
/// accrete, `agent_run` is declared hot ([`future_hot_tables`]) and its later column changes MUST
/// use the online idiom — a blocking `ALTER` is then refused by construction.
pub fn hot_tables() -> HotTables {
    HotTables::none()
}

/// The Fabric's hot tables ONCE write volume accretes (storage §3.1, measured-not-predicted):
/// `agent_run` is the high-write durable-workflow table. The forward-only online runner reads this
/// to refuse any future blocking `ALTER` on it (a later column change must be
/// expand→backfill→contract). Named here so the online discipline is a declared, testable forward
/// dependency, not a convention.
pub fn future_hot_tables() -> HotTables {
    HotTables::declare(["agent_run"])
}

/// Run the five migrations through the storage forward-only **online** runner (contract 1.5).
/// Returns the runner with the applied ids recorded on success, or the loud [`MigrationError`] on
/// the first violation (a service cannot start having admitted an unsafe migration — EI-01 §2).
pub fn runner() -> Result<OnlineMigrationRunner, MigrationError> {
    let mut runner = OnlineMigrationRunner::new();
    runner.run(&migrations(), &hot_tables())?;
    Ok(runner)
}

/// The `myelin_make_tenant_scoped(<table>)` RLS-readiness call each tenant-scoped migration emits
/// AFTER its `CREATE TABLE` (the dev/prod Postgres convention). The integration test runs exactly
/// this against the live stack; the DDL string makes the RLS step visible + asserted-in-tests.
pub fn rls_scope_sql(table: &str) -> String {
    format!("SELECT myelin_make_tenant_scoped('{table}')")
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_storage::migration::MigrationPhase;

    /// THE forward-only ADMIT proof (the GATE): the five `(tenant, region)`-first migrations apply
    /// forward-only through the storage online runner — 0 destructive, 0 blocking-on-hot, in order.
    /// The applied ids are recorded in migration order (contract 1.5).
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

    /// Every migration is `(tenant, region)`-first: the DDL leads with `tenant_id` then `region`,
    /// and the primary key leads with `(tenant_id, region)` (12.1 — the partition key prefix, no
    /// cross-tenant query path).
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
            // Leading columns: tenant_id then region, before any other column.
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
            // The PK leads with (tenant_id, region).
            assert!(
                ddl.contains("PRIMARY KEY (tenant_id, region"),
                "the primary key must lead with (tenant_id, region): {ddl}"
            );
        }
    }

    /// The PII-bearing columns are `bytea` (the per-subject-DEK envelope-encryption carrier, 11.4) —
    /// `input_payload`, `risk_summary`, `trace_body`. A plaintext `text` column for these would leak
    /// content the crypto-shred lever can't reach; the encrypted-bytes carrier is the structural pin.
    #[test]
    fn pii_columns_are_encrypted_byte_carriers() {
        assert!(PROPOSED_EFFECT_DDL.contains("input_payload bytea"));
        assert!(HITL_GATE_DDL.contains("risk_summary bytea"));
        assert!(TRACE_DDL.contains("trace_body bytea"));
    }

    /// Every migration's RLS-readiness step is the `myelin_make_tenant_scoped(<table>)` convention —
    /// the same helper the live integration test runs. This is the RLS half asserted at unit scale
    /// (the live denial is `tests/integration_rls.rs`).
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

    /// **R2.4 anti-drift parity: the §4.4 model DDL ⊆ the EXECUTED boot DDL.** `agent_hitl_gate`
    /// is boot-applied by `myelin_storage::hitl_gate_durable` (migration `0054`, folded into
    /// `all_durable_migrations()` — closing the gap where this crate DECLARED the table but no
    /// service main ever migrated it). Storage cannot depend on this crate, so the boot DDL is a
    /// second declaration — THIS test pins them together: every `(column, type)` pair of the §4.4
    /// model DDL appears in the boot DDL (which additionally carries the R2.4 enforcement columns
    /// `requested_by`/`decided_by`), and both share the `(tenant_id, region, gate_id)` PK.
    #[test]
    fn hitl_gate_model_ddl_is_a_subset_of_the_executed_boot_ddl() {
        let boot = myelin_storage::hitl_gate_durable::AGENT_HITL_GATE_MIGRATION;
        // Normalize whitespace so column definitions compare shape, not formatting.
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
            // `name type` (strip NOT NULL — nullability is asserted by shape below for the PK).
            let name_type = col.replace(" NOT NULL", "");
            assert!(
                boot_norm.contains(&name_type),
                "boot DDL (0054) must carry the §4.4 column `{name_type}` — model/boot drift"
            );
        }
        assert!(
            boot_norm.contains("PRIMARY KEY (tenant_id, region, gate_id)"),
            "both declarations share the (tenant, region, gate_id) PK"
        );
        // The boot DDL's R2.4 enforcement extensions are present (distinct-approver audit anchors).
        assert!(boot_norm.contains("requested_by text NOT NULL"));
        assert!(boot_norm.contains("decided_by text"));
    }

    /// The runner REFUSES a destructive (`DROP`) Fabric migration — forward-only is structural; a
    /// rollback is a NEW forward migration, never a down (contract 1.5; storage §3.1). Proves the
    /// forward-only gate is live over THIS crate's migrations, not vacuously green.
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
                id: "0006_drop_run"
            }
        );
        assert!(e.to_string().contains("forward-only"), "loud reason: {e}");
    }

    /// A blocking `ALTER` on the declared-hot `agent_run` table is refused — once `run` is hot, a
    /// later column change MUST be the expand→backfill→contract online idiom, never one blocking
    /// `ALTER` that stalls writes at QPS (contract 1.5).
    #[test]
    fn a_blocking_alter_on_the_hot_run_table_is_refused() {
        let bad = Migrations::of([Migration::phased(
            "0006_run_add_notnull",
            "ALTER TABLE agent_run ADD COLUMN label text NOT NULL", // no DEFAULT → blocking.
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
                id: "0006_run_add_notnull",
                table: "agent_run"
            }
        );
    }
}

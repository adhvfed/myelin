//! The myelin-flow forward-only `(tenant, region)`-first RLS migrations — the SIX-table data
//! model (architecture §3; contracts 1.5, 11.1, 12.1; P-FLOW-01 / P-197, M2).
//!
//! **Owning architecture doc:** `durable-workflow.md` §3 (the data model — `workflow_run` §3.1,
//! `wf_history` §3.2, `wf_timer` §3.3, `wf_signal` §3.4, `wf_activity_attempt` §3.5,
//! `wf_definition` §3.6; carried verbatim from Phase-3 §3). §2 (BUILD/DBOS-class, Postgres-
//! embedded — NO new datastore). **External insight:** `01-process-and-quality-doctrine.md` §2
//! (order-by-non-negotiability — silent data loss outranks every feature: the append-only journal
//! is the source of truth, the `UNIQUE(command_id)` makes journaling idempotent) + §1
//! (name-your-floors). **VISION §3/§4**: references-not-payloads; the EU-sovereign residency-
//! pinned partitioning. Units (architecture §5.1): timers in seconds; timestamps RFC-3339 UTC;
//! budgets/costs integer minor-units, never floats.
//!
//! Six tables — `workflow_run` (§3.1), `wf_history` (§3.2), `wf_timer` (§3.3), `wf_signal` (§3.4),
//! `wf_activity_attempt` (§3.5), `wf_definition` (§3.6) — each (except the global definition
//! registry, see below):
//! - **`(tenant, region)`-first** — `tenant_id text` + `region text` are the leading columns + the
//!   primary-key prefix (12.1, ADR-11): the partition key, from the verified token, never the path.
//!   This is the `residency-pin` / `tenant-predicate` lint floor (every key is tenant-first; there
//!   is no cross-tenant query path).
//! - **RLS-enabled** — a trailing `SELECT myelin_make_tenant_scoped('<table>')` makes the table
//!   `ENABLE`+`FORCE ROW LEVEL SECURITY` + installs the standard `(tenant_id, region)` isolation
//!   policy (the dev/prod Postgres convention, `scripts/pg-init/00-rls-conventions.sql`). With the
//!   app role `NOSUPERUSER NOBYPASSRLS`, a session set to tenant A reads ONLY tenant A's rows.
//! - **forward-only online** — applied through [`myelin_substrate::MigrationRunner`] (contract
//!   1.5): a `DROP` / a destructive rollback is refused. These six are fresh `CREATE TABLE`s
//!   (`Plain`, non-destructive) — the first migration of each table; later columns evolve via the
//!   expand→backfill→contract online idiom.
//! - **references-not-payloads + crypto-shred-capable** — `workflow_run.input` and
//!   `wf_history.result` / `wf_signal.payload` carry IDs/`ArtifactRef`s (jsonb), NEVER PII bodies
//!   (architecture §3.1/§3.2 — a workflow about a PR carries the PR's ref, never the PR body). The
//!   RARE inline-PII result/payload is envelope-encrypted via `result_key_ref` / `payload_key_ref`
//!   so erasure = crypto-shred (§4.8; ADR-12.3). Those key-ref columns are the PII locators tagged
//!   in [`crate::schema`].
//!
//! ## Reconciliation: §3 column names (`tenant uuid`) vs the platform RLS convention (deviation)
//! Architecture §3 names the partition columns `tenant uuid` + `region text`. The platform-wide
//! RLS helper `myelin_make_tenant_scoped` (the ONE dev/prod RLS convention every tenant table uses,
//! storage §3.1 / contract 11.1) requires a `tenant_id text` + `region text` column so its
//! `(tenant_id, region)` isolation policy binds. To keep ONE RLS convention across every subsystem
//! (a second column-naming would fork the platform RLS policy — EI-01 §7 coherence), these
//! migrations name the columns **`tenant_id text` + `region text`** while preserving §3's intent
//! verbatim: `tenant_id`/`region` are the FIRST columns / partition prefix + the RLS isolation key.
//! This matches the `myelin-notif` / `myelin-agent-service` migrations (one convention, not a third
//! dialect). The architecture's `wf_state`/`hist_kind` Postgres ENUM types are realised as `text` +
//! `CHECK` constraints so a forward-only vocabulary EXTENSION (a new history kind) is a non-blocking
//! `CHECK` add, never an enum-rewrite (forward-only, §9) — the same choice `myelin-notif` made.
//!
//! ## The `wf_definition` carve-out (NOT tenant-scoped — by design)
//! `wf_definition` (§3.6) is the GLOBAL versioned definition registry: workflow definitions are
//! CODE (deterministic Rust functions registered at `serve(AppSpec)` boot), not tenant data. Its PK
//! is `(wf_type, version)` and it carries NO `tenant_id`/`region`/PII — a `code_hash` drift-detector
//! and a status, that is all. It is therefore NOT RLS-scoped (there is no tenant column to isolate
//! on) and NOT a `PersonalDataHolder` surface. This is the ONE table here that is not
//! `(tenant, region)`-first, and it is so by construction (a definition is shared platform code).
//!
//! ## FLOOR named — this prompt ships the SCHEMA only
//! No AppSpec wiring (the bootable service shell + the migration RUNNER wiring is **P-FLOW-02** /
//! P-198), no holder registration (the `PersonalDataHolder` auto-registration is **P-FLOW-03** /
//! P-201), no algorithms — WfCtx + journal/outbox co-commit (**P-FLOW-04**), deterministic replay +
//! lease dispatch (**P-FLOW-05**), the executor (**P-FLOW-06**), durable signals (**P-FLOW-09**),
//! durable timers (**P-FLOW-13**) all land later. An empty journal is not a working engine. There
//! is **no mandatory-core algorithm module** here (a `CREATE TABLE` has no decision logic to
//! mutate), so there is **no mutation-score floor** on this prompt (stated explicitly per the
//! template's TESTS field). The live-DB apply + the RLS cross-tenant denial + the idempotency
//! constraints are proven against the dev stack in `tests/integration_flow_schema.rs` (the
//! `integration` cargo feature); the default `cargo build`/`cargo test --workspace` stay DB-free.

use myelin_substrate::{Migration, MigrationPhase, Migrations};

/// The six table names (architecture §3.1..§3.6), in their migration order. Exposed so the
/// integration test can build + RLS-scope each one without restating the names.
pub const TABLES: [&str; 6] = [
    "workflow_run",
    "wf_history",
    "wf_timer",
    "wf_signal",
    "wf_activity_attempt",
    "wf_definition",
];

/// The five `(tenant, region)`-first, RLS-scoped tables (everything except the global
/// `wf_definition` registry — see the module note). The integration test RLS-scopes exactly these.
pub const TENANT_SCOPED_TABLES: [&str; 5] = [
    "workflow_run",
    "wf_history",
    "wf_timer",
    "wf_signal",
    "wf_activity_attempt",
];

/// The `workflow_run` table DDL (§3.1) — the run lifecycle + the durable handle; `(tenant, region)`-
/// first. `state` is `text`+`CHECK` over the frozen six-value lifecycle. `cursor` is the replay
/// short-circuit floor. `budget` is the owned `RunBudget` (jsonb; integer minor-units, never floats).
/// The causality columns (`correlation_id`/`causation_id`/`caused_by`/`depth`) carry the BUS-5
/// causal root + the AG-6 loop-cap counter. `partition`+`lease_owner`/`lease_expires` are the
/// sharded lease-dispatch (§4.7) handles. `input` is references-not-payloads (IDs/`ArtifactRef`s).
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

/// The hot dispatch index on `workflow_run` (§3.1) — the runnable-work scan, leased rows only.
pub const WORKFLOW_RUN_RUNNABLE_IDX: &str =
    "CREATE INDEX wf_runnable ON workflow_run (partition, lease_expires) WHERE state IN ('running')";

/// The `wf_history` table DDL (§3.2) — the append-only journal, the SOURCE OF TRUTH;
/// `(tenant, region)`-first. `command_id` is DETERMINISTIC from the workflow position (the
/// replay-match key); `UNIQUE(tenant_id, run_id, command_id)` makes journaling idempotent (a crash
/// between "do the activity" and "journal its result" replays safely — the second insert is a no-op).
/// `result` is references-not-payloads; `result_key_ref` envelope-encrypts the rare inline-PII
/// result for crypto-shred (§4.8). `kind` is `text`+`CHECK` over the frozen history vocabulary.
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

/// The `wf_timer` table DDL (§3.3) — the durable timer (powers SC-11: millions of timers);
/// `(tenant, region)`-first. `bucket = epoch_minute(fire_at)` + the partial index
/// `(bucket, partition) WHERE NOT fired` is the world-scale move (a 30-day timer is never read until
/// its minute). `run_id` is the workflow to wake (NULL for a bare SLA timer); `command_id` is the
/// `wf_history` command this timer satisfies.
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

/// The hot dispatch index on `wf_timer` (§3.3) — only the imminent, UNFIRED bucket is scanned (the
/// SC-11 partial index that makes "millions of durable timers" an indexed range read, not a scan).
pub const WF_TIMER_DUE_IDX: &str =
    "CREATE INDEX wf_timer_due ON wf_timer (bucket, partition) WHERE NOT fired";

/// The `wf_signal` table DDL (§3.4) — durably-BUFFERED inbound signals (powers multi-day HITL
/// waits); `(tenant, region)`-first. The PK `(tenant_id, run_id, signal_name, idem_key)` is EXACTLY
/// what makes the per-effect `idem_key` rule (§6.4) and the `SCHEDULE_AND_RUN_JOB` handshake (§4.9)
/// idempotent by construction (a re-posted approval is a no-op). `payload` is references-not-
/// payloads; `payload_key_ref` crypto-shreds inline PII. `consumed_seq` NULL = buffered, unconsumed.
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

/// The pending-signal index on `wf_signal` (§3.4) — the buffered-unconsumed lookup (the wait wakes
/// on it). `(tenant, region)`-first dispatch; `region` rides so the partial index stays tenant-local.
pub const WF_SIGNAL_PENDING_IDX: &str =
    "CREATE INDEX wf_signal_pending ON wf_signal (tenant_id, region, run_id) WHERE consumed_seq IS NULL";

/// The `wf_activity_attempt` table DDL (§3.5) — the idempotency ledger; `(tenant, region)`-first.
/// `idem_token` bridges to BUS-2 so a retried emit is broker-deduped (an activity retried after a
/// crash that DID emit produces a broker-deduped event, not a duplicate). The `SCHEDULE_AND_RUN_JOB`
/// dispatch is one such attempt (§4.9). PK leads with `(tenant_id, region, run_id, command_id,
/// attempt)`.
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

/// The `wf_definition` table DDL (§3.6) — the GLOBAL versioned definition registry. Definitions are
/// CODE (deterministic Rust functions registered at boot), NOT tenant data: NO `tenant_id`/`region`/
/// PII, a `code_hash` drift-detector + a status, PK `(wf_type, version)`. A run is pinned to its
/// `wf_version` at start (§4.6) so a deploy cannot diverge an in-flight run. NOT RLS-scoped (no
/// tenant column to isolate) — the ONE non-tenant table here, by construction (see the module note).
pub const WF_DEFINITION_DDL: &str = "\
CREATE TABLE wf_definition (\
  wf_type text NOT NULL, \
  version integer NOT NULL, \
  code_hash text NOT NULL, \
  status text NOT NULL CHECK (status IN ('active','draining','retired')), \
  registered_at timestamptz NOT NULL DEFAULT now(), \
  PRIMARY KEY (wf_type, version))";

/// Online expand for the durable control plane. Existing runs remain readable with NULL values;
/// every new Postgres-backed start writes its idempotency key, and cancel writes a machine reason.
pub const WORKFLOW_RUN_CONTROL_EXPAND_DDL: &str = "\
ALTER TABLE workflow_run ADD COLUMN IF NOT EXISTS idem_key text;
ALTER TABLE workflow_run ADD COLUMN IF NOT EXISTS cancel_reason text";

/// Idempotent start is enforced by Postgres, not a read-then-write process mutex. This is separate
/// from the column expand because PostgreSQL requires `CONCURRENTLY` outside a transaction/script.
pub const WORKFLOW_RUN_IDEM_INDEX_DDL: &str = "\
CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS wf_run_idem \
ON workflow_run (tenant_id, region, idem_key) WHERE idem_key IS NOT NULL";

/// Online lease-fencing expansion for the PostgreSQL drive store. `lease_epoch` closes the
/// same-owner ABA window: every claim increments it, and renew/commit require the exact epoch that
/// was claimed. The last drive id/fingerprint make a deterministic retry after commit observable as
/// success while rejecting a different batch under the same id.
pub const WORKFLOW_RUN_DRIVE_EXPAND_DDL: &str = "\
ALTER TABLE workflow_run ADD COLUMN IF NOT EXISTS lease_epoch bigint NOT NULL DEFAULT 0;
ALTER TABLE workflow_run ADD COLUMN IF NOT EXISTS last_drive_id text;
ALTER TABLE workflow_run ADD COLUMN IF NOT EXISTS last_drive_fingerprint text";

/// Tenant- and residency-leading runnable claim index. The historical index remains valid for
/// existing deployments; this additive online index makes the production claim avoid scanning a
/// cell-wide partition before applying the verified `(tenant_id, region)` scope.
pub const WORKFLOW_RUN_SCOPED_RUNNABLE_INDEX_DDL: &str = "\
CREATE INDEX CONCURRENTLY IF NOT EXISTS wf_runnable_scoped \
ON workflow_run (tenant_id, region, partition, lease_expires, updated_at, run_id) \
WHERE state = 'running'";

/// The six `(table_id, ddl, table_name, rls_scoped)` tuples in migration order — the data-model
/// slice. Each `ddl` is the fresh `CREATE TABLE` above (plus its hot dispatch index where §3 names
/// one). `rls_scoped = true` rides a `myelin_make_tenant_scoped('<table>')` RLS-scope call on the
/// same forward migration; `wf_definition` (the global registry) is `false` (no tenant column).
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

/// The dispatch indexes that ride their table's forward migration (§3.1/§3.3/§3.4 partial indexes).
/// Keyed by table so they are appended to the `CREATE TABLE` migration (a fresh empty table — no hot
/// lock; `CREATE INDEX` is non-concurrent-safe before any rows exist).
const TABLE_INDEXES: &[(&str, &str)] = &[
    ("workflow_run", WORKFLOW_RUN_RUNNABLE_IDX),
    ("wf_timer", WF_TIMER_DUE_IDX),
    ("wf_signal", WF_SIGNAL_PENDING_IDX),
];

/// The `myelin_make_tenant_scoped(<table>)` RLS-readiness call each tenant-scoped migration emits
/// AFTER its `CREATE TABLE` (the dev/prod Postgres convention, `scripts/pg-init/00-rls-conventions.sql`).
/// The integration test runs exactly this against the live stack; the string makes the RLS step
/// visible + asserted-in-tests. myelin-flow does NOT fork the RLS policy — it uses the ONE helper
/// every tenant table uses (EI-01 §7).
pub fn rls_scope_sql(table: &str) -> String {
    format!("SELECT myelin_make_tenant_scoped('{table}')")
}

/// The myelin-flow data-model migration set (contract 1.5), built through the substrate framework so
/// the boot-time RUNNER applies it forward-only AND the `forward-only-migration` lint reads it at
/// source-scan. Six [`Migration`]s — one fresh `CREATE TABLE` per table (`MigrationPhase::Plain` — a
/// new table needs no expand→backfill→contract discipline), each carrying its dispatch index (where
/// §3 names one) + its RLS-scope call (for the five tenant tables) riding the same forward migration
/// (an empty fresh table; no hot-table lock). The DDL is held as `&str` constants (NOT mistaken for
/// live Rust by the lint), then assembled + `'static`-leaked once at boot.
pub fn migrations() -> Migrations {
    let mut items = TABLE_DDLS
        .iter()
        .map(|(id, create_ddl, table, rls_scoped)| {
            let mut ddl = String::new();
            ddl.push_str(create_ddl);
            ddl.push(';');
            for (idx_table, idx_ddl) in TABLE_INDEXES {
                if idx_table == table {
                    ddl.push('\n');
                    ddl.push_str(idx_ddl);
                    ddl.push(';');
                }
            }
            if *rls_scoped {
                ddl.push('\n');
                ddl.push_str(&rls_scope_sql(table));
                ddl.push(';');
            }
            // One-time, bounded leak — the migration set is built once at boot/serve; the substrate
            // `Migration` holds `&'static str` (the same pattern `myelin-notif` uses).
            let ddl: &'static str = Box::leak(ddl.into_boxed_str());
            // A table does not become hot until this first create finishes. The historical
            // workflow_run migration also builds its initial index non-concurrently while the
            // table is empty, so it deliberately has no hot-table target. Every later
            // workflow_run migration below names the table and is checked as hot.
            if *table == "workflow_run" {
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
    Migrations::of(items)
}

/// Whether `ddl` is forward-only-LEGAL (no destructive `DROP`, no down/rollback). The framework's
/// [`myelin_substrate::is_destructive`] / the `forward-only-migration` lint enforce this at boot +
/// source-scan; this is the in-module structural assertion the migration test rests on without a
/// live DB.
pub fn ddl_is_forward_only(ddl: &str) -> bool {
    !myelin_substrate::is_destructive(ddl)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_substrate::{HotTables, MigrationRunner};

    /// THE forward-only ADMIT proof (the GATE): the six `(tenant, region)`-first migrations apply
    /// forward-only through the substrate runner — 6 tables created, 0 destructive, in order. The
    /// applied ids are recorded in migration order (contract 1.5).
    #[test]
    fn the_schema_and_control_migrations_apply_forward_only_in_order() {
        let migrations = migrations();
        assert_eq!(
            migrations.0.len(),
            10,
            "six-table data model plus four online control/drive expansions"
        );
        let mut runner = MigrationRunner::new();
        // workflow_run becomes hot after creation; its two follow-ons use the online expand path.
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
            ],
            "tables then online control expands, in order — 0 backward migration"
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

    /// Every TENANT-scoped table is `(tenant, region)`-first: the DDL leads with `tenant_id` then
    /// `region`, and the primary key leads with `(tenant_id, region` (12.1 — the residency-pin /
    /// tenant-predicate floor, no cross-tenant query path). `wf_definition` is the documented
    /// exception (the global definition registry, PK `(wf_type, version)`, no tenant column — §3.6).
    #[test]
    fn every_tenant_table_is_tenant_region_first_with_a_tenant_first_pk() {
        for (_id, ddl, table, rls_scoped) in TABLE_DDLS {
            if !rls_scoped {
                // wf_definition: the global registry — PK (wf_type, version), no tenant column.
                assert_eq!(
                    *table, "wf_definition",
                    "the only non-tenant table is wf_definition (§3.6)"
                );
                assert!(
                    ddl.contains("PRIMARY KEY (wf_type, version)"),
                    "wf_definition is keyed by (wf_type, version) — definitions are code, not tenant data"
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
            // Every tenant table's PK LEADS with `tenant_id` (12.1 — the no-cross-tenant-query floor;
            // RLS isolates on `(tenant_id, region)` regardless of PK shape). `wf_signal` keys on
            // `(tenant_id, run_id, signal_name, idem_key)` — the per-effect idempotency anchor (§3.4)
            // — so its PK omits `region` (region rides as a column, still RLS-scoped); every other
            // tenant table's PK leads with `(tenant_id, region`.
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

    /// `command_id` is DETERMINISTIC from the workflow position (the replay-match key) and the
    /// `UNIQUE(tenant_id, run_id, command_id)` on `wf_history` makes journaling idempotent — the
    /// silent-data-loss floor: a crash between "do the activity" and "journal its result" replays
    /// safely (§3.2). The history kind vocabulary is `CHECK`-constrained (forward-only extensible).
    #[test]
    fn wf_history_journal_is_idempotent_by_construction() {
        // The command-id idempotency UNIQUE — the replay-safe journal key (§3.2).
        assert!(
            WF_HISTORY_DDL.contains("UNIQUE (tenant_id, run_id, command_id)"),
            "wf_history journaling is idempotent on (tenant, run_id, command_id) (§3.2)"
        );
        // command_id is a column (deterministic from the workflow position; the replay-match key).
        assert!(
            WF_HISTORY_DDL.contains("command_id text NOT NULL"),
            "the deterministic replay-match key"
        );
        // append-only journal: the (tenant, region, run_id, seq) replay-order PK.
        assert!(
            WF_HISTORY_DDL.contains("PRIMARY KEY (tenant_id, region, run_id, seq)"),
            "the per-run monotonic seq is the replay order (§3.2)"
        );
        // the inline-PII crypto-shred lever (envelope-encryption key ref, §4.8).
        assert!(
            WF_HISTORY_DDL.contains("result_key_ref text"),
            "the inline-PII crypto-shred key ref (§4.8)"
        );
    }

    /// The `wf_signal` PK `(tenant_id, run_id, signal_name, idem_key)` is EXACTLY what makes the
    /// per-effect `idem_key` rule (§6.4) and the `SCHEDULE_AND_RUN_JOB` handshake (§4.9) idempotent
    /// by construction — a re-posted signal is a no-op (the multi-day HITL durability anchor, §3.4).
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
        // payload is references-not-payloads (jsonb refs, never a PII body).
        assert!(
            WF_SIGNAL_DDL.contains("payload jsonb NOT NULL"),
            "the signal payload is refs-not-payloads (§3.4)"
        );
    }

    /// The `wf_timer` partial index `(bucket, partition) WHERE NOT fired` is the SC-11 world-scale
    /// move: a 30-day timer sits in a far-future bucket, never read until its minute (§3.3). The
    /// `bucket = epoch_minute(fire_at)` column + the partial index make "millions of timers" an
    /// indexed range read, not a table scan.
    #[test]
    fn wf_timer_partial_index_covers_bucket_partition_where_not_fired() {
        let m = migrations();
        let timer =
            m.0.iter()
                .find(|m| m.table == Some("wf_timer"))
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

    /// The `wf_activity_attempt` ledger carries the `idem_token` bridge to BUS-2 (a retried emit is
    /// broker-deduped) keyed by `(tenant, region, run_id, command_id, attempt)` (§3.5).
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

    /// `workflow_run` carries the §3.1 lifecycle invariants: the frozen six-state `CHECK`, the
    /// `cursor` replay short-circuit, the `budget` RunBudget, the BUS-5 causality columns, and the
    /// lease-dispatch handles. `input` is references-not-payloads.
    #[test]
    fn workflow_run_carries_the_3_1_lifecycle_invariants() {
        let ddl = WORKFLOW_RUN_DDL;
        // the frozen six-state lifecycle (§3.1) as a CHECK (forward-only extensible).
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
        // the BUS-5 causality columns + the AG-6 loop-cap depth counter.
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
        // the §4.7 sharded lease-dispatch handles.
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
        // input is references-not-payloads (jsonb refs, never a PII body — §3.1).
        assert!(
            ddl.contains("input jsonb NOT NULL"),
            "the input is refs-not-payloads (§3.1)"
        );
    }

    /// `wf_definition` (§3.6) is the GLOBAL versioned registry: definitions are CODE, not tenant
    /// data — `(wf_type, version)` PK, a `code_hash` drift-detector, a status, NO tenant column and
    /// NO PII. A run pins to its `wf_version` at start (§4.6).
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

    /// Each tenant-scoped table's RLS-readiness step is the `myelin_make_tenant_scoped(<table>)`
    /// convention — the SAME helper the live integration test runs (myelin-flow does not fork the
    /// RLS policy, EI-01 §7). `wf_definition` (the global registry) carries NO RLS-scope call (no
    /// tenant column to isolate).
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

    /// The runner REFUSES a destructive (`DROP`) flow migration — forward-only is structural; a
    /// rollback is a NEW forward migration, never a `down` (contract 1.5). Proves the forward-only
    /// gate is LIVE over THIS crate's migrations, not vacuously green.
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
        // the assembled real migration set is forward-only-legal (no DROP anywhere).
        for (_id, ddl, _table, _rls) in TABLE_DDLS {
            assert!(ddl_is_forward_only(ddl), "the real DDL is forward-only");
            assert!(
                !ddl.to_ascii_uppercase().contains("DROP"),
                "no DROP in the data-model DDL"
            );
        }
    }
}

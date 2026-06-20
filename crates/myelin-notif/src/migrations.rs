//! The Notif forward-only `(tenant, region)`-first RLS migrations — the nine-table data model
//! (architecture §2; contracts 1.5, 11.1, 12.1/12.4; NOTIF-P2 / P-180, M2).
//!
//! **Owning architecture doc:** `notifications.md` §2 (the data model), §2.1 (the `inbox_item`
//! load-bearing invariants). The column shapes are Phase-3 §2.1..§2.6 (cited-not-restated by refined
//! §2). **External insight:** `01-process-and-quality-doctrine.md` §1 (name-your-floors — the
//! read-state truth is ONE column, named not implicit), §5 (the committed ratchet — the residency-pin
//! and no-untagged-personal-data lints are gates). **VISION §3**: references-not-payloads; the
//! EU-sovereign residency-pinned partitioning.
//!
//! Nine tables — `inbox_item` (§2.1), `notif_pref` / `quiet_hours` (§2.2), `delivery` (§2.3),
//! `oncall_schedule` / `escalation_policy` / `escalation_run` (§2.4), `humanise_template` (§2.5),
//! `mute` (§2.6) — each:
//! - **`(tenant, region)`-first** — `tenant_id text` + `region text` are the leading columns + the
//!   primary-key prefix (12.1, ADR-11): the partition key, from the verified token, never the path.
//!   This is the `residency-pin` / `tenant-predicate` lint floor (every key is tenant-first; there is
//!   no cross-tenant query path). **Exception, by design:** `humanise_template` admits a NULL tenant
//!   (the platform-default row, §2.5) via `COALESCE(tenant_id, '<platform>')` in its PK.
//! - **RLS-enabled** — a trailing `SELECT myelin_make_tenant_scoped('<table>')` makes the table
//!   `ENABLE`+`FORCE ROW LEVEL SECURITY` + installs the standard `(tenant_id, region)` isolation
//!   policy (the dev/prod Postgres convention, `scripts/pg-init/00-rls-conventions.sql`). With the app
//!   role `NOSUPERUSER NOBYPASSRLS`, a session set to tenant A reads ONLY tenant A's rows.
//! - **forward-only online** — applied through [`myelin_substrate::MigrationRunner`] (contract 1.5):
//!   a `DROP` / a destructive rollback is refused. These nine are fresh `CREATE TABLE`s (`Plain`,
//!   non-destructive) — the first migration of each table; later columns evolve via the
//!   expand→backfill→contract online idiom.
//! - **encrypted-from-birth + residency-pinned** — every row carries a `dek_ref` per-tenant DEK ref
//!   (contract 11.3/11.4) so the bulk columns seal under the per-tenant DEK from the FIRST insert
//!   (the tenant-decommission crypto-shred unit). The PII columns are tagged in [`crate::schema`]
//!   (the `#[personal_data(...)]` classification, contract 10.2) — here the DDL declares the columns.
//!
//! ## Reconciliation: §2.1 column names vs the platform RLS convention (documented deviation)
//! Architecture §2.1 names the partition column `tenant uuid`. The platform-wide RLS helper
//! `myelin_make_tenant_scoped` (the ONE dev/prod RLS convention every tenant table uses, storage §3.1
//! / contract 11.1) requires a `tenant_id text` + `region text` column so its `(tenant_id, region)`
//! isolation policy binds. To keep ONE RLS convention across every subsystem (a second column-naming
//! would fork the platform RLS policy — EI-01 §7 coherence), these migrations name the columns
//! **`tenant_id text` + `region text`** while preserving §2.1's intent verbatim: `tenant_id`/`region`
//! are the FIRST columns / partition prefix + the RLS isolation key. This matches the
//! `myelin-refs-service` / `myelin-agent-service` migrations (one convention, not a third dialect).
//! The architecture's `notif_reason`/`notif_class`/`item_state`/… Postgres ENUM types are realised as
//! `text` + `CHECK` constraints so a forward-only vocabulary EXTENSION (a new reason) is a
//! non-blocking `CHECK` add, never an enum-rewrite (forward-only, §9) — the same choice
//! `myelin-refs-service` made for `edge_rel`.
//!
//! ## FLOOR named — the WRITERS land later; this prompt ships the SCHEMA only
//! The rows are written by later prompts: the Signal-consumer **router UPSERTs `inbox_item`**
//! (NOTIF-P3 / P-181); **prefs/quiet-hours** by NOTIF-P10; **delivery** by NOTIF-P16;
//! **on-call/escalation** by NOTIF-P14; **`humanise_template`** by NOTIF-P9; **`mute`** by NOTIF-P15.
//! An empty table is not a working inbox. There is **no mandatory-core algorithm module** here
//! (a `CREATE TABLE` has no decision logic to mutate), so there is **no mutation-score floor** on this
//! prompt (stated explicitly per the template's TESTS field). The live-DB apply + the RLS cross-tenant
//! denial is proven against the dev stack in `tests/integration_notif_schema.rs` (the `integration`
//! cargo feature); the default `cargo build`/`cargo test --workspace` stay DB-free. The world-scale
//! migration-under-load drill (SUB-D10) is a substrate floor (P-S34), not re-proven here.

use myelin_substrate::{Migration, MigrationPhase, Migrations};

/// The nine table names (architecture §2.1..§2.6), in their migration order. Exposed so the
/// integration test can build + RLS-scope each one without restating the names.
pub const TABLES: [&str; 9] = [
    "notif_inbox_item",
    "notif_pref",
    "notif_quiet_hours",
    "notif_delivery",
    "notif_oncall_schedule",
    "notif_escalation_policy",
    "notif_escalation_run",
    "notif_humanise_template",
    "notif_mute",
];

/// The `inbox_item` table DDL (§2.1) — the heart: refs-not-payloads, ONE read-state column, the
/// `UNIQUE(tenant_id, recipient, dedup_key)` write-time-collapse key. `(tenant, region)`-first.
///
/// - `subject` / `subject_root` / `origin_event` / `template_args_json` hold **`ArtifactRef`s**
///   (URN text), never rendered strings (the NOTIF-1 invariant — humanise resolves per-viewer at read
///   time, NOTIF-P9). `template_args_json` is a `jsonb` ref-array (`["myelin://…", …]`).
/// - `reason` / `class` / `state` are `text` + `CHECK` over the frozen vocabularies (forward-only
///   extensible, no enum-rewrite). `state` is the **ONE** read-state column (the C-9 truth).
/// - `coalesce_count` is the "+N more" counter (NOTIF-P11); `dedup_key` + the `UNIQUE` make
///   storm-control a write-time UPSERT (§3.2).
pub const INBOX_ITEM_DDL: &str = "\
CREATE TABLE notif_inbox_item (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  item_id text NOT NULL, \
  recipient text NOT NULL, \
  subject text NOT NULL, \
  subject_root text NOT NULL, \
  reason text NOT NULL CHECK (reason IN ('approval_requested','escalated','sla','review_requested','assigned','mentioned','replied','agent_proposal','watched','state_changed','fyi','blocked','unblocked','thread_watched','shared','comments')), \
  class text NOT NULL CHECK (class IN ('critical','direct','participating','watching','fyi')), \
  origin_event text NOT NULL, \
  template_key text NOT NULL, \
  template_args_json jsonb NOT NULL, \
  dedup_key text NOT NULL, \
  coalesce_count integer NOT NULL DEFAULT 1, \
  state text NOT NULL DEFAULT 'unread' CHECK (state IN ('unread','seen','read','snoozed','archived','done')), \
  snooze_until timestamptz, \
  occurred_at timestamptz NOT NULL, \
  created_at timestamptz NOT NULL DEFAULT now(), \
  dek_ref text NOT NULL, \
  PRIMARY KEY (tenant_id, region, recipient, item_id), \
  UNIQUE (tenant_id, recipient, dedup_key))";

/// The `notif_pref` table DDL (§2.2) — per-principal channel routing; `(tenant, region)`-first. The
/// matcher binds the frozen `QueryAst` (NOTIF-P10). `routing` / `digest` are `jsonb` config blobs.
pub const NOTIF_PREF_DDL: &str = "\
CREATE TABLE notif_pref (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  principal text NOT NULL, \
  routing jsonb NOT NULL, \
  digest jsonb, \
  dek_ref text NOT NULL, \
  PRIMARY KEY (tenant_id, region, principal))";

/// The `quiet_hours` table DDL (§2.2) — per-principal quiet windows in the recipient's tz;
/// `(tenant, region)`-first. `pierce_classes` defaults to `{critical}` (the on-call override —
/// critical/escalated pierce quiet-hours, §2.2). Written by NOTIF-P10.
pub const QUIET_HOURS_DDL: &str = "\
CREATE TABLE notif_quiet_hours (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  principal text NOT NULL, \
  tz text NOT NULL, \
  windows jsonb NOT NULL, \
  dnd_until timestamptz, \
  pierce_classes text[] NOT NULL DEFAULT '{critical}', \
  dek_ref text NOT NULL, \
  PRIMARY KEY (tenant_id, region, principal))";

/// The `delivery` table DDL (§2.3) — the at-least-once + idempotent channel ledger;
/// `(tenant, region)`-first. `UNIQUE(tenant_id, idem_key)` collapses a retried send to ONE delivery
/// (NOTIF-P16). `redacted` is the off-cell PII-minimisation flag (§3.6). Written by NOTIF-P16.
pub const DELIVERY_DDL: &str = "\
CREATE TABLE notif_delivery (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  delivery_id text NOT NULL, \
  item_id text NOT NULL, \
  recipient text NOT NULL, \
  channel text NOT NULL CHECK (channel IN ('in_app','web_push','mobile_push','email','desktop')), \
  adapter text NOT NULL, \
  idem_key text NOT NULL, \
  state text NOT NULL CHECK (state IN ('pending','sent','delivered','bounced','failed','suppressed')), \
  attempts integer NOT NULL DEFAULT 0, \
  provider_ref text, \
  redacted boolean NOT NULL DEFAULT false, \
  created_at timestamptz NOT NULL DEFAULT now(), \
  sent_at timestamptz, \
  dek_ref text NOT NULL, \
  PRIMARY KEY (tenant_id, region, delivery_id), \
  UNIQUE (tenant_id, idem_key))";

/// The `oncall_schedule` table DDL (§2.4) — a rotation roster; `(tenant, region)`-first. `rotation`
/// is a `jsonb` roster of OPAQUE principal pseudonyms (tagged in [`crate::schema`]). Written by
/// NOTIF-P14.
pub const ONCALL_SCHEDULE_DDL: &str = "\
CREATE TABLE notif_oncall_schedule (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  schedule_id text NOT NULL, \
  schedule_name text NOT NULL, \
  rotation jsonb NOT NULL, \
  tz text NOT NULL, \
  dek_ref text NOT NULL, \
  PRIMARY KEY (tenant_id, region, schedule_id))";

/// The `escalation_policy` table DDL (§2.4) — the frozen chain config (the C3 shape an SLA/on-call
/// producer passes to Notif); `(tenant, region)`-first. `steps` is the ordered `jsonb` step list.
/// Written by NOTIF-P14.
pub const ESCALATION_POLICY_DDL: &str = "\
CREATE TABLE notif_escalation_policy (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  policy_id text NOT NULL, \
  policy_name text NOT NULL, \
  steps jsonb NOT NULL, \
  repeat integer NOT NULL DEFAULT 1, \
  ack_window interval NOT NULL, \
  dek_ref text NOT NULL, \
  PRIMARY KEY (tenant_id, region, policy_id))";

/// The `escalation_run` table DDL (§2.4) — a LIVE escalation (a `myelin-flow` durable-workflow
/// instance handle); `(tenant, region)`-first. The state machine + timers live in the durable-workflow
/// engine (ADR-09); this is the policy handle + run state. `acked_by` is an OPAQUE pseudonym (tagged
/// in [`crate::schema`]). Written by NOTIF-P14.
pub const ESCALATION_RUN_DDL: &str = "\
CREATE TABLE notif_escalation_run (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  run_id text NOT NULL, \
  policy_id text NOT NULL, \
  trigger_event text NOT NULL, \
  workflow_ref text NOT NULL, \
  current_step integer NOT NULL DEFAULT 0, \
  state text NOT NULL CHECK (state IN ('active','acked','resolved','exhausted')), \
  acked_by text, \
  acked_at timestamptz, \
  dek_ref text NOT NULL, \
  PRIMARY KEY (tenant_id, region, run_id))";

/// The `humanise_template` table DDL (§2.5) — the ONE platform templating store (ICU MessageFormat,
/// platform-defaulted + tenant/locale-overridable). The ONLY table whose `tenant_id` is NULLABLE: a
/// NULL-tenant row is the platform default; a tenant row overrides (brand/locale). The PK uses
/// `COALESCE(tenant_id, '<platform>')` so the platform default + a tenant override coexist (§2.5).
/// Written by NOTIF-P9.
pub const HUMANISE_TEMPLATE_DDL: &str = "\
CREATE TABLE notif_humanise_template (\
  tenant_id text, \
  region text NOT NULL, \
  template_key text NOT NULL, \
  locale text NOT NULL DEFAULT 'en', \
  template_body text NOT NULL, \
  dek_ref text NOT NULL, \
  PRIMARY KEY (COALESCE(tenant_id, '00000000-0000-0000-0000-000000000000'), region, template_key, locale))";

/// The `mute` table DDL (§2.6) — per-principal thread/subject mutes; `(tenant, region)`-first.
/// `subject_root` is a ref-root (a chat thread / a PR), never a payload. Suppresses delivery, never
/// the audit (NOTIF-P11). Written by NOTIF-P15.
pub const MUTE_DDL: &str = "\
CREATE TABLE notif_mute (\
  tenant_id text NOT NULL, \
  region text NOT NULL, \
  principal text NOT NULL, \
  subject_root text NOT NULL, \
  until timestamptz, \
  dek_ref text NOT NULL, \
  PRIMARY KEY (tenant_id, region, principal, subject_root))";

/// The nine `(table_id, create_ddl, table_name)` tuples in migration order — the data-model slice.
/// Each `create_ddl` is the fresh `CREATE TABLE` above; the migration set rides each one with its
/// `myelin_make_tenant_scoped('<table>')` RLS-scope call so the table is RLS-on from creation.
const TABLE_DDLS: &[(&str, &str, &str)] = &[
    ("notif_0001_inbox_item", INBOX_ITEM_DDL, "notif_inbox_item"),
    ("notif_0002_pref", NOTIF_PREF_DDL, "notif_pref"),
    ("notif_0003_quiet_hours", QUIET_HOURS_DDL, "notif_quiet_hours"),
    ("notif_0004_delivery", DELIVERY_DDL, "notif_delivery"),
    ("notif_0005_oncall_schedule", ONCALL_SCHEDULE_DDL, "notif_oncall_schedule"),
    ("notif_0006_escalation_policy", ESCALATION_POLICY_DDL, "notif_escalation_policy"),
    ("notif_0007_escalation_run", ESCALATION_RUN_DDL, "notif_escalation_run"),
    ("notif_0008_humanise_template", HUMANISE_TEMPLATE_DDL, "notif_humanise_template"),
    ("notif_0009_mute", MUTE_DDL, "notif_mute"),
];

/// The `myelin_make_tenant_scoped(<table>)` RLS-readiness call each tenant-scoped migration emits
/// AFTER its `CREATE TABLE` (the dev/prod Postgres convention, `scripts/pg-init/00-rls-conventions.sql`).
/// The integration test runs exactly this against the live stack; the string makes the RLS step
/// visible + asserted-in-tests. Notif does NOT fork the RLS policy — it uses the ONE helper every
/// tenant table uses (EI-01 §7).
pub fn rls_scope_sql(table: &str) -> String {
    format!("SELECT myelin_make_tenant_scoped('{table}')")
}

/// The Notif data-model migration set (contract 1.5), built through the substrate framework so the
/// boot-time RUNNER applies it forward-only AND the `forward-only-migration` lint reads it at
/// source-scan. Nine [`Migration`]s — one fresh `CREATE TABLE` per table (`MigrationPhase::Plain` —
/// a new table needs no expand→backfill→contract discipline), each with its RLS-scope call riding
/// the same forward migration (an empty fresh table; no hot-table lock). The DDL is held as `&str`
/// constants (NOT mistaken for live Rust by the lint), then assembled + `'static`-leaked once at
/// boot (the same shape the framework expects, like `myelin-refs-service`).
pub fn migrations() -> Migrations {
    Migrations::of(TABLE_DDLS.iter().map(|(id, create_ddl, table)| {
        let mut ddl = String::new();
        ddl.push_str(create_ddl);
        ddl.push_str(";\n");
        ddl.push_str(&rls_scope_sql(table));
        ddl.push(';');
        // One-time, bounded leak — the migration set is built once at boot/serve; the substrate
        // `Migration` holds `&'static str` (the same pattern `myelin-refs-service` uses).
        let ddl: &'static str = Box::leak(ddl.into_boxed_str());
        Migration::phased(id, ddl, MigrationPhase::Plain, table)
    }))
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

    /// THE forward-only ADMIT proof (the GATE): the nine `(tenant, region)`-first migrations apply
    /// forward-only through the substrate runner — 9 tables created, 0 destructive, in order. The
    /// applied ids are recorded in migration order (contract 1.5).
    #[test]
    fn the_nine_migrations_apply_forward_only_in_order() {
        let migrations = migrations();
        assert_eq!(migrations.0.len(), 9, "the nine-table data model (§2.1..§2.6)");
        let mut runner = MigrationRunner::new();
        // The nine tables are brand-new `CREATE TABLE`s (cold at creation) — `none()` hot set.
        runner.run(&migrations, &HotTables::none()).expect("the nine migrations apply forward-only");
        assert_eq!(
            runner.applied(),
            &[
                "notif_0001_inbox_item",
                "notif_0002_pref",
                "notif_0003_quiet_hours",
                "notif_0004_delivery",
                "notif_0005_oncall_schedule",
                "notif_0006_escalation_policy",
                "notif_0007_escalation_run",
                "notif_0008_humanise_template",
                "notif_0009_mute",
            ],
            "9 tables created, in order — 0 backward migration"
        );
    }

    /// Every migration is `(tenant, region)`-first: the DDL leads with `tenant_id` then `region`,
    /// and the primary key leads with `(tenant_id, region` (12.1 — the residency-pin / tenant-predicate
    /// floor, no cross-tenant query path). `humanise_template` is the ONE exception by design (its PK
    /// is `COALESCE(tenant_id, …)` for the platform-default NULL-tenant row, §2.5) — still tenant-first.
    #[test]
    fn every_table_is_tenant_region_first_with_a_tenant_first_pk() {
        for (_id, ddl, table) in TABLE_DDLS {
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
            if *table == "notif_humanise_template" {
                // The platform-default exception: tenant-first via COALESCE (§2.5).
                assert!(
                    ddl.contains("PRIMARY KEY (COALESCE(tenant_id"),
                    "humanise_template PK is COALESCE(tenant_id, …) (the platform-default NULL row, §2.5)"
                );
            } else {
                assert!(
                    ddl.contains("PRIMARY KEY (tenant_id, region"),
                    "the primary key must lead with (tenant_id, region) ({table}): {ddl}"
                );
            }
        }
    }

    /// The `inbox_item` load-bearing invariants (§2.1): refs-not-payloads (`subject`/`subject_root`/
    /// `origin_event`/`template_args_json` carry refs, never rendered strings); EXACTLY ONE read-state
    /// column (`state`); the `UNIQUE(tenant_id, recipient, dedup_key)` write-time-collapse key; the
    /// `coalesce_count` "+N more" counter; the `origin_event` + `reason` provenance.
    #[test]
    fn inbox_item_carries_the_2_1_invariants() {
        let ddl = INBOX_ITEM_DDL;
        // refs-not-payloads: the subject/root/origin are text URN refs (humanise resolves at read).
        for col in ["subject text", "subject_root text", "origin_event text", "template_args_json jsonb"] {
            assert!(ddl.contains(col), "the refs-not-payloads column `{col}` is declared");
        }
        // EXACTLY ONE read-state column: `state` appears exactly once as a column declaration.
        assert_eq!(
            ddl.matches("state text").count(),
            1,
            "inbox_item has EXACTLY ONE read-state column (the C-9 truth, §2.1)"
        );
        // the write-time-collapse key + the "+N more" counter + the provenance.
        assert!(
            ddl.contains("UNIQUE (tenant_id, recipient, dedup_key)"),
            "the UNIQUE(tenant, recipient, dedup_key) write-time-collapse key (§3.2)"
        );
        assert!(ddl.contains("coalesce_count integer"), "the +N-more counter (NOTIF-P11)");
        assert!(ddl.contains("origin_event text"), "the NOTIF-2 origin_event provenance");
        assert!(ddl.contains("reason text"), "the NOTIF-2 reason provenance");
    }

    /// The `delivery` table is at-least-once + idempotent: `UNIQUE(tenant_id, idem_key)` collapses a
    /// retried send to ONE delivery (NOTIF-P16); the `redacted` off-cell PII-minimisation flag exists
    /// (§2.3 / §3.6).
    #[test]
    fn delivery_is_at_least_once_idempotent_with_a_redacted_flag() {
        assert!(
            DELIVERY_DDL.contains("UNIQUE (tenant_id, idem_key)"),
            "delivery is idempotent on idem_key (at-least-once + dedup, §2.3)"
        );
        assert!(DELIVERY_DDL.contains("redacted boolean"), "the off-cell PII-minimisation flag (§3.6)");
    }

    /// Every table carries the per-row `dek_ref` (encrypted-from-birth under the per-tenant DEK,
    /// contract 11.3/11.4) — no table is plaintext-then-encrypted (the tenant-decommission
    /// crypto-shred unit). The platform pattern (`myelin-refs-service`): the key ref travels with
    /// every row from the FIRST insert.
    #[test]
    fn every_table_is_encrypted_from_birth() {
        for (_id, ddl, table) in TABLE_DDLS {
            assert!(ddl.contains("dek_ref text"), "table `{table}` carries the per-row DEK ref: {ddl}");
        }
    }

    /// Each table's RLS-readiness step is the `myelin_make_tenant_scoped(<table>)` convention — the
    /// SAME helper the live integration test runs (Notif does not fork the RLS policy, EI-01 §7). The
    /// nine forward migrations each carry the `CREATE TABLE` + its RLS-scope call.
    #[test]
    fn each_table_gets_the_rls_scope_call() {
        let migrations = migrations();
        for (i, (_id, _ddl, table)) in TABLE_DDLS.iter().enumerate() {
            assert_eq!(rls_scope_sql(table), format!("SELECT myelin_make_tenant_scoped('{table}')"));
            let m = &migrations.0[i];
            assert!(
                m.ddl.contains(&rls_scope_sql(table)),
                "migration `{}` carries the RLS scoping for `{table}`",
                m.id
            );
            assert!(m.ddl.contains("CREATE TABLE"), "migration `{}` carries the create-table", m.id);
        }
        assert_eq!(TABLES.len(), 9, "the nine-table data model");
    }

    /// The runner REFUSES a destructive (`DROP`) Notif migration — forward-only is structural; a
    /// rollback is a NEW forward migration, never a `down` (contract 1.5). Proves the forward-only
    /// gate is LIVE over THIS crate's migrations, not vacuously green.
    #[test]
    fn a_destructive_notif_migration_is_refused() {
        let bad = Migrations::of([Migration::plain("notif_9999_drop", "DROP TABLE notif_inbox_item")]);
        let mut runner = MigrationRunner::new();
        let e = runner.run(&bad, &HotTables::none()).expect_err("a DROP must be refused");
        assert!(e.0.contains("forward-only"), "the refusal names forward-only: {}", e.0);
        // the assembled real migration set is forward-only-legal (no DROP anywhere).
        for (_id, ddl, _table) in TABLE_DDLS {
            assert!(ddl_is_forward_only(ddl), "the real DDL is forward-only");
            assert!(!ddl.to_ascii_uppercase().contains("DROP"), "no DROP in the data-model DDL");
        }
    }
}

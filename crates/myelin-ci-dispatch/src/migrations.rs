//! **The forward-only `consumer_dedup` ledger migration for CI Trigger & Dispatch** (CI-P6 / P-349;
//! contract 1.5 forward-only online migration; 11.1 OLTP; 12.1 the `(tenant, region)` partition key;
//! 2.5 the dedup ledger).
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/01-tech-and-data-model.md`
//! §3.8 (the `consumer_dedup` ledger — the platform consumer template's exactly-once-effect anchor:
//! Trigger & Dispatch dedups on the triggering `event_id` so one push = one run). Trigger & Dispatch
//! is "stateless except the dedup ledger" (arch 00 §4) — this is the ONE table it owns.
//!
//! ## What CI-P6 ships here — the table SHAPE, forward-only, RLS-ready (NOT the dedup logic)
//! The `consumer_dedup` table is created exactly as frozen in arch 01 §3.8, as a **forward-only**
//! migration (contract 1.5; no DROP) through the substrate framework so the boot-time RUNNER applies
//! it AND the `forward-only-migration` lint reads it at source-scan. It is:
//! - **`(tenant_id, region)`-first** (the partition prefix, contract 12.1) — the tenant-predicate
//!   floor; the dedup PRIMARY KEY is `(consumer, event_id)` (the exactly-once-effect key), and the
//!   row carries `tenant_id`/`region` as the leading partition columns so RLS isolates it;
//! - **RLS-enforced** via the platform-wide `myelin_make_tenant_scoped(...)` convention (one helper,
//!   no forked policy — EI-01 §7).
//!
//! ## Reconciliation: the §3.8 column name vs the platform RLS convention (documented deviation)
//! Architecture §3.8 names the partition column `tenant uuid`; the platform-wide RLS helper binds
//! its `(tenant_id, region)` isolation policy to `tenant_id text` + `region text` (storage §3.1).
//! These migrations name the columns **`tenant_id text` + `region text`** (the convention's exact
//! names) — the same deliberate, documented deviation `myelin-refs-service` / `myelin-knowledge`
//! record (EI-01 §1: the convention wins over the literal column name so the RLS floor is the SAME
//! one Postgres enforces for every tenant table).
//!
//! ## The exactly-once-effect key (arch 01 §3.8 / contract 2.5)
//! `PRIMARY KEY (consumer, event_id)` is the dedup anchor: an `INSERT … ON CONFLICT (consumer,
//! event_id) DO NOTHING` makes the trigger handler idempotent — one push (= one triggering
//! `event_id`) yields exactly one run even under the bus's at-least-once redelivery. The
//! `(tenant_id, region)` columns lead the row (the partition prefix + RLS key); the dedup key is
//! `(consumer, event_id)` per the frozen platform consumer template (contract 2.5).
//!
//! ## Floor named (VISION §3 / prompt DoD)
//! **This is the SCHEMA ONLY.** The dedup LOGIC — the `EventMatcher` (= `QueryAst`) match + the
//! `ON CONFLICT DO NOTHING` exactly-once dedup + the trust-tier evaluation + the single stamp — is
//! **CI-P10** (P-353). Nothing here inserts a dedup row; this migration gives CI-P10 its idempotency
//! target. The live-DB forward-only apply is proven in `tests/integration_ci_p6_dispatch_schema.rs`
//! (the `integration` cargo feature); the default `cargo build`/`cargo test --workspace` stay DB-free.

use myelin_substrate::{Migration, Migrations};

/// The `consumer_dedup` table name (arch 01 §3.8). PII-free opaque identifier — the platform
/// consumer template's exactly-once-effect ledger (contract 2.5).
pub const CONSUMER_DEDUP_TABLE: &str = "consumer_dedup";

/// The stable, ordered, PII-free migration id for the dedup-ledger schema.
pub const CONSUMER_DEDUP_MIGRATION_ID: &str = "ci_dispatch_0001_consumer_dedup";

/// The forward-only DDL that creates the `consumer_dedup` ledger (arch 01 §3.8 shape, verbatim
/// intent), `(tenant_id, region)`-first with the `(consumer, event_id)` exactly-once dedup PK. Held
/// as a `&str` so it is NOT mistaken for live Rust by the lints; the migration framework carries the
/// real DDL to the boot runner / the live integration test.
pub const CREATE_CONSUMER_DEDUP_DDL: &str = "\
CREATE TABLE IF NOT EXISTS consumer_dedup (
  tenant_id text NOT NULL,
  region    text NOT NULL,
  consumer  text NOT NULL,
  event_id  text NOT NULL,
  PRIMARY KEY (consumer, event_id)
)";

/// The RLS scoping DDL for the dedup ledger — the platform-wide `myelin_make_tenant_scoped`
/// convention (FORCE row-level security + the `(tenant_id, region)` isolation policy). CI does NOT
/// fork the RLS policy; it calls the ONE helper every tenant table uses (EI-01 §7).
pub const MAKE_CONSUMER_DEDUP_TENANT_SCOPED_DDL: &str =
    "SELECT myelin_make_tenant_scoped('consumer_dedup')";

/// **The CI Trigger & Dispatch forward-only migration set** (contract 1.5 / 2.5; arch 01 §3.8). ONE
/// [`Migration`] (`Plain` — a CREATE on an empty table): the `consumer_dedup` ledger create + the
/// platform RLS scoping, assembled into one forward DDL. The runner applies it forward-only at boot;
/// the `forward-only-migration` lint reads the same DDL at source-scan.
pub fn dispatch_migrations() -> Migrations {
    let mut ddl = String::from(CREATE_CONSUMER_DEDUP_DDL);
    ddl.push(';');
    ddl.push('\n');
    ddl.push_str(MAKE_CONSUMER_DEDUP_TENANT_SCOPED_DDL);
    ddl.push(';');
    // The substrate `Migration` holds `&'static str`; the set is built once at boot/serve, so this
    // is a one-time, bounded leak — the same shape the framework + refs-service expect.
    let ddl: &'static str = Box::leak(ddl.into_boxed_str());
    Migrations::of([Migration::plain_on(
        CONSUMER_DEDUP_MIGRATION_ID,
        ddl,
        CONSUMER_DEDUP_TABLE,
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The dedup ledger is the §3.8 shape: `(tenant_id, region)`-first with the `(consumer,
    /// event_id)` exactly-once PK.** The partition prefix leads the row; the dedup key is the
    /// platform consumer template's `(consumer, event_id)` (contract 2.5).
    #[test]
    fn the_dedup_ledger_is_the_3_8_shape() {
        let ddl = CREATE_CONSUMER_DEDUP_DDL;
        for col in ["tenant_id", "region", "consumer", "event_id"] {
            assert!(ddl.contains(col), "the §3.8 column `{col}` is declared");
        }
        let tenant_pos = ddl.find("tenant_id").unwrap();
        let region_pos = ddl.find("region").unwrap();
        assert!(tenant_pos < region_pos, "tenant_id is the FIRST column");
        assert!(
            ddl.contains("PRIMARY KEY (consumer, event_id)"),
            "the exactly-once dedup key is (consumer, event_id) — the platform consumer template"
        );
    }

    /// **The migration applies forward-only (no DROP) + installs the RLS policy (contract 1.5).**
    /// One forward migration; `is_destructive` is false; the platform RLS scoping rides it. The
    /// runner / lint enforce this at boot / source-scan; this is the in-module proof.
    #[test]
    fn the_migration_is_forward_only_and_rls_scoped() {
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
        assert!(
            m.ddl
                .contains("myelin_make_tenant_scoped('consumer_dedup')"),
            "the RLS scoping rides the migration"
        );
    }

    /// **The runner admits the dedup migration forward-only at boot (contract 1.5).** The substrate
    /// runner applies it (no DROP), recording it applied. A destructive variant would be rejected —
    /// proving the gate is not vacuous.
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

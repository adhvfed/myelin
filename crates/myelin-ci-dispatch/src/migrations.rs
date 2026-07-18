//! **The forward-only `consumer_dedup` ledger migration for CI Trigger & Dispatch** (CI-P6 / P-349;
//! contract 1.5 forward-only online migration; 11.1 OLTP; 2.5 the dedup ledger).
//!
//! **Owning architecture doc (byte-authoritative):**
//! `planning/04-subsystem-architectures/continuous-integration/architecture/01-tech-and-data-model.md`
//! §3.8 (the `consumer_dedup` ledger — the platform consumer template's exactly-once-effect anchor:
//! Trigger & Dispatch dedups on the triggering `event_id` so one push = one run). Trigger & Dispatch
//! is "stateless except the dedup ledger" (arch 00 §4) — this is the ONE table it owns.
//!
//! ## One shared schema authority
//! The platform foundation already owns `consumer_dedup`; Dispatch must not fork that table. This
//! module therefore reuses [`myelin_events::CONSUMER_DEDUP_MIGRATION`] byte-for-byte. The shared
//! ledger is intentionally keyed only by `(consumer, event_id)` and carries `recorded_at`: the
//! consumer name is deployment-unique and the event id is globally stable, so this infrastructure
//! idempotency table is not tenant-queryable application data and does not use tenant RLS.
//!
//! ## The exactly-once-effect key (arch 01 §3.8 / contract 2.5)
//! `PRIMARY KEY (consumer, event_id)` is the dedup anchor: an `INSERT … ON CONFLICT (consumer,
//! event_id) DO NOTHING` makes the trigger handler idempotent — one push (= one triggering
//! `event_id`) yields exactly one run even under the bus's at-least-once redelivery. The
//! row also records `recorded_at` per the frozen platform consumer template (contract 2.5).
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

/// The single platform-owned forward-only DDL for `consumer_dedup`, re-exported for compatibility
/// with CI's existing schema tests and consumers. Byte identity is load-bearing: the durable
/// [`myelin_storage::events_durable::DurableDedupBacking`] binds this exact shared shape.
pub const CREATE_CONSUMER_DEDUP_DDL: &str = myelin_events::CONSUMER_DEDUP_MIGRATION;

/// **The CI Trigger & Dispatch forward-only migration set** (contract 1.5 / 2.5; arch 01 §3.8). ONE
/// [`Migration`] (`Plain` — a CREATE on an empty table), using the foundation-owned DDL without a
/// competing Dispatch-local schema or policy.
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

    /// **Dispatch uses the platform §2.5 shape byte-for-byte.**
    #[test]
    fn the_dedup_ledger_is_the_shared_foundation_shape() {
        let ddl = CREATE_CONSUMER_DEDUP_DDL;
        assert_eq!(ddl, myelin_events::CONSUMER_DEDUP_MIGRATION);
        for col in ["consumer", "event_id", "recorded_at"] {
            assert!(ddl.contains(col), "the §3.8 column `{col}` is declared");
        }
        assert!(
            ddl.contains("PRIMARY KEY (consumer, event_id)"),
            "the exactly-once dedup key is (consumer, event_id) — the platform consumer template"
        );
        assert!(!ddl.contains("tenant_id"));
        assert!(!ddl.contains("myelin_make_tenant_scoped"));
    }

    /// **The migration applies the single shared forward-only DDL (contract 1.5).**
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

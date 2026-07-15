//! # W7.2 — the boot-migrations aggregate, proven against LIVE Postgres (doc-18 Part 5).
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo test -p myelin-storage` stays
//! DB-free. Runs ONLY against the docker-compose dev stack (or the make-it-real env):
//!
//!   DATABASE_URL=postgres://myelin_app:myelin_app_pw@localhost:5433/myelin \
//!     cargo test -p myelin-storage --features integration \
//!       --test integration_w7_boot_migrations -- --nocapture
//!
//! It proves the doc-18 LIVE DEFECT is fixed by the [`all_durable_migrations`] aggregate:
//!   1. **Red → green on a genuinely FRESH schema (isolated DDL proof):** create a brand-new,
//!      empty PG schema; apply ONLY the substrate foundation (the old identity-main boot subset) →
//!      the `principal` table (identity `0012`, the table `PrincipalStore::with_pg` binds to) is
//!      ABSENT — this is the defect. Then apply the boot sequence's second half
//!      (`all_durable_migrations`) into the SAME fresh schema → the `principal` table (and the whole
//!      `0010`–`0054` durable set) now EXISTS. A re-apply is idempotent.
//!   2. **The previously-broken store write path now succeeds:** after the full boot sequence
//!      (`migrate_foundation` + `all_durable_migrations`), a `DurablePrincipalBacking::put_principal`
//!      write (the storage-level backing behind `PrincipalStore::with_pg`) COMMITS and reads back —
//!      the runtime write that used to fail on a fresh DB because `0010`–`0019` were never migrated.
//!
//! Skips gracefully if the DB is unreachable (like the sibling integration tests).
#![cfg(feature = "integration")]

use std::str::FromStr;

use myelin_config::MyelinConfig;
use myelin_storage::migration::HotTables;
use myelin_storage::{
    all_durable_migrations, foundation_migrations, DurablePrincipalBacking, DurablePrincipalRow,
    PgMigrator, SubstrateProvider,
};
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};

/// DDL (`CREATE SCHEMA` / `CREATE TABLE`) runs as the migration/owner role — PG16 revokes `CREATE`
/// on `public` for the app role — so the migration tests use the admin role, the sibling convention.
fn admin_url() -> String {
    MyelinConfig::dev()
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn admin_config() -> MyelinConfig {
    let mut c = MyelinConfig::dev();
    c.database_url = admin_url();
    c
}

/// A per-run unique suffix (process id + nanos) so a fresh run genuinely CREATEs new objects.
fn uniq() -> String {
    format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// `to_regclass('<schema>.<table>')` — SCHEMA-QUALIFIED so the probe is independent of search_path
/// and of whatever the shared `public` schema already holds. `true` iff the table exists in `schema`.
async fn table_exists_in(pool: &PgPool, schema: &str, table: &str) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT to_regclass($1) IS NOT NULL")
        .bind(format!("{schema}.{table}"))
        .fetch_one(pool)
        .await
        .expect("probe table existence")
}

// =================================================================================================
// 1 — Red → green on a genuinely FRESH schema: the `principal` table is ABSENT after foundation-only
//     (the defect), PRESENT after the durable aggregate (the fix). Fully isolated from `public`.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_schema_principal_table_absent_after_foundation_then_present_after_aggregate() {
    // A bare admin pool (NO reset-on-release hook) whose sessions default their search_path to a
    // brand-new schema first, then `public` (so extension functions still resolve). Because there is
    // no `RESET ALL` on release, the search_path sticks for the migrator's whole run.
    let schema = format!("w7_boot_{}", uniq());

    // Bootstrap: create the fresh schema on a plain connection (search_path irrelevant here).
    let bootstrap = match PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url())
        .await
    {
        Ok(p) => p,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    sqlx::query(&format!("CREATE SCHEMA \"{schema}\""))
        .execute(&bootstrap)
        .await
        .expect("create the fresh isolation schema");

    // The search_path-pinned bare pool the migrator runs against (CREATEs land in `schema`).
    let opts = PgConnectOptions::from_str(&admin_url())
        .expect("parse admin DSN")
        .options([("search_path", format!("\"{schema}\",public").as_str())]);
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(opts)
        .await
        .expect("open the search_path-pinned admin pool");

    // (a) Apply ONLY the substrate foundation (0000/0001) — the minimal subset an old service main
    //     applied. The identity `principal` table (0012) is NOT in it.
    PgMigrator::apply(&pool, &foundation_migrations())
        .await
        .expect("foundation migrations apply into the fresh schema");
    assert!(
        table_exists_in(&pool, &schema, "outbox").await,
        "foundation created the outbox table in the fresh schema"
    );
    // RED — the doc-18 defect: the table `PrincipalStore::with_pg` binds to does NOT exist yet.
    assert!(
        !table_exists_in(&pool, &schema, "principal").await,
        "DEFECT REPRODUCED: `principal` (identity 0012) is ABSENT after foundation-only — a \
         principal write here would fail at runtime"
    );

    // (b) Apply the durable AGGREGATE (0010–0054) — the second half of the fixed boot sequence.
    PgMigrator::apply(&pool, &all_durable_migrations())
        .await
        .expect("the durable aggregate applies into the fresh schema");
    // GREEN — the whole durable set now exists; spot-check one table per group (0010→0054).
    for (table, group) in [
        ("rebac_tuple", "identity 0010"),
        ("principal", "identity 0012"),
        ("pseudonym_map", "pseudonym 0020"),
        ("cell", "placement 0030"),
        ("kms_sealed_root", "kms 0040"),
        ("cost_reservation", "cost 0050"),
        ("restore_erasure_ledger", "restore 0051"),
        ("post_pit_erasure_ledger", "post-pit 0052"),
        ("bus_erasure_ledger", "bus-erasure 0053"),
        ("agent_hitl_gate", "hitl-gate 0054 (R2.4)"),
    ] {
        assert!(
            table_exists_in(&pool, &schema, table).await,
            "the aggregate migrated `{table}` ({group}) into the fresh schema"
        );
    }

    // (c) Idempotent re-apply — a second boot runs no duplicate DDL, no error.
    PgMigrator::apply(&pool, &all_durable_migrations())
        .await
        .expect("re-applying the aggregate is idempotent");

    // Cleanup: drop the whole isolation schema (and everything the aggregate created inside it).
    sqlx::query(&format!("DROP SCHEMA \"{schema}\" CASCADE"))
        .execute(&bootstrap)
        .await
        .expect("drop the fresh isolation schema");
    println!(
        "OK [1]: on a fresh schema, `principal` is ABSENT after foundation-only (the defect) and \
         PRESENT after all_durable_migrations (the fix); re-apply idempotent."
    );
}

// =================================================================================================
// 2 — The previously-broken store WRITE path succeeds after the full boot sequence (shared schema,
//     the established unique-tenant-suffix pattern). This is the red-to-green at the STORE layer.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_principal_write_succeeds_after_the_boot_sequence() {
    // The boot sequence a service main now runs: foundation THEN the durable aggregate (admin role
    // for DDL; idempotent on the shared schema).
    let admin = match SubstrateProvider::connect(admin_config(), 4).await {
        Ok(p) => p,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    admin
        .migrate_foundation()
        .await
        .expect("boot step 1: foundation");
    admin
        .migrate(&all_durable_migrations(), &HotTables::none())
        .await
        .expect("boot step 2: the durable aggregate (the W7.2 fix)");

    // The stores run through the app role (NOBYPASSRLS, reset-on-release) — the production path.
    let app = SubstrateProvider::connect(MyelinConfig::dev(), 4)
        .await
        .expect("open the app-role provider");
    let region = app.config().region.clone();
    let suffix = uniq();
    let tenant = format!("w7-boot-{suffix}");

    // THE previously-broken write: DurablePrincipalBacking is the storage-level backing behind
    // `PrincipalStore::with_pg`. Before W7.2 the `principal` table was un-migrated at boot, so this
    // INSERT failed on a fresh DB. After the aggregate boot it COMMITS.
    let backing = DurablePrincipalBacking::new(app.clone());
    let row = DurablePrincipalRow {
        principal_id: "p:alice".into(),
        kind: "\"Human\"".into(),
        data_role: "\"Processor\"".into(),
        status: "\"Active\"".into(),
        profile: None,
    };
    backing
        .put_principal(&tenant, row.clone())
        .await
        .expect("the durable principal write COMMITS after the boot-migrations fix (was the defect)");

    // Read it back through a fresh backing over the SAME pool (the row is durable).
    let read = DurablePrincipalBacking::new(app.clone())
        .get_principal(&tenant, "p:alice")
        .await
        .expect("read the principal row back")
        .expect("the principal row is durable");
    assert_eq!(read.principal_id, "p:alice");
    assert_eq!(read.kind, "\"Human\"");

    // Cleanup (admin role — RLS-bypassing owner).
    let _ = sqlx::query("DELETE FROM principal WHERE tenant_id = $1 AND region = $2")
        .bind(&tenant)
        .bind(&region)
        .execute(admin.db_pool())
        .await;
    println!(
        "OK [2]: after `migrate_foundation` + `all_durable_migrations`, a durable principal write \
         commits + reads back — the doc-18 runtime write path is green."
    );
}

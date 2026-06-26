//! # MR-022 — the persistence-foundation integration tests (proven against LIVE Postgres).
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build/test --workspace` stays
//! DB-free. Runs ONLY against the docker-compose dev stack (or the make-it-real env):
//!
//!   DATABASE_URL=postgres://myelin_app:myelin_app_pw@localhost:5433/myelin \
//!     cargo test -p myelin-storage --features integration \
//!       --test integration_mr022_persistence_foundation -- --nocapture
//!
//! It proves the three MR-022 deliverables — each MUST hit the live DB (a pass on the in-memory
//! model would not count):
//!   A. **Real migration execution wired into the boot path** (SI-010): the composition root runs
//!      VALIDATE → EXECUTE against the live pool; the tables + `myelin_applied_migration` rows exist
//!      after boot; a re-run is idempotent (no error, no duplicate apply); a destructive migration is
//!      rejected before any DDL runs.
//!   B. **The composition root / real-pool provider** (SI-022): `SubstrateProvider::from_env` builds
//!      the REAL pool from the env config and it is reachable; `connect` + `migrate_foundation` runs
//!      the substrate's co-located `outbox` + `consumer_dedup` DDL against the live DB.
//!   C. **The tenant-scoped-TRANSACTION convention** (RESHAPE-002 / SI-005): a write under
//!      `with_tenant_tx` is invisible to another tenant (RLS applies INSIDE the tx — proving SET
//!      LOCAL took effect), AND after the connection is released and reused no residual tenant GUC
//!      bleeds (reset-on-release + transaction-scoping work).
//!
//! Skips gracefully if the DB is unreachable (like the sibling integration tests).
#![cfg(feature = "integration")]

use myelin_config::{Mode, MyelinConfig};
use myelin_storage::migration::{HotTables, Migration, Migrations};
use myelin_storage::pg::PgStore;
use myelin_storage::tenant_tx::{connect_pool_with_reset, with_tenant_tx};
use myelin_storage::{PgMigrator, ProviderError, SubstrateProvider};

/// DDL (`CREATE TABLE` on `public`) runs as the migration/owner role — PG16 revokes `CREATE` on
/// `public` for the app role — so the foundation/migration tests use the admin role, the convention
/// the sibling integration tests use.
fn admin_config(cfg: &MyelinConfig) -> MyelinConfig {
    let mut c = cfg.clone();
    c.database_url = c
        .database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    c
}

/// A per-run unique suffix so a fresh run genuinely CREATEs new (non-existing) objects — the clean
/// schema the deliverable-A test asserts against. Leaked to `'static` to fit `Migration`'s contract.
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

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// Try to build an admin-role provider; `None` (with a SKIP note) if the DB is unreachable.
async fn admin_provider(max: u32) -> Option<SubstrateProvider> {
    let cfg = admin_config(&MyelinConfig::dev());
    match SubstrateProvider::connect(cfg, max).await {
        Ok(p) => Some(p),
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            None
        }
    }
}

async fn table_exists(pool: &sqlx::PgPool, table: &str) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT to_regclass($1) IS NOT NULL")
        .bind(table)
        .fetch_one(pool)
        .await
        .expect("probe table existence")
}

// =================================================================================================
// A — Real migration execution wired into the boot path (SI-010): validate → EXECUTE, idempotent.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_boot_migration_executes_ddl_and_is_idempotent() {
    let Some(provider) = admin_provider(6).await else {
        return;
    };

    let suffix = uniq();
    let t1 = leak(format!("mr022_a_one_{suffix}"));
    let t2 = leak(format!("mr022_a_two_{suffix}"));
    let id1 = leak(format!("mr022a_{suffix}_0001"));
    let id2 = leak(format!("mr022a_{suffix}_0002"));
    let migrations = Migrations::of([
        Migration::plain(
            id1,
            leak(format!(
                "CREATE TABLE IF NOT EXISTS {t1} (id text PRIMARY KEY, body text NOT NULL);"
            )),
        ),
        Migration::plain(
            id2,
            leak(format!(
                "CREATE TABLE IF NOT EXISTS {t2} (id text PRIMARY KEY, n int NOT NULL);"
            )),
        ),
    ]);

    // (1) BOOT migrate — the composition root validates then EXECUTES the DDL against the live pool.
    provider
        .migrate(&migrations, &HotTables::none())
        .await
        .expect("boot migration executes DDL against the live DB");

    // After boot the substrate tables EXIST in the live DB (real DDL ran — not an in-memory model).
    assert!(table_exists(provider.db_pool(), t1).await, "{t1} must exist after boot migrate");
    assert!(table_exists(provider.db_pool(), t2).await, "{t2} must exist after boot migrate");
    // …and each migration id is recorded in the version table EXACTLY once.
    assert_eq!(PgMigrator::applied_count(provider.db_pool(), id1).await.unwrap(), 1);
    assert_eq!(PgMigrator::applied_count(provider.db_pool(), id2).await.unwrap(), 1);

    // (2) RE-RUN → idempotent: no error, no duplicate apply (each id still recorded exactly once).
    provider
        .migrate(&migrations, &HotTables::none())
        .await
        .expect("re-running the same migrations is idempotent");
    assert_eq!(
        PgMigrator::applied_count(provider.db_pool(), id1).await.unwrap(),
        1,
        "a second boot migrate must NOT duplicate-apply id1"
    );
    assert_eq!(PgMigrator::applied_count(provider.db_pool(), id2).await.unwrap(), 1);

    // (3) The VALIDATE half is wired into the boot path: a destructive (DROP) migration is rejected
    //     BEFORE any DDL runs (forward-only — the SI-010 reconciliation keeps the lint half live).
    let drop_id = leak(format!("mr022a_{suffix}_drop"));
    let destructive = Migrations::of([Migration::plain(
        drop_id,
        leak(format!("DROP TABLE {t1};")),
    )]);
    let err = provider
        .migrate(&destructive, &HotTables::none())
        .await
        .expect_err("a destructive migration must be rejected at boot");
    match err {
        ProviderError::Pg(e) => assert!(
            e.to_string().contains("forward-only"),
            "the rejection names forward-only: {e}"
        ),
        other => panic!("expected a Pg(forward-only) rejection, got {other}"),
    }
    // The destructive id was NEVER recorded and the table is still present (no DDL ran).
    assert_eq!(PgMigrator::applied_count(provider.db_pool(), drop_id).await.unwrap(), 0);
    assert!(table_exists(provider.db_pool(), t1).await, "{t1} must survive the rejected DROP");

    // cleanup (best effort) — leave the DB clean.
    for t in [t1, t2] {
        let _ = sqlx::raw_sql(&format!("DROP TABLE IF EXISTS {t}"))
            .execute(provider.db_pool())
            .await;
    }
    for id in [id1, id2] {
        let _ = sqlx::query("DELETE FROM myelin_applied_migration WHERE id = $1")
            .bind(id)
            .execute(provider.db_pool())
            .await;
    }
    println!("OK [A]: boot migrate executed DDL, idempotent on re-run, destructive rejected.");
}

// =================================================================================================
// B — The composition root / real-pool provider (SI-022): real pool from env + migrate at startup.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn b_provider_builds_real_pool_from_env_and_migrates_foundation() {
    // from_env builds the REAL pool from the env config (the dev↔prod CONFIG SWAP). DevDefaults
    // points at the docker-compose stack; the make-it-real env supplies DATABASE_URL etc.
    let provider = match SubstrateProvider::from_env(Mode::DevDefaults).await {
        Ok(p) => p,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    // The real pool is reachable (a live round trip — not an in-memory permit model).
    let one: i32 = sqlx::query_scalar("SELECT 1")
        .fetch_one(provider.db_pool())
        .await
        .expect("the env-built pool reaches the live DB");
    assert_eq!(one, 1);
    // The env config is threaded through the provider (the residency region pin).
    assert!(!provider.config().region.is_empty(), "the provider carries the region pin");

    // The substrate's co-located FOUNDATION migrations (outbox + consumer_dedup) run at startup
    // against the live DB (DDL → admin role). After migrate_foundation the substrate tables exist.
    let Some(admin) = admin_provider(4).await else {
        return;
    };
    admin
        .migrate_foundation()
        .await
        .expect("the composition root runs the foundation migrations at startup");
    assert!(table_exists(admin.db_pool(), "outbox").await, "the outbox table exists after boot");
    assert!(
        table_exists(admin.db_pool(), "consumer_dedup").await,
        "the consumer_dedup table exists after boot"
    );
    assert!(PgMigrator::is_applied(admin.db_pool(), "0000_outbox").await.unwrap());
    assert!(PgMigrator::is_applied(admin.db_pool(), "0001_consumer_dedup").await.unwrap());
    // Re-running the foundation is idempotent (the composition root is safe to re-boot).
    admin.migrate_foundation().await.expect("foundation migrate is idempotent");
    assert_eq!(
        PgMigrator::applied_count(admin.db_pool(), "0000_outbox").await.unwrap(),
        1,
        "the outbox foundation migration is applied EXACTLY once across re-boots"
    );
    println!("OK [B]: provider built the real pool from env + ran the foundation migrations live.");
}

// =================================================================================================
// C — The tenant-scoped-TRANSACTION convention (RESHAPE-002 / SI-005): RLS-in-tx + no bleed.
// =================================================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn c_tenant_tx_isolates_and_does_not_bleed() {
    let cfg = MyelinConfig::dev();
    let admin = admin_config(&cfg);

    // Ensure the RLS-policy'd `rebac_tuple` table exists (REUSE the existing PgStore::migrate — the
    // (tenant, region) FORCE-RLS policy keyed on current_setting('myelin.tenant_id', true)).
    let store = match PgStore::connect(&admin.database_url, &cfg.region, 4).await {
        Ok(s) => s,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    store.migrate().await.expect("rebac_tuple + RLS policy exist");

    // The tenant-scoped reads/writes MUST run as the APP role (`myelin_app` is NOSUPERUSER
    // NOBYPASSRLS — the admin/owner role is a superuser and would bypass every policy). DDL above
    // ran as admin; the convention below runs as the app role so RLS is genuinely enforced. The
    // app-role pool has reset-on-release wired (RESHAPE-002 defence-in-depth).
    let pool = connect_pool_with_reset(&cfg.database_url, &cfg.region, 4)
        .await
        .expect("open the app-role reset-on-release pool");

    let suffix = uniq();
    let tenant_a = format!("mr022A-{suffix}");
    let tenant_b = format!("mr022B-{suffix}");
    let obj = format!("obj-{suffix}");
    let region = cfg.region.clone();

    // (1) WRITE a tuple for tenant A THROUGH the convention (acquire → BEGIN → SET LOCAL → write →
    //     COMMIT). The RLS WITH CHECK admits it because the tx GUC scopes the session to (A, region).
    {
        let a = tenant_a.clone();
        let r = region.clone();
        let o = obj.clone();
        with_tenant_tx(&pool, &tenant_a, &region, move |conn| {
            Box::pin(async move {
                sqlx::query(
                    "INSERT INTO rebac_tuple (tenant_id, region, object_id, relation, subject) \
                     VALUES ($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING",
                )
                .bind(&a)
                .bind(&r)
                .bind(&o)
                .bind("viewer")
                .bind("user:1")
                .execute(&mut *conn)
                .await
                .map_err(|e| myelin_storage::pg::PgError::Query(e.to_string()))?;
                Ok(())
            })
        })
        .await
        .expect("tenant A write under the convention");
    }

    // Tenant A, under the convention, SEES its own row (a deliberately tenant-predicate-LESS SELECT —
    // the ONLY scoping is the DB RLS policy inside the tx). This also proves SET LOCAL TOOK EFFECT.
    let seen_by_a = read_objects(&pool, &tenant_a, &region).await;
    assert_eq!(seen_by_a, vec![obj.clone()], "tenant A sees its own row under RLS");

    // (2) Tenant B, under the convention, sees NONE of tenant A's rows — RLS applies inside the tx
    //     (the cross-tenant isolation the convention guarantees). If SET LOCAL were a silent no-op,
    //     this read would either leak A's row OR A wouldn't have seen its own — both are caught.
    let seen_by_b = read_objects(&pool, &tenant_b, &region).await;
    assert!(
        !seen_by_b.contains(&obj),
        "tenant B must NOT see tenant A's row (RLS inside the tenant-scoped transaction)"
    );

    // (3) RESET-ON-RELEASE / no bleed. Use a single-connection pool so the next acquire reuses the
    //     SAME backend connection — the exact surface the old session-scoped GUC bled across.
    let pool1 = connect_pool_with_reset(&cfg.database_url, &cfg.region, 1)
        .await
        .expect("single-connection app-role pool");

    // (3a) After a committed tenant-scoped tx, the transaction-scoped GUC is gone (SET LOCAL).
    with_tenant_tx(&pool1, "ghost-tenant", &region, |_conn| Box::pin(async move { Ok(()) }))
        .await
        .expect("a tenant-scoped tx commits");
    assert!(
        current_tenant_guc(&pool1).await.is_empty(),
        "no residual tenant GUC after a committed tenant-scoped transaction (SET LOCAL discarded)"
    );

    // (3b) Even a DELIBERATELY session-scoped GUC (the OLD bleed pattern) is scrubbed on release —
    //      the after_release(RESET ALL) defence-in-depth. Without it, the reused connection below
    //      would still read 'bleed'.
    {
        let mut bad = pool1.acquire().await.expect("acquire");
        sqlx::query("SELECT set_config('myelin.tenant_id', 'bleed', false)")
            .execute(&mut *bad)
            .await
            .expect("set a session-scoped GUC (the old leak pattern)");
        // `bad` drops here → returned to the pool → after_release(RESET ALL) scrubs it.
    }
    assert!(
        current_tenant_guc(&pool1).await.is_empty(),
        "reset-on-release scrubbed a session-scoped GUC — no cross-checkout bleed (defence in depth)"
    );

    // (3c) Isolate the TRANSACTION-scoping mechanism itself: a BARE pool with NO reset-on-release
    //      hook. SET LOCAL (`set_config(..., true)`) alone must discard the GUC at COMMIT, so a
    //      reused connection reads the database default (''). If the convention regressed to a
    //      session-scoped `set_config(..., false)`, this would read 'probe' and FAIL — this is the
    //      non-vacuity guard that the convention is genuinely transaction-scoped, not just scrubbed.
    let bare = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&cfg.database_url)
        .await
        .expect("bare single-connection pool (no reset-on-release)");
    with_tenant_tx(&bare, "probe-tenant", &region, |_conn| Box::pin(async move { Ok(()) }))
        .await
        .expect("a tenant-scoped tx commits on the bare pool");
    let residual_bare: Option<String> = {
        let mut conn = bare.acquire().await.expect("reacquire the same bare connection");
        sqlx::query_scalar("SELECT current_setting('myelin.tenant_id', true)")
            .fetch_one(&mut *conn)
            .await
            .expect("read GUC on the reused bare connection")
    };
    assert!(
        residual_bare.unwrap_or_default().is_empty(),
        "SET LOCAL alone (no reset-on-release) must discard the tenant GUC at COMMIT — \
         a session-scoped set_config(.., false) would BLEED here"
    );

    // cleanup: delete tenant A's seeded row THROUGH the convention (acquire → BEGIN → SET LOCAL
    // (A, region) → DELETE → COMMIT) so the RLS policy admits the delete. (MR-013 removed the bare
    // `PgStore::pool()` hatch; the durable path is the tenant-scoped transaction.)
    let _ = with_tenant_tx(&pool, &tenant_a, &region, {
        let o = obj.clone();
        move |conn| {
            Box::pin(async move {
                sqlx::query("DELETE FROM rebac_tuple WHERE object_id = $1")
                    .bind(&o)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| myelin_storage::pg::PgError::Query(e.to_string()))?;
                Ok(())
            })
        }
    })
    .await;
    println!("OK [C]: tenant-scoped-tx isolates across tenants (RLS in tx) + no GUC bleed on reuse.");
}

/// Read every `object_id` visible to `tenant` THROUGH the convention with NO tenant predicate — so
/// the ONLY scoping is the DB FORCE-RLS policy inside the tenant-scoped transaction.
async fn read_objects(pool: &sqlx::PgPool, tenant: &str, region: &str) -> Vec<String> {
    with_tenant_tx(pool, tenant, region, |conn| {
        Box::pin(async move {
            use sqlx::Row;
            let rows = sqlx::query("SELECT object_id FROM rebac_tuple ORDER BY object_id")
                .fetch_all(&mut *conn)
                .await
                .map_err(|e| myelin_storage::pg::PgError::Query(e.to_string()))?;
            Ok(rows.iter().map(|r| r.get::<String, _>("object_id")).collect::<Vec<_>>())
        })
    })
    .await
    .expect("RLS-scoped read under the convention")
}

/// The currently-set `myelin.tenant_id` GUC on a freshly-acquired connection (empty string when
/// unset — `current_setting(.., true)` returns NULL → mapped to "").
async fn current_tenant_guc(pool: &sqlx::PgPool) -> String {
    let mut conn = pool.acquire().await.expect("acquire");
    let v: Option<String> = sqlx::query_scalar("SELECT current_setting('myelin.tenant_id', true)")
        .fetch_one(&mut *conn)
        .await
        .expect("read current tenant GUC");
    v.unwrap_or_default()
}

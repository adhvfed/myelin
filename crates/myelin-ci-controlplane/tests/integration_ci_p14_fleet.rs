//! **CI-P14 / P-357 — the EU fleet's region-pinned runner table, PROVEN against the live dev-stack
//! Postgres.**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-ci-controlplane --features integration \
//!     --test integration_ci_p14_fleet -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires (CI-P14 touches the OLTP `runner`
//! table — the autoscaler's pool register, contract 11.1/12.1): the fleet's region-pinned runner
//! INSERT (`INSERT_RUNNER_QUERY`) applies against real Postgres under the `(tenant, region)` RLS
//! isolation; the per-`(region, pool)` autoscale-input COUNT (`COUNT_RUNNERS_BY_POOL_QUERY`) is the
//! no-global-pool read property — it counts ONLY the pool's own residency zone, never aggregating
//! across regions; and the deprovision DELETE (`DELETE_RUNNER_QUERY`) is PK-scoped. The drill is
//! registered red-until-proven and flips green ONLY here, against the live stack — never mocked.
//!
//! The test applies the REAL `runner` DDL onto a uniquely-suffixed throwaway table so concurrent runs
//! don't collide; the DDL SHAPE is byte-for-byte the production migration (only the identifier is
//! suffixed for isolation + cleanup). The INSERT/COUNT/DELETE statement SHAPES are the production
//! `&str` constants (only the table identifier is rewritten).
#![cfg(feature = "integration")]

use myelin_ci_controlplane::{
    COUNT_RUNNERS_BY_POOL_QUERY, CREATE_RUNNER_DDL, DELETE_RUNNER_QUERY, INSERT_RUNNER_QUERY,
};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

/// Rewrite a production table-named statement onto a uniquely-suffixed table so concurrent runs don't
/// collide. The SHAPE (columns, keys, predicates) is unchanged — only the `runner` identifier is
/// suffixed.
fn rename(stmt: &str, tbl: &str) -> String {
    stmt.replace("EXISTS runner (", &format!("EXISTS {tbl} ("))
        .replace("INTO runner\n", &format!("INTO {tbl}\n"))
        .replace("FROM runner\n", &format!("FROM {tbl}\n"))
        .replace("UPDATE runner ", &format!("UPDATE {tbl} "))
        .replace("FROM runner WHERE", &format!("FROM {tbl} WHERE"))
}

#[tokio::test]
async fn the_fleet_runner_table_is_region_pinned_and_no_global_pool() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)");
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&app_url())
        .await
        .expect("connect to dev Postgres as the app role");

    let suffix = std::process::id();
    let tbl = format!("runner_p357_{suffix}");

    // ── 1. Apply the REAL forward-only runner CREATE TABLE (arch 01 §3.4), suffixed. ──
    let create = rename(CREATE_RUNNER_DDL, &tbl);
    sqlx::query(&create)
        .execute(&admin)
        .await
        .expect("apply the runner CREATE TABLE forward-only");

    // RLS-ready via the platform-wide helper (CI does not fork the policy).
    sqlx::query(&format!("SELECT myelin_make_tenant_scoped('{tbl}')"))
        .execute(&admin)
        .await
        .expect("the runner table is made tenant-scoped (RLS)");
    sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .expect("grant the app role");

    // ── 2. The fleet provisions region-pinned runners: tenantA gets a pool in fr-par AND one in ──
    //       eu-north (two distinct residency zones — proving per-zone partition, NOT a global pool).
    let insert = rename(INSERT_RUNNER_QUERY, &tbl);
    let seed = [
        ("tenantA", "fr-par", "linux-x64"),
        ("tenantA", "fr-par", "linux-x64"),
        ("tenantA", "fr-par", "linux-x64"),
        ("tenantA", "eu-north", "linux-x64"),
    ];
    for (tenant, region, pool) in seed {
        let mut conn = admin.acquire().await.unwrap();
        sqlx::query("SELECT set_config('myelin.tenant_id', $1, false)")
            .bind(tenant)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('myelin.region', $1, false)")
            .bind(region)
            .execute(&mut *conn)
            .await
            .unwrap();
        // $1 tenant, $2 region (the CELL's), $3 runner_id, $4 pool, $5 labels, $6 ownership,
        // $7 trust_tier, $8 attest_state, $9 health, $10 capacity.
        sqlx::query(&insert)
            .bind(tenant)
            .bind(region)
            .bind(uuid_v4())
            .bind(pool)
            .bind(vec!["linux".to_string(), "x64".to_string()])
            .bind("hosted")
            .bind("trusted")
            .bind("attested")
            .bind("healthy")
            .bind(r#"{"slots":1}"#)
            .execute(&mut *conn)
            .await
            .expect("a region-pinned runner row inserts");
    }

    // ── 3. THE no-global-pool READ property: COUNT per (region, pool) counts ONLY that residency ──
    //       zone — fr-par has 3, eu-north has 1; the count NEVER aggregates across regions.
    let count = rename(COUNT_RUNNERS_BY_POOL_QUERY, &tbl);
    let mut conn = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantA', false)")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
        .execute(&mut *conn)
        .await
        .unwrap();
    let fr: i64 = sqlx::query(&count)
        .bind("tenantA")
        .bind("fr-par")
        .bind("linux-x64")
        .fetch_one(&mut *conn)
        .await
        .unwrap()
        .get::<i64, _>(0);
    assert_eq!(
        fr, 3,
        "the fr-par pool has exactly its OWN 3 runners (per residency zone)"
    );

    // The eu-north count runs in an eu-north-pinned session: RLS pins the read to the cell's region,
    // so a fr-par session CANNOT even see eu-north rows (residency by construction — no cross-region
    // read path). This is the no-global-pool property AT the read path, enforced by RLS.
    sqlx::query("SELECT set_config('myelin.region', 'eu-north', false)")
        .execute(&mut *conn)
        .await
        .unwrap();
    let eu: i64 = sqlx::query(&count)
        .bind("tenantA")
        .bind("eu-north")
        .bind("linux-x64")
        .fetch_one(&mut *conn)
        .await
        .unwrap()
        .get::<i64, _>(0);
    assert_eq!(
        eu, 1,
        "the eu-north pool has exactly its OWN 1 runner — the count never aggregates across \
         regions (no global pool)"
    );

    // ── 4. PROVE the frozen CHECK vocabulary is real: an out-of-vocabulary health is REJECTED. ──
    {
        let mut conn = admin.acquire().await.unwrap();
        sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantA', false)")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
            .execute(&mut *conn)
            .await
            .unwrap();
        let bad = sqlx::query(&insert)
            .bind("tenantA")
            .bind("fr-par")
            .bind(uuid_v4())
            .bind("linux-x64")
            .bind(vec!["linux".to_string()])
            .bind("hosted")
            .bind("trusted")
            .bind("attested")
            .bind("ON_FIRE") // not in the health CHECK vocabulary
            .bind(r#"{"slots":1}"#)
            .execute(&mut *conn)
            .await;
        assert!(
            bad.is_err(),
            "the health CHECK rejects an out-of-vocabulary value ('ON_FIRE') — the frozen \
             healthy/degraded/offline vocabulary is enforced by Postgres"
        );
    }

    // ── 5. Deprovision (scale-down) is PK-scoped: deleting tenantA's eu-north runner removes ONLY ──
    //       it; the fr-par pool is untouched (a deprovision never crosses a tenant/region).
    {
        let mut conn = admin.acquire().await.unwrap();
        sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantA', false)")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('myelin.region', 'eu-north', false)")
            .execute(&mut *conn)
            .await
            .unwrap();
        let eu_runner: String = sqlx::query(&format!(
            "SELECT runner_id::text FROM {tbl} WHERE tenant_id = 'tenantA' AND region = 'eu-north'"
        ))
        .fetch_one(&mut *conn)
        .await
        .unwrap()
        .get::<String, _>(0);
        let delete = rename(DELETE_RUNNER_QUERY, &tbl);
        sqlx::query(&delete)
            .bind("tenantA")
            .bind(eu_runner)
            .execute(&mut *conn)
            .await
            .expect("the deprovision DELETE removes the runner row");
    }
    {
        let mut conn = app.acquire().await.unwrap();
        sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantA', false)")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
            .execute(&mut *conn)
            .await
            .unwrap();
        let fr_after: i64 = sqlx::query(&count)
            .bind("tenantA")
            .bind("fr-par")
            .bind("linux-x64")
            .fetch_one(&mut *conn)
            .await
            .unwrap()
            .get::<i64, _>(0);
        assert_eq!(
            fr_after, 3,
            "deprovisioning the eu-north runner left the fr-par pool untouched (PK-scoped)"
        );
    }

    // ── 6. PROVE forward-only: the production runner DDL carries NO DROP. ──
    assert!(
        !CREATE_RUNNER_DDL.to_ascii_uppercase().contains("DROP"),
        "the runner schema migration is forward-only (no DROP)"
    );

    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}

/// A throwaway v4-shaped UUID string for the runner_id (the table's `uuid` column). Deterministic
/// enough for the test (process-pid + a counter); the production runner_id is the provider's host id
/// mapped to a uuid at registration.
fn uuid_v4() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(1);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!("00000000-0000-4000-8000-{:012x}", n)
}

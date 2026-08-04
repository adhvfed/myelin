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

    let create = rename(CREATE_RUNNER_DDL, &tbl);
    sqlx::query(&create)
        .execute(&admin)
        .await
        .expect("apply the runner CREATE TABLE forward-only");

    sqlx::query(&format!("SELECT myelin_make_tenant_scoped('{tbl}')"))
        .execute(&admin)
        .await
        .expect("the runner table is made tenant-scoped (RLS)");
    sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .expect("grant the app role");

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
        "the eu-north pool has exactly its OWN 1 runner - the count never aggregates across \
         regions (no global pool)"
    );

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
            .bind("ON_FIRE")
            .bind(r#"{"slots":1}"#)
            .execute(&mut *conn)
            .await;
        assert!(
            bad.is_err(),
            "the health CHECK rejects an out-of-vocabulary value ('ON_FIRE') - the frozen \
             healthy/degraded/offline vocabulary is enforced by Postgres"
        );
    }

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

    assert!(
        !CREATE_RUNNER_DDL.to_ascii_uppercase().contains("DROP"),
        "the runner schema migration is forward-only (no DROP)"
    );

    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}

fn uuid_v4() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(1);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!("00000000-0000-4000-8000-{:012x}", n)
}

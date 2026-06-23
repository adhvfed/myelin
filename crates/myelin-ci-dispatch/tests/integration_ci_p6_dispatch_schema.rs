//! **CI-P6 / P-349 — the CI Trigger & Dispatch `consumer_dedup` ledger, PROVEN against the live
//! dev-stack Postgres.**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build`/`cargo test
//! --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-ci-dispatch --features integration \
//!     --test integration_ci_p6_dispatch_schema -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires: the arch 01 §3.8 `consumer_dedup`
//! ledger APPLIES forward-only against real Postgres, the `(consumer, event_id)` PRIMARY KEY gives
//! the exactly-once effect (a second insert of the same `(consumer, event_id)` is REJECTED — one
//! push = one run), the `(tenant, region)` RLS policy ISOLATES tenants, and the schema is
//! forward-only (no DROP). The drill is registered red-until-proven and flips green ONLY here.
#![cfg(feature = "integration")]

use myelin_ci_dispatch::CREATE_CONSUMER_DEDUP_DDL;

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

#[tokio::test]
async fn consumer_dedup_ledger_applies_forward_only_with_exactly_once_key_and_rls() {
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
    let tbl = format!("consumer_dedup_p349_{suffix}");

    // ── 1. Apply the REAL forward-only consumer_dedup CREATE TABLE (arch 01 §3.8), suffixed. ──
    let create =
        CREATE_CONSUMER_DEDUP_DDL.replace("EXISTS consumer_dedup (", &format!("EXISTS {tbl} ("));
    sqlx::query(&create)
        .execute(&admin)
        .await
        .expect("apply the consumer_dedup CREATE TABLE forward-only");

    // ── 2. RLS-ready via the platform-wide convention helper. ──
    sqlx::query(&format!("SELECT myelin_make_tenant_scoped('{tbl}')"))
        .execute(&admin)
        .await
        .expect("the consumer_dedup ledger is made tenant-scoped (RLS)");
    sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .expect("grant the app role");

    // ── 3. PROVE the exactly-once effect: a second insert of the same (consumer, event_id) is ──
    //       REJECTED by the PRIMARY KEY (one push = one run, contract 2.5).
    let mut conn = admin.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantA', false)")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(&format!(
        "INSERT INTO {tbl} (tenant_id, region, consumer, event_id) \
         VALUES ('tenantA', 'fr-par', 'ci-dispatch', 'evt-push-1')"
    ))
    .execute(&mut *conn)
    .await
    .expect("the first trigger insert succeeds");

    let dup = sqlx::query(&format!(
        "INSERT INTO {tbl} (tenant_id, region, consumer, event_id) \
         VALUES ('tenantA', 'fr-par', 'ci-dispatch', 'evt-push-1')"
    ))
    .execute(&mut *conn)
    .await;
    assert!(
        dup.is_err(),
        "the (consumer, event_id) PRIMARY KEY rejects a duplicate trigger (the exactly-once \
         effect — one push = one run, contract 2.5)"
    );

    // The idempotent ON CONFLICT DO NOTHING form (what CI-P10 uses) is a no-op second time.
    let on_conflict = sqlx::query(&format!(
        "INSERT INTO {tbl} (tenant_id, region, consumer, event_id) \
         VALUES ('tenantA', 'fr-par', 'ci-dispatch', 'evt-push-1') \
         ON CONFLICT (consumer, event_id) DO NOTHING"
    ))
    .execute(&mut *conn)
    .await
    .expect("ON CONFLICT DO NOTHING is the idempotent dedup form");
    assert_eq!(
        on_conflict.rows_affected(),
        0,
        "the second ON CONFLICT DO NOTHING inserts 0 rows (the dedup'd redelivery)"
    );

    // ── 4. PROVE RLS isolates tenants: seed tenantB, then the app role pinned to tenantA sees only ─
    //       tenantA's dedup rows.
    {
        let mut bconn = admin.acquire().await.unwrap();
        sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantB', false)")
            .execute(&mut *bconn)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
            .execute(&mut *bconn)
            .await
            .unwrap();
        sqlx::query(&format!(
            "INSERT INTO {tbl} (tenant_id, region, consumer, event_id) \
             VALUES ('tenantB', 'fr-par', 'ci-dispatch', 'evt-push-2')"
        ))
        .execute(&mut *bconn)
        .await
        .unwrap();
    }
    let mut aconn = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantA', false)")
        .execute(&mut *aconn)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
        .execute(&mut *aconn)
        .await
        .unwrap();
    let rows = sqlx::query(&format!("SELECT tenant_id, event_id FROM {tbl}"))
        .fetch_all(&mut *aconn)
        .await
        .unwrap();
    assert_eq!(
        rows.len(),
        1,
        "RLS must hide tenantB's dedup row (no cross-tenant query path)"
    );
    assert_eq!(rows[0].get::<String, _>("tenant_id"), "tenantA");
    assert_eq!(rows[0].get::<String, _>("event_id"), "evt-push-1");

    // ── 5. PROVE forward-only: no DROP in the production DDL. ──
    assert!(
        !CREATE_CONSUMER_DEDUP_DDL
            .to_ascii_uppercase()
            .contains("DROP"),
        "the consumer_dedup schema migration is forward-only (no DROP)"
    );

    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}

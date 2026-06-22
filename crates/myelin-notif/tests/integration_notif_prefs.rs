//! Live-Postgres integration test (Stage 1 / infra) — the `notif_pref` / `notif_quiet_hours`
//! read/write contract (NOTIF-P10 / P-188, contract 7.4): the `(tenant_id, region, principal)`
//! UPSERT round-trips, the `pierce_classes` default is `{critical}`, and RLS isolates a tenant's
//! prefs — **0 cross-tenant prefs rows readable** — proven against REAL Postgres.
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free (the binding-policy floor — no DB at build). This runs
//! ONLY against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-notif --features integration --test integration_notif_prefs -- --nocapture
//!
//! Endpoints come from the myelin-config dev defaults (the dev<->prod CONFIG SWAP seam), so the same
//! test runs against Scaleway (fr-par) by exporting the prod env vars — never a code change.
//!
//! It proves, against REAL Postgres, that:
//!   1. The `notif_pref` UPSERT (`ON CONFLICT (tenant_id, region, principal) DO UPDATE`) round-trips
//!      the routing matcher (a frozen-QueryAst predicate stored as jsonb) + the digest config — a
//!      second set_prefs for the SAME principal UPDATES in place (the §2.2 per-principal row).
//!   2. The `notif_quiet_hours` DDL default `pierce_classes` is `{critical}` — the on-call override
//!      is a REAL database default (you cannot accidentally ship a row that silences on-call, §2.2).
//!   3. RLS isolates prefs end-to-end: a session set to tenant A reads ONLY tenant A's prefs row —
//!      **0 cross-tenant rows readable** (the no-cross-tenant-query-path invariant, in Postgres).
//!      The app role is NOSUPERUSER NOBYPASSRLS, so the policy is actually in force.
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_notif::migrations::{rls_scope_sql, NOTIF_PREF_DDL, QUIET_HOURS_DDL};

#[tokio::test]
async fn notif_prefs_upsert_round_trips_pierce_default_and_rls_denies_cross_tenant() {
    use sqlx::Row;

    let cfg = MyelinConfig::dev();
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&cfg.database_url)
        .await
        .expect("connect to dev Postgres as the app role (is the stack up?)");
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(
            &cfg.database_url
                .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw"),
        )
        .await
        .expect("connect as admin");

    let pref_tbl = format!("notif_pref_p188_{}", std::process::id());
    let quiet_tbl = format!("notif_quiet_hours_p188_{}", std::process::id());
    let pref_ddl = NOTIF_PREF_DDL.replacen("notif_pref", &pref_tbl, 1);
    let quiet_ddl = QUIET_HOURS_DDL.replacen("notif_quiet_hours", &quiet_tbl, 1);

    for t in [&pref_tbl, &quiet_tbl] {
        sqlx::query(&format!("DROP TABLE IF EXISTS {t}"))
            .execute(&admin)
            .await
            .unwrap();
    }
    sqlx::query(&pref_ddl)
        .execute(&admin)
        .await
        .expect("the notif_pref DDL applies");
    sqlx::query(&quiet_ddl)
        .execute(&admin)
        .await
        .expect("the notif_quiet_hours DDL applies");
    for t in [&pref_tbl, &quiet_tbl] {
        sqlx::query(&rls_scope_sql(t))
            .execute(&admin)
            .await
            .expect("RLS policy installs");
        sqlx::query(&format!("GRANT ALL ON {t} TO myelin_app"))
            .execute(&admin)
            .await
            .unwrap();
    }

    // ---- (1) UPSERT round-trip: set_prefs twice for the SAME principal UPDATES in place ---------
    let mut conn = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantA', false)")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
        .execute(&mut *conn)
        .await
        .unwrap();

    let upsert = format!(
        "INSERT INTO {pref_tbl} (tenant_id, region, principal, routing, digest, dek_ref) \
         VALUES ('tenantA', 'fr-par', 'psn:alice', $1::jsonb, $2::jsonb, 'kms://acme/0/tenant') \
         ON CONFLICT (tenant_id, region, principal) DO UPDATE SET routing = EXCLUDED.routing, digest = EXCLUDED.digest"
    );
    // first set: route critical → email.
    sqlx::query(&upsert)
        .bind(r#"[{"channel":"email","matcher":{"Cmp":{"op":"Eq","lhs":{"Var":"class"},"rhs":{"Lit":{"Str":"critical"}}}}}]"#)
        .bind(r#"{"cadence":"off"}"#)
        .execute(&mut *conn)
        .await
        .expect("first set_prefs UPSERT");
    // second set for the SAME principal: route direct → mobile_push (UPDATE in place).
    sqlx::query(&upsert)
        .bind(r#"[{"channel":"mobile_push","matcher":{"Cmp":{"op":"Eq","lhs":{"Var":"class"},"rhs":{"Lit":{"Str":"direct"}}}}}]"#)
        .bind(r#"{"cadence":"daily"}"#)
        .execute(&mut *conn)
        .await
        .expect("second set_prefs UPSERT (UPDATE in place)");

    let rows = sqlx::query(&format!(
        "SELECT routing, digest FROM {pref_tbl} WHERE principal = 'psn:alice'"
    ))
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        rows.len(),
        1,
        "the per-principal row is UPSERTed (one row, not two)"
    );
    let routing: serde_json::Value = rows[0].get("routing");
    assert_eq!(
        routing[0]["channel"], "mobile_push",
        "the second set_prefs UPDATED the routing in place"
    );
    let digest: serde_json::Value = rows[0].get("digest");
    assert_eq!(
        digest["cadence"], "daily",
        "the digest config round-trips (stored only — compose is the OQ5 floor)"
    );

    // ---- (2) the DDL default pierce_classes = {critical} (the on-call override is a real default) -
    sqlx::query(&format!(
        "INSERT INTO {quiet_tbl} (tenant_id, region, principal, tz, windows, dek_ref) \
         VALUES ('tenantA', 'fr-par', 'psn:alice', 'Europe/Paris', '[]'::jsonb, 'kms://acme/0/tenant')"
    ))
    .execute(&mut *conn)
    .await
    .expect("quiet-hours insert (relying on the pierce_classes DEFAULT)");
    let pierce: Vec<String> = sqlx::query_scalar(&format!(
        "SELECT pierce_classes FROM {quiet_tbl} WHERE principal = 'psn:alice'"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        pierce,
        vec!["critical".to_string()],
        "pierce_classes defaults to {{critical}} (you cannot silence on-call)"
    );

    // ---- (3) RLS denies cross-tenant: seed tenant B, read as tenant A → 0 cross-tenant rows ------
    {
        let mut admin_conn = admin.acquire().await.unwrap();
        sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantB', false)")
            .execute(&mut *admin_conn)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
            .execute(&mut *admin_conn)
            .await
            .unwrap();
        sqlx::query(&format!(
            "INSERT INTO {pref_tbl} (tenant_id, region, principal, routing, digest, dek_ref) \
             VALUES ('tenantB', 'fr-par', 'psn:bob', '[]'::jsonb, NULL, 'kms://acme/0/tenant')"
        ))
        .execute(&mut *admin_conn)
        .await
        .unwrap();
    }
    // back as tenant A: the only visible prefs row is tenant A's.
    let visible = sqlx::query(&format!("SELECT tenant_id FROM {pref_tbl}"))
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        visible.len(),
        1,
        "RLS hides tenant B's prefs — 0 cross-tenant rows"
    );
    assert_eq!(visible[0].get::<String, _>("tenant_id"), "tenantA");
    let cross: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM {pref_tbl} WHERE tenant_id = 'tenantB'"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        cross, 0,
        "a tenant-A session reads 0 cross-tenant (tenantB) prefs rows"
    );

    for t in [&pref_tbl, &quiet_tbl] {
        sqlx::query(&format!("DROP TABLE {t}"))
            .execute(&admin)
            .await
            .unwrap();
    }
}

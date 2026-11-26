#![cfg(feature = "integration")]

//! The human-session migrations on a fresh schema: people erased before 0183 are disabled by
//! 0185, only a permanent principal-wide revocation disables anyone, and (0186, 0187) nothing
//! enables or re-creates an erased person.

use std::str::FromStr;

mod common;

use myelin_storage::migration::Migrations;
use myelin_storage::{
    all_durable_migrations, foundation_migrations, human_session_migrations, PgMigrator,
    ERASED_PRINCIPAL_SQLSTATE,
};
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};

const TENANT: &str = "acme";
const REGION: &str = "eu-west";

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

async fn person(pool: &PgPool, id: &str) {
    sqlx::query(
        "INSERT INTO principal (tenant_id, region, principal_id, kind, data_role, status) \
         VALUES ($1, $2, $3, '\"Human\"', '\"Controller\"', '\"Active\"')",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(id)
    .execute(pool)
    .await
    .expect("insert a person");
}

async fn revoke(pool: &PgPool, id: &str, expires_at: Option<&str>) {
    sqlx::query(
        "INSERT INTO revocation (tenant_id, region, kind, handle, revoked_at, expires_at) \
         VALUES ($1, $2, 'principal', $3, '2026-11-26T00:00:00Z', $4)",
    )
    .bind(TENANT)
    .bind(REGION)
    .bind(id)
    .bind(expires_at)
    .execute(pool)
    .await
    .expect("insert a principal-wide revocation");
}

async fn state(pool: &PgPool, id: &str) -> (String, Option<i64>) {
    sqlx::query_as("SELECT status, sessions_not_before FROM principal WHERE principal_id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("read the person")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn people_erased_before_0183_are_disabled_and_only_permanent_revocations_disable() {
    let schema = format!("human_sessions_{}", uniq());
    let admin_url = common::admin_database_config().database_url;
    let bootstrap = PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
        .expect("connect to the admin Postgres backend");
    sqlx::query(&format!("CREATE SCHEMA \"{schema}\""))
        .execute(&bootstrap)
        .await
        .expect("create a fresh schema");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(
            PgConnectOptions::from_str(&admin_url)
                .expect("admin DSN")
                .options([("search_path", format!("\"{schema}\",public").as_str())]),
        )
        .await
        .expect("a pool pinned to the fresh schema");

    // Everything up to 0182: the cutoff column and the status trigger, but no erasure trigger.
    let human = human_session_migrations();
    let before_0183 = Migrations::of(human.0.iter().take(2).cloned());
    assert_eq!(before_0183.0[1].id, "0182_principal_status_ends_sessions");
    PgMigrator::apply(&pool, &foundation_migrations())
        .await
        .expect("foundation");
    PgMigrator::apply(&pool, &all_durable_migrations_without(&human))
        .await
        .expect("the other durable migrations");
    PgMigrator::apply(&pool, &before_0183)
        .await
        .expect("0181 and 0182");

    person(&pool, "erased-early").await;
    person(&pool, "suspended-for-a-while").await;
    person(&pool, "untouched").await;
    revoke(&pool, "erased-early", None).await;
    revoke(&pool, "suspended-for-a-while", Some("2099-01-01T00:00:00Z")).await;
    let (status, cutoff) = state(&pool, "erased-early").await;
    assert_eq!(
        status, "\"Active\"",
        "before 0183, an erasure left the person active"
    );
    let early_cutoff = cutoff.expect("0182's revocation trigger moved the cutoff");

    PgMigrator::apply(&pool, &human)
        .await
        .expect("0183 to 0185");

    let (status, cutoff) = state(&pool, "erased-early").await;
    assert_eq!(
        status, "\"Disabled\"",
        "0185 disables a person erased before 0183"
    );
    assert!(
        cutoff.unwrap() > early_cutoff,
        "and moves their cutoff again"
    );
    assert_eq!(state(&pool, "suspended-for-a-while").await.0, "\"Active\"");
    assert_eq!(state(&pool, "untouched").await.0, "\"Active\"");

    // From now on: a time-limited revocation leaves the person be; a permanent one disables.
    revoke(&pool, "untouched", Some("2099-01-01T00:00:00Z")).await;
    assert_eq!(state(&pool, "untouched").await.0, "\"Active\"");
    sqlx::query("DELETE FROM revocation WHERE handle = 'untouched'")
        .execute(&pool)
        .await
        .unwrap();
    revoke(&pool, "untouched", None).await;
    assert_eq!(state(&pool, "untouched").await.0, "\"Disabled\"");

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA \"{schema}\" CASCADE"))
        .execute(&bootstrap)
        .await
        .expect("drop the fresh schema");
}

fn sqlstate(error: &sqlx::Error) -> Option<String> {
    error
        .as_database_error()
        .and_then(|db| db.code())
        .map(|code| code.into_owned())
}

async fn set_status(pool: &PgPool, id: &str, status: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE principal SET status = $1 WHERE principal_id = $2")
        .bind(status)
        .bind(id)
        .execute(pool)
        .await
        .map(|_| ())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_erased_person_can_never_be_enabled_again() {
    let schema = format!("erasure_sticks_{}", uniq());
    let admin_url = common::admin_database_config().database_url;
    let bootstrap = PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
        .expect("connect to the admin Postgres backend");
    sqlx::query(&format!("CREATE SCHEMA \"{schema}\""))
        .execute(&bootstrap)
        .await
        .expect("create a fresh schema");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(
            PgConnectOptions::from_str(&admin_url)
                .expect("admin DSN")
                .options([("search_path", format!("\"{schema}\",public").as_str())]),
        )
        .await
        .expect("a pool pinned to the fresh schema");
    PgMigrator::apply(&pool, &foundation_migrations())
        .await
        .expect("foundation");
    PgMigrator::apply(&pool, &all_durable_migrations())
        .await
        .expect("every durable migration");

    person(&pool, "erased").await;
    revoke(&pool, "erased", None).await;
    assert_eq!(state(&pool, "erased").await.0, "\"Disabled\"");
    for status in ["\"Active\"", "\"Suspended\""] {
        let refused = set_status(&pool, "erased", status)
            .await
            .expect_err("an erased person stays disabled");
        assert_eq!(
            sqlstate(&refused),
            Some(ERASED_PRINCIPAL_SQLSTATE.to_string()),
            "{refused}"
        );
    }
    let upsert = sqlx::query(
        "INSERT INTO principal (tenant_id, region, principal_id, kind, data_role, status) \
         VALUES ($1, $2, 'erased', '\"Human\"', '\"Controller\"', '\"Active\"') \
         ON CONFLICT (tenant_id, region, principal_id) DO UPDATE SET status = EXCLUDED.status",
    )
    .bind(TENANT)
    .bind(REGION)
    .execute(&pool)
    .await;
    assert_eq!(
        sqlstate(&upsert.expect_err("nor does an upsert enable them")),
        Some(ERASED_PRINCIPAL_SQLSTATE.to_string())
    );

    // Nor can an erased id come back as a new row.
    sqlx::query("DELETE FROM principal WHERE principal_id = 'erased'")
        .execute(&pool)
        .await
        .unwrap();
    let recreated = sqlx::query(
        "INSERT INTO principal (tenant_id, region, principal_id, kind, data_role, status) \
         VALUES ($1, $2, 'erased', '\"Human\"', '\"Controller\"', '\"Active\"')",
    )
    .bind(TENANT)
    .bind(REGION)
    .execute(&pool)
    .await
    .expect_err("an erased id cannot be inserted active");
    assert_eq!(
        sqlstate(&recreated),
        Some(ERASED_PRINCIPAL_SQLSTATE.to_string())
    );
    sqlx::query(
        "INSERT INTO principal (tenant_id, region, principal_id, kind, data_role, status) \
         VALUES ($1, $2, 'erased', '\"Human\"', '\"Controller\"', '\"Disabled\"')",
    )
    .bind(TENANT)
    .bind(REGION)
    .execute(&pool)
    .await
    .expect("an erased id may exist disabled");
    set_status(&pool, "erased", "\"Disabled\"")
        .await
        .expect("writing Disabled again is fine");

    // A person disabled without an erasure, or under a time-limited revocation, can come back.
    person(&pool, "paused").await;
    revoke(&pool, "paused", Some("2099-01-01T00:00:00Z")).await;
    set_status(&pool, "paused", "\"Disabled\"").await.unwrap();
    set_status(&pool, "paused", "\"Active\"")
        .await
        .expect("only an erasure sticks");

    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA \"{schema}\" CASCADE"))
        .execute(&bootstrap)
        .await
        .expect("drop the fresh schema");
}

/// Every durable migration except the human-session group, in boot order.
fn all_durable_migrations_without(human: &Migrations) -> Migrations {
    Migrations::of(
        all_durable_migrations()
            .0
            .into_iter()
            .filter(|migration| !human.0.iter().any(|own| own.id == migration.id)),
    )
}

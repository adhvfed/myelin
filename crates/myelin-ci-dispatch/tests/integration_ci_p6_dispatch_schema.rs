#![cfg(feature = "integration")]

use myelin_ci_dispatch::CREATE_CONSUMER_DEDUP_DDL;
use sqlx::Row;

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

#[tokio::test]
async fn consumer_dedup_matches_foundation_and_enforces_exactly_once_key() {
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is docker-compose.dev.yml up?)");
    let table = format!("consumer_dedup_p349_{}", std::process::id());

    let create = CREATE_CONSUMER_DEDUP_DDL
        .replace("EXISTS consumer_dedup (", &format!("EXISTS {table} ("))
        .replace("consumer_dedup_pk", &format!("{table}_pk"));
    sqlx::query(&create)
        .execute(&admin)
        .await
        .expect("apply the shared consumer_dedup DDL");

    assert_eq!(
        CREATE_CONSUMER_DEDUP_DDL,
        myelin_events::CONSUMER_DEDUP_MIGRATION,
        "Dispatch must reuse the foundation DDL byte-for-byte"
    );
    assert!(!CREATE_CONSUMER_DEDUP_DDL.contains("tenant_id"));
    assert!(!CREATE_CONSUMER_DEDUP_DDL.contains("myelin_make_tenant_scoped"));

    sqlx::query(&format!(
        "INSERT INTO {table} (consumer, event_id) VALUES ('ci-dispatch', 'evt-push-1')"
    ))
    .execute(&admin)
    .await
    .expect("first delivery records its dedup key");

    let duplicate = sqlx::query(&format!(
        "INSERT INTO {table} (consumer, event_id) VALUES ('ci-dispatch', 'evt-push-1')"
    ))
    .execute(&admin)
    .await;
    assert!(
        duplicate.is_err(),
        "the shared primary key rejects a duplicate delivery"
    );

    let idempotent = sqlx::query(&format!(
        "INSERT INTO {table} (consumer, event_id) \
         VALUES ('ci-dispatch', 'evt-push-1') \
         ON CONFLICT (consumer, event_id) DO NOTHING"
    ))
    .execute(&admin)
    .await
    .expect("idempotent redelivery succeeds as a no-op");
    assert_eq!(idempotent.rows_affected(), 0);

    let row = sqlx::query(&format!(
        "SELECT event_id, recorded_at IS NOT NULL AS timestamped FROM {table}"
    ))
    .fetch_one(&admin)
    .await
    .expect("read the recorded delivery");
    assert_eq!(row.get::<String, _>("event_id"), "evt-push-1");
    assert!(row.get::<bool, _>("timestamped"));
    assert!(!CREATE_CONSUMER_DEDUP_DDL
        .to_ascii_uppercase()
        .contains("DROP"));

    sqlx::query(&format!("DROP TABLE {table}"))
        .execute(&admin)
        .await
        .expect("drop only the isolated proof table");
}

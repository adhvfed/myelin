#![cfg(feature = "integration")]

use myelin_ci_dispatch::CREATE_CONSUMER_DEDUP_DDL;
use sqlx::{PgPool, Row};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

async fn reopen() -> PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&admin_url())
        .await
        .expect("reconnect to dev Postgres as admin (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)")
}

#[tokio::test]
async fn consumer_dedup_double_delivery_lands_once_across_kill9() {
    let suffix = std::process::id();
    let tbl = format!("consumer_dedup_ct004_{suffix}");

    let p1 = reopen().await;
    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
        .execute(&p1)
        .await
        .ok();
    let create = CREATE_CONSUMER_DEDUP_DDL
        .replace("EXISTS consumer_dedup (", &format!("EXISTS {tbl} ("))
        .replace("consumer_dedup_pk", &format!("{tbl}_pk"));
    sqlx::query(&create)
        .execute(&p1)
        .await
        .expect("apply the consumer_dedup DDL");

    let deliver = |pool: &PgPool, event: &'static str| {
        let pool = pool.clone();
        let tbl = tbl.clone();
        async move {
            sqlx::query(&format!(
                "INSERT INTO {tbl} (consumer, event_id) \
                 VALUES ('ci-dispatch.trigger','{event}') \
                 ON CONFLICT (consumer, event_id) DO NOTHING"
            ))
            .execute(&pool)
            .await
            .expect("the ON CONFLICT DO NOTHING delivery applies")
            .rows_affected()
        }
    };

    assert_eq!(
        deliver(&p1, "evt-push-1").await,
        1,
        "the first delivery records the dedup row (the effect fires - one run)"
    );

    drop(p1);

    let p2 = reopen().await;
    assert_eq!(
        deliver(&p2, "evt-push-1").await,
        0,
        "after kill-9/reopen, a re-delivery of the same event is a no-op - the dedup row was durable \
         (a crash-then-redeliver does NOT double-run)"
    );
    assert_eq!(
        deliver(&p2, "evt-push-2").await,
        1,
        "a new event still fires once after reopen"
    );

    let count: i64 = sqlx::query(&format!(
        "SELECT count(*)::bigint AS n FROM {tbl} WHERE consumer='ci-dispatch.trigger' AND event_id='evt-push-1'"
    ))
    .fetch_one(&p2)
    .await
    .unwrap()
    .get("n");
    assert_eq!(
        count, 1,
        "double-effect = 0: the doubly-delivered event lands EXACTLY once"
    );

    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&p2)
        .await
        .ok();
    println!(
        "[CT-004] PASS dispatch dedup: deliver(evt-push-1)=1 then kill-9 → reopen → re-deliver=0 \
         (lands once, double-effect=0); a new event still fires once"
    );
}

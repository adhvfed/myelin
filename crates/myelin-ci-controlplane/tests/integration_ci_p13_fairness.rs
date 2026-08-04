#![cfg(feature = "integration")]

use myelin_ci_controlplane::{
    PlanTier, ADVANCE_DEFICIT_QUERY, BASE_QUANTUM, DEFICIT_CEILING, IN_FLIGHT_COUNT_QUERY,
    REPLENISH_DEFICIT_QUERY,
};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

#[tokio::test]
async fn drr_advance_replenish_and_in_flight_count_on_live_postgres() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)");

    let suffix = std::process::id();
    let fair = format!("fair_deficit_p356_{suffix}");
    let jq = format!("job_queue_p356_{suffix}");

    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {fair} (tenant_id text NOT NULL, region text NOT NULL, \
         fair_key text NOT NULL, deficit bigint NOT NULL DEFAULT 0, \
         last_served timestamptz NOT NULL DEFAULT now(), \
         PRIMARY KEY (tenant_id, region, fair_key))"
    ))
    .execute(&admin)
    .await
    .expect("create the suffixed fair_deficit");
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {jq} (tenant_id text NOT NULL, region text NOT NULL, \
         job_id text NOT NULL, state text NOT NULL, PRIMARY KEY (tenant_id, job_id))"
    ))
    .execute(&admin)
    .await
    .expect("create the suffixed job_queue");

    let mut conn = admin.acquire().await.unwrap();

    let advance_sql = ADVANCE_DEFICIT_QUERY.replace("fair_deficit", &fair);
    let replenish_sql = REPLENISH_DEFICIT_QUERY.replace("fair_deficit", &fair);
    let in_flight_sql = IN_FLIGHT_COUNT_QUERY.replace("job_queue", &jq);

    let bind_advance = |tenant: &str, region: &str, fair_key: &str, quantum: i64| {
        advance_sql
            .replacen("$1", &format!("'{tenant}'"), 1)
            .replacen("$2", &format!("'{region}'"), 1)
            .replacen("$3", &format!("'{fair_key}'"), 1)
            .replace("$4", &quantum.to_string())
    };

    let d1: i64 = sqlx::query(&bind_advance("hot", "fr-par", "hot", BASE_QUANTUM))
        .fetch_one(&mut *conn)
        .await
        .expect("the first advance UPSERTs the deficit")
        .get("deficit");
    assert_eq!(
        d1, -BASE_QUANTUM,
        "a first-ever claim materialises the fair_key at -quantum (the served key drops)"
    );
    let d2: i64 = sqlx::query(&bind_advance("hot", "fr-par", "hot", BASE_QUANTUM))
        .fetch_one(&mut *conn)
        .await
        .expect("the second advance decrements again")
        .get("deficit");
    assert_eq!(
        d2,
        -2 * BASE_QUANTUM,
        "a second serve decrements again (the hot tenant drops further in deficit DESC)"
    );

    for t in ["quiet1", "quiet2"] {
        sqlx::query(&bind_advance(t, "fr-par", t, BASE_QUANTUM))
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    }

    let order: Vec<(String, i64)> = sqlx::query(&format!(
        "SELECT fair_key, deficit FROM {fair} WHERE region='fr-par' ORDER BY deficit DESC"
    ))
    .fetch_all(&mut *conn)
    .await
    .unwrap()
    .iter()
    .map(|r| (r.get::<String, _>("fair_key"), r.get::<i64, _>("deficit")))
    .collect();
    assert_eq!(
        order.last().map(|(k, _)| k.as_str()),
        Some("hot"),
        "the over-served hot tenant sorts LAST in deficit DESC - the next claim picks a waiting \
         tenant (no starvation)"
    );

    let replenish_with = |region: &str, weighted_quantum: i64, ceiling: i64| {
        replenish_sql
            .replacen("$1", &format!("'{region}'"), 1)
            .replacen("$2", &weighted_quantum.to_string(), 1)
            .replacen("$3", &ceiling.to_string(), 1)
    };
    let free_q = BASE_QUANTUM * PlanTier::Free.quantum_weight();
    sqlx::query(&replenish_with("fr-par", free_q, DEFICIT_CEILING))
        .execute(&mut *conn)
        .await
        .expect("the Free-weighted replenish sweep");
    let hot_after: i64 = sqlx::query(&format!(
        "SELECT deficit FROM {fair} WHERE region='fr-par' AND fair_key='hot'"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap()
    .get("deficit");
    assert_eq!(
        hot_after,
        -2 * BASE_QUANTUM + free_q,
        "the Free replenish adds exactly the base quantum (the served tenant recovers over time)"
    );

    sqlx::query(&format!(
        "INSERT INTO {fair} (tenant_id, region, fair_key, deficit) VALUES \
         ('ent','fr-par','ent', {})",
        DEFICIT_CEILING - 1
    ))
    .execute(&mut *conn)
    .await
    .unwrap();
    let ent_q = BASE_QUANTUM * PlanTier::Enterprise.quantum_weight();
    sqlx::query(&replenish_with("fr-par", ent_q, DEFICIT_CEILING))
        .execute(&mut *conn)
        .await
        .unwrap();
    let ent_after: i64 = sqlx::query(&format!(
        "SELECT deficit FROM {fair} WHERE region='fr-par' AND fair_key='ent'"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap()
    .get("deficit");
    assert_eq!(
        ent_after, DEFICIT_CEILING,
        "the deficit is CLAMPED at the burst-credit ceiling (no unbounded priority hoarding)"
    );

    for (jid, state) in [
        ("a1", "leased"),
        ("a2", "leased"),
        ("a3", "running"),
        ("a4", "queued"),
        ("a5", "terminal"),
    ] {
        sqlx::query(&format!(
            "INSERT INTO {jq} (tenant_id, region, job_id, state) VALUES ('tenantA','fr-par','{jid}','{state}')"
        ))
        .execute(&mut *conn)
        .await
        .unwrap();
    }
    sqlx::query(&format!(
        "INSERT INTO {jq} (tenant_id, region, job_id, state) VALUES ('tenantB','fr-par','b1','leased')"
    ))
    .execute(&mut *conn)
    .await
    .unwrap();

    let count_for = |tenant: &str, region: &str| {
        in_flight_sql
            .replacen("$1", &format!("'{tenant}'"), 1)
            .replacen("$2", &format!("'{region}'"), 1)
    };
    let a_in_flight: i64 = sqlx::query(&count_for("tenantA", "fr-par"))
        .fetch_one(&mut *conn)
        .await
        .unwrap()
        .get("in_flight");
    assert_eq!(
        a_in_flight, 3,
        "the in-flight count is leased+running (queued + terminal are NOT in flight)"
    );
    let b_in_flight: i64 = sqlx::query(&count_for("tenantB", "fr-par"))
        .fetch_one(&mut *conn)
        .await
        .unwrap()
        .get("in_flight");
    assert_eq!(
        b_in_flight, 1,
        "the count is PER-TENANT - tenantB's load does not include tenantA's (blast radius)"
    );

    sqlx::query(&format!("DROP TABLE IF EXISTS {fair}"))
        .execute(&mut *conn)
        .await
        .ok();
    sqlx::query(&format!("DROP TABLE IF EXISTS {jq}"))
        .execute(&mut *conn)
        .await
        .ok();
}

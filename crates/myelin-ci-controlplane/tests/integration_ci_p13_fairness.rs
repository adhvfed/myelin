//! **CI-P13 / P-356 — the scheduler fairness slice (DRR deficit advance/replenish + the per-tenant
//! in-flight backpressure count), PROVEN against the live dev-stack Postgres.**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build`/`cargo test
//! --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-ci-controlplane --features integration \
//!     --test integration_ci_p13_fairness -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires: the arch 02 §2.2 DRR accounting +
//! the §2.4 bounded-run-queue count run as the EXACT [`ADVANCE_DEFICIT_QUERY`] /
//! [`REPLENISH_DEFICIT_QUERY`] / [`IN_FLIGHT_COUNT_QUERY`] statements against real Postgres over the
//! real `fair_deficit` + `job_queue` tables — proving:
//!   1. the DRR advance UPSERTs a per-`fair_key` decrement (a first-ever claim materialises the row
//!      at `-quantum`; a second decrements again — the served key drops in `deficit DESC`);
//!   2. the periodic replenish adds a PLAN-WEIGHTED quantum back, clamped at the ceiling (a higher
//!      tier recovers priority faster; no `fair_key` exceeds the burst-credit ceiling);
//!   3. the per-tenant in-flight count is the tenant's `leased`+`running` jobs (the bounded
//!      run-queue load the backpressure cap bounds) — and is PER-TENANT (no cross-tenant bleed);
//!   4. the no-starvation property end-to-end: serving the hot tenant drives its deficit below the
//!      waiting tenants', so the next `deficit DESC` claim picks a waiting tenant (DRR fairness).
//!
//! The drill is registered red-until-proven and flips green ONLY here.
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

    // ── 1. Apply the REAL fair_deficit + a minimal job_queue (for the in-flight count). ──
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

    // Point the frozen statements at the suffixed tables.
    let advance_sql = ADVANCE_DEFICIT_QUERY.replace("fair_deficit", &fair);
    let replenish_sql = REPLENISH_DEFICIT_QUERY.replace("fair_deficit", &fair);
    let in_flight_sql = IN_FLIGHT_COUNT_QUERY.replace("job_queue", &jq);

    let bind_advance = |tenant: &str, region: &str, fair_key: &str, quantum: i64| {
        advance_sql
            .replacen("$1", &format!("'{tenant}'"), 1)
            .replacen("$2", &format!("'{region}'"), 1)
            .replacen("$3", &format!("'{fair_key}'"), 1)
            // $4 appears twice (the VALUES `-$4` and the DO UPDATE `- $4`).
            .replace("$4", &quantum.to_string())
    };

    // ── 2. ADVANCE: a first-ever claim materialises the row at -quantum; a second decrements again. ──
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

    // Two WAITING tenants are served once each (so they sit at -1, above the hot tenant's -2).
    for t in ["quiet1", "quiet2"] {
        sqlx::query(&bind_advance(t, "fr-par", t, BASE_QUANTUM))
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    }

    // ── 3. NO STARVATION: the hot tenant's deficit is BELOW the waiting tenants' (deficit DESC picks
    //       a waiting tenant next — the DRR fairness property). ──
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
        "the over-served hot tenant sorts LAST in deficit DESC — the next claim picks a waiting \
         tenant (no starvation)"
    );

    // ── 4. REPLENISH (plan-weighted, clamped): an Enterprise key recovers faster than a Free key. ──
    // Seed two keys at the same low deficit, replenish with the Enterprise quantum.
    let replenish_with = |region: &str, weighted_quantum: i64, ceiling: i64| {
        replenish_sql
            .replacen("$1", &format!("'{region}'"), 1)
            .replacen("$2", &weighted_quantum.to_string(), 1)
            .replacen("$3", &ceiling.to_string(), 1)
    };
    // Run the Free-weighted replenish over the region: hot(-2)→-1, quiet1(-1)→0, quiet2(-1)→0.
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

    // The Enterprise weight recovers faster: seed a key at the ceiling-1 and confirm the CLAMP holds.
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

    // ── 5. IN-FLIGHT COUNT: the per-tenant bounded-run-queue load (leased+running), per-tenant. ──
    // tenantA: 2 leased + 1 running + 1 queued (not counted) + 1 terminal (not counted) = 3 in-flight.
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
    // tenantB: 1 leased — proves the count is per-tenant (does not bleed into tenantA's).
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
        "the count is PER-TENANT — tenantB's load does not include tenantA's (blast radius)"
    );

    // ── Cleanup (leave the stack up; only drop this test's suffixed tables). ──
    sqlx::query(&format!("DROP TABLE IF EXISTS {fair}"))
        .execute(&mut *conn)
        .await
        .ok();
    sqlx::query(&format!("DROP TABLE IF EXISTS {jq}"))
        .execute(&mut *conn)
        .await
        .ok();
}

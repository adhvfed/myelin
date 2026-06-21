//! Live-Postgres integration test (Stage 1 / infra) — the snooze re-surface durable-state contract
//! (NOTIF-P18 / P-196, contract 7.2 snooze + 9.3 the durable wheel): the `notif_inbox_item` row's
//! snooze state (`state='snoozed', snooze_until=<until>`) is the DURABLE handle a re-surface timer
//! fires against. This proves, against REAL Postgres, that:
//!
//!   1. The snooze recording (`UPDATE … SET state='snoozed', snooze_until=<until>`) persists the
//!      durable snooze handle — the row a Notif RESTART reads back to know the item is parked.
//!   2. The re-surface UPDATE (`SET state='unread', snooze_until=NULL WHERE … AND state='snoozed'`)
//!      flips a due snooze back to the active inbox in place — EXACTLY ONCE: the `state='snoozed'`
//!      guard makes a re-played fire (a restart over the already-re-surfaced row) flip ZERO rows
//!      (0 duplicate re-surface; the durable effectively-once property at the row level).
//!   3. The `state` CHECK constraint rejects an off-grammar state — the six-state machine is a REAL
//!      database invariant (a typo'd state cannot persist).
//!   4. RLS isolates the snooze state end-to-end: a tenant-B session reads 0 of tenant A's inbox rows
//!      — **0 cross-tenant rows readable** (the no-cross-tenant-query-path invariant, in Postgres).
//!      The app role is NOSUPERUSER NOBYPASSRLS, so the policy is actually in force.
//!
//! Gated behind the `integration` cargo feature so the DEFAULT `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free (the binding-policy floor — no DB at build). This runs ONLY
//! against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-notif --features integration --test integration_notif_snooze_resurface -- --nocapture
//!
//! Endpoints come from the myelin-config dev defaults (the dev<->prod CONFIG SWAP seam), so the same
//! test runs against Scaleway (fr-par) by exporting the prod env vars — never a code change.
//!
//! The durable TIMER itself is `myelin-flow`'s (P-FLOW-09/P-FLOW-13, a named floor; the in-memory
//! `InMemoryWheel` models the effectively-once fire in the CI drill). This integration test proves the
//! ROW-LEVEL durable state the timer fires against — the half of "0 missed / 0 duplicate re-surface"
//! that lives in Postgres.
#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_notif::migrations::{rls_scope_sql, INBOX_ITEM_DDL};

#[tokio::test]
async fn notif_snooze_resurface_update_round_trips_exactly_once_and_rls_denies_cross_tenant() {
    use sqlx::Row;

    let cfg = MyelinConfig::dev();
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&cfg.database_url)
        .await
        .expect("connect to dev Postgres as the app role (is the stack up?)");
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&cfg.database_url.replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw"))
        .await
        .expect("connect as admin");

    let tbl = format!("notif_inbox_item_p196_{}", std::process::id());
    let ddl = INBOX_ITEM_DDL.replacen("notif_inbox_item", &tbl, 1);

    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}")).execute(&admin).await.unwrap();
    sqlx::query(&ddl).execute(&admin).await.expect("the notif_inbox_item DDL applies");
    sqlx::query(&rls_scope_sql(&tbl)).execute(&admin).await.expect("RLS policy installs");
    sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app")).execute(&admin).await.unwrap();

    // ---- (1) seed an item for tenant A, then SNOOZE it (record the durable snooze handle) --------
    let mut conn = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantA', false)").execute(&mut *conn).await.unwrap();
    sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)").execute(&mut *conn).await.unwrap();

    let insert = format!(
        "INSERT INTO {tbl} \
         (tenant_id, region, item_id, recipient, subject, subject_root, reason, class, origin_event, \
          template_key, template_args_json, dedup_key, occurred_at, dek_ref) \
         VALUES ('tenantA','fr-par','itm-1','psn:alice','myelin://tenantA/issues/issue/7', \
                 'myelin://tenantA/issues/issue/7','assigned','direct','myelin://tenantA/bus/event/e1', \
                 'issue.assigned','[]'::jsonb,'itm-1', now(), 'kms://tenantA/0/tenant')"
    );
    sqlx::query(&insert).execute(&mut *conn).await.expect("the inbox item inserts (state defaults unread)");

    // snooze: record the durable snooze handle (state='snoozed', snooze_until=<until>).
    let snooze = format!(
        "UPDATE {tbl} SET state='snoozed', snooze_until='2026-06-25T09:00:00Z'::timestamptz \
         WHERE recipient='psn:alice' AND item_id='itm-1'"
    );
    let n = sqlx::query(&snooze).execute(&mut *conn).await.expect("the snooze UPDATE applies").rows_affected();
    assert_eq!(n, 1, "exactly the one item is snoozed");
    // Read state + a SQL-side NULL check on snooze_until (avoids naming a chrono type — the column is
    // timestamptz; `snooze_until IS NOT NULL` proves the re-surface instant persisted).
    let row = sqlx::query(&format!(
        "SELECT state, (snooze_until IS NOT NULL) AS has_until FROM {tbl} WHERE item_id='itm-1'"
    ))
    .fetch_one(&mut *conn)
    .await
    .expect("the durable snooze handle reads back");
    assert_eq!(row.get::<String, _>("state"), "snoozed", "the item is parked (durable snooze handle)");
    assert!(
        row.get::<bool, _>("has_until"),
        "the snooze_until is persisted (the re-surface instant the timer fires at)"
    );

    // ---- (2) the re-surface UPDATE flips snoozed → unread EXACTLY ONCE (the state='snoozed' guard) -
    let resurface = format!(
        "UPDATE {tbl} SET state='unread', snooze_until=NULL \
         WHERE recipient='psn:alice' AND item_id='itm-1' AND state='snoozed'"
    );
    let first = sqlx::query(&resurface).execute(&mut *conn).await.expect("re-surface UPDATE").rows_affected();
    assert_eq!(first, 1, "the first re-surface flips the snoozed row (NOT zero — 0 missed)");
    let after = sqlx::query(&format!(
        "SELECT state, (snooze_until IS NULL) AS until_cleared FROM {tbl} WHERE item_id='itm-1'"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(after.get::<String, _>("state"), "unread", "snoozed → unread (back in the active inbox)");
    assert!(
        after.get::<bool, _>("until_cleared"),
        "the snooze_until is cleared on re-surface"
    );

    // 0 DUPLICATE: a replayed re-surface (a restart over the already-re-surfaced row) flips ZERO rows
    // — the `state='snoozed'` guard makes the effectively-once property a REAL row-level invariant.
    let replay = sqlx::query(&resurface).execute(&mut *conn).await.expect("replayed re-surface").rows_affected();
    assert_eq!(replay, 0, "a replayed re-surface flips 0 rows (0 duplicate re-surface — the guard holds)");
    let still = sqlx::query(&format!("SELECT state FROM {tbl} WHERE item_id='itm-1'"))
        .fetch_one(&mut *conn).await.unwrap();
    assert_eq!(still.get::<String, _>("state"), "unread", "still exactly one re-surface");

    // ---- (3) the state CHECK constraint rejects an off-grammar state -----------------------------
    let bad = format!("UPDATE {tbl} SET state='paused' WHERE item_id='itm-1'");
    let err = sqlx::query(&bad).execute(&mut *conn).await;
    assert!(err.is_err(), "an off-grammar state ('paused') is rejected by the CHECK constraint");
    drop(conn);

    // ---- (4) RLS: a tenant B session reads 0 of tenant A's inbox rows ----------------------------
    let mut conn_b = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantB', false)").execute(&mut *conn_b).await.unwrap();
    sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)").execute(&mut *conn_b).await.unwrap();
    let cross: i64 = sqlx::query(&format!("SELECT count(*) AS n FROM {tbl}"))
        .fetch_one(&mut *conn_b)
        .await
        .unwrap()
        .get("n");
    assert_eq!(cross, 0, "RLS denies cross-tenant: tenant B reads 0 of tenant A's inbox rows");

    // cleanup
    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}")).execute(&admin).await.ok();
}

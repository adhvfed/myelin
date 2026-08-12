#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicU64, Ordering};

use myelin_config::MyelinConfig;
use myelin_storage::pg::{PgError, PgStore};
use myelin_storage::tenant_tx::{connect_pool_with_reset, with_tenant_tx};

fn admin_url(cfg: &MyelinConfig) -> String {
    cfg.database_url
        .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn uniq() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    )
}

async fn bare_single_conn(database_url: &str) -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(database_url)
        .await
        .expect("bare single-connection pool (is the stack up?)")
}

async fn residual_tenant_guc(pool: &sqlx::PgPool) -> String {
    let mut conn = pool.acquire().await.expect("reacquire pooled connection");
    let v: Option<String> = sqlx::query_scalar("SELECT current_setting('myelin.tenant_id', true)")
        .fetch_one(&mut *conn)
        .await
        .expect("read myelin.tenant_id GUC");
    v.unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mr013_pgstore_transaction_scoped_op_isolates_tenants() {
    let cfg = MyelinConfig::dev();
    let admin = match PgStore::connect(&admin_url(&cfg), &cfg.region, 4).await {
        Ok(s) => s,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    admin
        .migrate()
        .await
        .expect("migrate (rebac_tuple + RLS policy)");

    let tag = uniq();
    let tenant_a = format!("mr013-A-{tag}");
    let tenant_b = format!("mr013-B-{tag}");

    admin
        .put_tuple(&tenant_a, "A-secret-1", "reader", "user:alice")
        .await
        .expect("seed A1");
    admin
        .put_tuple(&tenant_a, "A-secret-2", "reader", "user:alice")
        .await
        .expect("seed A2");
    admin
        .put_tuple(&tenant_b, "B-secret-1", "reader", "user:mallory")
        .await
        .expect("seed B1");

    let app = PgStore::connect(&cfg.database_url, &cfg.region, 2)
        .await
        .expect("connect as the NOBYPASSRLS app role");

    let visible_as_a = {
        use sqlx::Row;
        let mut tx = app
            .scoped_conn(&tenant_a)
            .await
            .expect("tenant-A scoped transaction");
        let rows = sqlx::query("SELECT object_id FROM rebac_tuple ORDER BY object_id")
            .fetch_all(&mut *tx)
            .await
            .expect("predicate-less RLS read");
        rows.iter()
            .map(|r| r.get::<String, _>("object_id"))
            .collect::<Vec<String>>()
    };
    assert_eq!(
        visible_as_a,
        vec!["A-secret-1".to_string(), "A-secret-2".to_string()],
        "tenant A sees exactly its own rows via the transaction-scoped op (DB-enforced RLS)"
    );
    assert!(
        !visible_as_a.iter().any(|o| o.starts_with("B-")),
        "ZERO cross-tenant leak inside the transaction-scoped PgStore op"
    );

    let cross = app
        .reverse_index(&tenant_a, "user:mallory", "reader")
        .await
        .expect("reverse_index as A for B's subject");
    assert!(
        cross.is_empty(),
        "no cross-tenant query path: A's reverse_index for B's subject is empty, got {cross:?}"
    );

    println!(
        "[MR-013] PASS  A=transaction-scoped-RLS-isolation  tenantA_rows={}  cross_tenant_leak=0  \
         backend=real-PG (FORCE RLS, NOBYPASSRLS myelin_app)",
        visible_as_a.len()
    );

    for t in [&tenant_a, &tenant_b] {
        let _ = with_tenant_tx(
            &connect_pool_with_reset(&admin_url(&cfg), &cfg.region, 1)
                .await
                .expect("admin reset pool"),
            t,
            &cfg.region,
            {
                let t = t.clone();
                move |conn| {
                    Box::pin(async move {
                        sqlx::query("DELETE FROM rebac_tuple WHERE tenant_id = $1")
                            .bind(&t)
                            .execute(&mut *conn)
                            .await
                            .map_err(|e| PgError::Query(e.to_string()))?;
                        Ok(())
                    })
                }
            },
        )
        .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mr013_no_guc_bleed_on_reused_pooled_connection() {
    let cfg = MyelinConfig::dev();
    let region = cfg.region.clone();

    match PgStore::connect(&admin_url(&cfg), &region, 2).await {
        Ok(s) => s.migrate().await.expect("migrate"),
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    }

    let tag = uniq();
    let tenant = format!("mr013-bleed-{tag}");

    let pool = connect_pool_with_reset(&cfg.database_url, &region, 1)
        .await
        .expect("app-role reset-on-release single-conn pool");
    let tenant_q = tenant.clone();
    with_tenant_tx(&pool, &tenant, &region, move |conn| {
        Box::pin(async move {
            let _ = sqlx::query("SELECT object_id FROM rebac_tuple WHERE tenant_id = $1")
                .bind(&tenant_q)
                .fetch_all(&mut *conn)
                .await
                .map_err(|e| PgError::Query(e.to_string()))?;
            Ok(())
        })
    })
    .await
    .expect("tenant-scoped tx commits");
    assert_eq!(
        residual_tenant_guc(&pool).await,
        "",
        "no GUC bleed: a connection reused after a transaction-scoped PgStore op carries NO \
         residual myelin.tenant_id (reset-on-release + SET LOCAL discarded at COMMIT)"
    );

    let bleeder = format!("bleeder-{tag}");
    let bare_a = bare_single_conn(&cfg.database_url).await;
    {
        let mut conn = bare_a.acquire().await.expect("bare conn");
        sqlx::query("SELECT set_config('myelin.tenant_id', $1, false)")
            .bind(&bleeder)
            .execute(&mut *conn)
            .await
            .expect("set a session-scoped GUC (the old leak pattern)");
    }
    assert_eq!(
        residual_tenant_guc(&bare_a).await,
        bleeder,
        "non-vacuity: a session-scoped set_config(.., false) on a BARE pool BLEEDS across the \
         checkout (the test genuinely detects a bleed) - a test passing on the session-scoped path \
         would FAIL here"
    );

    let bare_b = bare_single_conn(&cfg.database_url).await;
    with_tenant_tx(&bare_b, &format!("probe-{tag}"), &region, |_conn| {
        Box::pin(async move { Ok(()) })
    })
    .await
    .expect("tenant-scoped tx commits on the bare pool");
    assert_eq!(
        residual_tenant_guc(&bare_b).await,
        "",
        "SET LOCAL alone (no reset-on-release) discards the tenant GUC at COMMIT - the \
         transaction-scoping mechanism PgStore delegates to does NOT bleed, by construction"
    );

    println!(
        "[MR-013] PASS  B=no-GUC-bleed  reset_pool_residual=''  bare_convention_residual=''  \
         bare_session_scoped_residual='{bleeder}' (the bleed the convention forecloses)  \
         backend=real-PG"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mr013_region_fail_fast_refuses_blank_region() {
    let cfg = MyelinConfig::dev();

    let empty = PgStore::connect(&cfg.database_url, "", 2).await;
    assert!(
        matches!(empty, Err(PgError::Connect(_))),
        "connect with an empty region must be refused loudly (region fail-fast)"
    );
    let blankish = PgStore::connect(&cfg.database_url, "   ", 2).await;
    assert!(
        matches!(blankish, Err(PgError::Connect(_))),
        "connect with a whitespace-only region must be refused (region fail-fast)"
    );

    let store = match PgStore::connect(&cfg.database_url, &cfg.region, 2).await {
        Ok(s) => s,
        Err(_) => {
            eprintln!("SKIP: dev Postgres unreachable (is the docker stack up?)");
            return;
        }
    };
    let scoped = store.scoped_conn_in_region("acme", "").await;
    assert!(
        matches!(scoped, Err(PgError::Query(_))),
        "scoped_conn_in_region with a blank region must be refused, got an Ok(_)"
    );
    let read = store
        .reverse_index_in_region("acme", "", "user:alice", "reader")
        .await;
    assert!(
        matches!(read, Err(PgError::Query(_))),
        "reverse_index_in_region with a blank region must be refused, got {read:?}"
    );

    println!(
        "[MR-013] PASS  C=region-fail-fast  connect('')=refused  connect('   ')=refused  \
         scoped_conn_in_region('')=refused  reverse_index_in_region('')=refused"
    );
}

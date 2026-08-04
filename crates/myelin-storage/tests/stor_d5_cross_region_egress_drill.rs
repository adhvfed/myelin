#![cfg(feature = "integration")]

use std::sync::atomic::{AtomicU64, Ordering};

use myelin_config::MyelinConfig;
use myelin_storage::pg::PgStore;

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stor_d5_cross_region_egress_impossible() {
    let cfg = MyelinConfig::dev();
    assert_eq!(
        cfg.region, "fr-par",
        "the dev/prod region is fr-par (MYELIN_REGION)"
    );

    let admin = PgStore::connect(&admin_url(&cfg), &cfg.region, 2)
        .await
        .expect("connect Postgres as admin (is the stack up?)");
    admin
        .migrate()
        .await
        .expect("migrate (rebac_tuple + (tenant,region) RLS WITH CHECK)");

    let store = PgStore::connect(&cfg.database_url, &cfg.region, 4)
        .await
        .expect("connect Postgres as the NOBYPASSRLS app role");

    let tag = uniq();
    let tenant = format!("tenant-d5-{tag}");

    store
        .put_tuple(&tenant, "doc-1", "reader", "user:alice")
        .await
        .expect("an in-region write (row.region == cell.region == fr-par) is admitted");

    let out_of_region = store
        .put_tuple_in_region(&tenant, "eu-central", "doc-evil", "reader", "user:mallory")
        .await;
    assert!(
        out_of_region.is_err(),
        "CP-D3: an out-of-region write (row.region=eu-central, cell.region=fr-par) MUST be REJECTED \
         by the DB's RLS WITH CHECK (the residency-pin write boundary) - got {out_of_region:?}"
    );
    let err_text = format!("{:?}", out_of_region.unwrap_err());
    assert!(
        err_text.to_lowercase().contains("row-level security")
            || err_text.to_lowercase().contains("policy")
            || err_text.contains("42501"),
        "the rejection is a DB RLS policy violation (not an app-side check): {err_text}"
    );

    let mut eu_conn = store
        .scoped_conn_in_region(&tenant, "eu-central")
        .await
        .expect("acquire a session scoped to eu-central");
    let eu_rows: i64 = {
        use sqlx::Row;
        let r = sqlx::query("SELECT count(*) AS n FROM rebac_tuple WHERE tenant_id = $1")
            .bind(&tenant)
            .fetch_one(&mut *eu_conn)
            .await
            .expect("count eu-central rows");
        r.get::<i64, _>("n")
    };
    assert_eq!(
        eu_rows, 0,
        "0 out-of-region writes admitted: no eu-central row for this tenant landed"
    );
    drop(eu_conn);

    let visible_from_eu = store
        .reverse_index_in_region(&tenant, "eu-central", "user:alice", "reader")
        .await
        .expect("read as an eu-central session");
    assert!(
        visible_from_eu.is_empty(),
        "STOR-D5: a session in eu-central sees ZERO of the tenant's fr-par rows - cross-region read \
         is impossible (0 cross-region PII egress); got {visible_from_eu:?}"
    );

    let visible_from_fr = store
        .reverse_index(&tenant, "user:alice", "reader")
        .await
        .expect("read as the in-region (fr-par) session");
    assert_eq!(
        visible_from_fr,
        vec!["doc-1".to_string()],
        "the in-region session sees the tenant's fr-par row (the read works inside the boundary)"
    );

    println!(
        "[2026-06-19] PASS  drill=STOR-D5/CP-D3-CROSS-REGION-EGRESS  cell_region=fr-par  \
         in_region_write=admitted  out_of_region_write=REJECTED-by-DB-RLS-WITH-CHECK  \
         out_of_region_writes_admitted=0  cross_region_read_rows=0 (0 PII egress)  \
         in_region_read_rows={}  backend=real-PG (FORCE RLS (tenant,region) WITH CHECK)",
        visible_from_fr.len()
    );

    let mut conn = store
        .scoped_conn(&tenant)
        .await
        .expect("acquire fr-par session for cleanup");
    sqlx::query("DELETE FROM rebac_tuple WHERE tenant_id = $1")
        .bind(&tenant)
        .execute(&mut *conn)
        .await
        .ok();
    conn.commit().await.ok();
}

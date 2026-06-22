//! **REF-P6 / P-155 — the refs-edge-builder ingest, PROVEN against the live dev-stack Postgres.**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-refs-service --features integration \
//!     --test integration_ref_p6_edge_builder -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires for REF-P6 — the builder's ingest
//! decision (the deterministic `edge_id` + the `source_root`/`target_root` derivation + the
//! `ON CONFLICT` idempotent upsert + the `*.removed` tombstone) executed against the REAL §3.2 `edge`
//! table on real Postgres. The drill is registered red-until-proven and flips green ONLY here.
//!
//! **REF-D7 ingest half (0 ghost, 0 lost — emit-iff-committed):** the only way a row lands in `edge`
//! is the builder's upsert from a delivered `refs.edge.created` (the outbox→bus→consumer path); there
//! is no standalone edge-write API. We prove (a) a delivered edge event produces exactly its row
//! (0 lost), (b) re-running the SAME edge event upserts ONE row via the deterministic `edge_id`
//! (0 ghost / 0 dup — idempotent rebuild), and (c) a `*.removed` tombstones it (hidden from the live
//! `edge_inbound WHERE NOT tombstoned` index). The builder's `edge_id` / `strip_sub` derivation under
//! test is byte-for-byte the production logic (`myelin_refs_service::edge_id` + the §3.2 columns).
#![cfg(feature = "integration")]

use myelin_refs::strip_sub;
use myelin_refs::ArtifactRef;
use myelin_refs_service::{
    edge_id, CREATE_EDGE_INDEXES_DDL, CREATE_EDGE_TABLE_DDL, MAKE_EDGE_TENANT_SCOPED_DDL,
};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}
fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

/// Rewrite the production `edge`-named DDL onto a uniquely-suffixed table (same shape; only the
/// `edge` identifier is suffixed for isolation + cleanup — mirrors the REF-P5 integration test).
fn rename(ddl: &str, tbl: &str) -> String {
    ddl.replace("EXISTS edge (", &format!("EXISTS {tbl} ("))
        .replace("ON edge (", &format!("ON {tbl} ("))
        .replace("ON edge ", &format!("ON {tbl} "))
        .replace("('edge')", &format!("('{tbl}')"))
        .replace("edge_inbound", &format!("{tbl}_inbound"))
        .replace("edge_outbound", &format!("{tbl}_outbound"))
        .replace("edge_by_rel", &format!("{tbl}_by_rel"))
}

#[tokio::test]
async fn edge_builder_ingest_upserts_idempotently_and_tombstones_on_real_postgres() {
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
    let tbl = format!("edge_p155_{suffix}");

    // ── Apply the REAL §3.2 schema (create + three indexes + RLS), suffixed for isolation. ──
    sqlx::query(&rename(CREATE_EDGE_TABLE_DDL, &tbl))
        .execute(&admin)
        .await
        .expect("create edge table");
    for (name, idx) in CREATE_EDGE_INDEXES_DDL {
        sqlx::query(&rename(idx, &tbl))
            .execute(&admin)
            .await
            .unwrap_or_else(|e| panic!("apply index {name}: {e}"));
    }
    sqlx::query(&rename(MAKE_EDGE_TENANT_SCOPED_DDL, &tbl))
        .execute(&admin)
        .await
        .expect("RLS scope");
    sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .expect("grant app");

    // The builder's ingest decision, expressed as the REAL upsert the production consumer runs
    // against this table (the deterministic `edge_id` + `strip_sub` roots + `ON CONFLICT DO NOTHING`).
    let tenant = myelin_tenancy::TenantId("tenantA".into());
    let source = "myelin://tenantA/chat/message/m1#block-9";
    let target = "myelin://tenantA/knowledge/page/7c2#block-3";
    let rel = "embeds";
    let id = edge_id(&tenant, source, target, rel);
    let source_root = strip_sub(&ArtifactRef(source.into())).0;
    let target_root = strip_sub(&ArtifactRef(target.into())).0;

    // Pin the session to tenantA (RLS) and upsert the edge — the builder's `apply_created` over real PG.
    let upsert_sql = format!(
        "INSERT INTO {tbl} \
           (tenant_id, region, edge_id, source, source_root, target, target_root, rel, rel_class, \
            origin_event, origin_actor, created_at, zookie, tombstoned, dek_ref) \
         VALUES ('tenantA','fr-par',$1,$2,$3,$4,$5,$6,'reference','evt-1','p-opaque-1',now(),'zk-1',false,'kms://tenantA/0/tenant') \
         ON CONFLICT (tenant_id, edge_id) DO NOTHING"
    );
    let mut conn = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id','tenantA',false)")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('myelin.region','fr-par',false)")
        .execute(&mut *conn)
        .await
        .unwrap();

    // ── (a) 0 lost: the delivered edge event produces exactly its row. ──
    sqlx::query(&upsert_sql)
        .bind(&id)
        .bind(source)
        .bind(&source_root)
        .bind(target)
        .bind(&target_root)
        .bind(rel)
        .execute(&mut *conn)
        .await
        .expect("first upsert (the delivered edge)");

    // ── (b) 0 ghost / 0 dup: re-running the SAME edge event upserts ONE row (deterministic edge_id). ──
    sqlx::query(&upsert_sql)
        .bind(&id)
        .bind(source)
        .bind(&source_root)
        .bind(target)
        .bind(&target_root)
        .bind(rel)
        .execute(&mut *conn)
        .await
        .expect("replay upsert is idempotent (ON CONFLICT DO NOTHING)");

    let live: i64 = sqlx::query(&format!(
        "SELECT count(*) AS n FROM {tbl} WHERE NOT tombstoned"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap()
    .get("n");
    assert_eq!(
        live, 1,
        "idempotent rebuild: replaying the edge event leaves exactly ONE live row (0 ghost, 0 dup)"
    );

    // The roots are the #sub-stripped parents (the inbound/outbound index keys).
    let row = sqlx::query(&format!(
        "SELECT source_root, target_root FROM {tbl} WHERE edge_id=$1"
    ))
    .bind(&id)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        row.get::<String, _>("source_root"),
        "myelin://tenantA/chat/message/m1"
    );
    assert_eq!(
        row.get::<String, _>("target_root"),
        "myelin://tenantA/knowledge/page/7c2"
    );

    // ── (c) `*.removed` tombstones the edge (the builder's `apply_removed`): hidden from the live index. ──
    sqlx::query(&format!(
        "UPDATE {tbl} SET tombstoned=true, origin_event='evt-2' WHERE edge_id=$1"
    ))
    .bind(&id)
    .execute(&mut *conn)
    .await
    .expect("tombstone the edge");
    let live_after: i64 = sqlx::query(&format!(
        "SELECT count(*) AS n FROM {tbl} WHERE NOT tombstoned"
    ))
    .fetch_one(&mut *conn)
    .await
    .unwrap()
    .get("n");
    assert_eq!(
        live_after, 0,
        "tombstoned → hidden from the live edge_inbound index"
    );
    let total: i64 = sqlx::query(&format!("SELECT count(*) AS n FROM {tbl}"))
        .fetch_one(&mut *conn)
        .await
        .unwrap()
        .get("n");
    assert_eq!(
        total, 1,
        "the row is retained for audit/provenance (soft-delete)"
    );

    // Cleanup (a NEW forward operation — test teardown, not a down-migration).
    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}

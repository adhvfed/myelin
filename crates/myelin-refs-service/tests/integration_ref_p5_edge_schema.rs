//! **REF-P5 / P-154 — the edge inverse-index schema, PROVEN against the live dev-stack Postgres.**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-refs-service --features integration \
//!     --test integration_ref_p5_edge_schema -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires: the §3.2 `edge` table migration
//! APPLIES forward-only against real Postgres, the THREE indexes exist with their exact WHERE
//! predicates, the `(tenant, region)` RLS policy ISOLATES tenants end-to-end (a session pinned to
//! tenant A sees ONLY tenant A's edge row), and the schema is forward-only (no DROP). The drill is
//! registered red-until-proven and flips green ONLY here, against the live stack — never mocked.
//!
//! The test instantiates the REAL [`myelin_refs_service`] DDL constants (the create + the three
//! index DDLs + the `myelin_make_tenant_scoped` RLS call) onto a uniquely-suffixed throwaway table
//! so concurrent runs don't collide; the DDL SHAPE under test is byte-for-byte the production
//! migration (only the table/index identifiers are suffixed for isolation + cleanup).
#![cfg(feature = "integration")]

use myelin_refs_service::{
    CREATE_EDGE_INDEXES_DDL, CREATE_EDGE_TABLE_DDL, EDGE_BY_REL_INDEX, EDGE_INBOUND_INDEX,
    EDGE_OUTBOUND_INDEX, MAKE_EDGE_TENANT_SCOPED_DDL,
};

/// The dev default mirrors the myelin-config dev DATABASE_URL; read inline so refs-service adds NO
/// crate edge (it stays the graph's terminal leaf consumer — no myelin-config dep).
fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

/// Rewrite the production `edge`-named DDL onto a uniquely-suffixed table so concurrent test runs
/// don't collide and the table is cleanable. The DDL SHAPE (columns, keys, predicates, RLS call) is
/// unchanged — only the `edge` identifier is suffixed.
fn rename(ddl: &str, tbl: &str) -> String {
    // Rename ONLY the `edge` TABLE identifier (not the `edge_id`/`edge_inbound` substrings) by
    // matching the anchored forms it appears in: the CREATE TABLE / CREATE INDEX `ON` target and the
    // RLS helper's quoted argument. The `edge_inbound`/`edge_outbound`/`edge_by_rel` INDEX names DO
    // embed `edge` and are renamed consistently via their own `edge_*` prefix (so the index id is
    // `<tbl>_inbound` etc. — matching what `CREATE INDEX edge_inbound ON edge` becomes).
    ddl.replace("EXISTS edge (", &format!("EXISTS {tbl} ("))
        .replace("ON edge (", &format!("ON {tbl} ("))
        .replace("ON edge ", &format!("ON {tbl} "))
        .replace("('edge')", &format!("('{tbl}')"))
        // The standalone index-name constants ("edge_inbound" / "edge_outbound" / "edge_by_rel")
        // and any remaining `edge_*` index id: rename the `edge_` prefix to `<tbl>_` so the asserted
        // names match the created ones. This does NOT touch `edge_id` because that only appears
        // inside the CREATE TABLE body, which is matched by the anchored forms above (the column
        // `edge_id` is left intact — there is no `edge_` prefix replacement applied to the table DDL).
        .replace("edge_inbound", &format!("{tbl}_inbound"))
        .replace("edge_outbound", &format!("{tbl}_outbound"))
        .replace("edge_by_rel", &format!("{tbl}_by_rel"))
}

#[tokio::test]
async fn edge_schema_applies_forward_only_with_rls_and_three_indexes() {
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

    // Unique per-process suffix so concurrent runs don't collide; the production shape is preserved.
    let suffix = std::process::id();
    let tbl = format!("edge_p154_{suffix}");

    // ── 1. Apply the REAL forward-only create-table DDL (the §3.2 shape), suffixed for isolation. ──
    // (`CREATE INDEX edge_inbound …` etc. embed the base name `edge`; rename them consistently.)
    let create = rename(CREATE_EDGE_TABLE_DDL, &tbl);
    sqlx::query(&create)
        .execute(&admin)
        .await
        .expect("apply the edge CREATE TABLE forward-only");

    // ── 2. Apply the THREE indexes (the §3.2 inbound/outbound/by_rel with their WHERE predicates). ──
    for (name, idx_ddl) in CREATE_EDGE_INDEXES_DDL {
        let idx = rename(idx_ddl, &tbl);
        sqlx::query(&idx)
            .execute(&admin)
            .await
            .unwrap_or_else(|e| panic!("apply index {name}: {e}"));
    }

    // ── 3. Make it RLS-ready via the platform-wide convention helper (FORCE RLS + the (tenant_id, ──
    //       region) isolation policy). Refs does NOT fork the policy — it calls the one helper.
    let rls = rename(MAKE_EDGE_TENANT_SCOPED_DDL, &tbl);
    sqlx::query(&rls)
        .execute(&admin)
        .await
        .expect("the edge table is made tenant-scoped (RLS)");
    // Grant the app role access (production grants ride ALTER DEFAULT PRIVILEGES; the test table is
    // created post-default-grant, so grant explicitly).
    sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .expect("grant the app role");

    // ── 4. PROVE the three indexes exist with their exact partial predicates (pg_indexes). ────────
    let idx_rows = sqlx::query(
        "SELECT indexname, indexdef FROM pg_indexes WHERE tablename = $1 ORDER BY indexname",
    )
    .bind(&tbl)
    .fetch_all(&admin)
    .await
    .expect("read pg_indexes for the edge table");
    let defs: std::collections::HashMap<String, String> = idx_rows
        .iter()
        .map(|r| {
            (
                r.get::<String, _>("indexname"),
                r.get::<String, _>("indexdef"),
            )
        })
        .collect();

    let inbound = rename(EDGE_INBOUND_INDEX, &tbl);
    let outbound = rename(EDGE_OUTBOUND_INDEX, &tbl);
    let by_rel = rename(EDGE_BY_REL_INDEX, &tbl);

    let inbound_def = defs
        .get(&inbound)
        .unwrap_or_else(|| panic!("edge_inbound index exists: {defs:?}"));
    assert!(
        inbound_def.contains("target_root"),
        "edge_inbound keys target_root: {inbound_def}"
    );
    assert!(
        inbound_def
            .to_lowercase()
            .contains("where (not tombstoned)")
            || inbound_def.to_lowercase().contains("where not tombstoned"),
        "edge_inbound is live-edges-only (WHERE NOT tombstoned): {inbound_def}"
    );

    let outbound_def = defs
        .get(&outbound)
        .unwrap_or_else(|| panic!("edge_outbound index exists: {defs:?}"));
    assert!(
        outbound_def.contains("source_root"),
        "edge_outbound keys source_root: {outbound_def}"
    );
    assert!(
        !outbound_def.to_lowercase().contains("where"),
        "edge_outbound has no partial predicate: {outbound_def}"
    );

    let by_rel_def = defs
        .get(&by_rel)
        .unwrap_or_else(|| panic!("edge_by_rel index exists: {defs:?}"));
    assert!(
        by_rel_def.contains("target_root") && by_rel_def.contains("rel"),
        "edge_by_rel keys (target_root, rel): {by_rel_def}"
    );
    assert!(
        by_rel_def
            .to_lowercase()
            .contains("rel_class = 'lifecycle'"),
        "edge_by_rel is lifecycle-class only: {by_rel_def}"
    );

    // ── 5. PROVE RLS isolates tenants end-to-end: seed two tenants' edges, then the app role pinned ─
    //       to tenant A sees ONLY tenant A's edge (the no-cross-tenant-query-path floor, live).
    for (tenant, edge_id) in [("tenantA", "e-aaa"), ("tenantB", "e-bbb")] {
        let mut conn = admin.acquire().await.unwrap();
        sqlx::query("SELECT set_config('myelin.tenant_id', $1, false)")
            .bind(tenant)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query(&format!(
            "INSERT INTO {tbl} \
               (tenant_id, region, edge_id, source, source_root, target, target_root, rel, \
                rel_class, origin_event, origin_actor, created_at, zookie, tombstoned, dek_ref) \
             VALUES ($1, 'fr-par', $2, 'myelin://t/chat/message/m1', 'myelin://t/chat/message/m1', \
                'myelin://t/issues/issue/ENG-1', 'myelin://t/issues/issue/ENG-1', 'mentions', \
                'reference', 'evt-1', 'principal-opaque-1', now(), 'zk-1', false, \
                'kms://t/0/tenant')"
        ))
        .bind(tenant)
        .bind(edge_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    // The app role pinned to tenantA sees ONLY tenantA's edge (RLS holds — no cross-tenant read).
    let mut conn = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantA', false)")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
        .execute(&mut *conn)
        .await
        .unwrap();
    let rows = sqlx::query(&format!("SELECT tenant_id, edge_id FROM {tbl}"))
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        rows.len(),
        1,
        "RLS must hide the other tenant's edge (no cross-tenant query path)"
    );
    assert_eq!(rows[0].get::<String, _>("tenant_id"), "tenantA");
    assert_eq!(rows[0].get::<String, _>("edge_id"), "e-aaa");

    // ── 6. PROVE the idempotency UNIQUE key holds: a second edge with the same (tenant, source, ────
    //       target, rel) but a different edge_id is REJECTED (the deterministic-edge_id backstop).
    {
        let mut conn = admin.acquire().await.unwrap();
        sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantA', false)")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
            .execute(&mut *conn)
            .await
            .unwrap();
        let dup = sqlx::query(&format!(
            "INSERT INTO {tbl} \
               (tenant_id, region, edge_id, source, source_root, target, target_root, rel, \
                rel_class, origin_event, origin_actor, created_at, zookie, tombstoned, dek_ref) \
             VALUES ('tenantA', 'fr-par', 'e-DIFFERENT', 'myelin://t/chat/message/m1', \
                'myelin://t/chat/message/m1', 'myelin://t/issues/issue/ENG-1', \
                'myelin://t/issues/issue/ENG-1', 'mentions', 'reference', 'evt-2', \
                'principal-opaque-1', now(), 'zk-2', false, 'kms://t/0/tenant')"
        ))
        .execute(&mut *conn)
        .await;
        assert!(
            dup.is_err(),
            "the UNIQUE (tenant_id, source, target, rel) key rejects a duplicate edge \
             (the idempotency backstop the deterministic edge_id rebuild relies on)"
        );
    }

    // ── 7. PROVE forward-only: the production DDL carries NO DROP (no down/rollback). ─────────────
    assert!(
        !CREATE_EDGE_TABLE_DDL.to_ascii_uppercase().contains("DROP"),
        "the edge schema migration is forward-only (no DROP in the create DDL)"
    );

    // Cleanup (a NEW forward operation, not a down-migration in the schema set — this is test teardown).
    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}

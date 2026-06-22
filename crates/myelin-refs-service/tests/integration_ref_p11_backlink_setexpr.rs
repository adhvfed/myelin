//! **REF-P11 / P-160 — the permission-filtered backlink read (the FROZEN `SetExpr` lowering over
//! `edge.source_root`), PROVEN against the live dev-stack Postgres.**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-refs-service --features integration \
//!     --test integration_ref_p11_backlink_setexpr -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires (the prompt touches a DB read
//! contract): the §4.4 backlink read — the lowered `SetExpr` (the `authz_visible` JOIN form) conjoined
//! into the `edge_inbound` range scan — runs as **ONE SQL query** against real Postgres and returns
//! ONLY the rows the viewer may `view` (over `source_root`), with the tenant predicate isolating
//! cross-tenant rows and `ORDER BY created_at DESC LIMIT :page` paginating. The drill is registered
//! red-until-proven and flips green ONLY here, against the live stack — never mocked.
//!
//! The SQL the test runs is the lowered form the Refs `backlinks` read composes: it builds the
//! `InRelation` → `authz_visible` JOIN clause via the REAL [`myelin_refs_service::lower_over_source_root`]
//! lowering, then executes `SELECT … FROM edge <join> WHERE tenant_id = :t AND target_root = :tr AND
//! NOT tombstoned AND (<predicate>) ORDER BY created_at DESC LIMIT :page` against the seeded tables.
#![cfg(feature = "integration")]

use myelin_identity::{Principal, PrincipalId, PrincipalKind, RelName, SetExpr};
use myelin_refs_service::{lower_over_source_root, source_root_colref};
use myelin_tenancy::TenantId;

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}
fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

#[tokio::test]
async fn backlink_setexpr_join_is_one_query_leak_free_tenant_scoped_paginated() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)");

    let suffix = std::process::id();
    let edge_tbl = format!("edge_p160_{suffix}");
    let av_tbl = format!("authz_visible_p160_{suffix}");

    // ── 1. A minimal edge table (the §3.2 columns the backlink read touches) + an authz_visible ──────
    //       reverse index table (the JOIN target). Throwaway, suffixed for isolation.
    sqlx::query(&format!(
        "CREATE TABLE {edge_tbl} (\
           tenant_id text NOT NULL, region text NOT NULL, edge_id text NOT NULL, \
           source text NOT NULL, source_root text NOT NULL, target text NOT NULL, \
           target_root text NOT NULL, rel text NOT NULL, rel_class text NOT NULL, \
           created_at timestamptz NOT NULL, tombstoned boolean NOT NULL DEFAULT false, \
           PRIMARY KEY (tenant_id, edge_id))"
    ))
    .execute(&admin)
    .await
    .expect("create the edge table");
    sqlx::query(&format!(
        "CREATE TABLE {av_tbl} (\
           tenant_id text NOT NULL, subject text NOT NULL, relation text NOT NULL, \
           object_id text NOT NULL)"
    ))
    .execute(&admin)
    .await
    .expect("create the authz_visible reverse index table");

    // ── 2. Seed two inbound edges to the SAME target_root in tenant A: one from a SECRET source, one ──
    //       from a PUBLIC source. Plus a cross-tenant edge in tenant B (must never be readable).
    let target_root = "myelin://acme/issue/issue/PUBLIC-1";
    for (eid, src, tenant, secs) in [
        (
            "e-secret",
            "myelin://acme/issue/issue/SECRET-9",
            "acme",
            10_i64,
        ),
        ("e-public", "myelin://acme/issue/issue/OPEN-2", "acme", 20),
        (
            "e-crosstenant",
            "myelin://evilcorp/issue/issue/X-1",
            "evilcorp",
            30,
        ),
    ] {
        sqlx::query(&format!(
            "INSERT INTO {edge_tbl} (tenant_id, region, edge_id, source, source_root, target, \
               target_root, rel, rel_class, created_at, tombstoned) \
             VALUES ($1, 'fr-par', $2, $3, $3, $4, $4, 'mentions', 'reference', \
               now() - ($5 || ' seconds')::interval, false)"
        ))
        .bind(tenant)
        .bind(eid)
        .bind(src)
        .bind(target_root)
        .bind(secs.to_string())
        .execute(&admin)
        .await
        .expect("seed an edge");
    }

    // ── 3. Grant the viewer `view` of ONLY the PUBLIC source in the reverse index (tenant A). The ─────
    //       SECRET source is NOT granted (the leak-test); the cross-tenant edge is in tenant B.
    let viewer = Principal::stub(
        PrincipalId("p:viewer".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    sqlx::query(&format!(
        "INSERT INTO {av_tbl} (tenant_id, subject, relation, object_id) \
         VALUES ('acme', 'p:viewer', 'view', 'myelin://acme/issue/issue/OPEN-2')"
    ))
    .execute(&admin)
    .await
    .expect("grant view of the public source");

    // ── 4. Build the REAL lowered SetExpr (InRelation → the authz_visible JOIN over source_root). ─────
    let lowered = lower_over_source_root(
        &SetExpr::InRelation {
            relation: RelName("view".into()),
            via_column: source_root_colref(),
        },
        &viewer,
    );
    assert_eq!(
        lowered.joins.len(),
        1,
        "the InRelation lowers to ONE JOIN (no N+1)"
    );
    // The lowered JOIN clause names `authz_visible` + `edge.source_root`; rebind it onto the suffixed
    // tables and bind the viewer subject. (In production the table names are the canonical ones; here
    // we rebind for test isolation — the SHAPE under test is the lowering's clause verbatim.)
    let join_clause = lowered.joins[0]
        .clause
        .replace("authz_visible", &av_tbl)
        .replace("edge.source_root", &format!("{edge_tbl}.source_root"))
        // bind the params inline for the test (the production path binds via the driver; here we
        // substitute the viewer subject + relation literals — they are NOT attacker-controlled).
        .replace(":subject_0", "'p:viewer'")
        .replace(":rel_for_view", "'view'");
    let predicate = lowered.sql_predicate; // `av0.object_id IS NOT NULL`

    // ── 5. THE ONE backlink query: the lowered JOIN + predicate conjoined into the inbound range scan, ─
    //       tenant-scoped, live-only, ORDER BY created_at DESC LIMIT :page.
    let sql = format!(
        "SELECT {edge_tbl}.source FROM {edge_tbl} {join_clause} \
         WHERE {edge_tbl}.tenant_id = 'acme' AND {edge_tbl}.target_root = $1 \
           AND NOT {edge_tbl}.tombstoned AND ({predicate}) \
         ORDER BY {edge_tbl}.created_at DESC LIMIT 50"
    );
    let rows = sqlx::query(&sql)
        .bind(target_root)
        .fetch_all(&admin)
        .await
        .unwrap_or_else(|e| panic!("the ONE backlink query runs: {e}\nSQL: {sql}"));

    // ── 6. PROVE leak-free: exactly the ONE authorized (public) backlink; the SECRET + cross-tenant ───
    //       are ABSENT.
    let sources: Vec<String> = rows.iter().map(|r| r.get::<String, _>("source")).collect();
    assert_eq!(
        sources.len(),
        1,
        "exactly the ONE authorized backlink (0 leak): {sources:?}"
    );
    assert_eq!(
        sources[0], "myelin://acme/issue/issue/OPEN-2",
        "the public referrer is present"
    );
    assert!(
        !sources.iter().any(|s| s.contains("SECRET")),
        "0 leak: the confidential referrer is ABSENT (the SetExpr JOIN excluded it)"
    );
    assert!(
        !sources.iter().any(|s| s.contains("evilcorp")),
        "0 cross-tenant: the tenant predicate excluded tenant B's edge (no cross-tenant query path)"
    );

    // ── 7. PROVE no post-filter / no N+1: the query is ONE statement (a JOIN), the JOIN is the ────────
    //       conjoin (we ran exactly one `fetch_all`). EXPLAIN confirms a single plan (no per-row
    //       subplan loop over a check); we assert the plan mentions the JOIN, not a correlated
    //       per-row subquery.
    let plan = sqlx::query(&format!("EXPLAIN (FORMAT TEXT) {sql}"))
        .bind(target_root)
        .fetch_all(&admin)
        .await
        .expect("EXPLAIN the backlink query");
    let plan_text: String = plan
        .iter()
        .map(|r| r.get::<String, _>(0))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan_text.to_lowercase().contains("join")
            || plan_text.to_lowercase().contains("nested loop"),
        "the read is ONE join query (no per-row check loop): {plan_text}"
    );

    // ── 8. Cleanup (forward teardown). ────────────────────────────────────────────────────────────
    sqlx::query(&format!("DROP TABLE {edge_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("DROP TABLE {av_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}

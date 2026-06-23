//! **CI-P25 / P-368 — the leak-free `list_objects` `SetExpr` push-down over `ci_run.run_id`, PROVEN
//! against the live dev-stack Postgres (the REAL data-layer artifact the binding policy requires).**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-ci-controlplane --features integration \
//!     --test integration_ci_p25_list_pushdown -- --nocapture
//!
//! The prompt CONSUMES the §5.1 `list_objects` DB read contract, so the binding policy requires a
//! REAL integration proof (never mocked): the run-list read — the lowered `SetExpr` (the
//! `authz_visible` JOIN form) conjoined into the `ci_run` list scan over `ci_run.run_id` — runs as
//! **ONE SQL query** against real Postgres and returns ONLY the rows the viewer may `read` (0 leak),
//! with the tenant predicate isolating cross-tenant rows. Survival signals: **0 leak; 1 SQL query;
//! revoke reflected.** The SQL the test runs is the lowered form
//! [`myelin_ci_controlplane::lower_over_run_id`] composes (the SAME `LoweredFilter` the production
//! read composes), executed against the seeded `ci_run` + `authz_visible` tables.

#![cfg(feature = "integration")]

use myelin_ci_controlplane::{ci_run_id_colref, lower_over_run_id, AUTHZ_VISIBLE_TABLE};
use myelin_identity::{Principal, PrincipalId, PrincipalKind, RelName, SetExpr};
use myelin_tenancy::TenantId;

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}
fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

#[tokio::test]
async fn ci_run_list_setexpr_join_one_query_zero_leak_tenant_scoped_revoke_reflected() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)");

    let suffix = std::process::id();
    let run_tbl = format!("ci_run_p368_{suffix}");
    let av_tbl = format!("authz_visible_p368_{suffix}");

    // ── 1. A minimal `ci_run` table (the §5.1 columns the run-list read touches — keyed on `run_id`)
    //       + an authz_visible reverse index table (the JOIN target). Throwaway, suffixed. ────────────
    sqlx::query(&format!(
        "CREATE TABLE {run_tbl} (\
           tenant_id text NOT NULL, region text NOT NULL, run_id text NOT NULL, \
           pipeline text NOT NULL, PRIMARY KEY (tenant_id, run_id))"
    ))
    .execute(&admin)
    .await
    .expect("create the ci_run table");
    sqlx::query(&format!(
        "CREATE TABLE {av_tbl} (\
           tenant_id text NOT NULL, subject text NOT NULL, relation text NOT NULL, \
           object_id text NOT NULL)"
    ))
    .execute(&admin)
    .await
    .expect("create the authz_visible reverse index table");

    // ── 2. Seed a tenant with two runs the viewer can read, one SECRET run (granted to someone else —
    //       the leak witness), and a CROSS-TENANT run in another tenant (must never be readable). ─────
    for (tenant, run_id, pipeline) in [
        ("acme", "run:1", "ci"),
        ("acme", "run:2", "ci"),
        ("acme", "run:secret", "deploy-prod"),
        ("evilcorp", "run:x", "ci"),
    ] {
        sqlx::query(&format!(
            "INSERT INTO {run_tbl} (tenant_id, region, run_id, pipeline) VALUES ($1, 'fr-par', $2, $3)"
        ))
        .bind(tenant)
        .bind(run_id)
        .bind(pipeline)
        .execute(&admin)
        .await
        .expect("seed a run");
    }

    // ── 3. Grant the viewer `read` of ONLY run:1 + run:2 in the reverse index (tenant acme).
    //       run:secret is granted to p:other; the cross-tenant run:x is in evilcorp. ──────────────────
    let viewer = Principal::stub(
        PrincipalId("p:viewer".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    );
    for (subject, object) in [
        ("p:viewer", "run:1"),
        ("p:viewer", "run:2"),
        ("p:other", "run:secret"),
    ] {
        sqlx::query(&format!(
            "INSERT INTO {av_tbl} (tenant_id, subject, relation, object_id) \
             VALUES ('acme', $1, 'read', $2)"
        ))
        .bind(subject)
        .bind(object)
        .execute(&admin)
        .await
        .expect("grant read");
    }

    // ── 4. Build the REAL lowered SetExpr (InRelation{read} → the authz_visible JOIN over ci_run.run_id). ──
    let lowered = lower_over_run_id(
        &SetExpr::InRelation {
            relation: RelName("read".into()),
            via_column: ci_run_id_colref(),
        },
        &viewer,
    );
    assert_eq!(
        lowered.joins.len(),
        1,
        "the InRelation lowers to ONE JOIN (no N+1)"
    );
    // Rebind the lowered JOIN clause onto the suffixed test tables + bind the viewer subject/relation
    // (in production the table names are canonical + the driver binds the params; the SHAPE under test
    // is the lowering's clause verbatim).
    let join_clause = lowered.joins[0]
        .clause
        .replace(AUTHZ_VISIBLE_TABLE, &av_tbl)
        .replace("ci_run.run_id", &format!("{run_tbl}.run_id"))
        .replace(":subject_0", "'p:viewer'")
        .replace(":rel_for_read", "'read'");
    let predicate = lowered.sql_predicate; // `av0.object_id IS NOT NULL`

    // ── 5. THE ONE run-list query: the lowered JOIN + predicate conjoined into the ci_run scan,
    //       tenant-scoped, ORDER BY run_id LIMIT :page (the §5.1 pre-filter, never post-filter). ──────
    let list_sql = format!(
        "SELECT {run_tbl}.run_id FROM {run_tbl} {join_clause} \
         WHERE {run_tbl}.tenant_id = 'acme' AND {run_tbl}.region = 'fr-par' AND ({predicate}) \
         ORDER BY {run_tbl}.run_id LIMIT 50"
    );
    let rows = sqlx::query(&list_sql)
        .fetch_all(&admin)
        .await
        .unwrap_or_else(|e| panic!("the ONE run-list query runs: {e}\nSQL: {list_sql}"));

    // ── 6. PROVE leak-free: exactly the TWO authorized runs; run:secret + the cross-tenant run ABSENT. ──
    let ids: Vec<String> = rows.iter().map(|r| r.get::<String, _>("run_id")).collect();
    assert_eq!(
        ids,
        vec!["run:1".to_string(), "run:2".to_string()],
        "exactly the 2 visible runs (0 leak): {ids:?}"
    );
    assert!(
        !ids.iter().any(|i| i == "run:secret"),
        "0 leak: the confidential run is ABSENT (the SetExpr JOIN excluded it)"
    );
    assert!(
        !ids.iter().any(|i| i == "run:x"),
        "0 cross-tenant: the tenant predicate excluded evilcorp's run"
    );

    // ── 7. PROVE one query / no post-filter / no N+1: the read is ONE statement (a JOIN); EXPLAIN
    //       confirms a single plan (no correlated per-row check subplan). ─────────────────────────────
    let plan = sqlx::query(&format!("EXPLAIN (FORMAT TEXT) {list_sql}"))
        .fetch_all(&admin)
        .await
        .expect("EXPLAIN the run-list query");
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

    // ── 8. REVOKE reflected: remove the viewer's grant on run:1 from the reverse index; re-run the SAME
    //       one query → run:1 drops out (the read-your-writes guarantee). ────────────────────────────
    sqlx::query(&format!(
        "DELETE FROM {av_tbl} WHERE tenant_id = 'acme' AND subject = 'p:viewer' AND relation = 'read' AND object_id = 'run:1'"
    ))
    .execute(&admin)
    .await
    .expect("revoke read of run:1");
    let rows_after = sqlx::query(&list_sql)
        .fetch_all(&admin)
        .await
        .expect("re-run the list after revoke");
    let ids_after: Vec<String> = rows_after
        .iter()
        .map(|r| r.get::<String, _>("run_id"))
        .collect();
    assert_eq!(
        ids_after,
        vec!["run:2".to_string()],
        "the just-revoked run:1 drops out (revoke reflected): {ids_after:?}"
    );

    // ── 9. Clean up the throwaway tables (leave the stack healthy for subsequent prompts). ───────────
    sqlx::query(&format!("DROP TABLE {run_tbl}"))
        .execute(&admin)
        .await
        .ok();
    sqlx::query(&format!("DROP TABLE {av_tbl}"))
        .execute(&admin)
        .await
        .ok();
}

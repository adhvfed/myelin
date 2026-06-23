//! **CI-P6 / P-349 — the complete CI Control-Plane data model, PROVEN against the live dev-stack
//! Postgres.**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-ci-controlplane --features integration \
//!     --test integration_ci_p6_controlplane_schema -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires (CI touches the OLTP contract,
//! 11.1): the arch 01 §3 control-plane schema APPLIES forward-only against real Postgres, the
//! scheduler claim index (`jq_claimable`) exists with its `WHERE state = 'queued'` predicate, the
//! `(tenant, region)` RLS policy ISOLATES tenants end-to-end (a session pinned to tenant A sees ONLY
//! tenant A's `ci_run` row), and the schema is forward-only (no DROP). The drill is registered
//! red-until-proven and flips green ONLY here, against the live stack — never mocked.
//!
//! The test applies the REAL DDL constants onto uniquely-suffixed throwaway tables so concurrent
//! runs don't collide; the DDL SHAPE under test is byte-for-byte the production migration (only the
//! table/index identifiers are suffixed for isolation + cleanup). Because the suffixed `ci_run` is a
//! fresh empty table, the `CREATE INDEX CONCURRENTLY` job-queue indexes are applied as separate
//! statements (CONCURRENTLY cannot run inside a transaction block).
#![cfg(feature = "integration")]

use myelin_ci_controlplane::{
    CREATE_CI_RUN_DDL, CREATE_JOB_QUEUE_DDL, CREATE_JOB_QUEUE_INDEXES_DDL, JQ_CLAIMABLE_INDEX,
};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

/// Rewrite a production table-named DDL onto a uniquely-suffixed table so concurrent runs don't
/// collide and the table is cleanable. The DDL SHAPE (columns, keys, predicates, RLS call) is
/// unchanged — only the identifier is suffixed.
fn rename(ddl: &str, base: &str, tbl: &str) -> String {
    ddl.replace(&format!("EXISTS {base} ("), &format!("EXISTS {tbl} ("))
        .replace(&format!("ON {base} ("), &format!("ON {tbl} ("))
        .replace(&format!("('{base}')"), &format!("('{tbl}')"))
}

#[tokio::test]
async fn ci_run_schema_applies_forward_only_with_rls() {
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
    let tbl = format!("ci_run_p349_{suffix}");

    // ── 1. Apply the REAL forward-only ci_run CREATE TABLE (the arch 01 §3.1 shape), suffixed. ──
    let create = rename(CREATE_CI_RUN_DDL, "ci_run", &tbl);
    sqlx::query(&create)
        .execute(&admin)
        .await
        .expect("apply the ci_run CREATE TABLE forward-only");

    // ── 2. Make it RLS-ready via the platform-wide convention helper (FORCE RLS + the (tenant_id, ──
    //       region) isolation policy). CI does NOT fork the policy — it calls the one helper.
    sqlx::query(&format!("SELECT myelin_make_tenant_scoped('{tbl}')"))
        .execute(&admin)
        .await
        .expect("the ci_run table is made tenant-scoped (RLS)");
    sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .expect("grant the app role");

    // ── 3. PROVE RLS isolates tenants end-to-end: seed two tenants' runs, then the app role pinned ─
    //       to tenant A sees ONLY tenant A's run (the no-cross-tenant-query-path floor, live).
    for (tenant, run_id) in [
        ("tenantA", "11111111-1111-1111-1111-111111111111"),
        ("tenantB", "22222222-2222-2222-2222-222222222222"),
    ] {
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
               (tenant_id, region, run_id, project_id, pipeline_id, wf_run_id, definition_snapshot, \
                trigger_kind, triggered_by, trust_tier, state, correlation_id) \
             VALUES ($1, 'fr-par', $2::uuid, gen_random_uuid(), gen_random_uuid(), gen_random_uuid(), \
                'blake3:snap', 'push', 'psn:actor-8a2f', 'trusted', 'queued', 'corr-1')"
        ))
        .bind(tenant)
        .bind(run_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    let mut conn = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', 'tenantA', false)")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
        .execute(&mut *conn)
        .await
        .unwrap();
    let rows = sqlx::query(&format!("SELECT tenant_id FROM {tbl}"))
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        rows.len(),
        1,
        "RLS must hide the other tenant's ci_run (no cross-tenant query path)"
    );
    assert_eq!(rows[0].get::<String, _>("tenant_id"), "tenantA");

    // ── 4. PROVE the frozen CHECK vocabulary is real: an out-of-vocabulary trust_tier is REJECTED. ─
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
        let bad = sqlx::query(&format!(
            "INSERT INTO {tbl} \
               (tenant_id, region, run_id, project_id, pipeline_id, wf_run_id, definition_snapshot, \
                trigger_kind, trust_tier, state, correlation_id) \
             VALUES ('tenantA', 'fr-par', gen_random_uuid(), gen_random_uuid(), gen_random_uuid(), \
                gen_random_uuid(), 'blake3:snap', 'push', 'WIDE_OPEN', 'queued', 'corr-2')"
        ))
        .execute(&mut *conn)
        .await;
        assert!(
            bad.is_err(),
            "the trust_tier CHECK rejects an out-of-vocabulary value ('WIDE_OPEN') — the frozen \
             three-tier vocabulary is enforced by Postgres"
        );
    }

    // ── 5. PROVE forward-only: the production DDL carries NO DROP. ──────────────────────────────
    assert!(
        !CREATE_CI_RUN_DDL.to_ascii_uppercase().contains("DROP"),
        "the ci_run schema migration is forward-only (no DROP)"
    );

    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}

#[tokio::test]
async fn job_queue_claim_index_applies_concurrently_with_its_predicate() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin");

    let suffix = std::process::id();
    let tbl = format!("job_queue_p349_{suffix}");

    // Apply the job_queue CREATE TABLE (the scheduler hot table, arch 01 §3.3), suffixed.
    let create = rename(CREATE_JOB_QUEUE_DDL, "job_queue", &tbl);
    sqlx::query(&create)
        .execute(&admin)
        .await
        .expect("apply the job_queue CREATE TABLE forward-only");

    // Apply the THREE claim indexes CONCURRENTLY (the hot-table expand discipline) — each as a
    // separate statement (CONCURRENTLY cannot run inside a transaction block).
    for (name, idx_ddl) in CREATE_JOB_QUEUE_INDEXES_DDL {
        let idx = idx_ddl.replace("ON job_queue (", &format!("ON {tbl} ("));
        // The index identifier embeds `jq_` — suffix it so concurrent runs don't collide.
        let idx = idx.replace("jq_", &format!("jq_{suffix}_"));
        sqlx::query(&idx)
            .execute(&admin)
            .await
            .unwrap_or_else(|e| panic!("apply index {name} CONCURRENTLY: {e}"));
    }

    // PROVE the claimable index exists with its `WHERE state = 'queued'` predicate (the claim
    // surface the CI-P12 FOR UPDATE SKIP LOCKED claim rides; region-led for the in-region claim).
    let claim_name = format!("jq_{suffix}_claimable");
    let _ = JQ_CLAIMABLE_INDEX; // the production index id this suffixed name maps from.
    let row =
        sqlx::query("SELECT indexdef FROM pg_indexes WHERE tablename = $1 AND indexname = $2")
            .bind(&tbl)
            .bind(&claim_name)
            .fetch_one(&admin)
            .await
            .expect("the jq_claimable index exists");
    let def: String = row.get("indexdef");
    assert!(
        def.contains("region") && def.contains("lane") && def.contains("enqueued_at"),
        "jq_claimable keys (region, lane, enqueued_at): {def}"
    );
    assert!(
        def.to_lowercase().contains("where (state = 'queued'")
            || def.to_lowercase().contains("where state = 'queued'"),
        "jq_claimable is queued-only (the claim surface): {def}"
    );

    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}

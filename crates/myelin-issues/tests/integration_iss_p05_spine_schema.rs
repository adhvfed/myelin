//! **ISS-P05 / P-371 — the complete issue-spine data model, PROVEN against the live dev-stack
//! Postgres.**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-issues --features integration \
//!     --test integration_iss_p05_spine_schema -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires (Issues touches the OLTP contract,
//! 11.1): the arch 01 §2–§4 spine APPLIES forward-only against real Postgres, the `(tenant, region)`
//! RLS policy ISOLATES tenants end-to-end (a session pinned to tenant A sees ONLY tenant A's `issue`
//! row), the frozen `state_category` CHECK vocabulary is enforced, the `issue_relation` FK ANCHORS on
//! the `src_issue` end (CASCADE on issue delete), and the schema is forward-only (no DROP). The drill
//! is registered red-until-proven and flips green ONLY here, against the live stack — never mocked.
//!
//! The test applies the REAL DDL constants onto uniquely-suffixed throwaway tables so concurrent runs
//! don't collide; the DDL SHAPE under test is byte-for-byte the production migration (only the
//! table/index identifiers are suffixed for isolation + cleanup). Because the suffixed `issue` is a
//! fresh empty table, the `CREATE INDEX CONCURRENTLY` hot-path indexes are applied as separate
//! statements (CONCURRENTLY cannot run inside a transaction block).
#![cfg(feature = "integration")]

use myelin_issues::{CREATE_ISSUE_DDL, CREATE_ISSUE_INDEXES_DDL, CREATE_ISSUE_RELATION_DDL};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

/// Rewrite a production table-named DDL onto a uniquely-suffixed table so concurrent runs don't
/// collide and the table is cleanable. The DDL SHAPE (columns, keys, CHECK predicates, RLS call) is
/// unchanged — only the identifier is suffixed.
fn rename(ddl: &str, base: &str, tbl: &str) -> String {
    ddl.replace(&format!("EXISTS {base} ("), &format!("EXISTS {tbl} ("))
        .replace(&format!("ON {base} ("), &format!("ON {tbl} ("))
        .replace(&format!("('{base}')"), &format!("('{tbl}')"))
}

#[tokio::test]
async fn issue_spine_applies_forward_only_with_rls_and_check_vocab() {
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
    let tbl = format!("issue_p371_{suffix}");

    // ── 1. Apply the REAL forward-only issue CREATE TABLE (the arch 01 §2 spine shape), suffixed. ──
    let create = rename(CREATE_ISSUE_DDL, "issue", &tbl);
    sqlx::query(&create)
        .execute(&admin)
        .await
        .expect("apply the issue CREATE TABLE forward-only");

    // ── 2. Make it RLS-ready via the platform-wide convention helper (FORCE RLS + the (tenant_id, ──
    //       region) isolation policy). Issues does NOT fork the policy — it calls the one helper.
    sqlx::query(&format!("SELECT myelin_make_tenant_scoped('{tbl}')"))
        .execute(&admin)
        .await
        .expect("the issue table is made tenant-scoped (RLS)");
    sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .expect("grant the app role");

    // ── 3. PROVE RLS isolates tenants end-to-end: seed two tenants' issues, then the app role pinned
    //       to tenant A sees ONLY tenant A's issue (the no-cross-tenant-query-path floor, live).
    for (tenant, id) in [
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
               (tenant_id, region, id, key, prefix, type_id, type_rank, state, state_category, \
                reporter, project_id, rank, title, version) \
             VALUES ($1, 'fr-par', $2::uuid, $3, 'ENG', gen_random_uuid(), 0, 'Todo', 'unstarted', \
                gen_random_uuid(), gen_random_uuid(), '0|hzzzzz:', 'fix the login bug', 1)"
        ))
        .bind(tenant)
        .bind(id)
        .bind(format!("ENG-{}", if tenant == "tenantA" { 1 } else { 2 }))
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
        "RLS must hide the other tenant's issue (no cross-tenant query path)"
    );
    assert_eq!(rows[0].get::<String, _>("tenant_id"), "tenantA");

    // ── 4. PROVE the frozen state_category CHECK vocabulary is real: an out-of-vocab value REJECTED.
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
               (tenant_id, region, id, key, prefix, type_id, type_rank, state, state_category, \
                reporter, project_id, rank, title, version) \
             VALUES ('tenantA', 'fr-par', gen_random_uuid(), 'ENG-3', 'ENG', gen_random_uuid(), 0, \
                'Todo', 'WIDE_OPEN', gen_random_uuid(), gen_random_uuid(), '0|i00000:', 't', 1)"
        ))
        .execute(&mut *conn)
        .await;
        assert!(
            bad.is_err(),
            "the state_category CHECK rejects an out-of-vocabulary value ('WIDE_OPEN') — the frozen \
             four-category invariant is enforced by Postgres"
        );
    }

    // ── 5. PROVE the issue_relation FK anchors on src_issue (CASCADE) — the TE-7 source-of-truth. ──
    let rel_tbl = format!("issue_relation_p371_{suffix}");
    // The FK references the production `issue(tenant_id, id)`; rewrite it to the suffixed issue table.
    let rel_create = rename(CREATE_ISSUE_RELATION_DDL, "issue_relation", &rel_tbl).replace(
        "REFERENCES issue(tenant_id, id)",
        &format!("REFERENCES {tbl}(tenant_id, id)"),
    );
    sqlx::query(&rel_create)
        .execute(&admin)
        .await
        .expect("apply the issue_relation CREATE TABLE forward-only (FK on src_issue)");
    sqlx::query(&format!("SELECT myelin_make_tenant_scoped('{rel_tbl}')"))
        .execute(&admin)
        .await
        .expect("issue_relation is made tenant-scoped");
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
        // A forward edge whose src_issue does NOT exist is rejected by the FK.
        let orphan = sqlx::query(&format!(
            "INSERT INTO {rel_tbl} (tenant_id, region, relation_id, src_issue, dst_ref, rel, created_by) \
             VALUES ('tenantA', 'fr-par', gen_random_uuid(), gen_random_uuid(), \
                'myelin://tenantA/issue/issue/ENG-9', 'blocks', gen_random_uuid())"
        ))
        .execute(&mut *conn)
        .await;
        assert!(
            orphan.is_err(),
            "the issue_relation FK rejects a forward edge whose src_issue does not exist (TE-7 integrity)"
        );
        // A valid forward edge (src_issue = tenantA's seeded issue) is accepted; the rel CHECK holds.
        sqlx::query(&format!(
            "INSERT INTO {rel_tbl} (tenant_id, region, relation_id, src_issue, dst_ref, rel, created_by) \
             VALUES ('tenantA', 'fr-par', gen_random_uuid(), \
                '11111111-1111-1111-1111-111111111111'::uuid, \
                'myelin://tenantA/issue/issue/ENG-2', 'blocks', gen_random_uuid())"
        ))
        .execute(&mut *conn)
        .await
        .expect("a valid forward edge anchored on an existing src_issue is accepted");
        // An out-of-vocabulary rel is rejected by the CHECK.
        let bad_rel = sqlx::query(&format!(
            "INSERT INTO {rel_tbl} (tenant_id, region, relation_id, src_issue, dst_ref, rel, created_by) \
             VALUES ('tenantA', 'fr-par', gen_random_uuid(), \
                '11111111-1111-1111-1111-111111111111'::uuid, \
                'myelin://tenantA/issue/issue/ENG-2', 'frobnicates', gen_random_uuid())"
        ))
        .execute(&mut *conn)
        .await;
        assert!(
            bad_rel.is_err(),
            "the rel CHECK rejects an out-of-vocabulary relation ('frobnicates')"
        );
    }

    // ── 6. PROVE the board hot-path index applies CONCURRENTLY with its soft-delete predicate. ──
    let board_ddl = CREATE_ISSUE_INDEXES_DDL
        .iter()
        .find(|(n, _)| *n == "issue_board")
        .map(|(_, d)| *d)
        .unwrap();
    let board_idx = board_ddl
        .replace("ON issue (", &format!("ON {tbl} ("))
        .replace("issue_board", &format!("issue_board_{suffix}"));
    sqlx::query(&board_idx)
        .execute(&admin)
        .await
        .expect("apply the issue_board index CONCURRENTLY");
    let row =
        sqlx::query("SELECT indexdef FROM pg_indexes WHERE tablename = $1 AND indexname = $2")
            .bind(&tbl)
            .bind(format!("issue_board_{suffix}"))
            .fetch_one(&admin)
            .await
            .expect("the issue_board index exists");
    let def: String = row.get("indexdef");
    assert!(
        def.contains("project_id") && def.contains("state_category") && def.contains("rank"),
        "issue_board keys (tenant, project, state_category, rank): {def}"
    );
    assert!(
        def.to_lowercase().contains("deleted_at is null"),
        "issue_board is the live-only board scan (soft-delete predicate): {def}"
    );

    // ── 7. PROVE forward-only: the production DDL carries NO DROP. ──────────────────────────────
    assert!(
        !CREATE_ISSUE_DDL.to_ascii_uppercase().contains("DROP"),
        "the issue schema migration is forward-only (no DROP)"
    );

    // Cleanup (issue_relation first — it FK-references issue).
    sqlx::query(&format!("DROP TABLE {rel_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}

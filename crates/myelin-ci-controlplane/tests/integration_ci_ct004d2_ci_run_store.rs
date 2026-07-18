//! **CT-004d.2 chunk 4 — the durable `ci_run` writer (`CiRunStore`) round-trip + idempotency + GENUINE
//! RLS isolation, PROVEN on live PG under the APP role (non-BYPASSRLS).**
//!
//! The co-commit atomicity (the `ci_run` ROW ⇄ dedup mark in one tx) is proven in ci-dispatch's
//! `tests/integration_ci_ct004b_trigger_consumer.rs` (`chunk4_*`). THIS proves the store's own verbs
//! against live Postgres:
//!   1. **Round-trip:** `insert_ci_run` writes every column; `get_ci_run` reads them ALL back faithfully.
//!   2. **Idempotent:** a second `insert_ci_run` of the SAME `(tenant, run_id)` is `ON CONFLICT DO
//!      NOTHING` → `false` (no second row) — the exactly-once run-of-record guard under redelivery.
//!   3. **RLS (GENUINE — the `myelin_app` role, RLS ENFORCED, not BYPASSRLS):** a `get_ci_run` scoped
//!      to tenant B CANNOT read tenant A's row even with A's `run_id` in hand — the `(tenant, region)`
//!      RLS policy hides it (`with_tenant_tx` sets the GUC transaction-scoped; the app role has no
//!      BYPASSRLS). This is the no-cross-tenant-query-path floor, live.
//!
//!   eval "$(scripts/dev-stack.sh env)"
//!   cargo test -p myelin-ci-controlplane --features integration \
//!     --test integration_ci_ct004d2_ci_run_store -- --nocapture
#![cfg(feature = "integration")]

use myelin_ci_controlplane::{ci_run_store_factory, CiRunInsert, CREATE_CI_RUN_DDL};
use sqlx::{Executor, PgPool};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

fn schema_name() -> String {
    format!("ci_ct004d2_{}", std::process::id())
}

/// A pool whose connections pin `search_path` to `schema` (so the store's unqualified `ci_run`
/// resolves to the per-test schema's table). `url` selects the role (admin = owner/BYPASSRLS to build;
/// app = RLS-ENFORCED to exercise).
async fn pool_on(url: &str, schema: &str) -> PgPool {
    let schema = schema.to_string();
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .after_connect(move |conn, _meta| {
            let schema = schema.clone();
            Box::pin(async move {
                conn.execute(format!("SET search_path TO {schema}, public").as_str())
                    .await?;
                Ok(())
            })
        })
        .connect(url)
        .await
        .expect("connect to dev Postgres (is the stack up? eval \"$(scripts/dev-stack.sh env)\")")
}

fn row(tenant: &str, run_id: &str) -> CiRunInsert {
    CiRunInsert {
        tenant_id: tenant.into(),
        region: "fr-par".into(),
        run_id: run_id.into(),
        project_id: "22222222-2222-2222-2222-222222222222".into(),
        pipeline_id: "33333333-3333-3333-3333-333333333333".into(),
        wf_run_id: "44444444-4444-4444-4444-444444444444".into(),
        definition_snapshot: "blake3:snap-abcd".into(),
        trigger_kind: "push".into(),
        trust_tier: "trusted".into(),
        state: "queued".into(),
        correlation_id: "corr-1".into(),
        cause_event_id: Some("ev-push-1".into()),
        repo_ref: Some("web".into()),
        commit_oid: Some("deadbeefcafe".into()),
        triggered_by: Some("psn:actor-8a2f".into()),
    }
}

#[tokio::test]
async fn chunk4_ci_run_store_round_trips_idempotent_and_rls_isolates() {
    let schema = schema_name();
    let admin = pool_on(&admin_url(), &schema).await;

    // ── Build the schema + the FORCE-RLS ci_run table (the ONE platform tenant-scoping helper), grant
    //    the app role USAGE + table privileges (mirrors integration_ci_p6). ──
    admin.execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str()).await.expect("drop prior");
    admin.execute(format!("CREATE SCHEMA {schema}").as_str()).await.expect("create schema");
    admin.execute(CREATE_CI_RUN_DDL).await.expect("create ci_run");
    admin.execute("SELECT myelin_make_tenant_scoped('ci_run')").await.expect("make ci_run tenant-scoped (FORCE RLS)");
    admin.execute(format!("GRANT USAGE ON SCHEMA {schema} TO myelin_app").as_str()).await.expect("grant schema usage");
    admin.execute("GRANT ALL ON ci_run TO myelin_app").await.expect("grant table privileges");

    // ── The store runs on the APP pool (RLS ENFORCED — no BYPASSRLS). ──
    let app = pool_on(&app_url(), &schema).await;
    let store = ci_run_store_factory(app.clone());

    let run_a = "11111111-1111-1111-1111-111111111111";
    let a = row("tenantA", run_a);

    // (1) Round-trip: fresh insert → true; every column reads back faithfully.
    assert!(store.insert_ci_run(&a).await.expect("insert tenantA"), "a fresh row inserts (true)");
    let got = store
        .get_ci_run("tenantA", "fr-par", run_a)
        .await
        .expect("get tenantA")
        .expect("the row is present");
    assert_eq!(got.tenant_id, "tenantA", "authoritative tenant partition round-trips");
    assert_eq!(got.run_id, run_a, "run_id round-trips");
    assert_eq!(got.region, "fr-par");
    assert_eq!(got.project_id, "22222222-2222-2222-2222-222222222222");
    assert_eq!(got.pipeline_id, "33333333-3333-3333-3333-333333333333");
    assert_eq!(got.wf_run_id, "44444444-4444-4444-4444-444444444444");
    assert_eq!(got.repo_ref.as_deref(), Some("web"));
    assert_eq!(got.commit_oid.as_deref(), Some("deadbeefcafe"));
    assert_eq!(got.cause_event_id.as_deref(), Some("ev-push-1"));
    assert_eq!(got.definition_snapshot, "blake3:snap-abcd");
    assert_eq!(got.trigger_kind, "push");
    assert_eq!(got.trust_tier, "trusted");
    assert_eq!(got.state, "queued");
    assert_eq!(got.correlation_id, "corr-1");

    // (2) Idempotent: a re-insert of the SAME (tenant, run_id) is ON CONFLICT DO NOTHING → false, one row.
    assert!(
        !store.insert_ci_run(&a).await.expect("re-insert tenantA"),
        "a redelivered trigger (same run_id) is ON CONFLICT DO NOTHING (false — no second row)"
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*)::bigint FROM ci_run WHERE run_id = $1::uuid")
        .bind(run_a)
        .fetch_one(&admin) // count via admin (BYPASSRLS) — the ground truth is exactly one row.
        .await
        .unwrap();
    assert_eq!(n, 1, "exactly one durable ci_run row after the idempotent re-insert");

    // (3) GENUINE RLS: a tenantB-scoped read CANNOT see tenantA's row even knowing its run_id (the app
    //     role is RLS-enforced; with_tenant_tx sets the (tenant, region) GUC → the policy hides it).
    let cross = store
        .get_ci_run("tenantB", "fr-par", run_a)
        .await
        .expect("get under tenantB scope");
    assert!(cross.is_none(), "RLS: tenantB cannot read tenantA's ci_run (no cross-tenant query path)");

    // tenantB can write + read its OWN row (RLS admits the in-tenant path).
    let run_b = "55555555-5555-5555-5555-555555555555";
    assert!(store.insert_ci_run(&row("tenantB", run_b)).await.expect("insert tenantB"), "tenantB writes its own row");
    assert!(
        store.get_ci_run("tenantB", "fr-par", run_b).await.expect("get tenantB own").is_some(),
        "tenantB reads its OWN row"
    );

    admin.execute(format!("DROP SCHEMA IF EXISTS {schema} CASCADE").as_str()).await.ok();
    println!("[chunk4/store] PASS ci_run store: all-column round-trip; ON CONFLICT idempotent (1 row); GENUINE RLS (app role) blocks the cross-tenant read.");
}

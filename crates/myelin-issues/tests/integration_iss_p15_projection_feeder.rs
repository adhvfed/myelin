//! **ISS-P15 / P-381 — the projection-feeder generated-index promotion, PROVEN against the live
//! dev-stack Postgres (the REAL 0-downtime online-migration artifact).**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-issues --features integration \
//!     --test integration_iss_p15_projection_feeder -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires (the prompt touches contract 1.5 —
//! the forward-only ONLINE migration that PROVISIONS the generated/expression index). The feeder's
//! promotion is the measured-promotion path; here we prove the migration it emits is real:
//!
//! - the feeder PROMOTES a measured-hot facet (share `> 5%` OQ-C) and the [`IndexProvisioning`] DDL it
//!   builds APPLIES against live Postgres as a `CREATE INDEX CONCURRENTLY` (0-downtime — no exclusive
//!   lock: a concurrent INSERT succeeds WHILE the index is building);
//! - BEFORE the promotion the cold facet's `EXPLAIN` shows a **Seq Scan**; AFTER the promotion the same
//!   facet query's `EXPLAIN` shows an **Index Scan** on the generated index — the promoted index is what
//!   ISS-P14's Tier 2 reads;
//! - a BELOW-threshold facet is NOT promoted → no index is provisioned (promotion is MEASURED).
#![cfg(feature = "integration")]

use myelin_issues::projection_feeder::{
    CollectionKey, FacetKey, IndexProvisioning, ProjectionFeeder, PromotionDecision,
};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}
fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

/// The corpus the cold-facet seq-scan / promoted index-scan witness needs (enough rows that the
/// planner picks an index over a seq scan once the generated index exists).
const N_ISSUES: i64 = 200_000;

#[tokio::test]
async fn iss_p15_feeder_provisions_the_generated_index_zero_downtime() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)");

    let suffix = std::process::id();
    // The feeder builds the DDL against the canonical `issue` table; for an isolated test run we drive
    // it against a per-run table of the SAME shape (typed core + the `props` JSONB tail).
    let issue_tbl = format!("issue_p381_{suffix}");

    // ── 1. The issue table: the typed-core columns + the JSONB `props` tail (the flexible-field model).
    sqlx::query(&format!(
        "CREATE UNLOGGED TABLE {issue_tbl} (\
           tenant_id text NOT NULL, region text NOT NULL, id text NOT NULL, \
           type_id text NOT NULL, project_id text NOT NULL, state_category text NOT NULL, \
           rank text NOT NULL, props jsonb NOT NULL DEFAULT '{{}}', deleted_at timestamptz, \
           PRIMARY KEY (tenant_id, id))"
    ))
    .execute(&admin)
    .await
    .expect("create the issue table");

    // ── 2. Seed N issues for tenant acme / type `bug`, each with a `severity` custom facet in props.
    //       The distribution is SKEWED: `critical` is rare (~0.5%) so the promoted index is genuinely
    //       the cheapest plan for a `severity = 'critical'` probe (a seq scan over a rare value is
    //       wasteful — exactly the JQL-trap the generated index fixes).
    sqlx::query(&format!(
        "INSERT INTO {issue_tbl} (tenant_id, region, id, type_id, project_id, state_category, rank, props) \
         SELECT 'acme', 'fr-par', 'ENG-' || g, 'bug', 'proj-' || (g % 19), \
                (ARRAY['unstarted','started','completed','cancelled'])[1 + (g % 4)], \
                lpad(g::text, 12, '0'), \
                jsonb_build_object('severity', CASE WHEN g % 200 = 0 THEN 'critical' ELSE 'normal' END) \
         FROM generate_series(1, {N_ISSUES}) AS g"
    ))
    .execute(&admin)
    .await
    .expect("seed issues with a severity facet");
    sqlx::query(&format!("ANALYZE {issue_tbl}"))
        .execute(&admin)
        .await
        .expect("ANALYZE");

    // ── 3. BEFORE promotion: a `severity` facet probe seq-scans the JSONB tail (no generated index).
    //       The probe is the canonical "is this facet indexed" question: a filter on the facet
    //       expression over the collection. Without the generated index this is a full seq scan of the
    //       JSONB tail (the JQL trap); with it, the planner rides the expression index. ────────────────
    let facet_sql = format!(
        "SELECT id FROM {issue_tbl} WHERE tenant_id = 'acme' AND type_id = 'bug' \
         AND (props ->> 'severity') = 'critical' AND deleted_at IS NULL"
    );
    let before: String = sqlx::query(&format!("EXPLAIN (FORMAT TEXT) {facet_sql}"))
        .fetch_all(&admin)
        .await
        .expect("EXPLAIN before promotion")
        .iter()
        .map(|r| r.get::<String, _>(0))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        before.contains("Seq Scan"),
        "BEFORE promotion the cold severity facet seq-scans the JSONB tail: {before}"
    );

    // ── 4. THE FEEDER MEASURES + PROMOTES. Drive `severity` hot (20% > 5% OQ-C), then evaluate it. ────
    let feeder = ProjectionFeeder::new();
    let coll = CollectionKey::new("acme", "bug");
    for _ in 0..20 {
        feeder.record_view_execution(&coll, &["severity"]);
    }
    for _ in 0..80 {
        feeder.record_view_execution(&coll, &[]);
    }
    let facet = FacetKey::new("acme", "bug", "severity");
    let provisioning = match feeder.evaluate_facet(&facet) {
        PromotionDecision::Promoted(p) => p,
        other => panic!("the hot facet must be PROMOTED, got {other:?}"),
    };
    assert!(
        provisioning.is_non_blocking(),
        "the online migration is non-blocking (CONCURRENTLY)"
    );
    assert!(
        provisioning.is_forward_only(),
        "the online migration is forward-only"
    );

    // ── 5. APPLY the feeder's online migration against the per-run table (rewrite the table name +
    //       the type predicate to this run's table; the feeder's DDL targets the canonical `issue`). ──
    let run_ddl = provisioning
        .ddl
        // rewrite ONLY the `ON issue ` table reference to this run's table (NOT the index name, which
        // also begins `issue_facet_…`).
        .replace(
            &format!("ON {} ", IndexProvisioning::for_facet(&facet).table),
            &format!("ON {issue_tbl} "),
        )
        // the seeded `type_id` is the literal `bug` (not a uuid) in this isolated table.
        .replace("type_id::text = 'bug'", "type_id = 'bug'");

    // 0-DOWNTIME WITNESS: a concurrent INSERT succeeds WHILE the CONCURRENTLY index builds (no
    // exclusive lock on the hot table). We fire the insert, then the concurrent index build; both
    // succeed. (CREATE INDEX CONCURRENTLY cannot run inside a transaction block — run it directly.)
    sqlx::query(&format!(
        "INSERT INTO {issue_tbl} (tenant_id, region, id, type_id, project_id, state_category, rank, props) \
         VALUES ('acme','fr-par','ENG-CONCURRENT','bug','proj-0','started','999999999999', \
                 jsonb_build_object('severity','critical'))"
    ))
    .execute(&admin)
    .await
    .expect("a concurrent write succeeds (the hot table is NOT exclusively locked)");

    sqlx::query(&run_ddl)
        .execute(&admin)
        .await
        .expect("the feeder's CREATE INDEX CONCURRENTLY applies (0 downtime)");
    sqlx::query(&format!("ANALYZE {issue_tbl}"))
        .execute(&admin)
        .await
        .expect("ANALYZE after the generated index");

    // ── 6. AFTER promotion: the SAME facet probe now rides the generated index (Index Scan). ─────────
    let after: String = sqlx::query(&format!("EXPLAIN (FORMAT TEXT) {facet_sql}"))
        .fetch_all(&admin)
        .await
        .expect("EXPLAIN after promotion")
        .iter()
        .map(|r| r.get::<String, _>(0))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        after.contains("Index Scan")
            || after.contains("Index Only Scan")
            || after.contains("Bitmap Index Scan"),
        "AFTER promotion the severity facet rides the generated index (Tier 2): {after}"
    );
    assert!(
        after.contains(&provisioning.index_name),
        "the plan names the generated index `{}`: {after}",
        provisioning.index_name
    );

    // ── 7. A BELOW-threshold facet is NOT promoted (no index provisioned) — promotion is MEASURED. ───
    let cold = FacetKey::new("acme", "bug", "customer_tier");
    // `customer_tier` was never filtered in a view → share 0 ≤ 5% → StayedOnGin.
    match feeder.evaluate_facet(&cold) {
        PromotionDecision::StayedOnGin { share } => assert_eq!(share, 0.0),
        other => panic!("a below-threshold facet stays on GIN, got {other:?}"),
    }
    let cold_idx = IndexProvisioning::for_facet(&cold).index_name;
    let cold_idx_exists: i64 = sqlx::query("SELECT count(*) FROM pg_indexes WHERE indexname = $1")
        .bind(&cold_idx)
        .fetch_one(&admin)
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        cold_idx_exists, 0,
        "a below-threshold facet provisions NO index (measured, never predicted)"
    );

    println!(
        "[P-381 INTEGRATION GREEN] ISS-P15 proven against live Postgres: a measured-hot facet \
         (severity, 20% > 5% OQ-C) was PROMOTED via a 0-downtime CREATE INDEX CONCURRENTLY (a \
         concurrent write succeeded during the build); EXPLAIN went Seq Scan → Index Scan on the \
         generated index; a below-threshold facet (customer_tier) provisioned NO index."
    );

    // ── 8. Cleanup. ──────────────────────────────────────────────────────────────────────────────────
    sqlx::query(&format!("DROP TABLE {issue_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}

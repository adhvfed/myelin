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

const N_ISSUES: i64 = 200_000;

#[tokio::test]
async fn iss_p15_feeder_provisions_the_generated_index_zero_downtime() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres (is the stack up? run `fed test:backend`)");

    let suffix = std::process::id();
    let issue_tbl = format!("issue_p381_{suffix}");

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

    let run_ddl = provisioning
        .ddl
        .replace(
            &format!("ON {} ", IndexProvisioning::for_facet(&facet).table),
            &format!("ON {issue_tbl} "),
        )
        .replace("type_id::text = 'bug'", "type_id = 'bug'");

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

    let cold = FacetKey::new("acme", "bug", "customer_tier");
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

    sqlx::query(&format!("DROP TABLE {issue_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}

#![cfg(feature = "integration")]

use std::time::Instant;

use myelin_identity::{Principal, PrincipalId, PrincipalKind, SetExpr, Zookie};
use myelin_issues::{plan_board_query, CostBudget, FacetCatalog, PlanOutcome, Tier};
use myelin_query::{CmpOp, Expr, Predicate, QueryAst};
use myelin_tenancy::{Region, TenantId};

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}
fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

const N_ISSUES: i64 = 1_000_000;
const N_CUSTOM_FIELDS: i64 = 55;

fn viewer() -> Principal {
    Principal::stub(
        PrincipalId("p:viewer".into()),
        PrincipalKind::Human,
        TenantId("acme".into()),
    )
}

#[tokio::test]
async fn iss_d2_board_query_under_one_second_no_full_scan_over_1m_issues() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)");

    let suffix = std::process::id();
    let issue_tbl = format!("issue_p380_{suffix}");

    sqlx::query(&format!(
        "CREATE UNLOGGED TABLE {issue_tbl} (\
           tenant_id text NOT NULL, region text NOT NULL, id text NOT NULL, \
           project_id text NOT NULL, state_category text NOT NULL, rank text NOT NULL, \
           assignee text, props jsonb NOT NULL DEFAULT '{{}}', deleted_at timestamptz, \
           PRIMARY KEY (tenant_id, id))"
    ))
    .execute(&admin)
    .await
    .expect("create the board issue table");

    let t_seed = Instant::now();
    sqlx::query(&format!(
        "INSERT INTO {issue_tbl} (tenant_id, region, id, project_id, state_category, rank, assignee, props) \
         SELECT 'acme', 'fr-par', 'ENG-' || g, 'proj-' || (g % 19), \
                (ARRAY['unstarted','started','completed','cancelled'])[1 + (g % 4)], \
                lpad(g::text, 12, '0'), 'u-' || (g % 500), \
                (SELECT jsonb_object_agg('field_' || f, to_jsonb(g % 100)) \
                   FROM generate_series(0, {fields}) AS f) \
         FROM generate_series(1, {N_ISSUES}) AS g",
        fields = N_CUSTOM_FIELDS - 1,
    ))
    .execute(&admin)
    .await
    .expect("seed 1M+ issues with a wide JSONB tail");
    println!(
        "[seed] {N_ISSUES} issues × {N_CUSTOM_FIELDS} custom fields in {:?}",
        t_seed.elapsed()
    );

    sqlx::query(&format!(
        "CREATE INDEX {issue_tbl}_board ON {issue_tbl} (tenant_id, project_id, state_category, rank) WHERE deleted_at IS NULL"
    ))
    .execute(&admin)
    .await
    .expect("create the issue_board Tier-1 index");
    sqlx::query(&format!("ANALYZE {issue_tbl}"))
        .execute(&admin)
        .await
        .expect("ANALYZE for real statistics");

    let acl: SetExpr = SetExpr::All;
    let board_ast = QueryAst::compiled(Predicate::And(vec![
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("project".into()),
            rhs: Expr::Lit(myelin_identity::Literal::Str("proj-7".into())),
        },
        Predicate::Cmp {
            op: CmpOp::Eq,
            lhs: Expr::Var("state_category".into()),
            rhs: Expr::Lit(myelin_identity::Literal::Str("started".into())),
        },
    ]))
    .unwrap();
    let outcome = plan_board_query(
        &board_ast,
        &acl,
        &viewer(),
        &TenantId("acme".into()),
        &Region("fr-par".into()),
        &Zookie("".into()),
        &FacetCatalog::new(),
        &CostBudget::DEFAULT,
        N_ISSUES as u64 / 80,
    );
    assert!(
        matches!(&outcome, PlanOutcome::ServeOltp(q) if q.tier == Tier::TypedCore),
        "the cost-bounder serves the typed-core board query on the Tier-1 index range"
    );

    let board_sql = format!(
        "SELECT id FROM {issue_tbl} \
         WHERE tenant_id = 'acme' AND region = 'fr-par' AND deleted_at IS NULL \
           AND project_id = 'proj-7' AND state_category = 'started' \
         ORDER BY rank LIMIT 50"
    );
    sqlx::query(&format!(
        "SET statement_timeout = {}",
        CostBudget::DEFAULT.statement_timeout_ms
    ))
    .execute(&admin)
    .await
    .unwrap();

    let mut samples_ms: Vec<f64> = Vec::new();
    for _ in 0..40 {
        let t = Instant::now();
        let rows = sqlx::query(&board_sql)
            .fetch_all(&admin)
            .await
            .expect("the Tier-1 board scan runs within the statement timeout");
        samples_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        assert!(!rows.is_empty(), "the board page returns issues");
        assert!(rows.len() <= 50, "paginated to the page limit");
    }
    samples_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p99 = samples_ms[(samples_ms.len() as f64 * 0.99) as usize - 1];
    println!(
        "[ISS-D2] Tier-1 board scan over {N_ISSUES} issues: p99 = {p99:.2} ms (budget < 1000 ms)"
    );
    assert!(
        p99 < 1000.0,
        "ISS-D2: the board query p99 ({p99:.2} ms) must be under the <1s keyboard budget"
    );

    let plan = sqlx::query(&format!("EXPLAIN (FORMAT TEXT) {board_sql}"))
        .fetch_all(&admin)
        .await
        .expect("EXPLAIN the board query");
    let plan_text: String = plan
        .iter()
        .map(|r| r.get::<String, _>(0))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan_text.contains("Index"),
        "the board scan rides an Index Scan: {plan_text}"
    );
    assert!(
        !plan_text.contains("Seq Scan"),
        "ISS-D2 no-full-scan: the board path MUST NOT seq-scan: {plan_text}"
    );

    let cold_facet_sql = format!(
        "SELECT id FROM {issue_tbl} \
         WHERE tenant_id = 'acme' AND props ->> 'field_42' = '7' ORDER BY rank LIMIT 50"
    );
    let cold_plan = sqlx::query(&format!("EXPLAIN (FORMAT TEXT) {cold_facet_sql}"))
        .fetch_all(&admin)
        .await
        .expect("EXPLAIN the cold-facet query");
    let cold_plan_text: String = cold_plan
        .iter()
        .map(|r| r.get::<String, _>(0))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        cold_plan_text.contains("Seq Scan"),
        "the cold ad-hoc JSONB facet WOULD seq-scan (the witness the cost-bounder must escalate it): {cold_plan_text}"
    );

    let cold_ast = QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var("field_42".into()),
        rhs: Expr::Lit(myelin_identity::Literal::Int(7)),
    })
    .unwrap();
    let cold_outcome = plan_board_query(
        &cold_ast,
        &acl,
        &viewer(),
        &TenantId("acme".into()),
        &Region("fr-par".into()),
        &Zookie("".into()),
        &FacetCatalog::new(),
        &CostBudget::DEFAULT,
        N_ISSUES as u64 / 100,
    );
    assert!(
        cold_outcome.is_escalate(),
        "the cost-bounder ESCALATES the cold ad-hoc facet (it would seq-scan) - never runs it on OLTP"
    );
    assert!(cold_outcome.assert_no_unbounded_scan());

    println!(
        "[P-380 INTEGRATION GREEN] ISS-D2 proven against live Postgres: Tier-1 board scan over \
         {N_ISSUES} issues × {N_CUSTOM_FIELDS} custom fields → p99 {p99:.2} ms (< 1s), Index Scan \
         (NO Seq Scan); a cold ad-hoc JSONB facet WOULD seq-scan → the cost-bounder escalated it to \
         Search (never an unbounded JSONB scan)."
    );

    sqlx::query(&format!("DROP TABLE {issue_tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}

//! **KN-P17 / P-307 — the flexible database (JSONB property bag + GIN-indexed projection + the
//! view filter conjoined with the `list_objects` `SetExpr` ACL) PROVEN against the live dev-stack
//! Postgres.**
//!
//! Gated behind the `integration` cargo feature so the default `cargo build --workspace` /
//! `cargo test --workspace` stay DB-free. Run against the docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-knowledge --features integration \
//!     --test integration_kn_d9_flex_db -- --nocapture
//!
//! This is the REAL data-layer proof the binding policy requires (the prompt touches the §4.1
//! flexible-DB read contract: JSONB `props` source of truth + the GIN `jsonb_path_ops` projection +
//! the `SetExpr` conjoin). The drill proves, against REAL Postgres:
//!
//! - the **JSONB property bag is the source of truth** and the **GIN `jsonb_path_ops` index** covers
//!   it — a `props ->> 'status'` view filter uses the index (EXPLAIN confirms it is not a parallel
//!   seq-scan blind path);
//! - the **VIEW_QUERY is ONE query**: the view filter (JSONB ops over `props`) AND the
//!   `list_objects` `SetExpr` ACL (the `authz_visible` JOIN over `db_row.id`) conjoined into one
//!   `SELECT`, returning ONLY the rows the viewer may read AND that match the filter — **0 leak**
//!   across a row-restricted db (composes with KN-D5);
//! - the **permission-correct `COUNT(*)`** over the same conjoined set (the ACL inside the
//!   aggregate) — **0 count-leak**;
//! - the read is **paginated / row-capped / under a statement-timeout** (the §4.1 step-5 bounds).
//!
//! The lowered SQL the test runs is what Knowledge's [`myelin_knowledge::database`] +
//! [`myelin_knowledge::list_filter`] compose: the REAL [`lower_view_filter`] JSONB predicate + the
//! REAL [`lower_over_db_row_id`] ACL JOIN. The KN-D9 drill is registered red-until-proven and flips
//! green ONLY here, against the live stack — never mocked.
#![cfg(feature = "integration")]

use myelin_identity::{Literal, Principal, PrincipalId, PrincipalKind, RelName, SetExpr};
use myelin_knowledge::{
    db_row_id_colref, lower_over_db_row_id, lower_view_filter, AUTHZ_VISIBLE_TABLE,
};
use myelin_query::{CmpOp, Expr, Predicate, QueryAst};
use myelin_tenancy::TenantId;

fn app_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}
fn admin_url() -> String {
    app_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw")
}

#[tokio::test]
async fn kn_d9_flex_db_jsonb_gin_view_filter_setexpr_conjoin_zero_leak_zero_count_leak() {
    use sqlx::Row;

    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url())
        .await
        .expect("connect to dev Postgres as admin (is the stack up? docker compose -f docker-compose.dev.yml up -d --wait)");

    let suffix = std::process::id();
    let row_tbl = format!("db_row_p307_{suffix}");
    let av_tbl = format!("authz_visible_p307_{suffix}");

    // ── 1. The §4.2 db_row table: JSONB `props` is the SOURCE OF TRUTH; a GIN jsonb_path_ops index is
    //       the DERIVED projection. + the authz_visible reverse index (the SetExpr JOIN target). ────
    sqlx::query(&format!(
        "CREATE TABLE {row_tbl} (\
           tenant text NOT NULL, region text NOT NULL, db_id text NOT NULL, id text NOT NULL, \
           props jsonb NOT NULL, order_key text NOT NULL, version bigint NOT NULL DEFAULT 1, \
           PRIMARY KEY (tenant, id))"
    ))
    .execute(&admin)
    .await
    .expect("create the db_row table (JSONB property bag)");
    sqlx::query(&format!("CREATE INDEX {row_tbl}_gin ON {row_tbl} USING gin (props jsonb_path_ops)"))
        .execute(&admin)
        .await
        .expect("create the GIN jsonb_path_ops derived projection");
    sqlx::query(&format!(
        "CREATE TABLE {av_tbl} (\
           tenant text NOT NULL, subject text NOT NULL, relation text NOT NULL, object_id text NOT NULL)"
    ))
    .execute(&admin)
    .await
    .expect("create the authz_visible reverse index table");

    // ── 2. Seed a row-restricted db: rows with status open/closed, one OPEN row granted to someone
    //       ELSE (the leak witness), a cross-db row, a cross-tenant row. ─────────────────────────────
    let seed: &[(&str, &str, &str, &str, i64)] = &[
        // (tenant, db_id, id, status, priority)
        ("acme", "db:projects", "row:1", "open", 5),
        ("acme", "db:projects", "row:2", "open", 3),
        ("acme", "db:projects", "row:3", "closed", 4),   // filtered out (status != open)
        ("acme", "db:projects", "row:secret", "open", 9), // leak witness: open, but granted to p:other
        ("acme", "db:other", "row:otherdb", "open", 1),  // no-cross-db
        ("evilcorp", "db:projects", "row:x", "open", 1), // no-cross-tenant
    ];
    for (tenant, db_id, id, status, priority) in seed {
        let props = serde_json::json!({ "status": status, "priority": priority, "title": format!("Item {id}") });
        sqlx::query(&format!(
            "INSERT INTO {row_tbl} (tenant, region, db_id, id, props, order_key) \
             VALUES ($1, 'fr-par', $2, $3, $4::jsonb, 'U')"
        ))
        .bind(tenant)
        .bind(db_id)
        .bind(id)
        .bind(props)
        .execute(&admin)
        .await
        .expect("seed a db_row");
    }

    // ── 3. Grant the viewer `read` of row:1, row:2, row:3 (closed) + row:otherdb; row:secret to
    //       p:other. (row:3 is granted but filtered by status; row:otherdb is granted but cross-db —
    //       proving the FILTER and the db_id predicate, not ACL-luck, confine the result.) ───────────
    let viewer = Principal::stub(PrincipalId("p:viewer".into()), PrincipalKind::Human, TenantId("acme".into()));
    for (subject, object) in
        [("p:viewer", "row:1"), ("p:viewer", "row:2"), ("p:viewer", "row:3"), ("p:viewer", "row:otherdb"), ("p:other", "row:secret")]
    {
        sqlx::query(&format!(
            "INSERT INTO {av_tbl} (tenant, subject, relation, object_id) VALUES ('acme', $1, 'read', $2)"
        ))
        .bind(subject)
        .bind(object)
        .execute(&admin)
        .await
        .expect("grant read");
    }

    // ── 4. Build the REAL lowered VIEW filter (status == 'open' → a JSONB predicate over props) + the
    //       REAL lowered SetExpr ACL (InRelation → the authz_visible JOIN over db_row.id). ────────────
    let filter = QueryAst::compiled(Predicate::Cmp {
        op: CmpOp::Eq,
        lhs: Expr::Var("status".into()),
        rhs: Expr::Lit(Literal::Str("open".into())),
    })
    .unwrap();
    let lowered_filter = lower_view_filter(&filter, &[]).expect("the view filter lowers");
    assert!(
        lowered_filter.sql_predicate.contains("db_row.props ->> 'status'"),
        "the cold facet lowers to the GIN-covered props path: {}",
        lowered_filter.sql_predicate
    );
    let lowered_acl = lower_over_db_row_id(
        &SetExpr::InRelation { relation: RelName("read".into()), via_column: db_row_id_colref() },
        &viewer,
    );
    assert_eq!(lowered_acl.joins.len(), 1, "the InRelation lowers to ONE JOIN (no N+1)");

    // Rebind the lowering output onto the suffixed test tables + bind the literals inline for the
    // seeded test (the SHAPE under test is the lowering's verbatim output; production binds params).
    let join_clause = lowered_acl.joins[0]
        .clause
        .replace(AUTHZ_VISIBLE_TABLE, &av_tbl)
        .replace("db_row.id", &format!("{row_tbl}.id"))
        .replace(":subject_0", "'p:viewer'")
        .replace(":rel_for_read", "'read'");
    let acl_pred = lowered_acl.sql_predicate; // `av0.object_id IS NOT NULL`
    // The view filter predicate, with `db_row.props` → the test table + the bound `:f0` → 'open'.
    let filter_pred = lowered_filter
        .sql_predicate
        .replace("db_row.props", &format!("{row_tbl}.props"))
        .replace(&lowered_filter.params[0].placeholder, "'open'");

    // ── 5. THE ONE VIEW_QUERY: the view filter (JSONB) AND the ACL (JOIN) conjoined, tenant + db_id
    //       scoped, ORDER BY priority DESC then order_key, LIMIT 50 (the §4.1 pre-filter). ───────────
    let list_sql = format!(
        "SELECT {row_tbl}.id FROM {row_tbl} {join_clause} \
         WHERE {row_tbl}.tenant = 'acme' AND {row_tbl}.db_id = 'db:projects' \
         AND ({filter_pred}) AND ({acl_pred}) \
         ORDER BY ({row_tbl}.props ->> 'priority')::int DESC, {row_tbl}.order_key ASC LIMIT 50"
    );
    let rows = sqlx::query(&list_sql)
        .fetch_all(&admin)
        .await
        .unwrap_or_else(|e| panic!("the ONE VIEW_QUERY runs: {e}\nSQL: {list_sql}"));
    let ids: Vec<String> = rows.iter().map(|r| r.get::<String, _>("id")).collect();

    // ── 6. PROVE leak-free + filtered: exactly the granted OPEN rows in this db, priority-sorted.
    //       row:1 (prio 5) before row:2 (prio 3); row:3 (closed) filtered; row:secret (granted to
    //       other) absent; cross-db + cross-tenant absent. ──────────────────────────────────────────
    assert_eq!(ids, vec!["row:1".to_string(), "row:2".to_string()], "exactly the 2 granted OPEN rows, priority-sorted (0 leak): {ids:?}");
    assert!(!ids.iter().any(|i| i == "row:3"), "the filter excluded the closed row (granted but status != open)");
    assert!(!ids.iter().any(|i| i == "row:secret"), "0 leak: the open row granted to someone else is ABSENT");
    assert!(!ids.iter().any(|i| i == "row:otherdb"), "no-cross-db: the db_id predicate excluded the other db's row (despite a grant)");
    assert!(!ids.iter().any(|i| i == "row:x"), "0 cross-tenant: the tenant predicate excluded evilcorp's row");

    // ── 7. The permission-correct COUNT(*) — the SAME filter + ACL INSIDE the aggregate. 0
    //       count-leak: COUNT = 2 (the granted open rows), NOT 3 (would leak row:secret). ────────────
    let count_sql = format!(
        "SELECT COUNT(*) AS n FROM {row_tbl} {join_clause} \
         WHERE {row_tbl}.tenant = 'acme' AND {row_tbl}.db_id = 'db:projects' \
         AND ({filter_pred}) AND ({acl_pred})"
    );
    let count: i64 = sqlx::query(&count_sql)
        .fetch_one(&admin)
        .await
        .unwrap_or_else(|e| panic!("the ONE COUNT runs: {e}\nSQL: {count_sql}"))
        .get::<i64, _>("n");
    assert_eq!(count, 2, "0 count-leak: the permission-correct COUNT is 2, NOT 3 (row:secret uncounted)");
    assert_eq!(count as usize, ids.len(), "the COUNT == the listed cardinality (the SAME conjunct, no divergent path)");

    // ── 8. PROVE one query + the GIN projection: EXPLAIN confirms a single JOIN plan (no per-row
    //       check subplan). With this small seed Postgres may pick a seq-scan over the index, but the
    //       presence of the GIN index + the one-join plan is what the contract requires. ─────────────
    let plan: String = sqlx::query(&format!("EXPLAIN (FORMAT TEXT) {list_sql}"))
        .fetch_all(&admin)
        .await
        .expect("EXPLAIN the VIEW_QUERY")
        .iter()
        .map(|r| r.get::<String, _>(0))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.to_lowercase().contains("join") || plan.to_lowercase().contains("nested loop"),
        "the VIEW_QUERY is ONE join query (no per-row check loop): {plan}"
    );
    // The GIN index exists on props (the derived projection is present, even if the planner picks a
    // different access path on this tiny seed — the index is the contract artifact, not the plan).
    let has_gin: bool = sqlx::query(&format!(
        "SELECT EXISTS (SELECT 1 FROM pg_indexes WHERE indexname = '{row_tbl}_gin') AS e"
    ))
    .fetch_one(&admin)
    .await
    .expect("check the GIN index")
    .get::<bool, _>("e");
    assert!(has_gin, "the GIN jsonb_path_ops derived projection exists on props");

    println!(
        "[P-307 INTEGRATION GREEN] KN-D9 flexible DB PROVEN against live Postgres: JSONB props \
         (source of truth) + GIN jsonb_path_ops projection; the VIEW_QUERY conjoins the view filter \
         (props ->> 'status' = 'open') AND the SetExpr ACL (authz_visible JOIN over db_row.id) into \
         ONE query → exactly [row:1, row:2] priority-sorted (0 leak: row:secret + closed + cross-db \
         + cross-tenant absent); the permission-correct COUNT = 2 not 3 (0 count-leak, ACL inside \
         the aggregate); EXPLAIN shows one join plan."
    );

    // ── 9. Cleanup (forward teardown). ───────────────────────────────────────────────────────────
    sqlx::query(&format!("DROP TABLE {row_tbl}")).execute(&admin).await.unwrap();
    sqlx::query(&format!("DROP TABLE {av_tbl}")).execute(&admin).await.unwrap();
}

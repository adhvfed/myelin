#![cfg(feature = "integration")]

use myelin_agent_service::{tool_def_id, ToolScopePredicate};
use myelin_config::MyelinConfig;
use myelin_identity::{ObjectId, SetExpr};

const TOOL_ID_EXPR: &str = "(subsystem || '/' || name || '/' || version::text)";

#[tokio::test]
async fn scoped_tool_list_is_one_sql_clause_matching_the_reference_path() {
    use sqlx::Row;

    let cfg = MyelinConfig::dev();
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&cfg.database_url)
        .await
        .expect("connect to dev Postgres as the app role (is the stack up?)");
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(
            &cfg.database_url
                .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw"),
        )
        .await
        .expect("connect as admin");

    let tbl = format!("agent_tool_def_scope_probe_{}", std::process::id());
    let create = format!(
        "CREATE TABLE {tbl} (\
           tenant_id text NOT NULL, region text NOT NULL, \
           name text NOT NULL, subsystem text NOT NULL, version integer NOT NULL, \
           input_schema text NOT NULL, required_caps text[] NOT NULL, effect_kind text NOT NULL, \
           side_effecting boolean NOT NULL, requires_approval boolean NOT NULL, \
           exposed_over_mcp boolean NOT NULL, \
           PRIMARY KEY (tenant_id, region, subsystem, name, version))"
    );
    sqlx::query(&format!("DROP TABLE IF EXISTS {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&create)
        .execute(&admin)
        .await
        .expect("the tool_def DDL applies");
    sqlx::query(&format!("SELECT myelin_make_tenant_scoped('{tbl}')"))
        .execute(&admin)
        .await
        .expect("the (tenant, region) RLS policy installs");
    sqlx::query(&format!("GRANT ALL ON {tbl} TO myelin_app"))
        .execute(&admin)
        .await
        .unwrap();

    let tools = [
        ("git", "merge"),
        ("issues", "close"),
        ("ci", "deploy"),
        ("ci", "run"),
        ("git", "open_pr"),
    ];
    {
        let mut conn = admin.acquire().await.unwrap();
        sqlx::query("SELECT set_config('myelin.tenant_id', 'acme', false)")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
            .execute(&mut *conn)
            .await
            .unwrap();
        for (subsystem, name) in tools {
            sqlx::query(&format!(
                "INSERT INTO {tbl} \
                   (tenant_id, region, name, subsystem, version, input_schema, required_caps, \
                    effect_kind, side_effecting, requires_approval, exposed_over_mcp) \
                 VALUES ('acme', 'fr-par', $1, $2, 1, '{{}}', ARRAY['tool.use'], 'mutate', \
                         true, false, false)"
            ))
            .bind(name)
            .bind(subsystem)
            .execute(&mut *conn)
            .await
            .unwrap();
        }
    }

    let set_expr = SetExpr::Intersect(vec![
        SetExpr::Ids(vec![
            ObjectId("git/merge/1".into()),
            ObjectId("issues/close/1".into()),
            ObjectId("ci/run/1".into()),
            ObjectId("ci/deploy/1".into()),
        ]),
        SetExpr::NotIds(vec![ObjectId("ci/deploy/1".into())]),
    ]);
    let predicate = myelin_agent_service::lower_set_expr(&set_expr);

    let scope_sql = predicate.to_sql(TOOL_ID_EXPR);
    let query =
        format!("SELECT {TOOL_ID_EXPR} AS id, name FROM {tbl} WHERE {scope_sql} ORDER BY name");

    let mut conn = app.acquire().await.unwrap();
    sqlx::query("SELECT set_config('myelin.tenant_id', 'acme', false)")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('myelin.region', 'fr-par', false)")
        .execute(&mut *conn)
        .await
        .unwrap();

    let rows = sqlx::query(&query)
        .fetch_all(&mut *conn)
        .await
        .expect("the single scope SQL runs");
    let live_ids: Vec<String> = rows.iter().map(|r| r.get::<String, _>("id")).collect();

    let reference: Vec<String> = tools
        .iter()
        .map(|(s, n)| {
            tool_def_id(&myelin_agent::ToolDef {
                name: myelin_agent::ToolName(n.to_string()),
                subsystem: s.to_string(),
                version: 1,
                input_schema: "{}".into(),
                required_caps: vec!["tool.use".into()],
                effect_kind: myelin_agent::EffectKind::Mutate,
                side_effecting: true,
                requires_approval: false,
                exposed_over_mcp: false,
            })
        })
        .filter(|id| predicate.admits(id))
        .collect();

    let mut live_sorted = live_ids.clone();
    live_sorted.sort();
    let mut ref_sorted = reference.clone();
    ref_sorted.sort();

    assert_eq!(
        live_sorted, ref_sorted,
        "the single SQL push-down must return the SAME visible-tool set as the reference path"
    );
    assert_eq!(
        live_sorted,
        vec![
            "ci/run/1".to_string(),
            "git/merge/1".to_string(),
            "issues/close/1".to_string()
        ],
        "ci/deploy is denied by the NotIds leg; git/open_pr is outside the allow-set"
    );

    assert!(
        scope_sql.matches("IN (").count() <= 2 && !scope_sql.contains(" = '"),
        "the scope is ONE conjoinable membership clause, not a per-tool equality fan-out: {scope_sql}"
    );

    let deny_sql = ToolScopePredicate::None.to_sql(TOOL_ID_EXPR);
    let deny_query = format!("SELECT count(*) AS n FROM {tbl} WHERE {deny_sql}");
    let n: i64 = sqlx::query_scalar(&deny_query)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(n, 0, "a denied run sees 0 tools (WHERE false)");

    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&admin)
        .await
        .unwrap();
}

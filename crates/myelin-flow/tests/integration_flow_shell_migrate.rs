#![cfg(feature = "integration")]

use myelin_config::MyelinConfig;
use myelin_events::OutboxStore;
use myelin_flow::{flow_app_spec, SERVICE_NAME};
use myelin_substrate::Config;

fn split_sql_statements(ddl: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut rest = ddl;
    let mut in_tag: Option<String> = None;
    while let Some(ch) = rest.chars().next() {
        if let Some(tag) = &in_tag {
            if rest.starts_with(tag.as_str()) {
                current.push_str(tag);
                rest = &rest[tag.len()..];
                in_tag = None;
                continue;
            }
            current.push(ch);
            rest = &rest[ch.len_utf8()..];
        } else if ch == '$' {
            if let Some(close) = rest[1..].find('$') {
                let body = &rest[1..1 + close];
                if body.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    let tag = format!("${body}$");
                    current.push_str(&tag);
                    rest = &rest[tag.len()..];
                    in_tag = Some(tag);
                    continue;
                }
            }
            current.push('$');
            rest = &rest[1..];
        } else if ch == ';' {
            out.push(std::mem::take(&mut current));
            rest = &rest[1..];
        } else {
            current.push(ch);
            rest = &rest[ch.len_utf8()..];
        }
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out
}

#[tokio::test]
async fn flow_shell_migration_set_applies_against_live_postgres() {
    let cfg = MyelinConfig::dev();
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(
            &cfg.database_url
                .replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw"),
        )
        .await
        .expect("connect as admin (is the dev stack up?)");

    let spec = flow_app_spec(Config::default(), OutboxStore::new());
    assert_eq!(spec.name, SERVICE_NAME);
    assert_eq!(
        spec.migrations.0.len(),
        12,
        "six table creates plus six online workflow control/drive/repair expands (incl. the concurrent-index validation)"
    );

    let schema = format!("flow_shell_probe_{}", std::process::id());
    let mut conn = admin.acquire().await.unwrap();
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query(&format!("SET search_path TO {schema}, public"))
        .execute(&mut *conn)
        .await
        .unwrap();

    for migration in &spec.migrations.0 {
        for stmt in split_sql_statements(migration.ddl.as_ref()) {
            let stmt = stmt.trim();
            if stmt.is_empty() {
                continue;
            }
            sqlx::query(stmt)
                .execute(&mut *conn)
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "migration `{}` statement failed live: {e}\nSQL: {stmt}",
                        migration.id
                    )
                });
        }
    }

    for table in myelin_flow::migrations::TABLES {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = $1 AND table_name = $2)",
        )
        .bind(&schema)
        .bind(table)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert!(
            exists,
            "the shell's migrate phase created `{table}` in Postgres"
        );
    }

    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&mut *conn)
        .await
        .unwrap();
}

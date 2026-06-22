//! Live Postgres ReBAC tuple-store integration test (Stage 1 / infra).
//!
//! Gated behind the `integration` cargo feature so the default build stays DB-free and identity
//! stays a DAG sink (no myelin-config edge — this reads DATABASE_URL directly). Run against the
//! docker-compose dev stack:
//!
//!   docker compose -f docker-compose.dev.yml up -d --wait
//!   cargo test -p myelin-identity --features integration --test integration_rebac -- --nocapture
//!
//! Proves the relation-tuple store ((object, relation, subject) rows + the S8 reverse-index
//! projection) round-trips against real Postgres: write two tuples, then resolve the
//! reverse-index "objects where subject has relation" lookup the authz path uses.
#![cfg(feature = "integration")]

/// The dev default mirrors the myelin-config dev DATABASE_URL; identity reads it inline so it
/// adds NO crate edge (preserving the sink invariant).
fn database_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin".into())
}

#[tokio::test]
async fn rebac_tuple_store_reverse_index() {
    use sqlx::Row;

    let admin = database_url().replace("myelin_app:myelin_app_pw", "myelin_admin:myelin_dev_pw");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin)
        .await
        .expect("connect to dev Postgres (is the stack up?)");

    let tbl = format!("rebac_tuple_{}", std::process::id());
    // The relation-tuple table shape (object_id, relation, subject) — the frozen
    // ⟨object#relation@subject⟩ tuple, with the (subject, relation, object_id) reverse index.
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {tbl} (\
            object_id text NOT NULL, \
            relation  text NOT NULL, \
            subject   text NOT NULL, \
            PRIMARY KEY (object_id, relation, subject))"
    ))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(&format!(
        "CREATE INDEX IF NOT EXISTS {tbl}_rev ON {tbl} (subject, relation, object_id)"
    ))
    .execute(&pool)
    .await
    .unwrap();

    // Two tuples: alice is reader on doc1 and doc2.
    for obj in ["doc1", "doc2"] {
        sqlx::query(&format!(
            "INSERT INTO {tbl} (object_id, relation, subject) VALUES ($1, 'reader', 'user:alice') \
             ON CONFLICT DO NOTHING"
        ))
        .bind(obj)
        .execute(&pool)
        .await
        .unwrap();
    }
    // A tuple that must NOT match the lookup (different subject).
    sqlx::query(&format!(
        "INSERT INTO {tbl} (object_id, relation, subject) VALUES ('doc3', 'reader', 'user:bob') \
         ON CONFLICT DO NOTHING"
    ))
    .execute(&pool)
    .await
    .unwrap();

    // The S8 reverse-index lookup: objects where alice is reader.
    let rows = sqlx::query(&format!(
        "SELECT object_id FROM {tbl} WHERE subject = $1 AND relation = $2 ORDER BY object_id"
    ))
    .bind("user:alice")
    .bind("reader")
    .fetch_all(&pool)
    .await
    .unwrap();

    let objs: Vec<String> = rows
        .iter()
        .map(|r| r.get::<String, _>("object_id"))
        .collect();
    assert_eq!(objs, vec!["doc1".to_string(), "doc2".to_string()]);

    sqlx::query(&format!("DROP TABLE {tbl}"))
        .execute(&pool)
        .await
        .unwrap();
}

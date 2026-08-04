#![cfg(feature = "integration")]

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
    sqlx::query(&format!(
        "INSERT INTO {tbl} (object_id, relation, subject) VALUES ('doc3', 'reader', 'user:bob') \
         ON CONFLICT DO NOTHING"
    ))
    .execute(&pool)
    .await
    .unwrap();

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

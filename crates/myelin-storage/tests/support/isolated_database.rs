pub struct IsolatedDatabase {
    schema: String,
    url: String,
    control: sqlx::PgPool,
}

impl IsolatedDatabase {
    pub async fn create(base_url: &str, prefix: &str) -> Self {
        assert!(
            !prefix.is_empty()
                && prefix
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'),
            "isolated schema prefixes must be non-empty SQL identifier fragments"
        );
        let schema = format!(
            "{prefix}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock must be after the Unix epoch")
                .as_nanos()
        );
        let control = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(base_url)
            .await
            .expect("connect isolated-database control pool");
        sqlx::query(&format!(
            "CREATE SCHEMA {schema} AUTHORIZATION myelin_admin"
        ))
        .execute(&control)
        .await
        .expect("create isolated test schema");
        let separator = if base_url.contains('?') { '&' } else { '?' };
        let url = format!("{base_url}{separator}options=-csearch_path%3D{schema}");
        Self {
            schema,
            url,
            control,
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub async fn drop_schema(&self) {
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.control)
            .await
            .expect("drop isolated test schema");
    }
}

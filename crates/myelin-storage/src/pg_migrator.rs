use crate::migration::{is_blocking_alter, is_destructive, HotTables, Migrations};
use crate::pg::PgError;
use sqlx::postgres::PgPool;
use sqlx::Executor;
use std::collections::BTreeMap;

pub const MIGRATION_LOCK_KEY: i64 = migration_lock_key();

const fn migration_lock_key() -> i64 {
    const BYTES: [u8; 8] = [0xf5, 0xd2, 0x6c, 0x96, 0x2c, 0x93, 0x58, 0x05];
    i64::from_be_bytes(BYTES)
}

const APPLIED_MIGRATION_DDL: &str = "\
CREATE TABLE IF NOT EXISTS myelin_applied_migration (
    id         text        PRIMARY KEY,
    applied_at timestamptz NOT NULL DEFAULT now(),
    checksum   text        NOT NULL
);";

pub struct PgMigrator;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationChecksumCollision {
    pub id: String,
    pub first_set: String,
    pub first_checksum: String,
    pub second_set: String,
    pub second_checksum: String,
}

pub fn migration_checksum_collisions<'a>(
    sets: impl IntoIterator<Item = (&'a str, &'a Migrations)>,
) -> Vec<MigrationChecksumCollision> {
    let mut first_by_id: BTreeMap<&str, (&str, String)> = BTreeMap::new();
    let mut collisions = Vec::new();
    for (set_name, migrations) in sets {
        for migration in &migrations.0 {
            let checksum = ddl_checksum(migration.ddl);
            match first_by_id.get(migration.id) {
                None => {
                    first_by_id.insert(migration.id, (set_name, checksum));
                }
                Some((_first_set, first_checksum)) if *first_checksum == checksum => {}
                Some((first_set, first_checksum)) => {
                    collisions.push(MigrationChecksumCollision {
                        id: migration.id.to_string(),
                        first_set: (*first_set).to_string(),
                        first_checksum: first_checksum.clone(),
                        second_set: set_name.to_string(),
                        second_checksum: checksum,
                    });
                }
            }
        }
    }
    collisions
}

impl PgMigrator {
    pub async fn apply(pool: &PgPool, migrations: &Migrations) -> Result<(), PgError> {
        let mut conn = pool
            .acquire()
            .await
            .map_err(|e| PgError::Migrate(format!("acquire migration connection: {e}")))?;

        let conn: &mut sqlx::PgConnection = &mut conn;
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(MIGRATION_LOCK_KEY)
            .execute(&mut *conn)
            .await
            .map_err(|e| PgError::Migrate(format!("acquire advisory migration lock: {e}")))?;

        let result = Self::apply_locked(&mut *conn, migrations).await;

        let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(MIGRATION_LOCK_KEY)
            .execute(&mut *conn)
            .await;

        result
    }

    pub async fn apply_validated(
        pool: &PgPool,
        migrations: &Migrations,
        hot_tables: &HotTables,
    ) -> Result<(), PgError> {
        for m in &migrations.0 {
            if is_destructive(m.ddl) {
                return Err(PgError::Migrate(format!(
                    "migration {} is destructive (DROP) - forward-only migrations only; a rollback \
                     is a NEW forward migration, never a down (§9.1)",
                    m.id
                )));
            }
            if let Some(table) = m.table {
                if hot_tables.is_hot(table) && is_blocking_alter(m.ddl) {
                    return Err(PgError::Migrate(format!(
                        "migration {} takes a blocking ALTER on the declared-HOT table `{}` - a \
                         hot-table change must be expand→backfill→contract, never one blocking \
                         ALTER that locks writes at QPS (§9.4)",
                        m.id, table
                    )));
                }
            }
        }
        Self::apply(pool, migrations).await
    }

    async fn apply_locked(
        conn: &mut sqlx::PgConnection,
        migrations: &Migrations,
    ) -> Result<(), PgError> {
        conn.execute(APPLIED_MIGRATION_DDL)
            .await
            .map_err(|e| PgError::Migrate(format!("create myelin_applied_migration: {e}")))?;

        for m in &migrations.0 {
            if is_destructive(m.ddl) {
                return Err(PgError::Migrate(format!(
                    "migration {} is destructive (DROP) - forward-only migrations only; a rollback \
                     is a NEW forward migration, never a down (storage §3.1)",
                    m.id
                )));
            }

            let expected_checksum = ddl_checksum(m.ddl);
            let recorded_checksum: Option<String> =
                sqlx::query_scalar("SELECT checksum FROM myelin_applied_migration WHERE id = $1")
                    .bind(m.id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(|e| {
                        PgError::Migrate(format!("check applied migration {}: {e}", m.id))
                    })?;
            if let Some(recorded_checksum) = recorded_checksum {
                verify_recorded_checksum(m.id, &recorded_checksum, &expected_checksum)?;
                continue;
            }

            conn.execute(m.ddl)
                .await
                .map_err(|e| PgError::Migrate(format!("apply migration {}: {e}", m.id)))?;

            sqlx::query(
                "INSERT INTO myelin_applied_migration (id, checksum) VALUES ($1, $2) \
                 ON CONFLICT (id) DO NOTHING",
            )
            .bind(m.id)
            .bind(&expected_checksum)
            .execute(&mut *conn)
            .await
            .map_err(|e| PgError::Migrate(format!("record migration {}: {e}", m.id)))?;
        }
        Ok(())
    }

    pub async fn is_applied(pool: &PgPool, id: &str) -> Result<bool, PgError> {
        let row: Option<i32> =
            sqlx::query_scalar("SELECT 1 FROM myelin_applied_migration WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await
                .map_err(|e| PgError::Migrate(format!("query applied migration {id}: {e}")))?;
        Ok(row.is_some())
    }

    pub async fn applied_count(pool: &PgPool, id: &str) -> Result<i64, PgError> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM myelin_applied_migration WHERE id = $1")
                .bind(id)
                .fetch_one(pool)
                .await
                .map_err(|e| PgError::Migrate(format!("count applied migration {id}: {e}")))?;
        Ok(count)
    }

    pub async fn audit_applied_checksums(
        pool: &PgPool,
        migrations: &Migrations,
    ) -> Result<(), PgError> {
        for migration in &migrations.0 {
            let recorded_checksum: Option<String> =
                sqlx::query_scalar("SELECT checksum FROM myelin_applied_migration WHERE id = $1")
                    .bind(migration.id)
                    .fetch_optional(pool)
                    .await
                    .map_err(|e| {
                        PgError::Migrate(format!(
                            "audit applied migration {} checksum: {e}",
                            migration.id
                        ))
                    })?;
            if let Some(recorded_checksum) = recorded_checksum {
                let expected_checksum = ddl_checksum(migration.ddl);
                verify_recorded_checksum(migration.id, &recorded_checksum, &expected_checksum)?;
            }
        }
        Ok(())
    }
}

pub async fn with_migration_lock(pool: &PgPool, ddl: &str) -> Result<(), PgError> {
    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| PgError::Migrate(format!("acquire migration connection: {e}")))?;
    let conn: &mut sqlx::PgConnection = &mut conn;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(MIGRATION_LOCK_KEY)
        .execute(&mut *conn)
        .await
        .map_err(|e| PgError::Migrate(format!("acquire advisory migration lock: {e}")))?;

    let result = conn
        .execute(ddl)
        .await
        .map(|_| ())
        .map_err(|e| PgError::Migrate(format!("apply DDL under migration lock: {e}")));

    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(MIGRATION_LOCK_KEY)
        .execute(&mut *conn)
        .await;

    result
}

pub fn ddl_checksum(ddl: &str) -> String {
    format!("blake3:{}", blake3::hash(ddl.as_bytes()).to_hex())
}

fn verify_recorded_checksum(
    id: &str,
    recorded_checksum: &str,
    expected_checksum: &str,
) -> Result<(), PgError> {
    if recorded_checksum == expected_checksum {
        return Ok(());
    }

    Err(PgError::Migrate(format!(
        "migration checksum mismatch for existing id `{id}`: recorded `{recorded_checksum}`, \
         current DDL is `{expected_checksum}`; applied migrations are immutable - restore the \
         original DDL and add a new forward migration id"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_lock_key_matches_digest() {
        let digest = blake3::hash(b"myelin.schema.migrate");
        let bytes = digest.as_bytes();
        let mut first8 = [0u8; 8];
        first8.copy_from_slice(&bytes[..8]);
        assert_eq!(
            MIGRATION_LOCK_KEY,
            i64::from_be_bytes(first8),
            "MIGRATION_LOCK_KEY must equal the first 8 bytes of blake3(\"myelin.schema.migrate\") \
             as a big-endian i64 - update the BYTES literal if blake3 changes"
        );
    }

    #[test]
    fn ddl_checksum_is_blake3_multihash() {
        let a = ddl_checksum("CREATE TABLE foo (id text)");
        let b = ddl_checksum("CREATE TABLE foo (id text)");
        let c = ddl_checksum("CREATE TABLE foo (id TEXT)");
        assert!(a.starts_with("blake3:"));
        assert_eq!(a, b, "same DDL → same checksum");
        assert_ne!(a, c, "different DDL → different checksum");
    }

    #[test]
    fn identical_recorded_checksum_admits_idempotent_skip() {
        let checksum = ddl_checksum("CREATE TABLE stable (id text PRIMARY KEY)");
        verify_recorded_checksum("0001_stable", &checksum, &checksum)
            .expect("same id and same DDL checksum must remain idempotent");
    }

    #[test]
    fn changed_ddl_under_existing_id_is_loudly_rejected() {
        let recorded = ddl_checksum("CREATE TABLE stable (id text PRIMARY KEY)");
        let current = ddl_checksum("CREATE TABLE stable (id text PRIMARY KEY, body text)");
        let error = verify_recorded_checksum("0001_stable", &recorded, &current)
            .expect_err("same id with different DDL must be rejected");
        let message = error.to_string();
        assert!(message.contains("checksum mismatch"));
        assert!(message.contains("0001_stable"));
        assert!(message.contains(&recorded));
        assert!(message.contains(&current));
        assert!(message.contains("new forward migration id"));
    }

    #[test]
    fn catalog_allows_exact_shared_entries_and_surfaces_incompatible_id_reuse() {
        let first = Migrations::of([crate::migration::Migration::plain(
            "0001_shared",
            "CREATE TABLE shared (id text PRIMARY KEY)",
        )]);
        let exact_subset = first.clone();
        let incompatible = Migrations::of([crate::migration::Migration::plain(
            "0001_shared",
            "CREATE TABLE shared (id text PRIMARY KEY, body text)",
        )]);

        assert!(migration_checksum_collisions(
            [("full", &first), ("exact_subset", &exact_subset),]
        )
        .is_empty());
        let collisions =
            migration_checksum_collisions([("full", &first), ("incompatible", &incompatible)]);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].id, "0001_shared");
        assert_eq!(collisions[0].first_set, "full");
        assert_eq!(collisions[0].second_set, "incompatible");
        assert_ne!(collisions[0].first_checksum, collisions[0].second_checksum);
    }
}

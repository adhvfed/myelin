use myelin_config::MyelinConfig;
use myelin_migration_audit::{borrowed_sets, production_migration_sets};
use myelin_storage::pg_migrator::{ddl_checksum, migration_checksum_collisions};
use myelin_storage::PgMigrator;
use sqlx::postgres::PgPoolOptions;
use std::collections::{BTreeMap, BTreeSet};

#[test]
fn production_catalog_has_no_incompatible_duplicate_migration_ids() {
    let sets = production_migration_sets();
    let collisions = migration_checksum_collisions(borrowed_sets(&sets));
    assert!(
        collisions.is_empty(),
        "production migration ids collide with different DDL checksums: {collisions:#?}"
    );

    let mut owners: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (set_name, migrations) in &sets {
        for migration in &migrations.0 {
            owners
                .entry(migration.id.as_ref())
                .or_default()
                .push(set_name);
        }
    }
    let exact_reuse: Vec<_> = owners
        .iter()
        .filter(|(_, set_names)| set_names.len() > 1)
        .collect();
    eprintln!(
        "production migration catalog: {} sets, {} unique ids, {} exact id/DDL reuses: {exact_reuse:?}",
        sets.len(),
        owners.len(),
        exact_reuse.len()
    );
}

#[tokio::test]
async fn self_tenant_applied_rows_match_every_authoritative_production_ddl() {
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&MyelinConfig::dev().database_migration_url)
        .await
        .expect("self_tenant dev PostgreSQL must be reachable for the checksum activation audit");
    let sets = production_migration_sets();
    let collisions = migration_checksum_collisions(borrowed_sets(&sets));
    assert!(
        collisions.is_empty(),
        "refuse self_tenant audit over an ambiguous migration catalog: {collisions:#?}"
    );

    for (set_name, migrations) in &sets {
        PgMigrator::audit_applied_checksums(&pool, migrations)
            .await
            .unwrap_or_else(|error| {
                panic!("self_tenant migration set `{set_name}` is incompatible: {error}")
            });
    }

    let applied: Vec<(String, String)> =
        sqlx::query_as("SELECT id, checksum FROM myelin_applied_migration ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("read self_tenant applied migration ledger");
    let mut expected = BTreeMap::new();
    for (_, migrations) in &sets {
        for migration in &migrations.0 {
            expected.insert(migration.id.as_ref(), ddl_checksum(migration.ddl.as_ref()));
        }
    }
    let historical_only: BTreeSet<_> = applied
        .iter()
        .filter(|(id, _)| !expected.contains_key(id.as_str()))
        .map(|(id, _)| id.as_str())
        .collect();
    eprintln!(
        "self_tenant checksum activation audit: {} authoritative ids, {} applied rows, {} historical/test-only rows ignored by the runtime guard: {historical_only:?}",
        expected.len(),
        applied.len(),
        historical_only.len()
    );
}

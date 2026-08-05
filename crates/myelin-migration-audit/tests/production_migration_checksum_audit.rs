use myelin_config::MyelinConfig;
use myelin_migration_audit::{borrowed_sets, production_migration_sets};
use myelin_storage::pg_migrator::{ddl_checksum, migration_checksum_collisions};
use myelin_storage::PgMigrator;
use sqlx::postgres::PgPoolOptions;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

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
            owners.entry(migration.id).or_default().push(set_name);
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
            expected.insert(migration.id, ddl_checksum(migration.ddl));
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

#[test]
fn catalog_covers_every_current_pgbootstrap_production_main() {
    let crates_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("audit crate lives directly beneath the workspace crates directory");
    let roots = [
        ("myelin-edge", "all_durable_migrations()"),
        ("myelin-identity-service", "identity_service_migrations()"),
        ("myelin-issues", "issues_migrations()"),
        ("myelin-flow", "flow_migrations()"),
        ("myelin-notif", "notif_migrations()"),
        ("myelin-search", "search_service_migrations()"),
        ("myelin-knowledge", "knowledge_service_migrations()"),
        (
            "myelin-ci-controlplane",
            "myelin_ci_controlplane::ci_controlplane_migrations()",
        ),
        (
            "myelin-ci-dispatch",
            "myelin_ci_dispatch::dispatch_migrations()",
        ),
    ];
    for (crate_name, service_set_token) in roots {
        let path = crates_dir.join(crate_name).join("src/main.rs");
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read production root {}: {error}", path.display()));
        assert!(
            source.contains("PgBootstrap"),
            "{} is no longer a PgBootstrap root",
            path.display()
        );
        assert!(
            source.contains(service_set_token),
            "{} no longer applies catalogued set token `{service_set_token}`",
            path.display()
        );
    }
}

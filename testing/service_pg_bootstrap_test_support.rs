pub fn assert_split_role_source(source: &str, service_migration: &str, serve_call: &str) {
    assert!(source.contains("PgBootstrap::from_env(Mode::RequireEnv)"));
    assert!(!source.contains("Mode::DevDefaults"));
    assert!(!source.contains("SubstrateProvider::"));
    assert!(!source.contains("provider.migrate_foundation()"));
    assert!(!source.contains("provider.migrate("));
    assert!(!source.contains("provider\n        .migrate("));

    let foundation = source
        .find("bootstrap.migrate_foundation()")
        .expect("foundation migration must run through bootstrap");
    let durable = source
        .find("all_durable_migrations()")
        .expect("durable aggregate must run through bootstrap");
    let owned = source
        .find(service_migration)
        .expect("service-owned migrations must run through bootstrap");
    let handoff = source
        .find("bootstrap.into_runtime()")
        .expect("bootstrap must be consumed by runtime handoff");
    let runtime_store = source
        .find("PgOutboxBacking::new")
        .expect("durable runtime outbox must remain wired");
    let serve = source
        .find(serve_call)
        .expect("service lifecycle must remain wired");

    assert!(foundation < durable);
    assert!(durable < owned);
    assert!(owned < handoff);
    assert!(handoff < runtime_store);
    assert!(runtime_store < serve);
}

pub fn assert_missing_migration_credential_fails_before_serve(
    binary: &str,
    service: &str,
    serve_marker: &str,
) {
    let output = std::process::Command::new(binary)
        .env("DATABASE_URL", "postgres://runtime.invalid/myelin")
        .env_remove("DATABASE_MIGRATION_URL")
        .env("S3_ENDPOINT", "http://storage.invalid")
        .env("S3_REGION", "fr-par")
        .env("S3_ACCESS_KEY", "test-access")
        .env("S3_SECRET_KEY", "test-secret")
        .env("S3_BUCKET", "test-bucket")
        .env("REDIS_URL", "redis://cache.invalid")
        .env("NATS_URL", "nats://bus.invalid")
        .env("MYELIN_REGION", "fr-par")
        .env_remove("MYELIN_OIDC_ISSUER")
        .env_remove("MYELIN_OIDC_AUDIENCE")
        .env_remove("MYELIN_OIDC_JWKS")
        .env_remove("MYELIN_OIDC_JWKS_FILE")
        .output()
        .unwrap_or_else(|error| panic!("{service} process must launch: {error}"));

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr must be UTF-8");
    assert!(stderr.contains("DATABASE_MIGRATION_URL"));
    assert!(!stderr.contains(serve_marker));
}

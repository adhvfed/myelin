#[test]
fn production_main_hands_privileged_bootstrap_off_before_runtime_store_construction() {
    let source = include_str!("../src/main.rs");

    assert!(source.contains("PgBootstrap::from_env(Mode::RequireEnv)"));
    assert!(!source.contains("Mode::DevDefaults"));
    assert!(!source.contains("SubstrateProvider::connect"));

    let foundation = source
        .find("bootstrap.migrate_foundation()")
        .expect("foundation migration must run through PgBootstrap");
    let durable = source
        .find("bootstrap\n        .migrate(&all_durable_migrations()")
        .expect("durable migration aggregate must run through PgBootstrap");
    let issues = source
        .find("bootstrap\n        .migrate(&issues_migrations()")
        .expect("Issues migrations must run through PgBootstrap");
    let handoff = source
        .find("bootstrap.into_runtime()")
        .expect("bootstrap must be consumed by the runtime handoff");
    let runtime_store = source
        .find("PgOutboxBacking::new")
        .expect("production runtime store must remain wired");

    assert!(foundation < durable);
    assert!(durable < issues);
    assert!(issues < handoff);
    assert!(handoff < runtime_store);
    assert!(source.contains("run_issues_until_shutdown(Config::default()"));
    assert!(source.contains("SignalKind::terminate()"));
}

#[test]
fn missing_migration_credential_exits_before_service_boot() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_issues"))
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
        .output()
        .expect("Issues process must launch");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr must be UTF-8");
    assert!(stderr.contains("DATABASE_MIGRATION_URL"));
    assert!(!stderr.contains("issues service failed"));
}

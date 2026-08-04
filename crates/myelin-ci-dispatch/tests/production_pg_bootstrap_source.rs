#[test]
fn production_main_hands_privileged_bootstrap_off_before_runtime_composition() {
    let source = include_str!("../src/main.rs");

    assert!(source.contains("PgBootstrap::from_env(Mode::RequireEnv)"));
    assert!(!source.contains("Mode::DevDefaults"));
    assert!(!source.contains("SubstrateProvider::connect"));

    let foundation = source
        .find("bootstrap.migrate_foundation()")
        .expect("foundation migration must run through PgBootstrap");
    let durable = source
        .find("bootstrap\n        .migrate(&all_durable_migrations()")
        .expect("durable aggregate must run through PgBootstrap");
    let shared_ci = source
        .find("&myelin_ci_controlplane::ci_durable_migrations()")
        .expect("shared CI writer migrations must run through PgBootstrap");
    let dispatch = source
        .find("&myelin_ci_dispatch::dispatch_migrations()")
        .expect("Dispatch declaration must run through PgBootstrap");
    let handoff = source
        .find("bootstrap.into_runtime()")
        .expect("bootstrap must be consumed by the runtime handoff");
    let shape_check = source
        .find("verify_ci_cost_event_shape(provider.db_pool())")
        .expect("runtime must verify the shared money-table shape");
    let first_store = source
        .find("PgOutboxBacking::new")
        .expect("durable runtime outbox must remain wired");
    let consumer = source
        .find("build_dispatch_consumers")
        .expect("production trigger consumer must remain wired");
    let service = source
        .find("run_dispatch_until_shutdown(")
        .expect("service lifecycle must remain wired");

    assert!(foundation < durable);
    assert!(durable < shared_ci);
    assert!(shared_ci < dispatch);
    assert!(dispatch < handoff);
    assert!(handoff < shape_check);
    assert!(shape_check < first_store);
    assert!(first_store < consumer);
    assert!(consumer < service);
}

#[test]
fn missing_migration_credential_exits_before_consumer_or_service_boot() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ci-dispatch"))
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
        .expect("CI Dispatch process must launch");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr must be UTF-8");
    assert!(stderr.contains("DATABASE_MIGRATION_URL"));
    assert!(!stderr.contains("reading CI config"));
    assert!(!stderr.contains("ci-dispatch service failed"));
}

//! Deployment guards for the CI Controlplane split-credential bootstrap sequence.

#[test]
fn production_main_hands_privileged_bootstrap_off_before_runtime_composition() {
    let source = include_str!("../src/main.rs");
    let library_source = include_str!("../src/lib.rs");
    let region_store_source = include_str!("../src/job_queue_region.rs");

    assert!(source.contains("MyelinConfig::from_env(Mode::RequireEnv)"));
    assert!(source.contains("CiSchedulerDbConfig::from_env(&platform_config)"));
    assert!(source.contains("PgBootstrap::connect(platform_config, DEFAULT_MAX_CONNECTIONS)"));
    assert!(!source.contains("Mode::DevDefaults"));
    assert!(!source.contains("SubstrateProvider::connect"));

    let foundation = source
        .find("bootstrap.migrate_foundation()")
        .expect("foundation migration must run through PgBootstrap");
    let durable = source
        .find("bootstrap\n        .migrate(&all_durable_migrations()")
        .expect("durable aggregate must run through PgBootstrap");
    let controlplane = source
        .find("&myelin_ci_controlplane::ci_controlplane_migrations()")
        .expect("complete Controlplane migrations must run through PgBootstrap");
    let hot_tables = source
        .find("&myelin_ci_controlplane::ci_controlplane_hot_tables()")
        .expect("complete Controlplane hot-table declaration must stay paired");
    let handoff = source
        .find("bootstrap.into_runtime()")
        .expect("bootstrap must be consumed by the runtime handoff");
    let scheduler_handoff = source
        .find("CiSchedulerDbProvider::connect")
        .expect("scheduler provider must validate its dedicated credential");
    let shape_check = source
        .find("verify_ci_cost_event_shape(provider.db_pool())")
        .expect("runtime must verify the CI money-table shape");
    let first_store = source
        .find("PgOutboxBacking::new")
        .expect("durable runtime outbox must remain wired");
    let reaper = source
        .find("JobQueueReaper::new")
        .expect("production reaper must remain wired");
    let starter_lane = source
        .find("ci_run_starter_factory(")
        .expect("the per-tenant ci_run starter lane must be composed at the root");
    let runner_gate = source
        .find("verify_startup_activation(runner_setting)")
        .expect("production runner activation must be refused explicitly");
    let bootstrap = source
        .find("PgBootstrap::connect(platform_config, DEFAULT_MAX_CONNECTIONS)")
        .expect("database bootstrap must remain wired");
    let service = source
        .find("run_controlplane(Config::default()")
        .expect("service lifecycle must remain wired");

    assert!(!source.contains("runner_hooks"));
    assert!(!source.contains("CiPipelineDriver"));
    assert!(!source.contains("unresolved_stage_spec_builder"));
    assert!(!source.contains("TenantId(\"ci-controlplane\""));
    assert!(!source.contains("synthetic tenant"));
    // The starter lane composes behind the SAME MYELIN_CI_RUNNER seam the runner uses, and stays DORMANT
    // while the refusal stands: it is gated on the runner-host request, not spawned unconditionally.
    assert!(source.contains("if runner_host_requested {"));
    assert!(source.contains("let runner_host_requested = matches!(&runner_setting"));
    // It routes an AUTHORITATIVE tenant, never a synthetic one — no starter is constructed for a fixed
    // service tenant at the root (the factory mints per discovered ci_run.tenant_id).
    assert!(!source.contains("PgCiPipelineStarter::new"));
    assert!(source.contains("InvalidRunnerSetting(String)"));
    assert!(source.contains("NonUnicodeRunnerSetting(OsString)"));
    assert!(!source.contains("std::env::var(\"MYELIN_CI_RUNNER\").ok()"));
    assert!(!source.contains("ci_job_queue_store(provider.db_pool().clone())"));
    assert!(source.contains("scheduler_provider.region_queue_store()"));
    assert!(!library_source.contains("pub fn ci_region_queue_store("));
    assert!(!region_store_source.contains("pub fn with_pg"));

    assert!(runner_gate < bootstrap);
    assert!(foundation < durable);
    assert!(durable < controlplane);
    assert!(controlplane < hot_tables);
    assert!(hot_tables < handoff);
    assert!(handoff < scheduler_handoff);
    assert!(scheduler_handoff < shape_check);
    assert!(shape_check < first_store);
    assert!(first_store < reaper);
    assert!(reaper < starter_lane);
    assert!(starter_lane < service);
}

#[test]
fn missing_migration_credential_exits_before_reaper_runner_or_service_boot() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ci-controlplane"))
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
        .env_remove("MYELIN_CI_RUNNER")
        .output()
        .expect("CI Controlplane process must launch");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr must be UTF-8");
    assert!(stderr.contains("DATABASE_MIGRATION_URL"));
    assert!(!stderr.contains("spawn the ci-pipeline-driver thread"));
    assert!(!stderr.contains("ci-controlplane service failed"));
}

#[test]
fn runner_activation_is_refused_before_any_database_attempt() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ci-controlplane"))
        .env_clear()
        .env("MYELIN_CI_RUNNER", "1")
        .output()
        .expect("CI Controlplane process must launch");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr must be UTF-8");
    assert!(stderr.contains("real durable CostLedger reserve/settle authority"));
    assert!(stderr.contains("live per-run-token verification"));
    assert!(stderr.contains("production runner activation is refused"));
    assert!(!stderr.contains("database bootstrap refused to start"));
    assert!(!stderr.contains("DATABASE_URL"));
    assert!(!stderr.contains("DATABASE_MIGRATION_URL"));
}

#[test]
fn invalid_runner_setting_is_refused_before_any_database_attempt() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ci-controlplane"))
        .env_clear()
        .env("MYELIN_CI_RUNNER", "true")
        .output()
        .expect("CI Controlplane process must launch");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr must be UTF-8");
    assert!(stderr.contains("invalid MYELIN_CI_RUNNER value \"true\""));
    assert!(stderr.contains("allowed values are `0`, `1`, or unset"));
    assert!(!stderr.contains("database bootstrap refused to start"));
    assert!(!stderr.contains("DATABASE_URL"));
    assert!(!stderr.contains("DATABASE_MIGRATION_URL"));
}

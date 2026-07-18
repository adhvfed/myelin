//! Deployment guards for the Edge binary and founder-dogfood split-credential handoff.

#[test]
fn production_main_destroys_the_privileged_pool_before_runtime_stores_and_bind() {
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
    let issues = source
        .find("&myelin_issues::issues_migrations()")
        .expect("Issues saga schema must run through PgBootstrap");
    let handoff = source
        .find("bootstrap.into_runtime()")
        .expect("bootstrap must be consumed by the runtime handoff");
    let first_runtime_store = source
        .find("DurableKmsBacking::new")
        .expect("durable KMS runtime store must remain wired");
    let bind = source
        .find("TcpListener::bind")
        .expect("serving listener must remain wired");

    assert!(foundation < durable);
    assert!(durable < issues);
    assert!(issues < handoff);
    assert!(handoff < first_runtime_store);
    assert!(handoff < bind);
}

#[test]
fn production_edge_owns_the_durable_issue_saga_worker_without_an_in_memory_fallback() {
    let main = include_str!("../src/main.rs");
    let adapter = include_str!("../src/issue_authz.rs");
    let dogfood = include_str!("../../../scripts/dogfood.sh");

    assert!(main.contains("StoreBackedIssueAuthorizer::new(check.clone())"));
    assert!(main.contains("myelin_issues::PgIssueStore::new("));
    assert!(main.contains("register_issues("));
    assert!(main.contains("spawn_issue_authorization_reconciler("));
    assert!(main.contains("tokio::signal::ctrl_c()"));
    assert!(main.contains("issue_reconciler.shutdown().await"));
    assert!(!main.contains("KmsEngine::new()"));
    assert!(!main.contains("StoreBackedCheck::new("));
    assert!(!main.contains("OutboxStore::new()"));

    let routes = include_str!("../src/issues_http.rs");
    assert!(routes.contains("/v1/issues"));
    assert!(routes.contains("self.api.store.list(ctx.principal, request)"));
    assert!(routes.contains("self.api.store.create(ctx.principal, proposal)"));
    assert!(routes.contains("self.api.store.view(ctx.principal, id)"));
    assert!(routes.contains("self.api.store.close(ctx.principal, id)"));
    assert!(routes.contains("IssuePermission::View"));
    assert!(routes.contains("IssuePermission::Close"));
    assert!(!routes.contains("VisibleIssues::All"));

    assert!(adapter.contains("VisibleIssues::effective_issue_view_filter()"));
    assert!(!adapter.contains("Ok(VisibleIssues::All)"));
    assert!(adapter.contains("MYELIN_ISSUES_RECONCILE_TENANTS"));
    assert!(dogfood.contains("export MYELIN_ISSUES_RECONCILE_TENANTS="));
}

#[test]
fn dogfood_exports_distinct_runtime_and_migration_credentials() {
    let dev_stack = include_str!("../../../scripts/dev-stack.sh");
    let dogfood = include_str!("../../../scripts/dogfood.sh");

    assert!(dev_stack.contains(
        "export DATABASE_URL=\"postgres://myelin_app:myelin_app_pw@localhost:5433/myelin\""
    ));
    assert!(dev_stack.contains(
        "export DATABASE_MIGRATION_URL=\"postgres://myelin_admin:myelin_dev_pw@localhost:5433/myelin\""
    ));
    assert!(!dogfood.contains("DATABASE_URL_ADMIN"));
    assert!(!dogfood.contains("db_app/myelin_app"));
    assert!(!dogfood.contains("export DATABASE_URL=\"${db_admin}\""));
}

#[test]
fn missing_migration_credential_exits_before_bind() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_edge"))
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
        .env("MYELIN_EDGE_ADDR", "127.0.0.1:0")
        .output()
        .expect("edge process must launch");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr must be UTF-8");
    assert!(stderr.contains("DATABASE_MIGRATION_URL"));
    assert!(!stderr.contains("edge: listening on"));
}

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
    let notif = source
        .find("&myelin_notif::migrations::migrations()")
        .expect("notification schema must run through PgBootstrap");
    let notif_index = source
        .find("verify_index_ready(\"notif_inbox_recipient_keyset\")")
        .expect("notification keyset index must be ready before serving");
    let issues_recent_index = source
        .find("verify_index_ready(myelin_issues::ISSUE_RECENT_LIST_INDEX)")
        .expect("Issues recent-list index must be ready before serving");
    let issues_prefix_index = source
        .find("verify_index_ready(myelin_issues::ISSUE_KEY_PREFIX_LIST_INDEX)")
        .expect("Issues key-prefix index must be ready before serving");
    let head_index = source
        .find("verify_index_ready(\"git_pr_head_repo_idx\")")
        .expect("Git PR provenance index must be ready before serving");
    let operation_index = source
        .find("verify_index_ready(\"git_pr_command_operation_scope_uidx\")")
        .expect("Git PR operation namespace index must be ready before serving");
    let ci_migrations = source
        .find("&myelin_ci_controlplane::ci_controlplane_migrations()")
        .expect("CI run-surface schema must run through PgBootstrap");
    let flow_migrations = source
        .find("&myelin_flow::migrations::migrations()")
        .expect("CI's Flow prerequisite must run through PgBootstrap");
    let ci_index = source
        .find("verify_index_ready_exact(myelin_ci_controlplane::CI_RUN_SURFACE_INDEX_READINESS)")
        .expect("CI run-list keyset index identity must be exact before serving");
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
    assert!(durable < flow_migrations);
    assert!(flow_migrations < ci_migrations);
    assert!(durable < ci_migrations);
    assert!(durable < issues);
    assert!(issues < notif);
    assert!(notif < notif_index);
    assert!(notif_index < issues_recent_index);
    assert!(issues < issues_recent_index);
    assert!(issues_recent_index < issues_prefix_index);
    assert!(issues_prefix_index < head_index);
    assert!(head_index < ci_index);
    assert!(ci_index < operation_index);
    assert!(ci_migrations < ci_index);
    assert!(operation_index < handoff);
    assert!(handoff < first_runtime_store);
    assert!(handoff < bind);
}

#[test]
fn production_edge_mounts_the_durable_recipient_scoped_notification_read() {
    let main = include_str!("../src/main.rs");
    let route = include_str!("../src/notif_http.rs");

    assert!(main.contains("register_notif("));
    assert!(main.contains("PgInboxStore::new(provider.db_pool().clone())"));
    assert!(main.contains("check.clone()"));
    assert!(route.contains("/v1/notif/inbox"));
    assert!(route.contains("tenant: ctx.principal.tenant.clone()"));
    assert!(route.contains("region: ctx.principal.region.clone()"));
    assert!(route.contains("recipient: ctx.principal.principal_id.0.clone()"));
    assert!(route.contains("can_read_subject"));
    assert!(!route.contains("query_param(\"tenant\")"));
    assert!(!route.contains("query_param(\"recipient\")"));
}

#[test]
fn production_edge_mounts_the_repo_authorized_durable_ci_reads() {
    let main = include_str!("../src/main.rs");
    let route = include_str!("../src/ci_http.rs");
    let authz = include_str!("../src/authz.rs");

    assert!(main.contains("register_ci("));
    assert!(main.contains("CiRunStore::with_pg_surface_cursor_key("));
    assert!(main.contains("seal_key.derive_service_key("));
    assert!(route.contains("/v1/ci/runs"));
    assert!(route.contains("/v1/ci/runs/{run}"));
    assert!(route.contains("visible_repo_slugs_for_ci(ctx.principal)"));
    assert!(route.contains("may_view_ci_repo(ctx.principal, repo_slug)"));
    assert!(route.contains("ctx.principal.tenant.as_str()"));
    assert!(route.contains("ctx.principal.region.as_str()"));
    assert!(!route.contains("query_param(\"tenant\")"));
    assert!(!route.contains("query_param(\"region\")"));
    assert!(authz.contains("requirement!(\"ci.runs.list\", \"run.view\", OP_AGENT_PAT)"));
    assert!(authz.contains("requirement!(\"ci.run.view\", \"run.view\", OP_AGENT_PAT)"));
}

#[test]
fn production_runtime_identity_and_git_storage_are_explicit_before_database_bootstrap() {
    let source = include_str!("../src/main.rs");
    let main = source
        .split_once("async fn main()")
        .expect("production entry point must remain wired")
        .1;
    let serving = main
        .split_once("None =>")
        .expect("no-argument serving path must remain wired")
        .1;

    let runtime_config = serving
        .find("runtime_config_or_exit(true)")
        .expect("serving must validate runtime identity and storage");
    let durable_core = serving
        .find("compose_core(runtime.cell_id).await")
        .expect("validated cell identity must feed durable composition");
    let git_root = serving
        .find("runtime.git_root")
        .expect("validated Git storage must feed serving");

    assert!(runtime_config < durable_core);
    assert!(runtime_config < git_root);
    assert!(source.contains("PgBootstrap::from_env(Mode::RequireEnv)"));
    assert!(!source.contains("\"cell-dev\".to_string()"));
    assert!(!source.contains(".join(\"myelin-git-data\")"));
    assert!(source.contains("MYELIN_GVISOR_GIT_ROOTFS"));
    assert!(source.contains("MYELIN_RUNSC_BIN"));
    assert!(source.contains("MemoryCgroup::create"));
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
    assert!(main.contains("let result = shutdown_signal().await;"));
    assert!(main.contains("git_shutdown_for_signal.store(true, Ordering::Release)"));
    assert!(main.contains("EDGE_SHUTDOWN_GRACE"));
    assert!(main.contains("ShutdownOutcome::Forced"));
    assert!(main.contains("serve_edge_until_shutdown_with_probe("));
    assert!(main.contains("provider.database_is_ready()"));
    assert!(main.contains("git_root_is_writable"));
    assert!(main.contains("EDGE_READINESS_CACHE_TTL"));
    assert!(main.contains("SignalKind::terminate()"));
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
fn production_git_wire_binds_the_verified_principal_to_the_live_identity_minter() {
    let main = include_str!("../src/main.rs");
    let durable = include_str!("../src/git_durable.rs");
    let http = include_str!("../src/git_wire_http.rs");
    let compact_main: String = main
        .split_whitespace()
        .collect::<String>()
        .replace(",)", ")");

    assert!(compact_main.contains("IdentityGitWireCredentialIssuerFactory::new(check.clone())"));
    assert!(main.contains(".with_git_wire_credential_issuer(git_wire_credentials)"));
    assert!(durable.contains("self.git_wire_credentials.bind(principal)"));
    assert!(http.contains(".wire_serving(ctx.principal)"));
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
fn founder_issue_identifiers_and_bootstrap_default_cannot_drift() {
    const PROJECT: &str = "20aee030-c7fa-4757-8243-700faf528690";
    const ISSUE_TYPE: &str = "7d457754-f6a1-4cd8-8738-21751570b627";

    let dogfood = include_str!("../../../scripts/dogfood.sh");
    let runbook = include_str!("../../../docs/dogfood.md");

    for value in [PROJECT, ISSUE_TYPE] {
        assert!(dogfood.contains(value));
        assert!(runbook.contains(value));
    }
    assert!(dogfood.contains("DOGFOOD_ISSUES_PREFIX=\"MYL\""));
    assert!(runbook.contains("PREFIX=MYL"));
    assert!(dogfood.contains("set -- \"$@\" --issues-project \"${MYELIN_DOGFOOD_ISSUES_PROJECT}\""));
    assert!(!dogfood.contains("11111111-1111-1111-1111-111111111111"));
    assert!(!dogfood.contains("22222222-2222-2222-2222-222222222222"));
    assert!(!runbook.contains("11111111-1111-1111-1111-111111111111"));
    assert!(!runbook.contains("22222222-2222-2222-2222-222222222222"));
}

#[test]
fn operator_bootstrap_validates_and_grants_the_explicit_issues_project_before_minting() {
    let source = include_str!("../src/main.rs");
    let bootstrap = source
        .split_once("async fn operator_bootstrap")
        .expect("operator bootstrap must remain wired")
        .1;

    let required_project = bootstrap
        .find("required_flag(args, \"--issues-project\")")
        .expect("the Issues project UUID is explicit and required");
    let validate = bootstrap
        .find("myelin_issues::api::is_canonical_uuid(&issues_project)")
        .expect("the project UUID is validated locally");
    let principal_store = bootstrap
        .find("let store = PrincipalStore::with_pg")
        .expect("durable principal store must remain wired");
    let tuple_store = bootstrap
        .find("let tuples = TupleStore::with_pg")
        .expect("durable tuple store must remain wired");
    let bootstrap_call = bootstrap
        .find("bootstrap_principal_and_mint(")
        .expect("the testable bootstrap body must remain wired");
    let print_token = bootstrap
        .find("println!(\"{}\", outcome.token)")
        .expect("token output must remain after successful bootstrap");

    assert!(required_project < validate);
    assert!(validate < principal_store);
    assert!(principal_store < tuple_store);
    assert!(tuple_store < bootstrap_call);
    assert!(bootstrap_call < print_token);
}

#[test]
fn missing_migration_credential_exits_before_bind() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_edge"))
        .arg("bootstrap")
        .env("MYELIN_CELL_ID", "cell-test")
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

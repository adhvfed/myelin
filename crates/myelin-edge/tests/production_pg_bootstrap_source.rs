//! Deployment guards for the Edge binary and founder-dogfood split-credential handoff.

fn git_wire_fixture() -> (std::path::PathBuf, std::path::PathBuf) {
    let rootfs = std::env::current_dir()
        .expect("test working directory must exist")
        .join("target/edge-production-source-rootfs");
    let guest_git = rootfs.join("usr/bin/git");
    std::fs::create_dir_all(guest_git.parent().unwrap()).expect("create guest bin directory");
    std::fs::copy("/bin/true", guest_git).expect("stage executable guest git fixture");
    let runsc = rootfs.join("runsc-fixture");
    std::fs::write(&runsc, "#!/bin/sh\necho 'runsc version test-fixture'\n")
        .expect("write runsc fixture");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&runsc, std::fs::Permissions::from_mode(0o755))
            .expect("make runsc fixture executable");
    }
    (
        rootfs.canonicalize().expect("canonical rootfs fixture"),
        runsc.canonicalize().expect("canonical runsc fixture"),
    )
}

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
    assert!(issues < issues_recent_index);
    assert!(issues_recent_index < issues_prefix_index);
    assert!(issues_prefix_index < head_index);
    assert!(head_index < operation_index);
    assert!(operation_index < handoff);
    assert!(handoff < first_runtime_store);
    assert!(handoff < bind);
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
    assert!(main.contains("shutdown_signal(),"));
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
    let git_root = std::env::current_dir().expect("test working directory must exist");
    let (git_wire_rootfs, runsc) = git_wire_fixture();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_edge"))
        .env("MYELIN_CELL_ID", "cell-test")
        .env("MYELIN_GIT_ROOT", git_root)
        .env("MYELIN_GVISOR_GIT_ROOTFS", git_wire_rootfs)
        .env("MYELIN_RUNSC_BIN", runsc)
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

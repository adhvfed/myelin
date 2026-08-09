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
    let device_authorization = source
        .find("&myelin_edge::device_authorization_migrations()")
        .expect("interactive CLI login state must run through PgBootstrap");
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
    assert!(durable < device_authorization);
    assert!(device_authorization < issues);
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
    assert!(route.contains("tenant: principal.tenant.clone()"));
    assert!(route.contains("region: principal.region.clone()"));
    assert!(route.contains("recipient: principal.principal_id.0.clone()"));
    assert!(route.contains("can_read_subject"));
    assert!(!route.contains("query_param(\"tenant\")"));
    assert!(!route.contains("query_param(\"recipient\")"));
}

#[test]
fn production_edge_mounts_durable_verifier_bound_cli_login() {
    let main = include_str!("../src/main.rs");
    let broker = include_str!("../src/device_auth.rs");

    assert!(main.contains("MYELIN_WEB_PUBLIC_URL"));
    assert!(main.contains("DeviceAuthorizationBroker::with_pg("));
    assert!(main.contains("provider.db_pool().clone()"));
    assert!(broker.contains("device_digest       bytea  PRIMARY KEY"));
    assert!(broker.contains("verifier_challenge  bytea  NOT NULL"));
    assert!(!broker.contains("access_token"));
    assert!(!broker.contains("browser_token"));
}

#[test]
fn production_edge_mounts_encrypted_durable_chat_topics_and_messages() {
    let main = include_str!("../src/main.rs");
    let route = include_str!("../src/chat_http.rs");
    let authz = include_str!("../src/authz.rs");

    assert!(main.contains("&myelin_chat::store::pg_conversation::chat_migrations()"));
    assert!(main.contains("CONVERSATION_RECENT_INDEX"));
    let chat_migration = main
        .split_once("&myelin_chat::store::pg_conversation::chat_migrations()")
        .expect("Chat migrations are mounted")
        .1;
    assert!(chat_migration.starts_with(",\n            &HotTables::none()"));
    assert!(main.contains("register_chat("));
    assert!(main.contains("kms.clone()"));
    assert!(route.contains("PgConversationStore::new(pool.clone())"));
    assert!(route.contains("PgMessageStore::new(pool, \"edge\", MESSAGE_TABLE)"));
    assert!(route.contains("create_co_commit("));
    assert!(route.contains("append_co_commit("));
    assert!(route.contains("encrypt_message_body("));
    assert!(route.contains("decode_encrypted_body("));
    assert!(!route.contains("body_inline: body.content.into_bytes()"));
    assert!(
        authz.contains("requirement!(\"chat.conversations.list\", \"chat.read\", OP_AGENT_PAT)")
    );
    assert!(authz.contains("requirement!(\"chat.conversation.create\", \"chat.manage\", OP_PAT)"));
    assert!(authz.contains("requirement!(\"chat.message.post\", \"chat.post\", OP_AGENT_PAT)"));
}

#[test]
fn production_edge_mounts_encrypted_durable_knowledge_pages() {
    let main = include_str!("../src/main.rs");
    let route = include_str!("../src/knowledge_http.rs");
    let store = include_str!("../../myelin-knowledge/src/pg_page.rs");
    let authz = include_str!("../src/authz.rs");

    assert!(main.contains("&myelin_knowledge::knowledge_page_migrations()"));
    assert!(main.contains("KNOWLEDGE_PAGE_RECENT_INDEX"));
    assert!(main.contains("register_knowledge("));
    assert!(main.contains("kms.clone()"));
    assert!(route.contains("/v1/knowledge/pages"));
    assert!(route.contains("KnowledgePageStore::new(pool)"));
    assert!(route.contains("encrypt_text("));
    assert!(route.contains("decrypt_text("));
    assert!(route.contains("current.owner != viewer"));
    assert!(store.contains("myelin_make_tenant_scoped"));
    assert_eq!(
        store.matches("PgRelay::co_commit_in_tx").count(),
        2,
        "create and save must each co-commit their domain event"
    );
    assert!(!store.contains("title_plaintext"));
    assert!(!store.contains("body_plaintext"));
    assert!(authz.contains(
        "requirement!(\"knowledge.pages.list\", \"knowledge.read\", OP_AGENT_PAT)"
    ));
    assert!(authz.contains(
        "requirement!(\"knowledge.page.save\", \"knowledge.edit\", OP_AGENT_PAT)"
    ));
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
    assert!(route.contains("may_view_ci_repo(principal, repo_slug)"));
    assert!(route.contains("fn authorized_repo_ref("));
    assert!(route.contains("self.api.read_run(ctx.principal, run_id)"));
    assert!(route.contains(".open_log_tail(ctx.principal, run_id, job_id, cursor)"));
    assert!(route.contains(".read_log(ctx.principal, run_id, job_id, request.start, request.limit)"));
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

//! Deployment guards for the CI Controlplane split-credential bootstrap sequence.

#[test]
fn production_main_hands_privileged_bootstrap_off_before_runtime_composition() {
    let source = include_str!("../src/main.rs");
    let library_source = include_str!("../src/lib.rs");
    let launch_authority_source = include_str!("../src/ci_launch_authority.rs");
    let claim_issuer_source = include_str!("../src/ci_claim_token_issuer.rs");
    let identity_adapter_source = include_str!("../src/ci_identity_adapter.rs");
    let runtime_composition_source = include_str!("../src/ci_runtime_composition.rs");
    let runner_identity_source = include_str!("../src/ci_runner_composition.rs");
    let job_queue_store_source = include_str!("../src/job_queue_store.rs");
    let region_store_source = include_str!("../src/job_queue_region.rs");
    let runner_bind_source = include_str!("../src/runner_bind.rs");

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
    let flow = source
        .find("&myelin_flow::migrations::migrations()")
        .expect("the Flow prerequisite must run through PgBootstrap");
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
    let runtime_factory = source
        .find("ci_production_runtime_factory(")
        .expect("the exact-tenant workflow/reporter factory must be composed at the root");
    let starter_poller = source
        .find("PgCiRunStarterPoller::new(")
        .expect("the region discovery to exact-tenant starter poller must be composed at the root");
    let workflow_poller = source
        .find(".workflow_poller(scheduler_provider.region_run_discovery()")
        .expect("active-run recovery must be composed at the dormant production root");
    let reporter_router = source
        .find("runner_runtime.reporter_router()")
        .expect("the accounted exact-tenant reporter router must be composed at the root");
    let runner_identity = source
        .find("ci_runner_identity_authorities(")
        .expect("the real CI Identity authorities must be composed at the root");
    let runner_hooks = source
        .find("ci_runner_hooks(")
        .expect("the scoped durable CI runner hooks must be composed at the root");
    let runner_cancellations = source
        .find("ci_runner_cancellation_coordinator(")
        .expect("accounted cancel-superseded capability must be composed at the root");
    let runner_gate = source
        .find("verify_startup_activation(runner_setting)")
        .expect("production runner activation must be refused explicitly");
    let bootstrap = source
        .find("PgBootstrap::connect(platform_config, DEFAULT_MAX_CONNECTIONS)")
        .expect("database bootstrap must remain wired");
    let service = source
        .find("run_controlplane_until_shutdown(Config::default()")
        .expect("signal-driven service lifecycle must remain wired");

    assert!(!source.contains("CiRunnerLoop::new"));
    assert!(!source.contains("spawn_until_shutdown"));
    assert!(!source.contains("CiPipelineDriver"));
    assert!(!source.contains("unresolved_stage_spec_builder"));
    assert!(!source.contains("TenantId(\"ci-controlplane\""));
    assert!(!source.contains("synthetic tenant"));
    // The starter lane composes behind the SAME MYELIN_CI_RUNNER seam the runner uses, and stays DORMANT
    // while the refusal stands: it is gated on the runner-host request, not spawned unconditionally.
    assert!(source.contains("if runner_host_requested {"));
    assert!(source.contains("let runner_host_requested = matches!(&runner_setting"));
    let starter_factory_source = &library_source[starter_lane_source_start(library_source)..];
    assert!(starter_factory_source.contains("LinuxSmallV1LaunchAuthority::new"));
    assert!(starter_factory_source.contains("PgTierPCiJobBudgetReservation::new"));
    assert!(starter_factory_source.contains("TIER_P_OPERATIONAL_ACTIVE_RESERVATION_CEILING"));
    assert!(!starter_factory_source.contains("DEFAULT_TENANT_IN_FLIGHT_CAP"));
    assert!(!starter_factory_source.contains("UnavailableCiJobBudgetReservation"));
    for production_dependency in [
        "DurableCellRootBacking::new",
        "RevocationStore::with_pg",
        "DurableRevocationBacking::new",
        "PasetoCapabilitySigner::new",
        "PasetoCapabilityVerifier::new",
        "RunTokenMinter::with_signer_and_tuples",
        "LockedManifestCiJobTokenIssuer::new",
        "IdentityCiJobLaunchAuthorizer::new",
    ] {
        assert!(
            runner_identity_source.contains(production_dependency),
            "production Identity composition must retain {production_dependency}"
        );
    }
    for forbidden_dependency in [
        "CellTokenAuthority::generate",
        "RevocationStore::new",
        "StructuralTokenSigner",
        "StructuralTokenVerifier",
    ] {
        assert!(
            !runner_identity_source.contains(forbidden_dependency),
            "production Identity composition must not admit {forbidden_dependency}"
        );
    }
    assert!(runner_identity_source.contains("provider.config().region.clone()"));
    for production_runtime_dependency in [
        "include_bytes!(\"ci_manifest_pipeline.rs\")",
        "include_bytes!(\"ci_manifest_job_runner.rs\")",
        "TenantScope::from_verified_token",
        "PgWorkerScope::new",
        "CiManifestInputResolver::new",
        "register_durable_ci_manifest_pipeline",
        "DurableCiRunFinalizer::new",
        "DurableCiJobAccounting::new",
        "TierPOperationalCiJobPricer",
        "CiPipelineReporter::new_accounted",
        "active_run_page",
        "route.partition",
        "run_until_idle",
    ] {
        assert!(
            runtime_composition_source.contains(production_runtime_dependency),
            "production exact-tenant runtime must retain {production_runtime_dependency}"
        );
    }
    assert!(claim_issuer_source.contains("request.region != self.region"));
    assert!(identity_adapter_source.contains("context.region != self.region.0"));
    assert!(job_queue_store_source.contains(
        "#[cfg(any(test, feature = \"test-support\"))]\n    pub async fn cancel_superseded("
    ));
    for production_lifecycle_dependency in [
        "CompletionSettlementOwner::TerminalReporter",
        "DurableCostLedger::with_runtime",
        "load_by_wf_run_on_conn",
        "get_launch_template_on_conn",
        "durable.spec != scope.template",
        "lock_exact_live_claim",
        "claim_nonce = $10::uuid",
        "begin_in_tx",
        "settle_in_tx",
        "ReleaseDisposition::CanceledBeforeLaunch",
        "completion_receipt.is_none()",
        "cancel_superseded_and_settle",
        "sqlx::query(crate::CANCEL_SUPERSEDED_QUERY)",
        "job.reserve_handle != scope.reserve_handle",
        "HardeningProfile::derive",
        "launch_authorizer.authorize(spec)",
    ] {
        assert!(
            runner_identity_source.contains(production_lifecycle_dependency),
            "production runner lifecycle must retain {production_lifecycle_dependency}"
        );
    }
    assert!(
        launch_authority_source.contains("ManifestBoundCiJobTokenAuthority::handle_for(request)")
    );
    assert!(!launch_authority_source.contains("CiJobTokenAuthorityProvider"));
    assert!(runner_bind_source.contains("token_issuer: LockedManifestCiJobTokenIssuer"));
    assert!(runner_bind_source.contains("pub fn durable_spec_resolver_test_support"));
    assert!(runner_bind_source.contains("#[cfg(any(test, feature = \"test-support\"))]"));
    assert!(!runner_bind_source.contains("#[cfg(any(test, feature = \"integration\"))]"));
    let claim_lock = claim_issuer_source
        .find("CiJobQueueStore::lock_for_token_mint_on_conn")
        .expect("claim-time issuer must lock the durable scheduler claim");
    let run_lock = claim_issuer_source
        .find("CiRunStore::lock_for_token_mint_on_conn")
        .expect("claim-time issuer must lock the durable run");
    let manifest = claim_issuer_source
        .find(".load_by_identity_on_conn(")
        .expect("claim-time issuer must reload the immutable manifest");
    let verify = claim_issuer_source
        .find("authority_from_durable_claim(&request, &run, &manifest)")
        .expect("claim-time issuer must reconstruct and verify durable authority");
    let mint = claim_issuer_source
        .find(".mint_verified(request.clone(), authority)")
        .expect("raw Identity mint must be last");
    assert!(claim_lock < run_lock && run_lock < manifest && manifest < verify && verify < mint);
    assert!(library_source.contains("PgCiRunStarterFactory::new_with_authority"));
    // It routes an AUTHORITATIVE tenant, never a synthetic one — no starter is constructed for a fixed
    // service tenant at the root (the factory mints per discovered ci_run.tenant_id).
    assert!(!source.contains("PgCiPipelineStarter::new"));
    assert!(source.contains("InvalidRunnerSetting(String)"));
    assert!(source.contains("NonUnicodeRunnerSetting(OsString)"));
    assert!(!source.contains("std::env::var(\"MYELIN_CI_RUNNER\").ok()"));
    assert!(!source.contains("ci_job_queue_store(provider.db_pool().clone())"));
    assert!(source.contains("scheduler_provider.region_queue_store()"));
    assert!(source.contains("reaper.run_until_shutdown(shutdown_rx)"));
    assert!(source.contains("shutdown_signal().await"));
    assert!(!library_source.contains("pub fn ci_region_queue_store("));
    assert!(!region_store_source.contains("pub fn with_pg"));

    assert!(runner_gate < bootstrap);
    assert!(foundation < durable);
    assert!(durable < flow);
    assert!(flow < controlplane);
    assert!(controlplane < hot_tables);
    assert!(hot_tables < handoff);
    assert!(handoff < scheduler_handoff);
    assert!(scheduler_handoff < shape_check);
    assert!(shape_check < first_store);
    assert!(first_store < reaper);
    assert!(reaper < starter_lane);
    assert!(starter_lane < runtime_factory);
    assert!(runtime_factory < starter_poller);
    assert!(starter_poller < workflow_poller);
    assert!(workflow_poller < reporter_router);
    assert!(reporter_router < runner_identity);
    assert!(runner_identity < runner_hooks);
    assert!(runner_hooks < runner_cancellations);
    assert!(runner_cancellations < service);
    assert!(runner_hooks < service);
    assert!(starter_lane < service);
}

fn starter_lane_source_start(source: &str) -> usize {
    source
        .find("pub fn ci_run_starter_factory(")
        .expect("production starter factory must stay named")
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
    assert!(stderr.contains("exact-tenant workflow-worker fan-out"));
    assert!(stderr.contains("complete launch/recovery proof"));
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

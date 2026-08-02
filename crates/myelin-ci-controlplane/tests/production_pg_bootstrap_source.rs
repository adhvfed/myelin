//! Deployment guards for the CI Controlplane split-credential bootstrap sequence.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use sqlx::PgPool;

fn executable_on_path(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|directory| directory.join(name))
        .find(|path| path.is_file())
}

#[test]
fn production_main_hands_privileged_bootstrap_off_before_runtime_composition() {
    let source = include_str!("../src/main.rs");
    let library_source = include_str!("../src/lib.rs");
    let dispatch_consumer_source = include_str!("../../myelin-ci-dispatch/src/consumer.rs");
    let git_events_source = include_str!("../../myelin-git/src/events.rs");
    let git_pr_event_source = include_str!("../../myelin-git/src/pg_pr_event.rs");
    let git_pr_store_source = include_str!("../../myelin-git/src/pg_pr_store.rs");
    let launch_authority_source = include_str!("../src/ci_launch_authority.rs");
    let run_store_source = include_str!("../src/ci_run_store.rs");
    let migrations_source = include_str!("../src/migrations.rs");
    let claim_issuer_source = include_str!("../src/ci_claim_token_issuer.rs");
    let identity_adapter_source = include_str!("../src/ci_identity_adapter.rs");
    let credential_generation_source = include_str!("../src/ci_credential_generation.rs");
    let checkout_composition_source = include_str!("../src/ci_checkout_composition.rs");
    let runtime_composition_source = include_str!("../src/ci_runtime_composition.rs");
    let manifest_runner_source = include_str!("../src/ci_manifest_job_runner.rs");
    let job_spec_store_source = include_str!("../src/job_spec_store.rs");
    let runner_identity_source = include_str!("../src/ci_runner_composition.rs");
    let supersession_source = include_str!("../src/ci_run_supersession.rs");
    let job_queue_store_source = include_str!("../src/job_queue_store.rs");
    let region_store_source = include_str!("../src/job_queue_region.rs");
    let runner_bind_source = include_str!("../src/runner_bind.rs");
    let runner_host_source = include_str!("../src/ci_runner_host.rs");
    let launch_gate_source = include_str!("../../myelin-ci-sandbox/src/launch_gate.rs");
    let gvisor_source = include_str!("../../myelin-ci-sandbox/src/gvisor.rs");
    let dogfood_source = include_str!("../../../scripts/dogfood.sh");

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
    // CT-007 round-2 blocker 1: the source-pinned definition is now activated by the CUTOVER FENCE
    // — one transaction that locks the superseded `wf_definition` row `FOR UPDATE` (mutually
    // exclusive with a fresh old-binary admission's `FOR SHARE`), runs the database-wide backlog
    // probe under that fence, and only then drains the old version and activates this one. The
    // preflight `activate_definition()` it replaced could not close that race.
    let definition_cutover = source
        .find(".cutover_definition(&scheduler_provider.region_run_discovery())")
        .expect("the source-pinned ci.pipeline definition must be activated through the fence");
    assert!(
        source[..definition_cutover].ends_with("runner_runtime\n            "),
        "the cutover must be invoked on the composed runtime factory"
    );
    // The diagnostic must come from the SCHEDULER capability, not the RLS-blind app pool: the app
    // role cannot see other tenants' rows, so an app-pool diagnostic reports "0 runs" for a real
    // backlog (round-3 finding 4).
    assert!(
        !source.contains("runner_runtime.activate_definition()"),
        "the unserialized preflight activation must never come back — it cannot fence fresh \
         admission by the outgoing binary"
    );
    let starter_poller = source
        .find("PgCiRunStarterPoller::new(")
        .expect("the region discovery to exact-tenant starter poller must be composed at the root");
    assert!(
        runtime_factory < definition_cutover,
        "the cutover needs the composed runtime factory"
    );
    let workflow_poller = source
        .find(".workflow_poller(scheduler_provider.region_run_discovery()")
        .expect("active-run recovery must be composed at the dormant production root");
    let reporter_router = source
        .find("runner_runtime.reporter_router()")
        .expect("the accounted exact-tenant reporter router must be composed at the root");
    let runner_identity = source
        .find("ci_runner_identity_authorities(")
        .expect("the real CI Identity authorities must be composed at the root");
    let runner_wiring = source
        .find("ci_runner_v2_wiring(")
        .expect("the coupled V2 resolver/hooks wiring must be selected at the root");
    assert_eq!(source.matches("ci_runner_v2_wiring(").count(), 1);
    assert_eq!(runner_bind_source.matches(".run_one_cycle(").count(), 1);
    assert!(!source.contains("myelin_ci_controlplane::ci_runner_hooks("));
    assert!(!source.contains("myelin_ci_controlplane::durable_spec_resolver("));
    let runner_loop = source
        .find("CiRunnerLoop::new(")
        .expect("the real sandbox runner loop must be composed at the dormant root");
    let runner_host = source
        .find("CiRunnerHost::new(")
        .expect("the coordinated runner host must own every dormant driver");
    let runner_gate = source
        .find("verify_startup_activation(runner_setting)")
        .expect("production runner activation must validate its explicit setting");
    let signal_install = source
        .find("let shutdown_signals = match ShutdownSignals::install()")
        .expect("OS signal handlers must be installed before runner-host intake");
    let signal_latch = source
        .find("let signal_task = tokio::spawn(async move")
        .expect("OS signals must be consumed into the intake latch during bootstrap");
    let executor_preflight = source
        .find("match prepare_runner_host() {")
        .expect("runner activation must preflight its exact executor before database bootstrap");
    let bootstrap = source
        .find("PgBootstrap::connect(platform_config, DEFAULT_MAX_CONNECTIONS)")
        .expect("database bootstrap must remain wired");
    let service = source
        .find("run_controlplane_until_shutdown(Config::default()")
        .expect("signal-driven service lifecycle must remain wired");

    // The cutover is the LAST boot gate: every fallible composition happens first (so a refusal
    // cannot strand a half-built process holding a committed registry transition), and nothing that
    // could admit work under the new version has spawned before the fence commits.
    for (name, position) in [
        ("starter poller", starter_poller),
        ("workflow poller", workflow_poller),
        ("reporter router", reporter_router),
        ("runner identity", runner_identity),
        ("V2 runner wiring", runner_wiring),
        ("runner loop", runner_loop),
    ] {
        assert!(
            position < definition_cutover,
            "{name} composition must complete BEFORE the definition cutover commits"
        );
    }
    assert!(
        definition_cutover < runner_host,
        "the cutover fence must be the last gate before the v4 lane is spawned"
    );

    assert!(!source.contains("spawn_until_shutdown"));
    assert!(!source.contains("runner.spawn("));
    assert!(!source.contains("runner.spawn_until_shutdown("));
    assert!(source.contains("CiRunnerHostConfig::production()"));
    assert!(source.contains("host.shutdown().await"));
    assert!(source.contains("wait_for_ci_runner_host_failure(failures)"));
    assert!(source.contains("wait_for_ci_runner_host_drain_timeout(failures)"));
    assert!(source.contains("runner_host_deadline_task"));
    assert!(
        runner_host_source.matches(".run_until_shutdown(").count() >= 2,
        "the host must drive both async production lanes through their shutdown-aware entrypoints"
    );
    for lifecycle_contract in [
        "runner.try_spawn_until_shutdown(shutdown)",
        "tokio::time::sleep(config.drain_timeout)",
        "drain.await",
        "shutdown_tx.send(true)",
    ] {
        assert!(
            runner_host_source.contains(lifecycle_contract),
            "runner host must retain lifecycle contract {lifecycle_contract}"
        );
    }
    assert!(!source.contains("ci_runner_cancellation_coordinator("));
    assert!(!source.contains("CiPipelineDriver"));
    assert!(!source.contains("unresolved_stage_spec_builder"));
    assert!(!source.contains("TenantId(\"ci-controlplane\""));
    assert!(!source.contains("synthetic tenant"));
    // The starter lane composes behind the SAME explicit MYELIN_CI_RUNNER seam the runner uses and is
    // never spawned unconditionally.
    assert!(source.contains("if runner_host_requested {"));
    assert!(source.contains("let runner_host_requested = matches!(&runner_setting"));
    let starter_factory_source = &library_source[starter_lane_source_start(library_source)..];
    assert!(starter_factory_source.contains("LinuxSmallV1LaunchAuthority::new"));
    assert!(starter_factory_source.contains("PgTierPCiJobBudgetReservation::new"));
    assert!(starter_factory_source.contains("TIER_P_OPERATIONAL_ACTIVE_RESERVATION_CEILING"));
    assert!(starter_factory_source.contains("new_with_authority_and_supersession"));
    assert!(starter_factory_source.contains("supersession_ledger"));
    assert!(!starter_factory_source.contains("DEFAULT_TENANT_IN_FLIGHT_CAP"));
    assert!(!starter_factory_source.contains("UnavailableCiJobBudgetReservation"));
    assert!(run_store_source.contains("pg_advisory_xact_lock"));
    assert!(run_store_source.contains("lock_pr_concurrency_group_on_conn(conn"));
    assert!(supersession_source.contains("lock_pr_concurrency_group_on_conn("));
    assert!(manifest_runner_source.contains("co_persist_active_flow_dispatch"));
    assert!(job_spec_store_source.contains("SELECT state FROM workflow_run"));
    assert!(job_spec_store_source.contains("state.as_deref() != Some(\"running\")"));
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
    assert_eq!(
        runtime_composition_source
            .matches("CiJobAccountingStore::with_pg_and_write_version(")
            .count(),
        2,
        "worker and reporter-router accounting both select V4"
    );
    assert_eq!(
        supersession_source
            .matches("CiJobAccountingStore::with_pg_and_write_version(")
            .count(),
        1,
        "supersession accounting selects V4 exactly once"
    );
    assert_eq!(
        library_source
            .matches("CiJobAccountingStore::with_pg_and_write_version(")
            .count(),
        1,
        "the public accounting factory selects V4 exactly once"
    );
    for (name, production_source) in [
        ("runtime composition", runtime_composition_source),
        ("supersession composition", supersession_source),
        ("public factory", library_source),
    ] {
        assert_eq!(
            production_source
                .matches("CiJobAccountingStore::with_pg(")
                .count(),
            0,
            "{name} must not retain a legacy V3 accounting constructor"
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
        "job.reserve_handle != scope.reserve_handle",
        "HardeningProfile::derive",
        "launch_authorizer.authorize_retained(spec)",
    ] {
        assert!(
            runner_identity_source.contains(production_lifecycle_dependency),
            "production runner lifecycle must retain {production_lifecycle_dependency}"
        );
    }
    assert!(job_queue_store_source.contains(".bind(CI_RUNNER_EXECUTION_LEASE_TTL_SECS)"));
    assert!(
        job_queue_store_source.contains("pg_backend_pid() AND locktype = 'advisory'"),
        "the final launch fence must reject a pooled session with stale advisory ownership"
    );
    let validate_ownership = launch_gate_source
        .find("ownership.validate()")
        .expect("launch ownership must be validated before gate release");
    let open_gate = launch_gate_source
        .find("gate.write_all(b\"launch\\n\")")
        .expect("the durable gate must have one explicit release byte");
    let release_ownership = open_gate
        + launch_gate_source[open_gate..]
            .find("ownership.release()")
            .expect("launch ownership must be released after the gate handoff");
    assert!(
        validate_ownership < open_gate && open_gate < release_ownership,
        "launch ownership must remain held across the exact child-gate write"
    );
    for watchdog_floor in [
        "CLOCK_BOOTTIME",
        "timerfd_create",
        "native_watchdog",
        "close_unneeded_fds",
        "kill_from_watchdog",
        "The watchdog must survive a parent-driven kill of the runtime group",
    ] {
        assert!(
            launch_gate_source.contains(watchdog_floor),
            "the independent launch watchdog must retain {watchdog_floor}"
        );
    }
    assert!(gvisor_source.contains("kill_cgroup_on_liveness_loss(kill_file)"));
    assert!(gvisor_source.contains("launch_permit.as_ref().map(|_| timeout)"));
    assert!(source.contains("preflight_gvisor_runner_host(&runsc, &rootfs)"));
    for executor_preflight in [
        "probe_runsc_version(&runsc)",
        "MYELIN_GVISOR_ROOTFS must contain executable",
        "args: vec![\"/bin/false\".into()]",
        "run_and_capture(",
        "outcome.exit != Some(1)",
    ] {
        assert!(
            gvisor_source.contains(executor_preflight),
            "runner-host executor preflight must retain {executor_preflight}"
        );
    }
    assert!(dogfood_source.contains("export MYELIN_GVISOR_ROOTFS="));
    assert!(dogfood_source.contains("export MYELIN_CI_RUNNER=1"));
    assert!(dogfood_source.contains("export MYELIN_CI_CHECKOUT_REPO_ROOT="));
    assert!(dogfood_source
        .contains("exec cargo run --quiet -p myelin-ci-controlplane --bin ci-controlplane"));
    for production_supersession_dependency in [
        "pr_head_generation",
        "cancel_stale_queued_on_conn",
        "cancel_running_on_conn",
        "cancel_on_conn",
        "settle_in_tx",
        "CI_RUN_CANCELLED",
        "CheckState::Cancelled",
        "emit_settled_cancelled_checks_on_conn",
    ] {
        assert!(
            supersession_source.contains(production_supersession_dependency),
            "production run supersession must retain {production_supersession_dependency}"
        );
    }
    assert!(launch_authority_source.contains(
        "token_authority_request.reserve_id = Some(reserve_handle.clone())"
    ));
    assert!(launch_authority_source.contains("CI_TOKEN_AUTHORITY_V4_HANDLE_PREFIX"));
    assert!(launch_authority_source.contains(
        ".map(|scope| scope.commit_hex().to_owned())"
    ));
    assert!(launch_authority_source
        .contains("ManifestBoundCiJobTokenAuthority::handle_for(&token_authority_request)"));
    assert_eq!(
        checkout_composition_source
            .matches("minted.checkout.as_ref()")
            .count(),
        2,
        "initial and rotated phase contexts must both use the durable minted checkout"
    );
    assert!(checkout_composition_source.contains("mint_phase_credential_for_checkout_scope"));
    assert!(credential_generation_source.contains(
        "(launch.spec #>> '{spec,workspace,commit}') IS NOT DISTINCT FROM $21::text"
    ));
    assert!(identity_adapter_source.contains("checkout-commit:{format}:{}#attest"));
    assert!(launch_authority_source.contains("labels: LINUX_SMALL_V1_RUNNER_LABELS"));
    assert!(dispatch_consumer_source.contains("let group = format!(\"pr:{repo}:{number}\")"));
    assert!(dispatch_consumer_source.contains(".get(\"head_generation\")"));
    assert!(dispatch_consumer_source.contains("(\"head_oid\", Some(group), Some(generation))"));
    assert!(dispatch_consumer_source.contains(".with_upcaster(pr_trigger_upcasters().into_hook())"));
    assert!(dispatch_consumer_source
        .contains("ev.schema_ver < myelin_git::events::GIT_PR_HEAD_TRIGGER_SCHEMA_V2"));
    assert!(dispatch_consumer_source
        .contains("concurrency_group: armed.reserve.concurrency_group.clone()"));
    assert!(
        dispatch_consumer_source.contains("pr_head_generation: armed.reserve.pr_head_generation")
    );
    assert!(git_events_source.contains("git_event_token_list() -> SubsystemTokenList"));
    assert!(git_events_source.contains("GIT_PR_HEAD_TRIGGER_SCHEMA_V2"));
    assert!(git_pr_store_source.contains("co_commit_event("));
    assert!(git_pr_event_source.contains("payload[\"head_generation\"] = generation.into()"));
    assert!(git_pr_event_source.contains("ctx.schema_ver = GIT_PR_HEAD_TRIGGER_SCHEMA_V2"));
    assert!(git_pr_event_source.contains("SELECT version FROM git_pr"));
    assert!(run_store_source.contains("valid_pr_concurrency_group(group)"));
    assert!(run_store_source.contains("row.pr_head_generation"));
    assert!(launch_authority_source.contains("launch_concurrency_group(record)?"));
    assert!(launch_authority_source.contains("record.pr_head_generation"));
    assert!(launch_authority_source.contains("concurrency_group: concurrency_group.clone()"));
    assert!(migrations_source.contains("ci_0001c_ci_run_concurrency_group"));
    assert!(migrations_source.contains("ci_0001d_ci_run_pr_head_generation"));
    assert!(source.contains("myelin_ci_controlplane::LINUX_SMALL_V1_RUNNER_LABELS"));
    assert!(!launch_authority_source.contains("CiJobTokenAuthorityProvider"));
    assert!(runner_bind_source.contains("token_issuer: LockedManifestCiJobTokenIssuer"));
    assert!(runner_bind_source.contains("pub fn durable_spec_resolver_test_support"));
    assert!(runner_bind_source.contains("#[cfg(any(test, feature = \"test-support\"))]"));
    assert!(!runner_bind_source.contains("#[cfg(any(test, feature = \"integration\"))]"));
    assert!(
        !runner_bind_source.contains("FirecrackerBackend"),
        "FirecrackerBackend must NEVER be wired into production runner_bind.rs: unlike \
         GvisorBackend, it does not yet make the Uncommitted/CommitOutcomeUnknown/\
         CommittedButNotExecuted/Executed phase distinction on a post-reserve launch failure — \
         every failure is compatibility-wrapped (phase-unclassified) as `SandboxLaunchError::Failed` \
         (see firecracker.rs's SandboxBackend impl). Wiring it into production today would \
         silently reproduce the exact reservation-leak class this whole CT-007 fix closed for \
         gVisor. This is a NAMED production-activation blocker, not merely a comment — give \
         FirecrackerBackend gVisor's same phase-aware treatment before ever removing this guard."
    );
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
        .find("authority_from_durable_claim(&request, &run, &manifest, &launch_template)")
        .expect("claim-time issuer must reconstruct and verify durable authority");
    let reserve_binding = claim_issuer_source
        .find("launch_template.spec.meter_to.reserve_id != job.reserve_handle")
        .expect("claim-time issuer must bind the dispatched reservation to the manifest");
    let checkout_commit_binding = claim_issuer_source
        .find("launch_template.spec.workspace.commit.as_deref()")
        .expect("claim-time issuer must bind the dispatched checkout commit to durable authority");
    let authority_function = claim_issuer_source
        .find("pub(crate) fn authority_from_durable_claim(")
        .expect("claim-time authority verifier must remain explicit");
    let authority_handle_verification = claim_issuer_source
        .find("ManifestBoundCiJobTokenAuthority::verifies(")
        .expect("claim-time issuer must verify the versioned authority handle");
    let mint = claim_issuer_source
        .find(".mint_verified(request.clone(), authority)")
        .expect("raw Identity mint must be last");
    assert!(claim_lock < run_lock && run_lock < manifest && manifest < verify && verify < mint);
    assert!(
        authority_function < reserve_binding
            && reserve_binding < checkout_commit_binding
            && checkout_commit_binding < authority_handle_verification,
        "reserve and checkout-commit bindings must happen inside durable-authority verification \
         before handle acceptance"
    );
    assert!(library_source.contains("PgCiRunStarterFactory::new_with_authority"));
    // It routes an AUTHORITATIVE tenant, never a synthetic one — no starter is constructed for a fixed
    // service tenant at the root (the factory mints per discovered ci_run.tenant_id).
    assert!(!source.contains("PgCiPipelineStarter::new"));
    assert!(source.contains("InvalidRunnerSetting(String)"));
    assert!(source.contains("NonUnicodeRunnerSetting(OsString)"));
    // CT-007 slice 4: the workspace-activation level is a SEPARATE opt-in gate layered on top of
    // MYELIN_CI_RUNNER, parsed/preflighted exactly once, and its owned result -- never a second
    // environment read -- is what reaches CiRunnerLoop::new.
    assert!(source.contains("InvalidWorkspaceMode(String)"));
    assert!(source.contains("NonUnicodeWorkspaceMode(OsString)"));
    assert!(source.contains("\"MYELIN_CI_WORKSPACE_MODE\""));
    assert!(source.contains("fn parse_workspace_activation_given("));
    assert!(source.contains("fn prepare_runner_host_given("));
    assert!(source.contains("gvisor_workspace_config"));
    assert!(source.contains("MYELIN_CI_CHECKOUT_REPO_ROOT"));
    assert!(source.contains("prepare_checkout_config()"));
    assert!(source.contains("gvisor_checkout_config"));
    let workspace_prep = source
        .find("let gvisor_workspace_config = if runner_host_requested {")
        .expect("workspace activation must be parsed/preflighted exactly once, before bootstrap");
    let workspace_construction = source
        .find("gvisor_workspace_config\n                .expect(")
        .expect("the preflighted workspace configuration must be passed into CiRunnerLoop::new");
    assert!(
        workspace_prep < bootstrap,
        "workspace-activation preparation must precede PostgreSQL bootstrap, exactly like the \
         rootless executor preflight it now wraps"
    );
    assert!(
        bootstrap < workspace_construction,
        "CiRunnerLoop::new must consume the ALREADY-preflighted configuration parsed before \
         bootstrap, never re-derive it from a second, later environment read"
    );
    assert!(!source.contains("std::env::var(\"MYELIN_CI_RUNNER\").ok()"));
    assert!(source.contains("ci_job_queue_store(provider.db_pool().clone())"));
    assert!(source.contains("scheduler_provider.region_queue_store()"));
    assert!(source.contains("reaper.run_until_shutdown(shutdown_tx.subscribe())"));
    assert!(source.contains(".start_with_shutdown("));
    assert!(source.contains("shutdown_tx.clone()"));
    assert!(source.contains("if *shutdown_rx.borrow_and_update()"));
    assert!(source.contains("_ = shutdown.as_mut()"));
    assert!(!library_source.contains("pub fn ci_region_queue_store("));
    assert!(!region_store_source.contains("pub fn with_pg"));

    assert!(runner_gate < signal_install);
    assert!(signal_install < signal_latch);
    assert!(signal_latch < executor_preflight);
    assert!(executor_preflight < bootstrap);
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
    assert!(runner_identity < runner_wiring);
    assert!(runner_wiring < runner_loop);
    assert!(runner_loop < runner_host);
    assert!(runner_host < service);
    assert!(runner_wiring < service);
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
fn runner_activation_refuses_a_missing_executor_before_platform_or_database_access() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ci-controlplane"))
        .env_clear()
        .env("MYELIN_CI_RUNNER", "1")
        .output()
        .expect("CI Controlplane process must launch");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr must be UTF-8");
    assert!(stderr.contains("runner-host executor preflight refused"));
    assert!(stderr.contains("MYELIN_RUNSC_BIN is required"));
    assert!(!stderr.contains("platform configuration refused to start"));
    assert!(!stderr.contains("database bootstrap refused to start"));
    assert!(!stderr.contains("DATABASE_URL"));
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

#[tokio::test(flavor = "multi_thread")]
async fn boot_time_sigterm_is_latched_before_the_real_runner_host_can_claim() {
    let required = std::env::var("MYELIN_REQUIRE_RUNSC").as_deref() == Ok("1");
    let Some(runsc) = std::env::var_os(myelin_ci_sandbox::gvisor::ENV_RUNSC_BIN)
        .map(std::path::PathBuf::from)
        .or_else(|| executable_on_path("runsc"))
    else {
        if required {
            panic!("MYELIN_REQUIRE_RUNSC=1 but runsc is unavailable");
        }
        eprintln!("SKIP live CI runner activation proof: runsc is unavailable");
        return;
    };
    let runsc = runsc
        .canonicalize()
        .expect("resolve the live runsc executable");
    let rootfs = myelin_ci_sandbox::resolved_gvisor_rootfs();
    if !rootfs.join("bin/sh").is_file() {
        if required {
            panic!(
                "MYELIN_REQUIRE_RUNSC=1 but the gVisor rootfs is unavailable at {}",
                rootfs.display()
            );
        }
        eprintln!(
            "SKIP live CI runner activation proof: gVisor rootfs is unavailable at {}",
            rootfs.display()
        );
        return;
    }
    let admin_url = "postgres://myelin_admin:myelin_dev_pw@localhost:5433/myelin";
    let admin = match PgPool::connect(admin_url).await {
        Ok(pool) => pool,
        Err(error) => {
            eprintln!("SKIP live CI runner activation proof: {error}");
            return;
        }
    };
    let suffix = format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    // One exact row in `public.ci_run` is a deliberately preserved, permanent exception to
    // "zero pre-existing active work": tenant_id = 'myelin', run_id =
    // '5db61d81-6aea-7dd9-b3f1-035abcf56b26', state = 'running'. It was intentionally left
    // running/unsettled as permanent historical negative evidence by the R4.2
    // publisher-capability-reconciliation investigation (see
    // planning/system-reviews/2026-06-26/12-ci-track-ledger.md, ledger entry CT-005f8a and the
    // "Honest remaining floor" note below it) and must never be relabelled, deleted, or counted.
    // This is a single named `(tenant_id, run_id)` pair, not a blanket tenant carve-out: any OTHER
    // active row (including any other row for tenant `myelin`, or any row in `job_queue`) still
    // fails this precondition as unexpected leftover work.
    const PRESERVED_NEGATIVE_EVIDENCE_TENANT_ID: &str = "myelin";
    const PRESERVED_NEGATIVE_EVIDENCE_RUN_ID: &str = "5db61d81-6aea-7dd9-b3f1-035abcf56b26";

    let mut job_queue_exists = false;
    for table in ["ci_run", "job_queue"] {
        let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
            .bind(format!("public.{table}"))
            .fetch_one(&admin)
            .await
            .expect("inspect public CI activation table");
        if table == "job_queue" {
            job_queue_exists = exists;
        }
        if exists {
            let active: i64 = if table == "ci_run" {
                sqlx::query_scalar(
                    "SELECT count(*) FROM public.ci_run \
                     WHERE state IN ('queued', 'leased', 'running') \
                       AND NOT (tenant_id = $1 AND run_id = $2::uuid)",
                )
                .bind(PRESERVED_NEGATIVE_EVIDENCE_TENANT_ID)
                .bind(PRESERVED_NEGATIVE_EVIDENCE_RUN_ID)
                .fetch_one(&admin)
                .await
                .expect("count active public CI rows, excluding the one preserved historical exception")
            } else {
                sqlx::query_scalar(&format!("SELECT count(*) FROM public.{table} WHERE state IN ('queued', 'leased', 'running')"))
                    .fetch_one(&admin)
                    .await
                    .expect("count active public CI rows")
            };
            assert_eq!(
                active, 0,
                "the production-root smoke test refuses to execute pre-existing active work \
                 (excluding the one named, permanently-preserved historical negative-evidence \
                 row tenant_id='myelin'/run_id='5db61d81-6aea-7dd9-b3f1-035abcf56b26' in ci_run)"
            );
        }
    }
    if !job_queue_exists {
        if required {
            panic!("MYELIN_REQUIRE_RUNSC=1 but the production job_queue schema is unavailable");
        }
        eprintln!(
            "SKIP live CI runner activation proof: production job_queue schema is unavailable"
        );
        return;
    }
    let tenant_id = format!("ci-runner-shutdown-{suffix}");
    let seeded_job_id: String = sqlx::query_scalar(
        "INSERT INTO public.job_queue \
           (tenant_id, region, job_id, run_id, lane, labels, trust_tier, concurrency_group, \
            fair_key, idem_token, stage, state, claim_window_secs, reservation_write_version) \
         VALUES ($1, 'fr-par', gen_random_uuid(), gen_random_uuid(), 'interactive', \
                 ARRAY[]::text[], 'trusted', NULL, $1, $2, 'shutdown-proof', 'queued', 900, 2) \
         RETURNING job_id::text",
    )
    .bind(&tenant_id)
    .bind(format!("shutdown-proof-{suffix}"))
    .fetch_one(&admin)
    .await
    .expect("seed one uniquely-owned queued job behind the startup signal gate");
    let cell_id = format!("ci-runner-activation-{suffix}");
    let checkout_repo_root = std::env::temp_dir().join(format!("myelin-ci-checkout-{suffix}"));
    std::fs::create_dir_all(&checkout_repo_root).expect("create checkout repository root");
    let mut child = Command::new(env!("CARGO_BIN_EXE_ci-controlplane"))
        .env_clear()
        .env(
            "DATABASE_URL",
            "postgres://myelin_app:myelin_app_pw@localhost:5433/myelin",
        )
        .env("DATABASE_MIGRATION_URL", admin_url)
        .env(
            "MYELIN_CI_SCHEDULER_DATABASE_URL",
            "postgres://myelin_ci_scheduler_fr_par:myelin_ci_scheduler_dev_pw@localhost:5433/myelin",
        )
        .env("S3_ENDPOINT", "http://localhost:9000")
        .env("S3_REGION", "fr-par")
        .env("S3_ACCESS_KEY", "myelin_dev_access")
        .env("S3_SECRET_KEY", "myelin_dev_secret")
        .env("S3_BUCKET", "myelin-dev")
        .env("REDIS_URL", "redis://localhost:6380")
        .env("NATS_URL", "nats://localhost:4222")
        .env("MYELIN_REGION", "fr-par")
        .env("MYELIN_CELL_ID", &cell_id)
        .env("MYELIN_KMS_SEAL_KEY", "55".repeat(32))
        .env(
            "XDG_RUNTIME_DIR",
            std::env::var("XDG_RUNTIME_DIR")
                .expect("live rootless runsc proof requires XDG_RUNTIME_DIR"),
        )
        .env(myelin_ci_sandbox::gvisor::ENV_RUNSC_BIN, &runsc)
        .env(myelin_ci_sandbox::gvisor::ENV_GVISOR_ROOTFS, &rootfs)
        .env("MYELIN_CI_RUNNER", "1")
        .env("MYELIN_CI_CHECKOUT_REPO_ROOT", &checkout_repo_root)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("launch the production CI Controlplane binary");
    let stderr = child.stderr.take().expect("capture child stderr");
    let captured = Arc::new(Mutex::new(String::new()));
    let captured_reader = captured.clone();
    let (armed_tx, armed_rx) = mpsc::sync_channel(1);
    let (started_tx, started_rx) = mpsc::sync_channel(1);
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let line = line.expect("read CI Controlplane stderr");
            captured_reader.lock().unwrap().push_str(&line);
            captured_reader.lock().unwrap().push('\n');
            if line.contains("shutdown handlers armed; startup termination is intake-gated") {
                let _ = armed_tx.try_send(());
            }
            if line.contains("started (region `fr-par`") {
                let _ = started_tx.try_send(());
            }
        }
    });

    let armed = armed_rx.recv_timeout(Duration::from_secs(10));
    let kill = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("signal the CI Controlplane process");
    let status = child.wait().expect("wait for the bounded production drain");
    reader.join().expect("join child stderr reader");
    let stderr = captured.lock().unwrap().clone();
    std::fs::remove_dir_all(&checkout_repo_root).expect("remove checkout repository root");

    let untouched_state: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT state, lease_owner FROM public.job_queue \
         WHERE tenant_id = $1 AND job_id = $2::uuid",
    )
    .bind(&tenant_id)
    .bind(&seeded_job_id)
    .fetch_optional(&admin)
    .await
    .expect("inspect the shutdown-gated queued job");
    let job_cleanup =
        sqlx::query("DELETE FROM public.job_queue WHERE tenant_id = $1 AND job_id = $2::uuid")
            .bind(&tenant_id)
            .bind(&seeded_job_id)
            .execute(&admin)
            .await;
    let cleanup = sqlx::query("DELETE FROM public.cell_token_root WHERE cell_id = $1")
        .bind(&cell_id)
        .execute(&admin)
        .await;
    admin.close().await;
    cleanup.expect("remove only the activation proof's disposable cell root");
    job_cleanup.expect("remove only the activation proof's disposable queued job");

    armed.unwrap_or_else(|error| {
        panic!("shutdown handlers were not armed before timeout: {error}; stderr={stderr}")
    });
    assert!(kill.success(), "SIGTERM must reach the live process");
    assert!(
        status.success(),
        "boot-time SIGTERM must cleanly drain the production root: status={status}; stderr={stderr}"
    );
    assert!(
        started_rx.try_recv().is_err(),
        "runner intake must never announce after bootstrap was termination-gated; stderr={stderr}"
    );
    assert_eq!(
        untouched_state,
        Some(("queued".to_owned(), None)),
        "work queued before boot-time SIGTERM must remain unclaimed"
    );
}

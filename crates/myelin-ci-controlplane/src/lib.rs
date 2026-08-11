pub mod artifact_cache;
pub mod check_emitter;
pub mod ci_checkout_composition;
pub mod ci_claim_token_issuer;
pub mod ci_claim_window;
pub mod ci_credential_generation;
pub mod ci_drive_manifest;
pub mod ci_identity_adapter;
pub mod ci_launch_authority;
pub mod ci_manifest_job_runner;
pub mod ci_manifest_pipeline;
pub mod ci_pipeline;
pub mod ci_pipeline_protocol;
pub mod ci_prelaunch_usage_journal;
pub mod ci_result_signal;
pub mod ci_runner_composition;
pub mod ci_runner_host;
pub mod ci_runtime_composition;
pub mod cli;
pub use ci_claim_token_issuer::{CiJobCredentialMinter, LockedManifestCiJobTokenIssuer};
pub use ci_credential_generation::{
    acquire_phase_generation_ownership, lock_phase_generation_query, phase_generation_id,
    verify_phase_generation_live, CiCredentialGenerationError, CiCredentialGenerationOutcome,
    CiCredentialPurpose, CiJobCredentialGenerationStore, CiJobCredentialWriteVersion,
    CiPhaseCredentialBinding, CiPhaseCredentialMintRequest, CiPhaseCredentialMinter,
    CiPhaseGenerationGate, CiPhaseGenerationInputs, MintedPhaseCredential,
    RetainedCiPhaseGeneration, CI_PHASE_CREDENTIAL_BINDING_V1,
    CI_PHASE_CREDENTIAL_GENERATION_PREFIX, CI_PHASE_CREDENTIAL_V1_DOMAIN,
    VERIFY_PHASE_GENERATION_QUERY,
};
pub use ci_drive_manifest::{
    ci_check_context_v1, CiDriveManifestError, CiDriveManifestStore, CiDriveManifestV1,
    CiJobLaunchGrantV1, CiLaunchAuthorityV1, CiManifestLaneV1, CiManifestLimitsV1,
    CiManifestSchedulingV1, CiManifestTrustTierV1, CiManifestWorkspaceV1, CiMergeWaiterV1,
    GrantedCiJobV1, CI_DRIVE_MANIFEST_DIGEST_V1_DOMAIN, CI_DRIVE_MANIFEST_SCHEMA_V1,
    MAX_CI_DRIVE_MANIFEST_BYTES,
};
pub use ci_identity_adapter::{
    ci_job_authorization_context, ci_job_phase_authorization_context, expected_phase_jti,
    phase_ci_capabilities, CiCredentialExpectation, IdentityCiJobCredentialMinter,
    IdentityCiJobLaunchAuthorizer, CI_JOB_PRINCIPAL_ID, CI_JOB_REQUIRED_CAPABILITIES,
};
pub use ci_launch_authority::{
    runner_labels_for_profile, runner_labels_for_profiles, CiAttemptBudgetPolicy,
    CiAttemptBudgetRevision, CiJobBudgetReservationProvider, CiJobRuntimeAuthorityRequest,
    LinuxSmallV1LaunchAuthority, ManifestBoundCiJobTokenAuthority,
    OperationalReservationWriteVersion, PgTierPCiJobBudgetReservation,
    ReservationWriteVersionMarker, TierPOperationalCiJobPricer, LINUX_BUILD_V1_RUNNER_LABELS,
    LINUX_SMALL_V1_POLICY_REVISION, LINUX_SMALL_V1_RUNNER_LABELS,
    TIER_P_OPERATIONAL_ACTIVE_RESERVATION_CEILING,
};
pub use ci_manifest_job_runner::{
    register_durable_ci_manifest_pipeline, secret_broker_ci_job_resolver,
    unavailable_ci_job_secret_resolver, CiJobSecretResolver, CiJobTokenIssueError,
    CiJobTokenIssuer, CiJobTokenRequest, CiManifestDurableJobRunner,
};
pub use ci_manifest_pipeline::{
    decode_resolved_ci_manifest, drive_resolved_ci_manifest_pipeline,
    register_ci_manifest_pipeline, run_ci_manifest_pipeline, CiManifestInputResolver,
    CiManifestPipelineOutcome,
};
pub use ci_prelaunch_usage_journal::{
    resolve_prelaunch_usage_on_conn, CiJobParentAttempt, CiParentAttemptAdmission,
    CiPrelaunchJournalOutcome, CiPrelaunchParentExpectation, CiPrelaunchSettlementIdentity,
    CiPrelaunchUnresolvedPolicy, CiPrelaunchUsageAccrual, CiPrelaunchUsageJournal,
    CiPrelaunchUsageJournalError, CiPrelaunchUsagePhase,
};
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub use ci_runner_composition::{
    ci_runner_cancellation_coordinator, CiRunnerCancellationCoordinator,
};
pub use ci_runner_composition::{
    ci_runner_hooks, ci_runner_identity_authorities, ci_runner_v2_wiring,
    ci_runner_v2_wiring_with_secret_resolver, CiRunnerIdentityAuthorities,
    CiRunnerIdentityCompositionError, CiRunnerV2Wiring,
};
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub use ci_runtime_composition::ci_production_runtime_factory_test_support;
pub use ci_runtime_composition::{
    ci_manifest_pipeline_definition, ci_production_runtime_factory, ActivationReadinessProbe,
    CiProductionRuntimeFactory, CiProductionWorkflowPoller, CiRuntimeCompositionError,
    CiSupersededDefinitionBacklog, CiSupersededDefinitionGuardError, CiWorkflowFanoutBatch,
    CutoverPlan, CI_DEFINITION_FENCE_LOCK_TIMEOUT_MS, CI_FLOW_OUTBOX_SCHEMA_VERSION,
    CI_FLOW_WORKER_LEASE_TTL_SECS, CI_MANIFEST_PIPELINE_SUPERSEDED_VERSION,
    CI_MANIFEST_PIPELINE_VERSION, MAX_CI_WORKFLOW_DRIVES_PER_SCOPE,
    MAX_CI_WORKFLOW_SCOPES_PER_PASS,
};
pub mod ci_run_region;
pub mod ci_run_starter_poller;
pub mod ci_run_store;
pub mod ci_run_supersession;
pub mod ci_scheduler_db;
pub mod cost_store;
pub mod crypto_shred_erase;
pub mod deployment;
#[cfg(any(test, feature = "test-support"))]
pub mod e2e_flagship;
#[cfg(any(test, feature = "test-support"))]
pub mod e2e_wedge;
pub mod events;
pub mod fairness;
pub mod fleet;
pub mod job_accounting_store;
pub mod job_queue_store;
pub mod job_spec_store;
pub mod run_plan;
pub use run_plan::{
    decode_resolved_run_plan, derive_concrete_job_name, load_launch_run_plan_v2,
    load_resolved_run_plan, CiExecutionProfileV1, CiExecutionRequestV1, PreparedRunPlan,
    PreparedRunPlanV2, RedispatchReason, ResolvedJobV1, ResolvedJobV2, ResolvedRunPlanV1,
    ResolvedRunPlanV2, RunPlanError, StructuredBuildToolV1, StructuredBuildV1,
    VersionedResolvedRunPlan, EXECUTION_REQUEST_SCHEMA_V1, LAUNCH_REQUEST_DIGEST_V1_DOMAIN,
    PLATFORM_CARGO_HOME, RUN_PLAN_SCHEMA_V1, RUN_PLAN_SCHEMA_V2,
};
pub mod pg_pipeline_starter;
pub use pg_pipeline_starter::{
    ci_job_id_v1, ci_job_id_v2, decode_ci_claimed_input, CiLaunchAuthorityError,
    CiLaunchAuthorityMaterializer, CiWorkflowDefinitionPin, ClaimedCiInput, ClaimedCiInputError,
    PgCiPipelineStarter, PgCiRunStarterFactory, PgCiStarterError, StartQueuedOutcome,
    CI_INITIAL_CHECK_EVENT_V1_DOMAIN, CI_JOB_ID_V1_DOMAIN, CI_JOB_ID_V2_DOMAIN,
};
pub mod ci_pipeline_driver;
pub mod ci_pipeline_reporter_router;
pub mod ci_secret_store;
pub mod floor_followons;
pub mod holder;
pub mod job_queue_region;
pub mod live_tail;
pub mod log_pipeline;
pub mod log_sink;
pub mod log_sink_durable;
pub mod metering;
pub mod migrations;
pub mod permanent_gates;
pub mod rebac_fragment;
pub mod residency_drill;
pub mod runner_bind;
pub mod schedule_and_run_job;
pub mod scheduler;
pub mod schema;
pub mod secret_admin;
pub mod secret_broker;
pub mod supply_chain;
pub mod surfacing;
pub mod surfacing_index;
pub mod surfacing_store;
pub mod surfacing_tools;
pub mod surge;

pub use ci_pipeline::{CheckFacts, PipelineRun, PipelineStage, RunVerdict, CI_PIPELINE_WF_TYPE};

pub use check_emitter::{
    assemble_check_status, check_status_payload, details_ref, summary_for, CheckAttemptCounter,
    CheckEmitContext, CheckProvider, CheckState, CostPosture, TrustTier, BUMP_CHECK_ATTEMPT_SQL,
};

pub use ci_result_signal::{CiResultSignal, RollupDelivery};

pub use supply_chain::{
    BuildIdentity, KeylessSignature, RekorLog, Sbom, SbomFormat, SlsaProvenance,
    SupplyChainVerifier, VerificationFailure,
};

pub use ci_secret_store::{
    durable_ci_job_secret_resolver, CiSecretStoreError, DurableCiSecretStore,
    DurableSecretCapability,
};
pub use secret_admin::{
    SecretAdmin, SecretAdminError, SecretBindingScope, SecretMaterial, SecretMetadata,
    SECRET_ADMIN_PERMISSION,
};
pub use secret_broker::{
    OidcCredential, ResolvedSecret, SecretBroker, SecretCapability, SecretLaunchError,
    SecretOutcome, SecretResolution, WithheldSecret, WithholdReason, SECRET_READ_PERMISSION,
};

pub use deployment::{
    deploy_outcome_of, deploy_requires_approval, deployment_approval_required_draft,
    deployment_approved_draft, deployment_failed_draft, deployment_rejected_draft,
    deployment_requested_draft, deployment_rolled_back_draft, deployment_started_draft,
    deployment_succeeded_draft, resolve_approvers, DeployGate, DeployGateOutcome, DeployState,
    ENVIRONMENT_APPROVE_PERMISSION,
};

pub use schedule_and_run_job::{complete_job, JobScheduleTerms, SchedulerJobRunner};

pub use metering::{
    meter_resource_seconds, metered_units_for, CiMeter, CostEventRow, CostKind, FlatBpsMarkup,
    MarkupPolicy, Meter, MeteredResource, ReserveSettleParitySignal, INSERT_COST_EVENT_QUERY,
    SELECT_COST_EVENTS_FOR_RUN_QUERY,
};
#[cfg(any(test, feature = "test-support"))]
pub use metering::reserve_settle_parity_drill;

pub use cost_store::{cost_id_for, verify_ci_cost_event_shape, CiCostEventStore, CiCostStoreError};
pub use job_accounting_store::{
    CiJobAccountingError, CiJobAccountingRecord, CiJobAccountingStore, CiJobAccountingWrite,
    CiJobAccountingWriteVersion, CiJobTerminalDisposition, INSERT_CI_JOB_ACCOUNTING_QUERY,
    SELECT_CI_JOB_ACCOUNTING_QUERY,
};

pub use holder::{
    ci_store_classifier, register_ci_holders, CiHolder, CiHolderRegistration, CiStoreClass,
    RestrictionFlag, CI_OLTP_STORE, CI_RESIDUAL_POSTURE_REF, ERASED_OUTCOME_NONE_REMAIN,
};

pub use crypto_shred_erase::{
    drive_ci_d3_erasure_reaches_every_holder, subject_dek_ref, tenant_dek_ref, CiD3Report,
    CiEraseFanOut, CiEraseReceipt, CiErasedTombstone, CiSealedRow, CiShredError,
    CiSubjectFootprint, CI_ERASED_VERB, ERASED_PSEUDONYM,
};

pub use log_pipeline::{
    AnchorStatus, CoalesceBudget, CrossRegionLogWrite, LogAnchorRow, LogAvailablePointer, LogCoord,
    LogPipeline, LogSegmentRow, LogWritePin, SealThreshold, SecretRedactor, CI_LOG_STREAM,
    INSERT_LOG_SEGMENT_QUERY, REDACTION_MARKER, UPSERT_LOG_ANCHOR_QUERY,
};

pub use log_sink::{
    FlushedJobLogs, LogPersist, LogPipelineSink, PRODUCTION_LOG_SEGMENT_MAX_BYTES, SINGLE_STEP_ID,
};
pub use log_sink_durable::DurableLogPersist;

pub use live_tail::{
    parse_step_ref, read_range_from_archive, DetailsRefError, DetailsRefResolver, LiveTail,
    ParsedStepRef, ResumeOutcome, SegmentIndex, SegmentRange, StepByteRange,
};

pub use events::{
    ci_event_tokens, is_durable, register_ci_taxonomy, register_ci_tokens, validate_ci_type_token,
    validate_ci_type_tokens, CiTypeTokenError, CI_DURABLE_TOKENS, CI_FIREHOSE_TOKENS,
    CI_SUBSYSTEM_TOKEN, CI_TYPE_TOKENS,
};

use myelin_substrate::{
    boot, serve, AppSpec, Config, CriticalDependencies, InternalRpc, OutboxSpec, PublicRoutes,
    ServeError, ServeHandle,
};

pub use surfacing::{
    ci_artifact_ref, ci_deployment_ref, ci_pipeline_ref, ci_run_id_colref, ci_run_ref,
    ci_runner_ref, commit_check_ref, compose_run_list_query, lower_over_run_id,
    run_search_pre_filter, run_step_line_ref, run_step_ref, ArtifactStore, AuthzJoin,
    AuthzVisibleIndex, BoundParam, CiArtifactType, CiSearchPreFilter, ComposedRunListQuery,
    DeploymentMeta, LoweredFilter, PipelineMeta, ProjectError, Projected, Projection, Projector,
    RenderHint, RunMeta, SubAnchor, Tombstone, TombstoneReason, AUTHZ_VISIBLE_TABLE, CI_SUBSYSTEM,
    RUN_LIST_PERMISSION, VIEW,
};

#[cfg(any(test, feature = "test-support"))]
pub use e2e_wedge::{
    run_ci_e2e_slices, run_e2e1_pr_context_pane, run_e2e3_spec_to_ship_lineage, E2eArtifact,
    E2E_SCENARIOS,
};

pub use surfacing_index::{
    ci_run_index_spec, ci_summary, register_ci_run_index_spec, register_ci_summary_templates,
    run_doc_is_indexable, summary_template_key, CheckVerdict, CiReindexSource, CiReplayKind,
    CiSummary, CI_RUN_ACL_OBJECT_TYPE, CI_RUN_TYPE, CI_SUMMARY_TEMPLATES,
};
pub use surfacing_tools::{
    ci_effect_kind, ci_required_caps, ci_requires_approval_default, ci_side_effecting, ci_tool_def,
    ci_tool_defs, register_ci_tools, CI_TOOL_NAMES,
};

pub use scheduler::{
    lane_token, state_token, ClaimRequest, Claimed, EnqueueOutcome, JobState, Lane, QueuedJob,
    SchedulerState, AUTHORIZE_JOB_LAUNCH_QUERY, AUTHORIZE_JOB_LAUNCH_V2_QUERY,
    CANCEL_SUPERSEDED_QUERY, CLAIM_QUERY, COMPLETE_JOB_QUERY, HEARTBEAT_QUERY,
    INSERT_JOB_QUEUE_QUERY, REAP_QUERY,
};

pub use job_queue_region::CiRegionQueueStore;
pub use job_queue_store::{
    CiJobLaunchClaim, CiJobQueueStore, DurableEnqueue, JobQueueReaper, JobQueueStoreError,
    LeasedJob, LOCK_JOB_CLAIM_FOR_TOKEN_MINT_QUERY,
};

pub use job_spec_store::{
    CiJobSpecStore, CiJobSpecStoreError, ClaimedDispatchIdentity, DispatchOutcome,
    DurableCiJobLaunchTemplate, INSERT_JOB_SPEC_QUERY, MAX_JOB_TIMEOUT_SECS,
    NON_TERMINAL_NULL_STAGE_JOBS_QUERY, SELECT_JOB_SPEC_IDENTITY_QUERY, SELECT_JOB_SPEC_QUERY,
};

pub use ci_run_region::{
    CiActiveRunCursor, CiActiveRunPage, CiActiveRunRoute, CiRegionRunDiscovery,
    SupersededCiPipelineRun, DISCOVER_ACTIVE_CI_RUNS_QUERY, DISCOVER_QUEUED_CI_RUN_TENANT_QUERY,
    DISCOVER_SUPERSEDED_CI_PIPELINE_RUNS_QUERY, MAX_ACTIVE_CI_RUN_PAGE,
    MAX_SUPERSEDED_CI_PIPELINE_RUN_PROBE, MAX_SUPERSEDED_CI_PIPELINE_RUN_REPORT,
};
pub use ci_run_starter_poller::{
    CiRunStarterBatch, CiRunStarterPollerError, PgCiRunStarterPoller, MAX_CI_RUN_START_BATCH,
};
pub use ci_run_store::{
    CiRunFinalization, CiRunFinalizationJob, CiRunFinalizationOutcome, CiRunFinalizationWrite,
    CiRunFinalizer, CiRunInsert, CiRunRecord, CiRunStore, CiRunStoreError, CiRunTerminalState,
    DurableCiRunFinalizer, FINALIZE_CI_RUN_QUERY, INSERT_CI_RUN_QUERY,
    LOCK_CI_RUN_FOR_FINALIZE_QUERY, LOCK_CI_RUN_FOR_TOKEN_MINT_QUERY,
    SELECT_CI_RUN_ACCOUNTING_QUERY, SELECT_CI_RUN_QUERY, VERIFY_CI_RUN_REPLAY_QUERY,
};
pub use ci_run_supersession::{CiRunSupersessionError, PgCiRunSupersession};
pub use ci_runner_host::{
    wait_for_ci_runner_host_drain_timeout, wait_for_ci_runner_host_failure, CiRunnerHost,
    CiRunnerHostConfig, CiRunnerHostFailure, CiRunnerHostHandle, CI_RUNNER_HOST_DRAIN_TIMEOUT,
    CI_RUNNER_HOST_POLL_INTERVAL,
};
pub use ci_scheduler_db::{
    CiSchedulerDbConfig, CiSchedulerDbError, CiSchedulerDbProvider, CI_SCHEDULER_DATABASE_URL_ENV,
};

#[cfg(any(test, feature = "test-support"))]
pub use runner_bind::durable_spec_resolver_test_support;
pub use runner_bind::{
    durable_spec_resolver, spec_store_unavailable_resolver, CiRunnerLoop, CiRunnerLoopExit,
    DurableLeaseAdapter, DurablePreparationLeaseCheckpoint, JobSpecResolver,
    CI_RUNNER_EXECUTION_LEASE_TTL_SECS,
};

pub use ci_claim_window::{
    claim_window_secs, claim_window_secs_for_template, is_checkout_bearing, CiClaimWindowError,
    CI_CHECKOUT_PARENT_ATTEMPT_EXECUTIONS, CI_EXECUTION_LEASE_HEADROOM_SECS,
    MAX_CI_JOB_CLAIM_WINDOW_SECS,
};

#[cfg(any(test, feature = "test-support"))]
pub use ci_pipeline_driver::{fixed_command_spec_builder, CiPipelineDriver, StartRunError};
pub use ci_pipeline_driver::{
    unresolved_stage_spec_builder, CiJobAccountingPricer, CiJobPricingError, CiPipelineReporter,
    ClaimRefusal, DurableCiJobAccounting, DurableJobRunner, PreparationRetryOutcome,
    PricedCiJobUsage, StageSpecBuilder, TIER_P_OPERATIONAL_PRICING_REVISION,
};
pub use ci_pipeline_reporter_router::{
    CiPipelineReporterFactory, CiPipelineReporterFactoryError, CiPipelineReporterRouter,
    CiPipelineReporterRouterError,
};

pub use fleet::{
    AutoscalePolicy, Autoscaler, BareMetalPxeAdapter, CrossRegionRunnerWrite, EuFleetProvider,
    FleetAdapter, FleetError, FleetEvent, FleetPools, FleetResidencyReport, GenericEuIaasAdapter,
    PoolKey, RunnerWritePin, ScalePlan, COUNT_RUNNERS_BY_POOL_QUERY, DELETE_RUNNER_QUERY,
    INSERT_RUNNER_QUERY,
};

pub use fairness::{
    shed_order, Backpressure, FairShare, PlanTier, ADVANCE_DEFICIT_QUERY, BASE_QUANTUM,
    DEFAULT_TENANT_IN_FLIGHT_CAP, DEFICIT_CEILING, IN_FLIGHT_COUNT_QUERY, REPLENISH_DEFICIT_QUERY,
};

pub use migrations::CI_RUN_SURFACE_INDEX_READINESS;
pub use migrations::{
    ci_controlplane_hot_tables, ci_controlplane_migrations, ci_durable_hot_tables,
    ci_durable_migrations, make_tenant_scoped_ddl, ALTER_CI_JOB_ACCOUNTING_ADD_DISPOSITION_V4_DDL,
    ALTER_CI_JOB_ACCOUNTING_ADD_DISPOSITION_V4_VERDICT_DDL,
    ALTER_CI_JOB_ACCOUNTING_ADD_SKIPPED_DDL, ALTER_CI_JOB_PRELAUNCH_USAGE_ADD_SEAL_DEADLINE_DDL,
    ALTER_CI_JOB_SPEC_ADD_STAGE_DDL, ALTER_CI_RUN_ADD_CAUSAL_PROVENANCE_DDL,
    ALTER_CI_RUN_ADD_CONCURRENCY_GROUP_DDL, ALTER_CI_RUN_ADD_PR_HEAD_GENERATION_DDL,
    ALTER_CI_RUN_ADD_SOURCE_REF_DDL,
    ALTER_JOB_QUEUE_ADD_CLAIM_AUTHORITY_DDL, ALTER_JOB_QUEUE_ADD_CLAIM_TIME_DDL,
    ALTER_JOB_QUEUE_ADD_CLAIM_WINDOW_DDL, ALTER_JOB_QUEUE_ADD_COMPLETION_DDL,
    ALTER_JOB_QUEUE_ADD_RESERVATION_WRITE_VERSION_DDL, ALTER_JOB_QUEUE_ADD_RETRY_ATTEMPTS_DDL,
    ARTIFACT_TABLE, CACHE_ENTRY_TABLE, CHECK_ATTEMPT_TABLE, CI_COST_EVENT_TABLE,
    CI_DRIVE_MANIFEST_TABLE, CI_DURABLE_WRITER_IDS, CI_JOB_ACCOUNTING_DISPOSITION_V4_MIGRATION_ID,
    CI_JOB_ACCOUNTING_DISPOSITION_V4_VERDICT_MIGRATION_ID, CI_JOB_ACCOUNTING_SKIPPED_MIGRATION_ID,
    CI_JOB_ACCOUNTING_TABLE, CI_JOB_QUEUE_CLAIM_AUTHORITY_MIGRATION_ID,
    CI_JOB_QUEUE_CLAIM_TIME_MIGRATION_ID, CI_JOB_QUEUE_CLAIM_WINDOW_MIGRATION_ID,
    CI_JOB_QUEUE_CLAIM_WINDOW_VALIDATE_MIGRATION_ID, CI_JOB_QUEUE_COMPLETION_MIGRATION_ID,
    CI_JOB_QUEUE_RETRY_ATTEMPTS_MIGRATION_ID, CI_JOB_RUN_LEDGER_INDEX,
    CI_JOB_RUN_LEDGER_INDEX_MIGRATION_ID, CI_JOB_RUN_LEDGER_VALIDATION_MIGRATION_ID,
    CI_JOB_SPEC_STAGE_MIGRATION_ID, CI_JOB_SPEC_TABLE, CI_JOB_TABLE,
    CI_PIPELINE_CUTOVER_FENCE_ROW_MIGRATION_ID, CI_PIPELINE_V3_CUTOVER_FENCE_ROW_MIGRATION_ID,
    CI_PIPELINE_VERSION_BACKLOG_PROBE_MIGRATION_ID, CI_REGION_SCHEDULER_RLS_MIGRATION_ID,
    CI_RUN_BRANCH_SCOPE_CONTRACT_MIGRATION_ID, CI_RUN_BRANCH_SCOPE_EXPAND_MIGRATION_ID,
    CI_RUN_BRANCH_SCOPE_VALIDATE_MIGRATION_ID,
    CI_RUN_CAUSAL_PROVENANCE_MIGRATION_ID, CI_RUN_CHECK_ATTEMPT_TABLE,
    CI_RUN_CONCURRENCY_GROUP_MIGRATION_ID, CI_RUN_PR_HEAD_GENERATION_MIGRATION_ID,
    CI_RUN_SOURCE_REF_CONSTRAINT_MIGRATION_ID,
    CI_RUN_SOURCE_REF_CONSTRAINT_VALIDATE_MIGRATION_ID, CI_RUN_SOURCE_REF_MIGRATION_ID,
    CI_RUN_QUEUED_REGION_INDEX, CI_RUN_QUEUED_REGION_INDEX_MIGRATION_ID,
    CI_RUN_SURFACE_REPO_CREATED_INDEX, CI_RUN_SURFACE_REPO_CREATED_INDEX_MIGRATION_ID,
    CI_RUN_TABLE, CI_SCHEDULER_CI_RUN_DISCOVERY_MIGRATION_ID,
    CI_SCHEDULER_CI_WORKFLOW_DISCOVERY_MIGRATION_ID, CI_SCHEDULER_CLAIM_NONCE_GRANT_MIGRATION_ID,
    CI_SCHEDULER_CLAIM_TIME_GRANT_MIGRATION_ID, CI_SCHEDULER_LEASE_EPOCH_GRANT_MIGRATION_ID,
    CI_SECRET_TABLE, CI_SECRET_VERSION_HIGH_WATER_MIGRATION_ID, CI_SECRET_VERSION_HIGH_WATER_TABLE,
    CI_WORKFLOW_ACTIVE_REGION_INDEX_MIGRATION_ID, CREATE_ARTIFACT_DDL, CREATE_CACHE_ENTRY_DDL,
    CREATE_CHECK_ATTEMPT_DDL, CREATE_CI_COST_EVENT_DDL, CREATE_CI_DRIVE_MANIFEST_DDL,
    CREATE_CI_JOB_ACCOUNTING_DDL, CREATE_CI_JOB_DDL, CREATE_CI_JOB_PARENT_ATTEMPT_DDL,
    CREATE_CI_JOB_PRELAUNCH_USAGE_DDL, CREATE_CI_JOB_RUN_LEDGER_INDEX_DDL, CREATE_CI_JOB_SPEC_DDL,
    CREATE_CI_PIPELINE_VERSION_BACKLOG_PROBE_DDL, CREATE_CI_REGION_SCHEDULER_RLS_DDL,
    CREATE_CI_RUN_CHECK_ATTEMPT_DDL, CREATE_CI_RUN_DDL, CREATE_CI_RUN_QUEUED_REGION_INDEX_DDL,
    CREATE_CI_RUN_SURFACE_REPO_CREATED_INDEX_DDL, CREATE_CI_SECRET_DDL,
    CREATE_CI_SECRET_VERSION_HIGH_WATER_DDL, CREATE_DEPLOYMENT_DDL, CREATE_ENVIRONMENT_DDL,
    CREATE_FAIR_DEFICIT_DDL, CREATE_JOB_QUEUE_DDL, CREATE_JOB_QUEUE_INDEXES_DDL,
    CREATE_LOG_ANCHOR_DDL, CREATE_LOG_SEGMENT_DDL, CREATE_RUNNER_DDL, CREATE_SECRET_BINDING_DDL,
    DEPLOYMENT_TABLE, ENVIRONMENT_TABLE, FAIR_DEFICIT_TABLE, GRANT_SCHEDULER_CI_RUN_DISCOVERY_DDL,
    GRANT_SCHEDULER_CLAIM_NONCE_DDL, GRANT_SCHEDULER_CLAIM_TIME_DDL,
    GRANT_SCHEDULER_LEASE_EPOCH_DDL, JOB_QUEUE_TABLE, JQ_CLAIMABLE_INDEX, JQ_IDEM_INDEX,
    JQ_SERIALIZE_INDEX, LOG_ANCHOR_TABLE, LOG_SEGMENT_TABLE, RUNNER_TABLE, SECRET_BINDING_TABLE,
    SEED_CI_PIPELINE_CUTOVER_FENCE_ROW_DDL, SEED_CI_PIPELINE_V3_CUTOVER_FENCE_ROW_DDL,
    VALIDATE_CI_JOB_RUN_LEDGER_INDEX_DDL, VALIDATE_JOB_QUEUE_CLAIM_WINDOW_DDL,
};

pub use permanent_gates::{
    ci_restore_verify_stores, m4_boundary_permanent_gates, run_ci_restore_verify_or_fail,
    PermanentGate, PermanentGateKind,
};

pub use floor_followons::{
    all_floor_followons, FloorFollowOn, TriggerStatus, DEFERRED_BY_REFERENCE_FLOORS,
    MEASURED_TRIGGER_FLOORS,
};

pub use surge::{
    drive_ci_d2_surge, CiDispatchShed, CiSurgeControls, CiSurgeGate, CiSurgeReport,
    StarvationHistogram, CI_SURGE_MULTIPLIER,
};

pub use residency_drill::{
    drive_ci_d10_self_hosted_boundary, drive_ci_r3_residency, CellJob, CiD10Report, CiR3Report,
    CiStoreResidency,
};

pub const SERVICE_NAME: &str = "ci-controlplane";

fn controlplane_critical() -> CriticalDependencies {
    CriticalDependencies::new(["broker", "authz", "runner_pool"])
}

pub fn controlplane_app_spec(config: Config, outbox: myelin_events::OutboxStore) -> AppSpec {
    AppSpec {
        name: SERVICE_NAME,
        config,
        migrations: ci_controlplane_migrations(),
        hot_tables: ci_controlplane_hot_tables(),
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        consumers: Vec::new(),
        holders: AppSpec::auto(),
        stores: myelin_substrate::StoreManifest::new(),
        outbox: OutboxSpec::external_relay(outbox),
        critical: controlplane_critical(),
    }
}

pub fn boot_controlplane(
    config: Config,
    outbox: myelin_events::OutboxStore,
) -> Result<ServeHandle, ServeError> {
    boot(controlplane_app_spec(config, outbox))
}

pub fn run_controlplane(
    config: Config,
    outbox: myelin_events::OutboxStore,
) -> Result<(), ServeError> {
    serve(controlplane_app_spec(config, outbox))
}

pub async fn run_controlplane_until_shutdown<F>(
    config: Config,
    outbox: myelin_events::OutboxStore,
    shutdown: F,
) -> Result<(), ServeError>
where
    F: std::future::Future<Output = ()>,
{
    myelin_substrate::serve_until_shutdown(controlplane_app_spec(config, outbox), shutdown).await
}

pub fn ci_cost_event_store(pool: sqlx::PgPool, region: myelin_tenancy::Region) -> CiCostEventStore {
    CiCostEventStore::with_pg(pool, region)
}

pub fn ci_job_accounting_store(
    pool: sqlx::PgPool,
    region: myelin_tenancy::Region,
) -> CiJobAccountingStore {
    CiJobAccountingStore::with_pg_and_write_version(
        pool,
        region,
        ci_pipeline_protocol::PRODUCTION_ACCOUNTING_WRITE_VERSION,
    )
}

pub fn ci_job_queue_store(pool: sqlx::PgPool) -> CiJobQueueStore {
    CiJobQueueStore::with_pg(pool)
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn ci_region_queue_store_test_support(pool: sqlx::PgPool) -> CiRegionQueueStore {
    CiRegionQueueStore::with_pg(pool)
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn ci_region_run_discovery_test_support(pool: sqlx::PgPool) -> CiRegionRunDiscovery {
    CiRegionRunDiscovery::with_pg(pool)
}

pub fn ci_job_spec_store(pool: sqlx::PgPool) -> CiJobSpecStore {
    CiJobSpecStore::with_pg(pool)
}

pub fn ci_run_store_factory(pool: sqlx::PgPool) -> CiRunStore {
    CiRunStore::with_pg(pool)
}

pub fn ci_run_starter_factory(
    pool: sqlx::PgPool,
    region: myelin_tenancy::Region,
    blobs: std::sync::Arc<dyn myelin_storage::BlobStore + Send + Sync>,
    rt: tokio::runtime::Handle,
    supersession_ledger: myelin_storage::DurableCostLedger,
) -> Result<PgCiRunStarterFactory, CiLaunchAuthorityError> {
    let reservations = ci_launch_authority::PgTierPCiJobBudgetReservation::new(
        pool.clone(),
        region.0.clone(),
        ci_launch_authority::TIER_P_OPERATIONAL_ACTIVE_RESERVATION_CEILING,
        ci_launch_authority::CiAttemptBudgetPolicy::production(),
        ci_pipeline_protocol::PRODUCTION_RESERVATION_WRITE_VERSION,
    )?;
    Ok(PgCiRunStarterFactory::new_with_authority_and_supersession(
        pool,
        rt,
        std::sync::Arc::new(myelin_events::MonotonicMinter::new()),
        region,
        blobs,
        std::sync::Arc::new(ci_launch_authority::LinuxSmallV1LaunchAuthority::new(
            std::sync::Arc::new(reservations),
        )),
        supersession_ledger,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_substrate::{Liveness, Surface};

    #[test]
    fn controlplane_boots_from_serve_appspec_with_three_ports() {
        let handle = boot_controlplane(Config::default(), myelin_events::OutboxStore::new())
            .expect("the CI Control Plane shell boots from serve(AppSpec)");
        assert_eq!(handle.name(), SERVICE_NAME, "the deployable service name");

        assert_eq!(
            handle.surfaces(),
            &[Surface::Public, Surface::Internal, Surface::MetricsHealth],
            "the three ports opened (contract 1.2)"
        );

        let mh = handle.metrics_health();
        assert_eq!(
            mh.liveness(),
            Liveness::Up,
            "liveness = not-wedged (never checks a dependency)"
        );
        assert!(
            mh.readiness().is_ready(),
            "readiness = can-serve-now (all critical deps healthy at boot) - distinct from liveness"
        );
    }

    #[test]
    fn dead_runner_pool_flips_readiness_not_liveness() {
        let handle =
            boot_controlplane(Config::default(), myelin_events::OutboxStore::new()).expect("boot");
        let mh = handle.metrics_health();
        assert!(
            mh.readiness().is_ready(),
            "ready while the runner pool is healthy"
        );

        handle.health_probe().mark_down("runner_pool");

        assert!(
            !mh.readiness().is_ready(),
            "no healthy runner pool → not-ready + shed (arch 00 §4)"
        );
        assert_eq!(
            mh.liveness(),
            Liveness::Up,
            "liveness stays UP (not-ready is NOT not-alive - no restart storm)"
        );

        let handle2 =
            boot_controlplane(Config::default(), myelin_events::OutboxStore::new()).expect("boot");
        handle2.health_probe().mark_down("authz");
        assert!(
            !handle2.metrics_health().readiness().is_ready(),
            "a dead authz also flips readiness (the trust/visibility decision dependency)"
        );
    }

    #[test]
    fn run_controlplane_runs_lifecycle_and_returns_ok() {
        assert_eq!(
            run_controlplane(Config::default(), myelin_events::OutboxStore::new()),
            Ok(()),
            "the CI Control Plane shell boots → … → drains cleanly"
        );
    }

    #[tokio::test]
    async fn production_controlplane_waits_for_shutdown_then_drains() {
        assert_eq!(
            run_controlplane_until_shutdown(
                Config::default(),
                myelin_events::OutboxStore::new(),
                async {},
            )
            .await,
            Ok(())
        );
    }

    #[test]
    fn failed_boot_returns_non_zero() {
        let r = run_controlplane(Config("BAD_POOL".into()), myelin_events::OutboxStore::new());
        assert!(r.is_err(), "a failed boot must return non-zero (Err)");
        assert!(
            r.unwrap_err().0.contains("fail-fast"),
            "the error names the §3.2 fail-fast validation"
        );
    }

    #[test]
    fn the_shell_carries_the_complete_data_model_and_no_consumers() {
        let spec = controlplane_app_spec(Config::default(), myelin_events::OutboxStore::new());
        assert_eq!(
            spec.migrations.0.len(),
            76,
            "the complete CI schema and every append-only provenance follow-on are present"
        );
        assert!(
            spec.consumers.is_empty(),
            "no consumers at the shell (the scheduler is not a bus consumer; dedup is the dispatch shell)"
        );
        for t in [
            JOB_QUEUE_TABLE,
            LOG_SEGMENT_TABLE,
            CI_COST_EVENT_TABLE,
            CHECK_ATTEMPT_TABLE,
            CI_RUN_CHECK_ATTEMPT_TABLE,
        ] {
            assert!(spec.hot_tables.is_hot(t), "`{t}` is declared hot");
        }
        let deps: Vec<&str> = spec.critical.deps().iter().map(|d| d.0.as_str()).collect();
        assert!(deps.contains(&"broker"), "broker is critical");
        assert!(deps.contains(&"authz"), "authz is critical");
        assert!(deps.contains(&"runner_pool"), "runner_pool is critical");
    }
}

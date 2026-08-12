pub mod agent_journal_privacy;
pub mod agent_model_step;
pub mod agent_run_gate;
pub mod agent_tool_effect;
pub mod agent_trace_durable;
pub mod agent_trigger_durable;
pub mod agent_wallet;
pub mod backup;
pub mod blob;
pub mod bus_shred;
pub mod cache;
pub mod cdn;
pub mod cell_migration;
pub mod ci_cache_scope;
#[cfg(any(test, feature = "test-support"))]
pub mod ci_log_index;
pub mod coloc;
#[cfg(any(test, feature = "test-support"))]
pub mod e2e3_reindex_parity;
pub mod encryption;
pub mod erase;
#[cfg(any(test, feature = "test-support"))]
pub mod firehose_archive;
pub mod gd4;
pub mod git_shred;
pub mod gitpack;
pub mod holder;
pub mod holder_fanout;
pub mod key_origin;
pub mod kms;
pub mod kms_failstatic;
pub mod migration;
pub mod migration_under_load;
pub mod mirror;
pub mod money;
pub mod multi_cell_erase;
pub mod object_packs;
pub mod olap;
pub mod olap_feed;
pub mod olap_restrict;
pub mod oltp;
pub mod reerase;
pub mod replicated_blob;
pub mod reserve_settle;
pub mod residency;
pub mod restore;
pub mod restore_verify;
pub mod rls;
pub mod storage_surge;

pub mod authz_projection_durable;
pub mod backend;
pub mod delegation_policy_durable;
pub mod elected_relay;
pub mod events_durable;
#[cfg(feature = "integration")]
pub mod events_serve;
pub mod external_agent_run_durable;
pub mod hitl_gate_durable;
pub mod identity_durable;
pub mod kms_durable;
pub mod outbox_durable;
pub mod pg;
pub mod pg_migrator;
pub mod pgrelay;
pub mod placement_durable;
pub mod provider;
pub mod pseudonym_durable;
pub mod reerase_durable;
pub mod reserve_settle_durable;
pub mod restore_verify_durable;
pub mod s3blob;
pub mod tenant_tx;
pub mod valkey;

pub mod cell_root_durable;

pub use agent_journal_privacy::{
    agent_journal_privacy_migrations, AGENT_JOURNAL_ENCRYPTION_MIGRATION,
    AGENT_JOURNAL_SUBJECT_MIGRATION,
};
pub use agent_model_step::{
    agent_model_step_migrations, AgentModelStepStore, ModelStepBegin, ModelStepCompletion,
    ModelStepError, AGENT_MODEL_STEP_GUARD_MIGRATION, AGENT_MODEL_STEP_MIGRATION,
};
pub use agent_run_gate::{AgentRunGate, AgentRunGateSignal, DispatchError, InFlightRun, RunKind};
pub use agent_tool_effect::{
    agent_tool_effect_migrations, AgentToolEffectStore, ToolEffectBegin, ToolEffectCompletion,
    ToolEffectError, AGENT_TOOL_EFFECT_GUARD_MIGRATION, AGENT_TOOL_EFFECT_MIGRATION,
};
pub use agent_trace_durable::{
    agent_trace_durable_migrations, agent_trace_encrypted_only_migrations,
    agent_trace_erasure_progress_migrations, AgentTraceAvailability, AgentTraceEraseReceipt,
    AgentTraceError, AgentTraceReceipt, AgentTraceResult, AgentTraceSubjectEraseReceipt,
    AgentTraceSubjectState, AgentTraceSubjectSummary, AgentTraceWrite, AgentTraceWriter,
    DurableAgentTraceStore, EraseAgentTraceOutcome, InMemoryAgentTraceStore,
    AGENT_TRACE_ENCRYPTED_ONLY_MIGRATION, AGENT_TRACE_ERASURE_PROGRESS_MIGRATION,
};
pub use agent_wallet::{
    agent_wallet_charge_migrations, agent_wallet_migrations, AgentWallet, CreditKind, DebitOutcome,
    WalletError, AGENT_WALLET_CHARGE_KEY_MIGRATION, AGENT_WALLET_MIGRATION,
};
pub use backup::{
    BackupError, BackupSet, BaseBackup, ContinuousArchiver, EpochSecs, LogTierSeal,
    ObjectTierBackup, ObjectVersion, StoreTier, WalOffset, WalSegment,
};
#[cfg(any(test, feature = "test-support"))]
pub use blob::FsBlobStore;
pub use blob::{
    BlobError, BlobMeta, BlobStore, BlobTelemetry, ContentHash, ContentWrap, HashAlgo, IdentityWrap,
};
pub use bus_shred::KmsBusShredder;
pub use cache::{Cache, CacheError, InMemoryCache};
pub use cdn::{CdnCloneClass, CdnEdgePop, CdnEdgeSet};
pub use cell_migration::{
    is_cell_local, migrate_cell_to_cell, storage_resolves_locally, CellMigrationError,
    CellMigrationReceipt, CellMigrationRequest, CellTenantTiers,
};
pub use ci_cache_scope::{
    CacheScope, CacheScopeError, CacheScopeTelemetry, CiCacheNamespace, TrustTier,
};
#[cfg(any(test, feature = "test-support"))]
pub use ci_log_index::{
    CiLogError, CiLogFrame, CiLogIndex, CiLogTier, SegmentKeying, StepAnchor, StepSpan,
    CI_LOG_STREAM,
};
pub use coloc::{ColocError, ColocatedOltp, ColocatedTx, COLOCATED_OUTBOX_MIGRATION};
#[cfg(any(test, feature = "test-support"))]
pub use e2e3_reindex_parity::{
    run_e2e3_storage_half, DerivedReindexSource, DerivedStoreClass, DerivedStoreParity,
    E2e3StorageArtifact,
};
pub use encryption::{
    key_class_for, ColumnCryptor, DekContentWrap, EncryptedColumn, KeyChoiceError, SubjectId,
};
pub use erase::{
    BlobShredReach, BusErase, CryptoShredErase, EpochMillis, EraseError, EraseHolders,
    ErasureLedgerSink, ErasureReceipt, PseudonymShred, RefsTombstone, SearchPurge,
};
#[cfg(any(test, feature = "test-support"))]
pub use firehose_archive::{
    segment_pointer_draft, ArchiveError, ArchiveTelemetry, FirehoseArchiver, SealedSegment,
    SegmentBytes,
};
pub use gd4::{
    assert_gd4_table_complete, assert_no_local_residual_statement, granularity_of_key_class,
    key_choice_granularity, structural_reach_uses_erase_seams, DataClass, Gd4TableReport,
    KeyGranularity, StructuralErasureFloor, StructuralFloorReport, RESIDUAL_POSTURE_REF,
};
pub use git_shred::{GitCryptoShredReach, GitResidual, GitShredReceipt, GitShreddable};
pub use gitpack::{
    git_object_address, GitObjectKind, GitPackError, GitPackTier, PackManifest, PlacementError,
    RepoGitPlacement, RepoId, RepoPlacementStatus, StorageGroup, GIT_PACKFILE_MAX_STORED_BYTES,
    GIT_PACK_OBJECT_MAX_STORED_BYTES,
};
pub use holder::{register_holder, BlobStoreHolder, OltpHolderRegistration, OltpStoreHolder};
pub use holder_fanout::{
    holder_ids_not_covered, FullHolderFanOut, HolderClass, HolderCoverage,
    HolderCoverageCertificate, HolderCoverageReceiptSet, HolderErasure, ResidualPosture,
};
pub use key_origin::{
    Byok, Dek, Hyok, HyokKeyService, HyokServiceDenied, IndexAdmission, KeyId, KeyOrigin,
    KeyOriginError, KeyOriginKind, KeyOriginTelemetry, PlatformManaged,
};
pub use kms::{
    CellRoot, DekHandle, DekId, ExportedKek, KekId, KeyClass, KmsAdapter, KmsDurableSnapshot,
    KmsEngine, KmsError, PiiKeyRef, SealKey, SealKeyError, SealedRoot, WrappedDek, KEY_LEN,
    NONCE_LEN,
};
pub use kms_failstatic::{
    KmsFailStaticSignals, KmsReadError, KmsReadPath, KmsReadResult, KmsReadiness,
};
pub use migration::{
    is_blocking_alter, is_destructive, HotTables, Migration, MigrationError, MigrationPhase,
    Migrations, OnlineMigrationRunner, PhaseProgress,
};
pub use migration_under_load::{
    lock_cost_ms, LockBudget, LockClass, MigrationLoadArtifact, MigrationLoadFailure,
    MigrationLoadVerdict, MigrationUnderLoad, StepLockMeasure, WriteLoad,
};
pub use mirror::{MirrorTelemetry, PushMirrorClass, PushMirrorTarget};
pub use money::MicroUsd;
pub use multi_cell_erase::{
    CellEraseContext, CellEraseReceipt, MultiCellEraseFanOut, MultiCellEraseReceiptSet,
};
pub use object_packs::{
    cdn_over_object_backing, object_backed_pack_tier, place_repo_object_backed,
    served_from_object_tier, CloneStormLoad, GitD4Ceiling, GitD4Report, ObjectBackedServe,
    SingleNodeServe,
};
pub use olap::{
    OlapApply, OlapDoc, OlapEvent, OlapFrameSignal, OlapIngestError, OlapReadStore, OlapStoreHolder,
};
pub use olap_feed::{
    reindex_olap_from_bus, OlapAnalyticsSource, OlapBusConsumer, OlapReindexParitySignal,
};
pub use olap_restrict::{
    AnalyticsAggregate, AnalyticsEligibility, OlapAnalytics, RestrictionGateSignal,
    RestrictionLeakAudit,
};
pub use oltp::{OltpConfig, OltpError, OltpPool, PermitGuard};
#[cfg(any(test, feature = "test-support"))]
pub use reerase::InMemoryPostPitLedger;
pub use reerase::{
    CellKillRestore, CellKillRtoReport, ErasureRecord, PostRestoreErasureLedger, ReErasePass,
    ReEraseReport, ReErasedSubject, RtoGrain,
};
pub use replicated_blob::{ReplicaTelemetry, ReplicatedBlobStore};
pub use reserve_settle::{
    CostEvent, CostLedger, LedgerUnavailable, MeteredUnit, Reservation, ReservationState,
    ReserveError, ReserveSettleSignal, RunId, SettleError, SettleOutcome,
};
pub use residency::{
    verify_region_pinning, RegionPinnedStore, RegionPinningAttestation, ResidencyStoreClass,
    ResidencyVerifySignal, ResidencyViolation, StoreResidencyReport, StoreSet,
};
pub use restore::{
    restore_to_offset, restored_key_counts, BlobPresence, ReindexFromSource, RestoreError,
    RestoreReport, SourceEvent, SourceLog, WalRow,
};
pub use restore_verify::{
    ErasureLedger, GateFailure, GateInputs, GateVerdict, GreenArtifact, RestoreTarget,
    RestoreVerifyGate, RestoredObject,
};
pub use rls::{RlsError, TenantQuery, TenantScope, TenantTable};
pub use storage_surge::{
    run_storage_lane_surge, StorageAdmission, StorageLaneBudget, StorageLaneClass, StorageLaneGate,
    StorageSurgeReport, STORAGE_SURGE_MULTIPLIER,
};

pub use agent_trigger_durable::{
    agent_trigger_durable_migrations, agent_trigger_evaluation_diagnostic_migrations,
    agent_trigger_owner_list_migrations, agent_trigger_terminal_reason_migrations,
    AgentTriggerApprovalDecision, AgentTriggerApprovalOutcome, AgentTriggerCapacityScope,
    AgentTriggerClaimRequest, AgentTriggerEvaluationDiagnostic, AgentTriggerEvaluationErrorCode,
    AgentTriggerFiringState, AgentTriggerLifecycleAction, AgentTriggerLifecycleOutcome,
    AgentTriggerStartRequest, ChangeAgentTriggerApprovalOutcome,
    ChangeAgentTriggerLifecycleOutcome, ClaimedAgentTriggerFiring,
    CreateAgentTriggerBindingOutcome, DurableAgentTriggerBacking, DurableAgentTriggerBinding,
    DurableAgentTriggerFiring, NewAgentTriggerBinding, ReserveAgentTriggerFiringOutcome,
    ReservedAgentTriggerFiring, StartAgentTriggerFiringOutcome, StartedAgentTriggerRun,
    TerminalizeAgentTriggerClaimOutcome, AGENT_TRIGGER_APPROVAL_MIGRATION,
    AGENT_TRIGGER_BUDGET_MIGRATION, AGENT_TRIGGER_CLAIM_MIGRATION,
    AGENT_TRIGGER_EVALUATION_DIAGNOSTIC_MIGRATION, AGENT_TRIGGER_MIGRATION,
    AGENT_TRIGGER_OWNER_LIST_MIGRATION, AGENT_TRIGGER_RLS_POLICY, AGENT_TRIGGER_RUN_MIGRATION,
    AGENT_TRIGGER_TERMINAL_REASON_MIGRATION, MAX_ACTIVE_AGENT_TRIGGERS_PER_EVENT,
    MAX_ACTIVE_AGENT_TRIGGERS_PER_OWNER_EVENT, MAX_AGENT_TRIGGER_BUDGET_MINOR_UNITS,
    MIN_AGENT_TRIGGER_BUDGET_MINOR_UNITS,
};
pub use authz_projection_durable::{
    authz_projection_durable_migrations, AUTHZ_PROJECTION_STATE_MIGRATION,
};
pub use delegation_policy_durable::{
    delegation_policy_durable_migrations, ensure_agent_policy_bundle_on_conn,
    DurableDelegationPolicyBacking, DurableDelegationPolicyBundle, DurableDelegationPolicyError,
    DurableDelegationPolicyHeadCursor, DurableDelegationPolicyRevisions,
    DurableDelegationPolicySnapshot, DurableDelegationPolicyVersions,
};
pub use events_durable::{
    bus_erasure_durable_migrations, consumer_dead_letter_migrations,
    consumer_delivery_quarantine_migrations, DurableBusErasureBacking, DurableDeadLetterBacking,
    DurableDedupBacking, DurableDeliveryQuarantineBacking, BUS_ERASURE_LEDGER_MIGRATION,
};
#[cfg(feature = "integration")]
pub use events_serve::{EventsRuntime, EventsServeError, DEFAULT_DRAIN_BATCH};
pub use external_agent_run_durable::{
    external_agent_run_durable_migrations, ClaimedExternalAgentRun, DurableExternalAgentRun,
    DurableExternalAgentRunBacking, ExternalAgentRunState, AGENT_LIFECYCLE_COMMAND_MIGRATION,
    AGENT_LIFECYCLE_COMMAND_RLS_POLICY, EXTERNAL_AGENT_RUN_MIGRATION,
    EXTERNAL_AGENT_RUN_RLS_POLICY,
};
pub use identity_durable::{
    auth_replay_durable_migrations, identity_agent_durable_migrations, identity_durable_migrations,
    identity_project_durable_migrations, DurablePrincipalBacking, DurablePrincipalRow,
    DurableProfileBlob, DurableReplayBacking, DurableRevocationBacking, DurableRevocationRow,
    DurableTupleBacking, TupleEdgeOp,
};
pub use kms_durable::{
    kms_durable_migrations, seal_key_from_env, DurableKmsBacking, KmsDurableError,
    KMS_SEALED_ROOT_MIGRATION, KMS_WRAPPED_DEK_MIGRATION, KMS_WRAPPED_KEK_MIGRATION, SEAL_KEY_ENV,
};
pub use outbox_durable::PgOutboxBacking;
pub use pg::{PgError, PgStore};
pub use pg_migrator::{with_migration_lock, PgMigrator, MIGRATION_LOCK_KEY};
pub use placement_durable::{
    placement_durable_migrations, DurableCellProvisioningRow, DurableCellRow,
    DurableLocalTenantRow, DurableMisrouteAuditBacking, DurableMisrouteRecord,
    DurablePlacementBacking, DurablePlacementRow, DurableRepoPlacementRow, PlacementWriteError,
};
pub use provider::{
    all_durable_migrations, durable_migration_groups, foundation_migrations, BootstrapError,
    IndexReadinessSpec, PgBootstrap, ProviderError, SubstrateProvider, DEFAULT_MAX_CONNECTIONS,
};
pub use pseudonym_durable::{
    pseudonym_durable_migrations, DurableErasureLedgerBacking, DurableErasureLedgerRow,
    DurablePseudonymBacking, DurablePseudonymRow,
};
pub use reerase_durable::{
    post_pit_durable_migrations, DurablePostPitLedger, POST_PIT_ERASURE_LEDGER_MIGRATION,
};
pub use reserve_settle_durable::{
    reserve_settle_durable_migrations, DurableCostLedger, DurableSettleError, COST_LEDGER_MIGRATION,
};
pub use restore_verify_durable::{
    restore_verify_durable_migrations, DurableRestoreErasureLedger,
    RESTORE_ERASURE_LEDGER_MIGRATION,
};
pub use tenant_tx::{
    connect_pool_with_reset, with_tenant_repeatable_read_tx, with_tenant_tx, with_tenant_tx_error,
    TxScope, TypedTxScope,
};

pub use cell_root_durable::{
    cell_root_durable_migrations, CellRootError, CellRootMaterial, DurableCellRootBacking,
    CELL_TOKEN_ROOT_MIGRATION,
};

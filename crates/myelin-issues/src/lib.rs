#![forbid(unsafe_code)]

pub mod agent_spend;
pub mod api;
pub mod app;
pub mod board_sync;
pub mod ci_guard;
pub mod content;
pub mod cost_bounder;
pub mod cross_cell_rollup;
pub mod declares;
pub mod dek;
pub mod dogfood;
#[cfg(any(test, feature = "test-support"))]
pub mod e2e_flagship;
pub mod e2e_lineage;
pub mod e2e_wedge;
pub mod events;
pub mod floor_triggers;
pub mod governance;
pub mod holder;
pub mod holder_erase;
pub mod holder_intent;
pub mod import;
pub mod keys;
pub mod migrations;
pub mod move_crdt;
pub mod my_work;
pub mod olap_feed;
pub mod pg_issue_store;
pub mod planner;
pub mod projection_feeder;
pub mod pseudonym;
pub mod query_coown;
pub mod rebac_fragment;
pub mod reflexes;
pub mod refs_glue;
pub mod reorder;
pub mod replay;
pub mod rollup;
pub mod schema;
pub mod schemes;
pub mod sla_calendar;
pub mod sla_escalation;
pub mod surge;
pub mod switch_test;
pub mod time_axis;
pub mod trigger;
pub mod views;
pub mod workflow;
pub mod write_path;

pub use replay::{IssueReindexSource, IssueReplayKind};
pub use surge::{
    open_surge_gate_from_thresholds, run_iss_d2_cell_scale, run_issues_owner_surge,
    IssD2CellScaleReport, IssuesOwnerShed, IssuesOwnerSurgeReport, ISSUES_SURGE_MULTIPLIER,
};

pub use app::{
    boot_issues, issues_app_spec, run_issues, run_issues_until_shutdown, SERVICE_NAME,
};
pub use holder::{
    issue_store_classifier, register_issue_holders, IssueHolder, IssueHolderRegistration,
    IssueStoreClass, RestrictionFlag, ISSUE_OLTP_STORE, ISSUE_RESIDUAL_POSTURE_REF,
};
pub use migrations::{
    issues_hot_tables, issues_migrations, make_tenant_scoped_ddl, CONSUMER_DEDUP_TABLE,
    CREATE_CONSUMER_DEDUP_DDL, CREATE_CYCLE_DDL, CREATE_CYCLE_MEMBERSHIP_DDL,
    CREATE_ISSUE_AUTHZ_BINDING_DDL, CREATE_ISSUE_AUTHZ_INVALIDATION_TRIGGERS_DDL,
    CREATE_ISSUE_AUTHZ_VISIBLE_DDL, CREATE_ISSUE_CHANGE_LOG_DDL, CREATE_ISSUE_DDL,
    CREATE_ISSUE_INDEXES_DDL, CREATE_ISSUE_KEY_PREFIX_LIST_INDEX_DDL,
    CREATE_ISSUE_RECENT_LIST_INDEX_DDL, CREATE_ISSUE_RELATION_DDL,
    CREATE_ISSUE_RELATION_INDEXES_DDL, CREATE_MILESTONE_DDL, CREATE_PREFIX_COUNTER_DDL,
    CREATE_SCHEME_ASSIGNMENT_DDL, CREATE_SCHEME_DDL, CYCLE_MEMBERSHIP_TABLE, CYCLE_TABLE,
    EXPAND_ISSUE_AUTHZ_CREATED_EVENT_DDL, ISSUE_ASSIGNEE_INDEX, ISSUE_AUTHZ_BINDING_TABLE,
    ISSUE_AUTHZ_VISIBLE_TABLE, ISSUE_BOARD_INDEX, ISSUE_CHANGE_LOG_TABLE, ISSUE_CYCLE_INDEX,
    ISSUE_KEY_PREFIX_LIST_INDEX, ISSUE_PARENT_INDEX, ISSUE_PROPS_GIN_INDEX,
    ISSUE_RECENT_LIST_INDEX, ISSUE_RELATION_TABLE, ISSUE_ROADMAP_INDEX, ISSUE_TABLE, MILESTONE_TABLE,
    OUTBOX_TABLE, PREFIX_COUNTER_TABLE, SCHEME_ASSIGNMENT_TABLE, SCHEME_TABLE,
};
pub use pg_issue_store::{
    is_canonical_request_event_id, CreateIssue, IssueAuthorizationBinding,
    IssueAuthorizationOutcome, IssueAuthorizationState, IssueAuthorizationStatus, IssueAuthorizer,
    IssueCreationReceipt, IssuePage, IssuePageRequest, IssuePermission, IssueStoreError,
    IssueTupleWriter, IssueViewProjectionRevision, PgIssueStore, StoredIssue, VisibleIssues,
};

pub use write_path::{
    apply_mutation, issue_aggregate_key, issue_ref, IssueDraft, MutationKind, WriteError,
    WriteOutcome, PERM_COMMENT, PERM_MANAGE, PERM_PERFORM_TRANSITION, PERM_TRANSITION,
};

pub use dek::{
    decrypt_free_text, encrypt_free_text, plaintext_at_rest, subject_dek_erasure, IssueFreeText,
};
pub use pseudonym::{
    is_raw_principal_id, is_resolvable_pseudonym, pseudonymise, IssuePseudonym, PseudonymError,
};
pub use write_path::{apply_mutation_sealed, SealError, SealedCreate};

pub use keys::{
    render_display_key, CanonicalKey, HiLoKeyAllocator, InMemoryPrefixCounter, PrefixReserve,
    ReserveError, ReservedBlock, INITIAL_BLOCK_SIZE, MAX_BLOCK_SIZE,
};
pub use write_path::create_issue;

pub use reorder::{
    cmp_ranked, rebalance, reorder, same_displayed_sequence, BoardRanking, RankedIssue,
    ReorderError, ReorderOutcome, ReorderRequest,
};

pub use move_crdt::{MoveCrdtBoard, MoveCrdtError, MoveCrdtFloors, ReorderPressure};

pub use cross_cell_rollup::{
    CellLocalRollupResolver, CrossCellDsrFanout, CrossCellPortfolioRollup, CrossCellRollupFloors,
    CrossCellRollupPointer, DsrCellReceipt, PortfolioProjection,
};

pub use floor_triggers::{
    ColumnStoreTrigger, DistributedSqlTrigger, Iss32FloorRegister, MaterialisedRollupTrigger,
    MonteCarloForecastTrigger,
};

pub use content::{
    emit_content_event, is_issue_block, paragraph_body, roundtrips_md, validate_subtree,
    CasConflict, ContentError, ContentKind, IssueContent, SubsetError, ISSUES_EXCLUDED_BLOCKS,
};

pub use schemes::{
    add_flexible_field, org_default_scheme_id, resolve, specificity_rank, FlexibleField,
    FlexibleFieldWrite, IndexPosture, Reassignment, ResolveContext, ResolveKey, Scheme,
    SchemeAssignment, SchemeKind, SchemeResolver, TypeDef, TypeSchemeBody,
};

pub use workflow::{
    arm_trigger_body, blocked_by_guard, example_arm_trigger, linked_pr_ci_green_guard,
    ArmedTrigger, GuardVar, IssueContext, PostAction, StateCategory, TransitionBlocked,
    TransitionPlan, Workflow, WorkflowError, WorkflowGuard, WorkflowState, WorkflowTransition,
};

pub use ci_guard::{
    bind_linked_pr_ctx, ci_done_guard, plan_agent_ci_gated_transition, plan_ci_gated_transition,
    AgentTransitionOutcome, LinkedPrCheck, CHECK_STATE_NEUTRAL, CHECK_STATE_SUCCESS,
    TRUST_TIER_TRUSTED, TRUST_TIER_UNTRUSTED_FORK,
};

pub use planner::{
    compose_board_query, issue_id_colref, lower_over_issue_id, AuthzJoin, AuthzVisibleIndex,
    BoundParam, ComposedBoardQuery, FilterMode, LoweredFilter, AUTHZ_VISIBLE_TABLE,
    ISSUE_VIEW_PERMISSION,
};

pub use cost_bounder::{
    classify_field, estimate_cost, lower_acl, plan_board_query, BoundedBoardQuery,
    CostBounderFloors, CostBudget, FacetCatalog, PlanOutcome, RefineHint, SearchEscalation, Tier,
    TIER3_FIELDS, TYPED_CORE_FIELDS,
};

pub use views::{
    board_and_roadmap_share_row, edit_on_board_reflects_on_roadmap, type_rank_split_is_partition,
    IssueView, RowProjection, ViewFloors, BOARD_TYPE_RANK_MAX, CYCLE_FIELD, ORDER_KEY_FIELD,
    ROADMAP_TYPE_RANK_MIN, STATE_CATEGORY_FIELD, TYPE_RANK_FIELD,
};

pub use board_sync::{
    board_stream, BoardCache, BoardCard, BoardOp, BoardSync, BoardSyncFloors, LocalMutationError,
    BOARD_FIREHOSE_STREAM_PREFIX,
};

pub use governance::{
    simulate_breach, workflow_unreachable_states, BreachSimulation, GovernanceFloors,
    GovernanceView, GovernanceViewModel, GuardLanguage, InspectorAnswer, PermissionInspector,
    PermissionResolver,
};

pub use rollup::{
    aggregate_snapshot, recompute_incremental, rollup_recomputed_draft, walk_parent_edges,
    DebounceCoalescer, DebounceWindow, LeafFact, RecomputeOutcome, RollupAggregate, RollupConsumer,
    RollupFloors, RollupStore,
};

pub use holder_erase::{
    store_classes_reached_by_free_text_shred, EraseFanoutError, HolderReceipt, HolderTarget,
    IssueEraseFanout, IssueEraseOutcome, IssueErasedSubject, IssueErasureLedger,
    IssueReErasureReceipt, ERASED_TOMBSTONE_TOKENS,
};

pub use olap_feed::{
    issue_analytics_aggregate_names, IssueOlapAnalytics, IssueOlapConsumer, IssueOlapFeedFloors,
    IssueOlapFeedSignal, IssueRestrictionLeakAudit, ReindexCtx, ISSUE_ANALYTICS_OLAP,
};

pub use import::{
    adapter_for, AdfBodyNode, CanonicalImport, CanonicalIssue, CanonicalRelation, CsvAdapter,
    DryRun, GitHubAdapter, IdMapEntry, ImportEngine, ImportError, ImportLaneBudget,
    InMemorySourceIdMap, JiraAdapter, LinearAdapter, ProviderRecord, ReconciliationReport,
    SourceAdapter, SourceIdMap, SourceSystem, Unresolved, UNSUPPORTED_PERMISSION_SCHEME,
};

pub use my_work::{
    issue_humanise_templates, list_my_work, list_my_work_default, my_work_filter,
    register_issue_humanise_templates, wire_issues_my_work, ISSUE_HUMANISE_TEMPLATES,
    TPL_APPROVAL_REQUESTED, TPL_SLA_AT_RISK, TPL_UNBLOCKED,
};

pub use reflexes::{
    linked_pr_from_payload, plan_branch_created, plan_chat_message_created, plan_check_updated,
    plan_member_event, plan_pr_merged, plan_pr_opened, plan_reflex, reflex_subjects,
    ReflexConsumer, ReflexEffect, AUTO_STATE_DONE, AUTO_STATE_IN_PROGRESS, CHAT_MESSAGE_CREATED,
    CI_CHECK_UPDATED, GIT_BRANCH_CREATED, GIT_PR_MERGED, GIT_PR_OPENED, IDENTITY_MEMBER_ADDED,
    IDENTITY_MEMBER_DEACTIVATED, IDENTITY_MEMBER_ERASED, REFLEX_SUBJECTS,
};

pub use refs_glue::{
    block_sub_ref, comment_sub_ref, edge_aggregate_key, emit_content_edges, emit_relation_edge,
    field_sub_ref, issue_root_ref, row_sub_ref, IssueLifecycleRel, IssueMeta, IssueProjectFetcher,
    IssueProjectionStore, IssueRelationGraph, LadderRung, ProjectError, Projected, Projection,
    Projector, RelationEdge, SubAnchor, SubState, Tombstone, TombstoneReason, TraversedNode,
    REFS_EDGE_CREATED, REL_CLASS_LIFECYCLE, REL_CLASS_REFERENCE, TRAVERSE_MAX_DEPTH,
};

pub use agent_spend::{
    per_effect_idem_key, spend_bearing_run, BalancedRunSignal, DispatchedRun, IssueRunKind,
    IssueSpendGate, SpendError,
};

pub use e2e_wedge::{
    run_e2e_1_pr_pane, run_issues_e2e_wedge, IssuesE2eArtifact, E2E_SCENARIO, FRESHNESS_BUDGET_SECS,
};

#[cfg(any(test, feature = "test-support"))]
pub use e2e_flagship::{run_e2e_2_issues_flagship, CLOSE_CARD_ID, E2E_FLAGSHIP_SCENARIO};

pub use e2e_lineage::{
    lineage_audit_anchor, run_e2e_3_lineage, run_issues_e2e_3, E2E_LINEAGE_SCENARIO,
    LINEAGE_DEPTH_BOUND,
};

pub use dogfood::{
    myelin_issue_backlog, proven_issues_rows, run_issues_truth_up_scorecard, IssuesDogfoodArtifact,
    IssuesIncident, IssuesRowStatus, IssuesTruthUpPass, IssuesTruthUpRed, IssuesTruthUpScorecard,
    IssuesTruthUpVerdict, MyelinIssue, ProvenIssuesRow, MYELIN_SELF_REGION, MYELIN_SELF_TENANT,
};
#[cfg(any(test, feature = "test-support"))]
pub use dogfood::run_issues_over_myelins_own_work;
pub use switch_test::{
    switch_capability_matrix, switch_surface_drive_record, IssuesOverlay, IssuesSwitchTest,
    IssuesSwitchVerdict, PrimaryScreenState, SwitchCapability,
};

pub use trigger::{
    default_stale_after, ArmRequest, ArmableCondition, IssueTriggerEngine, TriggerInboxItem,
    TriggerInboxKind, TriggerSnapshot, DEFAULT_STALE_AFTER_DAYS, VAR_ASSIGNEE,
    VAR_BLOCKED_BY_UNRESOLVED, VAR_STATE_CATEGORY,
};

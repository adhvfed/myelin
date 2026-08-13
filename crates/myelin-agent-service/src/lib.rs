pub mod app;
pub mod card_text;
pub mod catalogue;
pub mod chat_read_tools;
pub mod chat_tools;
pub mod ci_tools;
pub mod cost_gate;
pub mod defaults;
pub mod dispatch;
pub mod dispatch_surge;
pub mod dry_run;
pub mod dsr;
pub mod effect_api;
pub mod escape_gate;
pub mod exec;
pub mod git_read_tools;
pub mod git_tools;
pub mod hitl;
pub mod hitl_batch;
pub mod holder;
pub mod hosted_run_contract;
pub mod issues_agents;
pub mod issues_read_tools;
pub mod issues_tools;
pub mod knowledge_mcp_tools;
pub mod knowledge_tools;
pub mod long_park;
pub mod loop_guards;
pub mod metering;
pub mod migrations;
pub mod mock;
pub mod project_read_tools;
pub mod schema;
pub mod skeleton;
pub mod tool_exec;
pub mod tool_scope;
pub mod trace_seam;
pub mod trigger_consumer;
#[cfg(feature = "integration")]
pub mod trigger_handoff;

pub use holder::{
    agent_store_classifier, register_agent_holders, AgentHolderRegistration, AgentOltpHolder,
    AgentTraceHolder, AGENT_OLTP_STORE, AGENT_TRACE_STORE,
};

pub use catalogue::{catalogue_cursor, tool_ref, PlatformToolCatalogue, ToolCatalogueError};

pub use dsr::{
    subject_dek_ref, AgentFabricHolder, AgentFabricStore, FabricEraseReceipt, FabricErasureLedger,
    FabricLocateReport, FabricReErasureReceipt, FreeTextRow, RunAttribution,
};

pub use skeleton::{
    requesting_subject, ChildEnv, RunOutcomeKind, RunSubstrate, RunTokenRevoker, RunWallet,
    SkeletonAgent, SkeletonAgentRuntime, SkeletonError, SkeletonTelemetry, SpendCapStage,
    AGENT_RUN_TRACED_EVENT, DEFAULT_MAX_TURNS, SKELETON_STEP_UNIT, WALLET_MIN_BALANCE_FLOOR,
};

pub use metering::{price, ModelRates, PriceError, Priced, LUNA_RATES};

#[cfg(any(test, feature = "test-support"))]
pub use tool_exec::{MockToolExecutor, MockToolSurface};
pub use tool_exec::{ToolExecError, ToolExecutionContext, ToolExecutor};

pub use cost_gate::{runaway_brain, AgentFabricCostSignal, RunawaySelfLimiter, RunawayStep};

pub use mock::{
    build_conversation, model_turns_taken, replay, replay_bounded, select_runtime, HistoryEntry,
    MockAgentRuntime, MockScript, ReplayRecord, RuntimeFlag, TraceHistory, MOCK_MAX_STEPS,
};

pub use effect_api::{
    decode_proposed, effect_gate_key, effect_gate_key_str, encode_proposed, validate_call,
    validate_schema, validate_tool_arguments, ApplyError, CapabilityCheck, DelegationLookup,
    EffectApiBridge, EffectBudget, EffectCost, PipelineSignals, PipelineStep, PlanThenApply,
    PlanVerdict, PlannedEffect, SubsystemApply, TenantGuard,
};

pub use exec::{
    compute_tool_def, route_of, ExecError, RoutingError, SandboxJob, SandboxToolHands, ToolRoute,
    PLATFORM_TOKEN_ENV,
};

pub use escape_gate::{AgentExecGate, GateRefusal, ProductionBackendId};

pub use defaults::{
    assert_no_silent_loosening, default_for_tool, requires_approval_default,
    requires_approval_for_landing, seed_requires_approval, LooseningViolation, WrittenDeviation,
};

pub use git_tools::{
    git_author_tool_defs, git_comment_tool_def, git_history_rewrite_tool_def,
    git_merge_required_caps, git_merge_tool_def, git_resolve_thread_tool_def,
    git_scip_index_tool_def, git_submit_review_tool_def, git_suggest_change_tool_def,
    git_tool_defs, open_pr_required_caps, open_pr_tool_def, register_git_tools, GIT_MERGE_TOOL,
    GIT_SUBSYSTEM, GIT_TOOL_VERSION, OPEN_PR_TOOL,
};

pub use knowledge_tools::{
    comment_required_caps, comment_tool_def, draft_required_caps, draft_tool_def,
    edit_confidential_required_caps, edit_confidential_tool_def, knowledge_tool_defs,
    publish_required_caps, publish_tool_def, register_knowledge_tools, COMMENT_TOOL, DRAFT_TOOL,
    EDIT_CONFIDENTIAL_TOOL, KNOWLEDGE_SUBSYSTEM, KNOWLEDGE_TOOL_VERSION, PUBLISH_TOOL,
};

pub use issues_tools::{
    advisory_required_caps, forecast_tool_def, issues_tool_defs, register_issues_tools,
    sla_draft_tool_def, transition_caveat, transition_required_caps, transition_tool_def,
    triage_tool_def, FORECAST_TOOL, ISSUES_SUBSYSTEM, ISSUES_TOOL_VERSION, SLA_DRAFT_TOOL,
    TRANSITION_TOOL, TRIAGE_TOOL,
};

pub use issues_agents::{
    assign_required_caps, assign_tool_def, close_tool_def, create_required_caps, create_tool_def,
    estimate_tool_def, full_issues_tool_defs, link_tool_def, mock_forecast_agent,
    mock_triage_agent, register_full_issues_tools, reorder_tool_def, replay_forecast_agent,
    triage_effect_for, triage_suggestion_strip, update_required_caps, update_tool_def,
    ForecastInput, ForecastOutput, LinearForecast, ASSIGN_TOOL, CLOSE_TOOL, CREATE_TOOL,
    CREATE_TOOL_VERSION, ESTIMATE_TOOL, LINK_TOOL, REORDER_TOOL, UPDATE_TOOL,
};

pub use issues_read_tools::{
    issues_read_tool_defs, LIST_ISSUES_TOOL, VIEW_ISSUE_TOOL, VIEW_ISSUE_TOOL_VERSION,
};

pub use knowledge_mcp_tools::{
    knowledge_mcp_tool_defs, link_work_tool_def, LINK_WORK_TOOL, LINK_WORK_TOOL_VERSION,
    LIST_PAGES_TOOL, READ_PAGE_TOOL, READ_PAGE_TOOL_VERSION,
};

pub use project_read_tools::{
    project_read_tool_defs, LIST_PROJECTS_TOOL, PROJECTS_SUBSYSTEM, PROJECTS_TOOL_VERSION,
};

pub use chat_read_tools::{chat_read_tool_defs, LIST_CONVERSATIONS_TOOL, READ_MESSAGES_TOOL};

pub use git_read_tools::{
    git_read_tool_defs, LIST_REPOSITORIES_TOOL, READ_FILE_TOOL, SEARCH_CODE_TOOL,
};

pub use chat_tools::{
    chat_tool_defs, landing_requires_approval, post_message_tool_def, post_required_caps,
    react_tool_def, register_chat_tools, CHAT_SUBSYSTEM, CHAT_TOOL_VERSION, POST_MESSAGE_TOOL,
    REACT_TOOL,
};

pub use ci_tools::{
    approve_deploy_tool_def, ci_tool_defs, deploy_required_caps, deploy_tool_def,
    register_ci_tools, run_pipeline_required_caps, run_pipeline_tool_def,
    write_secret_required_caps, write_secret_tool_def, APPROVE_DEPLOY_TOOL, CI_SUBSYSTEM,
    CI_TOOL_VERSION, DEPLOY_TOOL, RUN_PIPELINE_TOOL, WRITE_SECRET_TOOL,
};

pub use dispatch::{classify, DispatchCounter, DispatchDecision, DispatchTrigger};

pub use dispatch_surge::{
    run_agent_dispatch_surge, AgentDispatchShed, AgentDispatchSurgeGate, AgentDispatchSurgeReport,
    DispatchFrontError, RetryAfterHonouringRuntime, RuntimeReaction,
    AGENT_DISPATCH_SURGE_MULTIPLIER, AGENT_LANE_SHED_BUDGET_IS_MEASURED,
};

pub use trace_seam::{
    is_content_addressed_kn_document, trace_ref_of, TraceDocument, STATELESS_EXCEPT_TRACE_FLOOR,
};

pub use dry_run::{
    dry_run_plan, proposed_effect_sequence, DryRunBridge, DryRunEntry, DryRunPlanner,
};

pub use hitl::{
    derive_approver_set, gate_id_of, live_cost_estimate, persist_gate_decision, persist_gate_open,
    run_hitl_loop, surface_card, ApprovedTools, ApproverSet, Halted, HitlCard, HitlGate,
    HitlGateState, HitlOutcome, HitlWait, InvalidTransition, RiskSummary, WaitDecision,
};

pub use card_text::{
    assert_no_raw_agent_surface, humanise_agent_message, humanise_card, humanise_risk_summary,
    register_agent_templates, AgentMessage, RawAgentString, RenderCtx, RenderedCard,
    AGENT_PLATFORM_DEFAULT_TEMPLATES,
};

pub use hitl_batch::{
    per_effect_idem_key, run_batch_hitl_loop, ApplyLedger, BatchApprovalCard, BatchGatedEffect,
    BatchHitlWait, BatchOutcome, DecisionScript, EffectOutcome,
};

pub use loop_guards::{
    AgentLoopGuards, GuardRefusal, GuardVerdict, IdempotentToolLedger, ReferenceGate, SelfGuard,
    AGENT_CEILING, AGENT_DISPATCH_POOL_CAP, AGENT_SHARED_ROOT_CAP,
};

pub use long_park::{
    dispatch_long_compute, dispatch_long_compute_metered, AgentJobDispatcher, LongComputeProfile,
    LongParkOutcome,
};

pub use tool_scope::{
    apply_scope_to_conversation, assert_apply_rechecks_revoked, build_scoped_tool_list,
    lower_list_objects, lower_set_expr, scoped_tool_ids_sql, tool_def_id, ScopedToolList,
    ToolCatalogueIds, ToolListObjects, ToolScopePredicate, TOOL_DEF_OBJECT_TYPE, TOOL_ID_COLUMN,
    TOOL_USE_PERMISSION,
};

pub use app::{
    agent_app_spec, agent_app_spec_with_ingestion, agent_dispatch_consumer_reg, boot_agent,
    governed_trigger_consumer_reg, run_agent, run_agent_ingestion_until_shutdown,
    trigger_intake_filter, SkeletonDispatchConsumer, AGENT_DISPATCH_SUBJECT_PREFIX,
    EVENT_DURABLE_CONSUMER, EVENT_STREAM_NAME, EVENT_SUBJECT_ROOT, SERVICE_NAME,
};

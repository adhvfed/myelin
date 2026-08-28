pub mod app;
pub mod catalogue;
pub mod chat_read_tools;
pub mod chat_tools;
pub mod ci_tools;
pub mod defaults;
pub mod effect_api;
pub mod git_read_tools;
pub mod git_tools;
pub mod hosted_run_contract;
pub mod issues_agents;
pub mod issues_read_tools;
pub mod knowledge_mcp_tools;
pub mod knowledge_tools;
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
pub mod workspace;
pub mod workspace_events;
pub mod workspace_execution;
pub mod workspace_tools;

pub use catalogue::{catalogue_cursor, tool_ref, PlatformToolCatalogue, ToolCatalogueError};

pub use skeleton::{
    requesting_subject, ChildEnv, RunOutcomeKind, RunSubstrate, RunTokenRevoker, RunWallet,
    SkeletonAgent, SkeletonAgentRuntime, SkeletonError, SkeletonTelemetry, SpendCapStage,
    AGENT_RUN_TRACED_EVENT, DEFAULT_MAX_TURNS, SKELETON_STEP_UNIT, WALLET_MIN_BALANCE_FLOOR,
};

pub use metering::{price, ModelRates, PriceError, Priced, LUNA_RATES};

#[cfg(any(test, feature = "test-support"))]
pub use tool_exec::{MockToolExecutor, MockToolSurface};
pub use tool_exec::{ToolExecError, ToolExecutionContext, ToolExecutor};

pub use mock::{
    build_conversation, model_turns_taken, replay, replay_bounded, select_runtime, HistoryEntry,
    MockAgentRuntime, MockScript, ReplayRecord, RuntimeFlag, TraceHistory, MOCK_MAX_STEPS,
};

pub use effect_api::{validate_call, validate_schema, validate_tool_arguments};

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

pub use issues_agents::{
    close_tool_def, create_required_caps, create_tool_def, issues_mutation_tool_defs,
    register_issues_mutation_tools, CLOSE_TOOL, CREATE_TOOL, CREATE_TOOL_VERSION, ISSUES_SUBSYSTEM,
    ISSUES_TOOL_VERSION,
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

pub use workspace_tools::{
    workspace_tool_defs, EXEC_WORKSPACE_COMMAND_TOOL, READ_WORKSPACE_FILE_TOOL,
    WORKSPACE_SUBSYSTEM, WORKSPACE_TOOL_VERSION, WRITE_WORKSPACE_FILE_TOOL,
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

pub use trace_seam::{is_content_addressed_kn_document, trace_ref_of, TraceDocument};

pub use tool_scope::{
    apply_scope_to_conversation, assert_apply_rechecks_revoked, build_scoped_tool_list,
    lower_list_objects, lower_set_expr, scoped_tool_ids_sql, tool_def_id, ScopedToolList,
    ToolCatalogueIds, ToolListObjects, ToolScopePredicate, TOOL_DEF_OBJECT_TYPE, TOOL_ID_COLUMN,
    TOOL_USE_PERMISSION,
};

pub use app::{
    governed_trigger_consumer_reg, placed_tenant_intake_scope, run_agent_ingestion_until_shutdown,
    trigger_intake_filter, TriggerConsumerDurability, EVENT_DURABLE_CONSUMER, EVENT_STREAM_NAME,
    EVENT_SUBJECT_ROOT, SERVICE_NAME,
};

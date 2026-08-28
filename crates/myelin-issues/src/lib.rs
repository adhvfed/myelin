#![forbid(unsafe_code)]

pub mod agent_spend;
pub mod api;
pub mod app;
pub mod content;
pub mod declares;
pub mod dek;
pub mod durable_erase;
pub mod events;
mod import_source;
pub mod keys;
pub mod migrations;
pub mod my_work;
pub mod pg_issue_store;
pub mod post_restore;
pub mod pseudonym;
pub mod query_coown;
pub mod rebac_fragment;
pub mod refs_glue;
pub mod schema;
pub mod write_path;

pub use app::{boot_issues, issues_app_spec, run_issues, run_issues_until_shutdown, SERVICE_NAME};
pub use durable_erase::{
    issue_title_holder_receipts, DurableIssueTitleEraser, DurableIssueTitleErasureError,
    DurableIssueTitleErasureProof,
};
pub use migrations::{
    issues_hot_tables, issues_migrations, make_tenant_scoped_ddl, CONSUMER_DEDUP_TABLE,
    CREATE_CONSUMER_DEDUP_DDL, CREATE_CYCLE_DDL, CREATE_CYCLE_MEMBERSHIP_DDL,
    CREATE_ISSUE_AUTHZ_BINDING_DDL, CREATE_ISSUE_AUTHZ_INVALIDATION_TRIGGERS_DDL,
    CREATE_ISSUE_AUTHZ_VISIBLE_DDL, CREATE_ISSUE_CHANGE_LOG_DDL,
    CREATE_ISSUE_CREATE_IDEMPOTENCY_DDL, CREATE_ISSUE_DDL, CREATE_ISSUE_INDEXES_DDL,
    CREATE_ISSUE_KEY_PREFIX_LIST_INDEX_DDL, CREATE_ISSUE_RECENT_LIST_INDEX_DDL,
    CREATE_ISSUE_RELATION_DDL, CREATE_ISSUE_RELATION_INDEXES_DDL,
    CREATE_ISSUE_TITLE_ERASURE_BATCH_INDEX_DDL, CREATE_ISSUE_TITLE_ERASURE_OPERATION_DDL,
    CREATE_ISSUE_VIEW_SUBJECT_DDL, CREATE_MILESTONE_DDL, CREATE_PREFIX_COUNTER_DDL,
    CREATE_SCHEME_ASSIGNMENT_DDL, CREATE_SCHEME_DDL, CYCLE_MEMBERSHIP_TABLE, CYCLE_TABLE,
    EXPAND_ISSUE_AUTHZ_CREATED_EVENT_DDL, EXPAND_ISSUE_CREATOR_KIND_DDL,
    EXPAND_ISSUE_RELATION_ACTOR_DDL, EXPAND_ISSUE_RELATION_CREATOR_KIND_DDL,
    EXPAND_ISSUE_TITLE_ERASURE_DDL, ISSUE_ASSIGNEE_INDEX, ISSUE_AUTHZ_BINDING_TABLE,
    ISSUE_AUTHZ_VISIBLE_TABLE, ISSUE_BOARD_INDEX, ISSUE_CHANGE_LOG_TABLE,
    ISSUE_CREATE_IDEMPOTENCY_TABLE, ISSUE_CYCLE_INDEX, ISSUE_KEY_PREFIX_LIST_INDEX,
    ISSUE_PARENT_INDEX, ISSUE_PROPS_GIN_INDEX, ISSUE_RECENT_LIST_INDEX, ISSUE_RELATION_TABLE,
    ISSUE_ROADMAP_INDEX, ISSUE_TABLE, ISSUE_TITLE_ERASURE_OPERATION_TABLE,
    ISSUE_VIEW_SUBJECT_TABLE, MILESTONE_TABLE, OUTBOX_TABLE, PREFIX_COUNTER_TABLE,
    SCHEME_ASSIGNMENT_TABLE, SCHEME_TABLE,
};
pub use migrations::{CREATE_IMPORT_MAP_DDL, IMPORT_MAP_TABLE};
pub use pg_issue_store::{
    is_canonical_request_event_id, visible_issue_keys_in_tx, AuthoredIssueTitleEraseReceipt,
    AuthoredIssueTitleErasureState, CreateIssue, CreateIssueIntent, IssueAuthorizationBinding,
    IssueAuthorizationOutcome, IssueAuthorizationState, IssueAuthorizationStatus, IssueAuthorizer,
    IssueCreationOutcome, IssueCreationReceipt, IssuePage, IssuePageRequest, IssuePermission,
    IssueRelationCreationOutcome, IssueStoreError, IssueTitleErasureAttempt, IssueTupleWriter,
    IssueViewProjectionRevision, IssueViewRebuildOutcome, PgIssueStore, StoredIssue,
    StoredIssueRelation, VisibleIssues, MAX_RELATIONS_PER_ISSUE,
};
pub use pg_issue_store::{ImportIssue, ImportIssueReceipt};
pub use post_restore::{
    PostRestoreIssueTitleError, PostRestoreIssueTitleReEraser, PostRestoreIssueTitleReport,
};

pub use write_path::{
    apply_mutation, issue_aggregate_key, issue_ref, IssueDraft, IssueUpdate, MutationKind,
    WriteError, WriteOutcome, PERM_COMMENT, PERM_MANAGE, PERM_PERFORM_TRANSITION, PERM_TRANSITION,
};

pub use dek::{
    decrypt_free_text, encrypt_free_text, plaintext_at_rest, subject_dek_erasure, IssueFreeText,
};
pub use pseudonym::{
    is_raw_principal_id, is_resolvable_pseudonym, pseudonymise, public_issue_actor, IssueActorKind,
    IssuePseudonym, PseudonymError,
};
pub use write_path::{apply_mutation_sealed, SealError, SealedCreate};

pub use keys::{
    render_display_key, CanonicalKey, HiLoKeyAllocator, InMemoryPrefixCounter, PrefixReserve,
    ReserveError, ReservedBlock, INITIAL_BLOCK_SIZE, MAX_BLOCK_SIZE,
};
pub use write_path::create_issue;

pub use content::{
    emit_content_event, is_issue_block, paragraph_body, roundtrips_md, validate_subtree,
    CasConflict, ContentError, ContentKind, IssueContent, SubsetError, ISSUES_EXCLUDED_BLOCKS,
};

pub use import_source::SourceSystem;

pub use my_work::{
    issue_humanise_templates, list_my_work, list_my_work_default, my_work_filter,
    register_issue_humanise_templates, wire_issues_my_work, ISSUE_HUMANISE_TEMPLATES,
    TPL_APPROVAL_REQUESTED, TPL_SLA_AT_RISK, TPL_UNBLOCKED,
};

pub use refs_glue::{
    issue_root_ref, IssueLifecycleRel, REFS_EDGE_CREATED, REL_CLASS_LIFECYCLE, REL_CLASS_REFERENCE,
};

pub use agent_spend::{
    per_effect_idem_key, spend_bearing_run, BalancedRunSignal, DispatchedRun, IssueRunKind,
    IssueSpendGate, SpendError,
};

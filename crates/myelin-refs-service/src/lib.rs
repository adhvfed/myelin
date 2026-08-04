#![forbid(unsafe_code)]

pub mod backlinks;
pub mod cache;
pub mod chat_producer;
pub mod ci_producer;
pub mod cross_cell;
pub mod dek;
#[cfg(any(test, feature = "test-support"))]
pub mod dogfood;
#[cfg(any(test, feature = "test-support"))]
pub mod e2e_wedge;
pub mod edge_builder;
pub mod emit;
pub mod erasure_posture;
pub mod git_producer;
pub mod holder;
pub mod invalidator;
pub mod issues_producer;
pub mod kn_producer;
pub mod ladder;
pub mod loop_guard;
pub mod migration;
pub mod mirror;
pub mod reach_index;
pub mod reindex;
#[cfg(any(test, feature = "test-support"))]
pub mod reindex_at_scale;
pub mod residency;
pub mod resolve;
pub mod restore_reerase;
pub mod restrict;
pub mod surge;
pub mod switch_test;
pub mod traverse;

pub use backlinks::{
    ids_result, lower_over_source_root, set_expr_admits, source_root_colref, view_permission,
    watermark_verdict, AuthzJoin, AuthzVisibleIndex, Backlink, BacklinkError, BacklinkPage,
    BacklinkRead, BoundParam, FilterMode, SourceRootFilter, WatermarkVerdict, AUTHZ_VISIBLE_TABLE,
    FILTER_MODE_SPLIT_SIGNAL, SOURCE_ROOT_COLUMN,
};
pub use cache::{CacheFillError, R2ProjectionCache, R2_DEFAULT_TTL, R2_KEY_PREFIX};
pub use chat_producer::{
    ChatAnchorState, ChatEdgeProducer, ChatOwner, CHAT_CHANNEL_TYPE, CHAT_OWNER_TOKEN,
};
pub use ci_producer::{CiOwner, StepAnchorResolver, StepResolution, CI_OWNER_TOKEN};
pub use cross_cell::{
    cross_cell_backlink_pointer, cross_cell_erase_receipt, fanout_carried_fields,
    migrate_home_cell, CellLocalBacklinkResolver, CrossCellEraseReceipt, CrossCellFanOut,
    CROSS_CELL_RAW_ROWS_SIGNAL, CROSS_CELL_RESOLVES_SIGNAL,
};
pub use dek::{ref_p5_inherited_gates, InheritedGate, RefsDekPin};
#[cfg(any(test, feature = "test-support"))]
pub use dogfood::{
    proven_refs_rows, run_refs_truth_up_scorecard, DogfoodArtifact, ProvenRefsRow, RefsIncident,
    RefsIncidentDrillTicket, RefsIncidentIssueDraft, RefsRowStatus, RefsScorecardEntry,
    RefsTruthUpPass, RefsTruthUpRed, RefsTruthUpScorecard, RefsTruthUpVerdict, MYELIN_SELF_REGION,
    MYELIN_SELF_TENANT,
};
#[cfg(any(test, feature = "test-support"))]
pub use dogfood::run_refs_over_myelins_own_work;
#[cfg(any(test, feature = "test-support"))]
pub use e2e_wedge::{run_e2e_1_pr_pane, run_e2e_3_spec_to_ship, E2eArtifact, E2E_SCENARIOS};
#[cfg(any(test, feature = "test-support"))]
pub use e2e_wedge::{run_e2e_4_dsar_fanout, run_refs_e2e_wedge};
pub use edge_builder::{
    edge_id, EdgeProjection, EdgeRow, ProjectError, RefsEdgeBuilder, RelClass,
    EDGE_BUILDER_CONSUMER, EDGE_BUILDER_SUBJECTS, EDGE_BUILDER_SUBJECT_PREFIXES,
};
pub use emit::{
    edge_aggregate_key, emit_edges, extract_edges, EdgeDraft, EdgeRel, REFS_EDGE_CREATED,
};
pub use erasure_posture::{erasure_posture, ErasurePosture};
pub use git_producer::{
    git_replay_scope, CommentState, GitEdgeProducer, GitOwner, GitReplayGrain, GIT_OWNER_TOKEN,
};
pub use holder::{
    refs_store_classifier, register_refs_holders, EdgeBacking, RefsCacheHolder, RefsEdgeHolder,
    RefsHolderRegistration, REFS_CACHE_STORE, REFS_EDGE_STORE,
};
pub use invalidator::{
    InvalidateError, InvalidationCall, NoOpCacheShim, ProjectionCache, RefsProjectionInvalidator,
    INVALIDATOR_CONSUMER, INVALIDATOR_SUBJECTS, INVALIDATOR_SUBJECT_PREFIXES,
};
pub use issues_producer::{
    mirror_issue_relation, project_issue_relation, reconverge_issue_relations, IssueAnchorState,
    IssueEdgeProducer, IssueOwner, IssueRelationEvent, ISSUE_OWNER_TOKEN,
};
pub use kn_producer::{
    kn_replay_scope, mirror_page_parent, project_page_parent, reconverge_page_tree, KnAnchorState,
    KnEdgeProducer, KnOwner, KnReplayGrain, PageParentEvent, KN_OWNER_TOKEN,
};
pub use ladder::{
    ladder_root, resolve_line_range, resolve_sub_outcome, LineRangeState, MintedLineRange,
    SubAnchorResolver, SubState, SyntheticSubResolver, TOMBSTONE_COUNT_SIGNAL,
};
pub use loop_guard::{
    is_retrigger_source, stamped_depth, target_is_structured_node, would_exceed_ceiling,
    GuardDecision, RefsLoopGuard, CAUSAL_DEPTH_CEILING,
};
pub use migration::{
    edge_ddl_is_forward_only, edge_table_dek_ref, edge_table_migrations, CREATE_EDGE_INDEXES_DDL,
    CREATE_EDGE_TABLE_DDL, EDGE_BY_REL_INDEX, EDGE_INBOUND_INDEX, EDGE_MIGRATION_ID,
    EDGE_OUTBOUND_INDEX, EDGE_TABLE, MAKE_EDGE_TENANT_SCOPED_DDL,
};
pub use mirror::{
    mirror_edges, project_typed_event, reconverge, Inverse, LifecycleRel, MirrorError,
    SyntheticTypedEvent,
};
pub use reach_index::{R4ReachIndex, R4Verdict, R4_READ_BUDGET_FANOUT};
pub use reindex::{
    RefsReindexSource, RefsReindexer, ReindexError, ReindexReceipt, SourceEdge,
    REFS_EDGE_SNAPSHOT_TYPE, REFS_OWNER_TOKEN,
};
#[cfg(any(test, feature = "test-support"))]
pub use reindex_at_scale::{
    build_full_scale_corpus, run_full_scale_reindex_parity, FiveProducerCorpus,
    FullScaleParityReport, FIVE_PRODUCERS, WORLD_SCALE_FLEET_LOAD_FLOOR,
};
pub use residency::{refs_store_descriptors, RefsStoreDescriptor};
pub use resolve::{
    bounded_stale, strong_read, AuthzServed, CrossCellDisposition, NoOpCacheRead, OwnerProjection,
    ProjectApi, ProjectApiError, ProjectOutcome, Projection, ProjectionCacheRead, ProjectionFlag,
    Resolution, ResolveMode, ResolveService, Tombstone, TombstoneReason,
    RESOLVE_CACHE_HIT_RATIO_SIGNAL, VIEW_PERMISSION,
};
pub use restore_reerase::{
    build_backup_scale_corpus, re_erase_at_backup_scale, BackupScaleErasureCorpus,
    BackupScaleReEraseReport, CorpusEdge, RefsErasedSubject, RefsErasureLedger,
    REERASE_RECOVERABLE_PII_SIGNAL, WORLD_SCALE_BACKUP_FLEET_FLOOR,
};
pub use restrict::RestrictSet;
pub use surge::{
    run_refs_surge, RefsShedGate, RefsShedRejection, RefsSurgeReport, R4_REACH_INDEX_FOLLOW_ON,
    REFS_SURGE_MULTIPLIER, SHARD_SPLIT_IS_MEASURED_ONLY,
};
pub use switch_test::{
    four_keystroke_jump_chain, switch_capability_matrix, switch_surface_drive_record,
    BrowserDriveStatus, MeasuredLatencies, RefsSwitchTest, RefsSwitchVerdict, SwitchCapability,
    SwitchSurfaceDrive,
};
pub use traverse::{
    apply_post_filter, depth_ceiling_from_thresholds, max_nodes_from_thresholds, Traverse,
    TraverseFilter, TraverseNode, TraverseResult, TRAVERSE_DEPTH_CEILING, TRAVERSE_MAX_NODES,
};

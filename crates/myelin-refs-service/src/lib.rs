#![forbid(unsafe_code)]

#[cfg(feature = "integration")]
use myelin_events::OutboxStore;
#[cfg(feature = "integration")]
use myelin_substrate::{
    AppSpec, Config, ConsumerReg, CriticalDependencies, InternalRpc, OutboxSpec, PublicRoutes,
};

pub mod backlinks;
pub mod cache;
pub mod chat_producer;
pub mod cross_cell;
pub mod dek;
pub mod edge_builder;
pub mod emit;
pub mod git_producer;
pub mod invalidator;
pub mod issues_producer;
pub mod kn_producer;
pub mod ladder;
pub mod loop_guard;
pub mod migration;
pub mod mirror;
#[cfg(feature = "integration")]
pub mod pg_edge;
pub mod reach_index;
pub mod reindex;
#[cfg(any(test, feature = "test-support"))]
pub mod reindex_at_scale;
pub mod residency;
pub mod resolve;
mod store;
pub mod surge;
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
pub use cross_cell::{
    cross_cell_backlink_pointer, cross_cell_erase_receipt, fanout_carried_fields,
    migrate_home_cell, CellLocalBacklinkResolver, CrossCellEraseReceipt, CrossCellFanOut,
    CROSS_CELL_RAW_ROWS_SIGNAL, CROSS_CELL_RESOLVES_SIGNAL,
};
pub use dek::{ref_p5_inherited_gates, InheritedGate, RefsDekPin};
pub use edge_builder::{
    edge_id, edge_mutation, EdgeMutation, EdgeProjection, EdgeRow, ProjectError, RefsEdgeBuilder,
    RelClass, EDGE_BUILDER_CONSUMER, EDGE_BUILDER_SUBJECTS, EDGE_BUILDER_SUBJECT_PREFIXES,
};
pub use emit::{
    edge_aggregate_key, emit_edges, extract_edges, EdgeDraft, EdgeRel, REFS_EDGE_CREATED,
};
pub use git_producer::{
    git_replay_scope, CommentState, GitEdgeProducer, GitOwner, GitReplayGrain, GIT_OWNER_TOKEN,
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
    edge_ddl_is_forward_only, edge_table_dek_ref, edge_table_migrations,
    CONTRACT_EDGE_REGION_IDENTITY_DDL, CREATE_EDGE_INBOUND_KEYSET_INDEX_DDL,
    CREATE_EDGE_INDEXES_DDL, CREATE_EDGE_OUTBOUND_KEYSET_INDEX_DDL,
    CREATE_EDGE_REGION_IDENTITY_INDEX_DDL, CREATE_EDGE_REGION_SEMANTICS_INDEX_DDL,
    CREATE_EDGE_TABLE_DDL, EDGE_BY_REL_INDEX, EDGE_INBOUND_INDEX, EDGE_INBOUND_KEYSET_INDEX,
    EDGE_INBOUND_KEYSET_MIGRATION_ID, EDGE_MIGRATION_ID, EDGE_OUTBOUND_INDEX,
    EDGE_OUTBOUND_KEYSET_INDEX, EDGE_OUTBOUND_KEYSET_MIGRATION_ID,
    EDGE_REGION_IDENTITY_CONTRACT_MIGRATION_ID, EDGE_REGION_IDENTITY_INDEX_MIGRATION_ID,
    EDGE_REGION_SEMANTICS_INDEX_MIGRATION_ID, EDGE_TABLE, MAKE_EDGE_TENANT_SCOPED_DDL,
};
pub use mirror::{
    mirror_edges, project_typed_event, reconverge, Inverse, LifecycleRel, MirrorError,
    SyntheticTypedEvent,
};
#[cfg(feature = "integration")]
pub use pg_edge::{
    build_pg_cell_edge_consumer, build_pg_edge_consumer, PgEdgeProjector, PgEdgeStore, StoredEdge,
};
pub use reach_index::{R4ReachIndex, R4Verdict, R4_READ_BUDGET_FANOUT};
pub use reindex::{
    RefsReindexSource, RefsReindexer, ReindexError, ReindexReceipt, SourceEdge,
    REFS_EDGE_SNAPSHOT_TYPE, REFS_OWNER_TOKEN,
};
#[cfg(any(test, feature = "test-support"))]
pub use reindex_at_scale::{
    build_full_scale_corpus, run_full_scale_reindex_parity, FiveProducerCorpus,
    FullScaleParityReport, FIVE_PRODUCERS,
};
pub use residency::{refs_store_descriptors, RefsStoreDescriptor};
pub use resolve::{
    bounded_stale, strong_read, AuthzServed, CrossCellDisposition, NoOpCacheRead, OwnerProjection,
    ProjectApi, ProjectApiError, ProjectOutcome, Projection, ProjectionCacheRead, ProjectionFlag,
    Resolution, ResolveMode, ResolveService, Tombstone, TombstoneReason,
    RESOLVE_CACHE_HIT_RATIO_SIGNAL, VIEW_PERMISSION,
};
pub use store::{REFS_CACHE_STORE, REFS_EDGE_STORE};
pub use surge::{
    run_refs_surge, RefsShedGate, RefsShedRejection, RefsSurgeReport, REFS_SURGE_MULTIPLIER,
};
pub use traverse::{
    apply_post_filter, depth_ceiling_from_thresholds, max_nodes_from_thresholds, Traverse,
    TraverseFilter, TraverseNode, TraverseResult, TRAVERSE_DEPTH_CEILING, TRAVERSE_MAX_NODES,
};

pub const SERVICE_NAME: &str = "refs";
pub const EVENT_STREAM_NAME: &str = "MYELIN_EVENTS";
pub const EVENT_SUBJECT_ROOT: &str = "myelin.events";
pub const EVENT_DURABLE_CONSUMER: &str = "refs-edge-builder-intake";

pub fn refs_intake_filter() -> String {
    format!("{EVENT_SUBJECT_ROOT}.evt.*.refs.>")
}

#[cfg(feature = "integration")]
pub async fn run_refs_ingestion_until_shutdown<F>(
    config: Config,
    outbox: OutboxStore,
    consumers: Vec<ConsumerReg>,
    intake: Box<dyn myelin_events::EventConsumer>,
    delivery_quarantine: std::sync::Arc<dyn myelin_events::DurableDeliveryQuarantine>,
    shutdown: F,
) -> Result<(), myelin_substrate::ServeError>
where
    F: std::future::Future<Output = ()>,
{
    let spec = AppSpec {
        name: SERVICE_NAME,
        config,
        migrations: edge_table_migrations(),
        hot_tables: myelin_substrate::HotTables::none(),
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        consumers,
        outbox: OutboxSpec::external_relay_with_consumer(outbox, intake, delivery_quarantine),
        critical: CriticalDependencies::default(),
        intake_scope: None,
    };
    myelin_substrate::serve_until_shutdown(spec, shutdown).await
}

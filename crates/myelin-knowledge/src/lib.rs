pub mod agent;
pub mod authority;
pub mod block_tree;
pub mod collab;
pub mod comments;
pub mod compaction;
pub mod crypto;
pub mod database;
pub mod editor;
pub mod emit;
pub mod export;
pub mod gdpr;
pub mod list_filter;
pub mod materialise;
pub mod merge;
pub mod notif_resolve;
pub mod pg_page;
pub mod rebac_fragment;
pub mod refs_glue;
pub mod replay;
pub mod rollup;
pub mod search_feed;
pub mod store;
pub mod subs;
pub mod surge;
pub mod sync_block;
pub mod transport;
pub mod yrs_engine;

pub use authority::{
    field_caveat, AclZookieTable, AuthZookie, CollectionSchema, ErasureLedger, IncomingOp,
    OpAuthorizer, OpDecision, OpPermission, RejectReason, SchemaValidator, StaleGrantCounter,
    STALE_GRANT_WRITES_METRIC,
};
pub use block_tree::{
    children_index_range_sql, recursive_subtree_cte_sql, BlockId, BlockRow, BlockTree, PageId,
    PageTree, TreeError,
};
pub use comments::{
    create_comment, mint_comment, mint_thread, register_knowledge_comment_kinds, resolve_comment,
    Comment, CommentAnchor, CommentError, CommentOpError, CommentStore, CommentThread,
    KNOWLEDGE_COMMENT_SUB_KINDS,
};
pub use compaction::{
    content_address, materialize, CompactionError, DocSnapshot, SnapshotCompactor,
};
pub use crypto::{decrypt_text, encrypt_text, knowledge_subject_erasure};
pub use database::{
    execute_view_count, execute_view_query, lower_view_filter, row_matches_filter, DbRelation,
    DbRow, FacetIndexHint, FacetPath, FacetTelemetry, FieldDef, FieldSchema, LoweredViewFilter,
    PageBound, PropertyBag, RelationEdgeEvent, RelationKind, RelationStore, SchemaError, ViewError,
    ViewQuery, FACET_PROMOTION_THRESHOLD,
};
pub use editor::{
    Document, EditOp, Editor, EditorBlock, EditorError, SecondViewer, BROWSER_DRIVE_EVIDENCE,
};
pub use emit::{
    block_ref, database_ref, emit_change, event_actor_pseudonym, page_ref,
    pseudonymized_event_principal, row_ref, KnowledgeChange, KnowledgeLivingDocHandler,
    KNOWLEDGE_LIVING_DOC_TRIGGERS,
};
pub use export::{
    export_rows_to_csv, import_adf, AdfImportResult, ExportBlock, ExportDoc, ExportError,
    ExportFormat, ParsedAdfNode, EXPORT_SCHEMA_VERSION,
};
pub use gdpr::{
    KnowledgeLocateReport, KnowledgePersonRecord, KnowledgePersonalDataHolder, LocatedKind,
    LocatedLocus, RectifyOutcome, RestrictSuppressor, RestrictionRegistry, RestrictionSink,
    SinkVerdict, HOLDER_ID as KNOWLEDGE_HOLDER_ID,
};
pub use list_filter::{
    compose_db_count_query, compose_db_view_query, db_row_id_colref, lower_over,
    lower_over_db_row_id, lower_over_page_id, page_id_colref, AuthzJoin, AuthzVisibleIndex,
    BoundParam, ComposedQuery, FilterMode, LoweredFilter, AUTHZ_VISIBLE_TABLE, PAGE_ID_COLUMN,
    PAGE_TABLE,
};
pub use materialise::{
    materialise_blob_store_parity, promote_facet, promote_facet_pii_cleared, read_time_recompute,
    target_numeric_value, BlobParityVerdict, FacetPromotionError, FacetPromotionPlan,
    FacetPromotionStep, MaterialisedRollup, MaterialisedValue, RowUpdatedDelta,
    DB_ROW_TABLE as MATERIALISE_DB_ROW_TABLE,
};
pub use merge::{
    cas_update_sql, BlockState, CasError, CasOutcome, CasStore, ConflictMeter, OfflineQueue,
    QueuedEdit, ReconcileResult, SimultaneousPresence, SoftLock, SoftLockTable,
    CAS_CONFLICT_RATE_METRIC,
};
pub use notif_resolve::KnowledgeRefResolver;
pub use pg_page::{
    knowledge_page_migrations, KnowledgeBlockRecord, KnowledgePageError, KnowledgePageRecord,
    KnowledgePageStore, KnowledgeVisibility, NewKnowledgePage, SaveKnowledgePage,
    EXPAND_KNOWLEDGE_BLOCK_REFERENCES_DDL, KNOWLEDGE_BLOCK_TABLE, KNOWLEDGE_PAGE_RECENT_INDEX,
    KNOWLEDGE_PAGE_TABLE, MAX_BLOCK_REFERENCES, MAX_PAGE_REFERENCES,
};
pub use rebac_fragment::{
    block_read_fragment, database_row_read_fragment, field_view_permission,
    knowledge_read_fragment, page_read_fragment, page_read_override, row_reader_set_expr,
    space_read_fragment,
};
pub use refs_glue::{
    edge_aggregate_key, emit_content_edges, emit_page_parent_set, emit_relation_edge,
    KnowledgeLifecycleRel, LadderRung, PageMeta, PageStore, ProjectError as RefsProjectError,
    Projected, Projector, SubAnchor, SubState, REFS_EDGE_CREATED, REL_CLASS_LIFECYCLE,
    REL_CLASS_REFERENCE,
};
pub use replay::{KnowledgeReindexSource, REFS_EDGE_SNAPSHOT};
pub use rollup::{
    compute_row, CellValue, FormulaExpr, FormulaField, FormulaSchema, FormulaSchemaError,
    MaterialisationHint, RollupFn, RollupLatencyTelemetry, RollupResolver, MAX_DEPENDENCY_DEPTH,
    MAX_FORMULA_DEPTH, MAX_FORMULA_NODES,
};
pub use search_feed::{
    feed_project, kn_declared_index_specs, kn_index_specs, kn_page_index_spec, kn_read_permission,
    kn_row_index_spec, kn_search_query, kn_search_semantic, page_search_projection,
    register_kn_index_specs, FeedGrain, SearchAclFilter, KN_READ_PERMISSION, KN_SEARCH_OBJECT_TYPE,
};
pub use store::{knowledge_scope, knowledge_store_migrations, KnowledgeStore, KnowledgeTable};
pub use subs::{
    mint_block, mint_heading, register_knowledge_sub_kinds, KNOWLEDGE_OWNED_SUB_KINDS,
    KNOWLEDGE_SUBSYSTEM,
};
pub use surge::{
    run_collab_surge, run_lexorank_storm, CollabShedReason, CollabShedRejection, CollabSurgeGate,
    CollabSurgeReport, LexoStormReport, COLLAB_SURGE_MULTIPLIER,
};
pub use sync_block::{
    render_sync_block, AllowAll, DenyAll, ProjectionFreshness, SourceReadCheck,
    SyncBlockProjection, SyncBlockRender, SyncSource, Tombstone, TombstoneReason, Viewer,
};
pub use transport::{
    doc_scope, knowledge_stream, AllowAllAuthority, AuthAction, CollabTransport, DocOp, DocOpLog,
    FailClosedAuthority, OpAuthority, OpId, OpKind, OpLogError, PageSnapshot, PersistedOp,
    Presence, Recovery, SendOutcome, TransportError,
};

use myelin_events::OutboxStore;
#[cfg(test)]
use myelin_events::{consume, ConsumerName, ConsumerSpec, DedupLedger, InProcessBus};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Decision, IdentityService, ListObjectsResult,
    ObjectType, Permission, Principal,
};
#[cfg(test)]
use myelin_substrate::ConsumerReg;
use myelin_substrate::{
    boot, AppSpec, Authorizer, Config, CriticalDependencies, HotTables, InternalRpc, Migration,
    Migrations, OutboxSpec, PublicRoutes, ServeError, ServeHandle, StoreManifest,
};
use myelin_tenancy::ArtifactRef;

pub const SERVICE_NAME: &str = "knowledge";

pub const HOT_TABLES: [&str; 3] = ["block", "db_row", "doc_op"];

pub fn knowledge_service_migrations() -> Migrations {
    let mut migrations = vec![Migration::plain(
        "0200_knowledge_schema_marker",
        "CREATE TABLE IF NOT EXISTS knowledge_schema_marker (applied_at TEXT)",
    )];
    migrations.extend(store::knowledge_store_migrations().0);
    Migrations::of(migrations)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FailClosedEntrypoint;

impl FailClosedEntrypoint {
    pub fn new() -> FailClosedEntrypoint {
        FailClosedEntrypoint
    }
}

impl IdentityService for FailClosedEntrypoint {
    fn authenticate(
        &self,
        _credential: &myelin_identity::Credential,
    ) -> myelin_identity::Result<Principal> {
        Err(AuthzError::NotYetImplemented(
            "authenticate is consumed from Identity (4.1); the Knowledge client wires at KN-P16",
        ))
    }

    fn check(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _object: &ArtifactRef,
        _at: &Consistency,
        _caveat: Option<&CaveatContext>,
    ) -> myelin_identity::Result<Decision> {
        Ok(Decision::Deny)
    }

    fn list_objects(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _ty: &ObjectType,
        _at: &Consistency,
    ) -> myelin_identity::Result<ListObjectsResult> {
        Err(AuthzError::NotYetImplemented(
            "list_objects (ABAC push-down) → KN-P16; the shell wires the slot, not the body",
        ))
    }

    fn list_subjects(
        &self,
        _object: &myelin_identity::ObjectId,
        _permission: &Permission,
        _at: &Consistency,
    ) -> myelin_identity::Result<myelin_identity::SubjectTree> {
        Err(AuthzError::NotYetImplemented(
            "list_subjects is an Identity-owned method; not a Knowledge entrypoint",
        ))
    }

    fn explain(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _object: &myelin_identity::ObjectId,
        _at: &Consistency,
    ) -> myelin_identity::Result<myelin_identity::RewriteTrace> {
        Err(AuthzError::NotYetImplemented(
            "explain is an Identity-owned method; not a Knowledge entrypoint",
        ))
    }

    fn delegation(
        &self,
        _agent: &Principal,
        _trigger_actor: &Principal,
    ) -> myelin_identity::Result<myelin_identity::EffectivePolicy> {
        Err(AuthzError::NotYetImplemented(
            "delegation is an Identity-owned method; not a Knowledge entrypoint",
        ))
    }

    fn write_tuples(
        &self,
        _deltas: &[myelin_identity::TupleDelta],
        _precondition: Option<&myelin_identity::Precondition>,
    ) -> myelin_identity::Result<myelin_identity::Zookie> {
        Err(AuthzError::NotYetImplemented(
            "write_tuples is an Identity-owned method; not a Knowledge entrypoint",
        ))
    }

    fn mint_run_token(
        &self,
        _agent_id: &myelin_identity::PrincipalId,
        _run_id: &myelin_identity::RunId,
        _delegation_caveats: &myelin_identity::DelegationCaveats,
        _ttl: &myelin_identity::FailStaticBound,
    ) -> myelin_identity::Result<myelin_identity::RunToken> {
        Err(AuthzError::NotYetImplemented(
            "mint_run_token is an Identity-owned method; not a Knowledge entrypoint",
        ))
    }

    fn revoke(&self, _target: &myelin_identity::RevokeTarget) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented(
            "revoke is an Identity-owned method; not a Knowledge entrypoint",
        ))
    }

    fn resolve_pseudonym(
        &self,
        _subject: &myelin_identity::PrincipalId,
        _tenant: &myelin_tenancy::TenantId,
    ) -> myelin_identity::Result<String> {
        Err(AuthzError::NotYetImplemented(
            "resolve_pseudonym is an Identity-owned method; not a Knowledge entrypoint",
        ))
    }

    fn erase(&self, _subject: &myelin_identity::PrincipalId) -> myelin_identity::Result<()> {
        Err(AuthzError::NotYetImplemented(
            "erase is an Identity-owned method; not a Knowledge entrypoint",
        ))
    }

    fn admit_fragment(
        &self,
        _fragment: &myelin_identity::NamespaceFragment,
    ) -> myelin_identity::Result<myelin_identity::FragmentAdmit> {
        Err(AuthzError::NotYetImplemented(
            "admit_fragment is an Identity-owned method; the Knowledge ReBAC fragment is admitted \
             by Identity (P-ID-26), not a Knowledge entrypoint",
        ))
    }
}

pub struct KnowledgeEntrypointAuthorizer<S: IdentityService + Send + Sync> {
    inner: S,
}

impl<S: IdentityService + Send + Sync> KnowledgeEntrypointAuthorizer<S> {
    pub fn new(inner: S) -> KnowledgeEntrypointAuthorizer<S> {
        KnowledgeEntrypointAuthorizer { inner }
    }
}

impl<S: IdentityService + Send + Sync> Authorizer for KnowledgeEntrypointAuthorizer<S> {
    fn authorize(&self, subject: &Principal, action: &str) -> bool {
        let permission = Permission(action.to_string());
        let object = ArtifactRef(format!(
            "myelin://{}/knowledge/action/{}",
            subject.tenant.0, action
        ));
        let at = Consistency {
            at_least: myelin_identity::Zookie(String::new()),
            mode: myelin_identity::ConsistencyMode::Strong,
        };
        matches!(
            self.inner.check(subject, &permission, &object, &at, None),
            Ok(Decision::Allow)
        )
    }
}

pub fn knowledge_app_spec(config: Config, outbox: OutboxStore) -> AppSpec {
    AppSpec {
        name: SERVICE_NAME,
        config,
        migrations: knowledge_service_migrations(),
        hot_tables: HotTables::declare(HOT_TABLES),
        public: PublicRoutes::default(),
        internal: InternalRpc::default(),
        consumers: Vec::new(),
        holders: AppSpec::auto(),
        stores: StoreManifest::new(),
        outbox: OutboxSpec::external_relay(outbox),
        critical: CriticalDependencies::default(),
        intake_scope: None,
    }
}

pub const LIVING_DOC_CONSUMER: &str = "knowledge-living-doc";

#[cfg(test)]
fn knowledge_app_spec_with_consumers(
    config: Config,
    outbox: OutboxStore,
    subjects: &[&str],
    dedup: DedupLedger,
) -> Result<AppSpec, myelin_events::SubscribeError> {
    let mut spec = knowledge_app_spec(config, outbox.clone());
    spec.outbox = OutboxSpec::new(outbox, InProcessBus::new());

    let consumer = consume(
        ConsumerSpec::new(ConsumerName(LIVING_DOC_CONSUMER.into()), subjects),
        KnowledgeLivingDocHandler::new(),
        dedup,
    )?;
    spec.consumers = vec![ConsumerReg::new(consumer)];
    Ok(spec)
}

pub fn entrypoint_authorizer() -> KnowledgeEntrypointAuthorizer<FailClosedEntrypoint> {
    KnowledgeEntrypointAuthorizer::new(FailClosedEntrypoint::new())
}

pub fn boot_knowledge(config: Config, outbox: OutboxStore) -> Result<ServeHandle, ServeError> {
    boot(knowledge_app_spec(config, outbox))
}

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_identity::{ObjectType, PrincipalId, PrincipalKind};
    use myelin_substrate::{serve, HotTables, Readiness, Startup, Surface};
    use myelin_tenancy::TenantId;

    fn principal() -> Principal {
        Principal::stub(
            PrincipalId("p".into()),
            PrincipalKind::Human,
            TenantId("acme".into()),
        )
    }

    #[test]
    fn knowledge_shell_boots_and_three_ports_bind() {
        let handle = boot_knowledge(Config::default(), OutboxStore::new())
            .expect("the knowledge shell boots");
        assert_eq!(handle.name(), "knowledge");
        assert_eq!(
            handle.surfaces(),
            &[Surface::Public, Surface::Internal, Surface::MetricsHealth],
            "the three ports (public / internal-RPC / metrics-health) all bound (3/3)"
        );
    }

    #[test]
    fn readiness_is_false_pre_migrate_but_liveness_is_up() {
        let surface = myelin_substrate::MetricsHealthSurface::new(
            CriticalDependencies::new(["oltp"]),
            myelin_substrate::HealthTable::new(),
        );
        assert_eq!(surface.startup(), Startup::Booting);
        let r = surface.readiness();
        assert_eq!(
            r.verdict,
            Readiness::NotReady,
            "readiness is FALSE until migrations apply (the migrate-complete gate)"
        );
        assert!(
            r.startup_incomplete,
            "the not-ready reason names the startup (pre-migrate) gate"
        );
        assert!(r.sheds(), "a not-ready instance sheds new traffic");
        assert_eq!(
            surface.liveness(),
            myelin_substrate::Liveness::Up,
            "liveness ≠ readiness: a booting instance is not-killed (liveness stays Up)"
        );

        surface.mark_started();
        assert_eq!(
            surface.readiness().verdict,
            Readiness::Ready,
            "after migrate-complete the readiness gate lifts → ready"
        );
    }

    #[test]
    fn booted_instance_is_ready_after_migrate_complete() {
        let handle = boot_knowledge(Config::default(), OutboxStore::new()).expect("boot");
        assert_eq!(
            handle.metrics_health().startup(),
            Startup::Complete,
            "boot completed → the migrate gate lifted"
        );
        assert_eq!(
            handle.metrics_health().readiness().verdict,
            Readiness::Ready,
            "a booted knowledge instance (migrations applied, deps up) is ready"
        );
    }

    #[test]
    fn hot_table_flags_are_declared() {
        let spec = knowledge_app_spec(Config::default(), OutboxStore::new());
        for table in HOT_TABLES {
            assert!(
                spec.hot_tables.is_hot(table),
                "the {table} table is declared hot (contract 1.5)"
            );
        }
        let mut declared: Vec<&str> = spec.hot_tables.tables().collect();
        declared.sort_unstable();
        assert_eq!(
            declared,
            ["block", "db_row", "doc_op"],
            "exactly the three high-write tables are hot"
        );
    }

    #[test]
    fn blocking_alter_on_hot_table_is_refused_at_boot() {
        let mut runner = myelin_substrate::MigrationRunner::new();
        let migrations = Migrations::of([Migration::phased(
            "0210_block_blocking_alter",
            "ALTER TABLE block ADD COLUMN extra TEXT NOT NULL",
            myelin_substrate::MigrationPhase::Plain,
            "block",
        )]);
        let r = runner.run(&migrations, &HotTables::declare(HOT_TABLES));
        assert!(
            r.is_err(),
            "a blocking ALTER on the declared-hot `block` table is refused at boot"
        );
    }

    #[test]
    fn migrations_are_forward_only() {
        let migrations = knowledge_service_migrations();
        for m in &migrations.0 {
            assert!(
                !myelin_substrate::is_destructive(m.ddl.as_ref()),
                "migration {} is forward-only (no backward/destructive DDL)",
                m.id
            );
        }
        let mut runner = myelin_substrate::MigrationRunner::new();
        let bad = Migrations::of([Migration::plain("0210_bad", "DROP TABLE block")]);
        assert!(
            runner.run(&bad, &HotTables::declare(HOT_TABLES)).is_err(),
            "a destructive migration is refused at boot (forward-only)"
        );
    }

    #[test]
    fn entrypoint_check_fail_closes_to_deny() {
        let slot = FailClosedEntrypoint::new();
        let at = Consistency {
            at_least: myelin_identity::Zookie("z".into()),
            mode: myelin_identity::ConsistencyMode::Strong,
        };
        let d = slot.check(
            &principal(),
            &Permission("knowledge.read".into()),
            &ArtifactRef("myelin://acme/knowledge/page/PAGE-1".into()),
            &at,
            None,
        );
        assert_eq!(
            d,
            Ok(Decision::Deny),
            "the un-wired check slot denies (fail-closed)"
        );
    }

    #[test]
    fn entrypoint_re_authorizes_against_fail_closed_check() {
        let surface = myelin_substrate::InternalSurface::new(entrypoint_authorizer());
        let r = surface.handle(&principal(), "knowledge.write");
        assert!(
            matches!(
                r,
                Err(myelin_substrate::InternalReject::Unauthorized { .. })
            ),
            "the entrypoint call is re-authorized against the fail-closed check and denied"
        );
    }

    #[test]
    fn authenticate_and_list_objects_error_loudly() {
        let slot = FailClosedEntrypoint::new();
        let at = Consistency {
            at_least: myelin_identity::Zookie("z".into()),
            mode: myelin_identity::ConsistencyMode::Strong,
        };
        assert!(
            matches!(
                slot.list_objects(
                    &principal(),
                    &Permission("knowledge.read".into()),
                    &ObjectType("page".into()),
                    &at
                ),
                Err(AuthzError::NotYetImplemented(_))
            ),
            "list_objects errors loudly until KN-P16 (never a permissive set)"
        );
    }

    #[test]
    fn knowledge_store_auto_registers_as_holder() {
        let handle = boot_knowledge(Config::default(), OutboxStore::new()).expect("boot");
        assert!(
            handle
                .holder_registry()
                .is_registered(myelin_substrate::StoreKind::Oltp, "knowledge"),
            "the knowledge OLTP store auto-registered as a PersonalDataHolder"
        );
    }

    #[test]
    fn knowledge_service_serves_and_drains_cleanly() {
        assert_eq!(
            serve(knowledge_app_spec(Config::default(), OutboxStore::new())),
            Ok(()),
            "the knowledge service boots → … → drains cleanly"
        );
    }

    #[test]
    fn wired_appspec_registers_the_living_doc_consumer_and_drains() {
        let spec = knowledge_app_spec_with_consumers(
            Config::default(),
            OutboxStore::new(),
            &["myelin://acme/issues/", "myelin://acme/ci/"],
            DedupLedger::new(),
        )
        .expect("the concrete living-document subjects are valid");
        assert_eq!(
            spec.consumers.len(),
            1,
            "exactly the one living-doc consumer is wired"
        );
        assert_eq!(
            serve(spec),
            Ok(()),
            "the wired knowledge service boots → migrates → relay → consumer → drains cleanly"
        );
        assert!(knowledge_app_spec(Config::default(), OutboxStore::new())
            .consumers
            .is_empty());
    }

    #[test]
    fn wired_appspec_rejects_a_wildcard_consumer_subject() {
        let error = match knowledge_app_spec_with_consumers(
            Config::default(),
            OutboxStore::new(),
            &["*"],
            DedupLedger::new(),
        ) {
            Ok(_) => panic!("a wildcard must reject the complete consumer topology"),
            Err(error) => error,
        };
        assert_eq!(
            error,
            myelin_events::SubscribeError::WildcardSubject("*".into()),
            "the invalid subscription is surfaced to the composition root"
        );
    }

    #[test]
    fn knowledge_failed_boot_returns_non_zero() {
        let r = boot_knowledge(Config("BAD_POOL".into()), OutboxStore::new());
        assert!(r.is_err(), "a failed knowledge boot returns non-zero (Err)");
    }
}

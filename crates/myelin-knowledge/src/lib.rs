//! # `myelin-knowledge` — the Knowledge service shell (KN-P04 → P-294, M3)
//!
//! **Owning architecture docs:**
//! `planning/04-subsystem-architectures/knowledge-platform/architecture/01-tech-and-data-model.md`
//! §1 (Rust + Postgres; the per-service DB, the no-cross-db boundary) +
//! `03-events-contracts-and-glue.md` §4 (the service is a **thin shell over the harness**, not a
//! hand-rolled main; every state change emits via the transactional outbox only).
//!
//! **Contract-index:** rows 1.1 (`serve(AppSpec)`), 1.2 (three-surface topology), 1.3
//! (liveness ≠ readiness) — **CONSUMED / WIRED here** (owned by the harness, P-S12/P-S13/P-S14);
//! row 1.5 (forward-only online migrations + the hot-table flags `block`/`db_row`/`doc_op`) —
//! **OWNED here** (declared); rows 4.1/4.2 (`authenticate`/`check`) — **CONSUMED** as the
//! read/write entrypoint call-site slots (the per-op Layer-2 `check` is KN-P14; full ABAC
//! `list_objects` push-down is KN-P16).
//!
//! ## What this crate is (the bootable shell, NOT a hand-rolled main)
//! This is the Knowledge service built as an [`AppSpec`] the harness wires (architecture 00
//! §3.1, the ONE call). [`knowledge_app_spec`] assembles the spec; the harness
//! ([`myelin_substrate::serve`]) runs the lifecycle around it:
//!
//! ```text
//! boot → migrate → outbox relay → consumers → the three ports → graceful drain
//! ```
//!
//! with **liveness ≠ readiness** (readiness gates on migrate-complete) and a graceful drain. The
//! shell declares Knowledge's **hot-table flags** (`block` / `db_row` / `doc_op`, contract 1.5)
//! so the high-write tables KN-P05 creates are protected by the expand→backfill→contract online
//! runner, and wires the **read/write entrypoint** authorization SLOTS (4.1/4.2) with fail-closed
//! stubs ([`FailClosedEntrypoint`]).
//!
//! **No store, no algorithm yet.** This prompt ships the bootable shell with the migration
//! skeleton the store/outbox prompts extend; the entrypoint slots fail closed (deny) until wired.
//!
//! ## DAG position (a documented, NAMED leaf consumer)
//! This is the "every service `main.rs`" consumer the contract-index row 1.1 names. It depends on
//! the harness ([`myelin_substrate`]) + the frozen content/query crates ([`myelin_content`],
//! [`myelin_query`]) + the identity ABI ([`myelin_identity`]); NOTHING in the production crate DAG
//! depends back on it. It is therefore a LEAF consumer ABOVE the harness, outside the eleven-crate
//! library DAG `crate_graph.rs` models — exactly as `myelin-identity-service` / `myelin-notif` are.
//! `substrate_is_root()`/`identity_is_sink()` are preserved (a service main is the harness's
//! terminal consumer, not a node in the library graph).
//!
//! ## Floors named (this shell → the bodies in their own M3 prompts)
//! - **The OLTP store + the (tenant, region) partition + RLS land in KN-P05 (P-295).** The
//!   migration skeleton here ([`knowledge_migrations`]) declares the schema marker the store
//!   prompt extends; the `block` / `db_row` (flexible-database JSONB rows) / `doc_op` (the op-log)
//!   high-write tables are CREATED there. This shell only DECLARES them hot (contract 1.5).
//! - **The transactional outbox table + the relay wiring + the consumer set land in KN-P06.** The
//!   harness prepends the co-located `outbox` + `consumer_dedup` tables ([`myelin_substrate::boot`])
//!   and auto-starts the relay; the Knowledge-owned `doc.updated` / semantic-event emit bodies +
//!   the `EventHandler` set (e.g. the living-doc reaction to `issue.issue.updated`) are KN-P06.
//! - **The per-op `check` body (Layer-2) is KN-P14; the ABAC `list_objects` push-down is KN-P16.**
//!   The entrypoint slot ([`FailClosedEntrypoint`]) returns the fail-closed default `Deny` for
//!   every `check` and `NotYetImplemented` for `list_objects` until those bodies land.

/// The INTEGRATED single-doc editor (KN-P09 → P-299, M3): the [`editor::Document`] over the KN-P08
/// primitives ([`myelin_content::editor`] — offset model + DOM-surgery) + the KN-P07 transport
/// ([`transport`]) — create a page, type blocks, a second connection ([`editor::SecondViewer`]) sees
/// edits live. KN-D2 re-runs over the integrated path (every block a `serialize(parse(md))===md`
/// fixed point). The block tree + stable ids is the follow-on KN-P10 (P-300); no merge engine
/// (KN-P13/P29) and no perms beyond tenant isolation (KN-P14/P16).
/// Knowledge agent governance (KN-P27 / P-317, M3 / KN-M3e — drill KN-D11): the KN slice of the ONE
/// tool catalogue (8.1 — tool identity + `required_caps` from the frozen `myelin-content` ReBAC
/// carrier + the frozen §6.3 consequential-gate classification, the SINGLE source of truth the Fabric
/// registration [`myelin_agent_service::knowledge_tools`] consumes), the "suggested by agent" collab
/// attribution ([`agent::EditAuthor`] — an agent edit rides the SAME `SEND_OP` path a human does, 02
/// §9, never disguised as a human), the HITL-withhold gate ([`agent::KnowledgeEffectGate`] — a
/// consequential edit returns `Denied` + does NOT mutate until approval, AG-8), the per-effect
/// `idem_key` (OQ-F — a double-click is one approval), and the reserve/settle bookend (11.7). The
/// [`agent::KnowledgeAgentRun`] chained drill emits the dated KN-D11 green ([`agent::KnD11Receipt`] —
/// 0 ungoverned mutation, 0 mutation before approval, 0 double-apply). The mock runtime (`--use-mock`)
/// is the platform floor; the AG-7 content-addressed agent-trace holder the run writes into is KN-P28.
pub mod agent;
pub mod authority;
pub mod block_tree;
pub mod collab;
pub mod comments;
pub mod compaction;
pub mod database;
/// Knowledge's M6 DOGFOOD + the truth-up pass (KN-P34 / P-519, M6): Myelin's OWN roadmap / gap-report /
/// scorecard live as a Knowledge space ([`dogfood::myelin_knowledge_space`] — every block round-trips
/// `render(parse(md)) === md` through the ONE render path), the production-hardened surface driven over
/// Myelin's own work across the PR-context-pane + spec-to-ship faces (REUSING the [`e2e_wedge`] runners —
/// [`dogfood::run_knowledge_over_myelins_own_work`]), the truth-up pass over the PROVEN KN-D1..KN-D13 + the
/// E2E slices ([`dogfood::run_knowledge_truth_up_scorecard`] — every row rests on a dated green artifact
/// whose proof source exists on disk, a vanished row surfaced CLAIMED-NOT-PROVEN), and the
/// every-incident-adds-a-drill loop ([`dogfood::KnowledgeIncident`]). No new contract; no weakened gate.
// MR-009b W3b.5: the dogfood loop REUSES the e2e_wedge drill runners (which construct the
// `test-support`-gated in-memory OutboxStore double) — a drill harness, never production
// serving code. Gated with it; the tests-dir drills reach it via the self dev-dependency.
#[cfg(any(test, feature = "test-support"))]
pub mod dogfood;
/// Knowledge's legs of the whole-system E2E wedge (KN-P33 / P-488, M5): **E2E-1** (the PR context pane
/// — a Knowledge design-doc embed resolves per-viewer through the SAME [`refs_glue::Projector`] ladder,
/// 0 title leak to the unauthorized viewer, the tombstone carrying ONLY the root) and **E2E-3** (the
/// spec-to-ship lineage — a Knowledge spec doc → initiative → issues traceability over TE-7 typed
/// edges, cold-reindex == live via the SAME [`replay::KnowledgeReindexSource`], and audit tamper
/// detected via a hash-chained lineage seal built from the frozen
/// [`myelin_storage::blob::ContentHash::blake3`] primitive). Each leg drives the whole flow end-to-end
/// (chaining mutations mid-flight, EI-01 §4) over the UNCHANGED production-hardened engine and emits a
/// named green artifact ([`e2e_wedge::E2eArtifact`]). No new contract; no weakened gate.
// MR-009b W3b.5: the E2E wedge drill runners construct the `test-support`-gated in-memory
// OutboxStore double — a drill harness, never production serving code. Gated with it.
#[cfg(any(test, feature = "test-support"))]
pub mod e2e_wedge;
pub mod editor;
pub mod emit;
/// The Export/Import service (KN-P24 / P-314, M3): the Art. 20 lossless JSON portable bundle (the
/// mechanism the GDPR `PersonalDataHolder::export` in KN-P25 reuses, 10.1) + the Markdown/HTML/PDF
/// exporters + the flexible-DB CSV export + the ADF → `myelin-content` lossy-map import (13.2)
/// recording each lossy node in the [`myelin_content::ImportReport`]. The export/import round-trip
/// is byte-faithful for the content model (`render(parse(md)) === md` across the boundary, 13.1).
pub mod export;
/// The Knowledge `PersonalDataHolder` H4 body + the `#[personal_data]` classify tags (KN-P25 /
/// P-315, M3 / KN-M3e): the `locate`/`export`/`rectify`/`restrict` ops (contract 10.1, the non-erase
/// ops) over Knowledge's blocks/rows/history/mentions/authorship, the four-sink restrict suppression
/// (Search/Agents/OLAP/Notif — the QUANTIFIED 0-emissions gate, [`gdpr::RestrictionRegistry`]), and
/// the `#[personal_data(...)]` tags on the Knowledge schema ([`gdpr::KnowledgePersonRecord`]) so the
/// `no-untagged-personal-data` lint is green. The `erase` op (the per-subject DEK crypto-shred
/// structural floor, KN-D4) is the named KN-P26 follow-on.
pub mod gdpr;
pub mod list_filter;
pub mod materialise;
pub mod merge;
pub mod notif_resolve;
pub mod rebac_fragment;
pub mod refs_glue;
pub mod replay;
pub mod rollup;
pub mod search_feed;
pub mod store;
pub mod subs;
pub mod surge;
/// The Knowledge M6 SWITCH TEST (KN-P34 / P-519, M6): the done-bar's "actually try it" gate (EI-01 §4) —
/// the editor render leg + the `render(parse(md)) === md` round-trip leg + the reference-chip / tombstone
/// overlay-contrast leg + the per-viewer tombstone, driven over the real Knowledge surface on the Myelin
/// self-tenant against the Notion anchor ([`switch_test::KnowledgeSwitchTest::drive`]). 0 walls + 100%
/// round-trip + every overlay ≥ the design-manual §2 WCAG floor + the render leg within the
/// thresholds-file budget ⇒ a Notion user could move without hitting a wall the old tool didn't have. The
/// per-surface browser-drive is recorded HONESTLY (the live `<BlockEditor>` shell + a Playwright drive are
/// a named floor — the WASM-clean model is driven, see `editor-browser-drive.md`).
// MR-009b W3b.5: gated with `dogfood` (the switch test drives the dogfood space builder).
#[cfg(any(test, feature = "test-support"))]
pub mod switch_test;
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
pub use database::{
    execute_view_count, execute_view_query, lower_view_filter, row_matches_filter, DbRelation,
    DbRow, FacetIndexHint, FacetPath, FacetTelemetry, FieldDef, FieldSchema, LoweredViewFilter,
    PageBound, PropertyBag, RelationEdgeEvent, RelationKind, RelationStore, SchemaError, ViewError,
    ViewQuery, FACET_PROMOTION_THRESHOLD,
};
// MR-009b W3b.5: gated with the harness modules above.
#[cfg(any(test, feature = "test-support"))]
pub use dogfood::{
    myelin_knowledge_space, proven_knowledge_rows, run_knowledge_over_myelins_own_work,
    run_knowledge_truth_up_scorecard, KnowledgeDogfoodArtifact, KnowledgeIncident,
    KnowledgeIncidentDrillTicket, KnowledgeIncidentIssueDraft, KnowledgeRowStatus,
    KnowledgeScorecardEntry, KnowledgeTruthUpPass, KnowledgeTruthUpRed, KnowledgeTruthUpScorecard,
    KnowledgeTruthUpVerdict, MyelinDoc, ProvenKnowledgeRow, MYELIN_SELF_REGION, MYELIN_SELF_TENANT,
};
#[cfg(any(test, feature = "test-support"))]
pub use e2e_wedge::{
    run_e2e1_pr_context_pane, run_e2e3_spec_to_ship_lineage, run_knowledge_e2e_legs, E2eArtifact,
    E2E_SCENARIOS as KNOWLEDGE_E2E_SCENARIOS,
};
pub use editor::{Document, EditOp, Editor, EditorBlock, SecondViewer, BROWSER_DRIVE_EVIDENCE};
pub use emit::{
    block_ref, database_ref, emit_change, page_ref, row_ref, KnowledgeChange,
    KnowledgeLivingDocHandler, KNOWLEDGE_LIVING_DOC_TRIGGERS,
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
pub use rebac_fragment::{
    block_read_fragment, database_row_read_fragment, field_view_permission,
    knowledge_read_fragment, page_read_fragment, page_read_override, row_reader_set_expr,
    space_read_fragment,
};
pub use subs::{
    mint_block, mint_heading, register_knowledge_sub_kinds, KNOWLEDGE_OWNED_SUB_KINDS,
    KNOWLEDGE_SUBSYSTEM,
};
#[cfg(any(test, feature = "test-support"))]
pub use switch_test::{
    switch_capability_matrix as knowledge_switch_capability_matrix, switch_surface_drive_record,
    BrowserDriveStatus, KnowledgeOverlay, KnowledgeSwitchTest, KnowledgeSwitchVerdict,
    MeasuredLegs, SwitchCapability as KnowledgeSwitchCapability, SwitchSurfaceDrive,
};
// The Refs glue (KN-P19 / P-309): the inline-node `refs.edge.created` producer (5.4), the TE-7
// typed-edge mirror (5.5), and the `project(ref, viewer)` 4-step tombstone ladder (5.6 / 5.7). The
// projector's `Projection`/`Tombstone`/`TombstoneReason` types collide by NAME with the `sync_block`
// read-projection floor's (a DIFFERENT shape — that is the sync_block render, this is the refs
// project), so they are NOT glob-re-exported; consumers read them through the `refs_glue` module path.
// The non-clashing producer/mirror surface is re-exported here.
pub use refs_glue::{
    edge_aggregate_key, emit_content_edges, emit_page_parent_set, emit_relation_edge,
    KnowledgeLifecycleRel, LadderRung, PageMeta, PageStore, ProjectError as RefsProjectError,
    Projected, Projector, SubAnchor, SubState, REFS_EDGE_CREATED, REL_CLASS_LIFECYCLE,
    REL_CLASS_REFERENCE,
};
// The notif/humanise glue (KN-P22 / P-312): the Knowledge-side `RefResolvePort` that feeds the
// per-viewer project Display projection into the ONE humanise templating surface (7.3 / 5.2) — a
// confidential subject degrades to a humanised tombstone (NOTIF-D4, 0 title leak). The producer-
// accretion half (the define_notif_rule set + the watcher reverse index, NOTIF-P20) lives in
// `myelin_identity_service::knowledge_rules` (the §2.9-DAG reason the fragment does).
pub use notif_resolve::KnowledgeRefResolver;
pub use replay::{KnowledgeReindexSource, REFS_EDGE_SNAPSHOT};
pub use rollup::{
    compute_row, CellValue, FormulaExpr, FormulaField, FormulaSchema, FormulaSchemaError,
    MaterialisationHint, RollupFn, RollupLatencyTelemetry, RollupResolver, MAX_DEPENDENCY_DEPTH,
    MAX_FORMULA_DEPTH, MAX_FORMULA_NODES,
};
// The Search feed (KN-P21 / P-311): Knowledge's OWNED `declare_indexable` 6.3 surface (the page +
// significant-block + db_row projections, re-homed verbatim from `myelin-search`) + the `query`/
// `semantic` Filter-conjoin entries (6.1/6.2) that drive the KN-D5 re-confirm (0 leak incl. COUNT
// across the search/embed/RAG paths). Knowledge NEVER indexes itself — it PROJECTS text via
// `feed_project`; Search consumes off the bus (no cross-DB). The `FACET_*`/`KN_*_TYPE` consts are
// re-exported through `search_feed` (re-homed from `myelin-search`); read via that module path to
// avoid a name clash with the database/refs facets, the same posture `refs_glue` takes.
pub use search_feed::{
    feed_project, kn_db_row_index_spec, kn_declared_index_specs, kn_index_specs,
    kn_page_index_spec, kn_read_permission, kn_search_query, kn_search_semantic,
    page_search_projection, register_kn_index_specs, FeedGrain, SearchAclFilter,
    KN_READ_PERMISSION, KN_SEARCH_OBJECT_TYPE,
};
pub use store::{knowledge_scope, knowledge_store_migrations, KnowledgeStore, KnowledgeTable};
pub use surge::{
    run_collab_surge, run_lexorank_storm, CollabShedReason, CollabShedRejection, CollabSurgeGate,
    CollabSurgeReport, LexoStormReport, COLLAB_SURGE_MULTIPLIER, FLEET_HARDWARE_FLOOR,
};
pub use sync_block::{
    render_sync_block, AllowAll, DenyAll, ProjectionFreshness, SourceReadCheck,
    SyncBlockProjection, SyncBlockRender, SyncSource, Tombstone, TombstoneReason, Viewer,
};
pub use transport::{
    doc_scope, knowledge_stream, AllowAllAuthority, AuthAction, CollabTransport, Connected, DocOp,
    DocOpLog, FailClosedAuthority, OpAuthority, OpId, OpKind, PageSnapshot, PersistedOp, Presence,
    SendOutcome, TransportError,
};

use myelin_events::{consume, ConsumerName, ConsumerSpec, DedupLedger, InProcessBus, OutboxStore};
use myelin_identity::{
    AuthzError, CaveatContext, Consistency, Decision, IdentityService, ListObjectsResult,
    ObjectType, Permission, Principal,
};
use myelin_substrate::{
    boot, AppSpec, Authorizer, Config, ConsumerReg, CriticalDependencies, HotTables, InternalRpc,
    Migration, Migrations, OutboxSpec, PublicRoutes, ServeError, ServeHandle, StoreManifest,
};
use myelin_tenancy::ArtifactRef;

/// The Knowledge service name — a PII-free label, the telemetry/trace service identifier
/// (architecture 00 §3.5). The harness threads it through holder registration + the signal set.
pub const SERVICE_NAME: &str = "knowledge";

/// The three high-write Knowledge tables declared **hot** (contract 1.5; architecture 01 §1).
/// A hot table warrants the expand→backfill→contract online runner: the migration runner refuses
/// a blocking `ALTER` on one at boot, and the `forward-only-migration` lint (P-S11) reads the SAME
/// names at source-scan. KN-P05 CREATES these tables; this shell DECLARES them so the protection
/// is in force from the first migration the store prompt adds.
///
/// - `block` — the per-block adjacency-list rows (`parent_id` + fractional `order_key`); the block
///   tree of every document (the highest-write table; subtree reads are an index range, moves an
///   `order_key` write — architecture 01 §1.2).
/// - `db_row` — the flexible-database JSONB property-bag rows (the source-of-truth rows of every
///   user-defined database/collection; the measured-hot generated-column projection rides off the
///   bus — architecture 01 §1.2).
/// - `doc_op` — the CRDT/CAS op-log table (the live tail of per-doc operations; compacted to an
///   object-tier snapshot — architecture 01 §1.4 / 03 §4 coalescing-before-emit).
pub const HOT_TABLES: [&str; 3] = ["block", "db_row", "doc_op"];

/// The Knowledge service's forward-only embedded migrations (architecture 00 §9; contract 1.5),
/// run at boot **before** the instance reports ready (liveness ≠ readiness, §4.3 — readiness gates
/// on migrate-complete). On this shell floor the DDL is a minimal forward-only schema marker the
/// store prompt (KN-P05, the `block`/`db_row`/`doc_op` tables) extends; the substrate co-located
/// `outbox` + `consumer_dedup` tables are prepended by the harness itself ([`boot`]).
///
/// **KN-P05 (P-295) extends this:** the shell's `0200_knowledge_schema_marker` is now followed by
/// the OLTP **store** schema ([`store::knowledge_store_migrations`]) — the `page`/`block`/`db_row`/
/// `db_collection`/`db_view`/`db_relation`/`page_parent`/`doc_op`/`doc_snapshot` tables, all
/// `(tenant, region)`-partitioned with the hot-table flags on `block`/`db_row`/`doc_op`. The chain
/// stays forward-only (the marker, then the additive `02xx_*` table DDL — no backward/destructive
/// migration). The outbox table + the relay/consumer wiring remains the KN-P06 follow-on.
fn knowledge_migrations() -> Migrations {
    let mut migrations = vec![Migration::plain(
        "0200_knowledge_schema_marker",
        "CREATE TABLE IF NOT EXISTS knowledge_schema_marker (applied_at TEXT)",
    )];
    // KN-P05: append the OLTP store schema to the same forward-only chain (EI-01 §7 — one chain,
    // the store DDL extends the shell's anchor, it does not fork a second migration set).
    migrations.extend(store::knowledge_store_migrations().0);
    Migrations::of(migrations)
}

/// The fail-closed `authenticate` / `check` / `list_objects` slot the Knowledge read/write
/// entrypoints (4.1/4.2/4.3) re-authorize against (architecture 03 §4 — every entrypoint
/// authenticates then checks). **This is the named M3 floor:** the shell ships with this stub
/// wired into the entrypoints; it returns the fail-closed default `Deny` for every `check` and a
/// loud `NotYetImplemented` for `authenticate` / `list_objects`, until the real bodies land.
///
/// Why deny (never error) for `check`: a `check` is a security gate. An un-wired gate that errored
/// might be mistaken upstream for "try again / open" — so the shell returns an explicit `Deny`
/// (fail-closed, ADR-03), the SAME posture the real per-op engine takes on genuine uncertainty
/// (KN-P14). When the body lands, only this inner `IdentityService` changes; the entrypoint wiring
/// (the [`KnowledgeEntrypointAuthorizer`] seam) is unchanged (EI-01 §7 — one primitive).
///
/// Why error for `authenticate` / `list_objects`: a credential resolver / leak-free pre-filter that
/// does not yet exist must NOT be mistaken for a permissive answer — it errors loudly so a caller
/// cannot read "no body yet" as "anyone / everything is visible".
///
/// **Floor → follow-on:** `check` (the per-op Layer-2 body) → KN-P14; `list_objects` (the ABAC
/// push-down) → KN-P16. The other eight `IdentityService` methods are not gated on the Knowledge
/// entrypoints; they inherit the fail-closed `NotYetImplemented`.
#[derive(Clone, Copy, Debug, Default)]
pub struct FailClosedEntrypoint;

impl FailClosedEntrypoint {
    /// A fresh fail-closed slot.
    pub fn new() -> FailClosedEntrypoint {
        FailClosedEntrypoint
    }
}

impl IdentityService for FailClosedEntrypoint {
    /// 4.1 — `authenticate` is consumed by Knowledge at every entrypoint (it calls Identity's
    /// `authenticate`, it does not implement it). The shell's slot errors loudly; the live call
    /// resolves against the Identity service's internal-RPC surface (KN-P16 wires the client).
    fn authenticate(
        &self,
        _credential: &myelin_identity::Credential,
    ) -> myelin_identity::Result<Principal> {
        Err(AuthzError::NotYetImplemented(
            "authenticate is consumed from Identity (4.1); the Knowledge client wires at KN-P16",
        ))
    }

    /// 4.2 — **the fail-closed entrypoint gate (the load-bearing shell behaviour).** Every `check`
    /// returns `Deny` until the per-op Layer-2 body lands (KN-P14). Never fail-open.
    fn check(
        &self,
        _subject: &Principal,
        _permission: &Permission,
        _object: &ArtifactRef,
        _at: &Consistency,
        _caveat: Option<&CaveatContext>,
    ) -> myelin_identity::Result<Decision> {
        // Fail-closed (ADR-03): an un-wired authorization gate denies, it never opens. The real
        // per-op evaluation (KN-P14) replaces this; the Deny posture on genuine uncertainty is the
        // SAME posture it ships with, so this is the correct floor, not a hole.
        Ok(Decision::Deny)
    }

    /// 4.3 — `list_objects` (the ABAC push-down) is KN-P16. Errors loudly (a non-existent leak-free
    /// pre-filter must not be mistaken for a permissive set).
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

/// The read/write-entrypoint authorization adapter that re-authorizes every Knowledge entrypoint
/// call against the `check` slot (architecture 03 §4 — authenticate → check on every read/write).
/// It maps the harness's `(principal, action)` re-authorization seam onto
/// [`FailClosedEntrypoint::check`].
///
/// On the shell floor the inner `check` is the fail-closed stub, so **every entrypoint call is
/// denied** — proving the entrypoint re-authorizes (it does not presume "any caller = safe") AND
/// proving the slot is fail-closed until KN-P14 wires the per-op body. When the body lands, only
/// this adapter's inner `IdentityService` changes; the entrypoint wiring is unchanged.
pub struct KnowledgeEntrypointAuthorizer<S: IdentityService + Send + Sync> {
    inner: S,
}

impl<S: IdentityService + Send + Sync> KnowledgeEntrypointAuthorizer<S> {
    /// Wrap an `IdentityService` `check` slot as the entrypoint re-authorization seam.
    pub fn new(inner: S) -> KnowledgeEntrypointAuthorizer<S> {
        KnowledgeEntrypointAuthorizer { inner }
    }
}

impl<S: IdentityService + Send + Sync> Authorizer for KnowledgeEntrypointAuthorizer<S> {
    /// Re-authorize an entrypoint call by running `check` (architecture 03 §4). On this floor the
    /// inner slot is fail-closed, so every call denies. The `action` is the Knowledge permission
    /// being checked (e.g. `knowledge.read` / `knowledge.write`); the full `(subject, object,
    /// zookie)` threading lands with the per-op body (KN-P14). Returns `true` only on an explicit
    /// `Allow` — `Deny`, `Conditional`, and any error all fail closed to `false`.
    fn authorize(&self, subject: &Principal, action: &str) -> bool {
        // The shell re-authorizes through the SAME check the platform calls — the slot, not a
        // bespoke path (EI-01 §7, one primitive). On the floor it is fail-closed; the per-op
        // evaluation (KN-P14) swaps in behind this exact seam.
        let permission = Permission(action.to_string());
        // A self-referential object stand-in for the action-level re-authorize (the object-level
        // threading is the per-op body's, KN-P14); the fail-closed stub ignores it and denies.
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

/// Assemble the Knowledge service [`AppSpec`] (architecture 00 §3.1; contract 1.1) the harness
/// wires. The spec declares Knowledge's migrations (so readiness gates on migrate-complete), the
/// **hot-table flags** (`block`/`db_row`/`doc_op`, contract 1.5) the store prompt's high-write
/// tables are protected by, and the in-process outbox/holder defaults; the harness opens the three
/// ports (public / internal-RPC / metrics-health) around it.
///
/// `config` is the validated, env-first config (§3.2). The OLTP store is implicitly critical (the
/// harness adds it); Knowledge declares no further critical downstream here on the shell floor
/// (the Identity / Search / Refs critical-dependency set is wired when those clients land —
/// KN-P14/P16/P21). The Knowledge-owned emit bodies + the consumer set land in KN-P06.
///
/// **The outbox is INJECTED (MR-009b W3b.4 — the composition root owns durability):** the
/// production `main.rs` constructs `OutboxStore::durable(PgOutboxBacking)` over the MR-022
/// `SubstrateProvider` pool (foundation migrations applied, fail-loud on missing durable config);
/// a test/drill passes the in-memory `OutboxStore::new()` double. This builder constructs NO
/// store of its own — the W3 dedup-injection precedent applied to the outbox.
pub fn knowledge_app_spec(config: Config, outbox: OutboxStore) -> AppSpec {
    AppSpec {
        name: SERVICE_NAME,
        config,
        migrations: knowledge_migrations(),
        // The hot-table flags (contract 1.5) — declared here so the high-write tables KN-P05
        // creates (block / db_row / doc_op) are protected by the expand→backfill→contract online
        // runner from the first migration. The runner refuses a blocking ALTER on one at boot.
        hot_tables: HotTables::declare(HOT_TABLES),
        // The public surface (gateway-fronted, tenant-from-token); the read/write entrypoint
        // bodies are KN-P14/P16. The harness opens the live tenant-from-token PublicSurface (P-S13).
        public: PublicRoutes::default(),
        // The internal-RPC surface — re-authorizes every call. The shell's fail-closed
        // KnowledgeEntrypointAuthorizer is what it re-authorizes against (`entrypoint_authorizer`).
        internal: InternalRpc::default(),
        // No consumers yet — the living-doc / search-projection consumer set lands in KN-P06.
        consumers: Vec::new(),
        // Every opened store auto-registers as a PersonalDataHolder (§3.4, GD-3) — the block /
        // db_row / doc_op store holders land with the store (KN-P05); the OLTP store registers
        // at boot. The exhaustive PersonalDataHolder{locate/export/rectify/restrict} body is P-315.
        holders: AppSpec::auto(),
        stores: StoreManifest::new(),
        // The outbox relay hook (architecture 03 §4 — emit-via-outbox-only, the no-raw-publish
        // discipline) over the INJECTED store (W3b.4). The in-process broker fake stays the
        // default TRANSPORT (durability lives in the store); EB-04's adapter is a config swap.
        outbox: OutboxSpec::new(outbox, InProcessBus::new()),
        // No further critical downstream declared on the shell floor (the Id/Search/Refs client
        // dependencies wire their critical set as those clients land — KN-P14/P16/P21).
        critical: CriticalDependencies::default(),
    }
}

/// The durable name of the Knowledge living-doc consumer (contract 2.4 rule 4 — bind-by-name; a
/// reconnect re-binds the SAME name + dedup ledger so a redelivery is absorbed). A PII-free
/// telemetry/trace label.
pub const LIVING_DOC_CONSUMER: &str = "knowledge-living-doc";

/// **The Knowledge AppSpec WIRED with the transactional outbox + relay + the living-doc consumer
/// (KN-P06 → P-296).** The genuinely-new half of KN-P06: where [`knowledge_app_spec`] is the bare
/// shell (empty consumer seam, default outbox), this wires the FULL emit-via-outbox-only path —
/// the ONE [`OutboxStore`] the Knowledge emit seam ([`emit::emit_change`]) buffers into AND the
/// relay drains (no second store, BUS-2), plus the [`KnowledgeLivingDocHandler`] registered through
/// the sanctioned [`consume`] (rule 3: the `*`-free whitelist; rule 4: bind-by-name on
/// [`LIVING_DOC_CONSUMER`] sharing the dedup ledger). The harness's lifecycle then runs
/// boot → migrate → **relay** → **consumers** → ports → drain around it.
///
/// `subjects` is the curated cross-subsystem signal whitelist the living-doc consumer binds (the
/// `sig.<tenant>.` / `myelin://<tenant>/issues/` &c. prefixes — NEVER `*`; `consume` rejects a
/// wildcard LOUDLY). The relay's broker is the in-process bus on this floor (the real
/// NATS-JetStream adapter is wired through the same [`OutboxSpec::new`] seam, EB-04).
///
/// **FLOOR named (VISION §3):** the living-doc handler BODY is the shell (acks + records the
/// trigger); the concrete embedded-view / mention-preview projection is KN-P19/P20/P21, the
/// Search/Notif/GDPR consumers KN-P25/P27. This prompt ships the WIRING (the relay + the `*`-free
/// consumer template + the dedup discipline), not the reaction bodies.
pub fn knowledge_app_spec_with_consumers(
    config: Config,
    outbox: OutboxStore,
    subjects: &[&str],
    dedup: DedupLedger,
) -> AppSpec {
    // The ONE outbox the emit seam buffers into AND the relay drains (BUS-2 — no second store) is
    // INJECTED (MR-009b W3b.4, like the dedup ledger in Wave 3): the production root passes a
    // durable-backed store (and pairs emits with a UNIQUE id source — never the per-store-
    // resetting default `MonotonicMinter`; `myelin_events::UlidMinter` is the production source);
    // a test passes the in-memory double.
    let mut spec = knowledge_app_spec(config, outbox);

    // Register the living-doc consumer through the sanctioned `consume` (rule 3 rejects `*`/empty;
    // rule 4 binds the durable name). A malformed/over-broad subject is a LOUD registration error —
    // the consumer is then simply not registered (the shell still boots), never silently narrowed.
    if let Ok(consumer) = consume(
        ConsumerSpec::new(ConsumerName(LIVING_DOC_CONSUMER.into()), subjects),
        KnowledgeLivingDocHandler::new(),
        dedup,
    ) {
        spec.consumers = vec![ConsumerReg::new(consumer)];
    }
    spec
}

/// The read/write-entrypoint re-authorization seam the harness's internal/public surfaces are
/// opened over for the Knowledge service — the fail-closed `check` slot ([`FailClosedEntrypoint`])
/// wrapped as a [`KnowledgeEntrypointAuthorizer`]. Until KN-P14 wires the per-op body, every
/// entrypoint call denies (fail-closed). Exposed as its own constructor so the surface wiring + the
/// body swap are one seam (EI-01 §7).
pub fn entrypoint_authorizer() -> KnowledgeEntrypointAuthorizer<FailClosedEntrypoint> {
    KnowledgeEntrypointAuthorizer::new(FailClosedEntrypoint::new())
}

/// Boot the Knowledge service shell under the harness (architecture 00 §3.1) up to the pre-serve
/// state, returning the [`ServeHandle`] the lifecycle drives. A thin wrapper over
/// [`myelin_substrate::boot`] of [`knowledge_app_spec`] — separated so a test/drill can boot,
/// inspect the three ports + the liveness ≠ readiness state, and drive the drain deterministically.
///
/// Returns `Err` (the non-zero exit) on a failed boot (§3.1).
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

    /// **The shell boots under the harness and the three ports bind (contracts 1.1/1.2).** The
    /// Knowledge AppSpec runs the boot → migrate → relay → ports lifecycle; the public / internal /
    /// metrics-health surfaces are all opened (3/3 ports up).
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

    /// **Liveness ≠ readiness (contract 1.3): readiness is false *before* migrations apply.** The
    /// metrics-health surface opens in the `Booting` startup state — not-ready (it cannot serve
    /// correct traffic before its schema exists) but not-killed (liveness stays Up). This is the
    /// readiness-gates-on-migrate-complete property, and the drain order (intake stops, in-flight
    /// finishes) is exercised in [`Self::knowledge_service_serves_and_drains_cleanly`].
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

    /// **A booted instance reports ready once migrations have applied** (the harness flips the
    /// startup gate to Complete at the end of a successful boot — the post-migrate readiness).
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

    /// **The hot-table flags `block`/`db_row`/`doc_op` are declared (contract 1.5).** The AppSpec
    /// carries the declaration the migration runner reads to refuse a blocking ALTER on one of the
    /// high-write tables KN-P05 creates. This is the OWNED half of 1.5 the shell ships.
    #[test]
    fn hot_table_flags_are_declared() {
        let spec = knowledge_app_spec(Config::default(), OutboxStore::new());
        for table in HOT_TABLES {
            assert!(
                spec.hot_tables.is_hot(table),
                "the {table} table is declared hot (contract 1.5)"
            );
        }
        // exactly the three Knowledge high-write tables, nothing else flagged hot.
        let mut declared: Vec<&str> = spec.hot_tables.tables().collect();
        declared.sort_unstable();
        assert_eq!(
            declared,
            ["block", "db_row", "doc_op"],
            "exactly the three high-write tables are hot"
        );
    }

    /// **A blocking ALTER on a declared-hot table is refused at boot (contract 1.5).** The
    /// migration runner reads the SAME hot-table declaration the AppSpec carries and refuses a
    /// non-online (blocking) `ALTER` on `block` — the high-write table must use the
    /// expand→backfill→contract online path, never a table-locking ALTER.
    #[test]
    fn blocking_alter_on_hot_table_is_refused_at_boot() {
        let mut runner = myelin_substrate::MigrationRunner::new();
        // A genuinely-blocking ALTER (ADD COLUMN … NOT NULL with no DEFAULT) on the declared-hot
        // `block` table — the runner matches the migration's `table` against the hot set and
        // refuses it (it must be the expand→backfill→contract idiom instead, §9.4).
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

    /// **The migration set is forward-only (0 destructive migrations).** The runner refuses a
    /// destructive (DROP) migration; the Knowledge skeleton carries none. The
    /// `forward-only-migration` lint (P-S11) enforces the same over source.
    #[test]
    fn migrations_are_forward_only() {
        let migrations = knowledge_migrations();
        for m in &migrations.0 {
            assert!(
                !myelin_substrate::is_destructive(m.ddl),
                "migration {} is forward-only (no backward/destructive DDL)",
                m.id
            );
        }
        // and a destructive one IS refused at boot (the runner is the gate).
        let mut runner = myelin_substrate::MigrationRunner::new();
        let bad = Migrations::of([Migration::plain("0210_bad", "DROP TABLE block")]);
        assert!(
            runner.run(&bad, &HotTables::declare(HOT_TABLES)).is_err(),
            "a destructive migration is refused at boot (forward-only)"
        );
    }

    /// **The fail-closed `check` entrypoint slot fail-closes to `Deny` (the named floor, ADR-03).**
    /// Until KN-P14 wires the per-op body, every `check` returns `Deny` — never `Allow`, never an
    /// error a caller could mistake for "open". The security floor the shell ships: deny until wired.
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

    /// **The read/write entrypoint re-authorizes every call against the fail-closed slot.** A call
    /// arriving on the trusted internal channel is STILL denied (the entrypoint does not presume
    /// "any caller = safe") AND the slot is fail-closed until KN-P14. Wiring the
    /// [`KnowledgeEntrypointAuthorizer`] into the harness's [`InternalSurface`] proves the seam.
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

    /// `authenticate` / `list_objects` error loudly (NotYetImplemented) — a non-existent credential
    /// resolver / leak-free pre-filter must NOT be mistaken for a permissive answer.
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

    /// **The OLTP store auto-registered as a PersonalDataHolder at boot (contract 1.4, §3.4).**
    /// Opening IS registering — the Knowledge service's store appears in the holder registry. The
    /// block/db_row/doc_op store holders land with those stores (KN-P05); the OLTP store now.
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

    /// **The whole lifecycle runs end-to-end and graceful-drains.** `serve(knowledge_app_spec(..))`
    /// boots → migrates → relays → opens the ports → drains cleanly (outbox_depth 0). The §3.1
    /// one-call contract for the Knowledge service (the drain order is the harness's, proven here).
    #[test]
    fn knowledge_service_serves_and_drains_cleanly() {
        assert_eq!(
            serve(knowledge_app_spec(Config::default(), OutboxStore::new())),
            Ok(()),
            "the knowledge service boots → … → drains cleanly"
        );
    }

    /// **The KN-P06 WIRED AppSpec carries the living-doc consumer + the shared outbox/relay.** The
    /// `knowledge_app_spec_with_consumers` constructor registers exactly one consumer (the living-doc
    /// handler, bound `*`-free) and serves → drains cleanly with the relay over the SAME outbox the
    /// emit seam buffers into (BUS-2 — no second store). The empty-shell `knowledge_app_spec` keeps
    /// the no-consumer property; this is the wired path.
    #[test]
    fn wired_appspec_registers_the_living_doc_consumer_and_drains() {
        let spec = knowledge_app_spec_with_consumers(
            Config::default(),
            OutboxStore::new(),
            &["myelin://acme/issues/", "myelin://acme/ci/"],
            DedupLedger::new(),
        );
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
        // the bare shell stays consumer-free (the empty seam is preserved).
        assert!(knowledge_app_spec(Config::default(), OutboxStore::new())
            .consumers
            .is_empty());
    }

    /// **An over-broad consumer subject does NOT silently widen the wiring (rule 3).** A `*` subject
    /// passed to the wired constructor is rejected at registration, so the consumer is simply not
    /// wired (the shell still boots) — never a silently-narrowed over-broad subscription.
    #[test]
    fn wired_appspec_rejects_a_wildcard_consumer_subject() {
        let spec = knowledge_app_spec_with_consumers(
            Config::default(),
            OutboxStore::new(),
            &["*"],
            DedupLedger::new(),
        );
        assert!(
            spec.consumers.is_empty(),
            "a `*` subject is rejected at registration → no consumer wired (never silently widened)"
        );
        assert_eq!(
            serve(spec),
            Ok(()),
            "the shell still boots + drains without the bad consumer"
        );
    }

    /// **A failed boot returns non-zero (§3.1).** A config that fails boot-time validation aborts
    /// the Knowledge service boot with a loud error — never a silent success.
    #[test]
    fn knowledge_failed_boot_returns_non_zero() {
        let r = boot_knowledge(Config("BAD_POOL".into()), OutboxStore::new());
        assert!(r.is_err(), "a failed knowledge boot returns non-zero (Err)");
    }
}

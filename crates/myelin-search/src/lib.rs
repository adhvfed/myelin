//! # `myelin-search` — the Search & Indexing service crate (SRCH-P01 → P-021, M0 floor)
//!
//! Search is overwhelmingly a **consumer** (architecture
//! `planning/05-refined-shared-systems-architecture/search-and-indexing.md` §0/§1): it owns
//! NO contract crate (it composes `myelin-query`, `myelin-identity`, `myelin-events`,
//! `myelin-gdpr`, `myelin-content`) and holds only derived, reconstructible state. Its entire
//! correctness story is downstream of Identity 4.3 (the `list_objects` `SetExpr` push-down).
//!
//! ## What this crate carries at M0 — the index-document NAME ANCHORS only (no mechanism)
//!
//! SRCH-P01 is the **Search ratchet** band (S-M0). Two deliverables land in M0:
//!
//! 1. **The `search-requires-acl-filter` lint** (contract 1.6) — a committed, permanent
//!    ratchet gate that fails any query path which reaches the private `engine.search` entry
//!    without a composed ACL `Filter` clause. **Reconciliation (coherence rule, EI-01 §7):**
//!    the lint, its engine, and its red+green fixtures were FIRST shipped by the substrate
//!    prompt P-S11 → P-018 (the remaining eight architecture lints; the lint harness is shared
//!    substrate). The P-018 source explicitly NAMES this prompt as the owner of the same row:
//!    "the Search subsystem also ships its OWN `search-requires-acl-filter` twin
//!    (SRCH-P01 / P-021)." Per EI-01 §7 (never define a lint twice, never build a parallel
//!    second scanner) SRCH-P01 **confirms and re-asserts the lint in place** rather than
//!    duplicating it: see [`mod tests`] below, which re-runs the lint over the red+green
//!    fixtures from THIS crate's perspective (the Search-owned proof the gate rejects the
//!    bypass path and admits the composed path), and the lint's own home is
//!    `myelin_lints::lints::search_requires_acl_filter` (matrix-wired in
//!    `crates/myelin-lints/tests/fixture_matrix.rs`, CI-wired via the `lint-gate` binary). The
//!    lint exists BEFORE the query path it guards (the M2 follow-on, below), so that path can
//!    never be written without it.
//!
//! 2. **The index-document name anchors** (this module) — the field/unit NAMES of the Search
//!    index document, anchored to the frozen `EventEnvelope` (contract 2.1, the names/units
//!    authority X-5) + the `ArtifactRef` token (contract 5.1, the `doc_id` key). NO mechanism:
//!    just the names, so the S-M2 build (the real index document + the IndexBackend) does not
//!    drift from the envelope. A later rename of an envelope field breaks the drift test
//!    ([`tests::index_doc_anchors_match_the_frozen_envelope`]) NOW, not in prod
//!    (EI-01 §7: reconcile names/units up front).
//!
//! ## The index document — the canonical projection (architecture §3.1)
//!
//! The Search index document (Phase-3 §3.1, CONFIRMED in Phase-5 §3.1) is the projection of an
//! `EventEnvelope` (+ the owner's content) into a searchable, **encrypted-from-birth**,
//! `(tenant, region)`-keyed document. Its field/unit anchors, each tied to its frozen source:
//!
//! | index-doc field        | unit / shape                               | frozen source              |
//! |------------------------|--------------------------------------------|----------------------------|
//! | `doc_id`               | the `ArtifactRef` key string               | `ArtifactRef` (5.1) ⟸ envelope `subject` |
//! | `tenant`               | `TenantId` — partition + residency, FIRST  | envelope `tenant` (2.1)    |
//! | `region`               | `Region` — partition + residency, FIRST    | envelope `region` (2.1)    |
//! | `acl_object`           | the `ArtifactRef` the ACL filter pins on   | envelope `subject` (the cheap pre-filter key, §3.1) |
//! | `acl_object_type`      | the artifact-type segment of the key       | derived from `subject` (§3.1) |
//! | `indexed_zookie`       | the consistency token at index time        | the staleness anchor (§3.1) |
//! | `version`              | monotonic projection version               | the staleness anchor (§3.1) |
//! | `lang`                 | analyzer-selection language tag            | analyzer selection (§3.1)  |
//! | `contains_personal_data` | `bool` — GDPR fan-out routing            | envelope `contains_personal_data` (2.1) |
//! | `data_role`            | controller \| processor                    | envelope `data_role` (2.1) |
//! | `visibility`           | public \| internal \| private (routing HINT) | envelope `visibility` (2.1) |
//! | `pii_key_ref`          | `kms://<tenant>/<dek-epoch>/<class>` URN   | envelope `pii_key_ref` (2.1) |
//!
//! `doc_id = the ArtifactRef key` and `tenant`/`region` come **first** (the partition +
//! residency key, never optional). `indexed_zookie + version` are the **staleness anchor** (the
//! zookie/consistency path, SRCH-P10). `lang` selects the analyzer chain (SRCH-P12). The four
//! GDPR routing fields (`contains_personal_data`/`data_role`/`visibility`/`pii_key_ref`) are
//! carried verbatim from the source envelope so Search-as-a-holder erase + the RoPA/data-map
//! fan-out reach the index (SRCH-P02 registers the holder; SRCH-P15 implements the real erase).
//!
//! ## FLOOR (named) — there is NO query engine here
//!
//! This crate is a **ratchet, not a feature** (the prompt's "FLOOR named: none"). State plainly:
//! **the query path the `search-requires-acl-filter` lint guards is the S-M2 follow-on**
//! (SRCH-P08 / P-171 — the permission-aware query pipeline — and SRCH-P09 / P-172 — the
//! `SetExpr` reverse-index JOIN). The service shell (`serve(AppSpec)`), the `IndexBackend`
//! trait, Tantivy, the vector HNSW shape, and the incremental indexer are SRCH-P03..SRCH-P07.
//! Nothing here must be mistaken for a working index: this M0 deliverable is the committed lint
//! plus the name anchors that keep the M2 build from drifting off the frozen envelope. The
//! anchors below are `const` field-NAME strings, deliberately carrying no runtime mechanism.
//!
//! ## What SRCH-P02 (P-122, M1) adds — holder registration + the per-tenant index DEK + residency
//!
//! On top of the M0 ratchet, SRCH-P02 (M1) registers Search into the platform GDPR/KMS/residency
//! substrate so the S-M2 index is encrypted-from-birth and the M5 DSAR fan-out cannot miss it.
//! **Reconciliation (coherence, EI-01 §7):** this is the EXACT analog of REF-P3/REF-P4 (P-120/P-121,
//! the Refs `myelin-refs-service` holder registration + per-tenant DEK pin). Refs needed a SEPARATE
//! `-service` crate because its glue crate is a modelled node in the eleven-crate library DAG
//! (`myelin-substrate::crate_graph`) that may not depend on `-gdpr`; **`myelin-search` is already a
//! consumer/service crate OUTSIDE that modelled DAG** (it is not a `Crate::*` node — substrate's
//! `crate_graph` does not model it), so SRCH-P02 EXTENDS this crate in place rather than spawning a
//! second one. It pulls in `-gdpr` / `-storage` / `-substrate` as the consumer dependencies the
//! holder + DEK + residency confirmation need.
//!
//! - [`holder`] — Search as a real, registered `PersonalDataHolder` (**H7 `SearchIndex`**) over its
//!   ONE store (the per-tenant index, §3.4), registered through the substrate holder registry
//!   (contract 1.4) so the H1–H18 list is exhaustive before any tenant data exists (10.1). At M1 a
//!   STUB surface: `locate`/`export` return empty-but-correct, `restrict`/`rectify`/`erase` are
//!   well-defined no-ops. The REAL erase — PURGE + REINDEX (the primary per-subject erasure), vectors
//!   compacted, restrict suppression — lands in **SRCH-P15**.
//! - [`dek`] — the Search **per-tenant index DEK** reserved in the cell's ONE KMS hierarchy
//!   (`myelin_storage::KmsEngine`, 11.3 / 11.4) so the (future) index is **encrypted-from-birth**,
//!   with **destroy callable** (the tenant-decommission crypto-shred + backup backstop) + the
//!   **per-subject SOURCE-DEK backstop** (§4.8, the added backstop) + the **HYOK structural-skip**
//!   reference ([`dek::hyok_skips_index`]) + the inherited-M1-gate precondition list named for
//!   SRCH-P03 ([`dek::srch_p03_inherited_gates`]).
//! - [`residency`] — the `(tenant, region)` + residency-tag store descriptor that confirms the
//!   residency-pin applies (no cross-region index read on personal data, §1/§3.4) + links the
//!   residency-pin / tenant-predicate lints.
//! - [`erasure_posture`] — the record that Search instantiates the ONE platform free-text/immutable
//!   erasure posture (X-7 / 10.9) **by reference** and adds NO new `[OPEN — LEGAL]` residual.
//!
//! **SRCH-P02 FLOOR (named):** THE floor is the per-tenant index DEK (the crypto-shred + backup
//! backstop unit) — reserved + destroyable here. The **PRIMARY per-subject erasure by purge +
//! reindex** is the follow-on, landing in **SRCH-P15** once the index exists; the DEK is NOT the
//! whole erasure answer. No index / layout / migration / real ciphertext / real vectors ship here.

pub mod analysis;
pub mod cache;
pub mod canonical;
pub mod chat_projection;
pub mod ci_log_projection;
pub mod compiler;
pub mod consistency;
pub mod cross_cell;
pub mod dek;
pub mod dogfood;
pub mod e2e_wedge;
pub mod engine;
pub mod erase;
pub mod erasure_posture;
pub mod filtered_ann;
pub mod freshness;
pub mod fusion;
pub mod git_code_projection;
pub mod holder;
pub mod hyok_scale;
pub mod indexer;
pub mod issues_projection;
pub mod kn_projection;
pub mod layout;
pub mod object_store_backstop;
pub mod pipeline;
pub mod projection_feeder;
pub mod reindex;
pub mod residency;
pub mod restore_verify;
pub mod shell;
pub mod subartifact;
pub mod surge;
pub mod switch_test;
pub mod telemetry;
pub mod tier3_valve;
pub mod vector;

pub use cache::{
    should_bypass, zookie_bucket, CacheStats, CacheTtl, FilterCache, ResultCache,
    TtlExceedsRevocationSla,
};
pub use chat_projection::{
    message_doc_ref, message_index_spec, message_index_specs, message_search_projection,
    register_message_index_specs, ChatFiveProducerCorpusNotWorldScaleFloor, CHAT_SUBSYSTEM,
    MESSAGE_ACL_OBJECT_TYPE, MESSAGE_TYPE,
};
pub use ci_log_projection::{
    ci_log_details_ref, ci_log_doc_ref, ci_log_index_spec, ci_log_index_specs,
    ci_log_search_projection, parse_step_anchor, register_ci_log_index_specs,
    CiLogDurableSegmentNotFirehoseFloor, CiLogProjectionInput, CiLogStepAnchor,
    CI_LOG_ACL_OBJECT_TYPE, CI_LOG_TYPE, CI_SUBSYSTEM, FACET_JOB_ID as CI_LOG_FACET_JOB_ID,
    FACET_RUN_ID as CI_LOG_FACET_RUN_ID, FACET_STEP_NO as CI_LOG_FACET_STEP_NO,
};
pub use compiler::{
    compile, render, CompileError, CompiledPlan, ConjoinedPlan, FieldDecl, FieldKind, FieldSchema,
    FtClause, PostFetchPredicate, Sort, StructuredClause, VectorBranch, FT_BODY_FIELD,
    SEMANTIC_FIELD, SORT_FIELD,
};
pub use consistency::{
    disposition, fail_static_bypass, stale_candidates, BoundedCheckPort, CandidateDisposition,
    ConsistencyStats,
};
pub use dek::{hyok_skips_index, srch_p03_inherited_gates, InheritedGate, SearchDekPin};
pub use dogfood::{
    proven_search_rows, run_search_truth_up_scorecard, DogfoodArtifact, ProvenSearchRow,
    SearchIncident, SearchIncidentDrillTicket, SearchIncidentIssueDraft, SearchRowStatus,
    SearchScorecardEntry, SearchTruthUpPass, SearchTruthUpRed, SearchTruthUpScorecard,
    SearchTruthUpVerdict, EMBEDDING_ADAPTER_POSTURE, MYELIN_SELF_REGION, MYELIN_SELF_TENANT,
};
// The in-process dogfood/E2E drill runners construct the in-memory KMS test double — MR-009b
// Wave 5: `test-support`-gated (the tests-dir drills reach them via the self dev-dependency).
#[cfg(any(test, feature = "test-support"))]
pub use dogfood::run_search_over_myelins_own_work;
pub use e2e_wedge::{run_e2e_1_pr_pane, E2eArtifact, E2E_SCENARIOS};
#[cfg(any(test, feature = "test-support"))]
pub use e2e_wedge::{run_e2e_3_spec_to_ship, run_e2e_4_dsar_fanout, run_search_e2e_wedge};
pub use engine::{
    AclFilter, Hit, IndexBackend, IndexDocument, IndexError, SubjectMatcher, TantivyBackend,
    DEFAULT_SUBJECT_LOCATOR_FACETS, ORDER_KEY_FIELD,
};
pub use erase::{EraseOutcome, SearchEraseHolder, SEARCH_ERASE_EVENT_TYPE};
pub use erasure_posture::{erasure_posture, ErasurePosture};
pub use filtered_ann::{
    measure_recall_at_k, FilteredAnnArtifact, FilteredAnnFailure, FilteredAnnGate,
    FilteredAnnStrategy, FilteredAnnVerdict, RecallMeasurement,
};
pub use freshness::{
    fresh_indexer, measure_event_to_searchable, p99_ms, FreshnessArtifact, FreshnessFailure,
    FreshnessGate, FreshnessVerdict, FRESHNESS_P99_SEED_MS,
};
pub use fusion::{fuse_with_k, reciprocal_rank_fusion, FusedHit, RankedList, RRF_K};
pub use git_code_projection::{
    git_blob_search_projection, git_code_projection_spec, git_index_specs,
    register_git_index_specs, trigram_query, trigrams, GitBlobProjectionInput,
    ScipLsifFindUsagesFloor, FACET_BLOB_OID as GIT_FACET_BLOB_OID,
    FACET_LANGUAGE as GIT_FACET_LANGUAGE, FACET_PATH as GIT_FACET_PATH, GIT_BLOB_ACL_OBJECT_TYPE,
    GIT_BLOB_TYPE, GIT_SUBSYSTEM, TRIGRAM_N,
};
pub use holder::{
    register_search_holder, search_index_holder, SearchHolderRegistration, SearchIndexHolder,
    SEARCH_INDEX_STORE,
};
pub use hyok_scale::{
    backup_scale_page_spec, build_live_corpus, subject_matcher, BackupScaleEraseArtifact,
    BackupScaleEraseFailure, BackupScaleEraseGate, BackupScaleEraseInputs, BackupScaleEraseVerdict,
    DerivedStore, HyokCrossStoreArtifact, HyokCrossStoreFailure, HyokCrossStoreGate,
    HyokCrossStoreInputs, HyokCrossStoreVerdict, MapFetcher, SealedBackupSegment,
};
pub use indexer::{
    EmbeddingAdapter, IncrementalIndexer, IndexEventError, IndexSpec, MockEmbeddingAdapter,
    ProjectFetchError, ProjectFetcher, SearchProjection, INDEXER_CONSUMER,
    INDEXER_SUBJECT_PREFIXES,
};
pub use issues_projection::{
    issue_index_spec, issue_index_specs, issue_search_projection, register_issue_index_specs,
    IssueGinScanProjectionFeederFloor, IssueProjectionInput,
    FACET_ASSIGNEE as ISSUE_FACET_ASSIGNEE, FACET_CYCLE_ID as ISSUE_FACET_CYCLE_ID,
    FACET_PRIORITY as ISSUE_FACET_PRIORITY, FACET_PROJECT_ID as ISSUE_FACET_PROJECT_ID,
    FACET_STATE_CATEGORY as ISSUE_FACET_STATE_CATEGORY, FACET_TYPE_RANK as ISSUE_FACET_TYPE_RANK,
    ISSUE_ACL_OBJECT_TYPE, ISSUE_PRODUCER_RANK_FACET, ISSUE_SUBSYSTEM, ISSUE_TYPE,
};
pub use kn_projection::{
    kn_db_row_index_spec, kn_index_specs, kn_page_index_spec, page_search_projection,
    register_kn_index_specs, FACET_ARTIFACT_REF, FACET_EMBED, FACET_MENTION, KN_DB_ROW_TYPE,
    KN_PAGE_TYPE, KN_SUBSYSTEM,
};
pub use layout::{
    derived_state_invariant_holds, srch_p03_floors, LayoutError, PerTenantIndexLayout,
    SrchP03Floor, StatefulComponent,
};
pub use object_store_backstop::{
    ObjectStoreBackstopArtifact, ObjectStoreBackstopFailure, ObjectStoreBackstopGate,
    ObjectStoreBackstopVerdict, SegmentBackstop, StoredSegment, SwappedSegments,
};
pub use pipeline::{
    query, query_consistent, semantic, ListObjectsPort, Page, QueryError, QueryStats, RankedResult,
    RankedResults, RelationalLeaf, ReverseIndexAnswer, RevisionWatermark, ScopedEngine,
    VectorQuery, READ_PERMISSION,
};
pub use projection_feeder::{
    FacetCollection, FacetDoc, FacetServingPath, ProjectionFeederArtifact, ProjectionFeederFailure,
    ProjectionFeederGate, ProjectionFeederVerdict, ViewExecutionTelemetry,
};
pub use reindex::{
    ReindexCursorStore, ReindexError, ReindexJob, ReindexProgress, SearchReindexer,
    DEFAULT_BATCH_CAP, DEFAULT_MAX_IN_FLIGHT_PER_TENANT,
};
pub use residency::{search_store_descriptors, SearchStoreDescriptor};
pub use restore_verify::{
    ErasedSubjectEntry, SearchErasureLedger, SearchRestoreArtifact, SearchRestoreFailure,
    SearchRestoreInputs, SearchRestoreVerdict, SearchRestoreVerifyGate,
};
pub use shell::{
    boot_search, run_search, search_app_spec, search_service_migrations,
    SEARCH_INDEX_DIR_MIGRATION, SERVICE_NAME,
};
pub use subartifact::{
    block_subdoc_projection, db_field_subdoc_projection, db_row_subdoc_projection,
    line_range_subdoc_facets, line_range_subdoc_projection, AnchorState, ContentAnchoredSpan,
    M4ProducerSubAnchorFloor, SubGrain, FACET_ANCHOR_STATE, FACET_LINE_END, FACET_LINE_START,
};
pub use surge::{
    run_search_surge, SearchShedGate, SearchShedRejection, SearchSurgeReport,
    FILTERED_ANN_FOLLOW_ON, SEARCH_SURGE_MULTIPLIER, SHARD_SPLIT_IS_MEASURED_ONLY,
};
pub use switch_test::{
    switch_capability_matrix, switch_surface_drive_record, BrowserDriveStatus, MeasuredLatencies,
    SearchSwitchTest, SearchSwitchVerdict, SwitchCapability, SwitchSurfaceDrive,
};
pub use telemetry::{
    signal as telemetry_signal, LabelledSignal, RedLabels, SearchTelemetry, CACHE_RATIO_ABSENT,
};
pub use tier3_valve::{
    board_acl_filter, escalate_to_search, oltp_board_admits, BoardEscalationAuthz, BoardQuery,
    OltpBudget, ReverseResolver, Tier3ValveSurgeFloor,
};
pub use vector::{Embedding, HnswVectorIndex, ModelRef, VectorHit, VectorRecord};

/// The frozen field-NAME of the Search index document's `doc_id` — the
/// [`ArtifactRef`](myelin_events::ArtifactRef) key (architecture §3.1; contract 5.1). This is
/// the envelope's `subject` ref, used verbatim as the document's primary key. NO mechanism: this
/// is the NAME, anchored so the S-M2 index build cannot drift from the frozen ref.
pub const DOC_ID: &str = "doc_id";

/// The `tenant` field name — the [`TenantId`](myelin_events::TenantId) partition + residency
/// key, carried FIRST (architecture §3.1; envelope 2.1 `tenant`). Never optional.
pub const TENANT: &str = "tenant";

/// The `region` field name — the [`Region`](myelin_events::Region) partition + residency key,
/// carried FIRST (architecture §3.1; envelope 2.1 `region`). Never optional.
pub const REGION: &str = "region";

/// The `indexed_zookie` field name — half the staleness anchor (the consistency token captured
/// at index time; architecture §3.1). Pairs with [`VERSION`]; read by the zookie/consistency
/// path (the SRCH-P10 follow-on). NO mechanism here — the NAME only.
pub const INDEXED_ZOOKIE: &str = "indexed_zookie";

/// The `version` field name — the other half of the staleness anchor (the monotonic projection
/// version; architecture §3.1). Pairs with [`INDEXED_ZOOKIE`].
pub const VERSION: &str = "version";

/// The `lang` field name — the analyzer-selection language tag (architecture §3.1; the
/// per-language analyzer chain is the SRCH-P12 follow-on).
pub const LANG: &str = "lang";

/// The `contains_personal_data` field name — GDPR fan-out routing, carried verbatim from the
/// source envelope (envelope 2.1 `contains_personal_data`). One of the four GDPR routing fields
/// so Search-as-a-holder erase + the RoPA fan-out reach the index.
pub const CONTAINS_PERSONAL_DATA: &str = "contains_personal_data";

/// The `data_role` field name — controller | processor (envelope 2.1 `data_role`). A GDPR
/// routing field. Typed by [`DataRole`](myelin_events::DataRole) at the source.
pub const DATA_ROLE: &str = "data_role";

/// The `visibility` field name — public | internal | private routing HINT, never an authz
/// decision (envelope 2.1 `visibility`; Id decides). A GDPR routing field. Typed by
/// [`Visibility`](myelin_events::Visibility) at the source.
pub const VISIBILITY: &str = "visibility";

/// The `pii_key_ref` field name — the `kms://<tenant>/<dek-epoch>/<class>` URN of the inline-PII
/// envelope key (envelope 2.1 `pii_key_ref`). A GDPR routing field. Typed by
/// [`PiiKeyRef`](myelin_events::PiiKeyRef) at the source. Present only on inline-PII documents.
pub const PII_KEY_REF: &str = "pii_key_ref";

/// The names/units anchor the prompt's TESTS field enumerates:
/// `doc_id / tenant / region / indexed_zookie / version / lang` + the four GDPR routing fields.
/// This array is the canonical list the drift test reconciles against the frozen
/// [`EventEnvelope`](myelin_events::EventEnvelope) field set — a later rename of an envelope
/// field (or of one of these anchors) breaks the test NOW, not in prod (EI-01 §7).
pub const INDEX_DOC_ANCHORS: [&str; 9] = [
    DOC_ID,
    TENANT,
    REGION,
    INDEXED_ZOOKIE,
    VERSION,
    LANG,
    CONTAINS_PERSONAL_DATA,
    DATA_ROLE,
    VISIBILITY,
    // pii_key_ref is the tenth in the GDPR routing set but is GROUPED with data_role/visibility
    // in the test below; it is asserted explicitly there. It is intentionally the trailing
    // optional-on-the-wire field, so the named-nine here is the prompt's enumerated core
    // (doc_id/tenant/region/indexed_zookie/version/lang + the 3 always-present GDPR fields).
    // (kept the array exactly at the prompt's named set; PII_KEY_REF asserted separately.)
];

/// The subset of [`INDEX_DOC_ANCHORS`] whose NAMES are carried verbatim from the frozen
/// `EventEnvelope` (so a rename of an envelope field MUST be matched here). The remaining
/// anchors (`doc_id`, `indexed_zookie`, `version`, `lang`) are Search-derived projection fields
/// (the `doc_id` is the envelope `subject`'s ArtifactRef key under a Search-local name; the
/// staleness + analyzer fields are Search-owned) and are anchored by §3.1, not by an envelope
/// field name.
pub const ENVELOPE_DERIVED_ANCHORS: [&str; 5] = [
    TENANT,
    REGION,
    CONTAINS_PERSONAL_DATA,
    DATA_ROLE,
    VISIBILITY,
];

#[cfg(test)]
mod tests {
    use super::*;
    use myelin_events::{
        ArtifactRef, DataRole, EventEnvelope, PiiKeyRef, Region, TenantId, Visibility,
    };

    /// Build a canonical `EventEnvelope` so the drift test reads the REAL frozen field names off
    /// a value (a rename of a struct field stops this constructor compiling — the names/units
    /// drift is caught at compile time, the value-level assertions catch a wire/serde rename).
    fn sample_envelope() -> EventEnvelope {
        use myelin_events::{Actor, AggregateKey, CorrelationId, EventId, EventType, Timestamp};
        use myelin_identity::{Principal, PrincipalId, PrincipalKind};
        EventEnvelope {
            event_id: EventId("01J0".into()),
            type_: EventType("search.doc.indexed".into()),
            schema_ver: 1,
            tenant: TenantId("acme".into()),
            region: Region("eu-west".into()),
            actor: Actor(Principal::stub(
                PrincipalId("p".into()),
                PrincipalKind::Human,
                TenantId("acme".into()),
            )),
            subject: ArtifactRef("myelin://acme/issues/issue/ENG-1421".into()),
            aggregate: AggregateKey("issue:ENG-1421".into()),
            causation_id: None,
            correlation_id: CorrelationId("01J0".into()),
            caused_by: None,
            depth: 0,
            contains_personal_data: true,
            data_role: DataRole::Controller,
            visibility: Visibility::Internal,
            pii_key_ref: Some(PiiKeyRef("kms://acme/3/subject:u42".into())),
            occurred_at: Timestamp("2026-06-19T00:00:00Z".into()),
            recorded_at: Timestamp("2026-06-19T00:00:01Z".into()),
            payload: serde_json::json!({ "ref": "myelin://acme/issues/issue/ENG-1421" }),
        }
    }

    /// **SRCH-P01 GATE artifact (the names/units drift test, 2026-06-19).** The index-document
    /// NAME anchors match the frozen `EventEnvelope` (contract 2.1) field list + the
    /// `ArtifactRef` (5.1) `doc_id` key. The prompt's TESTS field: assert the anchor names
    /// `doc_id / tenant / region / indexed_zookie / version / lang` + the GDPR routing fields
    /// match the frozen envelope field list, so a later rename of an envelope field breaks THIS
    /// test now, not in prod (EI-01 §7, reconcile names/units up front).
    #[test]
    fn index_doc_anchors_match_the_frozen_envelope() {
        let env = sample_envelope();

        // The frozen envelope serialised key set (the X-5 wire anchor; the same key set the Bus
        // provider-side CDC test pins). The index-doc's envelope-derived anchors MUST be a
        // SUBSET of these keys — a rename/drop of one of those envelope fields makes the
        // corresponding anchor absent here and fails the assertion.
        let json = serde_json::to_value(&env).expect("envelope serialises");
        let obj = json.as_object().expect("envelope is a JSON object");

        for anchor in ENVELOPE_DERIVED_ANCHORS {
            assert!(
                obj.contains_key(anchor),
                "index-doc anchor `{anchor}` no longer matches a frozen EventEnvelope field — \
                 the envelope (contract 2.1) was renamed; reconcile the Search index-doc anchor \
                 (EI-01 §7, names/units drift caught up front, not in prod)"
            );
        }

        // `doc_id` is the ArtifactRef key (5.1) = the envelope `subject`. Anchor the NAME of the
        // source ref so a rename of the envelope's `subject` field is caught here too.
        assert!(
            obj.contains_key("subject"),
            "the index `doc_id` is the envelope `subject` ArtifactRef key (5.1) — the envelope \
             `subject` field was renamed; reconcile DOC_ID"
        );
        // The doc_id value is the ArtifactRef key string verbatim (the canonical projection key).
        let subject = &env.subject;
        assert_eq!(
            subject.0,
            obj["subject"]
                .as_str()
                .expect("subject serialises as a string"),
            "doc_id is the ArtifactRef key string verbatim (architecture §3.1)"
        );

        // The four GDPR routing fields carried verbatim from the envelope are all present.
        for gdpr in [CONTAINS_PERSONAL_DATA, DATA_ROLE, VISIBILITY, PII_KEY_REF] {
            assert!(
                obj.contains_key(gdpr),
                "GDPR routing anchor `{gdpr}` must match a frozen EventEnvelope field (so \
                 Search-as-a-holder erase + the RoPA fan-out reach the index, SRCH-P02/P15)"
            );
        }
    }

    /// The prompt's enumerated anchor set is exactly the named core + has no duplicates. Pins the
    /// list shape so a future edit that drops/dupes an anchor is loud.
    #[test]
    fn the_named_anchor_set_is_the_prompt_enumerated_core() {
        // doc_id / tenant / region / indexed_zookie / version / lang + the three always-present
        // GDPR routing fields = the nine the prompt's TESTS field names.
        assert_eq!(
            INDEX_DOC_ANCHORS.len(),
            9,
            "the named-anchor core is exactly nine"
        );
        let mut sorted = INDEX_DOC_ANCHORS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 9, "no index-doc anchor name is duplicated");

        // The six the prompt spells out by name are all present.
        for required in [DOC_ID, TENANT, REGION, INDEXED_ZOOKIE, VERSION, LANG] {
            assert!(
                INDEX_DOC_ANCHORS.contains(&required),
                "the prompt-named anchor `{required}` is missing from INDEX_DOC_ANCHORS"
            );
        }
    }
}
// NOTE on test placement (EI-01 §7, self-consistency with the gate it ships): the Search-owned
// red+green lint-confirmation proof lives in the INTEGRATION test `tests/lint_confirmation.rs`,
// NOT in this `#[cfg(test)] mod tests`. The red sample it runs the lint over is a verbatim
// bypass-fingerprint (`index.search(query)` with no composed filter) — and the live workspace
// lint scan (`myelin-lints/tests/workspace_clean.rs`, the `lint-gate` binary) scans `crates/*/src`
// but EXCLUDES `crates/*/tests/**`. Keeping the bypass sample under `tests/` means the very gate
// this prompt confirms stays GREEN over Search's own source, while the proof still runs (the
// shared lints crate's own red samples live in `tests/fixtures/` for the identical reason).

//! # `ci_log_projection` — CI log search: the per-subject-DEK sealed segments + the
//! `(job, step, byte-range)` index (`details_ref` resolves) (SRCH-P22 / P-340, M4)
//!
//! **Owning architecture doc:** `search-and-indexing.md` change #11 (CI logs ride the firehose for
//! LIVE TAIL but Search consumes the DURABLE sealed segments, NOT the firehose — *no one wires Search
//! onto the firehose*; Search reads the durable bus `evt.*`, not the firehose live tier), change #9
//! (the per-subject source DEK incl. CI log segments — the crypto-shred backstop). **Contracts:** 11.8
//! (the T3 CI log tier: the per-subject-DEK sealed log segments + the `(job, step, byte-range)` index
//! that maps `(job, step, byte-range) → (segment-blob, offset)`), 5.9 (the X-1
//! `CheckStatus.details_ref` `#step-<n>` sub-anchor the index resolves to the failing step's bytes).
//!
//! ## What SRCH-P22 ships here — the CI-log slice of the 11.8 corpus (the engine is UNCHANGED)
//!
//! The CI/Storage producer (`myelin_storage::ci_log_index::CiLogTier`, P-328/P-329/P-332) already owns
//! the AUTHORITATIVE durable structure: it seals CI log frames into content-addressed, per-tenant-DEK
//! (and per-subject-DEK, C1) T2 segments AND builds the `(job, step, byte-range)` index, and resolves
//! a `myelin://<tenant>/ci/run/<run>#step-<n>` anchor to the EXACT failing step's bytes. SRCH-P22 is
//! the **consumer side**: Search models the searchable CI-log doc + ships the index-time **projection
//! BUILDER** ([`ci_log_search_projection`]) that turns a sealed segment's `(run, job, step)` keys +
//! the redacted log text into the [`SearchProjection`] Search indexes, so a CI-log query (and the
//! `#step-<n>` jump-to-failure) is searchable end-to-end. The body Search indexes is the durable
//! sealed segment's text — Search resolves it through the owner's `project(ref, viewer)` (5.6), which
//! reads the `(job, step, byte-range)` index + the sealed segment, NEVER the firehose (change #11).
//!
//! ## Coherence (EI-01 §7) — Search models the doc, it does NOT re-build the producer
//! - **The sealing + per-subject/per-tenant DEK + the `(job, step, byte-range)` index + the
//!   `#step-<n>` resolution are the producer's** (`myelin_storage::ci_log_index::CiLogTier`). Search
//!   does NOT re-seal, re-key, or re-index the bytes — it CONSUMES the durable sealed segment's
//!   projection. `myelin-storage` is a mid-tier crate that Search already depends on (the KMS/holder
//!   edge); Search does NOT re-implement the tier here. The [`ci_log_details_ref`] /
//!   [`parse_step_anchor`] helpers Search ships are the QUERY-SIDE `details_ref` shape (the
//!   `#step-<n>` → `(run, step)` facets a CI-log search resolves on), modelled byte-identically to the
//!   producer's `StepAnchor::parse` (5.9 / OQ-D) — Search cannot import the producer's resolver across
//!   the `serve` boundary at index time, so the doc carries the `(run, job, step)` as typed facets and
//!   the `#step-<n>` anchor resolves to them. The frozen anchor SHAPE is the one vocabulary.
//! - **The IndexSpec is the ONE frozen Search-owned shape** ([`ci_log_index_spec`]). It is the SAME
//!   posture [`crate::git_code_projection`] (Git `blob`→`repo`) and [`crate::issues_projection`]
//!   (Issues) take for their producer corpora — a structured facet half (the `(job, step)` index keys)
//!   + a full-text body (the redacted log text), `acl_object_type` pinned on the parent CI **run**.
//!
//! ## The ACL object — a CI log's reachability is its parent CI RUN's (the blob→repo analog)
//! A CI log segment has NO per-segment ACL object: its reachability is decided by the parent CI **run**
//! (the `ci_run` ReBAC object — a viewer who can `view` the run can read its logs), EXACTLY as a git
//! `blob`'s reachability is its parent `repo`'s (`acl_object_type = "repo"`). So
//! [`ci_log_index_spec`]'s `acl_object_type = "ci_run"` while `type_ = "ci_log"`. This is what makes
//! the SRCH-D1 fork-scoped-CI-log drill work: a CI log of a run a viewer cannot `view` (e.g. an
//! untrusted-fork run the viewer is not a member of) is NEVER in any result incl. counts — the ACL
//! pre-filter conjoins on the parent run, byte-identically to every other corpus.
//!
//! ## Floors named (SRCH-P22 DoD)
//! - **The firehose is explicitly NOT wired to Search (change #11).** Search consumes the DURABLE
//!   sealed segments (the `(job, step, byte-range)` index + the content-addressed T2 segment the
//!   producer sealed), NOT the firehose live tier. The firehose is the runner's LIVE-TAIL transport;
//!   Search reads the durable bus (`evt.*`) as an excepted infra consumer. Recorded so the
//!   durable-segment consumption is NOT mistaken for a firehose tap. Greppable as
//!   [`CiLogDurableSegmentNotFirehoseFloor`].
//! - **The per-subject CI-log DEK (C1) is the producer's crypto-shred backstop** (P-329/P-332,
//!   `myelin_storage::ci_log_index::CiLogTier::seal_ci_batch_for_subject`): a subject's Art. 17 erasure
//!   crypto-shreds exactly their isolable CI-log content. Search's PRIMARY per-subject erasure stays
//!   purge + re-index (change #9, [`crate::erase`]); the per-subject source DEK is the additional
//!   backstop. Search consumes the sealed segment's projection; it does not own the DEK. Named as the
//!   producer-owned backstop, not a Search floor.
//! - **The real CI-log projection EMITTER** (the live `project(ref, viewer)` that resolves the
//!   `(job, step, byte-range)` index + reads the sealed segment + emits the per-`(job, step)` doc
//!   projection through the outbox) is the CI producer's M4 emitter prompt. Here Search ships the SPEC
//!   model + the projection BUILDER the emitter feeds; the integration test drives the genuine builder
//!   over a real CI-log corpus (the sealed-segment text resolved by `(run, job, step)`).
//! - **No new mutation-core module** — the SRCH-P09 mutation floor (the SetExpr ACL conjoin decision
//!   logic) still holds on the CI-log corpus; this slice is producer-corpus WIRING, the engine
//!   decision logic is unchanged. The producer's OWN mutation-core (the `(job, step, byte-range)` index
//!   resolution byte-exactness) is pinned at 100% in `myelin_storage::ci_log_index` (P-328).

use std::collections::BTreeMap;

use myelin_query::{FieldType, FieldValue};

use crate::analysis::{Analyzer, Language};
use crate::indexer::{IndexSpec, SearchProjection};

/// The subsystem token CI declares its log projection under (`ci`) — the producer's CI subsystem
/// namespace (the `myelin://<tenant>/ci/...` artifact authority). Search models it here because the CI
/// producer depends on the platform consumer crates, never the reverse (the [`crate::kn_projection`]
/// posture — the shape, not a second contract).
pub const CI_SUBSYSTEM: &str = "ci";

/// The artifact type CI's log projection indexes — a `ci_log`: ONE searchable doc per `(run, job,
/// step)` sealed-segment log (the reconstructed step log the `(job, step, byte-range)` index resolves).
/// The canonical doc ref is `myelin://<tenant>/ci/ci_log/<run>:<job>:<step>` (the `(job, step)` index
/// key, NOT a raw byte offset — the offset lives in the producer's index, §3.1).
pub const CI_LOG_TYPE: &str = "ci_log";

/// The ACL object type a CI-log doc's reachability filter pins on — the parent **`ci_run`** (there is
/// NO per-log ACL; the CI run decides reachability — a viewer who can `view` the run reads its logs,
/// EXACTLY as a git blob's reachability is its parent `repo`'s). This is what makes the SRCH-D1
/// fork-scoped-CI-log drill hold: a log of a run the viewer cannot `view` is never in any result.
pub const CI_LOG_ACL_OBJECT_TYPE: &str = "ci_run";

/// The structured-facet key for the parent CI run (the `(job, step, byte-range)` index is per-run; the
/// `#step-<n>` anchor resolves against it). A [`FieldType::Relation`] ref facet (the run's artifact
/// ref) — the run-scoped filter + the ACL anchor.
pub const FACET_RUN_ID: &str = "run_id";
/// The structured-facet key for the CI job (the `(job, step, byte-range)` index's first key) — a
/// [`FieldType::Text`] opaque job id, exact-match (a per-job log filter).
pub const FACET_JOB_ID: &str = "job_id";
/// The structured-facet key for the step within the job (the index's second key; 1-based, matching the
/// X-1 `#step-<n>` anchor) — an ordered [`FieldType::Int`] (so `step >= n` range filters work and the
/// `#step-<n>` jump-to-failure resolves to the exact step).
pub const FACET_STEP_NO: &str = "step_no";

/// **CI's `declare_indexable` CI-log IndexSpec (contract 11.8 — the Search-side consumed model).**
/// `subsystem = "ci"`, `type = "ci_log"`, the three structured `(job, step, byte-range)`-index facets
/// (`run_id`/`job_id`/`step_no` — the keys the `#step-<n>` `details_ref` resolves through),
/// **non-semantic** (CI logs are literal/symbol/path-grade full-text like code, NOT vector-embedded in
/// v1), `acl_object_type = "ci_run"` (the parent run decides reachability — the blob→repo analog).
///
/// The full-text body (the redacted log text of the reconstructed `(job, step)` step log, read from
/// the DURABLE sealed segment — NOT the firehose, change #11) is NOT in the spec — it arrives at emit
/// time in the index-time [`SearchProjection::text`] ([`ci_log_search_projection`]). The spec is the
/// columnar schema; the projection is the row.
pub fn ci_log_index_spec() -> IndexSpec {
    let mut struct_fields: BTreeMap<String, FieldType> = BTreeMap::new();
    // The `(job, step, byte-range)` index keys, each at its frozen FieldType (13.3). The run is the
    // ACL anchor + per-run filter; the job/step are the index keys the `#step-<n>` anchor resolves to.
    struct_fields.insert(FACET_RUN_ID.to_string(), FieldType::Relation);
    struct_fields.insert(FACET_JOB_ID.to_string(), FieldType::Text);
    struct_fields.insert(FACET_STEP_NO.to_string(), FieldType::Int);

    // A CI log's reachability is the parent CI run's `view` (there is no per-log ACL object, UNLIKE an
    // issue whose ACL is its own object — like git's blob→repo). Made explicit so a future facet rename
    // can not silently drift the ACL anchor off the `ci_run` parent.
    IndexSpec::new(CI_SUBSYSTEM, CI_LOG_TYPE, struct_fields)
        .with_acl_object_type(CI_LOG_ACL_OBJECT_TYPE)
}

/// Every CI index spec (the one `ci_log` type) — the set a Search indexer registers to consume the
/// real CI-log corpus. Mirrors [`crate::issues_projection::issue_index_specs`].
pub fn ci_log_index_specs() -> Vec<IndexSpec> {
    vec![ci_log_index_spec()]
}

/// **Register CI's log index spec WITH Search (the GATE).** Builds [`ci_log_index_specs`] and proves
/// Search **accepts** it by admitting it into a live
/// [`IncrementalIndexer`](crate::indexer::IncrementalIndexer)'s per-tenant facet union without a schema
/// mismatch (the only honest definition of "accepted" — Search is the authority that admits). Returns
/// the specs that were accepted. Mirrors [`crate::issues_projection::register_issue_index_specs`].
pub fn register_ci_log_index_specs() -> Vec<IndexSpec> {
    let specs = ci_log_index_specs();
    // Admit them into a real indexer's facet union (the build-time declare_indexable surface). A
    // facet-type collision or a malformed shape would panic at construction; it does not.
    let _accepted = crate::indexer::IncrementalIndexer::new(
        specs.clone(),
        std::sync::Arc::new(NullProjectFetcher),
        std::sync::Arc::new(crate::indexer::MockEmbeddingAdapter::new(8)),
    );
    specs
}

/// A do-nothing [`ProjectFetcher`](crate::indexer::ProjectFetcher) used ONLY to admit the CI-log specs
/// into a live indexer for the registration GATE (the SPEC half + the projection BUILDER ship here; the
/// real owner-`project` fetch — which resolves the durable `(job, step, byte-range)` index + sealed
/// segment — is the CI producer's emitter). It never fetches — registration does not index. Mirrors
/// Issues' / git's `NullProjectFetcher`.
struct NullProjectFetcher;

impl crate::indexer::ProjectFetcher for NullProjectFetcher {
    fn project(
        &self,
        _tenant: &myelin_tenancy::TenantId,
        _region: &myelin_tenancy::Region,
        _ref_: &myelin_tenancy::ArtifactRef,
    ) -> Result<SearchProjection, crate::indexer::ProjectFetchError> {
        // The SPEC registration never fetches a projection (no emitter here). This is the registration
        // GATE — Search admits the schema — not the index path.
        Err(crate::indexer::ProjectFetchError::Gone)
    }
}

/// **The index-time inputs of a CI-log doc's [`SearchProjection`] (the owner's `project(ref, viewer)`
/// body Search consumes, contract 5.6).** In production the CI producer builds these by resolving the
/// `(job, step, byte-range)` index + reading the DURABLE sealed segment (NOT the firehose, change #11)
/// for the reconstructed `(run, job, step)` step log. Here the builder takes them directly — the same
/// shape the live store swaps in behind ([`ci_log_search_projection`] is the projection BUILDER the
/// emitter feeds).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CiLogProjectionInput {
    /// The parent CI run's artifact ref (`myelin://<tenant>/ci/run/<run>`) — the [`FACET_RUN_ID`]
    /// `Relation` facet AND the ACL anchor (the run decides reachability).
    pub run_id: String,
    /// The CI job id (the `(job, step)` index's first key) — the [`FACET_JOB_ID`] `Text` facet.
    pub job_id: String,
    /// The step within the job (1-based, matching the `#step-<n>` anchor) — the [`FACET_STEP_NO`]
    /// ordered `Int` facet.
    pub step_no: u32,
    /// The redacted log TEXT of the reconstructed `(job, step)` step log, read from the DURABLE sealed
    /// segment (the `(job, step, byte-range)` index resolved it; change #11 — NOT the firehose). This
    /// is the full-text body Search indexes so a log-content query hits the right step.
    pub log_text: String,
    /// The analyzer-selection language tag (§3.1). `None` lets the indexer detect; CI logs default to
    /// the CODE chain (literal/symbol/path-grade — a stack trace / an assertion message / a path is
    /// tokenized like code, not stemmed as prose).
    pub lang: Option<String>,
}

/// **Build a CI-log doc's [`SearchProjection`] from its index-time inputs (§4.1).** This is the owner's
/// `project(ref, viewer)` body Search consumes (contract 5.6) — NOT a firehose read and NOT a
/// re-sealing of the segment. It produces:
/// - the full-text `log_text` body (the redacted reconstructed step log from the DURABLE sealed
///   segment — change #11), tokenized under the CODE chain (so a stack-trace symbol / an assertion
///   literal / a path in the log is searchable, parity with code search);
/// - the three typed `(job, step, byte-range)`-index facets (`run_id`/`job_id`/`step_no`) so a
///   per-run / per-job / per-step filter works AND the `#step-<n>` `details_ref` resolves to the
///   exact step ([`ci_log_details_ref`] / [`parse_step_anchor`]).
///
/// The body is PRE-TOKENIZED into the space-separated `text` (the same posture
/// [`crate::git_code_projection::git_blob_search_projection`] takes — the engine's default tokenizer
/// re-splits it, so the engine stays UNCHANGED). A CI log doc is **non-semantic** in v1 (no vector).
pub fn ci_log_search_projection(input: &CiLogProjectionInput) -> SearchProjection {
    // CI logs are literal/symbol/path-grade (a stack trace, an assertion message, a file path) — the
    // CODE chain (camel/snake split keeping operators) is parity-correct, NOT a prose stemmer.
    let code = Analyzer::for_language(Language::Code);
    let text = code.analyze(&input.log_text).join(" ");

    let mut fields: BTreeMap<String, FieldValue> = BTreeMap::new();
    if !input.run_id.is_empty() {
        fields.insert(
            FACET_RUN_ID.to_string(),
            FieldValue::Relation(input.run_id.clone()),
        );
    }
    if !input.job_id.is_empty() {
        fields.insert(
            FACET_JOB_ID.to_string(),
            FieldValue::Text(input.job_id.clone()),
        );
    }
    // The step is always a valid 1-based facet (the index key); stamp it as the ordered Int facet so a
    // `#step-<n>` resolve + a `step >= n` range filter both work.
    fields.insert(
        FACET_STEP_NO.to_string(),
        FieldValue::Int(i64::from(input.step_no)),
    );

    SearchProjection {
        text,
        fields,
        // CI logs default to the CODE chain (literal/symbol/path-grade); the producer may pin a lang.
        lang: input
            .lang
            .clone()
            .or_else(|| Some(Language::Code.tag().to_string())),
    }
}

/// **The canonical CI-log doc ref for a `(run, job, step)` (the `(job, step)` index key as a doc id).**
/// `myelin://<tenant>/ci/ci_log/<run>:<job>:<step>` — the `(job, step)` index key, NOT a raw byte
/// offset (the offset lives in the producer's `(job, step, byte-range)` index, §3.1). This is the
/// Search `doc_id` for the reconstructed step log.
pub fn ci_log_doc_ref(tenant: &str, run_id: &str, job_id: &str, step_no: u32) -> String {
    format!("myelin://{tenant}/ci/{CI_LOG_TYPE}/{run_id}:{job_id}:{step_no}")
}

/// **The X-1 `CheckStatus.details_ref` `#step-<n>` sub-anchor for a `(run, step)` (contract 5.9 /
/// OQ-D).** `myelin://<tenant>/ci/run/<run>#step-<n>` — the jump-to-failure ref a `CheckStatus`
/// carries. Search resolves it to the matching CI-log doc's `(run_id, step_no)` facets
/// ([`parse_step_anchor`]). Modelled byte-identically to the producer's
/// `myelin_storage::ci_log_index::StepAnchor` shape (the ONE frozen anchor vocabulary — Search does
/// NOT invent a second `#step-<n>` grammar).
pub fn ci_log_details_ref(tenant: &str, run_id: &str, step_no: u32) -> String {
    format!("myelin://{tenant}/ci/run/{run_id}#step-{step_no}")
}

/// **A parsed `#step-<n>` `details_ref` (the query-side resolution target, 5.9 / OQ-D).** The `run_id`
/// from the `ci/run/<run>` path + the `step_no` from the `#step-<n>` sub-anchor — the
/// `(run, step)` facets a CI-log search filters on to resolve the failing step's doc.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiLogStepAnchor {
    /// The run id from the `ci/run/<run>` path (the [`FACET_RUN_ID`] this resolves on).
    pub run_id: String,
    /// The step number from the `#step-<n>` sub-anchor (the [`FACET_STEP_NO`] this resolves on).
    pub step_no: u32,
}

/// **Parse a `#step-<n>` `details_ref` → `(run_id, step_no)` (the query-side resolution, 5.9 / OQ-D).**
/// `None` on ANY malformation (a wrong-shaped ref is LOUD, never silently resolved to step 0) — a
/// missing `#step-`, a non-numeric step, a non-`ci/run` path, or an empty run id. Byte-identical in
/// shape to the producer's `myelin_storage::ci_log_index::StepAnchor::parse` (the ONE frozen anchor
/// grammar; Search models the same shape on the query side because it cannot import the producer's
/// resolver across the `serve` boundary at index time). Tolerant of an absent `myelin://` scheme.
pub fn parse_step_anchor(anchor: &str) -> Option<CiLogStepAnchor> {
    let (path, frag) = anchor.split_once('#')?;
    let step_no: u32 = frag.strip_prefix("step-")?.parse().ok()?;
    let run_id = path
        .split("ci/run/")
        .nth(1)
        .filter(|r| !r.is_empty())
        .map(|r| r.trim_end_matches('/').to_string())?;
    if run_id.is_empty() {
        return None;
    }
    Some(CiLogStepAnchor { run_id, step_no })
}

/// **FLOOR (named) — the firehose is explicitly NOT wired to Search (change #11).** A greppable
/// zero-sized marker: Search consumes the DURABLE sealed CI-log segments (the producer's
/// `(job, step, byte-range)` index + the content-addressed T2 segment), NOT the firehose live tier.
/// The firehose is the runner's LIVE-TAIL transport; Search reads the durable bus (`evt.*`) as an
/// excepted infra consumer. Recorded so the durable-segment consumption is NOT mistaken for a firehose
/// tap. The per-subject CI-log DEK (C1) crypto-shred is the PRODUCER's backstop (P-329/P-332); Search's
/// primary per-subject erasure stays purge + re-index (change #9).
#[derive(Clone, Copy, Debug)]
pub struct CiLogDurableSegmentNotFirehoseFloor;

impl CiLogDurableSegmentNotFirehoseFloor {
    /// The durable transport Search consumes (the bus, NOT the firehose live tier — change #11).
    pub const DURABLE_TRANSPORT: &'static str = "evt.* (durable bus)";
    /// The transport Search is explicitly NOT wired onto (change #11 — the runner's live-tail tier).
    pub const NOT_THE_FIREHOSE: &'static str = "firehose live tier";
    /// The producer that owns the per-subject CI-log DEK crypto-shred backstop (C1).
    pub const PER_SUBJECT_DEK_OWNER: &'static str = "myelin_storage::ci_log_index::CiLogTier";
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::IncrementalIndexer;

    /// **The CI-log spec is the consumed 11.8 shape.** Pins every facet + type + the acl_object_type. A
    /// rename of a Search `IndexSpec` field, or a drift in the facet set/types, breaks the registrant.
    #[test]
    fn ci_log_spec_is_the_consumed_11_8_shape() {
        let s = ci_log_index_spec();
        assert_eq!(s.subsystem, "ci");
        assert_eq!(s.type_, "ci_log");
        assert_eq!(
            s.acl_object_type, "ci_run",
            "a CI log's reachability is its parent CI run's `view` (the blob→repo analog)"
        );
        assert!(
            !s.semantic,
            "CI logs are literal/symbol/path-grade full-text, not vector-embedded in v1"
        );
        // The three structured `(job, step, byte-range)`-index facets, each at its frozen FieldType.
        assert_eq!(
            s.struct_fields.len(),
            3,
            "exactly the three (job, step, byte-range)-index facets"
        );
        assert_eq!(
            s.struct_fields.get(FACET_RUN_ID),
            Some(&FieldType::Relation)
        );
        assert_eq!(s.struct_fields.get(FACET_JOB_ID), Some(&FieldType::Text));
        assert_eq!(s.struct_fields.get(FACET_STEP_NO), Some(&FieldType::Int));
    }

    /// **The acl_object_type is the parent run, not the log itself (the blob→repo analog).** This is
    /// what makes the SRCH-D1 fork-scoped-CI-log drill hold — pinned so a future edit can not drift the
    /// ACL anchor off the parent `ci_run`.
    #[test]
    fn acl_object_is_the_parent_run_not_the_log() {
        let s = ci_log_index_spec();
        assert_eq!(s.acl_object_type, "ci_run");
        assert_ne!(
            s.acl_object_type, s.type_,
            "the ACL anchor is the parent run, NOT the per-log doc (no per-log ACL object)"
        );
    }

    /// **The full-text log body is NOT a structured facet.** The redacted log text arrives at emit time
    /// in `SearchProjection.text`, so it must be absent from `struct_fields` (the schema is the columnar
    /// `(job, step)` index half, not the body).
    #[test]
    fn log_text_is_not_a_struct_facet() {
        let s = ci_log_index_spec();
        for absent in ["log", "log_text", "text", "body", "bytes", "segment"] {
            assert!(
                !s.struct_fields.contains_key(absent),
                "`{absent}` is full-text projection body, not a structured facet"
            );
        }
    }

    /// **Search ACCEPTS the CI-log spec (the GATE).** Search admits it into a live indexer's per-tenant
    /// facet union without a schema mismatch — the accepted set is byte-equal to the declared set.
    #[test]
    fn registration_is_accepted_by_search() {
        let accepted = register_ci_log_index_specs();
        assert_eq!(
            accepted,
            ci_log_index_specs(),
            "Search accepts the declared CI-log spec verbatim"
        );
        let _ix = IncrementalIndexer::new(
            ci_log_index_specs(),
            std::sync::Arc::new(NullProjectFetcher),
            std::sync::Arc::new(crate::indexer::MockEmbeddingAdapter::new(8)),
        );
    }

    /// **The projection builds the typed `(job, step, byte-range)` facets + the searchable log body.**
    /// Every facet is typed to match `ci_log_index_spec`'s declaration; the body is the code-tokenized
    /// log text (a symbol / a literal / a path in the log is searchable).
    #[test]
    fn projection_builds_typed_facets_and_log_body() {
        let input = CiLogProjectionInput {
            run_id: "myelin://acme/ci/run/run-7".into(),
            job_id: "build".into(),
            step_no: 3,
            log_text: "FAIL: assertion at src/scheduler/deadlock.rs:42 detectDeadlock".into(),
            lang: None,
        };
        let p = ci_log_search_projection(&input);

        // The body is code-tokenized: a path segment, an identifier camel-split, the literal `FAIL`.
        let toks: std::collections::BTreeSet<&str> = p.text.split(' ').collect();
        assert!(
            toks.contains("scheduler"),
            "a path segment is searchable: {:?}",
            &p.text
        );
        assert!(toks.contains("deadlock"));
        assert!(
            toks.contains("detect"),
            "the camel-split identifier is searchable"
        );
        assert_eq!(
            p.lang.as_deref(),
            Some("code"),
            "CI logs default to the code chain"
        );

        // The three typed `(job, step, byte-range)`-index facets.
        assert_eq!(
            p.fields.get(FACET_RUN_ID),
            Some(&FieldValue::Relation("myelin://acme/ci/run/run-7".into()))
        );
        assert_eq!(
            p.fields.get(FACET_JOB_ID),
            Some(&FieldValue::Text("build".into()))
        );
        assert_eq!(p.fields.get(FACET_STEP_NO), Some(&FieldValue::Int(3)));

        // Every stamped facet's type matches the spec (the indexer would reject a mismatch).
        let spec = ci_log_index_spec();
        for (name, value) in &p.fields {
            assert_eq!(
                value.field_type(),
                *spec.struct_fields.get(name).expect("facet is declared"),
                "facet `{name}` value type matches its spec declaration"
            );
        }
    }

    /// **The step is always stamped (the index key), even for an empty/absent run/job.** The step facet
    /// is the load-bearing `#step-<n>` resolution key — it is never optional.
    #[test]
    fn step_facet_is_always_stamped() {
        let p = ci_log_search_projection(&CiLogProjectionInput {
            run_id: String::new(),
            job_id: String::new(),
            step_no: 5,
            log_text: String::new(),
            lang: None,
        });
        assert_eq!(
            p.fields.get(FACET_STEP_NO),
            Some(&FieldValue::Int(5)),
            "the step is always the index key, even with no run/job/text"
        );
        assert!(
            !p.fields.contains_key(FACET_RUN_ID),
            "an absent run is not stamped as empty"
        );
        assert!(
            !p.fields.contains_key(FACET_JOB_ID),
            "an absent job is not stamped as empty"
        );
    }

    /// **The doc ref is the `(run, job, step)` index key (not a byte offset).**
    #[test]
    fn doc_ref_is_the_run_job_step_key() {
        assert_eq!(
            ci_log_doc_ref("acme", "run-7", "build", 3),
            "myelin://acme/ci/ci_log/run-7:build:3"
        );
    }

    /// **The `#step-<n>` `details_ref` round-trips through the builder + parser (5.9 / OQ-D).** Search's
    /// query-side `details_ref` shape parses the X-1 anchor back into the `(run, step)` facets it
    /// resolves on — byte-identical in shape to the producer's `StepAnchor` (the ONE frozen grammar).
    #[test]
    fn details_ref_builds_and_parses_the_x1_anchor() {
        let anchor = ci_log_details_ref("acme", "run-42", 3);
        assert_eq!(anchor, "myelin://acme/ci/run/run-42#step-3");
        let parsed = parse_step_anchor(&anchor).expect("parse");
        assert_eq!(
            parsed,
            CiLogStepAnchor {
                run_id: "run-42".into(),
                step_no: 3
            }
        );
        // A bare (schemeless) anchor resolves identically (keys on ci/run/<run> + the sub-anchor).
        assert_eq!(parse_step_anchor("acme/ci/run/run-42#step-3"), Some(parsed));
    }

    /// **The `#step-<n>` parser rejects malformation LOUDLY (never a silent step-0).** A missing
    /// `#step-`, a non-numeric step, a non-`ci/run` path, or an empty run id all return `None`.
    #[test]
    fn step_anchor_parser_rejects_malformation() {
        assert_eq!(parse_step_anchor("myelin://acme/ci/run/run-1"), None); // no #step
        assert_eq!(parse_step_anchor("myelin://acme/ci/run/run-1#frag-1"), None); // wrong sub-anchor
        assert_eq!(parse_step_anchor("myelin://acme/ci/run/run-1#step-x"), None); // non-numeric
        assert_eq!(
            parse_step_anchor("myelin://acme/issue/issue/42#step-1"),
            None
        ); // not ci/run
        assert_eq!(parse_step_anchor("myelin://acme/ci/run/#step-1"), None); // empty run id
    }

    /// **The floor marker names the durable transport (NOT the firehose) + the per-subject DEK owner.**
    #[test]
    fn floor_marker_names_durable_not_firehose() {
        assert_eq!(
            CiLogDurableSegmentNotFirehoseFloor::DURABLE_TRANSPORT,
            "evt.* (durable bus)"
        );
        assert_eq!(
            CiLogDurableSegmentNotFirehoseFloor::NOT_THE_FIREHOSE,
            "firehose live tier"
        );
        assert_eq!(
            CiLogDurableSegmentNotFirehoseFloor::PER_SUBJECT_DEK_OWNER,
            "myelin_storage::ci_log_index::CiLogTier"
        );
    }
}

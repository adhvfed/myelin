//! # `surfacing_index` — the CI cross-fabric surfacing INDEX + REPLAY half (CI-P26 / P-369, M4)
//!
//! This is the **second half** of CI's cross-fabric surfacing (the first half — the leak-free
//! `list_objects` push-down + the `ArtifactRef`/`#sub` mints + `project(ref, viewer)` — is
//! [`crate::surfacing`], CI-P25 / P-368). Here CI ships:
//!
//! - **`declare_indexable` (contract 6.3, arch 03 §7.4).** The CI **`ci/run` `IndexSpec`** — CI
//!   declares WHAT a run projects into a Search index doc; Search owns the engine. `acl_object_type
//!   = ci_run` so Search pre-filters every run query via `list_objects(viewer, read, ci_run)` →
//!   `SetExpr` (the SAME OQ-E push-down [`crate::surfacing::lower_over_run_id`] lowers; the
//!   `search-requires-acl-filter` lint). The structured facets a release-readiness / triage query
//!   pins on, plus the **semantic `failure_summary`** field ("find the run where test X first
//!   failed", RAG/dedup). The restriction flag is honoured — a restricted subject's runs are
//!   EXCLUDED from the index ([`run_doc_is_indexable`]).
//! - **`replay(scope, since)` (contract 2.6, arch 03 §7.3) — CONFIRMED in place.** CI's per-owner
//!   reindex-from-source body is [`myelin_ci_sandbox::CiReindexSource`] (EB-27 / P-327): it re-emits
//!   `ci.run.snapshot` / `ci.deployment.snapshot` / `ci.pipeline.snapshot` through the outbox → the
//!   live consumer path, sub-artifact-granular (one-run scope), erased-skip (X-7). CI-P26 does NOT
//!   re-author it (one source of truth, EI-01 §7); it RE-EXPORTS it ([`CiReindexSource`] /
//!   [`CiReplayKind`]) so the surfacing surface is one import, and adds the **no-cross-db rebuild
//!   GATE** ([`rebuild_from_snapshots_without_ci_db`]) proving a `ci.run.snapshot` rebuilds the
//!   derived view WITHOUT reading CI's DB — replay is the only rebuild path.
//!
//! ## Reconciliation with the existing CI code (EI-01 §7 coherence — survey-first)
//! CI-P26's deliverable is FOUR halves; an EARLIER prompt already shipped two of them, so this
//! module EXTENDS / CONFIRMS, never duplicates:
//! - **replay (2.6)** was filled by EB-27 / P-327 ([`myelin_ci_sandbox::replay`]). RE-EXPORTED +
//!   GATE-proven here (no second replay body).
//! - **humanise (7.3)** was filled by NOTIF-P23 / P-344 ([`myelin_ci_sandbox::notif_rules`] — the
//!   `CheckStatus.summary` `HumanisedRef` templates + the `define_notif_rule` reason on the ONE
//!   humanise surface). RE-EXPORTED here ([`ci_summary`] / [`register_ci_summary_templates`] /
//!   [`summary_template_key`]) so the surfacing surface names it; no second template set.
//! - **`declare_indexable` (6.3)** is the GENUINELY-NEW `ci/run` `IndexSpec` shipped here (only the
//!   `ci_log` doc spec — `myelin_search::ci_log_index_spec`, type `ci_log`, the firehose-log doc —
//!   existed before; the **run** doc spec is new).
//! - **the `ToolDef` registrations (8.1)** are extended to the FULL frozen X-6 table in
//!   [`crate::surfacing_tools`] (the existing `myelin_agent_service::ci_tools` shipped only the four
//!   privileged/run-pipeline consumer defs; CI-P26 completes the read/run/cancel/retry/validate/plan/
//!   rollback rows so the frozen X-6 defaults are whole).
//!
//! ## FLOOR named (per the prompt)
//! The **SCIP/LSIF "find usages" code-search input** (contract 6.5) is a named follow-on (post-CI-M5,
//! arch 06 R-3): CI produces the artifact, Search consumes it later. It is NOT part of the run
//! `IndexSpec` here — the run failure-summary semantic field is the v1 semantic surface. The
//! run-projection EMITTER (the `ci.run.*` → `SearchProjection` push body, which REUSES
//! [`crate::surfacing::Projector::project`]) is the CI emit follow-on; this prompt registers the
//! SCHEMA (the spec), proven admitted by Search.
//!
//! ## Mutation-score floor (mandatory-core — EI-01 §3 / prove-it)
//! The index **restriction gate** [`run_doc_is_indexable`] is a leak surface (a restricted/erased
//! run that slipped into the index leaks via a search result/count/rank) — it is mandatory-core, and
//! the `declare_indexable_honours_restriction_zero_restricted_rows_indexed` test catches a mutation
//! of either `!`/`&&` arm. The 6.3 spec shape is pinned field-by-field
//! (`spec_is_cis_owned_6_3_run_shape` + the wire-shape serialization test), and the 2.6 no-cross-db
//! rebuild is proven against the live consumer path (`rebuild_from_snapshots_without_ci_db`). The
//! replay body itself ([`myelin_ci_sandbox::replay`]) carries its own mutation-tested suite (EB-27 /
//! P-327) — RE-EXPORTED here, not re-authored, so there is one mutation surface, not two.

use std::collections::BTreeMap;

use myelin_query::FieldType;
use myelin_search::{IncrementalIndexer, IndexSpec, MockEmbeddingAdapter, ProjectFetcher};

// Re-export CI's per-owner replay body (EB-27 / P-327) so the surfacing surface is ONE import — CI
// does NOT re-author replay (the `*.snapshot` re-emit is its single source of truth, EI-01 §7).
pub use myelin_ci_sandbox::replay::{CiReindexSource, CiReplayKind};
// Re-export CI's `CheckStatus.summary` HumanisedRef + humanise registration (NOTIF-P23 / P-344) so
// the surfacing surface names the humanise half too — CI does NOT re-author the template set.
pub use myelin_ci_sandbox::notif_rules::{
    ci_summary, register_ci_summary_templates, summary_template_key, CheckVerdict, CiSummary,
    CI_SUMMARY_TEMPLATES,
};

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 0. FROZEN NAMES for the ci/run IndexSpec (§7.4 / 6.3 — never a stray literal)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// The subsystem token CI declares its run projection under (`ci`, Bus §6.2). The SAME token
/// [`crate::events::CI_SUBSYSTEM_TOKEN`] / [`crate::surfacing::CI_SUBSYSTEM`] anchor.
pub const CI_SUBSYSTEM: &str = "ci";

/// The artifact type CI's run projection indexes — a `run` (the single-run searchable doc). The
/// canonical ref is `myelin://<tenant>/ci/run/<run_id>` ([`crate::surfacing::ci_run_ref`]).
pub const CI_RUN_TYPE: &str = "run";

/// **The ACL object type the run doc's reachability filter pins on — the `ci_run` object** (§7.4:
/// "Search pre-filters via `list_objects(viewer, read, ci_run)`"). The SAME ACL anchor the firehose
/// `ci_log` doc keys on ([`myelin_search::ci_log_projection`]) and the SAME column the §5.1 push-down
/// lowers over (`ci_run.run_id`), so a run doc and its log docs share ONE reachable-set filter (no
/// per-doc ACL drift). Note this is the SEARCH `acl_object_type` (`ci_run`), NOT the ReBAC relation
/// object token ([`crate::rebac_fragment::object_types::RUN`] = `run`) — Search keys on the column
/// name, the engine resolves it through the run fragment.
pub const CI_RUN_ACL_OBJECT_TYPE: &str = "ci_run";

// ── the structured (columnar/filterable) run facets a release-readiness / triage query pins on ──

/// The run lifecycle state facet (`passed`/`failed`/`running`/`queued`/…) — a `Select` option facet
/// (a release-readiness filter: "the failed runs on this branch").
pub const FACET_STATE: &str = "state";
/// The run trust tier facet (`trusted`/`untrusted_fork`/…) — a `Select` facet (the fork-trust filter).
pub const FACET_TRUST_TIER: &str = "trust_tier";
/// The deploy/target environment facet (`prod`/`staging`/…) — a `Select` facet.
pub const FACET_ENV: &str = "env";
/// The actor PSEUDONYM facet (the run's triggering principal pseudonym) — a `Principal` facet,
/// equality-only (NEVER the real identity; the pseudonym the index stores, EI-02).
pub const FACET_ACTOR_PSEUDONYM: &str = "actor_pseudonym";
/// The run creation timestamp facet — a `Date` facet (chronological range / "runs since yesterday").
pub const FACET_CREATED_AT: &str = "created_at";
/// The producing repo ref facet — a `Relation` facet (the `ArtifactRef` to the run's git repo; "runs
/// of this repo").
pub const FACET_REPO_REF: &str = "repo_ref";
/// The commit oid facet — a `Text` facet, exact-match ("the run for this commit").
pub const FACET_COMMIT_OID: &str = "commit_oid";

// ════════════════════════════════════════════════════════════════════════════════════════════════
// 1. declare_indexable — the ci/run IndexSpec (contract 6.3, §7.4)
// ════════════════════════════════════════════════════════════════════════════════════════════════

/// **Build CI's `declare_indexable` run-projection spec (contract 6.3, §7.4) — the CI-P26
/// deliverable.** The frozen Search-owned [`IndexSpec`] CI registers for the `ci/run` doc:
/// `subsystem = "ci"`, `type = "run"`, `acl_object_type = "ci_run"` (Search pre-filters every run
/// query via `list_objects(viewer, read, ci_run)` → the §5.1 push-down), the seven structured
/// facets (state / trust_tier / env / actor_pseudonym / created_at / repo_ref / commit_oid), and
/// **semantic** (the `failure_summary` field is vector-embedded so "find the run where test X first
/// failed" / failure-dedup RAG works — arch 03 §7.4).
///
/// The full-text projection body (`pipeline_name` / `branch` / `trigger_kind` /
/// `failed_test_name` / `log_excerpt_of_failure`, §7.4 `ft_fields`) + the `failure_summary` semantic
/// text are NOT in the spec — they arrive at emit time in the index-time
/// [`myelin_search::SearchProjection`] (`text` = the full-text/semantic body; the spec is the
/// schema, the projection is the row — GIT-P5 posture). So the spec carries the
/// structured/semantic/acl half; the projection body lands at emit time. The run-projection EMITTER
/// (the `ci.run.*` → `SearchProjection` push body) is the CI emit follow-on; this registers the
/// SCHEMA. The projection REUSES [`crate::surfacing::Projector::project`] (§7.2) — the projector is
/// the one cross-DB read, so the indexer never reads CI's DB.
pub fn ci_run_index_spec() -> IndexSpec {
    let mut struct_fields: BTreeMap<String, FieldType> = BTreeMap::new();
    // The structured/columnar facets a release-readiness / triage / search query filters on. The
    // full-text body (pipeline_name / branch / trigger_kind / failed_test_name / log_excerpt) +
    // the semantic failure_summary arrive via SearchProjection.text at emit time, NOT here.
    struct_fields.insert(FACET_STATE.to_string(), FieldType::Select);
    struct_fields.insert(FACET_TRUST_TIER.to_string(), FieldType::Select);
    struct_fields.insert(FACET_ENV.to_string(), FieldType::Select);
    struct_fields.insert(FACET_ACTOR_PSEUDONYM.to_string(), FieldType::Principal);
    struct_fields.insert(FACET_CREATED_AT.to_string(), FieldType::Date);
    struct_fields.insert(FACET_REPO_REF.to_string(), FieldType::Relation);
    struct_fields.insert(FACET_COMMIT_OID.to_string(), FieldType::Text);

    IndexSpec::new(CI_SUBSYSTEM, CI_RUN_TYPE, struct_fields)
        // The semantic failure-summary field is vector-embedded (RAG/dedup — "find the run where
        // test X first failed", §7.4). Unlike git code (trigram-only), a run's failure summary IS a
        // v1 semantic surface.
        .semantic()
        // A run's reachability is decided by the parent `ci_run` ACL object (the §5.1 push-down over
        // `ci_run.run_id`), NOT a per-doc ACL — the same anchor the firehose `ci_log` doc keys on.
        .with_acl_object_type(CI_RUN_ACL_OBJECT_TYPE)
}

/// **Register CI's run-projection spec WITH Search (the 6.3 GATE).** Builds [`ci_run_index_spec`]
/// and proves Search **accepts** it by admitting it into a live
/// [`IncrementalIndexer`](myelin_search::IncrementalIndexer)'s per-tenant facet union — the only
/// honest definition of "accepted" (Search is the authority that admits; CI does not assert
/// acceptance). Returns the accepted spec so a caller can assert the registered shape. If the facet
/// types collided or the shape were malformed, the indexer constructor would panic; it does not.
pub fn register_ci_run_index_spec() -> IndexSpec {
    let spec = ci_run_index_spec();
    let _accepted = IncrementalIndexer::new(
        vec![spec.clone()],
        std::sync::Arc::new(NullProjectFetcher),
        std::sync::Arc::new(MockEmbeddingAdapter::new(8)),
    );
    spec
}

/// A do-nothing [`ProjectFetcher`] used ONLY to admit the spec into a live indexer for the
/// registration GATE (the SPEC half ships here; the real owner-`project` fetch REUSES
/// [`crate::surfacing::Projector::project`] at the CI emit follow-on). It never fetches —
/// registration does not index.
struct NullProjectFetcher;

impl ProjectFetcher for NullProjectFetcher {
    fn project(
        &self,
        _tenant: &myelin_tenancy::TenantId,
        _region: &myelin_tenancy::Region,
        _ref_: &myelin_tenancy::ArtifactRef,
    ) -> Result<myelin_search::SearchProjection, myelin_search::ProjectFetchError> {
        // The SPEC registration never fetches a projection (no emitter here). A run that is never
        // asked-for projects to nothing; this is the registration GATE, not the index path.
        Err(myelin_search::ProjectFetchError::Gone)
    }
}

/// **The restriction-honouring index admission gate (§7.4 — "a restricted subject's runs are
/// excluded from the index").** Returns `true` iff a run doc MAY be indexed: a run whose subject is
/// RESTRICTED (the GDPR `restrict` flag) or whose run is ERASED is EXCLUDED (the index never carries
/// a restricted/erased run — the restriction-safe / erasure-safe property, mirrored from
/// [`crate::surfacing::Projector::project`]'s tombstone guards). The indexer calls this before
/// admitting a `ci.run.*` doc; a `false` means "delete/skip the doc", never "index it anyway".
///
/// This is the index-time twin of the projection-time tombstone: a restricted subject tombstones in
/// `project` (no leak to a viewer) AND is excluded from the index (no leak via a search result/count
/// /rank). Both honour the one restriction flag.
pub fn run_doc_is_indexable(restricted: bool, erased: bool) -> bool {
    !restricted && !erased
}

#[cfg(test)]
#[path = "surfacing_index_tests.rs"]
mod tests;
